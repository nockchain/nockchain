use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use alloy::consensus::Transaction as _;
use alloy::primitives::{Address, Bytes, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::eth::{Filter, RawLog};
use alloy::transports::ws::WsConnect;
use async_trait::async_trait;
use backon::Retryable;
use op_alloy::network::Optimism;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{info, info_span, warn, Instrument};

use crate::core::loop_policy::BaseObserverLoopPolicy;
use crate::shared::base::{
    base_automatic_reorg_rewind_depth, burn_for_withdrawal_signature_hash, compute_base_event_id,
    decode_burn_for_withdrawal_event, decode_burn_for_withdrawal_log_with_calldata,
    fetch_base_block_info, validate_base_chain_id, validate_base_log_block_hash,
    BurnForWithdrawalDecodeError,
};
use crate::shared::errors::BridgeError;
use crate::shared::types::{BaseEventId, WITHDRAWAL_POLICY_V1_ID, WITHDRAWAL_WIRE_V1_ID};
use crate::withdrawal::proposals::TrackedWithdrawalRequest;
use crate::withdrawal::sequencer::base_activity::{
    current_unix_timestamp_secs, BaseActivityCursor, BaseActivityHeaderCheckpoint,
    BaseActivityReorgPlan, BaseActivityStore, VerifiedBaseWithdrawalBurn,
};
use crate::withdrawal::sequencer::base_height::SequencerBaseHeightTracker;
use crate::withdrawal::sequencer::base_incidents::RejectedBaseWithdrawalBurn;
use crate::withdrawal::sequencer::store::{
    BaseBurnRecoveryReport, BaseJournalReconciliationOutcome, WithdrawalSequencerStore,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequencerBaseWithdrawalRejection {
    BaseHeightUnavailable,
    EventAboveConfirmed {
        base_batch_end: u64,
        confirmed_base_height: u64,
    },
    MissingBaseEventId {
        base_event_id_hex: String,
        batch_start: u64,
        batch_end: u64,
    },
    Compensated {
        base_event_id_hex: String,
    },
    EventOutsideClaimedBatchWindow {
        event_block: u64,
        batch_start: u64,
        batch_end: u64,
    },
    WrongContractAddress {
        expected: Address,
        actual: Address,
    },
    NotBurnForWithdrawal {
        reason: String,
    },
    AmountNotDivisible {
        amount_raw: String,
    },
    AmountOverflow {
        nicks: String,
    },
    InvalidCalldataTrailer {
        reason: String,
    },
    WrongLockRoot,
    WrongAmount {
        expected_nicks: u64,
        actual_nicks: u64,
    },
    RpcFailure {
        error: String,
    },
}

impl fmt::Display for SequencerBaseWithdrawalRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaseHeightUnavailable => write!(
                f,
                "sequencer base height watcher has not observed a confirmed Base height yet"
            ),
            Self::EventAboveConfirmed {
                base_batch_end,
                confirmed_base_height,
            } => write!(
                f,
                "withdrawal base_batch_end {base_batch_end} is above confirmed Base height {confirmed_base_height}"
            ),
            Self::MissingBaseEventId {
                base_event_id_hex,
                batch_start,
                batch_end,
            } => write!(
                f,
                "withdrawal burn base_event_id {base_event_id_hex} was not found in confirmed Base batch {batch_start}..={batch_end}"
            ),
            Self::Compensated { base_event_id_hex } => write!(
                f,
                "withdrawal burn base_event_id {base_event_id_hex} was compensated and cannot be admitted"
            ),
            Self::EventOutsideClaimedBatchWindow {
                event_block,
                batch_start,
                batch_end,
            } => write!(
                f,
                "withdrawal burn event block {event_block} is outside claimed Base batch {batch_start}..={batch_end}"
            ),
            Self::WrongContractAddress { expected, actual } => write!(
                f,
                "withdrawal burn log came from contract {actual:?}, expected {expected:?}"
            ),
            Self::NotBurnForWithdrawal { reason } => {
                write!(f, "matching Base log is not Nock::BurnForWithdrawal: {reason}")
            }
            Self::AmountNotDivisible { amount_raw } => write!(
                f,
                "BurnForWithdrawal amount {amount_raw} is not exactly divisible by NOCK_BASE_PER_NICK"
            ),
            Self::AmountOverflow { nicks } => {
                write!(f, "BurnForWithdrawal amount {nicks} nicks overflows u64")
            }
            Self::InvalidCalldataTrailer { reason } => {
                write!(f, "BurnForWithdrawal calldata trailer is invalid: {reason}")
            }
            Self::WrongLockRoot => write!(
                f,
                "BurnForWithdrawal lockRoot does not match tracked withdrawal recipient"
            ),
            Self::WrongAmount {
                expected_nicks,
                actual_nicks,
            } => write!(
                f,
                "BurnForWithdrawal amount {actual_nicks} nicks does not match tracked amount {expected_nicks} nicks"
            ),
            Self::RpcFailure { error } => {
                write!(f, "failed to verify withdrawal burn against Base RPC: {error}")
            }
        }
    }
}

impl std::error::Error for SequencerBaseWithdrawalRejection {}

pub(crate) fn sequencer_base_event_id_hex(base_event_id: &BaseEventId) -> String {
    format!("0x{}", hex::encode(&base_event_id.0))
}

#[async_trait]
pub trait SequencerBaseWithdrawalVerifier: Send + Sync {
    async fn verify(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), SequencerBaseWithdrawalRejection>;
}

