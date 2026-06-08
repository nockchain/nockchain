//! IP and endpoint-level connection hygiene.
//!
//! [`crate::behaviour::NockchainBehaviour`]'s peer-id block list cannot stop a
//! host that repeatedly advertises fresh peer IDs on the same IP. Kademlia can
//! then keep relearning `new-peer-id -> same-address` mappings from DHT query
//! responses and dial them without an application-level choke point.
//!
//! This module keeps a local, expiry-aware exclusion store. The swarm-facing
//! [`NetworkBehaviour`] hooks deny active IP exclusions before transport work,
//! while [`IpFilteredKad`] also filters Kademlia-owned dial candidates before
//! they leave the routing behaviour.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::fmt;
use std::net::IpAddr;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use libp2p::core::transport::PortUse;
use libp2p::core::{Endpoint, Multiaddr};
use libp2p::identity::PeerId;
use libp2p::kad;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{
    dummy, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
    THandlerOutEvent, ToSwarm,
};

use crate::config::PeerExclusionConfig;
use crate::p2p_util::MultiaddrExt;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum TransportKind {
    Tcp,
    Udp,
    Other,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct AddressKey {
    pub(crate) ip: IpAddr,
    pub(crate) transport: TransportKind,
    pub(crate) port: Option<u16>,
    pub(crate) expected_peer: Option<PeerId>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ExclusionReason {
    WrongPeerId,
    RepeatedWrongPeerId,
    PeerMisbehavior,
    RepeatedPeerMisbehavior,
    PermissionDenied,
    RepeatedDialFailure,
    KadSameIpCardinality,
}

impl fmt::Display for ExclusionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExclusionReason::WrongPeerId => write!(f, "wrong-peer-id"),
            ExclusionReason::RepeatedWrongPeerId => write!(f, "repeated-wrong-peer-id"),
            ExclusionReason::PeerMisbehavior => write!(f, "peer-misbehavior"),
            ExclusionReason::RepeatedPeerMisbehavior => write!(f, "repeated-peer-misbehavior"),
            ExclusionReason::PermissionDenied => write!(f, "permission-denied"),
            ExclusionReason::RepeatedDialFailure => write!(f, "repeated-dial-failure"),
            ExclusionReason::KadSameIpCardinality => write!(f, "kad-same-ip-cardinality"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AddressCooldownOutcome {
    pub(crate) key: AddressKey,
    pub(crate) address: Multiaddr,
    pub(crate) ttl: Duration,
    pub(crate) reason: ExclusionReason,
}

#[derive(Debug, Clone)]
pub(crate) struct IpExclusionOutcome {
    pub(crate) ip: IpAddr,
    pub(crate) ttl: Duration,
    pub(crate) reason: ExclusionReason,
    pub(crate) fail2ban: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExclusionOutcome {
    pub(crate) address_cooldown: Option<AddressCooldownOutcome>,
    pub(crate) ip_exclusion: Option<IpExclusionOutcome>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum EvidenceKind {
    WrongPeerId,
    PeerMisbehavior,
    DialFailure,
    PermissionDenied,
    PingFailure,
    KadCardinality,
}

#[derive(Debug, Clone)]
struct PeerHealthEvent {
    ip: IpAddr,
    port: Option<u16>,
    expected_peer: Option<PeerId>,
    obtained_peer: Option<PeerId>,
    at: Instant,
    kind: EvidenceKind,
}

#[derive(Debug, Clone)]
struct IpExclusion {
    expires_at: Instant,
    reason: ExclusionReason,
    strike_count: u32,
    last_seen: Instant,
}

#[derive(Debug, Clone)]
struct AddressExclusion {
    expires_at: Instant,
    reason: ExclusionReason,
    last_seen: Instant,
}

#[derive(Debug, Clone)]
struct PeerPenalty {
    expires_at: Instant,
    failure_count: u32,
    last_failure: Instant,
}

#[derive(Debug, Clone)]
struct IpHistory {
    last_excluded_at: Instant,
    exclusion_count: u32,
}

#[derive(Debug, Default)]
struct ExclusionState {
    ips: HashMap<IpAddr, IpExclusion>,
    addresses: HashMap<AddressKey, AddressExclusion>,
    peers: HashMap<PeerId, PeerPenalty>,
    events: VecDeque<PeerHealthEvent>,
    ip_history: HashMap<IpAddr, IpHistory>,
}

fn read_state(lock: &RwLock<ExclusionState>) -> RwLockReadGuard<'_, ExclusionState> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_state(lock: &RwLock<ExclusionState>) -> RwLockWriteGuard<'_, ExclusionState> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ExpireOutcome {
    pub(crate) ips: usize,
    pub(crate) addresses: usize,
    pub(crate) peers: usize,
}

/// Cheaply cloneable, shared exclusion state.
#[derive(Clone)]
pub(crate) struct PeerExclusions {
    inner: Arc<RwLock<ExclusionState>>,
    config: Arc<PeerExclusionConfig>,
}

impl Default for PeerExclusions {
    fn default() -> Self {
        Self::new(PeerExclusionConfig::default())
    }
}

impl PeerExclusions {
    pub(crate) fn new(config: PeerExclusionConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ExclusionState::default())),
            config: Arc::new(config),
        }
    }

    pub(crate) fn address_key(
        &self,
        addr: &Multiaddr,
        expected_peer: Option<PeerId>,
    ) -> Option<AddressKey> {
        address_key(addr, expected_peer)
    }

    pub(crate) fn is_ip_excluded(&self, ip: &IpAddr) -> bool {
        self.is_ip_excluded_at(ip, Instant::now())
    }

    pub(crate) fn is_address_excluded(
        &self,
        addr: &Multiaddr,
        expected_peer: Option<PeerId>,
    ) -> bool {
        self.is_address_excluded_at(addr, expected_peer, Instant::now())
    }

    pub(crate) fn is_ip_excluded_at(&self, ip: &IpAddr, now: Instant) -> bool {
        if !self.config.enabled || self.config.allow_ips.contains(ip) {
            return false;
        }
        read_state(&self.inner)
            .ips
            .get(ip)
            .is_some_and(|entry| entry.expires_at > now)
    }

    pub(crate) fn is_address_excluded_at(
        &self,
        addr: &Multiaddr,
        expected_peer: Option<PeerId>,
        now: Instant,
    ) -> bool {
        let Some(ip) = addr.ip_addr() else {
            return false;
        };
        if self.is_ip_excluded_at(&ip, now) {
            return true;
        }
        if !self.config.enabled || self.config.allow_ips.contains(&ip) {
            return false;
        }
        let Some(key) = address_key(addr, expected_peer) else {
            return false;
        };
        address_is_excluded(&read_state(&self.inner), key, now)
    }

    pub(crate) fn record_wrong_peer_id(
        &self,
        address: &Multiaddr,
        expected_peer: Option<PeerId>,
        obtained_peer: PeerId,
    ) -> ExclusionOutcome {
        self.record_wrong_peer_id_at(address, expected_peer, obtained_peer, Instant::now())
    }

    fn record_wrong_peer_id_at(
        &self,
        address: &Multiaddr,
        expected_peer: Option<PeerId>,
        obtained_peer: PeerId,
        now: Instant,
    ) -> ExclusionOutcome {
        let Some(key) = address_key(address, expected_peer) else {
            return ExclusionOutcome::default();
        };
        if !self.config.enabled || self.config.allow_ips.contains(&key.ip) {
            return ExclusionOutcome::default();
        }

        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        push_event(
            &mut state,
            PeerHealthEvent {
                ip: key.ip,
                port: key.port,
                expected_peer,
                obtained_peer: Some(obtained_peer),
                at: now,
                kind: EvidenceKind::WrongPeerId,
            },
            now,
            &self.config,
        );
        let address_cooldown = insert_address_cooldown(
            &mut state,
            key,
            address.clone(),
            self.config.address_cooldown(),
            ExclusionReason::WrongPeerId,
            now,
        );

        let ip_exclusion = if wrong_peer_threshold_met(&state, key.ip, now, &self.config) {
            insert_ip_exclusion(
                &mut state,
                key.ip,
                ExclusionReason::RepeatedWrongPeerId,
                now,
                &self.config,
            )
        } else {
            None
        };

        ExclusionOutcome {
            address_cooldown,
            ip_exclusion,
        }
    }

    pub(crate) fn record_peer_misbehavior(
        &self,
        address: &Multiaddr,
        peer_id: PeerId,
    ) -> ExclusionOutcome {
        self.record_peer_misbehavior_at(address, peer_id, Instant::now())
    }

    fn record_peer_misbehavior_at(
        &self,
        address: &Multiaddr,
        peer_id: PeerId,
        now: Instant,
    ) -> ExclusionOutcome {
        let Some(key) = address_key(address, Some(peer_id)) else {
            return ExclusionOutcome::default();
        };
        if !self.config.enabled || self.config.allow_ips.contains(&key.ip) {
            return ExclusionOutcome::default();
        }

        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        push_event(
            &mut state,
            PeerHealthEvent {
                ip: key.ip,
                port: key.port,
                expected_peer: Some(peer_id),
                obtained_peer: None,
                at: now,
                kind: EvidenceKind::PeerMisbehavior,
            },
            now,
            &self.config,
        );
        let address_cooldown = insert_address_cooldown(
            &mut state,
            key,
            address.clone(),
            self.config.address_cooldown(),
            ExclusionReason::PeerMisbehavior,
            now,
        );

        let ip_exclusion = if peer_threshold_met(
            &state,
            key.ip,
            EvidenceKind::PeerMisbehavior,
            now,
            &self.config,
        ) {
            insert_ip_exclusion(
                &mut state,
                key.ip,
                ExclusionReason::RepeatedPeerMisbehavior,
                now,
                &self.config,
            )
        } else {
            None
        };

        ExclusionOutcome {
            address_cooldown,
            ip_exclusion,
        }
    }

    pub(crate) fn record_permission_denied(&self, address: &Multiaddr) -> ExclusionOutcome {
        self.record_permission_denied_at(address, Instant::now())
    }

    fn record_permission_denied_at(&self, address: &Multiaddr, now: Instant) -> ExclusionOutcome {
        self.record_address_failure_at(
            address,
            None,
            EvidenceKind::PermissionDenied,
            ExclusionReason::PermissionDenied,
            self.config.permission_denied_cooldown(),
            now,
        )
    }

    pub(crate) fn record_dial_failure(
        &self,
        address: &Multiaddr,
        expected_peer: Option<PeerId>,
    ) -> ExclusionOutcome {
        self.record_dial_failure_at(address, expected_peer, Instant::now())
    }

    fn record_dial_failure_at(
        &self,
        address: &Multiaddr,
        expected_peer: Option<PeerId>,
        now: Instant,
    ) -> ExclusionOutcome {
        self.record_address_failure_at(
            address,
            expected_peer,
            EvidenceKind::DialFailure,
            ExclusionReason::RepeatedDialFailure,
            self.config.address_cooldown(),
            now,
        )
    }

    fn record_address_failure_at(
        &self,
        address: &Multiaddr,
        expected_peer: Option<PeerId>,
        kind: EvidenceKind,
        reason: ExclusionReason,
        address_ttl: Duration,
        now: Instant,
    ) -> ExclusionOutcome {
        let Some(key) = address_key(address, expected_peer) else {
            return ExclusionOutcome::default();
        };
        if !self.config.enabled || self.config.allow_ips.contains(&key.ip) {
            return ExclusionOutcome::default();
        }

        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        push_event(
            &mut state,
            PeerHealthEvent {
                ip: key.ip,
                port: key.port,
                expected_peer,
                obtained_peer: None,
                at: now,
                kind,
            },
            now,
            &self.config,
        );

        let address_cooldown =
            insert_address_cooldown(&mut state, key, address.clone(), address_ttl, reason, now);

        ExclusionOutcome {
            address_cooldown,
            ip_exclusion: None,
        }
    }

    pub(crate) fn record_ping_failure(&self, address: &Multiaddr) -> ExclusionOutcome {
        self.record_ping_failure_at(address, Instant::now())
    }

    fn record_ping_failure_at(&self, address: &Multiaddr, now: Instant) -> ExclusionOutcome {
        self.record_address_failure_at(
            address,
            None,
            EvidenceKind::PingFailure,
            ExclusionReason::RepeatedDialFailure,
            self.config.address_cooldown(),
            now,
        )
    }

    pub(crate) fn record_positive_ip(&self, ip: IpAddr) {
        if !self.config.enabled || self.config.allow_ips.contains(&ip) {
            return;
        }
        let now = Instant::now();
        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);

        let mut remaining_events = VecDeque::with_capacity(state.events.len());
        let mut removed = 0usize;
        while let Some(event) = state.events.pop_front() {
            if event.ip == ip && removed < 2 {
                removed += 1;
                continue;
            }
            remaining_events.push_back(event);
        }
        state.events = remaining_events;
    }

    pub(crate) fn record_peer_request_failure(&self, peer_id: PeerId) -> bool {
        if !self.config.enabled {
            return false;
        }
        let now = Instant::now();
        let expires_at = now + self.config.request_peer_cooldown();
        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        let was_active = state
            .peers
            .get(&peer_id)
            .is_some_and(|entry| entry.expires_at > now);
        state
            .peers
            .entry(peer_id)
            .and_modify(|entry| {
                entry.expires_at = expires_at;
                entry.failure_count = entry.failure_count.saturating_add(1);
                entry.last_failure = now;
            })
            .or_insert(PeerPenalty {
                expires_at,
                failure_count: 1,
                last_failure: now,
            });
        !was_active
    }

    pub(crate) fn record_peer_request_success(&self, peer_id: &PeerId) {
        let now = Instant::now();
        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        state.peers.remove(peer_id);
    }

    pub(crate) fn is_peer_request_cooled_down(&self, peer_id: &PeerId) -> bool {
        if !self.config.enabled {
            return false;
        }
        let now = Instant::now();
        read_state(&self.inner)
            .peers
            .get(peer_id)
            .is_some_and(|entry| entry.expires_at > now)
    }

    pub(crate) fn record_kad_cardinality(
        &self,
        ip: IpAddr,
        peer_count: usize,
        port_count: usize,
    ) -> Option<IpExclusionOutcome> {
        let now = Instant::now();
        self.record_kad_cardinality_at(ip, peer_count, port_count, now)
    }

    fn record_kad_cardinality_at(
        &self,
        ip: IpAddr,
        peer_count: usize,
        port_count: usize,
        now: Instant,
    ) -> Option<IpExclusionOutcome> {
        if !self.config.enabled || self.config.allow_ips.contains(&ip) {
            return None;
        }
        if peer_count < self.config.same_ip_kad_entry_threshold
            && port_count < self.config.same_ip_kad_entry_threshold
        {
            return None;
        }

        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        push_event(
            &mut state,
            PeerHealthEvent {
                ip,
                port: None,
                expected_peer: None,
                obtained_peer: None,
                at: now,
                kind: EvidenceKind::KadCardinality,
            },
            now,
            &self.config,
        );

        if has_recent_failure(&state, ip, now, &self.config) {
            insert_ip_exclusion(
                &mut state,
                ip,
                ExclusionReason::KadSameIpCardinality,
                now,
                &self.config,
            )
        } else {
            None
        }
    }

    pub(crate) fn expire(&self) -> ExpireOutcome {
        let now = Instant::now();
        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config)
    }

    pub(crate) fn active_ip_exclusion_count(&self) -> usize {
        let now = Instant::now();
        read_state(&self.inner)
            .ips
            .values()
            .filter(|entry| entry.expires_at > now)
            .count()
    }

    pub(crate) fn active_address_cooldown_count(&self) -> usize {
        let now = Instant::now();
        read_state(&self.inner)
            .addresses
            .values()
            .filter(|entry| entry.expires_at > now)
            .count()
    }
}

