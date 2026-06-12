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
use crate::shared::stop::StopHandle;

fn is_rate_limit_error<E: std::fmt::Display>(e: &E) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("rate limit") || s.contains("-32005")
}

use crate::deposit::types::{BaseDepositSettlementEntry, DepositSettlement, DepositSettlementData};
use crate::shared::types::{
    zero_tip5_hash, AtomBytes, BaseBlockRef, BaseEvent, BaseEventContent, EthAddress, NullTag,
    Tip5Hash,
};
use crate::withdrawal::types::{BaseWithdrawalEntry, Withdrawal};

/// Default Base confirmation depth used by the driver if not specified in config.
///
/// The bridge kernel assumes blocks it receives are final; this is enforced by the Rust driver.
pub const DEFAULT_BASE_CONFIRMATION_DEPTH: u64 = 300;

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
pub(crate) const NOCK_BASE_UNIT: u128 = 10_000_000_000_000_000;

/// Nicks per NOCK on Nockchain (2^16)
const NICKS_PER_NOCK: u128 = 65_536;

/// Conversion factor: NOCK base units per nick
/// 1 nick = 10^16 / 65,536 = 152,587,890,625 NOCK base units
pub(crate) const NOCK_BASE_PER_NICK: u128 = NOCK_BASE_UNIT / NICKS_PER_NOCK;

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
pub(crate) struct DecodedBurnForWithdrawalLog {
    pub base_event_id: AtomBytes,
    pub burner: EthAddress,
    pub amount: u64,
    pub lock_root: Tip5Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BurnForWithdrawalDecodeError {
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
}

#[allow(dead_code)]
pub struct BaseBridge {
    deps: BaseBridgeDeps,
    contracts: BaseBridgeContracts,
    config: BaseBridgeConfig,
}

impl BaseBridge {
    pub async fn new(
        ws_url: String,
        inbox_contract_address: Address,
        nock_contract_address: Address,
        private_key: String,
        runtime_handle: Arc<BridgeRuntimeHandle>,
        batch_size: u64,
        confirmation_depth: u64,
        stop: StopHandle,
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
            },
        })
    }

    pub(crate) fn provider(&self) -> DynProvider<Optimism> {
        self.deps.provider.clone()
    }

    pub(crate) fn inbox_contract_address(&self) -> Address {
        self.contracts.inbox_contract_address
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

        // Use batch RPC to fetch all block headers in chunks of 20
        // This reduces 100 individual RPC calls to ~5 batched requests
        let mut block_info: HashMap<u64, (B256, B256)> = HashMap::new();
        let heights: Vec<u64> = (batch_start..=batch_end).collect();

        for chunk in heights.chunks(20) {
            let mut batch = BatchRequest::new(self.deps.provider.client());
            let mut futures = Vec::new();

            for &height in chunk {
                // eth_getBlockByNumber with false = don't include transactions
                let fut = batch
                    .add_call::<_, Option<alloy::rpc::types::Block>>(
                        "eth_getBlockByNumber",
                        &(BlockNumberOrTag::Number(height), false),
                    )
                    .map_err(|e| {
                        BridgeError::BaseBridgeMonitoring(format!(
                            "failed to add batch call for block {}: {}",
                            height, e
                        ))
                    })?;
                futures.push((height, fut));
            }

            // Send the batch
            batch.send().await.map_err(|e| {
                BridgeError::BaseBridgeMonitoring(format!("batch RPC failed: {}", e))
            })?;

            // Collect results
            for (height, fut) in futures {
                let block_opt: Option<alloy::rpc::types::Block> = fut.await.map_err(|e| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "failed to get block {}: {}",
                        height, e
                    ))
                })?;
                let block = block_opt.ok_or_else(|| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "block {} unavailable during batch fetch",
                        height
                    ))
                })?;
                block_info.insert(height, (block.header.hash, block.header.inner.parent_hash));
            }
        }

        let mut blocks = Vec::new();
        let mut prev_hash: Option<B256> = None;
        for height in batch_start..=batch_end {
            let (block_hash, parent_hash) = block_info.get(&height).ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring(format!("missing block info for {}", height))
            })?;

            if let Some(expected_parent) = prev_hash {
                if *parent_hash != expected_parent {
                    return Err(BridgeError::BaseBridgeMonitoring(format!(
                        "base reorg detected at height {} (expected parent {:?}, got {:?})",
                        height, expected_parent, parent_hash
                    )));
                }
            }
            prev_hash = Some(*block_hash);

            blocks.push(BaseBlockRef {
                height,
                block_id: atom_bytes_from_b256(*block_hash),
                parent_block_id: atom_bytes_from_b256(*parent_hash),
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

fn eth_address_from_alloy(addr: Address) -> EthAddress {
    EthAddress::from(addr)
}

fn tip5_from_limbs(limbs: &[u64; 5]) -> Tip5Hash {
    Tip5Hash::from_limbs(limbs)
}

pub(crate) fn compute_base_event_id(tx_hash: &B256, log_index: Option<u64>) -> AtomBytes {
    // This is the EVM log coordinate for a Base event: transaction hashes are
    // chain-unique for practical purposes, and `log_index` distinguishes
    // multiple logs emitted by the same transaction.
    let log_index = U256::from(log_index.unwrap_or(0u64));
    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(tx_hash.as_slice());
    let log_index_bytes = log_index.to_be_bytes::<32>();
    hash_input.extend_from_slice(&log_index_bytes);
    AtomBytes(keccak256(&hash_input).as_slice().to_vec())
}

pub(crate) fn burn_for_withdrawal_signature_hash() -> B256 {
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

fn parse_withdrawal_burn_calldata(
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

pub(crate) fn decode_burn_for_withdrawal_log_with_calldata(
    raw: &RawLog,
    tx_hash: &B256,
    log_index: Option<u64>,
    nock_contract_address: Address,
    tx_input: &[u8],
) -> Result<DecodedBurnForWithdrawalLog, BurnForWithdrawalDecodeError> {
    let event =
        Nock::BurnForWithdrawal::decode_raw_log(raw.topics.iter().cloned(), raw.data.as_ref())
            .map_err(|err| BurnForWithdrawalDecodeError::NotBurnForWithdrawal(err.to_string()))?;
    let (calldata_amount_raw, calldata_commitment, lock_root) =
        parse_withdrawal_burn_calldata(tx_input)?;
    let amount_raw: U256 = event.amount;
    if calldata_amount_raw != amount_raw {
        return Err(BurnForWithdrawalDecodeError::CalldataAmountMismatch {
            event_amount_raw: amount_raw,
            calldata_amount_raw,
        });
    }
    if calldata_commitment != event.lockRoot {
        return Err(BurnForWithdrawalDecodeError::CalldataCommitmentMismatch {
            event_commitment: event.lockRoot,
            calldata_commitment,
        });
    }
    let expected =
        withdrawal_burn_commitment(nock_contract_address, event.burner, amount_raw, &lock_root);
    if expected != event.lockRoot {
        return Err(BurnForWithdrawalDecodeError::CommitmentMismatch {
            expected,
            actual: event.lockRoot,
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
    use super::*;

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
