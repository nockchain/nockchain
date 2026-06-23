use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use libp2p::identify::Event::Received;
use libp2p::identity::Keypair;
use libp2p::kad::NoKnownPeers;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::Event::*;
use libp2p::request_response::Message::*;
use libp2p::request_response::{self};
use libp2p::swarm::{ConnectionId, DialError, ListenError, SwarmEvent};
use libp2p::{
    allow_block_list, connection_limits, memory_connection_limits, ping, Multiaddr, PeerId, Swarm,
};
use nockapp::driver::IODriverFn;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{Wire, WireRepr};
use nockapp::NockAppError;
use nockvm::noun::{NounAllocator, D, T};
use nockvm_macros::tas;
use rand::seq::SliceRandom;
use rand::{rng, Rng};
use serde_bytes::ByteBuf;
use tokio::sync::{mpsc, Mutex, MutexGuard};
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::behaviour::{NockchainBehaviour, NockchainEvent};
use crate::config::LibP2PConfig;
use crate::ip_block::{
    AddressCooldownOutcome, ExclusionOutcome, IpExclusionOutcome, PeerExclusions,
};
use crate::messages::{
    block_with_txs_by_height_request_message, decode_request_item_message,
    request_slab_from_message, NockchainDataRequest, NockchainRequest, NockchainResponse,
    FACT_POKE_VERSION,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{OutboundRequestContext, P2PState, RangeCapability, ReqResGeneration};
use crate::p2p_util::{
    log_fail2ban_ipv4, log_fail2ban_ipv6, multiaddr_without_p2p, MultiaddrExt, PeerIdExt,
};
use crate::peer_stats::PeerReqResGeneration;
use crate::tip5_util::tip5_hash_to_base58_stack;
use crate::tracked_join_set::TrackedJoinSet;
use crate::traffic_cop;

mod actions;
mod gen1;
mod gen2;
mod kernel_io;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod watchdog;

pub(crate) use actions::SwarmAction;
use actions::{process_swarm_action, SwarmActionDispatcher};
pub(crate) use gen1::build_unsupported_protocol_fallback_contexts;
pub(crate) use gen2::{
    build_retry_request_contexts, collect_tip5_zset_strings, handle_outbound_request_failure,
    heard_block_height_from_fact_poke, heard_block_tx_ids_from_fact_poke,
    validate_response_envelope_for_request,
};
pub(crate) use kernel_io::{
    create_response_result_from_payload, create_scry_response, record_crown_error_metric,
    request_to_scry_slab,
};

//TODO This wire is a placeholder for now. The libp2p driver is entangled with the other types of nockchain pokes
//for historical reasons, and should be disentangled in the future.
pub enum NockchainWire {
    Local,
}

impl Wire for NockchainWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "nc";
}

#[derive(Debug)]
pub enum Libp2pWire {
    Gossip(PeerId),
    Response(PeerId),
}

impl Libp2pWire {
    fn verb(&self) -> &'static str {
        match self {
            Libp2pWire::Gossip(_) => "gossip",
            Libp2pWire::Response(_) => "response",
        }
    }

    fn peer_id(&self) -> &PeerId {
        match self {
            Libp2pWire::Gossip(peer_id) => peer_id,
            Libp2pWire::Response(peer_id) => peer_id,
        }
    }
}

impl Wire for Libp2pWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "libp2p";

    fn to_wire(&self) -> WireRepr {
        let tags = vec![self.verb().into(), "peer-id".into(), self.peer_id().to_base58().into()];
        WireRepr::new(Libp2pWire::SOURCE, Libp2pWire::VERSION, tags)
    }
}

enum EffectType {
    Gossip,
    Request,
    LiarPeer,
    LiarBlockId,
    Track,
    Seen,
    Unknown,
}

impl EffectType {
    fn from_noun_slab(noun_slab: &NounSlab) -> Self {
        let space = noun_slab.noun_space();
        let Ok(effect_cell) = unsafe { *noun_slab.root() }.in_space(&space).as_cell() else {
            return EffectType::Unknown;
        };

        let head = effect_cell.head();
        let Ok(atom) = head.as_atom() else {
            return EffectType::Unknown;
        };
        let Ok(bytes) = atom.to_bytes_until_nul() else {
            warn!("atom was not properly formatted");
            return EffectType::Unknown;
        };

        match bytes.as_slice() {
            b"gossip" => EffectType::Gossip,
            b"request" => EffectType::Request,
            b"liar-peer" => EffectType::LiarPeer,
            b"liar-block-id" => EffectType::LiarBlockId,
            b"track" => EffectType::Track,
            b"seen" => EffectType::Seen,
            _ => EffectType::Unknown,
        }
    }
}

