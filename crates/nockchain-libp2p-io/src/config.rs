use std::collections::HashSet;
use std::net::IpAddr;
use std::num::NonZero;
use std::str::FromStr;
use std::time::Duration;

use config::{Config, ConfigError, Environment};
use serde::Deserialize;

// Kademlia constants
/** How often we should run a kademlia bootstrap to keep our peer table fresh */
const KADEMLIA_BOOTSTRAP_INTERVAL: Duration = Duration::from_secs(300);

// If the --force-peer cli arg is passed, we will force dial it every FORCE_PEER_BOOT_INTERVAL
const FORCE_PEER_DIAL_INTERVAL: Duration = Duration::from_secs(1200);

/** How long we should keep a peer connection alive with no traffic */
const SWARM_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

// Core protocol (QUIC/ping/etc) constants
/** How many times we should retry dialing our initial peers if we can't get Kademlia initialized */
// TODO: Make command-line configurable
const INITIAL_PEER_RETRIES: u32 = 5;
/** How often we should send a keep-alive message to a peer */
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(12);
/** How long should we wait before timing out the handshake */
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/** How often we should send an identify message to a peer */
const IDENTIFY_INTERVAL: Duration = Duration::from_secs(120);

/** Maximum number of established *incoming* connections */
const MAX_ESTABLISHED_INCOMING_CONNECTIONS: u32 = 256;

/** Maximum number of established *incoming* connections */
const MAX_ESTABLISHED_OUTGOING_CONNECTIONS: u32 = 32;

/** Maximum number of established connections */
const MAX_ESTABLISHED_CONNECTIONS: u32 = 288;

/** Maximum number of established connections with a single peer ID */
const MAX_ESTABLISHED_CONNECTIONS_PER_PEER: u32 = 2;

/** Maximum pending incoming connections */
const MAX_PENDING_INCOMING_CONNECTIONS: u32 = 64;

/** Maximum pending outcoing connections */
const MAX_PENDING_OUTGOING_CONNECTIONS: u32 = 32;

/** Minimum number of peers */
const MIN_PEERS: usize = 8;

// Request/response constants
const REQUEST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_HIGH_THRESHOLD: u64 = 60;
const REQUEST_HIGH_RESET: Duration = Duration::from_secs(60);
const REQUEST_REPLAY_CACHE_TTL: Duration = Duration::from_secs(300);
const REQUEST_REPLAY_CACHE_MAX_PER_PEER: usize = 4096;
const IP_BUCKET_CONNECTION_LIMIT: usize = 64;
const IP_BUCKET_REQUEST_ADMISSION_LIMIT: u64 = REQUEST_HIGH_THRESHOLD;
const GOSSIP_BUCKET_CAPACITY: u32 = 120;
const GOSSIP_BUCKET_REFILL_PER_SECOND: u32 = 2;
// Transport tuning on recovery/fan-out shapes found that 64-item batches
// retain almost all request reduction from 128-item batches with much lower
// tail delay under the current 10 ms coalescing window.
const GEN2_BATCH_MAX_ITEMS: usize = 64;
// 10 MB caps, validated by the LAX1 stacked canary. The full chain-history
// sweep (44 188 blocks) put the max observed individual raw-tx at 1.2 MiB and
// the max block-plus-txs bundle at 1.34 MiB, so 10 MB is generous headroom for
// tuned catch-up. The CBOR codec request/response maximum tracks
// `gen2_batch_max_bytes` at runtime (see behaviour.rs), so the three move
// together.
const GEN2_BATCH_MAX_BYTES: usize = 10_000_000;
const GEN2_ITEM_MAX_BYTES: usize = 10_000_000;
const GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES: usize = 10_000_000;
const GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES_ENV: &str =
    "NOCKCHAIN_LIBP2P_GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES";
const GEN2_BATCH_COALESCE_WINDOW_MS: u64 = 10;
const GEN2_MAX_INFLIGHT_PER_PEER: usize = 128;
const GEN2_SWARM_ACTION_QUEUE_CAPACITY: usize = 1000;

// ---- catch-up prefetch (Phase 4 of catch-up prefetch epic) ----
//
// Outbound prefetch of contiguous block ranges when the catch-up signal
// reports `CatchingUp`. On by default after the LAX1 stacked canary validated
// the gen2 + prefetch stack; see `docs/GEN2-ROLLOUT-STAGE-GATES.md`.
const PREFETCH_ENABLED: bool = true;
// Initial prefetch window in blocks. Phase 4 grows from this base on
// hit-rate; Phase 6 tunes against checkpoint replay.
const PREFETCH_WINDOW_INITIAL: u8 = 16;
// Hard cap on the requested prefetch window. The responder returns the
// largest contiguous prefix that fits the byte budget, which lets the
// requester ask past the expected fit point without accepting an oversized
// response.
const PREFETCH_WINDOW_MAX: u8 = 128;
// Heard-but-undelivered backlog above frontier needed to declare
// CatchingUp. Mirrors the constant in `catch_up.rs`; surfaced here so
// operators can override without recompiling.
const PREFETCH_BEHIND_THRESHOLD: u64 = 8;
// Demonstrable peer-observed gap above frontier needed to declare
// CatchingUp. Mirrors `catch_up.rs`.
const PREFETCH_PEER_OBSERVED_THRESHOLD: u64 = 32;
// Drained-condition hold time before exiting CatchingUp.
const PREFETCH_HYSTERESIS_MS: u64 = 30_000;
// Cap on simultaneously-inflight prefetches per peer. LAX1-tuned to 4 to keep
// the catch-up pipeline full while bounding per-peer prefetch load.
const PREFETCH_MAX_INFLIGHT_PER_PEER: u8 = 4;
// Failure budget per height before declaring it "stuck". Eight attempts
// covers short peer gaps during aggressive prefetch without making one
// missing height dominate the sync loop.
const PREFETCH_HEIGHT_FAILURE_BUDGET: u8 = 8;
// Hold time after a height hits its failure budget. This keeps retries from
// hot-looping while allowing a close-to-live node to rejoin quickly.
const PREFETCH_STUCK_BACKOFF_SECS: u64 = 20;
// Per-peer prefetch-byte budget over a sliding 60s window. 200 MiB is high
// enough for tuned catch-up on a trusted backbone peer while still bounding
// bad-peer amplification.
const PREFETCH_BANDWIDTH_CAP_PER_PEER_BYTES_PER_MIN: usize = 200 * 1024 * 1024;

