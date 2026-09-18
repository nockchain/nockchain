//! Transport-neutral IP and endpoint exclusion policy.
//!
//! The policy tracks endpoint cooldowns, IP exclusions, and peer request
//! penalties. Transport adapters normalize their native addresses and enforce
//! the resulting decisions before dialing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use libp2p::core::Multiaddr;
use libp2p::identity::PeerId;
use libp2p::multiaddr::Protocol;

use crate::config::PeerExclusionConfig;
use crate::types::PeerAddress;

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

pub(crate) trait PolicyAddress {
    fn address_key(&self, expected_peer: Option<PeerId>) -> Option<AddressKey>;
    fn peer_address(&self) -> Option<PeerAddress>;
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
    pub(crate) address: PeerAddress,
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
struct IpEvidence {
    events: VecDeque<PeerHealthEvent>,
}

#[derive(Debug, Default)]
struct ExclusionState {
    ips: HashMap<IpAddr, IpExclusion>,
    addresses: HashMap<AddressKey, AddressExclusion>,
    peers: HashMap<PeerId, PeerPenalty>,
    ip_history: HashMap<IpAddr, IpHistory>,
    evidence_by_ip: HashMap<IpAddr, IpEvidence>,
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

    pub(crate) fn address_key<A: PolicyAddress + ?Sized>(
        &self,
        addr: &A,
        expected_peer: Option<PeerId>,
    ) -> Option<AddressKey> {
        addr.address_key(expected_peer)
    }

    pub(crate) fn is_ip_excluded(&self, ip: &IpAddr) -> bool {
        self.is_ip_excluded_at(ip, Instant::now())
    }

    pub(crate) fn is_address_excluded<A: PolicyAddress + ?Sized>(
        &self,
        addr: &A,
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

    pub(crate) fn is_address_excluded_at<A: PolicyAddress + ?Sized>(
        &self,
        addr: &A,
        expected_peer: Option<PeerId>,
        now: Instant,
    ) -> bool {
        let Some(key) = addr.address_key(expected_peer) else {
            return false;
        };
        if self.is_ip_excluded_at(&key.ip, now) {
            return true;
        }
        if !self.config.enabled || self.config.allow_ips.contains(&key.ip) {
            return false;
        }
        address_is_excluded(&read_state(&self.inner), key, now)
    }

    pub(crate) fn record_wrong_peer_id<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        expected_peer: Option<PeerId>,
        obtained_peer: PeerId,
    ) -> ExclusionOutcome {
        self.record_wrong_peer_id_at(address, expected_peer, obtained_peer, Instant::now())
    }

    fn record_wrong_peer_id_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        expected_peer: Option<PeerId>,
        obtained_peer: PeerId,
        now: Instant,
    ) -> ExclusionOutcome {
        let (Some(key), Some(neutral_address)) =
            (address.address_key(expected_peer), address.peer_address())
        else {
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
            &self.config,
        );
        let address_cooldown = insert_address_cooldown(
            &mut state,
            key,
            neutral_address,
            self.config.address_cooldown(),
            ExclusionReason::WrongPeerId,
            now,
            &self.config,
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

    pub(crate) fn record_peer_misbehavior<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        peer_id: PeerId,
    ) -> ExclusionOutcome {
        self.record_peer_misbehavior_at(address, peer_id, Instant::now())
    }

    fn record_peer_misbehavior_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        peer_id: PeerId,
        now: Instant,
    ) -> ExclusionOutcome {
        let (Some(key), Some(neutral_address)) =
            (address.address_key(Some(peer_id)), address.peer_address())
        else {
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
            &self.config,
        );
        let address_cooldown = insert_address_cooldown(
            &mut state,
            key,
            neutral_address,
            self.config.address_cooldown(),
            ExclusionReason::PeerMisbehavior,
            now,
            &self.config,
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

    pub(crate) fn record_permission_denied<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
    ) -> ExclusionOutcome {
        self.record_permission_denied_at(address, Instant::now())
    }

    fn record_permission_denied_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        now: Instant,
    ) -> ExclusionOutcome {
        self.record_address_failure_at(
            address,
            None,
            EvidenceKind::PermissionDenied,
            ExclusionReason::PermissionDenied,
            self.config.permission_denied_cooldown(),
            now,
        )
    }