fn prune_state(
    state: &mut ExclusionState,
    now: Instant,
    config: &PeerExclusionConfig,
) -> ExpireOutcome {
    let ip_before = state.ips.len();
    state.ips.retain(|_, entry| entry.expires_at > now);
    let address_before = state.addresses.len();
    state.addresses.retain(|_, entry| entry.expires_at > now);
    let peer_before = state.peers.len();
    state.peers.retain(|_, entry| entry.expires_at > now);

    while state
        .events
        .front()
        .is_some_and(|event| event.at + config.event_history() < now)
    {
        state.events.pop_front();
    }
    while state.events.len() > config.max_exclusion_entries {
        state.events.pop_front();
    }

    state
        .ip_history
        .retain(|_, history| history.last_excluded_at + config.ip_exclusion_history() >= now);

    ExpireOutcome {
        ips: ip_before.saturating_sub(state.ips.len()),
        addresses: address_before.saturating_sub(state.addresses.len()),
        peers: peer_before.saturating_sub(state.peers.len()),
    }
}

fn push_event(
    state: &mut ExclusionState,
    event: PeerHealthEvent,
    now: Instant,
    config: &PeerExclusionConfig,
) {
    state.events.push_back(event);
    prune_state(state, now, config);
}

fn insert_address_cooldown(
    state: &mut ExclusionState,
    key: AddressKey,
    address: Multiaddr,
    ttl: Duration,
    reason: ExclusionReason,
    now: Instant,
) -> Option<AddressCooldownOutcome> {
    let expires_at = now + ttl;
    match state.addresses.get_mut(&key) {
        Some(existing) if existing.expires_at >= expires_at => {
            existing.last_seen = now;
            None
        }
        Some(existing) => {
            existing.expires_at = expires_at;
            existing.reason = reason;
            existing.last_seen = now;
            Some(AddressCooldownOutcome {
                key,
                address,
                ttl,
                reason,
            })
        }
        None => {
            state.addresses.insert(
                key,
                AddressExclusion {
                    expires_at,
                    reason,
                    last_seen: now,
                },
            );
            Some(AddressCooldownOutcome {
                key,
                address,
                ttl,
                reason,
            })
        }
    }
}