// Elders debounce
const ELDERS_DEBOUNCE_RESET: Duration = Duration::from_secs(60);

// Cache clear interval of seen_tx cache handled in libp2p driver
const SEEN_TX_CLEAR_INTERVAL: u64 = 30;

// ALL PROTOCOLS MUST HAVE UNIQUE VERSIONS
const REQ_RES_PROTOCOL_VERSION_GEN1: &str = "/nockchain-1-req-res";
const REQ_RES_PROTOCOL_VERSION_GEN2: &str = "/nockchain-2-req-res";
const KAD_PROTOCOL_VERSION: &str = "/nockchain-1-kad";
const IDENTIFY_PROTOCOL_VERSION: &str = "/nockchain-1-identify";

const PEER_STORE_RECORD_CAPACITY: usize = 1024;

const LOW_PRIORITY_PEEK_TIMEOUT_SECS: u64 = 180;

// Default max failed pings before closing connection
const FAILED_PINGS_BEFORE_CLOSE: u64 = 4;

const IP_HYGIENE_ENABLED: bool = true;
const ADDRESS_COOLDOWN: Duration = Duration::from_secs(3600);
const IP_EXCLUSION: Duration = Duration::from_secs(3600);
const IP_EXTENDED_EXCLUSION: Duration = Duration::from_secs(21600);
const EVIDENCE_WINDOW: Duration = Duration::from_secs(600);
const IP_EXCLUSION_HISTORY: Duration = Duration::from_secs(86400);
const PERMISSION_DENIED_COOLDOWN: Duration = Duration::from_secs(300);
const WRONG_PEER_ID_IP_THRESHOLD: usize = 3;
const DIAL_FAILURE_IP_THRESHOLD: usize = 5;
const SAME_IP_KAD_ENTRY_THRESHOLD: usize = 8;
const MAX_AUTO_EXCLUSION: Duration = Duration::from_secs(21600);
const MAX_EXCLUSION_ENTRIES: usize = 4096;
const REQUEST_PEER_COOLDOWN: Duration = Duration::from_secs(300);
const FAIL2BAN_ON_TEMP_EXCLUSION: bool = false;

/// Configuration struct that allows overriding default constants from environment variables
#[derive(Debug, Deserialize, Clone)]
pub struct LibP2PConfig {
    /// How often we should run a kademlia bootstrap to keep our peer table fresh (seconds)
    #[serde(default = "default_kademlia_bootstrap_interval_secs")]
    pub kademlia_bootstrap_interval_secs: u64,

    /// How often we should force dial force peers (seconds)
    #[serde(default = "default_force_peer_dial_interval_secs")]
    pub force_peer_dial_interval_secs: u64,

    /// How long we should keep a peer connection alive with no traffic (seconds)
    #[serde(default = "default_swarm_idle_timeout_secs")]
    pub swarm_idle_timeout_secs: u64,

    /// How many times we should retry dialing our initial peers if we can't get Kademlia initialized
    #[serde(default = "default_initial_peer_retries")]
    pub initial_peer_retries: u32,

    /// How often we should send a keep-alive message to a peer (seconds)
    #[serde(default = "default_keep_alive_interval_secs")]
    pub keep_alive_interval_secs: u64,

    /// How long should we wait before timing out the handshake (seconds)
    #[serde(default = "default_handshake_timeout_secs")]
    pub handshake_timeout_secs: u64,

    /// How often we should send an identify message to a peer (seconds)
    #[serde(default = "default_identify_interval_secs")]
    pub identify_interval_secs: u64,

    /// Maximum number of established incoming connections
    #[serde(default = "default_max_established_incoming_connections")]
    pub max_established_incoming_connections: u32,

    /// Maximum number of established outgoing connections
    #[serde(default = "default_max_established_outgoing_connections")]
    pub max_established_outgoing_connections: u32,

    /// Maximum number of established connections
    #[serde(default = "default_max_established_connections")]
    pub max_established_connections: u32,

    /// Maximum number of established connections with a single peer ID
    #[serde(default = "default_max_established_connections_per_peer")]
    pub max_established_connections_per_peer: u32,

    /// Maximum pending incoming connections
    #[serde(default = "default_max_pending_incoming_connections")]
    pub max_pending_incoming_connections: u32,

    /// Maximum pending outgoing connections
    #[serde(default = "default_max_pending_outgoing_connections")]
    pub max_pending_outgoing_connections: u32,

    /// Minimum number of peers
    #[serde(default = "default_min_peers")]
    pub min_peers: usize,

    /// Request/response timeout (seconds)
    #[serde(default = "default_request_response_timeout_secs")]
    pub request_response_timeout_secs: u64,

    /// Request high threshold
    #[serde(default = "default_request_high_threshold")]
    pub request_high_threshold: u64,

    /// Request high reset timeout (seconds)
    #[serde(default = "default_request_high_reset_secs")]
    pub request_high_reset_secs: u64,

    /// How long inbound request replay keys stay live per peer. Zero disables replay tracking.
    #[serde(default = "default_request_replay_cache_ttl_secs")]
    pub request_replay_cache_ttl_secs: u64,

    /// Maximum live inbound request replay keys retained for one peer. Zero disables tracking.
    #[serde(default = "default_request_replay_cache_max_per_peer")]
    pub request_replay_cache_max_per_peer: usize,

    /// Maximum established connections accepted from one IP bucket. IPv4 buckets are /32;
    /// IPv6 buckets are /64. Zero disables the cap.
    #[serde(default = "default_ip_bucket_connection_limit")]
    pub ip_bucket_connection_limit: usize,

    /// Maximum request-response admissions from one IP bucket per request-high reset window.
    /// IPv4 buckets are /32; IPv6 buckets are /64. Zero disables the cap.
    #[serde(default = "default_ip_bucket_request_admission_limit")]
    pub ip_bucket_request_admission_limit: u64,

    /// Gossip token-bucket capacity per IP bucket. Zero disables the gossip bucket.
    #[serde(default = "default_gossip_bucket_capacity")]
    pub gossip_bucket_capacity: u32,

    /// Gossip tokens refilled per second per IP bucket.
    #[serde(default = "default_gossip_bucket_refill_per_second")]
    pub gossip_bucket_refill_per_second: u32,