#[derive(Debug, Clone)]
struct SequencerBaseLog {
    block_number: u64,
    block_hash: B256,
    parent_hash: B256,
    block_timestamp: u64,
    transaction_hash: B256,
    transaction_index: Option<u64>,
    log_index: Option<u64>,
    transaction_input: Bytes,
    raw: RawLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SequencerBaseHeader {
    number: u64,
    hash: B256,
    parent_hash: B256,
    timestamp: u64,
}

#[derive(Debug, Clone)]
struct SequencerBaseLogChunk {
    start: u64,
    end: u64,
    headers: Vec<SequencerBaseHeader>,
    logs: Vec<SequencerBaseLog>,
}

#[async_trait]
trait SequencerBaseLogSource: Send + Sync {
    async fn burn_log_chunk(
        &self,
        batch_start: u64,
        batch_end: u64,
    ) -> Result<SequencerBaseLogChunk, BridgeError>;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SequencerBaseActivityScanReport {
    pub confirmed_tip: u64,
    pub scan_start: u64,
    pub scan_end: u64,
    pub chunks_verified: u64,
    pub blocks_verified: u64,
    pub logs_seen: u64,
    pub burns_inserted: u64,
    pub burns_rejected: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivityBurnOutcome {
    Accepted(VerifiedBaseWithdrawalBurn),
    Rejected(RejectedBaseWithdrawalBurn),
    Compensated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencerBaseRecoveryPassReport {
    pub scan: SequencerBaseActivityScanReport,
    pub recovery: BaseBurnRecoveryReport,
    pub reconciliation: BaseJournalReconciliationOutcome,
}

#[derive(Clone)]
pub struct SequencerBaseRpcWithdrawalVerifier {
    chain_id: u64,
    base_start_height: u64,
    base_height_tracker: Arc<SequencerBaseHeightTracker>,
    base_blocks_chunk: u64,
    automatic_rewind_depth: u64,
    nock_contract_address: Address,
    log_source: Arc<dyn SequencerBaseLogSource>,
}

impl SequencerBaseRpcWithdrawalVerifier {
    pub async fn connect(
        ws_url: String,
        expected_chain_id: u64,
        nock_contract_address: Address,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_start_height: u64,
        base_blocks_chunk: u64,
        base_confirmation_depth: u64,
    ) -> Result<Self, BridgeError> {
        if base_blocks_chunk == 0 {
            return Err(BridgeError::Config(
                "base_blocks_chunk must be greater than 0".into(),
            ));
        }
        let policy = BaseObserverLoopPolicy::default();
        let connect = || async {
            ProviderBuilder::<_, _, Optimism>::default()
                .connect_ws(WsConnect::new(ws_url.clone()))
                .await
        };
        let provider = connect
            .retry(policy.rpc_retry.exponential_builder())
            .notify(|err, dur| {
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_verifier",
                    error = %err,
                    backoff_secs = dur.as_secs(),
                    "failed to connect sequencer Base withdrawal verifier, will retry"
                );
            })
            .await
            .map(|provider| provider.erased())
            .map_err(|err| {
                BridgeError::Runtime(format!(
                    "failed to connect sequencer Base withdrawal verifier at {ws_url}: {err}"
                ))
            })?;
        validate_base_chain_id(&provider, expected_chain_id, "sequencer Base verifier").await?;
        let mut verifier = Self::with_log_source(
            expected_chain_id,
            base_start_height,
            base_height_tracker,
            base_blocks_chunk,
            nock_contract_address,
            Arc::new(RpcSequencerBaseLogSource {
                provider,
                nock_contract_address,
            }),
        );
        verifier.automatic_rewind_depth =
            base_automatic_reorg_rewind_depth(base_confirmation_depth);
        Ok(verifier)
    }

    fn with_log_source(
        chain_id: u64,
        base_start_height: u64,
        base_height_tracker: Arc<SequencerBaseHeightTracker>,
        base_blocks_chunk: u64,
        nock_contract_address: Address,
        log_source: Arc<dyn SequencerBaseLogSource>,
    ) -> Self {
        Self {
            chain_id,
            base_start_height,
            base_height_tracker,
            base_blocks_chunk,
            automatic_rewind_depth: crate::shared::base::BASE_MAX_AUTOMATIC_REORG_REWIND_BLOCKS,
            nock_contract_address,
            log_source,
        }
    }

    fn claimed_batch_window(&self, base_batch_end: u64) -> (u64, u64) {
        let batch_start = base_batch_end.saturating_sub(self.base_blocks_chunk.saturating_sub(1));
        (batch_start, base_batch_end)
    }

    pub async fn scan_confirmed_burn_tail(
        &self,
        store: &BaseActivityStore,
        overlap_blocks: u64,
    ) -> Result<SequencerBaseActivityScanReport, BridgeError> {
        if overlap_blocks == 0 {
            return Err(BridgeError::Config(
                "Base activity overlap must be greater than 0".into(),
            ));
        }
        let confirmed_tip = self
            .base_height_tracker
            .latest_confirmed_base_height()
            .ok_or_else(|| {
                BridgeError::Runtime("confirmed Base height unavailable for activity scan".into())
            })?;
        let existing_cursor = store
            .load_cursor(self.chain_id, self.nock_contract_address)
            .await?;
        if let Some(cursor) = &existing_cursor {
            if cursor.last_verified_block < self.base_start_height {
                return Err(BridgeError::Runtime(format!(
                    "Base activity cursor {} precedes configured deployment start {}",
                    cursor.last_verified_block, self.base_start_height
                )));
            }
            if cursor.last_verified_block > confirmed_tip {
                return Err(BridgeError::Runtime(format!(
                    "confirmed Base tip {confirmed_tip} is behind activity cursor {}",
                    cursor.last_verified_block
                )));
            }
        }
        if confirmed_tip < self.base_start_height {
            return Ok(SequencerBaseActivityScanReport {
                confirmed_tip,
                scan_start: self.base_start_height,
                scan_end: confirmed_tip,
                ..SequencerBaseActivityScanReport::default()
            });
        }

        let scan_start = existing_cursor
            .as_ref()
            .map(|cursor| {
                cursor
                    .last_verified_block
                    .saturating_sub(overlap_blocks)
                    .max(self.base_start_height)
            })
            .unwrap_or(self.base_start_height);
        let compensated = store
            .incident_store()
            .list_compensated_withdrawals(self.chain_id, self.nock_contract_address)
            .await?
            .into_iter()
            .map(|record| record.base_event_id)
            .collect::<HashSet<_>>();
        let mut report = SequencerBaseActivityScanReport {
            confirmed_tip,
            scan_start,
            scan_end: confirmed_tip,
            ..SequencerBaseActivityScanReport::default()
        };
        let mut cursor_hash_verified = existing_cursor.is_none();
        let mut previous_chunk_end_hash = None;
        let verified_at = current_unix_timestamp_secs()?;
        let mut pending_records = Vec::new();
        let mut pending_rejections = Vec::new();
        let mut pending_headers = Vec::new();
        let mut chunk_start = scan_start;
        while chunk_start <= confirmed_tip {
            let chunk_end = chunk_start
                .saturating_add(self.base_blocks_chunk.saturating_sub(1))
                .min(confirmed_tip);
            let chunk = self
                .log_source
                .burn_log_chunk(chunk_start, chunk_end)
                .await?;
            validate_activity_chunk_headers(
                &chunk, chunk_start, chunk_end, previous_chunk_end_hash,
            )?;
            pending_headers.extend(chunk.headers.iter().map(|header| {
                BaseActivityHeaderCheckpoint {
                    chain_id: self.chain_id,
                    nock_contract_address: self.nock_contract_address,
                    block_number: header.number,
                    block_hash: header.hash,
                    parent_hash: header.parent_hash,
                    block_timestamp: header.timestamp,
                    verified_at,
                }
            }));
            if let Some(cursor) = &existing_cursor {
                if cursor.last_verified_block >= chunk_start
                    && cursor.last_verified_block <= chunk_end
                {
                    let header = header_at(&chunk, cursor.last_verified_block)?;
                    if header.hash != cursor.last_verified_block_hash {
                        return Err(BridgeError::BaseBridgeMonitoring(format!(
                            "Base activity reorg detected at cursor block {}: expected {:?}, got {:?}",
                            cursor.last_verified_block,
                            cursor.last_verified_block_hash,
                            header.hash
                        )));
                    }
                    cursor_hash_verified = true;
                }
            }

            report.logs_seen =
                report
                    .logs_seen
                    .saturating_add(u64::try_from(chunk.logs.len()).map_err(|err| {
                        BridgeError::ValueConversion(format!("Base log count overflow: {err}"))
                    })?);
            for log in &chunk.logs {
                let header = header_at(&chunk, log.block_number)?;
                if log.block_hash != header.hash
                    || log.parent_hash != header.parent_hash
                    || log.block_timestamp != header.timestamp
                {
                    return Err(BridgeError::BaseBridgeMonitoring(format!(
                        "Base burn log header mismatch at block {}",
                        log.block_number
                    )));
                }
                match self.decode_activity_burn(log, verified_at, &compensated)? {
                    ActivityBurnOutcome::Accepted(record) => pending_records.push(record),
                    ActivityBurnOutcome::Rejected(record) => pending_rejections.push(record),
                    ActivityBurnOutcome::Compensated => {}
                }
            }
            let end_header = header_at(&chunk, chunk_end)?;
            if cursor_hash_verified {
                let cursor = match &existing_cursor {
                    Some(existing) if existing.last_verified_block > chunk_end => existing.clone(),
                    _ => BaseActivityCursor {
                        chain_id: self.chain_id,
                        nock_contract_address: self.nock_contract_address,
                        last_verified_block: chunk_end,
                        last_verified_block_hash: end_header.hash,
                        updated_at: verified_at,
                    },
                };
                report.burns_rejected = report.burns_rejected.saturating_add(
                    store
                        .incident_store()
                        .record_rejected_burns(std::mem::take(&mut pending_rejections))
                        .await?,
                );
                let retained_from_block = cursor
                    .last_verified_block
                    .saturating_sub(self.automatic_rewind_depth)
                    .max(self.base_start_height);
                report.burns_inserted = report.burns_inserted.saturating_add(
                    store
                        .apply_verified_chunk_with_headers(
                            std::mem::take(&mut pending_records),
                            std::mem::take(&mut pending_headers),
                            cursor,
                            retained_from_block,
                        )
                        .await?,
                );
            }
            report.chunks_verified = report.chunks_verified.saturating_add(1);
            report.blocks_verified = report.blocks_verified.saturating_add(
                chunk_end
                    .checked_sub(chunk_start)
                    .and_then(|count| count.checked_add(1))
                    .ok_or_else(|| {
                        BridgeError::ValueConversion(
                            "Base activity verified block count overflow".into(),
                        )
                    })?,
            );
            previous_chunk_end_hash = Some(end_header.hash);
            if chunk_end == u64::MAX {
                break;
            }
            chunk_start = chunk_end + 1;
        }
        if !cursor_hash_verified {
            return Err(BridgeError::BaseBridgeMonitoring(
                "Base activity overlap did not revalidate the persisted cursor hash".into(),
            ));
        }
        Ok(report)
    }

    fn decode_activity_burn(
        &self,
        log: &SequencerBaseLog,
        verified_at: i64,
        compensated: &HashSet<BaseEventId>,
    ) -> Result<ActivityBurnOutcome, BridgeError> {
        if log.raw.address != self.nock_contract_address {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "Base burn log at block {} came from {:?}, expected {:?}",
                log.block_number, log.raw.address, self.nock_contract_address
            )));
        }
        let base_event_id = compute_base_event_id(&log.transaction_hash, log.log_index);
        if compensated.contains(&base_event_id) {
            return Ok(ActivityBurnOutcome::Compensated);
        }
        let log_index = log.log_index.ok_or_else(|| {
            BridgeError::BaseBridgeMonitoring(format!(
                "withdrawal burn {:?} is missing log index",
                base_event_id
            ))
        })?;
        let tx_index = log.transaction_index.ok_or_else(|| {
            BridgeError::BaseBridgeMonitoring(format!(
                "withdrawal burn {:?} is missing transaction index",
                base_event_id
            ))
        })?;
        let event_facts = decode_burn_for_withdrawal_event(&log.raw).ok();
        let decoded = match decode_burn_for_withdrawal_log_with_calldata(
            &log.raw,
            &log.transaction_hash,
            Some(log_index),
            self.nock_contract_address,
            log.transaction_input.as_ref(),
        ) {
            Ok(decoded) => decoded,
            Err(error) => {
                crate::observability::metrics::init_metrics()
                    .sequencer_withdrawal_base_activity_rejected
                    .increment();
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_activity",
                    chain_id = self.chain_id,
                    contract = %self.nock_contract_address,
                    base_event_id = %sequencer_base_event_id_hex(&base_event_id),
                    tx_hash = %log.transaction_hash,
                    log_index,
                    block_number = log.block_number,
                    rejection_code = error.code(),
                    rejection_detail = %error,
                    "quarantined unsupported Base withdrawal burn"
                );
                return Ok(ActivityBurnOutcome::Rejected(RejectedBaseWithdrawalBurn {
                    chain_id: self.chain_id,
                    nock_contract_address: self.nock_contract_address,
                    base_event_id,
                    block_number: log.block_number,
                    block_hash: log.block_hash,
                    parent_hash: log.parent_hash,
                    observed_at_unix_secs: Some(log.block_timestamp),
                    tx_hash: log.transaction_hash,
                    tx_index,
                    log_index,
                    burner: event_facts.as_ref().map(|event| event.burner),
                    amount_base_units: event_facts
                        .as_ref()
                        .map(|event| event.amount_raw.to_string()),
                    commitment: event_facts.as_ref().map(|event| event.commitment),
                    calldata: log.transaction_input.to_vec(),
                    rejection_code: error.code().to_string(),
                    rejection_detail: error.to_string(),
                    first_observed_at: verified_at,
                    last_observed_at: verified_at,
                }));
            }
        };
        let amount_base_units = event_facts
            .map(|event| event.amount_raw.to_string())
            .ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring(format!(
                    "verified withdrawal burn {:?} could not decode event facts",
                    decoded.base_event_id
                ))
            })?;
        Ok(ActivityBurnOutcome::Accepted(VerifiedBaseWithdrawalBurn {
            chain_id: self.chain_id,
            nock_contract_address: self.nock_contract_address,
            base_event_id: decoded.base_event_id,
            block_number: log.block_number,
            block_hash: log.block_hash,
            parent_hash: log.parent_hash,
            observed_at_unix_secs: Some(log.block_timestamp),
            tx_hash: log.transaction_hash,
            tx_index,
            log_index,
            burner: Address::from(decoded.burner),
            amount_base_units,
            amount_nicks: decoded.amount,
            lock_root: decoded.lock_root,
            calldata: log.transaction_input.to_vec(),
            base_batch_end: self.canonical_batch_end(log.block_number)?,
            withdrawal_nonce: None,
            verified_at,
            policy_id: Some(WITHDRAWAL_POLICY_V1_ID.to_string()),
            protocol_id: Some(WITHDRAWAL_WIRE_V1_ID.to_string()),
        }))
    }

    async fn plan_confirmed_activity_reorg(
        &self,
        store: &BaseActivityStore,
        activation_block: u64,
    ) -> Result<Option<BaseActivityReorgPlan>, BridgeError> {
        let Some(cursor) = store
            .load_cursor(self.chain_id, self.nock_contract_address)
            .await?
        else {
            return Ok(None);
        };
        let checkpoint_start = cursor
            .last_verified_block
            .saturating_sub(self.automatic_rewind_depth)
            .max(self.base_start_height);
        let checkpoints = store
            .load_header_checkpoints(
                self.chain_id, self.nock_contract_address, checkpoint_start,
                cursor.last_verified_block,
            )
            .await?;
        if checkpoints.is_empty() {
            // Legacy cursors acquire a bounded checkpoint window on their next
            // successful overlap scan. Until then the existing fail-closed
            // cursor comparison remains authoritative.
            return Ok(None);
        }
        let (Some(oldest), Some(newest)) = (checkpoints.first(), checkpoints.last()) else {
            return Ok(None);
        };
        if newest.block_number != cursor.last_verified_block
            || newest.block_hash != cursor.last_verified_block_hash
        {
            return Err(BridgeError::Runtime(format!(
                "Base reorg checkpoint/cursor mismatch: cursor {} {:?}, newest checkpoint {} {:?}",
                cursor.last_verified_block,
                cursor.last_verified_block_hash,
                newest.block_number,
                newest.block_hash
            )));
        }
        let confirmed_tip = self
            .base_height_tracker
            .latest_confirmed_base_height()
            .ok_or_else(|| {
                BridgeError::Runtime("confirmed Base height unavailable for reorg planning".into())
            })?;
        if confirmed_tip < cursor.last_verified_block {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "deep Base reorg: confirmed tip {confirmed_tip} is behind cursor {} {:?}; retained checkpoint range {}..={} max_depth={}",
                cursor.last_verified_block,
                cursor.last_verified_block_hash,
                oldest.block_number,
                newest.block_number,
                self.automatic_rewind_depth
            )));
        }
        let canonical = self
            .log_source
            .burn_log_chunk(checkpoint_start, cursor.last_verified_block)
            .await?;
        validate_activity_chunk_headers(
            &canonical, checkpoint_start, cursor.last_verified_block, None,
        )?;
        let canonical_cursor = header_at(&canonical, cursor.last_verified_block)?;
        if canonical_cursor.hash == cursor.last_verified_block_hash {
            return Ok(None);
        }
        let common_ancestor = checkpoints.iter().rev().find_map(|checkpoint| {
            let canonical_header = header_at(&canonical, checkpoint.block_number).ok()?;
            (canonical_header.hash == checkpoint.block_hash).then_some(checkpoint.clone())
        });
        let Some(common_ancestor) = common_ancestor else {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "deep Base reorg: no common ancestor in retained checkpoint range {}..={}; cursor_hash={:?} canonical_cursor_hash={:?} max_depth={} activation_block={activation_block}",
                oldest.block_number,
                newest.block_number,
                cursor.last_verified_block_hash,
                canonical_cursor.hash,
                self.automatic_rewind_depth
            )));
        };
        let rewind_depth = cursor
            .last_verified_block
            .saturating_sub(common_ancestor.block_number);
        if rewind_depth > self.automatic_rewind_depth {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "deep Base reorg: cursor={} ancestor={} depth={} exceeds max_depth={}; cursor_hash={:?} ancestor_hash={:?}",
                cursor.last_verified_block,
                common_ancestor.block_number,
                rewind_depth,
                self.automatic_rewind_depth,
                cursor.last_verified_block_hash,
                common_ancestor.block_hash
            )));
        }
        if common_ancestor.block_number < activation_block
            && cursor.last_verified_block >= activation_block
        {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "Base reorg crosses withdrawal activation boundary: cursor={} ancestor={} activation_block={} depth={}",
                cursor.last_verified_block,
                common_ancestor.block_number,
                activation_block,
                rewind_depth
            )));
        }
        let detected_at = current_unix_timestamp_secs()?;
        Ok(Some(BaseActivityReorgPlan {
            chain_id: self.chain_id,
            nock_contract_address: self.nock_contract_address,
            old_cursor: cursor,
            common_ancestor,
            canonical_cursor_header: BaseActivityHeaderCheckpoint {
                chain_id: self.chain_id,
                nock_contract_address: self.nock_contract_address,
                block_number: canonical_cursor.number,
                block_hash: canonical_cursor.hash,
                parent_hash: canonical_cursor.parent_hash,
                block_timestamp: canonical_cursor.timestamp,
                verified_at: detected_at,
            },
            rewind_depth,
            detected_at,
        }))
    }

    pub async fn scan_and_recover_confirmed_burns(
        &self,
        activity_store: &BaseActivityStore,
        sequencer_store: &WithdrawalSequencerStore,
        overlap_blocks: u64,
        activation_block: u64,
    ) -> Result<SequencerBaseRecoveryPassReport, BridgeError> {
        sequencer_store.ensure_reorg_ready().await?;
        let plan = match self
            .plan_confirmed_activity_reorg(activity_store, activation_block)
            .await
        {
            Ok(plan) => plan,
            Err(error)
                if error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("base reorg") =>
            {
                sequencer_store
                    .activate_base_reorg_guard(
                        self.chain_id,
                        self.nock_contract_address,
                        error.to_string(),
                    )
                    .await?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if let Some(plan) = plan {
            let report = sequencer_store.apply_base_activity_reorg(plan).await?;
            warn!(
                target: "nockchain.withdrawal_sequencer.base_activity",
                generation = report.generation,
                old_cursor_block = report.old_cursor_block,
                common_ancestor_block = report.common_ancestor_block,
                rewind_depth = report.rewind_depth,
                burns_invalidated = report.burns_invalidated,
                lifecycle_rows_invalidated = report.lifecycle_rows_invalidated,
                "completed bounded Base activity rewind"
            );
        }
        let scan = self
            .scan_confirmed_burn_tail(activity_store, overlap_blocks)
            .await?;
        let recovery = self
            .recover_unmatched_base_burns(sequencer_store, activation_block, scan.confirmed_tip)
            .await?;
        let reconciliation = sequencer_store
            .reconcile_journal_with_base(
                self.chain_id, self.nock_contract_address, activation_block,
            )
            .await?;
        Ok(SequencerBaseRecoveryPassReport {
            scan,
            recovery,
            reconciliation,
        })
    }

    pub async fn recover_unmatched_base_burns(
        &self,
        store: &WithdrawalSequencerStore,
        activation_block: u64,
        turn_started_base_height: u64,
    ) -> Result<BaseBurnRecoveryReport, BridgeError> {
        store
            .recover_unmatched_base_burns(
                self.chain_id, self.nock_contract_address, activation_block,
                turn_started_base_height,
            )
            .await
    }

    fn canonical_batch_end(&self, block_number: u64) -> Result<u64, BridgeError> {
        let offset = block_number
            .checked_sub(self.base_start_height)
            .ok_or_else(|| {
                BridgeError::ValueConversion(format!(
                    "Base block {block_number} precedes configured start {}",
                    self.base_start_height
                ))
            })?;
        let batch_index = offset / self.base_blocks_chunk;
        self.base_start_height
            .checked_add(
                batch_index
                    .checked_add(1)
                    .and_then(|index| index.checked_mul(self.base_blocks_chunk))
                    .ok_or_else(|| {
                        BridgeError::ValueConversion(
                            "Base activity batch-end multiplication overflow".into(),
                        )
                    })?,
            )
            .and_then(|end_exclusive| end_exclusive.checked_sub(1))
            .ok_or_else(|| BridgeError::ValueConversion("Base activity batch-end overflow".into()))
    }
}

fn validate_activity_chunk_headers(
    chunk: &SequencerBaseLogChunk,
    expected_start: u64,
    expected_end: u64,
    previous_chunk_end_hash: Option<B256>,
) -> Result<(), BridgeError> {
    if chunk.start != expected_start || chunk.end != expected_end {
        return Err(BridgeError::BaseBridgeMonitoring(format!(
            "Base activity source returned range {}..={} for requested {}..={}",
            chunk.start, chunk.end, expected_start, expected_end
        )));
    }
    let expected_count = expected_end
        .checked_sub(expected_start)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| {
            BridgeError::ValueConversion("Base activity header count overflow".into())
        })?;
    if u64::try_from(chunk.headers.len()).map_err(|err| {
        BridgeError::ValueConversion(format!("Base activity header count overflow: {err}"))
    })? != expected_count
    {
        return Err(BridgeError::BaseBridgeMonitoring(format!(
            "Base activity source returned {} headers for {} blocks",
            chunk.headers.len(),
            expected_count
        )));
    }
    let mut prior_hash = previous_chunk_end_hash;
    for (offset, header) in chunk.headers.iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|err| {
            BridgeError::ValueConversion(format!("Base activity header offset overflow: {err}"))
        })?;
        let expected_number = expected_start.checked_add(offset).ok_or_else(|| {
            BridgeError::ValueConversion("Base activity header height overflow".into())
        })?;
        if header.number != expected_number {
            return Err(BridgeError::BaseBridgeMonitoring(format!(
                "Base activity header gap: expected {expected_number}, got {}",
                header.number
            )));
        }
        if let Some(expected_parent) = prior_hash {
            if header.parent_hash != expected_parent {
                return Err(BridgeError::BaseBridgeMonitoring(format!(
                    "Base activity parent mismatch at block {}: expected {:?}, got {:?}",
                    header.number, expected_parent, header.parent_hash
                )));
            }
        }
        prior_hash = Some(header.hash);
    }
    Ok(())
}

