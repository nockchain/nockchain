use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::keccak256;
use bridge::shared::e2e_environment::{
    BaseSepoliaE2eManifest, DeployedContractIdentity, BASE_SEPOLIA_SOURCE_CHAIN_ID,
};
use reqwest::{redirect, Client, Url};
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::anvil::{AnvilBackend, AnvilConfig, AnvilEvidenceFacts, AnvilStartError};
use crate::environment::BaseE2eEnvironment;
use crate::fork_preflight::{
    ForkPreflight, ForkPreflightError, PristineDeploymentFacts, VerifiedPristineFork,
};

const SOURCE_RPC_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct PinnedForkConfig {
    pub source_rpc_url: String,
    pub anvil_binary: Option<PathBuf>,
    pub port: Option<u16>,
    pub startup_timeout: Duration,
}

impl PinnedForkConfig {
    pub fn new(source_rpc_url: String) -> Self {
        Self {
            source_rpc_url,
            anvil_binary: None,
            port: None,
            startup_timeout: Duration::from_secs(60),
        }
    }
}

impl fmt::Debug for PinnedForkConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedForkConfig")
            .field("source_rpc_url", &"<redacted>")
            .field("anvil_binary", &self.anvil_binary)
            .field("port", &self.port)
            .field("startup_timeout", &self.startup_timeout)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRpcIdentity {
    pub scheme: String,
    pub endpoint_keccak256: String,
    pub source_chain_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinnedForkEvidence {
    pub source_rpc: SourceRpcIdentity,
    pub source_block_number: u64,
    pub source_block_hash: String,
    pub anvil: AnvilEvidenceFacts,
    pub pristine: PristineDeploymentFacts,
}

pub struct PinnedAnvilFork {
    backend: AnvilBackend,
    pristine: VerifiedPristineFork,
    evidence: PinnedForkEvidence,
}

impl PinnedAnvilFork {
    pub async fn start(
        config: PinnedForkConfig,
        environment: &BaseE2eEnvironment,
    ) -> Result<Self, PinnedForkError> {
        let source_identity =
            probe_source_archive(&config.source_rpc_url, environment.manifest()).await?;
        let manifest = environment.manifest();
        let mut anvil_config = AnvilConfig::fork(
            config.source_rpc_url, manifest.source_chain.fork_block.number,
        );
        anvil_config.port = config.port;
        anvil_config.startup_timeout = config.startup_timeout;
        if let Some(binary) = config.anvil_binary {
            anvil_config.binary = binary;
        }
        let backend = AnvilBackend::start_unverified_fork(anvil_config, environment).await?;
        let pristine = match ForkPreflight::verify(backend.http_url(), environment).await {
            Ok(pristine) => pristine,
            Err(source) => {
                let output = backend.captured_output();
                let _ = backend.shutdown().await;
                return Err(PinnedForkError::Preflight { source, output });
            }
        };
        let evidence = PinnedForkEvidence {
            source_rpc: source_identity,
            source_block_number: manifest.source_chain.fork_block.number,
            source_block_hash: manifest.source_chain.fork_block.hash.clone(),
            anvil: backend.facts().clone(),
            pristine: pristine.facts().clone(),
        };
        Ok(Self {
            backend,
            pristine,
            evidence,
        })
    }

    pub fn backend(&self) -> &AnvilBackend {
        &self.backend
    }

    pub fn pristine(&self) -> &VerifiedPristineFork {
        &self.pristine
    }

    pub fn evidence(&self) -> &PinnedForkEvidence {
        &self.evidence
    }

    pub async fn shutdown(self) -> Result<(), crate::anvil::AnvilShutdownError> {
        self.backend.shutdown().await
    }
}

#[derive(Debug, Error)]
pub enum PinnedForkError {
    #[error("source RPC endpoint is missing or malformed")]
    InvalidSourceEndpoint,
    #[error("source RPC must use HTTP or HTTPS")]
    InvalidSourceScheme,
    #[error("source RPC request failed while checking {check}")]
    SourceUnavailable { check: &'static str },
    #[error("source RPC chain id mismatch: expected {expected}, observed {observed}")]
    SourceChainIdMismatch { expected: u64, observed: u64 },
    #[error("source RPC pinned block hash mismatch: expected {expected}, observed {observed}")]
    SourceBlockHashMismatch { expected: String, observed: String },
    #[error("source RPC has no historical code for {contract} at the pinned block")]
    SourceArchiveUnavailable { contract: &'static str },
    #[error("source RPC historical code mismatch for {contract}: expected {expected}, observed {observed}")]
    SourceCodeMismatch {
        contract: &'static str,
        expected: String,
        observed: String,
    },
    #[error(transparent)]
    Anvil(#[from] AnvilStartError),
    #[error("pinned fork failed pristine preflight; Anvil output:\n{output}")]
    Preflight {
        #[source]
        source: ForkPreflightError,
        output: String,
    },
}

async fn probe_source_archive(
    raw_url: &str,
    manifest: &BaseSepoliaE2eManifest,
) -> Result<SourceRpcIdentity, PinnedForkError> {
    let url = Url::parse(raw_url).map_err(|_| PinnedForkError::InvalidSourceEndpoint)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(PinnedForkError::InvalidSourceScheme);
    }
    let client = Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(SOURCE_RPC_TIMEOUT)
        .build()
        .map_err(|_| PinnedForkError::SourceUnavailable {
            check: "client construction",
        })?;
    let rpc = SourceRpc { client, url };
    let chain_id: String = rpc
        .call("source chain id", "eth_chainId", json!([]))
        .await?;
    let chain_id = decode_quantity(&chain_id).ok_or(PinnedForkError::SourceUnavailable {
        check: "source chain id",
    })?;
    if chain_id != BASE_SEPOLIA_SOURCE_CHAIN_ID {
        return Err(PinnedForkError::SourceChainIdMismatch {
            expected: BASE_SEPOLIA_SOURCE_CHAIN_ID,
            observed: chain_id,
        });
    }

    let block_tag = format!("0x{:x}", manifest.source_chain.fork_block.number);
    let block: Value = rpc
        .call(
            "pinned block",
            "eth_getBlockByNumber",
            json!([block_tag, false]),
        )
        .await?;
    let observed_hash = block
        .get("hash")
        .and_then(Value::as_str)
        .and_then(normalize_hash)
        .ok_or(PinnedForkError::SourceUnavailable {
            check: "pinned block",
        })?;
    if observed_hash != manifest.source_chain.fork_block.hash {
        return Err(PinnedForkError::SourceBlockHashMismatch {
            expected: manifest.source_chain.fork_block.hash.clone(),
            observed: observed_hash,
        });
    }

    for (contract, deployment) in [
        (
            "MessageInbox proxy", &manifest.contracts.message_inbox.proxy,
        ),
        (
            "MessageInbox implementation", &manifest.contracts.message_inbox.implementation,
        ),
        ("Nock", &manifest.contracts.nock),
    ] {
        verify_source_code(&rpc, contract, deployment, &block_tag).await?;
    }

    Ok(SourceRpcIdentity {
        scheme: rpc.url.scheme().to_owned(),
        endpoint_keccak256: format!("{:#x}", keccak256(raw_url.as_bytes())),
        source_chain_id: chain_id,
    })
}

async fn verify_source_code(
    rpc: &SourceRpc,
    contract: &'static str,
    deployment: &DeployedContractIdentity,
    block_tag: &str,
) -> Result<(), PinnedForkError> {
    let code: String = rpc
        .call(
            contract,
            "eth_getCode",
            json!([deployment.address, block_tag]),
        )
        .await?;
    let bytes = decode_hex(&code).ok_or(PinnedForkError::SourceUnavailable { check: contract })?;
    if bytes.is_empty() {
        return Err(PinnedForkError::SourceArchiveUnavailable { contract });
    }
    let observed = format!("{:#x}", keccak256(bytes));
    if observed != deployment.runtime_code_keccak256 {
        return Err(PinnedForkError::SourceCodeMismatch {
            contract,
            expected: deployment.runtime_code_keccak256.clone(),
            observed,
        });
    }
    Ok(())
}

struct SourceRpc {
    client: Client,
    url: Url,
}

impl SourceRpc {
    async fn call<T>(
        &self,
        check: &'static str,
        method: &'static str,
        params: Value,
    ) -> Result<T, PinnedForkError>
    where
        T: serde::de::DeserializeOwned,
    {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response: Value = self
            .client
            .post(self.url.clone())
            .json(&request)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|_| PinnedForkError::SourceUnavailable { check })?
            .json()
            .await
            .map_err(|_| PinnedForkError::SourceUnavailable { check })?;
        if response.get("error").is_some() {
            return Err(PinnedForkError::SourceUnavailable { check });
        }
        let result = response
            .get("result")
            .ok_or(PinnedForkError::SourceUnavailable { check })?;
        serde_json::from_value(result.clone())
            .map_err(|_| PinnedForkError::SourceUnavailable { check })
    }
}

fn decode_quantity(value: &str) -> Option<u64> {
    u64::from_str_radix(value.strip_prefix("0x")?, 16).ok()
}

fn normalize_hash(value: &str) -> Option<String> {
    let bytes = decode_hex(value)?;
    (bytes.len() == 32).then(|| format!("0x{}", hex::encode(bytes)))
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    hex::decode(value.strip_prefix("0x")?).ok()
}