fn request_effect_trace_summary(noun_slab: &NounSlab) -> String {
    let space = noun_slab.noun_space();
    let Ok(effect_cell) = unsafe { *noun_slab.root() }.in_space(&space).as_cell() else {
        return String::from("request malformed");
    };
    let Ok(request_body) = effect_cell.tail().as_cell() else {
        return String::from("request malformed");
    };
    let request_type = request_body
        .head()
        .as_atom()
        .ok()
        .and_then(|atom| atom.to_bytes_until_nul().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_else(|| String::from("<non-atom>"));
    if request_type != "block" {
        return format!("request {request_type}");
    }
    let Ok(block_body) = request_body.tail().as_cell() else {
        return String::from("request block malformed");
    };
    let block_head = block_body
        .head()
        .as_atom()
        .ok()
        .and_then(|atom| atom.to_bytes_until_nul().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_else(|| String::from("<non-atom>"));
    if block_head == "by-height" {
        let height = block_body
            .tail()
            .as_atom()
            .ok()
            .and_then(|atom| atom.as_u64().ok())
            .map(|height| height.to_string())
            .unwrap_or_else(|| String::from("<non-u64>"));
        return format!("request block by-height {height}");
    }
    format!("request block {block_head}")
}

fn should_redial_initial_peers(
    connected_peer_count: usize,
    initial_peers: &[Multiaddr],
    backbone_peers: &[Multiaddr],
) -> bool {
    connected_peer_count == 0 && !(initial_peers.is_empty() && backbone_peers.is_empty())
}

/// Returns the next round-robin window of up to `count` peers from `pool`,
/// starting at `*cursor` and wrapping around, then advances `*cursor` past the
/// window so the following call dials a different subset. An empty pool yields
/// an empty window.
fn next_backbone_window(pool: &[Multiaddr], cursor: &mut usize, count: usize) -> Vec<Multiaddr> {
    if pool.is_empty() {
        return Vec::new();
    }
    let take = count.min(pool.len());
    let start = *cursor % pool.len();
    let window = (0..take)
        .map(|i| pool[(start + i) % pool.len()].clone())
        .collect();
    *cursor = (start + take) % pool.len();
    window
}

fn redial_initial_peers(
    swarm: &mut Swarm<NockchainBehaviour>,
    initial_peers: &[Multiaddr],
    backbone_peers: &[Multiaddr],
    backbone_cursor: &mut usize,
    backbone_dial_count: usize,
    reason: &'static str,
) -> Result<bool, NockAppError> {
    let connected_peer_count = swarm.connected_peers().count();
    if !should_redial_initial_peers(connected_peer_count, initial_peers, backbone_peers) {
        return Ok(false);
    }

    let backbone_window =
        next_backbone_window(backbone_peers, backbone_cursor, backbone_dial_count);
    info!(
        reason,
        connected_peer_count,
        initial_peer_count = initial_peers.len(),
        backbone_dialed = backbone_window.len(),
        "Redialing initial peers while disconnected"
    );
    if !initial_peers.is_empty() {
        dial_peers(swarm, initial_peers)?;
    }
    if !backbone_window.is_empty() {
        dial_peers(swarm, &backbone_window)?;
    }
    Ok(true)
}

const INITIAL_PEER_REDIAL_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound for the exponential backoff between initial-peer redial attempts.
/// We never give up: once the backoff reaches this value we keep redialing at
/// this cadence (hourly) for as long as we have zero connected peers.
const INITIAL_PEER_REDIAL_MAX_INTERVAL: Duration = Duration::from_secs(3600);
const PREFETCH_TARGET_RESPONSE_NUMERATOR: usize = 31;
const PREFETCH_TARGET_RESPONSE_DENOMINATOR: usize = 32;
const PREFETCH_COLD_BLOCK_BUNDLE_ESTIMATE_BYTES: usize = 128 * 1024;
const PREFETCH_RANGE_PROBE_LEN: u8 = 2;

#[derive(Debug, Clone, Copy)]
struct PrefetchWindowSelection {
    window: u8,
    estimated_response_bytes_per_block: usize,
    estimate_source: &'static str,
    target_response_bytes: usize,
    response_budget_bytes: usize,
}

async fn select_prefetch_window(
    height: u64,
    prefetch_config: PrefetchConfig,
    driver_state: Arc<Mutex<P2PState>>,
    req_res_limits: gen2::ReqResRuntimeLimits,
) -> PrefetchWindowSelection {
    let floor = prefetch_config.window_initial.max(1);
    let ceiling = prefetch_config.window_max.max(floor);
    let request = NockchainDataRequest::BlockWithTxsByHeight(height);
    let (estimated_response_bytes, estimate_source) = driver_state
        .lock()
        .await
        .estimated_response_message_bytes(&request, req_res_limits.gen2_item_max_bytes);
    let estimated_response_bytes_per_block = match estimate_source {
        "configured_bundle_cap" => PREFETCH_COLD_BLOCK_BUNDLE_ESTIMATE_BYTES,
        _ => estimated_response_bytes.max(1),
    };
    let response_budget_bytes = gen2::block_batch_response_budget_bytes(req_res_limits);
    let target_response_bytes = response_budget_bytes
        .saturating_mul(PREFETCH_TARGET_RESPONSE_NUMERATOR)
        .checked_div(PREFETCH_TARGET_RESPONSE_DENOMINATOR)
        .unwrap_or(response_budget_bytes)
        .max(estimated_response_bytes_per_block);
    let estimated_window = target_response_bytes
        .checked_div(estimated_response_bytes_per_block)
        .unwrap_or(1)
        .max(1);
    let bounded_window = estimated_window.clamp(usize::from(floor), usize::from(ceiling));

    PrefetchWindowSelection {
        window: bounded_window as u8,
        estimated_response_bytes_per_block,
        estimate_source,
        target_response_bytes,
        response_budget_bytes,
    }
}

fn prefetch_peer_rendezvous_score(
    peer_id: &PeerId,
    start_height: u64,
    window: u8,
    weight: f64,
) -> f64 {
    let mut hasher = DefaultHasher::new();
    "nockchain-prefetch-peer-v2".hash(&mut hasher);
    start_height.hash(&mut hasher);
    window.hash(&mut hasher);
    peer_id.hash(&mut hasher);
    let unit = (hasher.finish() as f64 + 1.0) / (u64::MAX as f64 + 1.0);
    -unit.ln() / weight.max(0.01)
}

fn select_weighted_prefetch_peer(
    candidates: impl IntoIterator<Item = (PeerId, f64)>,
    start_height: u64,
    window: u8,
) -> Option<PeerId> {
    candidates
        .into_iter()
        .min_by(|(left_peer, left_weight), (right_peer, right_weight)| {
            prefetch_peer_rendezvous_score(left_peer, start_height, window, *left_weight)
                .total_cmp(&prefetch_peer_rendezvous_score(
                    right_peer, start_height, window, *right_weight,
                ))
                .then_with(|| left_peer.to_base58().cmp(&right_peer.to_base58()))
        })
        .map(|(peer_id, _)| peer_id)
}

async fn pick_prefetch_peer(
    connected_peers: &[PeerId],
    driver_state: Arc<Mutex<P2PState>>,
    max_inflight_per_peer: u8,
    start_height: u64,
    requested_window: u8,
) -> PrefetchPeerSelection {
    if connected_peers.is_empty() {
        return PrefetchPeerSelection::NoCandidate { saw_gen2: false };
    }
    let state_guard = driver_state.lock().await;
    let bandwidth_cap = state_guard.prefetch_bandwidth_cap_per_peer_bytes_per_min();
    let mut throttled = false;
    let mut saw_gen2 = false;
    let mut supported = Vec::new();
    let mut unknown = Vec::new();

    for peer_id in connected_peers {
        if state_guard.peer_req_res_generation(peer_id) != PeerReqResGeneration::Gen2 {
            continue;
        }
        saw_gen2 = true;
        if state_guard.inflight_prefetch_count_for_peer(peer_id) >= max_inflight_per_peer {
            continue;
        }
        if bandwidth_cap > 0
            && state_guard.prefetch_bandwidth_window_bytes(peer_id) >= bandwidth_cap
        {
            throttled = true;
            continue;
        }
        if state_guard.is_peer_prefetch_cooldown_active(peer_id) {
            continue;
        }
        let weight = state_guard.prefetch_peer_selection_weight(peer_id);
        match state_guard.peer_range_capability(peer_id) {
            RangeCapability::Supported => supported.push((*peer_id, weight)),
            RangeCapability::Unknown => unknown.push((*peer_id, weight)),
            RangeCapability::Unsupported => {}
        }
    }

    if let Some(peer_id) = select_weighted_prefetch_peer(supported, start_height, requested_window)
    {
        return PrefetchPeerSelection::Selected(PrefetchPeer {
            peer_id,
            window: requested_window,
            probe: false,
        });
    }
    if let Some(peer_id) = select_weighted_prefetch_peer(
        unknown,
        start_height,
        requested_window.min(PREFETCH_RANGE_PROBE_LEN),
    ) {
        return PrefetchPeerSelection::Selected(PrefetchPeer {
            peer_id,
            window: requested_window.min(PREFETCH_RANGE_PROBE_LEN),
            probe: true,
        });
    }
    if throttled {
        PrefetchPeerSelection::Throttled
    } else {
        PrefetchPeerSelection::NoCandidate { saw_gen2 }
    }
}

#[derive(Debug)]
struct PrefetchPeer {
    peer_id: PeerId,
    window: u8,
    probe: bool,
}

#[derive(Debug)]
enum PrefetchPeerSelection {
    Selected(PrefetchPeer),
    Throttled,
    NoCandidate { saw_gen2: bool },
}

fn select_request_peers_with_preferences(
    mut target_peers: Vec<PeerId>,
    max_peers: usize,
    preserve_peer_order: bool,
    preferred_peers: &[PeerId],
) -> Vec<PeerId> {
    let target_peer_set = target_peers.iter().copied().collect::<BTreeSet<_>>();
    let mut selected = Vec::new();
    let mut selected_set = BTreeSet::new();

    for preferred_peer in preferred_peers {
        if selected.len() >= max_peers {
            return selected;
        }
        if target_peer_set.contains(preferred_peer) && selected_set.insert(*preferred_peer) {
            selected.push(*preferred_peer);
        }
    }

    target_peers.retain(|peer| !selected_set.contains(peer));
    if preserve_peer_order {
        target_peers.sort_unstable_by_key(|peer| peer.to_base58());
        selected.extend(
            target_peers
                .into_iter()
                .take(max_peers.saturating_sub(selected.len())),
        );
        selected
    } else {
        let mut rng = rng();
        target_peers.shuffle(&mut rng);
        selected.extend(
            target_peers
                .into_iter()
                .take(max_peers.saturating_sub(selected.len())),
        );
        selected
    }
}

#[instrument(skip(keypair, bind, allowed, limits, memory_limits, equix_builder))]
pub fn make_libp2p_driver(
    keypair: Keypair,
    bind: Vec<Multiaddr>,
    allowed: Option<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>,
    limits: connection_limits::ConnectionLimits,
    memory_limits: Option<memory_connection_limits::Behaviour>,
    initial_peers: &[Multiaddr],
    backbone_peers: &[Multiaddr],
    backbone_dial_count: usize,
    force_peers: &[Multiaddr],
    prune_inbound_size: Option<usize>,
    mut equix_builder: equix::EquiXBuilder,
    chain_interval: Duration,
    init_complete_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> IODriverFn {
    let initial_peers = Vec::from(initial_peers);
    let backbone_peers = Vec::from(backbone_peers);
    let force_peers = Vec::from(force_peers);
    Box::new(move |handle| {
        let metrics = Arc::new(
            NockchainP2PMetrics::register(gnort::global_metrics_registry())
                .expect("Failed to register metrics!"),
        );

        Box::pin(async move {
            let libp2p_config = LibP2PConfig::from_env()?;
            debug!("Libp2p config: {:?}", libp2p_config);
            let peer_exclusion_config = libp2p_config.peer_exclusion_config()?;
            let peer_exclusions = PeerExclusions::new(peer_exclusion_config);
            if LibP2PConfig::gen2_block_batch_max_response_bytes_override_present() {
                warn!(
                    configured_cap = libp2p_config.gen2_block_batch_max_response_bytes(),
                    env_var = "NOCKCHAIN_LIBP2P_GEN2_BLOCK_BATCH_MAX_RESPONSE_BYTES",
                    "Ignoring inactive BlockByHeight gen2 response-budget override because outbound BlockByHeight requests remain singleton gen1 traffic"
                );
            }
            let kademlia_bootstrap_interval = libp2p_config.kademlia_bootstrap_interval();
            let force_peer_dial_interval = libp2p_config.force_peer_dial_interval();
            let request_high_reset = libp2p_config.request_high_reset();
            let request_high_threshold = libp2p_config.request_high_threshold;
            let gen2_batch_coalesce_window = libp2p_config.gen2_batch_coalesce_window();
            let req_res_limits = gen2::ReqResRuntimeLimits {
                request_high_threshold,
                request_replay_cache_ttl: libp2p_config.request_replay_cache_ttl(),
                request_replay_cache_max_per_peer: libp2p_config.request_replay_cache_max_per_peer,
                ip_bucket_request_admission_limit: libp2p_config.ip_bucket_request_admission_limit,
                ip_bucket_connection_limit: libp2p_config.ip_bucket_connection_limit,
                gossip_bucket_capacity: libp2p_config.gossip_bucket_capacity,
                gossip_bucket_refill_per_second: libp2p_config.gossip_bucket_refill_per_second,
                authenticated_gossip_send_enabled: libp2p_config
                    .req_res_authenticated_gossip_send_enabled,
                legacy_gossip_accept_enabled: libp2p_config.req_res_legacy_gossip_accept_enabled,
                block_range_max_len: libp2p_config.prefetch_window_max.max(1),
                gen2_batch_max_items: libp2p_config.gen2_batch_max_items(),
                gen2_batch_max_bytes: libp2p_config.gen2_batch_max_bytes(),
                gen2_item_max_bytes: libp2p_config.gen2_item_max_bytes(),
                gen2_block_batch_max_response_bytes: libp2p_config
                    .gen2_block_batch_max_response_bytes(),
                gen2_max_inflight_per_peer: libp2p_config.gen2_max_inflight_per_peer(),
            };
            let peer_status_interval = libp2p_config.peer_status_interval_secs();
            let seen_tx_clear_interval = libp2p_config.seen_tx_clear_interval();
            let min_peers = libp2p_config.min_peers();
            let low_priority_peek_timeout = libp2p_config.low_priority_peek_timeout();
            let failed_pings_before_close = libp2p_config.failed_pings_before_close();
            let req_res_gen2_send_enabled = libp2p_config.req_res_gen2_send_enabled;
            let req_res_gen2_bundle_enabled = libp2p_config.req_res_gen2_bundle_enabled;
            let prefetch_config = PrefetchConfig {
                enabled: libp2p_config.prefetch_enabled,
                window_initial: libp2p_config.prefetch_window_initial,
                window_max: libp2p_config.prefetch_window_max,
                max_inflight_per_peer: libp2p_config.prefetch_max_inflight_per_peer,
                kernel_demand_threshold: libp2p_config.prefetch_behind_threshold.max(1),
            };
            let prefetch_height_failure_budget = libp2p_config.prefetch_height_failure_budget;
            let prefetch_stuck_backoff =
                Duration::from_secs(libp2p_config.prefetch_stuck_backoff_secs);
            let prefetch_bandwidth_cap_per_peer_bytes_per_min =
                libp2p_config.prefetch_bandwidth_cap_per_peer_bytes_per_min;
            let prefetch_behind_threshold = libp2p_config.prefetch_behind_threshold;
            let prefetch_peer_observed_threshold = libp2p_config.prefetch_peer_observed_threshold;
            let prefetch_hysteresis = Duration::from_millis(libp2p_config.prefetch_hysteresis_ms);
            let swarm_action_queue_capacity = libp2p_config.gen2_swarm_action_queue_capacity();
            let mut swarm = match start_swarm(
                libp2p_config,
                keypair,
                bind,
                allowed,
                limits,
                memory_limits,
                peer_exclusions.clone(),
            ) {
                Ok(swarm) => swarm,
                Err(e) => {
                    error!("Could not create swarm: {}", e);
                    let (_, handle_clone) = handle.dup();
                    tokio::spawn(async move {
                        if let Err(e) = handle_clone.exit.exit(1).await {
                            error!("Failed to send exit signal: {}", e);
                        }
                    });
                    return Err(NockAppError::OtherError(String::from(
                        "Could not start swarm",
                    )));
                }
            };
            let (swarm_tx, mut swarm_rx) =
                mpsc::channel::<SwarmAction>(swarm_action_queue_capacity);
            let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
            let driver_state = Arc::new(Mutex::new(P2PState::new(
                metrics.clone(),
                seen_tx_clear_interval,
            )));
            // Phase 5: thread the safety knobs into P2PState so the retry
            // path can read them without touching every call site.
            {
                let mut state_guard = driver_state.lock().await;
                state_guard.set_prefetch_safety_config(
                    prefetch_height_failure_budget, prefetch_stuck_backoff,
                    prefetch_bandwidth_cap_per_peer_bytes_per_min,
                );
                state_guard.set_catch_up_config(
                    prefetch_behind_threshold, prefetch_peer_observed_threshold,
                    prefetch_hysteresis,
                );
            }
            let mut kad_bootstrap = tokio::time::interval(kademlia_bootstrap_interval);
            kad_bootstrap.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut initial_peer_redial = tokio::time::interval_at(
                Instant::now() + INITIAL_PEER_REDIAL_INTERVAL,
                INITIAL_PEER_REDIAL_INTERVAL,
            );
            initial_peer_redial.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut force_peer_dial = tokio::time::interval(force_peer_dial_interval);
            force_peer_dial.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut reset_request_counts = tokio::time::interval(request_high_reset);
            reset_request_counts.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut gen2_batch_flush =
                gen2::new_gen2_batch_flush_interval(gen2_batch_coalesce_window);
            let mut nockchain_timer = tokio::time::interval(chain_interval);
            nockchain_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let nockchain_timer_mutex = Arc::new(Mutex::new(()));
            let (traffic_handle, effect_handle) = handle.dup();
            let traffic_cop = traffic_cop::TrafficCop::new_with_peek_timeout(
                traffic_handle, &mut join_set, low_priority_peek_timeout,
            );

            let mut initial_peer_redial_backoff = INITIAL_PEER_REDIAL_INTERVAL;
            let mut pending_gen2_batches = BTreeMap::<PeerId, gen2::PendingGen2Batch>::new();
            let mut peer_gen2_inbound = BTreeMap::<PeerId, bool>::new();
            gen2::update_pending_batch_metrics(&metrics, &pending_gen2_batches);
            // Start the round-robin at a random offset so nodes don't all dial
            // the same backbone subset first; each dial then advances the window.
            let mut backbone_cursor: usize = if backbone_peers.is_empty() {
                0
            } else {
                rand::rng().random_range(0..backbone_peers.len())
            };
            if !initial_peers.is_empty() {
                dial_peers(&mut swarm, &initial_peers)?;
            }
            let initial_backbone_window =
                next_backbone_window(&backbone_peers, &mut backbone_cursor, backbone_dial_count);
            if !initial_backbone_window.is_empty() {
                dial_peers(&mut swarm, &initial_backbone_window)?;
            }
            if let Some(tx) = init_complete_tx {
                let _ = tx.send(());
                debug!("libp2p driver initialization complete signal sent");
            }

            // ---- deadlock-forensics instrumentation ----
            // Shared monotonic counter, bumped every 5 s by a tokio task.
            // The watchdog std::thread reads this counter to detect runtime
            // stalls, and an on-demand SIGQUIT handler (Linux only) can also
            // trigger a stack dump. Both write /proc/self/task/*/stack to a
            // timestamped file so the next LAX1-style freeze yields the
            // specific futex addresses the 2026-04-17 incident lacked.
            let heartbeat_counter: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
            {
                let hb = heartbeat_counter.clone();
                let metrics_hb = metrics.clone();
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(Duration::from_secs(5));
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        ticker.tick().await;
                        let tick = hb.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                        // Forward to gnort so Datadog can `rate()` this
                        // counter and alert when it stops advancing.
                        metrics_hb.heartbeat_tick.increment();
                        info!(
                            target: "nockchain::heartbeat",
                            tick,
                            "libp2p driver heartbeat"
                        );
                    }
                });
            }
            #[cfg(target_os = "linux")]
            watchdog::spawn_deadlock_watchdog(
                heartbeat_counter.clone(),
                traffic_cop.last_poke_completed_at(),
                metrics.clone(),
            );

            let mut connectivity_interval = tokio::time::interval(peer_status_interval);
            let mut buffered_swarm_actions = VecDeque::new();
            loop {
                if let Some(swarm_action) = buffered_swarm_actions.pop_front() {
                    process_swarm_action(
                        swarm_action, &mut swarm, &mut buffered_swarm_actions, &swarm_tx,
                        &mut join_set, &driver_state, &metrics, &peer_exclusions,
                        &mut equix_builder, &mut peer_gen2_inbound, &mut pending_gen2_batches,
                        req_res_gen2_send_enabled, req_res_limits, &traffic_cop,
                    )
                    .await?;
                    continue;
                }
                let timer_fut = async {
                    let _ = nockchain_timer.tick().await;
                    nockchain_timer_mutex.clone().lock_owned().await
                };
                tokio::select! {
                    guard = timer_fut => {
                        join_set.spawn("timer".to_string(), send_timer_poke(guard, traffic_cop.clone(), metrics.clone()))
                    }
                    _ = connectivity_interval.tick() => {
                        let peer_count = log_peer_status(
                            &mut swarm,
                            &metrics,
                            &peer_exclusions,
                            &driver_state
                        ).await;
                        if peer_count < min_peers {
                            let state_guard = driver_state.lock().await;
                            dial_more_peers(&mut swarm, state_guard, &peer_exclusions);
                        }
                    },
                    Ok(noun_slab) = effect_handle.next_effect() => {
                        let connected_peers: Vec<PeerId> = swarm.connected_peers().cloned().collect();
                        // Preserve kernel effect ordering within a poke burst so
                        // adjacent block requests can queue before trailing gossip
                        // effects interleave. Buffering those actions locally keeps
                        // the select loop from awaiting its own bounded swarm queue.
                        let mut swarm_actions =
                            SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
                        handle_effect_with_dispatcher(
                            noun_slab,
                            &mut swarm_actions,
                            connected_peers,
                            req_res_gen2_bundle_enabled,
                            prefetch_config,
                            req_res_limits,
                            Arc::clone(&driver_state),
                            metrics.clone(),
                            peer_exclusions.clone(),
                        )
                        .await?;
                    },
                    Some(event) = swarm.next() => {
                        match event {
                            SwarmEvent::NewListenAddr { address, .. } => {
                                info!("SEvent: Listening on {address:?}");
                            },
                            SwarmEvent::ListenerError { error, .. } => {
                                error!("SEvent: Listener error: {error:?}");
                            },
                            SwarmEvent::ListenerClosed { addresses, reason, .. } => {
                                if let Err(e) = reason {
                                    error!("SEvent: Listener closed on {addresses:?} because of {e:?}");
                                } else {
                                    info!("SEvent: Listener closed on {addresses:?}");
                                }
                            },
                            SwarmEvent::Behaviour(NockchainEvent::Identify(Received { connection_id: _, peer_id, info })) => {
                                trace!("SEvent: identify_received");
                                let supports_gen2 = identify_received(&mut swarm, peer_id, info, &peer_exclusions, &metrics)?;
                                driver_state.lock().await.observe_peer_generation(
                                    peer_id,
                                    if supports_gen2 {
                                        ReqResGeneration::Gen2
                                    } else {
                                        ReqResGeneration::Gen1
                                    },
                                );
                                peer_gen2_inbound.insert(peer_id, supports_gen2);
                                trace!(peer = %peer_id, supports_gen2, "peer gen2 inbound support recorded");
                            },
                            SwarmEvent::Behaviour(NockchainEvent::Kad(event)) => {
                                trace!("SEvent: kad event {event:?}");
                                observe_kad_cardinality_and_exclude(
                                    &mut swarm,
                                    &driver_state,
                                    &peer_exclusions,
                                    &metrics,
                                ).await;
                                prune_excluded_swarm_state(
                                    &mut swarm,
                                    &driver_state,
                                    &peer_exclusions,
                                    &metrics,
                                ).await;
                            },
                            SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                                let bucket_count = {
                                    let mut state_guard = driver_state.lock().await;
                                    state_guard.track_connection(connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                                    state_guard.ip_bucket_connection_count(connection_id)
                                };
                                if let Some((bucket, count)) = bucket_count {
                                    if req_res_limits.ip_bucket_connection_limit != 0
                                        && count > req_res_limits.ip_bucket_connection_limit
                                    {
                                        metrics.ip_bucket_connection_rejected.increment();
                                        warn!(
                                            peer = %peer_id,
                                            connection_id = ?connection_id,
                                            bucket = %bucket,
                                            count,
                                            limit = req_res_limits.ip_bucket_connection_limit,
                                            "Closing connection because IP bucket connection cap is exceeded"
                                        );
                                        swarm.close_connection(connection_id);
                                    }
                                }
                                debug!("SEvent: {peer_id} is new friend via: {endpoint:?}");
                            },
                            SwarmEvent::ConnectionClosed { connection_id, peer_id, endpoint, cause, .. } => {
                                let eperm = cause
                                    .as_ref()
                                    .is_some_and(|c| chain_has_permission_denied(c));
                                {
                                    let mut state_guard = driver_state.lock().await;
                                    let _ = state_guard.lost_connection(connection_id);
                                    if !state_guard.peer_connections.contains_key(&peer_id) {
                                        peer_gen2_inbound.remove(&peer_id);
                                    }
                                }
                                if let Some(cause) = &cause {
                                    debug!("SEvent: friendship ended with {peer_id} via: {endpoint:?}. cause: {cause:?}");
                                } else {
                                    debug!("SEvent: friendship ended by us with {peer_id} via: {endpoint:?}.");
                                }
                                if eperm {
                                    if let Some(ip) = endpoint.get_remote_address().ip_addr() {
                                        record_exclusion_outcome(
                                            &mut swarm,
                                            &driver_state,
                                            &peer_exclusions,
                                            &metrics,
                                            peer_exclusions.record_permission_denied(
                                                endpoint.get_remote_address()
                                            ),
                                            &[peer_id],
                                        )
                                        .await;
                                        if !peer_exclusions.is_ip_excluded(&ip) {
                                            trace!("PermissionDenied on {ip} stayed below IP exclusion threshold");
                                        }
                                    }
                                }
                            },
                            SwarmEvent::IncomingConnectionError { local_addr, send_back_addr, error, .. } => {
                               trace!("SEvent: Failed incoming connection from {} to {}: {}",
                               send_back_addr, local_addr, error);

                               // When connection limits are reached, randomly prune inbound connections
                               if let ListenError::Denied { cause } = error {
                                   metrics.incoming_connections_blocked_by_limits.increment();
                                   if let Some(prune_factor) = prune_inbound_size {
                                       if let Ok(_exceeded) = cause.downcast::<libp2p::connection_limits::Exceeded>() {
                                           driver_state.lock().await.prune_inbound_connections(metrics.clone(), &mut swarm, prune_factor);
                                       }
                                   }
                               }
                            },
                            SwarmEvent::Behaviour(NockchainEvent::RequestResponse(Message { connection_id , peer, message })) => {
                                trace!("SEvent: received RequestResponse");
                                let _span = tracing::debug_span!("SwarmEvent::Behavior(NockchainEvent::RequestResponse(...))").entered();
                                let swarm_tx_clone = swarm_tx.clone();
                                let mut equix_builder_clone = equix_builder.clone();
                                let local_peer_id = *swarm.local_peer_id();
                                // We have to dup and move a handle back into `handle` to propitiate the borrow checker
                                let traffic_clone = traffic_cop.clone();
                                let metrics = metrics.clone();
                                let state_arc = Arc::clone(&driver_state); // Clone the Arc, not the MessageTracker
                                let peer_exclusions_clone = peer_exclusions.clone();
                                join_set.spawn("handle_request_response".to_string(), async move {
                                    gen2::handle_request_response(peer, connection_id, message, swarm_tx_clone, &mut equix_builder_clone, local_peer_id, traffic_clone, metrics.clone(), state_arc, req_res_limits, peer_exclusions_clone).await
                                });
                            },
                            SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                                OutboundFailure { peer, request_id, error, .. }
                            )) => {
                                let mut swarm_actions =
                                    SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
                                gen2::handle_outbound_request_failure_with_dispatcher(
                                    &mut swarm_actions,
                                    Arc::clone(&driver_state),
                                    metrics.clone(),
                                    *swarm.local_peer_id(),
                                    &mut equix_builder,
                                    peer_exclusions.clone(),
                                    peer,
                                    request_id,
                                    error,
                                )
                                .await;
                            }
                            SwarmEvent::Behaviour(NockchainEvent::RequestResponse(InboundFailure { peer, error, .. })) => {
                                log_inbound_failure(peer, error, metrics.clone());
                            }
                            SwarmEvent::Behaviour(NockchainEvent::Ping(ping::Event{peer, connection, result})) => {
                                let mut state_guard = driver_state.lock().await;
                                let connection_address = state_guard.connection_address(connection);
                                match result {
                                    Ok(duration) => {
                                        state_guard.ping_succeeded(connection);
                                        if let Some(ip) = connection_address.as_ref().and_then(|addr| addr.ip_addr()) {
                                            peer_exclusions.record_positive_ip(ip);
                                        }
                                        log_ping_success(peer, connection_address, duration);
                                    }
                                    Err(error) => {
                                        let failures = state_guard.ping_failed(connection);
                                        log_ping_failure(peer, connection_address.clone(), error);
                                        drop(state_guard);
                                        if let Some(addr) = connection_address.as_ref() {
                                            let outcome = peer_exclusions.record_ping_failure(addr);
                                            record_exclusion_outcome(
                                                &mut swarm,
                                                &driver_state,
                                                &peer_exclusions,
                                                &metrics,
                                                outcome,
                                                &[peer],
                                            ).await;
                                        }
                                        if failures >= failed_pings_before_close {
                                            if let Some(ip) = connection_address.and_then(|c| c.ip_addr()) {
                                                info!("Closing connection to {peer} on {ip} after {failures} failed pings.");
                                            } else {
                                                info!("Closing connection to {peer} after {failures} failed pings.");
                                            }
                                            swarm.close_connection(connection);
                                        }
                                    }
                                }
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => {
                                handle_outgoing_connection_error(
                                    &mut swarm,
                                    &driver_state,
                                    &peer_exclusions,
                                    &metrics,
                                    error
                                ).await;
                            },
                            SwarmEvent::IncomingConnection {
                                local_addr,
                                send_back_addr,
                                connection_id,
                                ..
                            } => {
                                debug!("SEvent: Incoming connection from {local_addr:?} to {send_back_addr:?} with {connection_id:?}");
                            },
                            SwarmEvent::Dialing { peer_id, connection_id } => {
                                debug!("SEvent: Dialing {peer_id:?} {connection_id}");
                            },
                            _ => {
                                // Handle other swarm events
                                trace!("SEvent: other swarm event {:?}", event);
                            }
                        }
                    },
                    Some(swarm_action) = swarm_rx.recv() => {
                        buffered_swarm_actions.push_back(swarm_action);
                    },
                    _ = kad_bootstrap.tick() => {
                        // If we don't have any peers, we should retry dialing our initial peers
                        if let Err(NoKnownPeers())= swarm.behaviour_mut().kad.bootstrap() {
                            if redial_initial_peers(
                                &mut swarm,
                                &initial_peers,
                                &backbone_peers,
                                &mut backbone_cursor,
                                backbone_dial_count,
                                "kademlia_bootstrap_no_known_peers",
                            )? {
                                info!("Failed to bootstrap: {}", NoKnownPeers());
                            }
                        }
                    },
                    _ = initial_peer_redial.tick() => {
                        if redial_initial_peers(
                            &mut swarm,
                            &initial_peers,
                            &backbone_peers,
                            &mut backbone_cursor,
                            backbone_dial_count,
                            "startup_zero_peer_window",
                        )? {
                            // Still disconnected: grow the backoff up to the cap and keep
                            // retrying forever. reset_after must be called every tick to
                            // hold the backoff cadence (the interval's own period would
                            // otherwise resume at INITIAL_PEER_REDIAL_INTERVAL).
                            initial_peer_redial_backoff =
                                (initial_peer_redial_backoff * 2).min(INITIAL_PEER_REDIAL_MAX_INTERVAL);
                            initial_peer_redial.reset_after(initial_peer_redial_backoff);
                            debug!(
                                backoff_secs = initial_peer_redial_backoff.as_secs(),
                                "Initial peer redial tick fired while disconnected"
                            );
                        } else if initial_peer_redial_backoff != INITIAL_PEER_REDIAL_INTERVAL {
                            // Connected (or nothing to dial): reset the backoff so a future
                            // disconnect starts retrying promptly again.
                            initial_peer_redial_backoff = INITIAL_PEER_REDIAL_INTERVAL;
                            initial_peer_redial.reset_after(INITIAL_PEER_REDIAL_INTERVAL);
                        }
                    },
                    _ = force_peer_dial.tick() => {
                        debug!("Force dialing peers");
                        dial_peers(&mut swarm, &force_peers)?;
                    },
                    _ = gen2_batch_flush.tick(), if req_res_gen2_send_enabled => {
                        let local_peer_id = *swarm.local_peer_id();
                        let peers_to_flush: Vec<_> = pending_gen2_batches.keys().copied().collect();
                        for peer_id in peers_to_flush {
                            let should_flush = pending_gen2_batches
                                .get_mut(&peer_id)
                                .is_some_and(gen2::PendingGen2Batch::should_flush_on_tick);
                            if !should_flush {
                                continue;
                            }
                            if let Some(pending_batch) = pending_gen2_batches.get(&peer_id) {
                                gen2::log_pending_gen2_batch_flush(
                                    &peer_id,
                                    gen2::PendingBatchFlushReason::CoalesceTick,
                                    pending_batch,
                                    req_res_limits,
                                );
                            }
                            if let Some(flushed_batch) = gen2::take_pending_batch_request(
                                &mut pending_gen2_batches,
                                peer_id,
                                &local_peer_id,
                                &mut equix_builder,
                            )? {
                                gen2::send_outbound_request_now(
                                    &mut swarm,
                                    &driver_state,
                                    &metrics,
                                    flushed_batch,
                                )
                                .await;
                            }
                            gen2::update_pending_batch_metrics(&metrics, &pending_gen2_batches);
                        }
                    },
                    _ = reset_request_counts.tick() => {
                        trace!("Resetting request counts");
                        driver_state.lock().await.reset_requests();
                    },
                    Some(result) = join_set.join_next() => {
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                match &e {
                                    NockAppError::OneShotRecvError(_) => {
                                        metrics.oneshot_recv_error_total.increment();
                                        warn!(error = %e, "Background req-res task lost a oneshot response");
                                    }
                                    _ => {
                                        error!("Task returned error: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Task error: {:?}", e);
                            }
                        }
                    },
                }
            }
        })
    })
}

