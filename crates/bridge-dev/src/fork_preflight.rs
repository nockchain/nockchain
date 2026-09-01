use std::collections::HashMap;

use alloy::primitives::keccak256;
use bridge::shared::e2e_environment::{
    ContractArtifactIdentity, ContractArtifacts, ReciprocalContractPairing,
    WithdrawalProtocolIdentity,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::environment::BaseE2eEnvironment;
use crate::nonproduction_guard::{LoopbackBaseRpcUrl, NonproductionGuardError, ReadOnlyRpc};

const ERC1967_IMPLEMENTATION_SLOT: &str =
    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PristineBlockFacts {
    pub number: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCodeFacts {
    pub address: String,
    pub byte_len: usize,
    pub keccak256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PristineDeploymentFacts {
    pub source_block: PristineBlockFacts,
    pub message_inbox_proxy: RuntimeCodeFacts,
    pub message_inbox_implementation: RuntimeCodeFacts,
    pub nock: RuntimeCodeFacts,
    pub proxy_implementation: String,
    pub message_inbox_owner: String,
    pub nock_owner: String,
    pub bridge_nodes: [String; 5],
    pub threshold: u64,
    pub withdrawals_enabled: bool,
    pub reciprocal_pairing: ReciprocalContractPairing,
    pub protocol: WithdrawalProtocolIdentity,
    pub artifacts: ContractArtifacts,
}

#[derive(Debug, Clone)]
pub struct VerifiedPristineFork {
    facts: PristineDeploymentFacts,
}

impl VerifiedPristineFork {
    pub fn facts(&self) -> &PristineDeploymentFacts {
        &self.facts
    }

    pub fn into_facts(self) -> PristineDeploymentFacts {
        self.facts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentContract {
    MessageInboxProxy,
    MessageInboxImplementation,
    Nock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum DeploymentMismatch {
    SourceBlockNumber {
        expected: u64,
        observed: u64,
    },
    SourceBlockHash {
        expected: String,
        observed: String,
    },
    ContractAddress {
        contract: DeploymentContract,
        expected: String,
        observed: String,
    },
    EmptyRuntimeCode {
        contract: DeploymentContract,
        address: String,
    },
    RuntimeCodeHash {
        contract: DeploymentContract,
        expected: String,
        observed: String,
    },
    EmptyProxyImplementationSlot,
    ProxyImplementation {
        expected: String,
        observed: String,
    },
    MessageInboxOwner {
        expected: String,
        observed: String,
    },
    NockOwner {
        expected: String,
        observed: String,
    },
    BridgeNode {
        index: usize,
        expected: String,
        observed: String,
    },
    DuplicateBridgeNode {
        first_index: usize,
        second_index: usize,
        address: String,
    },
    Threshold {
        expected: u64,
        observed: u64,
    },
    WithdrawalsEnabled {
        expected: bool,
        observed: bool,
    },
    MessageInboxNock {
        expected: String,
        observed: String,
    },
    NockInbox {
        expected: String,
        observed: String,
    },
    LocalMetadata {
        field_path: String,
        expected: String,
        observed: String,
    },
}

#[derive(Debug, Error)]
pub enum ForkPreflightError {
    #[error("read-only fork RPC failed while reading {field}")]
    ReadField {
        field: &'static str,
        #[source]
        source: NonproductionGuardError,
    },
    #[error("fork RPC returned invalid {field}: {reason}")]
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
    #[error("pristine fork deployment does not match manifest ({mismatches_len} mismatches)")]
    Mismatches {
        mismatches_len: usize,
        mismatches: Vec<DeploymentMismatch>,
    },
}

impl ForkPreflightError {
    pub fn mismatches(&self) -> Option<&[DeploymentMismatch]> {
        match self {
            Self::Mismatches { mismatches, .. } => Some(mismatches),
            _ => None,
        }
    }
}

pub struct ForkPreflight;

impl ForkPreflight {
    pub async fn verify(
        endpoint: &LoopbackBaseRpcUrl,
        environment: &BaseE2eEnvironment,
    ) -> Result<VerifiedPristineFork, ForkPreflightError> {
        let facts = Self::observe(endpoint, environment).await?;
        let mismatches = Self::compare(environment, &facts);
        if mismatches.is_empty() {
            Ok(VerifiedPristineFork { facts })
        } else {
            Err(ForkPreflightError::Mismatches {
                mismatches_len: mismatches.len(),
                mismatches,
            })
        }
    }

    pub async fn observe(
        endpoint: &LoopbackBaseRpcUrl,
        environment: &BaseE2eEnvironment,
    ) -> Result<PristineDeploymentFacts, ForkPreflightError> {
        let manifest = environment.manifest();
        let rpc = endpoint
            .read_only_rpc()
            .map_err(|source| ForkPreflightError::ReadField {
                field: "RPC client",
                source,
            })?;
        let current_block: String =
            read_rpc(&rpc, "source block number", "eth_blockNumber", json!([])).await?;
        let current_block = decode_quantity("source block number", &current_block)?;
        let block_tag = format!("0x{:x}", manifest.source_chain.fork_block.number);
        let block: RpcBlock = read_rpc(
            &rpc,
            "source block",
            "eth_getBlockByNumber",
            json!([block_tag, false]),
        )
        .await?;
        let block_hash = normalize_hash(
            "source block hash",
            block
                .hash
                .as_deref()
                .ok_or(ForkPreflightError::InvalidField {
                    field: "source block hash",
                    reason: "block is absent or has no hash",
                })?,
        )?;

        let proxy = &manifest.contracts.message_inbox.proxy;
        let implementation = &manifest.contracts.message_inbox.implementation;
        let nock = &manifest.contracts.nock;
        let message_inbox_proxy =
            read_runtime_code(&rpc, "MessageInbox proxy code", &proxy.address, &block_tag).await?;
        let message_inbox_implementation = read_runtime_code(
            &rpc, "MessageInbox implementation code", &implementation.address, &block_tag,
        )
        .await?;
        let nock_code = read_runtime_code(&rpc, "Nock code", &nock.address, &block_tag).await?;

        let implementation_slot: String = read_rpc(
            &rpc,
            "proxy implementation slot",
            "eth_getStorageAt",
            json!([proxy.address, ERC1967_IMPLEMENTATION_SLOT, block_tag]),
        )
        .await?;
        let proxy_implementation =
            decode_address_word("proxy implementation slot", &implementation_slot)?;

        let message_inbox_owner = read_address_call(
            &rpc, "MessageInbox owner", &proxy.address, "owner()", &block_tag,
        )
        .await?;
        let nock_owner =
            read_address_call(&rpc, "Nock owner", &nock.address, "owner()", &block_tag).await?;
        let mut nodes = Vec::with_capacity(5);
        for index in 0..5u64 {
            nodes.push(
                read_address_call_with_u64(
                    &rpc, "MessageInbox bridge node", &proxy.address, "bridgeNodes(uint256)",
                    index, &block_tag,
                )
                .await?,
            );
        }
        let bridge_nodes: [String; 5] =
            nodes
                .try_into()
                .map_err(|_| ForkPreflightError::InvalidField {
                    field: "MessageInbox bridge nodes",
                    reason: "expected exactly five nodes",
                })?;
        let threshold = read_u64_call(
            &rpc, "MessageInbox threshold", &proxy.address, "THRESHOLD()", &block_tag,
        )
        .await?;
        let withdrawals_enabled = read_bool_call(
            &rpc, "MessageInbox withdrawal gate", &proxy.address, "withdrawalsEnabled()",
            &block_tag,
        )
        .await?;
        let message_inbox_nock = read_address_call(
            &rpc, "MessageInbox Nock pairing", &proxy.address, "nock()", &block_tag,
        )
        .await?;
        let nock_inbox = read_address_call(
            &rpc, "Nock inbox pairing", &nock.address, "inbox()", &block_tag,
        )
        .await?;

        Ok(PristineDeploymentFacts {
            source_block: PristineBlockFacts {
                number: current_block,
                hash: block_hash,
            },
            message_inbox_proxy,
            message_inbox_implementation,
            nock: nock_code,
            proxy_implementation,
            message_inbox_owner,
            nock_owner,
            bridge_nodes,
            threshold,
            withdrawals_enabled,
            reciprocal_pairing: ReciprocalContractPairing {
                message_inbox_nock,
                nock_inbox,
            },
            protocol: manifest.protocol.clone(),
            artifacts: manifest.artifacts.clone(),
        })
    }

    pub fn compare(
        environment: &BaseE2eEnvironment,
        facts: &PristineDeploymentFacts,
    ) -> Vec<DeploymentMismatch> {
        let manifest = environment.manifest();
        let mut mismatches = Vec::new();
        if facts.source_block.number != manifest.source_chain.fork_block.number {
            mismatches.push(DeploymentMismatch::SourceBlockNumber {
                expected: manifest.source_chain.fork_block.number,
                observed: facts.source_block.number,
            });
        }
        if facts.source_block.hash != manifest.source_chain.fork_block.hash {
            mismatches.push(DeploymentMismatch::SourceBlockHash {
                expected: manifest.source_chain.fork_block.hash.clone(),
                observed: facts.source_block.hash.clone(),
            });
        }
        compare_runtime_code(
            &mut mismatches,
            DeploymentContract::MessageInboxProxy,
            &manifest.contracts.message_inbox.proxy.address,
            &manifest
                .contracts
                .message_inbox
                .proxy
                .runtime_code_keccak256,
            &facts.message_inbox_proxy,
        );
        compare_runtime_code(
            &mut mismatches,
            DeploymentContract::MessageInboxImplementation,
            &manifest.contracts.message_inbox.implementation.address,
            &manifest
                .contracts
                .message_inbox
                .implementation
                .runtime_code_keccak256,
            &facts.message_inbox_implementation,
        );
        compare_runtime_code(
            &mut mismatches,
            DeploymentContract::Nock,
            &manifest.contracts.nock.address,
            &manifest.contracts.nock.runtime_code_keccak256,
            &facts.nock,
        );

        let expected_implementation = &manifest.contracts.message_inbox.implementation.address;
        if facts.proxy_implementation == ZERO_ADDRESS {
            mismatches.push(DeploymentMismatch::EmptyProxyImplementationSlot);
        }
        if facts.proxy_implementation != *expected_implementation {
            mismatches.push(DeploymentMismatch::ProxyImplementation {
                expected: expected_implementation.clone(),
                observed: facts.proxy_implementation.clone(),
            });
        }
        compare_string(
            &mut mismatches,
            &manifest.pristine_state.message_inbox_owner,
            &facts.message_inbox_owner,
            |expected, observed| DeploymentMismatch::MessageInboxOwner { expected, observed },
        );
        compare_string(
            &mut mismatches,
            &manifest.pristine_state.nock_owner,
            &facts.nock_owner,
            |expected, observed| DeploymentMismatch::NockOwner { expected, observed },
        );
        for index in 0..5 {
            if facts.bridge_nodes[index] != manifest.pristine_state.bridge_nodes[index] {
                mismatches.push(DeploymentMismatch::BridgeNode {
                    index,
                    expected: manifest.pristine_state.bridge_nodes[index].clone(),
                    observed: facts.bridge_nodes[index].clone(),
                });
            }
        }
        let mut seen_nodes: HashMap<&str, usize> = HashMap::with_capacity(5);
        for (index, node) in facts.bridge_nodes.iter().enumerate() {
            if let Some(first_index) = seen_nodes.insert(node, index) {
                mismatches.push(DeploymentMismatch::DuplicateBridgeNode {
                    first_index,
                    second_index: index,
                    address: node.clone(),
                });
            }
        }
        if facts.threshold != manifest.pristine_state.threshold {
            mismatches.push(DeploymentMismatch::Threshold {
                expected: manifest.pristine_state.threshold,
                observed: facts.threshold,
            });
        }
        if facts.withdrawals_enabled != manifest.pristine_state.withdrawals_enabled {
            mismatches.push(DeploymentMismatch::WithdrawalsEnabled {
                expected: manifest.pristine_state.withdrawals_enabled,
                observed: facts.withdrawals_enabled,
            });
        }
        compare_string(
            &mut mismatches,
            &manifest
                .pristine_state
                .reciprocal_pairing
                .message_inbox_nock,
            &facts.reciprocal_pairing.message_inbox_nock,
            |expected, observed| DeploymentMismatch::MessageInboxNock { expected, observed },
        );
        compare_string(
            &mut mismatches,
            &manifest.pristine_state.reciprocal_pairing.nock_inbox,
            &facts.reciprocal_pairing.nock_inbox,
            |expected, observed| DeploymentMismatch::NockInbox { expected, observed },
        );
        compare_metadata(
            &mut mismatches, "protocol.withdrawal_wire_id", &manifest.protocol.withdrawal_wire_id,
            &facts.protocol.withdrawal_wire_id,
        );
        compare_metadata(
            &mut mismatches, "protocol.withdrawal_policy_id",
            &manifest.protocol.withdrawal_policy_id, &facts.protocol.withdrawal_policy_id,
        );
        compare_artifacts(&mut mismatches, &manifest.artifacts, &facts.artifacts);
        mismatches
    }
}

fn compare_runtime_code(
    mismatches: &mut Vec<DeploymentMismatch>,
    contract: DeploymentContract,
    expected_address: &str,
    expected_hash: &str,
    observed: &RuntimeCodeFacts,
) {
    if observed.address != expected_address {
        mismatches.push(DeploymentMismatch::ContractAddress {
            contract,
            expected: expected_address.to_owned(),
            observed: observed.address.clone(),
        });
    }
    if observed.byte_len == 0 {
        mismatches.push(DeploymentMismatch::EmptyRuntimeCode {
            contract,
            address: observed.address.clone(),
        });
    }
    if observed.keccak256 != expected_hash {
        mismatches.push(DeploymentMismatch::RuntimeCodeHash {
            contract,
            expected: expected_hash.to_owned(),
            observed: observed.keccak256.clone(),
        });
    }
}

fn compare_string<F>(
    mismatches: &mut Vec<DeploymentMismatch>,
    expected: &str,
    observed: &str,
    mismatch: F,
) where
    F: FnOnce(String, String) -> DeploymentMismatch,
{
    if observed != expected {
        mismatches.push(mismatch(expected.to_owned(), observed.to_owned()));
    }
}

fn compare_metadata(
    mismatches: &mut Vec<DeploymentMismatch>,
    field_path: &str,
    expected: &str,
    observed: &str,
) {
    if observed != expected {
        mismatches.push(DeploymentMismatch::LocalMetadata {
            field_path: field_path.to_owned(),
            expected: expected.to_owned(),
            observed: observed.to_owned(),
        });
    }
}

fn compare_artifacts(
    mismatches: &mut Vec<DeploymentMismatch>,
    expected: &ContractArtifacts,
    observed: &ContractArtifacts,
) {
    compare_metadata(
        mismatches, "artifacts.hash_scheme.id", &expected.hash_scheme.id, &observed.hash_scheme.id,
    );
    compare_metadata(
        mismatches, "artifacts.hash_scheme.digest", &expected.hash_scheme.digest,
        &observed.hash_scheme.digest,
    );
    compare_metadata(
        mismatches, "artifacts.hash_scheme.encoding", &expected.hash_scheme.encoding,
        &observed.hash_scheme.encoding,
    );
    compare_metadata(
        mismatches, "artifacts.hash_scheme.canonicalization",
        &expected.hash_scheme.canonicalization, &observed.hash_scheme.canonicalization,
    );
    if expected.hash_scheme.artifact_fields != observed.hash_scheme.artifact_fields {
        mismatches.push(DeploymentMismatch::LocalMetadata {
            field_path: "artifacts.hash_scheme.artifact_fields".to_owned(),
            expected: expected.hash_scheme.artifact_fields.join(","),
            observed: observed.hash_scheme.artifact_fields.join(","),
        });
    }
    compare_artifact(
        mismatches, "artifacts.erc1967_proxy", &expected.erc1967_proxy, &observed.erc1967_proxy,
    );
    compare_artifact(
        mismatches, "artifacts.message_inbox", &expected.message_inbox, &observed.message_inbox,
    );
    compare_artifact(mismatches, "artifacts.nock", &expected.nock, &observed.nock);
}

fn compare_artifact(
    mismatches: &mut Vec<DeploymentMismatch>,
    prefix: &str,
    expected: &ContractArtifactIdentity,
    observed: &ContractArtifactIdentity,
) {
    for (field, expected, observed) in [
        (
            "contract_name", &expected.contract_name, &observed.contract_name,
        ),
        (
            "compiler_version", &expected.compiler_version, &observed.compiler_version,
        ),
        (
            "verified_artifact_sha256", &expected.verified_artifact_sha256,
            &observed.verified_artifact_sha256,
        ),
        ("abi_sha256", &expected.abi_sha256, &observed.abi_sha256),
        (
            "verification_url", &expected.verification_url, &observed.verification_url,
        ),
    ] {
        compare_metadata(mismatches, &format!("{prefix}.{field}"), expected, observed);
    }
}

async fn read_runtime_code(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    address: &str,
    block_tag: &str,
) -> Result<RuntimeCodeFacts, ForkPreflightError> {
    let code: String = read_rpc(rpc, field, "eth_getCode", json!([address, block_tag])).await?;
    let bytes = decode_hex(field, &code)?;
    Ok(RuntimeCodeFacts {
        address: address.to_owned(),
        byte_len: bytes.len(),
        keccak256: format!("{:#x}", keccak256(&bytes)),
    })
}

async fn read_address_call(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    contract: &str,
    signature: &str,
    block_tag: &str,
) -> Result<String, ForkPreflightError> {
    let output = eth_call(rpc, field, contract, selector(signature), block_tag).await?;
    decode_address_word(field, &output)
}

async fn read_address_call_with_u64(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    contract: &str,
    signature: &str,
    argument: u64,
    block_tag: &str,
) -> Result<String, ForkPreflightError> {
    let data = format!("{}{:064x}", selector(signature), argument);
    let output = eth_call(rpc, field, contract, data, block_tag).await?;
    decode_address_word(field, &output)
}

async fn read_u64_call(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    contract: &str,
    signature: &str,
    block_tag: &str,
) -> Result<u64, ForkPreflightError> {
    let output = eth_call(rpc, field, contract, selector(signature), block_tag).await?;
    let word = decode_word(field, &output)?;
    if word[..24].iter().any(|byte| *byte != 0) {
        return Err(ForkPreflightError::InvalidField {
            field,
            reason: "uint256 does not fit u64",
        });
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&word[24..]);
    Ok(u64::from_be_bytes(bytes))
}

async fn read_bool_call(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    contract: &str,
    signature: &str,
    block_tag: &str,
) -> Result<bool, ForkPreflightError> {
    match read_u64_call(rpc, field, contract, signature, block_tag).await? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ForkPreflightError::InvalidField {
            field,
            reason: "boolean word is neither zero nor one",
        }),
    }
}

async fn eth_call(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    contract: &str,
    data: String,
    block_tag: &str,
) -> Result<String, ForkPreflightError> {
    read_rpc(
        rpc,
        field,
        "eth_call",
        json!([{ "to": contract, "data": data }, block_tag]),
    )
    .await
}

async fn read_rpc<T>(
    rpc: &ReadOnlyRpc,
    field: &'static str,
    method: &'static str,
    params: serde_json::Value,
) -> Result<T, ForkPreflightError>
where
    T: serde::de::DeserializeOwned,
{
    rpc.call(method, params)
        .await
        .map_err(|source| ForkPreflightError::ReadField { field, source })
}

fn selector(signature: &str) -> String {
    let hash = keccak256(signature.as_bytes());
    format!("0x{}", hex::encode(&hash[..4]))
}

fn decode_address_word(field: &'static str, value: &str) -> Result<String, ForkPreflightError> {
    let word = decode_word(field, value)?;
    if word[..12].iter().any(|byte| *byte != 0) {
        return Err(ForkPreflightError::InvalidField {
            field,
            reason: "address word has nonzero prefix",
        });
    }
    Ok(format!("0x{}", hex::encode(&word[12..])))
}

fn decode_word(field: &'static str, value: &str) -> Result<[u8; 32], ForkPreflightError> {
    let bytes = decode_hex(field, value)?;
    bytes
        .try_into()
        .map_err(|_| ForkPreflightError::InvalidField {
            field,
            reason: "expected one 32-byte ABI word",
        })
}

fn decode_quantity(field: &'static str, value: &str) -> Result<u64, ForkPreflightError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(ForkPreflightError::InvalidField {
            field,
            reason: "quantity is not 0x-prefixed",
        })?;
    u64::from_str_radix(digits, 16).map_err(|_| ForkPreflightError::InvalidField {
        field,
        reason: "quantity does not fit u64",
    })
}

fn normalize_hash(field: &'static str, value: &str) -> Result<String, ForkPreflightError> {
    let bytes = decode_hex(field, value)?;
    if bytes.len() != 32 {
        return Err(ForkPreflightError::InvalidField {
            field,
            reason: "expected a 32-byte hash",
        });
    }
    Ok(format!("0x{}", hex::encode(bytes)))
}

fn decode_hex(field: &'static str, value: &str) -> Result<Vec<u8>, ForkPreflightError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(ForkPreflightError::InvalidField {
            field,
            reason: "hex value is not 0x-prefixed",
        })?;
    hex::decode(digits).map_err(|_| ForkPreflightError::InvalidField {
        field,
        reason: "hex value is malformed",
    })
}

#[derive(Deserialize)]
struct RpcBlock {
    hash: Option<String>,
}
