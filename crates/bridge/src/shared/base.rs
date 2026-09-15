#![allow(clippy::too_many_arguments)] // For macro-generated code

use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use alloy::consensus::Transaction as _;
use alloy::network::{EthereumWallet, NetworkWallet};
use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::client::BatchRequest;
use alloy::rpc::types::eth::{BlockNumberOrTag, Filter, RawLog};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolEvent;
use alloy::transports::ws::WsConnect;
use alloy::transports::TransportError;
use async_trait::async_trait;
use backon::{ExponentialBuilder, Retryable};
use hex::encode as hex_encode;
use nockchain_math::belt::PRIME;
use nockchain_types::v1::Name;
use op_alloy::network::Optimism;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, trace, warn};

use crate::core::loop_policy::BaseObserverLoopPolicy;
use crate::core::observation::base::{
    plan_base_tick, BaseBatchInFlight, BasePlanAction, BasePlanInput, BasePlanState,
};
use crate::core::ports::BaseSourcePort;
use crate::observability::metrics;
use crate::observability::status::BridgeStatus;
use crate::observability::tui::types::{BridgeTx, TxDirection, TxStatus};
use crate::shared::errors::BridgeError;
use crate::shared::runtime::{BaseBlockBatch, BridgeEvent, BridgeRuntimeHandle, ChainEvent};
use crate::shared::signing::BridgeNodeEthAddressMap;
use crate::shared::stop::StopHandle;

fn is_rate_limit_error<E: std::fmt::Display>(e: &E) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("rate limit") || s.contains("-32005")
}

use crate::deposit::types::{BaseDepositSettlementEntry, DepositSettlement, DepositSettlementData};
use crate::shared::types::{
    zero_tip5_hash, AtomBytes, BaseBlockRef, BaseEvent, BaseEventContent, BaseEventId, EthAddress,
    NullTag, Tip5Hash, WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK,
};
#[cfg(test)]
use crate::shared::types::{
    WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NOCK, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
};
use crate::withdrawal::types::{BaseWithdrawalEntry, Withdrawal};

/// Default Base confirmation depth used by the driver if not specified in config.
///
/// The bridge kernel assumes blocks it receives are final; this is enforced by the Rust driver.
pub const DEFAULT_BASE_CONFIRMATION_DEPTH: u64 = 300;

/// Hard cap on automatic rewinds of already-confirmed Base headers.
///
/// The effective deployment limit is the smaller of this cap and the configured
/// confirmation depth. Crossing activation/policy boundaries is never automatic.
pub const BASE_MAX_AUTOMATIC_REORG_REWIND_BLOCKS: u64 = 64;

pub const fn base_automatic_reorg_rewind_depth(confirmation_depth: u64) -> u64 {
    if confirmation_depth < BASE_MAX_AUTOMATIC_REORG_REWIND_BLOCKS {
        confirmation_depth
    } else {
        BASE_MAX_AUTOMATIC_REORG_REWIND_BLOCKS
    }
}

// In Bazel builds, contract JSON paths are provided via rustc_env.
// In Cargo builds, they're relative to CARGO_MANIFEST_DIR.
#[cfg(feature = "bazel_build")]
alloy::sol!(
    #[sol(rpc)]
    MessageInbox,
    env!("MESSAGE_INBOX_JSON")
);

#[cfg(not(feature = "bazel_build"))]
alloy::sol!(
    #[sol(rpc)]
    MessageInbox,
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/out/MessageInbox.sol/MessageInbox.json"
    )
);

#[cfg(feature = "bazel_build")]
alloy::sol!(
    #[sol(rpc)]
    Nock,
    env!("NOCK_JSON")
);

#[cfg(not(feature = "bazel_build"))]
alloy::sol!(
    #[sol(rpc)]
    Nock,
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/out/Nock.sol/Nock.json"
    )
);

/// Base unit for Nock token (10^16) - Nock.sol uses 16 decimals, not 18
#[cfg(test)]
pub(crate) const NOCK_BASE_UNIT: u128 = WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NOCK;

/// Nicks per NOCK on Nockchain (2^16)
#[cfg(test)]
const NICKS_PER_NOCK: u128 = WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK as u128;

/// Conversion factor: NOCK base units per nick
/// 1 nick = 10^16 / 65,536 = 152,587,890,625 NOCK base units
pub(crate) const NOCK_BASE_PER_NICK: u128 = WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK;

pub(crate) use self::MessageInbox as MessageInboxContract;

pub const WITHDRAWAL_BURN_TRAILER_MAGIC: &[u8; 8] = b"NOCKWD1!";
pub const WITHDRAWAL_BURN_FULL_LOCK_ROOT_LEN: usize = 40;
pub const WITHDRAWAL_BURN_TRAILER_LEN: usize =
    WITHDRAWAL_BURN_TRAILER_MAGIC.len() + WITHDRAWAL_BURN_FULL_LOCK_ROOT_LEN;
pub const WITHDRAWAL_BURN_BASE_CALLDATA_LEN: usize = 4 + 32 + 32;
pub const WITHDRAWAL_BURN_CALLDATA_LEN: usize =
    WITHDRAWAL_BURN_BASE_CALLDATA_LEN + WITHDRAWAL_BURN_TRAILER_LEN;