// fn emit_fail2ban(peer_ip: u128) -> Result<(), NockAppError> {
//     // get peer ip address
//     let peer_ip = peer_id.to_base58();
// }
//
async fn send_timer_poke(
    guard: tokio::sync::OwnedMutexGuard<()>,
    traffic_cop: traffic_cop::TrafficCop,
    metrics: Arc<NockchainP2PMetrics>,
) -> Result<(), NockAppError> {
    let mut slab = NounSlab::new();
    let timer_noun = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"timer")), D(0)]);
    slab.set_root(timer_noun);
    let wire = nockapp::drivers::timer::TimerWire::Tick.to_wire();
    let enable_fut = Box::pin(async { true });
    let (timing, timing_rx) = tokio::sync::oneshot::channel();
    traffic_cop
        .poke_high_priority(None, wire, slab, enable_fut, Some(timing))
        .await?;
    // The poke itself already completed above; a dropped timing channel
    // only costs the metric, not the task's success.
    if let Ok(elapsed) = timing_rx.await {
        let _ = metrics.timer_poke_time.add_timing(&elapsed);
    }
    drop(guard);
    Ok(())
}

fn extract_gossip_effect_payload(noun_slab: &NounSlab) -> Result<NounSlab, NockAppError> {
    let space = noun_slab.noun_space();
    let gossip_cell = unsafe { *noun_slab.root() }
        .in_space(&space)
        .as_cell()?
        .tail()
        .as_cell()?;
    let version = gossip_cell
        .head()
        .as_atom()
        .map_err(|_| NockAppError::OtherError(String::from("Malformed gossip fact version")))?
        .as_u64()?;
    if version != FACT_POKE_VERSION {
        warn!(
            version,
            expected = FACT_POKE_VERSION,
            "Rejecting gossip effect with unsupported fact version"
        );
        return Err(NockAppError::OtherError(format!(
            "Unsupported gossip fact version {version}"
        )));
    }

    let mut payload_slab = NounSlab::new();
    payload_slab.copy_into(gossip_cell.tail().noun(), &space);
    Ok(payload_slab)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PrefetchConfig {
    pub enabled: bool,
    pub window_initial: u8,
    pub window_max: u8,
    pub max_inflight_per_peer: u8,
    pub kernel_demand_threshold: u64,
}

impl PrefetchConfig {
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            window_initial: 0,
            window_max: 0,
            max_inflight_per_peer: 0,
            kernel_demand_threshold: u64::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LocalPeerAbuseKind {
    BadRequestPow,
    InboundAdmissionRejected,
    IpBucketRequestCap,
    GossipBucketCap,
    LiarPeer,
    LiarBlockId,
    ResponseValidationMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalPeerAbuseSeverity {
    Weak,
    Strong,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn record_local_peer_abuse(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    peer_exclusions: &PeerExclusions,
    peer_id: PeerId,
    connection_id: Option<ConnectionId>,
    address: Option<Multiaddr>,
    kind: LocalPeerAbuseKind,
    severity: LocalPeerAbuseSeverity,
    block_peer: bool,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    record_local_peer_abuse_with_dispatcher(
        &mut swarm_actions, driver_state, metrics, peer_exclusions, peer_id, connection_id,
        address, kind, severity, block_peer,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_local_peer_abuse_with_dispatcher(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    peer_exclusions: &PeerExclusions,
    peer_id: PeerId,
    connection_id: Option<ConnectionId>,
    address: Option<Multiaddr>,
    kind: LocalPeerAbuseKind,
    severity: LocalPeerAbuseSeverity,
    block_peer: bool,
) -> Result<(), NockAppError> {
    metrics.local_peer_abuse_recorded.increment();
    if peer_exclusions.record_peer_request_failure(peer_id) {
        metrics.request_peer_cooldowns_created.increment();
    }

    let resolved_address = match (address, connection_id) {
        (Some(address), _) => Some(address),
        (None, Some(connection_id)) => driver_state.lock().await.connection_address(connection_id),
        (None, None) => None,
    };

    if severity == LocalPeerAbuseSeverity::Strong {
        if let Some(address) = resolved_address.as_ref() {
            let outcome = peer_exclusions.record_peer_misbehavior(address, peer_id);
            if outcome.address_cooldown.is_some() || outcome.ip_exclusion.is_some() {
                swarm_actions
                    .dispatch(SwarmAction::RecordExclusionOutcome {
                        outcome,
                        related_peers: vec![peer_id],
                    })
                    .await
                    .map_err(|_| {
                        NockAppError::OtherError(String::from(
                            "Failed to send exclusion outcome action",
                        ))
                    })?;
            }
        } else {
            debug!(
                peer = %peer_id,
                abuse_kind = ?kind,
                "Local peer abuse had no trusted address context"
            );
        }
    }

    if block_peer {
        swarm_actions
            .dispatch(SwarmAction::BlockPeer { peer_id })
            .await
            .map_err(|_| {
                NockAppError::OtherError(String::from("Failed to send SwarmAction request"))
            })?;
    }

    Ok(())
}

async fn handle_effect_with_dispatcher(
    mut noun_slab: NounSlab,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    connected_peers: Vec<PeerId>,
    bundle_requests_enabled: bool,
    prefetch_config: PrefetchConfig,
    req_res_limits: gen2::ReqResRuntimeLimits,
    driver_state: Arc<Mutex<P2PState>>,
    metrics: Arc<NockchainP2PMetrics>,
    peer_exclusions: PeerExclusions,
) -> Result<(), NockAppError> {
    match EffectType::from_noun_slab(&noun_slab) {
        EffectType::Gossip => {
            let tail_slab = extract_gossip_effect_payload(&noun_slab)?;

            let gossip_kind = {
                let gossip_space = tail_slab.noun_space();
                let gossip_noun = unsafe { *tail_slab.root() };
                gossip_noun
                    .in_space(&gossip_space)
                    .as_cell()
                    .map(|data_cell| {
                        if data_cell.head().eq_bytes(b"heard-block") {
                            "heard-block"
                        } else if data_cell.head().eq_bytes(b"heard-tx") {
                            "heard-tx"
                        } else {
                            "unknown"
                        }
                    })
                    .unwrap_or("unknown")
            };
            let is_heard_block_gossip = gossip_kind == "heard-block";
            // Clear the serve caches on a new heaviest block (local-state
            // bookkeeping, independent of whether we re-broadcast) and, while
            // holding the lock, read whether we should suppress the outgoing
            // gossip because we are behind tip.
            let (suppress_outgoing_gossip, behind_tip_estimate) = {
                let mut state_guard = driver_state.lock().await;
                if is_heard_block_gossip {
                    trace!("Gossip effect for heard-block, clearing block and elders cache");
                    state_guard.block_cache.clear();
                    state_guard.elders_cache.clear();
                    state_guard.clear_elders_negative_cache();
                }
                (
                    state_guard.should_suppress_outgoing_gossip(),
                    state_guard.catch_up_signal().behind_tip_estimate(),
                )
            };

            if suppress_outgoing_gossip {
                // We are demonstrably behind tip (SyncMode::CatchingUp). The
                // node is intentionally quiet: no historic block rebroadcasts,
                // local tx submission gossip, or mining output until catch-up
                // exits.
                trace!(
                    behind_tip_estimate, gossip_kind,
                    "Suppressing outgoing gossip while catching up"
                );
                metrics.gossip_suppressed_behind_tip_total.increment();
            } else {
                let gossip_request = NockchainRequest::new_gossip(&tail_slab);
                debug!("Gossiping to {} peers", connected_peers.len());
                for peer_id in connected_peers.clone() {
                    let gossip_request_clone = gossip_request.clone();
                    swarm_actions
                        .dispatch(SwarmAction::SendRequest {
                            peer_id,
                            request: gossip_request_clone,
                            request_context: None,
                        })
                        .await
                        .map_err(|_e| {
                            NockAppError::OtherError(String::from("Failed to send gossip request"))
                        })?;
                }
            }
        }
        EffectType::Request => {
            trace!(
                effect = %request_effect_trace_summary(&noun_slab),
                connected_peer_count = connected_peers.len(),
                "Handling kernel request effect"
            );
            let mut is_limited_request = false;
            let mut preserve_peer_order = false;
            let mut preferred_peers = Vec::new();
            let mut requested_block_height = None;
            let mut raw_tx_reopen_id = None;
            let mut elders_cooldown_key = None;
            let mut request_desc: String;

            let target_peers = {
                let space = noun_slab.noun_space();
                let request_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let request_body = request_cell.tail().as_cell()?;
                let request_type = request_body.head().as_atom()?.as_direct()?;
                let request_tag = request_type.data();

                request_desc = if request_tag == tas!(b"block") {
                    String::from("block")
                } else if request_tag == tas!(b"raw-tx") {
                    String::from("raw-tx")
                } else {
                    format!("tag-{request_tag}")
                };

                let target_peers = if request_tag == tas!(b"block") {
                    let block_cell = request_body.tail().as_cell()?;
                    if block_cell.head().eq_bytes(b"elders") {
                        request_desc = String::from("block/elders");
                        // Extract peer ID from elders request
                        let elders_cell = block_cell.tail().as_cell()?;
                        let elders_block_id_noun = elders_cell.head().noun();
                        let peer_id_atom = elders_cell.tail().as_atom()?;
                        if let Ok(bytes) = peer_id_atom.to_bytes_until_nul() {
                            if let Ok(peer_id) = PeerId::from_bytes(&bytes) {
                                if let Ok(block_id) = tip5_hash_to_base58_stack(
                                    &mut noun_slab, elders_block_id_noun, &space,
                                ) {
                                    elders_cooldown_key =
                                        Some(format!("{block_id}:{}", peer_id.to_base58()));
                                }
                                vec![peer_id]
                            } else {
                                is_limited_request = true;
                                connected_peers.clone()
                            }
                        } else {
                            is_limited_request = true;
                            connected_peers.clone()
                        }
                    } else {
                        // Keep a stable peer subset for adjacent height requests so the
                        // driver can accumulate a per-peer block batch without
                        // spraying each height at a different peer.
                        preserve_peer_order = true;
                        if block_cell.head().eq_bytes(b"by-height") {
                            request_desc = String::from("block/by-height");
                            requested_block_height = Some(block_cell.tail().as_atom()?.as_u64()?);
                        }
                        connected_peers.clone()
                    }
                } else {
                    connected_peers.clone()
                };

                if request_tag == tas!(b"raw-tx") {
                    if let Ok(raw_tx_cell) = request_body.tail().as_cell() {
                        if raw_tx_cell.head().eq_bytes(b"by-id") {
                            request_desc = String::from("raw-tx/by-id");
                            is_limited_request = true;
                            trace!(
                                "Requesting raw transaction by ID, reopening both seen and processing gates"
                            );
                            let raw_tx_id = raw_tx_cell.tail();
                            let tx_id = tip5_hash_to_base58_stack(
                                &mut noun_slab,
                                raw_tx_id.noun(),
                                &space,
                            )?;
                            raw_tx_reopen_id = Some(tx_id);
                        }
                    }
                }
                target_peers
            };

            if let Some(cooldown_key) = elders_cooldown_key.as_deref() {
                let mut state_guard = driver_state.lock().await;
                if !state_guard.should_send_elders_request(
                    cooldown_key,
                    std::time::Instant::now(),
                    crate::p2p_state::ELDERS_REQUEST_COOLDOWN,
                ) {
                    drop(state_guard);
                    debug!(
                        cooldown_key,
                        "Suppressing duplicate elders request inside cooldown window"
                    );
                    return Ok(());
                }
            }

            if let Some(tx_id) = raw_tx_reopen_id {
                let mut state_guard = driver_state.clone().lock_owned().await;
                state_guard.seen_txs.remove(&tx_id);
                state_guard.cancel_processing_tx(&tx_id);
                preferred_peers = state_guard.get_peers_for_tx_id(&tx_id);
            }

            // Phase 2: a deferred-buffer hit may satisfy a kernel by-height
            // request only when the feature is enabled and the requested
            // height is already at the driver's frontier. Future buffered
            // heights still fall through to the outbound request path; they
            // need a later `%seen %block` frontier advance before local flush
            // can deliver them.
            if let Some(height) = requested_block_height {
                {
                    let mut state_guard = driver_state.lock().await;
                    state_guard.note_kernel_block_height_requested(height);
                }
                // Snapshot deferred-buffer presence, prefetch coverage, and
                // sync mode in one guard acquisition so the suppression
                // decision is consistent.
                let (ready_cache_hit, deferred_cache_hit, prefetch_covers, mode, frontier) = {
                    let state_guard = driver_state.lock().await;
                    (
                        state_guard.has_ready_deferred_block_at_height(height),
                        state_guard.has_deferred_block_at_height(height),
                        state_guard.is_prefetch_inflight_covering_height(height),
                        state_guard.catch_up_signal().mode(),
                        state_guard.first_negative,
                    )
                };
                if prefetch_config.enabled && ready_cache_hit {
                    metrics.prefetch_cache_hits_total.increment();
                    trace!(
                        height,
                        "Queueing deferred flush for buffered block-by-height request at frontier"
                    );
                    swarm_actions
                        .dispatch(SwarmAction::FlushDeferredHeardBlocks)
                        .await
                        .map_err(|_| {
                            NockAppError::OtherError(String::from(
                                "Failed to queue deferred heard-block flush",
                            ))
                        })?;
                    return Ok(());
                }
                if prefetch_config.enabled && deferred_cache_hit {
                    trace!(
                        height,
                        "Buffered block-by-height request is ahead of frontier; dispatching outbound request"
                    );
                }
                if prefetch_config.enabled && prefetch_covers {
                    metrics.prefetch_singleton_suppressed_total.increment();
                    trace!(
                        height,
                        "Suppressed outbound block-by-height request: prefetch already covers height"
                    );
                    return Ok(());
                }
                metrics.prefetch_cache_misses_total.increment();

                // Phase 4: if we're behind tip and prefetch is enabled, replace
                // the singleton outbound with a windowed range request that
                // includes `height`. Subsequent kernel singletons within the
                // window will hit the prefetch_covers branch above and ride
                // the prefetch response.
                let kernel_demand_depth = if frontier > 0 && height >= frontier {
                    height.saturating_sub(frontier).saturating_add(1)
                } else {
                    0
                };
                let kernel_demand_prefetch =
                    kernel_demand_depth >= prefetch_config.kernel_demand_threshold;
                let prefetch_eligible = prefetch_config.enabled
                    && height > 0
                    && (matches!(
                        mode,
                        crate::catch_up::SyncMode::CatchingUp | crate::catch_up::SyncMode::Cold
                    ) || kernel_demand_prefetch)
                    && prefetch_config.window_initial > 0
                    && prefetch_config.window_max > 0;
                if prefetch_eligible {
                    let window_selection = select_prefetch_window(
                        height,
                        prefetch_config,
                        Arc::clone(&driver_state),
                        req_res_limits,
                    )
                    .await;
                    let selection = pick_prefetch_peer(
                        &connected_peers,
                        Arc::clone(&driver_state),
                        prefetch_config.max_inflight_per_peer,
                        height,
                        window_selection.window,
                    )
                    .await;
                    match selection {
                        PrefetchPeerSelection::Selected(prefetch_peer) => {
                            let peer_id = prefetch_peer.peer_id;
                            let window = prefetch_peer.window;
                            match crate::messages::block_range_with_txs_request_message(
                                height, window,
                            ) {
                                Ok(range_message) => {
                                    trace!(
                                        height,
                                        window,
                                        peer = %peer_id,
                                        mode = mode.as_str(),
                                        frontier,
                                        kernel_demand_depth,
                                        kernel_demand_threshold =
                                            prefetch_config.kernel_demand_threshold,
                                        estimated_response_bytes_per_block =
                                            window_selection.estimated_response_bytes_per_block,
                                        estimate_source = window_selection.estimate_source,
                                        target_response_bytes = window_selection.target_response_bytes,
                                        response_budget_bytes = window_selection.response_budget_bytes,
                                        probe = prefetch_peer.probe,
                                        "Issuing catch-up prefetch in place of singleton block-by-height"
                                    );
                                    metrics.prefetch_issued_total.increment();
                                    metrics.prefetch_peer_selected_total.increment();
                                    if prefetch_peer.probe {
                                        metrics.prefetch_peer_probe_total.increment();
                                    }
                                    swarm_actions
                                        .dispatch(SwarmAction::QueueKernelRequest {
                                            peer_id,
                                            request_message: range_message,
                                        })
                                        .await
                                        .map_err(|_| {
                                            NockAppError::OtherError(String::from(
                                                "Failed to dispatch catch-up prefetch request",
                                            ))
                                        })?;
                                    return Ok(());
                                }
                                Err(err) => {
                                    warn!(
                                        height,
                                        window,
                                        error = %err,
                                        "Failed to build catch-up prefetch message; falling back to singleton"
                                    );
                                }
                            }
                        }
                        PrefetchPeerSelection::Throttled => {
                            metrics.prefetch_throttled_total.increment();
                            trace!(
                                height,
                                "Skipping catch-up prefetch: candidate peers over bandwidth cap"
                            );
                        }
                        PrefetchPeerSelection::NoCandidate { saw_gen2 } => {
                            metrics.prefetch_no_eligible_peer_total.increment();
                            if !saw_gen2 {
                                metrics.prefetch_peer_no_gen2_range_peer_total.increment();
                            }
                        }
                    }
                }
            }
            let max_request_peers = if requested_block_height.is_some() {
                1
            } else if is_limited_request {
                2
            } else {
                8
            };
            let request_peers = if let Some(height) = requested_block_height {
                let mut state_guard = driver_state.lock().await;
                let attempted_peers = state_guard
                    .get_block_height_attempted_peers(height)
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                let attempted_peer_count = attempted_peers.len();
                let target_peer_count = target_peers.len();
                let mut candidate_peers = target_peers.clone();
                candidate_peers.retain(|peer_id| !attempted_peers.contains(peer_id));
                let mut recycled_attempts = false;
                if candidate_peers.is_empty() && !target_peers.is_empty() {
                    state_guard.clear_block_height_attempted_peers(height);
                    recycled_attempts = true;
                    candidate_peers = target_peers;
                }
                let candidate_peer_count = candidate_peers.len();
                let selected_peers = state_guard.select_request_peers_with_preferences(
                    candidate_peers, max_request_peers, &peer_exclusions, preserve_peer_order,
                    &preferred_peers,
                );
                if !selected_peers.is_empty() {
                    state_guard
                        .track_block_height_attempted_peers(height, selected_peers.iter().copied());
                }
                trace!(
                        height,
                        max_request_peers,
                        preserve_peer_order,
                        target_peer_count,
                        attempted_peer_count,
                        candidate_peer_count,
                        selected_peer_count = selected_peers.len(),
                        recycled_attempts,
                    selected_peers = ?selected_peers,
                    "Selected outbound peers for block-by-height request"
                );
                selected_peers
            } else {
                let state_guard = driver_state.lock().await;
                state_guard.select_request_peers_with_preferences(
                    target_peers, max_request_peers, &peer_exclusions, preserve_peer_order,
                    &preferred_peers,
                )
            };
            info!(
                "Sending {request_desc} request to {} peer(s): {:?}",
                request_peers.len(),
                request_peers
                    .iter()
                    .map(|p| p.to_base58())
                    .collect::<Vec<_>>()
            );
            // Classic jam of whatever the kernel emitted. This is what
            // pre-bundle peers expect and what we fall back to per-peer when
            // the memo says a peer already rejected a bundle request.
            let classic_message = ByteBuf::from(noun_slab.jam().as_ref());
            // Bundle-upgraded jam: only populated when the feature flag is
            // on and the kernel effect is a block-by-height. Peers on the
            // `non_bundle_capable_peers` memo get the classic message even
            // when this is set.
            let bundle_message = match requested_block_height {
                Some(height) if bundle_requests_enabled => {
                    match block_with_txs_by_height_request_message(height) {
                        Ok(bytes) => Some(bytes),
                        Err(err) => {
                            warn!(
                                height,
                                error = %err,
                                "Failed to build bundle request message; falling back to classic request for all peers"
                            );
                            None
                        }
                    }
                }
                _ => None,
            };
            let non_bundle_peers = if bundle_message.is_some() {
                driver_state
                    .lock()
                    .await
                    .non_bundle_capable_peers_snapshot()
            } else {
                BTreeSet::new()
            };
            for peer_id in request_peers {
                let request_message = match (&bundle_message, non_bundle_peers.contains(&peer_id)) {
                    (Some(bundle), false) => bundle.clone(),
                    _ => classic_message.clone(),
                };
                swarm_actions
                    .dispatch(SwarmAction::QueueKernelRequest {
                        peer_id,
                        request_message,
                    })
                    .await
                    .map_err(|_e| {
                        NockAppError::OtherError(String::from("Failed to send SwarmAction request"))
                    })?;
            }
        }
        EffectType::LiarPeer => {
            let peer_id = {
                let space = noun_slab.noun_space();
                let effect_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let liar_peer_cell = effect_cell.tail().as_cell().map_err(|_| {
                    NockAppError::IoError(std::io::Error::other(
                        "Expected peer ID cell in liar-peer effect",
                    ))
                })?;
                let peer_id_atom = liar_peer_cell.head().as_atom().map_err(|_| {
                    NockAppError::IoError(std::io::Error::other(
                        "Expected peer ID atom in liar-peer effect",
                    ))
                })?;

                let bytes = peer_id_atom.to_bytes_until_nul().map_err(|_| {
                    NockAppError::IoError(std::io::Error::other(
                        "Invalid peer ID atom in liar-peer effect",
                    ))
                })?;

                let peer_id_str = String::from_utf8(bytes).map_err(|_| {
                    NockAppError::IoError(std::io::Error::other("Invalid UTF-8 in peer ID"))
                })?;

                PeerId::from_str(&peer_id_str).map_err(|_| {
                    NockAppError::IoError(std::io::Error::other("Invalid peer ID format"))
                })?
            };

            record_local_peer_abuse_with_dispatcher(
                swarm_actions,
                &driver_state,
                metrics.as_ref(),
                &peer_exclusions,
                peer_id,
                None,
                None,
                LocalPeerAbuseKind::LiarPeer,
                LocalPeerAbuseSeverity::Weak,
                true,
            )
            .await?;
        }
        EffectType::LiarBlockId => {
            let block_id_str = {
                let space = noun_slab.noun_space();
                let effect_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let liar_block_cell = effect_cell.tail().as_cell().map_err(|_| {
                    NockAppError::IoError(std::io::Error::other(
                        "Expected block ID cell in liar-block-id effect",
                    ))
                })?;
                let block_id = liar_block_cell.head().noun();
                tip5_hash_to_base58_stack(&mut noun_slab, block_id, &space)?
            };

            // Narrow the driver_state guard to just the state mutation.
            // holding it across the `swarm_tx.send(BlockPeer).await` loop
            // below would close a reentrant-through-channel cycle: only
            // the main libp2p select loop drains `swarm_tx`, and that same
            // loop takes `driver_state.lock().await` in ~8 of its branches.
            // Under load a full swarm_tx could force the sender to wait on
            // the select loop, which would itself be waiting on this lock.
            // See docs/incidents/lax1-freeze-20260417/driver-lock-audit.md.
            let peers_to_ban = {
                let mut state_guard = driver_state.lock().await;
                // Phase 5: drop any deferred-buffer entry for this block id
                // when the buffer slot needs a fresh fetch from a different
                // peer.
                if state_guard.invalidate_deferred_block_id(&block_id_str) {
                    metrics.prefetch_invalidated_total.increment();
                    trace!(
                        block_id = block_id_str,
                        "Invalidated deferred buffer entry on liar-block-id"
                    );
                }
                state_guard.process_bad_block_id_str(&block_id_str)
            };

            // Ban each peer that sent this block
            for peer_id in peers_to_ban {
                record_local_peer_abuse_with_dispatcher(
                    swarm_actions,
                    &driver_state,
                    metrics.as_ref(),
                    &peer_exclusions,
                    peer_id,
                    None,
                    None,
                    LocalPeerAbuseKind::LiarBlockId,
                    LocalPeerAbuseSeverity::Weak,
                    true,
                )
                .await?;
            }
        }
        EffectType::Track => {
            enum TrackEffect {
                Add {
                    block_id_str: String,
                    peer_id: PeerId,
                },
                Remove {
                    block_id_str: String,
                },
            }

            let track_effect = {
                let space = noun_slab.noun_space();
                let effect_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let track_cell = effect_cell.tail().as_cell()?;
                let action = track_cell.head();

                if action.eq_bytes(b"add") {
                    // Handle [%track %add block-id peer-id]
                    let data_cell = track_cell.tail().as_cell()?;
                    let block_id = data_cell.head().noun();
                    let block_id_str = tip5_hash_to_base58_stack(&mut noun_slab, block_id, &space)?;
                    let peer_id_atom = data_cell.tail().as_atom()?;

                    // Convert peer_id from base58 string to PeerId
                    let Ok(peer_id) = PeerId::from_noun(peer_id_atom.as_noun().noun(), &space)
                    else {
                        return Err(NockAppError::OtherError(String::from(
                            "Invalid peer ID format",
                        )));
                    };

                    TrackEffect::Add {
                        block_id_str,
                        peer_id,
                    }
                } else if action.eq_bytes(b"remove") {
                    // Handle [%track %remove block-id]
                    let block_id = track_cell.tail().noun();
                    let block_id_str = tip5_hash_to_base58_stack(&mut noun_slab, block_id, &space)?;
                    TrackEffect::Remove { block_id_str }
                } else {
                    return Err(NockAppError::IoError(std::io::Error::other(
                        "Invalid track action",
                    )));
                }
            };

            let mut state_guard = driver_state.lock().await;
            match track_effect {
                TrackEffect::Add {
                    block_id_str,
                    peer_id,
                } => {
                    let accepted_height =
                        state_guard.track_accepted_block_id_str_and_peer(block_id_str, peer_id);
                    if let Some(height) = accepted_height {
                        trace!(
                            peer_id = %peer_id,
                            height,
                            "Recorded peer-observed height after track add"
                        );
                    }
                }
                TrackEffect::Remove { block_id_str } => {
                    state_guard.remove_block_id_str(&block_id_str);
                }
            };
        }
        EffectType::Seen => {
            enum SeenEffect {
                Block {
                    block_id_str: String,
                    block_height: Option<u64>,
                },
                Tx {
                    tx_id_str: String,
                },
                Unknown,
            }

            let seen_effect = {
                let space = noun_slab.noun_space();
                let effect_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let seen_cell = effect_cell.tail().as_cell()?;
                let seen_type = seen_cell.head();

                if seen_type.eq_bytes(b"block") {
                    let seen_pq = seen_cell.tail().as_cell()?;
                    let block_id = seen_pq.head().as_cell()?;
                    let block_id_str = tip5_hash_to_base58_stack(
                        &mut noun_slab,
                        block_id.as_noun().noun(),
                        &space,
                    )
                    .expect("failed to convert block ID to base58");
                    let block_height = match seen_pq.tail().as_cell() {
                        Ok(block_height_unit_cell) => {
                            Some(block_height_unit_cell.tail().as_atom()?.as_u64()?)
                        }
                        Err(_) => None,
                    };
                    SeenEffect::Block {
                        block_id_str,
                        block_height,
                    }
                } else if seen_type.eq_bytes(b"tx") {
                    let tx_id = seen_cell.tail().as_cell()?;
                    let tx_id_str =
                        tip5_hash_to_base58_stack(&mut noun_slab, tx_id.as_noun().noun(), &space)
                            .expect("failed to convert tx ID to base58");
                    SeenEffect::Tx { tx_id_str }
                } else {
                    SeenEffect::Unknown
                }
            };

            match seen_effect {
                SeenEffect::Block {
                    block_id_str,
                    block_height,
                } => {
                    trace!("seen block id: {:?}", &block_id_str);
                    // The `state_guard` MUST be released before the
                    // `swarm_tx.send(FlushDeferredHeardBlocks)` below. The
                    // inner `{ ... }` block expression provides that scope.
                    // Holding the guard across that send would close the same
                    // reentrant-through-channel cycle the audit flagged at
                    // the LiarBlockId arm. See
                    // docs/incidents/lax1-freeze-20260417/driver-lock-audit.md.
                    let should_flush_deferred = {
                        let mut state_guard = driver_state.lock().await;
                        state_guard.finish_processing_block_seen(&block_id_str);
                        #[cfg(test)]
                        gen2::checkpoint_route_trace(
                            "seen-effect",
                            &block_id_str,
                            format!("seen_blocks={}", state_guard.seen_blocks.len()),
                        );

                        if let Some(block_height) = block_height {
                            if state_guard.first_negative <= block_height {
                                metrics.highest_block_height_seen.swap(block_height as f64);
                                state_guard.first_negative = block_height + 1;
                                trace!(
                                    "Setting state_guard.first_negative to {:?}",
                                    state_guard.first_negative
                                );
                                state_guard.note_frontier_advanced();

                                // Check if we should clear the tx cache
                                if block_height
                                    >= state_guard.last_tx_cache_clear_height
                                        + state_guard.seen_tx_clear_interval
                                {
                                    debug!(
                                        "Clearing seen_txs cache at block height {}",
                                        block_height
                                    );
                                    debug!("Cache before clearing: {:?}", state_guard.seen_txs);
                                    state_guard.seen_txs.clear();
                                    state_guard.last_tx_cache_clear_height = block_height;
                                }
                            }
                        }
                        state_guard.has_ready_deferred_heard_blocks()
                    };
                    if should_flush_deferred {
                        swarm_actions
                            .dispatch(SwarmAction::FlushDeferredHeardBlocks)
                            .await
                            .map_err(|_| {
                                NockAppError::OtherError(String::from(
                                    "Failed to queue deferred heard-block flush",
                                ))
                            })?;
                    }
                }
                SeenEffect::Tx { tx_id_str } => {
                    let mut state_guard = driver_state.lock().await;
                    trace!("seen tx id: {:?}", &tx_id_str);
                    state_guard.finish_processing_tx_seen(&tx_id_str);
                    state_guard.remove_tx_id_hint(&tx_id_str);
                    state_guard.clear_speculative_tx_prefetch(&tx_id_str);
                }
                SeenEffect::Unknown => {}
            }
        }
        EffectType::Unknown => {
            //  This isn't unexpected - any effect that this driver doesn't handle
            //  will hit this case.
        }
    }
    Ok(())
}

async fn log_peer_status(
    swarm: &mut Swarm<NockchainBehaviour>,
    metrics: &NockchainP2PMetrics,
    peer_exclusions: &PeerExclusions,
    driver_state: &Arc<Mutex<P2PState>>,
) -> usize {
    let expired = peer_exclusions.expire();
    for _ in 0..expired.ips {
        metrics.ip_exclusions_expired.increment();
    }
    let _ = metrics
        .ip_exclusions_active
        .swap(peer_exclusions.active_ip_exclusion_count() as f64);
    let _ = metrics
        .address_cooldowns_active
        .swap(peer_exclusions.active_address_cooldown_count() as f64);

    let connected_peer_count = {
        let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
        let peer_count = connected_peers.len();

        if peer_count == 0 {
            warn!(
                connected_peers = peer_count,
                peers = ?connected_peers.iter().map(|p| p.to_base58()).collect::<Vec<_>>(),
                "No current peers connected!"
            );
        } else {
            info!(
                connected_peers = peer_count,
                peers = ?connected_peers.iter().map(|p| p.to_base58()).collect::<Vec<_>>(),
                "Current peer status"
            );
        }

        let _ = metrics.active_peer_connections.swap(peer_count as f64);
        peer_count
    };

    // Count peers in the routing table by iterating through k-buckets
    let mut routing_table_size = 0;
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        routing_table_size += bucket.num_entries();
    }

    if routing_table_size == 0 {
        warn!(
            routing_table_size = routing_table_size,
            "Routing table is empty!"
        );
    } else {
        info!(
            routing_table_size = routing_table_size,
            "Routing table has {} entries", routing_table_size
        );
    };
    observe_kad_cardinality_and_exclude(swarm, driver_state, peer_exclusions, metrics).await;
    prune_excluded_swarm_state(swarm, driver_state, peer_exclusions, metrics).await;
    connected_peer_count
}

fn dial_peers(
    swarm: &mut Swarm<NockchainBehaviour>,
    peers: &[Multiaddr],
) -> Result<(), NockAppError> {
    let mut rng = rand::rng();

    let cloned_peers: &mut [libp2p::Multiaddr] = &mut peers.to_vec();
    cloned_peers.shuffle(&mut rng);

    for peer in cloned_peers {
        let peer = peer.clone();
        debug!("Dialing peer: {}", peer);
        let _ = swarm.dial(peer.clone()).map_err(log_dial_error);
    }
    Ok(())
}

/// Walks an error's `source()` chain looking for an [`std::io::Error`] of
/// kind [`PermissionDenied`](std::io::ErrorKind::PermissionDenied). This is
/// how a firewall-blocked egress surfaces: the quinn UDP socket's `sendmsg`
/// returns `EPERM`, which the QUIC transport reports as a `PermissionDenied`
/// io error inside `DialError::Transport`.
fn chain_has_permission_denied(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::PermissionDenied {
                return true;
            }
        }
        cur = e.source();
    }
    false
}

/// The peer id encoded as the trailing `/p2p/<peer-id>` of a multiaddr, if
/// any (Kademlia keys its routing table by it).
fn p2p_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(peer_id) => Some(peer_id),
        _ => None,
    })
}