    pub(crate) fn record_dial_failure<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        expected_peer: Option<PeerId>,
    ) -> ExclusionOutcome {
        self.record_dial_failure_at(address, expected_peer, Instant::now())
    }

    fn record_dial_failure_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
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

    fn record_address_failure_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        expected_peer: Option<PeerId>,
        kind: EvidenceKind,
        reason: ExclusionReason,
        address_ttl: Duration,
        now: Instant,
    ) -> ExclusionOutcome {
        let (Some(key), Some(neutral_address)) =
            (address.address_key(expected_peer), address.peer_address())
        else {
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
            &self.config,
        );

        let address_cooldown = insert_address_cooldown(
            &mut state, key, neutral_address, address_ttl, reason, now, &self.config,
        );

        ExclusionOutcome {
            address_cooldown,
            ip_exclusion: None,
        }
    }

    pub(crate) fn record_ping_failure<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
    ) -> ExclusionOutcome {
        self.record_ping_failure_at(address, Instant::now())
    }

    fn record_ping_failure_at<A: PolicyAddress + ?Sized>(
        &self,
        address: &A,
        now: Instant,
    ) -> ExclusionOutcome {
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

        let remove_evidence_entry = state.evidence_by_ip.get_mut(&ip).is_some_and(|evidence| {
            for _ in 0..2 {
                if evidence.events.pop_front().is_none() {
                    break;
                }
            }
            evidence.events.is_empty()
        });
        if remove_evidence_entry {
            state.evidence_by_ip.remove(&ip);
        }
    }

    pub(crate) fn record_peer_request_failure(&self, peer_id: PeerId) -> bool {
        if !self.config.enabled {
            return false;
        }
        let now = Instant::now();
        let expires_at = now + self.config.request_peer_cooldown();
        let mut state = write_state(&self.inner);
        prune_state(&mut state, now, &self.config);
        if !state.peers.contains_key(&peer_id)
            && state.peers.len() >= self.config.max_exclusion_entries
        {
            return false;
        }
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

    state
        .ip_history
        .retain(|_, history| history.last_excluded_at + config.ip_exclusion_history() >= now);

    for evidence in state.evidence_by_ip.values_mut() {
        evidence
            .events
            .retain(|event| event.at + config.evidence_window() >= now);
    }
    state
        .evidence_by_ip
        .retain(|_, evidence| !evidence.events.is_empty());

    ExpireOutcome {
        ips: ip_before.saturating_sub(state.ips.len()),
        addresses: address_before.saturating_sub(state.addresses.len()),
        peers: peer_before.saturating_sub(state.peers.len()),
    }
}

fn push_event(state: &mut ExclusionState, event: PeerHealthEvent, config: &PeerExclusionConfig) {
    let evidence_cap = config
        .wrong_peer_id_ip_threshold
        .saturating_mul(5)
        .max(16)
        .min(config.max_exclusion_entries.max(1));
    let ip = event.ip;
    if let Some(evidence) = state.evidence_by_ip.get_mut(&ip) {
        evidence.events.push_back(event);
        while evidence.events.len() > evidence_cap {
            evidence.events.pop_front();
        }
    } else if state.evidence_by_ip.len() < config.max_exclusion_entries {
        state.evidence_by_ip.insert(
            ip,
            IpEvidence {
                events: VecDeque::from([event]),
            },
        );
    }
}

