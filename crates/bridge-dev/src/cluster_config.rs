use std::collections::HashSet;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy::primitives::{keccak256, Address, B256};
use alloy::signers::local::PrivateKeySigner;
use bridge::shared::config::{
    derive_bridge_spend_authority_from_pkhs, BridgeConfigToml, BridgeConstantsToml, NodeInfoToml,
    SequencerConfigToml, SequencerJournalConfigToml, SequencerNodeInfoToml,
};
use bridge::shared::e2e_environment::BASE_SEPOLIA_E2E_CHAIN_ID;
use bridge::shared::types::WITHDRAWAL_POLICY_V1_ID;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::anvil::AnvilBackend;
use crate::anvil_fork::PinnedForkEvidence;
use crate::artifacts::E2eArtifacts;
use crate::fork_seeder::{read_contract_state, ForkContractState};
use crate::fork_state::ForkState;
use crate::hermetic_deploy::DeploymentFacts;
pub const BRIDGE_DEV_IRIS_SDK_VERSION_ENV: &str = "BRIDGE_DEV_IRIS_SDK_VERSION";

pub const BRIDGE_ETH_KEYS: [&str; 5] = [
    "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318",
    "0x5c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362319",
    "0x6c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231a",
    "0x7c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231b",
    "0x8c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231c",
];
pub const BRIDGE_ETH_ADDRS: [&str; 5] = [
    "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23", "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9",
    "0x274BD645de480C325D618c60c661F11275eB77F1", "0x6dc59eb20f7928935c47A391e35545a2CEC51013",
    "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375",
];
pub const BRIDGE_NOCK_KEYS: [&str; 5] = [
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T", "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8U",
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8V", "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8W",
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8X",
];
pub const BRIDGE_NOCK_PKHS: [&str; 5] = [
    "A47ZMEQ2U2x1h3bVMUNdkutKYNiyXFWMVTQZC8BWgXBmS5mc6ysAhLZ",
    "BYp766x6Zhu7DHbewMHu7ajsAenRMm1M7rgmpxUwY83BJy4RGMAG2z8",
    "2f7BtZpaaKVb9mCUFgMuYjcQXhrexfqCJs4h1es5t9jQrqdmhVgYLU6",
    "BLCg8KPPKDJPJ8hhdHSGsurxgKwBorqpF1qrHsCiojsPf96GEzwsFQ",
    "AeZ1jsSHoAg7bjBr2k4kMeRERsx85Bp68tfTMiiYZtjFRCtc4gexNWc",
];

