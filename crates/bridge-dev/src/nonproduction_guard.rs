use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bridge::shared::e2e_environment::{
    BaseSepoliaE2eManifest, BASE_SEPOLIA_E2E_CHAIN_ID, BASE_SEPOLIA_SOURCE_CHAIN_ID,
};
use reqwest::{redirect, Client, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

const BASE_MAINNET_CHAIN_ID: u64 = 8_453;
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const DENIED_HOST_SUFFIXES: &[&str] = &[
    "base.org", "publicnode.com", "tenderly.co", "alchemy.com", "alchemyapi.io", "infura.io",
    "quicknode.com", "drpc.org", "ankr.com",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackBaseRpcUrl(Url);

impl LoopbackBaseRpcUrl {
    pub fn parse(raw: &str) -> Result<Self, NonproductionGuardError> {
        let url = Url::parse(raw).map_err(|_| NonproductionGuardError::MalformedEndpoint)?;
        if url.scheme() != "http" {
            return Err(NonproductionGuardError::HttpSchemeRequired);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(NonproductionGuardError::CredentialsNotAllowed);
        }
        if url.query().is_some() {
            return Err(NonproductionGuardError::QueryNotAllowed);
        }
        if url.fragment().is_some() {
            return Err(NonproductionGuardError::FragmentNotAllowed);
        }
        if url.path() != "/" {
            return Err(NonproductionGuardError::RootPathRequired);
        }
        if url.port().is_none() {
            return Err(NonproductionGuardError::ExplicitPortRequired);
        }

        let host = url.host_str().ok_or(NonproductionGuardError::MissingHost)?;
        if denied_host(host) {
            return Err(NonproductionGuardError::DeniedHost {
                host: host.to_owned(),
            });
        }
        if !literal_loopback_host(host) {
            return Err(NonproductionGuardError::LoopbackHostRequired {
                host: host.to_owned(),
            });
        }

        Ok(Self(url))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub(crate) fn read_only_rpc(&self) -> Result<ReadOnlyRpc, NonproductionGuardError> {
        let client = Client::builder()
            .no_proxy()
            .redirect(redirect::Policy::none())
            .timeout(RPC_TIMEOUT)
            .build()
            .map_err(NonproductionGuardError::ClientBuild)?;
        Ok(ReadOnlyRpc::new(client, self.0.clone()))
    }
}

impl fmt::Display for LoopbackBaseRpcUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BaseManifestBinding {
    pub schema_id: String,
    pub schema_version: u64,
    pub environment_id: String,
    pub source_chain_id: u64,
    pub local_chain_id: u64,
    pub message_inbox_proxy: String,
    pub message_inbox_implementation: String,
    pub nock: String,
}

impl BaseManifestBinding {
    fn from_manifest(manifest: &BaseSepoliaE2eManifest) -> Self {
        Self {
            schema_id: manifest.schema_id.clone(),
            schema_version: manifest.schema_version,
            environment_id: manifest.environment_id.clone(),
            source_chain_id: manifest.source_chain.chain_id,
            local_chain_id: manifest.local_fork.chain_id,
            message_inbox_proxy: manifest.contracts.message_inbox.proxy.address.clone(),
            message_inbox_implementation: manifest
                .contracts
                .message_inbox
                .implementation
                .address
                .clone(),
            nock: manifest.contracts.nock.address.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnvilCapabilityFacts {
    pub client_version: String,
    pub chain_id: u64,
    pub current_block_number: u64,
    pub snapshot_round_trip: bool,
}

#[derive(Debug, Clone)]
pub struct GuardedBaseRpc {
    endpoint: LoopbackBaseRpcUrl,
    manifest: BaseManifestBinding,
    capabilities: AnvilCapabilityFacts,
}

impl GuardedBaseRpc {
    pub fn endpoint(&self) -> &LoopbackBaseRpcUrl {
        &self.endpoint
    }

    pub fn manifest(&self) -> &BaseManifestBinding {
        &self.manifest
    }

    pub fn capabilities(&self) -> &AnvilCapabilityFacts {
        &self.capabilities
    }
}

#[derive(Debug, Error)]
pub enum NonproductionGuardError {
    #[error("Base RPC endpoint is missing or malformed")]
    MalformedEndpoint,
    #[error("Base RPC endpoint must use plain HTTP on loopback")]
    HttpSchemeRequired,
    #[error("Base RPC endpoint must not contain credentials")]
    CredentialsNotAllowed,
    #[error("Base RPC endpoint must not contain a query string")]
    QueryNotAllowed,
    #[error("Base RPC endpoint must not contain a fragment")]
    FragmentNotAllowed,
    #[error("Base RPC endpoint must use the root path")]
    RootPathRequired,
    #[error("Base RPC endpoint must include an explicit port")]
    ExplicitPortRequired,
    #[error("Base RPC endpoint has no host")]
    MissingHost,
    #[error("Base RPC host {host:?} is explicitly denied")]
    DeniedHost { host: String },
    #[error("Base RPC host {host:?} is not a literal loopback host")]
    LoopbackHostRequired { host: String },
    #[error("Base E2E manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("requested environment {requested:?} does not match manifest {manifest:?}")]
    EnvironmentMismatch { requested: String, manifest: String },
    #[error("failed to construct the read-only Base RPC probe")]
    ClientBuild(#[source] reqwest::Error),
    #[error("read-only Base RPC request failed during {method}")]
    Request {
        method: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Base RPC method {method} returned JSON-RPC error {code}")]
    JsonRpc { method: &'static str, code: i64 },
    #[error("Base RPC method {method} returned an invalid response: {reason}")]
    InvalidResponse {
        method: &'static str,
        reason: &'static str,
    },
    #[error("refusing live Base chain id {0}")]
    LiveBaseChain(u64),
    #[error("Base RPC chain id mismatch: expected {expected}, observed {observed}")]
    ChainIdMismatch { expected: u64, observed: u64 },
    #[error("Anvil node-info chain id mismatch: expected {expected}, observed {observed}")]
    NodeInfoChainIdMismatch { expected: u64, observed: u64 },
    #[error("Base RPC endpoint failed Anvil capability proof: {0}")]
    Capability(&'static str),
}

pub struct NonproductionGuard;

impl NonproductionGuard {
    pub async fn acquire(
        raw_endpoint: &str,
        requested_environment_id: &str,
        manifest: &BaseSepoliaE2eManifest,
    ) -> Result<GuardedBaseRpc, NonproductionGuardError> {
        let endpoint = LoopbackBaseRpcUrl::parse(raw_endpoint)?;
        manifest
            .validate()
            .map_err(|error| NonproductionGuardError::InvalidManifest(error.to_string()))?;
        if requested_environment_id != manifest.environment_id {
            return Err(NonproductionGuardError::EnvironmentMismatch {
                requested: requested_environment_id.to_owned(),
                manifest: manifest.environment_id.clone(),
            });
        }
        if manifest.local_fork.chain_id != BASE_SEPOLIA_E2E_CHAIN_ID
            || manifest.source_chain.chain_id != BASE_SEPOLIA_SOURCE_CHAIN_ID
        {
            return Err(NonproductionGuardError::InvalidManifest(
                "manifest chain identities do not match the Base Sepolia E2E profile".to_owned(),
            ));
        }

        let rpc = endpoint.read_only_rpc()?;

        let chain_id_raw: String = rpc.call("eth_chainId", json!([])).await?;
        let chain_id = parse_quantity("eth_chainId", &chain_id_raw)?;
        validate_observed_chain_id(chain_id, manifest.local_fork.chain_id)?;

        let client_version: String = rpc.call("web3_clientVersion", json!([])).await?;
        if !client_version.to_ascii_lowercase().starts_with("anvil/") {
            return Err(NonproductionGuardError::Capability(
                "web3_clientVersion does not identify Anvil",
            ));
        }

        let node_info: AnvilNodeInfo = rpc.call("anvil_nodeInfo", json!([])).await?;
        let node_info_chain_id = node_info.environment.chain_id.into_u64("anvil_nodeInfo")?;
        if node_info_chain_id != chain_id {
            return Err(NonproductionGuardError::NodeInfoChainIdMismatch {
                expected: chain_id,
                observed: node_info_chain_id,
            });
        }
        let current_block_number =
            parse_quantity("anvil_nodeInfo", &node_info.current_block_number)?;

        prove_snapshot_round_trip(&rpc).await?;

        Ok(GuardedBaseRpc {
            endpoint,
            manifest: BaseManifestBinding::from_manifest(manifest),
            capabilities: AnvilCapabilityFacts {
                client_version,
                chain_id,
                current_block_number,
                snapshot_round_trip: true,
            },
        })
    }
}

fn validate_observed_chain_id(observed: u64, expected: u64) -> Result<(), NonproductionGuardError> {
    if matches!(
        observed,
        BASE_MAINNET_CHAIN_ID | BASE_SEPOLIA_SOURCE_CHAIN_ID
    ) {
        return Err(NonproductionGuardError::LiveBaseChain(observed));
    }
    if observed != expected {
        return Err(NonproductionGuardError::ChainIdMismatch { expected, observed });
    }
    Ok(())
}

fn denied_host(host: &str) -> bool {
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    DENIED_HOST_SUFFIXES
        .iter()
        .any(|suffix| normalized == *suffix || normalized.ends_with(&format!(".{suffix}")))
}

fn literal_loopback_host(host: &str) -> bool {
    let normalized = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }
    normalized
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn parse_quantity(method: &'static str, value: &str) -> Result<u64, NonproductionGuardError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(NonproductionGuardError::InvalidResponse {
            method,
            reason: "quantity is not 0x-prefixed",
        })?;
    if digits.is_empty() {
        return Err(NonproductionGuardError::InvalidResponse {
            method,
            reason: "quantity is empty",
        });
    }
    u64::from_str_radix(digits, 16).map_err(|_| NonproductionGuardError::InvalidResponse {
        method,
        reason: "quantity does not fit u64",
    })
}

fn validate_snapshot_id(value: &str) -> Result<(), NonproductionGuardError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(NonproductionGuardError::Capability(
            "snapshot id is not 0x-prefixed",
        ))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NonproductionGuardError::Capability(
            "snapshot id is not hexadecimal",
        ));
    }
    Ok(())
}

async fn prove_snapshot_round_trip(rpc: &ReadOnlyRpc) -> Result<(), NonproductionGuardError> {
    let first: String = rpc.call("evm_snapshot", json!([])).await?;
    validate_snapshot_id(&first)?;

    let second: String = match rpc.call("evm_snapshot", json!([])).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let _ = revert_snapshot(rpc, &first).await;
            return Err(error);
        }
    };
    if let Err(error) = validate_snapshot_id(&second) {
        let _ = revert_snapshot(rpc, &first).await;
        return Err(error);
    }
    if first == second {
        let _ = revert_snapshot(rpc, &second).await;
        return Err(NonproductionGuardError::Capability(
            "nested snapshots returned the same id",
        ));
    }

    if !revert_snapshot(rpc, &second).await? {
        let _ = revert_snapshot(rpc, &first).await;
        return Err(NonproductionGuardError::Capability(
            "nested snapshot could not be reverted",
        ));
    }
    if revert_snapshot(rpc, &second).await? {
        let _ = revert_snapshot(rpc, &first).await;
        return Err(NonproductionGuardError::Capability(
            "consumed snapshot could be reverted twice",
        ));
    }
    if !revert_snapshot(rpc, &first).await? {
        return Err(NonproductionGuardError::Capability(
            "parent snapshot could not be reverted",
        ));
    }
    Ok(())
}

async fn revert_snapshot(
    rpc: &ReadOnlyRpc,
    snapshot_id: &str,
) -> Result<bool, NonproductionGuardError> {
    rpc.call("evm_revert", json!([snapshot_id])).await
}

pub(crate) struct ReadOnlyRpc {
    client: Client,
    endpoint: Url,
    next_id: AtomicU64,
}

impl ReadOnlyRpc {
    fn new(client: Client, endpoint: Url) -> Self {
        Self {
            client,
            endpoint,
            next_id: AtomicU64::new(1),
        }
    }

    pub(crate) async fn call<T>(
        &self,
        method: &'static str,
        params: Value,
    ) -> Result<T, NonproductionGuardError>
    where
        T: DeserializeOwned,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&request)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|source| NonproductionGuardError::Request { method, source })?;
        let response: Value = response
            .json()
            .await
            .map_err(|source| NonproductionGuardError::Request { method, source })?;
        let object = response
            .as_object()
            .ok_or(NonproductionGuardError::InvalidResponse {
                method,
                reason: "JSON-RPC response is not an object",
            })?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || object.get("id").and_then(Value::as_u64) != Some(id)
        {
            return Err(NonproductionGuardError::InvalidResponse {
                method,
                reason: "JSON-RPC version or request id mismatch",
            });
        }
        if let Some(error) = object.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(-1);
            return Err(NonproductionGuardError::JsonRpc { method, code });
        }
        let result = object
            .get("result")
            .ok_or(NonproductionGuardError::InvalidResponse {
                method,
                reason: "missing result",
            })?;
        serde_json::from_value(result.clone()).map_err(|_| {
            NonproductionGuardError::InvalidResponse {
                method,
                reason: "result has an unexpected shape",
            }
        })
    }
}

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnvilNodeInfo {
    current_block_number: String,
    environment: AnvilEnvironment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnvilEnvironment {
    chain_id: RpcQuantity,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RpcQuantity {
    Number(u64),
    Hex(String),
}

impl RpcQuantity {
    fn into_u64(self, method: &'static str) -> Result<u64, NonproductionGuardError> {
        match self {
            Self::Number(value) => Ok(value),
            Self::Hex(value) => parse_quantity(method, &value),
        }
    }
}