    /// Accept inbound gen2 request-response traffic. On by default — the
    /// validated everything-on backbone posture (LAX1-tuned).
    #[serde(default = "default_req_res_gen2_accept_enabled")]
    pub req_res_gen2_accept_enabled: bool,

    /// Prefer outbound gen2 request-response traffic when the remote supports it.
    /// On by default; falls back to gen1 for peers that do not support gen2.
    #[serde(default = "default_req_res_gen2_send_enabled")]
    pub req_res_gen2_send_enabled: bool,

    /// Upgrade outbound `%request %block %by-height` effects to the
    /// bundled `%block-with-txs` request variant, asking peers to return
    /// the block plus its raw transactions in a single response. On by
    /// default. Against a pre-bundle peer this surfaces as
    /// `BatchErrorClass::Decode` and the chunk-4 fallback re-issues the
    /// classic request.
    #[serde(default = "default_req_res_gen2_bundle_enabled")]
    pub req_res_gen2_bundle_enabled: bool,

    /// Send the authenticated gossip request variant. Off by default until
    /// staged rollout confirms peer compatibility.
    #[serde(default = "default_req_res_authenticated_gossip_send_enabled")]
    pub req_res_authenticated_gossip_send_enabled: bool,

    /// Accept the legacy unauthenticated gossip request variant. On by default
    /// during the accept-old/send-new rollout period.
    #[serde(default = "default_req_res_legacy_gossip_accept_enabled")]
    pub req_res_legacy_gossip_accept_enabled: bool,

    /// Hard item cap for outbound and inbound gen2 batches.
    #[serde(default = "default_gen2_batch_max_items")]
    pub gen2_batch_max_items: usize,

    /// Hard byte cap for outbound and inbound gen2 batches.
    #[serde(default = "default_gen2_batch_max_bytes")]
    pub gen2_batch_max_bytes: usize,

    /// Hard byte cap for one gen2 batch item payload.
    #[serde(default = "default_gen2_item_max_bytes")]
    pub gen2_item_max_bytes: usize,

    /// Reserved response byte budget for outbound gen2 batches that include
    /// response-budgeted requests.
    ///
    /// This bound is applied in addition to the general gen2 batch byte cap so
    /// checkpoint-backed block sync and raw transaction recovery stay on a tighter requester replay
    /// budget.
    #[serde(default = "default_gen2_block_batch_max_response_bytes")]
    pub gen2_block_batch_max_response_bytes: usize,

    /// Time window for outbound batch coalescing.
    #[serde(default = "default_gen2_batch_coalesce_window_ms")]
    pub gen2_batch_coalesce_window_ms: u64,

    /// Hard cap on inflight req-res work per peer.
    #[serde(default = "default_gen2_max_inflight_per_peer")]
    pub gen2_max_inflight_per_peer: usize,

    /// Bounded queue size for driver -> swarm req-res actions.
    #[serde(default = "default_gen2_swarm_action_queue_capacity")]
    pub gen2_swarm_action_queue_capacity: usize,

    /// Enable catch-up block prefetch. When `true` and the catch-up signal
    /// reports `CatchingUp`, the driver issues `BlockRangeWithTxs` requests
    /// to refill the deferred-block buffer ahead of kernel demand. On by
    /// default after LAX1 canary validation.
    #[serde(default = "default_prefetch_enabled")]
    pub prefetch_enabled: bool,

    /// Initial prefetch window size in blocks; the per-peer window grows
    /// from this base on cache-hit rate.
    #[serde(default = "default_prefetch_window_initial")]
    pub prefetch_window_initial: u8,

    /// Hard cap on the prefetch window; bounds the responder cost and the
    /// requester decode size per range request.
    #[serde(default = "default_prefetch_window_max")]
    pub prefetch_window_max: u8,

    /// Override the catch-up signal's deferred-buffer threshold.
    #[serde(default = "default_prefetch_behind_threshold")]
    pub prefetch_behind_threshold: u64,

    /// Override the catch-up signal's peer-observed gap threshold.
    #[serde(default = "default_prefetch_peer_observed_threshold")]
    pub prefetch_peer_observed_threshold: u64,

    /// Override the catch-up signal's exit-hysteresis hold time in
    /// milliseconds.
    #[serde(default = "default_prefetch_hysteresis_ms")]
    pub prefetch_hysteresis_ms: u64,

    /// Cap on simultaneously-inflight prefetches per peer.
    #[serde(default = "default_prefetch_max_inflight_per_peer")]
    pub prefetch_max_inflight_per_peer: u8,

    /// Failure budget per height before backing off retries.
    #[serde(default = "default_prefetch_height_failure_budget")]
    pub prefetch_height_failure_budget: u8,

    /// Backoff hold time after a height hits its failure budget.
    #[serde(default = "default_prefetch_stuck_backoff_secs")]
    pub prefetch_stuck_backoff_secs: u64,

    /// Per-peer prefetch byte budget over a sliding 60s window.
    #[serde(default = "default_prefetch_bandwidth_cap_per_peer_bytes_per_min")]
    pub prefetch_bandwidth_cap_per_peer_bytes_per_min: usize,

    // These have to be static.
    // /// Request/response protocol version
    // #[serde(default = "default_req_res_protocol_version")]
    // pub req_res_protocol_version: String,

    // /// Kademlia protocol version
    // #[serde(default = "default_kad_protocol_version")]
    // pub kad_protocol_version: String,
    ///// Identify protocol version
    //#[serde(default = "default_identify_protocol_version")]
    //pub identify_protocol_version: String,
    /// Peer store record capacity
    /// This is the maximum number of records that can be stored in the peer store.
    #[serde(default = "default_peer_store_record_capacity")]
    pub peer_store_record_capacity: NonZero<usize>,

    /// Interval for logging peer status
    /// This is the interval at which peer status will be logged.
    #[serde(default = "default_peer_status_interval_secs")]
    pub peer_status_interval_secs: u64,

    /// Interval for debouncing elders
    #[serde(default = "default_elders_debounce_reset_secs")]
    pub elders_debounce_reset_secs: u64,

    /// Block interval for clearing seen transactions.
    /// The cache will clear after seeing this many new blocks
    /// added to the heaviest chain.
    #[serde(default = "default_seen_tx_clear_interval")]
    pub seen_tx_clear_interval: u64,