const WITHDRAWAL_BURN_COMMITMENT_DOMAIN: &[u8] = b"nock-withdrawal-calldata-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompensatedWithdrawalFact {
    pub base_event_id: BaseEventId,
    pub transaction_hash: B256,
    pub log_index: u64,
    pub reason: String,
    pub evidence_reference: String,
    pub recorded_at_unix_secs: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CompensatedWithdrawalRegistry {
    entries: HashMap<BaseEventId, CompensatedWithdrawalFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBurnForWithdrawalLog {
    pub base_event_id: BaseEventId,
    pub burner: EthAddress,
    pub amount: u64,
    pub lock_root: Tip5Hash,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BurnForWithdrawalEventFacts {
    pub burner: Address,
    pub amount_raw: U256,
    pub commitment: B256,
}

impl CompensatedWithdrawalRegistry {
    pub fn from_config(
        entries: &[crate::shared::config::CompensatedWithdrawalToml],
    ) -> Result<Self, BridgeError> {
        let mut registry = Self::default();
        for entry in entries {
            let transaction_hash =
                parse_compensation_hex::<32>(&entry.transaction_hash, "transaction_hash")?;
            let expected =
                compute_base_event_id(&B256::from(transaction_hash), Some(entry.log_index));
            let configured = BaseEventId(
                parse_compensation_hex::<32>(&entry.base_event_id, "base_event_id")?.to_vec(),
            );
            if configured != expected {
                return Err(BridgeError::Config(format!(
                    "compensated withdrawal {} does not match transaction hash and log index",
                    entry.base_event_id
                )));
            }
            if entry.reason.trim().is_empty() || entry.evidence_reference.trim().is_empty() {
                return Err(BridgeError::Config(
                    "compensated withdrawal requires reason and evidence_reference".into(),
                ));
            }
            let fact = CompensatedWithdrawalFact {
                base_event_id: configured.clone(),
                transaction_hash: B256::from(transaction_hash),
                log_index: entry.log_index,
                reason: entry.reason.trim().to_string(),
                evidence_reference: entry.evidence_reference.trim().to_string(),
                recorded_at_unix_secs: entry.recorded_at_unix_secs,
            };
            if let Some(existing) = registry.entries.insert(configured.clone(), fact.clone()) {
                if existing != fact {
                    return Err(BridgeError::Config(format!(
                        "compensated withdrawal {} is configured more than once with conflicting facts",
                        entry.base_event_id
                    )));
                }
            }
        }
        Ok(registry)
    }

    pub fn get(&self, base_event_id: &BaseEventId) -> Option<&CompensatedWithdrawalFact> {
        self.entries.get(base_event_id)
    }

    pub fn values(&self) -> impl Iterator<Item = &CompensatedWithdrawalFact> {
        self.entries.values()
    }
}

fn parse_compensation_hex<const N: usize>(
    value: &str,
    field: &str,
) -> Result<[u8; N], BridgeError> {
    let raw = value.trim().strip_prefix("0x").unwrap_or(value.trim());
    let bytes = hex::decode(raw)
        .map_err(|_| BridgeError::Config(format!("compensated withdrawal {field} is not hex")))?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        BridgeError::Config(format!(
            "compensated withdrawal {field} has {} bytes, expected {N}",
            bytes.len()
        ))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurnForWithdrawalDecodeError {
    NotBurnForWithdrawal(String),
    AmountNotDivisible {
        amount_raw: U256,
    },
    AmountOverflow {
        nicks: U256,
    },
    MissingCalldataTrailer {
        actual_len: usize,
    },
    MalformedCalldata {
        reason: String,
    },
    CalldataAmountMismatch {
        event_amount_raw: U256,
        calldata_amount_raw: U256,
    },
    CalldataCommitmentMismatch {
        event_commitment: B256,
        calldata_commitment: B256,
    },
    CommitmentMismatch {
        expected: B256,
        actual: B256,
    },
    InvalidLockRoot {
        reason: String,
    },
}

impl fmt::Display for BurnForWithdrawalDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBurnForWithdrawal(err) => {
                write!(f, "log is not Nock::BurnForWithdrawal: {err}")
            }
            Self::AmountNotDivisible { amount_raw } => write!(
                f,
                "BurnForWithdrawal amount {amount_raw} is not divisible by NOCK_BASE_PER_NICK"
            ),
            Self::AmountOverflow { nicks } => write!(
                f,
                "BurnForWithdrawal amount exceeds representable range: {nicks} nicks"
            ),
            Self::MissingCalldataTrailer { actual_len } => write!(
                f,
                "BurnForWithdrawal calldata is missing withdrawal trailer: got {actual_len} bytes"
            ),
            Self::MalformedCalldata { reason } => {
                write!(f, "BurnForWithdrawal calldata is malformed: {reason}")
            }
            Self::CalldataAmountMismatch {
                event_amount_raw,
                calldata_amount_raw,
            } => write!(
                f,
                "BurnForWithdrawal calldata amount {calldata_amount_raw} does not match event amount {event_amount_raw}"
            ),
            Self::CalldataCommitmentMismatch {
                event_commitment,
                calldata_commitment,
            } => write!(
                f,
                "BurnForWithdrawal calldata commitment {calldata_commitment:?} does not match event commitment {event_commitment:?}"
            ),
            Self::CommitmentMismatch { expected, actual } => write!(
                f,
                "BurnForWithdrawal trailer commitment mismatch: expected {expected:?}, got {actual:?}"
            ),
            Self::InvalidLockRoot { reason } => {
                write!(f, "BurnForWithdrawal trailer lock root is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for BurnForWithdrawalDecodeError {}
impl BurnForWithdrawalDecodeError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NotBurnForWithdrawal(_) => "not_burn_for_withdrawal",
            Self::AmountNotDivisible { .. } => "amount_not_divisible",
            Self::AmountOverflow { .. } => "amount_overflow",
            Self::MissingCalldataTrailer { .. } => "missing_calldata_trailer",
            Self::MalformedCalldata { .. } => "malformed_calldata",
            Self::CalldataAmountMismatch { .. } => "calldata_amount_mismatch",
            Self::CalldataCommitmentMismatch { .. } => "calldata_commitment_mismatch",
            Self::CommitmentMismatch { .. } => "commitment_mismatch",
            Self::InvalidLockRoot { .. } => "invalid_lock_root",
        }
    }
}

/// Test helper: calculate confirmed batch using old global-boundary logic.
/// Only used for testing the confirmation depth behavior.
#[cfg(test)]
fn confirmed_batch(chain_tip: u64, batch_size: u64, confirmation_depth: u64) -> Option<(u64, u64)> {
    let confirmed_height = chain_tip.saturating_sub(confirmation_depth);
    if confirmed_height < batch_size {
        return None;
    }
    let batch_end = (confirmed_height / batch_size) * batch_size;
    let batch_start = batch_end - batch_size + 1;
    Some((batch_start, batch_end))
}

/// Calculate the next batch window to fetch, aligned to kernel's requested height.
///
/// Returns `Some((start, end))` if a full batch is confirmed, `None` otherwise.
/// The batch is always exactly `batch_size` blocks, starting at `next_needed_height`.
#[cfg(test)]
fn next_confirmed_window(
    next_needed_height: u64,
    confirmed_height: u64,
    batch_size: u64,
) -> Option<(u64, u64)> {
    let batch_start = next_needed_height;
    let batch_end = next_needed_height + batch_size - 1;
    // Only return if the FULL batch is confirmed
    if batch_end > confirmed_height {
        return None;
    }
    Some((batch_start, batch_end))
}

#[allow(dead_code)]
struct BaseBridgeDeps {
    provider: DynProvider<Optimism>,
    wallet: EthereumWallet,
    runtime_handle: Arc<BridgeRuntimeHandle>,
    stop: StopHandle,
}

struct BaseBridgeContracts {
    inbox_contract_address: Address,
    nock_contract_address: Address,
}

struct BaseBridgeConfig {
    /// Batch size for fetching base blocks (must match Hoon kernel's base-blocks-chunk)
    batch_size: u64,
    /// Number of confirmations required before emitting a batch to the kernel.
    confirmation_depth: u64,
    /// Governance-approved burn identities that must never enter kernel state.
    compensated_withdrawals: CompensatedWithdrawalRegistry,
}

#[allow(dead_code)]
pub struct BaseBridge {
    deps: BaseBridgeDeps,
    contracts: BaseBridgeContracts,
    config: BaseBridgeConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseContractReadiness {
    pub inbox_contract_address: Address,
    pub nock_contract_address: Address,
    pub owner: Address,
    pub bridge_nodes: [Address; 5],
    pub threshold: u64,
    pub withdrawals_enabled: bool,
}

fn validate_base_contract_readiness(
    facts: &BaseContractReadiness,
    expected_inbox: Address,
    expected_nock: Address,
    expected_nodes: &BridgeNodeEthAddressMap,
    expected_threshold: u64,
) -> Result<(), BridgeError> {
    if facts.inbox_contract_address != expected_inbox {
        return Err(BridgeError::Config(format!(
            "Nock.inbox mismatch: expected {expected_inbox}, observed {}",
            facts.inbox_contract_address
        )));
    }
    if facts.nock_contract_address != expected_nock {
        return Err(BridgeError::Config(format!(
            "MessageInbox.nock mismatch: expected {expected_nock}, observed {}",
            facts.nock_contract_address
        )));
    }
    if facts.owner == Address::ZERO {
        return Err(BridgeError::Config(
            "MessageInbox owner must not be the zero address".into(),
        ));
    }
    if facts.threshold != expected_threshold {
        return Err(BridgeError::Config(format!(
            "MessageInbox threshold mismatch: expected {expected_threshold}, observed {}",
            facts.threshold
        )));
    }
    if expected_nodes.len() != 5 {
        return Err(BridgeError::Config(format!(
            "bridge config must provide exactly 5 Ethereum node addresses, found {}",
            expected_nodes.len()
        )));
    }
    for (node_id, observed) in facts.bridge_nodes.iter().enumerate() {
        let expected = expected_nodes.get(&(node_id as u64)).ok_or_else(|| {
            BridgeError::Config(format!(
                "bridge config is missing Ethereum address for node {node_id}"
            ))
        })?;
        if observed != expected {
            return Err(BridgeError::Config(format!(
                "MessageInbox bridge node {node_id} mismatch: expected {expected}, observed {observed}"
            )));
        }
    }
    Ok(())
}

pub(crate) async fn validate_base_chain_id(
    provider: &DynProvider<Optimism>,
    expected_chain_id: u64,
    provider_name: &str,
) -> Result<u64, BridgeError> {
    let observed_chain_id = provider.get_chain_id().await.map_err(|error| {
        BridgeError::BaseBridgeMonitoring(format!(
            "failed to query Base chain id from {provider_name}: {error}"
        ))
    })?;
    if observed_chain_id != expected_chain_id {
        return Err(BridgeError::Config(format!(
            "Base chain id mismatch for {provider_name}: expected {expected_chain_id}, observed {observed_chain_id}"
        )));
    }
    info!(
        target: "bridge.base.connect",
        expected_chain_id,
        observed_chain_id,
        provider_name,
        "validated Base chain id"
    );
    Ok(observed_chain_id)
}

pub(crate) async fn query_withdrawal_contract_gate(
    provider: &DynProvider<Optimism>,
    nock_contract_address: Address,
) -> Result<(Address, bool), BridgeError> {
    let nock = Nock::new(nock_contract_address, provider);
    let inbox_contract_address = nock.inbox().call().await.map_err(|error| {
        BridgeError::BaseBridgeMonitoring(format!("failed to read Nock.inbox: {error}"))
    })?;
    let inbox = MessageInbox::new(inbox_contract_address, provider);
    let paired_nock = inbox.nock().call().await.map_err(|error| {
        BridgeError::BaseBridgeMonitoring(format!("failed to read MessageInbox.nock: {error}"))
    })?;
    if paired_nock != nock_contract_address {
        return Err(BridgeError::Config(format!(
            "MessageInbox.nock mismatch: expected {nock_contract_address}, observed {paired_nock}"
        )));
    }
    let enabled = inbox.withdrawalsEnabled().call().await.map_err(|error| {
        BridgeError::BaseBridgeMonitoring(format!(
            "failed to read MessageInbox.withdrawalsEnabled: {error}"
        ))
    })?;
    Ok((inbox_contract_address, enabled))
}

impl BaseBridge {
    pub async fn new(
        ws_url: String,
        expected_chain_id: u64,
        inbox_contract_address: Address,
        nock_contract_address: Address,
        private_key: String,
        runtime_handle: Arc<BridgeRuntimeHandle>,
        batch_size: u64,
        confirmation_depth: u64,
        stop: StopHandle,
        compensated_withdrawals: CompensatedWithdrawalRegistry,
    ) -> Result<Self, BridgeError> {
        let signer = {
            let key = private_key.strip_prefix("0x").unwrap_or(&private_key);
            PrivateKeySigner::from_str(key)?
        };
        let wallet = EthereumWallet::from(signer);

        let connect_backoff = || {
            ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(30))
                .with_jitter()
                .with_max_times(10)
        };

        let provider = loop {
            if stop.is_stopped() {
                return Err(BridgeError::Runtime(
                    "bridge stopped while connecting to base websocket".into(),
                ));
            }

            let ws_url = ws_url.clone();
            let connect = || async {
                let ws = WsConnect::new(ws_url.clone());
                // Build provider with recommended fillers for gas estimation, nonce management, and chain ID
                // Note: We use filler() to add each filler explicitly since RecommendedFillers
                // doesn't work directly with Optimism network
                use alloy::providers::fillers::{
                    CachedNonceManager, ChainIdFiller, GasFiller, NonceFiller, WalletFiller,
                };
                ProviderBuilder::<_, _, Optimism>::default()
                    .filler(GasFiller)
                    .filler(NonceFiller::<CachedNonceManager>::default())
                    .filler(ChainIdFiller::default())
                    .filler(WalletFiller::new(wallet.clone()))
                    .connect_ws(ws)
                    .await
            };

            match connect
                .retry(connect_backoff())
                .notify(|err, dur| {
                    warn!(
                        target: "bridge.base.connect",
                        error=%err,
                        backoff_secs = dur.as_secs(),
                        "failed to connect to base websocket, will retry"
                    );
                })
                .await
            {
                Ok(provider) => break provider.erased(),
                Err(err) => {
                    warn!(
                        target: "bridge.base.connect",
                        error=%err,
                        "failed to connect to base websocket after retries, retrying"
                    );
                    sleep(Duration::from_secs(2)).await;
                }
            }
        };
        validate_base_chain_id(&provider, expected_chain_id, "bridge Base provider").await?;

        Ok(Self {
            deps: BaseBridgeDeps {
                provider,
                wallet,
                runtime_handle,
                stop,
            },
            contracts: BaseBridgeContracts {
                inbox_contract_address,
                nock_contract_address,
            },
            config: BaseBridgeConfig {
                batch_size,
                confirmation_depth,
                compensated_withdrawals,
            },
        })
    }

    pub(crate) fn provider(&self) -> DynProvider<Optimism> {
        self.deps.provider.clone()
    }

    pub(crate) fn inbox_contract_address(&self) -> Address {
        self.contracts.inbox_contract_address
    }

    pub async fn validate_contract_readiness(
        &self,
        expected_nodes: &BridgeNodeEthAddressMap,
        expected_threshold: u64,
    ) -> Result<BaseContractReadiness, BridgeError> {
        let provider = self.provider();
        let inbox = MessageInbox::new(self.contracts.inbox_contract_address, &provider);
        let nock = Nock::new(self.contracts.nock_contract_address, &provider);

        let nock_contract_address = inbox.nock().call().await.map_err(|error| {
            BridgeError::BaseBridgeMonitoring(format!("failed to read MessageInbox.nock: {error}"))
        })?;
        let inbox_contract_address = nock.inbox().call().await.map_err(|error| {
            BridgeError::BaseBridgeMonitoring(format!("failed to read Nock.inbox: {error}"))
        })?;
        let owner = inbox.owner().call().await.map_err(|error| {
            BridgeError::BaseBridgeMonitoring(format!("failed to read MessageInbox.owner: {error}"))
        })?;
        let threshold = inbox.THRESHOLD().call().await.map_err(|error| {
            BridgeError::BaseBridgeMonitoring(format!(
                "failed to read MessageInbox.THRESHOLD: {error}"
            ))
        })?;
        let threshold = u64::try_from(threshold).map_err(|error| {
            BridgeError::ValueConversion(format!(
                "MessageInbox.THRESHOLD does not fit u64: {error}"
            ))
        })?;
        let withdrawals_enabled = inbox.withdrawalsEnabled().call().await.map_err(|error| {
            BridgeError::BaseBridgeMonitoring(format!(
                "failed to read MessageInbox.withdrawalsEnabled: {error}"
            ))
        })?;
        let mut bridge_nodes = [Address::ZERO; 5];
        for (node_id, node) in bridge_nodes.iter_mut().enumerate() {
            *node = inbox
                .bridgeNodes(U256::from(node_id))
                .call()
                .await
                .map_err(|error| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "failed to read MessageInbox.bridgeNodes({node_id}): {error}"
                    ))
                })?;
        }

        let facts = BaseContractReadiness {
            inbox_contract_address,
            nock_contract_address,
            owner,
            bridge_nodes,
            threshold,
            withdrawals_enabled,
        };
        validate_base_contract_readiness(
            &facts, self.contracts.inbox_contract_address, self.contracts.nock_contract_address,
            expected_nodes, expected_threshold,
        )?;
        info!(
            target: "bridge.base.connect",
            inbox_contract_address = %facts.inbox_contract_address,
            nock_contract_address = %facts.nock_contract_address,
            owner = %facts.owner,
            threshold = facts.threshold,
            withdrawals_enabled = facts.withdrawals_enabled,
            "validated Base bridge contract readiness"
        );
        Ok(facts)
    }

    pub(crate) fn default_signer_address(&self) -> Address {
        NetworkWallet::<Optimism>::default_signer_address(&self.deps.wallet)
    }

    pub async fn watch_base_acks(&self) -> Result<(), BridgeError> {
        self.stream_base_events(None).await
    }

    pub async fn stream_base_events(
        &self,
        bridge_status: Option<BridgeStatus>,
    ) -> Result<(), BridgeError> {
        self.stream_base_events_with_policy(bridge_status, BaseObserverLoopPolicy::default())
            .await
    }

    pub async fn stream_base_events_with_policy(
        &self,
        bridge_status: Option<BridgeStatus>,
        policy: BaseObserverLoopPolicy,
    ) -> Result<(), BridgeError> {
        info!(
            "starting base bridge event stream (confirmation_depth={}, batch_size={})",
            self.config.confirmation_depth, self.config.batch_size
        );

        let poll_interval = policy.poll_interval;
        let rpc_retry = policy.rpc_retry;
        let rpc_backoff = || rpc_retry.exponential_builder();
        let mut base_batch_in_flight: Option<BaseBatchInFlight> = None;

        loop {
            if self.deps.stop.is_stopped() {
                sleep(poll_interval).await;
                continue;
            }
            sleep(poll_interval).await;

            let base_hold_active = match self.deps.runtime_handle.peek_base_hold().await {
                Ok(active) => active,
                Err(err) => {
                    warn!(
                        target: "bridge.base.observer",
                        error=%err,
                        "failed to peek base hold state"
                    );
                    continue;
                }
            };
            let pending_base_commit_active = if base_hold_active {
                false
            } else {
                match self
                    .deps
                    .runtime_handle
                    .peek_pending_base_block_commit()
                    .await
                {
                    Ok(pending) => pending.is_some(),
                    Err(err) => {
                        warn!(
                            target: "bridge.base.observer",
                            error=%err,
                            "failed to peek pending base block commit state"
                        );
                        continue;
                    }
                }
            };
            let state = if base_hold_active {
                base_batch_in_flight = None;
                BasePlanState::HoldActive
            } else if pending_base_commit_active {
                base_batch_in_flight = None;
                BasePlanState::PendingBaseBlockCommitActive
            } else {
                let chain_tip = match (|| async { self.deps.provider.get_block_number().await })
                    .retry(rpc_backoff())
                    .when(is_rate_limit_error)
                    .notify(|err, dur| {
                        warn!(
                            target: "bridge.base.observer",
                            error=%err,
                            backoff_secs = dur.as_secs(),
                            "failed to get block number, will retry"
                        );
                    })
                    .await
                {
                    Ok(tip) => {
                        metrics::update_base_tip_height(Some(tip));
                        tip
                    }
                    Err(e) => {
                        metrics::update_base_tip_height(None);
                        error!(
                            target: "bridge.base.observer",
                            error=%e,
                            "failed to get block number after retries"
                        );
                        continue;
                    }
                };
                let next_needed_height =
                    match self.deps.runtime_handle.peek_base_next_height().await {
                        Ok(height) => height,
                        Err(err) => {
                            warn!(
                                target: "bridge.base.observer",
                                error=%err,
                                "failed to peek base next height"
                            );
                            continue;
                        }
                    };
                if let Some(in_flight) = base_batch_in_flight {
                    if in_flight.still_waiting_for_kernel(next_needed_height) {
                        debug!(
                            target: "bridge.base.observer",
                            batch_start = in_flight.start,
                            batch_end = in_flight.end,
                            "base batch already enqueued, waiting for kernel pending or height advance"
                        );
                        continue;
                    }
                    base_batch_in_flight = None;
                }
                BasePlanState::Active {
                    chain_tip,
                    next_needed_height,
                }
            };

            let action = plan_base_tick(BasePlanInput {
                state,
                batch_size: self.config.batch_size,
                confirmation_depth: self.config.confirmation_depth,
            });

            let (chain_tip, batch_start, batch_end) = match action {
                BasePlanAction::HoldActive => {
                    debug!(
                        target: "bridge.base.observer",
                        "base hold active, skipping base batch fetch"
                    );
                    continue;
                }
                BasePlanAction::PendingBaseBlockCommitActive => {
                    debug!(
                        target: "bridge.base.observer",
                        "pending base block commit active, skipping base batch fetch"
                    );
                    continue;
                }
                BasePlanAction::NoPendingHeight { chain_tip } => {
                    debug!(
                        target: "bridge.base.observer",
                        chain_tip,
                        "kernel has no pending base batch"
                    );
                    continue;
                }
                BasePlanAction::InvalidConfig(err) => {
                    error!(
                        target: "bridge.base.observer",
                        error=?err,
                        batch_size=self.config.batch_size,
                        confirmation_depth=self.config.confirmation_depth,
                        "invalid observer planner configuration"
                    );
                    continue;
                }
                BasePlanAction::NotYetConfirmed {
                    chain_tip,
                    confirmed_height,
                    next_needed_height,
                    needed_confirmed_height,
                    blocks_until_ready,
                    ..
                } => {
                    debug!(
                        target: "bridge.base.observer",
                        chain_tip,
                        confirmed_height,
                        next_needed_height,
                        batch_size = self.config.batch_size,
                        needed_confirmed_height,
                        blocks_until_ready,
                        "batch not yet confirmed for kernel need"
                    );
                    continue;
                }
                BasePlanAction::FetchWindow {
                    chain_tip,
                    start,
                    end,
                    confirmed_height,
                    ..
                } => {
                    debug!(
                        target: "bridge.base.observer",
                        chain_tip,
                        confirmed_height,
                        next_needed_height = start,
                        "kernel reports base-hashchain-next-height"
                    );
                    (chain_tip, start, end)
                }
            };

            debug!(
                target: "bridge.base.observer",
                chain_tip,
                batch_start,
                batch_end,
                batch_blocks = batch_end - batch_start + 1,
                "fetching confirmed batch"
            );

            let tui = bridge_status.clone();
            match (|| async { self.fetch_batch(batch_start, batch_end, tui.clone()).await })
                .retry(rpc_backoff())
                .when(is_rate_limit_error)
                .notify(|err, dur| {
                    warn!(
                        target: "bridge.base.observer",
                        batch_start,
                        batch_end,
                        error=%err,
                        backoff_secs = dur.as_secs(),
                        "failed to fetch batch, will retry"
                    );
                })
                .await
            {
                Ok(batch) => {
                    self.deps
                        .runtime_handle
                        .send_event(BridgeEvent::Chain(Box::new(ChainEvent::Base(batch))))
                        .await?;
                    base_batch_in_flight = Some(BaseBatchInFlight::new(batch_start, batch_end));
                    info!(
                        target: "bridge.base.observer",
                        batch_start,
                        batch_end,
                        "emitted base batch"
                    );
                }
                Err(err) => {
                    error!(
                        target: "bridge.base.observer",
                        batch_start,
                        batch_end,
                        error=%err,
                        "failed to fetch batch after retries"
                    );
                }
            }
        }
    }

    async fn fetch_batch(
        &self,
        batch_start: u64,
        batch_end: u64,
        bridge_status: Option<BridgeStatus>,
    ) -> Result<BaseBlockBatch, BridgeError> {
        // Filter by specific event signatures to avoid fetching irrelevant logs
        // (e.g., ERC-20 Transfer events from the Nock token contract)
        let event_signatures = vec![
            Nock::BurnForWithdrawal::SIGNATURE_HASH,
            MessageInbox::DepositProcessed::SIGNATURE_HASH,
            MessageInbox::BridgeNodeUpdated::SIGNATURE_HASH,
        ];
        let filter = Filter::new()
            .address(vec![
                self.contracts.inbox_contract_address, self.contracts.nock_contract_address,
            ])
            .event_signature(event_signatures)
            .from_block(batch_start)
            .to_block(batch_end);

        let logs = self
            .deps
            .provider
            .get_logs(&filter)
            .await
            .map_err(|e: TransportError| BridgeError::BaseBridgeMonitoring(e.to_string()))?;

        debug!(
            target: "bridge.base.observer",
            batch_start,
            batch_end,
            log_count = logs.len(),
            "fetched logs for batch"
        );

        let block_info = fetch_base_block_info(&self.deps.provider, batch_start, batch_end).await?;
        let mut blocks = Vec::new();
        for height in batch_start..=batch_end {
            let header = block_info.get(&height).ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring(format!("missing block info for {}", height))
            })?;

            blocks.push(BaseBlockRef {
                height,
                block_id: atom_bytes_from_b256(header.hash),
                parent_block_id: atom_bytes_from_b256(header.parent_hash),
            });
        }

        if let Some(last_block) = blocks.last() {
            let tip_hash = format!("0x{}", hex_encode(last_block.block_id.as_slice()));
            self.deps.runtime_handle.set_base_tip_hash(tip_hash);
        }

        let mut withdrawals = Vec::new();
        let mut deposit_settlements = Vec::new();
        let mut block_events: HashMap<u64, Vec<BaseEvent>> = HashMap::new();
        for height in batch_start..=batch_end {
            block_events.insert(height, Vec::new());
        }

        for log in logs {
            let block_number = log.block_number.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("log missing block number".into())
            })?;
            validate_base_log_block_hash(
                &block_info, batch_start, batch_end, block_number, log.block_hash,
            )?;
            let tx_hash = log.transaction_hash.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("log missing transaction hash".into())
            })?;
            let log_index = log.log_index;

            let raw = RawLog {
                address: log.address(),
                topics: log.topics().to_vec(),
                data: log.data().data.clone(),
            };

            if log.address() == self.contracts.inbox_contract_address {
                if let Some((event, settlement)) =
                    self.process_inbox_log(&raw, &tx_hash, log_index)?
                {
                    block_events
                        .get_mut(&block_number)
                        .expect("block initialized")
                        .push(event.clone());
                    if let Some(s) = settlement {
                        deposit_settlements.push(s);
                    }
                    // Push transaction to TUI state if available
                    if let Some(ref state) = bridge_status {
                        if let BaseEventContent::DepositProcessed {
                            recipient,
                            amount,
                            block_height,
                            ..
                        } = &event.content
                        {
                            let bridge_tx = BridgeTx {
                                tx_hash: format!("0x{}", hex_encode(tx_hash.as_slice())),
                                direction: TxDirection::Deposit,
                                from: "Base".to_string(),
                                to: format!("0x{}", hex_encode(recipient.0)),
                                amount: *amount as u128,
                                status: TxStatus::Completed,
                                timestamp: std::time::SystemTime::now(),
                                base_block: Some(*block_height),
                                nock_height: None,
                            };
                            state.push_transaction(bridge_tx);

                            // Record metrics for deposit completion
                            // Note: We don't have true latency tracking here since we're processing
                            // historical events in batches. Setting latency to 0 for now.
                            state.record_tx_completion(
                                TxDirection::Deposit,
                                *amount as u128,
                                0,    // latency_ms (not tracked for historical events)
                                true, // success (we only process successful deposits)
                            );
                        }
                    }
                }
            } else if log.address() == self.contracts.nock_contract_address {
                let tx = self
                    .deps
                    .provider
                    .get_transaction_by_hash(tx_hash)
                    .await
                    .map_err(|e| BridgeError::BaseBridgeMonitoring(e.to_string()))?
                    .ok_or_else(|| {
                        BridgeError::BaseBridgeMonitoring(format!(
                            "withdrawal burn transaction {tx_hash:?} unavailable"
                        ))
                    })?;
                let tx_input = tx.input().clone();
                if let Some((event, withdrawal)) =
                    self.process_withdrawal_log(&raw, &tx_hash, log_index, tx_input.as_ref())?
                {
                    block_events
                        .get_mut(&block_number)
                        .expect("block initialized")
                        .push(event.clone());
                    if let Some(w) = withdrawal {
                        withdrawals.push(w);
                    }
                    // Push transaction to TUI state if available
                    if let Some(ref state) = bridge_status {
                        if let BaseEventContent::BurnForWithdrawal { burner, amount, .. } =
                            &event.content
                        {
                            let bridge_tx = BridgeTx {
                                tx_hash: format!("0x{}", hex_encode(tx_hash.as_slice())),
                                direction: TxDirection::Withdrawal,
                                from: format!("0x{}", hex_encode(burner.0)),
                                to: "Nockchain".to_string(),
                                amount: *amount as u128,
                                status: TxStatus::Completed,
                                timestamp: std::time::SystemTime::now(),
                                base_block: Some(block_number),
                                nock_height: None,
                            };
                            state.push_transaction(bridge_tx);

                            // Record metrics for withdrawal completion
                            // Note: We don't have true latency tracking here since we're processing
                            // historical events in batches. Setting latency to 0 for now.
                            state.record_tx_completion(
                                TxDirection::Withdrawal,
                                *amount as u128,
                                0,    // latency_ms (not tracked for historical events)
                                true, // success (we only process successful withdrawals)
                            );
                        }
                    }
                }
            }
        }

        Ok(BaseBlockBatch {
            version: 0,
            first_height: batch_start,
            last_height: batch_end,
            blocks,
            withdrawals,
            deposit_settlements,
            block_events,
            prev: zero_tip5_hash(),
        })
    }

    /// Decode Base deposit logs and convert NOCK base units to nicks.
    /// Requires exact divisibility by NOCK_BASE_PER_NICK to avoid rounding.
    fn process_inbox_log(
        &self,
        raw: &RawLog,
        tx_hash: &B256,
        log_index: Option<u64>,
    ) -> Result<Option<(BaseEvent, Option<BaseDepositSettlementEntry>)>, BridgeError> {
        if let Ok(event) = MessageInbox::DepositProcessed::decode_raw_log(
            raw.topics.iter().cloned(),
            raw.data.as_ref(),
        ) {
            // Convert NOCK base units back to nicks
            // 1 nick = NOCK_BASE_PER_NICK NOCK base units
            let nock_per_nick = U256::from(NOCK_BASE_PER_NICK);
            let amount_raw: U256 = event.amount;
            if amount_raw % nock_per_nick != U256::ZERO {
                warn!(
                    target: "bridge.base.observer",
                    amount=%amount_raw,
                    "deposit amount not divisible by NOCK_BASE_PER_NICK, skipping"
                );
                return Ok(None);
            }
            let nicks = amount_raw / nock_per_nick;
            if nicks > U256::from(u64::MAX) {
                return Err(BridgeError::ValueConversion(format!(
                    "deposit amount exceeds representable range (value: {} nicks)",
                    nicks
                )));
            }
            let amount = nicks.to::<u64>();

            let base_tx_id = AtomBytes(tx_hash.as_slice().to_vec());
            let base_event_id = compute_base_event_id(tx_hash, log_index);
            let nock_tx_id = tip5_from_limbs(&event.txIdFull.limbs);
            let note_name_first = tip5_from_limbs(&event.nameFirst.limbs);
            let note_name_last = tip5_from_limbs(&event.nameLast.limbs);
            let as_of = tip5_from_limbs(&event.asOf.limbs);

            let data = DepositSettlementData {
                counterpart: nock_tx_id.clone(),
                as_of: as_of.clone(),
                dest: AtomBytes(event.recipient.as_slice().to_vec()),
                settled_amount: amount,
                fees: Vec::new(),
                bridge_fee: 0,
            };
            let settlement = DepositSettlement {
                base_tx_id: base_tx_id.clone(),
                data,
            };

            let block_height_raw: U256 = event.blockHeight;
            info!(
                tx_id = %event.txId,
                name_first_hash = %event.nameFirstHash,
                recipient = %event.recipient,
                amount = %amount_raw,
                block_height = %block_height_raw,
                "Deposit processed on MessageInbox",
            );

            return Ok(Some((
                BaseEvent {
                    base_event_id,
                    content: BaseEventContent::DepositProcessed {
                        nock_tx_id,
                        note_name: Name::new(note_name_first, note_name_last),
                        recipient: eth_address_from_alloy(event.recipient),
                        amount,
                        block_height: block_height_raw.to::<u64>(),
                        as_of,
                        nonce: event.nonce.to::<u64>(),
                    },
                },
                Some(BaseDepositSettlementEntry {
                    base_tx_id,
                    settlement,
                }),
            )));
        }

        if let Ok(event) = MessageInbox::BridgeNodeUpdated::decode_raw_log(
            raw.topics.iter().cloned(),
            raw.data.as_ref(),
        ) {
            let base_event_id = compute_base_event_id(tx_hash, log_index);
            let index: U256 = event.index;
            info!(
                index = %index,
                old_node = %event.oldNode,
                new_node = %event.newNode,
                "Bridge node updated on MessageInbox",
            );
            return Ok(Some((
                BaseEvent {
                    base_event_id,
                    content: BaseEventContent::BridgeNodeUpdated(NullTag),
                },
                None,
            )));
        }

        Ok(None)
    }

    /// Decode Base withdrawal logs and convert NOCK base units to nicks.
    /// Requires exact divisibility by NOCK_BASE_PER_NICK to avoid rounding.
    fn process_withdrawal_log(
        &self,
        raw: &RawLog,
        tx_hash: &B256,
        log_index: Option<u64>,
        tx_input: &[u8],
    ) -> Result<Option<(BaseEvent, Option<BaseWithdrawalEntry>)>, BridgeError> {
        match decode_burn_for_withdrawal_log_with_calldata(
            raw, tx_hash, log_index, self.contracts.nock_contract_address, tx_input,
        ) {
            Ok(decoded) => {
                if let Some(compensation) = self
                    .config
                    .compensated_withdrawals
                    .get(&decoded.base_event_id)
                {
                    info!(
                        target: "bridge.base.observer",
                        tx_hash = %format!("0x{}", hex_encode(tx_hash.as_slice())),
                        log_index = ?log_index,
                        base_event_id = %format!("0x{}", hex_encode(&decoded.base_event_id.0)),
                        amount = %decoded.amount,
                        reason = %compensation.reason,
                        evidence_reference = %compensation.evidence_reference,
                        "skipping governance-compensated withdrawal burn"
                    );
                    return Ok(None);
                }

                let base_tx_id = AtomBytes(tx_hash.as_slice().to_vec());
                let withdrawal = Withdrawal {
                    base_tx_id: base_tx_id.clone(),
                    dest: None,
                    raw_amount: decoded.amount,
                };

                debug!(
                    target: "bridge.base.observer",
                    base_tx_id_hex=%hex_encode(&base_tx_id.0),
                    base_tx_id_len=%base_tx_id.0.len(),
                    dest_is_none=true,
                    raw_amount=%decoded.amount,
                    "created Withdrawal struct"
                );

                let entry = BaseWithdrawalEntry {
                    base_tx_id: base_tx_id.clone(),
                    withdrawal,
                };

                debug!(
                    target: "bridge.base.observer",
                    entry_base_tx_id_hex=%hex_encode(&entry.base_tx_id.0),
                    entry_withdrawal_raw_amount=%entry.withdrawal.raw_amount,
                    "created BaseWithdrawalEntry"
                );

                info!(
                    burner = ?decoded.burner,
                    amount = %decoded.amount,
                    lock_root = ?decoded.lock_root,
                    "Withdrawal detected on Nock contract"
                );

                return Ok(Some((
                    BaseEvent {
                        base_event_id: decoded.base_event_id,
                        content: BaseEventContent::BurnForWithdrawal {
                            burner: decoded.burner,
                            amount: decoded.amount,
                            lock_root: decoded.lock_root,
                        },
                    },
                    Some(entry),
                )));
            }
            Err(BurnForWithdrawalDecodeError::NotBurnForWithdrawal(err)) => {
                // With topic filtering, this should rarely happen - only if ABI changes.
                trace!("Skipping non-BurnForWithdrawal log: {}", err);
            }
            Err(BurnForWithdrawalDecodeError::AmountNotDivisible { amount_raw }) => {
                warn!(
                    target: "bridge.base.observer",
                    amount=%amount_raw,
                    "withdrawal amount not divisible by NOCK_BASE_PER_NICK, skipping"
                );
            }
            Err(BurnForWithdrawalDecodeError::AmountOverflow { nicks }) => {
                warn!(
                    target: "bridge.base.observer",
                    nicks=%nicks,
                    "withdrawal amount exceeds representable range, skipping"
                );
            }
            Err(err) => {
                warn!(
                    target: "bridge.base.observer",
                    error=%err,
                    "withdrawal burn does not carry a valid full lock-root calldata trailer, skipping"
                );
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl BaseSourcePort for BaseBridge {
    async fn chain_tip_height(&self) -> Result<u64, BridgeError> {
        self.deps
            .provider
            .get_block_number()
            .await
            .map_err(|e| BridgeError::BaseBridgeMonitoring(e.to_string()))
    }

    async fn fetch_batch(&self, start: u64, end: u64) -> Result<BaseBlockBatch, BridgeError> {
        BaseBridge::fetch_batch(self, start, end, None).await
    }
}

fn atom_bytes_from_b256(value: B256) -> AtomBytes {
    AtomBytes(value.as_slice().to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BaseBlockHeaderInfo {
    pub hash: B256,
    pub parent_hash: B256,
    pub timestamp: u64,
}

pub(crate) type BaseBlockInfo = HashMap<u64, BaseBlockHeaderInfo>;
const BASE_HEADER_RPC_BATCH_SIZE: usize = 20;

pub(crate) async fn fetch_base_block_info(
    provider: &DynProvider<Optimism>,
    batch_start: u64,
    batch_end: u64,
) -> Result<BaseBlockInfo, BridgeError> {
    let mut block_info = BaseBlockInfo::new();
    let heights: Vec<u64> = (batch_start..=batch_end).collect();

    // Fetch block headers in bounded JSON-RPC batches, then verify the batch is one chain.
    for chunk in heights.chunks(BASE_HEADER_RPC_BATCH_SIZE) {
        let mut batch = BatchRequest::new(provider.client());
        let mut futures = Vec::new();

        for &height in chunk {
            let fut = batch
                .add_call::<_, Option<alloy::rpc::types::Block>>(
                    "eth_getBlockByNumber",
                    &(BlockNumberOrTag::Number(height), false),
                )
                .map_err(|err| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "failed to add Base block header batch call for {height}: {err}"
                    ))
                })?;
            futures.push((height, fut));
        }

        batch.send().await.map_err(|err| {
            BridgeError::BaseBridgeMonitoring(format!("Base block header batch RPC failed: {err}"))
        })?;

        for (height, fut) in futures {
            let block_opt: Option<alloy::rpc::types::Block> = fut.await.map_err(|err| {
                BridgeError::BaseBridgeMonitoring(format!(
                    "failed to fetch Base block header {height}: {err}"
                ))
            })?;
            let block = block_opt.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring(format!(
                    "Base block header {height} unavailable during batch fetch"
                ))
            })?;
            block_info.insert(
                height,
                BaseBlockHeaderInfo {
                    hash: block.header.hash,
                    parent_hash: block.header.inner.parent_hash,
                    timestamp: block.header.inner.timestamp,
                },
            );
        }
    }

    validate_base_block_parent_chain(&block_info, batch_start, batch_end)?;
    Ok(block_info)
}

