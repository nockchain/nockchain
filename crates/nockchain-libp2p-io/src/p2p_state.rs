use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use libp2p::core::ConnectedPoint;
use libp2p::request_response::OutboundRequestId;
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId, Swarm};
use nockapp::noun::slab::NounSlab;
use nockapp::NockAppError;
#[cfg(test)]
use nockvm::noun::NounAllocator;
use nockvm::noun::{Noun, NounSpace};
use rand::prelude::SliceRandom;
use tracing::{debug, info, trace};

use crate::catch_up::{CatchUpSignal, ModeTransition};
use crate::ip_block::PeerExclusions;
use crate::messages::{NockchainDataRequest, NockchainFact, NockchainRequest, RequestReplayKey};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_util::{multiaddr_without_p2p, MultiaddrExt};
use crate::peer_stats::{
    global_peer_stats_registry, unix_timestamp_millis, PeerReqResGeneration, PeerStatsEntry,
    PeerStatsRegistry, PeerStatsSnapshot,
};
use crate::tip5_util::tip5_hash_to_base58;

const TX_SOURCE_HINT_CAP: usize = 65_536;
const BLOCK_HEIGHT_ATTEMPT_CAP: usize = 65_536;
const SPECULATIVE_TX_PREFETCH_CAP: usize = 8_192;
const KERNEL_BLOCK_HEIGHT_REQUEST_CAP: usize = 65_536;
const SEEN_BLOCKS_CAP: usize = 65_536;
const BLOCK_RECEIPT_CAP: usize = 65_536;
const ELDERS_NEGATIVE_CACHE_CAP: usize = 8_192;
const RESPONSE_SIZE_HINT_CAP: usize = 16_384;
const DEFERRED_HEARD_BLOCK_TOTAL_CAP: usize = 65_536;
pub(crate) const DEFERRED_HEARD_BLOCK_PER_PEER_CAP: usize = 4_096;
const OBSERVED_BLOCK_HEIGHT_CANDIDATE_CAP: usize = 65_536;
const INBOUND_REPLAY_TOTAL_CAP: usize = 65_536;

#[derive(Default)]
struct IpInfo {
    request_count: u64,
    ping_failure_count: u64,
    connections: BTreeSet<ConnectionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct IpBucket(IpAddr);

impl IpBucket {
    pub(crate) fn from_ip(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(ip) => Self(IpAddr::V4(ip)),
            IpAddr::V6(ip) => {
                let masked = u128::from(ip) & (!0u128 << 64);
                Self(IpAddr::V6(masked.into()))
            }
        }
    }
}

impl fmt::Display for IpBucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            IpAddr::V4(ip) => write!(f, "{ip}/32"),
            IpAddr::V6(ip) => write!(f, "{ip}/64"),
        }
    }
}

#[derive(Debug, Clone)]
struct GossipBucketState {
    tokens: u32,
    last_refill: Instant,
}