fn header_at(
    chunk: &SequencerBaseLogChunk,
    block_number: u64,
) -> Result<SequencerBaseHeader, BridgeError> {
    let offset = block_number.checked_sub(chunk.start).ok_or_else(|| {
        BridgeError::BaseBridgeMonitoring(format!(
            "Base block {block_number} precedes activity chunk start {}",
            chunk.start
        ))
    })?;
    let offset = usize::try_from(offset).map_err(|err| {
        BridgeError::ValueConversion(format!("Base activity header offset overflow: {err}"))
    })?;
    let header = chunk.headers.get(offset).copied().ok_or_else(|| {
        BridgeError::BaseBridgeMonitoring(format!(
            "Base activity chunk {}..={} has no header for block {block_number}",
            chunk.start, chunk.end
        ))
    })?;
    if header.number != block_number {
        return Err(BridgeError::BaseBridgeMonitoring(format!(
            "Base activity header index mismatch: requested {block_number}, got {}",
            header.number
        )));
    }
    Ok(header)
}

pub async fn run_confirmed_base_burn_tail_scanner(
    scanner: Arc<SequencerBaseRpcWithdrawalVerifier>,
    activity_store: Arc<BaseActivityStore>,
    sequencer_store: Arc<WithdrawalSequencerStore>,
    overlap_blocks: u64,
    activation_block: u64,
    policy: BaseObserverLoopPolicy,
) {
    let mut ticker = interval(policy.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match scanner
            .scan_and_recover_confirmed_burns(
                activity_store.as_ref(),
                sequencer_store.as_ref(),
                overlap_blocks,
                activation_block,
            )
            .await
        {
            Ok(report) => match &report.reconciliation {
                BaseJournalReconciliationOutcome::Ready(reconciliation) => {
                    info!(
                        target: "nockchain.withdrawal_sequencer.base_activity",
                        confirmed_tip = report.scan.confirmed_tip,
                        scan_start = report.scan.scan_start,
                        scan_end = report.scan.scan_end,
                        chunks_verified = report.scan.chunks_verified,
                        blocks_verified = report.scan.blocks_verified,
                        logs_seen = report.scan.logs_seen,
                        burns_inserted = report.scan.burns_inserted,
                        burns_rejected = report.scan.burns_rejected,
                        recovery_candidates = report.recovery.candidates_inspected,
                        recovered_pending = report.recovery.recovered_pending,
                        already_registered = report.recovery.already_registered,
                        ineligible = report.recovery.ineligible,
                        journal_sequence = reconciliation.journal_sequence,
                        base_cursor_block = reconciliation.base_cursor_block,
                        lifecycle_rows_validated = reconciliation.rows_validated,
                        "verified Base burn tail and reconciled journal lifecycle"
                    );
                }
                BaseJournalReconciliationOutcome::ScannerBehind {
                    current_verified_block,
                    required_base_batch_end,
                } => {
                    info!(
                        target: "nockchain.withdrawal_sequencer.base_activity",
                        current_verified_block = ?current_verified_block,
                        required_base_batch_end,
                        "Base scanner remains behind journal lifecycle; readiness frontier not advanced"
                    );
                }
            },
            Err(err) => {
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_activity",
                    error = %err,
                    "confirmed Base burn scan/recovery failed; readiness state retained"
                );
            }
        }
    }
}