fn insert_ip_exclusion(
    state: &mut ExclusionState,
    ip: IpAddr,
    reason: ExclusionReason,
    now: Instant,
    config: &PeerExclusionConfig,
) -> Option<IpExclusionOutcome> {
    let history = state.ip_history.entry(ip).or_insert(IpHistory {
        last_excluded_at: now,
        exclusion_count: 0,
    });
    let recent_recurrence = history.exclusion_count > 0
        && history.last_excluded_at + config.ip_exclusion_history() >= now;
    history.exclusion_count = if recent_recurrence {
        history.exclusion_count.saturating_add(1)
    } else {
        1
    };
    history.last_excluded_at = now;

    let ttl = if recent_recurrence {
        config.ip_extended_exclusion()
    } else {
        config.ip_exclusion()
    }
    .min(config.max_auto_exclusion());
    let expires_at = now + ttl;

    match state.ips.get_mut(&ip) {
        Some(existing) if existing.expires_at >= expires_at => {
            existing.last_seen = now;
            existing.strike_count = existing.strike_count.saturating_add(1);
            None
        }
        Some(existing) => {
            existing.expires_at = expires_at;
            existing.reason = reason;
            existing.last_seen = now;
            existing.strike_count = existing.strike_count.saturating_add(1);
            Some(IpExclusionOutcome {
                ip,
                ttl,
                reason,
                fail2ban: config.fail2ban_on_temp_exclusion,
            })
        }
        None => {
            state.ips.insert(
                ip,
                IpExclusion {
                    expires_at,
                    reason,
                    strike_count: 1,
                    last_seen: now,
                },
            );
            Some(IpExclusionOutcome {
                ip,
                ttl,
                reason,
                fail2ban: config.fail2ban_on_temp_exclusion,
            })
        }
    }
}