    /// Timeout for low-priority peeks.
    #[serde(default = "default_low_priority_peek_timeout_secs")]
    pub low_priority_peek_timeout_secs: u64,

    /// Number of failed pings before closing connection
    #[serde(default = "default_failed_pings_before_close")]
    pub failed_pings_before_close: u64,

    /// Whether temporary IP and endpoint hygiene is active
    #[serde(default = "default_ip_hygiene_enabled")]
    pub ip_hygiene_enabled: bool,

    /// Address cooldown duration after endpoint-local evidence
    #[serde(default = "default_address_cooldown_secs")]
    pub address_cooldown_secs: u64,

    /// First IP exclusion duration after same-IP evidence crosses a threshold
    #[serde(default = "default_ip_exclusion_secs")]
    pub ip_exclusion_secs: u64,

    /// Recurrent IP exclusion duration
    #[serde(default = "default_ip_extended_exclusion_secs")]
    pub ip_extended_exclusion_secs: u64,

    /// Evidence window for escalation
    #[serde(default = "default_evidence_window_secs")]
    pub evidence_window_secs: u64,

    /// Time horizon for repeated exclusion escalation
    #[serde(default = "default_ip_exclusion_history_secs")]
    pub ip_exclusion_history_secs: u64,

    /// Cooldown for local PermissionDenied transport failures
    #[serde(default = "default_permission_denied_cooldown_secs")]
    pub permission_denied_cooldown_secs: u64,

    /// Distinct wrong peer IDs or ports needed before IP exclusion
    #[serde(default = "default_wrong_peer_id_ip_threshold")]
    pub wrong_peer_id_ip_threshold: usize,

    /// Reserved compatibility knob; transport failures create endpoint cooldowns only
    #[serde(default = "default_dial_failure_ip_threshold")]
    pub dial_failure_ip_threshold: usize,

    /// Local Kademlia cardinality that marks same-IP pollution
    #[serde(default = "default_same_ip_kad_entry_threshold")]
    pub same_ip_kad_entry_threshold: usize,

    /// Upper bound for automatic exclusion duration
    #[serde(default = "default_max_auto_exclusion_secs")]
    pub max_auto_exclusion_secs: u64,

    /// Max evidence events retained in memory
    #[serde(default = "default_max_exclusion_entries")]
    pub max_exclusion_entries: usize,

    /// Peer request cooldown after request-response failure
    #[serde(default = "default_request_peer_cooldown_secs")]
    pub request_peer_cooldown_secs: u64,

    /// Comma-separated IPs exempt from automatic exclusion
    #[serde(default)]
    pub exclusion_allow_ips: String,

    /// Emit fail2ban-compatible lines for local peer blocks and temporary exclusions
    #[serde(default = "default_fail2ban_on_temp_exclusion")]
    pub fail2ban_on_temp_exclusion: bool,
}

// Default value functions
fn default_kademlia_bootstrap_interval_secs() -> u64 {
    KADEMLIA_BOOTSTRAP_INTERVAL.as_secs()
}
fn default_force_peer_dial_interval_secs() -> u64 {
    FORCE_PEER_DIAL_INTERVAL.as_secs()
}
fn default_swarm_idle_timeout_secs() -> u64 {
    SWARM_IDLE_TIMEOUT.as_secs()
}
fn default_initial_peer_retries() -> u32 {
    INITIAL_PEER_RETRIES
}
fn default_keep_alive_interval_secs() -> u64 {
    KEEP_ALIVE_INTERVAL.as_secs()
}
fn default_handshake_timeout_secs() -> u64 {
    HANDSHAKE_TIMEOUT.as_secs()
}
fn default_identify_interval_secs() -> u64 {
    IDENTIFY_INTERVAL.as_secs()
}
fn default_max_established_incoming_connections() -> u32 {
    MAX_ESTABLISHED_INCOMING_CONNECTIONS
}
fn default_max_established_outgoing_connections() -> u32 {
    MAX_ESTABLISHED_OUTGOING_CONNECTIONS
}
fn default_max_established_connections() -> u32 {
    MAX_ESTABLISHED_CONNECTIONS
}
fn default_max_established_connections_per_peer() -> u32 {
    MAX_ESTABLISHED_CONNECTIONS_PER_PEER
}
fn default_max_pending_incoming_connections() -> u32 {
    MAX_PENDING_INCOMING_CONNECTIONS
}
fn default_max_pending_outgoing_connections() -> u32 {
    MAX_PENDING_OUTGOING_CONNECTIONS
}
fn default_min_peers() -> usize {
    MIN_PEERS
}
fn default_request_response_timeout_secs() -> u64 {
    REQUEST_RESPONSE_TIMEOUT.as_secs()
}
fn default_request_high_threshold() -> u64 {
    REQUEST_HIGH_THRESHOLD
}
fn default_request_high_reset_secs() -> u64 {
    REQUEST_HIGH_RESET.as_secs()
}
fn default_request_replay_cache_ttl_secs() -> u64 {
    REQUEST_REPLAY_CACHE_TTL.as_secs()
}
fn default_request_replay_cache_max_per_peer() -> usize {
    REQUEST_REPLAY_CACHE_MAX_PER_PEER
}
fn default_ip_bucket_connection_limit() -> usize {
    IP_BUCKET_CONNECTION_LIMIT
}
fn default_ip_bucket_request_admission_limit() -> u64 {
    IP_BUCKET_REQUEST_ADMISSION_LIMIT
}
fn default_gossip_bucket_capacity() -> u32 {
    GOSSIP_BUCKET_CAPACITY
}
fn default_gossip_bucket_refill_per_second() -> u32 {
    GOSSIP_BUCKET_REFILL_PER_SECOND
}
fn default_req_res_gen2_accept_enabled() -> bool {
    true
}
fn default_req_res_gen2_send_enabled() -> bool {
    true
}
fn default_req_res_gen2_bundle_enabled() -> bool {
    true
}
fn default_req_res_authenticated_gossip_send_enabled() -> bool {
    false
}
fn default_req_res_legacy_gossip_accept_enabled() -> bool {
    true
}
fn default_gen2_batch_max_items() -> usize {
    GEN2_BATCH_MAX_ITEMS
}
fn default_gen2_batch_max_bytes() -> usize {
    GEN2_BATCH_MAX_BYTES
}
fn default_gen2_item_max_bytes() -> usize {
    GEN2_ITEM_MAX_BYTES
}
fn default_gen2_block_batch_max_response_bytes() -> usize {
    GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES
}
fn default_gen2_batch_coalesce_window_ms() -> u64 {
    GEN2_BATCH_COALESCE_WINDOW_MS
}
fn default_gen2_max_inflight_per_peer() -> usize {
    GEN2_MAX_INFLIGHT_PER_PEER
}
fn default_gen2_swarm_action_queue_capacity() -> usize {
    GEN2_SWARM_ACTION_QUEUE_CAPACITY
}
fn default_prefetch_enabled() -> bool {
    PREFETCH_ENABLED
}
fn default_prefetch_window_initial() -> u8 {
    PREFETCH_WINDOW_INITIAL
}
fn default_prefetch_window_max() -> u8 {
    PREFETCH_WINDOW_MAX
}
fn default_prefetch_behind_threshold() -> u64 {
    PREFETCH_BEHIND_THRESHOLD
}
fn default_prefetch_peer_observed_threshold() -> u64 {
    PREFETCH_PEER_OBSERVED_THRESHOLD
}
fn default_prefetch_hysteresis_ms() -> u64 {
    PREFETCH_HYSTERESIS_MS
}
fn default_prefetch_max_inflight_per_peer() -> u8 {
    PREFETCH_MAX_INFLIGHT_PER_PEER
}
fn default_prefetch_height_failure_budget() -> u8 {
    PREFETCH_HEIGHT_FAILURE_BUDGET
}
fn default_prefetch_stuck_backoff_secs() -> u64 {
    PREFETCH_STUCK_BACKOFF_SECS
}
fn default_prefetch_bandwidth_cap_per_peer_bytes_per_min() -> usize {
    PREFETCH_BANDWIDTH_CAP_PER_PEER_BYTES_PER_MIN
}