fn remove_stale_peer_address(
    swarm: &mut Swarm<NockchainBehaviour>,
    peer_id: PeerId,
    address: &Multiaddr,
) {
    let address_without_peer_id = multiaddr_without_p2p(address);
    let behaviour = swarm.behaviour_mut();
    behaviour
        .kad
        .remove_address(&peer_id, &address_without_peer_id);
    behaviour
        .peer_store
        .store_mut()
        .remove_address(&peer_id, &address_without_peer_id);
    behaviour
        .peer_store
        .store_mut()
        .remove_address(&peer_id, address);
}

async fn handle_outgoing_connection_error(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
    error: DialError,
) {
    match &error {
        // The host answered with a different identity than Kademlia
        // advertised. Poisoned peers exploit this with many ports / fresh
        // ids on one IP, so we never connect but keep retrying forever.
        DialError::WrongPeerId { obtained, address } => {
            let obtained = *obtained;
            let expected = p2p_peer_id(address);
            if address.ip_addr().is_none() {
                warn!("WrongPeerId for {address} had no IP component; cannot exclude by IP");
                return;
            }
            metrics.wrong_peer_id_observed.increment();
            if driver_state
                .lock()
                .await
                .peer_has_connection_at_address(&obtained, address)
            {
                if let Some(expected) = expected {
                    warn!(
                        "Wrong peer id from stale address {address}: obtained connected peer {obtained}, expected {expected}; removing stale address without IP ban"
                    );
                    remove_stale_peer_address(swarm, expected, address);
                } else {
                    warn!(
                        "Wrong peer id from {address}: obtained already-connected peer {obtained}; skipping IP ban"
                    );
                }
                return;
            }
            let mut ids: Vec<PeerId> = expected.into_iter().collect();
            ids.push(obtained);
            let outcome = peer_exclusions.record_wrong_peer_id(address, expected, obtained);
            record_exclusion_outcome(swarm, driver_state, peer_exclusions, metrics, outcome, &ids)
                .await;
        }
        // A firewall is dropping our egress to this address: quinn's
        // `sendmsg` returned EPERM (PermissionDenied). Treat this as local
        // reachability evidence first, with IP-wide action only after repeats.
        DialError::Transport(addr_errs) => {
            for (addr, transport_err) in addr_errs {
                let dyn_err: &(dyn std::error::Error + 'static) = transport_err;
                let ids: Vec<PeerId> = p2p_peer_id(addr).into_iter().collect();
                if chain_has_permission_denied(dyn_err) {
                    let outcome = peer_exclusions.record_permission_denied(addr);
                    record_exclusion_outcome(
                        swarm, driver_state, peer_exclusions, metrics, outcome, &ids,
                    )
                    .await;
                } else {
                    let outcome = peer_exclusions.record_dial_failure(addr, p2p_peer_id(addr));
                    record_exclusion_outcome(
                        swarm, driver_state, peer_exclusions, metrics, outcome, &ids,
                    )
                    .await;
                    trace!("Failed to dial address {}: {}", addr, transport_err);
                }
            }
        }
        _ => log_dial_error(error),
    }
}

async fn record_exclusion_outcome(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
    outcome: ExclusionOutcome,
    related_peers: &[PeerId],
) {
    if let Some(address) = outcome.address_cooldown {
        log_address_cooldown(&address);
        // Both counters are incremented here at cooldown-creation time. The
        // first is a misnamed legacy metric retained for existing dashboards;
        // the second is the correctly-named alias documented in #2013.
        metrics.address_cooldown_dial_denied.increment();
        metrics.address_cooldowns_created.increment();
        let mut peers_to_prune = related_peers.iter().copied().collect::<BTreeSet<_>>();
        if let Some(peer_id) = address.key.expected_peer {
            peers_to_prune.insert(peer_id);
        }
        if peers_to_prune.is_empty() {
            prune_one_address(swarm, metrics, None, &address.address).await;
        } else {
            for peer_id in peers_to_prune {
                prune_one_address(swarm, metrics, Some(peer_id), &address.address).await;
            }
        }
    }

    if let Some(ip) = outcome.ip_exclusion {
        log_ip_exclusion(&ip, related_peers);
        metrics.ip_exclusions_created.increment();
        if ip.fail2ban {
            let log_peer = related_peers
                .first()
                .copied()
                .unwrap_or_else(PeerId::random);
            match ip.ip {
                IpAddr::V4(v4) => log_fail2ban_ipv4(&log_peer, &v4),
                IpAddr::V6(v6) => log_fail2ban_ipv6(&log_peer, &v6),
            }
        }
        prune_excluded_swarm_state(swarm, driver_state, peer_exclusions, metrics).await;
    }
}

fn log_address_cooldown(outcome: &AddressCooldownOutcome) {
    info!(
        address = %outcome.address,
        ip = %outcome.key.ip,
        ttl_secs = outcome.ttl.as_secs(),
        reason = %outcome.reason,
        "temporarily cooling down peer endpoint"
    );
}

fn log_ip_exclusion(outcome: &IpExclusionOutcome, related_peers: &[PeerId]) {
    warn!(
        ip = %outcome.ip,
        ttl_secs = outcome.ttl.as_secs(),
        reason = %outcome.reason,
        peers = ?related_peers.iter().map(|peer| peer.to_base58()).collect::<Vec<_>>(),
        "temporarily excluding peer IP"
    );
}

async fn prune_one_address(
    swarm: &mut Swarm<NockchainBehaviour>,
    metrics: &NockchainP2PMetrics,
    peer_id: Option<PeerId>,
    address: &Multiaddr,
) {
    let Some(peer_id) = peer_id.or_else(|| p2p_peer_id(address)) else {
        return;
    };

    let stripped = multiaddr_without_p2p(address);
    let mut address_candidates = vec![address.clone()];
    if stripped != *address {
        address_candidates.push(stripped);
    }

    let mut removed_address = false;
    let mut removed_peer = false;
    for candidate in address_candidates {
        let had_kad_address = swarm.behaviour_mut().kad.kbuckets().any(|bucket| {
            bucket.iter().any(|peer| {
                peer.node.key.into_preimage() == peer_id
                    && peer.node.value.iter().any(|addr| addr == &candidate)
            })
        });
        if swarm
            .behaviour_mut()
            .kad
            .remove_address(&peer_id, &candidate)
            .is_some()
        {
            removed_peer = true;
        }
        let removed_from_peer_store = swarm
            .behaviour_mut()
            .peer_store
            .store_mut()
            .remove_address(&peer_id, &candidate);
        removed_address = removed_address || had_kad_address || removed_from_peer_store;
    }
    if removed_address {
        metrics.kad_addresses_pruned_for_exclusion.increment();
    }
    if removed_peer {
        metrics.kad_peers_pruned_for_exclusion.increment();
    }
}

async fn observe_kad_cardinality_and_exclude(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
) {
    let mut by_ip: BTreeMap<IpAddr, (BTreeSet<PeerId>, BTreeSet<u16>)> = BTreeMap::new();
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        for peer in bucket.iter() {
            let peer_id = peer.node.key.into_preimage();
            for address in peer.node.value.iter() {
                let Some(key) = peer_exclusions.address_key(address, Some(peer_id)) else {
                    continue;
                };
                let (peers, ports) = by_ip.entry(key.ip).or_default();
                peers.insert(peer_id);
                if let Some(port) = key.port {
                    ports.insert(port);
                }
            }
        }
    }

    let mut max_cardinality = 0usize;
    for (ip, (peers, ports)) in by_ip {
        max_cardinality = max_cardinality.max(peers.len()).max(ports.len());
        if let Some(outcome) = peer_exclusions.record_kad_cardinality(ip, peers.len(), ports.len())
        {
            let related_peers = peers.iter().copied().collect::<Vec<_>>();
            record_exclusion_outcome(
                swarm,
                driver_state,
                peer_exclusions,
                metrics,
                ExclusionOutcome {
                    address_cooldown: None,
                    ip_exclusion: Some(outcome),
                },
                &related_peers,
            )
            .await;
        }
    }
    let _ = metrics.same_ip_kad_cardinality.swap(max_cardinality as f64);
}