#[async_trait]
impl SequencerBaseWithdrawalVerifier for SequencerBaseRpcWithdrawalVerifier {
    async fn verify(
        &self,
        tracked: &TrackedWithdrawalRequest,
    ) -> Result<(), SequencerBaseWithdrawalRejection> {
        let confirmed_base_height = self
            .base_height_tracker
            .latest_confirmed_base_height()
            .ok_or(SequencerBaseWithdrawalRejection::BaseHeightUnavailable)?;
        if tracked.base_batch_end > confirmed_base_height {
            return Err(SequencerBaseWithdrawalRejection::EventAboveConfirmed {
                base_batch_end: tracked.base_batch_end,
                confirmed_base_height,
            });
        }

        let (batch_start, batch_end) = self.claimed_batch_window(tracked.base_batch_end);
        let logs = self
            .log_source
            .burn_log_chunk(batch_start, batch_end)
            .instrument(info_span!(
                "sequencer_verify_base_withdrawal",
                withdrawal_nonce = tracked.withdrawal_nonce,
                base_batch_end = tracked.base_batch_end
            ))
            .await
            .map_err(|err| SequencerBaseWithdrawalRejection::RpcFailure {
                error: err.to_string(),
            })?
            .logs;

        for log in logs {
            let base_event_id = compute_base_event_id(&log.transaction_hash, log.log_index);
            if base_event_id != tracked.id.base_event_id {
                continue;
            }
            if log.raw.address != self.nock_contract_address {
                return Err(SequencerBaseWithdrawalRejection::WrongContractAddress {
                    expected: self.nock_contract_address,
                    actual: log.raw.address,
                });
            }
            if log.block_number < batch_start || log.block_number > batch_end {
                return Err(
                    SequencerBaseWithdrawalRejection::EventOutsideClaimedBatchWindow {
                        event_block: log.block_number,
                        batch_start,
                        batch_end,
                    },
                );
            }
            let decoded = decode_burn_for_withdrawal_log_with_calldata(
                &log.raw,
                &log.transaction_hash,
                log.log_index,
                self.nock_contract_address,
                log.transaction_input.as_ref(),
            )
            .map_err(decode_error_to_rejection)?;
            if decoded.lock_root != tracked.recipient {
                return Err(SequencerBaseWithdrawalRejection::WrongLockRoot);
            }
            if decoded.amount != tracked.amount {
                return Err(SequencerBaseWithdrawalRejection::WrongAmount {
                    expected_nicks: tracked.amount,
                    actual_nicks: decoded.amount,
                });
            }
            info!(
                target: "nockchain.withdrawal_sequencer.base_verifier",
                withdrawal_nonce = tracked.withdrawal_nonce,
                base_batch_end = tracked.base_batch_end,
                "accepted withdrawal burn from Base RPC; withdrawal id.as_of is bridge kernel context, not sequencer identity"
            );
            return Ok(());
        }

        Err(SequencerBaseWithdrawalRejection::MissingBaseEventId {
            base_event_id_hex: sequencer_base_event_id_hex(&tracked.id.base_event_id),
            batch_start,
            batch_end,
        })
    }
}