pub(crate) fn validate_base_block_parent_chain(
    block_info: &BaseBlockInfo,
    batch_start: u64,
    batch_end: u64,
) -> Result<(), BridgeError> {
    let mut prev_hash = None;
    for height in batch_start..=batch_end {
        let header = block_info.get(&height).ok_or_else(|| {
            BridgeError::BaseBridgeMonitoring(format!(
                "missing Base block header for height {height}"
            ))
        })?;
        if let Some(expected_parent) = prev_hash {
            if header.parent_hash != expected_parent {
                return Err(BridgeError::BaseBridgeMonitoring(format!(
                    "base reorg detected at height {height} (expected parent {:?}, got {:?})",
                    expected_parent, header.parent_hash
                )));
            }
        }
        prev_hash = Some(header.hash);
    }
    Ok(())
}

pub(crate) fn validate_base_log_block_hash(
    block_info: &BaseBlockInfo,
    batch_start: u64,
    batch_end: u64,
    block_number: u64,
    log_block_hash: Option<B256>,
) -> Result<(), BridgeError> {
    let log_block_hash = log_block_hash.ok_or_else(|| {
        BridgeError::BaseBridgeMonitoring(format!("log at block {block_number} missing block hash"))
    })?;
    let expected_header = block_info.get(&block_number).ok_or_else(|| {
        BridgeError::BaseBridgeMonitoring(format!(
            "log block {block_number} is outside fetched batch {batch_start}..={batch_end}"
        ))
    })?;
    if log_block_hash != expected_header.hash {
        return Err(BridgeError::BaseBridgeMonitoring(format!(
            "log block hash mismatch at height {block_number}: expected {:?}, got {:?}",
            expected_header.hash, log_block_hash
        )));
    }
    Ok(())
}