fn default_peer_store_record_capacity() -> NonZero<usize> {
    PEER_STORE_RECORD_CAPACITY
        .try_into()
        .expect("Peer store record capacity must be non-zero")
}

fn default_peer_status_interval_secs() -> u64 {
    300 // Log peer status and potentially redial every 5 minutes
}

fn default_elders_debounce_reset_secs() -> u64 {
    ELDERS_DEBOUNCE_RESET.as_secs() // Reset elders debounce every 60 seconds
}

fn default_seen_tx_clear_interval() -> u64 {
    SEEN_TX_CLEAR_INTERVAL // By default, clear seen_tx cache after every new block on heaviest chain
}

fn default_low_priority_peek_timeout_secs() -> u64 {
    LOW_PRIORITY_PEEK_TIMEOUT_SECS
}

fn default_failed_pings_before_close() -> u64 {
    FAILED_PINGS_BEFORE_CLOSE // Number of failed pings before closing connection
}
fn default_ip_hygiene_enabled() -> bool {
    IP_HYGIENE_ENABLED
}
fn default_address_cooldown_secs() -> u64 {
    ADDRESS_COOLDOWN.as_secs()
}
fn default_ip_exclusion_secs() -> u64 {
    IP_EXCLUSION.as_secs()
}
fn default_ip_extended_exclusion_secs() -> u64 {
    IP_EXTENDED_EXCLUSION.as_secs()
}
fn default_evidence_window_secs() -> u64 {
    EVIDENCE_WINDOW.as_secs()
}
fn default_ip_exclusion_history_secs() -> u64 {
    IP_EXCLUSION_HISTORY.as_secs()
}
fn default_permission_denied_cooldown_secs() -> u64 {
    PERMISSION_DENIED_COOLDOWN.as_secs()
}
fn default_wrong_peer_id_ip_threshold() -> usize {
    WRONG_PEER_ID_IP_THRESHOLD
}
fn default_dial_failure_ip_threshold() -> usize {
    DIAL_FAILURE_IP_THRESHOLD
}
fn default_same_ip_kad_entry_threshold() -> usize {
    SAME_IP_KAD_ENTRY_THRESHOLD
}
fn default_max_auto_exclusion_secs() -> u64 {
    MAX_AUTO_EXCLUSION.as_secs()
}
fn default_max_exclusion_entries() -> usize {
    MAX_EXCLUSION_ENTRIES
}
fn default_request_peer_cooldown_secs() -> u64 {
    REQUEST_PEER_COOLDOWN.as_secs()
}
fn default_fail2ban_on_temp_exclusion() -> bool {
    FAIL2BAN_ON_TEMP_EXCLUSION
}

