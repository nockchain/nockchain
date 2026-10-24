use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use hickory_resolver::config::{ResolverConfig, ResolverOpts, CLOUDFLARE};
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
    Transport,
};
use nockapp::driver::IODriverFn;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::{Wire, WireRepr};
use nockapp::NockAppError;
use nockvm::noun::{NounAllocator, D, T};
use nockvm_macros::tas;
use rand::seq::SliceRandom;
use rand::Rng;
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
    block_id_from_page, block_with_txs_by_height_request_message, decode_request_item_message,
    tx_id_from_raw_tx, NockchainDataRequest, NockchainRequest, FACT_POKE_VERSION,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{OutboundRequestContext, P2PState, RangeCapability, RequestEndpoint};
use crate::p2p_util::{
    canonical_ip, log_fail2ban_ipv4, log_fail2ban_ipv6, multiaddr_without_p2p, quic_socket_addr,
    MultiaddrExt, PeerIdExt,
};
use crate::tip5_util::{tip5_hash_to_base58, tip5_hash_to_base58_stack};
use crate::tracked_join_set::TrackedJoinSet;
use crate::traffic_cop;

mod actions;
mod frontier_reconcile;
mod gen2;
mod kernel_io;
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
mod watchdog;

pub(crate) use actions::SwarmAction;
use actions::{process_swarm_action, SwarmActionDispatcher};
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
    outbound_peer_count: usize,
    configured_peer_missing: bool,
    initial_peers: &[Multiaddr],
    backbone_peers: &[Multiaddr],
) -> bool {
    !(initial_peers.is_empty() && backbone_peers.is_empty())
        && (outbound_peer_count == 0 || configured_peer_missing)
}

fn configured_peer_connected(
    swarm: &Swarm<NockchainBehaviour>,
    state: &P2PState,
    address: &Multiaddr,
) -> bool {
    p2p_peer_id(address).map_or_else(
        || state.has_outbound_connection_at_address(address),
        |peer_id| swarm.is_connected(&peer_id),
    )
}

const OBSERVED_ADDRESS_PENDING_LIMIT: usize = 64;
const OBSERVED_ADDRESS_CLAIMS_PER_PEER: usize = 4;
const OBSERVED_ADDRESS_PEERS_PER_CLAIM: usize = 8;
const OBSERVED_ADDRESS_ADMITTED_LIMIT: usize = 16;
const OBSERVED_ADDRESS_ADMISSION_HISTORY_LIMIT: usize = 64;
const OBSERVED_ADDRESS_VOTE_TTL: Duration = Duration::from_secs(10 * 60);
const OBSERVED_ADDRESS_ADMITTED_TTL: Duration = Duration::from_secs(60 * 60);

struct ObservedAddressVote {
    connection_id: ConnectionId,
    first_seen: Instant,
}

#[derive(Debug, Eq, PartialEq)]
struct ObservedAddressAdmission {
    admitted_at: Instant,
    observers: [PeerId; 2],
}

#[derive(Default)]
struct ObservedAddressUpdate {
    remove: Vec<Multiaddr>,
    add: Option<Multiaddr>,
}

impl ObservedAddressUpdate {
    fn apply(self, swarm: &mut Swarm<NockchainBehaviour>) {
        for address in self.remove {
            swarm.remove_external_address(&address);
        }
        if let Some(address) = self.add {
            swarm.add_external_address(address);
        }
    }
}

/// Distinct live authenticated identities may share an IP, including on localhost.
#[derive(Default)]
struct ObservedAddressVotes {
    pending: BTreeMap<Multiaddr, BTreeMap<PeerId, ObservedAddressVote>>,
    // Only these advertisements belong exclusively to this manager.
    admitted: BTreeMap<Multiaddr, ObservedAddressAdmission>,
    // Immutable charges survive replacement, independent ownership, and expiry
    // notifications until one hour after the original admission.
    // No address keys: a fresh quorum can readmit an address for a new charge.
    retired: Vec<ObservedAddressAdmission>,
}

fn observed_address_ip(address: &Multiaddr) -> Option<IpAddr> {
    let mut protocols = address.iter();
    let ip = match protocols.next()? {
        Protocol::Ip4(ip) => IpAddr::V4(ip),
        Protocol::Ip6(ip) => canonical_ip(IpAddr::V6(ip)),
        _ => return None,
    };
    if !matches!(protocols.next(), Some(Protocol::Udp(port)) if port != 0)
        || !matches!(protocols.next(), Some(Protocol::QuicV1))
        || protocols.next().is_some()
        || ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, IpAddr::V4(ip) if ip.is_broadcast())
    {
        return None;
    }
    Some(ip)
}

impl ObservedAddressVotes {
    fn peer_usage(&self, peer_id: &PeerId) -> usize {
        self.pending
            .values()
            .filter(|votes| votes.contains_key(peer_id))
            .count()
            + self
                .admitted
                .values()
                .chain(self.retired.iter())
                .filter(|receipt| receipt.observers.contains(peer_id))
                .count()
    }

    // Direct Swarm add/remove calls do not emit public confirmation/expiry
    // events. Those events reflect another behaviour's ownership or removal.
    // Any future manual insertion must also relinquish ownership before adding.
    fn relinquish(&mut self, address: &Multiaddr) {
        if let Some(receipt) = self.admitted.remove(address) {
            self.retired.push(receipt);
        }
    }

