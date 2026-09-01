use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, B256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::process::Command;

use crate::anvil::AnvilBackend;
use crate::base_backend::{BaseBackendError, SnapshotId, TransactionReceiptFacts};
use crate::fork_seeder::{
    encode_set_withdrawals_enabled, read_contract_state, ForkContractState, ForkSeedError,
};

pub const HERMETIC_ENVIRONMENT_ID: &str = "hermetic_current_artifacts";
const ERC1967_IMPLEMENTATION_SLOT: B256 = B256::new([
    0x36, 0x08, 0x94, 0xa1, 0x3b, 0xa1, 0xa3, 0x21, 0x06, 0x67, 0xc8, 0x28, 0x49, 0x2d, 0xb9, 0x8d,
    0xca, 0x3e, 0x20, 0x76, 0xcc, 0x37, 0x35, 0xa9, 0x20, 0xa3, 0xca, 0x50, 0x5d, 0x38, 0x2b, 0xbc,
]);

#[derive(Debug, Clone)]
pub struct HermeticDeployConfig {
    pub contracts_dir: PathBuf,
    pub deployment_dir: PathBuf,
    pub message_inbox_artifact: PathBuf,
    pub nock_artifact: PathBuf,
    pub proxy_artifact: PathBuf,
    pub forge_binary: PathBuf,
    pub deterministic_signers: [Address; 5],
    pub nock_name: String,
    pub nock_symbol: String,
}

