use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;

use libp2p::{request_response, PeerId};
use nockapp::utils::error::CrownError;
use nockapp::NockAppError;
use serde_bytes::ByteBuf;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tracing::{info, trace, warn};

use crate::driver::gen2::*;
use crate::messages::{
    BatchErrorClass, BatchRequestItem, BatchResultItem, BatchResultStatus, NockchainDataRequest,
    NockchainRequest, NockchainResponse, ResponseEnvelope,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{P2PState, ReqResGeneration};
use crate::tip5_util::TIP5_BASE58_MAX_CHARS;
use crate::traffic_cop;

#[derive(Debug)]
pub(crate) enum RequestExecutionOutcome {
    Result {
        response: NockchainResponse,
        envelope: ResponseEnvelope,
    },
    NotFound,
}
impl RequestExecutionOutcome {
    pub(crate) fn into_single_response(self) -> NockchainResponse {
        match self {
            Self::Result { response, .. } => response,
            Self::NotFound => NockchainResponse::Ack { acked: true },
        }
    }

    pub(crate) fn into_batch_result_item(self, item_id: u32) -> BatchResultItem {
        match self {
            Self::Result { envelope, .. } => BatchResultItem {
                item_id,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(envelope),
            },
            Self::NotFound => BatchResultItem {
                item_id,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) enum BatchItemExecutionOutcome {
    Completed(RequestExecutionOutcome),
    Failed(BatchErrorClass),
    Backpressure,
}

/// Minimal envelope metadata needed to project encoded `BatchResultItem` size.
#[derive(Clone, Debug)]
pub(crate) enum BatchItemResponseEnvelopeEstimate {
    HeardBlock { block_id_bytes_upper_bound: usize },
    HeardBlockWithTxs { block_id_bytes_upper_bound: usize },
    HeardBlockRangeWithTxs { block_count: usize },
    HeardTx { tx_id: String },
    HeardElders,
}
impl BatchItemResponseEnvelopeEstimate {
    pub(crate) fn from_request(request: &NockchainDataRequest) -> Self {
        match request {
            NockchainDataRequest::BlockByHeight(_) => Self::HeardBlock {
                block_id_bytes_upper_bound: TIP5_BASE58_MAX_CHARS,
            },
            NockchainDataRequest::BlockWithTxsByHeight(_) => Self::HeardBlockWithTxs {
                block_id_bytes_upper_bound: TIP5_BASE58_MAX_CHARS,
            },
            NockchainDataRequest::EldersById(_, _, _) => Self::HeardElders,
            NockchainDataRequest::RawTransactionById(tx_id, _) => Self::HeardTx {
                tx_id: tx_id.clone(),
            },
            NockchainDataRequest::BlockRangeWithTxs { len, .. } => Self::HeardBlockRangeWithTxs {
                block_count: usize::from(*len),
            },
        }
    }

    /// Build a synthetic envelope for local CBOR sizing only.
    pub(crate) fn into_sizing_envelope(self, message: Vec<u8>) -> ResponseEnvelope {
        match self {
            Self::HeardBlock {
                block_id_bytes_upper_bound,
            } => ResponseEnvelope::heard_block("1".repeat(block_id_bytes_upper_bound), message),
            Self::HeardBlockWithTxs {
                block_id_bytes_upper_bound,
            } => ResponseEnvelope::heard_block_with_txs(
                "1".repeat(block_id_bytes_upper_bound),
                message,
                Vec::new(),
                Vec::new(),
            ),
            Self::HeardBlockRangeWithTxs { block_count } => {
                let block_message_each = if block_count == 0 {
                    Vec::new()
                } else {
                    let chunk = message.len() / block_count.max(1);
                    vec![0u8; chunk.max(1)]
                };
                let blocks = (0..block_count)
                    .map(|i| crate::messages::BundledBlockWithTxs {
                        block_id: format!("{:0>width$}", i, width = TIP5_BASE58_MAX_CHARS),
                        block_message: serde_bytes::ByteBuf::from(block_message_each.clone()),
                        tx_envelopes: Vec::new(),
                        unincluded_tx_ids: Vec::new(),
                    })
                    .collect();
                ResponseEnvelope::heard_block_range_with_txs(blocks)
            }
            Self::HeardTx { tx_id } => ResponseEnvelope::heard_tx(tx_id, message),
            Self::HeardElders => ResponseEnvelope::heard_elders(message),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BatchItemResponseEstimate {
    pub(crate) request_kind: &'static str,
    pub(crate) envelope: BatchItemResponseEnvelopeEstimate,
    pub(crate) message_bytes: usize,
    pub(crate) source: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReqResRuntimeLimits {
    pub(crate) request_high_threshold: u64,
    pub(crate) request_replay_cache_ttl: Duration,
    pub(crate) request_replay_cache_max_per_peer: usize,
    pub(crate) ip_bucket_request_admission_limit: u64,
    pub(crate) ip_bucket_connection_limit: usize,
    pub(crate) gossip_bucket_capacity: u32,
    pub(crate) gossip_bucket_refill_per_second: u32,
    pub(crate) authenticated_gossip_send_enabled: bool,
    pub(crate) legacy_gossip_accept_enabled: bool,
    pub(crate) block_range_max_len: u8,
    pub(crate) gen2_batch_max_items: usize,
    pub(crate) gen2_batch_max_bytes: usize,
    pub(crate) gen2_item_max_bytes: usize,
    pub(crate) gen2_block_batch_max_response_bytes: usize,
    pub(crate) gen2_max_inflight_per_peer: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BatchTopLevelLimitViolation {
    TooManyItems {
        item_count: usize,
        max_items: usize,
    },
    TooManyBytes {
        payload_bytes: usize,
        max_bytes: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BatchRejectReason {
    Malformed,
    TooManyItems,
    TooManyBytes,
    Backpressure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueSaturationPath {
    InflightAdmission,
    RequestExecution,
    ResponseRoute,
    GossipRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingBatchInsertOutcome {
    Duplicate,
    Inserted {
        item_count: usize,
        payload_bytes: usize,
        estimated_response_bytes: usize,
        contains_response_budget_item: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingBatchFlushReason {
    PreInsertPayloadBytes,
    PreInsertResponseBudget,
    MaxItems,
    MaxPayloadBytes,
    ResponseBudget,
    CoalesceTick,
}

impl PendingBatchFlushReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PreInsertPayloadBytes => "pre_insert_payload_bytes",
            Self::PreInsertResponseBudget => "pre_insert_response_budget",
            Self::MaxItems => "max_items",
            Self::MaxPayloadBytes => "max_payload_bytes",
            Self::ResponseBudget => "response_budget",
            Self::CoalesceTick => "coalesce_tick",
        }
    }
}

#[derive(Debug)]
pub(crate) struct PendingGen2Batch {
    pub(crate) next_item_id: u32,
    pub(crate) payload_bytes: usize,
    pub(crate) estimated_response_bytes: usize,
    pub(crate) contains_response_budget_item: bool,
    pub(crate) contains_raw_tx_by_id: bool,
    pub(crate) contains_non_raw_tx: bool,
    pub(crate) flush_ticks_remaining: u8,
    pub(crate) items: Vec<BatchRequestItem>,
    pub(crate) seen_messages: BTreeSet<Vec<u8>>,
}
impl Default for PendingGen2Batch {
    fn default() -> Self {
        Self {
            next_item_id: 0,
            payload_bytes: std::mem::size_of::<u32>(),
            estimated_response_bytes: 0,
            contains_response_budget_item: false,
            contains_raw_tx_by_id: false,
            contains_non_raw_tx: false,
            flush_ticks_remaining: 0,
            items: Vec::new(),
            seen_messages: BTreeSet::new(),
        }
    }
}

impl PendingGen2Batch {
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn would_exceed_batch_bytes(
        &self,
        message_len: usize,
        max_batch_bytes: usize,
    ) -> Result<bool, NockAppError> {
        let next_payload_bytes = self
            .payload_bytes
            .checked_add(std::mem::size_of::<u32>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<u32>()))
            .and_then(|bytes| bytes.checked_add(message_len))
            .ok_or_else(|| NockAppError::OtherError(String::from("batch payload size overflow")))?;
        Ok(next_payload_bytes > max_batch_bytes)
    }

    pub(crate) fn would_exceed_response_budget(
        &self,
        next_estimated_response_bytes: usize,
        next_contains_response_budget_item: bool,
        limits: ReqResRuntimeLimits,
    ) -> bool {
        if !self.contains_response_budget_item && !next_contains_response_budget_item {
            return false;
        }

        self.estimated_response_bytes
            .checked_add(next_estimated_response_bytes)
            .is_none_or(|estimated_response_bytes| {
                estimated_response_bytes > block_batch_response_budget_bytes(limits)
            })
    }

    pub(crate) fn insert_request_message(
        &mut self,
        message: &[u8],
        estimated_response_bytes: usize,
        contains_response_budget_item: bool,
        contains_raw_tx_by_id: bool,
    ) -> Result<PendingBatchInsertOutcome, NockAppError> {
        let message_bytes = message.to_vec();
        if !self.seen_messages.insert(message_bytes.clone()) {
            return Ok(PendingBatchInsertOutcome::Duplicate);
        }

        self.payload_bytes = self
            .payload_bytes
            .checked_add(std::mem::size_of::<u32>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<u32>()))
            .and_then(|bytes| bytes.checked_add(message_bytes.len()))
            .ok_or_else(|| NockAppError::OtherError(String::from("batch payload size overflow")))?;
        self.estimated_response_bytes = self
            .estimated_response_bytes
            .checked_add(estimated_response_bytes)
            .ok_or_else(|| {
                NockAppError::OtherError(String::from("batch estimated response size overflow"))
            })?;
        self.contains_response_budget_item |= contains_response_budget_item;
        self.contains_raw_tx_by_id |= contains_raw_tx_by_id;
        self.contains_non_raw_tx |= !contains_raw_tx_by_id;
        self.flush_ticks_remaining = if self.contains_raw_tx_by_id && !self.contains_non_raw_tx {
            2
        } else {
            1
        };

        self.items.push(BatchRequestItem {
            item_id: self.next_item_id,
            message: ByteBuf::from(message_bytes),
        });
        self.next_item_id = self.next_item_id.saturating_add(1);

        Ok(PendingBatchInsertOutcome::Inserted {
            item_count: self.items.len(),
            payload_bytes: self.payload_bytes,
            estimated_response_bytes: self.estimated_response_bytes,
            contains_response_budget_item: self.contains_response_budget_item,
        })
    }

    pub(crate) fn take_items(&mut self) -> Vec<BatchRequestItem> {
        self.next_item_id = 0;
        self.payload_bytes = std::mem::size_of::<u32>();
        self.estimated_response_bytes = 0;
        self.contains_response_budget_item = false;
        self.contains_raw_tx_by_id = false;
        self.contains_non_raw_tx = false;
        self.flush_ticks_remaining = 0;
        self.seen_messages.clear();
        std::mem::take(&mut self.items)
    }

    pub(crate) fn should_flush_on_tick(&mut self) -> bool {
        if self.is_empty() {
            return false;
        }
        if self.flush_ticks_remaining > 1 {
            self.flush_ticks_remaining -= 1;
            return false;
        }
        self.flush_ticks_remaining = 0;
        true
    }
}

pub(crate) fn pending_batch_pre_insert_flush_reason(
    pending_batch: &PendingGen2Batch,
    message_len: usize,
    next_estimated_response_bytes: usize,
    next_contains_response_budget_item: bool,
    limits: ReqResRuntimeLimits,
) -> Result<Option<PendingBatchFlushReason>, NockAppError> {
    if pending_batch.is_empty() {
        return Ok(None);
    }
    if pending_batch.would_exceed_batch_bytes(message_len, limits.gen2_batch_max_bytes)? {
        return Ok(Some(PendingBatchFlushReason::PreInsertPayloadBytes));
    }
    if pending_batch.would_exceed_response_budget(
        next_estimated_response_bytes, next_contains_response_budget_item, limits,
    ) {
        return Ok(Some(PendingBatchFlushReason::PreInsertResponseBudget));
    }
    Ok(None)
}

pub(crate) fn inserted_batch_flush_reason(
    item_count: usize,
    payload_bytes: usize,
    estimated_response_bytes: usize,
    contains_response_budget_item: bool,
    limits: ReqResRuntimeLimits,
) -> Option<PendingBatchFlushReason> {
    if item_count >= limits.gen2_batch_max_items {
        return Some(PendingBatchFlushReason::MaxItems);
    }
    if payload_bytes >= limits.gen2_batch_max_bytes {
        return Some(PendingBatchFlushReason::MaxPayloadBytes);
    }
    if contains_response_budget_item
        && estimated_response_bytes >= block_batch_response_budget_bytes(limits)
    {
        return Some(PendingBatchFlushReason::ResponseBudget);
    }
    None
}

pub(crate) fn log_pending_gen2_batch_flush(
    peer_id: &PeerId,
    reason: PendingBatchFlushReason,
    pending_batch: &PendingGen2Batch,
    limits: ReqResRuntimeLimits,
) {
    let block_response_budget_bytes = block_batch_response_budget_bytes(limits);
    let response_budget_utilization = if block_response_budget_bytes == 0 {
        0.0
    } else {
        pending_batch.estimated_response_bytes as f64 / block_response_budget_bytes as f64
    };
    let payload_utilization = if limits.gen2_batch_max_bytes == 0 {
        0.0
    } else {
        pending_batch.payload_bytes as f64 / limits.gen2_batch_max_bytes as f64
    };
    info!(
        peer = %peer_id,
        flush_reason = reason.as_str(),
        request_keys = %batch_request_keys_csv(&pending_batch.items),
        request_block_heights = %batch_request_block_heights_csv(&pending_batch.items),
        batch_items = pending_batch.items.len(),
        batch_payload_bytes = pending_batch.payload_bytes,
        batch_payload_cap_bytes = limits.gen2_batch_max_bytes,
        batch_payload_utilization = payload_utilization,
        estimated_response_bytes = pending_batch.estimated_response_bytes,
        block_response_budget_bytes,
        block_response_budget_utilization = response_budget_utilization,
        contains_response_budget_item = pending_batch.contains_response_budget_item,
        contains_raw_tx_by_id = pending_batch.contains_raw_tx_by_id,
        contains_non_raw_tx = pending_batch.contains_non_raw_tx,
        pending_flush_ticks = pending_batch.flush_ticks_remaining,
        "Flushing pending gen2 batch"
    );
}
pub(crate) const PENDING_GEN2_BATCH_DUPLICATE_REASON: &str = "exact_message_already_pending";
pub(crate) const ACTIVE_OUTBOUND_REQUEST_DUPLICATE_REASON: &str = "exact_message_already_inflight";
pub(crate) const SPECULATIVE_TX_PREFETCH_TTL: Duration = Duration::from_secs(15);
pub(crate) const SPECULATIVE_TX_PREFETCH_MAX_IDS_PER_BLOCK: usize = 256;
pub(crate) const GEN2_RETRY_MAX_ATTEMPTS: u8 = 3;
pub(crate) const GEN2_RETRY_BASE_DELAY_MS: u64 = 100;
pub(crate) const GEN2_RETRY_MAX_DELAY_MS: u64 = 2_000;
pub(crate) const GEN2_RETRY_MAX_JITTER_MS: u64 = 50;
pub(crate) fn outbound_request_generation(
    request: &NockchainRequest,
    req_res_gen2_send_enabled: bool,
    peer_supports_gen2: bool,
) -> ReqResGeneration {
    match request {
        NockchainRequest::BatchRequest { .. } => ReqResGeneration::Gen2,
        NockchainRequest::Request { .. }
        | NockchainRequest::Gossip { .. }
        | NockchainRequest::AuthenticatedGossip { .. } => {
            if req_res_gen2_send_enabled && peer_supports_gen2 {
                ReqResGeneration::Gen2
            } else {
                ReqResGeneration::Gen1
            }
        }
    }
}

pub(crate) fn should_authenticate_outbound_gossip(
    request: &NockchainRequest,
    req_res_limits: ReqResRuntimeLimits,
    req_res_gen2_send_enabled: bool,
    peer_supports_gen2: bool,
    generation: ReqResGeneration,
) -> bool {
    req_res_limits.authenticated_gossip_send_enabled
        && req_res_gen2_send_enabled
        && peer_supports_gen2
        && generation == ReqResGeneration::Gen2
        && matches!(request, NockchainRequest::Gossip { .. })
}

pub(crate) fn outbound_request_shape(request: &NockchainRequest) -> &'static str {
    match request {
        NockchainRequest::Request { .. } => "request",
        NockchainRequest::Gossip { .. } => "gossip",
        NockchainRequest::AuthenticatedGossip { .. } => "authenticated-gossip",
        NockchainRequest::BatchRequest { .. } => "batch-request",
    }
}
pub(crate) fn batch_request_item_count(request: &NockchainRequest) -> Option<usize> {
    match request {
        NockchainRequest::BatchRequest { items, .. } => Some(items.len()),
        NockchainRequest::Request { .. }
        | NockchainRequest::Gossip { .. }
        | NockchainRequest::AuthenticatedGossip { .. } => None,
    }
}

pub(crate) fn increment_outbound_generation_failure_metrics(
    metrics: &NockchainP2PMetrics,
    generation: ReqResGeneration,
    error: &request_response::OutboundFailure,
) {
    match generation {
        ReqResGeneration::Gen1 => {
            metrics.gen1_outbound_failures.increment();
            if matches!(error, request_response::OutboundFailure::Timeout) {
                metrics.gen1_outbound_timeouts.increment();
            }
        }
        ReqResGeneration::Gen2 => {
            metrics.gen2_outbound_failures.increment();
            if matches!(error, request_response::OutboundFailure::Timeout) {
                metrics.gen2_outbound_timeouts.increment();
            }
        }
    }
}
pub(crate) fn increment_batch_item_error_metric(
    metrics: &NockchainP2PMetrics,
    error: Option<BatchErrorClass>,
) {
    match error {
        Some(BatchErrorClass::Decode) => {
            metrics.gen2_batch_item_error_decode.increment();
        }
        Some(BatchErrorClass::Backpressure) => {
            metrics.gen2_batch_item_error_backpressure.increment();
        }
        Some(BatchErrorClass::TooLarge) => {
            metrics.gen2_batch_item_error_too_large.increment();
        }
        Some(BatchErrorClass::InvalidPow) => {
            metrics.gen2_batch_item_error_invalid_pow.increment();
        }
        Some(BatchErrorClass::Internal) => {
            metrics.gen2_batch_item_error_internal.increment();
        }
        None => {}
    }
}

pub(crate) fn record_batch_result_item_errors(
    metrics: &NockchainP2PMetrics,
    results: &[BatchResultItem],
) {
    for result in results {
        if result.status == BatchResultStatus::Error {
            increment_batch_item_error_metric(metrics, result.error);
        }
    }
}
pub(crate) fn count_batch_result_item_failures(results: &[BatchResultItem]) -> u64 {
    results
        .iter()
        .filter(|result| result.status == BatchResultStatus::Error)
        .count() as u64
}

pub(crate) fn record_req_res_fallback(metrics: &NockchainP2PMetrics, fallback_count: usize) {
    if fallback_count > 0 {
        metrics.req_res_fallback_total.fetch_add(fallback_count);
    }
}
/// Record when a `BlockByHeight` request still ends up on gen1 even though
/// gen2 send is enabled and the peer supports gen2.
pub(crate) fn record_block_by_height_gen1_routed(
    metrics: &NockchainP2PMetrics,
    generation: ReqResGeneration,
    req_res_gen2_send_enabled: bool,
    peer_supports_gen2: bool,
    request: &NockchainRequest,
) {
    if generation == ReqResGeneration::Gen1
        && req_res_gen2_send_enabled
        && peer_supports_gen2
        && request_is_block_by_height(request)
    {
        metrics.req_res_block_by_height_gen1_routed.increment();
    }
}

pub(crate) const fn queue_saturation_decision(path: QueueSaturationPath) -> &'static str {
    match path {
        QueueSaturationPath::InflightAdmission => "reject",
        QueueSaturationPath::RequestExecution
        | QueueSaturationPath::ResponseRoute
        | QueueSaturationPath::GossipRoute => "defer",
    }
}

pub(crate) const fn queue_saturation_path(path: QueueSaturationPath) -> &'static str {
    match path {
        QueueSaturationPath::InflightAdmission => "inflight_admission",
        QueueSaturationPath::RequestExecution => "request_execution",
        QueueSaturationPath::ResponseRoute => "response_route",
        QueueSaturationPath::GossipRoute => "gossip_route",
    }
}
pub(crate) fn record_batch_rejection(metrics: &NockchainP2PMetrics, reason: BatchRejectReason) {
    match reason {
        BatchRejectReason::Malformed => {
            metrics.gen2_batch_rejected_malformed.increment();
        }
        BatchRejectReason::TooManyItems => {
            metrics.gen2_batch_rejected_too_many_items.increment();
        }
        BatchRejectReason::TooManyBytes => {
            metrics.gen2_batch_rejected_too_many_bytes.increment();
        }
        BatchRejectReason::Backpressure => {
            metrics.gen2_batch_rejected_backpressure.increment();
        }
    }
}

pub(crate) fn update_pending_batch_metrics(
    metrics: &NockchainP2PMetrics,
    pending_gen2_batches: &BTreeMap<PeerId, PendingGen2Batch>,
) {
    let pending_peers = pending_gen2_batches
        .values()
        .filter(|pending_batch| !pending_batch.is_empty())
        .count();
    let pending_items = pending_gen2_batches
        .values()
        .map(|pending_batch| pending_batch.items.len())
        .sum::<usize>();

    let _ = metrics.gen2_batch_pending_peers.swap(pending_peers as f64);
    let _ = metrics.gen2_batch_pending_items.swap(pending_items as f64);
}
pub(crate) fn queue_pending_gen2_batch_request(
    metrics: &NockchainP2PMetrics,
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    peer_id: PeerId,
    request_message: &[u8],
    estimated_response_bytes: usize,
    contains_response_budget_item: bool,
) -> Result<PendingBatchInsertOutcome, NockAppError> {
    let contains_raw_tx_by_id = request_message_is_raw_tx_by_id(request_message);
    let insert_outcome = {
        let pending_batch = pending_gen2_batches.entry(peer_id).or_default();
        pending_batch.insert_request_message(
            request_message, estimated_response_bytes, contains_response_budget_item,
            contains_raw_tx_by_id,
        )?
    };
    update_pending_batch_metrics(metrics, pending_gen2_batches);
    if matches!(insert_outcome, PendingBatchInsertOutcome::Duplicate) {
        metrics.req_res_effect_dedup_suppressed.increment();
    }
    if let PendingBatchInsertOutcome::Inserted {
        item_count,
        payload_bytes,
        estimated_response_bytes,
        contains_response_budget_item,
    } = insert_outcome
    {
        let request_key = pending_gen2_batch_request_key(request_message);
        let request_block_height = request_message_block_height(request_message)
            .map(|height| height.to_string())
            .unwrap_or_default();
        info!(
            peer = %peer_id,
            request_key = %request_key,
            request_block_height = %request_block_height,
            pending_items = item_count,
            pending_payload_bytes = payload_bytes,
            pending_estimated_response_bytes = estimated_response_bytes,
            contains_response_budget_item,
            contains_raw_tx_by_id,
            "Queued pending gen2 batch item"
        );
    }
    Ok(insert_outcome)
}

pub(crate) fn pending_gen2_batch_request_key(message: &[u8]) -> String {
    match decode_request_item_message(message) {
        Ok(NockchainDataRequest::BlockByHeight(height)) => format!("block-by-height:{height}"),
        Ok(NockchainDataRequest::BlockWithTxsByHeight(height)) => {
            format!("block-with-txs-by-height:{height}")
        }
        Ok(NockchainDataRequest::BlockRangeWithTxs { start_height, len }) => {
            format!("block-range-with-txs:{start_height}+{len}")
        }
        Ok(NockchainDataRequest::EldersById(block_id, request_peer_id, _)) => {
            format!(
                "elders-by-id:{block_id}:request-peer:{}",
                request_peer_id.to_base58()
            )
        }
        Ok(NockchainDataRequest::RawTransactionById(tx_id, _)) => {
            format!("raw-tx-by-id:{tx_id}")
        }
        Err(_) => {
            let mut key = format!("undecodable:{}:", message.len());
            for byte in message.iter().take(8) {
                use std::fmt::Write as _;

                let _ = write!(&mut key, "{byte:02x}");
            }
            key
        }
    }
}

pub(crate) fn request_message_block_height(message: &[u8]) -> Option<u64> {
    match decode_request_item_message(message) {
        Ok(NockchainDataRequest::BlockByHeight(height))
        | Ok(NockchainDataRequest::BlockWithTxsByHeight(height)) => Some(height),
        Ok(NockchainDataRequest::BlockRangeWithTxs { start_height, .. }) => Some(start_height),
        Ok(NockchainDataRequest::EldersById(_, _, _))
        | Ok(NockchainDataRequest::RawTransactionById(_, _))
        | Err(_) => None,
    }
}

pub(crate) fn batch_request_keys_csv(items: &[BatchRequestItem]) -> String {
    items
        .iter()
        .map(|item| pending_gen2_batch_request_key(&item.message))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn batch_request_block_heights_csv(items: &[BatchRequestItem]) -> String {
    items
        .iter()
        .filter_map(|item| request_message_block_height(&item.message))
        .map(|height| height.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn outbound_request_keys_csv(request: &NockchainRequest) -> String {
    match request {
        NockchainRequest::Request { message, .. } => pending_gen2_batch_request_key(message),
        NockchainRequest::Gossip { .. } => String::from("gossip"),
        NockchainRequest::AuthenticatedGossip { .. } => String::from("authenticated-gossip"),
        NockchainRequest::BatchRequest { items, .. } => batch_request_keys_csv(items),
    }
}

pub(crate) fn outbound_request_block_heights_csv(request: &NockchainRequest) -> String {
    match request {
        NockchainRequest::Request { message, .. } => request_message_block_height(message)
            .map(|height| height.to_string())
            .unwrap_or_default(),
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            String::new()
        }
        NockchainRequest::BatchRequest { items, .. } => batch_request_block_heights_csv(items),
    }
}

pub(crate) fn log_pending_gen2_batch_duplicate(
    peer_id: &PeerId,
    request_shape: &str,
    request_message: &[u8],
) {
    let request_key = pending_gen2_batch_request_key(request_message);
    trace!(
        peer = %peer_id,
        request_shape = request_shape,
        request_key = %request_key,
        reason = PENDING_GEN2_BATCH_DUPLICATE_REASON,
        "Suppressed duplicate request within pending gen2 batch"
    );
}
pub(crate) fn log_active_outbound_request_duplicate(
    peer_id: &PeerId,
    request_shape: &str,
    request_message: &[u8],
) {
    let request_key = pending_gen2_batch_request_key(request_message);
    trace!(
        peer = %peer_id,
        request_shape = request_shape,
        request_key = %request_key,
        reason = ACTIVE_OUTBOUND_REQUEST_DUPLICATE_REASON,
        "Suppressed duplicate request already in outbound flight"
    );
}

pub(crate) async fn suppress_duplicate_active_outbound_request(
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    peer_id: PeerId,
    request_shape: &str,
    request_message: &[u8],
) -> bool {
    let duplicate_inflight = {
        let state_guard = driver_state.lock().await;
        state_guard.has_active_outbound_request_item(peer_id, request_message)
    };
    if duplicate_inflight {
        metrics.req_res_effect_dedup_suppressed.increment();
        log_active_outbound_request_duplicate(&peer_id, request_shape, request_message);
        true
    } else {
        false
    }
}
pub(crate) fn new_gen2_batch_flush_interval(window: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(Instant::now() + window, window);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

pub(crate) fn block_batch_response_budget_bytes(limits: ReqResRuntimeLimits) -> usize {
    limits
        .gen2_block_batch_max_response_bytes
        .min(limits.gen2_batch_max_bytes)
}

pub(crate) fn response_estimate_fallback_message_bytes(limits: ReqResRuntimeLimits) -> usize {
    let response_budget = block_batch_response_budget_bytes(limits);
    if response_budget == 0 {
        return 0;
    }

    let envelope_headroom = response_budget
        .saturating_mul(31)
        .checked_div(32)
        .unwrap_or(response_budget)
        .max(1);
    limits.gen2_item_max_bytes.min(envelope_headroom)
}

pub(crate) fn batch_error_result(item_id: u32, error: BatchErrorClass) -> BatchResultItem {
    BatchResultItem {
        item_id,
        status: BatchResultStatus::Error,
        error: Some(error),
        envelope: None,
    }
}

pub(crate) fn req_res_message_encoded_bytes<T: serde::Serialize>(
    message: &T,
) -> Result<usize, NockAppError> {
    let mut encoded = Vec::new();
    cbor4ii::serde::to_writer(&mut encoded, message).map_err(|err| {
        NockAppError::OtherError(format!(
            "failed to encode req-res message with wire codec: {err}"
        ))
    })?;
    Ok(encoded.len())
}
pub(crate) fn batch_result_encoded_bytes(
    results: &[BatchResultItem],
) -> Result<usize, NockAppError> {
    req_res_message_encoded_bytes(&NockchainResponse::BatchResult {
        results: results.to_vec(),
    })
}
pub(crate) fn batch_results_fit(
    results: &[BatchResultItem],
    max_response_bytes: usize,
) -> Result<bool, NockAppError> {
    Ok(batch_result_encoded_bytes(results)? <= max_response_bytes)
}

pub(crate) fn batch_backpressure_results(items: &[BatchRequestItem]) -> Vec<BatchResultItem> {
    items
        .iter()
        .map(|item| batch_error_result(item.item_id, BatchErrorClass::Backpressure))
        .collect()
}
pub(crate) fn request_kind_name(request: &NockchainDataRequest) -> &'static str {
    match request {
        NockchainDataRequest::BlockByHeight(_) => "block-by-height",
        NockchainDataRequest::BlockWithTxsByHeight(_) => "block-with-txs-by-height",
        NockchainDataRequest::BlockRangeWithTxs { .. } => "block-range-with-txs",
        NockchainDataRequest::EldersById(_, _, _) => "elders-by-id",
        NockchainDataRequest::RawTransactionById(_, _) => "raw-tx-by-id",
    }
}

pub(crate) fn estimated_result_item(
    item_id: u32,
    estimate: &BatchItemResponseEstimate,
) -> BatchResultItem {
    let message = vec![0u8; estimate.message_bytes];
    let envelope = estimate.envelope.clone().into_sizing_envelope(message);

    BatchResultItem {
        item_id,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope),
    }
}
pub(crate) async fn estimate_batch_request_item_response(
    item: &BatchRequestItem,
    limits: ReqResRuntimeLimits,
    driver_state: &Arc<Mutex<P2PState>>,
) -> Result<Option<BatchItemResponseEstimate>, NockAppError> {
    let Ok(request) = decode_request_item_message(&item.message) else {
        return Ok(None);
    };
    let (message_bytes, source) = {
        let state_guard = driver_state.lock().await;
        state_guard.estimated_response_message_bytes(
            &request,
            response_estimate_fallback_message_bytes(limits),
        )
    };

    Ok(Some(BatchItemResponseEstimate {
        request_kind: request_kind_name(&request),
        envelope: BatchItemResponseEnvelopeEstimate::from_request(&request),
        message_bytes,
        source,
    }))
}

pub(crate) fn batch_results_with_backpressure_tail(
    head: &[BatchResultItem],
    tail: &[BatchRequestItem],
) -> Vec<BatchResultItem> {
    let mut results = Vec::with_capacity(head.len() + tail.len());
    results.extend(head.iter().cloned());
    results.extend(batch_backpressure_results(tail));
    results
}
pub(crate) fn batch_results_with_replacement_tail(
    head: &[BatchResultItem],
    current_item: &BatchRequestItem,
    replacement_error: BatchErrorClass,
    tail: &[BatchRequestItem],
) -> Vec<BatchResultItem> {
    let mut results = Vec::with_capacity(head.len() + 1 + tail.len());
    results.extend(head.iter().cloned());
    results.push(batch_error_result(current_item.item_id, replacement_error));
    results.extend(batch_backpressure_results(tail));
    results
}

pub(crate) fn batch_request_payload_bytes(
    items: &[BatchRequestItem],
) -> Result<usize, NockAppError> {
    items
        .iter()
        .try_fold(std::mem::size_of::<u32>(), |acc, item| {
            acc.checked_add(std::mem::size_of::<u32>())
                .and_then(|size| size.checked_add(std::mem::size_of::<u32>()))
                .and_then(|size| size.checked_add(item.message.len()))
                .ok_or_else(|| {
                    NockAppError::OtherError(String::from("batch payload size overflow"))
                })
        })
}
pub(crate) fn validate_batch_request_top_level_limits(
    items: &[BatchRequestItem],
    limits: ReqResRuntimeLimits,
) -> Result<(), BatchTopLevelLimitViolation> {
    if items.len() > limits.gen2_batch_max_items {
        return Err(BatchTopLevelLimitViolation::TooManyItems {
            item_count: items.len(),
            max_items: limits.gen2_batch_max_items,
        });
    }

    let payload_bytes = batch_request_payload_bytes(items).unwrap_or(usize::MAX);
    if payload_bytes > limits.gen2_batch_max_bytes {
        return Err(BatchTopLevelLimitViolation::TooManyBytes {
            payload_bytes,
            max_bytes: limits.gen2_batch_max_bytes,
        });
    }

    Ok(())
}

pub(crate) fn batch_request_item_too_large(
    item: &BatchRequestItem,
    limits: ReqResRuntimeLimits,
) -> bool {
    item.message.len() > limits.gen2_item_max_bytes
}
pub(crate) async fn execute_batch_request_items<E, EFut, F, Fut>(
    items: &[BatchRequestItem],
    max_response_bytes: usize,
    mut estimate_item: E,
    mut execute_item: F,
) -> Result<Vec<BatchResultItem>, NockAppError>
where
    E: FnMut(&BatchRequestItem) -> EFut,
    EFut: Future<Output = Result<Option<BatchItemResponseEstimate>, NockAppError>>,
    F: FnMut(&BatchRequestItem) -> Fut,
    Fut: Future<Output = BatchItemExecutionOutcome>,
{
    let all_backpressure = batch_backpressure_results(items);
    if !batch_results_fit(&all_backpressure, max_response_bytes)? {
        return Err(NockAppError::OtherError(String::from(
            "batch response size cap cannot fit a backpressure-only response",
        )));
    }

    let mut results = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let tail = &items[index + 1..];
        let retryable_tail = batch_results_with_backpressure_tail(&results, &items[index..]);
        if !batch_results_fit(&retryable_tail, max_response_bytes)? {
            return Err(NockAppError::OtherError(String::from(
                "batch response size cap cannot fit required backpressure tail",
            )));
        }
        let retryable_tail_bytes = batch_result_encoded_bytes(&retryable_tail)?;
        let mut projected_item_bytes = None;
        let mut projected_batch_bytes = None;
        let item_estimate = estimate_item(item).await?;
        if let Some(estimate) = item_estimate.as_ref() {
            let projected = estimated_result_item(item.item_id, estimate);
            let projected_single_bytes =
                batch_result_encoded_bytes(std::slice::from_ref(&projected))?;
            let mut projected_candidate = results.clone();
            projected_candidate.push(projected);
            projected_candidate.extend(batch_backpressure_results(tail));
            let projected_total_bytes = batch_result_encoded_bytes(&projected_candidate)?;
            projected_item_bytes = Some(projected_single_bytes);
            projected_batch_bytes = Some(projected_total_bytes);

            if projected_total_bytes > max_response_bytes {
                let stop_reason = if projected_single_bytes > max_response_bytes {
                    "estimated_item_too_large"
                } else {
                    "estimated_budget_exhausted"
                };
                warn!(
                    generation = "gen2",
                    item_id = item.item_id,
                    request_kind = estimate.request_kind,
                    estimate_source = estimate.source,
                    projected_item_bytes = projected_single_bytes,
                    projected_batch_bytes = projected_total_bytes,
                    retryable_tail_bytes,
                    configured_cap = max_response_bytes,
                    stop_reason,
                    "Stopping batch before executing item because estimate exceeds response budget"
                );
                if projected_single_bytes > max_response_bytes {
                    return Ok(batch_results_with_replacement_tail(
                        &results,
                        item,
                        BatchErrorClass::TooLarge,
                        tail,
                    ));
                }
                return Ok(retryable_tail);
            }
        }

        match execute_item(item).await {
            BatchItemExecutionOutcome::Completed(outcome) => {
                let actual = outcome.into_batch_result_item(item.item_id);
                let mut candidate = results.clone();
                candidate.push(actual.clone());
                candidate.extend(batch_backpressure_results(tail));
                let actual_total_bytes = batch_result_encoded_bytes(&candidate)?;
                if actual_total_bytes <= max_response_bytes {
                    results.push(actual);
                    continue;
                }

                let replacement_error =
                    if batch_results_fit(std::slice::from_ref(&actual), max_response_bytes)? {
                        BatchErrorClass::Backpressure
                    } else {
                        BatchErrorClass::TooLarge
                    };
                warn!(
                    generation = "gen2",
                    item_id = item.item_id,
                    projected_item_bytes,
                    projected_batch_bytes,
                    actual_batch_bytes = actual_total_bytes,
                    retryable_tail_bytes,
                    configured_cap = max_response_bytes,
                    stop_reason = match replacement_error {
                        BatchErrorClass::Backpressure => "actual_budget_exhausted",
                        BatchErrorClass::TooLarge => "actual_item_too_large",
                        _ => "actual_overflow",
                    },
                    "Executed batch item exceeded response budget"
                );
                return Ok(batch_results_with_replacement_tail(
                    &results, item, replacement_error, tail,
                ));
            }
            BatchItemExecutionOutcome::Failed(error) => {
                let failed = batch_error_result(item.item_id, error);
                let mut candidate = results.clone();
                candidate.push(failed.clone());
                candidate.extend(batch_backpressure_results(tail));
                if batch_results_fit(&candidate, max_response_bytes)? {
                    results.push(failed);
                    continue;
                }

                return Ok(batch_results_with_replacement_tail(
                    &results,
                    item,
                    BatchErrorClass::Backpressure,
                    tail,
                ));
            }
            BatchItemExecutionOutcome::Backpressure => {
                return Ok(retryable_tail);
            }
        }
    }
    Ok(results)
}

pub(crate) async fn execute_batch_request_item(
    peer: PeerId,
    item: &BatchRequestItem,
    limits: ReqResRuntimeLimits,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
) -> BatchItemExecutionOutcome {
    if batch_request_item_too_large(item, limits) {
        warn!(
            peer = %peer,
            generation = "gen2",
            item_id = item.item_id,
            observed_bytes = item.message.len(),
            configured_cap = limits.gen2_item_max_bytes,
            "Batch request item exceeded per-item byte limit"
        );
        return BatchItemExecutionOutcome::Failed(BatchErrorClass::TooLarge);
    }

    let data_request = match decode_request_item_message(&item.message) {
        Ok(data_request) => data_request,
        Err(err) => {
            debug!(
                peer = %peer,
                item_id = item.item_id,
                error = %err,
                "Batch request item failed to decode"
            );
            return BatchItemExecutionOutcome::Failed(BatchErrorClass::Decode);
        }
    };

    match execute_request_item(peer, data_request, limits, traffic, metrics, driver_state).await {
        Ok(outcome) => BatchItemExecutionOutcome::Completed(outcome),
        Err(err) => map_batch_request_execution_error(&peer, item.item_id, err),
    }
}
pub(crate) fn map_batch_request_execution_error(
    peer: &PeerId,
    item_id: u32,
    err: NockAppError,
) -> BatchItemExecutionOutcome {
    match err {
        NockAppError::MPSCFullError(_) => {
            trace!(
                peer = %peer,
                item_id,
                decision = queue_saturation_decision(QueueSaturationPath::RequestExecution),
                saturation_path = queue_saturation_path(QueueSaturationPath::RequestExecution),
                "Batch request execution hit backpressure"
            );
            BatchItemExecutionOutcome::Backpressure
        }
        NockAppError::CrownError(CrownError::Timeout) => {
            warn!(
                peer = %peer,
                item_id,
                error = %err,
                "Batch request execution timed out waiting on the kernel; retrying tail as backpressure"
            );
            BatchItemExecutionOutcome::Backpressure
        }
        NockAppError::OneShotRecvError(_) | NockAppError::ChannelClosedError => {
            warn!(
                peer = %peer,
                item_id,
                error = %err,
                "Batch request execution lost its local response channel; retrying tail as backpressure"
            );
            BatchItemExecutionOutcome::Backpressure
        }
        err => {
            warn!(
                peer = %peer,
                item_id,
                error = %err,
                "Batch request item failed during execution"
            );
            BatchItemExecutionOutcome::Failed(BatchErrorClass::Internal)
        }
    }
}

pub(crate) fn validate_batch_request_items(items: &[BatchRequestItem]) -> Result<(), NockAppError> {
    let mut seen_item_ids = std::collections::BTreeSet::new();
    for item in items {
        if !seen_item_ids.insert(item.item_id) {
            return Err(NockAppError::OtherError(format!(
                "duplicate batch item_id {}",
                item.item_id
            )));
        }
    }
    Ok(())
}