async fn prune_excluded_swarm_state(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
) {
    let mut addresses_to_remove = Vec::new();
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        for peer in bucket.iter() {
            let peer_id = peer.node.key.into_preimage();
            for address in peer.node.value.iter() {
                if peer_exclusions.is_address_excluded(address, Some(peer_id)) {
                    addresses_to_remove.push((peer_id, address.clone()));
                }
            }
        }
    }

    for (peer_id, address) in addresses_to_remove {
        metrics.kad_addresses_pruned_for_exclusion.increment();
        if swarm
            .behaviour_mut()
            .kad
            .remove_address(&peer_id, &address)
            .is_some()
        {
            metrics.kad_peers_pruned_for_exclusion.increment();
        }
        let _ = swarm
            .behaviour_mut()
            .peer_store
            .store_mut()
            .remove_address(&peer_id, &address);
    }

    let connections_to_close = {
        let state_guard = driver_state.lock().await;
        state_guard
            .peer_connections
            .iter()
            .flat_map(|(peer_id, connections)| {
                connections.iter().filter_map(|(connection_id, address)| {
                    let ip = address.ip_addr()?;
                    peer_exclusions
                        .is_ip_excluded(&ip)
                        .then_some((*peer_id, *connection_id))
                })
            })
            .collect::<Vec<_>>()
    };

    for (peer_id, connection_id) in connections_to_close {
        debug!("Closing connection {connection_id} to excluded peer {peer_id}");
        swarm.close_connection(connection_id);
    }
}