pub fn deterministic_cluster_nodes() -> [ClusterNodeIdentity; 5] {
    std::array::from_fn(|index| ClusterNodeIdentity {
        eth_private_key: BRIDGE_ETH_KEYS[index].to_owned(),
        eth_address: BRIDGE_ETH_ADDRS[index].to_owned(),
        nock_private_key: BRIDGE_NOCK_KEYS[index].to_owned(),
        nock_pkh: BRIDGE_NOCK_PKHS[index].to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterNodeIdentity {
    pub eth_private_key: String,
    pub eth_address: String,
    pub nock_private_key: String,
    pub nock_pkh: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterDeploymentFacts {
    pub environment_id: String,
    pub chain_id: u64,
    pub start_height: u64,
    pub start_block_hash: B256,
    pub inbox: Address,
    pub implementation: Address,
    pub nock: Address,
    pub state: ForkContractState,
    pub proxy_runtime_keccak256: B256,
    pub implementation_runtime_keccak256: B256,
    pub nock_runtime_keccak256: B256,
}

impl ClusterDeploymentFacts {
    pub fn from_hermetic(
        facts: &DeploymentFacts,
        chain_id: u64,
    ) -> Result<Self, ClusterConfigError> {
        let runtime = |name: &str| {
            facts
                .runtime_artifacts
                .iter()
                .find(|artifact| artifact.contract_name == name)
                .map(|artifact| artifact.runtime_keccak256)
                .ok_or(ClusterConfigError::MissingRuntimeFact(name.to_owned()))
        };
        Ok(Self {
            environment_id: facts.environment_id.clone(),
            chain_id,
            start_height: facts.block_number,
            start_block_hash: facts.block_hash,
            inbox: facts.addresses.message_inbox_proxy,
            implementation: facts.addresses.message_inbox_implementation,
            nock: facts.addresses.nock,
            state: facts.bridge_state.clone(),
            proxy_runtime_keccak256: runtime("ERC1967Proxy")?,
            implementation_runtime_keccak256: runtime("MessageInbox")?,
            nock_runtime_keccak256: runtime("Nock")?,
        })
    }

    pub fn from_fork(
        evidence: &PinnedForkEvidence,
        chain_id: u64,
    ) -> Result<Self, ClusterConfigError> {
        let pristine = &evidence.pristine;
        Ok(Self {
            environment_id: "base-sepolia-fork".to_owned(),
            chain_id,
            start_height: evidence.source_block_number,
            start_block_hash: parse_hash("source block", &evidence.source_block_hash)?,
            inbox: parse_address("MessageInbox proxy", &pristine.message_inbox_proxy.address)?,
            implementation: parse_address(
                "MessageInbox implementation", &pristine.message_inbox_implementation.address,
            )?,
            nock: parse_address("Nock", &pristine.nock.address)?,
            state: ForkContractState {
                owner: pristine.message_inbox_owner.clone(),
                nock_owner: pristine.nock_owner.clone(),
                bridge_nodes: pristine.bridge_nodes.clone(),
                threshold: pristine.threshold,
                withdrawals_enabled: pristine.withdrawals_enabled,
                message_inbox_nock: pristine.reciprocal_pairing.message_inbox_nock.clone(),
                nock_inbox: pristine.reciprocal_pairing.nock_inbox.clone(),
            },
            proxy_runtime_keccak256: parse_hash(
                "MessageInbox proxy runtime", &pristine.message_inbox_proxy.keccak256,
            )?,
            implementation_runtime_keccak256: parse_hash(
                "MessageInbox implementation runtime",
                &pristine.message_inbox_implementation.keccak256,
            )?,
            nock_runtime_keccak256: parse_hash("Nock runtime", &pristine.nock.keccak256)?,
        })
    }

    pub fn from_seeded_fork(
        evidence: &PinnedForkEvidence,
        state: &ForkState,
        chain_id: u64,
    ) -> Result<Self, ClusterConfigError> {
        let mut facts = Self::from_fork(evidence, chain_id)?;
        if state.baseline().block_number < evidence.source_block_number {
            return Err(ClusterConfigError::ForkBaselineBeforeSource {
                source_height: evidence.source_block_number,
                baseline: state.baseline().block_number,
            });
        }
        facts.start_height = state.baseline().block_number;
        facts.start_block_hash = state.baseline().block_hash;
        facts.state = state.baseline().bridge_state.clone();
        Ok(facts)
    }
}

#[derive(Debug, Clone)]
pub struct ClusterConfigInput {
    pub run_root: PathBuf,
    pub run_id: String,
    pub deployment: ClusterDeploymentFacts,
    pub artifacts: E2eArtifacts,
    pub nodes: [ClusterNodeIdentity; 5],
    pub base_http_url: String,
    pub base_ws_url: String,
    pub port_offset: u16,
    pub base_confirmation_depth: u64,
    pub nockchain_confirmation_depth: u64,
    pub withdrawal_activation_nock_next_height: u64,
    pub base_blocks_chunk: u64,
    pub fakenet_pow_len: u64,
    pub fakenet_log_difficulty: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterConfigPaths {
    pub config_dir: PathBuf,
    pub bridge_configs: [PathBuf; 5],
    pub sequencer_config: PathBuf,
    pub redacted_manifest: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedClusterManifest {
    pub schema_version: u64,
    pub run_id: String,
    pub environment_id: String,
    pub chain_id: u64,
    pub base_http_url: String,
    pub base_ws_url: String,
    pub start_height: u64,
    pub start_block_hash: String,
    pub inbox: String,
    pub implementation: String,
    pub nock: String,
    pub proxy_runtime_keccak256: String,
    pub implementation_runtime_keccak256: String,
    pub nock_runtime_keccak256: String,
    pub bridge_nodes: [String; 5],
    pub bridge_config_paths: [PathBuf; 5],
    pub sequencer_config_path: PathBuf,
    pub artifacts: E2eArtifacts,
    pub fakenet_pow_len: u64,
    pub fakenet_log_difficulty: u64,
}

#[derive(Debug, Clone)]
pub struct ClusterConfigBundle {
    pub paths: ClusterConfigPaths,
    pub manifest: RedactedClusterManifest,
}

pub struct ClusterConfigGenerator;

impl ClusterConfigGenerator {
    pub fn generate(input: &ClusterConfigInput) -> Result<ClusterConfigBundle, ClusterConfigError> {
        validate_input(input)?;
        let config_dir = input.run_root.join("cluster-config");
        if config_dir.exists() {
            return Err(ClusterConfigError::StaleConfig(config_dir));
        }
        fs::create_dir_all(&config_dir).map_err(ClusterConfigError::Filesystem)?;
        set_directory_private(&config_dir)?;

        let ports = ClusterPorts::from_offset(input.port_offset)?;
        let mut sequencer_journal = SequencerJournalConfigToml {
            enabled: false,
            ..SequencerJournalConfigToml::default()
        };
        sequencer_journal.object_store.journal_id = format!("bridge-e2e-{}", input.run_id);
        let bridge_lock_root = derive_bridge_lock_root(&input.nodes)?;
        let constants = BridgeConstantsToml {
            min_signers: 3,
            total_signers: 5,
            minimum_event_nocks: 100_000,
            nicks_fee_per_nock: 195,
            base_blocks_chunk: input.base_blocks_chunk,
            base_start_height: input.deployment.start_height,
            nockchain_start_height: 1,
        };
        let grpc_address = format!("http://127.0.0.1:{}", ports.private_grpc);
        let sequencer_address = format!("http://127.0.0.1:{}", ports.sequencer_api);
        let bridge_paths_vec = (0..5usize)
            .map(|node_id| {
                let config = BridgeConfigToml {
                    node_id: node_id as u64,
                    base_ws_url: input.base_ws_url.clone(),
                    base_chain_id: Some(input.deployment.chain_id),
                    bridge_lock_root: bridge_lock_root.clone(),
                    inbox_contract_address: Some(format!("{:#x}", input.deployment.inbox)),
                    nock_contract_address: Some(format!("{:#x}", input.deployment.nock)),
                    my_eth_key: input.nodes[node_id].eth_private_key.clone(),
                    my_nock_key: input.nodes[node_id].nock_private_key.clone(),
                    grpc_address: grpc_address.clone(),
                    nockchain_sequencer_api_address: Some(sequencer_address.clone()),

                    base_confirmation_depth: input.base_confirmation_depth,
                    nockchain_confirmation_depth: input.nockchain_confirmation_depth,
                    withdrawal_policy: WITHDRAWAL_POLICY_V1_ID.to_owned(),
                    compensated_withdrawals: Vec::new(),
                    deposit_nonce_epoch_base: None,
                    deposit_nonce_epoch_start_height: None,
                    deposit_nonce_epoch_start_tx_id_base58: None,
                    withdrawal_processing_enabled: true,
                    withdrawal_activation_nock_next_height: Some(
                        input.withdrawal_activation_nock_next_height,
                    ),
                    ingress_listen_address: Some(format!("127.0.0.1:{}", ports.ingress[node_id])),
                    nodes: (0..5usize)
                        .map(|peer_id| NodeInfoToml {
                            ip: format!("127.0.0.1:{}", ports.ingress[peer_id]),
                            eth_pubkey: input.nodes[peer_id].eth_address.clone(),
                            nock_pkh: input.nodes[peer_id].nock_pkh.clone(),
                        })
                        .collect(),
                    constants: Some(constants.clone()),
                };
                let path = config_dir.join(format!("bridge-{node_id}.toml"));
                write_private(&path, toml::to_string_pretty(&config)?.as_bytes())?;
                Ok(path)
            })
            .collect::<Result<Vec<_>, ClusterConfigError>>()?;
        let bridge_configs: [PathBuf; 5] = bridge_paths_vec
            .try_into()
            .map_err(|_| ClusterConfigError::Internal("expected five bridge configs"))?;

        let sequencer_config = SequencerConfigToml {
            nock_contract_address: format!("{:#x}", input.deployment.nock),
            base_chain_id: Some(input.deployment.chain_id),
            nockchain_confirmation_depth: input.nockchain_confirmation_depth,
            withdrawal_policy: WITHDRAWAL_POLICY_V1_ID.to_owned(),
            compensated_withdrawals: Vec::new(),
            public_withdrawal_admission_enabled: true,
            manual_submit_approval: false,
            manual_submit_approval_dir: None,
            nodes: (0..5usize)
                .map(|node_id| SequencerNodeInfoToml {
                    eth_pubkey: input.nodes[node_id].eth_address.clone(),
                    nock_pkh: input.nodes[node_id].nock_pkh.clone(),
                })
                .collect(),
            sequencer_journal,
            constants: Some(constants),
        };
        let sequencer_path = config_dir.join("sequencer.toml");
        write_private(
            &sequencer_path,
            toml::to_string_pretty(&sequencer_config)?.as_bytes(),
        )?;
        let manifest_path = config_dir.join("manifest.json");
        let manifest = RedactedClusterManifest {
            schema_version: 1,
            run_id: input.run_id.clone(),
            environment_id: input.deployment.environment_id.clone(),
            chain_id: input.deployment.chain_id,
            base_http_url: input.base_http_url.clone(),
            base_ws_url: input.base_ws_url.clone(),
            start_height: input.deployment.start_height,
            start_block_hash: format!("{:#x}", input.deployment.start_block_hash),
            inbox: format!("{:#x}", input.deployment.inbox),
            implementation: format!("{:#x}", input.deployment.implementation),
            nock: format!("{:#x}", input.deployment.nock),
            proxy_runtime_keccak256: format!("{:#x}", input.deployment.proxy_runtime_keccak256),
            implementation_runtime_keccak256: format!(
                "{:#x}",
                input.deployment.implementation_runtime_keccak256
            ),
            nock_runtime_keccak256: format!("{:#x}", input.deployment.nock_runtime_keccak256),
            bridge_nodes: input.nodes.clone().map(|node| node.eth_address),
            bridge_config_paths: bridge_configs.clone(),
            sequencer_config_path: sequencer_path.clone(),
            artifacts: input.artifacts.clone(),
            fakenet_pow_len: input.fakenet_pow_len,
            fakenet_log_difficulty: input.fakenet_log_difficulty,
        };
        fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
            .map_err(ClusterConfigError::Filesystem)?;
        let paths = ClusterConfigPaths {
            config_dir,
            bridge_configs,
            sequencer_config: sequencer_path,
            redacted_manifest: manifest_path,
        };
        Ok(ClusterConfigBundle { paths, manifest })
    }

    pub async fn verify_backend(
        backend: &AnvilBackend,
        input: &ClusterConfigInput,
    ) -> Result<(), ClusterConfigError> {
        if backend.facts().chain_id != input.deployment.chain_id {
            return Err(ClusterConfigError::ChainIdMismatch {
                expected: input.deployment.chain_id,
                observed: backend.facts().chain_id,
            });
        }
        let observed_start_hash = backend
            .block_hash(input.deployment.start_height)
            .await
            .map_err(|error| ClusterConfigError::Readiness(error.to_string()))?;
        if observed_start_hash != input.deployment.start_block_hash {
            return Err(ClusterConfigError::FrontierMismatch {
                expected: input.deployment.start_block_hash,
                observed: observed_start_hash,
            });
        }
        let state = read_contract_state(backend, input.deployment.inbox, input.deployment.nock)
            .await
            .map_err(|error| ClusterConfigError::Readiness(error.to_string()))?;
        if state != input.deployment.state {
            return Err(ClusterConfigError::Readiness(
                "contract state does not match deployment facts".to_owned(),
            ));
        }
        for (name, address, expected) in [
            (
                "MessageInbox proxy", input.deployment.inbox,
                input.deployment.proxy_runtime_keccak256,
            ),
            (
                "MessageInbox implementation", input.deployment.implementation,
                input.deployment.implementation_runtime_keccak256,
            ),
            (
                "Nock", input.deployment.nock, input.deployment.nock_runtime_keccak256,
            ),
        ] {
            let code = backend.backend().code(address, "latest").await?;
            let observed = keccak256(code);
            if observed != expected {
                return Err(ClusterConfigError::RuntimeMismatch {
                    contract: name,
                    expected,
                    observed,
                });
            }
        }
        Ok(())
    }
}

fn validate_input(input: &ClusterConfigInput) -> Result<(), ClusterConfigError> {
    validate_base_urls(&input.base_http_url, &input.base_ws_url)?;
    if input.deployment.chain_id != BASE_SEPOLIA_E2E_CHAIN_ID {
        return Err(ClusterConfigError::UrlChainBinding);
    }
    if input.run_id.trim().is_empty()
        || input.deployment.start_height == 0
        || input.deployment.start_block_hash == B256::ZERO
        || input.deployment.inbox == Address::ZERO
        || input.deployment.implementation == Address::ZERO
        || input.deployment.nock == Address::ZERO
        || input.deployment.proxy_runtime_keccak256 == B256::ZERO
        || input.deployment.implementation_runtime_keccak256 == B256::ZERO
        || input.deployment.nock_runtime_keccak256 == B256::ZERO
        || input.base_confirmation_depth == 0
        || input.nockchain_confirmation_depth == 0
        || input.withdrawal_activation_nock_next_height == 0
        || input.fakenet_pow_len == 0
        || input.fakenet_log_difficulty == 0
    {
        return Err(ClusterConfigError::InvalidInput(
            "deployment facts and local confirmation/frontier settings must be nonzero",
        ));
    }
    if input.base_blocks_chunk == 0 {
        return Err(ClusterConfigError::InvalidInput(
            "base_blocks_chunk must be positive",
        ));
    }
    let mut addresses = HashSet::new();
    for (index, node) in input.nodes.iter().enumerate() {
        let signer = PrivateKeySigner::from_str(&node.eth_private_key)
            .map_err(|_| ClusterConfigError::InvalidNodeIdentity(index))?;
        let derived = format!("{:#x}", signer.address());
        let configured = parse_address("node Ethereum address", &node.eth_address)?;
        if derived != format!("{configured:#x}")
            || node.nock_private_key.trim().is_empty()
            || NockPkh::from_base58(&node.nock_pkh).is_err()
            || !addresses.insert(configured)
            || input.deployment.state.bridge_nodes[index] != format!("{configured:#x}")
        {
            return Err(ClusterConfigError::InvalidNodeIdentity(index));
        }
    }
    if input.deployment.state.threshold != 3
        || !input.deployment.state.withdrawals_enabled
        || input.deployment.state.message_inbox_nock != format!("{:#x}", input.deployment.nock)
        || input.deployment.state.nock_inbox != format!("{:#x}", input.deployment.inbox)
    {
        return Err(ClusterConfigError::DeploymentStateMismatch);
    }
    for artifact in [
        &input.artifacts.bridge, &input.artifacts.node, &input.artifacts.miner,
        &input.artifacts.wallet, &input.artifacts.bridge_jam, &input.artifacts.roswell_jam,
        &input.artifacts.fakenet_genesis_jam,
    ] {
        if !artifact.path.is_file() {
            return Err(ClusterConfigError::MissingArtifact(artifact.path.clone()));
        }
    }
    Ok(())
}

fn validate_base_urls(http: &str, ws: &str) -> Result<(), ClusterConfigError> {
    let http_url = reqwest::Url::parse(http).map_err(|_| ClusterConfigError::InvalidBaseUrl)?;
    let ws_url = reqwest::Url::parse(ws).map_err(|_| ClusterConfigError::InvalidBaseUrl)?;
    let host = http_url
        .host_str()
        .ok_or(ClusterConfigError::InvalidBaseUrl)?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if http_url.scheme() != "http"
        || ws_url.scheme() != "ws"
        || !loopback
        || http_url.host_str() != ws_url.host_str()
        || http_url.port() != ws_url.port()
        || http_url.port().is_none()
        || !http_url.username().is_empty()
        || http_url.password().is_some()
        || http_url.query().is_some()
        || ws_url.query().is_some()
        || http_url.fragment().is_some()
        || ws_url.fragment().is_some()
    {
        return Err(ClusterConfigError::InvalidBaseUrl);
    }
    Ok(())
}

fn derive_bridge_lock_root(nodes: &[ClusterNodeIdentity; 5]) -> Result<String, ClusterConfigError> {
    let pkhs = nodes
        .iter()
        .map(|node| NockPkh::from_base58(&node.nock_pkh))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ClusterConfigError::InvalidInput("invalid Nock PKH"))?;
    let (_, lock_root) = derive_bridge_spend_authority_from_pkhs(3, pkhs)
        .map_err(|error| ClusterConfigError::LockRoot(error.to_string()))?;
    Ok(lock_root.to_base58())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ClusterConfigError> {
    fs::write(path, bytes).map_err(ClusterConfigError::Filesystem)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(ClusterConfigError::Filesystem)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_private(path: &Path) -> Result<(), ClusterConfigError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(ClusterConfigError::Filesystem)
}

#[cfg(not(unix))]
fn set_directory_private(_path: &Path) -> Result<(), ClusterConfigError> {
    Ok(())
}

fn parse_address(field: &'static str, value: &str) -> Result<Address, ClusterConfigError> {
    value
        .parse()
        .map_err(|_| ClusterConfigError::InvalidAddress(field))
}

fn parse_hash(field: &'static str, value: &str) -> Result<B256, ClusterConfigError> {
    value
        .parse()
        .map_err(|_| ClusterConfigError::InvalidHash(field))
}

#[derive(Debug, Error)]
pub enum ClusterConfigError {
    #[error("missing runtime fact for {0}")]
    MissingRuntimeFact(String),
    #[error("seeded fork baseline {baseline} precedes source block {source_height}")]
    ForkBaselineBeforeSource { source_height: u64, baseline: u64 },
    #[error("invalid {0} address")]
    InvalidAddress(&'static str),
    #[error("invalid {0} hash")]
    InvalidHash(&'static str),
    #[error("invalid cluster input: {0}")]
    InvalidInput(&'static str),
    #[error("cluster config directory already exists: {0}")]
    StaleConfig(PathBuf),
    #[error("cluster config filesystem operation failed")]
    Filesystem(#[source] std::io::Error),
    #[error("failed to derive bridge lock root: {0}")]
    LockRoot(String),
    #[error("cluster node {0} key/address identity is invalid")]
    InvalidNodeIdentity(usize),
    #[error("deployment state does not match cluster contracts")]
    DeploymentStateMismatch,
    #[error("base HTTP/WS URLs do not identify one local Anvil")]
    InvalidBaseUrl,
    #[error("base URL chain binding does not match deployment chain id")]
    UrlChainBinding,
    #[error("cluster chain id mismatch: expected {expected}, observed {observed}")]
    ChainIdMismatch { expected: u64, observed: u64 },
    #[error("cluster Base frontier mismatch: expected {expected:#x}, observed {observed:#x}")]
    FrontierMismatch { expected: B256, observed: B256 },
    #[error("cluster contract readiness failed: {0}")]
    Readiness(String),
    #[error("{contract} runtime mismatch: expected {expected:#x}, observed {observed:#x}")]
    RuntimeMismatch {
        contract: &'static str,
        expected: B256,
        observed: B256,
    },
    #[error("required artifact is missing: {0}")]
    MissingArtifact(PathBuf),
    #[error("cluster config internal error: {0}")]
    Internal(&'static str),
    #[error(transparent)]
    Backend(#[from] crate::base_backend::BaseBackendError),
    #[error(transparent)]
    Toml(#[from] toml::ser::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy)]
struct ClusterPorts {
    private_grpc: u16,
    sequencer_api: u16,
    ingress: [u16; 5],
}

impl ClusterPorts {
    fn from_offset(offset: u16) -> Result<Self, ClusterConfigError> {
        let port = |base: u16| {
            base.checked_add(offset)
                .ok_or(ClusterConfigError::InvalidInput("port offset overflow"))
        };
        Ok(Self {
            private_grpc: port(5_002)?,
            sequencer_api: port(5_102)?,
            ingress: [port(8_002)?, port(8_003)?, port(8_004)?, port(8_005)?, port(8_006)?],
        })
    }
}
