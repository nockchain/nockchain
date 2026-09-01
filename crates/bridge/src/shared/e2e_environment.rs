use std::collections::HashSet;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::shared::types::{WITHDRAWAL_POLICY_V1_ID, WITHDRAWAL_WIRE_V1_ID};

pub const BASE_SEPOLIA_E2E_SCHEMA_ID: &str = "nockchain.bridge.e2e-environment";
pub const BASE_SEPOLIA_E2E_SCHEMA_VERSION: u64 = 1;
pub const BASE_SEPOLIA_E2E_ENVIRONMENT_ID: &str = "base-sepolia-fork";
pub const BASE_SEPOLIA_SOURCE_CHAIN_ID: u64 = 84_532;
pub const BASE_SEPOLIA_E2E_CHAIN_ID: u64 = 31_338;
pub const BASE_SEPOLIA_BRIDGE_THRESHOLD: u64 = 3;

const BASE_SEPOLIA_CHAIN_NAME: &str = "base-sepolia";
const FINALIZED_BLOCK_ID: &str = "finalized";
const ANVIL_ENGINE_ID: &str = "anvil";
const LOOPBACK_ONLY_POLICY_ID: &str = "loopback-only";
const EXPLORER_ID: &str = "blockscout-base-sepolia";
const EXPLORER_ROOT: &str = "https://base-sepolia.blockscout.com/";
const ARTIFACT_HASH_SCHEME_ID: &str = "canonical-verified-compiler-artifact-v1";
const ARTIFACT_HASH_CANONICALIZATION: &str =
    "utf8-compact-json-sort-object-keys-sort-sources-by-file-path-preserve-other-array-order";