#[derive(Default)]
struct IpBucketInfo {
    request_count: u64,
    connections: BTreeSet<ConnectionId>,
    gossip: Option<GossipBucketState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboundReplayAdmission {
    Accepted,
    Replayed,
    Disabled,
}

#[derive(Default)]
struct PeerReplayCache {
    entries: BTreeMap<RequestReplayKey, Instant>,
    order: VecDeque<RequestReplayKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpBucketAdmission {
    UnknownConnection,
    MissingIp,
    Admitted {
        bucket: IpBucket,
        count: u64,
        limit: u64,
    },
    Rejected {
        bucket: IpBucket,
        count: u64,
        limit: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GossipBucketAdmission {
    UnknownConnection,
    MissingIp,
    Admitted {
        bucket: IpBucket,
        remaining_tokens: u32,
    },
    Rejected {
        bucket: IpBucket,
    },
}

#[derive(Default)]
struct PeerRequestHealth {
    successes: u64,
    failures: u64,
    last_success: Option<Instant>,
    last_failure: Option<Instant>,
}

#[derive(Default)]
struct ResponseSizeHints {
    block_message_bytes_by_height: BTreeMap<u64, usize>,
    bundle_message_bytes_by_height: BTreeMap<u64, usize>,
    tx_message_bytes_by_id: BTreeMap<String, usize>,
    elders_message_bytes_by_id: BTreeMap<String, usize>,
}

/// Phase 5 per-height retry budget record. `failures` is the running
/// consecutive-failure count; once it hits the configured budget,
/// `stuck_until` is set and further retries skip until the backoff
/// elapses. Cleared on a successful response that routes the height
/// to the kernel.
#[derive(Debug, Clone)]
pub struct HeightFailureRecord {
    pub failures: u8,
    pub stuck_until: Option<Instant>,
    pub last_observed: Instant,
}

/// Bandwidth window length for the prefetch-per-peer cap.
const PREFETCH_BANDWIDTH_WINDOW: Duration = Duration::from_secs(60);
const PREFETCH_PEER_COOLDOWN_INITIAL: Duration = Duration::from_secs(2);
const PREFETCH_PEER_COOLDOWN_MAX: Duration = Duration::from_secs(60);
const PREFETCH_PEER_ACTIVE_REQUEST_BIAS: f64 = 1.5;
const PREFETCH_PEER_EWMA_ALPHA: f64 = 0.2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RangeCapability {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Default)]
struct PrefetchPeerRangeStats {
    capability: RangeCapability,
    success_count: u64,
    failure_count: u64,
    timeout_count: u64,
    response_bytes_ewma: Option<f64>,
    rtt_ms_ewma: Option<f64>,
    cooldown_until: Option<Instant>,
}

/// Phase 4 prefetch context tracked alongside an `OutboundRequestId`.
/// Recorded on dispatch via `register_prefetch` and cleared on response or
/// failure via `clear_prefetch`. Carries enough metadata to maintain the
/// covered-heights set and per-peer counts. `started_at` is wired up
/// today so Phase 5 watchdog instrumentation can flag stuck prefetches
/// without changing the field surface.
#[derive(Debug, Clone)]
pub struct InflightPrefetch {
    pub peer_id: PeerId,
    pub start_height: u64,
    pub len: u8,
    #[allow(dead_code)]
    pub started_at: Instant,
}

fn height_in_prefetch(height: u64, prefetch: &InflightPrefetch) -> bool {
    let end_exclusive = prefetch.start_height + u64::from(prefetch.len);
    height >= prefetch.start_height && height < end_exclusive
}

/// Extract the (start_height, len) of a `BlockRangeWithTxs` request, or
/// `None` for any other request shape. Used by `record_outbound_request`
/// to register catch-up prefetches without changing the SwarmAction
/// surface.
fn prefetch_range_from_request(request: &NockchainRequest) -> Option<(u64, u8)> {
    let items: &[crate::messages::BatchRequestItem] = match request {
        NockchainRequest::Request { message, .. } => {
            return match crate::messages::decode_request_item_message(message) {
                Ok(NockchainDataRequest::BlockRangeWithTxs { start_height, len }) => {
                    Some((start_height, len))
                }
                _ => None,
            }
        }
        NockchainRequest::BatchRequest { items, .. } => items.as_slice(),
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            return None;
        }
    };
    if let [item] = items {
        if let Ok(NockchainDataRequest::BlockRangeWithTxs { start_height, len }) =
            crate::messages::decode_request_item_message(&item.message)
        {
            return Some((start_height, len));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSource {
    /// Block arrived via inbound gossip / request-response with a height
    /// above the current frontier. The original deferred-block flow.
    Gossip,
    /// Block arrived via an explicit catch-up prefetch request. Phase 4
    /// will mint these; Phase 2 only adds the variant so the buffer can
    /// distinguish provenance for metrics and invalidation.
    #[allow(dead_code)]
    Prefetch,
}

#[derive(Debug, Clone)]
struct DeferredHeardBlock {
    peer_id: PeerId,
    fact: NockchainFact,
    /// Provenance of the buffered block. Phase 4 (catch-up prefetch trigger)
    /// reads this for cache-hit attribution and reorg invalidation; Phase 2
    /// only sets it. The `dead_code` allow keeps the field visible to
    /// downstream phases without churning the warning.
    #[allow(dead_code)]
    source: BlockSource,
}

impl ResponseSizeHints {
    const BLOCK_HEIGHT_WINDOW: u64 = 16;
    const BLOCK_FORWARD_SAMPLE_COUNT: usize = 16;
    const RANGE_RESPONSE_TARGET_NUMERATOR: usize = 31;
    const RANGE_RESPONSE_TARGET_DENOMINATOR: usize = 32;

    fn estimate(
        &self,
        request: &NockchainDataRequest,
        fallback_message_bytes: usize,
    ) -> (usize, &'static str) {
        match request {
            NockchainDataRequest::BlockByHeight(height) => {
                self.estimate_block_message_bytes(*height, fallback_message_bytes)
            }
            NockchainDataRequest::BlockWithTxsByHeight(height) => {
                self.estimate_bundle_message_bytes(*height, fallback_message_bytes)
            }
            NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                // Per-block estimate scaled by the requested length. The
                // caller-supplied `fallback_message_bytes` is sized for a
                // single-bundle worst case (gen2_item_max_bytes around 2 MiB);
                // multiplying that ceiling by `len` would project a cold-
                // cache range well past the batch cap and cause the
                // responder to reject the request as TooLarge before
                // any real bundle data was ever observed. Divide the
                // single-bundle fallback by `len`, with response-budget
                // headroom for CBOR envelope overhead, to admit the
                // request and let the actual packing budget enforce the
                // real ceiling.
                let len_usize = usize::from(*len).max(1);
                let cold_target = fallback_message_bytes
                    .saturating_mul(Self::RANGE_RESPONSE_TARGET_NUMERATOR)
                    .checked_div(Self::RANGE_RESPONSE_TARGET_DENOMINATOR)
                    .unwrap_or(fallback_message_bytes)
                    .max(1);
                let cold_fallback = cold_target.checked_div(len_usize).unwrap_or(1).max(1);
                let (per_block, basis) =
                    self.estimate_bundle_message_bytes(*start_height, cold_fallback);
                let uncapped_total = per_block.saturating_mul(len_usize);
                let total = uncapped_total.min(cold_target);
                let source = match basis {
                    "configured_bundle_cap" => "range_cold_budget_scaled",
                    _ if total < uncapped_total => "range_bundle_capped",
                    _ => "range_bundle_scaled",
                };
                (total, source)
            }
            NockchainDataRequest::EldersById(id, _, _) => self.estimate_id_message_bytes(
                &self.elders_message_bytes_by_id, id, fallback_message_bytes,
            ),
            NockchainDataRequest::RawTransactionById(id, _) => self.estimate_id_message_bytes(
                &self.tx_message_bytes_by_id, id, fallback_message_bytes,
            ),
        }
    }

    fn estimate_id_message_bytes(
        &self,
        hints: &BTreeMap<String, usize>,
        id: &str,
        fallback_message_bytes: usize,
    ) -> (usize, &'static str) {
        hints
            .get(id)
            .copied()
            .map(|message_bytes| (message_bytes, "exact_request"))
            .unwrap_or((fallback_message_bytes, "configured_fallback"))
    }

    fn estimate_block_message_bytes(
        &self,
        height: u64,
        fallback_message_bytes: usize,
    ) -> (usize, &'static str) {
        if let Some(message_bytes) = self.block_message_bytes_by_height.get(&height).copied() {
            return (message_bytes, "exact_request");
        }

        if let Some((highest_height, _)) = self.block_message_bytes_by_height.last_key_value() {
            if height > *highest_height {
                if let Some(message_bytes) = self
                    .block_message_bytes_by_height
                    .iter()
                    .rev()
                    .take(Self::BLOCK_FORWARD_SAMPLE_COUNT)
                    .map(|(_, message_bytes)| *message_bytes)
                    .max()
                {
                    return (message_bytes, "block_forward_tail");
                }
            }
        }

        let window_start = height.saturating_sub(Self::BLOCK_HEIGHT_WINDOW);
        let window_end = height.saturating_add(Self::BLOCK_HEIGHT_WINDOW);
        self.block_message_bytes_by_height
            .range(window_start..=window_end)
            .map(|(_, message_bytes)| *message_bytes)
            .max()
            .map(|message_bytes| (message_bytes, "block_height_window"))
            .unwrap_or((fallback_message_bytes, "configured_fallback"))
    }

    fn estimate_bundle_message_bytes(
        &self,
        height: u64,
        fallback_message_bytes: usize,
    ) -> (usize, &'static str) {
        if let Some(message_bytes) = self.bundle_message_bytes_by_height.get(&height).copied() {
            return (message_bytes, "exact_bundle_request");
        }

        if let Some((highest_height, _)) = self.bundle_message_bytes_by_height.last_key_value() {
            if height > *highest_height {
                if let Some(message_bytes) = self
                    .bundle_message_bytes_by_height
                    .iter()
                    .rev()
                    .take(Self::BLOCK_FORWARD_SAMPLE_COUNT)
                    .map(|(_, message_bytes)| *message_bytes)
                    .max()
                {
                    return (message_bytes, "bundle_forward_tail");
                }
            }
        }

        let window_start = height.saturating_sub(Self::BLOCK_HEIGHT_WINDOW);
        let window_end = height.saturating_add(Self::BLOCK_HEIGHT_WINDOW);
        self.bundle_message_bytes_by_height
            .range(window_start..=window_end)
            .map(|(_, message_bytes)| *message_bytes)
            .max()
            .map(|message_bytes| (message_bytes, "bundle_height_window"))
            .unwrap_or((fallback_message_bytes, "configured_bundle_cap"))
    }

    fn record_max<K: Clone + Ord>(hints: &mut BTreeMap<K, usize>, key: K, message_bytes: usize) {
        hints
            .entry(key)
            .and_modify(|existing| *existing = (*existing).max(message_bytes))
            .or_insert(message_bytes);
        while hints.len() > RESPONSE_SIZE_HINT_CAP {
            let Some(oldest_key) = hints.keys().next().cloned() else {
                break;
            };
            hints.remove(&oldest_key);
        }
    }

    fn record(&mut self, request: &NockchainDataRequest, message_bytes: usize) {
        match request {
            NockchainDataRequest::BlockByHeight(height) => {
                Self::record_max(
                    &mut self.block_message_bytes_by_height, *height, message_bytes,
                );
            }
            NockchainDataRequest::BlockWithTxsByHeight(height) => {
                Self::record_max(
                    &mut self.bundle_message_bytes_by_height, *height, message_bytes,
                );
            }
            NockchainDataRequest::EldersById(id, _, _) => {
                Self::record_max(
                    &mut self.elders_message_bytes_by_id,
                    id.clone(),
                    message_bytes,
                );
            }
            NockchainDataRequest::RawTransactionById(id, _) => {
                Self::record_max(&mut self.tx_message_bytes_by_id, id.clone(), message_bytes);
            }
            NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                let len_usize = usize::from(*len).max(1);
                let per_height = message_bytes.div_ceil(len_usize).max(1);
                for offset in 0..u64::from(*len) {
                    Self::record_max(
                        &mut self.bundle_message_bytes_by_height,
                        start_height.saturating_add(offset),
                        per_height,
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqResGeneration {
    Gen1,
    Gen2,
}

#[derive(Debug, Clone)]
pub struct OutboundRequestContext {
    pub peer_id: PeerId,
    pub generation: ReqResGeneration,
    pub request: NockchainRequest,
    pub retry_count: u8,
    pub fallback_attempted: bool,
    pub started_at: Instant,
}

impl OutboundRequestContext {
    pub fn new(peer_id: PeerId, generation: ReqResGeneration, request: NockchainRequest) -> Self {
        Self::with_attempt(peer_id, generation, request, 0, false)
    }

    pub fn with_attempt(
        peer_id: PeerId,
        generation: ReqResGeneration,
        request: NockchainRequest,
        retry_count: u8,
        fallback_attempted: bool,
    ) -> Self {
        Self {
            peer_id,
            generation,
            request,
            retry_count,
            fallback_attempted,
            started_at: Instant::now(),
        }
    }

    pub fn logical_request_count(&self) -> Option<u64> {
        match &self.request {
            NockchainRequest::Request { .. } => Some(1),
            NockchainRequest::BatchRequest { items, .. } => Some(items.len() as u64),
            NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => None,
        }
    }

    pub fn mark_started(&mut self) {
        self.started_at = Instant::now();
    }
}

fn request_item_messages(request: &NockchainRequest) -> Vec<&[u8]> {
    match request {
        NockchainRequest::Request { message, .. } => vec![message.as_ref()],
        NockchainRequest::BatchRequest { items, .. } => {
            items.iter().map(|item| item.message.as_ref()).collect()
        }
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PeerStatsAccumulator {
    generation: PeerReqResGeneration,
    connected_at: Option<Instant>,
    request_count: u64,
    request_exchange_count: u64,
    bytes_sent: u64,
    bytes_received: u64,
    round_trip_total_ms: f64,
    round_trip_samples: u64,
    failure_count: u64,
    timeout_count: u64,
    blocks_received: u64,
    block_propagation_total_ms: f64,
    block_propagation_samples: u64,
}

impl PeerStatsAccumulator {
    fn connection_duration_seconds(&self) -> u64 {
        self.connected_at
            .map(|connected_at| connected_at.elapsed().as_secs())
            .unwrap_or(0)
    }
    fn average_round_trip_ms(&self) -> f64 {
        if self.round_trip_samples == 0 {
            0.0
        } else {
            self.round_trip_total_ms / self.round_trip_samples as f64
        }
    }

    fn average_batch_size(&self) -> f64 {
        if self.request_exchange_count == 0 {
            0.0
        } else {
            self.request_count as f64 / self.request_exchange_count as f64
        }
    }

    fn average_block_propagation_ms(&self) -> f64 {
        if self.block_propagation_samples == 0 {
            0.0
        } else {
            self.block_propagation_total_ms / self.block_propagation_samples as f64
        }
    }
}

pub struct P2PState {
    metrics: Arc<NockchainP2PMetrics>,
    peer_stats_registry: Arc<PeerStatsRegistry>,
    block_id_to_peers: BTreeMap<String, BTreeSet<PeerId>>,
    observed_block_height_candidates: BTreeMap<String, u64>,
    observed_block_height_candidate_order: VecDeque<String>,
    block_first_seen_at: BTreeMap<String, Instant>,
    block_receipts: BTreeMap<String, BTreeSet<PeerId>>,
    block_receipt_order: VecDeque<String>,
    peer_to_block_ids: BTreeMap<PeerId, BTreeSet<String>>,
    // It's stupid that we must track this state locally. libp2p does not expose
    // the lookup shape this driver needs.
    block_height_attempted_peers: BTreeMap<u64, BTreeSet<PeerId>>,
    peer_to_block_height_attempts: BTreeMap<PeerId, BTreeSet<u64>>,
    block_height_attempt_order: VecDeque<u64>,
    tx_id_to_peers: BTreeMap<String, BTreeSet<PeerId>>,
    peer_to_tx_ids: BTreeMap<PeerId, BTreeSet<String>>,
    tx_source_hint_order: VecDeque<String>,
    peer_stats: BTreeMap<PeerId, PeerStatsAccumulator>,
    outbound_requests: BTreeMap<OutboundRequestId, OutboundRequestContext>,
    peer_to_outbound_requests: BTreeMap<PeerId, BTreeSet<OutboundRequestId>>,
    peer_to_outbound_request_items: BTreeMap<PeerId, BTreeMap<Vec<u8>, usize>>,
    inbound_req_res_inflight: BTreeMap<PeerId, usize>,
    // libp2p does not expose the lookup shape this driver needs; keep it locally.
    connections: BTreeMap<ConnectionId, PeerId>,
    // subset of connections: all inbound connections
    inbound_connections: BTreeMap<ConnectionId, PeerId>,
    pub(crate) peer_connections: BTreeMap<PeerId, BTreeMap<ConnectionId, Multiaddr>>,
    ip_info: BTreeMap<IpAddr, IpInfo>,
    ip_bucket_info: BTreeMap<IpBucket, IpBucketInfo>,
    inbound_replay_by_peer: BTreeMap<PeerId, PeerReplayCache>,
    inbound_replay_order: VecDeque<(PeerId, RequestReplayKey)>,
    inbound_replay_total_count: usize,
    pub seen_blocks: BTreeSet<String>,
    seen_block_order: VecDeque<String>,
    pub seen_txs: BTreeSet<String>,
    processing_blocks: BTreeSet<String>,
    processing_txs: BTreeSet<String>,
    pub block_cache: BTreeMap<u64, NounSlab>,
    pub tx_cache: BTreeMap<String, NounSlab>,
    pub elders_cache: BTreeMap<String, NounSlab>,
    pub elders_negative_cache: BTreeSet<String>,
    elders_negative_cache_order: VecDeque<String>,
    peer_request_health: BTreeMap<PeerId, PeerRequestHealth>,
    deferred_heard_blocks: BTreeMap<u64, BTreeMap<String, DeferredHeardBlock>>,
    deferred_heard_block_count_by_peer: BTreeMap<PeerId, usize>,
    deferred_heard_block_total_count: usize,
    kernel_block_height_request_order: VecDeque<u64>,
    kernel_requested_block_heights: BTreeSet<u64>,
    speculative_tx_prefetches: BTreeMap<String, Instant>,
    /// Peers that returned a `Decode` error for a block-with-txs bundle
    /// request. We stick them on this set so subsequent `by-height` requests
    /// to the same peer fall back to the classic `%block %by-height` shape
    /// rather than eating another per-request round-trip.
    non_bundle_capable_peers: BTreeSet<PeerId>,
    /// Peers that returned a `Decode` error for a block-range bundle
    /// request. Mirror of `non_bundle_capable_peers` for the Phase 3 range
    /// path: once a peer signals it does not understand the wire shape, we
    /// fall back to per-height singleton requests and avoid repeated decode
    /// failures.
    non_range_capable_peers: BTreeSet<PeerId>,
    /// Phase 4 prefetch tracking. Each currently-inflight catch-up
    /// prefetch is keyed by the libp2p `OutboundRequestId` it was
    /// registered under, mirroring `outbound_requests`. The set of
    /// heights currently covered by inflight prefetches is precomputed
    /// in `prefetched_heights` so the kernel-singleton suppression
    /// check is O(log n).
    inflight_prefetches: BTreeMap<OutboundRequestId, InflightPrefetch>,
    /// Per-peer count of inflight prefetches. Used to enforce
    /// `prefetch_max_inflight_per_peer` without scanning
    /// `inflight_prefetches`.
    inflight_prefetch_count_per_peer: BTreeMap<PeerId, u8>,
    /// Heights currently covered by any inflight prefetch. Maintained
    /// alongside `inflight_prefetches` so the
    /// `is_prefetch_inflight_covering_height` lookup is a single
    /// `BTreeSet::contains`.
    prefetched_heights: BTreeSet<u64>,
    /// Phase 5: per-height retry budget. Each entry tracks consecutive
    /// fetch failures for a height; once the count hits the configured
    /// budget the height is marked stuck until the backoff window passes.
    /// Cleared when a successful response routes the height to the kernel.
    block_height_failures: BTreeMap<u64, HeightFailureRecord>,
    /// Phase 5: sliding-window prefetch byte counter per peer. Each entry
    /// is `VecDeque<(observed_at, response_bytes)>`; entries older than
    /// 60s are pruned on each insertion. Used to cap bad-peer amplification.
    prefetch_bandwidth_window: BTreeMap<PeerId, VecDeque<(Instant, usize)>>,
    prefetch_peer_range_stats: BTreeMap<PeerId, PrefetchPeerRangeStats>,
    /// Phase 5: per-height failure budget; if `None` use the
    /// `PREFETCH_HEIGHT_FAILURE_BUDGET` constant. Set by the driver at
    /// startup from the active `LibP2PConfig`.
    prefetch_height_failure_budget: u8,
    prefetch_stuck_backoff: Duration,
    prefetch_bandwidth_cap_per_peer_bytes_per_min: usize,
    response_size_hints: ResponseSizeHints,
    // Highest block height seen
    pub first_negative: u64,
    pub seen_tx_clear_interval: u64,
    pub last_tx_cache_clear_height: u64,
    catch_up: CatchUpSignal,
}

impl P2PState {
    pub fn new(metrics: Arc<NockchainP2PMetrics>, seen_tx_clear_interval: u64) -> Self {
        Self::with_peer_stats_registry(
            metrics,
            seen_tx_clear_interval,
            global_peer_stats_registry(),
        )
    }

    pub fn with_peer_stats_registry(
        metrics: Arc<NockchainP2PMetrics>,
        seen_tx_clear_interval: u64,
        peer_stats_registry: Arc<PeerStatsRegistry>,
    ) -> Self {
        let state = Self {
            metrics,
            peer_stats_registry,
            block_id_to_peers: BTreeMap::new(),
            observed_block_height_candidates: BTreeMap::new(),
            observed_block_height_candidate_order: VecDeque::new(),
            block_first_seen_at: BTreeMap::new(),
            block_receipts: BTreeMap::new(),
            block_receipt_order: VecDeque::new(),
            peer_to_block_ids: BTreeMap::new(),
            block_height_attempted_peers: BTreeMap::new(),
            peer_to_block_height_attempts: BTreeMap::new(),
            block_height_attempt_order: VecDeque::new(),
            tx_id_to_peers: BTreeMap::new(),
            peer_to_tx_ids: BTreeMap::new(),
            tx_source_hint_order: VecDeque::new(),
            peer_stats: BTreeMap::new(),
            outbound_requests: BTreeMap::new(),
            peer_to_outbound_requests: BTreeMap::new(),
            peer_to_outbound_request_items: BTreeMap::new(),
            inbound_req_res_inflight: BTreeMap::new(),
            connections: BTreeMap::new(),
            inbound_connections: BTreeMap::new(),
            peer_connections: BTreeMap::new(),
            ip_info: BTreeMap::new(),
            ip_bucket_info: BTreeMap::new(),
            inbound_replay_by_peer: BTreeMap::new(),
            inbound_replay_order: VecDeque::new(),
            inbound_replay_total_count: 0,
            seen_blocks: BTreeSet::new(),
            seen_block_order: VecDeque::new(),
            seen_txs: BTreeSet::new(),
            processing_blocks: BTreeSet::new(),
            processing_txs: BTreeSet::new(),
            block_cache: BTreeMap::new(),
            tx_cache: BTreeMap::new(),
            elders_cache: BTreeMap::new(),
            elders_negative_cache: BTreeSet::new(),
            elders_negative_cache_order: VecDeque::new(),
            peer_request_health: BTreeMap::new(),
            deferred_heard_blocks: BTreeMap::new(),
            deferred_heard_block_count_by_peer: BTreeMap::new(),
            deferred_heard_block_total_count: 0,
            kernel_block_height_request_order: VecDeque::new(),
            kernel_requested_block_heights: BTreeSet::new(),
            speculative_tx_prefetches: BTreeMap::new(),
            non_bundle_capable_peers: BTreeSet::new(),
            non_range_capable_peers: BTreeSet::new(),
            inflight_prefetches: BTreeMap::new(),
            inflight_prefetch_count_per_peer: BTreeMap::new(),
            prefetched_heights: BTreeSet::new(),
            block_height_failures: BTreeMap::new(),
            prefetch_bandwidth_window: BTreeMap::new(),
            prefetch_peer_range_stats: BTreeMap::new(),
            prefetch_height_failure_budget: 4,
            prefetch_stuck_backoff: Duration::from_secs(60),
            prefetch_bandwidth_cap_per_peer_bytes_per_min: 50 * 1024 * 1024,
            response_size_hints: ResponseSizeHints::default(),
            first_negative: 0,
            seen_tx_clear_interval,
            last_tx_cache_clear_height: 0,
            catch_up: CatchUpSignal::new(),
        };
        state.update_req_res_inflight_metrics();
        state.refresh_peer_stats_snapshot();
        state.publish_catch_up_metrics();
        state
    }

    fn req_res_generation(generation: ReqResGeneration) -> PeerReqResGeneration {
        match generation {
            ReqResGeneration::Gen1 => PeerReqResGeneration::Gen1,
            ReqResGeneration::Gen2 => PeerReqResGeneration::Gen2,
        }
    }

    fn ensure_peer_stats(&mut self, peer_id: PeerId) -> &mut PeerStatsAccumulator {
        self.peer_stats.entry(peer_id).or_default()
    }

    fn encoded_request_bytes(request: &NockchainRequest) -> u64 {
        let mut encoded = Vec::new();
        cbor4ii::serde::to_writer(&mut encoded, request)
            .map(|_| encoded.len() as u64)
            .unwrap_or(0)
    }

    fn peer_stats_snapshot(&self) -> PeerStatsSnapshot {
        let peers = self
            .peer_connections
            .keys()
            .copied()
            .map(|peer_id| {
                let stats = self.peer_stats.get(&peer_id).cloned().unwrap_or_default();
                PeerStatsEntry {
                    peer_id: peer_id.to_base58(),
                    protocol_generation: stats.generation,
                    request_count: stats.request_count,
                    bytes_sent: stats.bytes_sent,
                    bytes_received: stats.bytes_received,
                    average_round_trip_ms: stats.average_round_trip_ms(),
                    average_batch_size: stats.average_batch_size(),
                    failure_count: stats.failure_count,
                    timeout_count: stats.timeout_count,
                    blocks_received: stats.blocks_received,
                    average_block_propagation_ms: stats.average_block_propagation_ms(),
                    connection_duration_seconds: stats.connection_duration_seconds(),
                }
            })
            .collect();

        PeerStatsSnapshot {
            collected_at_unix_ms: unix_timestamp_millis(),
            peers,
        }
    }

    fn refresh_peer_stats_snapshot(&self) {
        self.peer_stats_registry
            .replace_snapshot(self.peer_stats_snapshot());
    }

    fn update_req_res_inflight_metrics(&self) {
        let total_outbound = self.outbound_requests.len();
        let total_inbound: usize = self.inbound_req_res_inflight.values().copied().sum();
        let mut peers = self
            .peer_to_outbound_requests
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        peers.extend(self.inbound_req_res_inflight.keys().copied());
        let max_per_peer = peers
            .into_iter()
            .map(|peer_id| self.req_res_inflight_for_peer(peer_id))
            .max()
            .unwrap_or(0);

        let _ = self
            .metrics
            .req_res_inflight_total
            .swap((total_outbound + total_inbound) as f64);
        let _ = self
            .metrics
            .req_res_inflight_max_per_peer
            .swap(max_per_peer as f64);
    }

    pub(crate) fn track_connection(
        &mut self,
        connection_id: ConnectionId,
        peer_id: PeerId,
        addr: &Multiaddr,
        endpoint: ConnectedPoint,
    ) {
        self.connections.insert(connection_id, peer_id);
        if let ConnectedPoint::Listener { .. } = endpoint {
            self.inbound_connections.insert(connection_id, peer_id);
        }
        if let Some(c) = self.peer_connections.get_mut(&peer_id) {
            c.insert(connection_id, addr.clone());
        } else {
            let mut new_map = BTreeMap::new();
            new_map.insert(connection_id, addr.clone());
            self.peer_connections.insert(peer_id, new_map);
        }
        if let Some(ip) = addr.ip_addr() {
            if let Some(info) = self.ip_info.get_mut(&ip) {
                info.connections.insert(connection_id);
            } else {
                let mut connections = BTreeSet::new();
                connections.insert(connection_id);
                self.ip_info.insert(
                    ip,
                    IpInfo {
                        connections,
                        request_count: 0,
                        ping_failure_count: 0,
                    },
                );
            }
            self.ip_bucket_info
                .entry(IpBucket::from_ip(ip))
                .or_default()
                .connections
                .insert(connection_id);
        }
        let peer_count = self.peer_connections.len() as f64;
        let _ = self.metrics.peer_count.swap(peer_count);
        let stats = self.ensure_peer_stats(peer_id);
        if stats.connected_at.is_none() {
            stats.connected_at = Some(Instant::now());
        }
        self.refresh_peer_stats_snapshot();
    }

    pub(crate) fn lost_connection(&mut self, connection_id: ConnectionId) -> usize {
        if let Some(peer_id) = self.connections.remove(&connection_id) {
            self.inbound_connections.remove(&connection_id);
            if let Some(c) = self.peer_connections.get_mut(&peer_id) {
                let addr = c.remove(&connection_id);
                if c.is_empty() {
                    self.peer_connections.remove(&peer_id);
                    self.clear_peer_session_state(&peer_id, false);
                }
                if let Some(addr) = addr {
                    if let Some(ip) = addr.ip_addr() {
                        if let Some(info) = self.ip_info.get_mut(&ip) {
                            info.connections.remove(&connection_id);
                            if info.connections.is_empty() {
                                self.ip_info.remove(&ip);
                            }
                        }
                        let bucket = IpBucket::from_ip(ip);
                        if let Some(info) = self.ip_bucket_info.get_mut(&bucket) {
                            info.connections.remove(&connection_id);
                            if info.connections.is_empty() && info.request_count == 0 {
                                self.ip_bucket_info.remove(&bucket);
                            }
                        }
                    }
                }
            }
        }
        let peer_count = self.peer_connections.len();
        let _ = self.metrics.peer_count.swap(peer_count as f64);
        peer_count
    }

    pub(crate) fn peer_has_connection_at_address(
        &self,
        peer_id: &PeerId,
        address: &Multiaddr,
    ) -> bool {
        let address_without_peer_id = multiaddr_without_p2p(address);
        self.peer_connections
            .get(peer_id)
            .is_some_and(|connections| {
                connections
                    .values()
                    .any(|addr| multiaddr_without_p2p(addr) == address_without_peer_id)
            })
    }

    pub(crate) fn prune_inbound_connections(
        &mut self,
        metrics: Arc<NockchainP2PMetrics>,
        swarm: &mut Swarm<crate::behaviour::NockchainBehaviour>,
        prune_n: usize,
    ) {
        let mut inbound_connections_vec = self
            .inbound_connections
            .keys()
            .cloned()
            .collect::<Vec<ConnectionId>>();
        inbound_connections_vec.shuffle(&mut rand::rng());
        let prune_actual = std::cmp::min(prune_n, inbound_connections_vec.len());
        for connection_id in &inbound_connections_vec[0..prune_actual] {
            metrics.incoming_connections_pruned.increment();
            swarm.close_connection(*connection_id);
        }
    }

    pub(crate) fn requested(&mut self, ip: IpAddr, threshhold: u64) -> Option<u64> {
        if let Some(info) = self.ip_info.get_mut(&ip) {
            info.request_count += 1;
            if info.request_count >= threshhold {
                Some(info.request_count)
            } else {
                None
            }
        } else {
            trace!("Not tracking {ip} but it is connected. Please inform the developers.");
            None
        }
    }

    fn remove_inbound_replay_key(&mut self, peer_id: PeerId, key: RequestReplayKey) -> bool {
        let Some(cache) = self.inbound_replay_by_peer.get_mut(&peer_id) else {
            return false;
        };
        let removed = cache.entries.remove(&key).is_some();
        let cache_empty = cache.entries.is_empty();
        if cache_empty {
            self.inbound_replay_by_peer.remove(&peer_id);
        }
        if removed {
            self.inbound_replay_total_count = self.inbound_replay_total_count.saturating_sub(1);
        }
        removed
    }

    fn expire_inbound_replay_entries(&mut self, now: Instant, ttl: Duration) {
        while let Some((peer_id, key)) = self.inbound_replay_order.front().copied() {
            let Some(observed_at) = self
                .inbound_replay_by_peer
                .get(&peer_id)
                .and_then(|cache| cache.entries.get(&key))
                .copied()
            else {
                self.inbound_replay_order.pop_front();
                continue;
            };
            if now.duration_since(observed_at) < ttl {
                break;
            }
            self.inbound_replay_order.pop_front();
            self.remove_inbound_replay_key(peer_id, key);
        }
    }

    fn prune_inbound_replay_cache_for_peer(&mut self, peer_id: PeerId, max_per_peer: usize) {
        loop {
            let over_limit = self
                .inbound_replay_by_peer
                .get(&peer_id)
                .is_some_and(|cache| cache.entries.len() > max_per_peer);
            if !over_limit {
                break;
            }

            let Some(key) = self
                .inbound_replay_by_peer
                .get_mut(&peer_id)
                .and_then(|cache| cache.order.pop_front())
            else {
                break;
            };
            self.remove_inbound_replay_key(peer_id, key);
        }
    }

    fn prune_inbound_replay_total_cap(&mut self) {
        while self.inbound_replay_total_count > INBOUND_REPLAY_TOTAL_CAP {
            let Some((peer_id, key)) = self.inbound_replay_order.pop_front() else {
                self.inbound_replay_total_count = 0;
                break;
            };
            self.remove_inbound_replay_key(peer_id, key);
        }
    }

    pub(crate) fn has_inbound_replay(
        &mut self,
        peer_id: PeerId,
        key: RequestReplayKey,
        ttl: Duration,
    ) -> bool {
        if ttl.is_zero() {
            return false;
        }
        let now = Instant::now();
        self.expire_inbound_replay_entries(now, ttl);
        self.inbound_replay_by_peer
            .get(&peer_id)
            .is_some_and(|cache| cache.entries.contains_key(&key))
    }

    pub(crate) fn admit_inbound_replay_key(
        &mut self,
        peer_id: PeerId,
        key: RequestReplayKey,
        ttl: Duration,
        max_per_peer: usize,
    ) -> InboundReplayAdmission {
        if ttl.is_zero() || max_per_peer == 0 {
            return InboundReplayAdmission::Disabled;
        }

        let now = Instant::now();
        self.expire_inbound_replay_entries(now, ttl);
        if self
            .inbound_replay_by_peer
            .get(&peer_id)
            .is_some_and(|cache| cache.entries.contains_key(&key))
        {
            return InboundReplayAdmission::Replayed;
        }

        let cache = self.inbound_replay_by_peer.entry(peer_id).or_default();
        cache.entries.insert(key, now);
        cache.order.push_back(key);
        self.inbound_replay_order.push_back((peer_id, key));
        self.inbound_replay_total_count = self.inbound_replay_total_count.saturating_add(1);
        self.prune_inbound_replay_cache_for_peer(peer_id, max_per_peer);
        self.prune_inbound_replay_total_cap();
        InboundReplayAdmission::Accepted
    }

    pub(crate) fn admit_request_from_connection(
        &mut self,
        connection_id: ConnectionId,
        limit: u64,
    ) -> IpBucketAdmission {
        let Some(addr) = self.connection_address(connection_id) else {
            return IpBucketAdmission::UnknownConnection;
        };
        let Some(ip) = addr.ip_addr() else {
            return IpBucketAdmission::MissingIp;
        };
        let bucket = IpBucket::from_ip(ip);
        let info = self.ip_bucket_info.entry(bucket).or_default();
        info.request_count = info.request_count.saturating_add(1);
        if limit != 0 && info.request_count > limit {
            IpBucketAdmission::Rejected {
                bucket,
                count: info.request_count,
                limit,
            }
        } else {
            IpBucketAdmission::Admitted {
                bucket,
                count: info.request_count,
                limit,
            }
        }
    }

    pub(crate) fn admit_gossip_from_connection(
        &mut self,
        connection_id: ConnectionId,
        capacity: u32,
        refill_per_second: u32,
    ) -> GossipBucketAdmission {
        let Some(addr) = self.connection_address(connection_id) else {
            return GossipBucketAdmission::UnknownConnection;
        };
        let Some(ip) = addr.ip_addr() else {
            return GossipBucketAdmission::MissingIp;
        };
        let bucket = IpBucket::from_ip(ip);
        if capacity == 0 {
            return GossipBucketAdmission::Admitted {
                bucket,
                remaining_tokens: 0,
            };
        }

        let now = Instant::now();
        let info = self.ip_bucket_info.entry(bucket).or_default();
        let gossip = info.gossip.get_or_insert(GossipBucketState {
            tokens: capacity,
            last_refill: now,
        });
        let elapsed_secs = now.duration_since(gossip.last_refill).as_secs();
        if elapsed_secs > 0 {
            let refill = elapsed_secs.saturating_mul(u64::from(refill_per_second));
            let tokens = u64::from(gossip.tokens).saturating_add(refill);
            gossip.tokens = tokens.min(u64::from(capacity)) as u32;
            gossip.last_refill = now;
        }

        if gossip.tokens == 0 {
            GossipBucketAdmission::Rejected { bucket }
        } else {
            gossip.tokens -= 1;
            GossipBucketAdmission::Admitted {
                bucket,
                remaining_tokens: gossip.tokens,
            }
        }
    }

    pub(crate) fn ip_bucket_connection_count(
        &self,
        connection_id: ConnectionId,
    ) -> Option<(IpBucket, usize)> {
        let addr = self.connection_address(connection_id)?;
        let bucket = IpBucket::from_ip(addr.ip_addr()?);
        let count = self
            .ip_bucket_info
            .get(&bucket)
            .map(|info| info.connections.len())
            .unwrap_or(0);
        Some((bucket, count))
    }

    pub(crate) fn reset_requests(&mut self) {
        for (_ip, info) in self.ip_info.iter_mut() {
            info.request_count = 0;
        }
        for info in self.ip_bucket_info.values_mut() {
            info.request_count = 0;
        }
    }

    pub(crate) fn record_request_success(&mut self, peer_id: PeerId) {
        let now = Instant::now();
        let health = self.peer_request_health.entry(peer_id).or_default();
        health.successes = health.successes.saturating_add(1);
        health.last_success = Some(now);
    }

    pub(crate) fn record_request_failure(&mut self, peer_id: PeerId) {
        let now = Instant::now();
        let health = self.peer_request_health.entry(peer_id).or_default();
        health.failures = health.failures.saturating_add(1);
        health.last_failure = Some(now);
    }

    pub(crate) fn select_request_peers_with_preferences(
        &self,
        target_peers: Vec<PeerId>,
        limit: usize,
        exclusions: &PeerExclusions,
        preserve_peer_order: bool,
        preferred_peers: &[PeerId],
    ) -> Vec<PeerId> {
        let mut healthy = Vec::new();
        let mut fallback = Vec::new();

        for peer_id in target_peers {
            if self.peer_has_only_excluded_ips(&peer_id, exclusions) {
                self.metrics.fast_sync_peers_skipped_for_health.increment();
                continue;
            }

            fallback.push(peer_id);
            if exclusions.is_peer_request_cooled_down(&peer_id) {
                self.metrics.fast_sync_peers_skipped_for_health.increment();
                continue;
            }
            healthy.push(peer_id);
        }

        let selected_from = if healthy.is_empty() {
            fallback
        } else {
            healthy
        };
        self.order_request_peers(selected_from, limit, preserve_peer_order, preferred_peers)
    }

    fn order_request_peers(
        &self,
        mut selected_from: Vec<PeerId>,
        limit: usize,
        preserve_peer_order: bool,
        preferred_peers: &[PeerId],
    ) -> Vec<PeerId> {
        let available_peers = selected_from.iter().copied().collect::<BTreeSet<_>>();
        let mut selected = Vec::new();
        let mut selected_set = BTreeSet::new();

        for preferred_peer in preferred_peers {
            if selected.len() >= limit {
                return selected;
            }
            if available_peers.contains(preferred_peer) && selected_set.insert(*preferred_peer) {
                selected.push(*preferred_peer);
            }
        }

        selected_from.retain(|peer| !selected_set.contains(peer));
        if preserve_peer_order {
            selected_from.sort_unstable_by_key(|peer| peer.to_base58());
        } else {
            selected_from.shuffle(&mut rand::rng());
            selected_from.sort_by_key(|peer_id| Reverse(self.request_score(peer_id)));
        }
        selected.extend(
            self.bucket_fair_peer_order(selected_from)
                .into_iter()
                .take(limit.saturating_sub(selected.len())),
        );
        selected
    }

    fn bucket_fair_peer_order(&self, peers: Vec<PeerId>) -> Vec<PeerId> {
        let mut buckets: BTreeMap<Option<IpBucket>, VecDeque<PeerId>> = BTreeMap::new();
        for peer in peers {
            buckets
                .entry(self.peer_ip_bucket(&peer))
                .or_default()
                .push_back(peer);
        }

        let mut ordered = Vec::new();
        loop {
            let mut made_progress = false;
            for queue in buckets.values_mut() {
                if let Some(peer) = queue.pop_front() {
                    ordered.push(peer);
                    made_progress = true;
                }
            }
            if !made_progress {
                break;
            }
        }
        ordered
    }

    fn peer_ip_bucket(&self, peer_id: &PeerId) -> Option<IpBucket> {
        self.peer_first_address(peer_id)
            .and_then(|address| address.ip_addr())
            .map(IpBucket::from_ip)
    }

    fn request_score(&self, peer_id: &PeerId) -> i64 {
        self.peer_request_health
            .get(peer_id)
            .map(|health| health.successes as i64 * 2 - health.failures as i64)
            .unwrap_or_default()
    }

    fn peer_has_only_excluded_ips(&self, peer_id: &PeerId, exclusions: &PeerExclusions) -> bool {
        let Some(connections) = self.peer_connections.get(peer_id) else {
            return false;
        };
        let mut saw_ip = false;
        for addr in connections.values() {
            let Some(ip) = addr.ip_addr() else {
                continue;
            };
            saw_ip = true;
            if !exclusions.is_ip_excluded(&ip) {
                return false;
            }
        }
        saw_ip
    }

    pub(crate) fn ping_succeeded(&mut self, connection: ConnectionId) {
        let addr = self.connection_address(connection);
        let Some(addr) = addr else {
            trace!("No address for connection {connection}. Please inform the developers.");
            return;
        };
        let Some(ip) = addr.ip_addr() else {
            debug!("No IP address for connection {connection}.");
            return;
        };
        if let Some(info) = self.ip_info.get_mut(&ip) {
            info.ping_failure_count = 0;
        }
    }

    pub(crate) fn ping_failed(&mut self, connection: ConnectionId) -> u64 {
        let addr = self.connection_address(connection);
        let Some(addr) = addr else {
            trace!("No address for connection {connection}. Please inform the developers.");
            return 0;
        };
        let Some(ip) = addr.ip_addr() else {
            debug!("No IP address for connection {connection}.");
            return 0;
        };
        if let Some(info) = self.ip_info.get_mut(&ip) {
            info.ping_failure_count += 1;
            info.ping_failure_count
        } else {
            0
        }
    }

    pub(crate) fn connection_address(&self, connection_id: ConnectionId) -> Option<Multiaddr> {
        self.connections.get(&connection_id).and_then(|peer_id| {
            self.peer_connections
                .get(peer_id)
                .and_then(|map| map.get(&connection_id))
                .cloned()
        })
    }

    pub(crate) fn peer_first_address(&self, peer_id: &PeerId) -> Option<Multiaddr> {
        self.peer_connections
            .get(peer_id)
            .and_then(|connections| connections.values().next())
            .cloned()
    }

    pub(crate) fn track_block_id_str_and_peer(&mut self, block_id_str: String, peer_id: PeerId) {
        self.block_id_to_peers
            .entry(block_id_str.clone())
            .or_default()
            .insert(peer_id);

        self.peer_to_block_ids
            .entry(peer_id)
            .or_default()
            .insert(block_id_str);
    }

    pub(crate) fn track_accepted_block_id_str_and_peer(
        &mut self,
        block_id_str: String,
        peer_id: PeerId,
    ) -> Option<u64> {
        self.track_block_id_str_and_peer(block_id_str.clone(), peer_id);
        let height = self.remove_observed_block_height_candidate(&block_id_str)?;
        self.note_peer_observed_height(height);
        Some(height)
    }

    pub(crate) fn record_observed_block_height_candidate(
        &mut self,
        block_id_str: String,
        height: u64,
    ) {
        if !self
            .observed_block_height_candidates
            .contains_key(&block_id_str)
        {
            self.observed_block_height_candidate_order
                .push_back(block_id_str.clone());
        }
        self.observed_block_height_candidates
            .insert(block_id_str, height);
        self.evict_observed_block_height_candidates();
    }

    pub(crate) fn remove_observed_block_height_candidate(
        &mut self,
        block_id_str: &str,
    ) -> Option<u64> {
        let height = self.observed_block_height_candidates.remove(block_id_str)?;
        self.observed_block_height_candidate_order
            .retain(|candidate| candidate != block_id_str);
        Some(height)
    }

    fn evict_observed_block_height_candidates(&mut self) {
        while self.observed_block_height_candidates.len() > OBSERVED_BLOCK_HEIGHT_CANDIDATE_CAP {
            let Some(oldest_block_id) = self.observed_block_height_candidate_order.pop_front()
            else {
                break;
            };
            self.observed_block_height_candidates
                .remove(&oldest_block_id);
        }
    }

    fn track_block_height_attempt_peer(&mut self, height: u64, peer_id: PeerId) {
        if !self.block_height_attempted_peers.contains_key(&height) {
            self.block_height_attempt_order.push_back(height);
        }

        self.block_height_attempted_peers
            .entry(height)
            .or_default()
            .insert(peer_id);

        self.peer_to_block_height_attempts
            .entry(peer_id)
            .or_default()
            .insert(height);

        self.evict_block_height_attempts();
    }

    pub(crate) fn remove_block_id_str(&mut self, block_id: &str) {
        self.remove_observed_block_height_candidate(block_id);
        let Some(peers) = self.block_id_to_peers.remove(block_id) else {
            return;
        };

        for peer_id in peers {
            let Some(block_ids) = self.peer_to_block_ids.get_mut(&peer_id) else {
                continue;
            };

            block_ids.remove(block_id);
            if block_ids.is_empty() {
                self.peer_to_block_ids.remove(&peer_id);
            }
        }
    }

    fn remove_block_height_attempt(&mut self, height: u64) {
        let Some(peers) = self.block_height_attempted_peers.remove(&height) else {
            return;
        };

        for peer_id in peers {
            let Some(heights) = self.peer_to_block_height_attempts.get_mut(&peer_id) else {
                continue;
            };

            heights.remove(&height);
            if heights.is_empty() {
                self.peer_to_block_height_attempts.remove(&peer_id);
            }
        }
    }

    fn track_tx_id_str_and_peer(&mut self, tx_id_str: String, peer_id: PeerId) {
        if !self.tx_id_to_peers.contains_key(&tx_id_str) {
            self.tx_source_hint_order.push_back(tx_id_str.clone());
        }

        self.tx_id_to_peers
            .entry(tx_id_str.clone())
            .or_default()
            .insert(peer_id);

        self.peer_to_tx_ids
            .entry(peer_id)
            .or_default()
            .insert(tx_id_str);

        self.evict_tx_source_hints();
    }

    fn remove_tx_id_str(&mut self, tx_id: &str) {
        let Some(peers) = self.tx_id_to_peers.remove(tx_id) else {
            return;
        };

        for peer_id in peers {
            let Some(tx_ids) = self.peer_to_tx_ids.get_mut(&peer_id) else {
                continue;
            };

            tx_ids.remove(tx_id);
            if tx_ids.is_empty() {
                self.peer_to_tx_ids.remove(&peer_id);
            }
        }
    }

    fn evict_tx_source_hints(&mut self) {
        while self.tx_id_to_peers.len() > TX_SOURCE_HINT_CAP {
            let Some(oldest_tx_id) = self.tx_source_hint_order.pop_front() else {
                break;
            };
            self.remove_tx_id_str(&oldest_tx_id);
        }
    }

    fn evict_block_height_attempts(&mut self) {
        while self.block_height_attempted_peers.len() > BLOCK_HEIGHT_ATTEMPT_CAP {
            let Some(oldest_height) = self.block_height_attempt_order.pop_front() else {
                break;
            };
            self.remove_block_height_attempt(oldest_height);
        }
    }

    fn evict_kernel_block_height_requests(&mut self) {
        while self.kernel_requested_block_heights.len() > KERNEL_BLOCK_HEIGHT_REQUEST_CAP {
            let Some(oldest_height) = self.kernel_block_height_request_order.pop_front() else {
                break;
            };
            self.kernel_requested_block_heights.remove(&oldest_height);
        }
    }

    fn forget_kernel_block_height_request(&mut self, height: u64) {
        if self.kernel_requested_block_heights.remove(&height) {
            self.kernel_block_height_request_order
                .retain(|requested_height| *requested_height != height);
        }
    }

    fn clear_peer_session_state(&mut self, peer_id: &PeerId, clear_replay_cache: bool) {
        self.clear_outbound_requests_for_peer(peer_id);
        self.inbound_req_res_inflight.remove(peer_id);
        self.peer_stats.remove(peer_id);
        self.peer_request_health.remove(peer_id);
        self.non_bundle_capable_peers.remove(peer_id);
        self.non_range_capable_peers.remove(peer_id);
        self.prefetch_peer_range_stats.remove(peer_id);
        self.prefetch_bandwidth_window.remove(peer_id);
        if clear_replay_cache {
            self.clear_inbound_replay_cache_for_peer(peer_id);
        }
        self.update_req_res_inflight_metrics();
        if let Some(block_ids) = self.peer_to_block_ids.remove(peer_id) {
            for block_id in block_ids {
                let Some(peers) = self.block_id_to_peers.get_mut(&block_id) else {
                    continue;
                };

                peers.remove(peer_id);
                if peers.is_empty() {
                    self.block_id_to_peers.remove(&block_id);
                }
            }
        }

        if let Some(tx_ids) = self.peer_to_tx_ids.remove(peer_id) {
            for tx_id in tx_ids {
                let Some(peers) = self.tx_id_to_peers.get_mut(&tx_id) else {
                    continue;
                };

                peers.remove(peer_id);
                if peers.is_empty() {
                    self.tx_id_to_peers.remove(&tx_id);
                }
            }
        }

        if let Some(heights) = self.peer_to_block_height_attempts.remove(peer_id) {
            for height in heights {
                let Some(peers) = self.block_height_attempted_peers.get_mut(&height) else {
                    continue;
                };

                peers.remove(peer_id);
                if peers.is_empty() {
                    self.block_height_attempted_peers.remove(&height);
                }
            }
        }
        self.refresh_peer_stats_snapshot();
    }

    fn clear_inbound_replay_cache_for_peer(&mut self, peer_id: &PeerId) {
        if let Some(cache) = self.inbound_replay_by_peer.remove(peer_id) {
            self.inbound_replay_total_count = self
                .inbound_replay_total_count
                .saturating_sub(cache.entries.len());
        }
    }

    /// Removes a peer from the tracker after a ban or an explicit cleanup.
    pub fn remove_peer(&mut self, peer_id: &PeerId) {
        info!("Removing peer: {}", peer_id);
        self.clear_peer_session_state(peer_id, true);
    }

    /// Adds a block ID and peer to the tracker.
    /// implements [%track %add block-id peer-id] effect
    #[cfg(test)]
    pub fn track_block_id_and_peer(
        &mut self,
        block_id: Noun,
        peer_id: PeerId,
        space: &NounSpace,
    ) -> Result<(), NockAppError> {
        let block_id_str = tip5_hash_to_base58(block_id, space)?;
        self.track_block_id_str_and_peer(block_id_str, peer_id);
        Ok(())
    }

    /// Adds a peer to an existing block ID. Returns true if the block ID exists and the peer was added,
    /// false if the block ID doesn't exist in the tracker.
    #[allow(dead_code)]
    pub fn add_peer_if_tracking_block_id(
        &mut self,
        block_id: Noun,
        peer_id: PeerId,
        space: &NounSpace,
    ) -> Result<bool, NockAppError> {
        let block_id_str = tip5_hash_to_base58(block_id, space)?;

        if self.block_id_to_peers.contains_key(&block_id_str) {
            self.track_block_id_str_and_peer(block_id_str, peer_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Removes a block ID from the tracker.
    /// implements [%track %remove block-id] effect
    #[cfg(test)]
    pub fn remove_block_id(
        &mut self,
        block_id: Noun,
        space: &NounSpace,
    ) -> Result<(), NockAppError> {
        let block_id_str = tip5_hash_to_base58(block_id, space)?;
        self.remove_block_id_str(&block_id_str);
        Ok(())
    }

    /// Returns a list of peers that have sent us a given block ID.
    #[allow(dead_code)]
    pub fn get_peers_for_block_id(&self, block_id: Noun, space: &NounSpace) -> Vec<PeerId> {
        let Ok(block_id_str) = tip5_hash_to_base58(block_id, space) else {
            panic!("Invalid block ID");
        };
        self.block_id_to_peers
            .get(&block_id_str)
            .map(|peers| peers.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    }

    /// Returns a list of block IDs that a given peer has sent us.
    #[allow(dead_code)]
    pub fn get_block_ids_for_peer(&self, peer_id: PeerId) -> Vec<String> {
        self.peer_to_block_ids
            .get(&peer_id)
            .map(|block_ids| block_ids.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    }

    pub fn track_tx_ids_and_peer<I>(&mut self, tx_ids: I, peer_id: PeerId)
    where
        I: IntoIterator<Item = String>,
    {
        for tx_id in tx_ids {
            self.track_tx_id_str_and_peer(tx_id, peer_id);
        }
    }

    pub fn remove_tx_id_hint(&mut self, tx_id: &str) {
        self.remove_tx_id_str(tx_id);
    }

    fn evict_expired_speculative_tx_prefetches(&mut self, now: Instant) {
        self.speculative_tx_prefetches
            .retain(|_, expires_at| *expires_at > now);
    }

    pub fn claim_speculative_tx_prefetch_ids<I>(
        &mut self,
        tx_ids: I,
        ttl: Duration,
        max_claims: usize,
    ) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        if max_claims == 0 {
            return Vec::new();
        }

        let now = Instant::now();
        self.evict_expired_speculative_tx_prefetches(now);
        let expires_at = now + ttl;
        let mut claimed = Vec::new();
        for tx_id in tx_ids {
            if claimed.len() >= max_claims
                || self.speculative_tx_prefetches.len() >= SPECULATIVE_TX_PREFETCH_CAP
            {
                break;
            }
            if self.seen_txs.contains(&tx_id) || self.speculative_tx_prefetches.contains_key(&tx_id)
            {
                continue;
            }
            self.speculative_tx_prefetches
                .insert(tx_id.clone(), expires_at);
            claimed.push(tx_id);
        }
        claimed
    }

    pub fn clear_speculative_tx_prefetch(&mut self, tx_id: &str) {
        self.speculative_tx_prefetches.remove(tx_id);
    }

    /// Mark a peer as non-bundle-capable so future `%block %by-height`
    /// requests to this peer skip the `%block-with-txs` upgrade. Called by
    /// the batch-result handler when a bundle request item fails in a way
    /// that makes the classic block-only request shape the next viable path.
    pub fn mark_peer_non_bundle_capable(&mut self, peer_id: PeerId) {
        self.non_bundle_capable_peers.insert(peer_id);
    }

    #[allow(dead_code)]
    pub fn is_peer_non_bundle_capable(&self, peer_id: &PeerId) -> bool {
        self.non_bundle_capable_peers.contains(peer_id)
    }

    pub fn non_bundle_capable_peers_snapshot(&self) -> BTreeSet<PeerId> {
        self.non_bundle_capable_peers.clone()
    }

    fn ensure_prefetch_peer_range_stats(&mut self, peer_id: PeerId) -> &mut PrefetchPeerRangeStats {
        self.prefetch_peer_range_stats.entry(peer_id).or_default()
    }

    fn update_range_ewma(value: &mut Option<f64>, sample: f64) {
        *value = Some(match *value {
            Some(current) => current.mul_add(
                1.0 - PREFETCH_PEER_EWMA_ALPHA,
                sample * PREFETCH_PEER_EWMA_ALPHA,
            ),
            None => sample,
        });
    }

    pub fn mark_peer_range_supported(&mut self, peer_id: PeerId) {
        let changed = self.peer_range_capability(&peer_id) != RangeCapability::Supported;
        {
            let stats = self.ensure_prefetch_peer_range_stats(peer_id);
            stats.capability = RangeCapability::Supported;
            stats.cooldown_until = None;
        }
        self.non_range_capable_peers.remove(&peer_id);
        if changed {
            self.metrics
                .prefetch_peer_capability_supported_total
                .increment();
        }
    }

    /// Mark a peer as non-range-capable so the catch-up prefetch path
    /// skips the `block-with-txs by-range` shape and falls back to
    /// per-height singleton requests for that peer. Called when a range
    /// request item returns a decode error.
    #[allow(dead_code)]
    pub fn mark_peer_non_range_capable(&mut self, peer_id: PeerId) {
        let changed = self.peer_range_capability(&peer_id) != RangeCapability::Unsupported;
        self.ensure_prefetch_peer_range_stats(peer_id).capability = RangeCapability::Unsupported;
        self.non_range_capable_peers.insert(peer_id);
        if changed {
            self.metrics
                .prefetch_peer_capability_unsupported_total
                .increment();
        }
    }

    #[allow(dead_code)]
    pub fn is_peer_non_range_capable(&self, peer_id: &PeerId) -> bool {
        self.non_range_capable_peers.contains(peer_id)
    }

    pub fn peer_req_res_generation(&self, peer_id: &PeerId) -> PeerReqResGeneration {
        self.peer_stats
            .get(peer_id)
            .map(|stats| stats.generation)
            .unwrap_or_default()
    }

    pub fn peer_range_capability(&self, peer_id: &PeerId) -> RangeCapability {
        if self.non_range_capable_peers.contains(peer_id) {
            return RangeCapability::Unsupported;
        }
        self.prefetch_peer_range_stats
            .get(peer_id)
            .map(|stats| stats.capability)
            .unwrap_or_default()
    }

    pub fn is_peer_prefetch_cooldown_active(&self, peer_id: &PeerId) -> bool {
        self.prefetch_peer_range_stats
            .get(peer_id)
            .and_then(|stats| stats.cooldown_until)
            .is_some_and(|deadline| deadline > Instant::now())
    }

    pub fn prefetch_peer_selection_weight(&self, peer_id: &PeerId) -> f64 {
        let Some(stats) = self.prefetch_peer_range_stats.get(peer_id) else {
            return 1.0;
        };
        let attempts = stats.success_count.saturating_add(stats.failure_count);
        let success_probability =
            (stats.success_count.saturating_add(2) as f64) / (attempts.saturating_add(4) as f64);
        let rtt_weight = stats
            .rtt_ms_ewma
            .map(|ms| (500.0 / ms.max(1.0)).clamp(0.25, 4.0))
            .unwrap_or(1.0);
        let payload_weight = stats
            .response_bytes_ewma
            .map(|bytes| (bytes.max(1.0).log2() / 20.0).clamp(0.5, 2.0))
            .unwrap_or(1.0);
        let inflight_penalty = (1.0 + f64::from(self.inflight_prefetch_count_for_peer(peer_id)))
            .powf(PREFETCH_PEER_ACTIVE_REQUEST_BIAS);
        (success_probability * rtt_weight * payload_weight / inflight_penalty).max(0.01)
    }

    #[allow(dead_code)]
    pub fn non_range_capable_peers_snapshot(&self) -> BTreeSet<PeerId> {
        self.non_range_capable_peers.clone()
    }

    /// Phase 4: register a freshly-dispatched catch-up prefetch. The
    /// covered heights `[start_height, start_height + len)` go into
    /// `prefetched_heights` so subsequent `BlockByHeight` requests for
    /// those heights are suppressed and ride the prefetch response. The
    /// per-peer count gates further prefetch issuance against
    /// `prefetch_max_inflight_per_peer`.
    pub fn register_prefetch(
        &mut self,
        request_id: OutboundRequestId,
        peer_id: PeerId,
        start_height: u64,
        len: u8,
    ) {
        if len == 0 {
            return;
        }
        let prefetch = InflightPrefetch {
            peer_id,
            start_height,
            len,
            started_at: Instant::now(),
        };
        // Replace any prior entry under the same request_id (defensive; the
        // outbound layer should never reuse ids).
        if let Some(prior) = self.inflight_prefetches.insert(request_id, prefetch) {
            self.release_prefetch_coverage(&prior);
        }
        for offset in 0..u64::from(len) {
            self.prefetched_heights.insert(start_height + offset);
        }
        let counter = self
            .inflight_prefetch_count_per_peer
            .entry(peer_id)
            .or_insert(0);
        *counter = counter.saturating_add(1);
    }

    /// Phase 4: clear a prefetch entry on response or failure. Returns the
    /// removed entry so callers can attribute timing or restore state.
    pub fn clear_prefetch(&mut self, request_id: OutboundRequestId) -> Option<InflightPrefetch> {
        let prefetch = self.inflight_prefetches.remove(&request_id)?;
        self.release_prefetch_coverage(&prefetch);
        Some(prefetch)
    }

    fn release_prefetch_coverage(&mut self, prefetch: &InflightPrefetch) {
        for offset in 0..u64::from(prefetch.len) {
            // Only remove the height if no other inflight prefetch still
            // covers it. Defensive scan; in steady state with one prefetch
            // per peer the cost is bounded by `prefetch_max_inflight_per_peer
            // * peer_count`.
            let still_covered = self
                .inflight_prefetches
                .values()
                .any(|other| height_in_prefetch(prefetch.start_height + offset, other));
            if !still_covered {
                self.prefetched_heights
                    .remove(&(prefetch.start_height + offset));
            }
        }
        if let Some(counter) = self
            .inflight_prefetch_count_per_peer
            .get_mut(&prefetch.peer_id)
        {
            *counter = counter.saturating_sub(1);
            if *counter == 0 {
                self.inflight_prefetch_count_per_peer
                    .remove(&prefetch.peer_id);
            }
        }
    }

    /// Phase 4: returns true iff some currently-inflight prefetch covers
    /// `height`. Used by the kernel-singleton suppression path.
    pub fn is_prefetch_inflight_covering_height(&self, height: u64) -> bool {
        self.prefetched_heights.contains(&height)
    }

    /// Phase 4: count of currently-inflight prefetches to `peer_id`.
    pub fn inflight_prefetch_count_for_peer(&self, peer_id: &PeerId) -> u8 {
        self.inflight_prefetch_count_per_peer
            .get(peer_id)
            .copied()
            .unwrap_or(0)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn inflight_prefetch_total(&self) -> usize {
        self.inflight_prefetches.len()
    }

    /// Phase 5: configure the per-height failure budget, the stuck-height
    /// backoff window, and the per-peer prefetch bandwidth cap. Called
    /// once at driver startup from `LibP2PConfig`. Defaults are the
    /// `PREFETCH_*` constants if never configured.
    pub fn set_prefetch_safety_config(
        &mut self,
        budget: u8,
        backoff: Duration,
        bandwidth_cap_bytes_per_min: usize,
    ) {
        self.prefetch_height_failure_budget = budget;
        self.prefetch_stuck_backoff = backoff;
        self.prefetch_bandwidth_cap_per_peer_bytes_per_min = bandwidth_cap_bytes_per_min;
    }

    pub fn set_catch_up_config(
        &mut self,
        behind_tip_threshold: u64,
        peer_observed_threshold: u64,
        hysteresis: Duration,
    ) {
        self.catch_up
            .configure(behind_tip_threshold, peer_observed_threshold, hysteresis);
        self.publish_catch_up_metrics();
    }

    pub fn prefetch_height_failure_budget(&self) -> u8 {
        self.prefetch_height_failure_budget
    }

    pub fn prefetch_stuck_backoff(&self) -> Duration {
        self.prefetch_stuck_backoff
    }

    pub fn prefetch_bandwidth_cap_per_peer_bytes_per_min(&self) -> usize {
        self.prefetch_bandwidth_cap_per_peer_bytes_per_min
    }

    /// Phase 5: record a fetch failure at `height`. Returns the new
    /// failure count. The caller compares against the configured
    /// `prefetch_height_failure_budget`; once we hit it, mark the height
    /// stuck for `backoff` time so retries back off rather than amplify.
    pub fn record_block_height_failure(
        &mut self,
        height: u64,
        budget: u8,
        backoff: Duration,
    ) -> u8 {
        let now = Instant::now();
        let entry = self
            .block_height_failures
            .entry(height)
            .or_insert(HeightFailureRecord {
                failures: 0,
                stuck_until: None,
                last_observed: now,
            });
        entry.failures = entry.failures.saturating_add(1);
        entry.last_observed = now;
        if entry.failures >= budget {
            entry.stuck_until = Some(now + backoff);
        }
        entry.failures
    }

    /// Phase 5: clear the per-height failure record after a successful
    /// fetch. Lets the height re-enter the eligible set for future
    /// re-requests (e.g. after a reorg).
    pub fn clear_block_height_failure(&mut self, height: u64) {
        self.block_height_failures.remove(&height);
    }

    /// Phase 5: returns true if the per-height retry budget has been
    /// exhausted and the backoff window has not yet elapsed. The
    /// alternate-peer retry path skips stuck heights so the kernel timer
    /// arm, not the driver, drives the next attempt.
    pub fn is_block_height_stuck(&self, height: u64) -> bool {
        let Some(record) = self.block_height_failures.get(&height) else {
            return false;
        };
        record
            .stuck_until
            .is_some_and(|deadline| deadline > Instant::now())
    }

    /// Phase 5: snapshot of currently-stuck heights for observability and
    /// the watchdog stack dump.
    #[allow(dead_code)]
    pub fn stuck_block_heights(&self) -> Vec<u64> {
        let now = Instant::now();
        self.block_height_failures
            .iter()
            .filter_map(|(height, record)| {
                record
                    .stuck_until
                    .filter(|deadline| *deadline > now)
                    .map(|_| *height)
            })
            .collect()
    }

    /// Phase 5: record `bytes` of prefetch response from `peer_id` and
    /// return the rolling 60s total *after* this insertion. Used to
    /// enforce `prefetch_bandwidth_cap_per_peer_bytes_per_min`.
    pub fn record_prefetch_bandwidth(&mut self, peer_id: PeerId, bytes: usize) -> usize {
        let now = Instant::now();
        let cutoff = now.checked_sub(PREFETCH_BANDWIDTH_WINDOW);
        let entry = self.prefetch_bandwidth_window.entry(peer_id).or_default();
        if let Some(cutoff) = cutoff {
            while let Some((stamp, _)) = entry.front() {
                if *stamp < cutoff {
                    entry.pop_front();
                } else {
                    break;
                }
            }
        }
        entry.push_back((now, bytes));
        entry.iter().map(|(_, b)| *b).sum()
    }

    /// Phase 5: 60s prefetch byte total for `peer_id`. Read-only check
    /// used before issuing a prefetch so we can throttle abusive peers
    /// without first recording another burst.
    pub fn prefetch_bandwidth_window_bytes(&self, peer_id: &PeerId) -> usize {
        let Some(entry) = self.prefetch_bandwidth_window.get(peer_id) else {
            return 0;
        };
        let cutoff = Instant::now().checked_sub(PREFETCH_BANDWIDTH_WINDOW);
        entry
            .iter()
            .filter(|(stamp, _)| cutoff.is_none_or(|cutoff| *stamp >= cutoff))
            .map(|(_, b)| *b)
            .sum()
    }

    fn record_prefetch_range_success(
        &mut self,
        peer_id: PeerId,
        response_bytes: usize,
        round_trip: Duration,
    ) {
        self.mark_peer_range_supported(peer_id);
        let stats = self.ensure_prefetch_peer_range_stats(peer_id);
        stats.success_count = stats.success_count.saturating_add(1);
        stats.cooldown_until = None;
        Self::update_range_ewma(&mut stats.response_bytes_ewma, response_bytes as f64);
        Self::update_range_ewma(&mut stats.rtt_ms_ewma, round_trip.as_secs_f64() * 1_000.0);
    }

    fn record_prefetch_range_failure(
        &mut self,
        peer_id: PeerId,
        timeout: bool,
        failure_count: u64,
    ) {
        let bounded_failures = failure_count.max(1);
        let stats = self.ensure_prefetch_peer_range_stats(peer_id);
        stats.failure_count = stats.failure_count.saturating_add(bounded_failures);
        if timeout {
            stats.timeout_count = stats.timeout_count.saturating_add(bounded_failures);
        }
        if stats.capability != RangeCapability::Unsupported {
            let exponent = stats.failure_count.saturating_sub(1).min(5);
            let multiplier = 1u32.checked_shl(exponent as u32).unwrap_or(u32::MAX);
            let cooldown = PREFETCH_PEER_COOLDOWN_INITIAL
                .saturating_mul(multiplier)
                .min(PREFETCH_PEER_COOLDOWN_MAX);
            stats.cooldown_until = Some(Instant::now() + cooldown);
            self.metrics.prefetch_peer_cooldown_total.increment();
        }
    }

    fn increment_deferred_count(&mut self, peer_id: PeerId) {
        self.deferred_heard_block_total_count =
            self.deferred_heard_block_total_count.saturating_add(1);
        *self
            .deferred_heard_block_count_by_peer
            .entry(peer_id)
            .or_default() += 1;
    }

    fn decrement_deferred_count(&mut self, peer_id: PeerId) {
        self.deferred_heard_block_total_count =
            self.deferred_heard_block_total_count.saturating_sub(1);
        if let Some(count) = self.deferred_heard_block_count_by_peer.get_mut(&peer_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.deferred_heard_block_count_by_peer.remove(&peer_id);
            }
        }
    }

    fn remove_deferred_entry(&mut self, height: u64, block_id: &str) -> Option<DeferredHeardBlock> {
        let removed = {
            let blocks = self.deferred_heard_blocks.get_mut(&height)?;
            blocks.remove(block_id)
        }?;
        if self
            .deferred_heard_blocks
            .get(&height)
            .is_some_and(BTreeMap::is_empty)
        {
            self.deferred_heard_blocks.remove(&height);
        }
        self.decrement_deferred_count(removed.peer_id);
        Some(removed)
    }

    fn evict_deferred_total_if_needed(&mut self) {
        while self.deferred_heard_block_total_count >= DEFERRED_HEARD_BLOCK_TOTAL_CAP {
            if !self.evict_oldest_deferred_entry(None) {
                break;
            }
        }
    }

    fn evict_deferred_for_peer_if_needed(&mut self, peer_id: PeerId) {
        while self
            .deferred_heard_block_count_by_peer
            .get(&peer_id)
            .copied()
            .unwrap_or_default()
            >= DEFERRED_HEARD_BLOCK_PER_PEER_CAP
        {
            if !self.evict_oldest_deferred_entry(Some(peer_id)) {
                break;
            }
        }
    }

    fn evict_oldest_deferred_entry(&mut self, peer_filter: Option<PeerId>) -> bool {
        let candidate = self
            .deferred_heard_blocks
            .iter()
            .find_map(|(height, blocks)| {
                blocks.iter().find_map(|(block_id, block)| {
                    if peer_filter.is_some_and(|peer_id| peer_id != block.peer_id) {
                        return None;
                    }
                    Some((*height, block_id.clone()))
                })
            });
        let Some((height, block_id)) = candidate else {
            return false;
        };
        self.remove_deferred_entry(height, &block_id).is_some()
    }

    /// Phase 5: drop any deferred-buffer entries for `block_id` and the
    /// matching prefetch. Called from the LiarBlockId arm when a forked or
    /// liar-served height needs to free the buffer for a fresh fetch from a
    /// different peer. Returns `true` if any entry was removed.
    pub fn invalidate_deferred_block_id(&mut self, block_id: &str) -> bool {
        let mut removed_any = false;
        let heights = self
            .deferred_heard_blocks
            .iter()
            .filter_map(|(height, blocks)| blocks.contains_key(block_id).then_some(*height))
            .collect::<Vec<_>>();
        for height in heights {
            if self.remove_deferred_entry(height, block_id).is_some() {
                removed_any = true;
            }
        }
        if removed_any {
            self.note_deferred_changed();
        }
        removed_any
    }

    pub fn get_peers_for_tx_id(&self, tx_id: &str) -> Vec<PeerId> {
        self.tx_id_to_peers
            .get(tx_id)
            .map(|peers| peers.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    }

    pub fn track_block_height_attempted_peers<I>(&mut self, height: u64, peers: I)
    where
        I: IntoIterator<Item = PeerId>,
    {
        for peer_id in peers {
            self.track_block_height_attempt_peer(height, peer_id);
        }
    }

    pub fn clear_block_height_attempted_peers(&mut self, height: u64) {
        self.remove_block_height_attempt(height);
    }

    pub fn get_block_height_attempted_peers(&self, height: u64) -> Vec<PeerId> {
        self.block_height_attempted_peers
            .get(&height)
            .map(|peers| peers.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default()
    }

    /// Record that the kernel explicitly asked for `height`.
    ///
    /// Prefetch range responses can arrive before the kernel is ready to
    /// validate every height in the window. This ledger keeps those range
    /// fills demand-driven: a prefetched deferred block is flushable only
    /// after the matching `%request %block %by-height` has been observed.
    pub fn note_kernel_block_height_requested(&mut self, height: u64) {
        if self.kernel_requested_block_heights.insert(height) {
            self.kernel_block_height_request_order.push_back(height);
            self.evict_kernel_block_height_requests();
        }
    }

    pub fn has_kernel_block_height_request(&self, height: u64) -> bool {
        self.kernel_requested_block_heights.contains(&height)
    }

    pub fn try_start_processing_block(&mut self, block_id: &str) -> bool {
        self.try_start_processing_block_with_seen_replay(block_id, false)
    }

    pub fn try_start_processing_block_with_seen_replay(
        &mut self,
        block_id: &str,
        allow_seen_replay: bool,
    ) -> bool {
        if self.processing_blocks.contains(block_id) {
            return false;
        }
        if self.seen_blocks.contains(block_id) && !allow_seen_replay {
            return false;
        }
        self.processing_blocks.insert(block_id.to_owned());
        true
    }

    pub fn try_start_processing_tx(&mut self, tx_id: &str) -> bool {
        if self.seen_txs.contains(tx_id) || self.processing_txs.contains(tx_id) {
            return false;
        }
        self.processing_txs.insert(tx_id.to_owned());
        true
    }

    pub fn finish_processing_block_seen(&mut self, block_id: &str) {
        self.processing_blocks.remove(block_id);
        if self.seen_blocks.insert(block_id.to_owned()) {
            self.seen_block_order.push_back(block_id.to_owned());
            self.evict_seen_blocks();
        }
    }

    pub fn finish_processing_tx_seen(&mut self, tx_id: &str) {
        self.processing_txs.remove(tx_id);
        self.seen_txs.insert(tx_id.to_owned());
    }

    pub fn cancel_processing_block(&mut self, block_id: &str) {
        self.processing_blocks.remove(block_id);
    }

    pub fn cancel_processing_tx(&mut self, tx_id: &str) {
        self.processing_txs.remove(tx_id);
    }

    fn evict_seen_blocks(&mut self) {
        while self.seen_blocks.len() > SEEN_BLOCKS_CAP {
            let Some(block_id) = self.seen_block_order.pop_front() else {
                break;
            };
            self.seen_blocks.remove(&block_id);
        }
    }

    fn evict_block_receipts(&mut self) {
        while self.block_receipts.len() > BLOCK_RECEIPT_CAP {
            let Some(block_id) = self.block_receipt_order.pop_front() else {
                break;
            };
            self.block_receipts.remove(&block_id);
            self.block_first_seen_at.remove(&block_id);
        }
    }

    pub fn record_elders_negative_cache(&mut self, id: String) {
        if self.elders_negative_cache.insert(id.clone()) {
            self.elders_negative_cache_order.push_back(id);
            self.evict_elders_negative_cache();
        }
    }

    pub fn clear_elders_negative_cache(&mut self) {
        self.elders_negative_cache.clear();
        self.elders_negative_cache_order.clear();
    }

    fn evict_elders_negative_cache(&mut self) {
        while self.elders_negative_cache.len() > ELDERS_NEGATIVE_CACHE_CAP {
            let Some(id) = self.elders_negative_cache_order.pop_front() else {
                break;
            };
            self.elders_negative_cache.remove(&id);
        }
    }

    pub fn defer_heard_block(
        &mut self,
        peer_id: PeerId,
        height: u64,
        block_id: String,
        fact: NockchainFact,
    ) -> bool {
        self.defer_heard_block_with_source(peer_id, height, block_id, fact, BlockSource::Gossip)
    }

    /// Insert a heard-block into the deferred buffer with explicit
    /// provenance. Phase 4's prefetch path uses
    /// `BlockSource::Prefetch` so we can distinguish prefetch-driven
    /// fills from gossip arrivals for invalidation and metrics. The
    /// existing gossip path keeps using `defer_heard_block`, which
    /// defaults to `BlockSource::Gossip`.
    #[allow(dead_code)]
    pub fn defer_heard_block_with_source(
        &mut self,
        peer_id: PeerId,
        height: u64,
        block_id: String,
        fact: NockchainFact,
        source: BlockSource,
    ) -> bool {
        if self
            .deferred_heard_blocks
            .get(&height)
            .is_some_and(|deferred| deferred.contains_key(&block_id))
        {
            return false;
        }
        self.evict_deferred_for_peer_if_needed(peer_id);
        self.evict_deferred_total_if_needed();
        let deferred = self.deferred_heard_blocks.entry(height).or_default();
        deferred.insert(
            block_id,
            DeferredHeardBlock {
                peer_id,
                fact,
                source,
            },
        );
        self.increment_deferred_count(peer_id);
        self.note_deferred_changed();
        true
    }

    /// Returns true if at least one block at `height` is currently held in
    /// the deferred buffer. Used by `handle_effect_with_dispatcher` to
    /// suppress redundant outbound block-by-height requests when the block
    /// is already locally buffered (Phase 2 of catch-up prefetch). The
    /// buffered block is delivered to the kernel by the existing
    /// `FlushDeferredHeardBlocks` path after the next `%seen %block`
    /// effect advances `first_negative`.
    pub fn has_deferred_block_at_height(&self, height: u64) -> bool {
        self.deferred_heard_blocks
            .get(&height)
            .is_some_and(|blocks| !blocks.is_empty())
    }

    fn deferred_block_is_flushable(&self, height: u64, block: &DeferredHeardBlock) -> bool {
        if height > self.first_negative {
            return false;
        }
        match block.source {
            BlockSource::Gossip => true,
            BlockSource::Prefetch => self.kernel_requested_block_heights.contains(&height),
        }
    }

    /// Returns true when `height` is buffered and no longer ahead of the
    /// driver's seen frontier. A request for this height can be answered by
    /// queueing the deferred flush path immediately. Prefetched entries also
    /// require explicit kernel demand for the same height.
    pub fn has_ready_deferred_block_at_height(&self, height: u64) -> bool {
        self.deferred_heard_blocks
            .get(&height)
            .is_some_and(|blocks| {
                blocks
                    .values()
                    .any(|block| self.deferred_block_is_flushable(height, block))
            })
    }

    /// Total number of heard-blocks currently held in the deferred
    /// buffer, summed across heights. Used as a gauge for observability.
    pub fn deferred_heard_block_total(&self) -> usize {
        self.deferred_heard_block_total_count
    }

    pub fn has_ready_deferred_heard_blocks(&self) -> bool {
        self.deferred_heard_blocks
            .iter()
            .take_while(|(height, _)| **height <= self.first_negative)
            .any(|(height, blocks)| {
                blocks
                    .values()
                    .any(|block| self.deferred_block_is_flushable(*height, block))
            })
    }

    pub fn take_ready_deferred_heard_blocks(&mut self) -> Vec<(PeerId, NockchainFact)> {
        let ready_heights = self
            .deferred_heard_blocks
            .keys()
            .copied()
            .take_while(|height| *height <= self.first_negative)
            .collect::<Vec<_>>();
        let mut ready = Vec::new();
        for height in ready_heights {
            let Some(blocks) = self.deferred_heard_blocks.remove(&height) else {
                continue;
            };
            let requested = self.kernel_requested_block_heights.contains(&height);
            let mut retained = BTreeMap::new();
            for (block_id, block) in blocks {
                let flushable = match block.source {
                    BlockSource::Gossip => true,
                    BlockSource::Prefetch => requested,
                };
                if flushable {
                    self.decrement_deferred_count(block.peer_id);
                    ready.push((block.peer_id, block.fact));
                } else {
                    retained.insert(block_id, block);
                }
            }
            if retained.is_empty() {
                self.forget_kernel_block_height_request(height);
            } else {
                self.deferred_heard_blocks.insert(height, retained);
            }
        }
        if !ready.is_empty() {
            self.note_deferred_changed();
        }
        ready
    }

    fn deferred_max_height(&self) -> Option<u64> {
        self.deferred_heard_blocks.last_key_value().map(|(h, _)| *h)
    }

    fn note_deferred_changed(&mut self) {
        let max = self.deferred_max_height();
        let transition = self.catch_up.note_deferred_max_height(Instant::now(), max);
        self.publish_catch_up_metrics();
        self.log_mode_transition(transition);
    }

    /// Notify the catch-up signal that the kernel-frontier `first_negative`
    /// advanced. The driver `%seen %block` handler updates `first_negative`
    /// directly; calling this immediately after keeps the signal in lockstep.
    pub fn note_frontier_advanced(&mut self) {
        let frontier = self.first_negative;
        let transition = self
            .catch_up
            .note_frontier_advance(Instant::now(), frontier);
        self.publish_catch_up_metrics();
        self.log_mode_transition(transition);
    }

    /// Notify the catch-up signal that we observed a successful block
    /// response at `height`. Drives `peer_observed_max_height`.
    pub fn note_peer_observed_height(&mut self, height: u64) {
        let transition = self
            .catch_up
            .note_peer_response_height(Instant::now(), height);
        self.publish_catch_up_metrics();
        self.log_mode_transition(transition);
    }

    /// Read-only view of the catch-up signal.
    #[allow(dead_code)]
    pub fn catch_up_signal(&self) -> &CatchUpSignal {
        &self.catch_up
    }

    fn refresh_catch_up_mode(&mut self, now: Instant) {
        let transition = self.catch_up.refresh_mode(now);
        self.publish_catch_up_metrics();
        self.log_mode_transition(transition);
    }

    /// Whether outgoing gossip should be suppressed right now.
    /// True only while the catch-up signal reports `CatchingUp` (demonstrably
    /// behind tip). This deliberately covers every outbound gossip effect:
    /// historic block rebroadcasts, tx submission gossip, and mining output.
    /// A catching-up node is not allowed to originate gossip until it returns
    /// to `Tip`.
    pub fn should_suppress_outgoing_gossip(&mut self) -> bool {
        self.refresh_catch_up_mode(Instant::now());
        self.catch_up.is_catching_up()
    }

    fn publish_catch_up_metrics(&self) {
        let _ = self
            .metrics
            .sync_mode
            .swap(self.catch_up.mode().as_metric_value());
        let _ = self
            .metrics
            .behind_tip_estimate
            .swap(self.catch_up.behind_tip_estimate() as f64);
        let _ = self
            .metrics
            .deferred_blocks_above_frontier
            .swap(self.catch_up.deferred_blocks_above_frontier() as f64);
        let _ = self
            .metrics
            .peer_observed_max_height
            .swap(self.catch_up.peer_observed_max_height() as f64);
        let _ = self
            .metrics
            .prefetch_buffer_size
            .swap(self.deferred_heard_block_total() as f64);
    }

    fn log_mode_transition(&self, transition: Option<ModeTransition>) {
        let Some(transition) = transition else {
            return;
        };
        self.metrics.sync_mode_transitions_total.increment();
        trace!(
            from = transition.from.as_str(),
            to = transition.to.as_str(),
            frontier = self.catch_up.frontier(),
            max_deferred = self.catch_up.max_deferred_height(),
            peer_observed = self.catch_up.peer_observed_max_height(),
            behind_tip_estimate = self.catch_up.behind_tip_estimate(),
            "sync mode transition"
        );
    }

    #[cfg(test)]
    pub fn deferred_heard_block_heights(&self) -> Vec<u64> {
        self.deferred_heard_blocks.keys().copied().collect()
    }

    #[cfg(test)]
    pub fn deferred_heard_block_count(&self) -> usize {
        self.deferred_heard_block_total_count
    }

    #[cfg(test)]
    pub fn deferred_heard_block_sources_at_height(&self, height: u64) -> Vec<BlockSource> {
        self.deferred_heard_blocks
            .get(&height)
            .map(|blocks| blocks.values().map(|block| block.source).collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn speculative_tx_prefetch_count(&self) -> usize {
        self.speculative_tx_prefetches.len()
    }

    /// Returns true if we are tracking a given block ID.
    #[allow(dead_code)]
    pub fn is_tracking_block_id(&self, block_id: Noun, space: &NounSpace) -> bool {
        let Ok(block_id_str) = tip5_hash_to_base58(block_id, space) else {
            return false;
        };
        self.block_id_to_peers.contains_key(&block_id_str)
    }

    #[allow(dead_code)]
    pub fn is_tracking_peer(&self, peer_id: PeerId) -> bool {
        self.peer_to_block_ids.contains_key(&peer_id)
    }

    //  Removes the block id from the MessageTracker maps and returns all the
    //  peers who had sent us that block.
    #[cfg(test)]
    pub fn process_bad_block_id(
        &mut self,
        block_id: Noun,
        space: &NounSpace,
    ) -> Result<Vec<PeerId>, NockAppError> {
        let block_id_str = tip5_hash_to_base58(block_id, space)?;
        Ok(self.process_bad_block_id_str(&block_id_str))
    }

    pub(crate) fn process_bad_block_id_str(&mut self, block_id_str: &str) -> Vec<PeerId> {
        let peers_to_ban = self
            .block_id_to_peers
            .get(block_id_str)
            .map(|peers| peers.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        // Remove each peer that sent us this bad block
        for peer in &peers_to_ban {
            self.remove_peer(peer);
        }

        self.remove_block_id_str(block_id_str);

        peers_to_ban
    }

    /// Records settled peer capability from Identify or unsupported-protocol
    /// fallback. Per-request bookkeeping intentionally does not overwrite this
    /// with provisional send-path guesses during startup or reconnect.
    pub fn observe_peer_generation(&mut self, peer_id: PeerId, generation: ReqResGeneration) {
        self.ensure_peer_stats(peer_id).generation = Self::req_res_generation(generation);
        self.refresh_peer_stats_snapshot();
    }

    pub fn record_outbound_response(
        &mut self,
        request_context: &OutboundRequestContext,
        response_bytes: usize,
        round_trip: Duration,
        failure_count: u64,
    ) {
        let Some(request_count) = request_context.logical_request_count() else {
            return;
        };
        // Phase 5: track prefetch byte volume against the responding peer
        // so the per-peer 60s bandwidth cap can throttle subsequent
        // prefetch issuance to the same peer.
        if prefetch_range_from_request(&request_context.request).is_some() {
            self.record_prefetch_bandwidth(request_context.peer_id, response_bytes);
            if failure_count == 0 {
                self.record_prefetch_range_success(
                    request_context.peer_id, response_bytes, round_trip,
                );
            } else {
                self.record_prefetch_range_failure(request_context.peer_id, false, failure_count);
            }
        }
        let stats = self.ensure_peer_stats(request_context.peer_id);
        stats.bytes_received = stats.bytes_received.saturating_add(response_bytes as u64);
        stats.round_trip_total_ms += round_trip.as_secs_f64() * 1_000.0;
        stats.round_trip_samples = stats.round_trip_samples.saturating_add(1);
        stats.failure_count = stats
            .failure_count
            .saturating_add(failure_count.min(request_count));
        self.refresh_peer_stats_snapshot();
    }

    pub fn record_outbound_failure(
        &mut self,
        request_context: &OutboundRequestContext,
        timeout: bool,
        failure_count: u64,
    ) {
        let Some(request_count) = request_context.logical_request_count() else {
            return;
        };
        if prefetch_range_from_request(&request_context.request).is_some() {
            self.record_prefetch_range_failure(request_context.peer_id, timeout, failure_count);
        }
        let stats = self.ensure_peer_stats(request_context.peer_id);
        let bounded_failures = failure_count.min(request_count);
        stats.failure_count = stats.failure_count.saturating_add(bounded_failures);
        if timeout {
            stats.timeout_count = stats.timeout_count.saturating_add(bounded_failures);
        }
        self.refresh_peer_stats_snapshot();
    }

    pub fn record_block_received(&mut self, peer_id: PeerId, block_id: &str) {
        let block_id_owned = block_id.to_owned();
        if !self.block_receipts.contains_key(block_id) {
            self.block_receipt_order.push_back(block_id_owned.clone());
        }
        if !self
            .block_receipts
            .entry(block_id_owned.clone())
            .or_default()
            .insert(peer_id)
        {
            return;
        }
        self.evict_block_receipts();

        let now = Instant::now();
        let first_seen = self
            .block_first_seen_at
            .entry(block_id_owned)
            .or_insert(now);
        let propagation_ms = now.saturating_duration_since(*first_seen).as_secs_f64() * 1_000.0;

        let stats = self.ensure_peer_stats(peer_id);
        stats.blocks_received = stats.blocks_received.saturating_add(1);
        stats.block_propagation_total_ms += propagation_ms;
        stats.block_propagation_samples = stats.block_propagation_samples.saturating_add(1);
        self.refresh_peer_stats_snapshot();
    }
    pub fn record_outbound_request(
        &mut self,
        request_id: OutboundRequestId,
        mut context: OutboundRequestContext,
    ) {
        context.mark_started();
        if let Some(request_count) = context.logical_request_count() {
            let request_bytes = Self::encoded_request_bytes(&context.request);
            let stats = self.ensure_peer_stats(context.peer_id);
            stats.request_count = stats.request_count.saturating_add(request_count);
            stats.request_exchange_count = stats.request_exchange_count.saturating_add(1);
            stats.bytes_sent = stats.bytes_sent.saturating_add(request_bytes);
        }
        self.peer_to_outbound_requests
            .entry(context.peer_id)
            .or_default()
            .insert(request_id);
        for message in request_item_messages(&context.request) {
            *self
                .peer_to_outbound_request_items
                .entry(context.peer_id)
                .or_default()
                .entry(message.to_vec())
                .or_default() += 1;
        }
        // Phase 4: detect prefetch range requests at registration time so
        // the singleton-suppression and per-peer cap are maintained without
        // touching the SwarmAction layer. A request whose only item decodes
        // to `BlockRangeWithTxs` is a catch-up prefetch.
        if let Some((start_height, len)) = prefetch_range_from_request(&context.request) {
            let peer_id = context.peer_id;
            self.outbound_requests.insert(request_id, context);
            self.register_prefetch(request_id, peer_id, start_height, len);
        } else {
            self.outbound_requests.insert(request_id, context);
        }
        self.update_req_res_inflight_metrics();
        self.refresh_peer_stats_snapshot();
    }

    pub fn outbound_request_context(
        &self,
        request_id: OutboundRequestId,
    ) -> Option<&OutboundRequestContext> {
        self.outbound_requests.get(&request_id)
    }

    pub fn remove_outbound_request(
        &mut self,
        request_id: OutboundRequestId,
    ) -> Option<OutboundRequestContext> {
        // Phase 4: clear any prefetch tracking before unwinding the rest of
        // the outbound bookkeeping so `prefetched_heights` stops covering
        // the range as soon as the request settles.
        self.clear_prefetch(request_id);
        let context = self.outbound_requests.remove(&request_id)?;
        if let Some(request_ids) = self.peer_to_outbound_requests.get_mut(&context.peer_id) {
            request_ids.remove(&request_id);
            if request_ids.is_empty() {
                self.peer_to_outbound_requests.remove(&context.peer_id);
            }
        }
        if let Some(active_items) = self
            .peer_to_outbound_request_items
            .get_mut(&context.peer_id)
        {
            for message in request_item_messages(&context.request) {
                let should_remove = match active_items.get_mut(message) {
                    Some(count) if *count <= 1 => true,
                    Some(count) => {
                        *count -= 1;
                        false
                    }
                    None => false,
                };
                if should_remove {
                    active_items.remove(message);
                }
            }
            if active_items.is_empty() {
                self.peer_to_outbound_request_items.remove(&context.peer_id);
            }
        }
        self.update_req_res_inflight_metrics();
        Some(context)
    }

    pub fn clear_outbound_requests_for_peer(
        &mut self,
        peer_id: &PeerId,
    ) -> Vec<OutboundRequestContext> {
        let Some(request_ids) = self.peer_to_outbound_requests.remove(peer_id) else {
            return Vec::new();
        };
        self.peer_to_outbound_request_items.remove(peer_id);

        let cleared = request_ids
            .into_iter()
            .filter_map(|request_id| {
                // Phase 4: peer-wide cleanup must also drop any prefetch
                // bookkeeping for requests that originated to this peer.
                self.clear_prefetch(request_id);
                self.outbound_requests.remove(&request_id)
            })
            .collect::<Vec<_>>();
        self.update_req_res_inflight_metrics();
        cleared
    }

    pub fn try_admit_inbound_req_res(&mut self, peer_id: PeerId, max_inflight: usize) -> bool {
        if self.req_res_inflight_for_peer(peer_id) >= max_inflight {
            return false;
        }
        *self.inbound_req_res_inflight.entry(peer_id).or_insert(0) += 1;
        self.update_req_res_inflight_metrics();
        true
    }

    pub fn release_inbound_req_res(&mut self, peer_id: PeerId) {
        let Some(inflight) = self.inbound_req_res_inflight.get_mut(&peer_id) else {
            return;
        };
        if *inflight <= 1 {
            self.inbound_req_res_inflight.remove(&peer_id);
        } else {
            *inflight -= 1;
        }
        self.update_req_res_inflight_metrics();
    }

    pub fn req_res_inflight_for_peer(&self, peer_id: PeerId) -> usize {
        let outbound = self
            .peer_to_outbound_requests
            .get(&peer_id)
            .map_or(0, BTreeSet::len);
        let inbound = self
            .inbound_req_res_inflight
            .get(&peer_id)
            .copied()
            .unwrap_or(0);
        outbound + inbound
    }

    pub fn has_active_outbound_request_item(&self, peer_id: PeerId, message: &[u8]) -> bool {
        self.peer_to_outbound_request_items
            .get(&peer_id)
            .is_some_and(|active_items| active_items.get(message).is_some_and(|count| *count > 0))
    }

    #[cfg(test)]
    pub fn is_processing_block(&self, block_id: &str) -> bool {
        self.processing_blocks.contains(block_id)
    }

    #[cfg(test)]
    pub fn is_processing_tx(&self, tx_id: &str) -> bool {
        self.processing_txs.contains(tx_id)
    }

    pub fn estimated_response_message_bytes(
        &self,
        request: &NockchainDataRequest,
        fallback_message_bytes: usize,
    ) -> (usize, &'static str) {
        self.response_size_hints
            .estimate(request, fallback_message_bytes)
    }

    pub fn record_response_message_hint(
        &mut self,
        request: &NockchainDataRequest,
        message_bytes: usize,
    ) {
        self.response_size_hints.record(request, message_bytes);
    }

    #[cfg(test)]
    pub fn outbound_request_count_for_peer(&self, peer_id: PeerId) -> usize {
        self.peer_to_outbound_requests
            .get(&peer_id)
            .map_or(0, BTreeSet::len)
    }

    #[cfg(test)]
    pub fn total_outbound_request_count(&self) -> usize {
        self.outbound_requests.len()
    }

    pub async fn check_cache(
        &mut self,
        request: NockchainDataRequest,
        metrics: &NockchainP2PMetrics,
    ) -> Result<CacheResponse, NockAppError> {
        match request {
            NockchainDataRequest::BlockByHeight(height) => {
                if height >= self.first_negative {
                    metrics.block_request_cache_negative.increment();
                    trace!("Request for block height not yet seen by cache, height = {:?}", height);
                    Ok(CacheResponse::NegativeCached)
                } else if let Some(cached_block) = self.block_cache.get(&height) {
                    trace!("found cached block request by height={:?}", height);
                    metrics.block_request_cache_hits.increment();
                    Ok(CacheResponse::Cached(cached_block.clone()))
                } else {
                    trace!("didn't find cached block request by height={:?}", height);
                    metrics.block_request_cache_misses.increment();
                    Ok(CacheResponse::NotCached)
                }
            }
            NockchainDataRequest::RawTransactionById(id, _) => {
                if let Some(cached_transaction) = self.tx_cache.get(&id) {
                    trace!("found cached transaction request by id={:?}", id);
                    metrics.tx_request_cache_hits.increment();
                    Ok(CacheResponse::Cached(cached_transaction.clone()))
                } else {
                    trace!("didn't find cached transaction request by id={:?}", id);
                    metrics.tx_request_cache_misses.increment();
                    Ok(CacheResponse::NotCached)
                }
            }
            NockchainDataRequest::EldersById(id, ..) => {
                if let Some(cached_elders) = self.elders_cache.get(&id) {
                    trace!("found cached elders request by id={:?}", id);
                    Ok(CacheResponse::Cached(cached_elders.clone()))
                } else if let Some(_cached_negative) = self.elders_negative_cache.get(&id) {
                    trace!("elders id={:?} is cached-not-known", id);
                    Ok(CacheResponse::NegativeCached)
                } else {
                    trace!("didn't find cached elders request by id={:?}", id);
                    Ok(CacheResponse::NotCached)
                }
            }
            NockchainDataRequest::BlockWithTxsByHeight(_) => {
                // Bundle responses are assembled from one kernel peek. Successful
                // assembly still populates the plain block and raw tx caches.
                Ok(CacheResponse::NotCached)
            }
            NockchainDataRequest::BlockRangeWithTxs { .. } => {
                // Range bundles are assembled per-call from a single peek over a
                // contiguous height span; no precomputed cache today.
                Ok(CacheResponse::NotCached)
            }
        }
    }
}

pub enum CacheResponse {
    Cached(NounSlab),
    NotCached,
    NegativeCached,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::LazyLock;

    use libp2p::core::transport::PortUse;
    use libp2p::core::{ConnectedPoint, Endpoint};
    use libp2p::request_response;
    use libp2p::swarm::ConnectionId;
    use nockapp::noun::slab::NounSlab;
    use nockapp::AtomExt;
    use nockvm::noun::{D, T};
    use serde_bytes::ByteBuf;

    use super::*;
    use crate::config::{LibP2PConfig, PeerExclusionConfig};
    use crate::ip_block::PeerExclusions;
    use crate::messages::{BatchRequestItem, NockchainDataRequest, NockchainResponse};
    use crate::p2p_util::PeerIdExt;

    pub static LIBP2P_CONFIG: LazyLock<LibP2PConfig> = LazyLock::new(LibP2PConfig::default);

    fn isolated_test_metrics() -> Arc<NockchainP2PMetrics> {
        let registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        Arc::new(NockchainP2PMetrics::register(&registry).expect("Could not register metrics"))
    }

    fn fresh_outbound_request_id() -> OutboundRequestId {
        let mut behaviour: request_response::cbor::Behaviour<NockchainRequest, NockchainResponse> =
            request_response::cbor::Behaviour::new(
                [(
                    libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen1_protocol_version()),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default(),
            );
        behaviour.send_request(
            &PeerId::random(),
            NockchainRequest::Gossip {
                message: ByteBuf::from(vec![0xAB]),
            },
        )
    }

    fn dialer_endpoint(address: Multiaddr) -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address,
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_message_tracker_basic() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();

        // Create a block ID as [1 2 3 4 5]
        let mut slab: NounSlab = NounSlab::new();
        let block_id_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let space = slab.noun_space();

        // Add the block ID
        tracker
            .track_block_id_and_peer(block_id_tuple, peer_id, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Get the block ID string
        let block_id_str = tip5_hash_to_base58(block_id_tuple, &space).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });

        // Verify it was added correctly
        assert!(tracker.block_id_to_peers.contains_key(&block_id_str));
        assert!(tracker.peer_to_block_ids.contains_key(&peer_id));

        // Remove the block ID
        tracker
            .remove_block_id(block_id_tuple, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify it was removed
        assert!(!tracker.block_id_to_peers.contains_key(&block_id_str));
        assert!(!tracker.peer_to_block_ids.contains_key(&peer_id));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_bad_block_id() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();

        // Create a block ID
        let mut slab: NounSlab = NounSlab::new();
        let block_id_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let space = slab.noun_space();

        // Track the block ID
        tracker
            .track_block_id_and_peer(block_id_tuple, peer_id, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Mark it as bad
        let peers_to_ban = tracker
            .process_bad_block_id(block_id_tuple, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify the peer is returned for banning
        assert_eq!(peers_to_ban.len(), 1);
        assert_eq!(peers_to_ban[0], peer_id);
    }

    #[test]
    fn test_track_connection_records_first_ip_connection() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(42);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/3030"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/3031".parse().expect("valid local addr");

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );

        let ip_info = state
            .ip_info
            .get(&IpAddr::V4(Ipv4Addr::LOCALHOST))
            .expect("expected ip info for tracked connection");

        assert!(ip_info.connections.contains(&connection_id));
    }

    #[test]
    fn test_outbound_request_context_lifecycle() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let request_id = fresh_outbound_request_id();
        let context = OutboundRequestContext::new(
            peer_id,
            ReqResGeneration::Gen1,
            NockchainRequest::Gossip {
                message: ByteBuf::from(vec![0x01, 0x02]),
            },
        );

        state.record_outbound_request(request_id, context);

        let stored = state
            .outbound_request_context(request_id)
            .expect("expected stored outbound request context");
        assert_eq!(stored.peer_id, peer_id);
        assert_eq!(stored.generation, ReqResGeneration::Gen1);
        assert_eq!(state.outbound_request_count_for_peer(peer_id), 1);
        assert_eq!(state.total_outbound_request_count(), 1);

        let removed = state.remove_outbound_request(request_id);
        assert!(
            removed.is_some(),
            "expected outbound request context removal"
        );
        assert_eq!(state.outbound_request_count_for_peer(peer_id), 0);
        assert_eq!(state.total_outbound_request_count(), 0);
    }

    #[test]
    fn test_active_outbound_request_items_track_singletons_and_batch_items() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let mut request_id_source: request_response::cbor::Behaviour<
            NockchainRequest,
            NockchainResponse,
        > = request_response::cbor::Behaviour::new(
            [(
                libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen1_protocol_version()),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );
        let single_request_id = request_id_source.send_request(
            &PeerId::random(),
            NockchainRequest::Gossip {
                message: ByteBuf::from(vec![0xaa]),
            },
        );
        let batch_request_id = request_id_source.send_request(
            &PeerId::random(),
            NockchainRequest::Gossip {
                message: ByteBuf::from(vec![0xbb]),
            },
        );
        let singleton_message = vec![0x01, 0x02, 0x03];
        let batch_message_a = vec![0x0a, 0x0b];
        let batch_message_b = vec![0x0c, 0x0d];

        state.record_outbound_request(
            single_request_id,
            OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen1,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 0,
                    message: ByteBuf::from(singleton_message.clone()),
                },
            ),
        );
        state.record_outbound_request(
            batch_request_id,
            OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 0,
                    items: vec![
                        BatchRequestItem {
                            item_id: 1,
                            message: ByteBuf::from(batch_message_a.clone()),
                        },
                        BatchRequestItem {
                            item_id: 2,
                            message: ByteBuf::from(batch_message_b.clone()),
                        },
                    ],
                },
            ),
        );

        assert!(state.has_active_outbound_request_item(peer_id, &singleton_message));
        assert!(state.has_active_outbound_request_item(peer_id, &batch_message_a));
        assert!(state.has_active_outbound_request_item(peer_id, &batch_message_b));

        state.remove_outbound_request(single_request_id);
        assert!(!state.has_active_outbound_request_item(peer_id, &singleton_message));
        assert!(state.has_active_outbound_request_item(peer_id, &batch_message_a));
        assert!(state.has_active_outbound_request_item(peer_id, &batch_message_b));

        state.remove_outbound_request(batch_request_id);
        assert!(!state.has_active_outbound_request_item(peer_id, &batch_message_a));
        assert!(!state.has_active_outbound_request_item(peer_id, &batch_message_b));
    }

    #[test]
    fn test_processing_sets_gate_duplicates_and_release_on_seen_or_cancel() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let block_id = "block-id";
        let tx_id = "tx-id";

        assert!(state.try_start_processing_block(block_id));
        assert!(state.is_processing_block(block_id));
        assert!(
            !state.try_start_processing_block(block_id),
            "duplicate block should be gated while processing"
        );
        state.finish_processing_block_seen(block_id);
        assert!(!state.is_processing_block(block_id));
        assert!(
            !state.try_start_processing_block(block_id),
            "seen block should continue gating future duplicates"
        );
        assert!(
            state.try_start_processing_block_with_seen_replay(block_id, true),
            "kernel-requested block replay should pass the seen gate"
        );
        assert!(state.is_processing_block(block_id));
        assert!(
            !state.try_start_processing_block_with_seen_replay(block_id, true),
            "active replay should gate concurrent duplicates"
        );
        state.cancel_processing_block(block_id);
        assert!(!state.is_processing_block(block_id));

        assert!(state.try_start_processing_tx(tx_id));
        assert!(state.is_processing_tx(tx_id));
        assert!(
            !state.try_start_processing_tx(tx_id),
            "duplicate tx should be gated while processing"
        );
        state.cancel_processing_tx(tx_id);
        assert!(!state.is_processing_tx(tx_id));
        assert!(
            state.try_start_processing_tx(tx_id),
            "cancelled tx processing should allow a fresh retry"
        );
    }

    #[test]
    fn test_peer_stats_snapshot_tracks_req_res_counters() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let mut state = P2PState::with_peer_stats_registry(
            metrics,
            LIBP2P_CONFIG.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        );
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(404);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4040"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/4041".parse().expect("valid local addr");
        let request_id = fresh_outbound_request_id();
        let request_context = OutboundRequestContext::new(
            peer_id,
            ReqResGeneration::Gen2,
            NockchainRequest::Request {
                pow: [0; 16],
                nonce: 0,
                message: ByteBuf::from(vec![0xAA; 32]),
            },
        );

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state.observe_peer_generation(peer_id, ReqResGeneration::Gen2);
        state.record_outbound_request(request_id, request_context.clone());
        state.record_outbound_response(&request_context, 256, Duration::from_millis(40), 1);
        state.record_outbound_failure(&request_context, true, 1);
        state.record_block_received(peer_id, "block-1");
        state.record_block_received(peer_id, "block-1");

        let snapshot = peer_stats_registry.snapshot();
        let entry = snapshot
            .peers
            .iter()
            .find(|entry| entry.peer_id == peer_id.to_base58())
            .expect("expected peer stats entry");

        assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen2);
        assert_eq!(entry.request_count, 1);
        assert!(entry.bytes_sent > 0);
        assert_eq!(entry.bytes_received, 256);
        assert_eq!(entry.average_batch_size, 1.0);
        assert_eq!(entry.failure_count, 2);
        assert_eq!(entry.timeout_count, 1);
        assert_eq!(entry.blocks_received, 1);
        assert!(entry.average_round_trip_ms >= 40.0);
        assert_eq!(entry.average_block_propagation_ms, 0.0);
        assert_eq!(entry.connection_duration_seconds, 0);
    }

    #[test]
    fn test_peer_stats_snapshot_tracks_connection_duration_seconds() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let mut state = P2PState::with_peer_stats_registry(
            metrics,
            LIBP2P_CONFIG.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        );
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(808);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/5050"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5051".parse().expect("valid local addr");

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state
            .peer_stats
            .get_mut(&peer_id)
            .expect("expected peer stats accumulator")
            .connected_at = Some(Instant::now() - Duration::from_secs(12));
        state.refresh_peer_stats_snapshot();

        let snapshot = peer_stats_registry.snapshot();
        let entry = snapshot
            .peers
            .iter()
            .find(|entry| entry.peer_id == peer_id.to_base58())
            .expect("expected peer stats entry");

        assert!(entry.connection_duration_seconds >= 12);
    }

    #[test]
    fn test_peer_stats_generation_stays_unknown_during_startup_until_capability_is_observed() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let mut state = P2PState::with_peer_stats_registry(
            metrics,
            LIBP2P_CONFIG.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        );
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(809);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/5052"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5053".parse().expect("valid local addr");
        let request_id = fresh_outbound_request_id();
        let request_context = OutboundRequestContext::new(
            peer_id,
            ReqResGeneration::Gen1,
            NockchainRequest::Request {
                pow: [0; 16],
                nonce: 0,
                message: ByteBuf::from(vec![0xAB; 16]),
            },
        );

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state.record_outbound_request(request_id, request_context.clone());
        state.record_outbound_response(&request_context, 128, Duration::from_millis(15), 0);

        let snapshot = peer_stats_registry.snapshot();
        let entry = snapshot
            .peers
            .iter()
            .find(|entry| entry.peer_id == peer_id.to_base58())
            .expect("expected peer stats entry");

        assert_eq!(entry.protocol_generation, PeerReqResGeneration::Unknown);
        assert_eq!(entry.request_count, 1);
        assert_eq!(entry.bytes_received, 128);
        assert!(entry.average_round_trip_ms >= 15.0);
    }

    #[test]
    fn test_peer_stats_generation_tracks_capability_not_last_request_path() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let mut state = P2PState::with_peer_stats_registry(
            metrics,
            LIBP2P_CONFIG.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        );
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(810);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/5054"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5055".parse().expect("valid local addr");
        let request_id = fresh_outbound_request_id();

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state.observe_peer_generation(peer_id, ReqResGeneration::Gen2);
        state.record_outbound_request(
            request_id,
            OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen1,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 0,
                    message: ByteBuf::from(vec![0xBC; 8]),
                },
            ),
        );

        let snapshot = peer_stats_registry.snapshot();
        let entry = snapshot
            .peers
            .iter()
            .find(|entry| entry.peer_id == peer_id.to_base58())
            .expect("expected peer stats entry");
        assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen2);
        assert_eq!(entry.request_count, 1);
    }

    #[test]
    fn test_reconnecting_peer_stats_reset_generation_until_capability_reobserved() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let mut state = P2PState::with_peer_stats_registry(
            metrics,
            LIBP2P_CONFIG.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        );
        let peer_id = PeerId::random();
        let first_connection_id = ConnectionId::new_unchecked(811);
        let second_connection_id = ConnectionId::new_unchecked(812);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/5056"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5057".parse().expect("valid local addr");

        state.track_connection(
            first_connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr: local_addr.clone(),
                send_back_addr: remote_addr.clone(),
            },
        );
        state.observe_peer_generation(peer_id, ReqResGeneration::Gen2);
        assert_eq!(
            peer_stats_registry.snapshot().peers[0].protocol_generation,
            PeerReqResGeneration::Gen2
        );

        state.lost_connection(first_connection_id);
        assert!(peer_stats_registry
            .snapshot()
            .peers
            .iter()
            .all(|entry| entry.peer_id != peer_id.to_base58()));

        state.track_connection(
            second_connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );

        let snapshot = peer_stats_registry.snapshot();
        let entry = snapshot
            .peers
            .iter()
            .find(|entry| entry.peer_id == peer_id.to_base58())
            .expect("expected peer stats entry after reconnect");
        assert_eq!(entry.protocol_generation, PeerReqResGeneration::Unknown);
    }

    #[test]
    fn test_lost_connection_clears_outbound_request_contexts_for_disconnected_peer() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(777);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4040"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/4041".parse().expect("valid local addr");
        let request_id = fresh_outbound_request_id();

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state.record_outbound_request(
            request_id,
            OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 0,
                    items: Vec::new(),
                },
            ),
        );

        let remaining_peers = state.lost_connection(connection_id);

        assert_eq!(remaining_peers, 0);
        assert!(state.outbound_request_context(request_id).is_none());
        assert_eq!(state.outbound_request_count_for_peer(peer_id), 0);
        assert_eq!(state.total_outbound_request_count(), 0);
    }