fn insert_address_cooldown(
    state: &mut ExclusionState,
    key: AddressKey,
    address: PeerAddress,
    ttl: Duration,
    reason: ExclusionReason,
    now: Instant,
    config: &PeerExclusionConfig,
) -> Option<AddressCooldownOutcome> {
    if !state.addresses.contains_key(&key) && state.addresses.len() >= config.max_exclusion_entries
    {
        return None;
    }
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
    if (!state.ips.contains_key(&ip) && state.ips.len() >= config.max_exclusion_entries)
        || (!state.ip_history.contains_key(&ip)
            && state.ip_history.len() >= config.max_exclusion_entries)
    {
        return None;
    }
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
    let Some(evidence) = state.evidence_by_ip.get(&ip) else {
        return false;
    };
    let mut peers = HashSet::new();
    let mut obtained_peers = HashSet::new();
    let mut ports = HashSet::new();
    for event in evidence
        .events
        .iter()
        .filter(|event| event.kind == kind && event.at + config.evidence_window() >= now)
    {
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
    state.evidence_by_ip.get(&ip).is_some_and(|evidence| {
        evidence.events.iter().any(|event| {
            event.at + config.evidence_window() >= now
                && matches!(
                    event.kind,
                    EvidenceKind::WrongPeerId
                        | EvidenceKind::PeerMisbehavior
                        | EvidenceKind::DialFailure
                        | EvidenceKind::PermissionDenied
                        | EvidenceKind::PingFailure
                )
        })
    })
}

impl PolicyAddress for Multiaddr {
    fn address_key(&self, expected_peer: Option<PeerId>) -> Option<AddressKey> {
        let mut ip = None;
        let mut transport = TransportKind::Other;
        let mut port = None;
        let mut peer = expected_peer;

        for protocol in self.iter() {
            match protocol {
                Protocol::Ip4(addr) => ip = Some(IpAddr::V4(addr)),
                Protocol::Ip6(addr) => ip = Some(IpAddr::V6(addr)),
                Protocol::Tcp(value) => {
                    transport = TransportKind::Tcp;
                    port = Some(value);
                }
                Protocol::Udp(value) => {
                    transport = TransportKind::Udp;
                    port = Some(value);
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

    fn peer_address(&self) -> Option<PeerAddress> {
        let key = self.address_key(None)?;
        Some(PeerAddress::new(std::net::SocketAddr::new(
            key.ip, key.port?,
        )))
    }
}

impl PolicyAddress for PeerAddress {
    fn address_key(&self, expected_peer: Option<PeerId>) -> Option<AddressKey> {
        Some(AddressKey {
            ip: self.socket.ip(),
            transport: TransportKind::Udp,
            port: Some(self.socket.port()),
            expected_peer,
        })
    }

    fn peer_address(&self) -> Option<PeerAddress> {
        Some(*self)
    }
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr};

    use libp2p::core::Endpoint;
    use libp2p::kad;
    use libp2p::swarm::{ConnectionId, NetworkBehaviour};

    use super::*;
    use crate::ip_block::{Behaviour, IpFilteredKad};
    use crate::p2p_util::MultiaddrExt;

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
    fn rotating_ips_cannot_grow_exclusion_state_past_cap() {
        let cap = 8;
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            max_exclusion_entries: cap,
            ..config()
        });
        let now = Instant::now();

        for host in 1..=32 {
            let ip = Ipv4Addr::new(203, 0, 113, host);
            for strike in 0..3 {
                exclusions.record_peer_misbehavior_at(
                    &quic_addr(ip, 4_000 + strike),
                    PeerId::random(),
                    now,
                );
            }
            exclusions.record_peer_request_failure(PeerId::random());
        }

        let state = read_state(&exclusions.inner);
        assert!(state.ips.len() <= cap);
        assert!(state.ip_history.len() <= cap);
        assert!(state.addresses.len() <= cap);
        assert!(state.peers.len() <= cap);
        assert!(state.evidence_by_ip.len() <= cap);
    }

    #[test]
    fn unrelated_evidence_cannot_evict_existing_ip_strikes() {
        let cap = 8;
        let exclusions = PeerExclusions::new(PeerExclusionConfig {
            max_exclusion_entries: cap,
            ..config()
        });
        let now = Instant::now();
        let victim = Ipv4Addr::new(198, 51, 100, 1);

        for port in [4_001, 4_002] {
            exclusions.record_peer_misbehavior_at(&quic_addr(victim, port), PeerId::random(), now);
        }
        for host in 2..=32 {
            exclusions.record_peer_misbehavior_at(
                &quic_addr(Ipv4Addr::new(198, 51, 100, host), 5_000),
                PeerId::random(),
                now,
            );
        }

        let outcome =
            exclusions.record_peer_misbehavior_at(&quic_addr(victim, 4_003), PeerId::random(), now);
        assert!(outcome.ip_exclusion.is_some());
        assert!(exclusions.is_ip_excluded_at(&IpAddr::V4(victim), now));
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