fn log_dial_error(error: DialError) {
    match error {
        DialError::NoAddresses => debug!("No addresses to dial"),
        DialError::LocalPeerId { address } => {
            debug!("Tried to dial ourselves at {}", address.to_string())
        }

        DialError::Aborted => trace!("Dial aborted"),
        DialError::WrongPeerId { obtained, address } => {
            warn!(
                "Wrong peer id {} from address {}",
                obtained,
                address.to_string()
            )
        }
        DialError::Denied { cause } => debug!("Outgoing connection denied: {}", cause),
        DialError::DialPeerConditionFalse(_) => debug!("Dial peer condition false"),
        DialError::Transport(addr_errs) => {
            for (addr, error) in addr_errs {
                trace!("Failed to dial address {}: {}", addr.to_string(), error);
            }
        }
    }
}

fn log_outbound_failure(
    peer: PeerId,
    request_id: request_response::OutboundRequestId,
    error: request_response::OutboundFailure,
    request_context: Option<&OutboundRequestContext>,
    metrics: Arc<NockchainP2PMetrics>,
) {
    metrics.request_failed.increment();
    if let Some(request_context) = request_context {
        gen2::increment_outbound_generation_failure_metrics(
            &metrics, request_context.generation, &error,
        );
        debug!(
            peer = %peer,
            request_id = %request_id,
            ?request_context.generation,
            request_shape = gen2::outbound_request_shape(&request_context.request),
            batch_items = gen2::batch_request_item_count(&request_context.request),
            retry_count = request_context.retry_count,
            fallback_attempted = request_context.fallback_attempted,
            "Outbound request failed with retained context"
        );
    } else {
        debug!(
            peer = %peer,
            request_id = %request_id,
            "Outbound request failed without retained context"
        );
    }
    match error {
        request_response::OutboundFailure::DialFailure => {
            debug!("Failed to dial peer {} for request", peer)
        }
        request_response::OutboundFailure::Timeout => debug!("Request to peer {} timed out", peer),
        request_response::OutboundFailure::ConnectionClosed => {
            debug!("Connection to peer {} closed with request pending", peer)
        }
        request_response::OutboundFailure::Io(err) => {
            debug!("Error making request to peer {}: {}", peer, err)
        }
        request_response::OutboundFailure::UnsupportedProtocols => {
            debug!("Unsupported protocol when making request to peer {}", peer)
        }
    }
}