fn decode_error_to_rejection(
    err: BurnForWithdrawalDecodeError,
) -> SequencerBaseWithdrawalRejection {
    let reason = err.to_string();
    match err {
        BurnForWithdrawalDecodeError::NotBurnForWithdrawal(reason) => {
            SequencerBaseWithdrawalRejection::NotBurnForWithdrawal { reason }
        }
        BurnForWithdrawalDecodeError::AmountNotDivisible { amount_raw } => {
            SequencerBaseWithdrawalRejection::AmountNotDivisible {
                amount_raw: amount_raw.to_string(),
            }
        }
        BurnForWithdrawalDecodeError::AmountOverflow { nicks } => {
            SequencerBaseWithdrawalRejection::AmountOverflow {
                nicks: nicks.to_string(),
            }
        }
        BurnForWithdrawalDecodeError::MissingCalldataTrailer { .. }
        | BurnForWithdrawalDecodeError::MalformedCalldata { .. }
        | BurnForWithdrawalDecodeError::CalldataAmountMismatch { .. }
        | BurnForWithdrawalDecodeError::CalldataCommitmentMismatch { .. }
        | BurnForWithdrawalDecodeError::CommitmentMismatch { .. }
        | BurnForWithdrawalDecodeError::InvalidLockRoot { .. } => {
            SequencerBaseWithdrawalRejection::InvalidCalldataTrailer { reason }
        }
    }
}

struct RpcSequencerBaseLogSource {
    provider: DynProvider<Optimism>,
    nock_contract_address: Address,
}