fn peer_threshold_met(
    state: &ExclusionState,
    ip: IpAddr,
    kind: EvidenceKind,
    now: Instant,
    config: &PeerExclusionConfig,
) -> bool {
    let mut peers = HashSet::new();
    let mut obtained_peers = HashSet::new();
    let mut ports = HashSet::new();
    for event in state.events.iter().filter(|event| {
        event.ip == ip && event.kind == kind && event.at + config.evidence_window() >= now
    }) {
        if let Some(peer) = event.expected_peer {
            peers.insert(peer);
        }
        if let Some(peer) = event.obtained_peer {
            obtained_peers.insert(peer);
        }
        if let Some(port) = event.port {
            ports.insert(port);
        }
    }
    peers.len() >= config.wrong_peer_id_ip_threshold
        || obtained_peers.len() >= config.wrong_peer_id_ip_threshold
        || ports.len() >= config.wrong_peer_id_ip_threshold
}

fn wrong_peer_threshold_met(
    state: &ExclusionState,
    ip: IpAddr,
    now: Instant,
    config: &PeerExclusionConfig,
) -> bool {
    peer_threshold_met(state, ip, EvidenceKind::WrongPeerId, now, config)
}

fn has_recent_failure(
    state: &ExclusionState,
    ip: IpAddr,
    now: Instant,
    config: &PeerExclusionConfig,
) -> bool {
    state.events.iter().any(|event| {
        event.ip == ip
            && event.at + config.evidence_window() >= now
            && matches!(
                event.kind,
                EvidenceKind::WrongPeerId
                    | EvidenceKind::PeerMisbehavior
                    | EvidenceKind::DialFailure
                    | EvidenceKind::PermissionDenied
                    | EvidenceKind::PingFailure
            )
    })
}