fn log_inbound_failure(
    peer: PeerId,
    error: request_response::InboundFailure,
    metrics: Arc<NockchainP2PMetrics>,
) {
    if let request_response::InboundFailure::ResponseOmission = error {
        metrics.response_dropped.increment();
    } else {
        metrics.response_failed_not_dropped.increment();
    }
    match error {
        request_response::InboundFailure::ResponseOmission => trace!(
            "Response to peer {} refused, likely load shedding or simply no data for request", peer
        ),
        request_response::InboundFailure::Timeout => warn!("Response to peer {} timed out", peer),
        request_response::InboundFailure::Io(err) => {
            warn!("Error responding to peer {}: {}", peer, err)
        }
        request_response::InboundFailure::ConnectionClosed => {
            debug!("Connection to peer {} closed with response pending", peer)
        }
        request_response::InboundFailure::UnsupportedProtocols => {
            debug!("Unsupported protocol when responding to peer {}", peer)
        }
    };
}

fn dial_more_peers(
    swarm: &mut Swarm<NockchainBehaviour>,
    state_guard: MutexGuard<P2PState>,
    peer_exclusions: &PeerExclusions,
) {
    let mut addresses_to_dial = Vec::new();
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        for peer in bucket.iter() {
            if state_guard
                .peer_connections
                .contains_key(&peer.node.key.into_preimage())
            {
                continue;
            }
            for address in peer.node.value.iter() {
                let mut address = address.clone();

                if peer_exclusions
                    .is_address_excluded(&address, Some(peer.node.key.into_preimage()))
                {
                    continue;
                }

                if let Ok(address_with_peer_id) =
                    address.clone().with_p2p(peer.node.key.into_preimage())
                {
                    address = address_with_peer_id;
                }
                addresses_to_dial.push(address);
            }
        }
    }
    addresses_to_dial.shuffle(&mut rand::rng());
    for address in addresses_to_dial {
        info!("Redialing {}", address);
        if let Err(err) = swarm.dial(address) {
            log_dial_error(err);
        };
    }
}