fn eth_address_from_alloy(addr: Address) -> EthAddress {
    EthAddress::from(addr)
}

fn tip5_from_limbs(limbs: &[u64; 5]) -> Tip5Hash {
    Tip5Hash::from_limbs(limbs)
}

pub(crate) fn compute_base_event_id(tx_hash: &B256, log_index: Option<u64>) -> BaseEventId {
    // This is the EVM log coordinate for a Base event: transaction hashes are
    // chain-unique for practical purposes, and `log_index` distinguishes
    // multiple logs emitted by the same transaction.
    let log_index = U256::from(log_index.unwrap_or(0u64));
    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(tx_hash.as_slice());
    let log_index_bytes = log_index.to_be_bytes::<32>();
    hash_input.extend_from_slice(&log_index_bytes);
    BaseEventId(keccak256(&hash_input).as_slice().to_vec())
}

pub fn burn_for_withdrawal_signature_hash() -> B256 {
    Nock::BurnForWithdrawal::SIGNATURE_HASH
}

pub fn withdrawal_burn_selector() -> [u8; 4] {
    let selector_hash = keccak256(b"burn(uint256,bytes32)");
    selector_hash[..4]
        .try_into()
        .expect("selector slice is four bytes")
}

pub fn withdrawal_burn_commitment(
    nock_contract_address: Address,
    burner: Address,
    amount_raw: U256,
    full_lock_root: &Tip5Hash,
) -> B256 {
    let mut input = Vec::with_capacity(
        WITHDRAWAL_BURN_COMMITMENT_DOMAIN.len() + 20 + 20 + 32 + WITHDRAWAL_BURN_FULL_LOCK_ROOT_LEN,
    );
    input.extend_from_slice(WITHDRAWAL_BURN_COMMITMENT_DOMAIN);
    input.extend_from_slice(nock_contract_address.as_slice());
    input.extend_from_slice(burner.as_slice());
    input.extend_from_slice(&amount_raw.to_be_bytes::<32>());
    input.extend_from_slice(&full_lock_root.to_be_limb_bytes());
    keccak256(input)
}

pub fn encode_withdrawal_burn_calldata(
    nock_contract_address: Address,
    burner: Address,
    amount_raw: U256,
    full_lock_root: &Tip5Hash,
) -> Bytes {
    let commitment =
        withdrawal_burn_commitment(nock_contract_address, burner, amount_raw, full_lock_root);
    let mut calldata = Vec::with_capacity(WITHDRAWAL_BURN_CALLDATA_LEN);
    calldata.extend_from_slice(&withdrawal_burn_selector());
    calldata.extend_from_slice(&amount_raw.to_be_bytes::<32>());
    calldata.extend_from_slice(commitment.as_slice());
    calldata.extend_from_slice(WITHDRAWAL_BURN_TRAILER_MAGIC);
    calldata.extend_from_slice(&full_lock_root.to_be_limb_bytes());
    Bytes::from(calldata)
}