fn address_key(addr: &Multiaddr, expected_peer: Option<PeerId>) -> Option<AddressKey> {
    let mut ip = None;
    let mut transport = TransportKind::Other;
    let mut port = None;
    let mut peer = expected_peer;

    for protocol in addr.iter() {
        match protocol {
            Protocol::Ip4(addr) => ip = Some(IpAddr::V4(addr)),
            Protocol::Ip6(addr) => ip = Some(IpAddr::V6(addr)),
            Protocol::Tcp(p) => {
                transport = TransportKind::Tcp;
                port = Some(p);
            }
            Protocol::Udp(p) => {
                transport = TransportKind::Udp;
                port = Some(p);
            }
            Protocol::P2p(peer_id) if peer.is_none() => peer = Some(peer_id),
            _ => {}
        }
    }

    ip.map(|ip| AddressKey {
        ip,
        transport,
        port,
        expected_peer: peer,
    })
}

fn address_is_excluded(state: &ExclusionState, key: AddressKey, now: Instant) -> bool {
    if state
        .addresses
        .get(&key)
        .is_some_and(|entry| entry.expires_at > now)
    {
        return true;
    }

    if key.expected_peer.is_none() {
        return false;
    }

    let wildcard_key = AddressKey {
        expected_peer: None,
        ..key
    };
    state
        .addresses
        .get(&wildcard_key)
        .is_some_and(|entry| entry.expires_at > now)
}

/// A connection was refused because its remote endpoint is under exclusion.
#[derive(Debug)]
pub(crate) struct BlockedEndpoint {
    addr: Multiaddr,
}

impl fmt::Display for BlockedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "endpoint {} is temporarily excluded", self.addr)
    }
}

impl std::error::Error for BlockedEndpoint {}

fn enforce_not_excluded(
    exclusions: &PeerExclusions,
    addr: &Multiaddr,
    expected_peer: Option<PeerId>,
) -> Result<(), ConnectionDenied> {
    if exclusions.is_address_excluded(addr, expected_peer) {
        return Err(ConnectionDenied::new(BlockedEndpoint {
            addr: addr.clone(),
        }));
    }
    Ok(())
}

fn enforce_ip_not_excluded(
    exclusions: &PeerExclusions,
    addr: &Multiaddr,
) -> Result<(), ConnectionDenied> {
    if addr
        .ip_addr()
        .is_some_and(|ip| exclusions.is_ip_excluded(&ip))
    {
        return Err(ConnectionDenied::new(BlockedEndpoint {
            addr: addr.clone(),
        }));
    }
    Ok(())
}

/// Kademlia plus IP/endpoint filtering for the addresses Kademlia contributes
/// to dials.
pub(crate) struct IpFilteredKad {
    inner: kad::Behaviour<kad::store::MemoryStore>,
    exclusions: PeerExclusions,
}

impl IpFilteredKad {
    pub(crate) fn new(
        inner: kad::Behaviour<kad::store::MemoryStore>,
        exclusions: PeerExclusions,
    ) -> Self {
        Self { inner, exclusions }
    }

    fn is_allowed_addr(&self, addr: &Multiaddr, expected_peer: Option<PeerId>) -> bool {
        !self.exclusions.is_address_excluded(addr, expected_peer)
    }
}