/// # Create a swarm and set it to listen
///
/// This function initializes a libp2p swarm with the provided keypair and binding addresses.
/// It configures the swarm to listen on specified multiaddresses and sets up the behavior for network interactions.
///
/// # Arguments
/// * `keypair` - The keypair for the node's identity
/// * `bind` - A vector of multiaddresses specifying the network interfaces to bind to
///
/// # Returns
/// A Result containing the Swarm instance or an error if any operation fails
pub(crate) fn start_swarm(
    libp2p_config: LibP2PConfig,
    keypair: Keypair,
    bind: Vec<Multiaddr>,
    allowed: Option<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>,
    limits: connection_limits::ConnectionLimits,
    memory_limits: Option<memory_connection_limits::Behaviour>,
    peer_exclusions: PeerExclusions,
) -> Result<Swarm<NockchainBehaviour>, Box<dyn Error>> {
    let (resolver_config, resolver_opts) =
        if let Ok(sys) = hickory_resolver::system_conf::read_system_conf() {
            debug!("resolver configs and opts: {:?}", sys);
            sys
        } else {
            (ResolverConfig::cloudflare(), ResolverOpts::default())
        };

    let max_idle_timeout_millisecs = libp2p_config.max_idle_timeout_millisecs();
    let keep_alive_interval = libp2p_config.keep_alive_interval();
    let handshake_timeout = libp2p_config.handshake_timeout();
    let connection_timeout = libp2p_config.connection_timeout();
    let swarm_idle_timeout = libp2p_config.swarm_idle_timeout();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic_config(|mut cfg| {
            cfg.max_idle_timeout = max_idle_timeout_millisecs;
            cfg.keep_alive_interval = keep_alive_interval;
            cfg.handshake_timeout = handshake_timeout;
            cfg
        })
        .with_dns_config(resolver_config, resolver_opts)
        .with_behaviour(NockchainBehaviour::pre_new(
            libp2p_config, allowed, limits, memory_limits, peer_exclusions,
        ))?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(swarm_idle_timeout))
        .with_connection_timeout(connection_timeout)
        .build();

    for bind_addr in bind {
        swarm.listen_on(bind_addr.clone()).map_err(|e| {
            error!("Failed to listen on {bind_addr:?}: {e}");
            e
        })?;
    }
    Ok(swarm)
}

///** Handler for "identify" messages */
//#[instrument(skip(swarm))]
/// Returns whether the remote peer advertises inbound support for the Gen2
/// req-res protocol.  The caller stores this in a per-peer map so the
/// batching decision can skip Gen2 batches for Gen1-only peers.
pub(crate) fn identify_received(
    swarm: &mut Swarm<NockchainBehaviour>,
    peer_id: PeerId,
    info: libp2p::identify::Info,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
) -> Result<bool, NockAppError> {
    swarm.add_external_address(info.observed_addr.clone());
    if let Some(ip) = info.observed_addr.ip_addr() {
        peer_exclusions.record_positive_ip(ip);
    }
    let us = *swarm.local_peer_id();

    let peer_supports_gen2_inbound = info
        .protocols
        .iter()
        .any(|p| p.as_ref() == LibP2PConfig::req_res_gen2_protocol_version());

    let kad = &mut swarm.behaviour_mut().kad;
    trace!("identify received for peer {}", peer_id);
    trace!("Adding address {} for us: {}", info.observed_addr, us);
    kad.add_address(&us, info.observed_addr);
    for addr in info.listen_addrs {
        if let Some(Protocol::Dnsaddr(_)) = addr.iter().next() {
            continue;
        }
        if peer_exclusions.is_address_excluded(&addr, Some(peer_id)) {
            trace!("Skipping excluded address {addr} for peer {peer_id}");
            metrics.identify_addresses_skipped_for_exclusion.increment();
            continue;
        }
        trace!("Adding address {} for peer {}", addr, peer_id);
        kad.add_address(&peer_id, addr);
    }
    Ok(peer_supports_gen2_inbound)
}

fn log_ping_success(peer: PeerId, connection_address: Option<Multiaddr>, duration: Duration) {
    let Some(connection_address) = connection_address else {
        trace!("Untracked connection to {peer}, please report this to the developers");
        return;
    };
    let ms = duration.as_millis();
    debug!("Ping to {peer} via {connection_address} succeeded in {ms}ms");
}

fn log_ping_failure(peer: PeerId, connection_address: Option<Multiaddr>, error: ping::Failure) {
    let Some(connection_address) = connection_address else {
        trace!("Untracked connection to {peer}, please report this to the developers");
        return;
    };
    debug!("Ping to {peer} via {connection_address} failed: {error}");
}