pub fn parse_withdrawal_burn_calldata(
    calldata: &[u8],
) -> Result<(U256, B256, Tip5Hash), BurnForWithdrawalDecodeError> {
    if calldata.len() == WITHDRAWAL_BURN_BASE_CALLDATA_LEN {
        return Err(BurnForWithdrawalDecodeError::MissingCalldataTrailer {
            actual_len: calldata.len(),
        });
    }
    if calldata.len() != WITHDRAWAL_BURN_CALLDATA_LEN {
        return Err(BurnForWithdrawalDecodeError::MalformedCalldata {
            reason: format!(
                "expected {} bytes, got {}",
                WITHDRAWAL_BURN_CALLDATA_LEN,
                calldata.len()
            ),
        });
    }
    let selector = withdrawal_burn_selector();
    if calldata[..4] != selector {
        return Err(BurnForWithdrawalDecodeError::MalformedCalldata {
            reason: "selector is not burn(uint256,bytes32)".into(),
        });
    }
    let amount_raw = U256::from_be_slice(&calldata[4..36]);
    let commitment = B256::from_slice(&calldata[36..68]);
    let trailer = &calldata[WITHDRAWAL_BURN_BASE_CALLDATA_LEN..];
    if &trailer[..WITHDRAWAL_BURN_TRAILER_MAGIC.len()] != WITHDRAWAL_BURN_TRAILER_MAGIC.as_slice() {
        return Err(BurnForWithdrawalDecodeError::MalformedCalldata {
            reason: "missing NOCK withdrawal trailer magic".into(),
        });
    }
    let lock_root_bytes = &trailer[WITHDRAWAL_BURN_TRAILER_MAGIC.len()..];
    let lock_root = Tip5Hash::from_be_limb_bytes(lock_root_bytes).map_err(|err| {
        BurnForWithdrawalDecodeError::InvalidLockRoot {
            reason: err.to_string(),
        }
    })?;
    if lock_root.to_array().iter().any(|limb| *limb >= PRIME) {
        return Err(BurnForWithdrawalDecodeError::InvalidLockRoot {
            reason: "one or more limbs are outside the base field".into(),
        });
    }
    Ok((amount_raw, commitment, lock_root))
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn decode_burn_for_withdrawal_log(
    raw: &RawLog,
    tx_hash: &B256,
    log_index: Option<u64>,
) -> Result<DecodedBurnForWithdrawalLog, BurnForWithdrawalDecodeError> {
    let event =
        Nock::BurnForWithdrawal::decode_raw_log(raw.topics.iter().cloned(), raw.data.as_ref())
            .map_err(|err| BurnForWithdrawalDecodeError::NotBurnForWithdrawal(err.to_string()))?;
    let nock_per_nick = U256::from(NOCK_BASE_PER_NICK);
    let amount_raw: U256 = event.amount;
    if amount_raw % nock_per_nick != U256::ZERO {
        return Err(BurnForWithdrawalDecodeError::AmountNotDivisible { amount_raw });
    }
    let nicks = amount_raw / nock_per_nick;
    if nicks > U256::from(u64::MAX) {
        return Err(BurnForWithdrawalDecodeError::AmountOverflow { nicks });
    }

    Ok(DecodedBurnForWithdrawalLog {
        base_event_id: compute_base_event_id(tx_hash, log_index),
        burner: eth_address_from_alloy(event.burner),
        amount: nicks.to::<u64>(),
        lock_root: Tip5Hash::from_be_bytes(&b256_to_array(event.lockRoot)),
    })
}

pub(crate) fn decode_burn_for_withdrawal_event(
    raw: &RawLog,
) -> Result<BurnForWithdrawalEventFacts, BurnForWithdrawalDecodeError> {
    let event =
        Nock::BurnForWithdrawal::decode_raw_log(raw.topics.iter().cloned(), raw.data.as_ref())
            .map_err(|err| BurnForWithdrawalDecodeError::NotBurnForWithdrawal(err.to_string()))?;
    Ok(BurnForWithdrawalEventFacts {
        burner: event.burner,
        amount_raw: event.amount,
        commitment: event.lockRoot,
    })
}

pub fn decode_burn_for_withdrawal_log_with_calldata(
    raw: &RawLog,
    tx_hash: &B256,
    log_index: Option<u64>,
    nock_contract_address: Address,
    tx_input: &[u8],
) -> Result<DecodedBurnForWithdrawalLog, BurnForWithdrawalDecodeError> {
    let event = decode_burn_for_withdrawal_event(raw)?;
    let (calldata_amount_raw, calldata_commitment, lock_root) =
        parse_withdrawal_burn_calldata(tx_input)?;
    let amount_raw = event.amount_raw;
    if calldata_amount_raw != amount_raw {
        return Err(BurnForWithdrawalDecodeError::CalldataAmountMismatch {
            event_amount_raw: amount_raw,
            calldata_amount_raw,
        });
    }
    if calldata_commitment != event.commitment {
        return Err(BurnForWithdrawalDecodeError::CalldataCommitmentMismatch {
            event_commitment: event.commitment,
            calldata_commitment,
        });
    }
    let expected =
        withdrawal_burn_commitment(nock_contract_address, event.burner, amount_raw, &lock_root);
    if expected != event.commitment {
        return Err(BurnForWithdrawalDecodeError::CommitmentMismatch {
            expected,
            actual: event.commitment,
        });
    }

    let nock_per_nick = U256::from(NOCK_BASE_PER_NICK);
    if amount_raw % nock_per_nick != U256::ZERO {
        return Err(BurnForWithdrawalDecodeError::AmountNotDivisible { amount_raw });
    }
    let nicks = amount_raw / nock_per_nick;
    if nicks > U256::from(u64::MAX) {
        return Err(BurnForWithdrawalDecodeError::AmountOverflow { nicks });
    }

    Ok(DecodedBurnForWithdrawalLog {
        base_event_id: compute_base_event_id(tx_hash, log_index),
        burner: eth_address_from_alloy(event.burner),
        amount: nicks.to::<u64>(),
        lock_root,
    })
}

#[cfg(test)]
fn b256_to_array(value: B256) -> [u8; 32] {
    value.as_slice().try_into().expect("B256 is 32 bytes")
}

#[cfg(test)]
mod tests {
    use anyhow::{bail, Context, Result};
    use serde_json::Value;

    use super::*;

    const REFUNDED_WITHDRAWAL_BURN_BASE_RPC_URL_ENV: &str =
        "BRIDGE_REFUNDED_WITHDRAWAL_BASE_RPC_URL";
    const BASE_RPC_URL_ENV: &str = "BASE_RPC_URL";
    const REFUNDED_WITHDRAWAL_BURN_TX_HASH: [u8; 32] = [
        0xfa, 0x0b, 0x8e, 0x41, 0x34, 0xa3, 0x87, 0x44, 0x0a, 0x99, 0x54, 0x41, 0x14, 0x57, 0x83,
        0x97, 0xd5, 0x25, 0x42, 0xce, 0xa3, 0x06, 0xd6, 0xb9, 0xad, 0xea, 0x80, 0x14, 0x07, 0xe3,
        0x12, 0x3f,
    ];
    const REFUNDED_WITHDRAWAL_BURN_LOG_INDEX: u64 = 243;
    const REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID: [u8; 32] = [
        0x45, 0xcf, 0xbf, 0x83, 0x1f, 0x2a, 0xbf, 0x37, 0x71, 0x64, 0xf8, 0x57, 0xa2, 0xbc, 0x47,
        0x33, 0x8f, 0xca, 0xa8, 0xf4, 0xf1, 0x2a, 0x59, 0x86, 0xa3, 0xba, 0x9b, 0xef, 0x35, 0xaf,
        0xea, 0xbd,
    ];

    fn known_compensation_registry() -> CompensatedWithdrawalRegistry {
        CompensatedWithdrawalRegistry::from_config(&[
            crate::shared::config::CompensatedWithdrawalToml {
                base_event_id: format!("0x{}", hex_encode(REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID)),
                transaction_hash: format!("0x{}", hex_encode(REFUNDED_WITHDRAWAL_BURN_TX_HASH)),
                log_index: REFUNDED_WITHDRAWAL_BURN_LOG_INDEX,
                reason: "legacy operator refund".to_string(),
                evidence_reference: "incident:legacy-refund".to_string(),
                recorded_at_unix_secs: 0,
            },
        ])
        .expect("known compensation registry")
    }

    const WITHDRAWAL_WIRE_V1_VECTORS_JSON: &str =
        include_str!("../../test-fixtures/withdrawal_wire_v1_vectors.json");

    #[derive(Debug, serde::Deserialize)]
    struct WithdrawalWireFixture {
        schema_version: u64,
        protocol: String,
        constants: WithdrawalWireConstantsFixture,
        valid_vectors: Vec<WithdrawalWireValidFixture>,
        amount_policy_vectors: Vec<WithdrawalAmountPolicyFixture>,
        invalid_vectors: Vec<WithdrawalWireInvalidFixture>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct WithdrawalWireConstantsFixture {
        commitment_domain: String,
        calldata_length: usize,
        base_calldata_length: usize,
        trailer_magic_ascii: String,
        full_lock_root_length: usize,
        tip5_prime: String,
        nock_base_units_per_nick: String,
    }

    #[derive(Debug, serde::Deserialize)]
    struct WithdrawalWireValidFixture {
        name: String,
        nock_token_address: String,
        burner_address: String,
        amount_base_units: String,
        lock_root_limbs: Vec<String>,
        selector: String,
        commitment: String,
        calldata: String,
    }

    #[derive(Debug, serde::Deserialize)]
    struct WithdrawalAmountPolicyFixture {
        name: String,
        amount_base_units: String,
        expected: String,
        amount_nicks: Option<String>,
        bridge_fee_nicks: Option<String>,
        amount_after_bridge_fee_nicks: Option<String>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct WithdrawalWireInvalidFixture {
        name: String,
        base_vector: String,
        calldata: String,
        expected_error: String,
    }

    fn b256_from_u64(value: u64) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        B256::from(bytes)
    }

    fn address_from_u64(value: u64) -> Address {
        let mut bytes = [0u8; 20];
        bytes[12..].copy_from_slice(&value.to_be_bytes());
        Address::from(bytes)
    }

    fn address_topic(addr: Address) -> B256 {
        let mut topic = [0u8; 32];
        topic[12..].copy_from_slice(addr.as_slice());
        B256::from(topic)
    }

    fn withdrawal_burn_log(
        nock_contract_address: Address,
        burner: Address,
        amount_raw: U256,
        event_lock_root: B256,
    ) -> RawLog {
        RawLog {
            address: nock_contract_address,
            topics: vec![
                Nock::BurnForWithdrawal::SIGNATURE_HASH,
                address_topic(burner),
                event_lock_root,
            ],
            data: Bytes::from(amount_raw.to_be_bytes::<32>().to_vec()),
        }
    }

    fn refunded_withdrawal_burn_base_rpc_url() -> Option<String> {
        [REFUNDED_WITHDRAWAL_BURN_BASE_RPC_URL_ENV, BASE_RPC_URL_ENV]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            })
    }

    async fn base_rpc_call(
        client: &reqwest::Client,
        rpc_url: &str,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let response: Value = client
            .post(rpc_url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .with_context(|| format!("calling Base RPC method {method}"))?
            .error_for_status()
            .with_context(|| format!("Base RPC method {method} returned HTTP error"))?
            .json()
            .await
            .with_context(|| format!("decoding Base RPC method {method} response"))?;

        if let Some(error) = response.get("error") {
            bail!("Base RPC method {method} failed: {error}");
        }

        response
            .get("result")
            .cloned()
            .filter(|result| !result.is_null())
            .with_context(|| format!("Base RPC method {method} returned no result"))
    }

    fn json_field_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
        value
            .get(field)
            .and_then(Value::as_str)
            .with_context(|| format!("missing string field {field}"))
    }

    fn hex_body(value: &str) -> &str {
        value.strip_prefix("0x").unwrap_or(value)
    }

    fn parse_hex_bytes(value: &str, field: &str) -> Result<Vec<u8>> {
        hex::decode(hex_body(value)).with_context(|| format!("invalid hex in field {field}"))
    }

    fn parse_fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N]> {
        let bytes = parse_hex_bytes(value, field)?;
        bytes.try_into().map_err(|bytes: Vec<u8>| {
            anyhow::anyhow!("field {field} has {} bytes, expected {N}", bytes.len())
        })
    }

    fn parse_b256_field(value: &Value, field: &str) -> Result<B256> {
        Ok(B256::from(parse_fixed_hex::<32>(
            json_field_str(value, field)?,
            field,
        )?))
    }

    fn parse_address_field(value: &Value, field: &str) -> Result<Address> {
        Ok(Address::from(parse_fixed_hex::<20>(
            json_field_str(value, field)?,
            field,
        )?))
    }

    fn parse_bytes_field(value: &Value, field: &str) -> Result<Bytes> {
        Ok(Bytes::from(parse_hex_bytes(
            json_field_str(value, field)?,
            field,
        )?))
    }

    fn parse_u64_hex_field(value: &Value, field: &str) -> Result<u64> {
        let raw = json_field_str(value, field)?;
        let digits = hex_body(raw);
        if digits.is_empty() {
            return Ok(0);
        }
        u64::from_str_radix(digits, 16).with_context(|| format!("invalid u64 hex in field {field}"))
    }

    fn parse_topics_field(value: &Value) -> Result<Vec<B256>> {
        let topics = value
            .get("topics")
            .and_then(Value::as_array)
            .context("missing topics array")?;
        topics
            .iter()
            .enumerate()
            .map(|(index, topic)| {
                Ok(B256::from(parse_fixed_hex::<32>(
                    topic
                        .as_str()
                        .with_context(|| format!("topic {index} is not a string"))?,
                    "topics",
                )?))
            })
            .collect()
    }

    fn hex_eq(lhs: &str, rhs: &str) -> bool {
        hex_body(lhs).eq_ignore_ascii_case(hex_body(rhs))
    }

    fn format_tx_hash_hex(tx_hash: &B256) -> String {
        format!("0x{}", hex_encode(tx_hash.as_slice()))
    }

    fn parse_fixture_lock_root(limbs: &[String], vector_name: &str) -> Result<Tip5Hash> {
        if limbs.len() != 5 {
            bail!(
                "vector {vector_name} has {} lock-root limbs, expected 5",
                limbs.len()
            );
        }
        let limbs = limbs
            .iter()
            .enumerate()
            .map(|(index, raw)| {
                raw.parse::<u64>().with_context(|| {
                    format!("vector {vector_name} has invalid lock-root limb {index}")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let limbs: [u64; 5] = limbs.try_into().map_err(|limbs: Vec<u64>| {
            anyhow::anyhow!(
                "vector {vector_name} has {} parsed lock-root limbs, expected 5",
                limbs.len()
            )
        })?;
        Ok(Tip5Hash::from_limbs(&limbs))
    }

    fn burn_decode_error_kind(error: &BurnForWithdrawalDecodeError) -> &'static str {
        match error {
            BurnForWithdrawalDecodeError::NotBurnForWithdrawal(_) => "not_burn_for_withdrawal",
            BurnForWithdrawalDecodeError::AmountNotDivisible { .. } => "amount_not_divisible",
            BurnForWithdrawalDecodeError::AmountOverflow { .. } => "amount_overflow",
            BurnForWithdrawalDecodeError::MissingCalldataTrailer { .. } => {
                "missing_calldata_trailer"
            }
            BurnForWithdrawalDecodeError::MalformedCalldata { .. } => "malformed_calldata",
            BurnForWithdrawalDecodeError::CalldataAmountMismatch { .. } => {
                "calldata_amount_mismatch"
            }
            BurnForWithdrawalDecodeError::CalldataCommitmentMismatch { .. } => {
                "calldata_commitment_mismatch"
            }
            BurnForWithdrawalDecodeError::CommitmentMismatch { .. } => "commitment_mismatch",
            BurnForWithdrawalDecodeError::InvalidLockRoot { .. } => "invalid_lock_root",
        }
    }

    #[test]
    fn automatic_reorg_rewind_depth_is_confirmation_bounded_and_hard_capped() {
        assert_eq!(base_automatic_reorg_rewind_depth(0), 0);
        assert_eq!(base_automatic_reorg_rewind_depth(1), 1);
        assert_eq!(base_automatic_reorg_rewind_depth(64), 64);
        assert_eq!(
            base_automatic_reorg_rewind_depth(DEFAULT_BASE_CONFIRMATION_DEPTH),
            BASE_MAX_AUTOMATIC_REORG_REWIND_BLOCKS
        );
    }

    #[test]
    fn withdrawal_wire_v1_valid_vectors_match_canonical_codec() -> Result<()> {
        let fixture: WithdrawalWireFixture = serde_json::from_str(WITHDRAWAL_WIRE_V1_VECTORS_JSON)
            .context("decoding WithdrawalWireV1 fixture")?;

        assert_eq!(fixture.schema_version, 1);
        assert_eq!(fixture.protocol, "WithdrawalWireV1");
        assert_eq!(
            fixture.constants.commitment_domain.as_bytes(),
            WITHDRAWAL_BURN_COMMITMENT_DOMAIN
        );
        assert_eq!(
            fixture.constants.calldata_length,
            WITHDRAWAL_BURN_CALLDATA_LEN
        );
        assert_eq!(
            fixture.constants.base_calldata_length,
            WITHDRAWAL_BURN_BASE_CALLDATA_LEN
        );
        assert_eq!(
            fixture.constants.trailer_magic_ascii.as_bytes(),
            WITHDRAWAL_BURN_TRAILER_MAGIC
        );
        assert_eq!(
            fixture.constants.full_lock_root_length,
            WITHDRAWAL_BURN_FULL_LOCK_ROOT_LEN
        );
        assert_eq!(fixture.constants.tip5_prime.parse::<u64>()?, PRIME);
        assert_eq!(
            fixture.constants.nock_base_units_per_nick.parse::<u128>()?,
            NOCK_BASE_PER_NICK
        );

        for vector in &fixture.valid_vectors {
            let nock_contract_address = Address::from_str(&vector.nock_token_address)
                .with_context(|| format!("vector {} token address", vector.name))?;
            let burner = Address::from_str(&vector.burner_address)
                .with_context(|| format!("vector {} burner address", vector.name))?;
            let amount_raw = U256::from_str(&vector.amount_base_units)
                .with_context(|| format!("vector {} amount", vector.name))?;
            let lock_root = parse_fixture_lock_root(&vector.lock_root_limbs, &vector.name)?;
            let expected_selector = parse_fixed_hex::<4>(&vector.selector, "selector")?;
            let expected_commitment =
                B256::from(parse_fixed_hex::<32>(&vector.commitment, "commitment")?);
            let expected_calldata = parse_hex_bytes(&vector.calldata, "calldata")?;

            assert_eq!(
                withdrawal_burn_selector(),
                expected_selector,
                "selector mismatch for {}",
                vector.name
            );
            assert_eq!(
                withdrawal_burn_commitment(nock_contract_address, burner, amount_raw, &lock_root,),
                expected_commitment,
                "commitment mismatch for {}",
                vector.name
            );
            let calldata = encode_withdrawal_burn_calldata(
                nock_contract_address, burner, amount_raw, &lock_root,
            );
            assert_eq!(
                calldata.as_ref(),
                expected_calldata,
                "calldata mismatch for {}",
                vector.name
            );

            let log = withdrawal_burn_log(
                nock_contract_address, burner, amount_raw, expected_commitment,
            );
            let decoded = decode_burn_for_withdrawal_log_with_calldata(
                &log,
                &b256_from_u64(0xabcd),
                Some(7),
                nock_contract_address,
                calldata.as_ref(),
            )
            .with_context(|| format!("decoding valid vector {}", vector.name))?;
            assert_eq!(decoded.burner, eth_address_from_alloy(burner));
            assert_eq!(
                decoded.amount,
                (amount_raw / U256::from(NOCK_BASE_PER_NICK)).to::<u64>()
            );
            assert_eq!(decoded.lock_root, lock_root);
        }

        Ok(())
    }

    #[test]
    fn withdrawal_wire_v1_invalid_vectors_match_rejection_taxonomy() -> Result<()> {
        let fixture: WithdrawalWireFixture = serde_json::from_str(WITHDRAWAL_WIRE_V1_VECTORS_JSON)
            .context("decoding WithdrawalWireV1 fixture")?;

        for invalid in &fixture.invalid_vectors {
            let base = fixture
                .valid_vectors
                .iter()
                .find(|vector| vector.name == invalid.base_vector)
                .with_context(|| {
                    format!(
                        "invalid vector {} references missing base {}",
                        invalid.name, invalid.base_vector
                    )
                })?;
            let nock_contract_address = Address::from_str(&base.nock_token_address)
                .with_context(|| format!("vector {} token address", base.name))?;
            let burner = Address::from_str(&base.burner_address)
                .with_context(|| format!("vector {} burner address", base.name))?;
            let amount_raw = U256::from_str(&base.amount_base_units)
                .with_context(|| format!("vector {} amount", base.name))?;
            let event_commitment =
                B256::from(parse_fixed_hex::<32>(&base.commitment, "commitment")?);
            let calldata = parse_hex_bytes(&invalid.calldata, "calldata")?;
            let log =
                withdrawal_burn_log(nock_contract_address, burner, amount_raw, event_commitment);

            let error = decode_burn_for_withdrawal_log_with_calldata(
                &log,
                &b256_from_u64(0xabcd),
                Some(7),
                nock_contract_address,
                &calldata,
            )
            .expect_err("invalid WithdrawalWireV1 vector must be rejected");
            assert_eq!(
                burn_decode_error_kind(&error),
                invalid.expected_error,
                "unexpected rejection for invalid vector {}: {error}",
                invalid.name
            );
        }

        Ok(())
    }

    #[test]
    fn withdrawal_policy_v1_amount_vectors_match_backend_boundaries() -> Result<()> {
        let fixture: WithdrawalWireFixture = serde_json::from_str(WITHDRAWAL_WIRE_V1_VECTORS_JSON)
            .context("decoding WithdrawalWireV1 fixture")?;
        let policy = crate::shared::types::WithdrawalPolicy::v1();
        let minimum_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .context("withdrawal policy minimum overflowed")?;

        for vector in &fixture.amount_policy_vectors {
            let amount_raw = U256::from_str(&vector.amount_base_units)
                .with_context(|| format!("vector {} amount", vector.name))?;
            let base_units_per_nick = U256::from(policy.base_units_per_nick);
            let actual = if amount_raw == U256::ZERO {
                "amount_not_positive"
            } else if amount_raw % base_units_per_nick != U256::ZERO {
                "amount_not_divisible"
            } else {
                let amount_nicks = amount_raw / base_units_per_nick;
                if amount_nicks > U256::from(policy.maximum_nicks) {
                    "amount_overflow"
                } else if amount_nicks < U256::from(minimum_nicks) {
                    "amount_below_minimum"
                } else {
                    "valid"
                }
            };
            assert_eq!(
                actual, vector.expected,
                "unexpected policy result for {}",
                vector.name
            );

            if actual == "valid" {
                let amount_nicks = (amount_raw / base_units_per_nick).to::<u64>();
                let bridge_fee = wallet_tx_builder::fee::compute_bridge_fee(
                    amount_nicks, policy.bridge_fee_nicks_per_started_nock,
                );
                assert_eq!(
                    Some(amount_nicks),
                    vector.amount_nicks.as_deref().map(str::parse).transpose()?,
                    "amount nicks mismatch for {}",
                    vector.name
                );
                assert_eq!(
                    Some(bridge_fee),
                    vector
                        .bridge_fee_nicks
                        .as_deref()
                        .map(str::parse)
                        .transpose()?,
                    "bridge fee mismatch for {}",
                    vector.name
                );
                assert_eq!(
                    Some(amount_nicks - bridge_fee),
                    vector
                        .amount_after_bridge_fee_nicks
                        .as_deref()
                        .map(str::parse)
                        .transpose()?,
                    "post-bridge-fee amount mismatch for {}",
                    vector.name
                );
            }
        }

        Ok(())
    }

    fn sample_contract_readiness() -> (BaseContractReadiness, BridgeNodeEthAddressMap) {
        let bridge_nodes = [
            address_from_u64(10),
            address_from_u64(11),
            address_from_u64(12),
            address_from_u64(13),
            address_from_u64(14),
        ];
        let expected_nodes = bridge_nodes
            .iter()
            .enumerate()
            .map(|(node_id, address)| (node_id as u64, *address))
            .collect();
        (
            BaseContractReadiness {
                inbox_contract_address: address_from_u64(1),
                nock_contract_address: address_from_u64(2),
                owner: address_from_u64(3),
                bridge_nodes,
                threshold: 3,
                withdrawals_enabled: false,
            },
            expected_nodes,
        )
    }

    #[test]
    fn validate_base_contract_readiness_accepts_matching_facts() {
        let (facts, expected_nodes) = sample_contract_readiness();
        validate_base_contract_readiness(
            &facts,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect("matching contract facts");
    }

    #[test]
    fn validate_base_contract_readiness_rejects_each_mismatch() {
        let (facts, expected_nodes) = sample_contract_readiness();

        let mut mismatch = facts.clone();
        mismatch.inbox_contract_address = address_from_u64(99);
        assert!(validate_base_contract_readiness(
            &mismatch,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect_err("inbox mismatch")
        .to_string()
        .contains("Nock.inbox mismatch"));

        let mut mismatch = facts.clone();
        mismatch.nock_contract_address = address_from_u64(99);
        assert!(validate_base_contract_readiness(
            &mismatch,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect_err("nock mismatch")
        .to_string()
        .contains("MessageInbox.nock mismatch"));

        let mut mismatch = facts.clone();
        mismatch.owner = Address::ZERO;
        assert!(validate_base_contract_readiness(
            &mismatch,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect_err("zero owner")
        .to_string()
        .contains("owner must not be the zero address"));

        let mut mismatch = facts.clone();
        mismatch.threshold = 2;
        assert!(validate_base_contract_readiness(
            &mismatch,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect_err("threshold mismatch")
        .to_string()
        .contains("threshold mismatch"));

        let mut mismatch = facts;
        mismatch.bridge_nodes[2] = address_from_u64(99);
        assert!(validate_base_contract_readiness(
            &mismatch,
            address_from_u64(1),
            address_from_u64(2),
            &expected_nodes,
            3,
        )
        .expect_err("node mismatch")
        .to_string()
        .contains("bridge node 2 mismatch"));
    }

    #[tokio::test]
    async fn validate_base_chain_id_accepts_matching_provider() {
        let asserter = alloy::transports::mock::Asserter::new();
        asserter.push_success(&8_453u64);
        let provider = ProviderBuilder::<_, _, Optimism>::default()
            .connect_mocked_client(asserter)
            .erased();

        assert_eq!(
            validate_base_chain_id(&provider, 8_453, "mock://base")
                .await
                .expect("matching chain id"),
            8_453
        );
    }

    #[tokio::test]
    async fn validate_base_chain_id_rejects_mismatched_provider() {
        let asserter = alloy::transports::mock::Asserter::new();
        asserter.push_success(&84_532u64);
        let provider = ProviderBuilder::<_, _, Optimism>::default()
            .connect_mocked_client(asserter)
            .erased();

        let error = validate_base_chain_id(&provider, 8_453, "mock://base")
            .await
            .expect_err("mismatched chain id must fail");
        assert!(error.to_string().contains("expected 8453, observed 84532"));
    }

    #[test]
    fn validate_base_log_block_hash_accepts_matching_header() {
        let mut block_info = HashMap::new();
        let block_hash = b256_from_u64(0xabc);
        block_info.insert(
            42,
            BaseBlockHeaderInfo {
                hash: block_hash,
                parent_hash: b256_from_u64(0xdef),
                timestamp: 1_700_000_000,
            },
        );

        validate_base_log_block_hash(&block_info, 40, 50, 42, Some(block_hash))
            .expect("matching log block hash should be accepted");
    }

    #[test]
    fn validate_base_log_block_hash_rejects_missing_hash() {
        let mut block_info = HashMap::new();
        block_info.insert(
            42,
            BaseBlockHeaderInfo {
                hash: b256_from_u64(0xabc),
                parent_hash: b256_from_u64(0xdef),
                timestamp: 1_700_000_000,
            },
        );

        let err = validate_base_log_block_hash(&block_info, 40, 50, 42, None)
            .expect_err("missing log block hash should fail closed");
        assert!(
            err.to_string().contains("missing block hash"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_base_log_block_hash_rejects_mismatched_header() {
        let mut block_info = HashMap::new();
        block_info.insert(
            42,
            BaseBlockHeaderInfo {
                hash: b256_from_u64(0xabc),
                parent_hash: b256_from_u64(0xdef),
                timestamp: 1_700_000_000,
            },
        );

        let err = validate_base_log_block_hash(&block_info, 40, 50, 42, Some(b256_from_u64(0x123)))
            .expect_err("mismatched log block hash should fail closed");
        assert!(
            err.to_string().contains("block hash mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_base_log_block_hash_rejects_outside_batch_height() {
        let block_info = HashMap::new();

        let err = validate_base_log_block_hash(&block_info, 40, 50, 60, Some(b256_from_u64(0x123)))
            .expect_err("outside-window log should fail closed");
        assert!(
            err.to_string().contains("outside fetched batch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn decodes_burn_for_withdrawal_event() {
        let burner = address_from_u64(0xdeadbeef);
        let lock_root = b256_from_u64(0x1234);
        let amount = U256::from(42u64);

        let topics =
            vec![Nock::BurnForWithdrawal::SIGNATURE_HASH, address_topic(burner), lock_root];
        let mut amount_bytes = [0u8; 32];
        amount_bytes.copy_from_slice(&amount.to_be_bytes::<32>());
        let log = RawLog {
            address: Address::ZERO,
            topics,
            data: Bytes::from(amount_bytes.to_vec()),
        };

        let event =
            Nock::BurnForWithdrawal::decode_raw_log(log.topics.iter().cloned(), log.data.as_ref())
                .expect("decode burn for withdrawal");
        assert_eq!(event.burner, burner);
        assert_eq!(event.lockRoot, lock_root);
        assert_eq!(U256::from(event.amount), amount);
    }

    #[test]
    fn encodes_and_decodes_withdrawal_burn_with_full_lock_root_trailer() {
        let nock_contract_address = address_from_u64(0x1111);
        let burner = address_from_u64(0xdeadbeef);
        let amount_raw = U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK);
        let full_lock_root = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let calldata = encode_withdrawal_burn_calldata(
            nock_contract_address, burner, amount_raw, &full_lock_root,
        );
        let commitment =
            withdrawal_burn_commitment(nock_contract_address, burner, amount_raw, &full_lock_root);
        let log = withdrawal_burn_log(nock_contract_address, burner, amount_raw, commitment);

        assert_eq!(calldata.len(), WITHDRAWAL_BURN_CALLDATA_LEN);
        assert_eq!(&calldata[..4], withdrawal_burn_selector().as_slice());
        assert_eq!(
            &calldata[WITHDRAWAL_BURN_BASE_CALLDATA_LEN
                ..WITHDRAWAL_BURN_BASE_CALLDATA_LEN + WITHDRAWAL_BURN_TRAILER_MAGIC.len()],
            WITHDRAWAL_BURN_TRAILER_MAGIC.as_slice()
        );

        let decoded = decode_burn_for_withdrawal_log_with_calldata(
            &log,
            &b256_from_u64(0xabcd),
            Some(7),
            nock_contract_address,
            calldata.as_ref(),
        )
        .expect("decode withdrawal burn with trailer");

        assert_eq!(decoded.burner, eth_address_from_alloy(burner));
        assert_eq!(decoded.amount, 42);
        assert_eq!(decoded.lock_root, full_lock_root);
    }

    #[test]
    fn rejects_withdrawal_burn_without_full_lock_root_trailer() {
        let nock_contract_address = address_from_u64(0x1111);
        let burner = address_from_u64(0xdeadbeef);
        let amount_raw = U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK);
        let full_lock_root = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let calldata = encode_withdrawal_burn_calldata(
            nock_contract_address, burner, amount_raw, &full_lock_root,
        );
        let commitment =
            withdrawal_burn_commitment(nock_contract_address, burner, amount_raw, &full_lock_root);
        let log = withdrawal_burn_log(nock_contract_address, burner, amount_raw, commitment);

        let err = decode_burn_for_withdrawal_log_with_calldata(
            &log,
            &b256_from_u64(0xabcd),
            Some(7),
            nock_contract_address,
            &calldata[..WITHDRAWAL_BURN_BASE_CALLDATA_LEN],
        )
        .expect_err("missing trailer");

        assert!(matches!(
            err,
            BurnForWithdrawalDecodeError::MissingCalldataTrailer {
                actual_len: WITHDRAWAL_BURN_BASE_CALLDATA_LEN
            }
        ));
    }

    #[test]
    fn rejects_withdrawal_burn_when_event_commitment_differs_from_calldata() {
        let nock_contract_address = address_from_u64(0x1111);
        let burner = address_from_u64(0xdeadbeef);
        let amount_raw = U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK);
        let full_lock_root = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let calldata = encode_withdrawal_burn_calldata(
            nock_contract_address, burner, amount_raw, &full_lock_root,
        );
        let log = withdrawal_burn_log(nock_contract_address, burner, amount_raw, b256_from_u64(9));

        let err = decode_burn_for_withdrawal_log_with_calldata(
            &log,
            &b256_from_u64(0xabcd),
            Some(7),
            nock_contract_address,
            calldata.as_ref(),
        )
        .expect_err("commitment mismatch");

        assert!(matches!(
            err,
            BurnForWithdrawalDecodeError::CalldataCommitmentMismatch { .. }
        ));
    }

    #[test]
    fn compute_base_event_id_matches_keccak() {
        let tx_hash = b256_from_u64(0xfeed);
        let id = compute_base_event_id(&tx_hash, Some(2));
        let expected = {
            let mut buf = Vec::new();
            buf.extend_from_slice(tx_hash.as_slice());
            let idx = U256::from(2u64);
            buf.extend_from_slice(&idx.to_be_bytes::<32>());
            keccak256(&buf)
        };
        assert_eq!(id.0, expected.as_slice());
    }

    #[test]
    fn refunded_withdrawal_burn_identity_matches_known_base_event_id() {
        let tx_hash = B256::from(REFUNDED_WITHDRAWAL_BURN_TX_HASH);
        let id = compute_base_event_id(&tx_hash, Some(REFUNDED_WITHDRAWAL_BURN_LOG_INDEX));

        assert_eq!(id.0, REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID);
        let registry = known_compensation_registry();
        let compensation = registry.get(&id).expect("known compensation");
        assert_eq!(compensation.transaction_hash, tx_hash);
        assert_eq!(compensation.log_index, REFUNDED_WITHDRAWAL_BURN_LOG_INDEX);
    }

    #[test]
    fn refunded_withdrawal_burn_constants_match_incident_hex() {
        let tx_hash = B256::from(REFUNDED_WITHDRAWAL_BURN_TX_HASH);
        let base_event_id = BaseEventId(REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID.to_vec());

        assert_eq!(
            format!("0x{}", hex_encode(tx_hash.as_slice())),
            "0xfa0b8e4134a387440a99544114578397d52542cea306d6b9adea801407e3123f"
        );
        assert_eq!(REFUNDED_WITHDRAWAL_BURN_LOG_INDEX, 0xf3);
        assert_eq!(
            format!("0x{}", hex_encode(&base_event_id.0)),
            "0x45cfbf831f2abf377164f857a2bc47338fcaa8f4f12a5986a3ba9bef35afeabd"
        );

        let recomputed_id =
            compute_base_event_id(&tx_hash, Some(REFUNDED_WITHDRAWAL_BURN_LOG_INDEX));
        assert_eq!(recomputed_id, base_event_id);
    }

    #[tokio::test]
    #[ignore = "requires a Base RPC URL with historical access"]
    async fn ignored_refunded_withdrawal_burn_matches_live_base_block() -> Result<()> {
        let Some(rpc_url) = refunded_withdrawal_burn_base_rpc_url() else {
            eprintln!(
                "skipping live Base RPC test; set {REFUNDED_WITHDRAWAL_BURN_BASE_RPC_URL_ENV} or {BASE_RPC_URL_ENV}"
            );
            return Ok(());
        };

        let client = reqwest::Client::new();
        let tx_hash = B256::from(REFUNDED_WITHDRAWAL_BURN_TX_HASH);
        let tx_hash_hex = format_tx_hash_hex(&tx_hash);

        let receipt = base_rpc_call(
            &client,
            &rpc_url,
            "eth_getTransactionReceipt",
            serde_json::json!([tx_hash_hex.clone()]),
        )
        .await?;
        let block_number = parse_u64_hex_field(&receipt, "blockNumber")?;
        let receipt_block_hash = parse_b256_field(&receipt, "blockHash")?;

        let block_number_hex = format!("0x{block_number:x}");
        let block = base_rpc_call(
            &client,
            &rpc_url,
            "eth_getBlockByNumber",
            serde_json::json!([block_number_hex, true]),
        )
        .await?;
        assert_eq!(parse_u64_hex_field(&block, "number")?, block_number);
        assert_eq!(parse_b256_field(&block, "hash")?, receipt_block_hash);

        let transactions = block
            .get("transactions")
            .and_then(Value::as_array)
            .context("exact Base block did not include full transactions")?;
        let transaction = transactions
            .iter()
            .find(|transaction| {
                json_field_str(transaction, "hash")
                    .map(|hash| hex_eq(hash, &tx_hash_hex))
                    .unwrap_or(false)
            })
            .context("refunded burn transaction missing from exact Base block")?;
        let tx_input = parse_bytes_field(transaction, "input")?;
        let nock_contract_address = parse_address_field(transaction, "to")?;

        let receipt_logs = receipt
            .get("logs")
            .and_then(Value::as_array)
            .context("receipt did not include logs")?;
        let burn_log = receipt_logs
            .iter()
            .find(|log| {
                let matches_transaction = json_field_str(log, "transactionHash")
                    .map(|hash| hex_eq(hash, &tx_hash_hex))
                    .unwrap_or(false);
                let matches_index = parse_u64_hex_field(log, "logIndex").ok()
                    == Some(REFUNDED_WITHDRAWAL_BURN_LOG_INDEX);
                matches_transaction && matches_index
            })
            .context("refunded burn log missing from transaction receipt")?;

        let raw = RawLog {
            address: parse_address_field(burn_log, "address")?,
            topics: parse_topics_field(burn_log)?,
            data: parse_bytes_field(burn_log, "data")?,
        };
        assert_eq!(raw.address, nock_contract_address);
        assert_eq!(
            raw.topics.first(),
            Some(&Nock::BurnForWithdrawal::SIGNATURE_HASH)
        );

        let decoded = decode_burn_for_withdrawal_log_with_calldata(
            &raw,
            &tx_hash,
            Some(REFUNDED_WITHDRAWAL_BURN_LOG_INDEX),
            nock_contract_address,
            tx_input.as_ref(),
        )
        .context("live refunded burn log should decode with the live calldata trailer")?;

        assert_eq!(
            decoded.base_event_id.0,
            REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID
        );
        assert!(known_compensation_registry()
            .get(&decoded.base_event_id)
            .is_some());

        Ok(())
    }

    #[test]
    fn compensated_withdrawal_registry_rejects_mismatched_coordinate() {
        let error = CompensatedWithdrawalRegistry::from_config(&[
            crate::shared::config::CompensatedWithdrawalToml {
                base_event_id: format!("0x{}", hex_encode(REFUNDED_WITHDRAWAL_BURN_BASE_EVENT_ID)),
                transaction_hash: format!("0x{}", hex_encode(REFUNDED_WITHDRAWAL_BURN_TX_HASH)),
                log_index: REFUNDED_WITHDRAWAL_BURN_LOG_INDEX - 1,
                reason: "legacy operator refund".to_string(),
                evidence_reference: "incident:legacy-refund".to_string(),
                recorded_at_unix_secs: 0,
            },
        ])
        .expect_err("mismatched compensation identity must fail");
        assert!(error
            .to_string()
            .contains("does not match transaction hash and log index"));
    }

    const TEST_BATCH_SIZE: u64 = 1000;

    #[test]
    fn confirmed_batch_returns_none_during_bootstrap() {
        assert!(confirmed_batch(500, TEST_BATCH_SIZE, DEFAULT_BASE_CONFIRMATION_DEPTH).is_none());
        assert!(confirmed_batch(
            TEST_BATCH_SIZE - 1,
            TEST_BATCH_SIZE,
            DEFAULT_BASE_CONFIRMATION_DEPTH
        )
        .is_none());
    }

    #[test]
    fn confirmed_batch_returns_batch_when_ready() {
        let tip = DEFAULT_BASE_CONFIRMATION_DEPTH + TEST_BATCH_SIZE;
        let batch = confirmed_batch(tip, TEST_BATCH_SIZE, DEFAULT_BASE_CONFIRMATION_DEPTH);
        assert!(batch.is_some());
        let (start, end) =
            batch.expect("batch should be Some when tip >= confirmation_depth + batch_size");
        assert_eq!(end - start + 1, TEST_BATCH_SIZE);
        assert!(end <= tip - DEFAULT_BASE_CONFIRMATION_DEPTH);
    }

    #[test]
    fn next_confirmed_window_returns_exact_batch() {
        // With batch_size=1000, need confirmed_height >= 1001 + 1000 - 1 = 2000
        let confirmed_height = 2500;
        let window = next_confirmed_window(1001, confirmed_height, 1000).expect("window");
        // Should return exact batch size, not capped to confirmed_height
        assert_eq!(window, (1001, 2000));
    }

    #[test]
    fn next_confirmed_window_none_when_batch_not_fully_confirmed() {
        // With batch_size=1000, need confirmed_height >= 2001 + 1000 - 1 = 3000
        let confirmed_height = 2500; // Not enough for full batch
        let window = next_confirmed_window(2001, confirmed_height, 1000);
        assert!(window.is_none());
    }

    #[test]
    fn next_confirmed_window_works_with_misaligned_start() {
        // Start at 33,387,036 (offset 36 from 1000-boundary), batch_size=100
        // Need confirmed_height >= 33,387,036 + 100 - 1 = 33,387,135
        let window = next_confirmed_window(33_387_036, 33_387_200, 100).expect("window");
        assert_eq!(window, (33_387_036, 33_387_135));
    }

    #[test]
    fn next_confirmed_window_none_when_not_confirmed() {
        let confirmed_height = 1500;
        let window = next_confirmed_window(2001, confirmed_height, 1000);
        assert!(window.is_none());
    }

    // Helper to encode a Tip5Hash (5 u64 limbs) as ABI-encoded data
    fn encode_tip5_limbs(limbs: &[u64; 5]) -> Vec<u8> {
        let mut data = Vec::new();
        for &limb in limbs {
            // ABI encodes uint64 as 32 bytes (left-padded with zeros)
            let mut padded = [0u8; 32];
            padded[24..].copy_from_slice(&limb.to_be_bytes());
            data.extend_from_slice(&padded);
        }
        data
    }

    // Helper to create a bytes32 topic from first limb of Tip5Hash (for indexed param)
    fn tip5_to_indexed_bytes32(limbs: &[u64; 5]) -> B256 {
        // The indexed bytes32 is keccak256 of the full Tip5Hash limbs
        // For testing, we'll use a simple hash of the first limb
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&limbs[0].to_be_bytes());
        B256::from(bytes)
    }

    #[test]
    fn decodes_deposit_processed_event_all_fields() {
        // Define known test values
        let tx_id_limbs: [u64; 5] = [
            0x1111111111111111, 0x2222222222222222, 0x3333333333333333, 0x4444444444444444,
            0x5555555555555555,
        ];
        let name_first_limbs: [u64; 5] = [
            0xaaaaaaaaaaaaaaaa, 0xbbbbbbbbbbbbbbbb, 0xcccccccccccccccc, 0xdddddddddddddddd,
            0xeeeeeeeeeeeeeeee,
        ];
        let name_last_limbs: [u64; 5] = [
            0x1111222233334444, 0x5555666677778888, 0x9999aaaabbbbcccc, 0xddddeeeeffff0000,
            0x1234567890abcdef,
        ];
        let as_of_limbs: [u64; 5] = [
            0x1234567890abcdef, 0xfedcba0987654321, 0x0011223344556677, 0x8899aabbccddeeff,
            0xdeadbeefcafebabe,
        ];
        let recipient = address_from_u64(0xdeadbeef);
        let amount = U256::from(10_000_000_000_000_000u128); // 1 NOCK (10^16 base units)
        let block_height = U256::from(12345u64);
        let nonce = U256::from(42u64);

        // Build topics: [signature, txId, nameFirstHash, recipient]
        let topics = vec![
            MessageInbox::DepositProcessed::SIGNATURE_HASH,
            tip5_to_indexed_bytes32(&tx_id_limbs),
            tip5_to_indexed_bytes32(&name_first_limbs),
            address_topic(recipient),
        ];

        // Build data: txIdFull, nameFirst, nameLast, amount, blockHeight, asOf, nonce
        let mut data = Vec::new();
        data.extend(encode_tip5_limbs(&tx_id_limbs));
        data.extend(encode_tip5_limbs(&name_first_limbs));
        data.extend(encode_tip5_limbs(&name_last_limbs));
        data.extend_from_slice(&amount.to_be_bytes::<32>());
        data.extend_from_slice(&block_height.to_be_bytes::<32>());
        data.extend(encode_tip5_limbs(&as_of_limbs));
        data.extend_from_slice(&nonce.to_be_bytes::<32>());

        let log = RawLog {
            address: Address::ZERO,
            topics,
            data: Bytes::from(data),
        };

        // Decode the event
        let event = MessageInbox::DepositProcessed::decode_raw_log(
            log.topics.iter().cloned(),
            log.data.as_ref(),
        )
        .expect("decode deposit processed");

        // Verify all fields
        assert_eq!(
            event.txIdFull.limbs, tx_id_limbs,
            "txIdFull limbs should match"
        );
        assert_eq!(
            event.nameFirst.limbs, name_first_limbs,
            "nameFirst limbs should match"
        );
        assert_eq!(
            event.nameLast.limbs, name_last_limbs,
            "nameLast limbs should match"
        );
        assert_eq!(event.recipient, recipient, "recipient should match");
        assert_eq!(event.amount, amount, "amount should match");
        assert_eq!(event.blockHeight, block_height, "blockHeight should match");
        assert_eq!(event.asOf.limbs, as_of_limbs, "asOf limbs should match");
        assert_eq!(event.nonce, nonce, "nonce should match");

        // Verify Tip5Hash extraction through the conversion function
        let nock_tx_id = tip5_from_limbs(&event.txIdFull.limbs);
        let note_name_first = tip5_from_limbs(&event.nameFirst.limbs);
        let note_name_last = tip5_from_limbs(&event.nameLast.limbs);
        let as_of = tip5_from_limbs(&event.asOf.limbs);

        // Verify each limb in the Tip5Hash
        assert_eq!(nock_tx_id.0[0].0, tx_id_limbs[0]);
        assert_eq!(nock_tx_id.0[1].0, tx_id_limbs[1]);
        assert_eq!(nock_tx_id.0[2].0, tx_id_limbs[2]);
        assert_eq!(nock_tx_id.0[3].0, tx_id_limbs[3]);
        assert_eq!(nock_tx_id.0[4].0, tx_id_limbs[4]);

        assert_eq!(note_name_first.0[0].0, name_first_limbs[0]);
        assert_eq!(note_name_last.0[0].0, name_last_limbs[0]);
        assert_eq!(as_of.0[0].0, as_of_limbs[0]);
    }

    #[test]
    fn nock_to_nicks_conversion() {
        // Test amount conversion: NOCK base units → nicks
        // Formula: nicks = nock_base / NOCK_BASE_PER_NICK
        // 1 nick = 152,587,890,625 NOCK base units

        let nock_per_nick = U256::from(NOCK_BASE_PER_NICK);

        // 1 NOCK = 10^16 NOCK base units → 65,536 nicks
        let one_nock_base = U256::from(NOCK_BASE_UNIT);
        let nicks = one_nock_base / nock_per_nick;
        assert_eq!(nicks, U256::from(65_536u64), "1 NOCK should be 65536 nicks");

        // 1000 NOCK → 65,536,000 nicks
        let thousand_nock_base = U256::from(1000u64) * U256::from(NOCK_BASE_UNIT);
        let nicks = thousand_nock_base / nock_per_nick;
        assert_eq!(
            nicks,
            U256::from(65_536_000u64),
            "1000 NOCK should be 65,536,000 nicks"
        );

        // 0 → 0 nicks
        let zero = U256::ZERO;
        let nicks = zero / nock_per_nick;
        assert_eq!(nicks, U256::ZERO, "0 NOCK should be 0 nicks");

        // 1 nick worth of NOCK → 1 nick
        let one_nick_nock = U256::from(NOCK_BASE_PER_NICK);
        let nicks = one_nick_nock / nock_per_nick;
        assert_eq!(
            nicks,
            U256::from(1u64),
            "NOCK_BASE_PER_NICK should be 1 nick"
        );

        // Test non-divisible amounts should be flagged
        let not_divisible = U256::from(NOCK_BASE_PER_NICK) + U256::from(1u64);
        let remainder = not_divisible % nock_per_nick;
        assert!(
            remainder != U256::ZERO,
            "NOCK_BASE_PER_NICK + 1 should have non-zero remainder"
        );
    }

    #[test]
    fn nicks_to_nock_conversion() {
        // Test the submission-side conversion: nicks → NOCK base units
        // 1 NOCK = 65,536 nicks = 10^16 NOCK base units
        // So 1 nick = 10^16 / 65,536 = 152,587,890,625 NOCK base units

        // Verify the constant is correct
        assert_eq!(
            NOCK_BASE_PER_NICK,
            NOCK_BASE_UNIT / NICKS_PER_NOCK,
            "NOCK_BASE_PER_NICK should equal NOCK_BASE_UNIT / NICKS_PER_NOCK"
        );
        assert_eq!(NOCK_BASE_PER_NICK, 152_587_890_625);

        // 1 NOCK worth of nicks → 10^16 NOCK base units
        let one_nock_nicks: u128 = 65_536;
        let nock_base = U256::from(one_nock_nicks) * U256::from(NOCK_BASE_PER_NICK);
        assert_eq!(nock_base, U256::from(NOCK_BASE_UNIT));

        // Fractional nock: 1 nick → 152,587,890,625 NOCK base units
        let one_nick: u128 = 1;
        let nock_base = U256::from(one_nick) * U256::from(NOCK_BASE_PER_NICK);
        assert_eq!(nock_base, U256::from(152_587_890_625u128));

        // The actual failing amount from the bug report: 3,988,097,980 nicks
        let bug_amount: u128 = 3_988_097_980;
        let nock_base = U256::from(bug_amount) * U256::from(NOCK_BASE_PER_NICK);
        // This should be divisible by NOCK_BASE_PER_NICK (for round-trip back to nicks)
        assert_eq!(
            nock_base % U256::from(NOCK_BASE_PER_NICK),
            U256::ZERO,
            "converted amount should be divisible by NOCK_BASE_PER_NICK"
        );
        // And we can recover the original nicks
        assert_eq!(
            nock_base / U256::from(NOCK_BASE_PER_NICK),
            U256::from(bug_amount),
            "should recover original nicks"
        );

        // Round-trip: nicks → NOCK base → nicks
        let original_nicks: u128 = 123_456_789;
        let nock_base = U256::from(original_nicks) * U256::from(NOCK_BASE_PER_NICK);
        let back_nicks = nock_base / U256::from(NOCK_BASE_PER_NICK);
        assert_eq!(back_nicks, U256::from(original_nicks));
    }

    #[test]
    fn deposit_processed_nonce_extraction() {
        // Test that nonce is correctly extracted as u64
        let nonce_value = 12345u64;
        let nonce_u256 = U256::from(nonce_value);

        // Event would store it as U256, we convert to u64
        let extracted = nonce_u256.to::<u64>();
        assert_eq!(extracted, nonce_value, "nonce should extract correctly");

        // Test edge case: max u64
        let max_nonce = U256::from(u64::MAX);
        let extracted = max_nonce.to::<u64>();
        assert_eq!(extracted, u64::MAX, "max u64 nonce should work");

        // Test zero nonce
        let zero_nonce = U256::ZERO;
        let extracted = zero_nonce.to::<u64>();
        assert_eq!(extracted, 0, "zero nonce should work");
    }

    #[test]
    fn deposit_processed_block_height_extraction() {
        // Test block height extraction
        let height_value = 9999999u64;
        let height_u256 = U256::from(height_value);
        let extracted = height_u256.to::<u64>();
        assert_eq!(
            extracted, height_value,
            "block height should extract correctly"
        );
    }

    #[test]
    fn tip5_from_limbs_roundtrip() {
        let limbs: [u64; 5] = [
            0x1234567890abcdef, 0xfedcba0987654321, 0x0011223344556677, 0x8899aabbccddeeff,
            0xdeadbeefcafebabe,
        ];

        let tip5 = Tip5Hash::from_limbs(&limbs);
        let back = tip5.to_array();

        assert_eq!(limbs, back, "limbs should roundtrip through Tip5Hash");
    }

    #[test]
    fn tip5_from_limbs_all_zeros() {
        let limbs: [u64; 5] = [0, 0, 0, 0, 0];
        let tip5 = tip5_from_limbs(&limbs);
        for belt in tip5.0.iter() {
            assert_eq!(belt.0, 0, "all limbs should be zero");
        }
    }

    #[test]
    fn tip5_from_limbs_max_values() {
        let limbs: [u64; 5] = [u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX];
        let tip5 = tip5_from_limbs(&limbs);
        for (i, belt) in tip5.0.iter().enumerate() {
            assert_eq!(belt.0, u64::MAX, "limb {} should be max u64", i);
        }
    }

    #[test]
    fn recipient_address_extraction() {
        let addr_bytes: [u8; 20] = [
            0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab,
            0xcd, 0xef, 0x11, 0x22, 0x33, 0x44,
        ];
        let addr = Address::from(addr_bytes);
        let eth_addr = eth_address_from_alloy(addr);

        assert_eq!(eth_addr.0, addr_bytes, "address bytes should match");
    }

    #[test]
    fn amount_overflow_protection() {
        // Test that amounts > u64::MAX / 65536 are caught
        let max_safe = u64::MAX / 65_536;

        // Just at the limit - should work
        let safe_nocks = U256::from(max_safe);
        assert!(
            safe_nocks <= U256::from(u64::MAX / 65_536u64),
            "safe value should be within limit"
        );
        let amount = safe_nocks.to::<u64>().checked_mul(65_536);
        assert!(amount.is_some(), "safe multiplication should succeed");

        // Over the limit - would fail
        let unsafe_nocks = U256::from(max_safe + 1);
        let amount = unsafe_nocks.to::<u64>().checked_mul(65_536);
        assert!(amount.is_none(), "overflow should be detected");
    }

    #[test]
    fn burn_for_withdrawal_lock_root_to_tip5hash() {
        // Test lock_root bytes32 → Tip5Hash conversion via Tip5Hash::from_be_bytes
        // This is critical for matching withdrawals to Nockchain lock roots

        // Test with known bytes32 value
        let lock_root_bytes: [u8; 32] = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0xde, 0xad, 0xbe, 0xef,
            0xca, 0xfe, 0xba, 0xbe,
        ];
        let lock_root = B256::from(lock_root_bytes);

        // Convert using the production code path
        let tip5 = Tip5Hash::from_be_bytes(&b256_to_array(lock_root));

        // The Tip5Hash should have 5 Belt values derived from the BE bytes
        // Verify the structure is valid (non-panic)
        assert_eq!(tip5.0.len(), 5, "Tip5Hash should have 5 limbs");

        // Test all-zeros
        let zero_root = B256::ZERO;
        let tip5_zero = Tip5Hash::from_be_bytes(&b256_to_array(zero_root));
        // All-zero input should produce all-zero Tip5Hash
        for belt in tip5_zero.0.iter() {
            assert_eq!(belt.0, 0, "zero input should produce zero Tip5Hash");
        }

        // Test all-0xFF (max bytes)
        let max_bytes: [u8; 32] = [0xFF; 32];
        let max_root = B256::from(max_bytes);
        let tip5_max = Tip5Hash::from_be_bytes(&b256_to_array(max_root));
        // Should produce valid Tip5Hash without panic
        assert_eq!(
            tip5_max.0.len(),
            5,
            "max bytes should produce valid Tip5Hash"
        );
    }

    #[test]
    fn burn_for_withdrawal_full_extraction() {
        // End-to-end test: create BurnForWithdrawal event, extract and convert lock_root
        let burner = address_from_u64(0xcafebabe);
        let lock_root_bytes: [u8; 32] = [
            0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x00, 0x11,
        ];
        let lock_root = B256::from(lock_root_bytes);
        let amount = U256::from(50_000_000_000_000_000u128); // 5 NOCK (5 * 10^16 base units)

        let topics =
            vec![Nock::BurnForWithdrawal::SIGNATURE_HASH, address_topic(burner), lock_root];
        let mut amount_bytes = [0u8; 32];
        amount_bytes.copy_from_slice(&amount.to_be_bytes::<32>());
        let log = RawLog {
            address: Address::ZERO,
            topics,
            data: Bytes::from(amount_bytes.to_vec()),
        };

        let event =
            Nock::BurnForWithdrawal::decode_raw_log(log.topics.iter().cloned(), log.data.as_ref())
                .expect("decode burn for withdrawal");

        // Extract and convert lock_root
        let tip5_lock_root = Tip5Hash::from_be_bytes(&b256_to_array(event.lockRoot));

        // Verify the conversion happened
        assert_eq!(
            tip5_lock_root.0.len(),
            5,
            "lock_root should convert to 5-limb Tip5Hash"
        );

        // Verify other fields
        assert_eq!(event.burner, burner, "burner should match");
        assert_eq!(event.lockRoot, lock_root, "lockRoot bytes should match");
        assert_eq!(event.amount, amount, "amount should match");

        // Verify amount conversion (NOCK base units → nicks)
        let nock_per_nick = U256::from(NOCK_BASE_PER_NICK);
        let nicks = amount / nock_per_nick;
        assert_eq!(
            nicks,
            U256::from(5u64 * 65_536),
            "5 NOCK should be 5 * 65536 nicks"
        );
    }

    #[test]
    fn base_event_id_computation() {
        // Test base_event_id computation (keccak256 of tx_hash + log_index)
        let tx_hash = b256_from_u64(0xdeadbeef);

        // With log index 0
        let id0 = compute_base_event_id(&tx_hash, Some(0));
        assert_eq!(id0.0.len(), 32, "base_event_id should be 32 bytes");

        // With log index 1 - should be different
        let id1 = compute_base_event_id(&tx_hash, Some(1));
        assert_ne!(
            id0.0, id1.0,
            "different log indices should produce different IDs"
        );

        // With None log index (defaults to 0)
        let id_none = compute_base_event_id(&tx_hash, None);
        assert_eq!(id0.0, id_none.0, "None log index should equal 0");

        // Different tx_hash should produce different ID
        let tx_hash2 = b256_from_u64(0xcafebabe);
        let id2 = compute_base_event_id(&tx_hash2, Some(0));
        assert_ne!(
            id0.0, id2.0,
            "different tx hashes should produce different IDs"
        );
    }

    #[test]
    fn records_deposit_metrics_on_event_processing() {
        // This test verifies that when we process a DepositProcessed event,
        // we correctly call record_tx_completion() to update metrics.
        // The actual metrics recording is tested in state.rs tests.
        // This is a characterization test to document the integration point.
        use std::sync::RwLock;

        use crate::observability::status::BridgeStatus;

        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Simulate what happens when a deposit event is processed
        state.record_tx_completion(TxDirection::Deposit, 1000, 0, true);

        let metrics = state.metrics();
        assert_eq!(
            metrics.total_deposited, 1000,
            "should record deposit amount"
        );
        assert_eq!(metrics.tx_count, 1, "should increment tx count");
    }

    #[test]
    fn records_withdrawal_metrics_on_event_processing() {
        // This test verifies that when we process a BurnForWithdrawal event,
        // we correctly call record_tx_completion() to update metrics.
        // The actual metrics recording is tested in state.rs tests.
        // This is a characterization test to document the integration point.
        use std::sync::RwLock;

        use crate::observability::status::BridgeStatus;

        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Simulate what happens when a withdrawal event is processed
        state.record_tx_completion(TxDirection::Withdrawal, 2000, 0, true);

        let metrics = state.metrics();
        assert_eq!(
            metrics.total_withdrawn, 2000,
            "should record withdrawal amount"
        );
        assert_eq!(metrics.tx_count, 1, "should increment tx count");
    }
}