impl Deref for IpFilteredKad {
    type Target = kad::Behaviour<kad::store::MemoryStore>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for IpFilteredKad {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl NetworkBehaviour for IpFilteredKad {
    type ConnectionHandler =
        <kad::Behaviour<kad::store::MemoryStore> as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = <kad::Behaviour<kad::store::MemoryStore> as NetworkBehaviour>::ToSwarm;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let addresses = self.inner.handle_pending_outbound_connection(
            connection_id, maybe_peer, addresses, effective_role,
        )?;
        Ok(addresses
            .into_iter()
            .filter(|addr| self.is_allowed_addr(addr, maybe_peer))
            .collect())
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        enforce_not_excluded(&self.exclusions, addr, Some(peer))?;
        self.inner.handle_established_outbound_connection(
            connection_id, peer, addr, role_override, port_use,
        )
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner
            .on_connection_handler_event(peer_id, connection_id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        self.inner.poll(cx)
    }
}

/// A [`NetworkBehaviour`] that denies IP-excluded connections and applies
/// endpoint cooldowns to outbound dials.
pub(crate) struct Behaviour {
    exclusions: PeerExclusions,
}

impl Behaviour {
    pub(crate) fn new(exclusions: PeerExclusions) -> Self {
        Self { exclusions }
    }

    fn enforce(
        &self,
        addr: &Multiaddr,
        expected_peer: Option<PeerId>,
    ) -> Result<(), ConnectionDenied> {
        enforce_not_excluded(&self.exclusions, addr, expected_peer)
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        _: ConnectionId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        enforce_ip_not_excluded(&self.exclusions, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        enforce_ip_not_excluded(&self.exclusions, remote_addr)?;
        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        _: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        if addresses.is_empty() {
            return Ok(vec![]);
        }

        if addresses
            .iter()
            .any(|addr| !self.exclusions.is_address_excluded(addr, maybe_peer))
        {
            return Ok(vec![]);
        }

        self.enforce(&addresses[0], maybe_peer)?;
        Ok(vec![])
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        _: Endpoint,
        _: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.enforce(addr, Some(peer))?;
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, _event: FromSwarm) {}

    fn on_connection_handler_event(
        &mut self,
        _: PeerId,
        _: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};

    use libp2p::swarm::NetworkBehaviour;

    use crate::ip_block::*;

    fn quic_addr(ip: Ipv4Addr, port: u16) -> Multiaddr {
        format!("/ip4/{ip}/udp/{port}/quic-v1")
            .parse()
            .expect("valid multiaddr")
    }

    fn config() -> PeerExclusionConfig {
        PeerExclusionConfig {
            address_cooldown_secs: 60,
            ip_exclusion_secs: 120,
            ip_extended_exclusion_secs: 360,
            evidence_window_secs: 60,
            ip_exclusion_history_secs: 300,
            permission_denied_cooldown_secs: 30,
            wrong_peer_id_ip_threshold: 3,
            same_ip_kad_entry_threshold: 3,
            max_auto_exclusion_secs: 360,
            max_exclusion_entries: 128,
            request_peer_cooldown_secs: 60,
            ..PeerExclusionConfig::default()
        }
    }

    #[test]
    fn exclusion_is_shared_across_clones_and_expires() {
        let exclusions = PeerExclusions::new(config());
        let clone = exclusions.clone();
        let ip: IpAddr = Ipv4Addr::new(15, 235, 216, 78).into();
        let now = Instant::now();

        assert!(!clone.is_ip_excluded_at(&ip, now));
        let mut state = exclusions.inner.write().expect("lock");
        let outcome = insert_ip_exclusion(
            &mut state,
            ip,
            ExclusionReason::RepeatedWrongPeerId,
            now,
            exclusions.config.as_ref(),
        );
        drop(state);

        assert!(outcome.is_some());
        assert!(clone.is_ip_excluded_at(&ip, now + Duration::from_secs(1)));
        assert!(!clone.is_ip_excluded_at(&ip, now + Duration::from_secs(121)));
    }

    #[test]
    fn address_cooldown_does_not_block_new_identity_on_same_endpoint() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let old_peer = PeerId::random();
        let new_peer = PeerId::random();
        let obtained = PeerId::random();

        let outcome = exclusions.record_wrong_peer_id_at(&addr, Some(old_peer), obtained, now);

        assert!(outcome.address_cooldown.is_some());
        assert!(exclusions.is_address_excluded_at(&addr, Some(old_peer), now));
        assert!(!exclusions.is_address_excluded_at(&addr, Some(new_peer), now));
        assert!(!exclusions.is_address_excluded_at(&addr, None, now));
    }

    #[test]
    fn wildcard_address_cooldown_blocks_peer_specific_lookup() {
        let exclusions = PeerExclusions::new(config());
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let peer = PeerId::random();

        let outcome = exclusions.record_permission_denied(&addr);

        assert!(outcome.address_cooldown.is_some());
        assert!(exclusions.is_address_excluded(&addr, None));
        assert!(exclusions.is_address_excluded(&addr, Some(peer)));
    }

    #[test]
    fn repeated_wrong_peer_ids_trigger_ip_exclusion() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);

        let first = exclusions.record_wrong_peer_id_at(
            &quic_addr(ip, 3602),
            Some(PeerId::random()),
            PeerId::random(),
            now,
        );
        let second = exclusions.record_wrong_peer_id_at(
            &quic_addr(ip, 3603),
            Some(PeerId::random()),
            PeerId::random(),
            now + Duration::from_secs(1),
        );
        let third = exclusions.record_wrong_peer_id_at(
            &quic_addr(ip, 3604),
            Some(PeerId::random()),
            PeerId::random(),
            now + Duration::from_secs(2),
        );

        assert!(first.ip_exclusion.is_none());
        assert!(second.ip_exclusion.is_none());
        assert!(third.ip_exclusion.is_some());
        assert!(exclusions.is_ip_excluded_at(&IpAddr::V4(ip), now + Duration::from_secs(3)));
    }

    #[test]
    fn peer_misbehavior_cools_precise_address_first() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let bad_peer = PeerId::random();
        let clean_peer = PeerId::random();

        let outcome = exclusions.record_peer_misbehavior_at(&addr, bad_peer, now);