    fn observe_at(
        &mut self,
        swarm: &Swarm<NockchainBehaviour>,
        peer_id: PeerId,
        connection_id: ConnectionId,
        address: &Multiaddr,
        now: Instant,
    ) -> ObservedAddressUpdate {
        let mut update = ObservedAddressUpdate::default();
        let Some(ip) = observed_address_ip(address) else {
            return update;
        };
        let recovery = &swarm.behaviour().recovery;
        let Some(observer) = recovery.connection_endpoint(&peer_id, connection_id) else {
            return update;
        };
        if ip.is_loopback() && !canonical_ip(observer.ip()).is_loopback() {
            return update;
        }

        // Validate before pruning. Repeats and replacement sessions retain the
        // original timestamp; disconnected votes remain charged until expiry.
        self.pending.retain(|_, votes| {
            votes.retain(|_, vote| now.duration_since(vote.first_seen) < OBSERVED_ADDRESS_VOTE_TTL);
            !votes.is_empty()
        });
        self.retired.retain(|receipt| {
            now.duration_since(receipt.admitted_at) < OBSERVED_ADDRESS_ADMITTED_TTL
        });
        self.admitted.retain(|address, receipt| {
            if now.duration_since(receipt.admitted_at) < OBSERVED_ADDRESS_ADMITTED_TTL {
                true
            } else {
                // Vec::new() allocates only when a removal is actually needed.
                // Expiry plus replacement removes at most 16 owned addresses.
                update.remove.push(address.clone());
                false
            }
        });
        if self.admitted.contains_key(address) {
            return update;
        }
        // The plan is applied synchronously. An expiring current claim is not
        // protected merely because its removal has not yet reached the swarm.
        if swarm
            .external_addresses()
            .any(|external| external == address)
            && !update.remove.contains(address)
        {
            self.pending.remove(address);
            return update;
        }
        let pending_votes = self.pending.get(address);
        let already_voted = pending_votes.is_some_and(|votes| votes.contains_key(&peer_id));
        if !already_voted && self.peer_usage(&peer_id) >= OBSERVED_ADDRESS_CLAIMS_PER_PEER {
            return update;
        }
        match pending_votes {
            Some(_) => {}
            None if self.pending.len() >= OBSERVED_ADDRESS_PENDING_LIMIT => return update,
            None => {
                self.pending.insert(address.clone(), BTreeMap::new());
            }
        }
        let votes = self
            .pending
            .get_mut(address)
            .expect("the observed claim has a pending entry");
        if let Some(vote) = votes.get_mut(&peer_id) {
            vote.connection_id = connection_id;
        } else if votes.len() < OBSERVED_ADDRESS_PEERS_PER_CLAIM {
            votes.insert(
                peer_id,
                ObservedAddressVote {
                    connection_id,
                    first_seen: now,
                },
            );
        } else {
            return update;
        }
        let mut live = votes.iter().filter_map(|(peer, vote)| {
            recovery
                .connection_endpoint(peer, vote.connection_id)
                .filter(|endpoint| !ip.is_loopback() || canonical_ip(endpoint.ip()).is_loopback())
                .map(|_| *peer)
        });
        let (Some(first), Some(second)) = (live.next(), live.next()) else {
            return update;
        };
        // Never withdraw a victim unless the new immutable receipt can fit.
        if self.admitted.len() + self.retired.len() >= OBSERVED_ADDRESS_ADMISSION_HISTORY_LIMIT {
            return update;
        }
        if self.admitted.len() >= OBSERVED_ADDRESS_ADMITTED_LIMIT {
            let oldest = self
                .admitted
                .iter()
                .min_by(|(left_address, left), (right_address, right)| {
                    (left.admitted_at, *left_address).cmp(&(right.admitted_at, *right_address))
                })
                .map(|(address, _)| address.clone())
                .expect("a full admitted table has an oldest entry");
            self.relinquish(&oldest);
            update.remove.push(oldest);
        }
        let (address, _) = self
            .pending
            .remove_entry(address)
            .expect("the corroborated claim remains pending");
        self.admitted.insert(
            address.clone(),
            ObservedAddressAdmission {
                admitted_at: now,
                observers: [first, second],
            },
        );
        update.add = Some(address);
        update
    }
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
    state: &P2PState,
    initial_peers: &[Multiaddr],
    backbone_peers: &[Multiaddr],
    backbone_cursor: &mut usize,
    backbone_dial_count: usize,
    reason: &'static str,
) -> Result<bool, NockAppError> {
    let outbound_peer_count = state.outbound_peer_count();
    let configured_peer_missing = initial_peers
        .iter()
        .chain(backbone_peers)
        .any(|address| !configured_peer_connected(swarm, state, address));
    if !should_redial_initial_peers(
        outbound_peer_count, configured_peer_missing, initial_peers, backbone_peers,
    ) {
        return Ok(false);
    }
    if swarm.behaviour_mut().recovery.reserved() || !swarm.behaviour().recovery.has_capacity() {
        return Ok(true);
    }

    let backbone_start = if backbone_peers.is_empty() {
        0
    } else {
        *backbone_cursor % backbone_peers.len()
    };
    let backbone_count = backbone_dial_count.min(backbone_peers.len());
    info!(
        reason,
        outbound_peer_count,
        connected_peer_count = swarm.connected_peers().count(),
        initial_peer_count = initial_peers.len(),
        backbone_candidates = backbone_count,
        configured_peer_missing,
        "Redialing configured peers to restore a recovery path"
    );
    let mut initial_index = swarm
        .behaviour()
        .recovery
        .initial_offset(initial_peers.len());
    let mut initial_left = initial_peers.len();
    let mut backbone_index = 0;
    let mut prefer_backbone = swarm.behaviour_mut().recovery.next_pool();
    while swarm.behaviour().recovery.has_capacity()
        && (initial_left > 0 || backbone_index < backbone_count)
    {
        let from_backbone =
            backbone_index < backbone_count && (prefer_backbone || initial_left == 0);
        let address = if from_backbone {
            let address = &backbone_peers[(backbone_start + backbone_index) % backbone_peers.len()];
            backbone_index += 1;
            address
        } else {
            let address = &initial_peers[initial_index];
            initial_left -= 1;
            initial_index = (initial_index + 1) % initial_peers.len();
            swarm
                .behaviour_mut()
                .recovery
                .advance_initial(initial_index);
            address
        };
        prefer_backbone = !prefer_backbone;
        if !configured_peer_connected(swarm, state, address)
            && !swarm.behaviour().recovery.pending_at(address)
            && !dial_recovery_peer(swarm, address.clone())
        {
            // Synchronous pending-limit denial has no completion event.
            // Leave this candidate first in its pool once capacity frees.
            if from_backbone {
                backbone_index -= 1;
            } else {
                initial_index = (initial_index + initial_peers.len() - 1) % initial_peers.len();
                swarm
                    .behaviour_mut()
                    .recovery
                    .advance_initial(initial_index);
            }
            break;
        }
    }
    if !backbone_peers.is_empty() {
        // Keep unvisited window entries for the next available admission slot.
        *backbone_cursor = (backbone_start + backbone_index) % backbone_peers.len();
    }
    Ok(true)
}

const INITIAL_PEER_REDIAL_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound for the exponential backoff between initial-peer redial attempts.
/// We never give up: once the backoff reaches this value we keep redialing at
/// this cadence (hourly) while a configured recovery peer remains absent.
const INITIAL_PEER_REDIAL_MAX_INTERVAL: Duration = Duration::from_secs(3600);

/// The backoff to arm after one `initial_peer_redial` tick. A tick that had to
/// redial doubles the current backoff up to the cap; a tick that found every
/// configured peer connected (or nothing to dial) drops straight back to the
/// base interval so a later disconnect is retried promptly.
fn next_initial_peer_redial_backoff(current: Duration, redialed: bool) -> Duration {
    if redialed {
        current
            .saturating_mul(2)
            .min(INITIAL_PEER_REDIAL_MAX_INTERVAL)
    } else {
        INITIAL_PEER_REDIAL_INTERVAL
    }
}

/// Missing pool entries retry in the background even while a configured path
/// is usable. Losing the last such path starts a fresh, base-cadence recovery
/// episode rather than inheriting that background retry deadline.
struct ConfiguredPeerRedial {
    timer: tokio::time::Interval,
    backoff: Duration,
    had_usable_path: bool,
}

impl ConfiguredPeerRedial {
    fn new() -> Self {
        let mut timer = tokio::time::interval_at(
            Instant::now() + INITIAL_PEER_REDIAL_INTERVAL,
            INITIAL_PEER_REDIAL_INTERVAL,
        );
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        Self {
            timer,
            backoff: INITIAL_PEER_REDIAL_INTERVAL,
            had_usable_path: false,
        }
    }

    fn observe_path(
        &mut self,
        swarm: &Swarm<NockchainBehaviour>,
        state: &P2PState,
        initial_peers: &[Multiaddr],
        backbone_peers: &[Multiaddr],
    ) {
        let has_usable_path = initial_peers
            .iter()
            .chain(backbone_peers)
            .any(|address| configured_peer_connected(swarm, state, address));
        if self.had_usable_path && !has_usable_path {
            self.backoff = INITIAL_PEER_REDIAL_INTERVAL;
            self.timer.reset_after(self.backoff);
            debug!("Last configured recovery path lost; base retry cadence armed");
        }
        self.had_usable_path = has_usable_path;
    }