// Do _not_ use this default implementation in production code. It's just a fallback.
// Use from_env() to load from environment variables with sensible defaults.
impl Default for LibP2PConfig {
    fn default() -> Self {
        Self {
            kademlia_bootstrap_interval_secs: default_kademlia_bootstrap_interval_secs(),
            force_peer_dial_interval_secs: default_force_peer_dial_interval_secs(),
            swarm_idle_timeout_secs: default_swarm_idle_timeout_secs(),
            initial_peer_retries: default_initial_peer_retries(),
            keep_alive_interval_secs: default_keep_alive_interval_secs(),
            handshake_timeout_secs: default_handshake_timeout_secs(),
            identify_interval_secs: default_identify_interval_secs(),
            max_established_incoming_connections: default_max_established_incoming_connections(),
            max_established_outgoing_connections: default_max_established_outgoing_connections(),
            max_established_connections: default_max_established_connections(),
            max_established_connections_per_peer: default_max_established_connections_per_peer(),
            max_pending_incoming_connections: default_max_pending_incoming_connections(),
            max_pending_outgoing_connections: default_max_pending_outgoing_connections(),
            min_peers: default_min_peers(),
            request_response_timeout_secs: default_request_response_timeout_secs(),
            request_high_threshold: default_request_high_threshold(),
            request_high_reset_secs: default_request_high_reset_secs(),
            request_replay_cache_ttl_secs: default_request_replay_cache_ttl_secs(),
            request_replay_cache_max_per_peer: default_request_replay_cache_max_per_peer(),
            ip_bucket_connection_limit: default_ip_bucket_connection_limit(),
            ip_bucket_request_admission_limit: default_ip_bucket_request_admission_limit(),
            gossip_bucket_capacity: default_gossip_bucket_capacity(),
            gossip_bucket_refill_per_second: default_gossip_bucket_refill_per_second(),
            req_res_gen2_accept_enabled: default_req_res_gen2_accept_enabled(),
            req_res_gen2_send_enabled: default_req_res_gen2_send_enabled(),
            req_res_gen2_bundle_enabled: default_req_res_gen2_bundle_enabled(),
            req_res_authenticated_gossip_send_enabled:
                default_req_res_authenticated_gossip_send_enabled(),
            req_res_legacy_gossip_accept_enabled: default_req_res_legacy_gossip_accept_enabled(),
            gen2_batch_max_items: default_gen2_batch_max_items(),
            gen2_batch_max_bytes: default_gen2_batch_max_bytes(),
            gen2_item_max_bytes: default_gen2_item_max_bytes(),
            gen2_block_batch_max_response_bytes: default_gen2_block_batch_max_response_bytes(),
            gen2_batch_coalesce_window_ms: default_gen2_batch_coalesce_window_ms(),
            gen2_max_inflight_per_peer: default_gen2_max_inflight_per_peer(),
            gen2_swarm_action_queue_capacity: default_gen2_swarm_action_queue_capacity(),
            prefetch_enabled: default_prefetch_enabled(),
            prefetch_window_initial: default_prefetch_window_initial(),
            prefetch_window_max: default_prefetch_window_max(),
            prefetch_behind_threshold: default_prefetch_behind_threshold(),
            prefetch_peer_observed_threshold: default_prefetch_peer_observed_threshold(),
            prefetch_hysteresis_ms: default_prefetch_hysteresis_ms(),
            prefetch_max_inflight_per_peer: default_prefetch_max_inflight_per_peer(),
            prefetch_height_failure_budget: default_prefetch_height_failure_budget(),
            prefetch_stuck_backoff_secs: default_prefetch_stuck_backoff_secs(),
            prefetch_bandwidth_cap_per_peer_bytes_per_min:
                default_prefetch_bandwidth_cap_per_peer_bytes_per_min(),
            peer_store_record_capacity: default_peer_store_record_capacity(),
            peer_status_interval_secs: default_peer_status_interval_secs(),
            elders_debounce_reset_secs: default_elders_debounce_reset_secs(),
            seen_tx_clear_interval: default_seen_tx_clear_interval(),
            low_priority_peek_timeout_secs: default_low_priority_peek_timeout_secs(),
            failed_pings_before_close: default_failed_pings_before_close(),
            ip_hygiene_enabled: default_ip_hygiene_enabled(),
            address_cooldown_secs: default_address_cooldown_secs(),
            ip_exclusion_secs: default_ip_exclusion_secs(),
            ip_extended_exclusion_secs: default_ip_extended_exclusion_secs(),
            evidence_window_secs: default_evidence_window_secs(),
            ip_exclusion_history_secs: default_ip_exclusion_history_secs(),
            permission_denied_cooldown_secs: default_permission_denied_cooldown_secs(),
            wrong_peer_id_ip_threshold: default_wrong_peer_id_ip_threshold(),
            dial_failure_ip_threshold: default_dial_failure_ip_threshold(),
            same_ip_kad_entry_threshold: default_same_ip_kad_entry_threshold(),
            max_auto_exclusion_secs: default_max_auto_exclusion_secs(),
            max_exclusion_entries: default_max_exclusion_entries(),
            request_peer_cooldown_secs: default_request_peer_cooldown_secs(),
            exclusion_allow_ips: String::new(),
            fail2ban_on_temp_exclusion: default_fail2ban_on_temp_exclusion(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PeerExclusionConfig {
    pub(crate) enabled: bool,
    pub(crate) address_cooldown_secs: u64,
    pub(crate) ip_exclusion_secs: u64,
    pub(crate) ip_extended_exclusion_secs: u64,
    pub(crate) evidence_window_secs: u64,
    pub(crate) ip_exclusion_history_secs: u64,
    pub(crate) permission_denied_cooldown_secs: u64,
    pub(crate) wrong_peer_id_ip_threshold: usize,
    pub(crate) same_ip_kad_entry_threshold: usize,
    pub(crate) max_auto_exclusion_secs: u64,
    pub(crate) max_exclusion_entries: usize,
    pub(crate) request_peer_cooldown_secs: u64,
    pub(crate) allow_ips: HashSet<IpAddr>,
    pub(crate) fail2ban_on_temp_exclusion: bool,
}

impl Default for PeerExclusionConfig {
    fn default() -> Self {
        Self {
            enabled: default_ip_hygiene_enabled(),
            address_cooldown_secs: default_address_cooldown_secs(),
            ip_exclusion_secs: default_ip_exclusion_secs(),
            ip_extended_exclusion_secs: default_ip_extended_exclusion_secs(),
            evidence_window_secs: default_evidence_window_secs(),
            ip_exclusion_history_secs: default_ip_exclusion_history_secs(),
            permission_denied_cooldown_secs: default_permission_denied_cooldown_secs(),
            wrong_peer_id_ip_threshold: default_wrong_peer_id_ip_threshold(),
            same_ip_kad_entry_threshold: default_same_ip_kad_entry_threshold(),
            max_auto_exclusion_secs: default_max_auto_exclusion_secs(),
            max_exclusion_entries: default_max_exclusion_entries(),
            request_peer_cooldown_secs: default_request_peer_cooldown_secs(),
            allow_ips: HashSet::new(),
            fail2ban_on_temp_exclusion: default_fail2ban_on_temp_exclusion(),
        }
    }
}

impl PeerExclusionConfig {
    pub(crate) fn from_libp2p_config(config: &LibP2PConfig) -> Result<Self, ConfigError> {
        let mut allow_ips = HashSet::new();
        for raw in config.exclusion_allow_ips.split(',') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let ip = IpAddr::from_str(raw).map_err(|err| {
                ConfigError::Message(format!(
                    "invalid NOCKCHAIN_LIBP2P_EXCLUSION_ALLOW_IPS entry {raw}: {err}"
                ))
            })?;
            allow_ips.insert(ip);
        }

        Ok(Self {
            enabled: config.ip_hygiene_enabled,
            address_cooldown_secs: config.address_cooldown_secs,
            ip_exclusion_secs: config.ip_exclusion_secs,
            ip_extended_exclusion_secs: config.ip_extended_exclusion_secs,
            evidence_window_secs: config.evidence_window_secs,
            ip_exclusion_history_secs: config.ip_exclusion_history_secs,
            permission_denied_cooldown_secs: config.permission_denied_cooldown_secs,
            wrong_peer_id_ip_threshold: config.wrong_peer_id_ip_threshold,
            same_ip_kad_entry_threshold: config.same_ip_kad_entry_threshold,
            max_auto_exclusion_secs: config.max_auto_exclusion_secs,
            max_exclusion_entries: config.max_exclusion_entries,
            request_peer_cooldown_secs: config.request_peer_cooldown_secs,
            allow_ips,
            fail2ban_on_temp_exclusion: config.fail2ban_on_temp_exclusion,
        })
    }

    pub(crate) fn address_cooldown(&self) -> Duration {
        Duration::from_secs(self.address_cooldown_secs)
    }

    pub(crate) fn ip_exclusion(&self) -> Duration {
        Duration::from_secs(self.ip_exclusion_secs)
    }

    pub(crate) fn ip_extended_exclusion(&self) -> Duration {
        Duration::from_secs(self.ip_extended_exclusion_secs)
    }

    pub(crate) fn evidence_window(&self) -> Duration {
        Duration::from_secs(self.evidence_window_secs)
    }

    pub(crate) fn ip_exclusion_history(&self) -> Duration {
        Duration::from_secs(self.ip_exclusion_history_secs)
    }

    pub(crate) fn permission_denied_cooldown(&self) -> Duration {
        Duration::from_secs(self.permission_denied_cooldown_secs)
    }

    pub(crate) fn max_auto_exclusion(&self) -> Duration {
        Duration::from_secs(self.max_auto_exclusion_secs)
    }

    pub(crate) fn event_history(&self) -> Duration {
        self.evidence_window().max(self.ip_exclusion_history())
    }

    pub(crate) fn request_peer_cooldown(&self) -> Duration {
        Duration::from_secs(self.request_peer_cooldown_secs)
    }
}

impl LibP2PConfig {
    /// Load configuration from environment variables with NOCKCHAIN_LIBP2P_ prefix
    pub fn from_env() -> Result<Self, ConfigError> {
        let config = Config::builder()
            .add_source(
                Environment::with_prefix("NOCKCHAIN_LIBP2P")
                    .prefix_separator("_")
                    .separator(".")
                    .try_parsing(true),
            )
            .build()?;

        config.try_deserialize()
    }

    /// Load configuration from environment variables, falling back to defaults on error
    pub fn from_env_or_default() -> Self {
        Self::from_env().unwrap_or_default()
    }

    pub fn kad_protocol_version() -> &'static str {
        KAD_PROTOCOL_VERSION
    }

    pub fn req_res_protocol_version() -> &'static str {
        Self::req_res_gen1_protocol_version()
    }

    pub fn req_res_gen1_protocol_version() -> &'static str {
        REQ_RES_PROTOCOL_VERSION_GEN1
    }