        assert!(outcome.address_cooldown.is_some());
        assert!(outcome.ip_exclusion.is_none());
        assert!(exclusions.is_address_excluded_at(&addr, Some(bad_peer), now));
        assert!(!exclusions.is_address_excluded_at(&addr, Some(clean_peer), now));
    }

    #[test]
    fn repeated_peer_misbehavior_triggers_ip_exclusion() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);

        let first =
            exclusions.record_peer_misbehavior_at(&quic_addr(ip, 3602), PeerId::random(), now);
        let second = exclusions.record_peer_misbehavior_at(
            &quic_addr(ip, 3603),
            PeerId::random(),
            now + Duration::from_secs(1),
        );
        let third = exclusions.record_peer_misbehavior_at(
            &quic_addr(ip, 3604),
            PeerId::random(),
            now + Duration::from_secs(2),
        );

        assert!(first.ip_exclusion.is_none());
        assert!(second.ip_exclusion.is_none());
        assert!(third.ip_exclusion.is_some());
        assert_eq!(
            third.ip_exclusion.as_ref().map(|outcome| outcome.reason),
            Some(ExclusionReason::RepeatedPeerMisbehavior)
        );
        assert!(exclusions.is_ip_excluded_at(&IpAddr::V4(ip), now + Duration::from_secs(3)));
    }

    #[test]
    fn repeated_obtained_wrong_peer_ids_trigger_ip_exclusion() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);
        let addr = quic_addr(ip, 3602);
        let expected = PeerId::random();

        let first =
            exclusions.record_wrong_peer_id_at(&addr, Some(expected), PeerId::random(), now);
        let second = exclusions.record_wrong_peer_id_at(
            &addr,
            Some(expected),
            PeerId::random(),
            now + Duration::from_secs(1),
        );
        let third = exclusions.record_wrong_peer_id_at(
            &addr,
            Some(expected),
            PeerId::random(),
            now + Duration::from_secs(2),
        );

        assert!(first.ip_exclusion.is_none());
        assert!(second.ip_exclusion.is_none());
        assert!(third.ip_exclusion.is_some());
        assert!(exclusions.is_ip_excluded_at(&IpAddr::V4(ip), now + Duration::from_secs(3)));
    }

    #[test]
    fn recurrent_ip_exclusion_uses_extended_ttl_with_cap() {
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            wrong_peer_id_ip_threshold: 1,
            ip_exclusion_secs: 10,
            ip_extended_exclusion_secs: 60,
            max_auto_exclusion_secs: 30,
            ip_exclusion_history_secs: 300,
            ..config()
        });
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);
        let addr = quic_addr(ip, 3602);

        let first = exclusions.record_wrong_peer_id_at(
            &addr,
            Some(PeerId::random()),
            PeerId::random(),
            now,
        );
        assert_eq!(
            first.ip_exclusion.as_ref().map(|outcome| outcome.ttl),
            Some(Duration::from_secs(10))
        );

        let second = exclusions.record_wrong_peer_id_at(
            &addr,
            Some(PeerId::random()),
            PeerId::random(),
            now + Duration::from_secs(11),
        );

        assert_eq!(
            second.ip_exclusion.as_ref().map(|outcome| outcome.ttl),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn allow_list_bypasses_evidence_and_dials() {
        let ip = IpAddr::V4(Ipv4Addr::new(15, 235, 216, 78));
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            wrong_peer_id_ip_threshold: 1,
            allow_ips: HashSet::from([ip]),
            ..config()
        });
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let outcome =
            exclusions.record_wrong_peer_id(&addr, Some(PeerId::random()), PeerId::random());
        let mut behaviour = Behaviour::new(exclusions.clone());

        assert!(outcome.address_cooldown.is_none());
        assert!(outcome.ip_exclusion.is_none());
        assert!(!exclusions.is_ip_excluded(&ip));
        assert!(behaviour
            .handle_pending_outbound_connection(
                ConnectionId::new_unchecked(7),
                None,
                std::slice::from_ref(&addr),
                Endpoint::Dialer,
            )
            .is_ok());
    }

    #[test]
    fn disabled_hygiene_records_no_penalties() {
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            enabled: false,
            wrong_peer_id_ip_threshold: 1,
            ..config()
        });
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let outcome =
            exclusions.record_wrong_peer_id(&addr, Some(PeerId::random()), PeerId::random());

        assert!(outcome.address_cooldown.is_none());
        assert!(outcome.ip_exclusion.is_none());
        assert!(!exclusions.is_address_excluded(&addr, Some(PeerId::random())));
    }

    #[test]
    fn permission_denied_is_address_cooldown_first() {
        let exclusions = PeerExclusions::new(config());
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);

        let outcome = exclusions.record_permission_denied(&addr);

        assert!(outcome.address_cooldown.is_some());
        assert!(outcome.ip_exclusion.is_none());
    }

    #[test]
    fn repeated_dial_failures_stay_endpoint_local() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);
        let first_peer = PeerId::random();
        let second_peer = PeerId::random();
        let third_peer = PeerId::random();

        let first = exclusions.record_dial_failure_at(&quic_addr(ip, 3602), Some(first_peer), now);
        let second = exclusions.record_dial_failure_at(
            &quic_addr(ip, 3603),
            Some(second_peer),
            now + Duration::from_secs(1),
        );
        let third = exclusions.record_dial_failure_at(
            &quic_addr(ip, 3604),
            Some(third_peer),
            now + Duration::from_secs(2),
        );

        assert!(first.address_cooldown.is_some());
        assert!(second.address_cooldown.is_some());
        assert!(third.address_cooldown.is_some());
        assert!(first.ip_exclusion.is_none());
        assert!(second.ip_exclusion.is_none());
        assert!(third.ip_exclusion.is_none());
        assert!(exclusions.is_address_excluded_at(
            &quic_addr(ip, 3602),
            Some(first_peer),
            now + Duration::from_secs(3)
        ));
        assert!(!exclusions.is_ip_excluded_at(&IpAddr::V4(ip), now + Duration::from_secs(3)));
        assert!(!exclusions.is_address_excluded_at(
            &quic_addr(ip, 3605),
            Some(PeerId::random()),
            now + Duration::from_secs(3)
        ));
    }

    #[test]
    fn transport_liveness_failures_do_not_prime_ip_exclusion() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip = Ipv4Addr::new(15, 235, 216, 78);

        exclusions.record_dial_failure_at(&quic_addr(ip, 3602), Some(PeerId::random()), now);
        exclusions.record_dial_failure_at(
            &quic_addr(ip, 3603),
            Some(PeerId::random()),
            now + Duration::from_secs(1),
        );
        let permission_denied = exclusions
            .record_permission_denied_at(&quic_addr(ip, 3604), now + Duration::from_secs(2));
        let ping_failure =
            exclusions.record_ping_failure_at(&quic_addr(ip, 3605), now + Duration::from_secs(3));

        assert!(permission_denied.address_cooldown.is_some());
        assert!(ping_failure.address_cooldown.is_some());
        assert!(permission_denied.ip_exclusion.is_none());
        assert!(ping_failure.ip_exclusion.is_none());
        assert!(!exclusions.is_ip_excluded_at(&IpAddr::V4(ip), now + Duration::from_secs(4)));
    }

    #[test]
    fn kad_cardinality_needs_recent_failure_before_ip_exclusion() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let ip: IpAddr = Ipv4Addr::new(15, 235, 216, 78).into();
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);

        assert!(exclusions
            .record_kad_cardinality_at(ip, 8, 8, now)
            .is_none());
        exclusions.record_wrong_peer_id_at(&addr, Some(PeerId::random()), PeerId::random(), now);
        assert!(exclusions
            .record_kad_cardinality_at(ip, 8, 8, now + Duration::from_secs(1))
            .is_some());
    }

    #[test]
    fn behaviour_denies_excluded_ip_and_allows_others() {
        let exclusions = PeerExclusions::new(config());
        let bad: IpAddr = Ipv4Addr::new(15, 235, 216, 78).into();
        let now = Instant::now();
        {
            let mut state = exclusions.inner.write().expect("lock");
            insert_ip_exclusion(
                &mut state,
                bad,
                ExclusionReason::RepeatedWrongPeerId,
                now,
                exclusions.config.as_ref(),
            );
        }
        let mut behaviour = Behaviour::new(exclusions);

        let bad_addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let good_addr = quic_addr(Ipv4Addr::new(203, 0, 113, 9), 3006);
        let cid = ConnectionId::new_unchecked(0);

        assert!(behaviour
            .handle_pending_inbound_connection(cid, &good_addr, &bad_addr)
            .is_err());
        assert!(behaviour
            .handle_pending_inbound_connection(cid, &good_addr, &good_addr)
            .is_ok());
        assert!(behaviour
            .handle_pending_outbound_connection(
                cid,
                None,
                std::slice::from_ref(&bad_addr),
                Endpoint::Dialer,
            )
            .is_err());
        assert!(behaviour
            .handle_pending_outbound_connection(
                cid,
                None,
                std::slice::from_ref(&good_addr),
                Endpoint::Dialer,
            )
            .is_ok());
        assert!(behaviour
            .handle_pending_outbound_connection(
                cid,
                None,
                &[bad_addr.clone(), good_addr],
                Endpoint::Dialer,
            )
            .is_ok());
    }

    #[test]
    fn behaviour_allows_inbound_endpoint_cooldown_and_denies_outbound() {
        let exclusions = PeerExclusions::new(config());
        let now = Instant::now();
        let peer = PeerId::random();
        let addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let local_addr = quic_addr(Ipv4Addr::new(203, 0, 113, 9), 3006);

        let outcome = exclusions.record_dial_failure_at(&addr, Some(peer), now);
        assert!(outcome.address_cooldown.is_some());
        assert!(exclusions.is_address_excluded_at(&addr, Some(peer), now));

        let mut behaviour = Behaviour::new(exclusions);
        let cid = ConnectionId::new_unchecked(42);

        assert!(behaviour
            .handle_pending_inbound_connection(cid, &local_addr, &addr)
            .is_ok());
        assert!(behaviour
            .handle_established_inbound_connection(cid, peer, &local_addr, &addr)
            .is_ok());
        assert!(behaviour
            .handle_pending_outbound_connection(
                cid,
                Some(peer),
                std::slice::from_ref(&addr),
                Endpoint::Dialer,
            )
            .is_err());
    }

    #[test]
    fn filters_excluded_kad_addresses_returned_for_peer_dial() {
        let exclusions = PeerExclusions::new(config());
        let bad: IpAddr = Ipv4Addr::new(15, 235, 216, 78).into();
        let now = Instant::now();
        {
            let mut state = exclusions.inner.write().expect("lock");
            insert_ip_exclusion(
                &mut state,
                bad,
                ExclusionReason::RepeatedWrongPeerId,
                now,
                exclusions.config.as_ref(),
            );
        }

        let local_peer = PeerId::random();
        let target_peer = PeerId::random();
        let store = kad::store::MemoryStore::new(local_peer);
        let kad = kad::Behaviour::new(local_peer, store);
        let mut behaviour = IpFilteredKad::new(kad, exclusions);

        let bad_addr = quic_addr(Ipv4Addr::new(15, 235, 216, 78), 3602);
        let good_addr = quic_addr(Ipv4Addr::new(203, 0, 113, 9), 3006);
        behaviour.add_address(&target_peer, bad_addr.clone());
        behaviour.add_address(&target_peer, good_addr.clone());

        let returned = behaviour
            .handle_pending_outbound_connection(
                ConnectionId::new_unchecked(1),
                Some(target_peer),
                &[],
                Endpoint::Dialer,
            )
            .expect("kad address filtering should not deny the dial");

        assert!(returned
            .iter()
            .any(|addr| addr.ip_addr() == good_addr.ip_addr()));
        assert!(!returned
            .iter()
            .any(|addr| addr.ip_addr() == bad_addr.ip_addr()));
    }
}