#[async_trait]
impl SequencerBaseLogSource for RpcSequencerBaseLogSource {
    async fn burn_log_chunk(
        &self,
        batch_start: u64,
        batch_end: u64,
    ) -> Result<SequencerBaseLogChunk, BridgeError> {
        let filter = Filter::new()
            .address(self.nock_contract_address)
            .event_signature(burn_for_withdrawal_signature_hash())
            .from_block(batch_start)
            .to_block(batch_end);
        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|err| BridgeError::BaseBridgeMonitoring(err.to_string()))?;
        let block_info = fetch_base_block_info(&self.provider, batch_start, batch_end).await?;
        let headers = (batch_start..=batch_end)
            .map(|number| {
                let header = block_info.get(&number).copied().ok_or_else(|| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "missing Base block header for activity scan height {number}"
                    ))
                })?;
                Ok(SequencerBaseHeader {
                    number,
                    hash: header.hash,
                    parent_hash: header.parent_hash,
                    timestamp: header.timestamp,
                })
            })
            .collect::<Result<Vec<_>, BridgeError>>()?;
        let mut out = Vec::with_capacity(logs.len());
        for log in logs {
            let block_number = log.block_number.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("Base burn log missing block number".into())
            })?;
            validate_base_log_block_hash(
                &block_info, batch_start, batch_end, block_number, log.block_hash,
            )?;
            let header = block_info.get(&block_number).copied().ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring(format!(
                    "Base burn log block {block_number} missing verified header"
                ))
            })?;
            let transaction_hash = log.transaction_hash.ok_or_else(|| {
                BridgeError::BaseBridgeMonitoring("Base burn log missing transaction hash".into())
            })?;
            let tx = self
                .provider
                .get_transaction_by_hash(transaction_hash)
                .await
                .map_err(|err| BridgeError::BaseBridgeMonitoring(err.to_string()))?
                .ok_or_else(|| {
                    BridgeError::BaseBridgeMonitoring(format!(
                        "Base burn transaction {transaction_hash:?} unavailable"
                    ))
                })?;
            out.push(SequencerBaseLog {
                block_number,
                block_hash: header.hash,
                parent_hash: header.parent_hash,
                block_timestamp: header.timestamp,
                transaction_hash,
                transaction_index: log.transaction_index,
                log_index: log.log_index,
                transaction_input: tx.input().clone(),
                raw: RawLog {
                    address: log.address(),
                    topics: log.topics().to_vec(),
                    data: log.data().data.clone(),
                },
            });
        }
        Ok(SequencerBaseLogChunk {
            start: batch_start,
            end: batch_end,
            headers,
            logs: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use alloy::primitives::{Bytes, U256};

    use super::*;
    use crate::shared::base::{
        encode_withdrawal_burn_calldata, NOCK_BASE_PER_NICK, WITHDRAWAL_BURN_BASE_CALLDATA_LEN,
    };
    use crate::shared::types::{BaseEventId, Tip5Hash};
    use crate::withdrawal::types::WithdrawalId;

    struct MockLogSource {
        logs: Mutex<Vec<SequencerBaseLog>>,
        error: Mutex<Option<String>>,
        requests: Mutex<Vec<(u64, u64)>>,
        header_hash_offset: Mutex<u64>,
        fork_from: Mutex<Option<u64>>,
        omitted_header: Mutex<Option<u64>>,
        filter_logs_to_range: bool,
    }

    #[async_trait]
    impl SequencerBaseLogSource for MockLogSource {
        async fn burn_log_chunk(
            &self,
            batch_start: u64,
            batch_end: u64,
        ) -> Result<SequencerBaseLogChunk, BridgeError> {
            self.requests
                .lock()
                .expect("mock requests lock")
                .push((batch_start, batch_end));
            if let Some(error) = self.error.lock().expect("mock error lock").clone() {
                return Err(BridgeError::Runtime(error));
            }
            let offset = *self
                .header_hash_offset
                .lock()
                .expect("mock header offset lock");
            let fork_from = *self.fork_from.lock().expect("mock fork start lock");
            let omitted_header = *self
                .omitted_header
                .lock()
                .expect("mock omitted header lock");
            let headers = (batch_start..=batch_end)
                .filter(|number| Some(*number) != omitted_header)
                .map(|number| {
                    let hash_offset = if fork_from.is_some_and(|fork| number >= fork) {
                        offset
                    } else {
                        0
                    };
                    let parent_number = number.saturating_sub(1);
                    let parent_offset = if fork_from.is_some_and(|fork| parent_number >= fork) {
                        offset
                    } else {
                        0
                    };
                    SequencerBaseHeader {
                        number,
                        hash: mock_block_hash(number, hash_offset),
                        parent_hash: mock_block_hash(parent_number, parent_offset),
                        timestamp: 1_700_000_000u64.saturating_add(number),
                    }
                })
                .collect();
            let logs = self.logs.lock().expect("mock logs lock");
            let logs = if self.filter_logs_to_range {
                logs.iter()
                    .filter(|log| log.block_number >= batch_start && log.block_number <= batch_end)
                    .cloned()
                    .collect()
            } else {
                logs.clone()
            };
            Ok(SequencerBaseLogChunk {
                start: batch_start,
                end: batch_end,
                headers,
                logs,
            })
        }
    }

    fn mock_log_source_with_filter(
        logs: Vec<SequencerBaseLog>,
        filter_logs_to_range: bool,
    ) -> Arc<MockLogSource> {
        Arc::new(MockLogSource {
            logs: Mutex::new(logs),
            error: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            header_hash_offset: Mutex::new(0),
            fork_from: Mutex::new(Some(0)),
            omitted_header: Mutex::new(None),
            filter_logs_to_range,
        })
    }

    fn mock_log_source(logs: Vec<SequencerBaseLog>) -> Arc<MockLogSource> {
        mock_log_source_with_filter(logs, false)
    }

    fn mock_scanner_source(logs: Vec<SequencerBaseLog>) -> Arc<MockLogSource> {
        mock_log_source_with_filter(logs, true)
    }

    fn b256_from_u64(value: u64) -> B256 {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        B256::from(bytes)
    }

    fn mock_block_hash(block_number: u64, offset: u64) -> B256 {
        b256_from_u64(
            0x100_000u64
                .saturating_add(block_number)
                .saturating_add(offset),
        )
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

    fn tip5_from_b256(value: B256) -> Tip5Hash {
        let bytes: [u8; 32] = value.as_slice().try_into().expect("B256 is 32 bytes");
        Tip5Hash::from_be_bytes(&bytes)
    }

    fn burn_log(block_number: u64, amount_raw: U256, lock_root: B256) -> SequencerBaseLog {
        let tx_hash = b256_from_u64(0xabc0 + block_number);
        let log_index = Some(3);
        let nock_contract_address = Address::ZERO;
        let burner = address_from_u64(0xbeef);
        let recipient = tip5_from_b256(lock_root);
        let calldata =
            encode_withdrawal_burn_calldata(nock_contract_address, burner, amount_raw, &recipient);
        let commitment = B256::from_slice(&calldata[36..68]);
        let topics = vec![burn_for_withdrawal_signature_hash(), address_topic(burner), commitment];
        SequencerBaseLog {
            block_number,
            block_hash: mock_block_hash(block_number, 0),
            parent_hash: mock_block_hash(block_number.saturating_sub(1), 0),
            block_timestamp: 1_700_000_000u64.saturating_add(block_number),
            transaction_hash: tx_hash,
            transaction_index: Some(2),
            log_index,
            transaction_input: calldata,
            raw: RawLog {
                address: nock_contract_address,
                topics,
                data: Bytes::from(amount_raw.to_be_bytes::<32>().to_vec()),
            },
        }
    }

    fn tracked_for(
        log: &SequencerBaseLog,
        amount: u64,
        lock_root: B256,
    ) -> TrackedWithdrawalRequest {
        TrackedWithdrawalRequest {
            id: WithdrawalId {
                as_of: tip5_from_b256(b256_from_u64(0x7777)),
                base_event_id: compute_base_event_id(&log.transaction_hash, log.log_index),
            },
            recipient: tip5_from_b256(lock_root),
            amount,
            base_batch_end: 109,
            withdrawal_nonce: 7,
        }
    }

    fn verifier_with_logs(
        confirmed_height: u64,
        base_blocks_chunk: u64,
        logs: Vec<SequencerBaseLog>,
    ) -> SequencerBaseRpcWithdrawalVerifier {
        let tracker = Arc::new(SequencerBaseHeightTracker::default());
        tracker.record_confirmed_base_height(confirmed_height);
        SequencerBaseRpcWithdrawalVerifier::with_log_source(
            8_453,
            100,
            tracker,
            base_blocks_chunk,
            Address::ZERO,
            mock_log_source(logs),
        )
    }

    fn scanner_with_source(
        confirmed_height: u64,
        base_blocks_chunk: u64,
        source: Arc<MockLogSource>,
    ) -> SequencerBaseRpcWithdrawalVerifier {
        let tracker = Arc::new(SequencerBaseHeightTracker::default());
        tracker.record_confirmed_base_height(confirmed_height);
        SequencerBaseRpcWithdrawalVerifier::with_log_source(
            8_453,
            100,
            tracker,
            base_blocks_chunk,
            Address::ZERO,
            source,
        )
    }

    async fn open_activity_store() -> (tempfile::TempDir, BaseActivityStore) {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let store = WithdrawalSequencerStore::open(directory.path().join("sequencer.sqlite"))
            .await
            .expect("open withdrawal state store");
        let activity = store.base_activity_store();
        (directory, activity)
    }

    #[tokio::test]
    async fn verifier_accepts_matching_confirmed_burn_for_withdrawal() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        verifier.verify(&tracked).await.expect("verified burn");
    }

    #[tokio::test]
    async fn verifier_leaves_compensation_policy_to_durable_store() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        log.transaction_hash = B256::from_slice(
            &hex::decode("fa0b8e4134a387440a99544114578397d52542cea306d6b9adea801407e3123f")
                .expect("compensated tx hash hex"),
        );
        log.log_index = Some(243);
        let tracked = tracked_for(&log, 42, lock_root);
        assert_eq!(
            sequencer_base_event_id_hex(&tracked.id.base_event_id),
            "0x45cfbf831f2abf377164f857a2bc47338fcaa8f4f12a5986a3ba9bef35afeabd"
        );
        let verifier = verifier_with_logs(109, 10, vec![log]);

        verifier
            .verify(&tracked)
            .await
            .expect("chain verifier should validate facts independently of compensation");
    }

    #[tokio::test]
    async fn verifier_rejects_matching_event_from_wrong_contract_address() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        log.raw.address = address_from_u64(0x9999);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("wrong contract address");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::WrongContractAddress { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_burn_without_full_lock_root_trailer() {
        let lock_root = b256_from_u64(0x1234);
        let mut log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        log.transaction_input =
            Bytes::from(log.transaction_input[..WITHDRAWAL_BURN_BASE_CALLDATA_LEN].to_vec());
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("missing trailer");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::InvalidCalldataTrailer { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_missing_base_event_id() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let mut tracked = tracked_for(&log, 42, lock_root);
        tracked.id.base_event_id = BaseEventId(vec![0xff; 32]);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("missing event");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::MissingBaseEventId { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_wrong_lock_root() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let mut tracked = tracked_for(&log, 42, lock_root);
        tracked.recipient = tip5_from_b256(b256_from_u64(0x9999));
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("wrong lock root");
        assert_eq!(err, SequencerBaseWithdrawalRejection::WrongLockRoot);
    }

    #[tokio::test]
    async fn verifier_rejects_wrong_amount() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(41u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("wrong amount");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::WrongAmount {
                expected_nicks: 42,
                actual_nicks: 41
            }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_amount_not_divisible_by_nock_base_per_nick() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(NOCK_BASE_PER_NICK) + U256::from(1u64),
            lock_root,
        );
        let tracked = tracked_for(&log, 1, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("fractional nick");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::AmountNotDivisible { .. }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_event_above_confirmed_base_height() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(108, 10, vec![log]);

        let err = verifier
            .verify(&tracked)
            .await
            .expect_err("above confirmed");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::EventAboveConfirmed {
                base_batch_end: 109,
                confirmed_base_height: 108
            }
        ));
    }

    #[tokio::test]
    async fn verifier_rejects_event_outside_claimed_batch_window() {
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            99,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);
        let verifier = verifier_with_logs(109, 10, vec![log]);

        let err = verifier.verify(&tracked).await.expect_err("outside batch");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::EventOutsideClaimedBatchWindow {
                event_block: 99,
                batch_start: 100,
                batch_end: 109
            }
        ));
    }

    #[tokio::test]
    async fn verifier_fails_closed_on_log_source_error() {
        let tracker = Arc::new(SequencerBaseHeightTracker::default());
        tracker.record_confirmed_base_height(109);
        let source = mock_log_source(Vec::new());
        *source.error.lock().expect("mock error lock") = Some("rpc unavailable".into());
        let verifier = SequencerBaseRpcWithdrawalVerifier::with_log_source(
            8_453,
            100,
            tracker,
            10,
            Address::ZERO,
            source,
        );
        let lock_root = b256_from_u64(0x1234);
        let log = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let tracked = tracked_for(&log, 42, lock_root);

        let err = verifier.verify(&tracked).await.expect_err("rpc failure");
        assert!(matches!(
            err,
            SequencerBaseWithdrawalRejection::RpcFailure { .. }
        ));
    }

    #[tokio::test]
    async fn activity_scanner_recovers_missing_overlap_burn_and_is_idempotent() {
        let (_directory, store) = open_activity_store().await;
        let lock_root = b256_from_u64(0x1234);
        let first = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            lock_root,
        );
        let first_id = compute_base_event_id(&first.transaction_hash, first.log_index);
        let source = mock_scanner_source(vec![first.clone()]);
        let scanner = scanner_with_source(109, 4, source.clone());

        let first_report = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect("initial Base activity scan");
        assert_eq!(first_report.scan_start, 100);
        assert_eq!(first_report.scan_end, 109);
        assert_eq!(first_report.chunks_verified, 3);
        assert_eq!(first_report.blocks_verified, 10);
        assert_eq!(first_report.burns_inserted, 1);
        assert_eq!(
            source
                .requests
                .lock()
                .expect("mock requests lock")
                .as_slice(),
            &[(100, 103), (104, 107), (108, 109)]
        );
        let stored_first = store
            .load_verified_burn(8_453, Address::ZERO, &first_id)
            .await
            .expect("load first burn")
            .expect("first burn exists");
        assert_eq!(stored_first.block_number, 105);
        assert_eq!(stored_first.base_batch_end, 107);
        assert_eq!(stored_first.amount_nicks, 42);
        assert_eq!(stored_first.observed_at_unix_secs, Some(1_700_000_105));
        assert_eq!(
            stored_first.policy_id.as_deref(),
            Some(WITHDRAWAL_POLICY_V1_ID)
        );
        assert_eq!(
            stored_first.protocol_id.as_deref(),
            Some(WITHDRAWAL_WIRE_V1_ID)
        );

        let second = burn_log(
            108,
            U256::from(43u64) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x5678),
        );
        let second_id = compute_base_event_id(&second.transaction_hash, second.log_index);
        source.logs.lock().expect("mock logs lock").push(second);
        let overlap_report = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect("overlap Base activity scan");
        assert_eq!(overlap_report.burns_inserted, 1);
        assert!(store
            .load_verified_burn(8_453, Address::ZERO, &second_id)
            .await
            .expect("load recovered burn")
            .is_some());

        let duplicate_report = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect("duplicate overlap scan");
        assert_eq!(duplicate_report.burns_inserted, 0);
        assert_eq!(
            store
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load activity cursor")
                .expect("activity cursor")
                .last_verified_block,
            109
        );
    }

    #[tokio::test]
    async fn activity_scanner_rejects_reorg_inside_overlap_without_advancing_cursor() {
        let (_directory, store) = open_activity_store().await;
        let source = mock_scanner_source(Vec::new());
        let scanner = scanner_with_source(109, 4, source.clone());
        scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect("initial Base activity scan");
        let cursor = store
            .load_cursor(8_453, Address::ZERO)
            .await
            .expect("load initial cursor")
            .expect("initial cursor");
        *source
            .header_hash_offset
            .lock()
            .expect("mock header offset lock") = 1;
        let mut fork_burn = burn_log(
            105,
            U256::from(44u64) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x9999),
        );
        fork_burn.block_hash = mock_block_hash(105, 1);
        fork_burn.parent_hash = mock_block_hash(104, 1);
        let fork_burn_id = compute_base_event_id(&fork_burn.transaction_hash, fork_burn.log_index);
        source.logs.lock().expect("mock logs lock").push(fork_burn);

        let error = scanner
            .scan_confirmed_burn_tail(&store, 5)
            .await
            .expect_err("overlap reorg must fail closed");
        assert!(error.to_string().contains("reorg detected at cursor block"));
        assert_eq!(
            store
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load retained cursor"),
            Some(cursor)
        );
        assert_eq!(
            store
                .load_verified_burn(8_453, Address::ZERO, &fork_burn_id)
                .await
                .expect("load uncommitted fork burn"),
            None
        );
    }

    #[tokio::test]
    async fn activity_scanner_quarantines_malformed_burn_and_commits_later_valid_burn() {
        let (_directory, store) = open_activity_store().await;
        let mut malformed = burn_log(
            105,
            U256::from(42u64) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x1234),
        );
        malformed.transaction_input =
            Bytes::from(malformed.transaction_input[..WITHDRAWAL_BURN_BASE_CALLDATA_LEN].to_vec());
        let malformed_id = compute_base_event_id(&malformed.transaction_hash, malformed.log_index);
        let valid = burn_log(
            106,
            U256::from(43u64) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x5678),
        );
        let valid_id = compute_base_event_id(&valid.transaction_hash, valid.log_index);
        let source = mock_scanner_source(vec![malformed, valid]);
        let scanner = scanner_with_source(109, 10, source);

        let report = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect("malformed burn should be quarantined without blocking the chunk");
        assert_eq!(report.burns_rejected, 1);
        assert_eq!(report.burns_inserted, 1);
        assert_eq!(
            store
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load cursor after quarantined burn")
                .expect("cursor should advance")
                .last_verified_block,
            109
        );
        let rejected = store
            .incident_store()
            .load_rejected_burn(8_453, Address::ZERO, &malformed_id)
            .await
            .expect("load quarantined burn")
            .expect("quarantined burn should be durable");
        assert_eq!(rejected.rejection_code, "missing_calldata_trailer");
        assert!(rejected
            .rejection_detail
            .contains("missing withdrawal trailer"));
        assert!(store
            .load_verified_burn(8_453, Address::ZERO, &valid_id)
            .await
            .expect("load later valid burn")
            .is_some());
    }

    #[tokio::test]
    async fn activity_scanner_rejects_header_gap_and_provider_failure() {
        let (_directory, store) = open_activity_store().await;
        let source = mock_scanner_source(Vec::new());
        *source
            .omitted_header
            .lock()
            .expect("mock omitted header lock") = Some(102);
        let scanner = scanner_with_source(109, 4, source.clone());

        let gap_error = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect_err("header gap must fail scan");
        assert!(gap_error
            .to_string()
            .contains("returned 3 headers for 4 blocks"));
        assert_eq!(
            store
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load cursor after header gap"),
            None
        );

        *source
            .omitted_header
            .lock()
            .expect("mock omitted header lock") = None;
        *source.error.lock().expect("mock error lock") = Some("rate limited".into());
        let rpc_error = scanner
            .scan_confirmed_burn_tail(&store, 10)
            .await
            .expect_err("provider failure must fail scan");
        assert!(rpc_error.to_string().contains("rate limited"));
        assert_eq!(
            store
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load cursor after provider failure"),
            None
        );
    }

    #[tokio::test]
    async fn activity_scanner_recovers_eligible_burn_as_pending_once() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let sequencer = WithdrawalSequencerStore::open(directory.path().join("sequencer.sqlite"))
            .await
            .expect("open withdrawal state store");
        let activity = sequencer.base_activity_store();
        let policy = crate::shared::types::WithdrawalPolicy::v1();
        let amount_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .expect("minimum withdrawal nicks");
        let log = burn_log(
            105,
            U256::from(amount_nicks) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x2468),
        );
        let base_event_id = compute_base_event_id(&log.transaction_hash, log.log_index);
        let source = mock_scanner_source(vec![log]);
        let scanner = scanner_with_source(109, 10, source);

        let first = scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("scan and recover pending withdrawal");
        assert_eq!(first.scan.burns_inserted, 1);
        assert_eq!(first.recovery.recovered_pending, 1);
        let id = WithdrawalId {
            as_of: crate::shared::types::zero_tip5_hash(),
            base_event_id,
        };
        let row = sequencer
            .fetch_sequenced_withdrawal(&id)
            .await
            .expect("fetch recovered pending withdrawal")
            .expect("recovered pending withdrawal");
        assert_eq!(
            row.state,
            crate::withdrawal::state::WithdrawalState::Pending
        );
        assert_eq!(row.withdrawal_nonce, Some(1));
        assert_eq!(row.gross_burned_amount, Some(amount_nicks));
        assert_eq!(row.base_batch_end, Some(109));

        let second = scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("idempotent scan and recovery");
        assert_eq!(second.scan.burns_inserted, 0);
        assert_eq!(second.recovery, BaseBurnRecoveryReport::default());
    }
    #[tokio::test]
    async fn shallow_base_reorg_invalidates_orphan_and_readmits_only_canonical_burn() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let path = directory.path().join("sequencer.sqlite");
        let sequencer = WithdrawalSequencerStore::open(path.clone())
            .await
            .expect("open withdrawal state store");
        let activity = sequencer.base_activity_store();
        let policy = crate::shared::types::WithdrawalPolicy::v1();
        let amount_nicks = policy
            .minimum_gross_nocks
            .checked_mul(policy.nicks_per_nock)
            .expect("minimum withdrawal nicks");
        let original = burn_log(
            105,
            U256::from(amount_nicks) * U256::from(NOCK_BASE_PER_NICK),
            b256_from_u64(0x2468),
        );
        let base_event_id = compute_base_event_id(&original.transaction_hash, original.log_index);
        let source = mock_scanner_source(vec![original.clone()]);
        let scanner = scanner_with_source(109, 10, source.clone());
        scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("initial burn recovery");

        *source.fork_from.lock().expect("mock fork start lock") = Some(105);
        *source
            .header_hash_offset
            .lock()
            .expect("mock header offset lock") = 1_000;
        source.logs.lock().expect("mock logs lock").clear();
        let recovered = scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("shallow Base rewind");
        assert_eq!(recovered.recovery, BaseBurnRecoveryReport::default());
        let id = WithdrawalId {
            as_of: crate::shared::types::zero_tip5_hash(),
            base_event_id: base_event_id.clone(),
        };
        let row = sequencer
            .fetch_sequenced_withdrawal(&id)
            .await
            .expect("fetch invalidated withdrawal")
            .expect("invalidated withdrawal row");
        assert_eq!(
            row.state,
            crate::withdrawal::state::WithdrawalState::Invalidated
        );
        assert_eq!(
            sequencer
                .current_live_withdrawal_nonce()
                .await
                .expect("current live nonce"),
            None
        );
        assert!(activity
            .load_verified_burn(8_453, Address::ZERO, &base_event_id)
            .await
            .expect("load orphaned burn")
            .is_none());

        let restarted = WithdrawalSequencerStore::open(path)
            .await
            .expect("restart withdrawal state store");
        let restarted_activity = restarted.base_activity_store();
        assert_eq!(
            restarted
                .fetch_sequenced_withdrawal(&id)
                .await
                .expect("fetch invalidated withdrawal after restart")
                .expect("invalidated row after restart")
                .state,
            crate::withdrawal::state::WithdrawalState::Invalidated
        );

        let mut canonical = original;
        canonical.block_hash = mock_block_hash(105, 1_000);
        canonical.parent_hash = mock_block_hash(104, 0);
        source.logs.lock().expect("mock logs lock").push(canonical);
        let readmitted = scanner
            .scan_and_recover_confirmed_burns(&restarted_activity, &restarted, 10, 100)
            .await
            .expect("canonical burn re-admission");
        assert_eq!(readmitted.recovery.recovered_pending, 1);
        let row = restarted
            .fetch_sequenced_withdrawal(&id)
            .await
            .expect("fetch readmitted withdrawal")
            .expect("readmitted withdrawal row");
        assert_eq!(
            row.state,
            crate::withdrawal::state::WithdrawalState::Pending
        );
        assert_eq!(row.current_epoch, 1);
        assert_eq!(row.withdrawal_nonce, Some(1));
    }

    #[tokio::test]
    async fn deep_base_reorg_reports_exact_retained_checkpoint_evidence() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let sequencer = WithdrawalSequencerStore::open(directory.path().join("sequencer.sqlite"))
            .await
            .expect("open withdrawal state store");
        let activity = sequencer.base_activity_store();
        let source = mock_scanner_source(Vec::new());
        let scanner = scanner_with_source(109, 10, source.clone());
        scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("initial checkpoint scan");
        let cursor = activity
            .load_cursor(8_453, Address::ZERO)
            .await
            .expect("load initial cursor")
            .expect("initial cursor");
        *source
            .header_hash_offset
            .lock()
            .expect("mock header offset lock") = 10_000;
        *source.fork_from.lock().expect("mock fork start lock") = Some(100);

        let error = scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect_err("deep fork must fail with checkpoint evidence");
        let message = error.to_string();
        assert!(message.contains("no common ancestor"));
        assert!(message.contains("100..=109"));
        assert!(message.contains("max_depth=64"));
        let guard = sequencer
            .ensure_reorg_ready()
            .await
            .expect_err("deep reorg must activate guard");
        assert!(guard.to_string().contains("100..=109"));
        assert_eq!(
            activity
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load retained cursor"),
            Some(cursor)
        );
    }
    #[tokio::test]
    async fn base_reorg_crossing_activation_boundary_fails_without_rewind() {
        let directory = tempfile::TempDir::new().expect("temporary directory");
        let sequencer = WithdrawalSequencerStore::open(directory.path().join("sequencer.sqlite"))
            .await
            .expect("open withdrawal state store");
        let activity = sequencer.base_activity_store();
        let source = mock_scanner_source(Vec::new());
        let scanner = scanner_with_source(109, 10, source.clone());
        scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 100)
            .await
            .expect("initial checkpoint scan");
        *source.fork_from.lock().expect("mock fork start lock") = Some(105);
        *source
            .header_hash_offset
            .lock()
            .expect("mock header offset lock") = 1_000;

        let error = scanner
            .scan_and_recover_confirmed_burns(&activity, &sequencer, 10, 107)
            .await
            .expect_err("activation-crossing fork must fail");
        assert!(error
            .to_string()
            .contains("crosses withdrawal activation boundary"));
        assert!(sequencer.ensure_reorg_ready().await.is_err());
        assert_eq!(
            activity
                .load_cursor(8_453, Address::ZERO)
                .await
                .expect("load retained cursor")
                .expect("retained cursor")
                .last_verified_block,
            109
        );
    }
}