    pub fn req_res_gen2_protocol_version() -> &'static str {
        REQ_RES_PROTOCOL_VERSION_GEN2
    }

    pub fn identify_protocol_version() -> &'static str {
        IDENTIFY_PROTOCOL_VERSION
    }

    /// Get kademlia bootstrap interval as Duration
    pub fn kademlia_bootstrap_interval(&self) -> Duration {
        Duration::from_secs(self.kademlia_bootstrap_interval_secs)
    }

    /// Get force peer dial interval as Duration
    pub fn force_peer_dial_interval(&self) -> Duration {
        Duration::from_secs(self.force_peer_dial_interval_secs)
    }

    /// Get swarm idle timeout as Duration
    pub fn swarm_idle_timeout(&self) -> Duration {
        Duration::from_secs(self.swarm_idle_timeout_secs)
    }

    /// Get keep alive interval as Duration
    pub fn keep_alive_interval(&self) -> Duration {
        Duration::from_secs(self.keep_alive_interval_secs)
    }

    /// Get handshake timeout as Duration
    pub fn handshake_timeout(&self) -> Duration {
        Duration::from_secs(self.handshake_timeout_secs)
    }

    /// Get identify interval as Duration
    pub fn identify_interval(&self) -> Duration {
        Duration::from_secs(self.identify_interval_secs)
    }

    /// Get request response timeout as Duration
    pub fn request_response_timeout(&self) -> Duration {
        Duration::from_secs(self.request_response_timeout_secs)
    }

    /// Get request high reset as Duration
    pub fn request_high_reset(&self) -> Duration {
        Duration::from_secs(self.request_high_reset_secs)
    }

    pub fn request_replay_cache_ttl(&self) -> Duration {
        Duration::from_secs(self.request_replay_cache_ttl_secs)
    }

    /// Get connection timeout (same as swarm idle timeout)
    pub fn connection_timeout(&self) -> Duration {
        self.swarm_idle_timeout()
    }

    /// Get max idle timeout in milliseconds for QUIC
    pub fn max_idle_timeout_millisecs(&self) -> u32 {
        self.connection_timeout().as_millis() as u32
    }

    /// Get request response max concurrent streams
    pub fn request_response_max_concurrent_streams(&self) -> usize {
        self.max_established_connections as usize * 8
    }

    pub fn gen2_batch_max_items(&self) -> usize {
        self.gen2_batch_max_items
    }

    pub fn gen2_batch_max_bytes(&self) -> usize {
        self.gen2_batch_max_bytes
    }

    pub fn gen2_item_max_bytes(&self) -> usize {
        self.gen2_item_max_bytes
    }

    pub fn gen2_block_batch_max_response_bytes(&self) -> usize {
        self.gen2_block_batch_max_response_bytes
    }

    pub fn gen2_block_batch_max_response_bytes_override_present() -> bool {
        std::env::var_os(GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES_ENV).is_some()
    }

    pub fn gen2_batch_coalesce_window(&self) -> Duration {
        Duration::from_millis(self.gen2_batch_coalesce_window_ms)
    }

    pub fn gen2_max_inflight_per_peer(&self) -> usize {
        self.gen2_max_inflight_per_peer
    }

    pub fn gen2_swarm_action_queue_capacity(&self) -> usize {
        self.gen2_swarm_action_queue_capacity
    }

    pub fn peer_status_interval_secs(&self) -> std::time::Duration {
        Duration::from_secs(self.peer_status_interval_secs)
    }

    pub fn elders_debounce_reset(&self) -> std::time::Duration {
        Duration::from_secs(self.elders_debounce_reset_secs)
    }

    pub fn seen_tx_clear_interval(&self) -> u64 {
        self.seen_tx_clear_interval
    }

    pub fn min_peers(&self) -> usize {
        self.min_peers
    }

    pub fn low_priority_peek_timeout(&self) -> Duration {
        Duration::from_secs(self.low_priority_peek_timeout_secs)
    }

    pub fn failed_pings_before_close(&self) -> u64 {
        self.failed_pings_before_close
    }

    pub(crate) fn peer_exclusion_config(&self) -> Result<PeerExclusionConfig, ConfigError> {
        PeerExclusionConfig::from_libp2p_config(self)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use crate::config::{LibP2PConfig, PeerExclusionConfig};

    #[test]
    fn peer_exclusion_config_parses_allow_ips_and_overrides() {
        let config = LibP2PConfig {
            ip_hygiene_enabled: false,
            address_cooldown_secs: 7,
            ip_exclusion_secs: 11,
            ip_extended_exclusion_secs: 13,
            evidence_window_secs: 17,
            ip_exclusion_history_secs: 19,
            permission_denied_cooldown_secs: 23,
            wrong_peer_id_ip_threshold: 3,
            dial_failure_ip_threshold: 5,
            same_ip_kad_entry_threshold: 8,
            max_auto_exclusion_secs: 29,
            max_exclusion_entries: 31,
            request_peer_cooldown_secs: 37,
            exclusion_allow_ips: String::from("203.0.113.1,2001:db8::1"),
            fail2ban_on_temp_exclusion: true,
            ..LibP2PConfig::default()
        };

        let exclusion =
            PeerExclusionConfig::from_libp2p_config(&config).expect("valid exclusion config");

        assert!(!exclusion.enabled);
        assert_eq!(exclusion.address_cooldown_secs, 7);
        assert_eq!(exclusion.max_exclusion_entries, 31);
        assert!(exclusion.fail2ban_on_temp_exclusion);
        assert!(exclusion
            .allow_ips
            .contains(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(exclusion.allow_ips.contains(&IpAddr::V6(
            "2001:db8::1".parse::<Ipv6Addr>().expect("valid ipv6")
        )));
    }

    #[test]
    fn peer_exclusion_config_rejects_invalid_allow_ip() {
        let config = LibP2PConfig {
            exclusion_allow_ips: String::from("203.0.113.1,nope"),
            ..LibP2PConfig::default()
        };

        assert!(PeerExclusionConfig::from_libp2p_config(&config).is_err());
    }
    #[test]
    fn test_req_res_protocol_versions_are_stable() {
        assert_eq!(
            LibP2PConfig::req_res_gen1_protocol_version(),
            "/nockchain-1-req-res"
        );
        assert_eq!(
            LibP2PConfig::req_res_gen2_protocol_version(),
            "/nockchain-2-req-res"
        );
        assert_eq!(
            LibP2PConfig::req_res_protocol_version(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
    }

    #[test]
    fn test_gen2_rollout_defaults() {
        let config = LibP2PConfig::default();

        assert!(config.req_res_gen2_accept_enabled);
        assert!(config.req_res_gen2_send_enabled);
        assert!(config.req_res_gen2_bundle_enabled);
        assert!(!config.req_res_authenticated_gossip_send_enabled);
        assert!(config.req_res_legacy_gossip_accept_enabled);
        assert_eq!(config.gen2_batch_max_items, 64);
        assert_eq!(config.gen2_batch_max_bytes, 10_000_000);
        assert_eq!(config.gen2_item_max_bytes, 10_000_000);
        assert_eq!(config.gen2_block_batch_max_response_bytes, 10_000_000);
        assert_eq!(config.gen2_batch_coalesce_window_ms, 10);
        assert_eq!(config.gen2_max_inflight_per_peer, 128);
        assert_eq!(config.gen2_swarm_action_queue_capacity, 1000);
        assert!(config.prefetch_enabled);
        assert_eq!(config.prefetch_window_initial, 16);
        assert_eq!(config.prefetch_window_max, 128);
        assert_eq!(config.prefetch_max_inflight_per_peer, 4);
        assert_eq!(config.low_priority_peek_timeout_secs, 180);
        assert_eq!(config.request_replay_cache_ttl_secs, 300);
        assert_eq!(config.request_replay_cache_max_per_peer, 4096);
    }

    #[test]
    fn test_gen2_rollout_flags_from_env() {
        std::env::set_var("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_ACCEPT_ENABLED", "true");
        std::env::set_var("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED", "true");
        std::env::set_var(
            "NOCKCHAIN_LIBP2P_REQ_RES_AUTHENTICATED_GOSSIP_SEND_ENABLED", "true",
        );
        std::env::set_var(
            "NOCKCHAIN_LIBP2P_REQ_RES_LEGACY_GOSSIP_ACCEPT_ENABLED", "false",
        );
        let result = LibP2PConfig::from_env();
        std::env::remove_var("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_ACCEPT_ENABLED");
        std::env::remove_var("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED");
        std::env::remove_var("NOCKCHAIN_LIBP2P_REQ_RES_AUTHENTICATED_GOSSIP_SEND_ENABLED");
        std::env::remove_var("NOCKCHAIN_LIBP2P_REQ_RES_LEGACY_GOSSIP_ACCEPT_ENABLED");

        let config = result.expect("env config should parse");
        assert!(
            config.req_res_gen2_accept_enabled,
            "gen2 accept should be enabled via NOCKCHAIN_LIBP2P_REQ_RES_GEN2_ACCEPT_ENABLED=true"
        );
        assert!(
            config.req_res_gen2_send_enabled,
            "gen2 send should be enabled via NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED=true"
        );
        assert!(config.req_res_authenticated_gossip_send_enabled);
        assert!(!config.req_res_legacy_gossip_accept_enabled);
    }

    #[test]
    fn test_gen2_block_batch_max_response_bytes_from_env() {
        std::env::set_var(
            "NOCKCHAIN_LIBP2P_GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES", "262144",
        );
        assert!(LibP2PConfig::gen2_block_batch_max_response_bytes_override_present());
        let config = LibP2PConfig::from_env().expect("env config should parse");
        std::env::remove_var("NOCKCHAIN_LIBP2P_GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES");

        assert_eq!(config.gen2_block_batch_max_response_bytes, 262_144);
        assert!(!LibP2PConfig::gen2_block_batch_max_response_bytes_override_present());
    }
}