    fn redial(
        &mut self,
        swarm: &mut Swarm<NockchainBehaviour>,
        state: &P2PState,
        initial_peers: &[Multiaddr],
        backbone_peers: &[Multiaddr],
        backbone_cursor: &mut usize,
        backbone_dial_count: usize,
    ) -> Result<(), NockAppError> {
        self.observe_path(swarm, state, initial_peers, backbone_peers);
        let redialed = redial_initial_peers(
            swarm, state, initial_peers, backbone_peers, backbone_cursor, backbone_dial_count,
            "configured_peer_retry",
        )?;
        self.backoff = next_initial_peer_redial_backoff(self.backoff, redialed);
        // Always rearm: otherwise Interval resumes its fixed five-second period.
        self.timer.reset_after(self.backoff);
        if redialed {
            debug!(
                backoff_secs = self.backoff.as_secs(),
                "Configured peer recovery retry armed"
            );
        }
        Ok(())
    }
}

fn dial_recovery_peer(swarm: &mut Swarm<NockchainBehaviour>, address: Multiaddr) -> bool {
    let opts = libp2p::swarm::dial_opts::DialOpts::from(address.clone());
    let id = opts.connection_id();
    if !swarm.behaviour_mut().recovery.track(id, address) {
        return false;
    }
    debug!(
        connection_id = ?id,
        endpoint = %swarm.behaviour().recovery.pending_address(id).expect("tracked configured dial has an endpoint"),
        "Dialing configured recovery peer"
    );
    swarm.behaviour_mut().recovery.arm(id);
    match swarm.dial(opts) {
        Ok(()) => true,
        Err(error) => {
            let pending_limit = matches!(&error, DialError::Denied { cause }
                if cause.downcast_ref::<connection_limits::Exceeded>().is_some());
            swarm.behaviour_mut().recovery.finish(id);
            log_dial_error(error);
            !pending_limit
        }
    }
}