impl HermeticDeployConfig {
    pub fn discover(workspace_root: &Path, deterministic_signers: [Address; 5]) -> Self {
        let contracts_dir = workspace_root.join("crates/bridge/contracts");
        let deployment_dir = contracts_dir.join("deployments");
        Self {
            deployment_dir,
            message_inbox_artifact: contracts_dir.join("out/MessageInbox.sol/MessageInbox.json"),
            nock_artifact: contracts_dir.join("out/Nock.sol/Nock.json"),
            proxy_artifact: contracts_dir.join("out/ERC1967Proxy.sol/ERC1967Proxy.json"),
            contracts_dir,
            forge_binary: PathBuf::from("forge"),
            deterministic_signers,
            nock_name: "Nock".to_owned(),
            nock_symbol: "NOCK".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HermeticAddresses {
    pub message_inbox_proxy: Address,
    pub message_inbox_implementation: Address,
    pub nock: Address,
    pub deployer: Address,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalArtifactRuntimeFacts {
    pub contract_name: String,
    pub artifact_path: PathBuf,
    pub artifact_sha256: String,
    pub runtime_keccak256: B256,
    pub normalized_runtime_keccak256: B256,
    pub immutable_reference_count: usize,
    pub runtime_matches_artifact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentReceiptRecord {
    pub role: String,
    pub receipt: TransactionReceiptFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentFacts {
    pub environment_id: String,
    pub block_number: u64,
    pub block_hash: B256,
    pub addresses: HermeticAddresses,
    pub bridge_state: ForkContractState,
    pub runtime_artifacts: Vec<LocalArtifactRuntimeFacts>,
    pub receipts: Vec<DeploymentReceiptRecord>,
}

pub struct HermeticDeployment {
    facts: DeploymentFacts,
    baseline_snapshot: SnapshotId,
    artifacts: LoadedArtifacts,
}

impl HermeticDeployment {
    pub async fn deploy(
        backend: &AnvilBackend,
        config: HermeticDeployConfig,
    ) -> Result<Self, HermeticDeployError> {
        if backend.facts().mode != "empty" {
            return Err(HermeticDeployError::WrongBackendMode);
        }
        validate_signers(&config.deterministic_signers)?;
        let artifacts = LoadedArtifacts::load(&config)?;
        let before_block = backend.block_number().await?;
        let predeploy_snapshot = backend.snapshot().await?;
        let deploy_result = deploy_inner(backend, &config, &artifacts, before_block).await;
        match deploy_result {
            Ok(facts) => {
                let baseline_snapshot = backend.snapshot().await?;
                Ok(Self {
                    facts,
                    baseline_snapshot,
                    artifacts,
                })
            }
            Err(error) => {
                let reverted = backend.revert(&predeploy_snapshot).await.unwrap_or(false);
                Err(HermeticDeployError::RolledBack {
                    reason: error.to_string(),
                    reverted,
                })
            }
        }
    }

    pub fn facts(&self) -> &DeploymentFacts {
        &self.facts
    }

    pub async fn reset(
        &mut self,
        backend: &AnvilBackend,
    ) -> Result<DeploymentFacts, HermeticDeployError> {
        if !backend.revert(&self.baseline_snapshot).await? {
            return Err(HermeticDeployError::SnapshotUnavailable);
        }
        let observed = observe_deployment(
            backend,
            &self.facts.addresses,
            &self.artifacts,
            self.facts.receipts.clone(),
        )
        .await?;
        if observed != self.facts {
            return Err(HermeticDeployError::ResetMismatch {
                expected: Box::new(self.facts.clone()),
                observed: Box::new(observed),
            });
        }
        self.baseline_snapshot = backend.snapshot().await?;
        Ok(self.facts.clone())
    }
}

async fn deploy_inner(
    backend: &AnvilBackend,
    config: &HermeticDeployConfig,
    artifacts: &LoadedArtifacts,
    before_block: u64,
) -> Result<DeploymentFacts, HermeticDeployError> {
    let deployer = backend
        .backend()
        .accounts()
        .await?
        .into_iter()
        .next()
        .ok_or(HermeticDeployError::NoAnvilAccount)?;
    let allowed_deployment_dir = config.contracts_dir.join("deployments");
    fs::create_dir_all(&allowed_deployment_dir).map_err(HermeticDeployError::Filesystem)?;
    fs::create_dir_all(&config.deployment_dir).map_err(HermeticDeployError::Filesystem)?;
    let deployment_file_name = format!(
        "hermetic-{}-{}.json",
        std::process::id(),
        backend.facts().port
    );
    let forge_deployment_path = allowed_deployment_dir.join(&deployment_file_name);
    let deployment_path = config.deployment_dir.join(deployment_file_name);
    let mut command = Command::new(&config.forge_binary);
    command
        .current_dir(&config.contracts_dir)
        .args([
            "script",
            "forge/Deploy.s.sol:Deploy",
            "--rpc-url",
            backend.http_url().as_url().as_str(),
            "--broadcast",
            "--unlocked",
            "--sender",
            &format!("{deployer:#x}"),
            "--non-interactive",
            "--quiet",
        ])
        .env("DEPLOYMENTS_PATH", &forge_deployment_path)
        .env("DEPLOY_TARGET_NETWORK", HERMETIC_ENVIRONMENT_ID)
        .env("DEPLOYER_ADDRESS", format!("{deployer:#x}"))
        .env("NOCK_NAME", &config.nock_name)
        .env("NOCK_SYMBOL", &config.nock_symbol)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (index, signer) in config.deterministic_signers.iter().enumerate() {
        command.env(format!("BRIDGE_NODE_{index}"), format!("{signer:#x}"));
    }
    let output = command
        .output()
        .await
        .map_err(HermeticDeployError::ForgeLaunch)?;
    if !output.status.success() {
        return Err(HermeticDeployError::ForgeFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    if forge_deployment_path != deployment_path {
        fs::rename(&forge_deployment_path, &deployment_path)
            .map_err(HermeticDeployError::Filesystem)?;
    }
    let file: DeploymentFile = serde_json::from_str(
        &fs::read_to_string(&deployment_path).map_err(HermeticDeployError::Filesystem)?,
    )
    .map_err(HermeticDeployError::DeploymentFile)?;
    let addresses = HermeticAddresses {
        message_inbox_proxy: parse_address("MessageInbox proxy", &file.message_inbox_proxy)?,
        message_inbox_implementation: parse_address(
            "MessageInbox implementation", &file.message_inbox_implementation,
        )?,
        nock: parse_address("Nock", &file.nock)?,
        deployer,
    };
    let enable_hash = backend
        .backend()
        .send_transaction(
            deployer,
            addresses.message_inbox_proxy,
            encode_set_withdrawals_enabled(true),
        )
        .await?;
    let enable_receipt = backend
        .backend()
        .wait_for_receipt(enable_hash, Duration::from_secs(10))
        .await?;
    if !enable_receipt.success {
        return Err(HermeticDeployError::RevertedReceipt(enable_hash));
    }
    let after_block = backend.block_number().await?;
    if after_block <= before_block {
        return Err(HermeticDeployError::NoDeploymentBlocks);
    }
    let receipts =
        collect_deployment_receipts(backend, before_block + 1, after_block, &addresses).await?;
    let facts = observe_deployment(backend, &addresses, artifacts, receipts).await?;
    let expected_signers = config
        .deterministic_signers
        .map(|signer| format!("{signer:#x}"));
    if facts.bridge_state.bridge_nodes != expected_signers {
        return Err(HermeticDeployError::DeployedSignerMismatch);
    }
    Ok(facts)
}

async fn collect_deployment_receipts(
    backend: &AnvilBackend,
    first_block: u64,
    last_block: u64,
    addresses: &HermeticAddresses,
) -> Result<Vec<DeploymentReceiptRecord>, HermeticDeployError> {
    let mut receipts = Vec::new();
    for block in first_block..=last_block {
        for hash in backend.backend().block_transactions(block).await? {
            let receipt = backend
                .backend()
                .transaction_receipt(hash)
                .await?
                .ok_or(HermeticDeployError::MissingReceipt(hash))?;
            if !receipt.success {
                return Err(HermeticDeployError::RevertedReceipt(hash));
            }
            let role = match receipt.contract_address {
                Some(address) if address == addresses.nock => "nock_deployment",
                Some(address) if address == addresses.message_inbox_implementation => {
                    "message_inbox_implementation_deployment"
                }
                Some(address) if address == addresses.message_inbox_proxy => {
                    "message_inbox_proxy_deployment"
                }
                Some(_) => "unexpected_contract_deployment",
                None if receipt
                    .logs
                    .iter()
                    .any(|log| log.address == addresses.message_inbox_proxy) =>
                {
                    "withdrawal_gate_enabled"
                }
                None => "nock_inbox_pairing",
            };
            receipts.push(DeploymentReceiptRecord {
                role: role.to_owned(),
                receipt,
            });
        }
    }
    for required in [
        "nock_deployment", "message_inbox_implementation_deployment",
        "message_inbox_proxy_deployment", "nock_inbox_pairing", "withdrawal_gate_enabled",
    ] {
        if !receipts.iter().any(|receipt| receipt.role == required) {
            return Err(HermeticDeployError::MissingReceiptRole(required));
        }
    }
    if receipts
        .iter()
        .any(|receipt| receipt.role == "unexpected_contract_deployment")
    {
        return Err(HermeticDeployError::UnexpectedDeploymentReceipt);
    }
    Ok(receipts)
}

async fn observe_deployment(
    backend: &AnvilBackend,
    addresses: &HermeticAddresses,
    artifacts: &LoadedArtifacts,
    receipts: Vec<DeploymentReceiptRecord>,
) -> Result<DeploymentFacts, HermeticDeployError> {
    let block_number = backend.block_number().await?;
    let block_hash = backend.block_hash(block_number).await?;
    let state = read_contract_state(backend, addresses.message_inbox_proxy, addresses.nock).await?;
    validate_state(&state, addresses)?;
    let implementation_word = backend
        .backend()
        .storage_at(
            addresses.message_inbox_proxy, ERC1967_IMPLEMENTATION_SLOT, "latest",
        )
        .await?;
    let implementation = Address::from_slice(&implementation_word.as_slice()[12..]);
    if implementation != addresses.message_inbox_implementation {
        return Err(HermeticDeployError::ProxyImplementationMismatch {
            expected: addresses.message_inbox_implementation,
            observed: implementation,
        });
    }
    let runtime_artifacts = vec![
        verify_runtime(
            backend, "MessageInbox", addresses.message_inbox_implementation,
            &artifacts.message_inbox,
        )
        .await?,
        verify_runtime(backend, "Nock", addresses.nock, &artifacts.nock).await?,
        verify_runtime(
            backend, "ERC1967Proxy", addresses.message_inbox_proxy, &artifacts.proxy,
        )
        .await?,
    ];
    Ok(DeploymentFacts {
        environment_id: HERMETIC_ENVIRONMENT_ID.to_owned(),
        block_number,
        block_hash,
        addresses: addresses.clone(),
        bridge_state: state,
        runtime_artifacts,
        receipts,
    })
}

fn validate_state(
    state: &ForkContractState,
    addresses: &HermeticAddresses,
) -> Result<(), HermeticDeployError> {
    if state.owner != format!("{:#x}", addresses.deployer)
        || state.nock_owner != format!("{:#x}", addresses.deployer)
        || state.threshold != 3
        || !state.withdrawals_enabled
        || state.message_inbox_nock != format!("{:#x}", addresses.nock)
        || state.nock_inbox != format!("{:#x}", addresses.message_inbox_proxy)
    {
        return Err(HermeticDeployError::ReadinessMismatch(Box::new(
            state.clone(),
        )));
    }
    let mut unique = HashSet::new();
    for node in &state.bridge_nodes {
        let address = parse_address("bridge node", node)?;
        if address == Address::ZERO || !unique.insert(address) {
            return Err(HermeticDeployError::InvalidSignerSet);
        }
    }
    Ok(())
}

async fn verify_runtime(
    backend: &AnvilBackend,
    contract_name: &str,
    address: Address,
    artifact: &LoadedArtifact,
) -> Result<LocalArtifactRuntimeFacts, HermeticDeployError> {
    let actual = backend.backend().code(address, "latest").await?;
    if actual.is_empty() {
        return Err(HermeticDeployError::EmptyRuntimeCode(
            contract_name.to_owned(),
        ));
    }
    if actual.len() != artifact.deployed_template.len() {
        return Err(HermeticDeployError::RuntimeLengthMismatch {
            contract: contract_name.to_owned(),
            expected: artifact.deployed_template.len(),
            observed: actual.len(),
        });
    }
    let mut normalized_actual = actual.to_vec();
    for reference in &artifact.immutable_references {
        let end = reference
            .start
            .checked_add(reference.length)
            .ok_or_else(|| {
                HermeticDeployError::InvalidArtifact("immutable range overflow".to_owned())
            })?;
        if end > normalized_actual.len() {
            return Err(HermeticDeployError::InvalidArtifact(
                "immutable range exceeds runtime bytecode".to_owned(),
            ));
        }
        normalized_actual[reference.start..end].fill(0);
    }
    let runtime_matches_artifact = normalized_actual == artifact.deployed_template;
    if !runtime_matches_artifact {
        return Err(HermeticDeployError::RuntimeArtifactMismatch(
            contract_name.to_owned(),
        ));
    }
    Ok(LocalArtifactRuntimeFacts {
        contract_name: contract_name.to_owned(),
        artifact_path: artifact.path.clone(),
        artifact_sha256: artifact.sha256.clone(),
        runtime_keccak256: keccak256(actual.as_ref()),
        normalized_runtime_keccak256: keccak256(&normalized_actual),
        immutable_reference_count: artifact.immutable_references.len(),
        runtime_matches_artifact,
    })
}

#[derive(Clone)]
struct LoadedArtifacts {
    message_inbox: LoadedArtifact,
    nock: LoadedArtifact,
    proxy: LoadedArtifact,
}

impl LoadedArtifacts {
    fn load(config: &HermeticDeployConfig) -> Result<Self, HermeticDeployError> {
        Ok(Self {
            message_inbox: LoadedArtifact::load(
                "MessageInbox",
                &config.message_inbox_artifact,
                &["initialize", "bridgeNodes", "nock", "owner", "THRESHOLD", "withdrawalsEnabled"],
            )?,
            nock: LoadedArtifact::load(
                "Nock",
                &config.nock_artifact,
                &["mint", "inbox", "owner", "updateInbox"],
            )?,
            proxy: LoadedArtifact::load("ERC1967Proxy", &config.proxy_artifact, &[])?,
        })
    }
}

#[derive(Clone)]
struct LoadedArtifact {
    path: PathBuf,
    sha256: String,
    deployed_template: Vec<u8>,
    immutable_references: Vec<ImmutableReference>,
}

impl LoadedArtifact {
    fn load(
        contract_name: &str,
        path: &Path,
        required_abi_functions: &[&str],
    ) -> Result<Self, HermeticDeployError> {
        let bytes = fs::read(path).map_err(|error| HermeticDeployError::ArtifactRead {
            path: path.to_path_buf(),
            source: error,
        })?;
        let artifact: FoundryArtifact =
            serde_json::from_slice(&bytes).map_err(|error| HermeticDeployError::ArtifactJson {
                path: path.to_path_buf(),
                source: error,
            })?;
        let creation = decode_bytecode(&artifact.bytecode.object)?;
        let deployed_template = decode_bytecode(&artifact.deployed_bytecode.object)?;
        if creation.is_empty() || deployed_template.is_empty() {
            return Err(HermeticDeployError::InvalidArtifact(format!(
                "{contract_name} artifact has empty bytecode"
            )));
        }
        let functions = artifact
            .abi
            .iter()
            .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("function"))
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
            .collect::<HashSet<_>>();
        for required in required_abi_functions {
            if !functions.contains(required) {
                return Err(HermeticDeployError::InvalidArtifact(format!(
                    "{contract_name} ABI is missing {required}"
                )));
            }
        }
        let immutable_references = artifact
            .deployed_bytecode
            .immutable_references
            .into_values()
            .flatten()
            .collect();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            path: path.to_path_buf(),
            sha256,
            deployed_template,
            immutable_references,
        })
    }
}

fn validate_signers(signers: &[Address; 5]) -> Result<(), HermeticDeployError> {
    let mut unique = HashSet::new();
    for signer in signers {
        if *signer == Address::ZERO || !unique.insert(*signer) {
            return Err(HermeticDeployError::InvalidSignerSet);
        }
    }
    Ok(())
}

fn parse_address(field: &'static str, value: &str) -> Result<Address, HermeticDeployError> {
    value
        .parse()
        .map_err(|_| HermeticDeployError::InvalidAddress(field))
}

fn decode_bytecode(value: &str) -> Result<Vec<u8>, HermeticDeployError> {
    let digits = value.strip_prefix("0x").ok_or_else(|| {
        HermeticDeployError::InvalidArtifact("bytecode is not 0x-prefixed".to_owned())
    })?;
    hex::decode(digits)
        .map_err(|_| HermeticDeployError::InvalidArtifact("bytecode is not hex".to_owned()))
}

#[derive(Deserialize)]
struct FoundryArtifact {
    abi: Vec<Value>,
    bytecode: ArtifactBytecode,
    #[serde(rename = "deployedBytecode")]
    deployed_bytecode: ArtifactDeployedBytecode,
}

#[derive(Deserialize)]
struct ArtifactBytecode {
    object: String,
}

#[derive(Deserialize)]
struct ArtifactDeployedBytecode {
    object: String,
    #[serde(rename = "immutableReferences", default)]
    immutable_references: HashMap<String, Vec<ImmutableReference>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ImmutableReference {
    start: usize,
    length: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentFile {
    message_inbox_implementation: String,
    message_inbox_proxy: String,
    nock: String,
}

#[derive(Debug, Error)]
pub enum HermeticDeployError {
    #[error("hermetic deployment requires an empty Anvil backend")]
    WrongBackendMode,
    #[error("hermetic bridge signers must be five unique nonzero addresses")]
    InvalidSignerSet,
    #[error("deployed bridge signers do not match deterministic configuration")]
    DeployedSignerMismatch,
    #[error("failed to read contract artifact {path}")]
    ArtifactRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse contract artifact {path}")]
    ArtifactJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid contract artifact: {0}")]
    InvalidArtifact(String),
    #[error("Anvil exposed no unlocked deployment account")]
    NoAnvilAccount,
    #[error("failed to create hermetic deployment file")]
    Filesystem(#[source] std::io::Error),
    #[error("failed to launch Forge deployment")]
    ForgeLaunch(#[source] std::io::Error),
    #[error("Forge deployment failed: {0}")]
    ForgeFailed(String),
    #[error("failed to parse Forge deployment result")]
    DeploymentFile(#[source] serde_json::Error),
    #[error("invalid {0} address")]
    InvalidAddress(&'static str),
    #[error("Forge produced no deployment blocks")]
    NoDeploymentBlocks,
    #[error("missing deployment receipt {0:#x}")]
    MissingReceipt(B256),
    #[error("deployment receipt reverted {0:#x}")]
    RevertedReceipt(B256),
    #[error("missing deployment receipt role {0}")]
    MissingReceiptRole(&'static str),
    #[error("deployment produced an unexpected contract receipt")]
    UnexpectedDeploymentReceipt,
    #[error("proxy implementation mismatch: expected {expected:#x}, observed {observed:#x}")]
    ProxyImplementationMismatch {
        expected: Address,
        observed: Address,
    },
    #[error("hermetic contract readiness mismatch")]
    ReadinessMismatch(Box<ForkContractState>),
    #[error("{0} runtime code is empty")]
    EmptyRuntimeCode(String),
    #[error("{contract} runtime length mismatch: expected {expected}, observed {observed}")]
    RuntimeLengthMismatch {
        contract: String,
        expected: usize,
        observed: usize,
    },
    #[error("{0} runtime bytecode does not match local artifact")]
    RuntimeArtifactMismatch(String),
    #[error("hermetic deployment failed and rollback completed={reverted}: {reason}")]
    RolledBack { reason: String, reverted: bool },
    #[error("hermetic deployment baseline snapshot is unavailable")]
    SnapshotUnavailable,
    #[error("hermetic deployment reset did not reproduce deployment facts")]
    ResetMismatch {
        expected: Box<DeploymentFacts>,
        observed: Box<DeploymentFacts>,
    },
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
    #[error("failed to read bridge contract state: {0}")]
    BridgeState(String),
}

impl From<ForkSeedError> for HermeticDeployError {
    fn from(error: ForkSeedError) -> Self {
        Self::BridgeState(error.to_string())
    }
}