    #[test]
    fn test_req_res_inflight_cap_counts_outbound_and_inbound_work() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let request_id = fresh_outbound_request_id();

        state.record_outbound_request(
            request_id,
            OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen2,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 0,
                    message: ByteBuf::from(vec![0x01]),
                },
            ),
        );

        assert!(state.try_admit_inbound_req_res(peer_id, 2));
        assert_eq!(state.req_res_inflight_for_peer(peer_id), 2);
        assert!(
            !state.try_admit_inbound_req_res(peer_id, 2),
            "peer should hit the hard inflight cap"
        );

        state.release_inbound_req_res(peer_id);
        assert_eq!(state.req_res_inflight_for_peer(peer_id), 1);

        state.remove_outbound_request(request_id);
        assert_eq!(state.req_res_inflight_for_peer(peer_id), 0);
    }

    #[test]
    fn test_lost_connection_refreshes_req_res_inflight_metrics() {
        let metrics = isolated_test_metrics();
        let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(778);
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/5050"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5051".parse().expect("valid local addr");

        state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        assert!(state.try_admit_inbound_req_res(peer_id, 2));
        assert_eq!(metrics.req_res_inflight_total.swap(0.0), 1.0);
        assert_eq!(metrics.req_res_inflight_max_per_peer.swap(0.0), 1.0);

        state.lost_connection(connection_id);

        assert_eq!(metrics.req_res_inflight_total.swap(0.0), 0.0);
        assert_eq!(metrics.req_res_inflight_max_per_peer.swap(0.0), 0.0);
    }

    #[test]
    fn test_response_message_hints_fall_back_then_track_exact_request_max() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let request = NockchainDataRequest::BlockByHeight(42);

        assert_eq!(
            state.estimated_response_message_bytes(&request, 1_024),
            (1_024, "configured_fallback")
        );

        state.record_response_message_hint(&request, 256);
        state.record_response_message_hint(&request, 128);
        state.record_response_message_hint(&request, 512);

        assert_eq!(
            state.estimated_response_message_bytes(&request, 1_024),
            (512, "exact_request")
        );
    }

    #[test]
    fn test_bundle_response_hints_do_not_reuse_plain_block_hints() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let block_request = NockchainDataRequest::BlockByHeight(42);
        let bundle_request = NockchainDataRequest::BlockWithTxsByHeight(42);

        state.record_response_message_hint(&block_request, 512);
        assert_eq!(
            state.estimated_response_message_bytes(&bundle_request, 2_048),
            (2_048, "configured_bundle_cap")
        );

        state.record_response_message_hint(&bundle_request, 1_500);
        state.record_response_message_hint(&bundle_request, 1_200);

        assert_eq!(
            state.estimated_response_message_bytes(&bundle_request, 2_048),
            (1_500, "exact_bundle_request")
        );
        assert_eq!(
            state.estimated_response_message_bytes(&block_request, 2_048),
            (512, "exact_request")
        );
    }

    #[test]
    fn test_range_response_hints_keep_cold_estimate_under_response_budget() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let request = NockchainDataRequest::BlockRangeWithTxs {
            start_height: 42,
            len: 16,
        };

        assert_eq!(
            state.estimated_response_message_bytes(&request, 2_097_152),
            (2_031_616, "range_cold_budget_scaled")
        );
    }

    #[test]
    fn test_range_response_hints_record_per_height_bundle_estimates() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let request = NockchainDataRequest::BlockRangeWithTxs {
            start_height: 100,
            len: 4,
        };

        state.record_response_message_hint(&request, 4_001);

        assert_eq!(
            state.estimated_response_message_bytes(
                &NockchainDataRequest::BlockWithTxsByHeight(100),
                2_048
            ),
            (1_001, "exact_bundle_request")
        );
        assert_eq!(
            state.estimated_response_message_bytes(
                &NockchainDataRequest::BlockWithTxsByHeight(103),
                2_048
            ),
            (1_001, "exact_bundle_request")
        );
        assert_eq!(
            state.estimated_response_message_bytes(
                &NockchainDataRequest::BlockWithTxsByHeight(104),
                2_048
            ),
            (1_001, "bundle_forward_tail")
        );
    }

    #[test]
    fn test_range_response_hints_cap_large_observed_windows_to_response_target() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let observed = NockchainDataRequest::BlockRangeWithTxs {
            start_height: 100,
            len: 4,
        };
        state.record_response_message_hint(&observed, 480_000);

        let request = NockchainDataRequest::BlockRangeWithTxs {
            start_height: 104,
            len: 64,
        };
        assert_eq!(
            state.estimated_response_message_bytes(&request, 2_097_152),
            (2_031_616, "range_bundle_capped")
        );
    }

    #[test]
    fn test_response_message_hints_use_exact_request_ids_for_raw_txs() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let tx_request =
            NockchainDataRequest::RawTransactionById(String::from("tx-id"), NounSlab::new());
        let other_tx_request =
            NockchainDataRequest::RawTransactionById(String::from("tx-id-2"), NounSlab::new());

        state.record_response_message_hint(&tx_request, 333);

        assert_eq!(
            state.estimated_response_message_bytes(&tx_request, 1_024),
            (333, "exact_request")
        );
        assert_eq!(
            state.estimated_response_message_bytes(&other_tx_request, 1_024),
            (1_024, "configured_fallback")
        );
    }

    #[test]
    fn test_response_message_hints_use_block_height_window_without_global_poisoning() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let nearby_request = NockchainDataRequest::BlockByHeight(100);
        let far_outlier_request = NockchainDataRequest::BlockByHeight(500);
        let estimated_request = NockchainDataRequest::BlockByHeight(103);

        state.record_response_message_hint(&nearby_request, 333);
        state.record_response_message_hint(&far_outlier_request, 900);

        assert_eq!(
            state.estimated_response_message_bytes(&estimated_request, 1_024),
            (333, "block_height_window")
        );
    }

    #[test]
    fn test_response_message_hints_use_bounded_forward_tail_for_future_blocks() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut state = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);

        state.record_response_message_hint(&NockchainDataRequest::BlockByHeight(10), 900);

        let recent_start_height = 100;
        for offset in 0..ResponseSizeHints::BLOCK_FORWARD_SAMPLE_COUNT {
            state.record_response_message_hint(
                &NockchainDataRequest::BlockByHeight(recent_start_height + offset as u64),
                200 + offset,
            );
        }

        assert_eq!(
            state.estimated_response_message_bytes(
                &NockchainDataRequest::BlockByHeight(1_000),
                1_024
            ),
            (
                200 + ResponseSizeHints::BLOCK_FORWARD_SAMPLE_COUNT - 1,
                "block_forward_tail"
            )
        );
    }

    #[tokio::test]
    async fn test_raw_tx_cache_metrics_track_hits_and_misses() {
        let metrics = isolated_test_metrics();
        let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
        let tx_id = String::from("tx-cache-id");
        let mut cached_tx = NounSlab::new();
        cached_tx.set_root(D(0));
        state.tx_cache.insert(tx_id.clone(), cached_tx.clone());

        let hit = state
            .check_cache(
                NockchainDataRequest::RawTransactionById(tx_id, NounSlab::new()),
                &metrics,
            )
            .await
            .expect("tx cache hit should succeed");
        let miss = state
            .check_cache(
                NockchainDataRequest::RawTransactionById(
                    String::from("missing-tx-cache-id"),
                    NounSlab::new(),
                ),
                &metrics,
            )
            .await
            .expect("tx cache miss should succeed");

        assert!(matches!(hit, CacheResponse::Cached(_)));
        assert!(matches!(miss, CacheResponse::NotCached));
        assert_eq!(metrics.tx_request_cache_hits.fetch_add(0), 1);
        assert_eq!(metrics.tx_request_cache_misses.fetch_add(0), 1);
    }

    #[test]
    fn test_peer_id_base58_roundtrip() {
        use nockvm::noun::Atom;
        // Generate a random PeerId
        let original_peer_id = PeerId::random();
        let base58_str = original_peer_id.to_base58();
        println!("Original base58: {}", base58_str);

        // Create a NounSlab and store the base58 string as an Atom
        let mut slab: NounSlab = NounSlab::new();
        let peer_id_atom = Atom::from_value(&mut slab, base58_str.as_bytes())
            .expect("Failed to create peer ID atom");
        let space = slab.noun_space();

        // Use the from_noun method to convert back to PeerId
        let recovered_peer_id =
            PeerId::from_noun(peer_id_atom.as_noun(), &space).unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify round trip
        assert_eq!(original_peer_id, recovered_peer_id);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_add_peer_if_tracking_block_id() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id1 = PeerId::random();
        let peer_id2 = PeerId::random();

        // Create a block ID
        let mut slab: NounSlab = NounSlab::new();
        let block_id_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let space = slab.noun_space();

        // First, try to add a peer to a non-existent block ID
        let result = tracker
            .add_peer_if_tracking_block_id(block_id_tuple, peer_id1, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        assert!(!result); // Should return false since block ID doesn't exist

        // Now track the block ID with peer1
        tracker
            .track_block_id_and_peer(block_id_tuple, peer_id1, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Add peer2 to the existing block ID
        let result = tracker
            .add_peer_if_tracking_block_id(block_id_tuple, peer_id2, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        assert!(result); // Should return true since block ID exists

        // Verify both peers are associated with the block ID
        let peers = tracker.get_peers_for_block_id(block_id_tuple, &space);
        assert_eq!(peers.len(), 2);
        assert!(peers.contains(&peer_id1));
        assert!(peers.contains(&peer_id2));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_add_peer_if_tracking_block_id_then_remove() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id1 = PeerId::random();
        let peer_id2 = PeerId::random();

        // Create a block ID
        let mut slab: NounSlab = NounSlab::new();
        let block_id_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);
        let space = slab.noun_space();
        let block_id_str = tip5_hash_to_base58(block_id_tuple, &space).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });

        // Track the block ID with peer1
        tracker
            .track_block_id_and_peer(block_id_tuple, peer_id1, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Add peer2 to the existing block ID
        let result = tracker
            .add_peer_if_tracking_block_id(block_id_tuple, peer_id2, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        assert!(result); // Should return true since block ID exists

        // Verify both peers are associated with the block ID
        let peers = tracker.get_peers_for_block_id(block_id_tuple, &space);
        assert_eq!(peers.len(), 2);
        assert!(peers.contains(&peer_id1));
        assert!(peers.contains(&peer_id2));

        // Now remove the block ID
        tracker
            .remove_block_id(block_id_tuple, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify the block ID is no longer tracked
        let peers_after_removal = tracker.get_peers_for_block_id(block_id_tuple, &space);
        assert_eq!(peers_after_removal.len(), 0);

        // Verify the block ID is removed from block_id_to_peers
        assert!(!tracker.block_id_to_peers.contains_key(&block_id_str));

        // Verify the peers either don't exist in the map anymore or don't have this block ID
        // For peer_id1
        if let Some(block_ids) = tracker.peer_to_block_ids.get(&peer_id1) {
            assert!(!block_ids.contains(&block_id_str));
        }
        // For peer_id2
        if let Some(block_ids) = tracker.peer_to_block_ids.get(&peer_id2) {
            assert!(!block_ids.contains(&block_id_str));
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
    fn test_process_bad_block_id_removes_peers() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let peer_id1 = PeerId::random();
        let peer_id2 = PeerId::random();

        // Create a block ID
        let mut slab: NounSlab = NounSlab::new();
        let block_id_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);

        // Create another block ID that both peers will share
        let other_block_id = T(&mut slab, &[D(6), D(7), D(8), D(9), D(10)]);
        let space = slab.noun_space();
        let bad_block_id_str =
            tip5_hash_to_base58(block_id_tuple, &space).expect("block id should encode");
        tracker.record_observed_block_height_candidate(bad_block_id_str.clone(), 999);

        // Track both block IDs with both peers
        tracker
            .track_block_id_and_peer(block_id_tuple, peer_id1, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        tracker
            .add_peer_if_tracking_block_id(block_id_tuple, peer_id2, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        tracker
            .track_block_id_and_peer(other_block_id, peer_id1, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        tracker
            .add_peer_if_tracking_block_id(other_block_id, peer_id2, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify both peers are tracked
        assert!(tracker.is_tracking_peer(peer_id1));
        assert!(tracker.is_tracking_peer(peer_id2));

        // Process the bad block ID
        let banned_peers = tracker
            .process_bad_block_id(block_id_tuple, &space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify both peers were returned for banning
        assert_eq!(banned_peers.len(), 2);
        assert!(banned_peers.contains(&peer_id1));
        assert!(banned_peers.contains(&peer_id2));

        // Verify both peers are no longer tracked
        assert!(!tracker.is_tracking_peer(peer_id1));
        assert!(!tracker.is_tracking_peer(peer_id2));

        // Verify the other block ID is also no longer tracked
        // (since we removed the peers entirely)
        assert!(!tracker.is_tracking_block_id(other_block_id, &space));
        assert_eq!(
            tracker.track_accepted_block_id_str_and_peer(bad_block_id_str, peer_id1),
            None,
            "bad block processing should clear the pending observed-height candidate"
        );
    }

    #[test]
    fn select_request_peers_skips_cooled_peer_when_healthy_peer_exists() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            request_peer_cooldown_secs: 60,
            ..PeerExclusionConfig::default()
        });
        let cooled_peer = PeerId::random();
        let healthy_peer = PeerId::random();

        assert!(exclusions.record_peer_request_failure(cooled_peer));

        let selected = tracker.select_request_peers_with_preferences(
            vec![cooled_peer, healthy_peer],
            10,
            &exclusions,
            false,
            &[],
        );

        assert_eq!(selected, vec![healthy_peer]);
    }

    #[test]
    fn select_request_peers_falls_back_when_every_peer_is_cooled() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            request_peer_cooldown_secs: 60,
            ..PeerExclusionConfig::default()
        });
        let left = PeerId::random();
        let right = PeerId::random();

        assert!(exclusions.record_peer_request_failure(left));
        assert!(exclusions.record_peer_request_failure(right));

        let selected = tracker.select_request_peers_with_preferences(
            vec![left, right],
            10,
            &exclusions,
            false,
            &[],
        );

        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&left));
        assert!(selected.contains(&right));
    }

    #[test]
    fn select_request_peers_drops_peer_with_only_excluded_ips() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            wrong_peer_id_ip_threshold: 1,
            ..PeerExclusionConfig::default()
        });
        let excluded_peer = PeerId::random();
        let healthy_peer = PeerId::random();
        let excluded_addr = "/ip4/15.235.216.78/udp/3602/quic-v1"
            .parse::<Multiaddr>()
            .expect("valid multiaddr");
        let healthy_addr = "/ip4/203.0.113.9/udp/3006/quic-v1"
            .parse::<Multiaddr>()
            .expect("valid multiaddr");

        tracker
            .peer_connections
            .entry(excluded_peer)
            .or_default()
            .insert(ConnectionId::new_unchecked(1), excluded_addr.clone());
        tracker
            .peer_connections
            .entry(healthy_peer)
            .or_default()
            .insert(ConnectionId::new_unchecked(2), healthy_addr);

        let outcome =
            exclusions.record_wrong_peer_id(&excluded_addr, Some(excluded_peer), PeerId::random());
        assert!(outcome.ip_exclusion.is_some());

        let selected = tracker.select_request_peers_with_preferences(
            vec![excluded_peer, healthy_peer],
            10,
            &exclusions,
            false,
            &[],
        );

        assert_eq!(selected, vec![healthy_peer]);
    }

    #[test]
    fn select_request_peers_keeps_peer_with_a_clean_connection() {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let mut tracker = P2PState::new(metrics, LIBP2P_CONFIG.seen_tx_clear_interval);
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            wrong_peer_id_ip_threshold: 1,
            ..PeerExclusionConfig::default()
        });
        let peer = PeerId::random();
        let excluded_addr = "/ip4/15.235.216.78/udp/3602/quic-v1"
            .parse::<Multiaddr>()
            .expect("valid multiaddr");
        let clean_addr = "/ip4/203.0.113.9/udp/3006/quic-v1"
            .parse::<Multiaddr>()
            .expect("valid multiaddr");

        tracker
            .peer_connections
            .entry(peer)
            .or_default()
            .insert(ConnectionId::new_unchecked(1), excluded_addr.clone());
        tracker
            .peer_connections
            .entry(peer)
            .or_default()
            .insert(ConnectionId::new_unchecked(2), clean_addr);

        let outcome = exclusions.record_wrong_peer_id(&excluded_addr, Some(peer), PeerId::random());
        assert!(outcome.ip_exclusion.is_some());

        let selected =
            tracker.select_request_peers_with_preferences(vec![peer], 10, &exclusions, false, &[]);

        assert_eq!(selected, vec![peer]);
    }

    #[test]
    fn request_success_clears_request_cooldown() {
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            request_peer_cooldown_secs: 60,
            ..PeerExclusionConfig::default()
        });
        let peer = PeerId::random();

        assert!(exclusions.record_peer_request_failure(peer));
        assert!(exclusions.is_peer_request_cooled_down(&peer));

        exclusions.record_peer_request_success(&peer);

        assert!(!exclusions.is_peer_request_cooled_down(&peer));
    }

    #[test]
    fn test_fail2ban_logging() {
        let peer_id: PeerId = libp2p::PeerId::from_bytes(&[0; 2]).unwrap();
        assert_eq!("11", peer_id.to_base58());
        let ipv4_addr = Ipv4Addr::new(192, 168, 1, 1);
        let ipv6_addr = Ipv6Addr::new(0x2001, 0x0db8, 0x0db8, 0x0db8, 0x0db8, 0x0db8, 0x0db8, 0x1);
        // Check the display representation of the IP addresses
        let ipv4_display = format!("{}", ipv4_addr);
        let ipv6_display = format!("{}", ipv6_addr);
        assert_eq!(ipv4_display, "192.168.1.1");
        assert_eq!(ipv6_display, "2001:db8:db8:db8:db8:db8:db8:1");
    }

    fn dummy_heard_block_fact() -> NockchainFact {
        let mut slab: NounSlab = NounSlab::new();
        let inner = T(&mut slab, &[D(1), D(2)]);
        slab.set_root(inner);
        NockchainFact::HeardBlock(String::from("dummy"), slab)
    }

    #[test]
    fn catch_up_signal_starts_cold() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::Cold
        );
        assert_eq!(state.catch_up_signal().behind_tip_estimate(), 0);

        // Advance frontier from 0 -> 1 with no backlog: cold flips to Tip.
        state.first_negative = 1;
        state.note_frontier_advanced();
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::Tip
        );
        assert_eq!(state.catch_up_signal().frontier(), 1);
    }

    #[test]
    fn catch_up_signal_picks_up_deferred_backlog() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        // Defer 10 future blocks at heights 100..=109, well above the
        // BEHIND_TIP_DEFERRED_THRESHOLD of 8.
        for height in 100..110u64 {
            state.defer_heard_block(
                peer_id,
                height,
                format!("block-{height}"),
                dummy_heard_block_fact(),
            );
        }
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );
        assert_eq!(state.catch_up_signal().max_deferred_height(), 109);
        // No frontier advance yet, so behind_tip_estimate is the deferred max.
        assert_eq!(state.catch_up_signal().behind_tip_estimate(), 109);
    }

    #[test]
    fn suppress_outgoing_gossip_only_while_catching_up() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        // Cold at boot: do not suppress (we don't yet know we're behind).
        assert!(!state.should_suppress_outgoing_gossip());

        // Build a deferred backlog above the threshold -> CatchingUp -> suppress.
        let peer_id = PeerId::random();
        for height in 100..110u64 {
            state.defer_heard_block(
                peer_id,
                height,
                format!("block-{height}"),
                dummy_heard_block_fact(),
            );
        }
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );
        assert!(state.should_suppress_outgoing_gossip());

        // A separate node that advances frontier with no backlog reaches Tip
        // and must not suppress.
        let mut tip_state = P2PState::new(isolated_test_metrics(), 100);
        tip_state.first_negative = 1;
        tip_state.note_frontier_advanced();
        assert_eq!(
            tip_state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::Tip
        );
        assert!(!tip_state.should_suppress_outgoing_gossip());
    }

    #[test]
    fn suppress_outgoing_gossip_refreshes_hysteresis_before_reading() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let now = Instant::now();

        state.catch_up.note_deferred_max_height(now, Some(100));
        state.catch_up.note_frontier_advance(now, 1);
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );

        let drained_at = now + Duration::from_millis(10_000);
        state.catch_up.note_deferred_max_height(drained_at, None);
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );

        let past_hysteresis = drained_at + Duration::from_millis(30_001);
        state.refresh_catch_up_mode(past_hysteresis);
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::Tip
        );
        assert!(!state.catch_up_signal().is_catching_up());
    }

    #[test]
    fn catch_up_signal_records_peer_observed_height() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        state.note_peer_observed_height(500);
        assert_eq!(state.catch_up_signal().peer_observed_max_height(), 500);
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );
    }

    #[test]
    fn accepted_block_tracking_consumes_observed_height_candidate() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        let block_id = String::from("block-42");

        state.record_observed_block_height_candidate(block_id.clone(), 42);
        assert_eq!(state.catch_up_signal().peer_observed_max_height(), 0);

        assert_eq!(
            state.track_accepted_block_id_str_and_peer(block_id.clone(), peer_id),
            Some(42)
        );
        assert_eq!(state.catch_up_signal().peer_observed_max_height(), 42);
        assert_eq!(
            state.track_accepted_block_id_str_and_peer(block_id, peer_id),
            None,
            "accepted height candidate should be consumed once"
        );
    }

    #[test]
    fn removing_block_id_clears_observed_height_candidate() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        let block_id = String::from("block-to-remove");

        state.record_observed_block_height_candidate(block_id.clone(), 88);
        state.track_block_id_str_and_peer(block_id.clone(), peer_id);
        state.remove_block_id_str(&block_id);

        assert_eq!(
            state.track_accepted_block_id_str_and_peer(block_id, peer_id),
            None
        );
        assert_eq!(state.catch_up_signal().peer_observed_max_height(), 0);
    }

    #[test]
    fn observed_height_candidate_cap_evicts_oldest_entry() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();

        for idx in 0..=OBSERVED_BLOCK_HEIGHT_CANDIDATE_CAP {
            state.record_observed_block_height_candidate(format!("block-{idx}"), idx as u64);
        }

        assert_eq!(
            state.track_accepted_block_id_str_and_peer(String::from("block-0"), peer_id),
            None,
            "oldest candidate should be evicted when the cap is exceeded"
        );
        assert_eq!(
            state.track_accepted_block_id_str_and_peer(
                format!("block-{OBSERVED_BLOCK_HEIGHT_CANDIDATE_CAP}"),
                peer_id,
            ),
            Some(OBSERVED_BLOCK_HEIGHT_CANDIDATE_CAP as u64)
        );
    }

    #[test]
    fn has_deferred_block_at_height_reflects_buffer_contents() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();

        assert!(!state.has_deferred_block_at_height(50));

        state.defer_heard_block(
            peer_id,
            50,
            String::from("block-50"),
            dummy_heard_block_fact(),
        );
        assert!(state.has_deferred_block_at_height(50));
        assert!(!state.has_deferred_block_at_height(51));

        state.defer_heard_block_with_source(
            peer_id,
            51,
            String::from("block-51"),
            dummy_heard_block_fact(),
            BlockSource::Prefetch,
        );
        assert!(state.has_deferred_block_at_height(51));
        assert_eq!(state.deferred_heard_block_total(), 2);

        // Drain everything by advancing frontier.
        state.first_negative = 100;
        let _ = state.take_ready_deferred_heard_blocks();
        assert!(!state.has_deferred_block_at_height(50));
        assert!(
            state.has_deferred_block_at_height(51),
            "prefetched blocks wait for matching kernel demand"
        );
        assert_eq!(state.deferred_heard_block_total(), 1);

        state.note_kernel_block_height_requested(51);
        let _ = state.take_ready_deferred_heard_blocks();
        assert!(!state.has_deferred_block_at_height(51));
        assert_eq!(state.deferred_heard_block_total(), 0);
    }

    #[test]
    fn prefetched_deferred_blocks_require_kernel_height_request_to_flush() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        state.first_negative = 11;
        state.defer_heard_block_with_source(
            peer_id,
            11,
            String::from("prefetch-11"),
            dummy_heard_block_fact(),
            BlockSource::Prefetch,
        );

        assert!(!state.has_ready_deferred_heard_blocks());
        assert!(!state.has_ready_deferred_block_at_height(11));
        assert!(
            state.take_ready_deferred_heard_blocks().is_empty(),
            "frontier progress alone must not flush prefetched blocks"
        );
        assert!(state.has_deferred_block_at_height(11));

        state.note_kernel_block_height_requested(11);
        assert!(state.has_ready_deferred_heard_blocks());
        assert!(state.has_ready_deferred_block_at_height(11));
        let ready = state.take_ready_deferred_heard_blocks();
        assert_eq!(ready.len(), 1);
        assert!(!state.has_deferred_block_at_height(11));
        assert!(!state.has_kernel_block_height_request(11));
    }

    #[test]
    fn gossip_deferred_blocks_keep_frontier_driven_flush_behavior() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        state.first_negative = 11;
        state.defer_heard_block(
            peer_id,
            11,
            String::from("gossip-11"),
            dummy_heard_block_fact(),
        );

        assert!(state.has_ready_deferred_heard_blocks());
        assert!(state.has_ready_deferred_block_at_height(11));
        let ready = state.take_ready_deferred_heard_blocks();
        assert_eq!(ready.len(), 1);
        assert!(!state.has_deferred_block_at_height(11));
    }

    #[test]
    fn non_range_capable_peers_memo_works_independently_of_bundle_memo() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        assert!(!state.is_peer_non_range_capable(&peer_a));
        assert!(!state.is_peer_non_bundle_capable(&peer_a));

        state.mark_peer_non_range_capable(peer_a);
        assert!(state.is_peer_non_range_capable(&peer_a));
        // Range-incapable does not imply bundle-incapable.
        assert!(!state.is_peer_non_bundle_capable(&peer_a));
        assert!(!state.is_peer_non_range_capable(&peer_b));

        state.mark_peer_non_bundle_capable(peer_b);
        assert!(state.is_peer_non_bundle_capable(&peer_b));
        assert!(!state.is_peer_non_range_capable(&peer_b));

        let snapshot = state.non_range_capable_peers_snapshot();
        assert!(snapshot.contains(&peer_a));
        assert!(!snapshot.contains(&peer_b));
    }

    #[test]
    fn block_height_retry_budget_marks_height_stuck_after_n_failures() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        state.set_prefetch_safety_config(3, Duration::from_secs(60), 50 * 1024 * 1024);
        let height = 9_801u64;
        for n in 1..3 {
            let count = state.record_block_height_failure(
                height,
                state.prefetch_height_failure_budget(),
                state.prefetch_stuck_backoff(),
            );
            assert_eq!(count, n);
            assert!(
                !state.is_block_height_stuck(height),
                "must not be stuck below budget"
            );
        }
        let count = state.record_block_height_failure(
            height,
            state.prefetch_height_failure_budget(),
            state.prefetch_stuck_backoff(),
        );
        assert_eq!(count, 3);
        assert!(state.is_block_height_stuck(height));
        assert_eq!(state.stuck_block_heights(), vec![height]);
    }

    #[test]
    fn block_height_retry_budget_clears_on_success() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        state.set_prefetch_safety_config(2, Duration::from_secs(60), 50 * 1024 * 1024);
        let height = 7u64;
        state.record_block_height_failure(
            height,
            state.prefetch_height_failure_budget(),
            state.prefetch_stuck_backoff(),
        );
        state.record_block_height_failure(
            height,
            state.prefetch_height_failure_budget(),
            state.prefetch_stuck_backoff(),
        );
        assert!(state.is_block_height_stuck(height));
        state.clear_block_height_failure(height);
        assert!(!state.is_block_height_stuck(height));
        assert!(state.stuck_block_heights().is_empty());
    }

    #[test]
    fn invalidate_deferred_block_id_drops_matching_entries() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        state.defer_heard_block(peer, 1, String::from("good-1"), dummy_heard_block_fact());
        state.defer_heard_block(peer, 1, String::from("liar-1"), dummy_heard_block_fact());
        state.defer_heard_block(peer, 2, String::from("liar-1"), dummy_heard_block_fact());

        assert_eq!(state.deferred_heard_block_total(), 3);
        let removed = state.invalidate_deferred_block_id("liar-1");
        assert!(removed);
        assert_eq!(
            state.deferred_heard_block_total(),
            1,
            "both copies of the liar block id should be dropped"
        );
        assert!(state.has_deferred_block_at_height(1));
        assert!(!state.has_deferred_block_at_height(2));
    }

    #[test]
    fn invalidate_deferred_block_id_no_op_when_not_present() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        assert!(!state.invalidate_deferred_block_id("missing"));
    }

    #[test]
    fn prefetch_bandwidth_window_aggregates_within_60s() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        let total_a = state.record_prefetch_bandwidth(peer, 1_000);
        assert_eq!(total_a, 1_000);
        let total_b = state.record_prefetch_bandwidth(peer, 2_500);
        assert_eq!(total_b, 3_500);
        assert_eq!(state.prefetch_bandwidth_window_bytes(&peer), 3_500);
    }

    #[test]
    fn ip_bucket_request_admission_groups_ipv6_by_64() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let conn_a = ConnectionId::new_unchecked(1001);
        let conn_b = ConnectionId::new_unchecked(1002);
        let addr_a: Multiaddr = "/ip6/2001:db8:abcd:12::1/tcp/5050"
            .parse()
            .expect("valid ipv6 addr");
        let addr_b: Multiaddr = "/ip6/2001:db8:abcd:12::2/tcp/5051"
            .parse()
            .expect("valid ipv6 addr");

        state.track_connection(conn_a, peer_a, &addr_a, dialer_endpoint(addr_a.clone()));
        state.track_connection(conn_b, peer_b, &addr_b, dialer_endpoint(addr_b.clone()));

        let first = state.admit_request_from_connection(conn_a, 1);
        let second = state.admit_request_from_connection(conn_b, 1);

        assert!(matches!(
            first,
            IpBucketAdmission::Admitted {
                count: 1,
                limit: 1,
                ..
            }
        ));
        assert!(matches!(
            second,
            IpBucketAdmission::Rejected {
                count: 2,
                limit: 1,
                ..
            }
        ));
    }

    #[test]
    fn ip_bucket_connection_count_groups_ipv6_by_64() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let conn_a = ConnectionId::new_unchecked(1003);
        let conn_b = ConnectionId::new_unchecked(1004);
        let addr_a: Multiaddr = "/ip6/2001:db8:ffff:40::1/tcp/5050"
            .parse()
            .expect("valid ipv6 addr");
        let addr_b: Multiaddr = "/ip6/2001:db8:ffff:40::99/tcp/5051"
            .parse()
            .expect("valid ipv6 addr");

        state.track_connection(
            conn_a,
            peer_a,
            &addr_a,
            ConnectedPoint::Listener {
                local_addr: "/ip6/::/tcp/0".parse().expect("valid local addr"),
                send_back_addr: addr_a.clone(),
            },
        );
        state.track_connection(
            conn_b,
            peer_b,
            &addr_b,
            ConnectedPoint::Listener {
                local_addr: "/ip6/::/tcp/0".parse().expect("valid local addr"),
                send_back_addr: addr_b.clone(),
            },
        );

        let Some((_bucket, count)) = state.ip_bucket_connection_count(conn_b) else {
            panic!("connection should have an IP bucket");
        };
        assert_eq!(count, 2);

        state.lost_connection(conn_a);
        let Some((_bucket, count)) = state.ip_bucket_connection_count(conn_b) else {
            panic!("remaining connection should have an IP bucket");
        };
        assert_eq!(count, 1);
    }

    #[test]
    fn gossip_bucket_rejects_over_capacity_before_refill() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        let conn = ConnectionId::new_unchecked(1005);
        let addr: Multiaddr = "/ip4/203.0.113.44/tcp/5050"
            .parse()
            .expect("valid ipv4 addr");
        state.track_connection(conn, peer, &addr, dialer_endpoint(addr.clone()));

        let first = state.admit_gossip_from_connection(conn, 1, 0);
        let second = state.admit_gossip_from_connection(conn, 1, 0);

        assert!(matches!(
            first,
            GossipBucketAdmission::Admitted {
                remaining_tokens: 0,
                ..
            }
        ));
        assert!(matches!(second, GossipBucketAdmission::Rejected { .. }));
    }

    #[test]
    fn inbound_replay_cache_rejects_duplicate_request_key() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        let key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 42,
            payload_hash: 7,
            payload_bytes: 12,
        };

        let first = state.admit_inbound_replay_key(peer, key, Duration::from_secs(60), 8);
        let second = state.admit_inbound_replay_key(peer, key, Duration::from_secs(60), 8);

        assert_eq!(first, InboundReplayAdmission::Accepted);
        assert_eq!(second, InboundReplayAdmission::Replayed);
        assert!(state.has_inbound_replay(peer, key, Duration::from_secs(60)));
    }

    #[test]
    fn inbound_replay_cache_allows_same_nonce_for_different_payloads() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        let first_key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 7,
            payload_hash: 10,
            payload_bytes: 4,
        };
        let second_key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 7,
            payload_hash: 11,
            payload_bytes: 4,
        };

        let first = state.admit_inbound_replay_key(peer, first_key, Duration::from_secs(60), 8);
        let second = state.admit_inbound_replay_key(peer, second_key, Duration::from_secs(60), 8);

        assert_eq!(first, InboundReplayAdmission::Accepted);
        assert_eq!(second, InboundReplayAdmission::Accepted);
    }

    #[test]
    fn inbound_replay_cache_evicts_oldest_per_peer() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();
        let first_key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 1,
            payload_hash: 10,
            payload_bytes: 4,
        };
        let second_key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 2,
            payload_hash: 20,
            payload_bytes: 4,
        };

        assert_eq!(
            state.admit_inbound_replay_key(peer, first_key, Duration::from_secs(60), 1),
            InboundReplayAdmission::Accepted
        );
        assert_eq!(
            state.admit_inbound_replay_key(peer, second_key, Duration::from_secs(60), 1),
            InboundReplayAdmission::Accepted
        );

        assert!(!state.has_inbound_replay(peer, first_key, Duration::from_secs(60)));
        assert!(state.has_inbound_replay(peer, second_key, Duration::from_secs(60)));
    }

    #[test]
    fn remove_peer_clears_capability_health_and_prefetch_state() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();

        state.record_request_failure(peer);
        state.mark_peer_non_bundle_capable(peer);
        state.mark_peer_non_range_capable(peer);
        state.record_prefetch_bandwidth(peer, 4096);
        state.record_prefetch_range_failure(peer, true, 1);
        let replay_key = RequestReplayKey {
            kind: crate::messages::RequestReplayKind::BatchRequest,
            nonce: 99,
            payload_hash: 100,
            payload_bytes: 8,
        };
        state.admit_inbound_replay_key(peer, replay_key, Duration::from_secs(60), 8);

        assert!(state.peer_request_health.contains_key(&peer));
        assert!(state.is_peer_non_bundle_capable(&peer));
        assert!(state.is_peer_non_range_capable(&peer));
        assert_eq!(state.prefetch_bandwidth_window_bytes(&peer), 4096);
        assert!(state.prefetch_peer_range_stats.contains_key(&peer));
        assert!(state.has_inbound_replay(peer, replay_key, Duration::from_secs(60)));

        state.remove_peer(&peer);

        assert!(!state.peer_request_health.contains_key(&peer));
        assert!(!state.is_peer_non_bundle_capable(&peer));
        assert_eq!(state.peer_range_capability(&peer), RangeCapability::Unknown);
        assert_eq!(state.prefetch_bandwidth_window_bytes(&peer), 0);
        assert!(!state.prefetch_peer_range_stats.contains_key(&peer));
        assert!(!state.has_inbound_replay(peer, replay_key, Duration::from_secs(60)));
    }

    #[test]
    fn request_peer_selection_interleaves_ip_buckets() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let peer_c = PeerId::random();
        let addr_a: Multiaddr = "/ip4/198.51.100.7/tcp/5050".parse().expect("valid addr");
        let addr_b: Multiaddr = "/ip4/198.51.100.7/tcp/5051".parse().expect("valid addr");
        let addr_c: Multiaddr = "/ip4/203.0.113.8/tcp/5052".parse().expect("valid addr");

        state.track_connection(
            ConnectionId::new_unchecked(1101),
            peer_a,
            &addr_a,
            dialer_endpoint(addr_a.clone()),
        );
        state.track_connection(
            ConnectionId::new_unchecked(1102),
            peer_b,
            &addr_b,
            dialer_endpoint(addr_b.clone()),
        );
        state.track_connection(
            ConnectionId::new_unchecked(1103),
            peer_c,
            &addr_c,
            dialer_endpoint(addr_c.clone()),
        );

        let selected = state.select_request_peers_with_preferences(
            vec![peer_a, peer_b, peer_c],
            2,
            &PeerExclusions::new(PeerExclusionConfig::default()),
            true,
            &[],
        );

        assert_eq!(selected.len(), 2);
        assert!(
            selected.contains(&peer_c),
            "selection should include the separate IP bucket before taking a second same-IP peer"
        );
    }

    #[test]
    fn seen_block_cache_stays_bounded() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);

        for index in 0..(SEEN_BLOCKS_CAP + 8) {
            state.finish_processing_block_seen(&format!("block-{index}"));
        }

        assert_eq!(state.seen_blocks.len(), SEEN_BLOCKS_CAP);
        assert!(!state.seen_blocks.contains("block-0"));
        assert!(state
            .seen_blocks
            .contains(&format!("block-{}", SEEN_BLOCKS_CAP + 7)));
    }

    #[test]
    fn elders_negative_cache_stays_bounded() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);

        for index in 0..(ELDERS_NEGATIVE_CACHE_CAP + 8) {
            state.record_elders_negative_cache(format!("elders-{index}"));
        }

        assert_eq!(state.elders_negative_cache.len(), ELDERS_NEGATIVE_CACHE_CAP);
        assert!(!state.elders_negative_cache.contains("elders-0"));
        assert!(state
            .elders_negative_cache
            .contains(&format!("elders-{}", ELDERS_NEGATIVE_CACHE_CAP + 7)));
    }

    #[test]
    fn response_size_hints_stay_bounded() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);

        for height in 0..(RESPONSE_SIZE_HINT_CAP as u64 + 8) {
            state.record_response_message_hint(&NockchainDataRequest::BlockByHeight(height), 128);
        }

        assert_eq!(
            state
                .response_size_hints
                .block_message_bytes_by_height
                .len(),
            RESPONSE_SIZE_HINT_CAP
        );
        assert!(!state
            .response_size_hints
            .block_message_bytes_by_height
            .contains_key(&0));
    }

    #[test]
    fn deferred_heard_blocks_stay_bounded_per_peer() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer = PeerId::random();

        for height in 0..(DEFERRED_HEARD_BLOCK_PER_PEER_CAP as u64 + 8) {
            state.defer_heard_block(
                peer,
                height,
                format!("block-{height}"),
                dummy_heard_block_fact(),
            );
        }

        assert_eq!(
            state.deferred_heard_block_total(),
            DEFERRED_HEARD_BLOCK_PER_PEER_CAP
        );
        assert!(!state.has_deferred_block_at_height(0));
        assert!(state.has_deferred_block_at_height(DEFERRED_HEARD_BLOCK_PER_PEER_CAP as u64 + 7));
    }

    #[test]
    fn defer_heard_block_with_source_dedups_per_block_id() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        let inserted = state.defer_heard_block(
            peer_id,
            42,
            String::from("block-42"),
            dummy_heard_block_fact(),
        );
        assert!(inserted);
        let dup = state.defer_heard_block_with_source(
            peer_id,
            42,
            String::from("block-42"),
            dummy_heard_block_fact(),
            BlockSource::Prefetch,
        );
        assert!(!dup);
        assert_eq!(state.deferred_heard_block_total(), 1);
    }

    #[test]
    fn catch_up_signal_max_deferred_drops_when_drained() {
        let mut state = P2PState::new(isolated_test_metrics(), 100);
        let peer_id = PeerId::random();
        for height in 1..=4u64 {
            state.defer_heard_block(
                peer_id,
                height,
                format!("block-{height}"),
                dummy_heard_block_fact(),
            );
        }
        // 4 deferred but threshold is 8: not yet CatchingUp.
        assert_eq!(
            state.catch_up_signal().mode(),
            crate::catch_up::SyncMode::Cold
        );

        // Advance frontier past all of them, then drain.
        state.first_negative = 5;
        state.note_frontier_advanced();
        let drained = state.take_ready_deferred_heard_blocks();
        assert_eq!(drained.len(), 4);
        assert_eq!(state.catch_up_signal().max_deferred_height(), 0);
    }
}