const ARTIFACT_HASH_FIELDS: [&str; 7] = [
    "abi", "compiler_settings", "compiler_version", "creation_bytecode", "deployed_bytecode",
    "name", "sources",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseSepoliaE2eManifest {
    pub schema_id: String,
    pub schema_version: u64,
    pub environment_id: String,
    pub source_chain: SourceChainIdentity,
    pub local_fork: LocalForkIdentity,
    pub contracts: ContractIdentity,
    pub pristine_state: PristineContractState,
    pub artifacts: ContractArtifacts,
    pub protocol: WithdrawalProtocolIdentity,
    pub evidence: ManifestEvidence,
    pub refresh: ManifestRefreshProcedure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceChainIdentity {
    pub name: String,
    pub chain_id: u64,
    pub fork_block: PinnedForkBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedForkBlock {
    pub number: u64,
    pub hash: String,
    pub timestamp: String,
    pub finality: String,
    pub explorer_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalForkIdentity {
    pub engine: String,
    pub chain_id: u64,
    pub rpc_host_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractIdentity {
    pub message_inbox: MessageInboxIdentity,
    pub nock: DeployedContractIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageInboxIdentity {
    pub proxy: DeployedContractIdentity,
    pub implementation: DeployedContractIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployedContractIdentity {
    pub address: String,
    pub runtime_code_keccak256: String,
    pub deployment: DeploymentReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentReference {
    pub transaction_hash: String,
    pub block_number: u64,
    pub block_hash: String,
    pub timestamp: String,
    pub explorer_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PristineContractState {
    pub message_inbox_owner: String,
    pub nock_owner: String,
    pub bridge_nodes: [String; 5],
    pub threshold: u64,
    pub withdrawals_enabled: bool,
    pub reciprocal_pairing: ReciprocalContractPairing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReciprocalContractPairing {
    pub message_inbox_nock: String,
    pub nock_inbox: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractArtifacts {
    pub hash_scheme: ArtifactHashScheme,
    pub erc1967_proxy: ContractArtifactIdentity,
    pub message_inbox: ContractArtifactIdentity,
    pub nock: ContractArtifactIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactHashScheme {
    pub id: String,
    pub digest: String,
    pub encoding: String,
    pub canonicalization: String,
    pub artifact_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractArtifactIdentity {
    pub contract_name: String,
    pub compiler_version: String,
    pub verified_artifact_sha256: String,
    pub abi_sha256: String,
    pub verification_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalProtocolIdentity {
    pub withdrawal_wire_id: String,
    pub withdrawal_policy_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEvidence {
    pub observed_at: String,
    pub rpc_sources: [String; 2],
    pub explorer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRefreshProcedure {
    pub mode: String,
    pub requires_finalized_block: bool,
    pub requires_two_rpc_sources: bool,
    pub requires_reviewed_code_hashes: bool,
    pub steps: Vec<String>,
}

#[derive(Debug, Error)]
pub enum BaseSepoliaE2eManifestError {
    #[error("invalid Base Sepolia E2E manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid Base Sepolia E2E manifest: {0}")]
    Validation(String),
    #[error("pinned fork block number mismatch: expected {expected}, observed {observed}")]
    PinnedBlockNumberMismatch { expected: u64, observed: u64 },
    #[error("pinned fork block hash mismatch: expected {expected}, observed {observed}")]
    PinnedBlockHashMismatch { expected: String, observed: String },
}

impl BaseSepoliaE2eManifest {
    pub fn from_json(input: &str) -> Result<Self, BaseSepoliaE2eManifestError> {
        let manifest: Self = serde_json::from_str(input)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String, BaseSepoliaE2eManifestError> {
        self.validate()?;
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn validate(&self) -> Result<(), BaseSepoliaE2eManifestError> {
        require_equal("schema_id", &self.schema_id, BASE_SEPOLIA_E2E_SCHEMA_ID)?;
        if self.schema_version != BASE_SEPOLIA_E2E_SCHEMA_VERSION {
            return invalid(format!(
                "schema_version must be {BASE_SEPOLIA_E2E_SCHEMA_VERSION}, got {}",
                self.schema_version
            ));
        }
        require_equal(
            "environment_id", &self.environment_id, BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
        )?;
        require_equal(
            "source_chain.name", &self.source_chain.name, BASE_SEPOLIA_CHAIN_NAME,
        )?;
        if self.source_chain.chain_id != BASE_SEPOLIA_SOURCE_CHAIN_ID {
            return invalid(format!(
                "source_chain.chain_id must be {BASE_SEPOLIA_SOURCE_CHAIN_ID}, got {}",
                self.source_chain.chain_id
            ));
        }
        if self.source_chain.fork_block.number == 0 {
            return invalid("source_chain.fork_block.number must be nonzero");
        }
        validate_hash(
            "source_chain.fork_block.hash", &self.source_chain.fork_block.hash,
        )?;
        let fork_timestamp = validate_utc_timestamp(
            "source_chain.fork_block.timestamp", &self.source_chain.fork_block.timestamp,
        )?;
        require_equal(
            "source_chain.fork_block.finality", &self.source_chain.fork_block.finality,
            FINALIZED_BLOCK_ID,
        )?;
        validate_explorer_url(
            "source_chain.fork_block.explorer_url", &self.source_chain.fork_block.explorer_url,
        )?;
        if !self
            .source_chain
            .fork_block
            .explorer_url
            .ends_with(&self.source_chain.fork_block.number.to_string())
        {
            return invalid(
                "source_chain.fork_block.explorer_url does not identify the pinned block",
            );
        }

        require_equal(
            "local_fork.engine", &self.local_fork.engine, ANVIL_ENGINE_ID,
        )?;
        if self.local_fork.chain_id != BASE_SEPOLIA_E2E_CHAIN_ID {
            return invalid(format!(
                "local_fork.chain_id must be the dedicated E2E chain id {BASE_SEPOLIA_E2E_CHAIN_ID}, got {}",
                self.local_fork.chain_id
            ));
        }
        if matches!(
            self.local_fork.chain_id,
            8_453 | BASE_SEPOLIA_SOURCE_CHAIN_ID
        ) {
            return invalid("local_fork.chain_id must not use a live Base chain id");
        }
        require_equal(
            "local_fork.rpc_host_policy", &self.local_fork.rpc_host_policy, LOOPBACK_ONLY_POLICY_ID,
        )?;

        self.validate_contract(
            "contracts.message_inbox.proxy", &self.contracts.message_inbox.proxy,
        )?;
        self.validate_contract(
            "contracts.message_inbox.implementation", &self.contracts.message_inbox.implementation,
        )?;
        self.validate_contract("contracts.nock", &self.contracts.nock)?;

        validate_address(
            "pristine_state.message_inbox_owner", &self.pristine_state.message_inbox_owner,
        )?;
        validate_address("pristine_state.nock_owner", &self.pristine_state.nock_owner)?;
        let mut bridge_nodes = HashSet::with_capacity(self.pristine_state.bridge_nodes.len());
        for (index, node) in self.pristine_state.bridge_nodes.iter().enumerate() {
            validate_address(&format!("pristine_state.bridge_nodes[{index}]"), node)?;
            if !bridge_nodes.insert(node.as_str()) {
                return invalid(format!(
                    "pristine_state.bridge_nodes contains duplicate address {node}"
                ));
            }
        }
        if self.pristine_state.threshold != BASE_SEPOLIA_BRIDGE_THRESHOLD {
            return invalid(format!(
                "pristine_state.threshold must be contract-observed value {BASE_SEPOLIA_BRIDGE_THRESHOLD}, got {}",
                self.pristine_state.threshold
            ));
        }
        require_equal(
            "pristine_state.reciprocal_pairing.message_inbox_nock",
            &self.pristine_state.reciprocal_pairing.message_inbox_nock,
            &self.contracts.nock.address,
        )?;
        require_equal(
            "pristine_state.reciprocal_pairing.nock_inbox",
            &self.pristine_state.reciprocal_pairing.nock_inbox,
            &self.contracts.message_inbox.proxy.address,
        )?;

        self.validate_artifacts()?;
        require_equal(
            "protocol.withdrawal_wire_id", &self.protocol.withdrawal_wire_id, WITHDRAWAL_WIRE_V1_ID,
        )?;
        require_equal(
            "protocol.withdrawal_policy_id", &self.protocol.withdrawal_policy_id,
            WITHDRAWAL_POLICY_V1_ID,
        )?;

        let observed_at =
            validate_utc_timestamp("evidence.observed_at", &self.evidence.observed_at)?;
        if observed_at < fork_timestamp {
            return invalid("evidence.observed_at predates the pinned fork block");
        }
        if self.evidence.rpc_sources[0] == self.evidence.rpc_sources[1] {
            return invalid("evidence.rpc_sources must name two independent sources");
        }
        for source in &self.evidence.rpc_sources {
            if source.trim().is_empty() || source.contains("://") {
                return invalid(
                    "evidence.rpc_sources must contain public source names, never endpoint URLs",
                );
            }
        }
        require_equal(
            "evidence.explorer_id", &self.evidence.explorer_id, EXPLORER_ID,
        )?;

        require_equal("refresh.mode", &self.refresh.mode, "manual-reviewed")?;
        if !self.refresh.requires_finalized_block {
            return invalid("refresh must require a finalized pinned block");
        }
        if !self.refresh.requires_two_rpc_sources {
            return invalid("refresh must require two independent RPC sources");
        }
        if !self.refresh.requires_reviewed_code_hashes {
            return invalid("refresh must require reviewed runtime code hashes");
        }
        if self.refresh.steps.len() < 6
            || self.refresh.steps.iter().any(|step| step.trim().is_empty())
        {
            return invalid("refresh.steps must contain the complete nonempty review procedure");
        }

        Ok(())
    }

    pub fn validate_pinned_block(
        &self,
        observed_number: u64,
        observed_hash: &str,
    ) -> Result<(), BaseSepoliaE2eManifestError> {
        validate_hash("observed pinned block hash", observed_hash)?;
        if observed_number != self.source_chain.fork_block.number {
            return Err(BaseSepoliaE2eManifestError::PinnedBlockNumberMismatch {
                expected: self.source_chain.fork_block.number,
                observed: observed_number,
            });
        }
        if observed_hash != self.source_chain.fork_block.hash {
            return Err(BaseSepoliaE2eManifestError::PinnedBlockHashMismatch {
                expected: self.source_chain.fork_block.hash.clone(),
                observed: observed_hash.to_owned(),
            });
        }
        Ok(())
    }

    fn validate_contract(
        &self,
        field: &str,
        contract: &DeployedContractIdentity,
    ) -> Result<(), BaseSepoliaE2eManifestError> {
        validate_address(&format!("{field}.address"), &contract.address)?;
        validate_hash(
            &format!("{field}.runtime_code_keccak256"),
            &contract.runtime_code_keccak256,
        )?;
        validate_hash(
            &format!("{field}.deployment.transaction_hash"),
            &contract.deployment.transaction_hash,
        )?;
        validate_hash(
            &format!("{field}.deployment.block_hash"),
            &contract.deployment.block_hash,
        )?;
        if contract.deployment.block_number == 0
            || contract.deployment.block_number > self.source_chain.fork_block.number
        {
            return invalid(format!(
                "{field}.deployment.block_number must be nonzero and no later than the pinned fork block"
            ));
        }
        validate_utc_timestamp(
            &format!("{field}.deployment.timestamp"),
            &contract.deployment.timestamp,
        )?;
        validate_explorer_url(
            &format!("{field}.deployment.explorer_url"),
            &contract.deployment.explorer_url,
        )?;
        if !contract
            .deployment
            .explorer_url
            .ends_with(&contract.deployment.transaction_hash)
        {
            return invalid(format!(
                "{field}.deployment.explorer_url does not identify its transaction"
            ));
        }
        Ok(())
    }

    fn validate_artifacts(&self) -> Result<(), BaseSepoliaE2eManifestError> {
        let scheme = &self.artifacts.hash_scheme;
        require_equal(
            "artifacts.hash_scheme.id", &scheme.id, ARTIFACT_HASH_SCHEME_ID,
        )?;
        require_equal("artifacts.hash_scheme.digest", &scheme.digest, "sha256")?;
        require_equal(
            "artifacts.hash_scheme.encoding", &scheme.encoding, "0x-prefixed-lowercase-hex",
        )?;
        require_equal(
            "artifacts.hash_scheme.canonicalization", &scheme.canonicalization,
            ARTIFACT_HASH_CANONICALIZATION,
        )?;
        if scheme
            .artifact_fields
            .iter()
            .map(String::as_str)
            .ne(ARTIFACT_HASH_FIELDS)
        {
            return invalid("artifacts.hash_scheme.artifact_fields does not match the v1 scheme");
        }

        validate_artifact(
            "artifacts.erc1967_proxy", &self.artifacts.erc1967_proxy, "ERC1967Proxy",
            &self.contracts.message_inbox.proxy.address,
        )?;
        validate_artifact(
            "artifacts.message_inbox", &self.artifacts.message_inbox, "MessageInbox",
            &self.contracts.message_inbox.implementation.address,
        )?;
        validate_artifact(
            "artifacts.nock", &self.artifacts.nock, "Nock", &self.contracts.nock.address,
        )?;
        Ok(())
    }
}

fn validate_artifact(
    field: &str,
    artifact: &ContractArtifactIdentity,
    expected_name: &str,
    deployed_address: &str,
) -> Result<(), BaseSepoliaE2eManifestError> {
    require_equal(
        &format!("{field}.contract_name"),
        &artifact.contract_name,
        expected_name,
    )?;
    if artifact.compiler_version.trim().is_empty() {
        return invalid(format!("{field}.compiler_version must not be empty"));
    }
    validate_hash(
        &format!("{field}.verified_artifact_sha256"),
        &artifact.verified_artifact_sha256,
    )?;
    validate_hash(&format!("{field}.abi_sha256"), &artifact.abi_sha256)?;
    validate_explorer_url(
        &format!("{field}.verification_url"),
        &artifact.verification_url,
    )?;
    if !artifact.verification_url.ends_with(deployed_address) {
        return invalid(format!(
            "{field}.verification_url does not identify the deployed contract"
        ));
    }
    Ok(())
}

fn validate_address(field: &str, value: &str) -> Result<(), BaseSepoliaE2eManifestError> {
    validate_prefixed_lower_hex(field, value, 20)?;
    if value[2..].bytes().all(|byte| byte == b'0') {
        return invalid(format!("{field} must not be the zero address"));
    }
    Ok(())
}

fn validate_hash(field: &str, value: &str) -> Result<(), BaseSepoliaE2eManifestError> {
    validate_prefixed_lower_hex(field, value, 32)?;
    if value[2..].bytes().all(|byte| byte == b'0') {
        return invalid(format!("{field} must not be the zero hash"));
    }
    Ok(())
}

fn validate_prefixed_lower_hex(
    field: &str,
    value: &str,
    byte_len: usize,
) -> Result<(), BaseSepoliaE2eManifestError> {
    if value.len() != 2 + byte_len * 2 || !value.starts_with("0x") {
        return invalid(format!(
            "{field} must be a 0x-prefixed {byte_len}-byte value"
        ));
    }
    if !value[2..]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!("{field} must use canonical lowercase hex"));
    }
    Ok(())
}

fn validate_utc_timestamp(
    field: &str,
    value: &str,
) -> Result<DateTime<FixedOffset>, BaseSepoliaE2eManifestError> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|error| BaseSepoliaE2eManifestError::Validation(format!("{field}: {error}")))?;
    if timestamp.offset().local_minus_utc() != 0 || !value.ends_with('Z') {
        return invalid(format!(
            "{field} must be a canonical UTC RFC 3339 timestamp"
        ));
    }
    Ok(timestamp)
}

fn validate_explorer_url(field: &str, value: &str) -> Result<(), BaseSepoliaE2eManifestError> {
    if !value.starts_with(EXPLORER_ROOT) || value.contains('@') {
        return invalid(format!(
            "{field} must be a credential-free Base Sepolia explorer URL"
        ));
    }
    Ok(())
}

fn require_equal(
    field: &str,
    actual: &str,
    expected: &str,
) -> Result<(), BaseSepoliaE2eManifestError> {
    if actual != expected {
        return invalid(format!("{field} must be {expected:?}, got {actual:?}"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, BaseSepoliaE2eManifestError> {
    Err(BaseSepoliaE2eManifestError::Validation(message.into()))
}