fn reclaim_recovery_slot(
    swarm: &mut Swarm<NockchainBehaviour>,
    state: &P2PState,
    connection_id: ConnectionId,
    target_peer: Option<PeerId>,
    error: &DialError,
    initial: &[Multiaddr],
    backbone: &[Multiaddr],
) {
    let already_reclaimed = swarm.behaviour_mut().recovery.reserved();
    let address = swarm.behaviour_mut().recovery.finish(connection_id);
    let limits_denied = matches!(error, DialError::Denied { cause }
    if cause.downcast_ref::<connection_limits::Exceeded>().is_some_and(|limit| {
        let network_info = swarm.network_info();
        let counters = network_info.connection_counters();
        limit.limit() > 0 && (counters.num_established_outgoing() >= limit.limit()
            || counters.num_established() >= limit.limit())
    }));
    // Established denials report the obtained identity even for unnamed dials.
    // A connected target can be at its per-peer limit; closing somebody else
    // cannot relieve that limit and must not disturb their useful connection.
    if !limits_denied
        || already_reclaimed
        || target_peer.is_none_or(|peer| swarm.is_connected(&peer))
    {
        return;
    }
    let Some(address) = address else { return };
    let Some(reclaimed) = state.recovery_connection_to_reclaim(initial, backbone) else {
        return;
    };
    if swarm.close_connection(reclaimed) {
        info!(connection_id = ?reclaimed, recovery_connection_id = ?connection_id,
            endpoint = %address, "Reclaiming an idle outbound slot for configured peer recovery");
        swarm.behaviour_mut().recovery.reserve(reclaimed, address);
    }
}
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
        return PrefetchPeerSelection::NoCandidate;
    }
    let state_guard = driver_state.lock().await;
    let bandwidth_cap = state_guard.prefetch_bandwidth_cap_per_peer_bytes_per_min();
    let mut throttled = false;
    let mut supported = Vec::new();
    let mut unknown = Vec::new();

    for peer_id in connected_peers {
        if state_guard.inflight_prefetch_count_for_peer(peer_id) >= max_inflight_per_peer {
            continue;
        }
        let mut numeric_candidates = false;
        let endpoints = state_guard
            .peer_connections
            .get(peer_id)
            .into_iter()
            .flat_map(|connections| connections.iter())
            .filter_map(|(connection_id, address)| {
                quic_socket_addr(address).map(|endpoint| {
                    numeric_candidates = true;
                    RequestEndpoint {
                        connection_id: *connection_id,
                        endpoint,
                    }
                })
            });
        if let Some((connection, budget)) = state_guard.prefetch_throttle(peer_id, endpoints) {
            debug!(monotonic_us = traffic_cop::observation_time_us(), peer = %peer_id,
                connection_id = ?connection.connection_id, endpoint = %budget.endpoint,
                bucket = %budget.bucket, peer_window_bytes = budget.peer_window_bytes,
                bucket_window_bytes = budget.bucket_window_bytes, peer_inflight = budget.peer_inflight,
                peer_cap_bytes = bandwidth_cap,
                bucket_cap_bytes = state_guard.prefetch_bucket_bandwidth_cap(),
                "Prefetch peer bandwidth window exhausted");
            throttled = true;
            continue;
        }
        if !numeric_candidates
            && bandwidth_cap > 0
            && (state_guard.prefetch_bandwidth_window_bytes(peer_id) >= bandwidth_cap
                || state_guard.prefetch_bucket_bandwidth_exhausted(peer_id))
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
        PrefetchPeerSelection::NoCandidate
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
    NoCandidate,
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
    let initial_peers: Arc<[Multiaddr]> = Arc::from(initial_peers);
    let backbone_peers: Arc<[Multiaddr]> = Arc::from(backbone_peers);
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
            let frontier_reconcile_interval = match libp2p_config.frontier_reconcile_interval_secs {
                0 => None,
                secs => Some(Duration::from_secs(secs)),
            };
            let failed_pings_before_close = libp2p_config.failed_pings_before_close();
            let req_res_gen2_bundle_enabled = libp2p_config.req_res_gen2_bundle_enabled;
            let prefetch_config = PrefetchConfig {
                enabled: libp2p_config.prefetch_enabled,
                window_initial: libp2p_config.prefetch_window_initial,
                window_max: libp2p_config.prefetch_window_max,
                max_inflight_per_peer: libp2p_config.prefetch_max_inflight_per_peer,
                kernel_demand_threshold: libp2p_config.prefetch_kernel_demand_threshold.max(1),
            };
            let prefetch_height_failure_budget = libp2p_config.prefetch_height_failure_budget;
            let prefetch_stuck_backoff =
                Duration::from_secs(libp2p_config.prefetch_stuck_backoff_secs);
            let prefetch_bandwidth_cap_per_peer_bytes_per_min =
                libp2p_config.prefetch_bandwidth_cap_per_peer_bytes_per_min;
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
                state_guard.set_configured_recovery_peers(
                    Arc::clone(&initial_peers),
                    Arc::clone(&backbone_peers),
                );
                state_guard.set_prefetch_safety_config(
                    prefetch_height_failure_budget, prefetch_stuck_backoff,
                    prefetch_bandwidth_cap_per_peer_bytes_per_min,
                );
                state_guard.set_prefetch_inflight_limit(prefetch_config.max_inflight_per_peer);
            }
            let mut kad_bootstrap = tokio::time::interval(kademlia_bootstrap_interval);
            kad_bootstrap.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut initial_peer_redial = ConfiguredPeerRedial::new();
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
            let lag_signal = Arc::new(tokio::sync::Notify::new());
            join_set.spawn(
                "frontier-reconcile".to_string(),
                frontier_reconcile::run_frontier_reconcile_task(
                    traffic_cop.clone(),
                    driver_state.clone(),
                    metrics.clone(),
                    swarm_tx.clone(),
                    lag_signal.clone(),
                    frontier_reconcile_interval,
                ),
            );
            let mut effects_open = true;

            let mut pending_gen2_batches = BTreeMap::<PeerId, gen2::PendingGen2Batch>::new();
            let mut observed_address_votes = ObservedAddressVotes::default();
            gen2::update_pending_batch_metrics(&metrics, &pending_gen2_batches);
            // Start the round-robin at a random offset so nodes don't all dial
            // the same backbone subset first; each dial then advances the window.
            let mut backbone_cursor: usize = if backbone_peers.is_empty() {
                0
            } else {
                rand::rng().random_range(0..backbone_peers.len())
            };
            if !initial_peers.is_empty() {
                dial_peers(&mut swarm, &initial_peers, true)?;
            }
            let initial_backbone_window =
                next_backbone_window(&backbone_peers, &mut backbone_cursor, backbone_dial_count);
            if !initial_backbone_window.is_empty() {
                dial_peers(&mut swarm, &initial_backbone_window, true)?;
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
            // timestamped file, including kernel-level thread states for
            // runtime-stall diagnosis.
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
            let deferred_flush_running = Arc::new(AtomicBool::new(false));
            loop {
                if let Some(swarm_action) = buffered_swarm_actions.pop_front() {
                    process_swarm_action(
                        swarm_action, &mut swarm, &swarm_tx, &deferred_flush_running,
                        &mut join_set, &driver_state, &metrics, &peer_exclusions,
                        &mut equix_builder, &mut pending_gen2_batches, req_res_limits,
                        &traffic_cop,
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
                        log_peer_status(
                            &mut swarm,
                            &metrics,
                            &peer_exclusions,
                            &driver_state
                        ).await;
                        let state_guard = driver_state.lock().await;
                        // Inbound connections do not provide an outbound recovery
                        // path and therefore do not satisfy the discovery floor.
                        if state_guard.outbound_peer_count() < min_peers {
                            dial_more_peers(&mut swarm, state_guard, &peer_exclusions);
                        }
                    },
                    result = effect_handle.next_effect(), if effects_open => {
                        let noun_slab = match frontier_reconcile::triage_kernel_effect_recv(
                            result, &metrics, &lag_signal,
                        ) {
                            frontier_reconcile::KernelEffectRecv::Effect(slab) => slab,
                            frontier_reconcile::KernelEffectRecv::Lagged => continue,
                            frontier_reconcile::KernelEffectRecv::Closed => {
                                effects_open = false;
                                continue;
                            }
                        };
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
                            SwarmEvent::ExternalAddrConfirmed { address }
                            | SwarmEvent::ExternalAddrExpired { address } => {
                                observed_address_votes.relinquish(&address);
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
                            SwarmEvent::Behaviour(NockchainEvent::Identify(Received { peer_id, connection_id, info })) => {
                                trace!("SEvent: identify_received");
                                identify_received(
                                    &mut swarm, &mut observed_address_votes, peer_id, connection_id,
                                    info, &peer_exclusions, &metrics,
                                )?;
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
                                swarm.behaviour_mut().recovery.finish(connection_id);
                                let bucket_count = {
                                    let mut state_guard = driver_state.lock().await;
                                    state_guard.track_connection(connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                                    initial_peer_redial.observe_path(
                                        &swarm, &state_guard, &initial_peers, &backbone_peers,
                                    );
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
                                debug!(peer = %peer_id, connection_id = ?connection_id, endpoint = ?endpoint,
                                    "SEvent: connection established");
                            },
                            SwarmEvent::Behaviour(NockchainEvent::ConnectionAddressChanged {
                                peer_id, connection_id, address: new,
                            }) => {
                                let bucket_count = {
                                    let mut state = driver_state.lock().await;
                                    state.update_connection_address(connection_id, &new);
                                    initial_peer_redial.observe_path(
                                        &swarm, &state, &initial_peers, &backbone_peers,
                                    );
                                    state.ip_bucket_connection_count(connection_id)
                                };
                                if let Some((bucket, count)) = bucket_count {
                                    if req_res_limits.ip_bucket_connection_limit != 0
                                        && count > req_res_limits.ip_bucket_connection_limit {
                                        metrics.ip_bucket_connection_rejected.increment();
                                        warn!(peer = %peer_id, connection_id = ?connection_id,
                                            endpoint = ?new, bucket = %bucket, count,
                                            limit = req_res_limits.ip_bucket_connection_limit,
                                            "Closing connection because IP bucket connection cap is exceeded");
                                        swarm.close_connection(connection_id);
                                    }
                                }
                                debug!(monotonic_us = traffic_cop::observation_time_us(),
                                    peer = %peer_id, connection_id = ?connection_id, endpoint = ?new,
                                    "SEvent: connection endpoint resolved");
                            },
                            SwarmEvent::ConnectionClosed { connection_id, peer_id, endpoint, cause, num_established, .. } => {
                                debug!(peer = %peer_id, connection_id = ?connection_id, endpoint = ?endpoint,
                                    remaining_established = num_established, "SEvent: connection closed");
                                let eperm = cause
                                    .as_ref()
                                    .is_some_and(|c| chain_has_permission_denied(c));
                                {
                                    let mut state_guard = driver_state.lock().await;
                                    let _ = state_guard.lost_connection(connection_id);
                                    initial_peer_redial.observe_path(
                                        &swarm, &state_guard, &initial_peers, &backbone_peers,
                                    );
                                    if let Some(address) = swarm.behaviour_mut().recovery.closed(connection_id) {
                                        dial_recovery_peer(&mut swarm, address);
                                    }
                                }
                                if let Some(cause) = &cause {
                                    debug!("SEvent: friendship ended with {peer_id} via: {endpoint:?}. cause: {cause:?}");
                                } else {
                                    debug!("SEvent: friendship ended by us with {peer_id} via: {endpoint:?}.");
                                }
                                if eperm {
                                    if let Some(ip) = endpoint.get_remote_address().canonical_ip_addr() {
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
                                if let Response { request_id, response } = &message {
                                    let Some(response_bytes) = response.received_bytes else {
                                        error!(peer = %peer, request_id = %request_id,
                                            "Ignoring response without transport byte measurement");
                                        driver_state.lock().await.remove_outbound_request(*request_id);
                                        continue;
                                    };
                                    let receipt_endpoint = swarm.behaviour().recovery.connection_endpoint(&peer, connection_id);
                                    let mut state = driver_state.lock().await;
                                    match state.charge_received_response(*request_id, peer, connection_id, receipt_endpoint, response_bytes) {
                                        Ok(Some(charge)) => {
                                            debug!(monotonic_us = traffic_cop::observation_time_us(),
                                                peer = %peer, request_id = %request_id, connection_id = ?connection_id,
                                                endpoint = %charge.endpoint, bucket = %charge.bucket, response_bytes,
                                                peer_window_bytes = charge.peer_window_bytes,
                                                bucket_window_bytes = charge.bucket_window_bytes,
                                                peer_inflight = charge.peer_inflight,
                                                "Prefetch response bandwidth charged");
                                        }
                                        Ok(None) => {}
                                        Err(reason) => {
                                            error!(monotonic_us = traffic_cop::observation_time_us(),
                                                peer = %peer, request_id = %request_id, connection_id = ?connection_id,
                                                candidate_connections = ?state.outbound_request_context(*request_id)
                                                    .map(|context| &context.request_endpoints),
                                                reason, "Ignoring response with inconsistent request attribution");
                                            state.remove_outbound_request(*request_id);
                                            continue;
                                        }
                                    }
                                }
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
                                ResponseSent { peer, request_id, connection_id }
                            )) => {
                                debug!(monotonic_us = traffic_cop::observation_time_us(),
                                    peer = %peer, request_id = %request_id, connection_id = ?connection_id,
                                    "Nous req-res inbound response sent");
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
                            SwarmEvent::Behaviour(NockchainEvent::Ping(event)) => {
                                handle_ping_event(
                                    &mut swarm, &driver_state, &peer_exclusions, &metrics,
                                    event, failed_pings_before_close, &initial_peers, &backbone_peers,
                                ).await;
                            }
                            SwarmEvent::OutgoingConnectionError { connection_id, peer_id, error, .. } => {
                                debug!(connection_id = ?connection_id, peer = ?peer_id, error = ?error,
                                    "SEvent: outgoing connection error");
                                reclaim_recovery_slot(&mut swarm, &*driver_state.lock().await,
                                    connection_id, peer_id, &error, &initial_peers, &backbone_peers);
                                handle_outgoing_connection_error(
                                    &mut swarm,
                                    &driver_state,
                                    &peer_exclusions,
                                    &metrics,
                                    error,
                                    peer_id,
                                    &initial_peers,
                                    &backbone_peers,
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
                        // Kademlia has no known peers; retry our configured bootstrap peers
                        // if inbound sockets are the only established connections.
                        if let Err(NoKnownPeers())= swarm.behaviour_mut().kad.bootstrap() {
                            let state_guard = driver_state.lock().await;
                            if redial_initial_peers(
                                &mut swarm,
                                &state_guard,
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
                    _ = initial_peer_redial.timer.tick() => {
                        let state_guard = driver_state.lock().await;
                        initial_peer_redial.redial(
                            &mut swarm,
                            &state_guard,
                            &initial_peers,
                            &backbone_peers,
                            &mut backbone_cursor,
                            backbone_dial_count,
                        )?;
                    },
                    _ = force_peer_dial.tick() => {
                        debug!("Force dialing peers");
                        dial_peers(&mut swarm, &force_peers, false)?;
                    },
                    _ = gen2_batch_flush.tick() => {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GossipOrigin {
    Peer,
    Local,
    Chain,
    Mining,
    Kernel,
}

impl GossipOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::Local => "local",
            Self::Chain => "chain",
            Self::Mining => "mining",
            Self::Kernel => "kernel",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GossipPhase {
    InitialAcceptance,
    LocalResend,
    PendingHead,
    NewHead,
    MiningRegossip,
}

impl GossipPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::InitialAcceptance => "initial_acceptance",
            Self::LocalResend => "local_resend",
            Self::PendingHead => "pending_head",
            Self::NewHead => "new_head",
            Self::MiningRegossip => "mining_regossip",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GossipContext {
    origin: GossipOrigin,
    phase: GossipPhase,
    source: Option<PeerId>,
}

impl GossipContext {
    fn first_transaction_source(self) -> Option<PeerId> {
        if self.origin == GossipOrigin::Peer && self.phase == GossipPhase::InitialAcceptance {
            self.source
        } else {
            None
        }
    }
}

fn extract_gossip_effect_payload(
    noun_slab: &NounSlab,
) -> Result<(GossipContext, NounSlab), NockAppError> {
    let space = noun_slab.noun_space();
    let effect = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
    let envelope = effect.tail().as_cell()?;
    let context = envelope.head().as_cell()?;
    let origin = context.head();
    let origin = if origin.eq_bytes(b"peer") {
        GossipOrigin::Peer
    } else if origin.eq_bytes(b"local") {
        GossipOrigin::Local
    } else if origin.eq_bytes(b"chain") {
        GossipOrigin::Chain
    } else if origin.eq_bytes(b"mining") {
        GossipOrigin::Mining
    } else if origin.eq_bytes(b"kernel") {
        GossipOrigin::Kernel
    } else {
        return Err(NockAppError::OtherError(String::from(
            "Invalid gossip producer origin",
        )));
    };
    let context_tail = context.tail().as_cell()?;
    let phase = context_tail.head();
    let phase = if phase.eq_bytes(b"initial-acceptance") {
        GossipPhase::InitialAcceptance
    } else if phase.eq_bytes(b"local-resend") {
        GossipPhase::LocalResend
    } else if phase.eq_bytes(b"pending-head") {
        GossipPhase::PendingHead
    } else if phase.eq_bytes(b"new-head") {
        GossipPhase::NewHead
    } else if phase.eq_bytes(b"mining-regossip") {
        GossipPhase::MiningRegossip
    } else {
        return Err(NockAppError::OtherError(String::from(
            "Invalid gossip producer phase",
        )));
    };
    let source = context_tail.tail();
    let source = match source.as_atom() {
        Ok(atom) => {
            if atom.as_u64()? != 0 {
                return Err(NockAppError::OtherError(String::from(
                    "Invalid gossip source unit",
                )));
            }
            None
        }
        Err(_) => {
            let unit = source.as_cell()?;
            if unit.head().as_atom()?.as_u64()? != 0 {
                return Err(NockAppError::OtherError(String::from(
                    "Invalid gossip source unit",
                )));
            }
            Some(PeerId::from_noun(unit.tail().noun(), &space)?)
        }
    };
    if (origin == GossipOrigin::Peer) != source.is_some() {
        return Err(NockAppError::OtherError(String::from(
            "Gossip source does not match producer origin",
        )));
    }
    let fact = envelope.tail().as_cell()?;
    let version = fact
        .head()
        .as_atom()
        .map_err(|_| NockAppError::OtherError(String::from("Malformed gossip fact version")))?
        .as_u64()?;
    if version != FACT_POKE_VERSION {
        return Err(NockAppError::OtherError(format!(
            "Unsupported gossip fact version {version}"
        )));
    }
    let mut payload_slab = NounSlab::new();
    payload_slab.copy_into(fact.tail().noun(), &space);
    Ok((
        GossipContext {
            origin,
            phase,
            source,
        },
        payload_slab,
    ))
}

static GOSSIP_EFFECT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn gossip_payload_span(
    context: GossipContext,
    kind: &'static str,
    content_id: &str,
) -> tracing::Span {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return tracing::Span::none();
    }
    let effect_sequence = GOSSIP_EFFECT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let effect_observed_monotonic_us = traffic_cop::observation_time_us();
    let source_peer = context.source.map(|peer| peer.to_string());
    tracing::debug_span!(
        "gossip_payload",
        effect_sequence,
        effect_observed_monotonic_us,
        content_id,
        kind,
        origin = context.origin.as_str(),
        phase = context.phase.as_str(),
        source_peer = source_peer.as_deref().unwrap_or("None"),
    )
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

pub(super) enum LocalPeerAbuseAddress {
    Connection(ConnectionId),
    Address(Multiaddr),
    Peer,
    Unattributed,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn record_local_peer_abuse(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    peer_exclusions: &PeerExclusions,
    peer_id: PeerId,
    address: LocalPeerAbuseAddress,
    kind: LocalPeerAbuseKind,
    severity: LocalPeerAbuseSeverity,
    block_peer: bool,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    record_local_peer_abuse_with_dispatcher(
        &mut swarm_actions, driver_state, metrics, peer_exclusions, peer_id, address, kind,
        severity, block_peer,
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
    address: LocalPeerAbuseAddress,
    kind: LocalPeerAbuseKind,
    severity: LocalPeerAbuseSeverity,
    block_peer: bool,
) -> Result<(), NockAppError> {
    metrics.local_peer_abuse_recorded.increment();
    if peer_exclusions.record_peer_request_failure(peer_id) {
        metrics.request_peer_cooldowns_created.increment();
    }

    let resolved_address = match address {
        LocalPeerAbuseAddress::Address(address) => Some(address),
        LocalPeerAbuseAddress::Connection(connection_id) => {
            driver_state.lock().await.connection_address(connection_id)
        }
        LocalPeerAbuseAddress::Peer if severity == LocalPeerAbuseSeverity::Strong => {
            driver_state.lock().await.peer_first_address(&peer_id)
        }
        LocalPeerAbuseAddress::Peer | LocalPeerAbuseAddress::Unattributed => None,
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
            let (context, tail_slab) = extract_gossip_effect_payload(&noun_slab)?;
            let (is_heard_block_gossip, is_heard_tx_gossip) = {
                let space = tail_slab.noun_space();
                let payload = unsafe { *tail_slab.root() }.in_space(&space).as_cell()?;
                (
                    payload.head().eq_bytes(b"heard-block"),
                    payload.head().eq_bytes(b"heard-tx"),
                )
            };
            let transaction_source = if is_heard_tx_gossip {
                context.first_transaction_source()
            } else {
                None
            };
            let gossip_block_id = if is_heard_block_gossip {
                let gossip_space = tail_slab.noun_space();
                let gossip_noun = unsafe { *tail_slab.root() };
                let block_id = (|| -> Result<String, NockAppError> {
                    let page = gossip_noun.in_space(&gossip_space).as_cell()?.tail();
                    let block_id = block_id_from_page(page)?;
                    tip5_hash_to_base58(block_id.noun(), &gossip_space)
                })();
                match block_id {
                    Ok(block_id) => Some(block_id),
                    Err(err) => {
                        debug!(error = %err, "Could not identify heard-block gossip source");
                        None
                    }
                }
            } else {
                None
            };
            let block_echo_sources = if is_heard_block_gossip {
                let mut state_guard = driver_state.lock().await;
                trace!("Gossip effect for heard-block, clearing block and elders cache");
                state_guard.clear_block_and_elders_responses();
                state_guard.clear_elders_negative_cache();
                gossip_block_id
                    .as_deref()
                    .and_then(|block_id| state_guard.block_echo_sources(block_id).cloned())
            } else {
                None
            };
            let span = if tracing::enabled!(tracing::Level::DEBUG) {
                if let Some(block_id) = gossip_block_id.as_deref() {
                    gossip_payload_span(context, "block", block_id)
                } else if is_heard_tx_gossip {
                    let space = tail_slab.noun_space();
                    let tx_id = (|| -> Result<String, NockAppError> {
                        let raw = unsafe { *tail_slab.root() }
                            .in_space(&space)
                            .as_cell()?
                            .tail();
                        tip5_hash_to_base58(tx_id_from_raw_tx(raw)?.noun(), &space)
                    })();
                    match tx_id {
                        Ok(tx_id) => gossip_payload_span(context, "tx", &tx_id),
                        Err(error) => {
                            debug!(%error, "Could not identify transaction gossip payload");
                            tracing::Span::none()
                        }
                    }
                } else {
                    tracing::Span::none()
                }
            } else {
                tracing::Span::none()
            };

            let gossip_message = ByteBuf::from(tail_slab.jam().as_ref());
            debug!("Gossiping to {} peers", connected_peers.len());
            for peer_id in connected_peers {
                if transaction_source == Some(peer_id)
                    || block_echo_sources
                        .as_ref()
                        .is_some_and(|sources| sources.contains(&peer_id))
                {
                    if !span.is_disabled() {
                        span.in_scope(|| {
                            debug!(
                                monotonic_us = traffic_cop::observation_time_us(),
                                destination_peer = %peer_id,
                                "Gossip payload skipped source"
                            )
                        });
                    }
                    continue;
                }
                swarm_actions
                    .dispatch(SwarmAction::SendGossip {
                        peer_id,
                        message: gossip_message.clone(),
                        span: span.clone(),
                    })
                    .await
                    .map_err(|_e| {
                        NockAppError::OtherError(String::from("Failed to send gossip request"))
                    })?;
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
            let mut elders_target_peer = None;
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
                        // Hoon encodes peer IDs as base58 text, not raw multihash bytes.
                        let elders_cell = block_cell.tail().as_cell()?;
                        let elders_block_id_noun = elders_cell.head().noun();
                        let peer_id_noun = elders_cell.tail();
                        peer_id_noun.as_atom()?;
                        match PeerId::from_noun(peer_id_noun.noun(), &space) {
                            Ok(peer_id) => {
                                match tip5_hash_to_base58_stack(
                                    &mut noun_slab, elders_block_id_noun, &space,
                                ) {
                                    Ok(block_id) => {
                                        elders_cooldown_key =
                                            Some(format!("{block_id}:{}", peer_id.to_base58()));
                                        elders_target_peer = Some(peer_id);
                                    }
                                    Err(err) => {
                                        warn!(
                                            peer = %peer_id,
                                            error = %err,
                                            "Dropping elders request whose block id cannot be keyed"
                                        );
                                        metrics.elders_requests_suppressed.increment();
                                        return Ok(());
                                    }
                                }
                                vec![peer_id]
                            }
                            Err(err) => {
                                warn!(
                                    error = %err,
                                    "Failed to parse elders source peer; falling back to connected peers"
                                );
                                is_limited_request = true;
                                connected_peers.clone()
                            }
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
                let Some(elders_peer) = elders_target_peer else {
                    debug!(cooldown_key, "Dropping elders request with no target peer");
                    return Ok(());
                };
                let mut state_guard = driver_state.lock().await;
                if !state_guard.should_send_elders_request(
                    elders_peer,
                    cooldown_key,
                    std::time::Instant::now(),
                    crate::p2p_state::ELDERS_REQUEST_COOLDOWN,
                    crate::p2p_state::ELDERS_INFLIGHT_TTL,
                ) {
                    drop(state_guard);
                    debug!(
                        cooldown_key,
                        peer = %elders_peer,
                        "Suppressing elders request: duplicate, or the peer's slot is in use"
                    );
                    metrics.elders_requests_suppressed.increment();
                    return Ok(());
                }
            }

            if let Some(tx_id) = raw_tx_reopen_id {
                let mut state_guard = driver_state.clone().lock_owned().await;
                state_guard.reopen_seen_tx(&tx_id);
                preferred_peers = state_guard.get_peers_for_tx_id(&tx_id);
            }

            if let Some(height) = requested_block_height {
                {
                    let mut state_guard = driver_state.lock().await;
                    state_guard.note_kernel_block_height_requested(height);
                }
                let (
                    ready_cache_hit,
                    deferred_cache_hit,
                    prefetch_covers,
                    frontier,
                    future_gossip_height,
                ) = {
                    let state_guard = driver_state.lock().await;
                    (
                        state_guard.has_ready_deferred_block_at_height(height),
                        state_guard.has_deferred_block_at_height(height),
                        state_guard.is_prefetch_inflight_covering_height(height),
                        state_guard.first_negative,
                        state_guard.retained_future_gossip_height(height),
                    )
                };
                if ready_cache_hit {
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
                } else {
                    metrics.prefetch_cache_misses_total.increment();
                }
                if prefetch_config.enabled && deferred_cache_hit && !ready_cache_hit {
                    trace!(
                        height,
                        "Buffered block-by-height request is ahead of frontier; dispatching outbound request"
                    );
                }

                let kernel_demand_depth = if frontier > 0 && height >= frontier {
                    height.saturating_sub(frontier).saturating_add(1)
                } else {
                    0
                };
                // Future gossip is buffered before the kernel can observe it.
                // Its retained height is only an advisory catch-up hint for a
                // genuine demand at the frontier, never validation or a new
                // request start. Keep the ordinary deep-demand path unchanged.
                let retained_gossip_depth = future_gossip_height
                    .map(|future| future.saturating_sub(height).saturating_add(1))
                    .unwrap_or(0);
                let kernel_demand_prefetch = kernel_demand_depth.max(retained_gossip_depth)
                    >= prefetch_config.kernel_demand_threshold;
                let prefetch_eligible = prefetch_config.enabled
                    && !prefetch_covers
                    && height > 0
                    && kernel_demand_prefetch
                    && prefetch_config.window_initial > 0
                    && prefetch_config.window_max > 0;

                if prefetch_config.enabled && prefetch_covers {
                    metrics.prefetch_duplicate_range_avoided_total.increment();
                    trace!(
                        height,
                        "Skipping duplicate range prefetch: existing prefetch already covers height"
                    );
                }

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
                                        frontier,
                                        kernel_demand_depth,
                                        future_gossip_height = ?future_gossip_height,
                                        retained_gossip_depth,
                                        kernel_demand_threshold =
                                            prefetch_config.kernel_demand_threshold,
                                        estimated_response_bytes_per_block =
                                            window_selection.estimated_response_bytes_per_block,
                                        estimate_source = window_selection.estimate_source,
                                        target_response_bytes = window_selection.target_response_bytes,
                                        response_budget_bytes = window_selection.response_budget_bytes,
                                        probe = prefetch_peer.probe,
                                        "Issuing kernel-demand range prefetch alongside singleton block-by-height"
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
                        PrefetchPeerSelection::NoCandidate => {
                            metrics.prefetch_no_eligible_peer_total.increment();
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
                let selected_peers = state_guard
                    .select_block_height_request_peer(candidate_peers, &peer_exclusions)
                    .into_iter()
                    .collect::<Vec<_>>();
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
            let (peer_id, failed_pow_witness) = {
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

                let peer_id = PeerId::from_str(&peer_id_str).map_err(|_| {
                    NockAppError::IoError(std::io::Error::other("Invalid peer ID format"))
                })?;
                (
                    peer_id,
                    liar_peer_cell.tail().eq_bytes(b"failed-pow-witness"),
                )
            };

            // A v5 witness failure is objective cryptographic misbehavior, but
            // unlike `%failed-pow-check` it cannot poison the semantic block ID:
            // another proof envelope for that ID may be valid. Escalate the
            // sender through the address/IP abuse path so rotating peer IDs on
            // one endpoint cannot buy an unbounded sequence of full verifier
            // runs. Other liar-peer causes remain peer-scoped because protocol
            // skew around an upgrade boundary can produce them honestly.
            let severity = if failed_pow_witness {
                LocalPeerAbuseSeverity::Strong
            } else {
                LocalPeerAbuseSeverity::Weak
            };
            record_local_peer_abuse_with_dispatcher(
                swarm_actions,
                &driver_state,
                metrics.as_ref(),
                &peer_exclusions,
                peer_id,
                LocalPeerAbuseAddress::Peer,
                LocalPeerAbuseKind::LiarPeer,
                severity,
                true,
            )
            .await?;
        }
        EffectType::LiarBlockId => {
            let (block_id_str, failed_pow_check) = {
                let space = noun_slab.noun_space();
                let effect_cell = unsafe { *noun_slab.root() }.in_space(&space).as_cell()?;
                let liar_block_cell = effect_cell.tail().as_cell().map_err(|_| {
                    NockAppError::IoError(std::io::Error::other(
                        "Expected block ID cell in liar-block-id effect",
                    ))
                })?;
                let block_id = liar_block_cell.head().noun();
                (
                    tip5_hash_to_base58_stack(&mut noun_slab, block_id, &space)?,
                    liar_block_cell.tail().eq_bytes(b"failed-pow-check"),
                )
            };

            // Narrow the driver_state guard to just the state mutation.
            // holding it across the `swarm_tx.send(BlockPeer).await` loop
            // below would close a reentrant-through-channel cycle: only
            // the main libp2p select loop drains `swarm_tx`, and that same
            // loop takes `driver_state.lock().await` in ~8 of its branches.
            // Under load a full swarm_tx could force the sender to wait on
            // the select loop, which would itself be waiting on this lock.
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
                let peers = state_guard.process_bad_block_id_str(&block_id_str);
                if failed_pow_check {
                    state_guard.record_rejected_pow_block(&block_id_str);
                }
                peers
            };

            // A failed proof is objective cryptographic misbehavior. Apply the
            // address/IP escalation path so peer-ID rotation cannot buy another
            // expensive verification from the same endpoint. Other liar causes
            // remain peer-scoped because honest protocol-version skew can produce
            // them around an upgrade boundary.
            let severity = if failed_pow_check {
                LocalPeerAbuseSeverity::Strong
            } else {
                LocalPeerAbuseSeverity::Weak
            };
            // Ban each peer that sent this block
            for peer_id in peers_to_ban {
                record_local_peer_abuse_with_dispatcher(
                    swarm_actions,
                    &driver_state,
                    metrics.as_ref(),
                    &peer_exclusions,
                    peer_id,
                    LocalPeerAbuseAddress::Peer,
                    LocalPeerAbuseKind::LiarBlockId,
                    severity,
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
                    state_guard.track_block_id_str_and_peer(block_id_str, peer_id);
                }
                TrackEffect::Remove { block_id_str } => {
                    state_guard.validate_and_untrack_block_sources(block_id_str);
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
                    debug!(monotonic_us = traffic_cop::observation_time_us(),
                        block_id = %block_id_str, block_height,
                        "Kernel observed block");
                    // The `state_guard` MUST be released before the
                    // `swarm_tx.send(FlushDeferredHeardBlocks)` below. The
                    // inner `{ ... }` block expression provides that scope.
                    // Release the state guard before sending queued swarm actions.
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
                            // A height-bearing %seen %block is the kernel's acceptance
                            // signal. An Ack from routing the response alone is not.
                            state_guard.clear_block_height_attempted_peers(block_height);
                            if state_guard.first_negative <= block_height {
                                state_guard
                                    .record_accepted_chain_progress(&block_id_str, block_height);
                                state_guard.advance_seen_frontier(block_height);
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
                    debug!(monotonic_us = traffic_cop::observation_time_us(),
                        tx_id = %tx_id_str, "Kernel observed transaction");
                    state_guard.finish_processing_tx_seen(&tx_id_str);
                    state_guard.remove_tx_id_hint(&tx_id_str);
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
    configured: bool,
) -> Result<(), NockAppError> {
    let mut rng = rand::rng();

    let mut shuffled_peers = peers.to_vec();
    shuffled_peers.shuffle(&mut rng);

    for peer in shuffled_peers {
        if configured {
            if swarm.behaviour().recovery.pending_at(&peer) {
                continue;
            }
            // Startup attempts share pending admission with subsequent recovery.
            if !dial_recovery_peer(swarm, peer) {
                break;
            }
        } else {
            debug!("Dialing peer: {}", peer);
            let _ = swarm.dial(peer).map_err(log_dial_error);
        }
    }
    Ok(())
}

/// Preserve concrete errors hidden by transparent I/O wrappers.
fn transport_error_source<'a>(
    error: &'a (dyn Error + 'static),
) -> Option<&'a (dyn Error + 'static)> {
    if let Some(io) = error.downcast_ref::<std::io::Error>() {
        if let Some(inner) = io.get_ref() {
            return Some(inner);
        }
    }
    if let Some(libp2p::quic::Error::Io(io)) = error.downcast_ref::<libp2p::quic::Error>() {
        return Some(io);
    }
    error.source()
}

/// Walks an error's concrete wrapper and `source()` chain looking for an [`std::io::Error`] of
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
        cur = transport_error_source(e);
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

// `start_swarm` combines the builder's empty transport with DNS-wrapped QUIC.
// Keep this type tied to that composition so its outer timeout stays typed.
type SwarmTransportError = either::Either<std::io::Error, libp2p::dns::Error<libp2p::quic::Error>>;

/// Only positively identified reachability failures use configured backoff.
/// Unknown errors, protocol errors and identity failures retain their policy.
fn error_chain_is_transport_outage(error: &(dyn Error + 'static)) -> bool {
    let mut outage = false;
    let mut source = Some(error);
    while let Some(error) = source {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::NotConnected => outage = true,
                // `Other` may wrap the concrete transport error.
                std::io::ErrorKind::Other => {}
                _ => return false,
            }
        }
        if matches!(
            error.downcast_ref::<libp2p::quic::Error>(),
            Some(libp2p::quic::Error::HandshakeTimedOut)
        ) {
            outage = true;
        }
        if matches!(
            error.downcast_ref::<
                libp2p::core::transport::timeout::TransportTimeoutError<SwarmTransportError>,
            >(),
            Some(libp2p::core::transport::timeout::TransportTimeoutError::Timeout)
        ) {
            outage = true;
        }
        source = transport_error_source(error);
    }
    outage
}

fn ping_failure_is_transport_outage(error: &ping::Failure) -> bool {
    match error {
        ping::Failure::Timeout => true,
        ping::Failure::Unsupported => false,
        ping::Failure::Other { error } => error_chain_is_transport_outage(error.as_ref()),
    }
}

async fn handle_ping_event(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
    event: ping::Event,
    failed_pings_before_close: u64,
    initial: &[Multiaddr],
    backbone: &[Multiaddr],
) {
    let ping::Event {
        peer,
        connection,
        result,
    } = event;
    let mut state_guard = driver_state.lock().await;
    match result {
        Ok(duration) => {
            if !state_guard.ping_succeeded(connection, peer) {
                return;
            }
            let connection_address = state_guard.connection_address(connection);
            if let Some(ip) = connection_address
                .as_ref()
                .and_then(|addr| addr.canonical_ip_addr())
            {
                peer_exclusions.record_positive_ip(ip);
            }
            log_ping_success(peer, connection_address, duration);
        }
        Err(error) => {
            let Some(failures) = state_guard.ping_failed(connection, peer) else {
                return;
            };
            let connection_address = state_guard.connection_address(connection);
            let configured_outage = ping_failure_is_transport_outage(&error)
                && connection_address.as_ref().is_some_and(|addr| {
                    state_guard.is_configured_transport_endpoint(
                        addr,
                        Some(peer),
                        initial,
                        backbone,
                    )
                });
            log_ping_failure(peer, connection_address.clone(), error);
            drop(state_guard);
            if let Some(addr) = connection_address.as_ref().filter(|_| !configured_outage) {
                let outcome = peer_exclusions.record_ping_failure(addr);
                record_exclusion_outcome(
                    swarm,
                    driver_state,
                    peer_exclusions,
                    metrics,
                    outcome,
                    &[peer],
                )
                .await;
            }
            if failures >= failed_pings_before_close {
                if let Some(ip) = connection_address.and_then(|c| c.canonical_ip_addr()) {
                    info!("Closing connection to {peer} on {ip} after {failures} failed pings.");
                } else {
                    info!("Closing connection to {peer} after {failures} failed pings.");
                }
                swarm.close_connection(connection);
            }
        }
    }
}

async fn handle_outgoing_connection_error(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
    error: DialError,
    expected_peer: Option<PeerId>,
    initial: &[Multiaddr],
    backbone: &[Multiaddr],
) {
    match &error {
        // The endpoint answered with an identity different from the advertised
        // one. Apply the identity-mismatch exclusion policy.
        DialError::WrongPeerId { obtained, address } => {
            let obtained = *obtained;
            let expected = p2p_peer_id(address);
            if address.canonical_ip_addr().is_none() {
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
        // PermissionDenied remains local reachability evidence regardless of
        // whether the endpoint is configured.
        DialError::Transport(addr_errs) => {
            for (addr, transport_err) in addr_errs {
                let dyn_err: &(dyn std::error::Error + 'static) = transport_err;
                let peer = expected_peer.or_else(|| p2p_peer_id(addr));
                let outcome = if chain_has_permission_denied(dyn_err) {
                    Some(peer_exclusions.record_permission_denied(addr))
                } else if error_chain_is_transport_outage(dyn_err)
                    && driver_state
                        .lock()
                        .await
                        .is_configured_transport_endpoint(addr, peer, initial, backbone)
                {
                    // The configured retry cadence owns ordinary reachability
                    // failures; do not add discovery cooldown or IP evidence.
                    None
                } else {
                    Some(peer_exclusions.record_dial_failure(addr, peer))
                };
                if let Some(outcome) = outcome {
                    let peers = peer.as_slice();
                    record_exclusion_outcome(
                        swarm, driver_state, peer_exclusions, metrics, outcome, peers,
                    )
                    .await;
                }
                trace!("Failed to dial address {}: {}", addr, transport_err);
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
                    let ip = address.canonical_ip_addr()?;
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
        gen2::increment_outbound_failure_metrics(&metrics, &error);
        debug!(
            peer = %peer,
            request_id = %request_id,
            request_shape = gen2::outbound_request_shape(&request_context.request),
            batch_items = gen2::batch_request_item_count(&request_context.request),
            retry_count = request_context.retry_count,
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
            (
                ResolverConfig::udp_and_tcp(&CLOUDFLARE),
                ResolverOpts::default(),
            )
        };

    let max_idle_timeout_millisecs = libp2p_config.max_idle_timeout_millisecs();
    let keep_alive_interval = libp2p_config.keep_alive_interval();
    let handshake_timeout = libp2p_config.handshake_timeout();
    let connection_timeout = libp2p_config.connection_timeout();
    let swarm_idle_timeout = libp2p_config.swarm_idle_timeout();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_other_transport(|keypair| {
            let mut cfg = libp2p::quic::Config::new(keypair);
            cfg.max_idle_timeout = max_idle_timeout_millisecs;
            cfg.keep_alive_interval = keep_alive_interval;
            cfg.handshake_timeout = handshake_timeout;
            let numeric =
                libp2p::quic::tokio::Transport::new(cfg).map(|(peer, connection), endpoint| {
                    (
                        peer,
                        crate::transport::ResolvedQuicMuxer::new(connection, endpoint),
                    )
                });
            libp2p::dns::tokio::Transport::custom(numeric, resolver_config, resolver_opts).map(
                |(peer, connection), endpoint| (peer, connection.for_original_endpoint(&endpoint)),
            )
        })?
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

/// Handler for identify messages.
fn identify_received(
    swarm: &mut Swarm<NockchainBehaviour>,
    observed_address_votes: &mut ObservedAddressVotes,
    peer_id: PeerId,
    connection_id: ConnectionId,
    info: libp2p::identify::Info,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
) -> Result<(), NockAppError> {
    let update = observed_address_votes.observe_at(
        swarm,
        peer_id,
        connection_id,
        &info.observed_addr,
        Instant::now(),
    );
    update.apply(swarm);
    let kad = &mut swarm.behaviour_mut().kad;
    trace!("identify received for peer {}", peer_id);
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
    Ok(())
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
