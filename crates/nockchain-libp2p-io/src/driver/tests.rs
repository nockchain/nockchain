use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::future::Future;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Instant;

use chaff::Chaff;
use either::Right;
use futures::StreamExt;
use kernels_open_dumb::KERNEL;
use libp2p::swarm::{ConnectionId, SwarmEvent};
use libp2p::{request_response, Multiaddr, PeerId};
use nockapp::driver::{IOAction, NockAppHandle, PokeResult};
use nockapp::kernel::boot;
use nockapp::nockapp::test::setup_nockapp;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::make_tas;
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, NockApp};
use nockchain::setup::{self, SetupCommand};
use nockchain_types::default_fakenet_blockchain_constants;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;
use noun_serde::NounEncode;
use serde::Serialize;
use serde_bytes::ByteBuf;
use tempfile::TempDir;
use zkvm_jetpack::hot::produce_prover_hot_state;

use super::gen1::*;
use super::gen2::*;
use super::*;
use crate::messages::{
    block_by_height_message, block_range_with_txs_request_message, request_slab_from_message,
    BatchErrorClass, BatchRequestItem, BatchResultItem, BatchResultStatus, BundledBlockWithTxs,
    BundledTxEnvelope, EnvelopeKind, NockchainDataRequest, NockchainFact, ResponseEnvelope,
};
use crate::p2p_state::BlockSource;
use crate::peer_stats::{PeerReqResGeneration, PeerStatsRegistry};
use crate::test_support::{
    build_req_res_test_swarm, bundled_block_for_height, first_common_outbound_protocol,
    jam_heard_tx_response, solve_authenticated_gossip, ReqResTestEvent, ReqResTestSwarm,
};
use crate::tip5_util::{tip5_hash_to_base58, TIP5_BASE58_MAX_CHARS};

pub static LIBP2P_CONFIG: LazyLock<LibP2PConfig> = LazyLock::new(LibP2PConfig::default);

async fn handle_effect(
    noun_slab: NounSlab,
    swarm_tx: mpsc::Sender<SwarmAction>,
    connected_peers: Vec<PeerId>,
    bundle_requests_enabled: bool,
    driver_state: Arc<Mutex<P2PState>>,
    metrics: Arc<NockchainP2PMetrics>,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(&swarm_tx);
    handle_effect_with_dispatcher(
        noun_slab,
        &mut swarm_actions,
        connected_peers,
        bundle_requests_enabled,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        driver_state,
        metrics,
        PeerExclusions::default(),
    )
    .await
}

#[derive(Clone)]
struct DriverTranscript {
    lines: Arc<StdMutex<Vec<String>>>,
    echo: bool,
}

impl DriverTranscript {
    fn silent() -> Self {
        Self {
            lines: Arc::new(StdMutex::new(Vec::new())),
            echo: false,
        }
    }

    fn record(&self, actor: &str, message: impl Into<String>) {
        let line = format!("{actor}: {}", message.into());
        if self.echo {
            println!("{line}");
        }
        self.lines.lock().unwrap().push(line);
    }

    fn render(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

impl Default for DriverTranscript {
    fn default() -> Self {
        Self {
            lines: Arc::new(StdMutex::new(Vec::new())),
            echo: true,
        }
    }
}

#[test]
fn should_redial_initial_peers_requires_zero_connected_peers() {
    let initial_peers: Vec<Multiaddr> = vec![
        "/ip4/127.0.0.1/udp/30001/quic-v1/p2p/12D3KooWQb2uWwR7C3yFqKf4LxQ7s7rC5QZr9dA4zD4m6J7QfJ6A"
            .parse()
            .expect("valid multiaddr"),
    ];

    assert!(should_redial_initial_peers(0, &initial_peers, &[]));
    assert!(!should_redial_initial_peers(1, &initial_peers, &[]));
}

#[test]
fn should_redial_initial_peers_requires_seed_peers() {
    let initial_peers: Vec<Multiaddr> = vec![
        "/ip4/127.0.0.1/udp/30001/quic-v1/p2p/12D3KooWQb2uWwR7C3yFqKf4LxQ7s7rC5QZr9dA4zD4m6J7QfJ6A"
            .parse()
            .expect("valid multiaddr"),
    ];

    // No seed or backbone peers: nothing to redial.
    assert!(!should_redial_initial_peers(0, &[], &[]));
    // With seed peers and zero connected peers we redial regardless of how
    // many attempts have already been made — we never give up.
    assert!(should_redial_initial_peers(0, &initial_peers, &[]));
    // Backbone peers alone are enough to trigger a redial.
    assert!(should_redial_initial_peers(0, &[], &initial_peers));
}

#[test]
fn next_backbone_window_rotates_round_robin() {
    let pool: Vec<Multiaddr> = (0..5)
        .map(|i| {
            format!("/ip4/127.0.0.1/udp/{}/quic-v1", 30000 + i)
                .parse()
                .expect("valid multiaddr")
        })
        .collect();

    let mut cursor = 0usize;
    // First window: indices 0,1,2.
    assert_eq!(
        next_backbone_window(&pool, &mut cursor, 3),
        vec![pool[0].clone(), pool[1].clone(), pool[2].clone()]
    );
    // Second window advances past the first and wraps: indices 3,4,0.
    assert_eq!(
        next_backbone_window(&pool, &mut cursor, 3),
        vec![pool[3].clone(), pool[4].clone(), pool[0].clone()]
    );
    // Third window continues from index 1: 1,2,3.
    assert_eq!(
        next_backbone_window(&pool, &mut cursor, 3),
        vec![pool[1].clone(), pool[2].clone(), pool[3].clone()]
    );
}

#[test]
fn next_backbone_window_handles_empty_and_small_pools() {
    let mut cursor = 0usize;
    assert!(next_backbone_window(&[], &mut cursor, 3).is_empty());

    let pool: Vec<Multiaddr> = vec!["/ip4/127.0.0.1/udp/30000/quic-v1"
        .parse()
        .expect("valid multiaddr")];
    // Count larger than the pool returns the whole pool without duplicating.
    assert_eq!(next_backbone_window(&pool, &mut cursor, 3), pool);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponderHintEstimatorKind {
    ObservedMax,
    DecayingMax75,
    RecentMaxWindow8,
    OracleActual,
}

impl ResponderHintEstimatorKind {
    fn label(self) -> &'static str {
        match self {
            Self::ObservedMax => "observed_max",
            Self::DecayingMax75 => "decaying_max_75pct",
            Self::RecentMaxWindow8 => "recent_max_window_8",
            Self::OracleActual => "oracle_actual",
        }
    }
}

#[derive(Debug)]
struct ResponderHintEstimator {
    kind: ResponderHintEstimatorKind,
    fallback_message_bytes: usize,
    observed_max: Option<usize>,
    decaying_max: Option<usize>,
    recent_window: VecDeque<usize>,
}

impl ResponderHintEstimator {
    fn new(
        kind: ResponderHintEstimatorKind,
        fallback_message_bytes: usize,
        seed_hint_message_bytes: Option<usize>,
    ) -> Self {
        let mut estimator = Self {
            kind,
            fallback_message_bytes,
            observed_max: None,
            decaying_max: None,
            recent_window: VecDeque::new(),
        };
        if let Some(seed_hint_message_bytes) = seed_hint_message_bytes {
            estimator.record_observation(seed_hint_message_bytes);
        }
        estimator
    }

    fn estimate_message_bytes(&self, actual_message_bytes: usize) -> (usize, &'static str) {
        match self.kind {
            ResponderHintEstimatorKind::ObservedMax => self
                .observed_max
                .map(|message_bytes| (message_bytes, "observed_max"))
                .unwrap_or((self.fallback_message_bytes, "configured_fallback")),
            ResponderHintEstimatorKind::DecayingMax75 => self
                .decaying_max
                .map(|message_bytes| (message_bytes, "decaying_max_75pct"))
                .unwrap_or((self.fallback_message_bytes, "configured_fallback")),
            ResponderHintEstimatorKind::RecentMaxWindow8 => self
                .recent_window
                .iter()
                .copied()
                .max()
                .map(|message_bytes| (message_bytes, "recent_max_window_8"))
                .unwrap_or((self.fallback_message_bytes, "configured_fallback")),
            ResponderHintEstimatorKind::OracleActual => (actual_message_bytes, "oracle_actual"),
        }
    }

    fn record_observation(&mut self, observed_message_bytes: usize) {
        match self.kind {
            ResponderHintEstimatorKind::ObservedMax => {
                self.observed_max = Some(
                    self.observed_max
                        .unwrap_or(observed_message_bytes)
                        .max(observed_message_bytes),
                );
            }
            ResponderHintEstimatorKind::DecayingMax75 => {
                let decayed = self
                    .decaying_max
                    .map(|current| current.saturating_mul(3).div_ceil(4))
                    .unwrap_or(observed_message_bytes);
                self.decaying_max = Some(decayed.max(observed_message_bytes));
            }
            ResponderHintEstimatorKind::RecentMaxWindow8 => {
                self.recent_window.push_back(observed_message_bytes);
                if self.recent_window.len() > 8 {
                    let _ = self.recent_window.pop_front();
                }
            }
            ResponderHintEstimatorKind::OracleActual => {}
        }
    }

    fn current_message_bytes(&self) -> Option<usize> {
        match self.kind {
            ResponderHintEstimatorKind::ObservedMax => self.observed_max,
            ResponderHintEstimatorKind::DecayingMax75 => self.decaying_max,
            ResponderHintEstimatorKind::RecentMaxWindow8 => {
                self.recent_window.iter().copied().max()
            }
            ResponderHintEstimatorKind::OracleActual => None,
        }
    }
}

struct ResponderPayloadFitScenarioDef {
    label: &'static str,
    request_mix: &'static str,
    payload_lens: Vec<usize>,
    seed_hint_message_bytes: usize,
    response_cap_bytes: usize,
}

#[derive(Debug)]
struct ResponderPayloadFitRun {
    estimator: ResponderHintEstimatorKind,
    estimate_source: String,
    starting_estimate_message_bytes: Option<usize>,
    ending_estimate_message_bytes: Option<usize>,
    response_bytes: usize,
    result_items: usize,
    not_found_items: usize,
    backpressure_items: usize,
    too_large_items: usize,
    stop_reason: String,
}

#[derive(Serialize)]
struct ResponderPayloadFitHeuristicSample {
    estimator: String,
    estimate_source: String,
    starting_estimate_message_bytes: Option<usize>,
    ending_estimate_message_bytes: Option<usize>,
    response_bytes: usize,
    result_items: usize,
    not_found_items: usize,
    backpressure_items: usize,
    too_large_items: usize,
    stop_reason: String,
    fit_loss_items: usize,
    fit_loss_response_bytes: usize,
    cap_headroom_bytes: usize,
    cap_utilization_ratio: f64,
}

#[derive(Serialize)]
struct ResponderPayloadFitScenarioSample {
    label: String,
    request_mix: String,
    item_count: usize,
    response_cap_bytes: usize,
    seed_hint_message_bytes: usize,
    min_actual_message_bytes: usize,
    p50_actual_message_bytes: usize,
    max_actual_message_bytes: usize,
    actual_fit_response_bytes: usize,
    actual_fit_result_items: usize,
    actual_fit_stop_reason: String,
    heuristic_samples: Vec<ResponderPayloadFitHeuristicSample>,
}

#[derive(Serialize)]
struct ResponderPayloadFitReport {
    schema_version: &'static str,
    scenario: &'static str,
    batch_max_bytes: usize,
    item_max_bytes: usize,
    samples: Vec<ResponderPayloadFitScenarioSample>,
}

#[derive(Serialize)]
struct RequesterCostSample {
    label: String,
    request_mix: String,
    item_count: usize,
    requested_item_count: Option<usize>,
    response_bytes: usize,
    total_ms: f64,
    per_item_us: f64,
    poke_count: usize,
    followup_request_count: usize,
    timed_out: bool,
    timeout_seconds: Option<u64>,
    window_start_height: Option<u64>,
    window_end_height: Option<u64>,
}

#[derive(Serialize)]
struct RequesterCostReport {
    schema_version: &'static str,
    scenario: &'static str,
    batch_max_bytes: usize,
    block_response_budget_bytes: usize,
    item_max_bytes: usize,
    samples: Vec<RequesterCostSample>,
}

#[derive(Serialize)]
struct CheckpointRequesterProfileSample {
    label: String,
    replay_blocks: usize,
    requested_item_count: usize,
    response_bytes: usize,
    window_start_height: u64,
    window_end_height: u64,
    first_height: u64,
    first_message_bytes: usize,
    first_item_decode_ms: f64,
    first_item_clone_ms: f64,
    first_item_gate_us: f64,
    first_item_poke_ms: f64,
    first_item_poke_timed_out: bool,
    first_item_poke_error: Option<String>,
}

#[derive(Serialize)]
struct CheckpointRequesterProfileReport {
    schema_version: &'static str,
    scenario: &'static str,
    batch_max_bytes: usize,
    block_response_budget_bytes: usize,
    item_max_bytes: usize,
    poke_timeout_seconds: u64,
    samples: Vec<CheckpointRequesterProfileSample>,
}

#[derive(Serialize)]
struct CheckpointRangeScryLatencySample {
    label: String,
    start_height: u64,
    end_height: u64,
    len: u8,
    cold_ms: f64,
    warm_runs: usize,
    warm_p50_ms: f64,
    warm_p95_ms: f64,
    warm_max_ms: f64,
    result_jam_bytes: usize,
    returned_some: bool,
}

#[derive(Serialize)]
struct CheckpointRangeScryLatencyReport {
    schema_version: &'static str,
    scenario: &'static str,
    checkpoint_path: String,
    head_height: u64,
    samples: Vec<CheckpointRangeScryLatencySample>,
}

#[derive(Serialize)]
struct ResidentSetSample {
    label: String,
    topology: String,
    generation: String,
    request_mix: String,
    item_count: usize,
    payload_len: usize,
    response_bytes: usize,
    total_ms: f64,
    per_item_us: f64,
    poke_count: usize,
    rss_before_kib: u64,
    rss_after_kib: u64,
    rss_peak_kib: u64,
    rss_delta_kib: u64,
}

#[derive(Serialize)]
struct ResidentSetReport {
    schema_version: &'static str,
    scenario: &'static str,
    batch_max_bytes: usize,
    item_max_bytes: usize,
    samples: Vec<ResidentSetSample>,
}

#[derive(Clone, Serialize)]
struct TwoPeerLatencySample {
    label: String,
    topology: String,
    generation: String,
    request_mix: String,
    item_count: usize,
    payload_len: usize,
    response_bytes: usize,
    total_ms: f64,
    per_item_ms: f64,
    protocol: String,
    peek_count: usize,
}

#[derive(Serialize)]
struct TwoPeerLatencyReport {
    schema_version: &'static str,
    scenario: &'static str,
    batch_max_bytes: usize,
    item_max_bytes: usize,
    samples: Vec<TwoPeerLatencySample>,
}

#[derive(Clone, Serialize)]
struct CheckpointLargestBlockSample {
    height: u64,
    response_message_bytes: usize,
}

#[derive(Serialize)]
struct CheckpointSizingReport {
    schema_version: &'static str,
    scenario: &'static str,
    checkpoint_path: String,
    sampled_height_start: u64,
    sampled_height_end: u64,
    sampled_block_count: usize,
    nominal_target_blocks: usize,
    batch_max_bytes: usize,
    block_response_budget_bytes: usize,
    fallback_item_max_bytes: usize,
    min_response_message_bytes: usize,
    p50_response_message_bytes: usize,
    p90_response_message_bytes: usize,
    p99_response_message_bytes: usize,
    max_response_message_bytes: usize,
    average_response_message_bytes: f64,
    min_blocks_fit_per_window: usize,
    p50_blocks_fit_per_window: usize,
    max_blocks_fit_per_window: usize,
    min_window_response_bytes: usize,
    p50_window_response_bytes: usize,
    p95_window_response_bytes: usize,
    max_window_response_bytes: usize,
    average_window_response_bytes: f64,
    average_window_response_fill_ratio: f64,
    p50_window_response_fill_ratio: f64,
    p95_window_response_fill_ratio: f64,
    max_window_response_fill_ratio: f64,
    all_windows_fit_nominal_target: bool,
    worst_window_start_height: u64,
    worst_window_end_height: u64,
    worst_window_response_bytes: usize,
    largest_blocks: Vec<CheckpointLargestBlockSample>,
}

#[derive(Clone, Serialize)]
struct RecoveryEnqueueSample {
    label: String,
    request_mix: String,
    stage_count: usize,
    input_request_count: usize,
    unique_request_count: usize,
    duplicate_requests: usize,
    outbound_request_count: usize,
    gen2_batch_request_count: usize,
    gen1_request_count: usize,
    single_item_batch_count: usize,
    min_outbound_items: usize,
    p50_outbound_items: usize,
    max_outbound_items: usize,
    average_outbound_items: f64,
    total_ms: f64,
}

#[derive(Clone, Serialize)]
struct RecoveryResponseSample {
    label: String,
    request_mix: String,
    wave_count: usize,
    input_item_count: usize,
    useful_pokes: usize,
    duplicate_gates: usize,
    kernel_effect_count: usize,
    followup_request_count: usize,
    response_bytes: usize,
    total_ms: f64,
    per_item_us: f64,
}

#[derive(Clone)]
struct RecoveryTimedRequest {
    at_ms: u64,
    peer_index: usize,
    message: Vec<u8>,
    estimated_response_bytes: usize,
    contains_response_budget_item: bool,
}

struct RecoveryTuningWorkload {
    label: &'static str,
    request_mix: &'static str,
    events: Vec<RecoveryTimedRequest>,
}

#[derive(Clone, Serialize)]
struct RecoveryTuningConfig {
    coalesce_window_ms: u64,
    batch_max_items: usize,
    batch_max_bytes: usize,
    item_max_bytes: usize,
    block_response_budget_bytes: usize,
    max_inflight_per_peer: usize,
}

#[derive(Clone, Serialize)]
struct RecoveryTuningSample {
    label: String,
    request_mix: String,
    coalesce_window_ms: u64,
    batch_max_items: usize,
    input_request_count: usize,
    unique_request_count: usize,
    duplicate_requests: usize,
    outbound_request_count: usize,
    gen2_batch_request_count: usize,
    gen1_request_count: usize,
    single_item_batch_count: usize,
    flush_reason_histogram: BTreeMap<String, usize>,
    min_outbound_items: usize,
    p50_outbound_items: usize,
    p95_outbound_items: usize,
    max_outbound_items: usize,
    average_outbound_items: f64,
    p50_added_delay_ms: f64,
    p95_added_delay_ms: f64,
    max_added_delay_ms: f64,
    average_added_delay_ms: f64,
    total_simulated_ms: u64,
    average_response_fill_ratio: f64,
    p95_response_fill_ratio: f64,
    max_response_fill_ratio: f64,
    average_payload_fill_ratio: f64,
    p95_payload_fill_ratio: f64,
    max_payload_fill_ratio: f64,
}

#[derive(Serialize)]
struct RecoveryPathReport {
    schema_version: &'static str,
    scenario: &'static str,
    enqueue_samples: Vec<RecoveryEnqueueSample>,
    response_samples: Vec<RecoveryResponseSample>,
    tuning_config: Vec<RecoveryTuningConfig>,
    tuning_samples: Vec<RecoveryTuningSample>,
}

fn maybe_write_report_json<T: Serialize>(report: &T) {
    let Ok(path) = std::env::var("REQ_RES_GEN2_REPORT_JSON") else {
        return;
    };
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent).expect("benchmark report parent directory should exist");
    }
    let json = serde_json::to_vec_pretty(report).expect("benchmark report should serialize");
    fs::write(&path, json).expect("benchmark report should write");
    println!("json report: {path}");
}

fn current_rss_kib() -> u64 {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .expect("ps should be available for rss sampling");
    assert!(
        output.status.success(),
        "ps rss sampling failed with status {:?}",
        output.status.code()
    );
    String::from_utf8(output.stdout)
        .expect("ps rss output should be utf8")
        .trim()
        .parse::<u64>()
        .expect("ps rss output should parse as kib")
}

struct CheckpointApp {
    _home: TempDir,
    app: NockApp<Chaff>,
}

fn checkpoint_path_for_report() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("REQ_RES_GEN2_CHECKPOINT_PATH") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let home = std::env::var("HOME").ok()?;
    [
        format!("{home}/gwe/oct-21-jams/0.chkjam"),
        format!("{home}/gwe/oct-21-jams/1.chkjam"),
        format!("{home}/gwe/oct-21-jams/0-001.chkjam"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64_list(name: &str, default: &[u64]) -> Vec<u64> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter_map(|part| part.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| default.to_vec())
}

fn env_usize_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .filter_map(|part| part.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| default.to_vec())
}

fn report_libp2p_config() -> LibP2PConfig {
    LibP2PConfig::from_env().expect("benchmark report libp2p config should parse")
}

fn checkpoint_stack_size_for_report() -> boot::NockStackSize {
    match std::env::var("REQ_RES_GEN2_CHECKPOINT_STACK_SIZE")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("tiny") => boot::NockStackSize::Tiny,
            Some("small") => boot::NockStackSize::Small,
            Some("normal") => boot::NockStackSize::Normal,
            Some("medium") => boot::NockStackSize::Medium,
            Some("large") => boot::NockStackSize::Large,
            Some("huge") => boot::NockStackSize::Huge,
            Some(other) => panic!(
                "invalid REQ_RES_GEN2_CHECKPOINT_STACK_SIZE={other}; expected tiny|small|normal|medium|large|huge"
            ),
            None => boot::NockStackSize::Large,
        }
}

async fn fetch_block_response_message(
    app: &mut NockApp<Chaff>,
    height: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let Some(scry_res_slab) = app
        .peek_handle(request_to_scry_slab(NockchainDataRequest::BlockByHeight(
            height,
        ))?)
        .await?
    else {
        return Err(format!("block {height} was not present in checkpoint").into());
    };

    let mut response_slab: NounSlab<NockJammer> = NounSlab::new();
    let space = scry_res_slab.noun_space();
    let payload = unsafe { *scry_res_slab.root() }.in_space(&space);
    match create_response_result_from_payload(payload, "heard-block", &mut response_slab) {
        Ok(NockchainResponse::Result { message }) => Ok(message.to_vec()),
        Ok(other) => {
            Err(format!("unexpected response shape for height {height}: {other:?}").into())
        }
        Err(err) => Err(Box::new(err)),
    }
}

async fn start_checkpoint_app(chkjam_path: &Path) -> CheckpointApp {
    try_start_checkpoint_app(chkjam_path)
        .await
        .unwrap_or_else(|err| panic!("checkpoint app should boot: {err}"))
}

async fn try_start_checkpoint_app(chkjam_path: &Path) -> Result<CheckpointApp, String> {
    let home = TempDir::new().expect("temp home should create");
    let mut cli = boot::default_boot_cli(true);
    cli.disable_fsync = true;
    cli.gc_interval = None;
    cli.rotating_snapshot_interval_event_time = None;
    cli.bootstrap_from_chkjam = Some(
        chkjam_path
            .to_str()
            .expect("checkpoint path should be valid utf8")
            .to_owned(),
    );
    cli.stack_size = checkpoint_stack_size_for_report();

    let hot_state = produce_prover_hot_state();
    let app = boot::setup::<Chaff>(
        KERNEL,
        cli,
        hot_state.as_slice(),
        "checkpoint-sizing-report",
        Some(home.path().to_path_buf()),
    )
    .await
    .map_err(|err| format!("{err:?}"))?;
    Ok(CheckpointApp { _home: home, app })
}

async fn start_nockchain_app() -> CheckpointApp {
    let home = TempDir::new().expect("temp home should create");
    let cli = boot::ephemeral_test_boot_cli(true);

    let hot_state = produce_prover_hot_state();
    let app = boot::setup::<Chaff>(
        KERNEL,
        cli,
        hot_state.as_slice(),
        "route-response-fact-regression",
        Some(home.path().to_path_buf()),
    )
    .await
    .expect("nockchain app should boot");
    CheckpointApp { _home: home, app }
}

async fn checkpoint_has_block(
    app: &mut NockApp<Chaff>,
    height: u64,
) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(app
        .peek_handle(request_to_scry_slab(NockchainDataRequest::BlockByHeight(
            height,
        ))?)
        .await?
        .is_some())
}

async fn discover_checkpoint_head_height(
    app: &mut NockApp<Chaff>,
) -> Result<u64, Box<dyn std::error::Error>> {
    if !checkpoint_has_block(app, 0).await? {
        return Err("checkpoint did not expose the genesis block".into());
    }

    let mut lower = 0u64;
    let mut upper = 1u64;
    while checkpoint_has_block(app, upper).await? {
        lower = upper;
        upper = upper.saturating_mul(2);
        if upper == lower {
            return Ok(lower);
        }
    }

    while lower + 1 < upper {
        let mid = lower + (upper - lower) / 2;
        if checkpoint_has_block(app, mid).await? {
            lower = mid;
        } else {
            upper = mid;
        }
    }

    Ok(lower)
}

async fn load_heaviest_checkpoint_block_window(
    chkjam_path: &Path,
    nominal_target_blocks: usize,
    sample_blocks: usize,
    max_response_bytes: usize,
) -> Vec<(u64, Vec<u8>)> {
    let mut checkpoint_app = start_checkpoint_app(chkjam_path).await;
    let head_height = discover_checkpoint_head_height(&mut checkpoint_app.app)
        .await
        .expect("checkpoint head height should resolve");
    let sampled_height_start = head_height.saturating_sub(sample_blocks.saturating_sub(1) as u64);
    let mut heights_and_messages = Vec::with_capacity(sample_blocks);
    for height in sampled_height_start..=head_height {
        let message = fetch_block_response_message(&mut checkpoint_app.app, height)
            .await
            .unwrap_or_else(|err| panic!("failed to fetch checkpoint block {height}: {err}"));
        heights_and_messages.push((height, message));
    }
    assert!(
            heights_and_messages.len() >= nominal_target_blocks,
            "checkpoint requester-cost report requires at least {nominal_target_blocks} sampled blocks, found {}",
            heights_and_messages.len()
        );

    let Some((window_idx, fit_blocks, _response_bytes)) = heights_and_messages
        .windows(nominal_target_blocks)
        .enumerate()
        .filter_map(|(idx, window)| {
            let (fit_blocks, response_bytes) =
                fit_prefix_of_block_messages(window, max_response_bytes, nominal_target_blocks);
            (fit_blocks > 0).then_some((idx, fit_blocks, response_bytes))
        })
        .max_by_key(|(idx, fit_blocks, response_bytes)| (*response_bytes, *fit_blocks, *idx))
    else {
        panic!(
                "checkpoint requester-cost report could not find any admissible block-response window under the configured {} byte response budget",
                max_response_bytes
            );
    };

    heights_and_messages[window_idx..window_idx + fit_blocks].to_vec()
}

fn percentile(sorted: &[usize], percentile: f64) -> usize {
    let idx = ((sorted.len().saturating_sub(1) as f64) * percentile).round() as usize;
    sorted[idx]
}

fn fit_prefix_of_block_messages(
    heights_and_messages: &[(u64, Vec<u8>)],
    cap_bytes: usize,
    nominal_target_blocks: usize,
) -> (usize, usize) {
    let mut results = Vec::new();
    let mut response_bytes = 0usize;
    for (idx, (_, message)) in heights_and_messages
        .iter()
        .take(nominal_target_blocks)
        .enumerate()
    {
        let envelope = response_envelope_from_result_message(message)
            .expect("checkpoint response message should decode into an envelope");
        let result = BatchResultItem {
            item_id: idx as u32 + 1,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(envelope),
        };
        let mut candidate = results.clone();
        candidate.push(result);
        let candidate_bytes =
            batch_result_encoded_bytes(&candidate).expect("batch result should encode");
        if candidate_bytes > cap_bytes {
            break;
        }
        response_bytes = candidate_bytes;
        results = candidate;
    }
    (results.len(), response_bytes)
}

struct ScriptedTrafficCopHarness {
    _script_task: tokio::task::JoinHandle<()>,
    _join_set: TrackedJoinSet<Result<(), NockAppError>>,
    traffic: traffic_cop::TrafficCop,
    peek_count: Arc<AtomicUsize>,
    poke_count: Arc<AtomicUsize>,
}

struct LiveTrafficCopHarness {
    _home: TempDir,
    _run_task: tokio::task::JoinHandle<()>,
    _join_set: TrackedJoinSet<Result<(), NockAppError>>,
    traffic: traffic_cop::TrafficCop,
    effect_handle: NockAppHandle,
}

impl Drop for LiveTrafficCopHarness {
    fn drop(&mut self) {
        self._run_task.abort();
    }
}

async fn build_scripted_traffic_cop(
    transcript: DriverTranscript,
    peek_results: Vec<Option<NounSlab>>,
    poke_results: Vec<PokeResult>,
) -> ScriptedTrafficCopHarness {
    let (_temp, app) = setup_nockapp("test-ker.jam").await;
    let base_handle = app.get_handle();
    let (io_sender, mut io_receiver) = tokio::sync::mpsc::channel(16);
    let effect_sender = base_handle.effect_sender.clone();
    let handle = NockAppHandle {
        io_sender,
        effect_sender: effect_sender.clone(),
        effect_receiver: tokio::sync::Mutex::new(effect_sender.subscribe()),
        metrics: base_handle.metrics.clone(),
        exit: base_handle.exit.clone(),
    };

    let peek_count = Arc::new(AtomicUsize::new(0));
    let poke_count = Arc::new(AtomicUsize::new(0));
    let peek_count_task = Arc::clone(&peek_count);
    let poke_count_task = Arc::clone(&poke_count);

    let script_task = tokio::spawn(async move {
        let mut scripted_peeks = VecDeque::from(peek_results);
        let mut scripted_pokes = VecDeque::from(poke_results);
        while let Some(action) = io_receiver.recv().await {
            match action {
                IOAction::Peek {
                    path: _,
                    result_channel,
                } => {
                    let next = scripted_peeks.pop_front().flatten();
                    let observed = peek_count_task.fetch_add(1, Ordering::SeqCst) + 1;
                    transcript.record(
                        "scripted-kernel",
                        format!(
                            "peek #{observed} -> {}",
                            if next.is_some() { "some" } else { "none" }
                        ),
                    );
                    let _ = result_channel.send(next);
                }
                IOAction::Poke {
                    wire,
                    poke: _,
                    ack_channel,
                    timeout: _,
                } => {
                    let result = scripted_pokes.pop_front().unwrap_or(PokeResult::Ack);
                    let observed = poke_count_task.fetch_add(1, Ordering::SeqCst) + 1;
                    transcript.record(
                        "scripted-kernel",
                        format!("poke #{observed} wire={wire:?} -> {result:?}"),
                    );
                    let _ = ack_channel.send(result);
                }
            }
        }
    });

    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let traffic = traffic_cop::TrafficCop::new(handle, &mut join_set, Duration::from_secs(1));
    ScriptedTrafficCopHarness {
        _script_task: script_task,
        _join_set: join_set,
        traffic,
        peek_count,
        poke_count,
    }
}

async fn build_scripted_traffic_cop_with_delay(
    transcript: DriverTranscript,
    peek_results: Vec<Option<NounSlab>>,
    poke_results: Vec<PokeResult>,
    peek_delay: Duration,
) -> ScriptedTrafficCopHarness {
    let (_temp, app) = setup_nockapp("test-ker.jam").await;
    let base_handle = app.get_handle();
    let (io_sender, mut io_receiver) = tokio::sync::mpsc::channel(16);
    let effect_sender = base_handle.effect_sender.clone();
    let handle = NockAppHandle {
        io_sender,
        effect_sender: effect_sender.clone(),
        effect_receiver: tokio::sync::Mutex::new(effect_sender.subscribe()),
        metrics: base_handle.metrics.clone(),
        exit: base_handle.exit.clone(),
    };

    let peek_count = Arc::new(AtomicUsize::new(0));
    let poke_count = Arc::new(AtomicUsize::new(0));
    let peek_count_task = Arc::clone(&peek_count);
    let poke_count_task = Arc::clone(&poke_count);

    let script_task = tokio::spawn(async move {
        let mut scripted_peeks = VecDeque::from(peek_results);
        let mut scripted_pokes = VecDeque::from(poke_results);
        while let Some(action) = io_receiver.recv().await {
            match action {
                IOAction::Peek {
                    path: _,
                    result_channel,
                } => {
                    if !peek_delay.is_zero() {
                        tokio::time::sleep(peek_delay).await;
                    }
                    let next = scripted_peeks.pop_front().flatten();
                    let observed = peek_count_task.fetch_add(1, Ordering::SeqCst) + 1;
                    transcript.record(
                        "scripted-kernel",
                        format!(
                            "peek #{observed} -> {} (delay={}ms)",
                            if next.is_some() { "some" } else { "none" },
                            peek_delay.as_millis()
                        ),
                    );
                    let _ = result_channel.send(next);
                }
                IOAction::Poke {
                    wire,
                    poke: _,
                    ack_channel,
                    timeout: _,
                } => {
                    let result = scripted_pokes.pop_front().unwrap_or(PokeResult::Ack);
                    let observed = poke_count_task.fetch_add(1, Ordering::SeqCst) + 1;
                    transcript.record(
                        "scripted-kernel",
                        format!("poke #{observed} wire={wire:?} -> {result:?}"),
                    );
                    let _ = ack_channel.send(result);
                }
            }
        }
    });

    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let traffic = traffic_cop::TrafficCop::new(handle, &mut join_set, Duration::from_secs(30));
    ScriptedTrafficCopHarness {
        _script_task: script_task,
        _join_set: join_set,
        traffic,
        peek_count,
        poke_count,
    }
}

async fn build_scripted_traffic_cop_with_dropped_peek_reply(
    transcript: DriverTranscript,
) -> ScriptedTrafficCopHarness {
    let (_temp, app) = setup_nockapp("test-ker.jam").await;
    let base_handle = app.get_handle();
    let (io_sender, mut io_receiver) = tokio::sync::mpsc::channel(16);
    let effect_sender = base_handle.effect_sender.clone();
    let handle = NockAppHandle {
        io_sender,
        effect_sender: effect_sender.clone(),
        effect_receiver: tokio::sync::Mutex::new(effect_sender.subscribe()),
        metrics: base_handle.metrics.clone(),
        exit: base_handle.exit.clone(),
    };

    let peek_count = Arc::new(AtomicUsize::new(0));
    let poke_count = Arc::new(AtomicUsize::new(0));
    let peek_count_task = Arc::clone(&peek_count);

    let script_task = tokio::spawn(async move {
        while let Some(action) = io_receiver.recv().await {
            match action {
                IOAction::Peek {
                    path: _,
                    result_channel,
                } => {
                    let observed = peek_count_task.fetch_add(1, Ordering::SeqCst) + 1;
                    transcript.record("scripted-kernel", format!("peek #{observed} -> drop-reply"));
                    drop(result_channel);
                }
                IOAction::Poke {
                    wire,
                    poke: _,
                    ack_channel,
                    timeout: _,
                } => {
                    transcript.record("scripted-kernel", format!("unexpected poke wire={wire:?}"));
                    let _ = ack_channel.send(PokeResult::Ack);
                }
            }
        }
    });

    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let traffic = traffic_cop::TrafficCop::new(handle, &mut join_set, Duration::from_secs(1));
    ScriptedTrafficCopHarness {
        _script_task: script_task,
        _join_set: join_set,
        traffic,
        peek_count,
        poke_count,
    }
}

fn build_live_traffic_cop(checkpoint_app: CheckpointApp) -> LiveTrafficCopHarness {
    let CheckpointApp { _home, mut app } = checkpoint_app;
    let handle = app.get_handle();
    let (traffic_handle, effect_handle) = handle.dup();
    let run_task = tokio::spawn(async move {
        let _ = app.run().await;
    });
    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let traffic = traffic_cop::TrafficCop::new_with_peek_timeout(
        traffic_handle,
        &mut join_set,
        Duration::from_secs(30),
    );
    LiveTrafficCopHarness {
        _home,
        _run_task: run_task,
        _join_set: join_set,
        traffic,
        effect_handle,
    }
}

async fn build_live_traffic_cop_with_test_kernel() -> LiveTrafficCopHarness {
    let (_home, mut app) = setup_nockapp("test-ker.jam").await;
    let handle = app.get_handle();
    let (traffic_handle, effect_handle) = handle.dup();
    let run_task = tokio::spawn(async move {
        let _ = app.run().await;
    });
    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let traffic = traffic_cop::TrafficCop::new_with_peek_timeout(
        traffic_handle,
        &mut join_set,
        Duration::from_secs(30),
    );
    LiveTrafficCopHarness {
        _home,
        _run_task: run_task,
        _join_set: join_set,
        traffic,
        effect_handle,
    }
}

fn state_peek_slab() -> NounSlab {
    [D(tas!(b"state")), D(0)].into()
}

fn timer_poke_slab() -> NounSlab {
    let mut slab = NounSlab::new();
    let timer_noun = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"timer")), D(0)]);
    slab.set_root(timer_noun);
    slab
}

fn base58_tip5_hash_noun(slab: &mut NounSlab, base58: &str) -> Noun {
    let ubig = crate::tip5_util::base58_to_ubig(base58.to_owned())
        .expect("base58 tip5 hash should decode");
    let words =
        crate::tip5_util::decimal_to_base_p(ubig).expect("tip5 hash should fit into 5 words");
    let word0 = Atom::new(slab, words[0]).as_noun();
    let word1 = Atom::new(slab, words[1]).as_noun();
    let word2 = Atom::new(slab, words[2]).as_noun();
    let word3 = Atom::new(slab, words[3]).as_noun();
    let word4 = Atom::new(slab, words[4]).as_noun();
    T(slab, &[word0, word1, word2, word3, word4])
}

fn heard_elders_fact(oldest: u64, elders: &[String]) -> NockchainFact {
    let mut slab = NounSlab::new();
    let ids = elders.iter().rev().fold(D(0), |tail, elder_id| {
        let elder_noun = base58_tip5_hash_noun(&mut slab, elder_id);
        T(&mut slab, &[elder_noun, tail])
    });
    let payload = T(&mut slab, &[D(oldest), ids]);
    let heard_elders = Atom::from_value(&mut slab, "heard-elders")
        .expect("heard-elders atom should build")
        .as_noun();
    let fact = T(&mut slab, &[heard_elders, payload]);
    slab.set_root(fact);
    NockchainFact::from_noun_slab(&mut slab).expect("heard-elders fact should decode")
}

fn setup_command_slab(command: SetupCommand) -> NounSlab {
    match command {
        SetupCommand::PokeFakenetConstants(constants) => {
            let mut poke_slab = NounSlab::new();
            let tag = make_tas(&mut poke_slab, "set-constants").as_noun();
            let constants_noun = constants.to_noun(&mut poke_slab);
            let poke_noun = T(&mut poke_slab, &[D(tas!(b"command")), tag, constants_noun]);
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetGenesisSeal(seal) => {
            let mut poke_slab = NounSlab::new();
            let block_height_noun = Atom::new(&mut poke_slab, 0u64).as_noun();
            let seal_noun = Atom::from_value(&mut poke_slab, seal)
                .expect("seal atom should build")
                .as_noun();
            let set_genesis_seal = Atom::from_value(&mut poke_slab, "set-genesis-seal")
                .expect("set-genesis-seal atom should build")
                .as_noun();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), set_genesis_seal, block_height_noun, seal_noun],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
        SetupCommand::PokeSetBtcData => {
            let mut poke_slab = NounSlab::new();
            let poke_noun = T(
                &mut poke_slab,
                &[D(tas!(b"command")), D(tas!(b"btc-data")), D(0)],
            );
            poke_slab.set_root(poke_noun);
            poke_slab
        }
    }
}

fn fake_genesis_block_message_fact() -> NockchainFact {
    let mut slab = NounSlab::new();
    let heard_block = Atom::from_value(&mut slab, "heard-block")
        .expect("heard-block atom should build")
        .as_noun();
    let genesis_page = slab
        .cue_into(bytes::Bytes::from_static(setup::FAKENET_GENESIS_BLOCK))
        .expect("fake genesis page should cue");
    let response = T(&mut slab, &[heard_block, genesis_page]);
    slab.set_root(response);
    NockchainFact::from_noun_slab(&mut slab).expect("fake genesis heard-block should decode")
}

async fn seed_fakenet_pre_genesis(effect_handle: &NockAppHandle) {
    let setup_pokes = vec![
        setup_command_slab(SetupCommand::PokeFakenetConstants(Box::new(
            default_fakenet_blockchain_constants(),
        ))),
        setup_command_slab(SetupCommand::PokeSetGenesisSeal(
            setup::FAKENET_GENESIS_MESSAGE.to_owned(),
        )),
        setup_command_slab(SetupCommand::PokeSetBtcData),
    ];

    for poke in setup_pokes {
        let result = effect_handle
            .poke(SystemWire.to_wire(), poke)
            .await
            .expect("setup poke should complete");
        assert!(
            matches!(result, nockapp::driver::PokeResult::Ack),
            "setup poke should ack, got {result:?}"
        );
    }
}

fn born_command_slab() -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let poke_noun = T(
        &mut poke_slab,
        &[D(tas!(b"command")), D(tas!(b"born")), D(0)],
    );
    poke_slab.set_root(poke_noun);
    poke_slab
}

async fn send_born(effect_handle: &NockAppHandle) {
    let result = effect_handle
        .poke(SystemWire.to_wire(), born_command_slab())
        .await
        .expect("born poke should complete");
    assert!(
        matches!(result, nockapp::driver::PokeResult::Ack),
        "born poke should ack, got {result:?}"
    );
}

async fn poke_direct(
    app: &mut NockApp<Chaff>,
    wire: nockapp::wire::WireRepr,
    poke: NounSlab,
    label: &str,
) -> Vec<NounSlab> {
    app.poke(wire, poke)
        .await
        .unwrap_or_else(|err| panic!("{label} should complete: {err:?}"))
}

async fn seed_fakenet_pre_genesis_direct(app: &mut NockApp<Chaff>) {
    let setup_pokes = vec![
        setup_command_slab(SetupCommand::PokeFakenetConstants(Box::new(
            default_fakenet_blockchain_constants(),
        ))),
        setup_command_slab(SetupCommand::PokeSetGenesisSeal(
            setup::FAKENET_GENESIS_MESSAGE.to_owned(),
        )),
        setup_command_slab(SetupCommand::PokeSetBtcData),
    ];

    for poke in setup_pokes {
        let _ = poke_direct(app, SystemWire.to_wire(), poke, "direct setup poke").await;
    }
}

async fn send_born_direct(app: &mut NockApp<Chaff>) -> Vec<NounSlab> {
    poke_direct(
        app,
        SystemWire.to_wire(),
        born_command_slab(),
        "direct born poke",
    )
    .await
}

fn request_effect_block_heights_from_effects(effects: &[NounSlab]) -> Vec<u64> {
    effects
        .iter()
        .filter_map(request_effect_block_height)
        .collect::<Vec<_>>()
}

async fn poke_fact_direct(
    app: &mut NockApp<Chaff>,
    peer: PeerId,
    fact: &NockchainFact,
    label: &str,
) -> Vec<NounSlab> {
    poke_direct(
        app,
        Libp2pWire::Response(peer).to_wire(),
        fact.fact_poke().clone(),
        label,
    )
    .await
}

fn request_effect_block_height(effect_slab: &NounSlab) -> Option<u64> {
    let space = effect_slab.noun_space();
    let effect_cell = unsafe { *effect_slab.root() }
        .in_space(&space)
        .as_cell()
        .ok()?;
    if !effect_cell.head().eq_bytes(b"request") {
        return None;
    }
    let request_body = effect_cell.tail().as_cell().ok()?;
    if !request_body.head().eq_bytes(b"block") {
        return None;
    }
    let block_body = request_body.tail().as_cell().ok()?;
    if !block_body.head().eq_bytes(b"by-height") {
        return None;
    }
    block_body.tail().as_atom().ok()?.as_u64().ok()
}

fn effect_head_bytes(effect_slab: &NounSlab) -> Option<Vec<u8>> {
    let space = effect_slab.noun_space();
    let effect_cell = unsafe { *effect_slab.root() }
        .in_space(&space)
        .as_cell()
        .ok()?;
    let atom = effect_cell.head().as_atom().ok()?;
    atom.to_bytes_until_nul().ok().map(|bytes| bytes.to_vec())
}

fn describe_effect(effect_slab: &NounSlab) -> String {
    let Some(head_bytes) = effect_head_bytes(effect_slab) else {
        return String::from("invalid-effect");
    };
    let head = String::from_utf8_lossy(&head_bytes);
    let space = effect_slab.noun_space();
    let effect_cell = match unsafe { *effect_slab.root() }.in_space(&space).as_cell() {
        Ok(cell) => cell,
        Err(_) => return format!("effect={head}"),
    };
    let tail = effect_cell.tail();
    if head_bytes == b"request" {
        let Ok(request_cell) = tail.as_cell() else {
            return String::from("effect=request malformed");
        };
        let request_head = request_cell
            .head()
            .as_atom()
            .ok()
            .and_then(|atom| atom.to_bytes_until_nul().ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|| String::from("<non-atom>"));
        if request_head == "block" {
            let Ok(block_cell) = request_cell.tail().as_cell() else {
                return String::from("effect=request block malformed");
            };
            let block_head = block_cell
                .head()
                .as_atom()
                .ok()
                .and_then(|atom| atom.to_bytes_until_nul().ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_else(|| String::from("<non-atom>"));
            if block_head == "by-height" {
                let height = block_cell
                    .tail()
                    .as_atom()
                    .ok()
                    .and_then(|atom| atom.as_u64().ok())
                    .map(|height| height.to_string())
                    .unwrap_or_else(|| String::from("<non-u64>"));
                return format!("effect=request block by-height {height}");
            }
            return format!("effect=request block {block_head}");
        }
        return format!("effect=request {request_head}");
    }
    format!("effect={head}")
}

async fn drain_effects(effect_handle: &NockAppHandle) {
    while tokio::time::timeout(Duration::from_millis(20), effect_handle.next_effect())
        .await
        .is_ok()
    {}
}

async fn collect_request_effect_block_heights(
    effect_handle: &NockAppHandle,
    expected_count: usize,
    phase: &str,
) -> Vec<u64> {
    let mut heights = Vec::with_capacity(expected_count);
    let mut observed = Vec::new();
    while heights.len() < expected_count {
        let effect = tokio::time::timeout(Duration::from_secs(1), effect_handle.next_effect())
            .await
            .unwrap_or_else(|_| {
                panic!("timed out waiting for kernel effect during {phase}; observed={observed:?}")
            })
            .expect("effect receiver should stay open");
        observed.push(describe_effect(&effect));
        let Some(height) = request_effect_block_height(&effect) else {
            continue;
        };
        heights.push(height);
    }
    heights
}

async fn run_driver_with_timeout<F>(
    transcript: &DriverTranscript,
    label: &str,
    fut: F,
) -> Result<(), NockAppError>
where
    F: Future<Output = Result<(), NockAppError>>,
{
    tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .unwrap_or_else(|_| panic!("{label} timed out\n{}", transcript.render()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_res_driver_gen2_like_high_priority_backlog_does_not_starve_followup_peek_or_timer() {
    use nockapp::drivers::timer::TimerWire;
    use nockapp::wire::{SystemWire, Wire};

    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        "gen2-like multi-peer high-priority backlog should not starve follow-up peek or timer",
    );

    // Pre-nous already had TrafficCop. The regression shape here is the
    // nous-era workload, many peers feeding high-priority timeout pokes
    // into one serialized dispatcher, not TrafficCop existing at all.
    let live_traffic = build_live_traffic_cop_with_test_kernel().await;
    let burst = 16_384usize;
    let probe_deadline = Duration::from_secs(5);

    let mut storm = tokio::task::JoinSet::new();
    for idx in 0..burst {
        let traffic = live_traffic.traffic.clone();
        let peer = PeerId::random();
        storm.spawn(async move {
            let (timing, timing_rx) = tokio::sync::oneshot::channel();
            let mut poke = NounSlab::new();
            poke.set_root(D(tas!(b"inc")));
            let result = traffic
                .poke_high_priority(
                    Some(peer),
                    SystemWire.to_wire(),
                    poke,
                    Box::pin(async { true }),
                    Some(timing),
                )
                .await;
            let timing_result = timing_rx.await;
            (idx, result, timing_result)
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let followup_timer_traffic = live_traffic.traffic.clone();
    let (followup_peek, followup_timer) = tokio::join!(
        tokio::time::timeout(
            probe_deadline,
            live_traffic.traffic.peek(None, state_peek_slab())
        ),
        tokio::time::timeout(probe_deadline, async move {
            let (timing, timing_rx) = tokio::sync::oneshot::channel();
            let timer_result = followup_timer_traffic
                .poke_high_priority(
                    None,
                    TimerWire::Tick.to_wire(),
                    timer_poke_slab(),
                    Box::pin(async { true }),
                    Some(timing),
                )
                .await;
            let timing_result = timing_rx.await;
            (timer_result, timing_result)
        })
    );

    storm.abort_all();

    assert!(
        followup_peek.is_ok(),
        "follow-up peek timed out behind gen2-like high-priority backlog; timer_completed={}",
        followup_timer.is_ok(),
    );
    assert!(
        followup_timer.is_ok(),
        "follow-up timer poke timed out behind gen2-like high-priority backlog; peek_completed={}",
        followup_peek.is_ok(),
    );

    let followup_peek = followup_peek.expect("peek timeout already checked");
    let followup_timer = followup_timer.expect("timer timeout already checked");

    followup_peek.expect("follow-up peek should complete while high-priority backlog is active");
    let (timer_result, timing_result) = followup_timer;
    timing_result.expect("follow-up timer should still report elapsed timing");
    assert!(
        matches!(
            timer_result,
            Ok(nockapp::driver::PokeResult::Ack) | Ok(nockapp::driver::PokeResult::Nack)
        ),
        "follow-up timer poke should complete while high-priority backlog is active: {timer_result:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_res_driver_route_response_fact_high_priority_backlog_does_not_starve_followup_peek_or_timer(
) {
    use nockapp::drivers::timer::TimerWire;
    use nockapp::wire::Wire;
    use nockvm::noun::Atom;

    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        "route_response_fact high-priority backlog should not starve follow-up peek or timer",
    );

    let live_traffic = build_live_traffic_cop(start_nockchain_app().await);
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let burst = 8_192usize;
    let probe_deadline = Duration::from_secs(5);

    let mut storm = tokio::task::JoinSet::new();
    for idx in 0..burst {
        let traffic = live_traffic.traffic.clone();
        let metrics = metrics.clone();
        let driver_state = Arc::clone(&driver_state);
        let swarm_tx = swarm_tx.clone();
        storm.spawn(async move {
            let scry_res = scry_some_raw_tx(300_000 + idx as u64, 64);
            let item = tx_result_item_from_scry(1, &scry_res);
            let envelope = item
                .envelope
                .expect("synthetic tx result item should carry an envelope");
            let response = response_fact_from_envelope(&envelope).expect("envelope should decode");
            route_response_fact(
                PeerId::random(),
                response,
                &traffic,
                &metrics,
                &driver_state,
                &swarm_tx,
            )
            .await
        });
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut tx_id_slab = NounSlab::new();
    let tx_id_noun = Atom::from_value(&mut tx_id_slab, String::from("storm-followup"))
        .expect("follow-up tx id atom should build")
        .as_noun();
    tx_id_slab.set_root(tx_id_noun);
    let peek_path = request_to_scry_slab(NockchainDataRequest::RawTransactionById(
        String::from("storm-followup"),
        tx_id_slab,
    ))
    .expect("follow-up peek path should build");
    let followup_timer_traffic = live_traffic.traffic.clone();
    let (followup_peek, followup_timer) = tokio::join!(
        tokio::time::timeout(probe_deadline, live_traffic.traffic.peek(None, peek_path)),
        tokio::time::timeout(probe_deadline, async move {
            let (timing, timing_rx) = tokio::sync::oneshot::channel();
            let timer_result = followup_timer_traffic
                .poke_high_priority(
                    None,
                    TimerWire::Tick.to_wire(),
                    timer_poke_slab(),
                    Box::pin(async { true }),
                    Some(timing),
                )
                .await;
            let timing_result = timing_rx.await;
            (timer_result, timing_result)
        })
    );

    storm.abort_all();

    assert!(
        followup_peek.is_ok(),
        "follow-up peek timed out behind route_response_fact high-priority backlog; timer_completed={}",
        followup_timer.is_ok(),
    );
    assert!(
        followup_timer.is_ok(),
        "follow-up timer poke timed out behind route_response_fact high-priority backlog; peek_completed={}",
        followup_peek.is_ok(),
    );

    let followup_peek = followup_peek.expect("peek timeout already checked");
    let followup_timer = followup_timer.expect("timer timeout already checked");

    followup_peek.expect("follow-up peek should complete while high-priority backlog is active");
    let (timer_result, timing_result) = followup_timer;
    timing_result.expect("follow-up timer should still report elapsed timing");
    assert!(
        matches!(
            timer_result,
            Ok(nockapp::driver::PokeResult::Ack) | Ok(nockapp::driver::PokeResult::Nack)
        ),
        "follow-up timer poke should complete while high-priority backlog is active: {timer_result:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_route_response_fact_tracks_tx_source_hints_from_heard_block() {
    let transcript = DriverTranscript::default();
    let scripted_traffic =
        build_scripted_traffic_cop(transcript, Vec::new(), vec![PokeResult::Ack]).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 42;
    }
    let peer = PeerId::random();
    let (response, tx_ids) = heard_block_fact_with_tx_ids(42, &[700, 800, 900]);

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("route_response_fact should accept heard-block");

    let state_guard = state_arc.lock().await;
    for tx_id in tx_ids {
        assert_eq!(
            state_guard.get_peers_for_tx_id(&tx_id),
            vec![peer],
            "heard-block ack should remember which peer can likely serve missing tx {tx_id}",
        );
    }
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_heard_block_ack_without_seen_releases_processing_for_replay() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Ack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 42;
    }
    let peer = PeerId::random();
    let height = 42u64;
    let (response, _) = heard_block_fact_with_tx_ids(height, &[]);
    let block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("first heard-block should route");
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_block(&block_id),
            "acked heard-block should release its processing claim"
        );
        assert!(
            !state_guard.seen_blocks.contains(&block_id),
            "ack without %seen must leave seen-block dedupe untouched"
        );
    }

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("replay after ack without %seen should route");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "ack without %seen should not permanently gate a replay"
    );
    assert_eq!(
        metrics.block_seen_cache_hits.fetch_add(0),
        0,
        "ack-released replay should not count as a cache gate hit"
    );
    assert_eq!(
        metrics.block_seen_cache_misses.fetch_add(0),
        2,
        "both heard-block responses should reach the kernel gate"
    );
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_block(&block_id),
            "second ack should also release the processing claim"
        );
        assert!(
            !state_guard.seen_blocks.contains(&block_id),
            "driver must not mark a block seen without the kernel %seen effect"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_seen_block_without_kernel_request_stays_gated() {
    let transcript = DriverTranscript::default();
    let scripted_traffic =
        build_scripted_traffic_cop(transcript, Vec::new(), vec![PokeResult::Ack]).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let height = 42u64;
    let (response, _) = heard_block_fact_with_tx_ids(height, &[]);
    let block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };
    {
        let mut state_guard = state_arc.lock().await;
        // Place the block below the frontier (already validated). The
        // frontier block itself always replays; see
        // `route_response_fact_seen_block_at_frontier_replays_to_kernel`.
        state_guard.first_negative = height + 1;
        state_guard.finish_processing_block_seen(&block_id);
    }

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("seen heard-block duplicate should gate cleanly");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        0,
        "seen block below the frontier without kernel demand should not reach the kernel"
    );
    assert_eq!(
        metrics.block_seen_cache_hits.fetch_add(0),
        1,
        "seen block duplicate should be counted as a gate hit"
    );
    let state_guard = state_arc.lock().await;
    assert!(
        !state_guard.is_processing_block(&block_id),
        "gated seen block should not acquire a processing claim"
    );
    assert!(
        state_guard.seen_blocks.contains(&block_id),
        "gated seen block should remain in seen-block dedupe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_seen_block_at_frontier_replays_to_kernel() {
    let transcript = DriverTranscript::default();
    let scripted_traffic =
        build_scripted_traffic_cop(transcript, Vec::new(), vec![PokeResult::Ack]).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let height = 42u64;
    let (response, _) = heard_block_fact_with_tx_ids(height, &[]);
    let block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };
    {
        let mut state_guard = state_arc.lock().await;
        // The block sits exactly at the frontier: it is the next block the
        // kernel needs but it was marked seen by a null-height `%seen
        // %block` (pending on missing txs). Gating it here with no kernel
        // by-height request armed freezes the frontier permanently.
        state_guard.first_negative = height;
        state_guard.finish_processing_block_seen(&block_id);
    }

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("frontier-height seen block should replay");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        1,
        "seen block at the frontier must replay to the kernel even without a kernel request"
    );
    assert_eq!(
        metrics.block_seen_cache_hits.fetch_add(0),
        0,
        "frontier replay should bypass the seen gate"
    );
    let state_guard = state_arc.lock().await;
    assert!(
        !state_guard.is_processing_block(&block_id),
        "acked frontier replay should release its processing claim"
    );
    assert!(
        state_guard.seen_blocks.contains(&block_id),
        "frontier replay should keep seen-block dedupe intact"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_kernel_requested_seen_block_ack_releases_for_next_replay() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Ack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let height = 42u64;
    let (response, _) = heard_block_fact_with_tx_ids(height, &[]);
    let block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = height;
        state_guard.finish_processing_block_seen(&block_id);
        state_guard.note_kernel_block_height_requested(height);
    }

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("kernel-requested seen block should replay");
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_block(&block_id),
            "acked seen-block replay should release its processing claim"
        );
        assert!(
            state_guard.seen_blocks.contains(&block_id),
            "acked replay must keep seen-block dedupe intact"
        );
    }

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("second kernel-requested seen block replay should also route");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "acked seen-block replay should not leave a stale processing claim"
    );
    assert_eq!(
        metrics.block_seen_cache_hits.fetch_add(0),
        0,
        "kernel-requested seen-block replay should bypass the seen gate"
    );
    assert_eq!(
        metrics.block_seen_cache_misses.fetch_add(0),
        2,
        "both kernel-requested replays should reach the kernel gate"
    );
    let state_guard = state_arc.lock().await;
    assert!(
        !state_guard.is_processing_block(&block_id),
        "second ack should release the seen-block replay claim"
    );
    assert!(
        state_guard.seen_blocks.contains(&block_id),
        "seen-block replay should not clear seen-block dedupe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_block_nack_releases_processing_for_retry() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Nack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 42;
    }
    let peer = PeerId::random();
    let (response, _) = heard_block_fact_with_tx_ids(42, &[]);
    let block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("nacked heard-block should still return cleanly");
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_block(&block_id),
            "nacked heard-block should release its processing claim immediately"
        );
    }
    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("retry after nack should be allowed");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "released processing claim should allow a fresh retry poke"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_heard_tx_ack_without_seen_releases_processing_for_replay() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Ack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let tx_seed = 55_000u64;
    let response = heard_tx_fact(tx_seed, 128);
    let tx_id = match &response {
        NockchainFact::HeardTx(tx_id, _) => tx_id.clone(),
        other => panic!("expected heard-tx fact, got {other:?}"),
    };

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("first heard-tx should route");
    {
        let state_guard = state_arc.lock().await;
        // The kernel acks a heard-tx with no `%seen %tx` when it discards
        // the tx (inputs not in heaviest balance, inputs spent,
        // context-invalid). Holding the claim past the ack would gate the
        // tx forever and starve any pending block that later needs it.
        assert!(
            !state_guard.is_processing_tx(&tx_id),
            "acked heard-tx should release its processing claim"
        );
        assert!(
            !state_guard.seen_txs.contains(&tx_id),
            "ack without %seen must leave seen-tx dedupe untouched"
        );
    }

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("replay after ack without %seen should route");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "ack without %seen should not permanently gate a tx replay"
    );
    assert_eq!(
        metrics.tx_seen_cache_hits.fetch_add(0),
        0,
        "ack-released tx replay should not count as a cache gate hit"
    );
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_tx(&tx_id),
            "second ack should also release the tx processing claim"
        );
        assert!(
            !state_guard.seen_txs.contains(&tx_id),
            "driver must not mark a tx seen without the kernel %seen effect"
        );
    }

    handle_effect(
        seen_tx_effect_slab(tx_seed),
        swarm_tx,
        vec![],
        false,
        state_arc.clone(),
        metrics,
    )
    .await
    .expect("seen tx effect should mark the tx seen");

    let state_guard = state_arc.lock().await;
    assert!(
        !state_guard.is_processing_tx(&tx_id),
        "%seen should leave no tx processing claim"
    );
    assert!(
        state_guard.seen_txs.contains(&tx_id),
        "%seen should still mark the tx as seen"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_tx_nack_releases_processing_for_retry() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Nack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let tx_seed = 56_000u64;
    let response = heard_tx_fact(tx_seed, 96);
    let tx_id = match &response {
        NockchainFact::HeardTx(tx_id, _) => tx_id.clone(),
        other => panic!("expected heard-tx fact, got {other:?}"),
    };

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("nacked heard-tx should still return cleanly");
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_tx(&tx_id),
            "nacked heard-tx should release its processing claim immediately"
        );
    }
    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("retry after tx nack should be allowed");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "released tx processing claim should allow a fresh retry poke"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_tx_request_replay_clears_processing_without_seen_effect() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Ack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let tx_seed = 57_000u64;
    let response = heard_tx_fact(tx_seed, 96);
    let tx_id = match &response {
        NockchainFact::HeardTx(tx_id, _) => tx_id.clone(),
        other => panic!("expected heard-tx fact, got {other:?}"),
    };

    route_response_fact(
        peer,
        response.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("first heard-tx should route");
    {
        let state_guard = state_arc.lock().await;
        assert!(
            !state_guard.is_processing_tx(&tx_id),
            "acked heard-tx should release its processing claim; the kernel may have discarded \
             the tx without emitting %seen and it must stay redeliverable"
        );
        assert!(
            !state_guard.seen_txs.contains(&tx_id),
            "driver must not mark the tx seen before a %seen effect arrives"
        );
    }

    let request_message = jam_raw_tx_request(tx_seed);
    handle_effect(
        jammed_request_slab(&request_message),
        swarm_tx.clone(),
        vec![peer],
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("raw-tx replay request should be accepted");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        } => {
            assert_eq!(peer_id, peer);
            assert_eq!(queued_message.as_ref(), request_message.as_slice());
        }
        other => panic!("expected QueueKernelRequest replay, got {other:?}"),
    }

    {
        let state_guard = state_arc.lock().await;
        assert!(
                !state_guard.is_processing_tx(&tx_id),
                "kernel raw-tx replay must clear any stranded processing claim before the retry arrives"
            );
        assert!(
            !state_guard.seen_txs.contains(&tx_id),
            "replay should reopen the tx gate without prematurely marking the tx as seen"
        );
    }

    route_response_fact(
        peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("replayed heard-tx should be allowed after raw-tx request");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "cleared processing claim should allow the replayed heard-tx to repoke the kernel"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_seen_effect_queues_deferred_flush_without_waiting_on_swarm_queue() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let (deferred_block, _) = heard_block_fact_with_tx_ids(11, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &deferred_block else {
        panic!("expected heard-block fact");
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
        assert!(
            state_guard.defer_heard_block(peer, 11, block_id.clone(), deferred_block),
            "test setup should store one deferred heard-block"
        );
    }

    let mut buffered_swarm_actions = VecDeque::new();
    tokio::time::timeout(Duration::from_millis(50), async {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_effect_with_dispatcher(
            seen_block_effect_slab(30_010, 10),
            &mut swarm_actions,
            vec![],
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            Arc::clone(&state_arc),
            metrics.clone(),
            PeerExclusions::default(),
        )
        .await
    })
    .await
    .expect("buffered seen-effect dispatch should not block")
    .expect("seen effect should succeed");

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!(
            "expected buffered deferred heard-block flush action, got {:?}",
            other
        ),
    }
    assert!(
        buffered_swarm_actions.is_empty(),
        "seen effect should only queue a single deferred flush action"
    );

    let state_guard = state_arc.lock().await;
    assert_eq!(
        state_guard.first_negative, 11,
        "seen height should still advance the frontier before queueing the flush"
    );
    assert_eq!(
        state_guard.deferred_heard_block_heights(),
        vec![11],
        "queued flush should leave deferred blocks buffered until the action runs"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seen_effect_does_not_flush_prefetch_without_kernel_height_request() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let (deferred_block, _) = heard_block_fact_with_tx_ids(11, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &deferred_block else {
        panic!("expected heard-block fact");
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
        assert!(
            state_guard.defer_heard_block_with_source(
                peer,
                11,
                block_id.clone(),
                deferred_block,
                BlockSource::Prefetch,
            ),
            "test setup should store one prefetched heard-block"
        );
    }

    let mut buffered_swarm_actions = VecDeque::new();
    {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_effect_with_dispatcher(
            seen_block_effect_slab(30_010, 10),
            &mut swarm_actions,
            vec![],
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            Arc::clone(&state_arc),
            metrics.clone(),
            PeerExclusions::default(),
        )
        .await
        .expect("seen effect should succeed");
    }

    assert!(
        buffered_swarm_actions.is_empty(),
        "prefetch entries should wait for explicit kernel height demand"
    );
    {
        let state_guard = state_arc.lock().await;
        assert_eq!(state_guard.first_negative, 11);
        assert!(state_guard.has_deferred_block_at_height(11));
        assert!(!state_guard.has_ready_deferred_heard_blocks());
    }

    let request_slab = request_slab_from_message(block_by_height_message(11).as_ref())
        .expect("block-by-height request slab should decode");
    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_effect_with_dispatcher(
            request_slab,
            &mut swarm_actions,
            vec![peer],
            false,
            prefetch_config,
            runtime_limits_from_config(&LIBP2P_CONFIG),
            Arc::clone(&state_arc),
            metrics,
            PeerExclusions::default(),
        )
        .await
        .expect("kernel height request should authorize prefetched flush");
    }

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred flush action, got {other:?}"),
    }
    assert!(
        buffered_swarm_actions.is_empty(),
        "authorized cache hit should not queue an outbound request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_by_height_request_with_buffered_block_and_prefetch_disabled_dispatches_outbound() {
    // Default-off baseline: a deferred-buffer hit must not alter the
    // classic by-height request path when catch-up prefetch is disabled.
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let height = 42u64;
    let (deferred_block, _) = heard_block_fact_with_tx_ids(height, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &deferred_block else {
        panic!("expected heard-block fact");
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = height;
        assert!(
            state_guard.defer_heard_block(peer, height, block_id.clone(), deferred_block),
            "test setup should buffer one heard-block at requested height"
        );
    }

    let request_slab = request_slab_from_message(block_by_height_message(height).as_ref())
        .expect("block-by-height request slab should decode");

    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("block-by-height effect should keep the disabled path unchanged");

    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected classic outbound request, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&queued)
        .expect("queued request should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(42)
        ),
        "expected block-by-height fallback, got {parsed:?}"
    );
    assert!(
        buffered_swarm_actions.is_empty(),
        "disabled cache hit should queue only the classic outbound request"
    );

    let state_guard = state_arc.lock().await;
    assert!(
        state_guard.has_deferred_block_at_height(height),
        "buffered block should remain available"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_enabled_buffered_block_at_frontier_queues_deferred_flush() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let height = 42u64;
    let (deferred_block, _) = heard_block_fact_with_tx_ids(height, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &deferred_block else {
        panic!("expected heard-block fact");
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = height;
        assert!(
            state_guard.defer_heard_block(peer, height, block_id.clone(), deferred_block),
            "test setup should buffer one heard-block at requested height"
        );
    }

    let request_slab = request_slab_from_message(block_by_height_message(height).as_ref())
        .expect("block-by-height request slab should decode");
    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };

    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("frontier cache hit should queue deferred flush");

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred flush action, got {other:?}"),
    }
    assert!(
        buffered_swarm_actions.is_empty(),
        "frontier cache hit should not queue an outbound request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_enabled_buffered_block_ahead_of_frontier_dispatches_outbound() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let height = 42u64;
    let (deferred_block, _) = heard_block_fact_with_tx_ids(height, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &deferred_block else {
        panic!("expected heard-block fact");
    };
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = height - 1;
        state_guard.mark_peer_non_range_capable(peer);
        assert!(
            state_guard.defer_heard_block(peer, height, block_id.clone(), deferred_block),
            "test setup should buffer one future heard-block"
        );
    }

    let request_slab = request_slab_from_message(block_by_height_message(height).as_ref())
        .expect("block-by-height request slab should decode");
    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };

    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("future cache hit should continue to outbound path");

    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected outbound request for future buffered height, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&queued)
        .expect("queued request should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(42)
        ),
        "expected block-by-height request, got {parsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_by_height_request_without_buffered_block_dispatches_outbound() {
    // Negative case for the cache-hit short-circuit: with an empty buffer,
    // the existing outbound dispatch path runs as before.
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let height = 42u64;
    let request_slab = request_slab_from_message(block_by_height_message(height).as_ref())
        .expect("block-by-height request slab should decode");

    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("block-by-height effect should succeed without cache");

    let dispatched = buffered_swarm_actions
        .iter()
        .filter(|a| matches!(a, SwarmAction::QueueKernelRequest { .. }))
        .count();
    assert_eq!(
        dispatched, 1,
        "expected exactly one outbound QueueKernelRequest, got {dispatched}; actions={:?}",
        buffered_swarm_actions
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_disabled_falls_through_to_singleton_outbound() {
    // Phase 4 baseline: with prefetch disabled, the cache-miss
    // path must dispatch the existing singleton block-by-height request
    // even when the catch-up signal is CatchingUp.
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        // Drive the signal into CatchingUp via deferred backlog so the
        // mode check would otherwise enable prefetch.
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(peer);
    }
    assert_eq!(
        state_arc.lock().await.catch_up_signal().mode(),
        crate::catch_up::SyncMode::CatchingUp
    );

    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("disabled prefetch should fall through to singleton");

    let dispatched = buffered_swarm_actions
        .iter()
        .filter(|a| matches!(a, SwarmAction::QueueKernelRequest { .. }))
        .count();
    assert_eq!(
        dispatched, 1,
        "disabled prefetch must still dispatch the kernel singleton, got {:?}",
        buffered_swarm_actions
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_keeps_genesis_request_singleton() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 0;
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 1,
    };
    let request_slab = request_slab_from_message(block_by_height_message(0).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("genesis request should dispatch as a singleton");

    assert_eq!(
        buffered_swarm_actions.len(),
        1,
        "exactly one outbound action expected"
    );
    let dispatched = match &buffered_swarm_actions[0] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&dispatched)
        .expect("dispatched message should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockByHeight(height) => {
            assert_eq!(height, 0);
        }
        other => panic!("expected genesis BlockByHeight request, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_replaces_singleton_with_range_request_when_eligible() {
    // Phase 4: with prefetch enabled and the catch-up signal in
    // CatchingUp, the kernel singleton must be replaced by a windowed
    // BlockRangeWithTxs request to a single peer.
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(peer);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("eligible prefetch should dispatch a range request");

    assert_eq!(
        buffered_swarm_actions.len(),
        1,
        "exactly one outbound action expected"
    );
    let dispatched = match &buffered_swarm_actions[0] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&dispatched)
        .expect("dispatched message should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            assert_eq!(start_height, 50);
            assert_eq!(len, 4);
        }
        other => panic!("expected BlockRangeWithTxs, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_replaces_frontier_singleton_when_kernel_demand_threshold_is_one() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 50;
        state_guard.note_frontier_advanced();
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(peer);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 1,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("frontier demand prefetch should dispatch a range request");

    assert_eq!(
        buffered_swarm_actions.len(),
        1,
        "exactly one outbound action expected"
    );
    let dispatched = match &buffered_swarm_actions[0] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&dispatched)
        .expect("dispatched message should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            assert_eq!(start_height, 50);
            assert_eq!(len, 4);
        }
        other => panic!("expected BlockRangeWithTxs, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_window_targets_response_budget_when_cold() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(peer);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 16,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    limits.gen2_item_max_bytes = 2 * 1024 * 1024;
    limits.gen2_block_batch_max_response_bytes = 2 * 1024 * 1024;
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        limits,
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("eligible prefetch should dispatch a tuned range request");

    let dispatched = match &buffered_swarm_actions[0] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&dispatched)
        .expect("dispatched message should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            assert_eq!(start_height, 50);
            assert_eq!(len, 15);
        }
        other => panic!("expected BlockRangeWithTxs, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_window_uses_observed_range_bytes_for_next_tail() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_020u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.record_response_message_hint(
            &crate::messages::NockchainDataRequest::BlockRangeWithTxs {
                start_height: 50,
                len: 17,
            },
            1_995_241,
        );
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(peer);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 64,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    limits.gen2_item_max_bytes = 2 * 1024 * 1024;
    limits.gen2_block_batch_max_response_bytes = 2 * 1024 * 1024;
    let request_slab = request_slab_from_message(block_by_height_message(67).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        limits,
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("eligible prefetch should dispatch a range from observed bytes");

    let dispatched = match &buffered_swarm_actions[0] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&dispatched)
        .expect("dispatched message should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            assert_eq!(start_height, 67);
            assert_eq!(len, 17);
        }
        other => panic!("expected BlockRangeWithTxs, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_skips_peers_marked_non_range_capable() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer_a,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        // Mark BOTH connected peers as non-range-capable to force the
        // no-eligible-peer path.
        state_guard.observe_peer_generation(peer_a, ReqResGeneration::Gen2);
        state_guard.observe_peer_generation(peer_b, ReqResGeneration::Gen2);
        state_guard.mark_peer_non_range_capable(peer_a);
        state_guard.mark_peer_non_range_capable(peer_b);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer_a, peer_b],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("non-range-capable peers should fall back to singleton");

    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected QueueKernelRequest fallback, got {other:?}"),
    };
    let parsed =
        crate::messages::decode_request_item_message(&queued).expect("fallback should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(50)
        ),
        "expected singleton fallback at height 50, got {parsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_selects_supported_gen2_peer_when_gen1_is_listed_first() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let gen1_peer = PeerId::random();
    let gen2_peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                gen2_peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(gen1_peer, ReqResGeneration::Gen1);
        state_guard.observe_peer_generation(gen2_peer, ReqResGeneration::Gen2);
        state_guard.mark_peer_range_supported(gen2_peer);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![gen1_peer, gen2_peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("eligible prefetch should choose the supported gen2 peer");

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(peer_id, gen2_peer);
            let parsed = crate::messages::decode_request_item_message(&request_message)
                .expect("range request should decode");
            match parsed {
                crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                    assert_eq!(start_height, 50);
                    assert_eq!(len, 4);
                }
                other => panic!("expected BlockRangeWithTxs, got {other:?}"),
            }
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_probes_unknown_gen2_peer_with_bounded_range() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 8,
        window_max: 8,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("unknown gen2 peer should receive a bounded probe");

    let request_message = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(peer_id, peer);
            request_message
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&request_message)
        .expect("probe request should decode");
    match parsed {
        crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            assert_eq!(start_height, 50);
            assert_eq!(len, 2);
        }
        other => panic!("expected BlockRangeWithTxs probe, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_falls_back_to_singleton_when_no_gen2_peer_is_available() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 0;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen1);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(50).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("no-gen2 prefetch should use the singleton path");

    let request_message = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(peer_id, peer);
            request_message
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&request_message)
        .expect("singleton fallback should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(50)
        ),
        "expected BlockByHeight fallback, got {parsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_singleton_suppression_when_inflight_covers_height() {
    // Phase 4: a kernel singleton for a height already covered
    // by an inflight prefetch must be fully suppressed (no new outbound).
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    {
        let mut state_guard = state_arc.lock().await;
        // Simulate a prefetch in flight covering heights 50..54.
        state_guard.register_prefetch(request_id, peer, 50, 4);
    }

    let prefetch_config = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    let request_slab = request_slab_from_message(block_by_height_message(52).as_ref())
        .expect("block-by-height request slab should decode");
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        request_slab,
        &mut swarm_actions,
        vec![peer],
        false,
        prefetch_config,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("singleton-over-prefetch should succeed");

    assert!(
        buffered_swarm_actions.is_empty(),
        "kernel singleton must be suppressed when prefetch covers height; got {:?}",
        buffered_swarm_actions
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lax1_missing_parent_loop_backs_off_after_failure_budget() {
    // Phase 5: reproduce the shape of the LAX1 sync-gap incident
    // where a height N is repeatedly requested but never served. The
    // alternate-peer retry path must back off after the configured
    // failure budget rather than amplifying the loop on every retrigger.
    use crate::messages::block_by_height_message;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.set_prefetch_safety_config(3, Duration::from_secs(60), 50 * 1024 * 1024);
    }
    let stuck_height = 9_801u64;
    let request_message = block_by_height_message(stuck_height);

    // Two peers connected; pretend both have already been attempted to
    // simulate prior failures + subsequent retry decisions.
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        // Connect both peers so the retry path has candidates to choose
        // from on the first few attempts.
        for peer in [peer_a, peer_b] {
            state_guard
                .peer_connections
                .insert(peer, std::collections::BTreeMap::new());
        }
    }

    // Drive three retries against the stuck height. Each call records a
    // failure; the third should mark the height stuck and short-circuit.
    let mut buffered_swarm_actions = VecDeque::new();
    let mut retry_results = Vec::new();
    {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        for _ in 0..3 {
            let result =
                crate::driver::gen2::queue_block_height_retry_to_alternate_peer_with_dispatcher(
                    &mut swarm_actions,
                    &state_arc,
                    stuck_height,
                    request_message.clone(),
                )
                .await
                .expect("retry call should not error");
            retry_results.push(result);
        }
    }

    // The third call must have marked the height stuck and returned false
    // without dispatching another outbound.
    assert_eq!(retry_results, vec![true, true, false]);
    let state_guard = state_arc.lock().await;
    assert!(
        state_guard.is_block_height_stuck(stuck_height),
        "stuck_until must be set after the failure budget is exhausted"
    );
    assert_eq!(state_guard.stuck_block_heights(), vec![stuck_height]);

    // Two retries dispatched, third short-circuited.
    let dispatched = buffered_swarm_actions
        .iter()
        .filter(|a| matches!(a, SwarmAction::QueueKernelRequest { .. }))
        .count();
    assert_eq!(
        dispatched, 2,
        "stuck heights must not amplify retries beyond the failure budget"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payload_only_seen_block_effect_tracks_id_without_advancing_frontier() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
    }

    let block_seed = 30_123;
    let expected_block_id = base58_for_tip5_seed(block_seed);
    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    handle_effect_with_dispatcher(
        seen_block_payload_only_effect_slab(block_seed),
        &mut swarm_actions,
        vec![],
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("payload-only seen effect should succeed");

    assert!(
        buffered_swarm_actions.is_empty(),
        "payload-only seen effect must not queue a deferred flush without frontier progress"
    );

    let state_guard = state_arc.lock().await;
    assert!(
        state_guard.seen_blocks.contains(&expected_block_id),
        "payload-only seen effect should still track the block id"
    );
    assert_eq!(
        state_guard.first_negative, 10,
        "payload-only seen effect must not advance the block frontier"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_defers_peer_observed_height_until_track_add() {
    use tokio::sync::mpsc;

    let transcript = DriverTranscript::default();
    let scripted_traffic =
        build_scripted_traffic_cop(transcript, Vec::new(), vec![PokeResult::Ack]).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 20;
    }
    let (swarm_tx, _swarm_rx) = mpsc::channel(8);
    let peer = PeerId::random();
    let height = 11u64;
    let (block_11, _) = heard_block_fact_with_tx_ids(height, &[]);

    route_response_fact(
        peer, block_11, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("heard-block response should route cleanly");

    assert_eq!(
        state_arc
            .lock()
            .await
            .catch_up_signal()
            .peer_observed_max_height(),
        0,
        "routing alone must not advance trusted peer-observed height"
    );

    handle_effect(
        track_add_effect_slab(10_000 + height, peer),
        swarm_tx,
        vec![],
        false,
        state_arc.clone(),
        metrics,
    )
    .await
    .expect("track add should succeed");

    assert_eq!(
        state_arc
            .lock()
            .await
            .catch_up_signal()
            .peer_observed_max_height(),
        height,
        "track add should advance trusted peer-observed height"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn track_add_without_observed_height_candidate_does_not_advance_peer_observed_height() {
    use tokio::sync::mpsc;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = mpsc::channel(8);
    let peer = PeerId::random();

    handle_effect(
        track_add_effect_slab(44_000, peer),
        swarm_tx,
        vec![],
        false,
        state_arc.clone(),
        metrics,
    )
    .await
    .expect("track add without candidate should still track the peer");

    assert_eq!(
        state_arc
            .lock()
            .await
            .catch_up_signal()
            .peer_observed_max_height(),
        0,
        "unknown block id should not alter trusted peer-observed height"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_defers_future_heard_blocks_until_seen_frontier_advances() {
    use tokio::sync::mpsc;

    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript,
        Vec::new(),
        vec![PokeResult::Ack, PokeResult::Ack],
    )
    .await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
    }
    let peer = PeerId::random();
    let (block_11, _) = heard_block_fact_with_tx_ids(11, &[]);
    let (block_12, _) = heard_block_fact_with_tx_ids(12, &[]);

    route_response_fact(
        peer,
        block_11.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("future height 11 should defer cleanly");
    route_response_fact(
        peer,
        block_12.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("future height 12 should defer cleanly");
    route_response_fact(
        peer, block_12, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("duplicate future height 12 should defer cleanly");

    {
        let state_guard = state_arc.lock().await;
        assert_eq!(
            state_guard.deferred_heard_block_heights(),
            vec![11, 12],
            "future heard-blocks should stay buffered by height until the seen frontier moves"
        );
        assert_eq!(
            state_guard.deferred_heard_block_count(),
            2,
            "duplicate future heard-blocks should be coalesced while buffered"
        );
        assert_eq!(
            state_guard.catch_up_signal().peer_observed_max_height(),
            0,
            "future deferral should not advance trusted peer-observed height"
        );
    }
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        0,
        "buffered future heard-blocks must not poke the kernel early",
    );

    handle_effect(
        seen_block_effect_slab(30_010, 10),
        swarm_tx.clone(),
        vec![],
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("seen height 10 should advance the frontier");
    match swarm_rx.recv().await {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred heard-block flush action, got {other:?}"),
    }
    let flushed = flush_ready_deferred_heard_blocks(
        &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("flushing ready deferred block height 11 should succeed");
    assert_eq!(flushed, 1, "only the next frontier height should flush");
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        1,
        "height 11 should poke once after height 10 becomes seen",
    );
    {
        let state_guard = state_arc.lock().await;
        assert_eq!(
            state_guard.deferred_heard_block_heights(),
            vec![12],
            "height 12 should remain buffered until height 11 becomes seen"
        );
    }

    handle_effect(
        seen_block_effect_slab(30_011, 11),
        swarm_tx.clone(),
        vec![],
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("seen height 11 should advance the frontier again");
    match swarm_rx.recv().await {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred heard-block flush action, got {other:?}"),
    }
    let flushed = flush_ready_deferred_heard_blocks(
        &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("flushing ready deferred block height 12 should succeed");
    assert_eq!(
        flushed, 1,
        "height 12 should flush after height 11 becomes seen"
    );
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        2,
        "buffered future heights should reach the kernel exactly once in frontier order",
    );
    assert!(
        state_arc
            .lock()
            .await
            .deferred_heard_block_heights()
            .is_empty(),
        "all buffered future heard-blocks should be drained once the frontier catches up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_block_range_marks_future_deferred_blocks_as_prefetch() {
    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(transcript, Vec::new(), Vec::new()).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
    }
    let peer = PeerId::random();
    let (block_11, _) = heard_block_fact_with_tx_ids(11, &[]);
    let (block_12, _) = heard_block_fact_with_tx_ids(12, &[]);
    let envelope = ResponseEnvelope::heard_block_range_with_txs(vec![
        bundled_block_from_fact(&block_11),
        bundled_block_from_fact(&block_12),
    ]);

    let mut buffered_swarm_actions = VecDeque::new();
    let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
    route_block_range_envelope_with_dispatcher(
        peer, &envelope, &scripted_traffic.traffic, &metrics, &state_arc, &mut swarm_actions,
    )
    .await
    .expect("future range blocks should defer cleanly");

    let state_guard = state_arc.lock().await;
    assert_eq!(
        state_guard.deferred_heard_block_sources_at_height(11),
        vec![BlockSource::Prefetch]
    );
    assert_eq!(
        state_guard.deferred_heard_block_sources_at_height(12),
        vec![BlockSource::Prefetch]
    );
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        0,
        "future range blocks should not poke the kernel before frontier advance"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_response_fact_deferred_future_heard_block_queues_speculative_raw_tx_prefetches() {
    use tokio::sync::mpsc;

    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(transcript, Vec::new(), Vec::new()).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(16);
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
    }
    let peer = PeerId::random();
    let (block_11, tx_ids) = heard_block_fact_with_tx_ids(11, &[700, 800, 900]);

    route_response_fact(
        peer,
        block_11.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("future heard-block should defer cleanly");

    let mut queued_tx_ids = Vec::new();
    for _ in 0..tx_ids.len() {
        let action = swarm_rx
            .recv()
            .await
            .expect("speculative raw-tx prefetch should queue an action");
        match action {
            SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            } => {
                assert_eq!(
                    peer_id, peer,
                    "speculative raw-tx prefetch should stay on the block source peer"
                );
                let NockchainDataRequest::RawTransactionById(tx_id, _) =
                    decode_request_item_message(&request_message)
                        .expect("speculative raw-tx request should decode")
                else {
                    panic!("expected speculative raw-tx request");
                };
                queued_tx_ids.push(tx_id);
            }
            other => panic!("expected speculative QueueKernelRequest, got {other:?}"),
        }
    }
    queued_tx_ids.sort();
    let mut expected_tx_ids = tx_ids.clone();
    expected_tx_ids.sort();
    assert_eq!(
        queued_tx_ids, expected_tx_ids,
        "future heard-block deferral should immediately prefetch every unseen tx id once"
    );
    {
        let state_guard = state_arc.lock().await;
        assert_eq!(
            state_guard.deferred_heard_block_heights(),
            vec![11],
            "future heard-block should still remain buffered until frontier advances"
        );
        assert_eq!(
            state_guard.speculative_tx_prefetch_count(),
            tx_ids.len(),
            "speculative tx prefetch ledger should remember the queued tx ids"
        );
        for tx_id in &tx_ids {
            assert_eq!(
                state_guard.get_peers_for_tx_id(tx_id),
                vec![peer],
                "deferred future heard-block should still record tx source hints"
            );
        }
    }
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        0,
        "deferred future heard-block should not poke the kernel early",
    );

    route_response_fact(
        peer,
        block_11.clone(),
        &scripted_traffic.traffic,
        &metrics,
        &state_arc,
        &swarm_tx,
    )
    .await
    .expect("duplicate future heard-block should still defer cleanly");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), swarm_rx.recv())
            .await
            .is_err(),
        "duplicate future heard-block should not queue duplicate speculative raw-tx prefetches",
    );

    handle_effect(
        seen_tx_effect_slab(700),
        swarm_tx.clone(),
        vec![],
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("seen tx effect should clear one speculative claim");
    assert_eq!(
        state_arc.lock().await.speculative_tx_prefetch_count(),
        tx_ids.len() - 1,
        "seen tx effect should retire the corresponding speculative prefetch claim"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn heard_elders_re_emits_same_recovery_window_until_progress() {
    let mut checkpoint_app = start_nockchain_app().await;
    seed_fakenet_pre_genesis_direct(&mut checkpoint_app.app).await;
    let _ = send_born_direct(&mut checkpoint_app.app).await;
    let peer = PeerId::random();
    let genesis_id = match fake_genesis_block_message_fact() {
        NockchainFact::HeardBlock(block_id, _) => block_id,
        other => panic!("expected fake genesis heard-block fact, got {other:?}"),
    };
    let synthetic_branch_head = {
        let mut slab = NounSlab::new();
        let noun = tip5_tuple(&mut slab, 90_001);
        let space = slab.noun_space();
        tip5_hash_to_base58(noun, &space).expect("synthetic branch head should convert to base58")
    };
    let heard_elders = heard_elders_fact(0, &[synthetic_branch_head, genesis_id]);

    let first_effects = poke_fact_direct(
        &mut checkpoint_app.app, peer, &heard_elders, "first direct heard-elders poke",
    )
    .await;
    assert_eq!(
        request_effect_block_heights_from_effects(&first_effects),
        vec![0],
        "before genesis is accepted, heard-elders should request by-height 0"
    );

    let second_effects = poke_fact_direct(
        &mut checkpoint_app.app, peer, &heard_elders, "second direct heard-elders poke",
    )
    .await;
    assert_eq!(
        request_effect_block_heights_from_effects(&second_effects),
        vec![0],
        "without progress, the same heard-elders payload should still reissue by-height 0"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_response_fact_repeated_heard_elders_re_emits_recovery_window_until_progress() {
    let live_traffic = build_live_traffic_cop(start_nockchain_app().await);
    drain_effects(&live_traffic.effect_handle).await;
    seed_fakenet_pre_genesis(&live_traffic.effect_handle).await;
    send_born(&live_traffic.effect_handle).await;
    drain_effects(&live_traffic.effect_handle).await;

    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let genesis_id = match fake_genesis_block_message_fact() {
        NockchainFact::HeardBlock(block_id, _) => block_id,
        other => panic!("expected fake genesis heard-block fact, got {other:?}"),
    };
    let synthetic_branch_head = {
        let mut slab = NounSlab::new();
        let noun = tip5_tuple(&mut slab, 90_001);
        let space = slab.noun_space();
        tip5_hash_to_base58(noun, &space).expect("synthetic branch head should convert to base58")
    };
    let heard_elders = heard_elders_fact(0, &[synthetic_branch_head, genesis_id]);

    route_response_fact(
        peer,
        heard_elders.clone(),
        &live_traffic.traffic,
        &metrics,
        &driver_state,
        &swarm_tx,
    )
    .await
    .expect("first heard-elders response should route before genesis");

    let first_window =
        collect_request_effect_block_heights(&live_traffic.effect_handle, 1, "first elders").await;
    assert_eq!(
        first_window,
        vec![0],
        "before genesis is accepted, heard-elders should request by-height 0"
    );

    route_response_fact(
        peer, heard_elders, &live_traffic.traffic, &metrics, &driver_state, &swarm_tx,
    )
    .await
    .expect("repeated heard-elders response should still route before progress");
    let second_window =
        collect_request_effect_block_heights(&live_traffic.effect_handle, 1, "second elders").await;
    assert_eq!(
        second_window,
        vec![0],
        "without progress, repeated heard-elders should still re-emit by-height 0"
    );
}

fn fresh_outbound_request_id() -> request_response::OutboundRequestId {
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

fn jam_block_by_height_request(height: u64) -> Vec<u8> {
    crate::messages::block_by_height_message(height).into_vec()
}

fn jam_raw_tx_request(seed: u64) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let tx_id = T(
        &mut slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    );
    let by_id = T(&mut slab, &[D(tas!(b"by-id")), tx_id]);
    let raw_tx = T(&mut slab, &[D(tas!(b"raw-tx")), by_id]);
    let request = T(&mut slab, &[D(tas!(b"request")), raw_tx]);
    slab.set_root(request);
    slab.jam().as_ref().to_vec()
}

fn tip5_tuple(slab: &mut NounSlab, seed: u64) -> Noun {
    T(
        slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    )
}

fn tip5_zset(slab: &mut NounSlab, seeds: &[u64]) -> Noun {
    seeds.iter().rev().fold(D(0), |tree, seed| {
        let item = tip5_tuple(slab, *seed);
        T(slab, &[item, D(0), tree])
    })
}

fn heard_block_fact_with_tx_ids(height: u64, tx_seeds: &[u64]) -> (NockchainFact, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let heard_block = Atom::from_value(&mut slab, "heard-block")
        .expect("heard-block atom should build")
        .as_noun();
    let response = T(&mut slab, &[heard_block, page]);
    slab.set_root(response);
    let tx_ids = tx_seeds
        .iter()
        .map(|seed| {
            let mut tx_slab = NounSlab::new();
            let noun = tip5_tuple(&mut tx_slab, *seed);
            let space = tx_slab.noun_space();
            tip5_hash_to_base58(noun, &space).expect("tx id tuple should convert to base58")
        })
        .collect::<Vec<_>>();
    (
        NockchainFact::from_noun_slab(&mut slab).expect("heard-block fact should decode"),
        tx_ids,
    )
}

fn result_message_from_fact(fact: &NockchainFact) -> ByteBuf {
    let fact_slab = fact.fact_poke();
    let space = fact_slab.noun_space();
    let fact_noun = unsafe { *fact_slab.root() };
    let response = fact_noun
        .in_space(&space)
        .as_cell()
        .expect("fact poke should be a cell")
        .tail()
        .as_cell()
        .expect("fact poke tail should be a cell")
        .tail()
        .noun();
    let mut message_slab: NounSlab = NounSlab::new();
    message_slab.copy_into(response, &space);
    ByteBuf::from(message_slab.jam().as_ref().to_vec())
}

fn bundled_block_from_fact(fact: &NockchainFact) -> BundledBlockWithTxs {
    let NockchainFact::HeardBlock(block_id, _) = fact else {
        panic!("expected heard-block fact");
    };
    BundledBlockWithTxs {
        block_id: block_id.clone(),
        block_message: result_message_from_fact(fact),
        tx_envelopes: Vec::new(),
        unincluded_tx_ids: Vec::new(),
    }
}

fn heard_tx_fact(seed: u64, payload_len: usize) -> NockchainFact {
    let scry_res = scry_some_raw_tx(seed, payload_len);
    let mut response_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let response = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut response_slab,
    );
    let Right(Ok(NockchainResponse::Result { message })) = response else {
        panic!("expected heard-tx response");
    };
    response_fact_from_result_message(&message).expect("heard-tx fact should decode")
}

fn seen_block_effect_slab(block_seed: u64, height: u64) -> NounSlab {
    let mut effect_slab = NounSlab::new();
    let block_id = tip5_tuple(&mut effect_slab, block_seed);
    let height_unit = T(&mut effect_slab, &[D(0), D(height)]);
    let effect = T(
        &mut effect_slab,
        &[D(tas!(b"seen")), D(tas!(b"block")), block_id, height_unit],
    );
    effect_slab.set_root(effect);
    effect_slab
}

fn seen_block_payload_only_effect_slab(block_seed: u64) -> NounSlab {
    let mut effect_slab = NounSlab::new();
    let block_id = tip5_tuple(&mut effect_slab, block_seed);
    let effect = T(
        &mut effect_slab,
        &[D(tas!(b"seen")), D(tas!(b"block")), block_id, D(0)],
    );
    effect_slab.set_root(effect);
    effect_slab
}

fn track_add_effect_slab(block_seed: u64, peer_id: PeerId) -> NounSlab {
    let mut effect_slab = NounSlab::new();
    let block_id = tip5_tuple(&mut effect_slab, block_seed);
    let peer_id_atom =
        Atom::from_value(&mut effect_slab, peer_id.to_base58()).expect("peer id atom should build");
    let data = T(&mut effect_slab, &[block_id, peer_id_atom.as_noun()]);
    let add = T(&mut effect_slab, &[D(tas!(b"add")), data]);
    let effect = T(&mut effect_slab, &[D(tas!(b"track")), add]);
    effect_slab.set_root(effect);
    effect_slab
}

fn seen_tx_effect_slab(tx_seed: u64) -> NounSlab {
    let mut effect_slab = NounSlab::new();
    let tx_id = tip5_tuple(&mut effect_slab, tx_seed);
    let effect = T(&mut effect_slab, &[D(tas!(b"seen")), D(tas!(b"tx")), tx_id]);
    effect_slab.set_root(effect);
    effect_slab
}

fn jammed_request_slab(message: &[u8]) -> NounSlab {
    request_slab_from_message(message).expect("request slab should decode")
}

fn loopback_quic_addr() -> Multiaddr {
    "/ip4/127.0.0.1/udp/0/quic-v1"
        .parse()
        .expect("loopback quic address should parse")
}

fn loopback_quic_addr_with_port(port: u16) -> Multiaddr {
    format!("/ip4/127.0.0.1/udp/{port}/quic-v1")
        .parse()
        .expect("loopback quic address with port should parse")
}

fn build_test_swarm(config: LibP2PConfig) -> ReqResTestSwarm {
    build_req_res_test_swarm(
        config,
        libp2p::identity::Keypair::generate_ed25519(),
        vec![loopback_quic_addr()],
    )
    .expect("req-res test swarm should build")
}

async fn wait_for_listen_addr(
    swarm: &mut ReqResTestSwarm,
    transcript: &DriverTranscript,
) -> Multiaddr {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match swarm.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    transcript.record("swarm", format!("listening on {address}"));
                    return address;
                }
                other => {
                    transcript.record("swarm", format!("waiting for listen addr saw {other:?}"))
                }
            }
        }
    })
    .await
    .expect("listen address timeout")
}

async fn connect_test_swarms(
    requester: &mut ReqResTestSwarm,
    responder: &mut ReqResTestSwarm,
    responder_addr: &Multiaddr,
    transcript: &DriverTranscript,
) {
    requester
        .dial(responder_addr.clone())
        .expect("dial should be accepted");
    transcript.record("requester", format!("dialing {responder_addr}"));

    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();
    let mut requester_connected = false;
    let mut responder_connected = false;

    tokio::time::timeout(Duration::from_secs(15), async {
            while !(requester_connected && responder_connected) {
                tokio::select! {
                    event = requester.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == responder_peer_id => {
                                requester_connected = true;
                                transcript.record("requester", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("requester", format!("connect loop saw {other:?}")),
                        }
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == requester_peer_id => {
                                responder_connected = true;
                                transcript.record("responder", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("responder", format!("connect loop saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("connection timeout");
}

async fn recv_request_event(
    requester: &mut ReqResTestSwarm,
    responder: &mut ReqResTestSwarm,
    transcript: &DriverTranscript,
) -> (
    PeerId,
    ConnectionId,
    request_response::Message<NockchainRequest, NockchainResponse>,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    event = requester.select_next_some() => {
                        transcript.record("requester", format!("request pump saw {event:?}"));
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                                request_response::Event::Message {
                                    peer,
                                    connection_id,
                                    message,
                                },
                            )) => {
                                transcript.record(
                                    "responder",
                                    format!(
                                        "received driver-path request from {peer} on {connection_id:?}"
                                    ),
                                );
                                return (peer, connection_id, message);
                            }
                            other => transcript.record("responder", format!("request wait saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("request event timeout")
}

async fn recv_response_event(
    requester: &mut ReqResTestSwarm,
    responder: &mut ReqResTestSwarm,
    transcript: &DriverTranscript,
) -> NockchainResponse {
    tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    event = requester.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                                request_response::Event::Message { peer, message, .. },
                            )) => match message {
                                request_response::Message::Response {
                                    request_id,
                                    response,
                                } => {
                                    transcript.record(
                                        "requester",
                                        format!("received response for {request_id:?} from {peer}"),
                                    );
                                    return response;
                                }
                                request_response::Message::Request { request_id, .. } => {
                                    transcript.record(
                                        "requester",
                                        format!("unexpected inbound request {request_id:?}"),
                                    );
                                }
                            },
                            other => transcript.record("requester", format!("response wait saw {other:?}")),
                        }
                    }
                    event = responder.select_next_some() => {
                        transcript.record("responder", format!("response pump saw {event:?}"));
                    }
                }
            }
        })
        .await
        .expect("response event timeout")
}
async fn recv_outbound_failure_event(
    requester: &mut ReqResTestSwarm,
    responder: &mut ReqResTestSwarm,
    transcript: &DriverTranscript,
) -> request_response::OutboundFailure {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            tokio::select! {
                event = requester.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                            request_response::Event::OutboundFailure {
                                peer,
                                request_id,
                                error,
                                ..
                            },
                        )) => {
                            transcript.record(
                                "requester",
                                format!(
                                    "outbound failure for {request_id:?} to {peer}: {error:?}"
                                ),
                            );
                            return error;
                        }
                        other => transcript.record(
                            "requester",
                            format!("failure wait saw {other:?}"),
                        ),
                    }
                }
                event = responder.select_next_some() => {
                    transcript.record("responder", format!("failure pump saw {event:?}"));
                }
            }
        }
    })
    .await
    .expect("outbound failure timeout")
}

async fn recv_swarm_action(swarm_rx: &mut tokio::sync::mpsc::Receiver<SwarmAction>) -> SwarmAction {
    tokio::time::timeout(Duration::from_secs(15), swarm_rx.recv())
        .await
        .expect("swarm action timeout")
        .expect("swarm action channel should stay open")
}

fn scry_some_raw_tx(seed: u64, payload_len: usize) -> NounSlab {
    let mut slab = NounSlab::new();
    let tx_id = T(
        &mut slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    );
    let payload = Atom::from_value(&mut slab, vec![0xCDu8; payload_len])
        .expect("payload atom should build")
        .as_noun();
    let raw_tx = T(&mut slab, &[tx_id, payload]);
    let scry_some = T(&mut slab, &[D(0), D(0), raw_tx]);
    slab.set_root(scry_some);
    slab
}

fn tx_result_item_from_scry(item_id: u32, scry_res: &NounSlab) -> BatchResultItem {
    let mut response_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let response = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut response_slab,
    );
    let Right(Ok(NockchainResponse::Result { message })) = response else {
        panic!("expected tx result response");
    };
    let envelope =
        response_envelope_from_result_message(&message).expect("tx result envelope should decode");
    BatchResultItem {
        item_id,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope),
    }
}

fn tx_result_outcome_for_seed(seed: u64, payload_len: usize) -> RequestExecutionOutcome {
    let scry_res = scry_some_raw_tx(seed, payload_len);
    let mut response_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let response = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut response_slab,
    );
    let Right(Ok(NockchainResponse::Result { message })) = response else {
        panic!("expected tx result response");
    };
    let envelope = response_envelope_from_result_message(&message)
        .expect("tx response envelope should decode");
    RequestExecutionOutcome::Result {
        response: NockchainResponse::Result { message },
        envelope,
    }
}

fn tx_result_outcome(item_id: u32, payload_len: usize) -> RequestExecutionOutcome {
    tx_result_outcome_for_seed(80_000 + u64::from(item_id), payload_len)
}

fn tx_result_message_bytes_for_seed(seed: u64, payload_len: usize) -> usize {
    match tx_result_outcome_for_seed(seed, payload_len) {
        RequestExecutionOutcome::Result { envelope, .. } => envelope.message.len(),
        RequestExecutionOutcome::NotFound => 0,
    }
}

fn tx_result_message_bytes(item_id: u32, payload_len: usize) -> usize {
    match tx_result_outcome(item_id, payload_len) {
        RequestExecutionOutcome::Result { envelope, .. } => envelope.message.len(),
        RequestExecutionOutcome::NotFound => 0,
    }
}

fn build_tx_payload_fit_items(item_count: usize) -> Vec<BatchRequestItem> {
    (0..item_count)
        .map(|idx| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_raw_tx_request(20_000 + idx as u64)),
        })
        .collect()
}

async fn run_responder_payload_fit_estimator(
    estimator_kind: ResponderHintEstimatorKind,
    items: &[BatchRequestItem],
    payload_lens: &[usize],
    actual_message_bytes: &[usize],
    response_cap_bytes: usize,
    fallback_message_bytes: usize,
    seed_hint_message_bytes: usize,
) -> ResponderPayloadFitRun {
    assert_eq!(
        items.len(),
        payload_lens.len(),
        "payload-fit scenario requires one payload length per item"
    );
    assert_eq!(
        items.len(),
        actual_message_bytes.len(),
        "payload-fit scenario requires one actual response size per item"
    );

    let estimator = Arc::new(StdMutex::new(ResponderHintEstimator::new(
        estimator_kind,
        fallback_message_bytes,
        Some(seed_hint_message_bytes),
    )));
    let starting_estimate_message_bytes = estimator
        .lock()
        .unwrap()
        .current_message_bytes()
        .or(Some(fallback_message_bytes));
    let payload_lens = Arc::new(payload_lens.to_vec());
    let actual_message_bytes = Arc::new(actual_message_bytes.to_vec());
    let results = execute_batch_request_items(
        items,
        response_cap_bytes,
        {
            let estimator = Arc::clone(&estimator);
            let actual_message_bytes = Arc::clone(&actual_message_bytes);
            move |item| {
                let estimator = Arc::clone(&estimator);
                let actual_message_bytes = Arc::clone(&actual_message_bytes);
                let item_id = item.item_id;
                async move {
                    let idx = (item_id - 1) as usize;
                    let (message_bytes, source) = estimator
                        .lock()
                        .unwrap()
                        .estimate_message_bytes(actual_message_bytes[idx]);
                    Ok(Some(BatchItemResponseEstimate {
                        request_kind: "raw-tx-by-id",
                        envelope: BatchItemResponseEnvelopeEstimate::HeardTx {
                            tx_id: format!("tx-{item_id}"),
                        },
                        message_bytes,
                        source,
                    }))
                }
            }
        },
        {
            let estimator = Arc::clone(&estimator);
            let payload_lens = Arc::clone(&payload_lens);
            let actual_message_bytes = Arc::clone(&actual_message_bytes);
            move |item| {
                let estimator = Arc::clone(&estimator);
                let payload_lens = Arc::clone(&payload_lens);
                let actual_message_bytes = Arc::clone(&actual_message_bytes);
                let item_id = item.item_id;
                async move {
                    let idx = (item_id - 1) as usize;
                    let request_seed = 20_000 + idx as u64;
                    let outcome = tx_result_outcome_for_seed(request_seed, payload_lens[idx]);
                    estimator
                        .lock()
                        .unwrap()
                        .record_observation(actual_message_bytes[idx]);
                    BatchItemExecutionOutcome::Completed(outcome)
                }
            }
        },
    )
    .await
    .expect("responder payload-fit scenario should execute");

    let response_bytes =
        batch_result_encoded_bytes(&results).expect("payload-fit results should encode");
    let estimate_source = if estimator_kind == ResponderHintEstimatorKind::OracleActual {
        String::from("oracle_actual")
    } else {
        estimator_kind.label().to_string()
    };
    let ending_estimate_message_bytes = estimator.lock().unwrap().current_message_bytes();
    ResponderPayloadFitRun {
        estimator: estimator_kind,
        estimate_source,
        starting_estimate_message_bytes,
        ending_estimate_message_bytes,
        response_bytes,
        result_items: results
            .iter()
            .filter(|item| item.status == BatchResultStatus::Result)
            .count(),
        not_found_items: results
            .iter()
            .filter(|item| item.status == BatchResultStatus::NotFound)
            .count(),
        backpressure_items: results
            .iter()
            .filter(|item| item.error == Some(BatchErrorClass::Backpressure))
            .count(),
        too_large_items: results
            .iter()
            .filter(|item| item.error == Some(BatchErrorClass::TooLarge))
            .count(),
        stop_reason: batch_result_stop_reason(&results).to_string(),
    }
}

fn batch_result_item_from_result_message(item_id: u32, message: &[u8]) -> BatchResultItem {
    let envelope = response_envelope_from_result_message(message)
        .expect("result message should decode into an envelope");
    BatchResultItem {
        item_id,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope),
    }
}

fn checkpoint_requester_replay_counts(fit_blocks: usize) -> Vec<usize> {
    let mut replay_counts = [1usize, 2, 8, 16, 32, 64, 128]
        .into_iter()
        .filter(|count| *count <= fit_blocks)
        .collect::<Vec<_>>();
    if fit_blocks >= 16 {
        let near_cap_tail = fit_blocks - 1;
        if !replay_counts.contains(&near_cap_tail) {
            replay_counts.push(near_cap_tail);
        }
    }
    if fit_blocks > 0 && replay_counts.last().copied() != Some(fit_blocks) {
        replay_counts.push(fit_blocks);
    }
    replay_counts.sort_unstable();
    replay_counts.dedup();
    replay_counts
}

fn stable_checkpoint_report_metrics() -> Arc<NockchainP2PMetrics> {
    let registry = gnort::MetricsRegistry::new(gnort::RegistryConfig {
        observation_period: Some(Duration::from_secs(600)),
        delay_time: Some(Duration::from_secs(600)),
        ..Default::default()
    });
    Arc::new(NockchainP2PMetrics::register(&registry).expect("Could not register metrics"))
}

fn isolated_test_metrics() -> Arc<NockchainP2PMetrics> {
    let registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
    Arc::new(NockchainP2PMetrics::register(&registry).expect("Could not register metrics"))
}

fn percentile_or_zero(sorted: &[usize], q: f64) -> usize {
    if sorted.is_empty() {
        0
    } else {
        percentile(sorted, q)
    }
}

fn average_or_zero(values: &[usize]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<usize>() as f64 / values.len() as f64
    }
}

fn percentile_f64_or_zero(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        0.0
    } else {
        let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted[idx]
    }
}

fn average_f64_or_zero(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn outbound_request_item_count(request: &NockchainRequest) -> usize {
    batch_request_item_count(request).unwrap_or(1)
}

fn routed_response_poke_count(metrics: &NockchainP2PMetrics) -> usize {
    let acked = metrics.responses_acked_heard_block.fetch_add(0)
        + metrics.responses_acked_heard_tx.fetch_add(0)
        + metrics.responses_acked_heard_elders.fetch_add(0);
    let nacked = metrics.responses_nacked_heard_block.fetch_add(0)
        + metrics.responses_nacked_heard_tx.fetch_add(0)
        + metrics.responses_nacked_heard_elders.fetch_add(0);
    let erred = metrics.responses_erred_heard_block.fetch_add(0)
        + metrics.responses_erred_heard_tx.fetch_add(0)
        + metrics.responses_erred_heard_elders.fetch_add(0);
    acked + nacked + erred
}

fn batch_result_not_found_item(item_id: u32) -> BatchResultItem {
    BatchResultItem {
        item_id,
        status: BatchResultStatus::NotFound,
        error: None,
        envelope: None,
    }
}

fn tx_request_items(seeds: &[u64]) -> Vec<BatchRequestItem> {
    seeds
        .iter()
        .enumerate()
        .map(|(idx, seed)| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_raw_tx_request(*seed)),
        })
        .collect()
}

fn tx_result_items_from_seeds(seeds: &[u64], payload_len: usize) -> Vec<BatchResultItem> {
    seeds
        .iter()
        .enumerate()
        .map(|(idx, seed)| {
            let scry_res = scry_some_raw_tx(*seed, payload_len);
            tx_result_item_from_scry(idx as u32 + 1, &scry_res)
        })
        .collect()
}

fn run_recovery_enqueue_sample(
    label: &str,
    request_mix: &str,
    stages: &[Vec<Vec<u8>>],
) -> RecoveryEnqueueSample {
    let metrics = isolated_test_metrics();
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let mut pending_gen2_batches = BTreeMap::<PeerId, PendingGen2Batch>::new();
    let mut equix_builder = equix::EquiXBuilder::new();

    let mut input_request_count = 0usize;
    let mut unique_request_count = 0usize;
    let mut duplicate_requests = 0usize;
    let mut outbound_request_count = 0usize;
    let mut gen2_batch_request_count = 0usize;
    let mut gen1_request_count = 0usize;
    let mut single_item_batch_count = 0usize;
    let mut outbound_items = Vec::new();

    let started = Instant::now();
    for stage in stages {
        for request_message in stage {
            input_request_count += 1;
            match queue_pending_gen2_batch_request(
                &metrics, &mut pending_gen2_batches, peer, request_message, 0, false,
            )
            .expect("recovery enqueue sample should queue request")
            {
                PendingBatchInsertOutcome::Inserted { .. } => unique_request_count += 1,
                PendingBatchInsertOutcome::Duplicate => duplicate_requests += 1,
            }
        }

        while let Some(request_context) = take_pending_batch_request(
            &mut pending_gen2_batches, peer, &local_peer, &mut equix_builder,
        )
        .expect("recovery enqueue sample should flush pending batch")
        {
            outbound_request_count += 1;
            let item_count = outbound_request_item_count(&request_context.request);
            outbound_items.push(item_count);
            match request_context.generation {
                ReqResGeneration::Gen1 => gen1_request_count += 1,
                ReqResGeneration::Gen2 => {
                    gen2_batch_request_count += 1;
                    if item_count == 1 {
                        single_item_batch_count += 1;
                    }
                }
            }
        }
    }
    let total_ms = started.elapsed().as_secs_f64() * 1_000.0;
    outbound_items.sort_unstable();

    RecoveryEnqueueSample {
        label: label.to_string(),
        request_mix: request_mix.to_string(),
        stage_count: stages.len(),
        input_request_count,
        unique_request_count,
        duplicate_requests,
        outbound_request_count,
        gen2_batch_request_count,
        gen1_request_count,
        single_item_batch_count,
        min_outbound_items: outbound_items.first().copied().unwrap_or(0),
        p50_outbound_items: percentile_or_zero(&outbound_items, 0.50),
        max_outbound_items: outbound_items.last().copied().unwrap_or(0),
        average_outbound_items: average_or_zero(&outbound_items),
        total_ms,
    }
}

fn tx_tuning_event(
    seed: u64,
    at_ms: u64,
    peer_index: usize,
    estimated_response_bytes: usize,
) -> RecoveryTimedRequest {
    RecoveryTimedRequest {
        at_ms,
        peer_index,
        message: jam_raw_tx_request(seed),
        estimated_response_bytes,
        contains_response_budget_item: false,
    }
}

fn build_recovery_tuning_workloads() -> Vec<RecoveryTuningWorkload> {
    let small_tx_response_bytes = tx_result_message_bytes(1, 512);
    let large_tx_response_bytes = tx_result_message_bytes(1, 6144);
    let mut workloads = Vec::new();

    workloads.push(RecoveryTuningWorkload {
        label: "tx-burst-128",
        request_mix: "raw-tx-burst",
        events: (0..128)
            .map(|idx| tx_tuning_event(100_000 + idx, 0, 0, small_tx_response_bytes))
            .collect(),
    });

    for gap_ms in [1u64, 5, 10, 15, 20, 25, 50, 100] {
        workloads.push(RecoveryTuningWorkload {
            label: match gap_ms {
                1 => "tx-drip-1ms-128",
                5 => "tx-drip-5ms-128",
                10 => "tx-drip-10ms-128",
                15 => "tx-drip-15ms-128",
                20 => "tx-drip-20ms-128",
                25 => "tx-drip-25ms-128",
                50 => "tx-drip-50ms-128",
                100 => "tx-drip-100ms-128",
                _ => unreachable!(),
            },
            request_mix: "raw-tx-drip",
            events: (0..128)
                .map(|idx| {
                    tx_tuning_event(
                        101_000 + idx,
                        idx.saturating_mul(gap_ms),
                        0,
                        small_tx_response_bytes,
                    )
                })
                .collect(),
        });
    }

    workloads.push(RecoveryTuningWorkload {
        label: "tx-duplicate-storm-32x4",
        request_mix: "raw-tx-duplicate-replay",
        events: (0u64..4)
            .flat_map(|wave| {
                (0..32).map(move |idx| {
                    tx_tuning_event(
                        102_000 + idx,
                        wave.saturating_mul(5),
                        0,
                        small_tx_response_bytes,
                    )
                })
            })
            .collect(),
    });

    workloads.push(RecoveryTuningWorkload {
        label: "tx-tail-heavy-8-8-8-64",
        request_mix: "raw-tx-tail-heavy",
        events: [
            (0u64, 8usize, 103_000u64),
            (5, 8, 103_100),
            (25, 8, 103_200),
            (100, 64, 103_300),
        ]
        .into_iter()
        .flat_map(|(at_ms, count, base_seed)| {
            (0..count).map(move |idx| {
                tx_tuning_event(base_seed + idx as u64, at_ms, 0, small_tx_response_bytes)
            })
        })
        .collect(),
    });

    workloads.push(RecoveryTuningWorkload {
        label: "tx-large-burst-64",
        request_mix: "raw-tx-large-payload",
        events: (0..64)
            .map(|idx| tx_tuning_event(104_000 + idx, 0, 0, large_tx_response_bytes))
            .collect(),
    });

    workloads.push(RecoveryTuningWorkload {
        label: "tx-multi-peer-4x32",
        request_mix: "raw-tx-many-requesters",
        events: (0..4)
            .flat_map(|peer_index| {
                (0..32).map(move |idx| {
                    tx_tuning_event(
                        105_000 + peer_index as u64 * 1_000 + idx,
                        0,
                        peer_index,
                        small_tx_response_bytes,
                    )
                })
            })
            .collect(),
    });

    workloads
}

struct RecoveryTuningAccumulator {
    flush_reason_histogram: BTreeMap<String, usize>,
    outbound_items: Vec<usize>,
    added_delays_ms: Vec<f64>,
    response_fill_ratios: Vec<f64>,
    payload_fill_ratios: Vec<f64>,
    outbound_request_count: usize,
    gen2_batch_request_count: usize,
    gen1_request_count: usize,
    single_item_batch_count: usize,
    last_flush_ms: u64,
}

impl RecoveryTuningAccumulator {
    fn new() -> Self {
        Self {
            flush_reason_histogram: BTreeMap::new(),
            outbound_items: Vec::new(),
            added_delays_ms: Vec::new(),
            response_fill_ratios: Vec::new(),
            payload_fill_ratios: Vec::new(),
            outbound_request_count: 0,
            gen2_batch_request_count: 0,
            gen1_request_count: 0,
            single_item_batch_count: 0,
            last_flush_ms: 0,
        }
    }

    fn record_flush(
        &mut self,
        reason: PendingBatchFlushReason,
        generation: ReqResGeneration,
        item_count: usize,
        payload_bytes: usize,
        estimated_response_bytes: usize,
        limits: ReqResRuntimeLimits,
        now_ms: u64,
        enqueued_at_ms: Vec<u64>,
    ) {
        *self
            .flush_reason_histogram
            .entry(reason.as_str().to_string())
            .or_insert(0) += 1;
        self.outbound_request_count += 1;
        self.outbound_items.push(item_count);
        match generation {
            ReqResGeneration::Gen1 => self.gen1_request_count += 1,
            ReqResGeneration::Gen2 => {
                self.gen2_batch_request_count += 1;
                if item_count == 1 {
                    self.single_item_batch_count += 1;
                }
            }
        }
        for enqueued_ms in enqueued_at_ms {
            self.added_delays_ms
                .push(now_ms.saturating_sub(enqueued_ms) as f64);
        }
        if limits.gen2_batch_max_bytes > 0 {
            self.response_fill_ratios
                .push(estimated_response_bytes as f64 / limits.gen2_batch_max_bytes as f64);
            self.payload_fill_ratios
                .push(payload_bytes as f64 / limits.gen2_batch_max_bytes as f64);
        }
        self.last_flush_ms = self.last_flush_ms.max(now_ms);
    }
}

fn flush_recovery_tuning_peer(
    peer: PeerId,
    reason: PendingBatchFlushReason,
    now_ms: u64,
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    pending_enqueued_at_ms: &mut BTreeMap<PeerId, Vec<u64>>,
    local_peer: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    limits: ReqResRuntimeLimits,
    accumulator: &mut RecoveryTuningAccumulator,
) {
    let Some(pending_batch) = pending_gen2_batches.get(&peer) else {
        return;
    };
    if pending_batch.is_empty() {
        return;
    }
    let item_count = pending_batch.items.len();
    let payload_bytes = pending_batch.payload_bytes;
    let estimated_response_bytes = pending_batch.estimated_response_bytes;
    let enqueued_at_ms = pending_enqueued_at_ms.remove(&peer).unwrap_or_default();
    let request_context =
        take_pending_batch_request(pending_gen2_batches, peer, local_peer, equix_builder)
            .expect("recovery tuning flush should build request")
            .expect("recovery tuning flush should have pending items");
    accumulator.record_flush(
        reason, request_context.generation, item_count, payload_bytes, estimated_response_bytes,
        limits, now_ms, enqueued_at_ms,
    );
}

fn process_recovery_tuning_tick(
    now_ms: u64,
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    pending_enqueued_at_ms: &mut BTreeMap<PeerId, Vec<u64>>,
    local_peer: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    limits: ReqResRuntimeLimits,
    accumulator: &mut RecoveryTuningAccumulator,
) {
    let peers = pending_gen2_batches.keys().copied().collect::<Vec<_>>();
    for peer in peers {
        let should_flush = pending_gen2_batches
            .get_mut(&peer)
            .is_some_and(PendingGen2Batch::should_flush_on_tick);
        if should_flush {
            flush_recovery_tuning_peer(
                peer,
                PendingBatchFlushReason::CoalesceTick,
                now_ms,
                pending_gen2_batches,
                pending_enqueued_at_ms,
                local_peer,
                equix_builder,
                limits,
                accumulator,
            );
        }
    }
}

fn run_recovery_tuning_sample(
    workload: &RecoveryTuningWorkload,
    coalesce_window_ms: u64,
    limits: ReqResRuntimeLimits,
) -> RecoveryTuningSample {
    let metrics = isolated_test_metrics();
    let local_peer = PeerId::random();
    let peers = (0..4).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut pending_gen2_batches = BTreeMap::<PeerId, PendingGen2Batch>::new();
    let mut pending_enqueued_at_ms = BTreeMap::<PeerId, Vec<u64>>::new();
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut accumulator = RecoveryTuningAccumulator::new();
    let mut input_request_count = 0usize;
    let mut unique_request_count = 0usize;
    let mut duplicate_requests = 0usize;
    let mut next_tick_ms = coalesce_window_ms;
    let mut events = workload.events.clone();
    events.sort_by_key(|event| (event.at_ms, event.peer_index));

    for event in events {
        while next_tick_ms <= event.at_ms {
            process_recovery_tuning_tick(
                next_tick_ms, &mut pending_gen2_batches, &mut pending_enqueued_at_ms, &local_peer,
                &mut equix_builder, limits, &mut accumulator,
            );
            next_tick_ms = next_tick_ms.saturating_add(coalesce_window_ms);
        }

        input_request_count += 1;
        let peer = peers[event.peer_index % peers.len()];
        let flush_reason = pending_gen2_batches
            .get(&peer)
            .map(|pending_batch| {
                pending_batch_pre_insert_flush_reason(
                    pending_batch,
                    event.message.len(),
                    event.estimated_response_bytes,
                    event.contains_response_budget_item,
                    limits,
                )
            })
            .transpose()
            .expect("recovery tuning pre-insert limit check should succeed")
            .flatten();
        if let Some(reason) = flush_reason {
            flush_recovery_tuning_peer(
                peer, reason, event.at_ms, &mut pending_gen2_batches, &mut pending_enqueued_at_ms,
                &local_peer, &mut equix_builder, limits, &mut accumulator,
            );
        }

        let insert_outcome = queue_pending_gen2_batch_request(
            &metrics, &mut pending_gen2_batches, peer, &event.message,
            event.estimated_response_bytes, event.contains_response_budget_item,
        )
        .expect("recovery tuning enqueue should queue request");

        match insert_outcome {
            PendingBatchInsertOutcome::Duplicate => duplicate_requests += 1,
            PendingBatchInsertOutcome::Inserted {
                item_count,
                payload_bytes,
                estimated_response_bytes,
                contains_response_budget_item,
            } => {
                unique_request_count += 1;
                pending_enqueued_at_ms
                    .entry(peer)
                    .or_default()
                    .push(event.at_ms);
                if let Some(reason) = inserted_batch_flush_reason(
                    item_count, payload_bytes, estimated_response_bytes,
                    contains_response_budget_item, limits,
                ) {
                    flush_recovery_tuning_peer(
                        peer, reason, event.at_ms, &mut pending_gen2_batches,
                        &mut pending_enqueued_at_ms, &local_peer, &mut equix_builder, limits,
                        &mut accumulator,
                    );
                }
            }
        }
    }

    while pending_gen2_batches
        .values()
        .any(|pending_batch| !pending_batch.is_empty())
    {
        process_recovery_tuning_tick(
            next_tick_ms, &mut pending_gen2_batches, &mut pending_enqueued_at_ms, &local_peer,
            &mut equix_builder, limits, &mut accumulator,
        );
        next_tick_ms = next_tick_ms.saturating_add(coalesce_window_ms);
    }

    accumulator.outbound_items.sort_unstable();
    accumulator
        .added_delays_ms
        .sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(CmpOrdering::Equal));
    accumulator
        .response_fill_ratios
        .sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(CmpOrdering::Equal));
    accumulator
        .payload_fill_ratios
        .sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap_or(CmpOrdering::Equal));

    RecoveryTuningSample {
        label: workload.label.to_string(),
        request_mix: workload.request_mix.to_string(),
        coalesce_window_ms,
        batch_max_items: limits.gen2_batch_max_items,
        input_request_count,
        unique_request_count,
        duplicate_requests,
        outbound_request_count: accumulator.outbound_request_count,
        gen2_batch_request_count: accumulator.gen2_batch_request_count,
        gen1_request_count: accumulator.gen1_request_count,
        single_item_batch_count: accumulator.single_item_batch_count,
        flush_reason_histogram: accumulator.flush_reason_histogram,
        min_outbound_items: accumulator.outbound_items.first().copied().unwrap_or(0),
        p50_outbound_items: percentile_or_zero(&accumulator.outbound_items, 0.50),
        p95_outbound_items: percentile_or_zero(&accumulator.outbound_items, 0.95),
        max_outbound_items: accumulator.outbound_items.last().copied().unwrap_or(0),
        average_outbound_items: average_or_zero(&accumulator.outbound_items),
        p50_added_delay_ms: percentile_f64_or_zero(&accumulator.added_delays_ms, 0.50),
        p95_added_delay_ms: percentile_f64_or_zero(&accumulator.added_delays_ms, 0.95),
        max_added_delay_ms: accumulator.added_delays_ms.last().copied().unwrap_or(0.0),
        average_added_delay_ms: average_f64_or_zero(&accumulator.added_delays_ms),
        total_simulated_ms: accumulator.last_flush_ms,
        average_response_fill_ratio: average_f64_or_zero(&accumulator.response_fill_ratios),
        p95_response_fill_ratio: percentile_f64_or_zero(&accumulator.response_fill_ratios, 0.95),
        max_response_fill_ratio: accumulator
            .response_fill_ratios
            .last()
            .copied()
            .unwrap_or(0.0),
        average_payload_fill_ratio: average_f64_or_zero(&accumulator.payload_fill_ratios),
        p95_payload_fill_ratio: percentile_f64_or_zero(&accumulator.payload_fill_ratios, 0.95),
        max_payload_fill_ratio: accumulator
            .payload_fill_ratios
            .last()
            .copied()
            .unwrap_or(0.0),
    }
}

fn run_recovery_tuning_sweep(
    config: &LibP2PConfig,
) -> (Vec<RecoveryTuningConfig>, Vec<RecoveryTuningSample>) {
    let windows_ms = env_u64_list(
        "REQ_RES_GEN2_TUNING_WINDOWS_MS",
        &[1, 5, 10, 25, 50, 100, 250],
    );
    let batch_max_items = env_usize_list(
        "REQ_RES_GEN2_TUNING_BATCH_MAX_ITEMS",
        &[32, 64, 128, 256, 512],
    );
    let base_limits = runtime_limits_from_config(config);
    let workloads = build_recovery_tuning_workloads();
    let mut tuning_config = Vec::new();
    let mut tuning_samples = Vec::new();

    for coalesce_window_ms in windows_ms {
        for batch_max_items in &batch_max_items {
            let limits = ReqResRuntimeLimits {
                gen2_batch_max_items: *batch_max_items,
                ..base_limits
            };
            tuning_config.push(RecoveryTuningConfig {
                coalesce_window_ms,
                batch_max_items: *batch_max_items,
                batch_max_bytes: limits.gen2_batch_max_bytes,
                item_max_bytes: limits.gen2_item_max_bytes,
                block_response_budget_bytes: block_batch_response_budget_bytes(limits),
                max_inflight_per_peer: limits.gen2_max_inflight_per_peer,
            });
            for workload in &workloads {
                tuning_samples.push(run_recovery_tuning_sample(
                    workload, coalesce_window_ms, limits,
                ));
            }
        }
    }

    (tuning_config, tuning_samples)
}

#[derive(Default)]
struct SwarmFollowupCounts {
    queue_kernel_requests: usize,
    retry_requests: usize,
    other_actions: usize,
}

fn seed_connected_peer(state: &mut P2PState, peer: PeerId, connection_seed: usize) {
    let connection_id = ConnectionId::new_unchecked(connection_seed);
    let remote_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 45_000 + connection_seed)
        .parse()
        .expect("benchmark remote addr should parse");
    let local_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", 55_000 + connection_seed)
        .parse()
        .expect("benchmark local addr should parse");
    state.track_connection(
        connection_id,
        peer,
        &remote_addr,
        libp2p::core::ConnectedPoint::Listener {
            local_addr,
            send_back_addr: remote_addr.clone(),
        },
    );
    state.observe_peer_generation(peer, ReqResGeneration::Gen2);
}

async fn drain_swarm_followup_counts(
    swarm_rx: &mut tokio::sync::mpsc::Receiver<SwarmAction>,
) -> SwarmFollowupCounts {
    let mut counts = SwarmFollowupCounts::default();
    loop {
        match tokio::time::timeout(Duration::from_millis(20), swarm_rx.recv()).await {
            Ok(Some(SwarmAction::QueueKernelRequest { .. })) => counts.queue_kernel_requests += 1,
            Ok(Some(SwarmAction::RetryRequests { requests, .. })) => {
                counts.retry_requests += requests.len();
            }
            Ok(Some(_)) => counts.other_actions += 1,
            Ok(None) | Err(_) => break,
        }
    }
    counts
}

async fn drain_expected_swarm_followups(
    swarm_rx: &mut tokio::sync::mpsc::Receiver<SwarmAction>,
) -> usize {
    let counts = drain_swarm_followup_counts(swarm_rx).await;
    assert_eq!(
        counts.other_actions, 0,
        "requester-cost report should only emit follow-up request actions"
    );
    counts.queue_kernel_requests + counts.retry_requests
}

async fn drain_effects_into_driver(
    effect_handle: &NockAppHandle,
    swarm_tx: &tokio::sync::mpsc::Sender<SwarmAction>,
    connected_peers: &[PeerId],
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
) -> usize {
    let mut handled = 0usize;
    loop {
        let effect = match tokio::time::timeout(
            Duration::from_millis(50),
            effect_handle.next_effect(),
        )
        .await
        {
            Ok(effect) => effect.expect("effect receiver should stay open"),
            Err(_) => break,
        };
        handle_effect(
            effect,
            swarm_tx.clone(),
            connected_peers.to_vec(),
            false,
            Arc::clone(driver_state),
            metrics.clone(),
        )
        .await
        .expect("benchmark effect handling should succeed");
        handled += 1;
    }
    handled
}

async fn run_tx_live_response_sample(
    label: &str,
    unique_count: usize,
    wave_count: usize,
    payload_len: usize,
) -> RecoveryResponseSample {
    let transcript = DriverTranscript::silent();
    transcript.record(
            "scenario",
            format!(
                "live tx replay sample {label} unique_count={unique_count} wave_count={wave_count} payload_len={payload_len}"
            ),
        );
    let live_traffic = build_live_traffic_cop(start_nockchain_app().await);
    drain_effects(&live_traffic.effect_handle).await;

    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let tx_seeds = (0..unique_count)
        .map(|idx| 74_000 + idx as u64)
        .collect::<Vec<_>>();
    let request_items = tx_request_items(&tx_seeds);
    let result_items = tx_result_items_from_seeds(&tx_seeds, payload_len);
    let wave_response_bytes =
        batch_result_encoded_bytes(&result_items).expect("live tx replay sample should encode");
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(128);
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut kernel_effect_count = 0usize;

    let started = Instant::now();
    for wave in 0..wave_count {
        let request_id = fresh_outbound_request_id();
        driver_state.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
                false,
            ),
        );
        run_driver_with_timeout(
            &transcript,
            "live tx replay response processing",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(900 + wave),
                request_response::Message::Response {
                    request_id,
                    response: NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    },
                },
                swarm_tx.clone(),
                &mut equix_builder,
                local_peer,
                live_traffic.traffic.clone(),
                metrics.clone(),
                Arc::clone(&driver_state),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("live tx replay response should process");
        kernel_effect_count += drain_effects_into_driver(
            &live_traffic.effect_handle,
            &swarm_tx,
            &[peer],
            &driver_state,
            &metrics,
        )
        .await;
    }
    drop(swarm_tx);

    let elapsed = started.elapsed();
    let followup_counts = drain_swarm_followup_counts(&mut swarm_rx).await;

    RecoveryResponseSample {
        label: label.to_string(),
        request_mix: String::from("raw-tx-live"),
        wave_count,
        input_item_count: unique_count * wave_count,
        useful_pokes: routed_response_poke_count(&metrics),
        duplicate_gates: metrics.tx_seen_cache_hits.fetch_add(0),
        kernel_effect_count,
        followup_request_count: followup_counts.queue_kernel_requests
            + followup_counts.retry_requests,
        response_bytes: wave_response_bytes * wave_count,
        total_ms: elapsed.as_secs_f64() * 1_000.0,
        per_item_us: elapsed.as_micros() as f64 / (unique_count * wave_count) as f64,
    }
}

async fn run_tx_driver_seen_replay_response_sample(
    label: &str,
    unique_count: usize,
    wave_count: usize,
    payload_len: usize,
) -> RecoveryResponseSample {
    let transcript = DriverTranscript::silent();
    transcript.record(
            "scenario",
            format!(
                "driver replay sample {label} unique_count={unique_count} wave_count={wave_count} payload_len={payload_len}"
            ),
        );
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let tx_seeds = (0..unique_count)
        .map(|idx| 76_000 + idx as u64)
        .collect::<Vec<_>>();
    let tx_ids = tx_seeds
        .iter()
        .map(|seed| {
            let mut tx_slab = NounSlab::new();
            let noun = tip5_tuple(&mut tx_slab, *seed);
            let space = tx_slab.noun_space();
            tip5_hash_to_base58(noun, &space).expect("tx id tuple should convert to base58")
        })
        .collect::<Vec<_>>();
    let result_items = tx_result_items_from_seeds(&tx_seeds, payload_len);
    let wave_response_bytes =
        batch_result_encoded_bytes(&result_items).expect("driver replay sample should encode");

    let started = Instant::now();
    for wave in 0..wave_count {
        if wave == 1 {
            let mut state_guard = state_arc.lock().await;
            for tx_id in &tx_ids {
                state_guard.seen_txs.insert(tx_id.clone());
            }
        }

        for result_item in &result_items {
            let envelope = result_item
                .envelope
                .as_ref()
                .expect("tx replay sample should carry response envelopes");
            let response =
                response_fact_from_envelope(envelope).expect("tx response should decode");
            let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(8);
            match route_response_fact(
                peer, response, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
            )
            .await
            {
                Ok(()) => {}
                Err(NockAppError::OneShotRecvError(_)) => {}
                Err(err) => panic!("driver replay fact routing should not fail: {err:?}"),
            }
        }
    }

    let elapsed = started.elapsed();

    RecoveryResponseSample {
        label: label.to_string(),
        request_mix: String::from("raw-tx-replay-after-driver-seen"),
        wave_count,
        input_item_count: unique_count * wave_count,
        useful_pokes: routed_response_poke_count(&metrics),
        duplicate_gates: metrics.tx_seen_cache_hits.fetch_add(0),
        kernel_effect_count: 0,
        followup_request_count: 0,
        response_bytes: wave_response_bytes * wave_count,
        total_ms: elapsed.as_secs_f64() * 1_000.0,
        per_item_us: elapsed.as_micros() as f64 / (unique_count * wave_count) as f64,
    }
}

async fn run_block_all_miss_response_sample(
    label: &str,
    window_len: usize,
    wave_count: usize,
) -> RecoveryResponseSample {
    let transcript = DriverTranscript::silent();
    transcript.record(
            "scenario",
            format!("block all-miss response sample {label} window_len={window_len} wave_count={wave_count}"),
        );
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let alternate_peer = PeerId::random();
    let local_peer = PeerId::random();
    {
        let mut state_guard = driver_state.lock().await;
        seed_connected_peer(&mut state_guard, peer, 20);
        seed_connected_peer(&mut state_guard, alternate_peer, 21);
    }
    let heights = (0..window_len)
        .map(|idx| 88_000 + idx as u64)
        .collect::<Vec<_>>();
    let request_items = heights
        .iter()
        .enumerate()
        .map(|(idx, height)| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_block_by_height_request(*height)),
        })
        .collect::<Vec<_>>();
    let result_items = request_items
        .iter()
        .map(|item| batch_result_not_found_item(item.item_id))
        .collect::<Vec<_>>();
    let wave_response_bytes =
        batch_result_encoded_bytes(&result_items).expect("block all-miss sample should encode");
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(256);
    let mut equix_builder = equix::EquiXBuilder::new();

    let started = Instant::now();
    for wave in 0..wave_count {
        let request_id = fresh_outbound_request_id();
        driver_state.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
                false,
            ),
        );
        run_driver_with_timeout(
            &transcript,
            "block all-miss response processing",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(1_000 + wave),
                request_response::Message::Response {
                    request_id,
                    response: NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    },
                },
                swarm_tx.clone(),
                &mut equix_builder,
                local_peer,
                scripted_traffic.traffic.clone(),
                metrics.clone(),
                Arc::clone(&driver_state),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("block all-miss response should process");
    }
    drop(swarm_tx);

    let elapsed = started.elapsed();
    let followup_counts = drain_swarm_followup_counts(&mut swarm_rx).await;

    RecoveryResponseSample {
        label: label.to_string(),
        request_mix: String::from("block-batch-all-miss"),
        wave_count,
        input_item_count: window_len * wave_count,
        useful_pokes: routed_response_poke_count(&metrics),
        duplicate_gates: metrics.block_seen_cache_hits.fetch_add(0),
        kernel_effect_count: 0,
        followup_request_count: followup_counts.queue_kernel_requests
            + followup_counts.retry_requests,
        response_bytes: wave_response_bytes * wave_count,
        total_ms: elapsed.as_secs_f64() * 1_000.0,
        per_item_us: elapsed.as_micros() as f64 / (window_len * wave_count) as f64,
    }
}

async fn run_block_batch_miss_response_sample(
    label: &str,
    wave_count: usize,
) -> Option<RecoveryResponseSample> {
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        println!(
                "recovery block-batch sample skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
            );
        return None;
    };
    let transcript = DriverTranscript::silent();
    transcript.record(
        "scenario",
        format!("recovery response sample {label} wave_count={wave_count}"),
    );
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let alternate_peer = PeerId::random();
    let local_peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        seed_connected_peer(&mut state_guard, peer, 22);
        seed_connected_peer(&mut state_guard, alternate_peer, 23);
    }
    let block_response_budget_bytes =
        block_batch_response_budget_bytes(runtime_limits_from_config(&LIBP2P_CONFIG));
    let block_window =
        load_heaviest_checkpoint_block_window(&chkjam_path, 8, 32, block_response_budget_bytes)
            .await;
    let heights = block_window
        .iter()
        .map(|(height, _)| *height)
        .collect::<Vec<_>>();
    let request_items = heights
        .iter()
        .enumerate()
        .map(|(idx, height)| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_block_by_height_request(*height)),
        })
        .collect::<Vec<_>>();
    let result_items = std::iter::once(batch_result_not_found_item(1))
        .chain(
            block_window[1..]
                .iter()
                .enumerate()
                .map(|(idx, (_, message))| {
                    batch_result_item_from_result_message(idx as u32 + 2, message)
                }),
        )
        .collect::<Vec<_>>();
    let wave_response_bytes =
        batch_result_encoded_bytes(&result_items).expect("block miss sample should encode");
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(64);
    let mut equix_builder = equix::EquiXBuilder::new();

    let started = Instant::now();
    for wave in 0..wave_count {
        let request_id = fresh_outbound_request_id();
        state_arc.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
                false,
            ),
        );
        run_driver_with_timeout(
            &transcript,
            "recovery block miss response processing",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(1_200 + wave),
                request_response::Message::Response {
                    request_id,
                    response: NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    },
                },
                swarm_tx.clone(),
                &mut equix_builder,
                local_peer,
                scripted_traffic.traffic.clone(),
                metrics.clone(),
                Arc::clone(&state_arc),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("block miss response should process");
    }
    drop(swarm_tx);

    let elapsed = started.elapsed();
    let followup_counts = drain_swarm_followup_counts(&mut swarm_rx).await;

    Some(RecoveryResponseSample {
        label: label.to_string(),
        request_mix: String::from("block-batch-miss"),
        wave_count,
        input_item_count: result_items.len() * wave_count,
        useful_pokes: routed_response_poke_count(&metrics),
        duplicate_gates: metrics.block_seen_cache_hits.fetch_add(0),
        kernel_effect_count: 0,
        followup_request_count: followup_counts.queue_kernel_requests
            + followup_counts.retry_requests,
        response_bytes: wave_response_bytes * wave_count,
        total_ms: elapsed.as_secs_f64() * 1_000.0,
        per_item_us: elapsed.as_micros() as f64 / (result_items.len() * wave_count) as f64,
    })
}

fn batch_result_stop_reason(results: &[BatchResultItem]) -> &'static str {
    for item in results {
        match item.error {
            Some(BatchErrorClass::TooLarge) => return "too_large",
            Some(BatchErrorClass::Backpressure) => return "backpressure_tail",
            _ => {}
        }
    }
    "completed"
}

#[test]
fn responder_hint_estimators_recover_after_poisoned_seed() {
    let seed_hint_message_bytes = 24 * 1024;
    let small_message_bytes = 1024usize;

    let mut observed_max = ResponderHintEstimator::new(
        ResponderHintEstimatorKind::ObservedMax,
        LIBP2P_CONFIG.gen2_item_max_bytes(),
        Some(seed_hint_message_bytes),
    );
    let mut decaying_max = ResponderHintEstimator::new(
        ResponderHintEstimatorKind::DecayingMax75,
        LIBP2P_CONFIG.gen2_item_max_bytes(),
        Some(seed_hint_message_bytes),
    );
    let mut recent_max = ResponderHintEstimator::new(
        ResponderHintEstimatorKind::RecentMaxWindow8,
        LIBP2P_CONFIG.gen2_item_max_bytes(),
        Some(seed_hint_message_bytes),
    );

    for _ in 0..8 {
        observed_max.record_observation(small_message_bytes);
        decaying_max.record_observation(small_message_bytes);
        recent_max.record_observation(small_message_bytes);
    }

    assert_eq!(
        observed_max.estimate_message_bytes(small_message_bytes).0,
        seed_hint_message_bytes
    );
    assert!(
        decaying_max.estimate_message_bytes(small_message_bytes).0 < seed_hint_message_bytes,
        "decaying max should shrink after repeated small observations"
    );
    assert_eq!(
        recent_max.estimate_message_bytes(small_message_bytes).0,
        small_message_bytes
    );
}

#[tokio::test]
async fn responder_payload_fit_poisoning_alternatives_reduce_fit_loss() {
    let limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    let payload_lens = vec![1024usize; 96];
    let items = build_tx_payload_fit_items(payload_lens.len());
    let actual_message_bytes = payload_lens
        .iter()
        .enumerate()
        .map(|(idx, payload_len)| {
            tx_result_message_bytes_for_seed(20_000 + idx as u64, *payload_len)
        })
        .collect::<Vec<_>>();
    let response_cap_bytes = 96 * 1024;
    let seed_hint_message_bytes = 24 * 1024;

    let actual_fit = run_responder_payload_fit_estimator(
        ResponderHintEstimatorKind::OracleActual,
        &items,
        &payload_lens,
        &actual_message_bytes,
        response_cap_bytes,
        limits.gen2_item_max_bytes,
        seed_hint_message_bytes,
    )
    .await;
    let observed_max = run_responder_payload_fit_estimator(
        ResponderHintEstimatorKind::ObservedMax,
        &items,
        &payload_lens,
        &actual_message_bytes,
        response_cap_bytes,
        limits.gen2_item_max_bytes,
        seed_hint_message_bytes,
    )
    .await;
    let decaying_max = run_responder_payload_fit_estimator(
        ResponderHintEstimatorKind::DecayingMax75,
        &items,
        &payload_lens,
        &actual_message_bytes,
        response_cap_bytes,
        limits.gen2_item_max_bytes,
        seed_hint_message_bytes,
    )
    .await;
    let recent_max = run_responder_payload_fit_estimator(
        ResponderHintEstimatorKind::RecentMaxWindow8,
        &items,
        &payload_lens,
        &actual_message_bytes,
        response_cap_bytes,
        limits.gen2_item_max_bytes,
        seed_hint_message_bytes,
    )
    .await;

    assert!(
        actual_fit.result_items > observed_max.result_items,
        "poisoning scenario should expose fit loss for observed_max"
    );
    assert!(
        decaying_max.result_items > observed_max.result_items,
        "decaying max should recover more fit than observed_max"
    );
    assert!(
        recent_max.result_items > observed_max.result_items,
        "recent max window should recover more fit than observed_max"
    );
    assert!(decaying_max.result_items <= actual_fit.result_items);
    assert!(recent_max.result_items <= actual_fit.result_items);
}

fn runtime_limits_from_config(config: &LibP2PConfig) -> ReqResRuntimeLimits {
    ReqResRuntimeLimits {
        request_high_threshold: config.request_high_threshold,
        request_replay_cache_ttl: config.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: config.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: config.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: config.ip_bucket_connection_limit,
        gossip_bucket_capacity: config.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: config.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: config.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: config.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: config.prefetch_window_max.max(1),
        gen2_batch_max_items: config.gen2_batch_max_items(),
        gen2_batch_max_bytes: config.gen2_batch_max_bytes(),
        gen2_item_max_bytes: config.gen2_item_max_bytes(),
        gen2_block_batch_max_response_bytes: config.gen2_block_batch_max_response_bytes(),
        gen2_max_inflight_per_peer: config.gen2_max_inflight_per_peer(),
    }
}

#[test]
fn authenticated_outbound_gossip_requires_gen2_rollout_gates() {
    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    limits.authenticated_gossip_send_enabled = true;
    let gossip = NockchainRequest::Gossip {
        message: ByteBuf::from(b"gossip".to_vec()),
    };

    assert!(should_authenticate_outbound_gossip(
        &gossip,
        limits,
        true,
        true,
        ReqResGeneration::Gen2,
    ));
    assert!(!should_authenticate_outbound_gossip(
        &gossip,
        limits,
        false,
        true,
        ReqResGeneration::Gen2,
    ));
    assert!(!should_authenticate_outbound_gossip(
        &gossip,
        limits,
        true,
        false,
        ReqResGeneration::Gen2,
    ));
    assert!(!should_authenticate_outbound_gossip(
        &gossip,
        limits,
        true,
        true,
        ReqResGeneration::Gen1,
    ));

    let mut disabled_limits = limits;
    disabled_limits.authenticated_gossip_send_enabled = false;
    assert!(!should_authenticate_outbound_gossip(
        &gossip,
        disabled_limits,
        true,
        true,
        ReqResGeneration::Gen2,
    ));

    let authenticated = NockchainRequest::AuthenticatedGossip {
        pow: [0; 16],
        nonce: 0,
        message: ByteBuf::from(b"gossip".to_vec()),
    };
    assert!(!should_authenticate_outbound_gossip(
        &authenticated,
        limits,
        true,
        true,
        ReqResGeneration::Gen2,
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_outbound_gossip_conversion_records_send_metric() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "authenticated outbound gossip conversion should record a send metric",
    );
    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        req_res_authenticated_gossip_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let mut requester = start_swarm(
        requester_config.clone(),
        Keypair::generate_ed25519(),
        vec![loopback_quic_addr()],
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("driver swarm should build");
    let mut responder = build_test_swarm(responder_config);
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match requester.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    transcript.record("driver-swarm", format!("listening on {address}"));
                    return address;
                }
                other => transcript.record(
                    "driver-swarm",
                    format!("waiting for listen addr saw {other:?}"),
                ),
            }
        }
    })
    .await
    .expect("driver swarm listen address timeout");
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    requester
        .dial(responder_addr.clone())
        .expect("driver swarm dial should be accepted");
    transcript.record("driver-swarm", format!("dialing {responder_addr}"));

    let mut requester_connected = false;
    let mut responder_connected = false;
    tokio::time::timeout(Duration::from_secs(15), async {
            while !(requester_connected && responder_connected) {
                tokio::select! {
                    event = requester.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == responder_peer_id => {
                                requester_connected = true;
                                transcript.record("driver-swarm", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("driver-swarm", format!("connect loop saw {other:?}")),
                        }
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == requester_peer_id => {
                                responder_connected = true;
                                transcript.record("responder", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("responder", format!("connect loop saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("mixed swarm connection timeout");

    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let mut peer_gen2_inbound = BTreeMap::from([(responder_peer_id, true)]);
    let mut pending_gen2_batches = BTreeMap::new();
    let mut requester_equix = equix::EquiXBuilder::new();
    let gossip_message = ByteBuf::from(jam_heard_tx_response(61_001, 32));

    process_send_request_action(
        responder_peer_id,
        NockchainRequest::Gossip {
            message: gossip_message.clone(),
        },
        None,
        &mut requester,
        &driver_state,
        &metrics,
        &mut requester_equix,
        &mut peer_gen2_inbound,
        &mut pending_gen2_batches,
        requester_config.req_res_gen2_send_enabled,
        runtime_limits_from_config(&requester_config),
    )
    .await
    .expect("authenticated outbound gossip conversion should send");

    assert_eq!(metrics.authenticated_gossip_sent.fetch_add(0), 1);
    assert_eq!(metrics.legacy_gossip_received.fetch_add(0), 0);
    assert!(pending_gen2_batches.is_empty());

    let (peer, message) = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    event = requester.select_next_some() => {
                        transcript.record("driver-swarm", format!("request pump saw {event:?}"));
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                                request_response::Event::Message { peer, message, .. },
                            )) => {
                                transcript.record(
                                    "responder",
                                    format!("received authenticated gossip request from {peer}"),
                                );
                                return (peer, message);
                            }
                            other => transcript.record("responder", format!("request wait saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("authenticated gossip request event timeout");
    assert_eq!(peer, requester_peer_id);
    let request_response::Message::Request { request, .. } = message else {
        panic!("expected authenticated gossip request to arrive");
    };
    let NockchainRequest::AuthenticatedGossip {
        pow,
        nonce,
        message,
    } = request
    else {
        panic!("expected outbound gossip to convert to authenticated gossip");
    };
    assert_eq!(message, gossip_message);
    NockchainRequest::AuthenticatedGossip {
        pow,
        nonce,
        message,
    }
    .verify_pow(
        &mut equix::EquiXBuilder::new(),
        &responder_peer_id,
        &requester_peer_id,
    )
    .expect("converted authenticated gossip proof should verify");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_gossip_compatibility_flag_controls_inbound_driver_path() {
    let transcript = DriverTranscript::default();
    let gossip_message = ByteBuf::from(jam_heard_tx_response(31_415, 32));

    for accept_legacy in [true, false] {
        transcript.record(
            "scenario",
            format!("legacy gossip inbound compatibility accept_legacy={accept_legacy}"),
        );
        let requester_config = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };
        let responder_config = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            req_res_legacy_gossip_accept_enabled: accept_legacy,
            ..LibP2PConfig::default()
        };
        let mut requester = build_test_swarm(requester_config);
        let mut responder = build_test_swarm(responder_config.clone());
        let requester_peer_id = *requester.local_peer_id();
        let responder_peer_id = *responder.local_peer_id();

        let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
        let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
        connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

        requester.behaviour_mut().request_response.send_request(
            &responder_peer_id,
            NockchainRequest::Gossip {
                message: gossip_message.clone(),
            },
        );
        let (peer, connection_id, message) =
            recv_request_event(&mut requester, &mut responder, &transcript).await;
        assert_eq!(peer, requester_peer_id);

        let metrics = isolated_test_metrics();
        let driver_state = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        let scripted_traffic =
            build_scripted_traffic_cop(transcript.clone(), Vec::new(), vec![PokeResult::Ack]).await;
        let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
        let mut responder_equix = equix::EquiXBuilder::new();

        run_driver_with_timeout(
            &transcript,
            "driver should apply legacy gossip compatibility gate",
            handle_request_response(
                peer,
                connection_id,
                message,
                swarm_tx,
                &mut responder_equix,
                responder_peer_id,
                scripted_traffic.traffic.clone(),
                metrics.clone(),
                Arc::clone(&driver_state),
                runtime_limits_from_config(&responder_config),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("legacy gossip compatibility gate should not error");

        if accept_legacy {
            let response = match recv_swarm_action(&mut swarm_rx).await {
                SwarmAction::SendResponse { channel, response } => {
                    responder
                        .behaviour_mut()
                        .request_response
                        .send_response(channel, response.clone())
                        .expect("legacy gossip ack should send");
                    response
                }
                other => panic!("expected legacy gossip SendResponse, got {other:?}"),
            };
            assert_eq!(response, NockchainResponse::Ack { acked: true });
            let requester_response =
                recv_response_event(&mut requester, &mut responder, &transcript).await;
            assert_eq!(requester_response, NockchainResponse::Ack { acked: true });
            assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 1);
            assert_eq!(metrics.legacy_gossip_received.fetch_add(0), 1);
            assert_eq!(metrics.legacy_gossip_compatibility_rejected.fetch_add(0), 0);
            assert_eq!(metrics.authenticated_gossip_verified.fetch_add(0), 0);
            assert_eq!(metrics.gossip_dropped.fetch_add(0), 0);
        } else {
            match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
                Ok(None) => {}
                Ok(Some(other)) => panic!("legacy gossip should be dropped, got {other:?}"),
                Err(_) => panic!("legacy gossip drop should close the action channel promptly"),
            }
            assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
            assert_eq!(metrics.legacy_gossip_received.fetch_add(0), 1);
            assert_eq!(metrics.legacy_gossip_compatibility_rejected.fetch_add(0), 1);
            assert_eq!(metrics.authenticated_gossip_verified.fetch_add(0), 0);
            assert_eq!(metrics.gossip_dropped.fetch_add(0), 1);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_authenticated_gossip_pow_is_rejected_before_kernel_work() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "malformed authenticated gossip proof should block peer before kernel work",
    );
    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let mut requester = build_test_swarm(requester_config);
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let valid_message = ByteBuf::from(jam_heard_tx_response(41_001, 32));
    let invalid_message = ByteBuf::from(jam_heard_tx_response(41_002, 32));
    let valid_request =
        solve_authenticated_gossip(&requester_peer_id, &responder_peer_id, &valid_message);
    let NockchainRequest::AuthenticatedGossip { pow, nonce, .. } = valid_request else {
        panic!("authenticated gossip solver should return authenticated gossip");
    };
    let invalid_request = NockchainRequest::AuthenticatedGossip {
        pow,
        nonce,
        message: invalid_message,
    };
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, invalid_request);
    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(peer, requester_peer_id);

    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), vec![PokeResult::Ack]).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should reject malformed authenticated gossip proof",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx,
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&driver_state),
            runtime_limits_from_config(&responder_config),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("malformed authenticated gossip proof should be handled without error");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::BlockPeer { peer_id } => assert_eq!(peer_id, requester_peer_id),
        other => panic!("expected malformed authenticated gossip to block peer, got {other:?}"),
    }
    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Ok(None) => {}
        Ok(Some(other)) => {
            panic!("unexpected extra swarm action after bad gossip proof: {other:?}")
        }
        Err(_) => panic!("bad gossip proof should close the action channel promptly"),
    }
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert_eq!(metrics.local_peer_abuse_recorded.fetch_add(0), 1);
    assert_eq!(metrics.legacy_gossip_received.fetch_add(0), 0);
    assert_eq!(metrics.legacy_gossip_compatibility_rejected.fetch_add(0), 0);
    assert_eq!(metrics.authenticated_gossip_verified.fetch_add(0), 0);
    assert_eq!(metrics.gossip_dropped.fetch_add(0), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_gossip_still_routes_when_legacy_gossip_is_disabled() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        "authenticated gossip should route after legacy gossip compatibility is disabled",
    );
    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        req_res_legacy_gossip_accept_enabled: false,
        ..LibP2PConfig::default()
    };
    let mut requester = build_test_swarm(requester_config);
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let gossip_message = ByteBuf::from(jam_heard_tx_response(51_001, 32));
    let request =
        solve_authenticated_gossip(&requester_peer_id, &responder_peer_id, &gossip_message);
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, request);
    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(peer, requester_peer_id);

    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), vec![PokeResult::Ack]).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should route authenticated gossip with legacy compatibility disabled",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx,
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&driver_state),
            runtime_limits_from_config(&responder_config),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("authenticated gossip should route after legacy compatibility closes");

    let response = match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse { channel, response } => {
            responder
                .behaviour_mut()
                .request_response
                .send_response(channel, response.clone())
                .expect("authenticated gossip ack should send");
            response
        }
        other => panic!("expected authenticated gossip SendResponse, got {other:?}"),
    };
    assert_eq!(response, NockchainResponse::Ack { acked: true });
    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(requester_response, NockchainResponse::Ack { acked: true });
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 1);
    assert_eq!(metrics.legacy_gossip_received.fetch_add(0), 0);
    assert_eq!(metrics.legacy_gossip_compatibility_rejected.fetch_add(0), 0);
    assert_eq!(metrics.authenticated_gossip_verified.fetch_add(0), 1);
    assert_eq!(metrics.gossip_dropped.fetch_add(0), 0);
    assert_eq!(metrics.local_peer_abuse_recorded.fetch_add(0), 0);
}

/// Phase 6 paired-rollout sampler. Mirrors
/// `run_checkpoint_requester_cost_sample` but synthesises a single
/// `HeardBlockRangeWithTxs` response that bundles the entire replay window.
/// The returned `RequesterCostSample` carries the same shape so prefetch-on
/// vs prefetch-off JSON artifacts can be diffed by a downstream analyzer
/// without harness-specific schemas. `request_mix` is set to
/// `"checkpoint-block-range"` so consumers can split the curves apart.
async fn run_checkpoint_prefetch_cost_sample(
    config: &LibP2PConfig,
    chkjam_path: &Path,
    nominal_target_blocks: usize,
    block_window: &[(u64, Vec<u8>)],
    replay_blocks: usize,
    timeout_seconds: u64,
) -> RequesterCostSample {
    assert!(
        replay_blocks > 0 && replay_blocks <= block_window.len(),
        "checkpoint prefetch replay must stay within the discovered fit window"
    );
    let len_u8 = u8::try_from(replay_blocks)
        .expect("checkpoint prefetch sampler caps replay at u8::MAX heights");

    let window = &block_window[..replay_blocks];
    let window_start_height = window[0].0;
    let window_end_height = window[replay_blocks - 1].0;

    let range_request_bytes = block_range_with_txs_request_message(window_start_height, len_u8)
        .expect("range request message should encode");
    let request_item = BatchRequestItem {
        item_id: 1,
        message: ByteBuf::from(range_request_bytes.to_vec()),
    };

    let bundled_blocks: Vec<BundledBlockWithTxs> = window
        .iter()
        .map(|(_, message)| {
            let fact = NockchainFact::from_message_bytes(message)
                .expect("checkpoint window message should decode as a fact");
            let block_id = match fact {
                NockchainFact::HeardBlock(id, _) => id,
                other => panic!("checkpoint window must hold heard-block facts, got {other:?}"),
            };
            BundledBlockWithTxs {
                block_id,
                block_message: ByteBuf::from(message.clone()),
                tx_envelopes: Vec::new(),
                unincluded_tx_ids: Vec::new(),
            }
        })
        .collect();
    let envelope = ResponseEnvelope::heard_block_range_with_txs(bundled_blocks);
    envelope
        .validate()
        .expect("synthetic range envelope should validate");
    let result_item = BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope),
    };
    let results = vec![result_item];
    let response_bytes =
        batch_result_encoded_bytes(&results).expect("checkpoint range result should encode");
    let label = format!("checkpoint-block-range-{replay_blocks}-of-{nominal_target_blocks}");

    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        format!(
            "prefetch-cost checkpoint-block-range requested_items={nominal_target_blocks} replay_items={replay_blocks} heights={window_start_height}..={window_end_height} response_bytes={response_bytes}"
        ),
    );

    let metrics = stable_checkpoint_report_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        config.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    state_arc.lock().await.record_outbound_request(
        request_id,
        OutboundRequestContext::with_attempt(
            peer,
            ReqResGeneration::Gen2,
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items: vec![request_item],
            },
            0,
            false,
        ),
    );

    let checkpoint_app = start_checkpoint_app(chkjam_path).await;
    let live_traffic = build_live_traffic_cop(checkpoint_app);
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    let started = Instant::now();
    let processing_result = tokio::time::timeout(
        Duration::from_secs(timeout_seconds),
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(500 + replay_blocks),
            request_response::Message::Response {
                request_id,
                response: NockchainResponse::BatchResult { results },
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            live_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(config),
            PeerExclusions::default(),
        ),
    )
    .await;
    let elapsed = started.elapsed();
    let timed_out = processing_result.is_err();

    let mut followup_request_count = 0usize;
    if let Ok(result) = processing_result {
        result.expect("checkpoint prefetch-cost response should process");
        followup_request_count = drain_expected_swarm_followups(&mut swarm_rx).await;
    } else {
        transcript.record(
            "scenario",
            format!(
                "checkpoint prefetch-cost sample timed out after {timeout_seconds}s; treating elapsed time as a lower bound"
            ),
        );
    }

    let poke_count = routed_response_poke_count(&metrics);
    let total_ms = elapsed.as_secs_f64() * 1_000.0;
    let per_item_us = elapsed.as_micros() as f64 / replay_blocks as f64;
    if timed_out {
        println!(
            "{:<32} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8} timeout={}s",
            label,
            replay_blocks,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count,
            timeout_seconds
        );
    } else {
        println!(
            "{:<32} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8}",
            label,
            replay_blocks,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count
        );
        assert!(
            poke_count <= replay_blocks,
            "checkpoint prefetch-cost workload cannot route more block responses than replayed"
        );
    }

    RequesterCostSample {
        label,
        request_mix: String::from("checkpoint-block-range"),
        item_count: replay_blocks,
        requested_item_count: Some(nominal_target_blocks),
        response_bytes,
        total_ms,
        per_item_us,
        poke_count,
        followup_request_count,
        timed_out,
        timeout_seconds: timed_out.then_some(timeout_seconds),
        window_start_height: Some(window_start_height),
        window_end_height: Some(window_end_height),
    }
}

async fn run_checkpoint_requester_cost_sample(
    config: &LibP2PConfig,
    chkjam_path: &Path,
    nominal_target_blocks: usize,
    block_window: &[(u64, Vec<u8>)],
    replay_blocks: usize,
    timeout_seconds: u64,
) -> RequesterCostSample {
    assert!(
        replay_blocks > 0 && replay_blocks <= block_window.len(),
        "checkpoint requester replay must stay within the discovered fit window"
    );

    let window = &block_window[..replay_blocks];
    let window_start_height = window
        .first()
        .expect("checkpoint requester-cost window should exist")
        .0;
    let window_end_height = window
        .last()
        .expect("checkpoint requester-cost window should exist")
        .0;
    let items = window
        .iter()
        .enumerate()
        .map(|(idx, (height, _))| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_block_by_height_request(*height)),
        })
        .collect::<Vec<_>>();
    let results = window
        .iter()
        .enumerate()
        .map(|(idx, (_, message))| batch_result_item_from_result_message(idx as u32 + 1, message))
        .collect::<Vec<_>>();
    let response_bytes =
        batch_result_encoded_bytes(&results).expect("checkpoint batch result should encode");
    let label = format!("checkpoint-block-{replay_blocks}-of-{nominal_target_blocks}");
    let transcript = DriverTranscript::default();
    transcript.record(
            "scenario",
            format!(
                "requester-cost checkpoint-block-batch requested_items={nominal_target_blocks} replay_items={replay_blocks} heights={window_start_height}..={window_end_height} response_bytes={response_bytes}"
            ),
        );
    let metrics = stable_checkpoint_report_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        config.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    state_arc.lock().await.record_outbound_request(
        request_id,
        OutboundRequestContext::with_attempt(
            peer,
            ReqResGeneration::Gen2,
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items,
            },
            0,
            false,
        ),
    );
    let checkpoint_app = start_checkpoint_app(chkjam_path).await;
    let live_traffic = build_live_traffic_cop(checkpoint_app);
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    let started = Instant::now();
    let processing_result = tokio::time::timeout(
        Duration::from_secs(timeout_seconds),
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(400 + replay_blocks),
            request_response::Message::Response {
                request_id,
                response: NockchainResponse::BatchResult { results },
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            live_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(config),
            PeerExclusions::default(),
        ),
    )
    .await;
    let elapsed = started.elapsed();
    let timed_out = processing_result.is_err();

    let mut followup_request_count = 0usize;
    if let Ok(result) = processing_result {
        result.expect("checkpoint requester-cost response should process");
        followup_request_count = drain_expected_swarm_followups(&mut swarm_rx).await;
    } else {
        transcript.record(
                "scenario",
                format!(
                    "checkpoint requester-cost sample timed out after {timeout_seconds}s; treating elapsed time as a lower bound"
                ),
            );
    }

    let poke_count = routed_response_poke_count(&metrics);
    let total_ms = elapsed.as_secs_f64() * 1_000.0;
    let per_item_us = elapsed.as_micros() as f64 / replay_blocks as f64;
    if timed_out {
        println!(
            "{:<24} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8} timeout={}s",
            label,
            replay_blocks,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count,
            timeout_seconds
        );
    } else {
        println!(
            "{:<24} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8}",
            label,
            replay_blocks,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count
        );
        assert!(
            poke_count <= replay_blocks,
            "checkpoint requester-cost workload cannot route more block responses than replayed"
        );
    }

    RequesterCostSample {
        label,
        request_mix: String::from("checkpoint-heard-block"),
        item_count: replay_blocks,
        requested_item_count: Some(nominal_target_blocks),
        response_bytes,
        total_ms,
        per_item_us,
        poke_count,
        followup_request_count,
        timed_out,
        timeout_seconds: timed_out.then_some(timeout_seconds),
        window_start_height: Some(window_start_height),
        window_end_height: Some(window_end_height),
    }
}

async fn profile_checkpoint_requester_first_item(
    chkjam_path: &Path,
    nominal_target_blocks: usize,
    block_window: &[(u64, Vec<u8>)],
    replay_blocks: usize,
    poke_timeout: Duration,
) -> CheckpointRequesterProfileSample {
    assert!(
        replay_blocks > 0 && replay_blocks <= block_window.len(),
        "checkpoint requester profile must stay within the discovered fit window"
    );

    let window = &block_window[..replay_blocks];
    let window_start_height = window
        .first()
        .expect("checkpoint requester profile window should exist")
        .0;
    let window_end_height = window
        .last()
        .expect("checkpoint requester profile window should exist")
        .0;
    let results = window
        .iter()
        .enumerate()
        .map(|(idx, (_, message))| batch_result_item_from_result_message(idx as u32 + 1, message))
        .collect::<Vec<_>>();
    let response_bytes =
        batch_result_encoded_bytes(&results).expect("checkpoint batch result should encode");
    let label = format!("checkpoint-profile-{replay_blocks}-of-{nominal_target_blocks}");
    let first_height = window[0].0;
    let first_message = &window[0].1;
    let first_envelope = response_envelope_from_result_message(first_message)
        .expect("checkpoint response message should decode into an envelope");

    let decode_started = Instant::now();
    let response = response_fact_from_envelope(&first_envelope)
        .expect("profiled checkpoint response should decode into a fact");
    let first_item_decode_ms = decode_started.elapsed().as_secs_f64() * 1_000.0;

    let clone_started = Instant::now();
    let poke_slab = response.fact_poke().clone();
    let first_item_clone_ms = clone_started.elapsed().as_secs_f64() * 1_000.0;

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let response_gate = ResponseProcessingGate::from(&response);
    let gate_started = Instant::now();
    let _gate_allowed = {
        let mut state_guard = state_arc.lock().await;
        should_process_response(&response_gate, &mut state_guard, &metrics, false)
    };
    let first_item_gate_us = gate_started.elapsed().as_micros() as f64;

    let peer = PeerId::random();
    let mut checkpoint_app = start_checkpoint_app(chkjam_path).await;
    let wire = Libp2pWire::Response(peer).to_wire();
    let poke_started = Instant::now();
    // Call NockApp::poke_timeout directly, bypassing traffic cop.
    // The traffic cop sends IOAction::Poke to the NockApp event loop,
    // but the event loop isn't running in this test harness. Direct
    // invocation hits the SerfThread immediately.
    let poke_result = checkpoint_app
        .app
        .poke_timeout(wire, poke_slab, poke_timeout)
        .await;
    let first_item_poke_ms = poke_started.elapsed().as_secs_f64() * 1_000.0;
    let first_item_poke_timed_out = poke_result.is_err();
    let first_item_poke_error = match poke_result {
        Ok(_) => None,
        Err(ref err) => Some(err.to_string()),
    };

    CheckpointRequesterProfileSample {
        label,
        replay_blocks,
        requested_item_count: nominal_target_blocks,
        response_bytes,
        window_start_height,
        window_end_height,
        first_height,
        first_message_bytes: first_message.len(),
        first_item_decode_ms,
        first_item_clone_ms,
        first_item_gate_us,
        first_item_poke_ms,
        first_item_poke_timed_out,
        first_item_poke_error,
    }
}

async fn run_requester_response_workload(
    generation: ReqResGeneration,
    item_count: usize,
    payload_len: usize,
    transcript: &DriverTranscript,
) -> (usize, Duration, usize, u64, u64, u64) {
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let scripted_traffic = build_scripted_traffic_cop(
        transcript.clone(),
        Vec::new(),
        std::iter::repeat_with(|| PokeResult::Ack)
            .take(item_count)
            .collect(),
    )
    .await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();
    let rss_before_kib = current_rss_kib();
    let mut rss_peak_kib = rss_before_kib;
    let started = Instant::now();
    let mut response_bytes = 0usize;

    match generation {
        ReqResGeneration::Gen1 => {
            for idx in 0..item_count {
                let request_id = fresh_outbound_request_id();
                state_arc.lock().await.record_outbound_request(
                    request_id,
                    OutboundRequestContext::with_attempt(
                        peer,
                        ReqResGeneration::Gen1,
                        NockchainRequest::Request {
                            pow: [0; 16],
                            nonce: 1,
                            message: ByteBuf::from(jam_raw_tx_request(60_000 + idx as u64)),
                        },
                        0,
                        false,
                    ),
                );
                let response = match tx_result_outcome_for_seed(60_000 + idx as u64, payload_len) {
                    RequestExecutionOutcome::Result { response, .. } => response,
                    other => panic!("unexpected gen1 response outcome: {other:?}"),
                };
                response_bytes += cbor4ii::serde::to_vec(Vec::new(), &response)
                    .expect("gen1 requester workload response should encode")
                    .len();
                let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
                run_driver_with_timeout(
                    transcript,
                    "resident-set singleton response processing",
                    handle_request_response(
                        peer,
                        ConnectionId::new_unchecked(90 + idx),
                        request_response::Message::Response {
                            request_id,
                            response,
                        },
                        swarm_tx,
                        &mut equix_builder,
                        local_peer,
                        scripted_traffic.traffic.clone(),
                        metrics.clone(),
                        Arc::clone(&state_arc),
                        runtime_limits_from_config(&LIBP2P_CONFIG),
                        PeerExclusions::default(),
                    ),
                )
                .await
                .expect("resident-set gen1 response should process");
                match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
                    Ok(None) => {}
                    Ok(Some(other)) => panic!("unexpected follow-up swarm action: {other:?}"),
                    Err(_) => {
                        panic!("swarm action channel should close promptly for success path")
                    }
                }
                rss_peak_kib = rss_peak_kib.max(current_rss_kib());
            }
        }
        ReqResGeneration::Gen2 => {
            let request_id = fresh_outbound_request_id();
            let items = (0..item_count)
                .map(|idx| BatchRequestItem {
                    item_id: idx as u32 + 1,
                    message: ByteBuf::from(jam_raw_tx_request(40_000 + idx as u64)),
                })
                .collect::<Vec<_>>();
            state_arc.lock().await.record_outbound_request(
                request_id,
                OutboundRequestContext::with_attempt(
                    peer,
                    ReqResGeneration::Gen2,
                    NockchainRequest::BatchRequest {
                        pow: [0; 16],
                        nonce: 1,
                        items: items.clone(),
                    },
                    0,
                    false,
                ),
            );
            let results = (0..item_count)
                .map(|idx| {
                    tx_result_outcome_for_seed(40_000 + idx as u64, payload_len)
                        .into_batch_result_item(idx as u32 + 1)
                })
                .collect::<Vec<_>>();
            response_bytes = batch_result_encoded_bytes(&results)
                .expect("resident-set gen2 response should encode");
            run_driver_with_timeout(
                transcript,
                "resident-set batch response processing",
                handle_request_response(
                    peer,
                    ConnectionId::new_unchecked(90 + item_count),
                    request_response::Message::Response {
                        request_id,
                        response: NockchainResponse::BatchResult { results },
                    },
                    swarm_tx,
                    &mut equix_builder,
                    local_peer,
                    scripted_traffic.traffic.clone(),
                    metrics,
                    Arc::clone(&state_arc),
                    runtime_limits_from_config(&LIBP2P_CONFIG),
                    PeerExclusions::default(),
                ),
            )
            .await
            .expect("resident-set gen2 response should process");
            match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
                Ok(None) => {}
                Ok(Some(other)) => panic!("unexpected follow-up swarm action: {other:?}"),
                Err(_) => {
                    panic!("swarm action channel should close promptly for success path")
                }
            }
            rss_peak_kib = rss_peak_kib.max(current_rss_kib());
        }
    }

    let elapsed = started.elapsed();
    let rss_after_kib = current_rss_kib();
    let poke_count = scripted_traffic.poke_count.load(Ordering::SeqCst);
    (
        response_bytes,
        elapsed,
        poke_count,
        rss_before_kib,
        rss_after_kib,
        rss_peak_kib.max(rss_after_kib),
    )
}

async fn run_two_peer_driver_latency_workload(
    generation: ReqResGeneration,
    item_count: usize,
    payload_len: usize,
    transcript: &DriverTranscript,
) -> (usize, Duration, String, usize) {
    let requester_config = match generation {
        ReqResGeneration::Gen1 => LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        },
        ReqResGeneration::Gen2 => LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        },
    };
    let responder_config = requester_config.clone();
    let protocol = first_common_outbound_protocol(&requester_config, &responder_config)
        .unwrap_or_else(|| String::from("none"));
    let mut requester = build_test_swarm(requester_config.clone());
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();
    let _requester_addr = wait_for_listen_addr(&mut requester, transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, transcript).await;

    let state_arc = Arc::new(Mutex::new(P2PState::new(
        Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        ),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let scripted_traffic = build_scripted_traffic_cop(
        transcript.clone(),
        (0..item_count)
            .map(|idx| Some(scry_some_raw_tx(100_000 + idx as u64, payload_len)))
            .collect(),
        Vec::new(),
    )
    .await;
    let limits = runtime_limits_from_config(&responder_config);
    let mut request_builder = equix::EquiXBuilder::new();
    let mut responder_equix_builder = equix::EquiXBuilder::new();
    let started = Instant::now();
    let mut response_bytes = 0usize;

    match generation {
        ReqResGeneration::Gen1 => {
            for idx in 0..item_count {
                let request_slab = jammed_request_slab(&jam_raw_tx_request(70_000 + idx as u64));
                let request = NockchainRequest::new_request(
                    &mut request_builder, &requester_peer_id, &responder_peer_id, &request_slab,
                );
                requester
                    .behaviour_mut()
                    .request_response
                    .send_request(&responder_peer_id, request);
                let (peer, connection_id, message) =
                    recv_request_event(&mut requester, &mut responder, transcript).await;
                let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
                run_driver_with_timeout(
                    transcript,
                    "two-peer singleton latency response",
                    handle_request_response(
                        peer,
                        connection_id,
                        message,
                        swarm_tx,
                        &mut responder_equix_builder,
                        responder_peer_id,
                        scripted_traffic.traffic.clone(),
                        metrics.clone(),
                        Arc::clone(&state_arc),
                        limits,
                        PeerExclusions::default(),
                    ),
                )
                .await
                .expect("gen1 latency request should process");
                let action = recv_swarm_action(&mut swarm_rx).await;
                let response = match action {
                    SwarmAction::SendResponse { channel, response } => {
                        responder
                            .behaviour_mut()
                            .request_response
                            .send_response(channel, response.clone())
                            .expect("response should send");
                        response
                    }
                    other => panic!("expected SendResponse, got {other:?}"),
                };
                response_bytes += cbor4ii::serde::to_vec(Vec::new(), &response)
                    .expect("latency response should encode")
                    .len();
                let requester_response =
                    recv_response_event(&mut requester, &mut responder, transcript).await;
                assert!(matches!(
                    requester_response,
                    NockchainResponse::Result { .. }
                ));
            }
        }
        ReqResGeneration::Gen2 => {
            let items = (0..item_count)
                .map(|idx| BatchRequestItem {
                    item_id: idx as u32 + 1,
                    message: ByteBuf::from(jam_raw_tx_request(80_000 + idx as u64)),
                })
                .collect::<Vec<_>>();
            let request = NockchainRequest::new_batch_request(
                &mut request_builder, &requester_peer_id, &responder_peer_id, items,
            )
            .expect("batch request should build");
            requester
                .behaviour_mut()
                .request_response
                .send_request(&responder_peer_id, request);
            let (peer, connection_id, message) =
                recv_request_event(&mut requester, &mut responder, transcript).await;
            let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
            run_driver_with_timeout(
                transcript,
                "two-peer batch latency response",
                handle_request_response(
                    peer,
                    connection_id,
                    message,
                    swarm_tx,
                    &mut responder_equix_builder,
                    responder_peer_id,
                    scripted_traffic.traffic.clone(),
                    metrics,
                    Arc::clone(&state_arc),
                    limits,
                    PeerExclusions::default(),
                ),
            )
            .await
            .expect("gen2 latency batch request should process");
            let action = recv_swarm_action(&mut swarm_rx).await;
            let response = match action {
                SwarmAction::SendResponse { channel, response } => {
                    responder
                        .behaviour_mut()
                        .request_response
                        .send_response(channel, response.clone())
                        .expect("response should send");
                    response
                }
                other => panic!("expected SendResponse, got {other:?}"),
            };
            response_bytes = cbor4ii::serde::to_vec(Vec::new(), &response)
                .expect("latency batch response should encode")
                .len();
            let requester_response =
                recv_response_event(&mut requester, &mut responder, transcript).await;
            let NockchainResponse::BatchResult { results } = requester_response else {
                panic!("expected batch result response");
            };
            assert_eq!(results.len(), item_count);
        }
    }

    let elapsed = started.elapsed();
    let peek_count = scripted_traffic.peek_count.load(Ordering::SeqCst);
    assert_eq!(
        peek_count, item_count,
        "two-peer latency workload should perform one responder peek per logical item"
    );
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    (response_bytes, elapsed, protocol, peek_count)
}

fn benchmark_singleton_requests(
    local_peer_id: &PeerId,
    remote_peer_id: &PeerId,
    messages: &[Vec<u8>],
    iterations: usize,
) -> (Duration, usize) {
    let mut total_encoded_bytes = 0usize;
    let started = Instant::now();
    for _ in 0..iterations {
        let mut equix_builder = equix::EquiXBuilder::new();
        for message in messages {
            let request_slab = jammed_request_slab(message);
            let request = NockchainRequest::new_request(
                &mut equix_builder, local_peer_id, remote_peer_id, &request_slab,
            );
            request
                .verify_pow(&mut equix_builder, remote_peer_id, local_peer_id)
                .expect("singleton pow verification should succeed");
            total_encoded_bytes = total_encoded_bytes.saturating_add(
                serde_cbor::to_vec(black_box(&request))
                    .expect("encode")
                    .len(),
            );
        }
    }
    (started.elapsed(), total_encoded_bytes)
}

fn benchmark_batch_request(
    local_peer_id: &PeerId,
    remote_peer_id: &PeerId,
    messages: &[Vec<u8>],
    iterations: usize,
) -> (Duration, usize, usize) {
    let mut total_encoded_bytes = 0usize;
    let mut total_payload_bytes = 0usize;
    let started = Instant::now();
    for _ in 0..iterations {
        let mut equix_builder = equix::EquiXBuilder::new();
        let items = messages
            .iter()
            .enumerate()
            .map(|(item_id, message)| BatchRequestItem {
                item_id: item_id as u32,
                message: ByteBuf::from(message.clone()),
            })
            .collect::<Vec<_>>();
        let payload_bytes =
            batch_request_payload_bytes(&items).expect("batch payload size should fit");
        let request = NockchainRequest::new_batch_request(
            &mut equix_builder, local_peer_id, remote_peer_id, items,
        )
        .expect("batch request should build");
        request
            .verify_pow(&mut equix_builder, remote_peer_id, local_peer_id)
            .expect("batch pow verification should succeed");
        total_payload_bytes = total_payload_bytes.saturating_add(payload_bytes);
        total_encoded_bytes = total_encoded_bytes.saturating_add(
            serde_cbor::to_vec(black_box(&request))
                .expect("encode")
                .len(),
        );
    }
    (started.elapsed(), total_encoded_bytes, total_payload_bytes)
}

#[test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
fn test_request_to_scry_slab() {
    // Test block by-height request
    {
        let mut slab: NounSlab = NounSlab::new();
        let height = 123u64;
        let by_height_tas = make_tas(&mut slab, "by-height");
        let by_height = T(&mut slab, &[by_height_tas.as_noun(), D(height)]);
        let block_cell = T(&mut slab, &[D(tas!(b"block")), by_height]);
        let request = T(&mut slab, &[D(tas!(b"request")), block_cell]);
        slab.set_root(request);

        let space = slab.noun_space();
        let data_request = NockchainDataRequest::from_noun(request, &space)
            .expect("Failed to create request from noun");

        let result_slab = request_to_scry_slab(data_request).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let result = unsafe { result_slab.root() };
        let result_space = result_slab.noun_space();

        assert!(result.is_cell());
        let result_cell = result
            .in_space(&result_space)
            .as_cell()
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        assert!(result_cell.head().eq_bytes(b"heavy-n"));

        // Get the tail cell and check its components
        let tail_cell = result_cell.tail().as_cell().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let height_atom = tail_cell.head().as_atom().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(
            height_atom.as_u64().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            height
        );
        let tail_atom = tail_cell.tail().as_atom().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(
            tail_atom.as_u64().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            0
        );
    }

    // Test block-with-txs by-height request
    {
        let mut slab: NounSlab = NounSlab::new();
        let height = 456u64;
        let by_height_tas = make_tas(&mut slab, "by-height");
        let by_height = T(&mut slab, &[by_height_tas.as_noun(), D(height)]);
        let bundle_tas = make_tas(&mut slab, "block-with-txs");
        let bundle_cell = T(&mut slab, &[bundle_tas.as_noun(), by_height]);
        let request = T(&mut slab, &[D(tas!(b"request")), bundle_cell]);
        slab.set_root(request);

        let space = slab.noun_space();
        let data_request = NockchainDataRequest::from_noun(request, &space)
            .expect("Failed to create request from noun");

        let result_slab = request_to_scry_slab(data_request).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let result = unsafe { result_slab.root() };
        let result_space = result_slab.noun_space();

        assert!(result.is_cell());
        let result_cell = result
            .in_space(&result_space)
            .as_cell()
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });
        assert!(result_cell.head().eq_bytes(b"heavy-txs"));

        let tail_cell = result_cell.tail().as_cell().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let height_atom = tail_cell.head().as_atom().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(
            height_atom.as_u64().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            height
        );
        let tail_atom = tail_cell.tail().as_atom().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(
            tail_atom.as_u64().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            }),
            0
        );
    }

    // Test block-with-txs by-range request scry path
    {
        let start_height = 100u64;
        let len: u8 = 8;
        let request_bytes =
            crate::messages::block_range_with_txs_request_message(start_height, len)
                .expect("range request message should encode");
        let request_slab = crate::messages::request_slab_from_message(request_bytes.as_ref())
            .expect("range request slab should decode");
        let request = unsafe { request_slab.root() };
        let space = request_slab.noun_space();
        let data_request = NockchainDataRequest::from_noun(*request, &space)
            .expect("Failed to create range request from noun");

        let result_slab = request_to_scry_slab(data_request).expect("range scry slab should build");
        let result = unsafe { result_slab.root() };
        let result_space = result_slab.noun_space();

        let result_cell = result
            .in_space(&result_space)
            .as_cell()
            .expect("range scry should be a cell");
        assert!(result_cell.head().eq_bytes(b"heaviest-chain-blocks-range"));

        let tail_cell = result_cell
            .tail()
            .as_cell()
            .expect("range scry tail should be a cell");
        let parsed_start = tail_cell.head().as_atom().unwrap().as_u64().unwrap();
        assert_eq!(parsed_start, start_height);

        let after_start = tail_cell
            .tail()
            .as_cell()
            .expect("range scry should have end + terminator");
        let parsed_end = after_start.head().as_atom().unwrap().as_u64().unwrap();
        assert_eq!(parsed_end, start_height + u64::from(len) - 1);
    }

    // Test invalid request (not a cell)
    {
        let mut slab: NounSlab = NounSlab::new();
        slab.set_root(D(123));
        let space = slab.noun_space();
        let result = NockchainDataRequest::from_noun(*unsafe { slab.root() }, &space)
            .and_then(request_to_scry_slab);
        assert!(result.is_err());
    }

    // Test elders request
    {
        let mut slab: NounSlab = NounSlab::new();
        // Create a 5-tuple [1 2 3 4 5] for the block ID
        let five_tuple = T(&mut slab, &[D(1), D(2), D(3), D(4), D(5)]);

        // Create a random peer ID and store its bytes
        let peer_id = PeerId::random();
        let peer_id_atom = Atom::from_value(&mut slab, peer_id.to_base58()).unwrap();

        let elders_cell = T(&mut slab, &[five_tuple, peer_id_atom.as_noun()]);
        let elders_tas = D(tas!(b"elders"));
        let inner_cell = T(&mut slab, &[elders_tas, elders_cell]);
        let block_cell = T(&mut slab, &[D(tas!(b"block")), inner_cell]);
        let request = T(&mut slab, &[D(tas!(b"request")), block_cell]);
        slab.set_root(request);

        let space = slab.noun_space();
        let data_request = NockchainDataRequest::from_noun(request, &space)
            .expect("Could not create request from noun");

        let result_slab = request_to_scry_slab(data_request).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let result = unsafe { result_slab.root() };
        let result_space = result_slab.noun_space();

        // Verify the structure: [%elders block_id_b58 0]
        assert!(result.is_cell());
        let result_cell = result
            .in_space(&result_space)
            .as_cell()
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Check %elders tag
        assert!(result_cell.head().eq_bytes(b"elders"));

        // Get the tail cell
        let tail_cell = result_cell.tail().as_cell().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });

        // Check block ID (should be base58 encoded)
        let block_id_atom = tail_cell.head().as_atom().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let block_id_bytes = block_id_atom.to_bytes_until_nul().unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        let block_id_str = String::from_utf8(block_id_bytes).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });

        // Get the expected base58 string
        let expected_b58 = tip5_hash_to_base58(five_tuple, &space).unwrap_or_else(|_| {
            panic!(
                "Called `expect()` at {}:{} (git sha: {})",
                file!(),
                line!(),
                option_env!("GIT_SHA").unwrap_or("unknown")
            )
        });
        assert_eq!(block_id_str, expected_b58);

        // Check final 0
        assert_eq!(
            tail_cell
                .tail()
                .as_atom()
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                })
                .as_u64()
                .unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                }),
            0
        );
    }

    // Test invalid elders request (not a cell)
    {
        let mut slab: NounSlab = NounSlab::new();
        let invalid_request = T(
            &mut slab,
            &[D(tas!(b"request")), D(tas!(b"block")), D(tas!(b"elders"))],
        );
        slab.set_root(invalid_request);

        let space = slab.noun_space();
        let result =
            NockchainDataRequest::from_noun(invalid_request, &space).and_then(request_to_scry_slab);
        assert!(result.is_err());
        drop(slab);
    }
}

#[test]
#[cfg_attr(miri, ignore)] // equix uses a foreign function so miri fails this tes
fn test_equix_pow_verification() {
    // Create EquiX builder - new() doesn't return Result
    let mut builder = equix::EquiXBuilder::new();

    // Create test peer IDs
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();

    // Create test message
    let message = ByteBuf::from(vec![1, 2, 3, 4, 5]);

    // Create valid request with correct PoW
    let valid_request =
        NockchainRequest::new_request(&mut builder, &local_peer_id, &remote_peer_id, &{
            let mut slab = NounSlab::new();
            let message_noun = Atom::from_value(&mut slab, &message[..])
                .expect("Failed to create message atom")
                .as_noun();
            slab.set_root(message_noun);
            slab
        });

    // Verify the valid request
    match &valid_request {
        NockchainRequest::Request {
            pow,
            nonce,
            message: _,
        } => {
            // Test successful verification
            let result = valid_request.verify_pow(
                &mut builder, &remote_peer_id, // Note: peers are swapped for verification
                &local_peer_id,
            );
            assert!(result.is_ok(), "Valid PoW should verify successfully");

            // Test failed verification with tampered nonce
            let tampered_request = NockchainRequest::Request {
                pow: *pow,
                nonce: nonce + 1, // Tamper with the nonce
                message: message.clone(),
            };
            let result = tampered_request.verify_pow(&mut builder, &remote_peer_id, &local_peer_id);
            assert!(result.is_err(), "Tampered nonce should fail verification");

            // Test failed verification with wrong peer order
            let result = valid_request.verify_pow(
                &mut builder, &local_peer_id, // Wrong order - not swapped
                &remote_peer_id,
            );
            assert!(result.is_err(), "Wrong peer order should fail verification");
        }
        _ => panic!("Expected Request variant"),
    }

    // Test that gossip requests always verify successfully
    let gossip_request = NockchainRequest::Gossip {
        message: message.clone(),
    };
    let result = gossip_request.verify_pow(&mut builder, &remote_peer_id, &local_peer_id);
    assert!(
        result.is_ok(),
        "Gossip requests should always verify successfully"
    );
}

fn build_gossip_effect_with_tag(
    version: u64,
    tag: &'static str,
    page_words: &[u64],
) -> (NounSlab, ByteBuf) {
    let mut effect_slab = NounSlab::new();
    let gossip_tag =
        Atom::from_value(&mut effect_slab, tag).expect("Failed to create gossip tag atom");
    let page = match page_words {
        [] => D(0),
        [word] => D(*word),
        words => T(
            &mut effect_slab,
            &words.iter().copied().map(D).collect::<Vec<_>>(),
        ),
    };
    let payload = T(&mut effect_slab, &[gossip_tag.as_noun(), page]);
    let effect = T(&mut effect_slab, &[D(tas!(b"gossip")), D(version), payload]);
    effect_slab.set_root(effect);

    let mut payload_slab: NounSlab = NounSlab::new();
    let space = effect_slab.noun_space();
    payload_slab.copy_into(payload, &space);
    (
        effect_slab,
        ByteBuf::from(payload_slab.jam().as_ref().to_vec()),
    )
}

fn build_gossip_effect(version: u64, page_words: &[u64]) -> (NounSlab, ByteBuf) {
    build_gossip_effect_with_tag(version, "heard-block", page_words)
}

#[test]
fn test_select_request_peers_preserves_order_for_block_windows() {
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peer_c = PeerId::random();
    let peers = vec![peer_a, peer_b, peer_c];
    let mut expected = peers.clone();
    expected.sort_unstable_by_key(|peer| peer.to_base58());

    assert_eq!(
        select_request_peers_with_preferences(peers.clone(), 2, true, &[]),
        expected[..2].to_vec()
    );
    assert_eq!(
        select_request_peers_with_preferences(peers.clone(), 8, true, &[]),
        expected
    );
}

#[test]
fn test_select_request_peers_prefers_tx_source_hints() {
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peer_c = PeerId::random();
    let peer_d = PeerId::random();

    let selected = select_request_peers_with_preferences(
        vec![peer_a, peer_b, peer_c, peer_d],
        2,
        false,
        &[peer_c, peer_a, PeerId::random()],
    );
    assert_eq!(selected, vec![peer_c, peer_a]);

    let selected_with_stable_tail = select_request_peers_with_preferences(
        vec![peer_d, peer_b, peer_a, peer_c],
        3,
        true,
        &[peer_c, peer_a],
    );
    let mut expected_tail = [peer_d, peer_b];
    expected_tail.sort_unstable_by_key(|peer| peer.to_base58());
    assert_eq!(
        selected_with_stable_tail,
        vec![peer_c, peer_a, expected_tail[0]]
    );
}

#[tokio::test]
async fn test_request_effect_queues_block_request_for_single_stable_peer() {
    use tokio::sync::mpsc;

    let expected_message = ByteBuf::from(jam_block_by_height_request(42));
    let effect_slab =
        request_slab_from_message(&expected_message).expect("request slab should decode");
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peers = vec![peer_a, peer_b];
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let (swarm_tx, mut swarm_rx) = mpsc::channel(4);
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        peers.clone(),
        false,
        state_arc,
        metrics,
    )
    .await;

    assert!(result.is_ok(), "request effect should be accepted");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(peer_id, canonical_peers[0]);
            assert_eq!(request_message, expected_message);
        }
        other => panic!("expected QueueKernelRequest action, got {:?}", other),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "block request should initially queue only one peer"
    );
}

#[tokio::test]
async fn test_request_effect_skips_bundle_upgrade_for_non_bundle_capable_peer() {
    use tokio::sync::mpsc;

    let classic_message = ByteBuf::from(jam_block_by_height_request(91));
    let effect_slab =
        request_slab_from_message(&classic_message).expect("request slab should decode");
    let mut peers = vec![PeerId::random(), PeerId::random()];
    peers.sort_unstable_by_key(|peer| peer.to_base58());
    let classic_peer = peers[0];

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.mark_peer_non_bundle_capable(classic_peer);
    }

    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
    handle_effect(
        effect_slab,
        swarm_tx,
        peers.clone(),
        true, // bundle_requests_enabled
        state_arc,
        metrics,
    )
    .await
    .expect("per-peer bundle dispatch should succeed");

    match swarm_rx
        .recv()
        .await
        .expect("selected peer should receive a QueueKernelRequest")
    {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, classic_peer);
            let decoded = decode_request_item_message(&request_message)
                .expect("classic fallback request message should decode");
            match decoded {
                NockchainDataRequest::BlockByHeight(height) => assert_eq!(height, 91),
                other => {
                    panic!("memoed non-bundle peer expected classic BlockByHeight, got {other:?}")
                }
            }
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "block request dispatch should keep the stable single-peer subset"
    );
}

#[test]
fn test_bundle_request_batch_item_height_distinguishes_bundle_from_classic() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(jam_block_by_height_request(10)),
        },
        BatchRequestItem {
            item_id: 2,
            message: block_with_txs_by_height_request_message(20)
                .expect("bundle request jam should build"),
        },
    ];
    let request = NockchainRequest::BatchRequest {
        pow: [0u8; 16],
        nonce: 0,
        items,
    };
    let context = OutboundRequestContext {
        peer_id: PeerId::random(),
        generation: ReqResGeneration::Gen2,
        request,
        retry_count: 0,
        fallback_attempted: false,
        started_at: std::time::Instant::now(),
    };

    assert_eq!(
        bundle_request_batch_item_height(Some(&context), 1),
        None,
        "classic by-height item must not be reported as a bundle request"
    );
    assert_eq!(
        bundle_request_batch_item_height(Some(&context), 2),
        Some(20),
        "bundle item must be reported at its height"
    );
    assert_eq!(
        bundle_request_batch_item_height(Some(&context), 42),
        None,
        "unknown item_id must yield None"
    );
}

#[tokio::test]
async fn test_bundle_response_estimate_uses_bundle_envelope_shape() {
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics, LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let item = BatchRequestItem {
        item_id: 1,
        message: crate::messages::block_with_txs_by_height_request_message(20)
            .expect("bundle request message should build"),
    };
    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    limits.gen2_item_max_bytes = 16;

    let estimate = estimate_batch_request_item_response(&item, limits, &driver_state)
        .await
        .expect("bundle estimate should not error")
        .expect("bundle estimate should decode");
    assert_eq!(estimate.request_kind, "block-with-txs-by-height");
    assert_eq!(estimate.source, "configured_bundle_cap");

    let projected = estimated_result_item(1, &estimate);
    let envelope = projected
        .envelope
        .expect("estimated bundle result should carry an envelope");
    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockWithTxs);
    assert!(envelope.tx_envelopes.as_ref().is_some());
    assert!(envelope.unincluded_tx_ids.as_ref().is_some());
}

#[test]
fn test_block_by_height_message_round_trips_through_decoder() {
    let bytes = crate::messages::block_by_height_message(123);
    let decoded =
        decode_request_item_message(&bytes).expect("classic by-height jam should decode back");
    match decoded {
        NockchainDataRequest::BlockByHeight(height) => assert_eq!(height, 123),
        other => panic!("expected BlockByHeight, got {other:?}"),
    }
}

#[tokio::test]
async fn test_request_effect_upgrades_block_by_height_to_bundle_when_flag_enabled() {
    use tokio::sync::mpsc;

    let classic_message = ByteBuf::from(jam_block_by_height_request(77));
    let effect_slab =
        request_slab_from_message(&classic_message).expect("request slab should decode");
    let peer = PeerId::random();

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(4);

    handle_effect(
        effect_slab,
        swarm_tx,
        vec![peer],
        true, // bundle_requests_enabled
        state_arc,
        metrics,
    )
    .await
    .expect("bundle-upgrade effect should queue cleanly");

    let action = swarm_rx
        .recv()
        .await
        .expect("bundle-upgrade should emit a QueueKernelRequest");
    match action {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, peer);
            let decoded = decode_request_item_message(&request_message)
                .expect("upgraded request should decode");
            match decoded {
                NockchainDataRequest::BlockWithTxsByHeight(height) => assert_eq!(height, 77),
                other => panic!("expected BlockWithTxsByHeight after upgrade, got {other:?}"),
            }
            assert_ne!(
                request_message, classic_message,
                "upgraded request bytes must differ from the classic %block %by-height jam"
            );
        }
        other => panic!("expected QueueKernelRequest, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "only one queued request expected for a single peer"
    );
}

#[tokio::test]
async fn test_block_request_effects_keep_stable_peer_subset_for_batching() {
    use tokio::sync::mpsc;

    let first_message = ByteBuf::from(jam_block_by_height_request(42));
    let second_message = ByteBuf::from(jam_block_by_height_request(43));
    let first_effect =
        request_slab_from_message(&first_message).expect("request slab should decode");
    let second_effect =
        request_slab_from_message(&second_message).expect("request slab should decode");
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peer_c = PeerId::random();
    let peers = vec![peer_a, peer_b, peer_c];
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());
    let permuted_peers = vec![peer_c, peer_a, peer_b];

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let (swarm_tx, mut swarm_rx) = mpsc::channel(16);
    handle_effect(
        first_effect,
        swarm_tx.clone(),
        peers.clone(),
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("first block request should be accepted");
    handle_effect(
        second_effect, swarm_tx, permuted_peers, false, state_arc, metrics,
    )
    .await
    .expect("second block request should be accepted");

    for expected_message in [first_message, second_message] {
        match swarm_rx.recv().await {
            Some(SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            }) => {
                assert_eq!(peer_id, canonical_peers[0]);
                assert_eq!(request_message, expected_message);
            }
            other => panic!("expected QueueKernelRequest action, got {:?}", other),
        }
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "block requests should keep the same stable peer across adjacent heights"
    );
}

#[tokio::test]
async fn test_block_request_effects_skip_previously_attempted_peers_on_repeat() {
    use tokio::sync::mpsc;

    let request_message = ByteBuf::from(jam_block_by_height_request(42));
    let effect_slab =
        request_slab_from_message(&request_message).expect("request slab should decode");
    let peers = (0..10).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    handle_effect(
        effect_slab.clone(),
        swarm_tx.clone(),
        peers.clone(),
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("first block request should be accepted");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        }) => {
            assert_eq!(peer_id, canonical_peers[0]);
            assert_eq!(queued_message, request_message);
        }
        other => panic!("expected QueueKernelRequest action, got {:?}", other),
    }

    handle_effect(effect_slab, swarm_tx, peers, false, state_arc, metrics)
        .await
        .expect("repeated block request should be accepted");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        }) => {
            assert_eq!(peer_id, canonical_peers[1]);
            assert_eq!(queued_message, request_message);
        }
        other => panic!("expected QueueKernelRequest action, got {:?}", other),
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "repeated block request should only queue unattempted peers"
    );
}

#[tokio::test]
async fn test_block_request_effects_recycle_attempted_peers_when_exhausted() {
    use tokio::sync::mpsc;

    let request_message = ByteBuf::from(jam_block_by_height_request(42));
    let effect_slab =
        request_slab_from_message(&request_message).expect("request slab should decode");
    let peer = PeerId::random();

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
    handle_effect(
        effect_slab.clone(),
        swarm_tx.clone(),
        vec![peer],
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("first single-peer block request should be accepted");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        }) => {
            assert_eq!(peer_id, peer);
            assert_eq!(queued_message, request_message);
        }
        other => panic!("expected QueueKernelRequest action, got {:?}", other),
    }

    handle_effect(
        effect_slab,
        swarm_tx,
        vec![peer],
        false,
        state_arc.clone(),
        metrics,
    )
    .await
    .expect("exhausted single-peer block request should recycle");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        }) => {
            assert_eq!(peer_id, peer);
            assert_eq!(queued_message, request_message);
        }
        other => panic!(
            "expected recycled QueueKernelRequest action, got {:?}",
            other
        ),
    }

    assert_eq!(
        state_arc.lock().await.get_block_height_attempted_peers(42),
        vec![peer],
        "recycled single-peer request should still record the live attempt",
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_gossip_effect_current_version_forwards_payload_and_clears_caches() {
    use tokio::sync::mpsc;

    let (effect_slab, expected_message) = build_gossip_effect(FACT_POKE_VERSION, &[1, 2, 3]);
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peers = vec![peer_a, peer_b];

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.block_cache.insert(42, NounSlab::new());
        state_guard
            .elders_cache
            .insert(String::from("elders"), NounSlab::new());
        state_guard
            .elders_negative_cache
            .insert(String::from("missing-elders"));
    }

    let (swarm_tx, mut swarm_rx) = mpsc::channel(4);
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        peers.clone(),
        false,
        state_arc.clone(),
        metrics,
    )
    .await;

    assert!(
        result.is_ok(),
        "current gossip fact version should be accepted"
    );

    for expected_peer in peers {
        match swarm_rx.recv().await {
            Some(SwarmAction::SendRequest {
                peer_id,
                request,
                request_context,
            }) => {
                assert_eq!(peer_id, expected_peer, "gossip should preserve peer order");
                assert!(request_context.is_none());
                assert_eq!(
                    request,
                    NockchainRequest::Gossip {
                        message: expected_message.clone(),
                    }
                );
            }
            other => panic!("expected gossip SendRequest action, got {:?}", other),
        }
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "should only send one gossip per peer"
    );

    let state_guard = state_arc.lock().await;
    assert!(state_guard.block_cache.is_empty());
    assert!(state_guard.elders_cache.is_empty());
    assert!(state_guard.elders_negative_cache.is_empty());
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_gossip_effect_suppresses_all_outbound_gossip_while_catching_up() {
    use tokio::sync::mpsc;

    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peers = vec![peer_a, peer_b];
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    {
        let source_peer = PeerId::random();
        let mut state_guard = state_arc.lock().await;
        for height in 100..110u64 {
            let (fact, _) = heard_block_fact_with_tx_ids(height, &[]);
            let block_id = match &fact {
                NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
                other => panic!("expected heard-block fact, got {other:?}"),
            };
            assert!(
                state_guard.defer_heard_block(source_peer, height, block_id, fact),
                "test setup should build a catch-up backlog",
            );
        }
        assert_eq!(
            state_guard.catch_up_signal().mode(),
            crate::catch_up::SyncMode::CatchingUp
        );
    }

    let (swarm_tx, mut swarm_rx) = mpsc::channel(4);
    for (tag, seed) in [("heard-block", 10), ("heard-tx", 20)] {
        let (effect_slab, _) = build_gossip_effect_with_tag(FACT_POKE_VERSION, tag, &[seed]);
        handle_effect(
            effect_slab,
            swarm_tx.clone(),
            peers.clone(),
            false,
            state_arc.clone(),
            metrics.clone(),
        )
        .await
        .expect("catch-up gossip suppression should not error");
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "catching-up nodes must not fan out block, tx, or mining gossip",
    );
    assert_eq!(metrics.gossip_suppressed_behind_tip_total.fetch_add(0), 2);
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_buffered_gossip_effect_returns_without_waiting_on_swarm_queue() {
    let (effect_slab, expected_message) = build_gossip_effect(FACT_POKE_VERSION, &[4, 5, 6]);
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peers = vec![peer_a, peer_b];
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let mut buffered_swarm_actions = VecDeque::new();

    tokio::time::timeout(Duration::from_millis(50), async {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_effect_with_dispatcher(
            effect_slab,
            &mut swarm_actions,
            peers.clone(),
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            state_arc,
            metrics,
            PeerExclusions::default(),
        )
        .await
    })
    .await
    .expect("buffered effect dispatch should not block")
    .expect("gossip effect should succeed");

    for expected_peer in peers {
        match buffered_swarm_actions.pop_front() {
            Some(SwarmAction::SendRequest {
                peer_id,
                request,
                request_context,
            }) => {
                assert_eq!(peer_id, expected_peer, "gossip should preserve peer order");
                assert!(request_context.is_none());
                assert_eq!(
                    request,
                    NockchainRequest::Gossip {
                        message: expected_message.clone(),
                    }
                );
            }
            other => panic!(
                "expected buffered gossip SendRequest action, got {:?}",
                other
            ),
        }
    }
    assert!(
        buffered_swarm_actions.is_empty(),
        "buffered gossip path should only queue one request per peer"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_gossip_effect_rejects_unsupported_versions_before_side_effects() {
    use tokio::sync::mpsc;

    let (effect_slab, _) = build_gossip_effect(FACT_POKE_VERSION + 1, &[9, 8, 7]);
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.block_cache.insert(7, NounSlab::new());
        state_guard
            .elders_cache
            .insert(String::from("elders"), NounSlab::new());
        state_guard
            .elders_negative_cache
            .insert(String::from("missing-elders"));
    }

    let (swarm_tx, mut swarm_rx) = mpsc::channel(1);
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![PeerId::random()],
        false,
        state_arc.clone(),
        metrics,
    )
    .await;

    match result {
        Err(NockAppError::OtherError(message)) => {
            assert!(
                message.contains("Unsupported gossip fact version 1"),
                "unexpected rejection error: {message}"
            );
        }
        other => panic!("expected unsupported-version error, got {:?}", other),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "unsupported gossip versions must not emit outbound requests"
    );

    let state_guard = state_arc.lock().await;
    assert_eq!(state_guard.block_cache.len(), 1);
    assert_eq!(state_guard.elders_cache.len(), 1);
    assert_eq!(state_guard.elders_negative_cache.len(), 1);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_liar_peer_effect() {
    use tokio::sync::mpsc;

    // Create a test peer ID and convert to base58
    let peer_id = PeerId::random();
    let peer_id_base58 = peer_id.to_base58();

    // Create the liar-peer effect noun
    let mut effect_slab = NounSlab::new();
    let liar_peer_atom =
        Atom::from_value(&mut effect_slab, "liar-peer").expect("Failed to create liar-peer atom");
    let peer_id_atom =
        Atom::from_value(&mut effect_slab, peer_id_base58).expect("Failed to create peer ID atom");
    let reason_atom = make_tas(&mut effect_slab, "bad peer");
    let effect = T(
        &mut effect_slab,
        &[liar_peer_atom.as_noun(), peer_id_atom.as_noun(), reason_atom.as_noun()],
    );
    effect_slab.set_root(effect);
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    // Create channel to receive SwarmAction
    let (swarm_tx, mut swarm_rx) = mpsc::channel(1);

    // Call handle_effect with the liar-peer effect
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        ))),
        metrics,
    )
    .await;

    // Verify the function succeeded
    assert!(result.is_ok(), "handle_effect should succeed");

    // Verify that a BlockPeer action was sent with the correct peer ID
    match swarm_rx.recv().await {
        Some(SwarmAction::BlockPeer {
            peer_id: blocked_peer,
        }) => {
            assert_eq!(blocked_peer, peer_id, "Wrong peer ID was blocked");
        }
        other => panic!("Expected BlockPeer action, got {:?}", other),
    }

    // Verify no more actions were sent
    assert!(swarm_rx.try_recv().is_err(), "Should only send one action");
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_track_add_effect() {
    use tokio::sync::mpsc;

    // Create test peer ID
    let peer_id = PeerId::random();
    let peer_id_base58 = peer_id.to_base58();

    // Create the track add effect noun
    let mut effect_slab = NounSlab::new();
    let track_atom = make_tas(&mut effect_slab, "track");
    let add_atom = make_tas(&mut effect_slab, "add");

    // Create block ID as [1 2 3 4 5]
    let block_id_tuple = T(&mut effect_slab, &[D(1), D(2), D(3), D(4), D(5)]);
    let peer_id_atom =
        Atom::from_value(&mut effect_slab, peer_id_base58).expect("Failed to create peer ID atom");

    // Build the noun structure: [%track %add block-id peer-id]
    let data_cell = T(&mut effect_slab, &[block_id_tuple, peer_id_atom.as_noun()]);
    let add_cell = T(&mut effect_slab, &[add_atom.as_noun(), data_cell]);
    let track_cell = T(&mut effect_slab, &[track_atom.as_noun(), add_cell]);
    effect_slab.set_root(track_cell);

    // Create message tracker and other required components
    let (swarm_tx, _swarm_rx) = mpsc::channel(1);

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    // Call handle_effect with the track add effect
    let result = handle_effect(
        effect_slab.clone(), // test fails if we don't clone
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        state_arc.clone(),
        metrics,
    )
    .await;

    // Verify the function succeeded
    assert!(result.is_ok(), "handle_effect should succeed");

    // Get the expected block ID string (base58 of [1 2 3 4 5])
    let effect_space = effect_slab.noun_space();
    let block_id_str = tip5_hash_to_base58(block_id_tuple, &effect_space).unwrap_or_else(|_| {
        panic!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        )
    });

    // Verify the message tracker state
    let state_guard = state_arc.lock().await;

    // Check block_id_to_peers mapping
    let peers = state_guard.get_peers_for_block_id(block_id_tuple, &effect_space);
    assert!(
        peers.contains(&peer_id),
        "Peer ID should be associated with block ID"
    );

    // Check peer_to_block_ids mapping
    let block_ids = state_guard.get_block_ids_for_peer(peer_id);
    assert!(
        block_ids.contains(&block_id_str),
        "Block ID should be associated with peer ID"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_track_remove_effect() {
    use tokio::sync::mpsc;

    // Create test peer ID
    let peer_id = PeerId::random();

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    // Create a message tracker and add an entry that we'll later remove
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    // Create block ID as [1 2 3 4 5]
    let mut setup_slab: NounSlab = NounSlab::new();
    let block_id_tuple = T(&mut setup_slab, &[D(1), D(2), D(3), D(4), D(5)]);
    let setup_space = setup_slab.noun_space();

    {
        let mut state_guard = state_arc.lock().await;
        state_guard
            .track_block_id_and_peer(block_id_tuple, peer_id, &setup_space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify it was added correctly
        assert!(state_guard.is_tracking_block_id(block_id_tuple, &setup_space));
        assert!(state_guard.is_tracking_peer(peer_id));
    }

    // Now create the track remove effect noun
    let mut effect_slab = NounSlab::new();
    let track_atom = make_tas(&mut effect_slab, "track");
    let remove_atom = make_tas(&mut effect_slab, "remove");

    // Copy the block ID tuple to the effect slab
    let block_id_tuple_in_effect = T(&mut effect_slab, &[D(1), D(2), D(3), D(4), D(5)]);

    // Build the noun structure: [%track %remove block-id]
    let remove_cell = T(
        &mut effect_slab,
        &[remove_atom.as_noun(), block_id_tuple_in_effect],
    );
    let track_cell = T(&mut effect_slab, &[track_atom.as_noun(), remove_cell]);
    effect_slab.set_root(track_cell);

    // Create channel for SwarmAction (not used in this test)
    let (swarm_tx, _swarm_rx) = mpsc::channel(1);

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    // Call handle_effect with the track remove effect
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        state_arc.clone(),
        metrics,
    )
    .await;

    // Verify the function succeeded
    assert!(result.is_ok(), "handle_effect should succeed");

    // Verify the message tracker state after removal
    let state_guard = state_arc.lock().await;

    // Check that the block ID was removed from block_id_to_peers
    assert!(
        !state_guard.is_tracking_block_id(block_id_tuple, &setup_space),
        "Block ID should be removed"
    );

    // Check that the peer's entry in peer_to_block_ids is also removed
    // (since this was the only block ID associated with the peer)
    assert!(
        !state_guard.is_tracking_peer(peer_id),
        "Peer ID should be removed since it has no more block IDs"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_liar_block_id_effect() {
    use tokio::sync::mpsc;

    println!("Starting test_liar_block_id_effect");

    // Create test peer IDs
    let bad_peer1 = PeerId::random();
    let bad_peer2 = PeerId::random();
    let good_peer = PeerId::random();
    println!(
        "Created peer_ids: bad1={}, bad2={}, good={}",
        bad_peer1, bad_peer2, good_peer
    );

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    // Create a message tracker and add entries
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    // Create block IDs
    let mut setup_slab: NounSlab = NounSlab::new();
    // Bad block ID as [1 2 3 4 5]
    let bad_block_id = T(&mut setup_slab, &[D(1), D(2), D(3), D(4), D(5)]);
    // Good block ID as [6 7 8 9 10]
    let good_block_id = T(&mut setup_slab, &[D(6), D(7), D(8), D(9), D(10)]);
    let setup_space = setup_slab.noun_space();
    println!("Created block_ids");

    {
        let mut state_guard = state_arc.lock().await;
        println!("Tracking block_ids and peers");

        // Associate bad_peer1 with the bad block
        state_guard
            .track_block_id_and_peer(bad_block_id, bad_peer1, &setup_space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Associate bad_peer2 with the bad block
        state_guard
            .add_peer_if_tracking_block_id(bad_block_id, bad_peer2, &setup_space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Associate good_peer with a different block
        state_guard
            .track_block_id_and_peer(good_block_id, good_peer, &setup_space)
            .unwrap_or_else(|_| {
                panic!(
                    "Called `expect()` at {}:{} (git sha: {})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA").unwrap_or("unknown")
                )
            });

        // Verify tracking is working
        assert!(state_guard.is_tracking_block_id(bad_block_id, &setup_space));
        assert!(state_guard.is_tracking_block_id(good_block_id, &setup_space));
        assert!(state_guard.is_tracking_peer(bad_peer1));
        assert!(state_guard.is_tracking_peer(bad_peer2));
        assert!(state_guard.is_tracking_peer(good_peer));
        println!("Verified tracking is working");
    }

    // Now create the liar-block-id effect noun for the bad block
    let mut effect_slab = NounSlab::new();
    let liar_block_id_atom = Atom::from_value(&mut effect_slab, "liar-block-id")
        .expect("Failed to create liar-block-id atom");

    // Copy the bad block ID tuple to the effect slab
    let bad_block_id_in_effect = T(&mut effect_slab, &[D(1), D(2), D(3), D(4), D(5)]);

    let reason_atom = make_tas(&mut effect_slab, "bad block");
    // Build the noun structure: [%liar-block-id bad-block-id]
    let effect = T(
        &mut effect_slab,
        &[liar_block_id_atom.as_noun(), bad_block_id_in_effect, reason_atom.as_noun()],
    );
    effect_slab.set_root(effect);
    println!("Created liar-block-id effect");

    // Create channel for SwarmAction
    let (swarm_tx, mut swarm_rx) = mpsc::channel(10); // Increased capacity for multiple actions
    println!("Created swarm channel");

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    // Call handle_effect with the liar-block-id effect
    println!("Calling handle_effect");
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        state_arc.clone(),
        metrics,
    )
    .await;

    println!("handle_effect result: {:?}", result);

    // Verify the function succeeded
    assert!(result.is_ok(), "handle_effect should succeed");
    println!("Verified handle_effect succeeded");

    // Collect all the block actions
    let mut blocked_peers = Vec::new();
    while let Ok(action) = swarm_rx.try_recv() {
        match action {
            SwarmAction::BlockPeer { peer_id } => {
                println!("Received BlockPeer action for peer: {}", peer_id);
                blocked_peers.push(peer_id);
            }
            other => {
                println!("Unexpected action received: {:?}", other);
                panic!("Expected BlockPeer action, got {:?}", other);
            }
        }
    }

    // Verify both bad peers were blocked
    assert_eq!(
        blocked_peers.len(),
        2,
        "Should have blocked exactly 2 peers"
    );
    assert!(
        blocked_peers.contains(&bad_peer1),
        "bad_peer1 should be blocked"
    );
    assert!(
        blocked_peers.contains(&bad_peer2),
        "bad_peer2 should be blocked"
    );
    assert!(
        !blocked_peers.contains(&good_peer),
        "good_peer should not be blocked"
    );
    println!("Verified correct peers were blocked");

    // Verify the bad block ID was removed from the tracker
    {
        let state_guard = state_arc.lock().await;

        // Bad block should be removed
        assert!(
            !state_guard.is_tracking_block_id(bad_block_id, &setup_space),
            "Bad block ID should be removed"
        );

        // Good block should still be tracked
        assert!(
            state_guard.is_tracking_block_id(good_block_id, &setup_space),
            "Good block ID should still be tracked"
        );

        // Bad peers should be removed
        assert!(
            !state_guard.is_tracking_peer(bad_peer1),
            "bad_peer1 should be removed from tracker"
        );
        assert!(
            !state_guard.is_tracking_peer(bad_peer2),
            "bad_peer2 should be removed from tracker"
        );

        // Good peer should still be tracked
        assert!(
            state_guard.is_tracking_peer(good_peer),
            "good_peer should still be tracked"
        );

        println!("Verified tracker state is correct after processing effect");
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test

async fn test_seen_block_effect() {
    use tokio::sync::mpsc;

    let mut effect_slab = NounSlab::new();
    let block_id = T(&mut effect_slab, &[D(1), D(2), D(3), D(4), D(5)]);
    let effect_space = effect_slab.noun_space();
    let block_id_str = tip5_hash_to_base58(block_id, &effect_space).unwrap_or_else(|_| {
        panic!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        )
    });
    let effect = T(
        &mut effect_slab,
        &[D(tas!(b"seen")), D(tas!(b"block")), block_id, D(0)],
    );
    effect_slab.set_root(effect);

    let (swarm_tx, _) = mpsc::channel(1);

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let state_arc_clone = Arc::clone(&state_arc); // Clone the Arc, not the MessageTracker
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        state_arc_clone,
        metrics,
    )
    .await;

    assert!(result.is_ok(), "handle_effect should succeed");

    // Verify that the block id was added to the seen_blocks set
    let state_guard = state_arc.lock().await;
    let contains = state_guard.seen_blocks.contains(&block_id_str);
    assert!(contains, "Block ID should be marked as seen");
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // ibig has a memory leak so miri fails this test
async fn test_seen_tx_effect() {
    use tokio::sync::mpsc;

    let mut effect_slab = NounSlab::new();
    let tx_id = T(&mut effect_slab, &[D(1), D(2), D(3), D(4), D(5)]);
    let effect_space = effect_slab.noun_space();
    let tx_id_str = tip5_hash_to_base58(tx_id, &effect_space).unwrap_or_else(|_| {
        panic!(
            "Called `expect()` at {}:{} (git sha: {})",
            file!(),
            line!(),
            option_env!("GIT_SHA").unwrap_or("unknown")
        )
    });
    let effect = T(&mut effect_slab, &[D(tas!(b"seen")), D(tas!(b"tx")), tx_id]);

    effect_slab.set_root(effect);

    let (swarm_tx, _) = mpsc::channel(1);
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let state_arc_clone = Arc::clone(&state_arc); // Clone the Arc, not the MessageTracker
    let result = handle_effect(
        effect_slab,
        swarm_tx,
        vec![], // connected peers (not relevant for this test)
        false,
        state_arc_clone,
        metrics,
    )
    .await;

    assert!(result.is_ok(), "handle_effect should succeed");

    // Verify that the tx id was added to the seen_txs set
    let state_guard = state_arc.lock().await;
    let contains = state_guard.seen_txs.contains(&tx_id_str);
    assert!(contains, "tx ID should be marked as seen");
}

#[tokio::test]
async fn test_execute_batch_request_items_preserves_wire_order_and_mixed_outcomes() {
    let items = vec![
        BatchRequestItem {
            item_id: 7,
            message: ByteBuf::from(vec![0x07]),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(vec![0x03]),
        },
        BatchRequestItem {
            item_id: 9,
            message: ByteBuf::from(vec![0x09]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);

    let results = execute_batch_request_items(
        &items,
        1024,
        |_| async { Ok(None) },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let item_id = item.item_id;
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    7 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    3 => BatchItemExecutionOutcome::Failed(BatchErrorClass::Decode),
                    9 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
                        response: NockchainResponse::Result {
                            message: ByteBuf::from(vec![0xAA]),
                        },
                        envelope: ResponseEnvelope::heard_tx(String::from("tx-9"), [0xAA]),
                    }),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("batch execution should succeed");

    assert_eq!(*observed_order.lock().unwrap(), vec![7, 3, 9]);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].item_id, 7);
    assert_eq!(results[0].status, BatchResultStatus::NotFound);
    assert_eq!(results[1].item_id, 3);
    assert_eq!(results[1].status, BatchResultStatus::Error);
    assert_eq!(results[1].error, Some(BatchErrorClass::Decode));
    assert_eq!(results[2].item_id, 9);
    assert_eq!(results[2].status, BatchResultStatus::Result);
    assert_eq!(
        results[2]
            .envelope
            .as_ref()
            .and_then(|envelope| envelope.tx_id.as_deref()),
        Some("tx-9")
    );
}

#[tokio::test]
async fn test_execute_batch_request_items_marks_backpressure_tail() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(vec![0x03]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);

    let results = execute_batch_request_items(
        &items,
        1024,
        |_| async { Ok(None) },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let item_id = item.item_id;
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    1 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    2 => BatchItemExecutionOutcome::Backpressure,
                    3 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("batch execution should succeed");

    assert_eq!(*observed_order.lock().unwrap(), vec![1, 2]);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, BatchResultStatus::NotFound);
    assert_eq!(results[1].status, BatchResultStatus::Error);
    assert_eq!(results[1].error, Some(BatchErrorClass::Backpressure));
    assert_eq!(results[2].status, BatchResultStatus::Error);
    assert_eq!(results[2].error, Some(BatchErrorClass::Backpressure));
}

#[tokio::test]
async fn test_map_batch_request_execution_error_treats_oneshot_drop_as_backpressure() {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    drop(tx);
    let recv_error = rx
        .await
        .expect_err("receiver should observe dropped sender");
    let outcome = map_batch_request_execution_error(
        &PeerId::random(),
        42,
        NockAppError::OneShotRecvError(recv_error),
    );

    assert!(
        matches!(outcome, BatchItemExecutionOutcome::Backpressure),
        "unexpected outcome: {outcome:?}"
    );
}

#[test]
fn test_map_batch_request_execution_error_treats_channel_close_as_backpressure() {
    let outcome =
        map_batch_request_execution_error(&PeerId::random(), 42, NockAppError::ChannelClosedError);

    assert!(
        matches!(outcome, BatchItemExecutionOutcome::Backpressure),
        "unexpected outcome: {outcome:?}"
    );
}

#[tokio::test]
async fn test_execute_batch_request_item_treats_dropped_peek_reply_as_backpressure() {
    let transcript = DriverTranscript::default();
    let scripted_traffic =
        build_scripted_traffic_cop_with_dropped_peek_reply(transcript.clone()).await;
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let item = BatchRequestItem {
        item_id: 17,
        message: ByteBuf::from(jam_raw_tx_request(42)),
    };

    let outcome = execute_batch_request_item(
        PeerId::random(),
        &item,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await;

    assert!(
        matches!(outcome, BatchItemExecutionOutcome::Backpressure),
        "unexpected outcome: {outcome:?}"
    );
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 1);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert!(transcript.render().contains("drop-reply"));
}

/// Scry-result helper: wrap a constructed page noun in the `[~ ~ payload]`
/// shape that `ScryResult::from` interprets as `Some(payload)`.
fn scry_some_page_with_tx_ids(height: u64, tx_seeds: &[u64]) -> (NounSlab, String, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id_noun,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let scry_some = T(&mut slab, &[D(0), D(0), page]);
    slab.set_root(scry_some);

    let block_id_base58 = {
        let mut id_slab = NounSlab::new();
        let id_noun = tip5_tuple(&mut id_slab, 10_000 + height);
        let space = id_slab.noun_space();
        tip5_hash_to_base58(id_noun, &space).expect("block-id tuple should convert to base58")
    };
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| {
            let mut tx_slab = NounSlab::new();
            let noun = tip5_tuple(&mut tx_slab, *seed);
            let space = tx_slab.noun_space();
            tip5_hash_to_base58(noun, &space).expect("tx-id tuple should convert to base58")
        })
        .collect();
    (slab, block_id_base58, tx_ids_base58)
}

fn base58_for_tip5_seed(seed: u64) -> String {
    let mut slab = NounSlab::new();
    let noun = tip5_tuple(&mut slab, seed);
    let space = slab.noun_space();
    tip5_hash_to_base58(noun, &space).expect("tip5 tuple should convert to base58")
}

fn raw_tx_noun(slab: &mut NounSlab, seed: u64, payload_len: usize) -> Noun {
    let tx_id = tip5_tuple(slab, seed);
    let payload = Atom::from_value(slab, vec![0xCDu8; payload_len])
        .expect("payload atom should build")
        .as_noun();
    T(slab, &[tx_id, payload])
}

fn raw_tx_entry_list(slab: &mut NounSlab, seeds: &[u64], payload_len: usize) -> Noun {
    seeds.iter().rev().fold(D(0), |list, seed| {
        let tx_id = tip5_tuple(slab, *seed);
        let raw_tx = raw_tx_noun(slab, *seed, payload_len);
        let entry = T(slab, &[tx_id, raw_tx]);
        T(slab, &[entry, list])
    })
}

fn raw_tx_entry_zmap(slab: &mut NounSlab, seeds: &[u64], payload_len: usize) -> Noun {
    seeds.iter().rev().fold(D(0), |tree, seed| {
        let tx_id = tip5_tuple(slab, *seed);
        let raw_tx = raw_tx_noun(slab, *seed, payload_len);
        let entry = T(slab, &[tx_id, raw_tx]);
        T(slab, &[entry, D(0), tree])
    })
}

fn validated_tx_entry_zmap(slab: &mut NounSlab, seeds: &[u64], payload_len: usize) -> Noun {
    seeds.iter().rev().fold(D(0), |tree, seed| {
        let tx_id = tip5_tuple(slab, *seed);
        let raw_tx = raw_tx_noun(slab, *seed, payload_len);
        let validated_tx = T(slab, &[D(0), raw_tx, D(payload_len as u64), D(0)]);
        let entry = T(slab, &[tx_id, validated_tx]);
        T(slab, &[entry, D(0), tree])
    })
}

fn raw_tx_index_entry_zmap(slab: &mut NounSlab, seeds: &[u64], payload_len: usize) -> Noun {
    seeds.iter().rev().fold(D(0), |tree, seed| {
        let tx_id = tip5_tuple(slab, *seed);
        let raw_tx = raw_tx_noun(slab, *seed, payload_len);
        let indexed_raw_tx = T(slab, &[raw_tx, D(44480)]);
        let entry = T(slab, &[tx_id, indexed_raw_tx]);
        T(slab, &[entry, D(0), tree])
    })
}

fn scry_some_heavy_txs(
    height: u64,
    tx_seeds: &[u64],
    payload_len: usize,
) -> (NounSlab, String, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id_noun,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let raw_txs = raw_tx_entry_list(&mut slab, tx_seeds, payload_len);
    let payload = T(&mut slab, &[D(height), block_id_noun, page, raw_txs]);
    let scry_some = T(&mut slab, &[D(0), D(0), payload]);
    slab.set_root(scry_some);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();

    (slab, block_id_base58, tx_ids_base58)
}

fn scry_some_single_block_range_with_txs(
    height: u64,
    tx_seeds: &[u64],
    payload_len: usize,
) -> (NounSlab, String, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id_noun,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let raw_txs = raw_tx_entry_zmap(&mut slab, tx_seeds, payload_len);
    let entry = T(&mut slab, &[D(height), block_id_noun, page, raw_txs]);
    let payload = T(&mut slab, &[entry, D(0)]);
    let scry_some = T(&mut slab, &[D(0), D(0), payload]);
    slab.set_root(scry_some);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();

    (slab, block_id_base58, tx_ids_base58)
}

fn scry_some_single_block_range_with_validated_txs(
    height: u64,
    tx_seeds: &[u64],
    payload_len: usize,
) -> (NounSlab, String, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id_noun,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let validated_txs = validated_tx_entry_zmap(&mut slab, tx_seeds, payload_len);
    let entry = T(&mut slab, &[D(height), block_id_noun, page, validated_txs]);
    let payload = T(&mut slab, &[entry, D(0)]);
    let scry_some = T(&mut slab, &[D(0), D(0), payload]);
    slab.set_root(scry_some);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();

    (slab, block_id_base58, tx_ids_base58)
}

fn scry_some_single_block_range_with_raw_tx_index_values(
    height: u64,
    tx_seeds: &[u64],
    payload_len: usize,
) -> (NounSlab, String, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
    let parent_id = tip5_tuple(&mut slab, 20_000 + height);
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id_noun,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    );
    let indexed_raw_txs = raw_tx_index_entry_zmap(&mut slab, tx_seeds, payload_len);
    let entry = T(
        &mut slab,
        &[D(height), block_id_noun, page, indexed_raw_txs],
    );
    let payload = T(&mut slab, &[entry, D(0)]);
    let scry_some = T(&mut slab, &[D(0), D(0), payload]);
    slab.set_root(scry_some);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();

    (slab, block_id_base58, tx_ids_base58)
}

fn scry_some_multi_block_range_with_txs(
    start_height: u64,
    tx_seed_sets: &[Vec<u64>],
    payload_len: usize,
) -> (NounSlab, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_ids_base58 = tx_seed_sets
        .iter()
        .enumerate()
        .map(|(idx, _)| base58_for_tip5_seed(10_000 + start_height + idx as u64))
        .collect::<Vec<_>>();
    let mut tail = D(0);

    for (idx, tx_seeds) in tx_seed_sets.iter().enumerate().rev() {
        let height = start_height + idx as u64;
        let block_id_noun = tip5_tuple(&mut slab, 10_000 + height);
        let parent_id = tip5_tuple(&mut slab, 20_000 + height);
        let tx_ids = tip5_zset(&mut slab, tx_seeds);
        let page = T(
            &mut slab,
            &[
                D(1),
                block_id_noun,
                D(0),
                parent_id,
                tx_ids,
                D(0),
                D(0),
                D(0),
                D(0),
                D(0),
                D(height),
                D(0),
            ],
        );
        let raw_txs = raw_tx_entry_zmap(&mut slab, tx_seeds, payload_len);
        let entry = T(&mut slab, &[D(height), block_id_noun, page, raw_txs]);
        tail = T(&mut slab, &[entry, tail]);
    }

    let scry_some = T(&mut slab, &[D(0), D(0), tail]);
    slab.set_root(scry_some);

    (slab, block_ids_base58)
}

fn range_batch_result_bytes(blocks: Vec<BundledBlockWithTxs>) -> usize {
    batch_result_encoded_bytes(&[BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(ResponseEnvelope::heard_block_range_with_txs(blocks)),
    }])
    .expect("range batch result should encode")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_bundle_assembles_block_plus_all_txs_in_page_order() {
    let transcript = DriverTranscript::default();
    let height = 321u64;
    let tx_seeds = [700u64, 800, 900];
    let (heavy_txs_scry, expected_block_id, expected_tx_ids) =
        scry_some_heavy_txs(height, &tx_seeds, 128);

    let scripted_traffic =
        build_scripted_traffic_cop(transcript, vec![Some(heavy_txs_scry)], Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockWithTxsByHeight(height),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await
    .expect("bundle execute should succeed");

    let envelope = match outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope,
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockWithTxs);
    assert_eq!(
        envelope.block_id.as_deref(),
        Some(expected_block_id.as_str())
    );
    assert!(envelope.tx_id.is_none());

    let bundled_tx_ids: Vec<String> = envelope
        .tx_envelopes
        .as_ref()
        .expect("bundle envelope must carry tx_envelopes")
        .iter()
        .map(|bundled| bundled.tx_id.clone())
        .collect();
    assert_eq!(
        bundled_tx_ids, expected_tx_ids,
        "bundled tx-ids must appear in page-declared order"
    );
    assert_eq!(
        envelope
            .unincluded_tx_ids
            .as_ref()
            .expect("bundle envelope must carry unincluded_tx_ids list")
            .len(),
        0,
        "all txs should fit under the default gen2_item_max_bytes cap"
    );
    envelope
        .validate()
        .expect("assembled bundle envelope must validate");

    assert_eq!(
        scripted_traffic.peek_count.load(Ordering::SeqCst),
        1,
        "bundle assembly should use one heavy-txs peek"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_bundle_uses_range_fallback_when_heavy_txs_is_missing() {
    let transcript = DriverTranscript::default();
    let height = 44471u64;
    let tx_seeds = [701u64, 801, 901];
    let (range_scry, expected_block_id, expected_tx_ids) =
        scry_some_single_block_range_with_txs(height, &tx_seeds, 128);

    let scripted_traffic =
        build_scripted_traffic_cop(transcript, vec![None, Some(range_scry)], Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockWithTxsByHeight(height),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await
    .expect("bundle range fallback should succeed");

    let envelope = match outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope,
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockWithTxs);
    assert_eq!(
        envelope.block_id.as_deref(),
        Some(expected_block_id.as_str())
    );
    let mut bundled_tx_ids: Vec<String> = envelope
        .tx_envelopes
        .as_ref()
        .expect("bundle envelope must carry tx_envelopes")
        .iter()
        .map(|bundled| bundled.tx_id.clone())
        .collect();
    let mut expected_tx_ids = expected_tx_ids;
    bundled_tx_ids.sort();
    expected_tx_ids.sort();
    assert_eq!(bundled_tx_ids, expected_tx_ids);
    envelope
        .validate()
        .expect("assembled fallback bundle envelope must validate");

    assert_eq!(
        scripted_traffic.peek_count.load(Ordering::SeqCst),
        2,
        "bundle fallback should use heavy-txs, then one range peek"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_range_accepts_validated_tx_map_values() {
    let transcript = DriverTranscript::default();
    let height = 44472u64;
    let tx_seeds = [702u64, 802, 902];
    let (range_scry, expected_block_id, expected_tx_ids) =
        scry_some_single_block_range_with_validated_txs(height, &tx_seeds, 128);

    let scripted_traffic =
        build_scripted_traffic_cop(transcript, vec![Some(range_scry)], Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockRangeWithTxs {
            start_height: height,
            len: 1,
        },
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await
    .expect("range execute should unwrap validated tx values");

    let envelope = match outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope,
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockRangeWithTxs);
    let range_blocks = envelope
        .range_blocks
        .as_ref()
        .expect("range envelope must carry blocks");
    assert_eq!(range_blocks.len(), 1);
    assert_eq!(range_blocks[0].block_id, expected_block_id);

    let mut bundled_tx_ids: Vec<String> = range_blocks[0]
        .tx_envelopes
        .iter()
        .map(|bundled| bundled.tx_id.clone())
        .collect();
    let mut expected_tx_ids = expected_tx_ids;
    bundled_tx_ids.sort();
    expected_tx_ids.sort();
    assert_eq!(bundled_tx_ids, expected_tx_ids);
    envelope
        .validate()
        .expect("assembled range envelope must validate");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_range_returns_budget_fitting_prefix() {
    let start_height = 44500u64;
    let tx_seed_sets = vec![vec![704u64, 804, 904], vec![705u64, 805, 905], vec![706u64, 806, 906]];
    let len = u8::try_from(tx_seed_sets.len()).expect("test range length should fit in u8");
    let (full_scry, expected_block_ids) =
        scry_some_multi_block_range_with_txs(start_height, &tx_seed_sets, 256);

    let full_scripted_traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![Some(full_scry)],
        Vec::new(),
    )
    .await;
    let metrics = isolated_test_metrics();
    let full_driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let full_outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockRangeWithTxs { start_height, len },
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &full_scripted_traffic.traffic,
        &metrics,
        &full_driver_state,
    )
    .await
    .expect("full range execute should succeed");
    let full_blocks = match full_outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope
            .range_blocks
            .expect("full range envelope must carry blocks"),
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };
    assert_eq!(full_blocks.len(), tx_seed_sets.len());

    let first_prefix_bytes = range_batch_result_bytes(full_blocks[..1].to_vec());
    let second_prefix_bytes = range_batch_result_bytes(full_blocks[..2].to_vec());
    assert!(second_prefix_bytes > first_prefix_bytes);
    let response_budget_bytes =
        first_prefix_bytes + ((second_prefix_bytes - first_prefix_bytes) / 2);

    let (capped_scry, _) = scry_some_multi_block_range_with_txs(start_height, &tx_seed_sets, 256);
    let capped_scripted_traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![Some(capped_scry)],
        Vec::new(),
    )
    .await;
    let capped_metrics = isolated_test_metrics();
    let capped_driver_state = Arc::new(Mutex::new(P2PState::new(
        capped_metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    limits.gen2_block_batch_max_response_bytes = response_budget_bytes;

    let capped_outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockRangeWithTxs { start_height, len },
        limits,
        &capped_scripted_traffic.traffic,
        &capped_metrics,
        &capped_driver_state,
    )
    .await
    .expect("capped range execute should succeed");
    let capped_blocks = match capped_outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope
            .range_blocks
            .expect("capped range envelope must carry blocks"),
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(capped_blocks.len(), 1);
    assert_eq!(capped_blocks[0].block_id, expected_block_ids[0]);
    assert!(
        range_batch_result_bytes(capped_blocks) <= response_budget_bytes,
        "range response should fit the configured block response budget"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_range_accepts_raw_tx_index_map_values() {
    let transcript = DriverTranscript::default();
    let height = 44472u64;
    let tx_seeds = [703u64, 803, 903];
    let (range_scry, expected_block_id, expected_tx_ids) =
        scry_some_single_block_range_with_raw_tx_index_values(height, &tx_seeds, 128);

    let scripted_traffic =
        build_scripted_traffic_cop(transcript, vec![Some(range_scry)], Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockRangeWithTxs {
            start_height: height,
            len: 1,
        },
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await
    .expect("range execute should unwrap raw-tx index values");

    let envelope = match outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope,
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockRangeWithTxs);
    let range_blocks = envelope
        .range_blocks
        .as_ref()
        .expect("range envelope must carry blocks");
    assert_eq!(range_blocks.len(), 1);
    assert_eq!(range_blocks[0].block_id, expected_block_id);

    let mut bundled_tx_ids: Vec<String> = range_blocks[0]
        .tx_envelopes
        .iter()
        .map(|bundled| bundled.tx_id.clone())
        .collect();
    let mut expected_tx_ids = expected_tx_ids;
    bundled_tx_ids.sort();
    expected_tx_ids.sort();
    assert_eq!(bundled_tx_ids, expected_tx_ids);
    envelope
        .validate()
        .expect("assembled range envelope must validate");
}

/// Build a bundle envelope directly from synthetic scry results so
/// receive-side tests don't have to spin up the full responder pipeline.
/// Mirrors the shape `execute_request_item` produces for a
/// `BlockWithTxsByHeight` request: block jam + per-tx jam envelopes in
/// declared order, plus any unincluded tx-ids.
fn build_bundle_envelope(
    height: u64,
    bundled_tx_seeds: &[u64],
    unincluded_tx_seeds: &[u64],
) -> (ResponseEnvelope, String, Vec<String>, Vec<String>) {
    let all_seeds: Vec<u64> = bundled_tx_seeds
        .iter()
        .chain(unincluded_tx_seeds.iter())
        .copied()
        .collect();
    let (page_scry, block_id_base58, declared_tx_ids) =
        scry_some_page_with_tx_ids(height, &all_seeds);
    let bundled_tx_ids: Vec<String> = declared_tx_ids
        .iter()
        .take(bundled_tx_seeds.len())
        .cloned()
        .collect();
    let unincluded_tx_ids: Vec<String> = declared_tx_ids
        .iter()
        .skip(bundled_tx_seeds.len())
        .cloned()
        .collect();

    let mut block_res_slab: NounSlab = NounSlab::new();
    let page_scry_space = page_scry.noun_space();
    let block_response = match create_scry_response(
        unsafe { page_scry.root() },
        &page_scry_space,
        "heard-block",
        &mut block_res_slab,
    ) {
        Right(Ok(response)) => response,
        other => panic!("heard-block scry synthesis failed: {other:?}"),
    };
    let block_message = match block_response {
        NockchainResponse::Result { message } => message,
        other => panic!("expected NockchainResponse::Result, got {other:?}"),
    };

    let mut bundled = Vec::new();
    for (tx_id, seed) in bundled_tx_ids.iter().zip(bundled_tx_seeds.iter()) {
        let tx_scry = scry_some_raw_tx(*seed, 128);
        let mut tx_res_slab: NounSlab = NounSlab::new();
        let tx_scry_space = tx_scry.noun_space();
        let tx_response = match create_scry_response(
            unsafe { tx_scry.root() },
            &tx_scry_space,
            "heard-tx",
            &mut tx_res_slab,
        ) {
            Right(Ok(response)) => response,
            other => panic!("heard-tx scry synthesis failed: {other:?}"),
        };
        let tx_message = match tx_response {
            NockchainResponse::Result { message } => message,
            other => panic!("expected NockchainResponse::Result, got {other:?}"),
        };
        bundled.push(BundledTxEnvelope {
            tx_id: tx_id.clone(),
            message: tx_message,
        });
    }

    let envelope = ResponseEnvelope::heard_block_with_txs(
        block_id_base58.clone(),
        &block_message,
        bundled,
        unincluded_tx_ids.clone(),
    );
    envelope
        .validate()
        .expect("synthetic bundle envelope must validate");
    (envelope, block_id_base58, bundled_tx_ids, unincluded_tx_ids)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_bundle_envelope_unpacks_block_txs_and_queues_unincluded_remainder() {
    use tokio::sync::mpsc;

    let transcript = DriverTranscript::default();
    let height = 55u64;
    let bundled_seeds = [500u64, 501];
    let unincluded_seeds = [502u64];
    let (envelope, _block_id, bundled_tx_ids, unincluded_tx_ids) =
        build_bundle_envelope(height, &bundled_seeds, &unincluded_seeds);

    // Three pokes: 1 block + 2 bundled txs. Scripted-poke results are all
    // Acks so `route_response_fact` completes successfully for each.
    let poke_results: Vec<PokeResult> = (0..(1 + bundled_seeds.len()))
        .map(|_| PokeResult::Ack)
        .collect();
    let scripted_traffic = build_scripted_traffic_cop(transcript, Vec::new(), poke_results).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = driver_state.lock().await;
        // Make the block on-frontier so route_response_fact pokes rather
        // than defers.
        state_guard.first_negative = height + 1;
    }
    let (swarm_tx, mut swarm_rx) = mpsc::channel(16);
    let peer = PeerId::random();

    route_bundle_envelope(
        peer, &envelope, &scripted_traffic.traffic, &metrics, &driver_state, &swarm_tx,
    )
    .await
    .expect("bundle unpack should succeed");

    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        1 + bundled_seeds.len(),
        "bundle should poke once for the block and once per bundled tx"
    );

    // Drain the swarm action queue and verify one QueueKernelRequest per
    // unincluded tx-id, each pointing to the same source peer.
    let mut queued_unincluded = Vec::new();
    for _ in 0..unincluded_tx_ids.len() {
        match swarm_rx
            .recv()
            .await
            .expect("unincluded remainder must queue a kernel request")
        {
            SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            } => {
                assert_eq!(peer_id, peer, "remainder request must stay on source peer");
                let NockchainDataRequest::RawTransactionById(tx_id, _) =
                    decode_request_item_message(&request_message)
                        .expect("remainder request should decode")
                else {
                    panic!("expected RawTransactionById remainder request");
                };
                queued_unincluded.push(tx_id);
            }
            other => panic!("expected QueueKernelRequest remainder, got {other:?}"),
        }
    }
    queued_unincluded.sort();
    let mut expected_unincluded = unincluded_tx_ids.clone();
    expected_unincluded.sort();
    assert_eq!(
        queued_unincluded, expected_unincluded,
        "each unincluded tx-id must be queued exactly once"
    );
    // Sanity: no extra actions beyond the remainder requests.
    assert!(
        tokio::time::timeout(Duration::from_millis(25), swarm_rx.recv())
            .await
            .is_err(),
        "bundle unpack should not queue extra SwarmActions"
    );

    // bundled_tx_ids is unused in asserts but retained so the test
    // remains self-describing if we later want to verify poke payloads.
    let _ = bundled_tx_ids;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_bundle_partial_fill_spills_overflow_to_unincluded() {
    let transcript = DriverTranscript::default();
    let height = 322u64;
    let tx_seeds = [1_001u64, 1_002, 1_003];
    // Each tx is 2 KiB of payload. With a 5 KiB per-item cap, only two
    // should fit; the third lands in unincluded_tx_ids.
    let tx_payload_bytes = 2 * 1024;
    let (heavy_txs_scry, expected_block_id, expected_tx_ids) =
        scry_some_heavy_txs(height, &tx_seeds, tx_payload_bytes);

    let scripted_traffic =
        build_scripted_traffic_cop(transcript, vec![Some(heavy_txs_scry)], Vec::new()).await;
    let metrics = isolated_test_metrics();
    let driver_state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let mut limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    // Tight per-item cap to force a partial fill. Block page ~ small header,
    // but each tx envelope contributes ~2 KiB message + overhead, so 5 KiB
    // leaves room for ~2 txs on top of the block header.
    limits.gen2_item_max_bytes = 5 * 1024;

    let outcome = execute_request_item(
        PeerId::random(),
        NockchainDataRequest::BlockWithTxsByHeight(height),
        limits,
        &scripted_traffic.traffic,
        &metrics,
        &driver_state,
    )
    .await
    .expect("partial bundle execute should succeed");

    let envelope = match outcome {
        RequestExecutionOutcome::Result { envelope, .. } => envelope,
        other => panic!("expected RequestExecutionOutcome::Result, got {other:?}"),
    };

    assert_eq!(envelope.kind, EnvelopeKind::HeardBlockWithTxs);
    assert_eq!(
        envelope.block_id.as_deref(),
        Some(expected_block_id.as_str())
    );

    let bundled_tx_ids: Vec<String> = envelope
        .tx_envelopes
        .as_ref()
        .expect("partial bundle must still carry tx_envelopes")
        .iter()
        .map(|bundled| bundled.tx_id.clone())
        .collect();
    let unincluded = envelope
        .unincluded_tx_ids
        .as_ref()
        .expect("partial bundle must carry unincluded_tx_ids");

    assert!(
        !bundled_tx_ids.is_empty(),
        "partial bundle must include at least one tx"
    );
    assert!(
        !unincluded.is_empty(),
        "partial bundle must leave at least one tx unincluded under the tight cap"
    );
    assert_eq!(
        bundled_tx_ids.len() + unincluded.len(),
        expected_tx_ids.len(),
        "every declared tx must appear in exactly one of the two lists"
    );
    for declared in &expected_tx_ids {
        assert!(
            bundled_tx_ids.contains(declared) || unincluded.contains(declared),
            "declared tx-id {declared} missing from both bundle lists"
        );
    }
    envelope
        .validate()
        .expect("partial bundle envelope must still validate");
}

#[tokio::test]
async fn test_decode_error_does_not_abort_sibling_batch_items() {
    let items = vec![
        BatchRequestItem {
            item_id: 11,
            message: ByteBuf::from(vec![0xFF, 0x00, 0xAA]),
        },
        BatchRequestItem {
            item_id: 12,
            message: ByteBuf::from(jam_block_by_height_request(42)),
        },
    ];

    let results = execute_batch_request_items(
        &items,
        1024,
        |_| async { Ok(None) },
        |item| {
            let message = item.message.clone();
            async move {
                match decode_request_item_message(&message) {
                    Ok(_) => {
                        BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound)
                    }
                    Err(_) => BatchItemExecutionOutcome::Failed(BatchErrorClass::Decode),
                }
            }
        },
    )
    .await
    .expect("batch execution should succeed");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].item_id, 11);
    assert_eq!(results[0].status, BatchResultStatus::Error);
    assert_eq!(results[0].error, Some(BatchErrorClass::Decode));
    assert_eq!(results[1].item_id, 12);
    assert_eq!(results[1].status, BatchResultStatus::NotFound);
}

#[test]
fn test_validate_batch_request_top_level_limits_rejects_item_cap() {
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 2,
        gen2_batch_max_bytes: 1024,
        gen2_item_max_bytes: 256,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(vec![0x03]),
        },
    ];

    let violation = validate_batch_request_top_level_limits(&items, limits)
        .expect_err("expected batch item cap violation");
    assert_eq!(
        violation,
        BatchTopLevelLimitViolation::TooManyItems {
            item_count: 3,
            max_items: 2,
        }
    );
}

#[test]
fn test_validate_batch_request_top_level_limits_accepts_exact_item_cap() {
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 2,
        gen2_batch_max_bytes: 1024,
        gen2_item_max_bytes: 256,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
    ];

    validate_batch_request_top_level_limits(&items, limits)
        .expect("batch at item cap should be accepted");
}

#[test]
fn test_validate_batch_request_top_level_limits_rejects_byte_cap() {
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 8,
        gen2_batch_max_bytes: 15,
        gen2_item_max_bytes: 256,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0xAA; 4]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0xBB; 4]),
        },
    ];

    let violation = validate_batch_request_top_level_limits(&items, limits)
        .expect_err("expected batch byte cap violation");
    assert_eq!(
        violation,
        BatchTopLevelLimitViolation::TooManyBytes {
            payload_bytes: 28,
            max_bytes: 15,
        }
    );
}

#[test]
fn test_validate_batch_request_top_level_limits_accepts_exact_byte_cap() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0xAA; 4]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0xBB; 4]),
        },
    ];
    let exact_payload_bytes =
        batch_request_payload_bytes(&items).expect("payload bytes should be computable");
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 8,
        gen2_batch_max_bytes: exact_payload_bytes,
        gen2_item_max_bytes: 256,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };

    validate_batch_request_top_level_limits(&items, limits)
        .expect("batch at byte cap should be accepted");
}

#[test]
fn test_batch_request_item_too_large_uses_configured_limit() {
    let item = BatchRequestItem {
        item_id: 99,
        message: ByteBuf::from(vec![0xAB; 9]),
    };
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 8,
        gen2_batch_max_bytes: 1024,
        gen2_item_max_bytes: 8,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };

    assert!(batch_request_item_too_large(&item, limits));
}

#[test]
fn test_batch_request_item_too_large_accepts_exact_limit() {
    let item = BatchRequestItem {
        item_id: 99,
        message: ByteBuf::from(vec![0xAB; 8]),
    };
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 8,
        gen2_batch_max_bytes: 1024,
        gen2_item_max_bytes: 8,
        gen2_block_batch_max_response_bytes: 512,
        gen2_max_inflight_per_peer: 4,
    };

    assert!(!batch_request_item_too_large(&item, limits));
}

#[test]
fn test_batch_results_fit_respects_exact_byte_boundary() {
    let results = vec![BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Ack,
        error: None,
        envelope: None,
    }];
    let encoded_bytes = batch_result_encoded_bytes(&results).expect("batch result should encode");

    assert!(batch_results_fit(&results, encoded_bytes).expect("exact boundary should evaluate"));
    assert!(
        !batch_results_fit(&results, encoded_bytes.saturating_sub(1))
            .expect("one-byte-over boundary should evaluate")
    );
}

#[test]
fn test_pending_gen2_batch_dedupes_duplicate_messages_with_stable_item_ids() {
    let mut pending_batch = PendingGen2Batch::default();
    let first = pending_batch
        .insert_request_message(&jam_block_by_height_request(1), 100, true, false)
        .expect("first insert should succeed");
    let duplicate = pending_batch
        .insert_request_message(&jam_block_by_height_request(1), 100, true, false)
        .expect("duplicate insert should succeed");
    let second = pending_batch
        .insert_request_message(&jam_block_by_height_request(2), 100, true, false)
        .expect("second insert should succeed");

    assert_eq!(
        first,
        PendingBatchInsertOutcome::Inserted {
            item_count: 1,
            payload_bytes: batch_request_payload_bytes(&pending_batch.items[0..1])
                .expect("payload size"),
            estimated_response_bytes: 100,
            contains_response_budget_item: true,
        }
    );
    assert_eq!(duplicate, PendingBatchInsertOutcome::Duplicate);
    assert_eq!(
        second,
        PendingBatchInsertOutcome::Inserted {
            item_count: 2,
            payload_bytes: batch_request_payload_bytes(&pending_batch.items).expect("payload size"),
            estimated_response_bytes: 200,
            contains_response_budget_item: true,
        }
    );
    assert_eq!(pending_batch.items.len(), 2);
    assert_eq!(pending_batch.items[0].item_id, 0);
    assert_eq!(pending_batch.items[1].item_id, 1);
}

#[test]
fn test_queue_pending_gen2_batch_request_increments_dedup_metric_once() {
    let peer_id = PeerId::random();
    let metrics = isolated_test_metrics();
    let mut pending_gen2_batches = BTreeMap::<PeerId, PendingGen2Batch>::new();

    let first = queue_pending_gen2_batch_request(
        &metrics,
        &mut pending_gen2_batches,
        peer_id,
        &jam_block_by_height_request(1),
        100,
        true,
    )
    .expect("first insert should succeed");

    assert!(matches!(first, PendingBatchInsertOutcome::Inserted { .. }));
    assert_eq!(metrics.req_res_effect_dedup_suppressed.fetch_add(0), 0);

    let duplicate = queue_pending_gen2_batch_request(
        &metrics,
        &mut pending_gen2_batches,
        peer_id,
        &jam_block_by_height_request(1),
        100,
        true,
    )
    .expect("duplicate insert should succeed");

    assert_eq!(duplicate, PendingBatchInsertOutcome::Duplicate);
    assert_eq!(metrics.req_res_effect_dedup_suppressed.fetch_add(0), 1);

    let second = queue_pending_gen2_batch_request(
        &metrics,
        &mut pending_gen2_batches,
        peer_id,
        &jam_block_by_height_request(2),
        100,
        true,
    )
    .expect("second insert should succeed");

    assert!(matches!(second, PendingBatchInsertOutcome::Inserted { .. }));
    assert_eq!(metrics.req_res_effect_dedup_suppressed.fetch_add(0), 1);
}

#[test]
fn test_pending_gen2_batch_request_key_formats_block_by_height() {
    assert_eq!(
        pending_gen2_batch_request_key(&jam_block_by_height_request(42)),
        "block-by-height:42"
    );
    assert_eq!(
        PENDING_GEN2_BATCH_DUPLICATE_REASON,
        "exact_message_already_pending"
    );
}

#[test]
fn test_batch_request_trace_keys_include_heights() {
    let items = vec![
        BatchRequestItem {
            item_id: 0,
            message: ByteBuf::from(jam_block_by_height_request(7)),
        },
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(jam_block_by_height_request(8)),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(jam_raw_tx_request(9)),
        },
    ];

    assert!(batch_request_keys_csv(&items).contains("block-by-height:7"));
    assert!(batch_request_keys_csv(&items).contains("block-by-height:8"));
    assert_eq!(batch_request_block_heights_csv(&items), "7,8");

    let request = NockchainRequest::BatchRequest {
        pow: [0; 16],
        nonce: 1,
        items,
    };
    assert!(outbound_request_keys_csv(&request).contains("block-by-height:7"));
    assert_eq!(outbound_request_block_heights_csv(&request), "7,8");
}

#[test]
fn test_pending_gen2_batch_request_key_formats_raw_tx_ids() {
    let message = jam_raw_tx_request(7);
    let NockchainDataRequest::RawTransactionById(tx_id, _) =
        decode_request_item_message(&message).expect("raw tx request should decode")
    else {
        panic!("expected raw tx request");
    };

    assert_eq!(
        pending_gen2_batch_request_key(&message),
        format!("raw-tx-by-id:{tx_id}")
    );
}

#[test]
fn test_pending_gen2_batch_request_key_has_stable_fallback_for_undecodable_messages() {
    assert_eq!(
        pending_gen2_batch_request_key(&[0xde, 0xad, 0xbe, 0xef]),
        "undecodable:4:deadbeef"
    );
}

#[test]
fn test_pending_gen2_batch_activates_response_budget_for_large_responses() {
    let limits = ReqResRuntimeLimits {
        request_high_threshold: LIBP2P_CONFIG.request_high_threshold,
        request_replay_cache_ttl: LIBP2P_CONFIG.request_replay_cache_ttl(),
        request_replay_cache_max_per_peer: LIBP2P_CONFIG.request_replay_cache_max_per_peer,
        ip_bucket_request_admission_limit: LIBP2P_CONFIG.ip_bucket_request_admission_limit,
        ip_bucket_connection_limit: LIBP2P_CONFIG.ip_bucket_connection_limit,
        gossip_bucket_capacity: LIBP2P_CONFIG.gossip_bucket_capacity,
        gossip_bucket_refill_per_second: LIBP2P_CONFIG.gossip_bucket_refill_per_second,
        authenticated_gossip_send_enabled: LIBP2P_CONFIG.req_res_authenticated_gossip_send_enabled,
        legacy_gossip_accept_enabled: LIBP2P_CONFIG.req_res_legacy_gossip_accept_enabled,
        block_range_max_len: LIBP2P_CONFIG.prefetch_window_max.max(1),
        gen2_batch_max_items: 128,
        gen2_batch_max_bytes: 1_048_576,
        gen2_item_max_bytes: 131_072,
        gen2_block_batch_max_response_bytes: 375_000,
        gen2_max_inflight_per_peer: 4,
    };
    let mut pending_batch = PendingGen2Batch::default();
    let raw_tx_request = jam_raw_tx_request(1);

    assert!(
        request_message_uses_response_budget(&raw_tx_request),
        "raw tx fetches must be gated by the response budget"
    );

    pending_batch
        .insert_request_message(&raw_tx_request, 200_000, true, true)
        .expect("tx insert should succeed");
    assert!(
        pending_batch.would_exceed_response_budget(200_000, true, limits),
        "coalesced raw tx requests must obey the same response budget"
    );
    assert!(
        pending_batch.would_exceed_response_budget(400_000, true, limits),
        "adding a block should also flush an oversized mixed batch before insertion"
    );

    let mut range_batch = PendingGen2Batch::default();
    let first_range =
        block_range_with_txs_request_message(100, 82).expect("range request message should encode");
    range_batch
        .insert_request_message(first_range.as_ref(), 200_000, true, false)
        .expect("range insert should succeed");
    assert!(
        range_batch.would_exceed_response_budget(200_000, true, limits),
        "coalesced block-range requests must obey the same response budget"
    );

    let mut block_batch = PendingGen2Batch::default();
    for height in 1..=2 {
        block_batch
            .insert_request_message(&jam_block_by_height_request(height), 125_000, true, false)
            .expect("block insert should succeed");
    }
    assert!(
        !block_batch.would_exceed_response_budget(125_000, true, limits),
        "another block should fit exactly at the configured block budget"
    );
    assert!(
        block_batch
            .insert_request_message(&jam_block_by_height_request(3), 125_000, true, false)
            .is_ok(),
        "the budget-filling block should still be admitted before the caller flushes the full batch"
    );
    assert!(
        block_batch.would_exceed_response_budget(1, true, limits),
        "any additional payload must flush once the bounded block budget is full"
    );
}

#[test]
fn test_pending_gen2_batch_raw_tx_only_defers_first_flush_tick() {
    let mut pending_batch = PendingGen2Batch::default();
    pending_batch
        .insert_request_message(&jam_raw_tx_request(42), 512, false, true)
        .expect("raw tx insert should succeed");

    assert!(
        !pending_batch.should_flush_on_tick(),
        "raw-tx-only batches should survive one extra coalesce tick"
    );
    assert!(
        pending_batch.should_flush_on_tick(),
        "raw-tx-only batches should flush on the second tick if nothing new arrives"
    );
}

#[test]
fn test_pending_gen2_batch_refreshes_raw_tx_defer_on_new_insert() {
    let mut pending_batch = PendingGen2Batch::default();
    pending_batch
        .insert_request_message(&jam_raw_tx_request(1), 512, false, true)
        .expect("first raw tx insert should succeed");

    assert!(
        !pending_batch.should_flush_on_tick(),
        "first tick should defer a raw-tx-only batch"
    );

    pending_batch
        .insert_request_message(&jam_raw_tx_request(2), 512, false, true)
        .expect("second raw tx insert should succeed");

    assert!(
        !pending_batch.should_flush_on_tick(),
        "a fresh raw-tx insert should reopen the extra coalesce tick"
    );
    assert!(
        pending_batch.should_flush_on_tick(),
        "the reopened batch should still flush once the refreshed grace period expires"
    );
}

#[test]
fn test_pending_gen2_batch_non_raw_flushes_on_first_tick() {
    let mut pending_batch = PendingGen2Batch::default();
    pending_batch
        .insert_request_message(&jam_block_by_height_request(42), 131_072, true, false)
        .expect("block insert should succeed");

    assert!(
        pending_batch.should_flush_on_tick(),
        "non-raw batches should keep the original one-tick flush behavior"
    );
}

#[tokio::test]
async fn test_suppress_duplicate_active_outbound_request_blocks_exact_inflight_repeat() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer_id = PeerId::random();
    let request_message = jam_block_by_height_request(42);
    let request_id = fresh_outbound_request_id();

    state_arc.lock().await.record_outbound_request(
        request_id,
        OutboundRequestContext::new(
            peer_id,
            ReqResGeneration::Gen1,
            NockchainRequest::Request {
                pow: [0; 16],
                nonce: 0,
                message: ByteBuf::from(request_message.clone()),
            },
        ),
    );

    {
        let state_guard = state_arc.lock().await;
        assert_eq!(state_guard.total_outbound_request_count(), 1);
        assert!(
            state_guard.has_active_outbound_request_item(peer_id, &request_message),
            "first outbound request should register its item bytes as active",
        );
    }
    assert_eq!(metrics.req_res_effect_dedup_suppressed.fetch_add(0), 0);

    let suppressed = suppress_duplicate_active_outbound_request(
        &state_arc, &metrics, peer_id, "request", &request_message,
    )
    .await;
    assert!(suppressed, "exact duplicate request should be suppressed");

    {
        let state_guard = state_arc.lock().await;
        assert_eq!(
            state_guard.total_outbound_request_count(),
            1,
            "duplicate queue request should not allocate a second outbound slot",
        );
    }
    assert_eq!(metrics.req_res_effect_dedup_suppressed.fetch_add(0), 1);

    let cleared = state_arc
        .lock()
        .await
        .clear_outbound_requests_for_peer(&peer_id);
    assert_eq!(
        cleared.len(),
        1,
        "test setup should clear the live outbound request"
    );

    let suppressed = suppress_duplicate_active_outbound_request(
        &state_arc, &metrics, peer_id, "request", &request_message,
    )
    .await;
    assert!(
        !suppressed,
        "request should be allowed again once the active outbound slot clears",
    );
}

#[test]
fn test_take_pending_batch_request_demotes_single_block_to_gen1_request() {
    let peer_id = PeerId::random();
    let local_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut pending_gen2_batches = BTreeMap::<PeerId, PendingGen2Batch>::new();
    let metrics = isolated_test_metrics();

    pending_gen2_batches
        .entry(peer_id)
        .or_default()
        .insert_request_message(&jam_block_by_height_request(42), 131_072, true, false)
        .expect("block insert should succeed");
    update_pending_batch_metrics(&metrics, &pending_gen2_batches);

    assert_eq!(metrics.gen2_batch_pending_peers.swap(0.0), 1.0);
    assert_eq!(metrics.gen2_batch_pending_items.swap(0.0), 1.0);

    let flushed = take_pending_batch_request(
        &mut pending_gen2_batches, peer_id, &local_peer_id, &mut equix_builder,
    )
    .expect("flush should succeed")
    .expect("pending batch should produce a request");

    assert_eq!(flushed.generation, ReqResGeneration::Gen1);
    let NockchainRequest::Request { message, .. } = flushed.request else {
        panic!("singleton block flush should demote to a gen1 Request");
    };
    assert!(matches!(
        decode_request_item_message(&message),
        Ok(NockchainDataRequest::BlockByHeight(42))
    ));
    assert!(
        pending_gen2_batches.is_empty(),
        "pending batch map should be drained after flush"
    );
    update_pending_batch_metrics(&metrics, &pending_gen2_batches);
    assert_eq!(metrics.gen2_batch_pending_peers.swap(0.0), 0.0);
    assert_eq!(metrics.gen2_batch_pending_items.swap(0.0), 0.0);
}

#[test]
fn test_take_pending_batch_request_keeps_multi_block_batch_on_gen2() {
    let peer_id = PeerId::random();
    let local_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut pending_gen2_batches = BTreeMap::<PeerId, PendingGen2Batch>::new();
    let metrics = isolated_test_metrics();
    let pending_batch = pending_gen2_batches.entry(peer_id).or_default();

    pending_batch
        .insert_request_message(&jam_block_by_height_request(7), 131_072, true, false)
        .expect("first block insert should succeed");
    pending_batch
        .insert_request_message(&jam_block_by_height_request(8), 131_072, true, false)
        .expect("second block insert should succeed");
    update_pending_batch_metrics(&metrics, &pending_gen2_batches);

    assert_eq!(metrics.gen2_batch_pending_peers.swap(0.0), 1.0);
    assert_eq!(metrics.gen2_batch_pending_items.swap(0.0), 2.0);

    let flushed = take_pending_batch_request(
        &mut pending_gen2_batches, peer_id, &local_peer_id, &mut equix_builder,
    )
    .expect("flush should succeed")
    .expect("pending batch should produce a request");

    assert_eq!(flushed.generation, ReqResGeneration::Gen2);
    let NockchainRequest::BatchRequest { items, .. } = flushed.request else {
        panic!("multi-block flush should stay as a gen2 BatchRequest");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(
        decode_request_item_message(&items[0].message),
        Ok(NockchainDataRequest::BlockByHeight(7))
    ));
    assert!(matches!(
        decode_request_item_message(&items[1].message),
        Ok(NockchainDataRequest::BlockByHeight(8))
    ));
    assert!(
        pending_gen2_batches.is_empty(),
        "pending batch map should be drained after flush"
    );
    update_pending_batch_metrics(&metrics, &pending_gen2_batches);
    assert_eq!(metrics.gen2_batch_pending_peers.swap(0.0), 0.0);
    assert_eq!(metrics.gen2_batch_pending_items.swap(0.0), 0.0);
}

#[test]
fn test_checkpoint_requester_replay_counts_cover_curve_and_near_cap_tail() {
    assert_eq!(checkpoint_requester_replay_counts(9), vec![1, 2, 8, 9]);
    assert_eq!(
        checkpoint_requester_replay_counts(16),
        vec![1, 2, 8, 15, 16]
    );
    assert_eq!(checkpoint_requester_replay_counts(8), vec![1, 2, 8]);
    assert_eq!(checkpoint_requester_replay_counts(3), vec![1, 2, 3]);
    assert_eq!(checkpoint_requester_replay_counts(1), vec![1]);
}

#[test]
fn test_should_batch_request_respects_item_and_batch_limits() {
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::new(
        peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_raw_tx_request(11)),
        },
    );
    let block_request_context = OutboundRequestContext::new(
        peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(22)),
        },
    );

    // gen2 send enabled, peer supports gen2, within limits
    assert!(should_batch_request(&request_context, true, true, 128, 256));
    // item too large
    assert!(!should_batch_request(&request_context, true, true, 8, 256));
    // batch too large
    assert!(!should_batch_request(&request_context, true, true, 128, 8));
    // gen2 send disabled
    assert!(!should_batch_request(
        &request_context, false, true, 128, 256
    ));
    // peer does not support gen2
    assert!(!should_batch_request(
        &request_context, true, false, 128, 256
    ));
    // block-by-height requests can join gen2 batches when the bounded response budget is active
    assert!(should_batch_request(
        &block_request_context, true, true, 128, 256
    ));
}

#[test]
fn test_outbound_request_generation_respects_peer_support_for_singletons() {
    let tx_request = NockchainRequest::Request {
        pow: [0; 16],
        nonce: 0,
        message: ByteBuf::from(jam_raw_tx_request(5)),
    };
    let block_request = NockchainRequest::Request {
        pow: [0; 16],
        nonce: 0,
        message: ByteBuf::from(jam_block_by_height_request(7)),
    };
    let gossip = NockchainRequest::Gossip {
        message: ByteBuf::from(vec![0xCD; 8]),
    };
    let batch = NockchainRequest::BatchRequest {
        pow: [0; 16],
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0xEF; 8]),
        }],
    };

    assert_eq!(
        outbound_request_generation(&tx_request, true, true),
        ReqResGeneration::Gen2
    );
    assert_eq!(
        outbound_request_generation(&tx_request, true, false),
        ReqResGeneration::Gen1
    );
    assert_eq!(
        outbound_request_generation(&tx_request, false, true),
        ReqResGeneration::Gen1
    );
    assert_eq!(
        outbound_request_generation(&block_request, true, true),
        ReqResGeneration::Gen2
    );
    assert_eq!(
        outbound_request_generation(&gossip, true, false),
        ReqResGeneration::Gen1
    );
    assert_eq!(
        outbound_request_generation(&batch, true, false),
        ReqResGeneration::Gen2
    );
}

#[test]
fn test_retry_delay_is_bounded_and_nonzero() {
    let delay = retry_delay_for_attempt(1);
    assert!(delay >= Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS));
    assert!(delay <= Duration::from_millis(GEN2_RETRY_MAX_DELAY_MS));
}

#[test]
fn test_build_retry_request_contexts_splits_batches_after_repeated_failures() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let request_context = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 5,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(1)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(2)),
                },
                BatchRequestItem {
                    item_id: 3,
                    message: ByteBuf::from(jam_block_by_height_request(3)),
                },
                BatchRequestItem {
                    item_id: 4,
                    message: ByteBuf::from(jam_block_by_height_request(4)),
                },
            ],
        },
        1,
        false,
    );

    let retry_contexts =
        build_retry_request_contexts(&request_context, &local_peer_id, &mut equix_builder, None)
            .expect("retry contexts should build");

    assert_eq!(retry_contexts.len(), 2);
    for retry_context in retry_contexts {
        assert_eq!(retry_context.retry_count, 2);
        match retry_context.request {
            NockchainRequest::BatchRequest { items, .. } => {
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected split retry batch request"),
        }
    }
}

#[test]
fn test_build_retry_request_contexts_honors_retry_budget_and_item_filter() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let exhausted = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(1)),
        },
        GEN2_RETRY_MAX_ATTEMPTS,
        false,
    );
    let filtered = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 10,
                    message: ByteBuf::from(jam_block_by_height_request(10)),
                },
                BatchRequestItem {
                    item_id: 20,
                    message: ByteBuf::from(jam_block_by_height_request(20)),
                },
            ],
        },
        0,
        false,
    );
    let retry_item_ids = BTreeSet::from([20]);

    assert!(
        build_retry_request_contexts(&exhausted, &local_peer_id, &mut equix_builder, None)
            .expect("exhausted retries should not error")
            .is_empty()
    );

    let filtered_retry = build_retry_request_contexts(
        &filtered,
        &local_peer_id,
        &mut equix_builder,
        Some(&retry_item_ids),
    )
    .expect("filtered retry should build");
    assert_eq!(filtered_retry.len(), 1);
    match &filtered_retry[0].request {
        NockchainRequest::BatchRequest { items, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].item_id, 20);
        }
        _ => panic!("expected filtered retry batch request"),
    }
}

#[test]
fn test_build_retry_request_contexts_drops_range_request_same_peer_retry() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let request_context = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: block_range_with_txs_request_message(100, 8)
                .expect("range request should encode"),
        },
        0,
        false,
    );

    let retry_contexts =
        build_retry_request_contexts(&request_context, &local_peer_id, &mut equix_builder, None)
            .expect("range retry decision should not error");

    assert!(
        retry_contexts.is_empty(),
        "range failures must re-enter peer selection rather than preserve the failed peer"
    );
}

#[test]
fn test_build_unsupported_protocol_fallback_contexts_decomposes_batch_to_gen1_requests() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let request_context = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 9,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(11)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(22)),
                },
            ],
        },
        0,
        false,
    );

    let fallback_contexts = build_unsupported_protocol_fallback_contexts(
        &request_context, &local_peer_id, &mut equix_builder,
    )
    .expect("fallback decomposition should succeed");

    assert_eq!(fallback_contexts.len(), 2);
    let mut fallback_heights = Vec::new();
    for fallback_context in &fallback_contexts {
        assert_eq!(fallback_context.peer_id, remote_peer_id);
        assert_eq!(fallback_context.generation, ReqResGeneration::Gen1);
        assert_eq!(fallback_context.retry_count, 1);
        assert!(fallback_context.fallback_attempted);
        let message = match &fallback_context.request {
            NockchainRequest::Request { message, .. } => message,
            _ => panic!("expected fallback singleton request"),
        };
        let data_request =
            decode_request_item_message(message).expect("fallback request should decode");
        let NockchainDataRequest::BlockByHeight(height) = data_request else {
            panic!("expected fallback block-by-height request");
        };
        fallback_heights.push(height);
        fallback_context
            .request
            .verify_pow(&mut equix_builder, &remote_peer_id, &local_peer_id)
            .expect("fallback request PoW should verify");
    }
    assert_eq!(
        fallback_heights,
        vec![11, 22],
        "fallback requests must preserve original batch wire order"
    );
}

#[test]
fn test_build_unsupported_protocol_fallback_contexts_rebuilds_singleton_requests_as_gen1() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();
    let request_context = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_raw_tx_request(8)),
        },
        0,
        false,
    );

    let fallback_contexts = build_unsupported_protocol_fallback_contexts(
        &request_context, &local_peer_id, &mut equix_builder,
    )
    .expect("singleton fallback should succeed");

    assert_eq!(fallback_contexts.len(), 1);
    let fallback_context = &fallback_contexts[0];
    assert_eq!(fallback_context.peer_id, remote_peer_id);
    assert_eq!(fallback_context.generation, ReqResGeneration::Gen1);
    assert_eq!(fallback_context.retry_count, 1);
    assert!(fallback_context.fallback_attempted);

    let message = match &fallback_context.request {
        NockchainRequest::Request { message, .. } => message,
        other => panic!("expected fallback singleton request, got {other:?}"),
    };
    let data_request =
        decode_request_item_message(message).expect("fallback request should decode");
    let NockchainDataRequest::RawTransactionById(tx_id, _) = data_request else {
        panic!("expected fallback raw transaction request");
    };
    assert!(!tx_id.is_empty());
    fallback_context
        .request
        .verify_pow(&mut equix_builder, &remote_peer_id, &local_peer_id)
        .expect("fallback request PoW should verify");
}

#[test]
fn test_build_unsupported_protocol_fallback_contexts_skips_repeated_or_gossip_fallbacks() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let mut equix_builder = equix::EquiXBuilder::new();

    let already_fallback = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen1,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(7)),
        },
        1,
        true,
    );
    let gossip = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Gossip {
            message: ByteBuf::from(jam_block_by_height_request(8)),
        },
        0,
        false,
    );

    assert!(build_unsupported_protocol_fallback_contexts(
        &already_fallback, &local_peer_id, &mut equix_builder,
    )
    .expect("already-fallback case should not error")
    .is_empty());
    assert!(build_unsupported_protocol_fallback_contexts(
        &gossip, &local_peer_id, &mut equix_builder,
    )
    .expect("gossip case should not error")
    .is_empty());
}

#[test]
fn test_record_batch_rejection_metrics_cover_reasons() {
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    record_batch_rejection(&metrics, BatchRejectReason::Malformed);
    record_batch_rejection(&metrics, BatchRejectReason::TooManyItems);
    record_batch_rejection(&metrics, BatchRejectReason::TooManyBytes);
    record_batch_rejection(&metrics, BatchRejectReason::Backpressure);

    assert_eq!(metrics.gen2_batch_rejected_malformed.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_rejected_too_many_items.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_rejected_too_many_bytes.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_rejected_backpressure.fetch_add(0), 1);
}

#[test]
fn test_queue_saturation_paths_preserve_reject_vs_defer_contract() {
    assert_eq!(
        queue_saturation_decision(QueueSaturationPath::InflightAdmission),
        "reject"
    );
    assert_eq!(
        queue_saturation_path(QueueSaturationPath::InflightAdmission),
        "inflight_admission"
    );
    assert_eq!(
        queue_saturation_decision(QueueSaturationPath::RequestExecution),
        "defer"
    );
    assert_eq!(
        queue_saturation_path(QueueSaturationPath::RequestExecution),
        "request_execution"
    );
    assert_eq!(
        queue_saturation_decision(QueueSaturationPath::ResponseRoute),
        "defer"
    );
    assert_eq!(
        queue_saturation_path(QueueSaturationPath::ResponseRoute),
        "response_route"
    );
    assert_eq!(
        queue_saturation_decision(QueueSaturationPath::GossipRoute),
        "defer"
    );
    assert_eq!(
        queue_saturation_path(QueueSaturationPath::GossipRoute),
        "gossip_route"
    );
}

#[test]
fn test_record_batch_result_item_errors_counts_each_error_class() {
    let metrics = isolated_test_metrics();
    let results = vec![
        batch_error_result(1, BatchErrorClass::Decode),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::TooLarge),
        batch_error_result(4, BatchErrorClass::InvalidPow),
        batch_error_result(5, BatchErrorClass::Internal),
        BatchResultItem {
            item_id: 6,
            status: BatchResultStatus::NotFound,
            error: None,
            envelope: None,
        },
    ];

    record_batch_result_item_errors(&metrics, &results);

    assert_eq!(metrics.gen2_batch_item_error_decode.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_item_error_backpressure.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_item_error_too_large.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_item_error_invalid_pow.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_item_error_internal.fetch_add(0), 1);
}

#[test]
fn test_record_req_res_fallback_increments_metric() {
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    record_req_res_fallback(&metrics, 0);
    record_req_res_fallback(&metrics, 3);

    assert_eq!(metrics.req_res_fallback_total.fetch_add(0), 3);
}

#[test]
fn test_record_block_by_height_gen1_routed_increments_only_for_exclusion() {
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );

    let block_request = NockchainRequest::Request {
        pow: [0; 16],
        nonce: 0,
        message: ByteBuf::from(jam_block_by_height_request(7)),
    };
    let tx_request = NockchainRequest::Request {
        pow: [0; 16],
        nonce: 0,
        message: ByteBuf::from(jam_raw_tx_request(5)),
    };

    // BlockByHeight with gen2 enabled + peer supports gen2 -> should increment
    record_block_by_height_gen1_routed(
        &metrics,
        ReqResGeneration::Gen1,
        true,
        true,
        &block_request,
    );
    assert_eq!(metrics.req_res_block_by_height_gen1_routed.fetch_add(0), 1);

    // Non-block request on gen1 with gen2 enabled -> should NOT increment
    record_block_by_height_gen1_routed(&metrics, ReqResGeneration::Gen1, true, true, &tx_request);
    assert_eq!(metrics.req_res_block_by_height_gen1_routed.fetch_add(0), 1);

    // BlockByHeight on gen1 but gen2 send disabled -> should NOT increment
    record_block_by_height_gen1_routed(
        &metrics,
        ReqResGeneration::Gen1,
        false,
        true,
        &block_request,
    );
    assert_eq!(metrics.req_res_block_by_height_gen1_routed.fetch_add(0), 1);

    // BlockByHeight on gen1 but peer does not support gen2 -> should NOT increment
    record_block_by_height_gen1_routed(
        &metrics,
        ReqResGeneration::Gen1,
        true,
        false,
        &block_request,
    );
    assert_eq!(metrics.req_res_block_by_height_gen1_routed.fetch_add(0), 1);

    // BlockByHeight on gen2 -> should NOT increment (impossible in practice but guards logic)
    record_block_by_height_gen1_routed(
        &metrics,
        ReqResGeneration::Gen2,
        true,
        true,
        &block_request,
    );
    assert_eq!(metrics.req_res_block_by_height_gen1_routed.fetch_add(0), 1);
}

#[test]
fn test_log_outbound_failure_tracks_gen2_timeout_metrics() {
    let metrics = isolated_test_metrics();
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::with_attempt(
        peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(jam_block_by_height_request(7)),
            }],
        },
        1,
        false,
    );

    log_outbound_failure(
        peer_id,
        fresh_outbound_request_id(),
        request_response::OutboundFailure::Timeout,
        Some(&request_context),
        metrics.clone(),
    );

    assert_eq!(metrics.request_failed.fetch_add(0), 1);
    assert_eq!(metrics.gen2_outbound_failures.fetch_add(0), 1);
    assert_eq!(metrics.gen2_outbound_timeouts.fetch_add(0), 1);
    assert_eq!(metrics.gen1_outbound_failures.fetch_add(0), 0);
}

#[test]
fn test_log_outbound_failure_tracks_gen1_timeout_metrics() {
    let metrics = isolated_test_metrics();
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::with_attempt(
        peer_id,
        ReqResGeneration::Gen1,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(7)),
        },
        1,
        false,
    );

    log_outbound_failure(
        peer_id,
        fresh_outbound_request_id(),
        request_response::OutboundFailure::Timeout,
        Some(&request_context),
        metrics.clone(),
    );

    assert_eq!(metrics.request_failed.fetch_add(0), 1);
    assert_eq!(metrics.gen1_outbound_failures.fetch_add(0), 1);
    assert_eq!(metrics.gen1_outbound_timeouts.fetch_add(0), 1);
    assert_eq!(metrics.gen2_outbound_failures.fetch_add(0), 0);
    assert_eq!(metrics.gen2_outbound_timeouts.fetch_add(0), 0);
}

#[test]
fn test_log_outbound_failure_tracks_gen1_non_timeout_metrics() {
    let metrics = isolated_test_metrics();
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::with_attempt(
        peer_id,
        ReqResGeneration::Gen1,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(9)),
        },
        1,
        false,
    );

    log_outbound_failure(
        peer_id,
        fresh_outbound_request_id(),
        request_response::OutboundFailure::ConnectionClosed,
        Some(&request_context),
        metrics.clone(),
    );

    assert_eq!(metrics.request_failed.fetch_add(0), 1);
    assert_eq!(metrics.gen1_outbound_failures.fetch_add(0), 1);
    assert_eq!(metrics.gen1_outbound_timeouts.fetch_add(0), 0);
    assert_eq!(metrics.gen2_outbound_failures.fetch_add(0), 0);
    assert_eq!(metrics.gen2_outbound_timeouts.fetch_add(0), 0);
}

#[tokio::test]
async fn test_queue_retry_requests_tracks_scheduled_total() {
    let metrics = isolated_test_metrics();
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(1);
    let peer_id = PeerId::random();
    let requests = vec![OutboundRequestContext::with_attempt(
        peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_block_by_height_request(1)),
        },
        1,
        false,
    )];

    queue_retry_requests(&swarm_tx, &metrics, requests, Duration::from_millis(25))
        .await
        .expect("retry queue should succeed");

    match swarm_rx.recv().await {
        Some(SwarmAction::RetryRequests { requests, delay }) => {
            assert_eq!(requests.len(), 1);
            assert_eq!(delay, Duration::from_millis(25));
        }
        other => panic!("expected RetryRequests, got {:?}", other),
    }
    assert_eq!(metrics.req_res_retry_scheduled_total.fetch_add(0), 1);
}

#[tokio::test]
async fn wrong_peer_id_for_connected_obtained_peer_removes_stale_address_without_ip_exclusion() {
    let peer_exclusions = PeerExclusions::default();
    let mut swarm = start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        peer_exclusions.clone(),
    )
    .expect("driver swarm should build");
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let expected = PeerId::random();
    let obtained = PeerId::random();
    let stale_base_addr = loopback_quic_addr_with_port(36046);
    let stale_expected_addr = stale_base_addr
        .clone()
        .with_p2p(expected)
        .expect("stale address should accept peer id");
    let ip = stale_base_addr
        .ip_addr()
        .expect("stale address should include an IP");

    {
        let mut state_guard = state_arc.lock().await;
        state_guard.track_connection(
            ConnectionId::new_unchecked(1),
            obtained,
            &stale_base_addr,
            libp2p::core::ConnectedPoint::Dialer {
                address: stale_base_addr.clone(),
                role_override: libp2p::core::Endpoint::Dialer,
                port_use: libp2p::core::transport::PortUse::Reuse,
            },
        );
    }
    swarm
        .behaviour_mut()
        .kad
        .add_address(&expected, stale_base_addr.clone());
    swarm
        .behaviour_mut()
        .peer_store
        .store_mut()
        .add_address(&expected, &stale_base_addr);

    handle_outgoing_connection_error(
        &mut swarm,
        &state_arc,
        &peer_exclusions,
        &metrics,
        DialError::WrongPeerId {
            obtained,
            address: stale_expected_addr,
        },
    )
    .await;

    assert!(
        !peer_exclusions.is_ip_excluded(&ip),
        "stale address for an already-connected peer should not exclude the IP"
    );
    assert!(
        state_arc
            .lock()
            .await
            .peer_connections
            .contains_key(&obtained),
        "the active obtained peer should stay connected in driver state"
    );
    let mut stale_peer_in_kad = false;
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        if bucket
            .iter()
            .any(|entry| entry.node.key.into_preimage() == expected)
        {
            stale_peer_in_kad = true;
            break;
        }
    }
    assert!(
        !stale_peer_in_kad,
        "stale expected peer should be removed from Kademlia"
    );
    assert!(
        swarm
            .behaviour_mut()
            .peer_store
            .address_of_peer(&expected)
            .is_none(),
        "stale expected peer should be removed from the peer store"
    );
}

#[tokio::test]
async fn wrong_peer_id_for_unconnected_obtained_peer_records_address_cooldown() {
    let peer_exclusions = PeerExclusions::default();
    let mut swarm = start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        peer_exclusions.clone(),
    )
    .expect("driver swarm should build");
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let expected = PeerId::random();
    let obtained = PeerId::random();
    let stale_base_addr = loopback_quic_addr_with_port(36047);
    let stale_expected_addr = stale_base_addr
        .clone()
        .with_p2p(expected)
        .expect("stale address should accept peer id");
    let ip = stale_base_addr
        .ip_addr()
        .expect("stale address should include an IP");

    handle_outgoing_connection_error(
        &mut swarm,
        &state_arc,
        &peer_exclusions,
        &metrics,
        DialError::WrongPeerId {
            obtained,
            address: stale_expected_addr,
        },
    )
    .await;

    assert!(
        peer_exclusions.is_address_excluded(&stale_base_addr, Some(expected)),
        "unknown wrong-peer-id responses should cool down the stale address"
    );
    assert!(
        !peer_exclusions.is_ip_excluded(&ip),
        "single wrong-peer-id response should not exclude the IP before the repeat threshold"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_outbound_gen2_batch_send_updates_send_counters() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path outbound gen2 batch send increments transport counters",
    );

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let mut requester = start_swarm(
        requester_config,
        Keypair::generate_ed25519(),
        vec![loopback_quic_addr()],
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("driver swarm should build");
    let mut responder = build_test_swarm(responder_config);
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match requester.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    transcript.record("driver-swarm", format!("listening on {address}"));
                    return address;
                }
                other => transcript.record(
                    "driver-swarm",
                    format!("waiting for listen addr saw {other:?}"),
                ),
            }
        }
    })
    .await
    .expect("driver swarm listen address timeout");
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    requester
        .dial(responder_addr.clone())
        .expect("driver swarm dial should be accepted");
    transcript.record("driver-swarm", format!("dialing {responder_addr}"));

    let mut requester_connected = false;
    let mut responder_connected = false;
    tokio::time::timeout(Duration::from_secs(15), async {
            while !(requester_connected && responder_connected) {
                tokio::select! {
                    event = requester.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == responder_peer_id => {
                                requester_connected = true;
                                transcript.record("driver-swarm", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("driver-swarm", format!("connect loop saw {other:?}")),
                        }
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == requester_peer_id => {
                                responder_connected = true;
                                transcript.record("responder", format!("connected to {peer_id}"));
                            }
                            other => transcript.record("responder", format!("connect loop saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("mixed swarm connection timeout");

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(jam_raw_tx_request(77_001)),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(jam_raw_tx_request(77_002)),
            },
        ],
    )
    .expect("outbound batch request should build");

    send_outbound_request_now(
        &mut requester,
        &state_arc,
        &metrics,
        OutboundRequestContext::new(responder_peer_id, ReqResGeneration::Gen2, request),
    )
    .await;

    assert_eq!(metrics.gen2_batch_requests_sent.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_items_sent.fetch_add(0), 2);
    assert_eq!(metrics.gen2_batch_requests_received.fetch_add(0), 0);
    assert_eq!(metrics.gen2_batch_items_received.fetch_add(0), 0);

    let (peer, message) = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                tokio::select! {
                    event = requester.select_next_some() => {
                        transcript.record("driver-swarm", format!("request pump saw {event:?}"));
                    }
                    event = responder.select_next_some() => {
                        match event {
                            SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                                request_response::Event::Message { peer, message, .. },
                            )) => {
                                transcript.record(
                                    "responder",
                                    format!("received driver-path request from {peer}"),
                                );
                                return (peer, message);
                            }
                            other => transcript.record("responder", format!("request wait saw {other:?}")),
                        }
                    }
                }
            }
        })
        .await
        .expect("driver send-path request event timeout");
    assert_eq!(peer, requester_peer_id);
    let request_response::Message::Request { request, .. } = message else {
        panic!("expected outbound batch request to arrive on the live req-res path");
    };
    let NockchainRequest::BatchRequest { items, .. } = request else {
        panic!("expected outbound gen2 request to stay batched");
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(
        decode_request_item_message(&items[0].message),
        Ok(NockchainDataRequest::RawTransactionById(_, _))
    ));
    assert!(matches!(
        decode_request_item_message(&items[1].message),
        Ok(NockchainDataRequest::RawTransactionById(_, _))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_inbound_gen2_batch_request_updates_receive_counters() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path inbound gen2 batch request increments transport counters",
    );

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let limits = runtime_limits_from_config(&responder_config);
    let mut requester = build_test_swarm(requester_config);
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let first_scry = scry_some_raw_tx(88_001, 16);
    let second_scry = scry_some_raw_tx(88_002, 24);
    let expected_results = vec![
        tx_result_item_from_scry(1, &first_scry),
        tx_result_item_from_scry(2, &second_scry),
    ];
    let scripted_traffic = build_scripted_traffic_cop(
        transcript.clone(),
        vec![Some(first_scry), Some(second_scry)],
        Vec::new(),
    )
    .await;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    let request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(jam_raw_tx_request(88_001)),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(jam_raw_tx_request(88_002)),
            },
        ],
    )
    .expect("inbound batch request should build");
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, request);

    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    run_driver_with_timeout(
        &transcript,
        "driver should process inbound gen2 batch request for receive counter proof",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx,
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process inbound batch request");

    let response = match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse { channel, response } => {
            responder
                .behaviour_mut()
                .request_response
                .send_response(channel, response.clone())
                .expect("batch response should send");
            response
        }
        other => panic!("expected SendResponse for inbound batch request, got {other:?}"),
    };

    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: expected_results.clone(),
        }
    );
    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(
        requester_response,
        NockchainResponse::BatchResult {
            results: expected_results,
        }
    );

    assert_eq!(metrics.gen2_batch_requests_received.fetch_add(0), 1);
    assert_eq!(metrics.gen2_batch_items_received.fetch_add(0), 2);
    assert_eq!(metrics.gen2_batch_requests_sent.fetch_add(0), 0);
    assert_eq!(metrics.gen2_batch_items_sent.fetch_add(0), 0);
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 2);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
}

#[test]
#[ignore = "benchmark harness, run explicitly with -- --ignored --nocapture"]
fn req_res_gen2_transport_report() {
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let iterations = 8usize;
    let workloads = [
        // -- existing --
        (
            "block-burst-32",
            (0..32)
                .map(|height| jam_block_by_height_request(height + 1))
                .collect::<Vec<_>>(),
        ),
        (
            "mixed-block-tx-128",
            (0..128)
                .map(|idx| {
                    if idx % 2 == 0 {
                        jam_block_by_height_request(idx as u64 + 1)
                    } else {
                        jam_raw_tx_request(idx as u64 + 1000)
                    }
                })
                .collect::<Vec<_>>(),
        ),
        // -- new: pure tx at max batch depth --
        (
            "tx-burst-128",
            (0..128)
                .map(|seed| jam_raw_tx_request(seed + 3000))
                .collect::<Vec<_>>(),
        ),
        // -- new: pure blocks at max batch depth --
        (
            "block-burst-128",
            (0..128)
                .map(|height| jam_block_by_height_request(height + 1))
                .collect::<Vec<_>>(),
        ),
        // -- new: realistic sync ratio (8 blocks + 120 txs per block) --
        (
            "sync-ratio-128",
            (0..128)
                .map(|idx| {
                    if idx < 8 {
                        jam_block_by_height_request(idx as u64 + 500)
                    } else {
                        jam_raw_tx_request(idx as u64 + 5000)
                    }
                })
                .collect::<Vec<_>>(),
        ),
        // -- new: single-item batch (measures batching overhead tax) --
        ("single-item-1", vec![jam_raw_tx_request(9001)]),
    ];

    // -- crossover sweep: find the item count where gen2 beats gen1 --
    let crossover_counts = [1usize, 2, 4, 8, 16, 32, 64, 128];

    println!("req-res gen2 transport benchmark");
    println!(
        "config: iterations={} batch_max_items={} batch_max_bytes={} item_max_bytes={}",
        iterations,
        LIBP2P_CONFIG.gen2_batch_max_items(),
        LIBP2P_CONFIG.gen2_batch_max_bytes(),
        LIBP2P_CONFIG.gen2_item_max_bytes(),
    );
    println!(
        "{:<22} {:>10} {:>14} {:>14} {:>12} {:>12}",
        "workload", "items", "gen1_ms", "gen2_ms", "bytes_ratio", "payload_fit"
    );

    for (label, messages) in workloads {
        let (gen1_elapsed, gen1_bytes) =
            benchmark_singleton_requests(&local_peer_id, &remote_peer_id, &messages, iterations);
        let (gen2_elapsed, gen2_bytes, batch_payload_bytes) =
            benchmark_batch_request(&local_peer_id, &remote_peer_id, &messages, iterations);
        let bytes_ratio = gen2_bytes as f64 / gen1_bytes as f64;
        let payload_fit = batch_payload_bytes <= LIBP2P_CONFIG.gen2_batch_max_bytes();

        println!(
            "{:<22} {:>10} {:>14.3} {:>14.3} {:>12.3} {:>12}",
            label,
            messages.len(),
            gen1_elapsed.as_secs_f64() * 1_000.0,
            gen2_elapsed.as_secs_f64() * 1_000.0,
            bytes_ratio,
            payload_fit,
        );
        assert!(
            payload_fit,
            "benchmark workload must fit current batch byte cap"
        );
    }

    println!();
    println!("gen2 crossover analysis (tx-only, mixed-block-tx)");
    println!(
        "{:<22} {:>10} {:>14} {:>14} {:>12}",
        "workload", "items", "gen1_ms", "gen2_ms", "speedup"
    );
    for count in crossover_counts {
        let tx_messages: Vec<Vec<u8>> = (0..count)
            .map(|seed| jam_raw_tx_request(seed as u64 + 7000))
            .collect();
        let (gen1_elapsed, _) =
            benchmark_singleton_requests(&local_peer_id, &remote_peer_id, &tx_messages, iterations);
        let (gen2_elapsed, _, _) =
            benchmark_batch_request(&local_peer_id, &remote_peer_id, &tx_messages, iterations);
        let speedup = gen1_elapsed.as_secs_f64() / gen2_elapsed.as_secs_f64();
        println!(
            "{:<22} {:>10} {:>14.3} {:>14.3} {:>12.1}x",
            format!("crossover-tx-{count}"),
            count,
            gen1_elapsed.as_secs_f64() * 1_000.0,
            gen2_elapsed.as_secs_f64() * 1_000.0,
            speedup,
        );
    }
}

#[test]
#[ignore = "payload-fit harness, run explicitly with -- --ignored --nocapture"]
fn req_res_gen2_payload_fit_report() {
    let block_messages = (0..256)
        .map(|height| jam_block_by_height_request(height + 1))
        .collect::<Vec<_>>();
    let mixed_messages = (0..256)
        .map(|idx| {
            if idx % 2 == 0 {
                jam_block_by_height_request(idx as u64 + 1)
            } else {
                jam_raw_tx_request(idx as u64 + 2000)
            }
        })
        .collect::<Vec<_>>();

    println!("req-res gen2 payload-fit report");
    println!(
        "config: batch_max_items={} batch_max_bytes={} item_max_bytes={}",
        LIBP2P_CONFIG.gen2_batch_max_items(),
        LIBP2P_CONFIG.gen2_batch_max_bytes(),
        LIBP2P_CONFIG.gen2_item_max_bytes(),
    );
    println!(
        "{:<22} {:>10} {:>14} {:>14}",
        "workload", "fit_items", "payload_bytes", "item_bytes_max"
    );

    let tx_messages = (0..256)
        .map(|seed| jam_raw_tx_request(seed as u64 + 4000))
        .collect::<Vec<_>>();
    let tx_heavy_messages = (0..256)
        .map(|idx| {
            // 90% tx, 10% block, closer to real sync ratio
            if idx % 10 == 0 {
                jam_block_by_height_request(idx as u64 + 1)
            } else {
                jam_raw_tx_request(idx as u64 + 6000)
            }
        })
        .collect::<Vec<_>>();

    for (label, messages) in [
        ("block-only", block_messages),
        ("mixed-block-tx", mixed_messages),
        ("tx-only", tx_messages),
        ("tx-heavy-90pct", tx_heavy_messages),
    ] {
        let mut fit_items = 0usize;
        let mut payload_bytes = std::mem::size_of::<u32>();
        let mut max_item_bytes = 0usize;
        for message in &messages {
            max_item_bytes = max_item_bytes.max(message.len());
            let next_payload_bytes = payload_bytes
                .checked_add(std::mem::size_of::<u32>())
                .and_then(|bytes| bytes.checked_add(std::mem::size_of::<u32>()))
                .and_then(|bytes| bytes.checked_add(message.len()))
                .expect("payload size overflow");
            if fit_items >= LIBP2P_CONFIG.gen2_batch_max_items()
                || next_payload_bytes > LIBP2P_CONFIG.gen2_batch_max_bytes()
            {
                break;
            }
            payload_bytes = next_payload_bytes;
            fit_items += 1;
        }

        println!(
            "{:<22} {:>10} {:>14} {:>14}",
            label, fit_items, payload_bytes, max_item_bytes
        );
        assert!(
            fit_items > 0,
            "payload-fit harness must admit at least one item"
        );
        assert!(
            max_item_bytes <= LIBP2P_CONFIG.gen2_item_max_bytes(),
            "workload item must respect the configured per-item limit"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "checkpoint-backed sizing harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_checkpoint_block_sizing_report() {
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        eprintln!(
                "skipping checkpoint sizing report: set REQ_RES_GEN2_CHECKPOINT_PATH or provide ~/gwe/oct-21-jams/0.chkjam or ~/gwe/oct-21-jams/1.chkjam (legacy fallback: 0-001.chkjam)"
            );
        return;
    };

    let sample_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_SAMPLE_BLOCKS", 256);
    let nominal_target_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_TARGET_BLOCKS", 128);
    let config = report_libp2p_config();
    let limits = runtime_limits_from_config(&config);
    let block_response_budget_bytes = block_batch_response_budget_bytes(limits);
    let mut checkpoint_app = match try_start_checkpoint_app(&chkjam_path).await {
        Ok(checkpoint_app) => checkpoint_app,
        Err(err) => {
            println!(
                "checkpoint range scry latency report skipped: checkpoint app did not boot: {err}"
            );
            return;
        }
    };
    let head_height = match discover_checkpoint_head_height(&mut checkpoint_app.app).await {
        Ok(head_height) => head_height,
        Err(err) => {
            println!(
                "checkpoint range scry latency report skipped: checkpoint head height did not resolve: {err}"
            );
            return;
        }
    };
    let sampled_height_start = head_height.saturating_sub(sample_blocks.saturating_sub(1) as u64);
    let sampled_height_end = head_height;

    println!("req-res gen2 checkpoint block sizing report");
    println!(
            "checkpoint={} head_height={} sample_range={}..={} sample_blocks={} nominal_target_blocks={} batch_max_bytes={} block_response_budget_bytes={} fallback_item_max_bytes={}",
            chkjam_path.display(),
            head_height,
            sampled_height_start,
            sampled_height_end,
            sample_blocks,
            nominal_target_blocks,
            config.gen2_batch_max_bytes(),
            block_response_budget_bytes,
            config.gen2_item_max_bytes(),
        );

    let mut heights_and_messages = Vec::new();
    for height in sampled_height_start..=sampled_height_end {
        let message = fetch_block_response_message(&mut checkpoint_app.app, height)
            .await
            .unwrap_or_else(|err| panic!("failed to fetch checkpoint block {height}: {err}"));
        heights_and_messages.push((height, message));
    }
    assert!(
        !heights_and_messages.is_empty(),
        "checkpoint sizing report requires at least one sampled block"
    );

    let mut response_message_bytes = heights_and_messages
        .iter()
        .map(|(_, message)| message.len())
        .collect::<Vec<_>>();
    response_message_bytes.sort_unstable();
    let total_response_bytes: usize = heights_and_messages
        .iter()
        .map(|(_, message)| message.len())
        .sum();
    let average_response_message_bytes =
        total_response_bytes as f64 / heights_and_messages.len() as f64;

    let mut largest_blocks = heights_and_messages
        .iter()
        .map(|(height, message)| CheckpointLargestBlockSample {
            height: *height,
            response_message_bytes: message.len(),
        })
        .collect::<Vec<_>>();
    largest_blocks.sort_by(|left, right| {
        right
            .response_message_bytes
            .cmp(&left.response_message_bytes)
            .then_with(|| left.height.cmp(&right.height))
    });
    largest_blocks.truncate(10);

    let mut fit_windows = Vec::new();
    if heights_and_messages.len() < nominal_target_blocks {
        let (fit_blocks, response_bytes) = fit_prefix_of_block_messages(
            &heights_and_messages, block_response_budget_bytes, nominal_target_blocks,
        );
        fit_windows.push((
            heights_and_messages.first().expect("sample should exist").0,
            heights_and_messages.last().expect("sample should exist").0,
            fit_blocks,
            response_bytes,
        ));
    } else {
        for window in heights_and_messages.windows(nominal_target_blocks) {
            let (fit_blocks, response_bytes) = fit_prefix_of_block_messages(
                window, block_response_budget_bytes, nominal_target_blocks,
            );
            fit_windows.push((
                window.first().expect("window should exist").0,
                window.last().expect("window should exist").0,
                fit_blocks,
                response_bytes,
            ));
        }
    }

    let mut fit_counts = fit_windows
        .iter()
        .map(|(_, _, fit_blocks, _)| *fit_blocks)
        .collect::<Vec<_>>();
    fit_counts.sort_unstable();
    let mut window_response_bytes = fit_windows
        .iter()
        .map(|(_, _, _, response_bytes)| *response_bytes)
        .collect::<Vec<_>>();
    window_response_bytes.sort_unstable();
    let total_window_response_bytes: usize = window_response_bytes.iter().sum();
    let average_window_response_bytes =
        total_window_response_bytes as f64 / window_response_bytes.len() as f64;
    let average_window_response_fill_ratio =
        average_window_response_bytes / block_response_budget_bytes as f64;
    let p50_window_response_fill_ratio =
        percentile(&window_response_bytes, 0.50) as f64 / block_response_budget_bytes as f64;
    let p95_window_response_fill_ratio =
        percentile(&window_response_bytes, 0.95) as f64 / block_response_budget_bytes as f64;
    let max_window_response_fill_ratio = window_response_bytes
        .last()
        .copied()
        .expect("window response byte sample should exist")
        as f64
        / block_response_budget_bytes as f64;
    let worst_window = fit_windows
        .iter()
        .min_by(|left, right| {
            left.2
                .cmp(&right.2)
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| {
                    if left.0 < right.0 {
                        CmpOrdering::Less
                    } else if left.0 > right.0 {
                        CmpOrdering::Greater
                    } else {
                        CmpOrdering::Equal
                    }
                })
        })
        .expect("fit windows should exist");

    println!(
        "response_message_bytes min={} p50={} p90={} p99={} max={} avg={:.1}",
        response_message_bytes[0],
        percentile(&response_message_bytes, 0.50),
        percentile(&response_message_bytes, 0.90),
        percentile(&response_message_bytes, 0.99),
        response_message_bytes
            .last()
            .copied()
            .expect("response byte sample should exist"),
        average_response_message_bytes,
    );
    println!(
        "contiguous_window_fit_blocks min={} p50={} max={} all_fit_nominal_target={}",
        fit_counts[0],
        percentile(&fit_counts, 0.50),
        fit_counts
            .last()
            .copied()
            .expect("fit count sample should exist"),
        fit_windows
            .iter()
            .all(|(_, _, fit_blocks, _)| *fit_blocks >= nominal_target_blocks),
    );
    println!(
        "window_response_bytes min={} p50={} p95={} max={} avg={:.1} fill_avg={:.3} fill_p95={:.3}",
        window_response_bytes[0],
        percentile(&window_response_bytes, 0.50),
        percentile(&window_response_bytes, 0.95),
        window_response_bytes
            .last()
            .copied()
            .expect("window response byte sample should exist"),
        average_window_response_bytes,
        average_window_response_fill_ratio,
        p95_window_response_fill_ratio,
    );
    println!(
        "worst_window start_height={} end_height={} fit_blocks={} response_bytes={}",
        worst_window.0, worst_window.1, worst_window.2, worst_window.3,
    );
    println!("{:<12} {:>18}", "height", "response_bytes");
    for sample in &largest_blocks {
        println!(
            "{:<12} {:>18}",
            sample.height, sample.response_message_bytes
        );
    }

    maybe_write_report_json(&CheckpointSizingReport {
        schema_version: "req_res_gen2_checkpoint_block_sizing_v1",
        scenario: "checkpoint-block-sizing",
        checkpoint_path: chkjam_path.display().to_string(),
        sampled_height_start,
        sampled_height_end,
        sampled_block_count: heights_and_messages.len(),
        nominal_target_blocks,
        batch_max_bytes: config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        fallback_item_max_bytes: config.gen2_item_max_bytes(),
        min_response_message_bytes: response_message_bytes[0],
        p50_response_message_bytes: percentile(&response_message_bytes, 0.50),
        p90_response_message_bytes: percentile(&response_message_bytes, 0.90),
        p99_response_message_bytes: percentile(&response_message_bytes, 0.99),
        max_response_message_bytes: response_message_bytes
            .last()
            .copied()
            .expect("response byte sample should exist"),
        average_response_message_bytes,
        min_blocks_fit_per_window: fit_counts[0],
        p50_blocks_fit_per_window: percentile(&fit_counts, 0.50),
        max_blocks_fit_per_window: fit_counts
            .last()
            .copied()
            .expect("fit count sample should exist"),
        min_window_response_bytes: window_response_bytes[0],
        p50_window_response_bytes: percentile(&window_response_bytes, 0.50),
        p95_window_response_bytes: percentile(&window_response_bytes, 0.95),
        max_window_response_bytes: window_response_bytes
            .last()
            .copied()
            .expect("window response byte sample should exist"),
        average_window_response_bytes,
        average_window_response_fill_ratio,
        p50_window_response_fill_ratio,
        p95_window_response_fill_ratio,
        max_window_response_fill_ratio,
        all_windows_fit_nominal_target: fit_windows
            .iter()
            .all(|(_, _, fit_blocks, _)| *fit_blocks >= nominal_target_blocks),
        worst_window_start_height: worst_window.0,
        worst_window_end_height: worst_window.1,
        worst_window_response_bytes: worst_window.3,
        largest_blocks,
    });
}

#[tokio::test]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_responder_payload_fit_report() {
    let limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    let workloads = vec![
        ResponderPayloadFitScenarioDef {
            label: "tx-flat-fit-64",
            request_mix: "raw-tx-only",
            payload_lens: vec![1024usize; 64],
            seed_hint_message_bytes: 1024usize,
            response_cap_bytes: limits.gen2_batch_max_bytes,
        },
        ResponderPayloadFitScenarioDef {
            label: "tx-ramp-tail-stop",
            request_mix: "raw-tx-only",
            payload_lens: (0..64)
                .map(|idx| 2048usize + idx * 384usize)
                .collect::<Vec<_>>(),
            seed_hint_message_bytes: 4096usize,
            response_cap_bytes: 96 * 1024,
        },
        ResponderPayloadFitScenarioDef {
            label: "tx-impossible-head",
            request_mix: "raw-tx-only",
            payload_lens: std::iter::once(limits.gen2_batch_max_bytes + 1024)
                .chain(std::iter::repeat_n(2048usize, 7))
                .collect::<Vec<_>>(),
            seed_hint_message_bytes: limits.gen2_batch_max_bytes + 1024,
            response_cap_bytes: 64 * 1024,
        },
        ResponderPayloadFitScenarioDef {
            label: "tx-poison-flat-96",
            request_mix: "raw-tx-only",
            payload_lens: vec![1024usize; 96],
            seed_hint_message_bytes: 24 * 1024,
            response_cap_bytes: 96 * 1024,
        },
        ResponderPayloadFitScenarioDef {
            label: "tx-poison-sawtooth-96",
            request_mix: "raw-tx-only",
            payload_lens: (0..96)
                .map(|idx| if idx % 12 == 0 { 8192usize } else { 1024usize })
                .collect::<Vec<_>>(),
            seed_hint_message_bytes: 24 * 1024,
            response_cap_bytes: 128 * 1024,
        },
    ];
    let mut samples = Vec::with_capacity(workloads.len());

    println!("req-res gen2 responder payload-fit report");
    println!(
        "config: batch_max_bytes={} item_max_bytes={}",
        limits.gen2_batch_max_bytes, limits.gen2_item_max_bytes,
    );
    println!(
        "{:<22} {:<20} {:>8} {:>10} {:>10} {:>10} {:>10} {:>12} {:<18}",
        "workload",
        "estimator",
        "items",
        "resp_cap",
        "seed",
        "fit",
        "loss",
        "resp_bytes",
        "stop_reason"
    );

    for workload in workloads {
        let items = build_tx_payload_fit_items(workload.payload_lens.len());
        let actual_message_bytes = workload
            .payload_lens
            .iter()
            .enumerate()
            .map(|(idx, payload_len)| {
                tx_result_message_bytes_for_seed(20_000 + idx as u64, *payload_len)
            })
            .collect::<Vec<_>>();
        let mut actual_message_bytes_sorted = actual_message_bytes.clone();
        actual_message_bytes_sorted.sort_unstable();
        let actual_fit = run_responder_payload_fit_estimator(
            ResponderHintEstimatorKind::OracleActual,
            &items,
            &workload.payload_lens,
            &actual_message_bytes,
            workload.response_cap_bytes,
            limits.gen2_item_max_bytes,
            workload.seed_hint_message_bytes,
        )
        .await;
        assert!(
            actual_fit.response_bytes <= workload.response_cap_bytes,
            "oracle responder payload-fit baseline should honor the configured cap"
        );

        let mut heuristic_samples = Vec::new();
        for estimator_kind in [
            ResponderHintEstimatorKind::ObservedMax,
            ResponderHintEstimatorKind::DecayingMax75,
            ResponderHintEstimatorKind::RecentMaxWindow8,
        ] {
            let run = run_responder_payload_fit_estimator(
                estimator_kind, &items, &workload.payload_lens, &actual_message_bytes,
                workload.response_cap_bytes, limits.gen2_item_max_bytes,
                workload.seed_hint_message_bytes,
            )
            .await;

            println!(
                "{:<22} {:<20} {:>8} {:>10} {:>10} {:>10} {:>10} {:>12} {:<18}",
                workload.label,
                run.estimator.label(),
                workload.payload_lens.len(),
                workload.response_cap_bytes,
                workload.seed_hint_message_bytes,
                run.result_items,
                actual_fit.result_items.saturating_sub(run.result_items),
                run.response_bytes,
                run.stop_reason
            );
            assert!(
                run.response_bytes <= workload.response_cap_bytes,
                "payload-fit report response should honor its configured cap"
            );

            heuristic_samples.push(ResponderPayloadFitHeuristicSample {
                estimator: run.estimator.label().to_string(),
                estimate_source: run.estimate_source,
                starting_estimate_message_bytes: run.starting_estimate_message_bytes,
                ending_estimate_message_bytes: run.ending_estimate_message_bytes,
                response_bytes: run.response_bytes,
                result_items: run.result_items,
                not_found_items: run.not_found_items,
                backpressure_items: run.backpressure_items,
                too_large_items: run.too_large_items,
                stop_reason: run.stop_reason,
                fit_loss_items: actual_fit.result_items.saturating_sub(run.result_items),
                fit_loss_response_bytes: actual_fit
                    .response_bytes
                    .saturating_sub(run.response_bytes),
                cap_headroom_bytes: workload
                    .response_cap_bytes
                    .saturating_sub(run.response_bytes),
                cap_utilization_ratio: run.response_bytes as f64
                    / workload.response_cap_bytes as f64,
            });
        }

        samples.push(ResponderPayloadFitScenarioSample {
            label: workload.label.to_string(),
            request_mix: workload.request_mix.to_string(),
            item_count: workload.payload_lens.len(),
            response_cap_bytes: workload.response_cap_bytes,
            seed_hint_message_bytes: workload.seed_hint_message_bytes,
            min_actual_message_bytes: actual_message_bytes_sorted[0],
            p50_actual_message_bytes: percentile(&actual_message_bytes_sorted, 0.50),
            max_actual_message_bytes: actual_message_bytes_sorted
                .last()
                .copied()
                .expect("actual message size sample should exist"),
            actual_fit_response_bytes: actual_fit.response_bytes,
            actual_fit_result_items: actual_fit.result_items,
            actual_fit_stop_reason: actual_fit.stop_reason,
            heuristic_samples,
        });
    }

    maybe_write_report_json(&ResponderPayloadFitReport {
        schema_version: "req_res_gen2_responder_payload_fit_v2",
        scenario: "responder-payload-fit",
        batch_max_bytes: limits.gen2_batch_max_bytes,
        item_max_bytes: limits.gen2_item_max_bytes,
        samples,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_requester_cost_report() {
    let config = report_libp2p_config();
    let block_response_budget_bytes =
        block_batch_response_budget_bytes(runtime_limits_from_config(&config));
    let workloads = [
        ("tx-response-32", "raw-tx-only", 32usize, 512usize),
        ("tx-response-128", "raw-tx-only", 128usize, 512usize),
        ("tx-response-128-large", "raw-tx-only", 128usize, 2048usize),
        // -- new: near-cap responses (128 x 6KB, around 768KB under 1MB cap) --
        (
            "tx-response-128-near-cap", "raw-tx-only", 128usize, 6144usize,
        ),
    ];
    let mut samples = Vec::with_capacity(workloads.len());

    println!("req-res gen2 requester cost report");
    println!(
        "config: batch_max_bytes={} block_response_budget_bytes={} item_max_bytes={}",
        config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        config.gen2_item_max_bytes(),
    );
    println!(
        "{:<24} {:>8} {:>14} {:>12} {:>12} {:>8} {:>8}",
        "workload", "items", "response_bytes", "total_ms", "per_item_us", "pokes", "followup"
    );

    for (label, request_mix, item_count, payload_len) in workloads {
        let transcript = DriverTranscript::default();
        transcript.record(
            "scenario",
            format!("requester-cost report {label} items={item_count} payload_len={payload_len}"),
        );
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let state_arc = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        let local_peer = PeerId::random();
        let request_id = fresh_outbound_request_id();
        let items = (0..item_count)
            .map(|idx| BatchRequestItem {
                item_id: idx as u32 + 1,
                message: ByteBuf::from(jam_raw_tx_request(40_000 + idx as u64)),
            })
            .collect::<Vec<_>>();
        state_arc.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: items.clone(),
                },
                0,
                false,
            ),
        );
        let results = (0..item_count)
            .map(|idx| {
                tx_result_outcome_for_seed(40_000 + idx as u64, payload_len)
                    .into_batch_result_item(idx as u32 + 1)
            })
            .collect::<Vec<_>>();
        let response_bytes =
            batch_result_encoded_bytes(&results).expect("requester-cost response should encode");
        let response = NockchainResponse::BatchResult { results };
        let scripted_traffic = build_scripted_traffic_cop(
            transcript.clone(),
            Vec::new(),
            std::iter::repeat_with(|| PokeResult::Ack)
                .take(item_count)
                .collect(),
        )
        .await;
        let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
        let mut equix_builder = equix::EquiXBuilder::new();

        let started = Instant::now();
        run_driver_with_timeout(
            &transcript,
            "requester-cost batch response processing",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(90 + item_count),
                request_response::Message::Response {
                    request_id,
                    response,
                },
                swarm_tx,
                &mut equix_builder,
                local_peer,
                scripted_traffic.traffic.clone(),
                metrics,
                Arc::clone(&state_arc),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("requester-cost response should process");
        let elapsed = started.elapsed();

        let followup_request_count = drain_expected_swarm_followups(&mut swarm_rx).await;
        assert_eq!(
            followup_request_count, 0,
            "synthetic raw-tx requester-cost workload should not emit follow-up requests"
        );

        let poke_count = scripted_traffic.poke_count.load(Ordering::SeqCst);
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_us = elapsed.as_micros() as f64 / item_count as f64;
        println!(
            "{:<24} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8}",
            label,
            item_count,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count
        );
        assert_eq!(
            poke_count, item_count,
            "requester-cost workload should route every successful item"
        );

        samples.push(RequesterCostSample {
            label: label.to_string(),
            request_mix: request_mix.to_string(),
            item_count,
            requested_item_count: None,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count,
            timed_out: false,
            timeout_seconds: None,
            window_start_height: None,
            window_end_height: None,
        });
    }

    // -- variable-size section: per-item payload varies from 128B to 4KB --
    {
        let label = "tx-response-128-variable";
        let item_count = 128usize;
        let transcript = DriverTranscript::default();
        transcript.record("scenario", format!("requester-cost variable-size {label}"));
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let state_arc = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        let local_peer = PeerId::random();
        let request_id = fresh_outbound_request_id();
        let items = (0..item_count)
            .map(|idx| BatchRequestItem {
                item_id: idx as u32 + 1,
                message: ByteBuf::from(jam_raw_tx_request(50_000 + idx as u64)),
            })
            .collect::<Vec<_>>();
        state_arc.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: items.clone(),
                },
                0,
                false,
            ),
        );
        // Variable payload: item N gets (128 + N * 30) bytes, range around 128B..3968B
        let results = (0..item_count)
            .map(|idx| {
                let payload_len = 128 + idx * 30;
                tx_result_outcome_for_seed(50_000 + idx as u64, payload_len)
                    .into_batch_result_item(idx as u32 + 1)
            })
            .collect::<Vec<_>>();
        let response_bytes =
            batch_result_encoded_bytes(&results).expect("variable-size response should encode");
        let response = NockchainResponse::BatchResult { results };
        let scripted_traffic = build_scripted_traffic_cop(
            transcript.clone(),
            Vec::new(),
            std::iter::repeat_with(|| PokeResult::Ack)
                .take(item_count)
                .collect(),
        )
        .await;
        let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
        let mut equix_builder = equix::EquiXBuilder::new();

        let started = Instant::now();
        run_driver_with_timeout(
            &transcript,
            "requester-cost variable-size",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(200),
                request_response::Message::Response {
                    request_id,
                    response,
                },
                swarm_tx,
                &mut equix_builder,
                local_peer,
                scripted_traffic.traffic.clone(),
                metrics,
                Arc::clone(&state_arc),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("variable-size response should process");
        let elapsed = started.elapsed();

        let followup_request_count = drain_expected_swarm_followups(&mut swarm_rx).await;
        assert_eq!(
            followup_request_count, 0,
            "synthetic variable-size requester-cost workload should not emit follow-up requests"
        );

        let poke_count = scripted_traffic.poke_count.load(Ordering::SeqCst);
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_us = elapsed.as_micros() as f64 / item_count as f64;
        println!(
            "{:<24} {:>8} {:>14} {:>12.3} {:>12.3} {:>8} {:>8}",
            label,
            item_count,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count,
        );
        assert_eq!(poke_count, item_count);

        samples.push(RequesterCostSample {
            label: label.to_string(),
            request_mix: String::from("raw-tx-variable-size"),
            item_count,
            requested_item_count: None,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            followup_request_count,
            timed_out: false,
            timeout_seconds: None,
            window_start_height: None,
            window_end_height: None,
        });
    }

    if let Some(chkjam_path) = checkpoint_path_for_report() {
        let nominal_target_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_TARGET_BLOCKS", 128);
        let sample_blocks = env_usize(
            "REQ_RES_GEN2_CHECKPOINT_SAMPLE_BLOCKS",
            nominal_target_blocks.max(256),
        );
        let window = load_heaviest_checkpoint_block_window(
            &chkjam_path, nominal_target_blocks, sample_blocks, block_response_budget_bytes,
        )
        .await;
        let timeout_seconds = env_usize("REQ_RES_GEN2_REQUESTER_TIMEOUT_SECS", 30)
            .try_into()
            .unwrap();
        let replay_counts = checkpoint_requester_replay_counts(window.len());
        for replay_blocks in replay_counts {
            samples.push(
                run_checkpoint_requester_cost_sample(
                    &config, &chkjam_path, nominal_target_blocks, &window, replay_blocks,
                    timeout_seconds,
                )
                .await,
            );
        }
    } else {
        println!(
                "requester-cost checkpoint block sample skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
            );
    }

    maybe_write_report_json(&RequesterCostReport {
        schema_version: "req_res_gen2_requester_cost_v1",
        scenario: "requester-cost",
        batch_max_bytes: config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        item_max_bytes: config.gen2_item_max_bytes(),
        samples,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "recovery-path harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_recovery_path_report() {
    let config = report_libp2p_config();
    let enqueue_samples = vec![
        run_recovery_enqueue_sample(
            "tx-burst-22",
            "raw-tx-live-burst",
            &[{
                (0..22)
                    .map(|idx| jam_raw_tx_request(90_000 + idx as u64))
                    .collect::<Vec<_>>()
            }],
        ),
        run_recovery_enqueue_sample(
            "tx-stagger-3-1-4-9",
            "raw-tx-live-stagger",
            &[
                (0..3)
                    .map(|idx| jam_raw_tx_request(91_000 + idx as u64))
                    .collect::<Vec<_>>(),
                vec![jam_raw_tx_request(91_100)],
                (0..4)
                    .map(|idx| jam_raw_tx_request(91_200 + idx as u64))
                    .collect::<Vec<_>>(),
                (0..9)
                    .map(|idx| jam_raw_tx_request(91_300 + idx as u64))
                    .collect::<Vec<_>>(),
            ],
        ),
        run_recovery_enqueue_sample(
            "tx-duplicate-storm-8x4",
            "raw-tx-duplicate-replay",
            &[{
                (0..4)
                    .flat_map(|_| (0..8).map(|idx| jam_raw_tx_request(92_000 + idx as u64)))
                    .collect::<Vec<_>>()
            }],
        ),
        run_recovery_enqueue_sample(
            "tx-tail-heavy-4-4-4-4-18",
            "raw-tx-live-tail-heavy",
            &[
                (0..4)
                    .map(|idx| jam_raw_tx_request(93_000 + idx as u64))
                    .collect::<Vec<_>>(),
                (0..4)
                    .map(|idx| jam_raw_tx_request(93_100 + idx as u64))
                    .collect::<Vec<_>>(),
                (0..4)
                    .map(|idx| jam_raw_tx_request(93_200 + idx as u64))
                    .collect::<Vec<_>>(),
                (0..4)
                    .map(|idx| jam_raw_tx_request(93_300 + idx as u64))
                    .collect::<Vec<_>>(),
                (0..18)
                    .map(|idx| jam_raw_tx_request(93_400 + idx as u64))
                    .collect::<Vec<_>>(),
            ],
        ),
    ];

    let mut response_samples = vec![
        run_tx_live_response_sample("tx-first-hit-22-live", 22, 1, 512).await,
        run_tx_driver_seen_replay_response_sample("tx-replay-after-driver-seen-22x4", 22, 4, 512)
            .await,
        run_block_all_miss_response_sample("block-batch-all-miss-8x8", 8, 8).await,
    ];
    if let Some(sample) = run_block_batch_miss_response_sample("block-batch-miss-8x8", 8).await {
        response_samples.push(sample);
    }
    let (tuning_config, tuning_samples) = run_recovery_tuning_sweep(&config);

    println!("req-res gen2 recovery-path report");
    println!();
    println!("enqueue-side recovery bursts");
    println!(
        "{:<24} {:<22} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "workload",
        "request_mix",
        "stages",
        "input",
        "unique",
        "dup",
        "out",
        "gen2",
        "gen1",
        "1item",
        "p50",
        "avg"
    );
    for sample in &enqueue_samples {
        println!(
            "{:<24} {:<22} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8.2}",
            sample.label,
            sample.request_mix,
            sample.stage_count,
            sample.input_request_count,
            sample.unique_request_count,
            sample.duplicate_requests,
            sample.outbound_request_count,
            sample.gen2_batch_request_count,
            sample.gen1_request_count,
            sample.single_item_batch_count,
            sample.p50_outbound_items,
            sample.average_outbound_items
        );
    }

    println!();
    println!("response-side recovery churn");
    println!(
        "{:<28} {:<24} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>12} {:>12}",
        "workload",
        "request_mix",
        "waves",
        "items",
        "useful",
        "dups",
        "effects",
        "followup",
        "resp_bytes",
        "per_item_us"
    );
    for sample in &response_samples {
        println!(
            "{:<28} {:<24} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>12} {:>12.2}",
            sample.label,
            sample.request_mix,
            sample.wave_count,
            sample.input_item_count,
            sample.useful_pokes,
            sample.duplicate_gates,
            sample.kernel_effect_count,
            sample.followup_request_count,
            sample.response_bytes,
            sample.per_item_us
        );
    }

    println!();
    println!("transport tuning sweep");
    println!(
        "{:<24} {:>6} {:>6} {:>8} {:>8} {:>8} {:>8} {:>10} {:>10}",
        "workload", "win", "items", "out", "p50", "p95", "avg", "p95_delay", "p95_fill"
    );
    for sample in tuning_samples.iter().filter(|sample| {
        matches!(
            sample.label.as_str(),
            "tx-burst-128" | "tx-drip-25ms-128" | "tx-drip-100ms-128" | "tx-large-burst-64"
        )
    }) {
        println!(
            "{:<24} {:>6} {:>6} {:>8} {:>8} {:>8} {:>8.2} {:>10.1} {:>9.3}%",
            sample.label,
            sample.coalesce_window_ms,
            sample.batch_max_items,
            sample.outbound_request_count,
            sample.p50_outbound_items,
            sample.p95_outbound_items,
            sample.average_outbound_items,
            sample.p95_added_delay_ms,
            sample.p95_response_fill_ratio * 100.0
        );
    }

    maybe_write_report_json(&RecoveryPathReport {
        schema_version: "req_res_gen2_recovery_path_v3",
        scenario: "recovery-path",
        enqueue_samples,
        response_samples,
        tuning_config,
        tuning_samples,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "checkpoint-only requester replay harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_checkpoint_requester_cost_report() {
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        println!(
                "checkpoint requester cost report skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
            );
        return;
    };

    let nominal_target_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_TARGET_BLOCKS", 128);
    let sample_blocks = env_usize(
        "REQ_RES_GEN2_CHECKPOINT_SAMPLE_BLOCKS",
        nominal_target_blocks.max(256),
    );
    let config = report_libp2p_config();
    let block_response_budget_bytes =
        block_batch_response_budget_bytes(runtime_limits_from_config(&config));
    let timeout_seconds = env_usize("REQ_RES_GEN2_REQUESTER_TIMEOUT_SECS", 30)
        .try_into()
        .unwrap();
    let window = load_heaviest_checkpoint_block_window(
        &chkjam_path, nominal_target_blocks, sample_blocks, block_response_budget_bytes,
    )
    .await;

    println!("req-res gen2 checkpoint requester cost report");
    println!(
            "config: batch_max_bytes={} block_response_budget_bytes={} item_max_bytes={} poke_timeout_s={timeout_seconds}",
            config.gen2_batch_max_bytes(),
            block_response_budget_bytes,
            config.gen2_item_max_bytes(),
        );
    println!(
        "{:<24} {:>8} {:>14} {:>12} {:>12} {:>8} {:>8}",
        "workload", "items", "response_bytes", "total_ms", "per_item_us", "pokes", "followup"
    );

    let mut samples = Vec::new();
    for replay_blocks in checkpoint_requester_replay_counts(window.len()) {
        samples.push(
            run_checkpoint_requester_cost_sample(
                &config, &chkjam_path, nominal_target_blocks, &window, replay_blocks,
                timeout_seconds,
            )
            .await,
        );
    }

    maybe_write_report_json(&RequesterCostReport {
        schema_version: "req_res_gen2_requester_cost_v1",
        scenario: "checkpoint-requester-cost",
        batch_max_bytes: config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        item_max_bytes: config.gen2_item_max_bytes(),
        samples,
    });
}

/// Phase 6 paired-rollout report. Replays the same heaviest-window the
/// requester-cost report uses, but as a single `BlockRangeWithTxs`
/// request whose response is one `HeardBlockRangeWithTxs` envelope
/// carrying every block in the window. Operators run this alongside
/// `req_res_gen2_checkpoint_requester_cost_report` and diff the JSON
/// outputs to see the prefetch-on vs prefetch-off cost curve on the
/// same kernel checkpoint. The sample shape is identical so a single
/// downstream analyzer can ingest both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_checkpoint_prefetch_cost_report() {
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        println!(
            "checkpoint prefetch cost report skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
        );
        return;
    };

    let nominal_target_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_TARGET_BLOCKS", 128);
    let sample_blocks = env_usize(
        "REQ_RES_GEN2_CHECKPOINT_SAMPLE_BLOCKS",
        nominal_target_blocks.max(256),
    );
    let config = report_libp2p_config();
    let block_response_budget_bytes =
        block_batch_response_budget_bytes(runtime_limits_from_config(&config));
    let timeout_seconds = env_usize("REQ_RES_GEN2_REQUESTER_TIMEOUT_SECS", 30)
        .try_into()
        .unwrap();
    let window = load_heaviest_checkpoint_block_window(
        &chkjam_path, nominal_target_blocks, sample_blocks, block_response_budget_bytes,
    )
    .await;

    println!("req-res gen2 checkpoint prefetch cost report");
    println!(
        "config: batch_max_bytes={} block_response_budget_bytes={} item_max_bytes={} poke_timeout_s={timeout_seconds}",
        config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        config.gen2_item_max_bytes(),
    );
    println!(
        "{:<32} {:>8} {:>14} {:>12} {:>12} {:>8} {:>8}",
        "workload", "items", "response_bytes", "total_ms", "per_item_us", "pokes", "followup"
    );

    // Range-based replay caps at u8::MAX for the BlockRangeWithTxs len field,
    // so trim the standard replay-counts curve to that bound. In practice the
    // 2 MiB block-batch response budget already keeps the realistic ceiling
    // around 16 blocks, so this filter is defensive.
    let mut samples = Vec::new();
    for replay_blocks in checkpoint_requester_replay_counts(window.len()) {
        if replay_blocks == 0 || replay_blocks > usize::from(u8::MAX) {
            continue;
        }
        samples.push(
            run_checkpoint_prefetch_cost_sample(
                &config, &chkjam_path, nominal_target_blocks, &window, replay_blocks,
                timeout_seconds,
            )
            .await,
        );
    }

    maybe_write_report_json(&RequesterCostReport {
        schema_version: "req_res_gen2_requester_cost_v1",
        scenario: "checkpoint-prefetch-cost",
        batch_max_bytes: config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        item_max_bytes: config.gen2_item_max_bytes(),
        samples,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_checkpoint_requester_profile_report() {
    // init tracing so %slog profiling markers get timestamped output
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        println!(
                "checkpoint requester profile skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
            );
        return;
    };

    let nominal_target_blocks = env_usize("REQ_RES_GEN2_CHECKPOINT_TARGET_BLOCKS", 128);
    let sample_blocks = env_usize(
        "REQ_RES_GEN2_CHECKPOINT_SAMPLE_BLOCKS",
        nominal_target_blocks.max(256),
    );
    let config = report_libp2p_config();
    let block_response_budget_bytes =
        block_batch_response_budget_bytes(runtime_limits_from_config(&config));
    let poke_timeout_seconds = env_usize(
        "REQ_RES_GEN2_PROFILE_TIMEOUT_SECS",
        env_usize("REQ_RES_GEN2_REQUESTER_TIMEOUT_SECS", 30),
    ) as u64;
    let block_window = load_heaviest_checkpoint_block_window(
        &chkjam_path, nominal_target_blocks, sample_blocks, block_response_budget_bytes,
    )
    .await;

    println!("req-res gen2 checkpoint requester profile report");
    println!(
            "config: batch_max_bytes={} block_response_budget_bytes={} item_max_bytes={} poke_timeout_s={poke_timeout_seconds}",
            config.gen2_batch_max_bytes(),
            block_response_budget_bytes,
            config.gen2_item_max_bytes(),
        );
    println!(
        "{:<24} {:>8} {:>14} {:>12} {:>10} {:>10} {:>12} {:<18}",
        "workload",
        "items",
        "response_bytes",
        "decode_ms",
        "clone_ms",
        "gate_us",
        "poke_ms",
        "outcome"
    );

    let mut samples = Vec::new();
    for replay_blocks in [1usize, 2]
        .into_iter()
        .filter(|count| *count <= block_window.len())
    {
        let sample = profile_checkpoint_requester_first_item(
            &chkjam_path,
            nominal_target_blocks,
            &block_window,
            replay_blocks,
            Duration::from_secs(poke_timeout_seconds),
        )
        .await;
        let outcome = if sample.first_item_poke_timed_out {
            format!("timeout={poke_timeout_seconds}s")
        } else if let Some(error) = sample.first_item_poke_error.as_deref() {
            error.to_string()
        } else {
            String::from("completed")
        };
        println!(
            "{:<24} {:>8} {:>14} {:>12.3} {:>10.3} {:>10.3} {:>12.3} {:<18}",
            sample.label,
            sample.replay_blocks,
            sample.response_bytes,
            sample.first_item_decode_ms,
            sample.first_item_clone_ms,
            sample.first_item_gate_us,
            sample.first_item_poke_ms,
            outcome
        );
        samples.push(sample);
    }

    maybe_write_report_json(&CheckpointRequesterProfileReport {
        schema_version: "req_res_gen2_checkpoint_requester_profile_v1",
        scenario: "checkpoint-requester-profile",
        batch_max_bytes: config.gen2_batch_max_bytes(),
        block_response_budget_bytes,
        item_max_bytes: config.gen2_item_max_bytes(),
        poke_timeout_seconds,
        samples,
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "checkpoint kernel latency harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_checkpoint_range_scry_latency_report() {
    let Some(chkjam_path) = checkpoint_path_for_report() else {
        println!(
            "checkpoint range scry latency report skipped: no checkpoint found via REQ_RES_GEN2_CHECKPOINT_PATH or ~/gwe/oct-21-jams/{{0,1}}.chkjam"
        );
        return;
    };

    let requested_lens = env_usize_list("REQ_RES_GEN2_RANGE_SCRY_LENS", &[1, 2, 8, 16]);
    let warm_runs = env_usize("REQ_RES_GEN2_RANGE_SCRY_WARM_RUNS", 5);
    let mut checkpoint_app = match try_start_checkpoint_app(&chkjam_path).await {
        Ok(checkpoint_app) => checkpoint_app,
        Err(err) => {
            println!(
                "checkpoint range scry latency report skipped: checkpoint app did not boot: {err}"
            );
            return;
        }
    };
    let head_height = match discover_checkpoint_head_height(&mut checkpoint_app.app).await {
        Ok(head_height) => head_height,
        Err(err) => {
            println!(
                "checkpoint range scry latency report skipped: checkpoint head height did not resolve: {err}"
            );
            return;
        }
    };
    let live_traffic = build_live_traffic_cop(checkpoint_app);
    let peer = PeerId::random();

    println!("req-res gen2 checkpoint range scry latency report");
    println!("checkpoint: {}", chkjam_path.display());
    println!("head_height: {head_height}");
    println!(
        "{:<18} {:>12} {:>12} {:>5} {:>12} {:>8} {:>12} {:>12} {:>12} {:>12} {:>8}",
        "workload",
        "start",
        "end",
        "len",
        "bytes",
        "some",
        "cold_ms",
        "warm_p50",
        "warm_p95",
        "warm_max",
        "runs"
    );

    let mut samples = Vec::new();
    for len in requested_lens {
        let Ok(len_u8) = u8::try_from(len) else {
            println!("skip len={len}: range request length must fit u8");
            continue;
        };
        if len_u8 == 0 {
            continue;
        }
        let len_u64 = u64::from(len_u8);
        if head_height + 1 < len_u64 {
            println!(
                "skip len={len}: checkpoint has only {} heights",
                head_height + 1
            );
            continue;
        }
        let start_height = head_height + 1 - len_u64;
        let end_height = head_height;
        let label = format!("range-len-{len_u8}");

        let cold = measure_checkpoint_range_scry(&live_traffic.traffic, peer, start_height, len_u8)
            .await
            .expect("cold checkpoint range scry should complete");
        assert!(
            cold.returned_some,
            "checkpoint range scry should return data for {start_height}..={end_height}"
        );

        let mut warm_ms = Vec::with_capacity(warm_runs);
        let mut result_jam_bytes = cold.result_jam_bytes;
        for _ in 0..warm_runs {
            let warm =
                measure_checkpoint_range_scry(&live_traffic.traffic, peer, start_height, len_u8)
                    .await
                    .expect("warm checkpoint range scry should complete");
            assert!(warm.returned_some);
            result_jam_bytes = warm.result_jam_bytes;
            warm_ms.push(warm.elapsed.as_secs_f64() * 1_000.0);
        }
        warm_ms.sort_by(|left, right| left.partial_cmp(right).unwrap_or(CmpOrdering::Equal));

        let cold_ms = cold.elapsed.as_secs_f64() * 1_000.0;
        let warm_p50_ms = percentile_f64_or_zero(&warm_ms, 0.50);
        let warm_p95_ms = percentile_f64_or_zero(&warm_ms, 0.95);
        let warm_max_ms = warm_ms.last().copied().unwrap_or(0.0);

        println!(
            "{:<18} {:>12} {:>12} {:>5} {:>12} {:>8} {:>12.3} {:>12.3} {:>12.3} {:>12.3} {:>8}",
            label,
            start_height,
            end_height,
            len_u8,
            result_jam_bytes,
            cold.returned_some,
            cold_ms,
            warm_p50_ms,
            warm_p95_ms,
            warm_max_ms,
            warm_runs
        );

        samples.push(CheckpointRangeScryLatencySample {
            label,
            start_height,
            end_height,
            len: len_u8,
            cold_ms,
            warm_runs,
            warm_p50_ms,
            warm_p95_ms,
            warm_max_ms,
            result_jam_bytes,
            returned_some: true,
        });
    }

    maybe_write_report_json(&CheckpointRangeScryLatencyReport {
        schema_version: "req_res_gen2_checkpoint_range_scry_latency_v1",
        scenario: "checkpoint-range-scry-latency",
        checkpoint_path: chkjam_path.display().to_string(),
        head_height,
        samples,
    });
}

struct RangeScryMeasurement {
    elapsed: Duration,
    result_jam_bytes: usize,
    returned_some: bool,
}

async fn measure_checkpoint_range_scry(
    traffic: &traffic_cop::TrafficCop,
    peer: PeerId,
    start_height: u64,
    len: u8,
) -> Result<RangeScryMeasurement, NockAppError> {
    let scry_slab =
        request_to_scry_slab(NockchainDataRequest::BlockRangeWithTxs { start_height, len })?;
    let started = Instant::now();
    let result = traffic.peek(Some(peer), scry_slab).await?;
    let elapsed = started.elapsed();
    let result_jam_bytes = result
        .as_ref()
        .map(|slab| slab.jam().as_ref().len())
        .unwrap_or(0);
    Ok(RangeScryMeasurement {
        elapsed,
        result_jam_bytes,
        returned_some: result.is_some(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_resident_set_report() {
    let workloads = [
        (
            "gen1-singleton-32-large",
            ReqResGeneration::Gen1,
            32usize,
            2048usize,
        ),
        (
            "gen2-batch-32-large",
            ReqResGeneration::Gen2,
            32usize,
            2048usize,
        ),
        (
            "gen1-singleton-128-large",
            ReqResGeneration::Gen1,
            128usize,
            2048usize,
        ),
        (
            "gen2-batch-128-large",
            ReqResGeneration::Gen2,
            128usize,
            2048usize,
        ),
        // -- new: near-cap payloads to stress RSS under heavy batches --
        (
            "gen2-batch-128-near-cap",
            ReqResGeneration::Gen2,
            128usize,
            6144usize,
        ),
    ];
    let mut samples = Vec::with_capacity(workloads.len());

    println!("req-res gen2 current-runtime resident-set report");
    println!(
        "config: batch_max_bytes={} item_max_bytes={}",
        LIBP2P_CONFIG.gen2_batch_max_bytes(),
        LIBP2P_CONFIG.gen2_item_max_bytes(),
    );
    println!(
        "{:<26} {:<8} {:>8} {:>10} {:>14} {:>12} {:>12} {:>10} {:>10} {:>10} {:>10}",
        "workload",
        "gen",
        "items",
        "payload",
        "response_bytes",
        "total_ms",
        "per_item_us",
        "rss_pre",
        "rss_post",
        "rss_peak",
        "rss_delta"
    );

    for (label, generation, item_count, payload_len) in workloads {
        let transcript = DriverTranscript::default();
        transcript.record(
                "scenario",
                format!(
                    "resident-set report {label} generation={generation:?} items={item_count} payload_len={payload_len}"
                ),
            );
        let (response_bytes, elapsed, poke_count, rss_before_kib, rss_after_kib, rss_peak_kib) =
            run_requester_response_workload(generation, item_count, payload_len, &transcript).await;
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_us = elapsed.as_micros() as f64 / item_count as f64;
        let rss_delta_kib = rss_peak_kib.saturating_sub(rss_before_kib);

        println!(
            "{:<26} {:<8} {:>8} {:>10} {:>14} {:>12.3} {:>12.3} {:>10} {:>10} {:>10} {:>10}",
            label,
            match generation {
                ReqResGeneration::Gen1 => "gen1",
                ReqResGeneration::Gen2 => "gen2",
            },
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_us,
            rss_before_kib,
            rss_after_kib,
            rss_peak_kib,
            rss_delta_kib
        );
        assert_eq!(
            poke_count, item_count,
            "resident-set workload should route every successful item"
        );

        samples.push(ResidentSetSample {
            label: label.to_string(),
            topology: String::from("single-process requester-path"),
            generation: match generation {
                ReqResGeneration::Gen1 => String::from("gen1"),
                ReqResGeneration::Gen2 => String::from("gen2"),
            },
            request_mix: String::from("raw-tx-only"),
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_us,
            poke_count,
            rss_before_kib,
            rss_after_kib,
            rss_peak_kib,
            rss_delta_kib,
        });
    }

    maybe_write_report_json(&ResidentSetReport {
        schema_version: "req_res_gen2_resident_set_v1",
        scenario: "resident-set",
        batch_max_bytes: LIBP2P_CONFIG.gen2_batch_max_bytes(),
        item_max_bytes: LIBP2P_CONFIG.gen2_item_max_bytes(),
        samples,
    });
}

#[test]
fn test_create_response_result_from_payload_matches_wrapped_scry_response() {
    let scry_res = scry_some_raw_tx(12_345, 64);
    let mut expected_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let expected = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut expected_slab,
    );
    let payload = unsafe { *scry_res.root() }
        .in_space(&space)
        .as_cell()
        .expect("scry result should be a cell")
        .tail()
        .as_cell()
        .expect("scry result tail should be a cell")
        .tail();
    let mut actual_slab = NounSlab::new();
    let actual =
        create_response_result_from_payload(payload, "heard-tx", &mut actual_slab).unwrap();

    let Right(Ok(NockchainResponse::Result {
        message: expected_message,
    })) = expected
    else {
        panic!("expected wrapped scry response to produce a result message");
    };
    let NockchainResponse::Result {
        message: actual_message,
    } = actual
    else {
        panic!("expected payload helper to produce a result message");
    };

    assert_eq!(actual_message, expected_message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_two_peer_latency_report() {
    let workloads = [
        (
            "gen1-singleton-32",
            ReqResGeneration::Gen1,
            32usize,
            512usize,
        ),
        ("gen2-batch-32", ReqResGeneration::Gen2, 32usize, 512usize),
        (
            "gen1-singleton-128-large",
            ReqResGeneration::Gen1,
            128usize,
            2048usize,
        ),
        (
            "gen2-batch-128-large",
            ReqResGeneration::Gen2,
            128usize,
            2048usize,
        ),
        // -- new: near-cap payloads (128 x 6KB, around 768KB response) --
        (
            "gen2-batch-128-near-cap",
            ReqResGeneration::Gen2,
            128usize,
            6144usize,
        ),
        // -- new: small batch for overhead measurement --
        ("gen1-singleton-4", ReqResGeneration::Gen1, 4usize, 512usize),
        ("gen2-batch-4", ReqResGeneration::Gen2, 4usize, 512usize),
    ];
    let mut samples = Vec::with_capacity(workloads.len());

    println!("req-res gen2 two-peer latency report");
    println!(
        "{:<24} {:<8} {:>8} {:>12} {:>14} {:>12} {:>12} {:<24}",
        "workload",
        "gen",
        "items",
        "payload",
        "response_bytes",
        "total_ms",
        "per_item_ms",
        "protocol"
    );

    for (label, generation, item_count, payload_len) in workloads {
        let transcript = DriverTranscript::default();
        transcript.record(
                "scenario",
                format!(
                    "two-peer latency report {label} generation={generation:?} items={item_count} payload_len={payload_len}"
                ),
            );
        let (response_bytes, elapsed, protocol, peek_count) =
            run_two_peer_driver_latency_workload(generation, item_count, payload_len, &transcript)
                .await;
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_ms = total_ms / item_count as f64;

        println!(
            "{:<24} {:<8} {:>8} {:>12} {:>14} {:>12.3} {:>12.3} {:<24}",
            label,
            match generation {
                ReqResGeneration::Gen1 => "gen1",
                ReqResGeneration::Gen2 => "gen2",
            },
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_ms,
            protocol
        );

        samples.push(TwoPeerLatencySample {
            label: label.to_string(),
            topology: String::from("two-peer driver-path"),
            generation: match generation {
                ReqResGeneration::Gen1 => String::from("gen1"),
                ReqResGeneration::Gen2 => String::from("gen2"),
            },
            request_mix: String::from("raw-tx-only"),
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_ms,
            protocol,
            peek_count,
        });
    }

    maybe_write_report_json(&TwoPeerLatencyReport {
        schema_version: "req_res_gen2_two_peer_latency_v1",
        scenario: "two-peer-latency",
        batch_max_bytes: LIBP2P_CONFIG.gen2_batch_max_bytes(),
        item_max_bytes: LIBP2P_CONFIG.gen2_item_max_bytes(),
        samples: samples.clone(),
    });

    // RTT projection: show what the numbers mean at real-world latencies.
    // gen1 pays one round-trip per item; gen2 pays one round-trip per batch.
    let rtts_ms = [10.0, 50.0, 100.0, 200.0];
    println!();
    println!("RTT projection (estimated wall-clock at real-world latencies)");
    println!(
        "{:<24} {:<8} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "workload", "gen", "items", "rtt_10ms", "rtt_50ms", "rtt_100ms", "rtt_200ms"
    );
    for sample in &samples {
        let item_count = sample.item_count;
        let processing_ms = sample.total_ms;
        let projected: Vec<String> = rtts_ms
            .iter()
            .map(|rtt| {
                let rtt_cost = match sample.generation.as_str() {
                    "gen1" => *rtt * item_count as f64, // one RTT per request
                    _ => *rtt,                          // one RTT per batch
                };
                format!("{:>10.0}ms", processing_ms + rtt_cost)
            })
            .collect();
        println!(
            "{:<24} {:<8} {:>8} {}",
            sample.label,
            sample.generation,
            item_count,
            projected.join(" ")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_kernel_sensitivity_report() {
    // Measures how the gen2 latency advantage degrades as kernel execution
    // cost increases.  The scripted kernel introduces an artificial delay
    // per peek (scry) call to simulate real kernel work.
    //
    // At zero kernel delay, gen2 is dominated by transport savings.
    // As kernel delay grows, per-item processing dominates and the
    // advantage converges toward "saves N-1 round-trips", which is
    // zero on loopback and significant on real networks.
    let item_count = 32usize;
    let payload_len = 512usize;
    let kernel_delays_ms = [0u64, 1, 5, 10];

    println!("req-res gen2 kernel sensitivity report (two-peer, {item_count} items, {payload_len}B payload)");
    println!(
        "{:<28} {:<8} {:>12} {:>14} {:>12} {:>12}",
        "workload", "gen", "kernel_ms", "total_ms", "per_item_ms", "vs_0ms"
    );

    let mut gen2_zero_ms = None;

    for delay_ms in kernel_delays_ms {
        let delay = Duration::from_millis(delay_ms);

        // -- gen2 batch --
        let transcript = DriverTranscript::default();
        transcript.record(
                "scenario",
                format!(
                    "kernel-sensitivity gen2-batch-{item_count} kernel_delay={delay_ms}ms payload_len={payload_len}"
                ),
            );
        let requester_config = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };
        let responder_config = requester_config.clone();
        let mut requester = build_test_swarm(requester_config);
        let mut responder = build_test_swarm(responder_config.clone());
        let requester_peer_id = *requester.local_peer_id();
        let responder_peer_id = *responder.local_peer_id();
        let _req_addr = wait_for_listen_addr(&mut requester, &transcript).await;
        let resp_addr = wait_for_listen_addr(&mut responder, &transcript).await;
        connect_test_swarms(&mut requester, &mut responder, &resp_addr, &transcript).await;

        let state_arc = Arc::new(Mutex::new(P2PState::new(
            Arc::new(
                NockchainP2PMetrics::register(gnort::global_metrics_registry())
                    .expect("Could not register metrics"),
            ),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        );
        let scripted_traffic = build_scripted_traffic_cop_with_delay(
            transcript.clone(),
            (0..item_count)
                .map(|idx| Some(scry_some_raw_tx(200_000 + idx as u64, payload_len)))
                .collect(),
            Vec::new(),
            delay,
        )
        .await;
        let limits = runtime_limits_from_config(&responder_config);
        let mut request_builder = equix::EquiXBuilder::new();
        let mut responder_equix = equix::EquiXBuilder::new();

        let items = (0..item_count)
            .map(|idx| BatchRequestItem {
                item_id: idx as u32 + 1,
                message: ByteBuf::from(jam_raw_tx_request(200_000 + idx as u64)),
            })
            .collect::<Vec<_>>();
        let request = NockchainRequest::new_batch_request(
            &mut request_builder, &requester_peer_id, &responder_peer_id, items,
        )
        .expect("batch request should build");
        requester
            .behaviour_mut()
            .request_response
            .send_request(&responder_peer_id, request);

        let started = Instant::now();
        let (peer, connection_id, message) =
            recv_request_event(&mut requester, &mut responder, &transcript).await;
        let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
        tokio::time::timeout(
            Duration::from_secs(30),
            handle_request_response(
                peer,
                connection_id,
                message,
                swarm_tx,
                &mut responder_equix,
                responder_peer_id,
                scripted_traffic.traffic.clone(),
                metrics,
                Arc::clone(&state_arc),
                limits,
                PeerExclusions::default(),
            ),
        )
        .await
        .expect("kernel sensitivity driver timeout")
        .expect("kernel sensitivity driver should process");

        let action = recv_swarm_action(&mut swarm_rx).await;
        match action {
            SwarmAction::SendResponse { channel, response } => {
                responder
                    .behaviour_mut()
                    .request_response
                    .send_response(channel, response)
                    .expect("response should send");
            }
            other => panic!("expected SendResponse, got {other:?}"),
        }
        let _ = recv_response_event(&mut requester, &mut responder, &transcript).await;
        let elapsed = started.elapsed();
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_ms = total_ms / item_count as f64;
        if delay_ms == 0 {
            gen2_zero_ms = Some(total_ms);
        }
        let vs_zero = gen2_zero_ms
            .map(|zero| format!("{:.1}x", total_ms / zero))
            .unwrap_or_else(|| String::from("1.0x"));

        println!(
            "{:<28} {:<8} {:>12} {:>14.3} {:>12.3} {:>12}",
            format!("gen2-batch-{item_count}-k{delay_ms}ms"),
            "gen2",
            delay_ms,
            total_ms,
            per_item_ms,
            vs_zero,
        );

        assert_eq!(
            scripted_traffic.peek_count.load(Ordering::SeqCst),
            item_count,
            "kernel sensitivity workload should peek once per item"
        );
    }

    println!();
    println!(
        "interpretation: as kernel_ms grows, gen2 total_ms approaches \
             (items x kernel_ms) + transport_overhead, and the transport \
             savings become a smaller fraction of wall-clock time."
    );
}

#[test]
fn test_response_fact_from_envelope_accepts_matching_metadata() {
    let scry_res = scry_some_raw_tx(42, 32);
    let mut response_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let response = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut response_slab,
    );
    let Right(Ok(NockchainResponse::Result { message })) = response else {
        panic!("expected tx result response");
    };
    let envelope = response_envelope_from_result_message(&message)
        .expect("tx response envelope should decode");

    let fact =
        response_fact_from_envelope(&envelope).expect("matching envelope metadata should decode");

    match fact {
        NockchainFact::HeardTx(tx_id, _) => {
            assert_eq!(envelope.tx_id.as_deref(), Some(tx_id.as_str()));
        }
        other => panic!("expected heard-tx fact, got {other:?}"),
    }
}

#[test]
fn test_response_fact_from_envelope_rejects_mismatched_metadata() {
    let scry_res = scry_some_raw_tx(7, 24);
    let mut response_slab = NounSlab::new();
    let space = scry_res.noun_space();
    let response = create_scry_response(
        unsafe { scry_res.root() },
        &space,
        "heard-tx",
        &mut response_slab,
    );
    let Right(Ok(NockchainResponse::Result { message })) = response else {
        panic!("expected tx result response");
    };
    let valid_envelope = response_envelope_from_result_message(&message)
        .expect("tx response envelope should decode");
    let invalid_envelope = ResponseEnvelope {
        tx_id: Some(String::from("wrong-tx-id")),
        ..valid_envelope
    };

    assert!(response_fact_from_envelope(&invalid_envelope).is_err());
}

#[test]
fn test_batch_request_item_ids_extracts_correlation_keys() {
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::with_attempt(
        peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 4,
                    message: ByteBuf::from(jam_block_by_height_request(1)),
                },
                BatchRequestItem {
                    item_id: 9,
                    message: ByteBuf::from(jam_block_by_height_request(2)),
                },
            ],
        },
        0,
        false,
    );

    let item_ids = batch_request_item_ids(Some(&request_context)).expect("expected batch item ids");
    assert_eq!(item_ids, BTreeSet::from([4, 9]));
}

#[test]
fn test_missing_batch_result_item_ids_returns_unseen_expected_items() {
    let expected = BTreeSet::from([1, 2, 3, 8]);
    let observed = BTreeSet::from([2, 8]);

    let missing = missing_batch_result_item_ids(&expected, &observed);

    assert_eq!(missing, BTreeSet::from([1, 3]));
}

#[tokio::test]
async fn test_execute_batch_request_items_preserves_tail_contract_on_overflow() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(vec![0x03]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);
    let first_envelope = ResponseEnvelope::heard_tx(String::from("head"), vec![0xAA; 32]);
    let second_envelope = ResponseEnvelope::heard_tx(String::from("overflow"), vec![0xBB; 256]);
    let first_result = BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(first_envelope.clone()),
    };
    let second_result = BatchResultItem {
        item_id: 2,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(second_envelope.clone()),
    };
    let full_with_second = batch_result_encoded_bytes(&[
        first_result.clone(),
        second_result.clone(),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("expanded response should encode");
    let limit = full_with_second.saturating_sub(1);
    let expected = vec![
        first_result.clone(),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ];
    assert!(
        batch_result_encoded_bytes(std::slice::from_ref(&second_result))
            .expect("single result should encode")
            <= limit
    );
    assert!(
        batch_result_encoded_bytes(&expected).expect("expected response should encode") <= limit
    );

    let results = execute_batch_request_items(
        &items,
        limit,
        |_| async { Ok(None) },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let item_id = item.item_id;
            let first_envelope = first_envelope.clone();
            let second_envelope = second_envelope.clone();
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    1 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
                        response: NockchainResponse::Result {
                            message: first_envelope.message.clone(),
                        },
                        envelope: first_envelope,
                    }),
                    2 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
                        response: NockchainResponse::Result {
                            message: second_envelope.message.clone(),
                        },
                        envelope: second_envelope,
                    }),
                    3 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("batch execution should preserve tail contract");

    assert_eq!(*observed_order.lock().unwrap(), vec![1, 2]);
    assert_eq!(results, expected);
    assert!(batch_result_encoded_bytes(&results).expect("bounded response should encode") <= limit);
}

#[tokio::test]
async fn test_execute_batch_request_items_marks_oversize_current_item_too_large() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);
    let large_envelope = ResponseEnvelope::heard_tx(String::from("too-large"), vec![0xCC; 512]);
    let expected = vec![
        batch_error_result(1, BatchErrorClass::TooLarge),
        batch_error_result(2, BatchErrorClass::Backpressure),
    ];
    let limit = batch_result_encoded_bytes(&[
        batch_error_result(1, BatchErrorClass::Backpressure),
        batch_error_result(2, BatchErrorClass::Backpressure),
    ])
    .expect("backpressure-only response should encode");
    let actual_result = BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(large_envelope.clone()),
    };
    assert!(
        batch_result_encoded_bytes(&expected).expect("expected response should encode") <= limit
    );
    assert!(
        batch_result_encoded_bytes(std::slice::from_ref(&actual_result))
            .expect("actual result should encode")
            > limit
    );

    let results = execute_batch_request_items(
        &items,
        limit,
        |_| async { Ok(None) },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let item_id = item.item_id;
            let large_envelope = large_envelope.clone();
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    1 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
                        response: NockchainResponse::Result {
                            message: large_envelope.message.clone(),
                        },
                        envelope: large_envelope,
                    }),
                    2 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("batch execution should downgrade oversize current item");

    assert_eq!(*observed_order.lock().unwrap(), vec![1]);
    assert_eq!(results, expected);
    assert!(batch_result_encoded_bytes(&results).expect("bounded response should encode") <= limit);
}

#[tokio::test]
async fn test_execute_batch_request_items_stops_before_executing_estimated_tail() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(vec![0x03]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);
    let head_envelope = ResponseEnvelope::heard_tx(String::from("head"), vec![0xAA; 24]);
    let head_result = BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(head_envelope.clone()),
    };
    let estimated_tail = BatchItemResponseEstimate {
        request_kind: "raw-tx-by-id",
        envelope: BatchItemResponseEnvelopeEstimate::HeardTx {
            tx_id: String::from("tail"),
        },
        message_bytes: 64,
        source: "observed_max",
    };
    let limit = batch_result_encoded_bytes(&[
        head_result.clone(),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("bounded fallback result should encode");

    let projected_too_large = batch_result_encoded_bytes(&[
        head_result.clone(),
        estimated_result_item(2, &estimated_tail),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("projected tail estimate should encode");
    assert!(projected_too_large > limit);

    let results = execute_batch_request_items(
        &items,
        limit,
        move |item| {
            let estimate = estimated_tail.clone();
            let item_id = item.item_id;
            async move {
                if item_id == 1 {
                    Ok(None)
                } else {
                    Ok(Some(estimate))
                }
            }
        },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let head_envelope = head_envelope.clone();
            let item_id = item.item_id;
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    1 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
                        response: NockchainResponse::Result {
                            message: head_envelope.message.clone(),
                        },
                        envelope: head_envelope,
                    }),
                    2 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    3 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("estimated stop should succeed");

    assert_eq!(*observed_order.lock().unwrap(), vec![1]);
    assert_eq!(
        results,
        vec![
            head_result,
            batch_error_result(2, BatchErrorClass::Backpressure),
            batch_error_result(3, BatchErrorClass::Backpressure),
        ]
    );
}

#[tokio::test]
async fn test_execute_batch_request_items_marks_estimated_impossible_item_too_large() {
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0x01]),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(vec![0x02]),
        },
    ];
    let observed_order = Arc::new(StdMutex::new(Vec::new()));
    let observed_order_clone = Arc::clone(&observed_order);
    let estimate = BatchItemResponseEstimate {
        request_kind: "block-by-height",
        envelope: BatchItemResponseEnvelopeEstimate::HeardBlock {
            block_id_bytes_upper_bound: TIP5_BASE58_MAX_CHARS,
        },
        message_bytes: 96,
        source: "configured_fallback",
    };
    let limit = batch_result_encoded_bytes(&[
        batch_error_result(1, BatchErrorClass::Backpressure),
        batch_error_result(2, BatchErrorClass::Backpressure),
    ])
    .expect("backpressure-only result should encode");
    let projected_single = batch_result_encoded_bytes(&[estimated_result_item(1, &estimate)])
        .expect("projected single item should encode");
    assert!(projected_single > limit);

    let results = execute_batch_request_items(
        &items,
        limit,
        move |_| {
            let estimate = estimate.clone();
            async move { Ok(Some(estimate)) }
        },
        move |item| {
            let observed_order = Arc::clone(&observed_order_clone);
            let item_id = item.item_id;
            async move {
                observed_order.lock().unwrap().push(item_id);
                match item_id {
                    1 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    2 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::NotFound),
                    _ => unreachable!("unexpected test item"),
                }
            }
        },
    )
    .await
    .expect("impossible estimate should degrade without execution");

    assert_eq!(*observed_order.lock().unwrap(), Vec::<u32>::new());
    assert_eq!(
        results,
        vec![
            batch_error_result(1, BatchErrorClass::TooLarge),
            batch_error_result(2, BatchErrorClass::Backpressure),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_responder_fit_stops_before_executing_tail_item() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        "driver-path responder-fit heuristic stops before executing an estimated tail item",
    );

    let small_tx = scry_some_raw_tx(500, 12);
    let expected_head = tx_result_item_from_scry(1, &small_tx);
    let estimated_message_bytes = expected_head
        .envelope
        .as_ref()
        .expect("expected envelope")
        .message
        .len();
    let estimate = BatchItemResponseEstimate {
        request_kind: "raw-tx-by-id",
        envelope: BatchItemResponseEnvelopeEstimate::HeardTx {
            tx_id: String::from("tx-estimate"),
        },
        message_bytes: estimated_message_bytes,
        source: "configured_fallback",
    };
    let first_projection = batch_result_encoded_bytes(&[
        estimated_result_item(1, &estimate),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("first projected batch should encode");
    let second_projection = batch_result_encoded_bytes(&[
        expected_head.clone(),
        estimated_result_item(2, &estimate),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("second projected batch should encode");
    let first_actual = batch_result_encoded_bytes(&[
        expected_head.clone(),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("first actual batch should encode");
    let limit = first_projection.max(first_actual);
    assert!(
        second_projection > limit,
        "test budget must admit the first item but reject the second projection"
    );

    let item1_message = jam_raw_tx_request(100);
    let item2_message = jam_raw_tx_request(200);
    let item3_message = jam_raw_tx_request(300);
    let request_item_max_bytes = item1_message
        .len()
        .max(item2_message.len())
        .max(item3_message.len());

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        gen2_batch_max_bytes: limit,
        gen2_item_max_bytes: request_item_max_bytes,
        ..LibP2PConfig::default()
    };
    let limits = runtime_limits_from_config(&responder_config);

    let mut requester = build_test_swarm(requester_config.clone());
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let batch_items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(item1_message.clone()),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(item2_message.clone()),
        },
        BatchRequestItem {
            item_id: 3,
            message: ByteBuf::from(item3_message.clone()),
        },
    ];
    let request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        batch_items,
    )
    .expect("batch request should build");

    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, request);

    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    let expected_results = vec![
        expected_head.clone(),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ];
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Could not register metrics"),
        ),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state = state_arc.lock().await;
        let data_request = decode_request_item_message(&item1_message)
            .expect("request should decode for hint pre-seed");
        state.record_response_message_hint(&data_request, estimated_message_bytes);
    }

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), vec![Some(small_tx.clone())], Vec::new())
            .await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should handle inbound batch request",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx,
            &mut equix_builder,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics,
            Arc::clone(&state_arc),
            ReqResRuntimeLimits {
                gen2_batch_max_bytes: limit,
                ..limits
            },
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should handle inbound batch request");

    let action = recv_swarm_action(&mut swarm_rx).await;
    let response = match action {
        SwarmAction::SendResponse { channel, response } => {
            transcript.record("driver", "emitted SendResponse for bounded batch result");
            responder
                .behaviour_mut()
                .request_response
                .send_response(channel, response.clone())
                .expect("response should send");
            response
        }
        other => panic!("expected SendResponse, got {other:?}"),
    };

    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 1);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: expected_results.clone(),
        }
    );

    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(
        requester_response,
        NockchainResponse::BatchResult {
            results: expected_results,
        }
    );

    let rendered = transcript.render();
    assert!(rendered.contains("driver-path responder-fit heuristic"));
    assert!(rendered.contains("peek #1 -> some"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_wholesale_inbound_backpressure_reject_recovers_on_same_connection() {
    let transcript = DriverTranscript::default();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        gen2_max_inflight_per_peer: 1,
        ..LibP2PConfig::default()
    };
    transcript.record(
        "scenario",
        format!(
            "driver-path wholesale inflight_backpressure reject expected_common_protocol={:?}",
            first_common_outbound_protocol(&requester_config, &responder_config)
        ),
    );

    let limits = runtime_limits_from_config(&responder_config);
    let mut requester = build_test_swarm(requester_config.clone());
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state = state_arc.lock().await;
        assert!(
            state.try_admit_inbound_req_res(
                requester_peer_id,
                responder_config.gen2_max_inflight_per_peer(),
            ),
            "test setup should saturate responder inflight gate"
        );
    }

    let followup_scry = scry_some_raw_tx(90_002, 16);
    let expected_followup = tx_result_item_from_scry(2, &followup_scry);
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), vec![Some(followup_scry)], Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    let rejected_request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(jam_raw_tx_request(90_001)),
        }],
    )
    .expect("rejected batch request should build");
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, rejected_request);

    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    run_driver_with_timeout(
        &transcript,
        "driver should reject inbound batch before execution when inflight gate is full",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx.clone(),
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should reject saturated inbound batch");

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) => {}
        Ok(Some(other)) => {
            panic!("expected no swarm action for wholesale reject, got {other:?}")
        }
        Ok(None) => panic!("swarm action channel closed unexpectedly"),
    }

    assert_eq!(metrics.gen2_batch_rejected_backpressure.fetch_add(0), 1);
    assert_eq!(metrics.requests_dropped.fetch_add(0), 1);
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 0);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);

    let rejection_failure =
        recv_outbound_failure_event(&mut requester, &mut responder, &transcript).await;
    assert!(
            matches!(rejection_failure, request_response::OutboundFailure::Io(_)),
            "wholesale inflight reject should surface as an EOF-style requester IO failure on the live req-res path, got {rejection_failure:?}"
        );

    {
        let mut state = state_arc.lock().await;
        state.release_inbound_req_res(requester_peer_id);
        assert_eq!(
            state.req_res_inflight_for_peer(requester_peer_id),
            0,
            "releasing the pre-filled inbound slot should restore capacity",
        );
    }

    let followup_request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(jam_raw_tx_request(90_002)),
        }],
    )
    .expect("follow-up batch request should build");
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, followup_request);

    let (followup_peer, followup_connection_id, followup_message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(followup_peer, requester_peer_id);
    assert_eq!(
        followup_connection_id, connection_id,
        "follow-up traffic should stay on the original libp2p connection after the timeout"
    );

    run_driver_with_timeout(
        &transcript,
        "driver should accept follow-up batch once inbound pressure clears",
        handle_request_response(
            followup_peer,
            followup_connection_id,
            followup_message,
            swarm_tx.clone(),
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process follow-up batch after pressure clears");

    let response = match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse { channel, response } => {
            transcript.record(
                "driver", "emitted SendResponse for follow-up batch after backpressure cleared",
            );
            responder
                .behaviour_mut()
                .request_response
                .send_response(channel, response.clone())
                .expect("follow-up response should send");
            response
        }
        other => panic!("expected SendResponse for follow-up batch, got {other:?}"),
    };
    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: vec![expected_followup.clone()],
        }
    );

    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(
        requester_response,
        NockchainResponse::BatchResult {
            results: vec![expected_followup],
        }
    );
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 1);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert_eq!(metrics.gen2_batch_rejected_backpressure.fetch_add(0), 1);
    assert_eq!(metrics.requests_dropped.fetch_add(0), 1);
    assert_eq!(
        state_arc
            .lock()
            .await
            .req_res_inflight_for_peer(requester_peer_id),
        0,
        "follow-up success should release the admitted inbound slot",
    );

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("UnexpectedEof"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_rejects_replayed_inbound_batch_before_execution() {
    let transcript = DriverTranscript::default();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let limits = runtime_limits_from_config(&responder_config);
    let mut requester = build_test_swarm(requester_config);
    let mut responder = build_test_swarm(responder_config);
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let raw_tx_scry = scry_some_raw_tx(91_001, 16);
    let expected = tx_result_item_from_scry(1, &raw_tx_scry);
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), vec![Some(raw_tx_scry)], Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    let request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(jam_raw_tx_request(91_001)),
        }],
    )
    .expect("batch request should build");

    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, request.clone());
    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    run_driver_with_timeout(
        &transcript,
        "driver should accept first inbound batch",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx.clone(),
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("first request should execute");

    let response = match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse { channel, response } => {
            responder
                .behaviour_mut()
                .request_response
                .send_response(channel, response.clone())
                .expect("first response should send");
            response
        }
        other => panic!("expected SendResponse for first request, got {other:?}"),
    };
    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: vec![expected],
        }
    );
    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(requester_response, response);

    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, request);
    let (replay_peer, replay_connection_id, replay_message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(replay_peer, requester_peer_id);
    assert_eq!(
        replay_connection_id, connection_id,
        "replayed request should arrive on the established connection"
    );

    run_driver_with_timeout(
        &transcript,
        "driver should reject replayed inbound batch before execution",
        handle_request_response(
            replay_peer,
            replay_connection_id,
            replay_message,
            swarm_tx.clone(),
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("replayed request should be handled as a local reject");

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) => {}
        Ok(Some(other)) => panic!("expected no response for replayed request, got {other:?}"),
        Ok(None) => panic!("swarm action channel closed unexpectedly"),
    }
    assert_eq!(metrics.request_replay_rejected.fetch_add(0), 1);
    assert_eq!(metrics.requests_dropped.fetch_add(0), 1);
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_releases_inbound_slot_after_admitted_request_error() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path admitted inbound decode error releases inflight slot",
    );

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: 1,
        gen2_max_inflight_per_peer: 1,
        ..LibP2PConfig::default()
    };
    let limits = runtime_limits_from_config(&responder_config);
    let mut requester = build_test_swarm(requester_config);
    let mut responder = build_test_swarm(responder_config.clone());
    let requester_peer_id = *requester.local_peer_id();
    let responder_peer_id = *responder.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_test_swarms(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut responder_equix = equix::EquiXBuilder::new();

    let mut malformed_slab = NounSlab::new();
    malformed_slab.set_root(D(0));
    let malformed_request = NockchainRequest::new_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        &malformed_slab,
    );
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, malformed_request);

    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(peer, requester_peer_id);

    let result = run_driver_with_timeout(
        &transcript,
        "driver should release inbound slot after admitted decode error",
        handle_request_response(
            peer,
            connection_id,
            message,
            swarm_tx,
            &mut responder_equix,
            responder_peer_id,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            limits,
            PeerExclusions::default(),
        ),
    )
    .await;
    assert!(
        result.is_err(),
        "malformed admitted request should return a decode error"
    );

    assert_eq!(
        state_arc
            .lock()
            .await
            .req_res_inflight_for_peer(requester_peer_id),
        0,
        "decode error after admission must release the inbound slot"
    );
    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 0);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) | Ok(None) => {}
        Ok(Some(other)) => panic!("expected no swarm action for malformed request, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_retry_requeues_only_retryable_and_missing_batch_items() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path batch response retries only retryable and missing items",
    );

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_raw_tx_request(100)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_raw_tx_request(200)),
                },
                BatchRequestItem {
                    item_id: 3,
                    message: ByteBuf::from(jam_raw_tx_request(300)),
                },
            ],
        },
        0,
        false,
    );
    state_arc
        .lock()
        .await
        .record_outbound_request(request_id, request_context.clone());

    let good_tx = scry_some_raw_tx(100, 16);
    let response = NockchainResponse::BatchResult {
        results: vec![
            tx_result_item_from_scry(1, &good_tx),
            batch_error_result(2, BatchErrorClass::Backpressure),
        ],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), vec![PokeResult::Ack]).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should process batch response",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(1),
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process batch response");

    let action = recv_swarm_action(&mut swarm_rx).await;
    match action {
        SwarmAction::RetryRequests { requests, delay } => {
            transcript.record(
                "driver",
                format!(
                    "scheduled {} retry request(s) after {:?}",
                    requests.len(),
                    delay
                ),
            );
            let min_delay = Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS);
            let max_delay =
                Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS + GEN2_RETRY_MAX_JITTER_MS);
            assert!(
                delay >= min_delay && delay <= max_delay,
                "retry delay {delay:?} should stay within first-attempt jitter window"
            );
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].retry_count, 1);
            match &requests[0].request {
                NockchainRequest::BatchRequest { items, .. } => {
                    let retry_ids = items.iter().map(|item| item.item_id).collect::<Vec<_>>();
                    assert_eq!(retry_ids, vec![2, 3]);
                }
                other => panic!("expected batch retry request, got {other:?}"),
            }
        }
        other => panic!("expected RetryRequests, got {other:?}"),
    }

    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 0);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 1);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "completed batch response should clear retained outbound context"
    );

    let rendered = transcript.render();
    assert!(rendered.contains("retryable and missing items"));
    assert!(rendered.contains("poke #1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_bundle_too_large_error_queues_classic_fallback() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path bundle TooLarge response downgrades to classic block fetch",
    );

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: crate::messages::block_with_txs_by_height_request_message(123)
                    .expect("bundle request message should build"),
            }],
        },
        0,
        false,
    );
    state_arc
        .lock()
        .await
        .record_outbound_request(request_id, request_context);

    let response = NockchainResponse::BatchResult {
        results: vec![batch_error_result(7, BatchErrorClass::TooLarge)],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should downgrade oversized bundle response",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(1),
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process oversized bundle response");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, peer);
            let decoded = crate::messages::decode_request_item_message(&request_message)
                .expect("classic fallback request should decode");
            match decoded {
                NockchainDataRequest::BlockByHeight(height) => assert_eq!(height, 123),
                other => panic!("expected classic BlockByHeight fallback, got {other:?}"),
            }
        }
        other => panic!("expected QueueKernelRequest fallback, got {other:?}"),
    }

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) | Ok(None) => {}
        Ok(Some(other)) => panic!("expected no retry action for TooLarge bundle, got {other:?}"),
    }

    let state_guard = state_arc.lock().await;
    assert!(state_guard.is_peer_non_bundle_capable(&peer));
    assert!(
        state_guard.outbound_request_context(request_id).is_none(),
        "completed batch response should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_invalid_range_response_queues_classic_fallback() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path invalid range response downgrades to classic block fetch",
    );

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: block_range_with_txs_request_message(50, 2)
                    .expect("range request message should build"),
            }],
        },
        0,
        false,
    );
    state_arc
        .lock()
        .await
        .record_outbound_request(request_id, request_context);

    let response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_block_range_with_txs(vec![
                bundled_block_for_height(50, &[]),
                bundled_block_for_height(52, &[]),
            ])),
        }],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should downgrade invalid range response",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(1),
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process invalid range response");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, peer);
            let decoded = crate::messages::decode_request_item_message(&request_message)
                .expect("classic fallback request should decode");
            match decoded {
                NockchainDataRequest::BlockByHeight(height) => assert_eq!(height, 50),
                other => panic!("expected classic BlockByHeight fallback, got {other:?}"),
            }
        }
        other => panic!("expected QueueKernelRequest fallback, got {other:?}"),
    }

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) | Ok(None) => {}
        Ok(Some(other)) => panic!("expected no retry action for invalid range, got {other:?}"),
    }

    let state_guard = state_arc.lock().await;
    assert!(state_guard.is_peer_non_range_capable(&peer));
    assert!(
        state_guard.outbound_request_context(request_id).is_none(),
        "completed batch response should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_unsupported_protocol_fallback_replays_batch_items_in_order() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        Arc::new(PeerStatsRegistry::default()),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(11)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(22)),
                },
                BatchRequestItem {
                    item_id: 3,
                    message: ByteBuf::from(jam_block_by_height_request(33)),
                },
            ],
        },
        0,
        false,
    );
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        Arc::clone(&state_arc),
        metrics.clone(),
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        peer,
        request_id,
        request_response::OutboundFailure::UnsupportedProtocols,
    )
    .await;

    let mut fallback_heights = Vec::new();
    for expected_height in [11, 22, 33] {
        match recv_swarm_action(&mut swarm_rx).await {
            SwarmAction::SendRequest {
                peer_id,
                request,
                request_context,
            } => {
                assert_eq!(peer_id, peer);
                let fallback_context =
                    request_context.expect("fallback queue should retain request context");
                assert_eq!(fallback_context.peer_id, peer);
                assert_eq!(fallback_context.generation, ReqResGeneration::Gen1);
                assert_eq!(fallback_context.retry_count, 1);
                assert!(fallback_context.fallback_attempted);

                request
                    .verify_pow(&mut equix_builder, &peer, &local_peer)
                    .expect("fallback request PoW should verify");

                let message = match request {
                    NockchainRequest::Request { message, .. } => message,
                    other => panic!("expected fallback singleton request, got {other:?}"),
                };
                let data_request =
                    decode_request_item_message(&message).expect("fallback request should decode");
                let NockchainDataRequest::BlockByHeight(height) = data_request else {
                    panic!("expected fallback block-by-height request");
                };
                assert_eq!(height, expected_height);
                fallback_heights.push(height);
            }
            other => panic!("expected SendRequest fallback action, got {other:?}"),
        }
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "unsupported-protocol fallback should only queue one singleton request per batch item"
    );
    assert_eq!(
        fallback_heights,
        vec![11, 22, 33],
        "live fallback queue must preserve original batch order"
    );
    assert_eq!(metrics.req_res_fallback_total.fetch_add(0), 3);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "unsupported-protocol failure should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_unsupported_protocol_fallback_replays_singleton_request() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        Arc::new(PeerStatsRegistry::default()),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 1,
            message: ByteBuf::from(jam_raw_tx_request(44)),
        },
        0,
        false,
    );
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        Arc::clone(&state_arc),
        metrics.clone(),
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        peer,
        request_id,
        request_response::OutboundFailure::UnsupportedProtocols,
    )
    .await;

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendRequest {
            peer_id,
            request,
            request_context,
        } => {
            assert_eq!(peer_id, peer);
            let fallback_context =
                request_context.expect("fallback queue should retain request context");
            assert_eq!(fallback_context.peer_id, peer);
            assert_eq!(fallback_context.generation, ReqResGeneration::Gen1);
            assert_eq!(fallback_context.retry_count, 1);
            assert!(fallback_context.fallback_attempted);

            request
                .verify_pow(&mut equix_builder, &peer, &local_peer)
                .expect("fallback request PoW should verify");

            let message = match request {
                NockchainRequest::Request { message, .. } => message,
                other => panic!("expected fallback singleton request, got {other:?}"),
            };
            let data_request =
                decode_request_item_message(&message).expect("fallback request should decode");
            let NockchainDataRequest::RawTransactionById(tx_id, _) = data_request else {
                panic!("expected fallback raw transaction request");
            };
            assert!(!tx_id.is_empty());
        }
        other => panic!("expected SendRequest fallback action, got {other:?}"),
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "singleton unsupported-protocol fallback should only queue one request"
    );
    assert_eq!(metrics.req_res_fallback_total.fetch_add(0), 1);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "unsupported-protocol failure should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_timeout_retries_batch_request_and_clears_context() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        Arc::new(PeerStatsRegistry::default()),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(11)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(22)),
                },
                BatchRequestItem {
                    item_id: 3,
                    message: ByteBuf::from(jam_block_by_height_request(33)),
                },
            ],
        },
        0,
        false,
    );
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        Arc::clone(&state_arc),
        metrics.clone(),
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        peer,
        request_id,
        request_response::OutboundFailure::Timeout,
    )
    .await;

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::RetryRequests { requests, delay } => {
            let min_delay = Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS);
            let max_delay =
                Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS + GEN2_RETRY_MAX_JITTER_MS);
            assert!(
                delay >= min_delay && delay <= max_delay,
                "retry delay {delay:?} should stay within first-attempt jitter window"
            );
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].peer_id, peer);
            assert_eq!(requests[0].generation, ReqResGeneration::Gen2);
            assert_eq!(requests[0].retry_count, 1);
            assert!(!requests[0].fallback_attempted);
            requests[0]
                .request
                .verify_pow(&mut equix_builder, &peer, &local_peer)
                .expect("retry request PoW should verify");
            match &requests[0].request {
                NockchainRequest::BatchRequest { items, .. } => {
                    let retry_ids = items.iter().map(|item| item.item_id).collect::<Vec<_>>();
                    assert_eq!(retry_ids, vec![1, 2, 3]);
                }
                other => panic!("expected batch retry request, got {other:?}"),
            }
        }
        other => panic!("expected RetryRequests, got {other:?}"),
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "timeout retry path should only queue one retry action for the first attempt"
    );
    assert_eq!(metrics.req_res_retry_scheduled_total.fetch_add(0), 1);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "timeout failure should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn buffered_outbound_failure_queues_retry_without_waiting_on_swarm_queue() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        Arc::new(PeerStatsRegistry::default()),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(11)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(22)),
                },
            ],
        },
        0,
        false,
    );
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let mut equix_builder = equix::EquiXBuilder::new();
    let mut buffered_swarm_actions = VecDeque::new();
    tokio::time::timeout(Duration::from_millis(50), async {
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_outbound_request_failure_with_dispatcher(
            &mut swarm_actions,
            Arc::clone(&state_arc),
            metrics.clone(),
            local_peer,
            &mut equix_builder,
            PeerExclusions::default(),
            peer,
            request_id,
            request_response::OutboundFailure::Timeout,
        )
        .await;
    })
    .await
    .expect("buffered outbound-failure handling should not block");

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::RetryRequests { requests, .. }) => {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].peer_id, peer);
            assert_eq!(requests[0].generation, ReqResGeneration::Gen2);
            assert_eq!(requests[0].retry_count, 1);
            assert!(!requests[0].fallback_attempted);
        }
        other => panic!("expected buffered RetryRequests action, got {:?}", other),
    }
    assert!(
        buffered_swarm_actions.is_empty(),
        "buffered outbound-failure path should only queue one retry action"
    );
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "buffered timeout failure should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_timeout_updates_peer_stats_snapshot() {
    let metrics = isolated_test_metrics();
    let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        peer_stats_registry.clone(),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(9);
    let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4800"
        .parse()
        .expect("valid remote addr");
    let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/4801".parse().expect("valid local addr");
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 1,
            message: ByteBuf::from(jam_raw_tx_request(44)),
        },
        0,
        false,
    );
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.track_connection(
            connection_id,
            peer,
            &remote_addr,
            libp2p::core::ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        Arc::clone(&state_arc),
        metrics.clone(),
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        peer,
        request_id,
        request_response::OutboundFailure::Timeout,
    )
    .await;

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::RetryRequests { requests, .. } => {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].peer_id, peer);
            assert_eq!(requests[0].generation, ReqResGeneration::Gen2);
            assert_eq!(requests[0].retry_count, 1);
            match &requests[0].request {
                NockchainRequest::Request { message, .. } => {
                    assert_eq!(message.as_ref(), jam_raw_tx_request(44).as_slice());
                }
                other => panic!("expected singleton retry request, got {other:?}"),
            }
        }
        other => panic!("expected RetryRequests, got {other:?}"),
    }

    let snapshot = peer_stats_registry.snapshot();
    let entry = snapshot
        .peers
        .iter()
        .find(|entry| entry.peer_id == peer.to_base58())
        .expect("expected peer stats entry");
    assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen2);
    assert_eq!(entry.request_count, 1);
    assert_eq!(entry.failure_count, 1);
    assert_eq!(entry.timeout_count, 1);
    assert_eq!(metrics.gen2_outbound_failures.fetch_add(0), 1);
    assert_eq!(metrics.gen2_outbound_timeouts.fetch_add(0), 1);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "timeout failure should clear retained outbound context after stats update"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_single_response_updates_peer_stats_snapshot() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path single response records peer stats snapshot",
    );

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
    let state_arc = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
        peer_stats_registry.clone(),
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(7);
    let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4700"
        .parse()
        .expect("valid remote addr");
    let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/4701".parse().expect("valid local addr");
    let request_id = fresh_outbound_request_id();
    let mut request_context = OutboundRequestContext::new(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::Request {
            pow: [0; 16],
            nonce: 0,
            message: ByteBuf::from(jam_raw_tx_request(80_001)),
        },
    );
    request_context.started_at = Instant::now() - Duration::from_millis(25);

    {
        let mut state_guard = state_arc.lock().await;
        state_guard.track_connection(
            connection_id,
            peer,
            &remote_addr,
            libp2p::core::ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state_guard.observe_peer_generation(peer, ReqResGeneration::Gen2);
        state_guard.record_outbound_request(request_id, request_context);
    }

    let response = match tx_result_outcome(1, 16) {
        RequestExecutionOutcome::Result { response, .. } => response,
        other => panic!("expected singleton response outcome, got {other:?}"),
    };
    let response_bytes =
        req_res_message_encoded_bytes(&response).expect("response bytes should encode");
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), vec![PokeResult::Ack]).await;
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should process single response and update peer stats",
        handle_request_response(
            peer,
            connection_id,
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics,
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process single response");

    let snapshot = peer_stats_registry.snapshot();
    let entry = snapshot
        .peers
        .iter()
        .find(|entry| entry.peer_id == peer.to_base58())
        .expect("expected peer stats entry");
    assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen2);
    assert_eq!(entry.request_count, 1);
    assert_eq!(entry.bytes_received, response_bytes as u64);
    assert_eq!(entry.failure_count, 0);
    assert_eq!(entry.timeout_count, 0);
    assert!(entry.average_round_trip_ms >= 20.0);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "completed single response should clear retained outbound context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_terminal_batch_ack_and_not_found_clear_context_without_retry() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario",
        "driver-path terminal batch ack/not-found response clears context without retry",
    );

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_raw_tx_request(100)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_raw_tx_request(200)),
                },
            ],
        },
        0,
        false,
    );
    state_arc
        .lock()
        .await
        .record_outbound_request(request_id, request_context);

    let response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
        ],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should process terminal batch response without retry",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(1),
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics.clone(),
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process terminal batch response");

    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Err(_) | Ok(None) => {}
        Ok(Some(other)) => {
            panic!("expected no swarm action for terminal batch response, got {other:?}")
        }
    }

    assert_eq!(scripted_traffic.peek_count.load(Ordering::SeqCst), 0);
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert_eq!(metrics.req_res_retry_scheduled_total.fetch_add(0), 0);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "terminal batch response should clear retained outbound context"
    );

    let rendered = transcript.render();
    assert!(rendered.contains("terminal batch ack/not-found response"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_block_by_height_ack_queues_alternate_peer_retry() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(21);
    let peers = (0..10).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());
    let initial_peers = canonical_peers.iter().take(8).copied().collect::<Vec<_>>();
    let source_peer = canonical_peers[0];
    let expected_retry_peer = canonical_peers[8];
    let request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        for (index, peer_id) in peers.iter().copied().enumerate() {
            let remote_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4800 + index)
                .parse()
                .expect("valid remote addr");
            let local_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", 5800 + index)
                .parse()
                .expect("valid local addr");
            state_guard.track_connection(
                ConnectionId::new_unchecked(100 + index),
                peer_id,
                &remote_addr,
                libp2p::core::ConnectedPoint::Listener {
                    local_addr,
                    send_back_addr: remote_addr.clone(),
                },
            );
        }
        state_guard.track_block_height_attempted_peers(10030, initial_peers.iter().copied());
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen1,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 1,
                    message: request_message.clone(),
                },
                0,
                false,
            ),
        );
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();
    let scripted_traffic =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;

    handle_request_response(
        source_peer,
        connection_id,
        request_response::Message::Response {
            request_id,
            response: NockchainResponse::Ack { acked: true },
        },
        swarm_tx,
        &mut equix_builder,
        local_peer,
        scripted_traffic.traffic,
        metrics,
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .expect("block-by-height ack should be handled");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        } => {
            assert_eq!(peer_id, expected_retry_peer);
            assert_eq!(queued_message, request_message);
        }
        other => panic!("expected alternate QueueKernelRequest retry, got {other:?}"),
    }
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .contains(&expected_retry_peer),
        "alternate peer retry should be recorded as attempted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_block_by_height_timeout_queues_alternate_peer_retry() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let peers = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());
    let source_peer = canonical_peers[0];
    let expected_retry_peer = canonical_peers[1];
    let request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        for (index, peer_id) in peers.iter().copied().enumerate() {
            let remote_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4950 + index)
                .parse()
                .expect("valid remote addr");
            let local_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", 5950 + index)
                .parse()
                .expect("valid local addr");
            state_guard.track_connection(
                ConnectionId::new_unchecked(400 + index),
                peer_id,
                &remote_addr,
                libp2p::core::ConnectedPoint::Listener {
                    local_addr,
                    send_back_addr: remote_addr.clone(),
                },
            );
        }
        state_guard.track_block_height_attempted_peers(10030, [source_peer]);
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen1,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 1,
                    message: request_message.clone(),
                },
                0,
                false,
            ),
        );
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        state_arc.clone(),
        metrics,
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        source_peer,
        request_id,
        request_response::OutboundFailure::Timeout,
    )
    .await;

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message: queued_message,
        } => {
            assert_eq!(peer_id, expected_retry_peer);
            assert_eq!(queued_message, request_message);
        }
        other => panic!("expected alternate QueueKernelRequest retry, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "block timeout should schedule alternate-peer retry directly"
    );
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .contains(&expected_retry_peer),
        "alternate peer retry should be recorded as attempted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_block_by_height_ack_recycles_attempts_when_no_alternate_peer_exists() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(121);
    let source_peer = PeerId::random();
    let request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4801"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5801".parse().expect("valid local addr");
        state_guard.track_connection(
            ConnectionId::new_unchecked(301),
            source_peer,
            &remote_addr,
            libp2p::core::ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state_guard.track_block_height_attempted_peers(10030, [source_peer]);
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen1,
                NockchainRequest::Request {
                    pow: [0; 16],
                    nonce: 1,
                    message: request_message,
                },
                0,
                false,
            ),
        );
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();
    let scripted_traffic =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;

    handle_request_response(
        source_peer,
        connection_id,
        request_response::Message::Response {
            request_id,
            response: NockchainResponse::Ack { acked: true },
        },
        swarm_tx,
        &mut equix_builder,
        local_peer,
        scripted_traffic.traffic,
        metrics,
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .expect("single-peer block-by-height ack should recycle attempts");

    assert!(
        swarm_rx.try_recv().is_err(),
        "single-peer ack should recycle state without immediate self-retry"
    );
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .is_empty(),
        "single-peer ack should clear exhausted attempt state",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_batch_block_timeout_queues_alternate_peer_retry() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let peers = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());
    let source_peer = canonical_peers[0];
    let expected_retry_peer = canonical_peers[1];
    let block_request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        for (index, peer_id) in peers.iter().copied().enumerate() {
            let remote_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4975 + index)
                .parse()
                .expect("valid remote addr");
            let local_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", 5975 + index)
                .parse()
                .expect("valid local addr");
            state_guard.track_connection(
                ConnectionId::new_unchecked(500 + index),
                peer_id,
                &remote_addr,
                libp2p::core::ConnectedPoint::Listener {
                    local_addr,
                    send_back_addr: remote_addr.clone(),
                },
            );
        }
        state_guard.track_block_height_attempted_peers(10030, [source_peer]);
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![
                        BatchRequestItem {
                            item_id: 1,
                            message: block_request_message.clone(),
                        },
                        BatchRequestItem {
                            item_id: 2,
                            message: ByteBuf::from(jam_raw_tx_request(44)),
                        },
                    ],
                },
                0,
                false,
            ),
        );
    }

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    handle_outbound_request_failure(
        &swarm_tx,
        state_arc.clone(),
        metrics,
        local_peer,
        &mut equix_builder,
        PeerExclusions::default(),
        source_peer,
        request_id,
        request_response::OutboundFailure::Timeout,
    )
    .await;

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, expected_retry_peer);
            assert_eq!(request_message, block_request_message);
        }
        other => panic!("expected alternate QueueKernelRequest retry, got {other:?}"),
    }
    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::RetryRequests { requests, .. } => {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].peer_id, source_peer);
            match &requests[0].request {
                NockchainRequest::BatchRequest { items, .. } => {
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].item_id, 2);
                    let NockchainDataRequest::RawTransactionById(_, _) =
                        decode_request_item_message(&items[0].message)
                            .expect("retry batch item should decode")
                    else {
                        panic!("expected raw transaction retry item");
                    };
                }
                other => panic!("expected filtered retry batch, got {other:?}"),
            }
        }
        other => panic!("expected RetryRequests after alternate block retry, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "mixed batch timeout should only queue alternate block retry and filtered retry batch"
    );
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .contains(&expected_retry_peer),
        "alternate peer retry should be recorded as attempted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_batch_block_not_found_queues_alternate_peer_retry() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(22);
    let peers = (0..10).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable_by_key(|peer| peer.to_base58());
    let initial_peers = canonical_peers.iter().take(8).copied().collect::<Vec<_>>();
    let source_peer = canonical_peers[0];
    let expected_retry_peer = canonical_peers[8];
    let block_request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        for (index, peer_id) in peers.iter().copied().enumerate() {
            let remote_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4900 + index)
                .parse()
                .expect("valid remote addr");
            let local_addr: Multiaddr = format!("/ip4/0.0.0.0/tcp/{}", 5900 + index)
                .parse()
                .expect("valid local addr");
            state_guard.track_connection(
                ConnectionId::new_unchecked(200 + index),
                peer_id,
                &remote_addr,
                libp2p::core::ConnectedPoint::Listener {
                    local_addr,
                    send_back_addr: remote_addr.clone(),
                },
            );
        }
        state_guard.track_block_height_attempted_peers(10030, initial_peers.iter().copied());
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![
                        BatchRequestItem {
                            item_id: 1,
                            message: block_request_message.clone(),
                        },
                        BatchRequestItem {
                            item_id: 2,
                            message: ByteBuf::from(jam_raw_tx_request(44)),
                        },
                    ],
                },
                0,
                false,
            ),
        );
    }

    let response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
        ],
    };
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();
    let scripted_traffic =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;

    handle_request_response(
        source_peer,
        connection_id,
        request_response::Message::Response {
            request_id,
            response,
        },
        swarm_tx,
        &mut equix_builder,
        local_peer,
        scripted_traffic.traffic,
        metrics,
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .expect("batch not-found block item should be handled");

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(peer_id, expected_retry_peer);
            assert_eq!(request_message, block_request_message);
        }
        other => panic!("expected alternate QueueKernelRequest retry, got {other:?}"),
    }
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .contains(&expected_retry_peer),
        "alternate peer retry should be recorded as attempted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_batch_block_not_found_recycles_attempts_when_no_alternate_peer_exists() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let connection_id = ConnectionId::new_unchecked(122);
    let source_peer = PeerId::random();
    let block_request_message = ByteBuf::from(jam_block_by_height_request(10030));
    let request_id = fresh_outbound_request_id();

    {
        let mut state_guard = state_arc.lock().await;
        let remote_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4901"
            .parse()
            .expect("valid remote addr");
        let local_addr: Multiaddr = "/ip4/0.0.0.0/tcp/5901".parse().expect("valid local addr");
        state_guard.track_connection(
            ConnectionId::new_unchecked(302),
            source_peer,
            &remote_addr,
            libp2p::core::ConnectedPoint::Listener {
                local_addr,
                send_back_addr: remote_addr.clone(),
            },
        );
        state_guard.track_block_height_attempted_peers(10030, [source_peer]);
        state_guard.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                source_peer,
                ReqResGeneration::Gen2,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![BatchRequestItem {
                        item_id: 1,
                        message: block_request_message,
                    }],
                },
                0,
                false,
            ),
        );
    }

    let response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 1,
            status: BatchResultStatus::NotFound,
            error: None,
            envelope: None,
        }],
    };
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();
    let scripted_traffic =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;

    handle_request_response(
        source_peer,
        connection_id,
        request_response::Message::Response {
            request_id,
            response,
        },
        swarm_tx,
        &mut equix_builder,
        local_peer,
        scripted_traffic.traffic,
        metrics,
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .expect("single-peer batch not-found should recycle attempts");

    assert!(
        swarm_rx.try_recv().is_err(),
        "single-peer batch not-found should recycle state without immediate self-retry"
    );
    assert!(
        state_arc
            .lock()
            .await
            .get_block_height_attempted_peers(10030)
            .is_empty(),
        "single-peer batch not-found should clear exhausted attempt state",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_retry_respects_bounded_backoff_and_split_batches() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path batch retries keep bounded delay and split large retry sets",
    );

    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(jam_block_by_height_request(11)),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(jam_block_by_height_request(22)),
                },
                BatchRequestItem {
                    item_id: 3,
                    message: ByteBuf::from(jam_block_by_height_request(33)),
                },
                BatchRequestItem {
                    item_id: 4,
                    message: ByteBuf::from(jam_block_by_height_request(44)),
                },
            ],
        },
        1,
        false,
    );
    state_arc
        .lock()
        .await
        .record_outbound_request(request_id, request_context.clone());

    let response = NockchainResponse::BatchResult {
        results: vec![
            batch_error_result(1, BatchErrorClass::Backpressure),
            batch_error_result(2, BatchErrorClass::Backpressure),
            batch_error_result(3, BatchErrorClass::Backpressure),
            batch_error_result(4, BatchErrorClass::Backpressure),
        ],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    run_driver_with_timeout(
        &transcript,
        "driver should process backpressure batch response",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(2),
            request_response::Message::Response {
                request_id,
                response,
            },
            swarm_tx,
            &mut equix_builder,
            local_peer,
            scripted_traffic.traffic.clone(),
            metrics,
            Arc::clone(&state_arc),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("driver should process backpressure batch response");

    let action = recv_swarm_action(&mut swarm_rx).await;
    match action {
        SwarmAction::RetryRequests { requests, delay } => {
            transcript.record(
                "driver",
                format!(
                    "scheduled split retry set of {} request(s) after {:?}",
                    requests.len(),
                    delay
                ),
            );
            let min_delay = Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS * 2);
            let max_delay =
                Duration::from_millis(GEN2_RETRY_BASE_DELAY_MS * 2 + GEN2_RETRY_MAX_JITTER_MS);
            assert!(
                delay >= min_delay && delay <= max_delay,
                "retry delay {delay:?} should stay within second-attempt jitter window"
            );
            assert_eq!(requests.len(), 2);
            let retry_shapes = requests
                .iter()
                .map(|context| match &context.request {
                    NockchainRequest::BatchRequest { items, .. } => {
                        items.iter().map(|item| item.item_id).collect::<Vec<_>>()
                    }
                    other => panic!("expected split batch retry request, got {other:?}"),
                })
                .collect::<Vec<_>>();
            assert_eq!(retry_shapes, vec![vec![1, 2], vec![3, 4]]);
        }
        other => panic!("expected RetryRequests, got {other:?}"),
    }

    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    assert!(
        state_arc
            .lock()
            .await
            .outbound_request_context(request_id)
            .is_none(),
        "batch retry scheduling should clear retained outbound context"
    );
}

#[test]
fn test_heard_tx_response_dedupe_ignores_seen_blocks() {
    let metrics = isolated_test_metrics();
    let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
    let tx_id = String::from("tx-id");

    state.seen_blocks.insert(tx_id.clone());

    let should_process = should_process_response(
        &ResponseProcessingGate::HeardTx(tx_id),
        &mut state,
        &metrics,
        false,
    );

    assert!(
        should_process,
        "seen block IDs must not suppress tx responses"
    );
    assert_eq!(metrics.tx_seen_cache_misses.fetch_add(0), 1);
    assert_eq!(metrics.block_seen_cache_misses.fetch_add(0), 0);
}

#[test]
fn test_heard_tx_response_dedupe_uses_tx_metrics() {
    let metrics = isolated_test_metrics();
    let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
    let tx_id = String::from("tx-id");

    state.seen_txs.insert(tx_id.clone());

    let should_process = should_process_response(
        &ResponseProcessingGate::HeardTx(tx_id),
        &mut state,
        &metrics,
        false,
    );

    assert!(
        !should_process,
        "seen tx IDs must suppress duplicate tx responses"
    );
    assert_eq!(metrics.tx_seen_cache_hits.fetch_add(0), 1);
    assert_eq!(metrics.block_seen_cache_hits.fetch_add(0), 0);
}

#[test]
fn test_heard_block_response_dedupe_uses_processing_metrics() {
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry())
            .expect("Could not register metrics"),
    );
    let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
    let block_id = String::from("block-id");

    let first_should_process = should_process_response(
        &ResponseProcessingGate::HeardBlock(block_id.clone()),
        &mut state,
        &metrics,
        false,
    );
    let second_should_process = should_process_response(
        &ResponseProcessingGate::HeardBlock(block_id.clone()),
        &mut state,
        &metrics,
        false,
    );

    assert!(first_should_process, "first block should begin processing");
    assert!(
        !second_should_process,
        "duplicate block should be gated while the first one is processing"
    );
    assert!(state.is_processing_block(&block_id));
    assert_eq!(metrics.block_seen_cache_misses.fetch_add(0), 1);
    assert_eq!(metrics.block_seen_cache_hits.fetch_add(0), 1);
}

#[test]
fn test_heard_block_response_dedupe_allows_kernel_requested_replay() {
    let metrics = isolated_test_metrics();
    let mut state = P2PState::new(metrics.clone(), LIBP2P_CONFIG.seen_tx_clear_interval);
    let block_id = String::from("block-id");

    state.finish_processing_block_seen(&block_id);

    let duplicate_should_process = should_process_response(
        &ResponseProcessingGate::HeardBlock(block_id.clone()),
        &mut state,
        &metrics,
        false,
    );
    let requested_should_process = should_process_response(
        &ResponseProcessingGate::HeardBlock(block_id.clone()),
        &mut state,
        &metrics,
        true,
    );

    assert!(
        !duplicate_should_process,
        "ordinary duplicate block responses should stay gated"
    );
    assert!(
        requested_should_process,
        "explicit kernel demand should replay a seen block response"
    );
    assert!(state.is_processing_block(&block_id));
    assert_eq!(metrics.block_seen_cache_hits.fetch_add(0), 1);
    assert_eq!(metrics.block_seen_cache_misses.fetch_add(0), 1);
}
