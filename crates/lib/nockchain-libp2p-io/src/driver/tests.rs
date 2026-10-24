use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::future::Future;
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

use super::gen2::*;
use super::*;
use crate::messages::{
    block_by_height_message, block_range_with_txs_request_message, request_slab_from_message,
    BatchErrorClass, BatchRequestItem, BatchResultItem, BatchResultStatus, BundledBlockWithTxs,
    BundledTxEnvelope, EnvelopeKind, NockchainDataRequest, NockchainFact, NockchainResponse,
    ResponseEnvelope,
};
use crate::p2p_state::BlockSource;
use crate::peer_stats::{PeerReqResGeneration, PeerStatsRegistry};
use crate::test_support::{
    build_req_res_test_swarm, bundled_block_for_height, first_common_outbound_protocol,
    jam_heard_tx_response, solve_authenticated_gossip, ReqResTestEvent, ReqResTestSwarm,
};
use crate::tip5_util::{tip5_hash_to_base58, TIP5_BASE58_MAX_CHARS};

pub static LIBP2P_CONFIG: LazyLock<LibP2PConfig> = LazyLock::new(LibP2PConfig::default);

fn observed_address_test_swarm() -> Swarm<NockchainBehaviour> {
    start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("production swarm should build")
}

fn observed_address_test_connection(
    swarm: &mut Swarm<NockchainBehaviour>,
    peer_id: PeerId,
    connection_id: ConnectionId,
    address: &str,
) -> libp2p::core::ConnectedPoint {
    use libp2p::swarm::behaviour::ConnectionEstablished;
    use libp2p::swarm::{FromSwarm, NetworkBehaviour};

    let endpoint = libp2p::core::ConnectedPoint::Dialer {
        address: address.parse().unwrap(),
        role_override: libp2p::core::Endpoint::Dialer,
        port_use: libp2p::core::transport::PortUse::Reuse,
    };
    swarm
        .behaviour_mut()
        .recovery
        .on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id,
            connection_id,
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        }));
    endpoint
}

fn observed_address_test_close(
    swarm: &mut Swarm<NockchainBehaviour>,
    peer_id: PeerId,
    connection_id: ConnectionId,
    endpoint: &libp2p::core::ConnectedPoint,
) {
    use libp2p::swarm::behaviour::ConnectionClosed;
    use libp2p::swarm::{FromSwarm, NetworkBehaviour};

    swarm
        .behaviour_mut()
        .recovery
        .on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id,
            connection_id,
            endpoint,
            cause: None,
            remaining_established: 0,
        }));
}

fn observed_address_test_info(
    keypair: &Keypair,
    observed_addr: &Multiaddr,
    listen_addrs: Vec<Multiaddr>,
) -> libp2p::identify::Info {
    libp2p::identify::Info {
        public_key: keypair.public(),
        protocol_version: String::from("heterogeneous-identify"),
        agent_version: String::from("another-implementation"),
        listen_addrs,
        protocols: Vec::new(),
        observed_addr: observed_addr.clone(),
        signed_peer_record: None,
    }
}

struct ObservedAddressTestReporter {
    keypair: Keypair,
    connection_id: ConnectionId,
    endpoint: libp2p::core::ConnectedPoint,
}

impl ObservedAddressTestReporter {
    fn new(swarm: &mut Swarm<NockchainBehaviour>, id: usize) -> Self {
        let keypair = Keypair::generate_ed25519();
        let connection_id = ConnectionId::new_unchecked(10_000 + id);
        let endpoint = observed_address_test_connection(
            swarm,
            keypair.public().to_peer_id(),
            connection_id,
            "/ip4/127.0.0.1/udp/3000/quic-v1",
        );
        Self {
            keypair,
            connection_id,
            endpoint,
        }
    }

    fn peer_id(&self) -> PeerId {
        self.keypair.public().to_peer_id()
    }
}

fn observed_address_test_claim(port: usize) -> Multiaddr {
    format!("/ip4/127.0.0.1/udp/{port}/quic-v1")
        .parse()
        .unwrap()
}

fn observed_address_test_has(swarm: &Swarm<NockchainBehaviour>, address: &Multiaddr) -> bool {
    swarm
        .external_addresses()
        .any(|external| external == address)
}

fn observed_address_test_observe(
    swarm: &mut Swarm<NockchainBehaviour>,
    votes: &mut ObservedAddressVotes,
    peer_id: PeerId,
    connection_id: ConnectionId,
    address: &Multiaddr,
    now: tokio::time::Instant,
) -> bool {
    let update = votes.observe_at(swarm, peer_id, connection_id, address, now);
    let added = update.add.as_ref() == Some(address);
    assert!(update.remove.len() <= OBSERVED_ADDRESS_ADMITTED_LIMIT);
    update.apply(swarm);
    assert!(votes.pending.len() <= OBSERVED_ADDRESS_PENDING_LIMIT);
    assert!(votes
        .pending
        .values()
        .all(|claim| claim.len() <= OBSERVED_ADDRESS_PEERS_PER_CLAIM));
    assert!(votes.admitted.len() <= OBSERVED_ADDRESS_ADMITTED_LIMIT);
    assert!(votes.admitted.len() + votes.retired.len() <= OBSERVED_ADDRESS_ADMISSION_HISTORY_LIMIT);
    added
}

fn observed_address_test_report(
    swarm: &mut Swarm<NockchainBehaviour>,
    votes: &mut ObservedAddressVotes,
    reporter: &ObservedAddressTestReporter,
    address: &Multiaddr,
    now: tokio::time::Instant,
) -> bool {
    observed_address_test_observe(
        swarm,
        votes,
        reporter.peer_id(),
        reporter.connection_id,
        address,
        now,
    )
}

fn observed_address_test_admit(
    swarm: &mut Swarm<NockchainBehaviour>,
    votes: &mut ObservedAddressVotes,
    pair: &[ObservedAddressTestReporter; 2],
    address: &Multiaddr,
    now: tokio::time::Instant,
) {
    assert!(!observed_address_test_report(
        swarm, votes, &pair[0], address, now
    ));
    assert!(observed_address_test_report(
        swarm, votes, &pair[1], address, now
    ));
    assert!(observed_address_test_has(swarm, address));
    let receipt = &votes.admitted[address];
    assert_eq!(receipt.admitted_at, now);
    assert_eq!(receipt.observers.len(), 2);
    for reporter in pair {
        assert!(receipt.observers.contains(&reporter.peer_id()));
        assert!(votes.peer_usage(&reporter.peer_id()) <= OBSERVED_ADDRESS_CLAIMS_PER_PEER);
    }
}

// Fill using disjoint pairs, four admissions per pair, never a quota bypass.
// Unique times make the first generated claim the unequivocal FIFO victim.
fn observed_address_test_fill(
    swarm: &mut Swarm<NockchainBehaviour>,
    votes: &mut ObservedAddressVotes,
    count: usize,
    now: tokio::time::Instant,
) -> Vec<[ObservedAddressTestReporter; 2]> {
    let mut pairs = Vec::new();
    for slot in 0..count {
        if slot % OBSERVED_ADDRESS_CLAIMS_PER_PEER == 0 {
            pairs.push([
                ObservedAddressTestReporter::new(swarm, slot * 2),
                ObservedAddressTestReporter::new(swarm, slot * 2 + 1),
            ]);
        }
        observed_address_test_admit(
            swarm,
            votes,
            pairs.last().unwrap(),
            &observed_address_test_claim(4000 + slot),
            now + Duration::from_millis(slot as u64),
        );
    }
    pairs
}

#[test]
fn observed_address_identify_requires_exact_live_identity_and_connection() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let claimed = "/ip4/203.0.113.10/udp/4001/quic-v1".parse().unwrap();
    let first = Keypair::generate_ed25519();
    let second = Keypair::generate_ed25519();
    let first_peer = first.public().to_peer_id();
    let second_peer = second.public().to_peer_id();
    let live = ConnectionId::new_unchecked(100);
    let unresolved = ConnectionId::new_unchecked(101);
    let closed = ConnectionId::new_unchecked(102);
    observed_address_test_connection(
        &mut swarm, first_peer, live, "/ip4/198.51.100.1/udp/3000/quic-v1",
    );
    observed_address_test_connection(
        &mut swarm, second_peer, unresolved, "/dns4/localhost/udp/3000/quic-v1",
    );
    let endpoint = observed_address_test_connection(
        &mut swarm, second_peer, closed, "/ip4/198.51.100.1/udp/3001/quic-v1",
    );
    observed_address_test_close(&mut swarm, second_peer, closed, &endpoint);
    let exclusions = PeerExclusions::default();
    let metrics = isolated_test_metrics();

    for connection in [ConnectionId::new_unchecked(999), live, unresolved, closed] {
        identify_received(
            &mut swarm,
            &mut votes,
            second_peer,
            connection,
            observed_address_test_info(&second, &claimed, Vec::new()),
            &exclusions,
            &metrics,
        )
        .unwrap();
        assert!(
            votes.pending.is_empty(),
            "unsupported or foreign CIDs cannot vote"
        );
        assert!(swarm.external_addresses().next().is_none());
    }
    identify_received(
        &mut swarm,
        &mut votes,
        first_peer,
        live,
        observed_address_test_info(&first, &claimed, Vec::new()),
        &exclusions,
        &metrics,
    )
    .unwrap();
    assert_eq!(votes.pending[&claimed].len(), 1);
    assert!(swarm.external_addresses().next().is_none());
}

#[test]
fn observed_address_identify_repeats_sessions_and_reconnects_count_one_identity() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let claimed = "/ip4/203.0.113.10/udp/4001/quic-v1".parse().unwrap();
    let first = Keypair::generate_ed25519();
    let second = Keypair::generate_ed25519();
    let first_peer = first.public().to_peer_id();
    let second_peer = second.public().to_peer_id();
    let exclusions = PeerExclusions::default();
    let metrics = isolated_test_metrics();
    let mut overlapping = None;
    for id in 1..=3 {
        let connection = ConnectionId::new_unchecked(id);
        let endpoint = observed_address_test_connection(
            &mut swarm, first_peer, connection, "/ip4/198.51.100.1/udp/3000/quic-v1",
        );
        for _ in 0..3 {
            identify_received(
                &mut swarm,
                &mut votes,
                first_peer,
                connection,
                observed_address_test_info(&first, &claimed, Vec::new()),
                &exclusions,
                &metrics,
            )
            .unwrap();
            assert_eq!(votes.pending[&claimed].len(), 1);
            assert!(swarm.external_addresses().next().is_none());
        }
        if id == 1 {
            // The second session overlaps the first without adding a voter.
            overlapping = Some((connection, endpoint));
            continue;
        }
        observed_address_test_close(&mut swarm, first_peer, connection, &endpoint);
        if let Some((connection, endpoint)) = overlapping.take() {
            observed_address_test_close(&mut swarm, first_peer, connection, &endpoint);
        }
    }
    let second_connection = ConnectionId::new_unchecked(4);
    observed_address_test_connection(
        &mut swarm, second_peer, second_connection, "/ip4/198.51.100.1/udp/3000/quic-v1",
    );
    identify_received(
        &mut swarm,
        &mut votes,
        second_peer,
        second_connection,
        observed_address_test_info(&second, &claimed, Vec::new()),
        &exclusions,
        &metrics,
    )
    .unwrap();
    assert!(
        swarm.external_addresses().next().is_none(),
        "a vote from a closed CID cannot combine with another identity"
    );
    let reconnected = ConnectionId::new_unchecked(5);
    observed_address_test_connection(
        &mut swarm, first_peer, reconnected, "/ip4/198.51.100.1/udp/3000/quic-v1",
    );
    identify_received(
        &mut swarm,
        &mut votes,
        first_peer,
        reconnected,
        observed_address_test_info(&first, &claimed, Vec::new()),
        &exclusions,
        &metrics,
    )
    .unwrap();
    assert!(swarm
        .external_addresses()
        .any(|address| address == &claimed));
    assert!(votes.pending.is_empty());
    assert_eq!(votes.admitted.len(), 1);
}

#[test]
fn observed_address_requires_exact_numeric_unicast_quic_endpoint() {
    let mut swarm = observed_address_test_swarm();
    let peers = [PeerId::random(), PeerId::random()];
    let connections = [ConnectionId::new_unchecked(1), ConnectionId::new_unchecked(2)];
    for (peer, connection) in peers.into_iter().zip(connections) {
        observed_address_test_connection(
            &mut swarm, peer, connection, "/ip4/198.51.100.1/udp/3000/quic-v1",
        );
    }
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let mut invalid = vec![
        "/dns4/example.com/udp/4001/quic-v1".to_owned(),
        "/ip4/203.0.113.10/tcp/4001".to_owned(),
        "/ip4/203.0.113.10/udp/0/quic-v1".to_owned(),
        "/ip4/203.0.113.10/udp/4001".to_owned(),
        "/ip4/203.0.113.10/udp/4001/quic".to_owned(),
        "/ip4/0.0.0.0/udp/4001/quic-v1".to_owned(),
        "/ip6/::/udp/4001/quic-v1".to_owned(),
        "/ip4/224.0.0.1/udp/4001/quic-v1".to_owned(),
        "/ip6/ff02::1/udp/4001/quic-v1".to_owned(),
        "/ip4/255.255.255.255/udp/4001/quic-v1".to_owned(),
        "/ip6/::ffff:0.0.0.0/udp/4001/quic-v1".to_owned(),
        "/ip6/::ffff:224.0.0.1/udp/4001/quic-v1".to_owned(),
        "/ip4/127.0.0.1/udp/4001/quic-v1".to_owned(),
        "/ip6/::1/udp/4001/quic-v1".to_owned(),
        "/ip6/::ffff:127.0.0.1/udp/4001/quic-v1".to_owned(),
    ];
    invalid.push(format!(
        "/ip4/203.0.113.10/udp/4001/quic-v1/p2p/{}",
        peers[0]
    ));
    for address in invalid {
        let address = address.parse().unwrap();
        for (peer, connection) in peers.into_iter().zip(connections) {
            assert!(!observed_address_test_observe(
                &mut swarm, &mut votes, peer, connection, &address, now,
            ));
        }
        assert!(
            votes.pending.is_empty(),
            "{address} must not consume vote capacity"
        );
    }
    let first = "/ip4/10.0.0.1/udp/4001/quic-v1".parse().unwrap();
    let different_port = "/ip4/10.0.0.1/udp/4002/quic-v1".parse().unwrap();
    let different_ip = "/ip4/10.0.0.2/udp/4001/quic-v1".parse().unwrap();
    assert!(!observed_address_test_observe(
        &mut swarm, &mut votes, peers[0], connections[0], &first, now
    ));
    assert!(!observed_address_test_observe(
        &mut swarm, &mut votes, peers[1], connections[1], &different_port, now
    ));
    assert!(!observed_address_test_observe(
        &mut swarm, &mut votes, peers[1], connections[1], &different_ip, now
    ));
    assert!(observed_address_test_observe(
        &mut swarm, &mut votes, peers[1], connections[1], &first, now
    ));
    let v6 = "/ip6/fd00::1/udp/4001/quic-v1".parse().unwrap();
    assert!(!observed_address_test_observe(
        &mut swarm, &mut votes, peers[0], connections[0], &v6, now
    ));
    assert!(observed_address_test_observe(
        &mut swarm, &mut votes, peers[1], connections[1], &v6, now
    ));
}

#[test]
fn observed_address_votes_expire_without_repeat_or_switch_refresh() {
    let mut swarm = observed_address_test_swarm();
    let pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let first = observed_address_test_claim(4001);
    let switched = observed_address_test_claim(4002);
    let now = tokio::time::Instant::now();
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[0], &first, now
    ));
    let later = now + OBSERVED_ADDRESS_VOTE_TTL - Duration::from_secs(1);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[0], &switched, later
    ));
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[0], &first, later
    ));
    let replacement = ConnectionId::new_unchecked(30_000);
    observed_address_test_connection(
        &mut swarm,
        pair[0].peer_id(),
        replacement,
        "/ip4/127.0.0.1/udp/3001/quic-v1",
    );
    assert!(!observed_address_test_observe(
        &mut swarm,
        &mut votes,
        pair[0].peer_id(),
        replacement,
        &first,
        later
    ));
    assert_eq!(votes.pending[&first][&pair[0].peer_id()].first_seen, now);
    let expired = now + OBSERVED_ADDRESS_VOTE_TTL;
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[1], &first, expired
    ));
    assert_eq!(votes.pending[&first].len(), 1);
    assert!(observed_address_test_observe(
        &mut swarm,
        &mut votes,
        pair[0].peer_id(),
        replacement,
        &first,
        expired
    ));
    assert_eq!(votes.admitted[&first].admitted_at, expired);
}

#[test]
fn observed_address_pending_bounds_resist_one_identity_churn_and_recover() {
    let mut swarm = observed_address_test_swarm();
    let first = ObservedAddressTestReporter::new(&mut swarm, 0);
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    for port in 4000..4200 {
        assert!(!observed_address_test_report(
            &mut swarm,
            &mut votes,
            &first,
            &observed_address_test_claim(port),
            now
        ));
    }
    assert_eq!(votes.pending.len(), OBSERVED_ADDRESS_CLAIMS_PER_PEER);
    for id in 1..16 {
        let reporter = ObservedAddressTestReporter::new(&mut swarm, id);
        for slot in 0..OBSERVED_ADDRESS_CLAIMS_PER_PEER {
            let claim =
                observed_address_test_claim(5000 + id * OBSERVED_ADDRESS_CLAIMS_PER_PEER + slot);
            assert!(!observed_address_test_report(
                &mut swarm, &mut votes, &reporter, &claim, now
            ));
        }
    }
    assert_eq!(votes.pending.len(), OBSERVED_ADDRESS_PENDING_LIMIT);
    let other = ObservedAddressTestReporter::new(&mut swarm, 100);
    let fresh = observed_address_test_claim(6000);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &other, &fresh, now
    ));
    assert!(!votes.pending.contains_key(&fresh));
    let existing = observed_address_test_claim(4000);
    assert!(observed_address_test_report(
        &mut swarm, &mut votes, &other, &existing, now
    ));
    assert!(!observed_address_test_report(
        &mut swarm,
        &mut votes,
        &other,
        &fresh,
        now + OBSERVED_ADDRESS_VOTE_TTL
    ));
    assert_eq!(votes.pending.len(), 1);
    assert!(votes.pending.contains_key(&fresh));
    assert_eq!(
        votes.peer_usage(&other.peer_id()),
        2,
        "one immutable admission plus one pending vote"
    );
}

#[test]
fn observed_address_fixed_pair_cannot_fill_table_or_refund_by_reconnecting() {
    let mut swarm = observed_address_test_swarm();
    let mut pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let exclusions = PeerExclusions::default();
    let metrics = isolated_test_metrics();
    // Exercise the actual Identify caller for more than sixteen claims.
    for port in 4000..4020 {
        let claim = observed_address_test_claim(port);
        for reporter in &pair {
            identify_received(
                &mut swarm,
                &mut votes,
                reporter.peer_id(),
                reporter.connection_id,
                observed_address_test_info(&reporter.keypair, &claim, Vec::new()),
                &exclusions,
                &metrics,
            )
            .unwrap();
        }
        assert_eq!(observed_address_test_has(&swarm, &claim), port < 4004);
    }
    assert_eq!(votes.admitted.len(), 4);
    assert!(votes.pending.is_empty());
    assert_eq!(swarm.external_addresses().count(), 4);
    let receipts: Vec<_> = votes
        .admitted
        .iter()
        .map(|(address, receipt)| (address.clone(), receipt.admitted_at, receipt.observers))
        .collect();
    for (_, _, witnesses) in &receipts {
        for reporter in &pair {
            assert!(witnesses.contains(&reporter.peer_id()));
        }
    }
    let later = tokio::time::Instant::now() + OBSERVED_ADDRESS_VOTE_TTL;
    for (slot, reporter) in pair.iter_mut().enumerate() {
        observed_address_test_close(
            &mut swarm,
            reporter.peer_id(),
            reporter.connection_id,
            &reporter.endpoint,
        );
        reporter.connection_id = ConnectionId::new_unchecked(30_000 + slot);
        reporter.endpoint = observed_address_test_connection(
            &mut swarm,
            reporter.peer_id(),
            reporter.connection_id,
            "/ip4/127.0.0.1/udp/3001/quic-v1",
        );
    }
    for port in 4000..4020 {
        for reporter in &pair {
            assert!(!observed_address_test_report(
                &mut swarm,
                &mut votes,
                reporter,
                &observed_address_test_claim(port),
                later
            ));
            assert_eq!(votes.peer_usage(&reporter.peer_id()), 4);
        }
    }
    for (address, admitted_at, observers) in receipts {
        assert_eq!(votes.admitted[&address].admitted_at, admitted_at);
        assert_eq!(votes.admitted[&address].observers, observers);
    }
    assert_eq!(votes.admitted.len(), 4);
    assert!(votes.pending.is_empty());
}

#[test]
fn observed_address_fresh_pair_replaces_full_table_without_refunding_retired_receipt() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let original = observed_address_test_fill(&mut swarm, &mut votes, 16, now);
    let victim = observed_address_test_claim(4000);
    let witnesses = votes.admitted[&victim].observers;
    let fresh = [
        ObservedAddressTestReporter::new(&mut swarm, 100),
        ObservedAddressTestReporter::new(&mut swarm, 101),
    ];
    let candidate = observed_address_test_claim(6000);
    let later = now + Duration::from_secs(1);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &fresh[0], &candidate, later
    ));
    let update = votes.observe_at(
        &swarm,
        fresh[1].peer_id(),
        fresh[1].connection_id,
        &candidate,
        later,
    );
    assert_eq!(update.remove, vec![victim.clone()]);
    assert_eq!(update.add.as_ref(), Some(&candidate));
    update.apply(&mut swarm);
    assert_eq!(votes.admitted.len(), 16);
    assert_eq!(votes.retired.len(), 1);
    assert_eq!(swarm.external_addresses().count(), 16);
    assert!(!observed_address_test_has(&swarm, &victim));
    assert!(observed_address_test_has(&swarm, &candidate));
    assert_eq!(votes.retired[0].admitted_at, now);
    assert_eq!(votes.retired[0].observers, witnesses);
    for reporter in &original[0] {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 4);
        assert!(!observed_address_test_report(
            &mut swarm,
            &mut votes,
            reporter,
            &observed_address_test_claim(6001),
            later
        ));
    }
    // At the original receipt boundary the old pair regains exactly that slot.
    let boundary = now + OBSERVED_ADDRESS_ADMITTED_TTL;
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &original[0][0], &victim, boundary
    ));
    assert!(votes
        .retired
        .iter()
        .all(|receipt| receipt.admitted_at != now));
    assert_eq!(votes.peer_usage(&original[0][0].peer_id()), 4);
    assert_eq!(votes.pending[&victim].len(), 1);
    assert!(observed_address_test_report(
        &mut swarm, &mut votes, &original[0][1], &victim, boundary
    ));
    assert_eq!(votes.admitted[&victim].admitted_at, boundary);
}

#[test]
fn observed_address_evicted_real_address_restores_without_refunding_old_charges() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let honest = [
        ObservedAddressTestReporter::new(&mut swarm, 500),
        ObservedAddressTestReporter::new(&mut swarm, 501),
    ];
    let real = observed_address_test_claim(4000);
    observed_address_test_admit(&mut swarm, &mut votes, &honest, &real, now);
    let original_witnesses = votes.admitted[&real].observers;
    // The real address's dedicated pair has no other initial charges.
    let mut filler_pairs = Vec::new();
    for slot in 0..15 {
        if slot % OBSERVED_ADDRESS_CLAIMS_PER_PEER == 0 {
            filler_pairs.push([
                ObservedAddressTestReporter::new(&mut swarm, slot * 2),
                ObservedAddressTestReporter::new(&mut swarm, slot * 2 + 1),
            ]);
        }
        observed_address_test_admit(
            &mut swarm,
            &mut votes,
            filler_pairs.last().unwrap(),
            &observed_address_test_claim(4001 + slot),
            now + Duration::from_millis(1 + slot as u64),
        );
    }
    assert_eq!(votes.admitted.len(), 16);
    let malicious = [
        ObservedAddressTestReporter::new(&mut swarm, 600),
        ObservedAddressTestReporter::new(&mut swarm, 601),
    ];
    observed_address_test_admit(
        &mut swarm,
        &mut votes,
        &malicious,
        &observed_address_test_claim(6000),
        now + Duration::from_secs(1),
    );
    assert!(!observed_address_test_has(&swarm, &real));
    assert_eq!(votes.retired.len(), 1);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert_eq!(votes.retired[0].observers, original_witnesses);
    let restored_at = now + Duration::from_secs(2);
    observed_address_test_admit(&mut swarm, &mut votes, &honest, &real, restored_at);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert_eq!(votes.retired[0].observers, original_witnesses);
    for reporter in &honest {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 2);
    }
    for slot in 1..4 {
        observed_address_test_admit(
            &mut swarm,
            &mut votes,
            &malicious,
            &observed_address_test_claim(6000 + slot),
            now + Duration::from_secs(2 + slot as u64),
        );
        assert!(observed_address_test_has(&swarm, &real));
        assert_eq!(votes.admitted[&real].admitted_at, restored_at);
    }
    let fifth = observed_address_test_claim(6004);
    for reporter in &malicious {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 4);
        assert!(!observed_address_test_report(
            &mut swarm,
            &mut votes,
            reporter,
            &fifth,
            now + Duration::from_secs(6)
        ));
    }
    assert!(!votes.pending.contains_key(&fifth));
    assert!(!observed_address_test_has(&swarm, &fifth));
    assert_eq!(votes.admitted.len(), 16);
    assert_eq!(votes.retired.len(), 5);
    assert_eq!(votes.admitted.len() + votes.retired.len(), 21);
    assert_eq!(swarm.external_addresses().count(), 16);
    assert!(observed_address_test_has(&swarm, &real));
    // Active repeats do not refresh the new receipt or the old charge.
    let before_expiry = now + OBSERVED_ADDRESS_ADMITTED_TTL - Duration::from_secs(1);
    for reporter in &honest {
        assert!(!observed_address_test_report(
            &mut swarm, &mut votes, reporter, &real, before_expiry
        ));
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 2);
    }
    assert_eq!(votes.admitted[&real].admitted_at, restored_at);
    assert_eq!(votes.admitted[&real].observers, original_witnesses);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert_eq!(votes.retired[0].observers, original_witnesses);
    // Only the original admission expires at its original boundary.
    assert!(!observed_address_test_report(
        &mut swarm,
        &mut votes,
        &honest[0],
        &real,
        now + OBSERVED_ADDRESS_ADMITTED_TTL
    ));
    for reporter in &honest {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 1);
    }
    for reporter in &malicious {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 4);
    }
    assert_eq!(votes.admitted[&real].admitted_at, restored_at);
    assert_eq!(votes.retired.len(), 4);
    assert_eq!(votes.admitted.len() + votes.retired.len(), 20);
    assert!(observed_address_test_has(&swarm, &real));
}

#[test]
fn observed_address_receipt_selects_only_two_live_witnesses_and_starts_new_clock() {
    let mut swarm = observed_address_test_swarm();
    let stale = ObservedAddressTestReporter::new(&mut swarm, 0);
    let first = ObservedAddressTestReporter::new(&mut swarm, 1);
    let second = ObservedAddressTestReporter::new(&mut swarm, 2);
    let mut votes = ObservedAddressVotes::default();
    let address = observed_address_test_claim(4000);
    let now = tokio::time::Instant::now();
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &stale, &address, now
    ));
    observed_address_test_close(
        &mut swarm,
        stale.peer_id(),
        stale.connection_id,
        &stale.endpoint,
    );
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &first, &address, now
    ));
    let admitted_at = now + OBSERVED_ADDRESS_VOTE_TTL - Duration::from_secs(1);
    assert!(observed_address_test_report(
        &mut swarm, &mut votes, &second, &address, admitted_at
    ));
    let receipt = &votes.admitted[&address];
    assert_eq!(receipt.admitted_at, admitted_at);
    assert!(receipt.observers.contains(&first.peer_id()));
    assert!(receipt.observers.contains(&second.peer_id()));
    assert_eq!(
        votes.peer_usage(&stale.peer_id()),
        0,
        "an unselected stale voter is not an admission sponsor"
    );
    let witnesses = receipt.observers;
    // Disconnecting selected witnesses does not revoke or refund their receipt.
    observed_address_test_close(
        &mut swarm,
        first.peer_id(),
        first.connection_id,
        &first.endpoint,
    );
    observed_address_test_close(
        &mut swarm,
        second.peer_id(),
        second.connection_id,
        &second.endpoint,
    );
    let fresh = ObservedAddressTestReporter::new(&mut swarm, 3);
    assert!(!observed_address_test_report(
        &mut swarm,
        &mut votes,
        &fresh,
        &address,
        now + OBSERVED_ADDRESS_VOTE_TTL
    ));
    assert_eq!(votes.admitted[&address].admitted_at, admitted_at);
    assert_eq!(votes.admitted[&address].observers, witnesses);
    assert_eq!(votes.peer_usage(&first.peer_id()), 1);
    assert_eq!(votes.peer_usage(&second.peer_id()), 1);
    assert!(observed_address_test_has(&swarm, &address));
}

#[test]
fn observed_address_history64_backpressures_without_forgetting_or_withdrawing() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let pairs = observed_address_test_fill(&mut swarm, &mut votes, 64, now);
    assert_eq!(votes.admitted.len(), 16);
    assert_eq!(votes.retired.len(), 48);
    assert_eq!(swarm.external_addresses().count(), 16);
    for pair in &pairs {
        for reporter in pair {
            assert_eq!(votes.peer_usage(&reporter.peer_id()), 4);
        }
    }
    let candidate = observed_address_test_claim(6000);
    let later = now + Duration::from_secs(1);
    let mut fresh = Vec::new();
    for id in 100..109 {
        let reporter = ObservedAddressTestReporter::new(&mut swarm, id);
        let update = votes.observe_at(
            &swarm,
            reporter.peer_id(),
            reporter.connection_id,
            &candidate,
            later,
        );
        assert!(
            update.remove.is_empty(),
            "backpressure cannot evict before a receipt fits"
        );
        assert!(update.add.is_none());
        update.apply(&mut swarm);
        fresh.push(reporter);
    }
    assert_eq!(votes.pending[&candidate].len(), 8);
    assert_eq!(votes.peer_usage(&fresh[8].peer_id()), 0);
    assert_eq!(votes.admitted.len() + votes.retired.len(), 64);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert!(observed_address_test_has(
        &swarm,
        &observed_address_test_claim(4048)
    ));
    // The first exact one-hour boundary frees one receipt, not every receipt.
    let boundary = now + OBSERVED_ADDRESS_ADMITTED_TTL;
    let invalid = "/ip4/0.0.0.0/udp/6000/quic-v1".parse().unwrap();
    let update = votes.observe_at(
        &swarm,
        fresh[0].peer_id(),
        fresh[0].connection_id,
        &invalid,
        boundary,
    );
    assert!(update.remove.is_empty());
    assert!(update.add.is_none());
    assert_eq!(votes.admitted.len(), 16);
    assert_eq!(
        votes.retired.len(),
        48,
        "invalid input must not prune retired receipts either"
    );
    assert_eq!(votes.pending[&candidate].len(), 8);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &fresh[0], &candidate, boundary
    ));
    assert_eq!(
        votes.pending[&candidate].len(),
        1,
        "old pending votes expired independently"
    );
    assert!(observed_address_test_report(
        &mut swarm, &mut votes, &fresh[1], &candidate, boundary
    ));
    assert_eq!(votes.admitted.len(), 16);
    assert_eq!(votes.retired.len(), 48);
    assert_eq!(votes.admitted.len() + votes.retired.len(), 64);
    assert!(observed_address_test_has(&swarm, &candidate));
    assert!(!observed_address_test_has(
        &swarm,
        &observed_address_test_claim(4048)
    ));
    assert!(votes
        .retired
        .iter()
        .all(|receipt| receipt.admitted_at != now));
}

#[test]
fn observed_address_expiry_validates_before_pruning_and_preserves_protected_addresses() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let pairs = observed_address_test_fill(&mut swarm, &mut votes, 16, now);
    let protected = observed_address_test_claim(7000);
    let fresh = [
        ObservedAddressTestReporter::new(&mut swarm, 100),
        ObservedAddressTestReporter::new(&mut swarm, 101),
    ];
    // A pending claim becomes independently installed before corroboration.
    let later = now + Duration::from_secs(1);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &fresh[0], &protected, later
    ));
    swarm.add_external_address(protected.clone());
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &fresh[1], &protected, later
    ));
    assert!(!votes.pending.contains_key(&protected));
    assert!(!votes.admitted.contains_key(&protected));
    assert_eq!(votes.peer_usage(&fresh[0].peer_id()), 0);
    let pending = observed_address_test_claim(7001);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &fresh[0], &pending, later
    ));
    let unresolved_peer = PeerId::random();
    let unresolved = ConnectionId::new_unchecked(30_000);
    observed_address_test_connection(
        &mut swarm, unresolved_peer, unresolved, "/dns4/localhost/udp/3000/quic-v1",
    );
    let closed = ObservedAddressTestReporter::new(&mut swarm, 102);
    observed_address_test_close(
        &mut swarm,
        closed.peer_id(),
        closed.connection_id,
        &closed.endpoint,
    );
    let remote_peer = PeerId::random();
    let remote = ConnectionId::new_unchecked(30_001);
    observed_address_test_connection(
        &mut swarm, remote_peer, remote, "/ip4/198.51.100.1/udp/3000/quic-v1",
    );
    let expired = now + OBSERVED_ADDRESS_ADMITTED_TTL + Duration::from_secs(1);
    let invalid = "/ip4/0.0.0.0/udp/7002/quic-v1".parse().unwrap();
    for (peer, connection, address) in [
        (fresh[0].peer_id(), fresh[0].connection_id, &invalid),
        (fresh[0].peer_id(), fresh[1].connection_id, &pending), // foreign exact CID
        (
            fresh[0].peer_id(),
            ConnectionId::new_unchecked(99_999),
            &pending,
        ),
        (unresolved_peer, unresolved, &pending),
        (closed.peer_id(), closed.connection_id, &pending),
        (remote_peer, remote, &pending), // public observer cannot sponsor loopback
    ] {
        let update = votes.observe_at(&swarm, peer, connection, address, expired);
        assert!(update.remove.is_empty());
        assert!(update.add.is_none());
        assert_eq!(
            votes.admitted.len(),
            16,
            "invalid input must not prune active receipts"
        );
        assert!(
            votes.pending.contains_key(&pending),
            "invalid input must not prune pending votes"
        );
        assert_eq!(swarm.external_addresses().count(), 17);
    }
    // Valid input with no quorum still withdraws all expired owned advertisements.
    let update = votes.observe_at(
        &swarm,
        fresh[1].peer_id(),
        fresh[1].connection_id,
        &pending,
        expired,
    );
    assert_eq!(update.remove.len(), 16);
    assert!(update.add.is_none());
    update.apply(&mut swarm);
    assert!(votes.admitted.is_empty());
    assert_eq!(votes.pending[&pending].len(), 1);
    assert_eq!(swarm.external_addresses().count(), 1);
    assert!(observed_address_test_has(&swarm, &protected));
    // Protected ownership also survives replacement of another full table.
    for (slot, pair) in pairs.iter().enumerate() {
        for index in 0..4 {
            observed_address_test_admit(
                &mut swarm,
                &mut votes,
                pair,
                &observed_address_test_claim(8000 + slot * 4 + index),
                expired,
            );
        }
    }
    observed_address_test_admit(
        &mut swarm,
        &mut votes,
        &fresh,
        &observed_address_test_claim(9000),
        expired,
    );
    assert_eq!(votes.admitted.len(), 16);
    assert!(observed_address_test_has(&swarm, &protected));
    assert_eq!(swarm.external_addresses().count(), 17);
}

#[test]
fn observed_address_just_expired_current_claim_can_start_a_new_quorum() {
    let mut swarm = observed_address_test_swarm();
    let pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let address = observed_address_test_claim(4000);
    observed_address_test_admit(&mut swarm, &mut votes, &pair, &address, now);
    let boundary = now + OBSERVED_ADDRESS_ADMITTED_TTL;
    let update = votes.observe_at(
        &swarm,
        pair[0].peer_id(),
        pair[0].connection_id,
        &address,
        boundary,
    );
    assert_eq!(update.remove, vec![address.clone()]);
    assert!(update.add.is_none());
    assert_eq!(
        votes.pending[&address].len(),
        1,
        "pre-application swarm membership is not independent ownership"
    );
    update.apply(&mut swarm);
    assert!(!observed_address_test_has(&swarm, &address));
    assert!(observed_address_test_report(
        &mut swarm, &mut votes, &pair[1], &address, boundary
    ));
    assert_eq!(votes.admitted[&address].admitted_at, boundary);
}

#[test]
fn observed_address_independent_confirmation_or_expiry_preserves_receipt_not_ownership() {
    let mut swarm = observed_address_test_swarm();
    let pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let confirmed = observed_address_test_claim(4000);
    let externally_expired = observed_address_test_claim(4001);
    observed_address_test_admit(&mut swarm, &mut votes, &pair, &confirmed, now);
    observed_address_test_admit(&mut swarm, &mut votes, &pair, &externally_expired, now);
    let witnesses = votes.admitted[&confirmed].observers;
    // Same helper as ExternalAddrConfirmed: leave the independently owned ad.
    votes.relinquish(&confirmed);
    // Same helper as ExternalAddrExpired: the swarm has already removed the ad.
    swarm.remove_external_address(&externally_expired);
    votes.relinquish(&externally_expired);
    votes.relinquish(&confirmed); // Duplicate notification is harmless.
    assert!(votes.admitted.is_empty());
    assert_eq!(votes.retired.len(), 2);
    assert_eq!(votes.retired[0].admitted_at, now);
    assert_eq!(votes.retired[0].observers, witnesses);
    for reporter in &pair {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 2);
    }
    assert!(!observed_address_test_has(&swarm, &externally_expired));
    // External expiry does not reinstall automatically; a new quorum pays
    // another charge while both original receipts keep their original clocks.
    let readmitted_at = now + Duration::from_secs(1);
    observed_address_test_admit(
        &mut swarm, &mut votes, &pair, &externally_expired, readmitted_at,
    );
    for reporter in &pair {
        assert_eq!(votes.peer_usage(&reporter.peer_id()), 3);
    }
    assert_eq!(votes.retired.len(), 2);
    assert!(votes
        .retired
        .iter()
        .all(|receipt| receipt.admitted_at == now));
    let boundary = now + OBSERVED_ADDRESS_ADMITTED_TTL;
    let update = votes.observe_at(
        &swarm,
        pair[0].peer_id(),
        pair[0].connection_id,
        &confirmed,
        boundary,
    );
    assert!(
        update.remove.is_empty(),
        "independent confirmations must not later be withdrawn"
    );
    assert!(update.add.is_none());
    update.apply(&mut swarm);
    assert!(votes.retired.is_empty());
    assert_eq!(votes.peer_usage(&pair[0].peer_id()), 1);
    assert_eq!(swarm.external_addresses().count(), 2);
    assert!(observed_address_test_has(&swarm, &confirmed));
}

#[test]
fn observed_address_pending_active_and_retired_share_one_identity_budget() {
    let mut swarm = observed_address_test_swarm();
    let pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let retired = observed_address_test_claim(4000);
    let active = observed_address_test_claim(4001);
    observed_address_test_admit(&mut swarm, &mut votes, &pair, &retired, now);
    observed_address_test_admit(&mut swarm, &mut votes, &pair, &active, now);
    swarm.remove_external_address(&retired);
    votes.relinquish(&retired);
    for port in 5000..5002 {
        assert!(!observed_address_test_report(
            &mut swarm,
            &mut votes,
            &pair[0],
            &observed_address_test_claim(port),
            now
        ));
    }
    assert_eq!(votes.peer_usage(&pair[0].peer_id()), 4);
    let fifth = observed_address_test_claim(5002);
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[0], &fifth, now
    ));
    assert!(!votes.pending.contains_key(&fifth));
    // Pending expiration refunds only the two pending votes, not either receipt.
    let boundary = now + OBSERVED_ADDRESS_VOTE_TTL;
    assert!(!observed_address_test_report(
        &mut swarm, &mut votes, &pair[0], &fifth, boundary
    ));
    assert_eq!(votes.peer_usage(&pair[0].peer_id()), 3);
    assert_eq!(votes.admitted.len(), 1);
    assert_eq!(votes.retired.len(), 1);
    assert!(observed_address_test_has(&swarm, &active));
}

#[test]
fn observed_address_same_address_retirements_keep_each_charge_and_exhaust_quota() {
    let mut swarm = observed_address_test_swarm();
    let pair = [
        ObservedAddressTestReporter::new(&mut swarm, 0),
        ObservedAddressTestReporter::new(&mut swarm, 1),
    ];
    let mut votes = ObservedAddressVotes::default();
    let now = tokio::time::Instant::now();
    let address = observed_address_test_claim(4000);
    for slot in 0..OBSERVED_ADDRESS_CLAIMS_PER_PEER {
        observed_address_test_admit(
            &mut swarm,
            &mut votes,
            &pair,
            &address,
            now + Duration::from_secs(slot as u64),
        );
        swarm.remove_external_address(&address);
        votes.relinquish(&address);
        assert!(votes.admitted.is_empty());
        assert_eq!(votes.retired.len(), slot + 1);
        for reporter in &pair {
            assert_eq!(votes.peer_usage(&reporter.peer_id()), slot + 1);
        }
    }
    let fifth_at = now + Duration::from_secs(OBSERVED_ADDRESS_CLAIMS_PER_PEER as u64);
    for reporter in &pair {
        assert!(!observed_address_test_report(
            &mut swarm, &mut votes, reporter, &address, fifth_at
        ));
        assert_eq!(
            votes.peer_usage(&reporter.peer_id()),
            OBSERVED_ADDRESS_CLAIMS_PER_PEER
        );
    }
    assert!(votes.pending.is_empty());
    assert!(votes.admitted.is_empty());
    assert!(!observed_address_test_has(&swarm, &address));
    assert_eq!(votes.retired.len(), OBSERVED_ADDRESS_CLAIMS_PER_PEER);
    for (slot, receipt) in votes.retired.iter().enumerate() {
        assert_eq!(receipt.admitted_at, now + Duration::from_secs(slot as u64));
        for reporter in &pair {
            assert!(receipt.observers.contains(&reporter.peer_id()));
        }
    }
}

#[test]
fn observed_address_identify_preserves_claimed_ip_evidence_and_remote_discovery() {
    let mut swarm = observed_address_test_swarm();
    let mut votes = ObservedAddressVotes::default();
    let exclusions = PeerExclusions::new(crate::config::PeerExclusionConfig {
        wrong_peer_id_ip_threshold: 3,
        ..crate::config::PeerExclusionConfig::default()
    });
    for port in [4002, 4003] {
        let address = format!("/ip4/203.0.113.10/udp/{port}/quic-v1")
            .parse()
            .unwrap();
        assert!(exclusions
            .record_wrong_peer_id(&address, Some(PeerId::random()), PeerId::random())
            .ip_exclusion
            .is_none());
    }
    let claimed = "/ip4/203.0.113.10/udp/4001/quic-v1".parse().unwrap();
    let excluded: Multiaddr = "/ip4/198.51.100.30/udp/3000/quic-v1".parse().unwrap();
    exclusions.record_permission_denied(&excluded);
    let discovered: Multiaddr = "/ip4/198.51.100.20/udp/3000/quic-v1".parse().unwrap();
    let dnsaddr: Multiaddr = "/dnsaddr/example.com".parse().unwrap();
    let metrics = isolated_test_metrics();
    let mut reporting_peers = Vec::new();
    for id in 1..=2 {
        let keypair = Keypair::generate_ed25519();
        let peer = keypair.public().to_peer_id();
        reporting_peers.push(peer);
        let connection = ConnectionId::new_unchecked(id);
        observed_address_test_connection(
            &mut swarm, peer, connection, "/ip4/198.51.100.1/udp/3000/quic-v1",
        );
        // A rejected observation still carries ordinary peer discovery data.
        let invalid = "/ip4/0.0.0.0/udp/4001/quic-v1".parse().unwrap();
        identify_received(
            &mut swarm,
            &mut votes,
            peer,
            connection,
            observed_address_test_info(
                &keypair,
                &invalid,
                vec![discovered.clone(), excluded.clone(), dnsaddr.clone()],
            ),
            &exclusions,
            &metrics,
        )
        .unwrap();
        identify_received(
            &mut swarm,
            &mut votes,
            peer,
            connection,
            observed_address_test_info(&keypair, &claimed, Vec::new()),
            &exclusions,
            &metrics,
        )
        .unwrap();
    }
    assert!(swarm
        .external_addresses()
        .any(|address| address == &claimed));
    let mut discovered_peers = BTreeSet::new();
    for bucket in swarm.behaviour_mut().kad.kbuckets() {
        for entry in bucket.iter() {
            let peer = entry.node.key.into_preimage();
            if reporting_peers.contains(&peer) {
                assert_eq!(entry.node.value.len(), 1);
                assert_eq!(
                    entry.node.value.first(),
                    &discovered.clone().with_p2p(peer).unwrap(),
                    "excluded endpoints and dnsaddr hints must not enter the routing table"
                );
                discovered_peers.insert(peer);
            }
        }
    }
    assert_eq!(discovered_peers.len(), 2);
    let third = "/ip4/203.0.113.10/udp/4004/quic-v1".parse().unwrap();
    assert!(
        exclusions
            .record_wrong_peer_id(&third, Some(PeerId::random()), PeerId::random())
            .ip_exclusion
            .is_some(),
        "corroborating a claim must not erase the two earlier negative evidence events"
    );
}

#[tokio::test]
async fn observed_address_three_production_quic_swarms_replace_and_publish_loopback() {
    fn production_swarm() -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig {
                identify_interval_secs: 1,
                ..LibP2PConfig::default()
            },
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            PeerExclusions::default(),
        )
        .expect("production QUIC swarm should build")
    }

    // Thirty seconds is a failure deadline, not a sleep. Identify runs every second
    // and this returns as soon as a downstream consumer receives the endpoint.
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut subject = production_swarm();
        let mut first = production_swarm();
        let mut second = production_swarm();
        let subject_peer = *subject.local_peer_id();
        let observers = [*first.local_peer_id(), *second.local_peer_id()];
        assert_ne!(observers[0], observers[1]);
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = subject.select_next_some().await {
                break address;
            }
        };
        let mut votes = ObservedAddressVotes::default();
        let primed_at = tokio::time::Instant::now() - Duration::from_secs(1);
        // Controlled fixtures only prime capacity. The target claim below comes
        // exclusively from real authenticated QUIC/Identify Received events.
        let _priming_pairs = observed_address_test_fill(&mut subject, &mut votes, 16, primed_at);
        let victim = observed_address_test_claim(4000);
        let victim_witnesses = votes.admitted[&victim].observers;
        assert_eq!(subject.external_addresses().count(), 16);
        assert!(!observed_address_test_has(&subject, &address));
        first
            .dial(address.clone().with_p2p(subject_peer).unwrap())
            .unwrap();
        let mut identified = BTreeSet::new();
        let mut second_started = false;
        let exclusions = PeerExclusions::default();
        let metrics = isolated_test_metrics();
        loop {
            let (actor, event) = tokio::select! {
                event = subject.select_next_some() => (0, event),
                event = first.select_next_some() => (1, event),
                event = second.select_next_some() => (2, event),
            };
            match event {
                SwarmEvent::Behaviour(NockchainEvent::Identify(Received {
                    peer_id,
                    connection_id,
                    info,
                })) if actor == 0 => {
                    assert!(observers.contains(&peer_id));
                    assert_eq!(info.public_key.to_peer_id(), peer_id);
                    assert_eq!(info.observed_addr, address);
                    let endpoint = subject
                        .behaviour()
                        .recovery
                        .connection_endpoint(&peer_id, connection_id)
                        .expect("genuine Identify must have exact live transport evidence");
                    assert!(endpoint.ip().is_loopback());
                    identified.insert(peer_id);
                    identify_received(
                        &mut subject, &mut votes, peer_id, connection_id, info, &exclusions,
                        &metrics,
                    )
                    .unwrap();
                    if identified.len() == 1 {
                        assert!(!observed_address_test_has(&subject, &address));
                        assert!(observed_address_test_has(&subject, &victim));
                        if !second_started {
                            second
                                .dial(address.clone().with_p2p(subject_peer).unwrap())
                                .unwrap();
                            second_started = true;
                        }
                    } else {
                        assert!(observed_address_test_has(&subject, &address));
                        assert!(!observed_address_test_has(&subject, &victim));
                        assert_eq!(votes.admitted.len(), 16);
                        assert_eq!(votes.retired.len(), 1);
                        assert_eq!(votes.retired[0].admitted_at, primed_at);
                        assert_eq!(votes.retired[0].observers, victim_witnesses);
                        assert_eq!(subject.external_addresses().count(), 16);
                        let receipt = &votes.admitted[&address];
                        assert!(
                            receipt.admitted_at.duration_since(primed_at)
                                < OBSERVED_ADDRESS_ADMITTED_TTL
                        );
                        for observer in observers {
                            assert!(receipt.observers.contains(&observer));
                        }
                    }
                }
                SwarmEvent::Behaviour(NockchainEvent::Identify(Received {
                    peer_id, info, ..
                })) => {
                    assert_eq!(peer_id, subject_peer);
                    if info.listen_addrs.contains(&address) {
                        assert_eq!(identified.len(), 2);
                        assert!(observed_address_test_has(&subject, &address));
                        assert!(!observed_address_test_has(&subject, &victim));
                        assert_eq!(subject.external_addresses().count(), 16);
                        break;
                    }
                }
                SwarmEvent::ExternalAddrConfirmed { address }
                | SwarmEvent::ExternalAddrExpired { address }
                    if actor == 0 =>
                {
                    votes.relinquish(&address);
                }
                SwarmEvent::OutgoingConnectionError { error, .. } => {
                    panic!("loopback production QUIC dial failed: {error}");
                }
                SwarmEvent::ConnectionClosed { .. } => {
                    panic!("both authenticated observers must remain live");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("fresh shared-IP quorum must replace and advertise to a real consumer promptly");
}

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
fn should_redial_initial_peers_recovers_named_seed_even_with_other_outbound_peers() {
    let initial_peers: Vec<Multiaddr> = vec![
        "/ip4/127.0.0.1/udp/30001/quic-v1/p2p/12D3KooWQb2uWwR7C3yFqKf4LxQ7s7rC5QZr9dA4zD4m6J7QfJ6A"
            .parse()
            .expect("valid multiaddr"),
    ];

    assert!(should_redial_initial_peers(0, true, &initial_peers, &[]));
    assert!(should_redial_initial_peers(8, true, &initial_peers, &[]));
    assert!(!should_redial_initial_peers(8, false, &initial_peers, &[]));
}

#[test]
fn should_redial_initial_peers_requires_seed_peers() {
    let initial_peers: Vec<Multiaddr> = vec![
        "/ip4/127.0.0.1/udp/30001/quic-v1/p2p/12D3KooWQb2uWwR7C3yFqKf4LxQ7s7rC5QZr9dA4zD4m6J7QfJ6A"
            .parse()
            .expect("valid multiaddr"),
    ];

    // No seed or backbone peers: nothing to redial.
    assert!(!should_redial_initial_peers(0, false, &[], &[]));
    // With seed peers and zero outbound peers we redial regardless of how
    // many attempts have already been made — we never give up.
    assert!(should_redial_initial_peers(0, true, &initial_peers, &[]));
    // Backbone peers alone are enough to trigger a redial.
    assert!(should_redial_initial_peers(0, true, &[], &initial_peers));
    // Public backbone addresses have no /p2p suffix. They still need periodic
    // redial if a different set of outbound peers occupies the connections.
    assert!(should_redial_initial_peers(8, true, &[], &initial_peers));
}

/// the redial backoff is a pure function of "did this tick redial".
#[test]
fn initial_peer_redial_backoff_grows_while_missing_and_resets_when_connected() {
    let mut backoff = INITIAL_PEER_REDIAL_INTERVAL;
    backoff = next_initial_peer_redial_backoff(backoff, true);
    assert_eq!(backoff, INITIAL_PEER_REDIAL_INTERVAL * 2);
    for _ in 0..16 {
        backoff = next_initial_peer_redial_backoff(backoff, true);
    }
    assert_eq!(backoff, INITIAL_PEER_REDIAL_MAX_INTERVAL);
    assert_eq!(
        next_initial_peer_redial_backoff(backoff, true),
        INITIAL_PEER_REDIAL_MAX_INTERVAL,
        "the backoff holds at the cap while a configured peer stays missing"
    );
    assert_eq!(
        next_initial_peer_redial_backoff(backoff, false),
        INITIAL_PEER_REDIAL_INTERVAL,
        "one tick with every configured peer connected resets straight to the base interval"
    );
    assert_eq!(
        next_initial_peer_redial_backoff(INITIAL_PEER_REDIAL_INTERVAL * 8, false),
        INITIAL_PEER_REDIAL_INTERVAL
    );
}

fn configured_recovery_scheduler_test_swarm() -> Swarm<NockchainBehaviour> {
    start_swarm(
        LibP2PConfig {
            handshake_timeout_secs: 1,
            swarm_idle_timeout_secs: 2,
            ..LibP2PConfig::default()
        },
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("production swarm should build")
}

async fn configured_recovery_scheduler_test_endpoint() -> (tokio::net::UdpSocket, Multiaddr) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = format!(
        "/ip4/127.0.0.1/udp/{}/quic-v1",
        socket.local_addr().unwrap().port(),
    )
    .parse()
    .unwrap();
    (socket, address)
}

async fn configured_recovery_scheduler_finish_failure(
    swarm: &mut Swarm<NockchainBehaviour>,
    state: &P2PState,
    initial: &[Multiaddr],
    backbone: &[Multiaddr],
) -> Multiaddr {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let SwarmEvent::OutgoingConnectionError {
                connection_id,
                peer_id,
                error,
                ..
            } = swarm.select_next_some().await
            {
                assert!(matches!(&error, DialError::Transport(_)), "{error}");
                let address = swarm
                    .behaviour()
                    .recovery
                    .pending_address(connection_id)
                    .expect("the failed transport dial must belong to configured recovery")
                    .clone();
                reclaim_recovery_slot(
                    swarm, state, connection_id, peer_id, &error, initial, backbone,
                );
                assert!(!swarm.behaviour().recovery.pending_at(&address));
                return address;
            }
        }
    })
    .await
    .expect("the nonresponsive endpoint must finish its actual transport dial")
}

#[tokio::test(start_paused = true)]
async fn configured_recovery_lost_last_path_preempts_hourly_background_backoff() {
    use futures::FutureExt;

    let (_healthy_socket, healthy) = configured_recovery_scheduler_test_endpoint().await;
    let (_other_healthy_socket, other_healthy) =
        configured_recovery_scheduler_test_endpoint().await;
    let (_dead_socket, dead) = configured_recovery_scheduler_test_endpoint().await;
    let backbone = vec![dead.clone(), healthy.clone(), other_healthy.clone()];
    let mut swarm = configured_recovery_scheduler_test_swarm();
    let mut state = P2PState::new(isolated_test_metrics(), 100);
    let healthy_id = ConnectionId::new_unchecked(90_000);
    let endpoint = libp2p::core::ConnectedPoint::Dialer {
        address: healthy.clone(),
        role_override: libp2p::core::Endpoint::Dialer,
        port_use: libp2p::core::transport::PortUse::Reuse,
    };
    state.track_connection(
        healthy_id,
        PeerId::random(),
        endpoint.get_remote_address(),
        endpoint.clone(),
    );
    let other_healthy_id = ConnectionId::new_unchecked(90_001);
    let other_endpoint = libp2p::core::ConnectedPoint::Dialer {
        address: other_healthy.clone(),
        role_override: libp2p::core::Endpoint::Dialer,
        port_use: libp2p::core::transport::PortUse::Reuse,
    };
    state.track_connection(
        other_healthy_id,
        PeerId::random(),
        other_endpoint.get_remote_address(),
        other_endpoint.clone(),
    );
    let mut recovery = ConfiguredPeerRedial::new();
    recovery.observe_path(&swarm, &state, &[], &backbone);
    let mut cursor = 0;
    let mut remaining = INITIAL_PEER_REDIAL_INTERVAL;
    let epsilon = Duration::from_millis(1);

    // A live path plus one permanently absent endpoint is not an all-connected
    // pool. Real failed dials must keep their bounded background backoff.
    for attempt in 0..12 {
        tokio::time::advance(remaining - epsilon).await;
        assert!(recovery.timer.tick().now_or_never().is_none());
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            0,
            "no dial before the armed deadline",
        );
        tokio::time::advance(epsilon).await;
        recovery.timer.tick().await;
        recovery
            .redial(&mut swarm, &state, &[], &backbone, &mut cursor, 3)
            .unwrap();
        let retry_at = tokio::time::Instant::now() + recovery.backoff;
        assert!(swarm.behaviour().recovery.pending_at(&dead));
        assert!(!swarm.behaviour().recovery.pending_at(&healthy));
        assert!(!swarm.behaviour().recovery.pending_at(&other_healthy));
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            1,
        );
        assert_eq!(
            configured_recovery_scheduler_finish_failure(&mut swarm, &state, &[], &backbone).await,
            dead,
        );
        if attempt >= 9 {
            assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_MAX_INTERVAL);
        }
        remaining = retry_at.saturating_duration_since(tokio::time::Instant::now());
        assert!(remaining > epsilon);
    }
    assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_MAX_INTERVAL);

    // Losing one configured path while another remains usable must not turn
    // background pool maintenance into urgent recovery or reset its deadline.
    state.lost_connection(other_healthy_id);
    recovery.observe_path(&swarm, &state, &[], &backbone);
    assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_MAX_INTERVAL);
    tokio::time::advance(INITIAL_PEER_REDIAL_INTERVAL).await;
    recovery.observe_path(&swarm, &state, &[], &backbone);
    assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_MAX_INTERVAL);
    assert!(recovery.timer.tick().now_or_never().is_none());

    // A background attempt may already be in flight when the last live path
    // disappears. Its real failure must not keep postponing urgent recovery.
    assert!(dial_recovery_peer(&mut swarm, dead.clone()));

    // The actual connection-loss path must preempt the outstanding hour-long
    // deadline even though the permanently dead pool member never recovered.
    state.lost_connection(healthy_id);
    let lost_at = tokio::time::Instant::now();
    recovery.observe_path(&swarm, &state, &[], &backbone);
    assert_eq!(
        configured_recovery_scheduler_finish_failure(&mut swarm, &state, &[], &backbone).await,
        dead,
    );
    recovery.observe_path(&swarm, &state, &[], &backbone);
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(1)).await;
        recovery.observe_path(&swarm, &state, &[], &backbone);
        assert!(recovery.timer.tick().now_or_never().is_none());
    }
    let remaining = INITIAL_PEER_REDIAL_INTERVAL
        .checked_sub(lost_at.elapsed())
        .expect("the pending failed dial must leave time before the base deadline");
    assert!(remaining > epsilon);
    tokio::time::advance(remaining - epsilon).await;
    assert!(recovery.timer.tick().now_or_never().is_none());
    assert_eq!(
        swarm
            .network_info()
            .connection_counters()
            .num_pending_outgoing(),
        0
    );
    tokio::time::advance(epsilon).await;
    recovery.timer.tick().await;
    recovery
        .redial(&mut swarm, &state, &[], &backbone, &mut cursor, 3)
        .unwrap();
    let recovery_delay = tokio::time::Instant::now() - lost_at;
    assert!(recovery_delay >= INITIAL_PEER_REDIAL_INTERVAL);
    assert!(recovery_delay <= INITIAL_PEER_REDIAL_INTERVAL + epsilon);
    assert!(swarm.behaviour().recovery.pending_at(&healthy));
    assert!(swarm.behaviour().recovery.pending_at(&other_healthy));
    assert!(swarm.behaviour().recovery.pending_at(&dead));
    assert_eq!(
        swarm
            .network_info()
            .connection_counters()
            .num_pending_outgoing(),
        3
    );
    assert!(
        recovery.timer.tick().now_or_never().is_none(),
        "no immediate retry storm"
    );
}

#[tokio::test(start_paused = true)]
async fn configured_recovery_scheduler_keeps_dead_backbone_windows_fair() {
    let mut sockets = Vec::new();
    let mut backbone = Vec::new();
    for _ in 0..3 {
        let (socket, address) = configured_recovery_scheduler_test_endpoint().await;
        sockets.push(socket);
        backbone.push(address);
    }
    let mut swarm = configured_recovery_scheduler_test_swarm();
    let state = P2PState::new(isolated_test_metrics(), 100);
    let mut recovery = ConfiguredPeerRedial::new();
    let mut cursor = 0;
    let mut counts = [0; 3];
    for round in 0..6 {
        recovery.timer.tick().await;
        recovery
            .redial(&mut swarm, &state, &[], &backbone, &mut cursor, 1)
            .unwrap();
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            1
        );
        let failed =
            configured_recovery_scheduler_finish_failure(&mut swarm, &state, &[], &backbone).await;
        assert_eq!(failed, backbone[round % backbone.len()]);
        counts[round % backbone.len()] += 1;
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            0
        );
    }
    assert_eq!(counts, [2, 2, 2]);
    drop(sockets);
}

#[tokio::test(start_paused = true)]
async fn configured_recovery_scheduler_respects_capacity_and_reserved_slots() {
    use futures::FutureExt;

    let (_socket, address) = configured_recovery_scheduler_test_endpoint().await;
    let state = P2PState::new(isolated_test_metrics(), 100);
    for reserved in [false, true] {
        let mut swarm = configured_recovery_scheduler_test_swarm();
        if reserved {
            swarm
                .behaviour_mut()
                .recovery
                .reserve(ConnectionId::new_unchecked(91_000), address.clone());
        } else {
            for index in 0..64 {
                let pending: Multiaddr = format!("/ip4/127.0.0.1/udp/{}/quic-v1", 30_000 + index,)
                    .parse()
                    .unwrap();
                assert!(swarm
                    .behaviour_mut()
                    .recovery
                    .track(ConnectionId::new_unchecked(92_000 + index), pending,));
            }
            assert!(!swarm.behaviour().recovery.has_capacity());
        }
        let mut recovery = ConfiguredPeerRedial::new();
        let mut cursor = 0;
        recovery.timer.tick().await;
        recovery
            .redial(
                &mut swarm,
                &state,
                &[],
                std::slice::from_ref(&address),
                &mut cursor,
                1,
            )
            .unwrap();
        assert!(!swarm.behaviour().recovery.pending_at(&address));
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            0
        );
        assert_eq!(cursor, 0);
        assert!(recovery.timer.tick().now_or_never().is_none());
        assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_INTERVAL * 2);
    }
}

#[tokio::test(start_paused = true)]
async fn configured_recovery_scheduler_empty_pool_never_dials() {
    let mut swarm = configured_recovery_scheduler_test_swarm();
    let state = P2PState::new(isolated_test_metrics(), 100);
    let mut recovery = ConfiguredPeerRedial::new();
    let mut cursor = 0;
    for _ in 0..3 {
        recovery.timer.tick().await;
        recovery
            .redial(&mut swarm, &state, &[], &[], &mut cursor, 0)
            .unwrap();
        assert_eq!(recovery.backoff, INITIAL_PEER_REDIAL_INTERVAL);
        assert_eq!(
            swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            0
        );
        assert_eq!(cursor, 0);
    }
}

#[tokio::test]
async fn configured_startup_dials_remain_pending_until_transport_completion() {
    fn test_swarm(listeners: Vec<Multiaddr>) -> Swarm<NockchainBehaviour> {
        let config = LibP2PConfig {
            handshake_timeout_secs: 1,
            swarm_idle_timeout_secs: 2,
            ..LibP2PConfig::default()
        };
        start_swarm(
            config,
            Keypair::generate_ed25519(),
            listeners,
            None,
            connection_limits::ConnectionLimits::default().with_max_pending_outgoing(Some(32)),
            None,
            PeerExclusions::default(),
        )
        .expect("production swarm should build")
    }

    async fn listen_at(swarm: &mut Swarm<NockchainBehaviour>, address: &Multiaddr) {
        swarm
            .listen_on(address.clone())
            .expect("listener should bind");
        loop {
            if let SwarmEvent::NewListenAddr {
                address: actual, ..
            } = swarm.select_next_some().await
            {
                assert_eq!(&actual, address);
                return;
            }
        }
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        for named in [false, true] {
            for initially_online in [false, true] {
                // An ordinary UDP socket keeps an offline endpoint nonresponsive
                // until the real transport reports its terminal handshake error.
                let mut offline_socket =
                    Some(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
                let port = offline_socket.as_ref().unwrap().local_addr().unwrap().port();
                let address: Multiaddr =
                    format!("/ip4/127.0.0.1/udp/{port}/quic-v1").parse().unwrap();
                let mut server = test_swarm(Vec::new());
                let server_peer = *server.local_peer_id();
                let configured = if named {
                    address.clone().with_p2p(server_peer).unwrap()
                } else {
                    address.clone()
                };
                let mut client =
                    test_swarm(vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()]);
                let mut state = P2PState::new(isolated_test_metrics(), 100);
                let mut cursor = 0;

                if initially_online {
                    drop(offline_socket.take());
                    listen_at(&mut server, &address).await;
                }
                dial_peers(&mut client, std::slice::from_ref(&configured), true).unwrap();
                // A overlapping backbone startup window must share the same
                // admission state, just like an immediate recovery trigger.
                dial_peers(&mut client, std::slice::from_ref(&configured), true).unwrap();
                assert!(redial_initial_peers(
                    &mut client,
                    &state,
                    std::slice::from_ref(&configured),
                    &[],
                    &mut cursor,
                    0,
                    "initial_configured_pending",
                )
                .unwrap());
                assert!(client.behaviour().recovery.pending_at(&configured));
                assert_eq!(
                    client.network_info().connection_counters().num_pending_outgoing(),
                    1,
                );

                if !initially_online {
                    loop {
                        if let SwarmEvent::OutgoingConnectionError {
                            connection_id, peer_id, error, ..
                        } = client.select_next_some().await
                        {
                            assert!(matches!(&error, DialError::Transport(_)));
                            reclaim_recovery_slot(
                                &mut client,
                                &state,
                                connection_id,
                                peer_id,
                                &error,
                                std::slice::from_ref(&configured),
                                &[],
                            );
                            break;
                        }
                    }
                    assert!(!client.behaviour().recovery.pending_at(&configured));
                    assert_eq!(
                        client.network_info().connection_counters().num_pending_outgoing(),
                        0,
                    );
                    // Restore the same socket endpoint and the same server key.
                    drop(offline_socket.take());
                    listen_at(&mut server, &address).await;
                    assert!(redial_initial_peers(
                        &mut client,
                        &state,
                        std::slice::from_ref(&configured),
                        &[],
                        &mut cursor,
                        0,
                        "configured_transport_completed",
                    )
                    .unwrap());
                    assert!(client.behaviour().recovery.pending_at(&configured));
                }

                let mut client_connected = false;
                let mut server_connected = false;
                while !client_connected || !server_connected {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished {
                                connection_id, peer_id, endpoint, ..
                            } => {
                                assert!(!client_connected, "only one configured connection");
                                assert_eq!(peer_id, server_peer);
                                assert!(endpoint.is_dialer());
                                assert!(client.behaviour_mut().recovery.finish(connection_id).is_some());
                                state.track_connection(
                                    connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone(),
                                );
                                client_connected = true;
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => {
                                panic!("restored configured endpoint failed: {error}");
                            }
                            _ => {}
                        },
                        event = server.select_next_some() => {
                            if let SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } = event {
                                assert!(!server_connected, "server accepts only one connection");
                                assert_eq!(peer_id, *client.local_peer_id());
                                assert!(endpoint.is_listener());
                                server_connected = true;
                            }
                        }
                    }
                }
                assert!(!client.behaviour().recovery.pending_at(&configured));
                assert_eq!(client.network_info().connection_counters().num_pending_outgoing(), 0);
                assert_eq!(client.network_info().connection_counters().num_established_outgoing(), 1);
                assert_eq!(server.network_info().connection_counters().num_established_incoming(), 1);
                assert!(!redial_initial_peers(
                    &mut client,
                    &state,
                    std::slice::from_ref(&configured),
                    &[],
                    &mut cursor,
                    0,
                    "configured_connected",
                )
                .unwrap());
            }
        }
    })
    .await
    .expect("real configured startup and terminal retry must complete");
}

/// a `/dnsaddr` backbone without a `/p2p` suffix counts as connected once
/// its dial is tracked, so `configured_peer_missing` clears and the backoff
/// resets instead of ratcheting to the hourly cap.
///
/// libp2p reports an outbound connection under the address exactly as dialed
/// (the DNS transport resolves internally), so the tracked endpoint of a
/// dnsaddr dial is the unresolved `/dnsaddr/...` form. The live counterpart is
/// `dns_backbone_dial_is_tracked_under_its_configured_address`.
#[tokio::test]
async fn dnsaddr_backbone_counts_as_connected_once_its_dial_is_tracked() {
    let mut swarm = start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("production swarm should build");
    let mut state = P2PState::new(isolated_test_metrics(), 100);
    let backbone: Multiaddr = "/dnsaddr/backbone.example".parse().unwrap();
    let connection_id = ConnectionId::new_unchecked(7);
    assert!(p2p_peer_id(&backbone).is_none());
    assert!(!configured_peer_connected(&swarm, &state, &backbone));

    let endpoint = libp2p::core::ConnectedPoint::Dialer {
        address: backbone.clone(),
        role_override: libp2p::core::Endpoint::Dialer,
        port_use: libp2p::core::transport::PortUse::Reuse,
    };
    state.track_connection(
        connection_id,
        PeerId::random(),
        endpoint.get_remote_address(),
        endpoint.clone(),
    );
    assert!(state.has_outbound_connection_at_address(&backbone));
    assert!(configured_peer_connected(&swarm, &state, &backbone));

    let mut cursor = 0;
    let redialed = redial_initial_peers(
        &mut swarm,
        &state,
        &[],
        std::slice::from_ref(&backbone),
        &mut cursor,
        1,
        "dnsaddr_backbone_connected",
    )
    .expect("redial decision should succeed");
    assert!(
        !redialed,
        "a connected dnsaddr backbone must not count as missing"
    );
    assert_eq!(
        next_initial_peer_redial_backoff(INITIAL_PEER_REDIAL_MAX_INTERVAL, redialed),
        INITIAL_PEER_REDIAL_INTERVAL,
        "a tick that finds the backbone connected resets the backoff"
    );

    state.lost_connection(connection_id);
    assert!(!configured_peer_connected(&swarm, &state, &backbone));
    assert!(redial_initial_peers(
        &mut swarm,
        &state,
        &[],
        &[backbone],
        &mut cursor,
        1,
        "dnsaddr_backbone_lost",
    )
    .expect("redial decision should succeed"));
}

/// DNS-named recovery keeps its configured alias while request accounting
/// follows the numeric socket reported by the production QUIC transport.
#[tokio::test]
async fn dns_backbone_dial_is_tracked_under_its_configured_address() {
    use libp2p::swarm::SwarmEvent;

    fn test_swarm() -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            PeerExclusions::default(),
        )
        .expect("production swarm should build")
    }

    let mut victim = test_swarm();
    let mut backbone = test_swarm();
    let backbone_addr = loop {
        if let Some(SwarmEvent::NewListenAddr { address, .. }) = backbone.next().await {
            break address;
        }
    };
    let port = backbone_addr
        .iter()
        .find_map(|protocol| match protocol {
            Protocol::Udp(port) => Some(port),
            _ => None,
        })
        .expect("listen address should carry a UDP port");
    let configured: Multiaddr = format!("/dns4/localhost/udp/{port}/quic-v1")
        .parse()
        .unwrap();
    let mut state = P2PState::new(isolated_test_metrics(), 100);

    tokio::time::timeout(Duration::from_secs(20), async {
        victim
            .dial(configured.clone())
            .expect("dns dial should start");
        loop {
            tokio::select! {
                event = victim.select_next_some() => match event {
                    SwarmEvent::ConnectionEstablished {
                        peer_id, connection_id, endpoint, ..
                    } => {
                        assert_eq!(
                            endpoint.get_remote_address(),
                            &configured,
                            "libp2p reports the connection under the address as dialed"
                        );
                        state.track_connection(
                            connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone()
                        );
                    }
                    SwarmEvent::Behaviour(NockchainEvent::ConnectionAddressChanged {
                        peer_id, connection_id, address: new,
                    }) => {
                        let numeric = crate::p2p_util::quic_socket_addr(&new)
                            .expect("DNS transport must report the numeric QUIC socket");
                        state.update_connection_address(connection_id, &new);
                        let endpoints = victim.behaviour().recovery.request_endpoints(&peer_id)
                            .expect("SDK selection candidates must have resolved endpoints");
                        assert_eq!(endpoints.len(), 1);
                        assert_eq!(endpoints[0].connection_id, connection_id);
                        assert_eq!(endpoints[0].endpoint, numeric);
                        assert_eq!(state.connection_address(connection_id), Some(new.clone()));
                        assert_eq!(state.ip_bucket_connection_count(connection_id).unwrap().1, 1);
                        state.update_connection_address(connection_id, &new);
                        assert_eq!(state.ip_bucket_connection_count(connection_id).unwrap().1, 1);
                        assert!(state.recovery_connection_to_reclaim(&[], std::slice::from_ref(&configured)).is_none());
                        break;
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => {
                        panic!("dns dial to the local backbone failed: {error}");
                    }
                    _ => {}
                },
                _ = backbone.select_next_some() => {}
            }
        }
    })
    .await
    .expect("dns-named backbone dial must complete before timeout");

    assert!(state.has_outbound_connection_at_address(&configured));
    assert!(configured_peer_connected(&victim, &state, &configured));
    let mut cursor = 0;
    assert!(!redial_initial_peers(
        &mut victim,
        &state,
        &[],
        &[configured],
        &mut cursor,
        1,
        "live_dns_backbone_connected",
    )
    .expect("connected dns backbone should not redial"));
}

#[tokio::test]
async fn unnamed_backbone_recovers_with_unrelated_outbound_connection() {
    use libp2p::swarm::SwarmEvent;

    fn test_swarm() -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            PeerExclusions::default(),
        )
        .expect("production swarm should build")
    }

    async fn listen_address(swarm: &mut Swarm<NockchainBehaviour>) -> Multiaddr {
        loop {
            if let Some(SwarmEvent::NewListenAddr { address, .. }) = swarm.next().await {
                return address;
            }
        }
    }

    let mut victim = test_swarm();
    let mut unrelated = test_swarm();
    let mut backbone = test_swarm();
    let unrelated_addr = listen_address(&mut unrelated).await;
    let backbone_addr = listen_address(&mut backbone).await;
    let unrelated_id = *unrelated.local_peer_id();
    let backbone_id = *backbone.local_peer_id();
    let mut state = P2PState::new(isolated_test_metrics(), 100);

    tokio::time::timeout(Duration::from_secs(20), async {
        victim
            .dial(unrelated_addr)
            .expect("unrelated dial should start");
        loop {
            tokio::select! {
                event = victim.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished {
                        peer_id, connection_id, endpoint, ..
                    } = event {
                        state.track_connection(
                            connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone()
                        );
                        if peer_id == unrelated_id { break; }
                    }
                }
                _ = unrelated.select_next_some() => {}
            }
        }
        assert_eq!(state.outbound_peer_count(), 1);
        assert!(!state.has_outbound_connection_at_address(&backbone_addr));

        let mut cursor = 0;
        assert!(redial_initial_peers(
            &mut victim,
            &state,
            &[],
            std::slice::from_ref(&backbone_addr),
            &mut cursor,
            1,
            "live_unnamed_backbone_recovery"
        )
        .expect("backbone redial should start"));
        loop {
            tokio::select! {
                event = victim.select_next_some() => {
                    if let SwarmEvent::ConnectionEstablished {
                        peer_id, connection_id, endpoint, ..
                    } = event {
                        state.track_connection(
                            connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone()
                        );
                        if peer_id == backbone_id { break; }
                    }
                }
                _ = unrelated.select_next_some() => {}
                _ = backbone.select_next_some() => {}
            }
        }
        assert_eq!(state.outbound_peer_count(), 2);
        assert!(state.has_outbound_connection_at_address(&backbone_addr));
        assert!(!redial_initial_peers(
            &mut victim,
            &state,
            &[],
            &[backbone_addr],
            &mut cursor,
            1,
            "live_unnamed_backbone_recovery"
        )
        .expect("connected backbone should not redial"));
    })
    .await
    .expect("real swarm recovery must complete before timeout");
}

async fn assert_configured_recovery_reclaims_slot(total_limit: bool) {
    fn test_swarm(limits: connection_limits::ConnectionLimits) -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            limits,
            None,
            PeerExclusions::default(),
        )
        .expect("production swarm should build")
    }
    async fn listen(swarm: &mut Swarm<NockchainBehaviour>) -> Multiaddr {
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                return address;
            }
        }
    }
    let limits = if total_limit {
        connection_limits::ConnectionLimits::default().with_max_established(Some(1))
    } else {
        connection_limits::ConnectionLimits::default().with_max_established_outgoing(Some(1))
    };
    let mut victim = test_swarm(limits);
    let mut unrelated = test_swarm(connection_limits::ConnectionLimits::default());
    let mut recovery = test_swarm(connection_limits::ConnectionLimits::default());
    let unrelated_address = listen(&mut unrelated).await;
    let recovery_address = listen(&mut recovery).await;
    let recovery_id = *recovery.local_peer_id();
    let mut state = P2PState::new(isolated_test_metrics(), 100);
    let configured = vec![recovery_address.clone()];
    victim.dial(unrelated_address).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut attempted = false;
        let mut reclaimed = false;
        loop {
            tokio::select! {
                event = victim.select_next_some() => match event {
                    SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                        victim.behaviour_mut().recovery.finish(connection_id);
                        state.track_connection(connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                        if peer_id == recovery_id {
                            assert!(reclaimed, "the existing slot must actually be reclaimed");
                            assert_eq!(state.outbound_peer_count(), 1);
                            break;
                        }
                        if !attempted {
                            attempted = true;
                            state.record_request_success(peer_id);
                            assert!(state.try_admit_inbound_req_res(peer_id, 4));
                            let mut cursor = 0;
                            redial_initial_peers(&mut victim, &state, &configured, &[], &mut cursor, 0, "test").unwrap();
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { connection_id, peer_id, error, .. } => {
                        reclaim_recovery_slot(&mut victim, &state, connection_id, peer_id, &error, &configured, &[]);
                    }
                    SwarmEvent::ConnectionClosed { connection_id, .. } => {
                        state.lost_connection(connection_id);
                        if let Some(address) = victim.behaviour_mut().recovery.closed(connection_id) {
                            reclaimed = true;
                            dial_recovery_peer(&mut victim, address);
                        }
                    }
                    _ => {}
                },
                _ = unrelated.select_next_some() => {},
                _ = recovery.select_next_some() => {},
            }
        }
    }).await.expect("configured endpoint must recover without unrelated peer cooperation");
}

#[tokio::test]
async fn configured_recovery_reclaims_an_exhausted_outbound_slot() {
    assert_configured_recovery_reclaims_slot(false).await;
}

#[tokio::test]
async fn configured_recovery_reclaims_exhausted_total_capacity() {
    assert_configured_recovery_reclaims_slot(true).await;
}

#[test]
fn configured_recovery_preserves_configured_and_recently_useful_connections() {
    let mut state = P2PState::new(isolated_test_metrics(), 100);
    let trusted = PeerId::random();
    let useful = PeerId::random();
    let idle = PeerId::random();
    let mut configured = Vec::new();
    for (number, peer) in [(1, trusted), (2, useful), (3, idle)] {
        let address: Multiaddr = format!("/ip4/127.0.0.1/udp/{}/quic-v1", 3000 + number)
            .parse()
            .unwrap();
        if peer == trusted {
            configured.push(address.clone());
        }
        let endpoint = libp2p::core::ConnectedPoint::Dialer {
            address: address.clone(),
            role_override: libp2p::core::Endpoint::Dialer,
            port_use: libp2p::core::transport::PortUse::Reuse,
        };
        state.track_connection(
            ConnectionId::new_unchecked(number),
            peer,
            &address,
            endpoint,
        );
    }
    state.track_block_id_str_and_peer("accepted".to_owned(), useful);
    state.validate_and_untrack_block_sources("accepted".to_owned());
    state.record_accepted_chain_progress("accepted", 1);
    assert_eq!(
        state.recovery_connection_to_reclaim(&configured, &[]),
        Some(ConnectionId::new_unchecked(3))
    );
    state.lost_connection(ConnectionId::new_unchecked(3));
    assert_eq!(state.recovery_connection_to_reclaim(&configured, &[]), None);
}

#[tokio::test]
async fn configured_recovery_preserves_other_peers_on_per_peer_denial() {
    fn test_swarm(per_peer: Option<u32>) -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default()
                .with_max_established_per_peer(per_peer)
                .with_max_established_outgoing(Some(4))
                .with_max_established(Some(8)),
            None,
            PeerExclusions::default(),
        )
        .expect("production swarm should build")
    }
    async fn listen(swarm: &mut Swarm<NockchainBehaviour>) -> Multiaddr {
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                return address;
            }
        }
    }
    let mut victim = test_swarm(Some(1));
    let mut target = test_swarm(None);
    let mut other = test_swarm(None);
    let victim_address = listen(&mut victim).await;
    let target_address = listen(&mut target).await;
    let other_address = listen(&mut other).await;
    let target_id = *target.local_peer_id();
    let other_id = *other.local_peer_id();
    let mut state = P2PState::new(isolated_test_metrics(), 100);
    target.dial(victim_address).unwrap();
    victim.dial(other_address).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut attempted = false;
        loop {
            tokio::select! {
                event = victim.select_next_some() => match event {
                    SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                        state.track_connection(connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                        if !attempted && victim.is_connected(&target_id) && victim.is_connected(&other_id) {
                            attempted = true;
                            dial_recovery_peer(&mut victim, target_address.clone());
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { connection_id, peer_id, error, .. } => {
                        assert_eq!(peer_id, Some(target_id));
                        assert!(matches!(&error, DialError::Denied { cause }
                            if cause.downcast_ref::<connection_limits::Exceeded>().is_some()));
                        reclaim_recovery_slot(&mut victim, &state, connection_id, peer_id, &error,
                            std::slice::from_ref(&target_address), &[]);
                        assert!(!victim.behaviour_mut().recovery.reserved(), "another peer cannot relieve the target's per-peer limit");
                        assert!(victim.is_connected(&other_id));
                        assert_eq!(victim.network_info().connection_counters().num_established_outgoing(), 1);
                        assert_eq!(victim.network_info().connection_counters().num_established(), 2);
                        break;
                    }
                    SwarmEvent::ConnectionClosed { connection_id, peer_id, .. } => {
                        state.lost_connection(connection_id);
                        panic!("existing connection {peer_id} must not be reclaimed");
                    }
                    _ => {}
                },
                _ = target.select_next_some() => {},
                _ = other.select_next_some() => {},
            }
        }
    }).await.expect("alternate unnamed endpoint should produce the per-peer denial");
}

#[tokio::test]
async fn configured_recovery_admission_rotates_and_deduplicates_pending_endpoints() {
    // Real, non-responding UDP endpoints give each configured dial its normal
    // transport lifecycle without racing other listeners or freeing slots early.
    let mut sockets = Vec::new();
    let mut addresses = Vec::new();
    for _ in 0..130 {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        addresses.push(
            format!("/ip4/127.0.0.1/udp/{port}/quic-v1")
                .parse::<Multiaddr>()
                .unwrap(),
        );
        sockets.push(socket);
    }
    let (initial, backbone) = addresses.split_at(65);
    let state = P2PState::new(isolated_test_metrics(), 100);
    for pending_limit in [32, 128] {
        let config = LibP2PConfig {
            handshake_timeout_secs: 1,
            swarm_idle_timeout_secs: 2,
            ..LibP2PConfig::default()
        };
        let mut swarm = start_swarm(
            config,
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default()
                .with_max_pending_outgoing(Some(pending_limit)),
            None,
            PeerExclusions::default(),
        )
        .unwrap();
        let admitted = pending_limit.min(64);
        let rounds = if pending_limit == 32 { 5 } else { 3 };
        let mut cursor = 0;
        tokio::time::timeout(Duration::from_secs(20), async {
            for round in 0..rounds {
                redial_initial_peers(
                    &mut swarm, &state, initial, backbone, &mut cursor, 65, "test",
                )
                .unwrap();
                assert_eq!(
                    swarm
                        .network_info()
                        .connection_counters()
                        .num_pending_outgoing(),
                    admitted
                );
                redial_initial_peers(
                    &mut swarm, &state, initial, backbone, &mut cursor, 65, "test",
                )
                .unwrap();
                assert_eq!(
                    swarm
                        .network_info()
                        .connection_counters()
                        .num_pending_outgoing(),
                    admitted
                );
                if round == rounds - 1 {
                    assert!(swarm.behaviour().recovery.pending_at(&initial[64]));
                    assert!(swarm.behaviour().recovery.pending_at(&backbone[64]));
                    break;
                }
                for _ in 0..admitted {
                    loop {
                        if let SwarmEvent::OutgoingConnectionError { connection_id, .. } =
                            swarm.select_next_some().await
                        {
                            swarm.behaviour_mut().recovery.finish(connection_id);
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("both configured list tails must be admitted after bounded transport completions");
    }
    drop(sockets);
}

#[tokio::test]
async fn configured_transport_outage_keeps_retry_admissible() {
    for case in [
        "initial", "backbone", "unnamed", "outer_timeout", "discovery", "alternate",
        "other_identity",
    ] {
        let mut config = LibP2PConfig::default();
        if case == "outer_timeout" {
            config.handshake_timeout_secs = 20;
            config.swarm_idle_timeout_secs = 1;
        } else {
            config.handshake_timeout_secs = 1;
        }
        let exclusions = PeerExclusions::default();
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
        let keypair = Keypair::generate_ed25519();
        let peer = keypair.public().to_peer_id();
        // Keep the endpoint allocated but non-responsive until the real QUIC
        // dial completes, then serve that same endpoint and identity.
        let offline = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = loopback_quic_addr_with_port(offline.local_addr().unwrap().port());
        let alternate = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let alternate_address =
            loopback_quic_addr_with_port(alternate.local_addr().unwrap().port());
        let intended = match case {
            "alternate" => alternate_address.with_p2p(peer).unwrap(),
            "other_identity" => address.clone().with_p2p(PeerId::random()).unwrap(),
            "unnamed" => address.clone(),
            _ => address.clone().with_p2p(peer).unwrap(),
        };
        let initial = if matches!(case, "backbone" | "discovery") {
            Vec::new()
        } else {
            vec![intended.clone()]
        };
        let backbone = if case == "backbone" {
            vec![intended]
        } else {
            Vec::new()
        };
        let dial_address = if case == "unnamed" {
            address.clone()
        } else {
            address.clone().with_p2p(peer).unwrap()
        };
        let mut client = start_swarm(
            config.clone(),
            Keypair::generate_ed25519(),
            Vec::new(),
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            exclusions.clone(),
        )
        .unwrap();
        let retry_admissible = matches!(case, "initial" | "backbone" | "unnamed" | "outer_timeout");
        assert!(dial_recovery_peer(&mut client, dial_address.clone()));
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let SwarmEvent::OutgoingConnectionError {
                    connection_id,
                    peer_id,
                    error,
                    ..
                } = client.select_next_some().await
                {
                    assert!(matches!(&error, DialError::Transport(_)), "{case}: {error}");
                    reclaim_recovery_slot(
                        &mut client,
                        &*state.lock().await,
                        connection_id,
                        peer_id,
                        &error,
                        &initial,
                        &backbone,
                    );
                    handle_outgoing_connection_error(
                        &mut client, &state, &exclusions, &metrics, error, peer_id, &initial,
                        &backbone,
                    )
                    .await;
                    break;
                }
            }
        })
        .await
        .expect("the allocated non-responsive endpoint must finish its dial");
        assert_eq!(
            exclusions.is_address_excluded(&dial_address, p2p_peer_id(&dial_address)),
            !retry_admissible,
            "{case}",
        );
        assert_eq!(
            metrics.address_cooldowns_created.fetch_add(0),
            usize::from(!retry_admissible)
        );
        if !retry_admissible {
            assert!(
                matches!(
                    client.dial(dial_address.clone()),
                    Err(DialError::Denied { .. })
                ),
                "{case}"
            );
        } else {
            drop(offline);
            let mut server = start_swarm(
                config,
                keypair,
                vec![address.clone()],
                None,
                connection_limits::ConnectionLimits::default(),
                None,
                PeerExclusions::default(),
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    if let SwarmEvent::NewListenAddr { .. } = server.select_next_some().await {
                        break;
                    }
                }
                client
                    .dial(dial_address.clone())
                    .expect("an ordinary configured outage must not deny a fresh dial");
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                assert_eq!(peer_id, peer);
                                break;
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => {
                                panic!("{case}: the restored endpoint must connect: {error}");
                            }
                            _ => {}
                        },
                        _ = server.select_next_some() => {}
                    }
                }
            })
            .await
            .expect("the fresh configured retry must reach the restored peer");
        }
        let ip = address.canonical_ip_addr().unwrap();
        assert_eq!(
            exclusions
                .record_kad_cardinality(ip, usize::MAX, usize::MAX)
                .is_some(),
            !retry_admissible,
            "configured outages must not supply corroborating IP failure evidence: {case}",
        );
        if retry_admissible {
            // Other observed behavior still excludes the IP, including its
            // configured endpoint; configuration is not an admission bypass.
            exclusions.record_wrong_peer_id(&address, Some(PeerId::random()), PeerId::random());
            assert!(exclusions
                .record_kad_cardinality(ip, usize::MAX, usize::MAX)
                .is_some());
            assert!(matches!(
                client.dial(dial_address),
                Err(DialError::Denied { .. })
            ));
        }
    }
}

#[tokio::test]
async fn configured_transport_penalties_still_deny_retry() {
    use libp2p::core::transport::TransportError;

    for case in ["permission", "permission_wrapped", "invalid", "unsupported", "unknown"] {
        let exclusions = PeerExclusions::default();
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
        let mut client = start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            Vec::new(),
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            exclusions.clone(),
        )
        .unwrap();
        let peer = PeerId::random();
        let address = loopback_quic_addr_with_port(36048).with_p2p(peer).unwrap();
        let error = if case == "unsupported" {
            TransportError::MultiaddrNotSupported(address.clone())
        } else if case == "permission_wrapped" {
            TransportError::Other(std::io::Error::other(libp2p::quic::Error::Io(
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "egress denied"),
            )))
        } else {
            TransportError::Other(std::io::Error::new(
                match case {
                    "permission" => std::io::ErrorKind::PermissionDenied,
                    "invalid" => std::io::ErrorKind::InvalidData,
                    _ => std::io::ErrorKind::Other,
                },
                "dial failed",
            ))
        };
        handle_outgoing_connection_error(
            &mut client,
            &state,
            &exclusions,
            &metrics,
            DialError::Transport(vec![(address.clone(), error)]),
            Some(peer),
            std::slice::from_ref(&address),
            &[],
        )
        .await;
        assert!(
            exclusions.is_address_excluded(&address, Some(peer)),
            "{case}"
        );
        assert!(
            !exclusions.is_ip_excluded(&address.canonical_ip_addr().unwrap()),
            "{case}"
        );
        assert_eq!(metrics.address_cooldowns_created.fetch_add(0), 1);
        assert!(
            matches!(client.dial(address.clone()), Err(DialError::Denied { .. })),
            "{case}"
        );
        let after_permission_cooldown = Instant::now()
            + Duration::from_secs(
                crate::config::PeerExclusionConfig::default().permission_denied_cooldown_secs + 1,
            );
        assert_eq!(
            exclusions.is_address_excluded_at(&address, Some(peer), after_permission_cooldown),
            !case.starts_with("permission"),
            "PermissionDenied must retain its own shorter cooldown: {case}",
        );
    }
}

#[tokio::test]
async fn configured_ping_outages_preserve_retry_and_semantic_penalties() {
    for dns in [false, true] {
        for case in [
            "timeout", "reset", "permission", "invalid", "unsupported", "unknown", "alternate",
        ] {
            let exclusions = PeerExclusions::default();
            let metrics = isolated_test_metrics();
            let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
            let mut client = start_swarm(
                LibP2PConfig::default(),
                Keypair::generate_ed25519(),
                Vec::new(),
                None,
                connection_limits::ConnectionLimits::default(),
                None,
                exclusions.clone(),
            )
            .unwrap();
            let mut server = start_swarm(
                LibP2PConfig::default(),
                Keypair::generate_ed25519(),
                vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
                None,
                connection_limits::ConnectionLimits::default(),
                None,
                PeerExclusions::default(),
            )
            .unwrap();
            let peer = *server.local_peer_id();
            tokio::time::timeout(Duration::from_secs(20), async {
                let numeric = loop {
                    if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await {
                        break address;
                    }
                };
                let port = quic_socket_addr(&numeric).unwrap().port();
                let original = if dns {
                    format!("/dns4/localhost/udp/{port}/quic-v1").parse::<Multiaddr>().unwrap()
                } else {
                    numeric.clone().with_p2p(peer).unwrap()
                };
                let configured = if case == "alternate" {
                    loopback_quic_addr_with_port(if port == u16::MAX { port - 1 } else { port + 1 })
                        .with_p2p(peer).unwrap()
                } else {
                    original.clone()
                };
                client.dial(original.clone()).unwrap();
                let connection = loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                                state.lock().await.track_connection(
                                    connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone(),
                                );
                                if !dns { break connection_id; }
                            }
                            SwarmEvent::Behaviour(NockchainEvent::ConnectionAddressChanged {
                                connection_id, address, ..
                            }) => {
                                state.lock().await.update_connection_address(connection_id, &address);
                                if dns { break connection_id; }
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => panic!("{error}"),
                            _ => {}
                        },
                        _ = server.select_next_some() => {}
                    }
                };
                assert!(state.lock().await.has_outbound_connection_at_address(&original));
                let error = match case {
                    "timeout" | "alternate" => ping::Failure::Timeout,
                    "unsupported" => ping::Failure::Unsupported,
                    _ => ping::Failure::Other {
                        error: Box::new(std::io::Error::new(match case {
                            "reset" => std::io::ErrorKind::ConnectionReset,
                            "permission" => std::io::ErrorKind::PermissionDenied,
                            "invalid" => std::io::ErrorKind::InvalidData,
                            _ => std::io::ErrorKind::Other,
                        }, "ping failed")),
                    },
                };
                handle_ping_event(
                    &mut client, &state, &exclusions, &metrics,
                    ping::Event { peer, connection, result: Err(error) },
                    2, &[], std::slice::from_ref(&configured),
                ).await;
                let outage = matches!(case, "timeout" | "reset");
                assert_eq!(
                    exclusions.is_address_excluded(&numeric, Some(peer)), !outage,
                    "{case}, DNS={dns}",
                );
                assert!(client.is_connected(&peer), "one failure must retain the connection");
                assert_eq!(metrics.address_cooldowns_created.fetch_add(0), usize::from(!outage));
                assert_eq!(
                    exclusions.record_kad_cardinality(
                        numeric.canonical_ip_addr().unwrap(), usize::MAX, usize::MAX,
                    ).is_some(), !outage,
                    "{case}, DNS={dns}",
                );
                if !outage {
                    assert!(matches!(
                        client.dial(numeric.clone().with_p2p(peer).unwrap()),
                        Err(DialError::Denied { .. }),
                    ));
                    return;
                }
                handle_ping_event(
                    &mut client, &state, &exclusions, &metrics,
                    ping::Event { peer, connection, result: Err(ping::Failure::Timeout) },
                    2, &[], std::slice::from_ref(&configured),
                ).await;
                loop {
                    tokio::select! {
                        event = client.select_next_some() => if let SwarmEvent::ConnectionClosed {
                            connection_id, ..
                        } = event {
                            assert_eq!(connection_id, connection);
                            state.lock().await.lost_connection(connection_id);
                            break;
                        },
                        _ = server.select_next_some() => {}
                    }
                }
                client.dial(original).expect("ping closure must leave configured recovery admissible");
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                assert_eq!(peer_id, peer);
                                break;
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => panic!("{error}"),
                            _ => {}
                        },
                        _ = server.select_next_some() => {}
                    }
                }
            }).await.expect("ping failure policy must retain normal close and retry behavior");
        }
    }
}

async fn assert_ping_closure_preserves_healthy_shared_ip_connection(same_peer: bool) {
    fn test_swarm() -> Swarm<NockchainBehaviour> {
        start_swarm(
            LibP2PConfig::default(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
            None,
            connection_limits::ConnectionLimits::default().with_max_established_per_peer(Some(2)),
            None,
            PeerExclusions::default(),
        )
        .unwrap()
    }
    async fn listen(swarm: &mut Swarm<NockchainBehaviour>) -> Multiaddr {
        loop {
            if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                return address;
            }
        }
    }
    let mut client = test_swarm();
    let mut target = test_swarm();
    let mut healthy = test_swarm();
    let target_address = listen(&mut target).await;
    let healthy_address = listen(&mut healthy).await;
    let target_peer = *target.local_peer_id();
    let healthy_peer = if same_peer {
        target_peer
    } else {
        *healthy.local_peer_id()
    };
    let configured = target_address.clone().with_p2p(target_peer).unwrap();
    let metrics = isolated_test_metrics();
    let exclusions = PeerExclusions::default();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut established = Vec::new();
        for (peer, address) in [
            (healthy_peer, if same_peer { target_address.clone() } else { healthy_address }),
            (target_peer, target_address),
        ] {
            let opts = libp2p::swarm::dial_opts::DialOpts::peer_id(peer)
                .condition(libp2p::swarm::dial_opts::PeerCondition::Always)
                .addresses(vec![address])
                .build();
            let expected_cid = opts.connection_id();
            client.dial(opts).unwrap();
            loop {
                tokio::select! {
                    event = client.select_next_some() => match event {
                        SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                            assert_eq!((connection_id, peer_id), (expected_cid, peer));
                            state.lock().await.track_connection(
                                connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone(),
                            );
                            established.push(connection_id);
                            break;
                        }
                        SwarmEvent::OutgoingConnectionError { error, .. } => panic!("{error}"),
                        SwarmEvent::ConnectionClosed { .. } => panic!("both sessions must remain live"),
                        _ => {}
                    },
                    _ = target.select_next_some() => {},
                    _ = healthy.select_next_some() => {},
                }
            }
        }
        let healthy_cid = established[0];
        let mut failing_cid = established[1];
        let threshold = LibP2PConfig::default().failed_pings_before_close();
        let mut backoff = INITIAL_PEER_REDIAL_MAX_INTERVAL;
        let mut cursor = 0;
        for _ in 0..if same_peer { 1 } else { 2 } {
            let redialed = redial_initial_peers(
                &mut client, &*state.lock().await, std::slice::from_ref(&configured), &[],
                &mut cursor, 0, "configured_ping_healthy",
            ).unwrap();
            assert!(!redialed);
            backoff = next_initial_peer_redial_backoff(backoff, redialed);
            assert_eq!(backoff, INITIAL_PEER_REDIAL_INTERVAL);
            for _ in 0..threshold {
                handle_ping_event(
                    &mut client, &state, &exclusions, &metrics,
                    ping::Event { peer: target_peer, connection: failing_cid, result: Err(ping::Failure::Timeout) },
                    threshold, std::slice::from_ref(&configured), &[],
                ).await;
                handle_ping_event(
                    &mut client, &state, &exclusions, &metrics,
                    ping::Event { peer: healthy_peer, connection: healthy_cid, result: Ok(Duration::ZERO) },
                    threshold, std::slice::from_ref(&configured), &[],
                ).await;
            }
            loop {
                tokio::select! {
                    event = client.select_next_some() => {
                        if let SwarmEvent::ConnectionClosed { connection_id, peer_id, .. } = event {
                            assert_eq!((connection_id, peer_id), (failing_cid, target_peer));
                            state.lock().await.lost_connection(connection_id);
                            break;
                        }
                    },
                    _ = target.select_next_some() => {},
                    _ = healthy.select_next_some() => {},
                }
            }
            assert!(client.is_connected(&healthy_peer));
            assert_eq!(client.network_info().connection_counters().num_established_outgoing(), 1);
            assert!(!exclusions.is_address_excluded(&configured, Some(target_peer)));
            assert_eq!(metrics.address_cooldowns_created.fetch_add(0), 0);
            for (peer, connection) in [
                (target_peer, failing_cid),
                (PeerId::random(), healthy_cid),
                (target_peer, ConnectionId::new_unchecked(usize::MAX)),
            ] {
                for _ in 0..threshold {
                    handle_ping_event(
                        &mut client, &state, &exclusions, &metrics,
                        ping::Event { peer, connection, result: Err(ping::Failure::Timeout) },
                        threshold, std::slice::from_ref(&configured), &[],
                    ).await;
                }
            }
            let redialed = redial_initial_peers(
                &mut client, &*state.lock().await, std::slice::from_ref(&configured), &[],
                &mut cursor, 0, "configured_ping_connection_closed",
            ).unwrap();
            assert_eq!(redialed, !same_peer);
            backoff = next_initial_peer_redial_backoff(backoff, redialed);
            assert_eq!(
                backoff,
                INITIAL_PEER_REDIAL_INTERVAL * if same_peer { 1 } else { 2 },
            );
            if !same_peer {
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                                assert_eq!(peer_id, target_peer);
                                assert_ne!(connection_id, failing_cid);
                                assert_ne!(connection_id, healthy_cid);
                                client.behaviour_mut().recovery.finish(connection_id);
                                state.lock().await.track_connection(
                                    connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone(),
                                );
                                failing_cid = connection_id;
                                break;
                            }
                            SwarmEvent::ConnectionClosed { .. } => panic!("healthy session must survive"),
                            SwarmEvent::OutgoingConnectionError { error, .. } => panic!("{error}"),
                            _ => {}
                        },
                        _ = target.select_next_some() => {},
                        _ = healthy.select_next_some() => {},
                    }
                }
                assert!(client.is_connected(&healthy_peer));
                assert_eq!(client.network_info().connection_counters().num_established_outgoing(), 2);
            } else {
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionClosed { .. } => {
                                panic!("stale and foreign ping events must not close the healthy CID");
                            }
                            SwarmEvent::Behaviour(NockchainEvent::Ping(event))
                                if event.connection == healthy_cid && event.result.is_ok() => break,
                            _ => {}
                        },
                        _ = target.select_next_some() => {},
                        _ = healthy.select_next_some() => {},
                    }
                }
                assert!(client.is_connected(&healthy_peer));
            }
        }
    }).await.expect("connection-local ping failures must close and permit configured recovery");
}

#[tokio::test]
async fn configured_ping_failures_close_only_the_failing_shared_ip_peer_connection() {
    assert_ping_closure_preserves_healthy_shared_ip_connection(false).await;
}

#[tokio::test]
async fn configured_ping_failures_close_only_the_failing_same_peer_connection() {
    assert_ping_closure_preserves_healthy_shared_ip_connection(true).await;
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

    None
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
                    observer: _,
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
                    observer: _,
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
                    observer: _,
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

    // Many peers feed high-priority pokes into one serialized dispatcher
    // while peeks and timers retain service.
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
    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected outbound request after deferred flush, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&queued)
        .expect("queued request should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(11)
        ),
        "expected block-by-height request, got {parsed:?}"
    );
    assert!(
        buffered_swarm_actions.is_empty(),
        "authorized cache hit should queue only flush plus the kernel singleton"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_by_height_request_with_buffered_block_and_prefetch_disabled_dispatches_outbound() {
    // A ready deferred-buffer hit queues local flush work without replacing
    // the kernel's normal by-height request path.
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

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred flush action, got {other:?}"),
    }
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
        "ready cache hit should queue only flush plus the kernel singleton"
    );

    let state_guard = state_arc.lock().await;
    assert!(
        state_guard.has_deferred_block_at_height(height),
        "buffered block should remain available"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deferred_candidate_does_not_suppress_kernel_singleton() {
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
    .expect("frontier cache hit should queue deferred flush and keep singleton live");

    match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::FlushDeferredHeardBlocks) => {}
        other => panic!("expected deferred flush action, got {other:?}"),
    }
    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected outbound request after deferred flush, got {other:?}"),
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
    assert!(
        buffered_swarm_actions.is_empty(),
        "deferred candidate should queue only flush plus the kernel singleton"
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

#[tokio::test]
async fn retained_future_gossip_prefetches_on_native_frontier_demand() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let (future, _) = heard_block_fact_with_tx_ids(24, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &future else {
        panic!("expected heard-block fact");
    };
    {
        let mut state = state_arc.lock().await;
        state.first_negative = 3;
        assert!(state.defer_heard_block(peer, 24, block_id.clone(), future));
    }
    let mut actions = VecDeque::new();
    handle_effect_with_dispatcher(
        request_slab_from_message(block_by_height_message(3).as_ref()).unwrap(),
        &mut SwarmActionDispatcher::Buffered(&mut actions),
        vec![peer],
        false,
        PrefetchConfig {
            enabled: true,
            window_initial: LIBP2P_CONFIG.prefetch_window_initial,
            window_max: LIBP2P_CONFIG.prefetch_window_max,
            max_inflight_per_peer: LIBP2P_CONFIG.prefetch_max_inflight_per_peer,
            kernel_demand_threshold: 8,
        },
        runtime_limits_from_config(&LIBP2P_CONFIG),
        Arc::clone(&state_arc),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("frontier demand with retained future gossip should dispatch catch-up");
    assert_eq!(
        actions.len(),
        2,
        "range advisory must accompany the kernel singleton"
    );
    let SwarmAction::QueueKernelRequest {
        peer_id,
        request_message,
    } = actions.pop_front().unwrap()
    else {
        panic!("expected range request");
    };
    assert_eq!(peer_id, peer);
    assert!(
        matches!(
            crate::messages::decode_request_item_message(&request_message).unwrap(),
            crate::messages::NockchainDataRequest::BlockRangeWithTxs {
                start_height: 3,
                len: 2
            }
        ),
        "unknown range capability must use the standard bounded probe at demanded height"
    );
    let SwarmAction::QueueKernelRequest {
        peer_id,
        request_message,
    } = actions.pop_front().unwrap()
    else {
        panic!("expected singleton request");
    };
    assert_eq!(peer_id, peer);
    assert!(matches!(
        crate::messages::decode_request_item_message(&request_message).unwrap(),
        crate::messages::NockchainDataRequest::BlockByHeight(3)
    ));
    let state = state_arc.lock().await;
    assert_eq!(
        state.first_negative, 3,
        "future gossip is not accepted progress"
    );
    assert!(state.has_kernel_block_height_request(3));
    assert!(!state.has_kernel_block_height_request(24));
    assert!(state.has_deferred_block_at_height(24));
    assert!(!state.has_ready_deferred_heard_blocks());
}

#[tokio::test]
async fn retained_future_gossip_prefetch_preserves_boundaries_and_fallbacks() {
    // Inclusive catch-up depth matches the existing deep-demand threshold.
    for (case, frontier, demand, future_height, expected_range) in [
        ("no_hint", 3, 3, None, false),
        ("below_threshold", 3, 3, Some(9), false),
        ("threshold", 3, 3, Some(10), true),
        ("nonfrontier", 3, 4, Some(24), false),
        ("genesis", 0, 0, Some(24), false),
        ("prefetch_only", 3, 3, Some(24), false),
        ("invalidated", 3, 3, Some(24), false),
        ("evicted", 3, 3, Some(24), false),
        ("disabled", 3, 3, Some(24), false),
        ("unsupported", 3, 3, Some(24), false),
        ("inflight", 3, 3, Some(24), false),
        ("covered", 3, 3, Some(24), false),
        ("no_peer", 3, 3, Some(24), false),
        ("ordinary_deep_demand", 3, 10, None, true),
    ] {
        let metrics = isolated_test_metrics();
        let state_arc = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        {
            let mut state = state_arc.lock().await;
            state.first_negative = frontier;
            state.mark_peer_range_supported(peer);
            if let Some(height) = future_height {
                let (future, _) = heard_block_fact_with_tx_ids(height, &[]);
                let NockchainFact::HeardBlock(block_id, _) = &future else {
                    panic!("expected heard-block fact");
                };
                let block_id = block_id.clone();
                assert!(state.defer_heard_block_with_source(
                    peer,
                    height,
                    block_id.clone(),
                    future,
                    if case == "prefetch_only" {
                        BlockSource::Prefetch
                    } else {
                        BlockSource::Gossip
                    },
                ));
                if case == "invalidated" {
                    assert!(state.invalidate_deferred_block_id(&block_id));
                } else if case == "evicted" {
                    state.set_deferred_heard_block_byte_caps(0, 0);
                }
            }
            if case == "unsupported" {
                state.mark_peer_non_range_capable(peer);
            } else if case == "inflight" || case == "covered" {
                state.register_prefetch(
                    fresh_outbound_request_id(),
                    peer,
                    if case == "covered" { demand } else { 100 },
                    2,
                );
            }
        }
        let mut actions = VecDeque::new();
        handle_effect_with_dispatcher(
            request_slab_from_message(block_by_height_message(demand).as_ref()).unwrap(),
            &mut SwarmActionDispatcher::Buffered(&mut actions),
            if case == "no_peer" {
                vec![]
            } else {
                vec![peer]
            },
            false,
            PrefetchConfig {
                enabled: case != "disabled",
                window_initial: 4,
                window_max: 4,
                max_inflight_per_peer: 1,
                kernel_demand_threshold: 8,
            },
            runtime_limits_from_config(&LIBP2P_CONFIG),
            Arc::clone(&state_arc),
            metrics,
            PeerExclusions::default(),
        )
        .await
        .expect("catch-up boundaries should retain ordinary dispatch");
        assert_eq!(
            actions.len(),
            if expected_range {
                2
            } else if case == "no_peer" {
                0
            } else {
                1
            },
            "{case}"
        );
        if expected_range {
            let Some(SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            }) = actions.pop_front()
            else {
                panic!("{case}: expected range");
            };
            assert_eq!(peer_id, peer, "{case}");
            assert!(
                matches!(
                    crate::messages::decode_request_item_message(&request_message).unwrap(),
                    crate::messages::NockchainDataRequest::BlockRangeWithTxs { start_height, len: 4 }
                        if start_height == demand
                ),
                "{case}: preserve standard window and demanded start"
            );
        }
        if case != "no_peer" {
            let Some(SwarmAction::QueueKernelRequest {
                request_message, ..
            }) = actions.pop_front()
            else {
                panic!("{case}: expected singleton");
            };
            assert!(
                matches!(
                    crate::messages::decode_request_item_message(&request_message).unwrap(),
                    crate::messages::NockchainDataRequest::BlockByHeight(height) if height == demand
                ),
                "{case}: preserve kernel singleton"
            );
        }
        let state = state_arc.lock().await;
        assert_eq!(state.first_negative, frontier, "{case}");
        assert!(!state.has_ready_deferred_heard_blocks(), "{case}");
        if let Some(height) = future_height {
            assert!(!state.has_kernel_block_height_request(height), "{case}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefetch_disabled_falls_through_to_singleton_outbound() {
    // With prefetch disabled, the cache-miss path must dispatch the existing
    // singleton block-by-height request.
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.mark_peer_range_supported(peer);
    }
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
    // Range prefetch is driven by validated kernel demand and runs
    // concurrently with the kernel's singleton block-by-height request.
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
        state_guard.first_negative = 43;
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
        2,
        "range prefetch and singleton should both be queued"
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
    let singleton = match &buffered_swarm_actions[1] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected singleton QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&singleton)
        .expect("singleton message should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(50)
        ),
        "expected concurrent BlockByHeight singleton, got {parsed:?}"
    );
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
        2,
        "range prefetch and singleton should both be queued"
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
    let singleton = match &buffered_swarm_actions[1] {
        SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        } => {
            assert_eq!(*peer_id, peer);
            request_message.clone()
        }
        other => panic!("expected singleton QueueKernelRequest, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&singleton)
        .expect("singleton message should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(50)
        ),
        "expected concurrent BlockByHeight singleton, got {parsed:?}"
    );
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
        state_guard.first_negative = 43;
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
        state_guard.first_negative = 60;
        state_guard.record_response_message_hint(
            &crate::messages::NockchainDataRequest::BlockRangeWithTxs {
                start_height: 50,
                len: 17,
            },
            1_995_241,
        );
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
async fn prefetch_skips_non_range_peer_listed_first() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let unsupported_peer = PeerId::random();
    let range_peer = PeerId::random();
    {
        let mut state_guard = state_arc.lock().await;
        for h in 1_000..1_010u64 {
            state_guard.defer_heard_block(
                range_peer,
                h,
                format!("future-block-{h}"),
                heard_block_fact_with_tx_ids(h, &[]).0,
            );
        }
        state_guard.first_negative = 43;
        state_guard.mark_peer_range_supported(range_peer);
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
        vec![unsupported_peer, range_peer],
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
            assert_eq!(peer_id, range_peer);
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
async fn prefetch_probes_peer_with_unknown_range_capability() {
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
        state_guard.first_negative = 43;
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
async fn prefetch_falls_back_to_singleton_when_no_connected_peer_is_available() {
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
    .expect("prefetch without a connected peer should use the singleton path");

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
async fn inflight_prefetch_does_not_suppress_kernel_singleton() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    {
        let mut state_guard = state_arc.lock().await;
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

    let queued = match buffered_swarm_actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            request_message, ..
        }) => request_message,
        other => panic!("expected kernel singleton request, got {other:?}"),
    };
    let parsed = crate::messages::decode_request_item_message(&queued)
        .expect("queued request should decode");
    assert!(
        matches!(
            parsed,
            crate::messages::NockchainDataRequest::BlockByHeight(52)
        ),
        "expected BlockByHeight singleton, got {parsed:?}"
    );
    assert!(
        buffered_swarm_actions.is_empty(),
        "inflight prefetch should avoid only a duplicate range request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_parent_retries_back_off_after_failure_budget() {
    // Repeated requests for an unavailable height back off after the
    // configured failure budget.
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
                    &PeerExclusions::default(),
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

#[tokio::test]
async fn only_height_bearing_seen_block_clears_sync_peer_attempts() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    state_arc
        .lock()
        .await
        .track_block_height_attempted_peers(42, [peer]);

    for (effect, should_clear) in [
        (seen_block_payload_only_effect_slab(42_001), false),
        (seen_block_effect_slab(42_002, 42), true),
    ] {
        let mut buffered_swarm_actions = VecDeque::new();
        let mut swarm_actions = SwarmActionDispatcher::Buffered(&mut buffered_swarm_actions);
        handle_effect_with_dispatcher(
            effect,
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
        .expect("seen block effect should be processed");
        assert_eq!(
            state_arc
                .lock()
                .await
                .get_block_height_attempted_peers(42)
                .is_empty(),
            should_clear,
            "only an accepted block should recycle a height's peer attempts"
        );
    }
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
async fn route_response_fact_deferred_future_heard_block_records_tx_hints_without_prefetch() {
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

    assert!(
        tokio::time::timeout(Duration::from_millis(25), swarm_rx.recv())
            .await
            .is_err(),
        "deferred future heard-block must not issue raw-tx prefetches",
    );
    {
        let state_guard = state_arc.lock().await;
        assert_eq!(
            state_guard.deferred_heard_block_heights(),
            vec![11],
            "future heard-block should remain buffered until frontier advances"
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
        peer, block_11, &scripted_traffic.traffic, &metrics, &state_arc, &swarm_tx,
    )
    .await
    .expect("duplicate future heard-block should still defer cleanly");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), swarm_rx.recv())
            .await
            .is_err(),
        "duplicate future heard-block must not issue raw-tx prefetches",
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
async fn block_height_source_preference_native_demand_after_failure_budget() {
    let mut checkpoint_app = start_nockchain_app().await;
    seed_fakenet_pre_genesis_direct(&mut checkpoint_app.app).await;
    let _ = send_born_direct(&mut checkpoint_app.app).await;
    let mut peers = (0u8..33)
        .map(|seed| {
            libp2p::identity::Keypair::ed25519_from_bytes([seed; 32])
                .expect("fixed ed25519 seed should be valid")
                .public()
                .to_peer_id()
        })
        .collect::<Vec<_>>();
    peers.sort_unstable();
    let source = peers[32];
    let genesis_id = match fake_genesis_block_message_fact() {
        NockchainFact::HeardBlock(block_id, _) => block_id,
        other => panic!("expected genesis heard-block fact, got {other:?}"),
    };
    let synthetic_branch_head = {
        let mut slab = NounSlab::new();
        let noun = tip5_tuple(&mut slab, 90_001);
        let space = slab.noun_space();
        tip5_hash_to_base58(noun, &space).unwrap()
    };
    let heard_elders = heard_elders_fact(0, &[synthetic_branch_head, genesis_id]);
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let address: Multiaddr = format!("/ip4/127.0.0.1/udp/3000/quic-v1/p2p/{source}")
        .parse()
        .unwrap();
    {
        let mut guard = state.lock().await;
        for (index, peer) in peers.iter().enumerate() {
            seed_connected_peer(&mut guard, *peer, 500 + index);
        }
        guard.track_connection(
            ConnectionId::new_unchecked(600),
            source,
            &address,
            libp2p::core::ConnectedPoint::Dialer {
                address: address.clone(),
                role_override: libp2p::core::Endpoint::Dialer,
                port_use: libp2p::core::transport::PortUse::Reuse,
            },
        );
        guard.set_configured_recovery_peers(Arc::from([address]), Arc::default());
        guard.set_prefetch_safety_config(8, Duration::from_secs(20), 50 * 1024 * 1024);
        let budget = guard.prefetch_height_failure_budget();
        let backoff = guard.prefetch_stuck_backoff();
        guard.track_block_height_attempted_peers(0, peers[..usize::from(budget)].iter().copied());
        for _ in 0..budget {
            guard.record_block_height_failure(0, budget, backoff);
        }
        assert!(guard.is_block_height_stuck(0));
    }
    let effects = poke_fact_direct(
        &mut checkpoint_app.app, source, &heard_elders, "native height demand",
    )
    .await;
    assert_eq!(request_effect_block_heights_from_effects(&effects), vec![0]);
    let effect = effects
        .into_iter()
        .find(|effect| request_effect_block_height(effect) == Some(0))
        .unwrap();
    let mut actions = VecDeque::new();
    handle_effect_with_dispatcher(
        effect,
        &mut SwarmActionDispatcher::Buffered(&mut actions),
        peers,
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        state.clone(),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .unwrap();
    match actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(peer_id, source);
            assert_eq!(request_message, block_by_height_message(0));
        }
        other => panic!("expected singleton height demand, got {other:?}"),
    }
    assert!(actions.is_empty());
    let guard = state.lock().await;
    assert!(
        guard.is_block_height_stuck(0),
        "kernel demand must not reset the retry budget"
    );
    assert!(guard.get_block_height_attempted_peers(0).contains(&source));
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
                libp2p::StreamProtocol::new(LibP2PConfig::req_res_protocol_version()),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );
    behaviour.send_request(
        &PeerId::random(),
        NockchainRequest::AuthenticatedGossip {
            pow: [0; 16],
            nonce: 0,
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

fn heard_block_fact_with_block_seed(
    height: u64,
    block_seed: u64,
    tx_seeds: &[u64],
) -> (NockchainFact, Vec<String>) {
    let mut slab = NounSlab::new();
    let block_id = tip5_tuple(&mut slab, block_seed);
    let parent_id = tip5_tuple(&mut slab, block_seed.wrapping_add(10_000));
    let tx_ids = tip5_zset(&mut slab, tx_seeds);
    let height_atom = Atom::new(&mut slab, height).as_noun();
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
            height_atom,
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

fn heard_block_fact_with_tx_ids(height: u64, tx_seeds: &[u64]) -> (NockchainFact, Vec<String>) {
    heard_block_fact_with_block_seed(height, 10_000u64.wrapping_add(height), tx_seeds)
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

fn measured_test_response(message: NockchainResponse) -> crate::behaviour::TransportResponse {
    let received_bytes = Some(req_res_message_encoded_bytes(&message).unwrap());
    crate::behaviour::TransportResponse {
        message,
        received_bytes,
    }
}

async fn recv_request_event(
    requester: &mut ReqResTestSwarm,
    responder: &mut ReqResTestSwarm,
    transcript: &DriverTranscript,
) -> (
    PeerId,
    ConnectionId,
    request_response::Message<NockchainRequest, crate::behaviour::TransportResponse>,
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
                                    return response.message;
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
    RequestExecutionOutcome::Result { envelope }
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
            gen2_batch_request_count += 1;
            if item_count == 1 {
                single_item_batch_count += 1;
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
            single_item_batch_count: 0,
            last_flush_ms: 0,
        }
    }

    fn record_flush(
        &mut self,
        reason: PendingBatchFlushReason,
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
        self.gen2_batch_request_count += 1;
        if item_count == 1 {
            self.single_item_batch_count += 1;
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
    let _request_context =
        take_pending_batch_request(pending_gen2_batches, peer, local_peer, equix_builder)
            .expect("recovery tuning flush should build request")
            .expect("recovery tuning flush should have pending items");
    accumulator.record_flush(
        reason, item_count, payload_bytes, estimated_response_bytes, limits, now_ms, enqueued_at_ms,
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
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
                    response: measured_test_response(NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    }),
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
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
                    response: measured_test_response(NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    }),
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
        println!("recovery block-batch sample skipped: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: wave as u64 + 1,
                    items: request_items.clone(),
                },
                0,
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
                    response: measured_test_response(NockchainResponse::BatchResult {
                        results: result_items.clone(),
                    }),
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
        block_range_max_len: config.prefetch_window_max.max(1),
        gen2_batch_max_items: config.gen2_batch_max_items(),
        gen2_batch_max_bytes: config.gen2_batch_max_bytes(),
        gen2_item_max_bytes: config.gen2_item_max_bytes(),
        gen2_block_batch_max_response_bytes: config.gen2_block_batch_max_response_bytes(),
        gen2_max_inflight_per_peer: config.gen2_max_inflight_per_peer(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_outbound_gossip_conversion_records_send_metric() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "authenticated outbound gossip conversion should record a send metric",
    );
    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
    let mut requester_equix = equix::EquiXBuilder::new();
    let gossip_message = ByteBuf::from(jam_heard_tx_response(61_001, 32));

    process_send_gossip_action(
        responder_peer_id,
        gossip_message.clone(),
        &mut requester,
        &driver_state,
        &metrics,
        &mut requester_equix,
    )
    .await
    .expect("authenticated outbound gossip should send");

    assert_eq!(metrics.authenticated_gossip_sent.fetch_add(0), 1);

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
async fn malformed_authenticated_gossip_pow_is_rejected_before_kernel_work() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "malformed authenticated gossip proof should block peer before kernel work",
    );
    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
    assert_eq!(metrics.authenticated_gossip_verified.fetch_add(0), 0);
    assert_eq!(metrics.gossip_dropped.fetch_add(0), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_gossip_routes_to_kernel() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "authenticated gossip should route to the kernel",
    );
    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
        "driver should route authenticated gossip",
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
    .expect("authenticated gossip should route");

    let response = match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse { channel, response } => {
            responder
                .behaviour_mut()
                .request_response
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
                .expect("authenticated gossip ack should send");
            response
        }
        other => panic!("expected authenticated gossip SendResponse, got {other:?}"),
    };
    assert_eq!(response, NockchainResponse::Ack { acked: true });
    let requester_response = recv_response_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(requester_response, NockchainResponse::Ack { acked: true });
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 1);
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
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items: vec![request_item],
            },
            0,
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
                response: measured_test_response(NockchainResponse::BatchResult { results }),
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
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items,
            },
            0,
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
                response: measured_test_response(NockchainResponse::BatchResult { results }),
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
    let started = Instant::now();

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
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items,
            },
            0,
        ),
    );
    let results = (0..item_count)
        .map(|idx| {
            tx_result_outcome_for_seed(40_000 + idx as u64, payload_len)
                .into_batch_result_item(idx as u32 + 1)
        })
        .collect::<Vec<_>>();
    let response_bytes =
        batch_result_encoded_bytes(&results).expect("resident-set response should encode");
    run_driver_with_timeout(
        transcript,
        "resident-set batch response processing",
        handle_request_response(
            peer,
            ConnectionId::new_unchecked(90 + item_count),
            request_response::Message::Response {
                request_id,
                response: measured_test_response(NockchainResponse::BatchResult { results }),
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
    .expect("resident-set response should process");
    match tokio::time::timeout(Duration::from_millis(50), swarm_rx.recv()).await {
        Ok(None) => {}
        Ok(Some(other)) => panic!("unexpected follow-up swarm action: {other:?}"),
        Err(_) => panic!("swarm action channel should close promptly for success path"),
    }

    let elapsed = started.elapsed();
    let rss_after_kib = current_rss_kib();
    let rss_peak_kib = rss_before_kib.max(current_rss_kib()).max(rss_after_kib);
    let poke_count = scripted_traffic.poke_count.load(Ordering::SeqCst);
    (
        response_bytes, elapsed, poke_count, rss_before_kib, rss_after_kib, rss_peak_kib,
    )
}

async fn run_two_peer_driver_latency_workload(
    item_count: usize,
    payload_len: usize,
    transcript: &DriverTranscript,
) -> (usize, Duration, String, usize) {
    let requester_config = LibP2PConfig::default();
    let responder_config = requester_config.clone();
    let protocol = first_common_outbound_protocol(&requester_config, &responder_config)
        .unwrap_or_else(|| String::from("none"));
    let mut requester = build_test_swarm(requester_config);
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
    .expect("latency batch request should process");
    let action = recv_swarm_action(&mut swarm_rx).await;
    let response = match action {
        SwarmAction::SendResponse { channel, response } => {
            responder
                .behaviour_mut()
                .request_response
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
                .expect("response should send");
            response
        }
        other => panic!("expected SendResponse, got {other:?}"),
    };
    let response_bytes = cbor4ii::serde::to_vec(Vec::new(), &response)
        .expect("latency batch response should encode")
        .len();
    let requester_response = recv_response_event(&mut requester, &mut responder, transcript).await;
    let NockchainResponse::BatchResult { results } = requester_response else {
        panic!("expected batch result response");
    };
    assert_eq!(results.len(), item_count);

    let elapsed = started.elapsed();
    let peek_count = scripted_traffic.peek_count.load(Ordering::SeqCst);
    assert_eq!(
        peek_count, item_count,
        "two-peer latency workload should perform one responder peek per logical item"
    );
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
    (response_bytes, elapsed, protocol, peek_count)
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
    let mut builder = equix::EquiXBuilder::new();
    let local_peer_id = PeerId::random();
    let remote_peer_id = PeerId::random();
    let message = ByteBuf::from(vec![1, 2, 3, 4, 5]);

    let valid_request = NockchainRequest::new_batch_request(
        &mut builder,
        &local_peer_id,
        &remote_peer_id,
        vec![BatchRequestItem {
            item_id: 0,
            message: message.clone(),
        }],
    )
    .expect("batch request proof should build");
    let NockchainRequest::BatchRequest { pow, nonce, items } = &valid_request else {
        panic!("expected BatchRequest");
    };
    valid_request
        .verify_pow(&mut builder, &remote_peer_id, &local_peer_id)
        .expect("valid batch request PoW should verify");

    let tampered_request = NockchainRequest::BatchRequest {
        pow: *pow,
        nonce: nonce + 1,
        items: items.clone(),
    };
    assert!(
        tampered_request
            .verify_pow(&mut builder, &remote_peer_id, &local_peer_id)
            .is_err(),
        "tampered nonce should fail verification"
    );
    assert!(
        valid_request
            .verify_pow(&mut builder, &local_peer_id, &remote_peer_id)
            .is_err(),
        "wrong peer order should fail verification"
    );

    let gossip_request = NockchainRequest::authenticated_gossip_from_message(
        &mut builder, &local_peer_id, &remote_peer_id, message,
    )
    .expect("authenticated gossip proof should build");
    gossip_request
        .verify_pow(&mut builder, &remote_peer_id, &local_peer_id)
        .expect("authenticated gossip PoW should verify");
}

fn gossip_test_context(
    slab: &mut NounSlab,
    origin: &str,
    phase: &str,
    source: Option<PeerId>,
) -> nockvm::noun::Noun {
    let origin = Atom::from_value(slab, origin).expect("producer origin");
    let phase = Atom::from_value(slab, phase).expect("producer phase");
    let source = match source {
        Some(peer) => {
            let peer = Atom::from_value(slab, peer.to_string().as_str()).expect("source peer");
            T(slab, &[D(0), peer.as_noun()])
        }
        None => D(0),
    };
    T(slab, &[origin.as_noun(), phase.as_noun(), source])
}

fn gossip_effect_from_message(
    message: &ByteBuf,
    origin: &str,
    phase: &str,
    source: Option<PeerId>,
) -> NounSlab {
    let payload = request_slab_from_message(message).expect("valid fact payload");
    let mut effect = NounSlab::new();
    let copied = effect.copy_into(unsafe { *payload.root() }, &payload.noun_space());
    let context = gossip_test_context(&mut effect, origin, phase, source);
    let noun = T(
        &mut effect,
        &[D(tas!(b"gossip")), context, D(FACT_POKE_VERSION), copied],
    );
    effect.set_root(noun);
    effect
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn first_peer_tx_effect_excludes_source_but_later_announcements_reach_it() {
    let first_source = PeerId::random();
    let second_source = PeerId::random();
    let recipient = PeerId::random();
    let peers = vec![first_source, second_source, recipient];
    let message = result_message_from_fact(&heard_tx_fact(91_777, 64));
    let other_message = result_message_from_fact(&heard_tx_fact(91_778, 64));
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    for (payload, origin, phase, source) in [
        (&message, "peer", "initial-acceptance", Some(first_source)),
        (&message, "local", "local-resend", None),
        (&message, "chain", "new-head", None),
        (&message, "mining", "mining-regossip", None),
        (
            &other_message,
            "peer",
            "initial-acceptance",
            Some(second_source),
        ),
    ] {
        let effect = gossip_effect_from_message(payload, origin, phase, source);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        handle_effect(
            effect,
            tx,
            peers.clone(),
            false,
            state.clone(),
            metrics.clone(),
        )
        .await
        .expect("canonical kernel gossip effect");
        for expected_peer in peers.iter().copied().filter(|peer| Some(*peer) != source) {
            match rx.recv().await {
                Some(SwarmAction::SendGossip {
                    peer_id, message, ..
                }) => {
                    assert_eq!(peer_id, expected_peer);
                    assert_eq!(
                        &message, payload,
                        "producer metadata must not alter fact bytes"
                    );
                }
                other => panic!("expected directed unchanged gossip payload, got {other:?}"),
            }
        }
        assert!(
            rx.try_recv().is_err(),
            "source excluded only for this initial peer effect"
        );
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn gossip_context_rejects_malformed_metadata_before_side_effects() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    state.lock().await.cache_block_response(42, NounSlab::new());
    let (_, message) = build_gossip_effect(FACT_POKE_VERSION, &[1, 2, 3]);
    for (origin, phase, source) in [
        ("invalid", "new-head", None),
        ("chain", "invalid", None),
        ("peer", "initial-acceptance", None),
        ("local", "local-resend", Some(PeerId::random())),
    ] {
        let effect = gossip_effect_from_message(&message, origin, phase, source);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        assert!(handle_effect(
            effect,
            tx,
            vec![PeerId::random()],
            false,
            state.clone(),
            metrics.clone()
        )
        .await
        .is_err());
        assert!(rx.try_recv().is_err());
        assert_eq!(state.lock().await.block_cache.len(), 1);
    }
    let payload = request_slab_from_message(&message).expect("valid payload");
    let mut invalid_unit = NounSlab::new();
    let copied = invalid_unit.copy_into(unsafe { *payload.root() }, &payload.noun_space());
    let context = T(
        &mut invalid_unit,
        &[D(tas!(b"chain")), D(tas!(b"new-head")), D(7)],
    );
    let noun = T(
        &mut invalid_unit,
        &[D(tas!(b"gossip")), context, D(FACT_POKE_VERSION), copied],
    );
    invalid_unit.set_root(noun);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    assert!(handle_effect(
        invalid_unit,
        tx,
        vec![PeerId::random()],
        false,
        state.clone(),
        metrics
    )
    .await
    .is_err());
    assert!(rx.try_recv().is_err());
    assert_eq!(state.lock().await.block_cache.len(), 1);
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
    let context = gossip_test_context(&mut effect_slab, "chain", "new-head", None);
    let effect = T(
        &mut effect_slab,
        &[D(tas!(b"gossip")), context, D(version), payload],
    );
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
    let state = P2PState::new(isolated_test_metrics(), 100);
    let exclusions = PeerExclusions::default();
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peer_c = PeerId::random();
    let peers = vec![peer_a, peer_b, peer_c];
    let mut expected = peers.clone();
    expected.sort_unstable_by_key(|peer| peer.to_base58());

    assert_eq!(
        state.select_request_peers_with_preferences(peers.clone(), 2, &exclusions, true, &[]),
        expected[..2].to_vec()
    );
    assert_eq!(
        state.select_request_peers_with_preferences(peers.clone(), 8, &exclusions, true, &[]),
        expected
    );
}

#[test]
fn test_select_request_peers_prefers_tx_source_hints() {
    let state = P2PState::new(isolated_test_metrics(), 100);
    let exclusions = PeerExclusions::default();
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peer_c = PeerId::random();
    let peer_d = PeerId::random();

    let selected = state.select_request_peers_with_preferences(
        vec![peer_a, peer_b, peer_c, peer_d],
        2,
        &exclusions,
        false,
        &[peer_c, peer_a, PeerId::random()],
    );
    assert_eq!(selected, vec![peer_c, peer_a]);

    let selected_with_stable_tail = state.select_request_peers_with_preferences(
        vec![peer_d, peer_b, peer_a, peer_c],
        3,
        &exclusions,
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
async fn block_height_source_preference_primary_and_alternate_keep_eligibility() {
    use crate::config::PeerExclusionConfig;

    for condition in ["eligible", "cooled", "excluded", "attempted", "all-cooled"] {
        for alternate in [false, true] {
            let metrics = isolated_test_metrics();
            let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
            let exclusions = PeerExclusions::new(PeerExclusionConfig {
                request_peer_cooldown_secs: 60,
                wrong_peer_id_ip_threshold: 1,
                ..PeerExclusionConfig::default()
            });
            let mut peers = (0..4).map(|_| PeerId::random()).collect::<Vec<_>>();
            peers.sort_unstable();
            let source = peers[3];
            let address: Multiaddr = format!("/ip4/203.0.113.9/udp/3000/quic-v1/p2p/{source}")
                .parse()
                .unwrap();
            {
                let mut guard = state.lock().await;
                for (index, peer) in peers[..3].iter().enumerate() {
                    seed_connected_peer(&mut guard, *peer, 700 + index);
                }
                guard.track_connection(
                    ConnectionId::new_unchecked(703),
                    source,
                    &address,
                    libp2p::core::ConnectedPoint::Dialer {
                        address: address.clone(),
                        role_override: libp2p::core::Endpoint::Dialer,
                        port_use: libp2p::core::transport::PortUse::Reuse,
                    },
                );
                guard.set_configured_recovery_peers(Arc::default(), Arc::from([address.clone()]));
                if condition == "attempted" {
                    guard.track_block_height_attempted_peers(42, [source]);
                }
            }
            match condition {
                "cooled" => {
                    assert!(exclusions.record_peer_request_failure(source));
                }
                "excluded" => {
                    assert!(exclusions
                        .record_wrong_peer_id(&address, Some(source), PeerId::random(),)
                        .ip_exclusion
                        .is_some());
                }
                "all-cooled" => {
                    for peer in &peers {
                        assert!(exclusions.record_peer_request_failure(*peer));
                    }
                }
                _ => {}
            }
            let expected = if matches!(condition, "eligible" | "all-cooled") {
                source
            } else {
                peers[0]
            };
            let message = block_by_height_message(42);
            let mut actions = VecDeque::new();
            let mut dispatcher = SwarmActionDispatcher::Buffered(&mut actions);
            if alternate {
                assert!(queue_block_height_retry_to_alternate_peer_with_dispatcher(
                    &mut dispatcher,
                    &state,
                    &exclusions,
                    42,
                    message.clone(),
                )
                .await
                .unwrap());
            } else {
                handle_effect_with_dispatcher(
                    request_slab_from_message(&message).unwrap(),
                    &mut dispatcher,
                    peers,
                    false,
                    PrefetchConfig::disabled(),
                    runtime_limits_from_config(&LIBP2P_CONFIG),
                    state.clone(),
                    metrics,
                    exclusions,
                )
                .await
                .unwrap();
            }
            match actions.pop_front() {
                Some(SwarmAction::QueueKernelRequest {
                    peer_id,
                    request_message,
                }) => {
                    assert_eq!(
                        peer_id, expected,
                        "condition={condition}, alternate={alternate}"
                    );
                    assert_eq!(request_message, message);
                }
                other => panic!("expected singleton request, got {other:?}"),
            }
            assert!(actions.is_empty());
            assert!(state
                .lock()
                .await
                .get_block_height_attempted_peers(42)
                .contains(&expected));
        }
    }
}

#[tokio::test]
async fn block_height_source_preference_accepted_supplier_follows_frontier_not_gossip_ack() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let mut peers = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    peers.sort_unstable();
    let supplier = peers[2];
    let gossip_peer = peers[1];
    {
        let mut guard = state.lock().await;
        guard.first_negative = 1;
        for (index, peer) in peers.iter().enumerate() {
            seed_connected_peer(&mut guard, *peer, 800 + index);
        }
    }
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        Vec::new(),
        vec![PokeResult::Ack],
    )
    .await;
    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
    let request_id = fresh_outbound_request_id();
    state.lock().await.record_outbound_request(
        request_id,
        OutboundRequestContext::new(
            gossip_peer,
            NockchainRequest::AuthenticatedGossip {
                pow: [0; 16],
                nonce: 0,
                message: ByteBuf::from(vec![0xAB]),
            },
        ),
    );
    handle_request_response(
        gossip_peer,
        ConnectionId::new_unchecked(801),
        request_response::Message::Response {
            request_id,
            response: measured_test_response(NockchainResponse::Ack { acked: true }),
        },
        swarm_tx.clone(),
        &mut equix::EquiXBuilder::new(),
        PeerId::random(),
        traffic.traffic.clone(),
        metrics.clone(),
        state.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .unwrap();
    let (block, _) = heard_block_fact_with_tx_ids(1, &[]);
    let block_id = match &block {
        NockchainFact::HeardBlock(block_id, _) => block_id.clone(),
        other => panic!("expected heard-block fact, got {other:?}"),
    };
    route_response_fact(
        supplier, block, &traffic.traffic, &metrics, &state, &swarm_tx,
    )
    .await
    .unwrap();
    assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 1);
    assert!(swarm_rx.try_recv().is_err());
    {
        let mut guard = state.lock().await;
        guard.track_block_id_str_and_peer(block_id.clone(), supplier);
        guard.validate_and_untrack_block_sources(block_id);
    }

    // A routed block Ack and a gossip Ack are not height acceptance.
    let mut actions = VecDeque::new();
    handle_effect_with_dispatcher(
        request_slab_from_message(block_by_height_message(2).as_ref()).unwrap(),
        &mut SwarmActionDispatcher::Buffered(&mut actions),
        peers.clone(),
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        state.clone(),
        metrics.clone(),
        PeerExclusions::default(),
    )
    .await
    .unwrap();
    match actions.pop_front() {
        Some(SwarmAction::QueueKernelRequest { peer_id, .. }) => assert_eq!(peer_id, peers[0]),
        other => panic!("expected pre-acceptance singleton, got {other:?}"),
    }
    assert!(actions.is_empty());
    state.lock().await.clear_block_height_attempted_peers(2);
    handle_effect(
        seen_block_effect_slab(10_001, 1),
        swarm_tx,
        peers.clone(),
        false,
        state.clone(),
        metrics.clone(),
    )
    .await
    .unwrap();
    assert_eq!(state.lock().await.first_negative, 2);
    assert!(swarm_rx.try_recv().is_err());

    for alternate in [false, true] {
        state.lock().await.clear_block_height_attempted_peers(2);
        let mut dispatcher = SwarmActionDispatcher::Buffered(&mut actions);
        if alternate {
            assert!(queue_block_height_retry_to_alternate_peer_with_dispatcher(
                &mut dispatcher,
                &state,
                &PeerExclusions::default(),
                2,
                block_by_height_message(2),
            )
            .await
            .unwrap());
        } else {
            handle_effect_with_dispatcher(
                request_slab_from_message(block_by_height_message(2).as_ref()).unwrap(),
                &mut dispatcher,
                peers.clone(),
                false,
                PrefetchConfig::disabled(),
                runtime_limits_from_config(&LIBP2P_CONFIG),
                state.clone(),
                metrics.clone(),
                PeerExclusions::default(),
            )
            .await
            .unwrap();
        }
        match actions.pop_front() {
            Some(SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            }) => {
                assert_eq!(peer_id, supplier);
                assert_eq!(request_message, block_by_height_message(2));
            }
            other => panic!("expected accepted-source singleton, got {other:?}"),
        }
        assert!(actions.is_empty());
    }
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
    canonical_peers.sort_unstable();

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
async fn test_elders_request_targets_source_peer_and_applies_cooldown() {
    use tokio::sync::mpsc;

    let source_peer = PeerId::random();
    let peers = vec![PeerId::random(), source_peer, PeerId::random()];
    let mut effect_slab = NounSlab::new();
    let block_id = T(&mut effect_slab, &[D(101), D(102), D(103), D(104), D(105)]);
    let source_peer_atom =
        Atom::from_value(&mut effect_slab, source_peer.to_base58()).expect("peer ID should encode");
    let elders = T(&mut effect_slab, &[block_id, source_peer_atom.as_noun()]);
    let block_request = T(&mut effect_slab, &[D(tas!(b"elders")), elders]);
    let request = T(&mut effect_slab, &[D(tas!(b"block")), block_request]);
    let effect = T(&mut effect_slab, &[D(tas!(b"request")), request]);
    effect_slab.set_root(effect);
    let expected_message = ByteBuf::from(effect_slab.jam().as_ref());

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);

    handle_effect(
        effect_slab.clone(),
        swarm_tx.clone(),
        peers.clone(),
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("elders request should be accepted");

    match swarm_rx.recv().await {
        Some(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        }) => {
            assert_eq!(
                peer_id, source_peer,
                "elders must be requested from the peer that supplied the orphan block"
            );
            assert_eq!(request_message, expected_message);
        }
        other => panic!("expected QueueKernelRequest action, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "elders request should initially target only its source peer"
    );

    handle_effect(effect_slab, swarm_tx, peers, false, state_arc, metrics)
        .await
        .expect("duplicate elders request should be accepted and suppressed");
    assert!(
        swarm_rx.try_recv().is_err(),
        "duplicate elders request inside the cooldown must not be sent"
    );
}

/// Distinct block ids share a single in-flight elders slot per peer.
#[tokio::test]
async fn elders_requests_from_one_peer_are_bounded_by_its_slot() {
    use tokio::sync::mpsc;

    let source_peer = PeerId::random();
    let peers = vec![PeerId::random(), source_peer];
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(16);

    // Each effect names a different block id.
    let elders_effect_for = |seed: u64| {
        let mut slab = NounSlab::new();
        let block_id = T(&mut slab, &[D(seed), D(102), D(103), D(104), D(105)]);
        let peer_atom =
            Atom::from_value(&mut slab, source_peer.to_base58()).expect("peer ID should encode");
        let elders = T(&mut slab, &[block_id, peer_atom.as_noun()]);
        let block_request = T(&mut slab, &[D(tas!(b"elders")), elders]);
        let request = T(&mut slab, &[D(tas!(b"block")), block_request]);
        let effect = T(&mut slab, &[D(tas!(b"request")), request]);
        slab.set_root(effect);
        slab
    };

    for seed in 0..6u64 {
        handle_effect(
            elders_effect_for(seed),
            swarm_tx.clone(),
            peers.clone(),
            false,
            state_arc.clone(),
            metrics.clone(),
        )
        .await
        .expect("elders request should be accepted");
    }

    let mut queued = 0usize;
    while let Ok(action) = swarm_rx.try_recv() {
        if matches!(action, SwarmAction::QueueKernelRequest { .. }) {
            queued += 1;
        }
    }
    assert_eq!(
        queued, 1,
        "six distinct block ids from one peer must yield one outbound elders request, not six"
    );

    // The walk's next step goes out as soon as the peer's response lands.
    state_arc.lock().await.clear_elders_inflight(&source_peer);
    handle_effect(
        elders_effect_for(6),
        swarm_tx,
        peers,
        false,
        state_arc,
        metrics,
    )
    .await
    .expect("elders request after the response should be accepted");
    assert!(
        matches!(
            swarm_rx.try_recv(),
            Ok(SwarmAction::QueueKernelRequest { .. })
        ),
        "releasing the slot on the response must not delay the sequential recovery walk"
    );
}

/// An elders request needs a key before it can claim a scheduling slot.
#[tokio::test]
async fn elders_request_with_unkeyable_block_id_is_dropped_not_sent_ungated() {
    use tokio::sync::mpsc;

    let source_peer = PeerId::random();
    let peers = vec![PeerId::random(), source_peer];
    let mut slab = NounSlab::new();
    // Four limbs: not a Tip5 digest, so no slot key can be derived from it.
    let block_id = T(&mut slab, &[D(101), D(102), D(103), D(104)]);
    let peer_atom =
        Atom::from_value(&mut slab, source_peer.to_base58()).expect("peer ID should encode");
    let elders = T(&mut slab, &[block_id, peer_atom.as_noun()]);
    let block_request = T(&mut slab, &[D(tas!(b"elders")), elders]);
    let request = T(&mut slab, &[D(tas!(b"block")), block_request]);
    let effect = T(&mut slab, &[D(tas!(b"request")), request]);
    slab.set_root(effect);

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);

    handle_effect(
        slab,
        swarm_tx,
        peers,
        false,
        state_arc.clone(),
        metrics.clone(),
    )
    .await
    .expect("dropping an unkeyable elders request is not an error");
    assert!(
        swarm_rx.try_recv().is_err(),
        "an elders request that cannot claim a slot must not be sent at all"
    );
    assert_eq!(metrics.elders_requests_suppressed.fetch_add(0), 1);
    assert_eq!(state_arc.lock().await.elders_inflight_len(), 0);
}

#[tokio::test]
async fn test_elders_request_rejects_non_atom_source_peer() {
    use tokio::sync::mpsc;

    let peers = vec![PeerId::random(), PeerId::random()];
    let mut effect_slab = NounSlab::new();
    let block_id = T(&mut effect_slab, &[D(101), D(102), D(103), D(104), D(105)]);
    let invalid_peer = T(&mut effect_slab, &[D(1), D(2)]);
    let elders = T(&mut effect_slab, &[block_id, invalid_peer]);
    let block_request = T(&mut effect_slab, &[D(tas!(b"elders")), elders]);
    let request = T(&mut effect_slab, &[D(tas!(b"block")), block_request]);
    let effect = T(&mut effect_slab, &[D(tas!(b"request")), request]);
    effect_slab.set_root(effect);

    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);

    let result = handle_effect(effect_slab, swarm_tx, peers, false, state_arc, metrics).await;
    assert!(
        result.is_err(),
        "malformed elders source peer should reject the request effect"
    );
    assert!(
        swarm_rx.try_recv().is_err(),
        "malformed elders request must not fall back to connected peers"
    );
}

#[tokio::test]
async fn test_request_effect_skips_bundle_upgrade_for_non_bundle_capable_peer() {
    use tokio::sync::mpsc;

    let classic_message = ByteBuf::from(jam_block_by_height_request(91));
    let effect_slab =
        request_slab_from_message(&classic_message).expect("request slab should decode");
    let mut peers = vec![PeerId::random(), PeerId::random()];
    peers.sort_unstable();
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
    let context = OutboundRequestContext::new(PeerId::random(), request);

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

    let actual = BatchResultItem {
        item_id: 1,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(ResponseEnvelope::heard_block_with_txs(
            "1".repeat(TIP5_BASE58_MAX_CHARS),
            vec![0; estimate.message_bytes],
            Vec::new(),
            Vec::new(),
        )),
    };
    assert_eq!(
        estimated_single_result_bytes(1, &estimate).expect("estimated size"),
        batch_result_encoded_bytes(&[actual]).expect("wire size"),
    );
}

#[tokio::test]
async fn test_estimated_enormous_body_stops_without_execution() {
    let items = vec![BatchRequestItem {
        item_id: u32::MAX,
        message: crate::messages::block_by_height_message(1),
    }];
    for message_bytes in [usize::MAX / 2, usize::MAX] {
        let result = execute_batch_request_items(
            &items,
            1024,
            move |_| async move {
                Ok(Some(BatchItemResponseEstimate {
                    request_kind: "block-by-height",
                    envelope: BatchItemResponseEnvelopeEstimate::HeardBlock {
                        block_id_bytes_upper_bound: TIP5_BASE58_MAX_CHARS,
                    },
                    message_bytes,
                    source: "test",
                }))
            },
            |_| async { panic!("oversize projection must not execute a body lookup") },
        )
        .await;
        if message_bytes == usize::MAX {
            assert!(result.is_err(), "overflow must fail before execution");
        } else {
            assert_eq!(
                result.expect("enormous representable projection"),
                vec![batch_error_result(u32::MAX, BatchErrorClass::TooLarge)],
            );
        }
    }
}

#[tokio::test]
async fn test_cold_block_batch_all_64_not_found_keeps_hints_cold() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    // Requested historical heights must reach the scripted kernel, not the
    // known-negative frontier cache.
    state.lock().await.first_negative = 80_000;
    let scripted = build_scripted_traffic_cop(
        DriverTranscript::default(),
        (0..64).map(|_| None).collect(),
        Vec::new(),
    )
    .await;
    let limits = runtime_limits_from_config(&LIBP2P_CONFIG);
    let items: Vec<_> = (0..64)
        .map(|index| BatchRequestItem {
            item_id: index + 1,
            message: crate::messages::block_by_height_message(u64::from(index) + 100),
        })
        .collect();
    let mut before = Vec::new();
    for item in &items {
        let estimate = estimate_batch_request_item_response(item, limits, &state)
            .await
            .expect("estimate")
            .expect("valid request");
        before.push((estimate.message_bytes, estimate.source));
    }
    let peer = PeerId::random();
    let results = execute_batch_request_items(
        &items,
        limits.gen2_batch_max_bytes,
        |item| {
            let item = item.clone();
            let state = Arc::clone(&state);
            async move { estimate_batch_request_item_response(&item, limits, &state).await }
        },
        |item| {
            let item = item.clone();
            let state = Arc::clone(&state);
            let metrics = Arc::clone(&metrics);
            let traffic = &scripted.traffic;
            async move {
                execute_batch_request_item(peer, &item, limits, traffic, &metrics, &state).await
            }
        },
    )
    .await
    .expect("cold batch");
    assert_eq!(scripted.peek_count.load(Ordering::SeqCst), 64);
    assert_eq!(results.len(), 64);
    for ((item, result), expected_hint) in items.iter().zip(&results).zip(before) {
        assert_eq!(result.item_id, item.item_id);
        assert_eq!(result.status, BatchResultStatus::NotFound);
        assert!(result.error.is_none());
        assert!(result.envelope.is_none());
        let estimate = estimate_batch_request_item_response(item, limits, &state)
            .await
            .expect("estimate")
            .expect("valid request");
        assert_eq!((estimate.message_bytes, estimate.source), expected_hint);
    }
    assert!(
        batch_result_encoded_bytes(&results).expect("wire size") <= limits.gen2_batch_max_bytes
    );
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
async fn test_block_request_effects_rotate_equal_priority_peers_across_heights() {
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

    let mut selected = Vec::new();
    for expected_message in [first_message, second_message] {
        match swarm_rx.recv().await {
            Some(SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            }) => {
                assert!(peers.contains(&peer_id));
                selected.push(peer_id);
                assert_eq!(request_message, expected_message);
            }
            other => panic!("expected QueueKernelRequest action, got {:?}", other),
        }
    }
    assert_ne!(
        selected[0], selected[1],
        "independent heights must rotate peers"
    );
    assert!(
        swarm_rx.try_recv().is_err(),
        "each block demand should queue only one peer"
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
    canonical_peers.sort_unstable();

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
            assert!(canonical_peers[1..].contains(&peer_id));
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
async fn heard_block_gossip_skips_all_recorded_source_peers() {
    use tokio::sync::mpsc;

    let (fact, _) = heard_block_fact_with_block_seed(42, 91_001, &[]);
    let NockchainFact::HeardBlock(block_id, _) = &fact else {
        panic!("expected heard-block fact");
    };
    let message = result_message_from_fact(&fact);
    let payload_slab = request_slab_from_message(&message).expect("valid heard-block payload");
    let mut effect_slab = NounSlab::new();
    let payload =
        effect_slab.copy_into(unsafe { *payload_slab.root() }, &payload_slab.noun_space());
    let context = gossip_test_context(&mut effect_slab, "chain", "new-head", None);
    let effect = T(
        &mut effect_slab,
        &[D(tas!(b"gossip")), context, D(FACT_POKE_VERSION), payload],
    );
    effect_slab.set_root(effect);

    let source = PeerId::random();
    let second_source = PeerId::random();
    let other = PeerId::random();
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state = state_arc.lock().await;
        state.record_block_echo_source(source, block_id);
        state.record_block_echo_source(second_source, block_id);
        state.record_block_echo_source(source, block_id);
    }
    let (swarm_tx, mut swarm_rx) = mpsc::channel(4);
    handle_effect(
        effect_slab,
        swarm_tx,
        vec![source, second_source, other],
        false,
        state_arc,
        metrics,
    )
    .await
    .expect("valid gossip effect");

    match swarm_rx.recv().await {
        Some(SwarmAction::SendGossip {
            peer_id,
            message: sent,
            ..
        }) => {
            assert_eq!(peer_id, other);
            assert_eq!(sent, message);
        }
        other => panic!("expected gossip to the other peer, got {other:?}"),
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "neither source must receive an echo"
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
        state_guard.cache_block_response(42, NounSlab::new());
        state_guard.cache_elders_response(String::from("elders"), NounSlab::new());
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
            Some(SwarmAction::SendGossip {
                peer_id, message, ..
            }) => {
                assert_eq!(peer_id, expected_peer, "gossip should preserve peer order");
                assert_eq!(message, expected_message);
            }
            other => panic!("expected gossip SendGossip action, got {:?}", other),
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
async fn unvalidated_future_block_does_not_suppress_outbound_gossip() {
    use tokio::sync::mpsc;

    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let peers = vec![peer_a, peer_b];
    let source_peer = PeerId::random();
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state_arc.lock().await;
        state_guard.first_negative = 10;
    }

    let transcript = DriverTranscript::default();
    let scripted_traffic = build_scripted_traffic_cop(transcript, Vec::new(), Vec::new()).await;
    let (future_block, _) = heard_block_fact_with_block_seed(u64::MAX, 900_000, &[]);
    let NockchainFact::HeardBlock(future_block_id, _) = &future_block else {
        panic!("expected heard-block fact");
    };
    let future_block_id = future_block_id.clone();
    let (defer_swarm_tx, _defer_swarm_rx) = mpsc::channel(1);
    route_response_fact(
        source_peer, future_block, &scripted_traffic.traffic, &metrics, &state_arc, &defer_swarm_tx,
    )
    .await
    .expect("future heard-block should defer without kernel validation");
    assert_eq!(
        scripted_traffic.poke_count.load(Ordering::SeqCst),
        0,
        "future heard-block must not reach the kernel before validation"
    );
    assert!(
        state_arc
            .lock()
            .await
            .has_deferred_block_at_height(u64::MAX),
        "source-provided future block should be deferred"
    );
    assert!(
        state_arc
            .lock()
            .await
            .block_echo_sources(&future_block_id)
            .is_some_and(|sources| sources.contains(&source_peer)),
        "response routing should retain the source before the kernel poke"
    );

    let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
    let mut expected_messages = Vec::new();
    for (tag, seed) in [("heard-block", 10), ("heard-tx", 20)] {
        let (effect_slab, expected_message) =
            build_gossip_effect_with_tag(FACT_POKE_VERSION, tag, &[seed]);
        expected_messages.push(expected_message.clone());
        handle_effect(
            effect_slab,
            swarm_tx.clone(),
            peers.clone(),
            false,
            state_arc.clone(),
            metrics.clone(),
        )
        .await
        .expect("untrusted future block must not suppress outbound gossip");
    }

    let expected = [
        (peer_a, expected_messages[0].clone()),
        (peer_b, expected_messages[0].clone()),
        (peer_a, expected_messages[1].clone()),
        (peer_b, expected_messages[1].clone()),
    ];
    for (expected_peer, expected_message) in expected {
        let action = tokio::time::timeout(Duration::from_millis(100), swarm_rx.recv())
            .await
            .expect("kernel gossip should fan out despite unvalidated deferred height");
        match action {
            Some(SwarmAction::SendGossip {
                peer_id, message, ..
            }) => {
                assert_eq!(peer_id, expected_peer, "gossip should preserve peer order");
                assert_eq!(message, expected_message);
            }
            other => panic!("expected gossip SendGossip action, got {other:?}"),
        }
    }
    assert!(
        swarm_rx.try_recv().is_err(),
        "should only send one gossip per peer per effect"
    );
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
            Some(SwarmAction::SendGossip {
                peer_id, message, ..
            }) => {
                assert_eq!(peer_id, expected_peer, "gossip should preserve peer order");
                assert_eq!(message, expected_message);
            }
            other => panic!(
                "expected buffered gossip SendGossip action, got {:?}",
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
        state_guard.cache_block_response(7, NounSlab::new());
        state_guard.cache_elders_response(String::from("elders"), NounSlab::new());
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
async fn failed_pow_witness_liar_peer_creates_address_exclusion() {
    let peer = PeerId::random();
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry()).expect("register metrics"),
    );
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    {
        let mut state_guard = state.lock().await;
        seed_connected_peer(&mut state_guard, peer, 904);
    }

    let mut effect_slab = NounSlab::new();
    let effect_tag = make_tas(&mut effect_slab, "liar-peer");
    let peer_id =
        Atom::from_value(&mut effect_slab, peer.to_base58()).expect("failed witness peer ID atom");
    let effect_cause = make_tas(&mut effect_slab, "failed-pow-witness");
    let effect = T(
        &mut effect_slab,
        &[effect_tag.as_noun(), peer_id.as_noun(), effect_cause.as_noun()],
    );
    effect_slab.set_root(effect);

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(10);
    let mut swarm_actions = SwarmActionDispatcher::Channel(&swarm_tx);
    handle_effect_with_dispatcher(
        effect_slab,
        &mut swarm_actions,
        Vec::new(),
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        state.clone(),
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("handle failed v5 witness liar effect");

    let mut blocked = false;
    let mut excluded = false;
    while let Ok(action) = swarm_rx.try_recv() {
        match action {
            SwarmAction::BlockPeer { peer_id } => blocked |= peer_id == peer,
            SwarmAction::RecordExclusionOutcome { outcome, .. } => {
                excluded |= outcome.address_cooldown.is_some();
            }
            other => panic!("unexpected swarm action: {other:?}"),
        }
    }
    assert!(blocked, "the invalid witness sender must be peer-blocked");
    assert!(
        excluded,
        "an invalid v5 witness must create an address exclusion"
    );
    assert!(
        state.lock().await.rejected_pow_blocks.is_empty(),
        "a witness-specific failure must not poison the semantic block ID"
    );
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
        // A non-cryptographic liar reason must remain peer-scoped even when the
        // runtime has a trusted connection address.
        seed_connected_peer(&mut state_guard, bad_peer1, 901);
        seed_connected_peer(&mut state_guard, bad_peer2, 902);

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
async fn failed_pow_liar_block_id_creates_address_exclusion() {
    let peer = PeerId::random();
    let metrics = Arc::new(
        NockchainP2PMetrics::register(gnort::global_metrics_registry()).expect("register metrics"),
    );
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));

    let mut block_slab: NounSlab = NounSlab::new();
    let block_id = T(&mut block_slab, &[D(11), D(12), D(13), D(14), D(15)]);
    let block_space = block_slab.noun_space();
    {
        let mut state_guard = state.lock().await;
        seed_connected_peer(&mut state_guard, peer, 903);
        state_guard
            .track_block_id_and_peer(block_id, peer, &block_space)
            .expect("track failed-PoW block source");
    }

    let mut effect_slab: NounSlab = NounSlab::new();
    let effect_tag = make_tas(&mut effect_slab, "liar-block-id");
    let effect_block_id = T(&mut effect_slab, &[D(11), D(12), D(13), D(14), D(15)]);
    let effect_cause = make_tas(&mut effect_slab, "failed-pow-check");
    let effect = T(
        &mut effect_slab,
        &[effect_tag.as_noun(), effect_block_id, effect_cause.as_noun()],
    );
    effect_slab.set_root(effect);

    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(10);
    let mut swarm_actions = SwarmActionDispatcher::Channel(&swarm_tx);
    handle_effect_with_dispatcher(
        effect_slab,
        &mut swarm_actions,
        Vec::new(),
        false,
        PrefetchConfig::disabled(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        state,
        metrics,
        PeerExclusions::default(),
    )
    .await
    .expect("handle failed-PoW liar effect");

    let mut blocked = false;
    let mut excluded = false;
    while let Ok(action) = swarm_rx.try_recv() {
        match action {
            SwarmAction::BlockPeer { peer_id } => blocked |= peer_id == peer,
            SwarmAction::RecordExclusionOutcome { outcome, .. } => {
                excluded |= outcome.address_cooldown.is_some();
            }
            other => panic!("unexpected swarm action: {other:?}"),
        }
    }
    assert!(blocked, "the proof sender must be peer-blocked");
    assert!(
        excluded,
        "a cryptographically invalid proof must create an address exclusion"
    );
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

fn cache_test_block_scry(height: u64, payload_len: usize) -> NounSlab {
    let mut slab = NounSlab::new();
    let block_id = tip5_tuple(&mut slab, 10_000 + height);
    let payload = Atom::from_value(&mut slab, vec![0xCDu8; payload_len])
        .expect("payload atom should build")
        .as_noun();
    let page = T(
        &mut slab,
        &[
            D(1),
            block_id,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            payload,
        ],
    );
    let root = T(&mut slab, &[D(0), D(0), page]);
    slab.set_root(root);
    slab
}

async fn execute_cache_test_request(
    request: NockchainDataRequest,
    response: NounSlab,
    metrics: &Arc<NockchainP2PMetrics>,
    state: &Arc<Mutex<P2PState>>,
) {
    let scripted = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![Some(response)],
        Vec::new(),
    )
    .await;
    let outcome = execute_request_item(
        PeerId::random(),
        request.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &scripted.traffic,
        metrics,
        state,
    )
    .await
    .expect("ordinary response should be serviced");
    assert!(
        matches!(outcome, RequestExecutionOutcome::Result { .. }),
        "expected successful response for {request:?}, got {outcome:?}"
    );
    assert_eq!(scripted.peek_count.load(Ordering::SeqCst), 1);
    state.lock().await.assert_response_cache_bounds();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_ordinary_caches_share_combined_entry_cap() {
    for block_request in [true, false] {
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        {
            let mut guard = state.lock().await;
            guard.first_negative = 80_000;
            for index in 0..4_096 {
                let mut slab = NounSlab::new();
                slab.set_root(D(0));
                match index % 3 {
                    0 => guard.cache_block_response(index, slab),
                    1 => guard.cache_tx_response(format!("prefill-{index}"), slab),
                    _ => guard.cache_elders_response(format!("prefill-{index}"), slab),
                }
            }
            assert_eq!(
                guard.block_cache.len() + guard.tx_cache.len() + guard.elders_cache.len(),
                4_096
            );
            guard.assert_response_cache_bounds();
        }
        let request = if block_request {
            NockchainDataRequest::BlockByHeight(50_000)
        } else {
            decode_request_item_message(&jam_raw_tx_request(50_000)).expect("valid tx request")
        };
        let response = if block_request {
            cache_test_block_scry(50_000, 64)
        } else {
            scry_some_raw_tx(50_000, 64)
        };
        execute_cache_test_request(request, response, &metrics, &state).await;
        let (bundle, _, _) = scry_some_heavy_txs(50_000, &[50_002], 64);
        execute_cache_test_request(
            NockchainDataRequest::BlockWithTxsByHeight(50_000),
            bundle,
            &metrics,
            &state,
        )
        .await;
        state.lock().await.clear_block_and_elders_responses();
        state.lock().await.assert_response_cache_bounds();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_ordinary_caches_enforce_slab_byte_budget() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    state.lock().await.first_negative = 80_000;
    let mut supplied_bytes = 0;
    for index in 0..8 {
        let height = 60_000 + index;
        let (request, response) = if index % 2 == 0 {
            (
                NockchainDataRequest::BlockByHeight(height),
                cache_test_block_scry(height, 8 * 1024 * 1024),
            )
        } else {
            (
                decode_request_item_message(&jam_raw_tx_request(height)).expect("valid tx request"),
                scry_some_raw_tx(height, 8 * 1024 * 1024),
            )
        };
        supplied_bytes += response.allocated_bytes();
        execute_cache_test_request(request, response, &metrics, &state).await;
    }
    assert!(supplied_bytes > 100 * 1024 * 1024);
    let (bundle, _, _) = scry_some_heavy_txs(60_006, &[60_101], 64);
    execute_cache_test_request(
        NockchainDataRequest::BlockWithTxsByHeight(60_006),
        bundle,
        &metrics,
        &state,
    )
    .await;
    state.lock().await.clear_block_and_elders_responses();
    state.lock().await.assert_response_cache_bounds();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_request_item_oversized_slabs_are_serviced_without_retention() {
    for block_request in [true, false] {
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
        )));
        state.lock().await.first_negative = 80_000;
        let request = if block_request {
            NockchainDataRequest::BlockByHeight(70_000)
        } else {
            decode_request_item_message(&jam_raw_tx_request(70_000)).expect("valid tx request")
        };
        let response = if block_request {
            cache_test_block_scry(70_000, 101 * 1024 * 1024)
        } else {
            scry_some_raw_tx(70_000, 101 * 1024 * 1024)
        };
        assert!(response.allocated_bytes() > 100 * 1024 * 1024);
        execute_cache_test_request(request, response, &metrics, &state).await;
        let guard = state.lock().await;
        assert!(guard.block_cache.is_empty());
        assert!(guard.tx_cache.is_empty());
        assert!(guard.elders_cache.is_empty());
    }
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
async fn route_bundle_envelope_unpacks_block_txs_and_records_unincluded_hints() {
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

    {
        let state_guard = driver_state.lock().await;
        for tx_id in &unincluded_tx_ids {
            assert_eq!(
                state_guard.get_peers_for_tx_id(tx_id),
                vec![peer],
                "unincluded bundle remainder should record tx source hints"
            );
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(25), swarm_rx.recv())
            .await
            .is_err(),
        "bundle unpack must not issue raw-tx prefetches"
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
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 0,
                items: vec![BatchRequestItem {
                    item_id: 0,
                    message: ByteBuf::from(request_message.clone()),
                }],
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

    let retired = state_arc
        .lock()
        .await
        .remove_outbound_request(request_id)
        .expect("terminal retirement should remove the retained request");
    assert_eq!(retired.peer_id, peer_id);
    assert_eq!(state_arc.lock().await.total_outbound_request_count(), 0);

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
fn test_take_pending_batch_request_keeps_single_block_on_gen2() {
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

    let NockchainRequest::BatchRequest { items, .. } = flushed.request else {
        panic!("singleton block flush should stay a gen2 BatchRequest");
    };
    assert_eq!(items.len(), 1);
    assert!(matches!(
        decode_request_item_message(&items[0].message),
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
fn test_request_message_batch_limits() {
    let request_message = jam_raw_tx_request(11);
    assert!(request_message_can_join_batch(&request_message, 128, 256));
    assert!(!request_message_can_join_batch(&request_message, 8, 256));
    assert!(!request_message_can_join_batch(&request_message, 128, 8));
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
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 0,
                message: ByteBuf::from(jam_block_by_height_request(1)),
            }],
        },
        GEN2_RETRY_MAX_ATTEMPTS,
    );
    let filtered = OutboundRequestContext::with_attempt(
        remote_peer_id,
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
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 0,
                message: block_range_with_txs_request_message(100, 8)
                    .expect("range request should encode"),
            }],
        },
        0,
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
fn test_log_outbound_failure_tracks_gen2_timeout_metrics() {
    let metrics = isolated_test_metrics();
    let peer_id = PeerId::random();
    let request_context = OutboundRequestContext::with_attempt(
        peer_id,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(jam_block_by_height_request(7)),
            }],
        },
        1,
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

#[tokio::test]
async fn test_queue_retry_requests_tracks_scheduled_total() {
    let metrics = isolated_test_metrics();
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(1);
    let peer_id = PeerId::random();
    let requests = vec![OutboundRequestContext::with_attempt(
        peer_id,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 0,
                message: ByteBuf::from(jam_block_by_height_request(1)),
            }],
        },
        1,
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
        .canonical_ip_addr()
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
        Some(expected),
        &[],
        &[],
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
        .canonical_ip_addr()
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
        Some(expected),
        std::slice::from_ref(&stale_base_addr),
        &[],
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
    assert_eq!(metrics.address_cooldowns_created.fetch_add(0), 1);
    assert!(matches!(
        swarm.dial(stale_base_addr.with_p2p(expected).unwrap()),
        Err(DialError::Denied { .. }),
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_outbound_gen2_batch_send_updates_send_counters() {
    let transcript = DriverTranscript::default();
    transcript.record(
        "scenario", "driver-path outbound gen2 batch send increments transport counters",
    );

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
        OutboundRequestContext::new(responder_peer_id, request),
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
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
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
        eprintln!("skipping checkpoint sizing report: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: items.clone(),
                },
                0,
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
                    response: measured_test_response(response),
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: items.clone(),
                },
                0,
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
                    response: measured_test_response(response),
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
            "requester-cost checkpoint block sample skipped: set REQ_RES_GEN2_CHECKPOINT_PATH"
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
        "{:<24} {:<22} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "workload",
        "request_mix",
        "stages",
        "input",
        "unique",
        "dup",
        "out",
        "gen2",
        "1item",
        "p50",
        "avg"
    );
    for sample in &enqueue_samples {
        println!(
            "{:<24} {:<22} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8.2}",
            sample.label,
            sample.request_mix,
            sample.stage_count,
            sample.input_request_count,
            sample.unique_request_count,
            sample.duplicate_requests,
            sample.outbound_request_count,
            sample.gen2_batch_request_count,
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
        println!("checkpoint requester cost report skipped: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
        println!("checkpoint prefetch cost report skipped: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
        println!("checkpoint requester profile skipped: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
        println!("checkpoint range scry latency report skipped: set REQ_RES_GEN2_CHECKPOINT_PATH");
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
#[ignore = "performance harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_resident_set_report() {
    let workloads = [
        ("batch-32-large", 32usize, 2048usize),
        ("batch-128-large", 128usize, 2048usize),
        ("batch-128-near-cap", 128usize, 6144usize),
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

    for (label, item_count, payload_len) in workloads {
        let transcript = DriverTranscript::default();
        transcript.record(
            "scenario",
            format!("resident-set report {label} items={item_count} payload_len={payload_len}"),
        );
        let (response_bytes, elapsed, poke_count, rss_before_kib, rss_after_kib, rss_peak_kib) =
            run_requester_response_workload(item_count, payload_len, &transcript).await;
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_us = elapsed.as_micros() as f64 / item_count as f64;
        let rss_delta_kib = rss_peak_kib.saturating_sub(rss_before_kib);

        println!(
            "{:<26} {:<8} {:>8} {:>10} {:>14} {:>12.3} {:>12.3} {:>10} {:>10} {:>10} {:>10}",
            label,
            "gen2",
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
            generation: String::from("gen2"),
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
#[ignore = "performance harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_two_peer_latency_report() {
    let workloads = [
        ("batch-32", 32usize, 512usize),
        ("batch-128-large", 128usize, 2048usize),
        ("batch-128-near-cap", 128usize, 6144usize),
        ("batch-4", 4usize, 512usize),
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

    for (label, item_count, payload_len) in workloads {
        let transcript = DriverTranscript::default();
        transcript.record(
            "scenario",
            format!("two-peer latency report {label} items={item_count} payload_len={payload_len}"),
        );
        let (response_bytes, elapsed, protocol, peek_count) =
            run_two_peer_driver_latency_workload(item_count, payload_len, &transcript).await;
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_ms = total_ms / item_count as f64;

        println!(
            "{:<24} {:<8} {:>8} {:>12} {:>14} {:>12.3} {:>12.3} {:<24}",
            label, "gen2", item_count, payload_len, response_bytes, total_ms, per_item_ms, protocol
        );

        samples.push(TwoPeerLatencySample {
            label: label.to_string(),
            topology: String::from("two-peer driver-path"),
            generation: String::from("gen2"),
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

    let rtts_ms = [10.0, 50.0, 100.0, 200.0];
    println!();
    println!("RTT projection (estimated wall-clock at real-world latencies)");
    println!(
        "{:<24} {:<8} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "workload", "gen", "items", "rtt_10ms", "rtt_50ms", "rtt_100ms", "rtt_200ms"
    );
    for sample in &samples {
        let projected: Vec<String> = rtts_ms
            .iter()
            .map(|rtt| format!("{:>10.0}ms", sample.total_ms + rtt))
            .collect();
        println!(
            "{:<24} {:<8} {:>8} {}",
            sample.label,
            sample.generation,
            sample.item_count,
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
                    .send_response(
                        channel,
                        crate::behaviour::TransportResponse::outgoing(response),
                    )
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
                        envelope: first_envelope,
                    }),
                    2 => BatchItemExecutionOutcome::Completed(RequestExecutionOutcome::Result {
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

    let projected_too_large = limit
        - req_res_message_encoded_bytes(&batch_error_result(2, BatchErrorClass::Backpressure))
            .expect("tail size")
        + estimated_result_item_bytes(2, &estimated_tail).expect("estimated tail size");
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
    let projected_single = estimated_single_result_bytes(1, &estimate)
        .expect("projected single item should fit usize");
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
    let baseline = batch_result_encoded_bytes(&[
        batch_error_result(1, BatchErrorClass::Backpressure),
        batch_error_result(2, BatchErrorClass::Backpressure),
        batch_error_result(3, BatchErrorClass::Backpressure),
    ])
    .expect("baseline size");
    let first_projection = baseline
        - req_res_message_encoded_bytes(&batch_error_result(1, BatchErrorClass::Backpressure))
            .expect("reserved size")
        + estimated_result_item_bytes(1, &estimate).expect("first estimate size");
    let second_projection = first_projection
        - req_res_message_encoded_bytes(&batch_error_result(2, BatchErrorClass::Backpressure))
            .expect("reserved size")
        + req_res_message_encoded_bytes(&expected_head).expect("head size")
        - estimated_result_item_bytes(1, &estimate).expect("first estimate size")
        + estimated_result_item_bytes(2, &estimate).expect("second estimate size");
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
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
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
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
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
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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
                .send_response(
                    channel,
                    crate::behaviour::TransportResponse::outgoing(response.clone()),
                )
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
        request_response_timeout_secs: 1,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
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

    let mut malformed_slab: NounSlab = NounSlab::new();
    malformed_slab.set_root(D(0));
    let malformed_request = NockchainRequest::new_batch_request(
        &mut equix::EquiXBuilder::new(),
        &requester_peer_id,
        &responder_peer_id,
        vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(malformed_slab.jam().as_ref().to_vec()),
        }],
    )
    .expect("malformed item should still fit an authenticated batch");
    requester
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, malformed_request);

    let (peer, connection_id, message) =
        recv_request_event(&mut requester, &mut responder, &transcript).await;
    assert_eq!(peer, requester_peer_id);

    run_driver_with_timeout(
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
    .await
    .expect("malformed batch item should produce an item-level error response");

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

    match recv_swarm_action(&mut swarm_rx).await {
        SwarmAction::SendResponse {
            response: NockchainResponse::BatchResult { results },
            ..
        } => {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].item_id, 1);
            assert_eq!(results[0].status, BatchResultStatus::Error);
            assert_eq!(results[0].error, Some(BatchErrorClass::Decode));
        }
        other => panic!("expected batch decode-error response, got {other:?}"),
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
                response: measured_test_response(response),
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

async fn connected_fallback_test_peer(
    state: &Arc<Mutex<P2PState>>,
) -> (
    Swarm<NockchainBehaviour>,
    ReqResTestSwarm,
    PeerId,
    ConnectionId,
) {
    let mut client = start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .unwrap();
    let mut server = build_req_res_test_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
    )
    .unwrap();
    let peer = *server.local_peer_id();
    let connection_id = tokio::time::timeout(Duration::from_secs(20), async {
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await {
                break address;
            }
        };
        client.dial(address).unwrap();
        loop {
            tokio::select! {
                event = client.select_next_some() => match event {
                    SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                        assert_eq!(peer_id, peer);
                        state.lock().await.track_connection(
                            connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                        break connection_id;
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => panic!("fixture dial failed: {error}"),
                    _ => {}
                },
                _ = server.select_next_some() => {}
            }
        }
    }).await.expect("fallback fixture must establish a real SDK connection");
    (client, server, peer, connection_id)
}

async fn fallback_test_wire_response(
    client: &mut Swarm<NockchainBehaviour>,
    server: &mut ReqResTestSwarm,
    state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    context: OutboundRequestContext,
    response: NockchainResponse,
    expected_connection: ConnectionId,
) -> (
    request_response::OutboundRequestId,
    request_response::Message<NockchainRequest, crate::behaviour::TransportResponse>,
) {
    send_outbound_request_now(client, state, metrics, context).await;
    let mut response = Some(response);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            tokio::select! {
                event = client.select_next_some() => match event {
                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(request_response::Event::Message {
                        peer, connection_id, message,
                    })) => {
                        if let request_response::Message::Response { request_id, response } = &message {
                            assert_eq!(connection_id, expected_connection);
                            let bytes = response.received_bytes.expect("codec measurement");
                            let receipt_endpoint = client.behaviour().recovery.connection_endpoint(&peer, connection_id);
                            state.lock().await.charge_received_response(*request_id, peer, connection_id, receipt_endpoint, bytes).unwrap();
                            return (*request_id, message);
                        }
                    }
                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                        request_response::Event::OutboundFailure { error, .. }
                    )) => panic!("fixture exchange failed: {error}"),
                    _ => {}
                },
                event = server.select_next_some() => {
                    if let SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message {
                        message: request_response::Message::Request { channel, .. }, ..
                    })) = event {
                        server.behaviour_mut().request_response.send_response(
                            channel, crate::behaviour::TransportResponse::outgoing(response.take().expect("fixture serves exactly one request"))).unwrap();
                    }
                }
            }
        }
    }).await.expect("fallback fixture must receive a real decoded SDK response")
}

async fn close_fallback_test_peer(
    client: &mut Swarm<NockchainBehaviour>,
    server: &mut ReqResTestSwarm,
    state: &Arc<Mutex<P2PState>>,
    connection_id: ConnectionId,
) {
    assert!(client.close_connection(connection_id));
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            tokio::select! {
                event = client.select_next_some() => {
                    if let SwarmEvent::ConnectionClosed { connection_id: closed, .. } = event {
                        assert_eq!(closed, connection_id);
                        state.lock().await.lost_connection(closed);
                        break;
                    }
                },
                _ = server.select_next_some() => {}
            }
        }
    })
    .await
    .expect("fallback fixture must retire its actual SDK connection");
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
    let (mut client, mut server, peer, connection_id) =
        connected_fallback_test_peer(&state_arc).await;
    let local_peer = *client.local_peer_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
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
    );

    let response = NockchainResponse::BatchResult {
        results: vec![batch_error_result(7, BatchErrorClass::TooLarge)],
    };
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    let mut equix_builder = equix::EquiXBuilder::new();

    let (request_id, response_message) = fallback_test_wire_response(
        &mut client, &mut server, &state_arc, &metrics, request_context, response, connection_id,
    )
    .await;

    run_driver_with_timeout(
        &transcript,
        "driver should downgrade oversized bundle response",
        handle_request_response(
            peer,
            connection_id,
            response_message,
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
    assert!(client.is_connected(&peer));
    drop(state_guard);
    close_fallback_test_peer(&mut client, &mut server, &state_arc, connection_id).await;
    assert!(
        !state_arc.lock().await.is_peer_non_bundle_capable(&peer),
        "capability downgrade is connection-session metadata, not a permanent identity blacklist"
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
    let (mut client, mut server, peer, connection_id) =
        connected_fallback_test_peer(&state_arc).await;
    let local_peer = *client.local_peer_id();
    let request_context = OutboundRequestContext::with_attempt(
        peer,
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
    );

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

    let (request_id, response_message) = fallback_test_wire_response(
        &mut client, &mut server, &state_arc, &metrics, request_context, response, connection_id,
    )
    .await;

    run_driver_with_timeout(
        &transcript,
        "driver should downgrade invalid range response",
        handle_request_response(
            peer,
            connection_id,
            response_message,
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

    let actual_endpoint = state_arc
        .lock()
        .await
        .connection_address(connection_id)
        .and_then(|address| quic_socket_addr(&address))
        .expect("the live response stream must have a numeric QUIC endpoint");
    let mut fallback_count = 0;
    let mut exclusion_count = 0;
    let mut retry_request_count = 0;
    while let Ok(action) = swarm_rx.try_recv() {
        match action {
            SwarmAction::QueueKernelRequest {
                peer_id,
                request_message,
            } => {
                assert_eq!(peer_id, peer);
                let decoded = crate::messages::decode_request_item_message(&request_message)
                    .expect("classic fallback request should decode");
                assert!(
                    matches!(decoded, NockchainDataRequest::BlockByHeight(50)),
                    "range fallback must request its missing start height"
                );
                fallback_count += 1;
            }
            SwarmAction::RecordExclusionOutcome {
                outcome,
                related_peers,
            } => {
                assert_eq!(related_peers, vec![peer]);
                let cooldown = outcome
                    .address_cooldown
                    .expect("a validated response mismatch records endpoint-scoped evidence");
                assert_eq!(quic_socket_addr(&cooldown.address), Some(actual_endpoint));
                assert_eq!(cooldown.key.expected_peer, Some(peer));
                assert_eq!(
                    cooldown.reason,
                    crate::ip_block::ExclusionReason::PeerMisbehavior
                );
                assert!(
                    outcome.ip_exclusion.is_none(),
                    "one invalid response must not exclude sibling endpoints on its IP"
                );
                exclusion_count += 1;
            }
            SwarmAction::RetryRequests { requests, .. } => retry_request_count += requests.len(),
            SwarmAction::SendRequest { .. } => retry_request_count += 1,
            other => panic!("unexpected response-consumer action: {other:?}"),
        }
    }
    assert_eq!(
        fallback_count, 1,
        "exactly one classic start-height fallback is required"
    );
    assert_eq!(
        exclusion_count, 1,
        "the actual response endpoint must retain mismatch evidence"
    );
    assert_eq!(
        retry_request_count, 0,
        "invalid ranges must not queue unrelated second requests"
    );

    let state_guard = state_arc.lock().await;
    assert!(state_guard.is_peer_non_range_capable(&peer));
    assert!(
        state_guard.outbound_request_context(request_id).is_none(),
        "completed batch response should clear retained outbound context"
    );
    assert!(client.is_connected(&peer));
    drop(state_guard);
    close_fallback_test_peer(&mut client, &mut server, &state_arc, connection_id).await;
    assert!(
        !state_arc.lock().await.is_peer_non_range_capable(&peer),
        "late session cleanup must not leave a permanent range downgrade after disconnect"
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
    );
    {
        let mut state_guard = state_arc.lock().await;
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
            assert_eq!(requests[0].retry_count, 1);
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
    );
    {
        let mut state_guard = state_arc.lock().await;
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
            assert_eq!(requests[0].retry_count, 1);
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
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 1,
            items: vec![BatchRequestItem {
                item_id: 0,
                message: ByteBuf::from(jam_raw_tx_request(44)),
            }],
        },
        0,
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
            assert_eq!(requests[0].retry_count, 1);
            match &requests[0].request {
                NockchainRequest::BatchRequest { items, .. } => {
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].message.as_ref(), jam_raw_tx_request(44).as_slice());
                }
                other => panic!("expected singleton batch retry request, got {other:?}"),
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
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 0,
                message: ByteBuf::from(jam_raw_tx_request(80_001)),
            }],
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
        state_guard.record_outbound_request(request_id, request_context);
    }

    let response = NockchainResponse::BatchResult {
        results: vec![tx_result_outcome(1, 16).into_batch_result_item(0)],
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
                response: measured_test_response(response),
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

#[tokio::test]
async fn response_without_transport_measurement_clears_context_without_consumption() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let peer = PeerId::random();
    let request_id = fresh_outbound_request_id();
    state.lock().await.record_outbound_request(
        request_id,
        OutboundRequestContext::new(
            peer,
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items: vec![BatchRequestItem {
                    item_id: 0,
                    message: block_range_with_txs_request_message(42, 2).unwrap(),
                }],
            },
        ),
    );
    let traffic =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(4);
    handle_request_response(
        peer,
        ConnectionId::new_unchecked(1),
        request_response::Message::Response {
            request_id,
            response: crate::behaviour::TransportResponse::outgoing(NockchainResponse::Ack {
                acked: true,
            }),
        },
        swarm_tx,
        &mut equix::EquiXBuilder::new(),
        PeerId::random(),
        traffic.traffic.clone(),
        metrics.clone(),
        state.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        PeerExclusions::default(),
    )
    .await
    .unwrap();
    let guard = state.lock().await;
    assert!(guard.outbound_request_context(request_id).is_none());
    assert_eq!(guard.inflight_prefetch_count_for_peer(&peer), 0);
    assert_eq!(guard.prefetch_bandwidth_window_bytes(&peer), 0);
    assert!(swarm_rx.try_recv().is_err());
    assert_eq!(traffic.peek_count.load(Ordering::SeqCst), 0);
    assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 0);
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
                response: measured_test_response(response),
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

fn jam_elders_request(seed: u64, source_peer: &PeerId) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let block_id = tip5_tuple(&mut slab, seed);
    let peer_atom =
        Atom::from_value(&mut slab, source_peer.to_base58()).expect("peer ID should encode");
    let elders = T(&mut slab, &[block_id, peer_atom.as_noun()]);
    let block_request = T(&mut slab, &[D(tas!(b"elders")), elders]);
    let request = T(&mut slab, &[D(tas!(b"block")), block_request]);
    let effect = T(&mut slab, &[D(tas!(b"request")), request]);
    slab.set_root(effect);
    slab.jam().as_ref().to_vec()
}

/// A terminal elders response releases its peer's scheduling slot immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_terminal_elders_batch_item_releases_elders_slot() {
    let transcript = DriverTranscript::default();
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let peer = PeerId::random();
    let local_peer = PeerId::random();
    let scripted_traffic =
        build_scripted_traffic_cop(transcript.clone(), Vec::new(), Vec::new()).await;
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let mut equix_builder = equix::EquiXBuilder::new();

    let terminal_results = [
        (BatchResultStatus::NotFound, None),
        (BatchResultStatus::Ack, None),
        (BatchResultStatus::Error, Some(BatchErrorClass::InvalidPow)),
    ];
    for (round, (status, error)) in terminal_results.into_iter().enumerate() {
        let elders_message = ByteBuf::from(jam_elders_request(500 + round as u64, &peer));
        assert!(matches!(
            decode_request_item_message(&elders_message),
            Ok(NockchainDataRequest::EldersById(..))
        ));
        let key = format!("elders-round-{round}:{}", peer.to_base58());
        {
            let mut state_guard = state_arc.lock().await;
            assert!(
                state_guard.should_send_elders_request(
                    peer,
                    &key,
                    Instant::now(),
                    crate::p2p_state::ELDERS_REQUEST_COOLDOWN,
                    crate::p2p_state::ELDERS_INFLIGHT_TTL,
                ),
                "round {round}: the slot should be free before the request goes out"
            );
            assert_eq!(state_guard.elders_inflight_len(), 1);
        }
        let request_id = fresh_outbound_request_id();
        state_arc.lock().await.record_outbound_request(
            request_id,
            OutboundRequestContext::with_attempt(
                peer,
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![BatchRequestItem {
                        item_id: 1,
                        message: elders_message,
                    }],
                },
                0,
            ),
        );

        let response = NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status,
                error,
                envelope: None,
            }],
        };
        run_driver_with_timeout(
            &transcript,
            "driver should process a terminal elders batch item",
            handle_request_response(
                peer,
                ConnectionId::new_unchecked(1),
                request_response::Message::Response {
                    request_id,
                    response: measured_test_response(response),
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
        .expect("driver should process a terminal elders batch item");

        assert_eq!(
            state_arc.lock().await.elders_inflight_len(),
            0,
            "round {round}: a {status:?} answer for the elders item must release the peer's slot"
        );
        while swarm_rx.try_recv().is_ok() {}
    }
    assert_eq!(scripted_traffic.poke_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exhausted_eof_height_retry_dispatches_alternate_or_stops_without_one() {
    for has_alternate in [true, false] {
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
            metrics.clone(),
            LIBP2P_CONFIG.seen_tx_clear_interval,
            Arc::new(PeerStatsRegistry::default()),
        )));
        let local_peer = PeerId::random();
        let source_peer = PeerId::random();
        let alternate_peer = PeerId::random();
        let height = 10030;
        let height_message = ByteBuf::from(jam_block_by_height_request(height));
        let tx_message = ByteBuf::from(jam_raw_tx_request(44));
        {
            let mut guard = state.lock().await;
            seed_connected_peer(&mut guard, source_peer, 400);
            if has_alternate {
                seed_connected_peer(&mut guard, alternate_peer, 401);
            }
            guard.track_block_height_attempted_peers(height, [source_peer]);
        }
        let context = OutboundRequestContext::with_attempt(
            source_peer,
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 1,
                items: vec![
                    BatchRequestItem {
                        item_id: 1,
                        message: height_message.clone(),
                    },
                    BatchRequestItem {
                        item_id: 2,
                        message: tx_message.clone(),
                    },
                ],
            },
            0,
        );
        let mut pending = VecDeque::from([context]);
        let mut equix_builder = equix::EquiXBuilder::new();
        let exclusions = PeerExclusions::default();
        let mut alternate_dispatches = 0;
        while let Some(failed_context) = pending.pop_front() {
            let attempt = failed_context.retry_count;
            let NockchainRequest::BatchRequest { items, .. } = &failed_context.request else {
                panic!("expected retained batch");
            };
            let expected_items = items
                .iter()
                .map(|item| (item.item_id, item.message.clone()))
                .collect::<BTreeMap<_, _>>();
            let request_id = fresh_outbound_request_id();
            state
                .lock()
                .await
                .record_outbound_request(request_id, failed_context);
            let mut buffered = VecDeque::new();
            let mut dispatcher = SwarmActionDispatcher::Buffered(&mut buffered);
            handle_outbound_request_failure_with_dispatcher(
                &mut dispatcher,
                state.clone(),
                metrics.clone(),
                local_peer,
                &mut equix_builder,
                exclusions.clone(),
                source_peer,
                request_id,
                request_response::OutboundFailure::Io(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )),
            )
            .await;
            assert!(state
                .lock()
                .await
                .outbound_request_context(request_id)
                .is_none());
            if attempt < GEN2_RETRY_MAX_ATTEMPTS {
                let Some(SwarmAction::RetryRequests { requests, .. }) = buffered.pop_front() else {
                    panic!("unexhausted EOF must retain same-peer retries");
                };
                let mut retried_items = BTreeMap::new();
                for retry in requests {
                    assert_eq!(retry.peer_id, source_peer);
                    assert_eq!(retry.retry_count, attempt + 1);
                    retry
                        .request
                        .verify_pow(&mut equix_builder, &source_peer, &local_peer)
                        .expect("same-peer retry must retain authenticated identity");
                    let NockchainRequest::BatchRequest { items, .. } = &retry.request else {
                        panic!("expected batch retry");
                    };
                    for item in items {
                        assert!(retried_items
                            .insert(item.item_id, item.message.clone())
                            .is_none());
                    }
                    pending.push_back(retry);
                }
                assert_eq!(retried_items, expected_items);
                assert_eq!(
                    state.lock().await.get_block_height_attempted_peers(height),
                    vec![source_peer],
                );
            } else {
                while let Some(action) = buffered.pop_front() {
                    let SwarmAction::QueueKernelRequest {
                        peer_id,
                        request_message,
                    } = action
                    else {
                        panic!("exhausted EOF cannot restart same-peer retries");
                    };
                    assert!(has_alternate);
                    assert_eq!(peer_id, alternate_peer);
                    assert_eq!(request_message, height_message);
                    alternate_dispatches += 1;
                }
            }
            assert!(buffered.is_empty(), "failure must not queue unrelated work");
        }
        assert_eq!(alternate_dispatches, usize::from(has_alternate));
        let guard = state.lock().await;
        assert_eq!(guard.total_outbound_request_count(), 0);
        let attempted = guard.get_block_height_attempted_peers(height);
        if has_alternate {
            assert!(attempted.contains(&source_peer));
            assert!(attempted.contains(&alternate_peer));
        } else {
            assert!(
                attempted.is_empty(),
                "no alternate recycles history without looping"
            );
        }
    }
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
    canonical_peers.sort_unstable();
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![BatchRequestItem {
                        item_id: 0,
                        message: request_message.clone(),
                    }],
                },
                0,
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
async fn req_res_driver_batch_block_timeout_queues_alternate_peer_retry() {
    let metrics = isolated_test_metrics();
    let state_arc = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let local_peer = PeerId::random();
    let peers = (0..3).map(|_| PeerId::random()).collect::<Vec<_>>();
    let mut canonical_peers = peers.clone();
    canonical_peers.sort_unstable();
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
    canonical_peers.sort_unstable();
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
    let exclusions = PeerExclusions::default();
    exclusions.record_peer_request_failure(source_peer);
    state_arc.lock().await.record_request_failure(source_peer);

    handle_request_response(
        source_peer,
        connection_id,
        request_response::Message::Response {
            request_id,
            response: measured_test_response(response),
        },
        swarm_tx.clone(),
        &mut equix_builder,
        local_peer,
        scripted_traffic.traffic.clone(),
        metrics.clone(),
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        exclusions.clone(),
    )
    .await
    .expect("batch not-found block item should be handled");
    assert!(exclusions.is_peer_request_cooled_down(&source_peer));
    assert_eq!(state_arc.lock().await.request_score(&source_peer), -1);

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
    let retry_id = fresh_outbound_request_id();
    state_arc.lock().await.record_outbound_request(
        retry_id,
        OutboundRequestContext::with_attempt(
            expected_retry_peer,
            NockchainRequest::BatchRequest {
                pow: [0; 16],
                nonce: 2,
                items: vec![
                    BatchRequestItem {
                        item_id: 1,
                        message: block_request_message,
                    },
                    BatchRequestItem {
                        item_id: 2,
                        message: ByteBuf::from(jam_raw_tx_request(44)),
                    },
                ],
            },
            0,
        ),
    );
    let block = bundled_block_from_fact(&heard_block_fact_with_tx_ids(10030, &[]).0);
    let useful_traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        Vec::new(),
        vec![PokeResult::Ack],
    )
    .await;
    handle_request_response(
        expected_retry_peer,
        connection_id,
        request_response::Message::Response {
            request_id: retry_id,
            response: measured_test_response(NockchainResponse::BatchResult {
                results: vec![
                    BatchResultItem {
                        item_id: 1,
                        status: BatchResultStatus::Result,
                        error: None,
                        envelope: Some(ResponseEnvelope::heard_block(
                            block.block_id, block.block_message,
                        )),
                    },
                    batch_result_not_found_item(2),
                ],
            }),
        },
        swarm_tx,
        &mut equix_builder,
        local_peer,
        useful_traffic.traffic,
        metrics,
        state_arc.clone(),
        runtime_limits_from_config(&LIBP2P_CONFIG),
        exclusions.clone(),
    )
    .await
    .expect("alternate useful block should route");
    let guard = state_arc.lock().await;
    assert_eq!(guard.request_score(&source_peer), -1);
    assert_eq!(guard.request_score(&expected_retry_peer), 2);
    assert!(
        guard
            .get_block_height_attempted_peers(10030)
            .contains(&expected_retry_peer),
        "kernel Ack alone must not claim accepted-chain progress"
    );
    assert!(exclusions.is_peer_request_cooled_down(&source_peer));
    assert!(
        swarm_rx.try_recv().is_err(),
        "useful alternate must not retry"
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
                NockchainRequest::BatchRequest {
                    pow: [0; 16],
                    nonce: 1,
                    items: vec![BatchRequestItem {
                        item_id: 1,
                        message: block_request_message,
                    }],
                },
                0,
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
            response: measured_test_response(response),
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
    assert_eq!(
        state_arc.lock().await.request_score(&source_peer),
        0,
        "NotFound-only must not earn useful-response score"
    );

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
                response: measured_test_response(response),
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

// ---- deferred flush worker lifecycle ----

async fn dispatch_deferred_flush(
    swarm: &mut Swarm<NockchainBehaviour>,
    swarm_tx: &tokio::sync::mpsc::Sender<SwarmAction>,
    deferred_flush_running: &Arc<std::sync::atomic::AtomicBool>,
    join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
    state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
    traffic: &traffic_cop::TrafficCop,
) {
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut pending_gen2_batches = BTreeMap::new();
    process_swarm_action(
        SwarmAction::FlushDeferredHeardBlocks,
        swarm,
        swarm_tx,
        deferred_flush_running,
        join_set,
        state,
        metrics,
        &PeerExclusions::default(),
        &mut equix_builder,
        &mut pending_gen2_batches,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        traffic,
    )
    .await
    .expect("flush dispatch should succeed");
}

fn deferred_flush_test_swarm() -> Swarm<NockchainBehaviour> {
    start_swarm(
        LibP2PConfig::default(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .expect("production swarm should build")
}

fn malformed_heard_block_fact(block_id: &str) -> NockchainFact {
    let mut slab: NounSlab = NounSlab::new();
    let inner = T(&mut slab, &[D(1), D(2)]);
    slab.set_root(inner);
    NockchainFact::HeardBlock(String::from(block_id), slab)
}

/// Let a freshly spawned worker run until it parks on the state lock the
/// test is holding. Under the current-thread test runtime a few yields are
/// enough to poll it to that point.
async fn let_worker_park() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// a second flush action arriving while the worker flag is set is
/// coalesced; it does not spawn a second worker.
#[tokio::test]
async fn deferred_flush_action_is_coalesced_while_worker_runs() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let harness =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;
    let mut swarm = deferred_flush_test_swarm();
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();

    // Hold the state lock so the first worker is parked while the second
    // action arrives.
    let guard = state.lock().await;
    dispatch_deferred_flush(
        &mut swarm, &swarm_tx, &running, &mut join_set, &state, &metrics, &harness.traffic,
    )
    .await;
    assert!(
        running.load(Ordering::Acquire),
        "the first action must claim the worker flag"
    );
    dispatch_deferred_flush(
        &mut swarm, &swarm_tx, &running, &mut join_set, &state, &metrics, &harness.traffic,
    )
    .await;
    drop(guard);

    let first = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("worker should finish")
        .expect("one worker should have been spawned")
        .expect("worker task should not panic");
    assert!(first.is_ok(), "worker should exit cleanly: {first:?}");
    assert!(
        join_set.join_next().await.is_none(),
        "the coalesced action must not spawn a second worker"
    );
    assert!(
        !running.load(Ordering::Acquire),
        "the worker releases its flag on exit"
    );
    assert!(
        swarm_rx.try_recv().is_err(),
        "nothing was ready, so the worker must not re-arm itself"
    );
}

/// a wakeup that is coalesced after the worker's empty check but before
/// it releases the flag is recovered by the post-release re-check.
#[tokio::test]
async fn deferred_flush_wakeup_arriving_as_worker_exits_is_recovered() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let harness =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;
    let mut swarm = deferred_flush_test_swarm();
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let peer = PeerId::random();

    let mut guard = state.lock().await;
    guard.first_negative = 10;
    dispatch_deferred_flush(
        &mut swarm, &swarm_tx, &running, &mut join_set, &state, &metrics, &harness.traffic,
    )
    .await;
    let_worker_park().await;
    drop(guard);
    // tokio's mutex is FIFO: this guard is handed over right after the
    // worker's empty take and before its post-release re-check, which is the
    // window a coalesced wakeup falls into.
    let mut guard = state.lock().await;
    assert!(!guard.has_ready_deferred_heard_blocks());
    guard.defer_heard_block(
        peer,
        9,
        String::from("ready-9"),
        malformed_heard_block_fact("ready-9"),
    );
    assert!(guard.has_ready_deferred_heard_blocks());
    drop(guard);

    let result = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("worker should finish")
        .expect("one worker should have been spawned")
        .expect("worker task should not panic");
    assert!(
        result.is_ok(),
        "an empty flush must exit cleanly: {result:?}"
    );
    match tokio::time::timeout(Duration::from_secs(1), swarm_rx.recv()).await {
        Ok(Some(SwarmAction::FlushDeferredHeardBlocks)) => {}
        other => panic!("expected the exiting worker to re-arm the flush, got {other:?}"),
    }
    assert!(!running.load(Ordering::Acquire));
}

/// a worker that fails mid-flush still releases its flag and re-arms for
/// the ready blocks it did not reach, instead of parking them until the next
/// seen-effect.
#[tokio::test]
async fn failed_deferred_flush_worker_releases_flag_and_rearms_for_remaining_blocks() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(
        metrics.clone(),
        LIBP2P_CONFIG.seen_tx_clear_interval,
    )));
    let harness =
        build_scripted_traffic_cop(DriverTranscript::default(), Vec::new(), Vec::new()).await;
    let mut swarm = deferred_flush_test_swarm();
    let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
    let running = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut join_set = TrackedJoinSet::<Result<(), NockAppError>>::new();
    let peer = PeerId::random();

    let mut guard = state.lock().await;
    guard.first_negative = 10;
    // Ready now; its malformed page makes routing fail before any kernel poke.
    assert!(guard.defer_heard_block(
        peer,
        5,
        String::from("broken-5"),
        malformed_heard_block_fact("broken-5"),
    ));
    // Parked until the kernel asks for its height.
    assert!(guard.defer_heard_block_with_source(
        peer,
        7,
        String::from("parked-7"),
        malformed_heard_block_fact("parked-7"),
        BlockSource::Prefetch,
    ));
    dispatch_deferred_flush(
        &mut swarm, &swarm_tx, &running, &mut join_set, &state, &metrics, &harness.traffic,
    )
    .await;
    let_worker_park().await;
    drop(guard);
    // Handed over after the worker took block 5 and before its re-check.
    let mut guard = state.lock().await;
    assert!(
        !guard.has_deferred_block_at_height(5),
        "the worker should have taken the ready block"
    );
    assert!(!guard.has_ready_deferred_heard_blocks());
    guard.note_kernel_block_height_requested(7);
    assert!(guard.has_ready_deferred_heard_blocks());
    drop(guard);

    let result = tokio::time::timeout(Duration::from_secs(5), join_set.join_next())
        .await
        .expect("worker should finish")
        .expect("one worker should have been spawned")
        .expect("worker task should not panic");
    assert!(
        result.is_err(),
        "a malformed deferred block must surface as a worker error"
    );
    match tokio::time::timeout(Duration::from_secs(1), swarm_rx.recv()).await {
        Ok(Some(SwarmAction::FlushDeferredHeardBlocks)) => {}
        other => panic!("expected the failed worker to re-arm for block 7, got {other:?}"),
    }
    assert!(
        !running.load(Ordering::Acquire),
        "a failed worker must release its flag"
    );
    assert_eq!(harness.poke_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prefetch_response_budget_follows_the_actual_sdk_connection() {
    use libp2p::core::ConnectedPoint;

    use crate::p2p_state::IpBucket;
    for reverse in [false, true] {
        let config = LibP2PConfig::default();
        let mut client = start_swarm(
            config.clone(),
            Keypair::generate_ed25519(),
            Vec::new(),
            None,
            connection_limits::ConnectionLimits::default(),
            None,
            PeerExclusions::default(),
        )
        .unwrap();
        let (mut server, _trace, _raw_request, raw_response) =
            crate::test_support::build_req_res_test_swarm_with_protocol_trace(
                "attribution-server",
                config,
                Keypair::generate_ed25519(),
                vec![
                    "/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap(),
                    "/ip6/::1/udp/0/quic-v1".parse().unwrap(),
                ],
            )
            .unwrap();
        let server_peer = *server.local_peer_id();
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
        let mut server_sockets = BTreeMap::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut addresses = Vec::new();
            while addresses.len() < 2 {
                if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await {
                    addresses.push(address);
                }
            }
            addresses.sort();
            if reverse { addresses.reverse(); }
            for address in &addresses {
                client.dial(address.clone()).unwrap();
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                                state.lock().await.track_connection(
                                    connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                                break;
                            }
                            SwarmEvent::OutgoingConnectionError { error, .. } => panic!("dial failed: {error}"),
                            _ => {}
                        },
                        event = server.select_next_some() => {
                            if let SwarmEvent::ConnectionEstablished {
                                connection_id, endpoint: ConnectedPoint::Listener { local_addr, .. }, ..
                            } = event {
                                server_sockets.insert(connection_id, quic_socket_addr(&local_addr).unwrap());
                            }
                        }
                    }
                }
            }
            let snapshot = client.behaviour().recovery.request_endpoints(&server_peer).unwrap();
            assert_eq!(snapshot.len(), 2);
            assert_ne!(snapshot[0].endpoint.ip(), snapshot[1].endpoint.ip());
            let mut served_endpoints = BTreeSet::new();
            let mut expected_socket = None;
            for nonce in 0..2 {
                send_outbound_request_now(&mut client, &state, &metrics,
                    OutboundRequestContext::new(server_peer, NockchainRequest::BatchRequest {
                        pow: [0; 16], nonce,
                        items: vec![BatchRequestItem {
                            item_id: 0,
                            message: block_range_with_txs_request_message(42 + nonce, 2).unwrap(),
                        }],
                    })).await;
                loop {
                    tokio::select! {
                        event = client.select_next_some() => match event {
                            SwarmEvent::Behaviour(NockchainEvent::RequestResponse(request_response::Event::Message {
                                peer, connection_id,
                                message: request_response::Message::Response { request_id, response },
                            })) => {
                                let bytes = response.received_bytes.expect("codec measurement");
                                assert_eq!(bytes, req_res_message_encoded_bytes(&response.message).unwrap() + 257);
                                let mut guard = state.lock().await;
                                let context = guard.outbound_request_context(request_id).unwrap();
                                assert_eq!(context.request_endpoints.len(), 2);
                                assert!(!context.request_endpoints.spilled());
                                assert!(context.request_endpoints.iter().any(|candidate|
                                    candidate.connection_id == connection_id));
                                let bucket_windows: Vec<_> = context.request_endpoints.iter()
                                    .map(|candidate| {
                                        let bucket = IpBucket::from_ip(candidate.endpoint.ip());
                                        (bucket, guard.prefetch_bucket_window_bytes(bucket))
                                    }).collect();
                                let charge = guard.charge_received_response(request_id, peer, connection_id, None, bytes)
                                    .unwrap().unwrap();
                                assert_eq!(Some(charge.endpoint), expected_socket.take());
                                assert_eq!(charge.bucket, IpBucket::from_ip(charge.endpoint.ip()));
                                for (bucket, before) in bucket_windows {
                                    assert_eq!(guard.prefetch_bucket_window_bytes(bucket),
                                        before + if bucket == charge.bucket { bytes } else { 0 });
                                }
                                served_endpoints.insert(charge.endpoint);
                                guard.remove_outbound_request(request_id);
                                break;
                            }
                            SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                                request_response::Event::OutboundFailure { error, .. }
                            )) => panic!("range exchange failed: {error}"),
                            _ => {}
                        },
                        event = server.select_next_some() => match event {
                            SwarmEvent::ConnectionEstablished {
                                connection_id, endpoint: ConnectedPoint::Listener { local_addr, .. }, ..
                            } => {
                                server_sockets.insert(connection_id, quic_socket_addr(&local_addr).unwrap());
                            }
                            SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message {
                                connection_id, message: request_response::Message::Request { channel, .. }, ..
                            })) => {
                                expected_socket = Some(server_sockets[&connection_id]);
                                let message = NockchainResponse::BatchResult { results: vec![BatchResultItem {
                                    item_id: 0, status: BatchResultStatus::NotFound, error: None, envelope: None,
                                }] };
                                let mut raw = cbor4ii::serde::to_vec(Vec::new(), &message).unwrap();
                                raw.resize(raw.len() + 257, 0);
                                raw_response.inject_once(raw);
                                server.behaviour_mut().request_response.send_response(
                                    channel, crate::behaviour::TransportResponse::outgoing(message)).unwrap();
                            }
                            _ => {}
                        }
                    }
                }
            }
            assert_eq!(served_endpoints.len(), 2,
                "both actual SDK-selected streams must be attributed, independent of dial order");
            for nonce in 2..7 {
                send_outbound_request_now(&mut client, &state, &metrics,
                    OutboundRequestContext::new(server_peer, NockchainRequest::BatchRequest {
                        pow: [0; 16], nonce,
                        items: vec![BatchRequestItem {
                            item_id: 0,
                            message: block_range_with_txs_request_message(42 + nonce, 2).unwrap(),
                        }],
                    })).await;
            }
            assert_eq!(state.lock().await.inflight_prefetch_count_for_peer(&server_peer), 4,
                "buffered producer actions cannot exceed the submission-time limit");
        }).await.expect("local dual-connection exchanges must complete");
    }
}

#[tokio::test]
async fn range_prefetch_is_accounted_separately_from_ordinary_requests() {
    use crate::messages::{block_with_txs_by_height_request_message, decode_request_item_message};
    use crate::p2p_state::IpBucket;

    let config = LibP2PConfig::default();
    let limits = runtime_limits_from_config(&config);
    let mut client = start_swarm(
        config.clone(),
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        PeerExclusions::default(),
    )
    .unwrap();
    let (mut server, _trace, _raw_request, raw_response) =
        crate::test_support::build_req_res_test_swarm_with_protocol_trace(
            "accounting-server",
            config.clone(),
            Keypair::generate_ed25519(),
            vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
        )
        .unwrap();
    let peer = *server.local_peer_id();
    let local_peer = *client.local_peer_id();
    let metrics = isolated_test_metrics();
    let peer_stats = Arc::new(PeerStatsRegistry::default());
    let state = Arc::new(Mutex::new(P2PState::with_peer_stats_registry(
        metrics.clone(),
        100,
        peer_stats.clone(),
    )));
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        Vec::new(),
        (0..8).map(|_| PokeResult::Ack).collect(),
    )
    .await;
    let (swarm_tx, _swarm_rx) = tokio::sync::mpsc::channel(32);
    let mut builder = equix::EquiXBuilder::new();
    let mut pending = BTreeMap::new();
    let prefetch = PrefetchConfig {
        enabled: true,
        window_initial: 4,
        window_max: 4,
        max_inflight_per_peer: 1,
        kernel_demand_threshold: 8,
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await {
                break address;
            }
        };
        client.dial(address).unwrap();
        loop {
            tokio::select! {
                event = client.select_next_some() => match event {
                    SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                        state.lock().await.track_connection(
                            connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                        break;
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => panic!("dial failed: {error}"),
                    _ => {}
                },
                _ = server.select_next_some() => {}
            }
        }
        let endpoints = client.behaviour().recovery.request_endpoints(&peer).unwrap();
        assert_eq!(endpoints.len(), 1);
        let endpoint = endpoints[0];
        let bucket = IpBucket::from_ip(endpoint.endpoint.ip());
        let mut charged_bytes = 0;
        let mut received_bytes_total = 0u64;
        let mut expected_responses = VecDeque::new();
        for phase in 0..3 {
            let expected_items = match phase { 0 => 2, 1 => 3, _ => 1 };
            if phase == 0 {
                state.lock().await.first_negative = 43;
                process_queue_kernel_request_action(
                    peer, block_with_txs_by_height_request_message(40).unwrap(),
                    &mut client, &state, &metrics, &mut builder, &mut pending, limits,
                ).await.unwrap();
                process_queue_kernel_request_action(
                    peer, block_by_height_message(40), &mut client, &state, &metrics,
                    &mut builder, &mut pending, limits,
                ).await.unwrap();
            } else {
                if phase == 1 {
                    let mut guard = state.lock().await;
                    guard.mark_peer_range_supported(peer);
                    // A unit boundary cap, not a production-volume profile.
                    guard.set_prefetch_safety_config(3, Duration::from_secs(60), 1);
                    drop(guard);
                    process_queue_kernel_request_action(
                        peer, block_by_height_message(49), &mut client, &state, &metrics,
                        &mut builder, &mut pending, limits,
                    ).await.unwrap();
                    let range = block_range_with_txs_request_message(50, 4).unwrap();
                    let range_estimate = outbound_request_message_estimated_response_bytes(
                        &range, limits, &state).await;
                    let ordinary_estimate = outbound_request_message_estimated_response_bytes(
                        &block_by_height_message(50), limits, &state).await;
                    assert!(range_estimate + 2 * ordinary_estimate < block_batch_response_budget_bytes(limits),
                        "observed small responses must leave room for ordinary coalescing");
                }
                let height = if phase == 1 { 50 } else { 60 };
                let mut actions = VecDeque::new();
                handle_effect_with_dispatcher(
                    request_slab_from_message(block_by_height_message(height).as_ref()).unwrap(),
                    &mut SwarmActionDispatcher::Buffered(&mut actions), vec![peer], false,
                    prefetch, limits, state.clone(), metrics.clone(), PeerExclusions::default(),
                ).await.unwrap();
                assert_eq!(actions.len(), if phase == 1 { 2 } else { 1 });
                while let Some(action) = actions.pop_front() {
                    match action {
                        SwarmAction::QueueKernelRequest { peer_id, request_message } => {
                            process_queue_kernel_request_action(
                                peer_id, request_message, &mut client, &state, &metrics,
                                &mut builder, &mut pending, limits,
                            ).await.unwrap();
                        }
                        other => panic!("unexpected demand action: {other:?}"),
                    }
                }
            }
            if let Some(context) = take_pending_batch_request(
                &mut pending, peer, &local_peer, &mut builder,
            ).unwrap() {
                send_outbound_request_now(&mut client, &state, &metrics, context).await;
            }
            assert!(pending.is_empty());
            let mut received_items = 0;
            let mut received_heights = BTreeSet::new();
            let mut queued_responses: VecDeque<(
                request_response::ResponseChannel<crate::behaviour::TransportResponse>,
                NockchainResponse,
            )> = VecDeque::new();
            while received_items < expected_items {
                // The raw injection hook owns one pending buffer, so serialize only
                // these test replies while continuing to drive both SDK swarms.
                if expected_responses.is_empty() {
                    if let Some((channel, message)) = queued_responses.pop_front() {
                        let mut raw = cbor4ii::serde::to_vec(Vec::new(), &message).unwrap();
                        let canonical_bytes = raw.len();
                        raw.resize(canonical_bytes + 257, 0);
                        assert!(raw.len() < config.gen2_batch_max_bytes());
                        expected_responses.push_back((message.clone(), raw.len(), canonical_bytes));
                        raw_response.inject_once(raw);
                        server.behaviour_mut().request_response.send_response(
                            channel, crate::behaviour::TransportResponse::outgoing(message)).unwrap();
                    }
                }
                tokio::select! {
                    event = server.select_next_some() => {
                        if let SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message {
                            message: request_response::Message::Request { request, channel, .. }, ..
                        })) = event {
                            let NockchainRequest::BatchRequest { items, .. } = request else {
                                panic!("expected batch request");
                            };
                            let results = items.into_iter().map(|item| {
                                let envelope = match decode_request_item_message(&item.message).unwrap() {
                                    NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                                        ResponseEnvelope::heard_block_range_with_txs(
                                            (start_height..start_height + u64::from(len)).map(|height| {
                                                bundled_block_from_fact(&heard_block_fact_with_tx_ids(height, &[]).0)
                                            }).collect())
                                    }
                                    NockchainDataRequest::BlockWithTxsByHeight(height) => {
                                        let block = bundled_block_from_fact(&heard_block_fact_with_tx_ids(height, &[]).0);
                                        ResponseEnvelope::heard_block_with_txs(
                                            block.block_id, &block.block_message, Vec::new(), Vec::new())
                                    }
                                    NockchainDataRequest::BlockByHeight(height) => {
                                        let block = bundled_block_from_fact(&heard_block_fact_with_tx_ids(height, &[]).0);
                                        ResponseEnvelope::heard_block(block.block_id, block.block_message)
                                    }
                                    other => panic!("unexpected request: {other:?}"),
                                };
                                envelope.validate().unwrap();
                                BatchResultItem { item_id: item.item_id, status: BatchResultStatus::Result,
                                    error: None, envelope: Some(envelope) }
                            }).collect();
                            let message = NockchainResponse::BatchResult { results };
                            queued_responses.push_back((channel, message));
                        }
                    },
                    event = client.select_next_some() => match event {
                        SwarmEvent::Behaviour(NockchainEvent::RequestResponse(request_response::Event::Message {
                            peer: response_peer, connection_id,
                            message: request_response::Message::Response { request_id, response },
                        })) => {
                            let bytes = response.received_bytes.expect("codec measurement");
                            let expected_index = expected_responses.iter().position(|(message, _, _)| message == &response.message).unwrap();
                            let (expected, raw_bytes, canonical_bytes) = expected_responses.remove(expected_index).unwrap();
                            assert_eq!(response.message, expected);
                            assert_eq!(bytes, raw_bytes);
                            assert!(bytes > canonical_bytes);
                            let mut guard = state.lock().await;
                            let context = guard.outbound_request_context(request_id).unwrap();
                            assert_eq!(context.request_endpoints.len(), 1);
                            assert_eq!(context.request_endpoints[0].connection_id, endpoint.connection_id);
                            assert_eq!(context.request_endpoints[0].endpoint, endpoint.endpoint);
                            let NockchainRequest::BatchRequest { items, .. } = &context.request else {
                                panic!("expected batch context");
                            };
                            received_items += items.len();
                            let mut has_range = false;
                            for item in items {
                                match decode_request_item_message(&item.message).unwrap() {
                                    NockchainDataRequest::BlockRangeWithTxs { start_height, .. } => {
                                        has_range = true;
                                        received_heights.insert(start_height);
                                    }
                                    NockchainDataRequest::BlockByHeight(h) |
                                    NockchainDataRequest::BlockWithTxsByHeight(h) => { received_heights.insert(h); }
                                    other => panic!("unexpected context: {other:?}"),
                                }
                            }
                            let charge = guard.charge_received_response(
                                request_id, response_peer, connection_id, None, bytes).unwrap();
                            if has_range {
                                charged_bytes += bytes;
                                let charge = charge.expect("range receipt must create peer and endpoint debt");
                                assert_eq!(charge.endpoint, endpoint.endpoint);
                                assert_eq!(charge.bucket, bucket);
                                assert!(guard.charge_received_response(
                                    request_id, response_peer, connection_id, None, bytes).unwrap().is_none());
                            } else {
                                assert!(charge.is_none());
                            }
                            assert_eq!(guard.prefetch_bandwidth_window_bytes(&peer), charged_bytes);
                            assert_eq!(guard.prefetch_bucket_window_bytes(bucket), charged_bytes);
                            drop(guard);
                            handle_request_response(
                                response_peer, connection_id,
                                request_response::Message::Response { request_id, response },
                                swarm_tx.clone(), &mut builder, local_peer, traffic.traffic.clone(),
                                metrics.clone(), state.clone(), limits, PeerExclusions::default(),
                            ).await.unwrap();
                            received_bytes_total += bytes as u64;
                            let snapshot = peer_stats.snapshot();
                            let entry = snapshot.peers.iter().find(|entry| entry.peer_id == peer.to_string()).unwrap();
                            assert_eq!(entry.bytes_received, received_bytes_total);
                            let mut guard = state.lock().await;
                            assert!(guard.outbound_request_context(request_id).is_none());
                            assert!(guard.charge_received_response(request_id, response_peer, connection_id, None, bytes).unwrap().is_none());
                            assert_eq!(guard.prefetch_bandwidth_window_bytes(&peer), charged_bytes);
                        }
                        SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                            request_response::Event::OutboundFailure { error, .. }
                        )) => panic!("exchange failed: {error}"),
                        _ => {}
                    }
                }
            }
            assert_eq!(received_items, expected_items);
            assert_eq!(received_heights, match phase {
                0 => BTreeSet::from([40]), 1 => BTreeSet::from([49, 50]), _ => BTreeSet::from([60]),
            });
            let guard = state.lock().await;
            assert_eq!(guard.prefetch_bandwidth_window_bytes(&peer), charged_bytes);
            assert_eq!(guard.prefetch_bucket_window_bytes(bucket), charged_bytes);
            if phase > 0 {
                assert!(charged_bytes > guard.prefetch_bandwidth_cap_per_peer_bytes_per_min());
                assert!(charged_bytes > guard.prefetch_bucket_bandwidth_cap());
                for height in 51..54 {
                    assert_eq!(guard.deferred_heard_block_sources_at_height(height), vec![BlockSource::Prefetch]);
                }
                assert!(guard.has_deferred_block_at_height(49));
                assert_eq!(guard.inflight_prefetch_count_for_peer(&peer), 0);
            }
        }
    }).await.expect("local range and ordinary exchanges must complete");
}

mod response_validation;

#[tokio::test]
async fn wrong_identity_on_another_socket_preserves_same_identity_service() {
    let config = LibP2PConfig::default();
    let keypair = Keypair::generate_ed25519();
    let mut good = build_req_res_test_swarm(
        config.clone(),
        keypair.clone(),
        vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
    )
    .unwrap();
    let mut alternate = build_req_res_test_swarm(
        config.clone(),
        keypair,
        vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()],
    )
    .unwrap();
    let good_peer = *good.local_peer_id();
    let exclusions = PeerExclusions::default();
    let mut client = start_swarm(
        config,
        Keypair::generate_ed25519(),
        Vec::new(),
        None,
        connection_limits::ConnectionLimits::default(),
        None,
        exclusions.clone(),
    )
    .unwrap();
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    tokio::time::timeout(Duration::from_secs(30), async {
        let good_addr = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = good.select_next_some().await { break address; }
        };
        let alternate_addr = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = alternate.select_next_some().await { break address; }
        };
        assert_eq!(good_addr.canonical_ip_addr(), alternate_addr.canonical_ip_addr());
        assert_ne!(good_addr, alternate_addr);
        client.dial(good_addr.clone()).unwrap();
        loop {
            tokio::select! {
                event = client.select_next_some() => if let SwarmEvent::ConnectionEstablished {
                    connection_id, peer_id, endpoint, ..
                } = event {
                    state.lock().await.track_connection(
                        connection_id, peer_id, endpoint.get_remote_address(), endpoint.clone());
                    break;
                },
                _ = good.select_next_some() => {}
            }
        }
        let expected = PeerId::random();
        client.behaviour_mut().kad.add_address(&good_peer, good_addr.clone());
        client.behaviour_mut().kad.add_address(&good_peer, alternate_addr.clone());
        client.behaviour_mut().peer_store.store_mut().add_address(&good_peer, &good_addr);
        client.behaviour_mut().peer_store.store_mut().add_address(&good_peer, &alternate_addr);
        client.behaviour_mut().peer_store.store_mut().add_address(&expected, &alternate_addr);
        client.dial(alternate_addr.clone().with_p2p(expected).unwrap()).unwrap();
        loop {
            tokio::select! {
                event = client.select_next_some() => if let SwarmEvent::OutgoingConnectionError { error, .. } = event {
                    assert!(matches!(&error, DialError::WrongPeerId { obtained, .. } if *obtained == good_peer));
                    handle_outgoing_connection_error(
                        &mut client, &state, &exclusions, &metrics, error,
                        Some(expected), std::slice::from_ref(&alternate_addr), &[],
                    ).await;
                    break;
                },
                _ = good.select_next_some() => {},
                _ = alternate.select_next_some() => {}
            }
        }
        assert!(exclusions.is_address_excluded(&alternate_addr, Some(expected)));
        assert!(!exclusions.is_ip_excluded(&good_addr.canonical_ip_addr().unwrap()));
        assert!(client.is_connected(&good_peer));
        assert!(state.lock().await.peer_has_connection_at_address(&good_peer, &good_addr));
        assert!(client.behaviour_mut().peer_store.address_of_peer(&good_peer).unwrap()
            .any(|address| address == &good_addr));
        send_outbound_request_now(&mut client, &state, &metrics,
            OutboundRequestContext::new(good_peer, NockchainRequest::BatchRequest {
                pow: [0; 16], nonce: 99, items: Vec::new(),
            })).await;
        loop {
            tokio::select! {
                event = client.select_next_some() => match event {
                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(request_response::Event::Message {
                        peer, message: request_response::Message::Response {
                            request_id, response: crate::behaviour::TransportResponse { message: NockchainResponse::Ack { acked: true }, .. },
                        }, ..
                    })) => {
                        assert_eq!(peer, good_peer);
                        state.lock().await.remove_outbound_request(request_id);
                        break;
                    }
                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                        request_response::Event::OutboundFailure { error, .. }
                    )) => panic!("preserved endpoint failed to serve: {error}"),
                    _ => {}
                },
                event = good.select_next_some() => {
                    if let SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message {
                        message: request_response::Message::Request { channel, .. }, ..
                    })) = event {
                        good.behaviour_mut().request_response.send_response(
                            channel, crate::behaviour::TransportResponse::outgoing(NockchainResponse::Ack { acked: true })).unwrap();
                    }
                }
            }
        }
    }).await.expect("same-IP identity attribution control must complete");
}

#[test]
fn encoded_response_size_matches_transport_cbor_without_a_payload_copy() {
    let response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_block(
                "encoded-block".to_owned(),
                vec![42; 10_000],
            )),
        }],
    };
    let mut encoded = Vec::new();
    cbor4ii::serde::to_writer(&mut encoded, &response).unwrap();
    assert_eq!(
        req_res_message_encoded_bytes(&response).unwrap(),
        encoded.len()
    );
}

#[tokio::test]
async fn late_disconnected_failures_retire_session_metadata_across_peer_churn() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let mut identifiers =
        crate::behaviour::build_request_response_behaviour(&LibP2PConfig::default());
    let local_peer = PeerId::random();
    let exclusions = PeerExclusions::default();
    let mut equix_builder = equix::EquiXBuilder::new();
    let address: Multiaddr = "/ip4/198.51.100.7/udp/6000/quic-v1".parse().unwrap();
    for index in 0..64 {
        let peer = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(40_000 + index);
        let mut request_ids = Vec::new();
        {
            let mut guard = state.lock().await;
            guard.track_connection(
                connection_id,
                peer,
                &address,
                libp2p::core::ConnectedPoint::Dialer {
                    address: address.clone(),
                    role_override: libp2p::core::Endpoint::Dialer,
                    port_use: libp2p::core::transport::PortUse::Reuse,
                },
            );
            for nonce in 0..2 {
                let mut context = OutboundRequestContext::with_attempt(
                    peer,
                    NockchainRequest::BatchRequest {
                        pow: [0; 16],
                        nonce,
                        items: vec![BatchRequestItem {
                            item_id: 0,
                            message: block_range_with_txs_request_message(42, 2).unwrap(),
                        }],
                    },
                    GEN2_RETRY_MAX_ATTEMPTS,
                );
                context.request_endpoints.push(RequestEndpoint {
                    connection_id,
                    endpoint: quic_socket_addr(&address).unwrap(),
                });
                let request_id = identifiers.send_request(&peer, context.request.clone());
                guard.record_outbound_request(request_id, context);
                request_ids.push(request_id);
            }
            guard.lost_connection(connection_id);
            for request_id in &request_ids {
                assert_eq!(
                    guard
                        .outbound_request_context(*request_id)
                        .unwrap()
                        .request_endpoints
                        .len(),
                    1,
                    "disconnect must retain issued attribution until terminal failure"
                );
            }
        }
        let mut buffered = VecDeque::new();
        for (position, request_id) in request_ids.into_iter().enumerate() {
            let mut dispatcher = SwarmActionDispatcher::Buffered(&mut buffered);
            handle_outbound_request_failure_with_dispatcher(
                &mut dispatcher,
                state.clone(),
                metrics.clone(),
                local_peer,
                &mut equix_builder,
                exclusions.clone(),
                peer,
                request_id,
                request_response::OutboundFailure::ConnectionClosed,
            )
            .await;
            let guard = state.lock().await;
            assert!(guard.outbound_request_context(request_id).is_none());
            if position == 0 {
                assert_eq!(guard.total_outbound_request_count(), 1);
                assert!(
                    guard.request_session_metadata_count() > 0,
                    "another retained request still owns this peer's session metadata"
                );
            } else {
                assert_eq!(guard.total_outbound_request_count(), 0);
                assert_eq!(guard.inflight_prefetch_count_for_peer(&peer), 0);
                assert_eq!(
                    guard.request_session_metadata_count(),
                    0,
                    "completed late failures cannot leave identity-keyed metadata behind"
                );
            }
        }
        assert!(
            buffered.is_empty(),
            "exhausted retry attempts must not schedule more work"
        );
    }
}

fn heaviest_chain_scry_result(height: u64, block_seed: u64) -> NounSlab {
    let mut slab = NounSlab::new();
    let block_id = tip5_tuple(&mut slab, block_seed);
    let height = Atom::new(&mut slab, height).as_noun();
    let tip = T(&mut slab, &[height, block_id]);
    let result = T(&mut slab, &[D(0), D(0), tip]);
    slab.set_root(result);
    slab
}

async fn reconcile_frontier_once(
    traffic: &traffic_cop::TrafficCop,
    state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    stall_warn_after: Duration,
) -> (frontier_reconcile::FrontierReconcileOutcome, bool) {
    let mut buffered = VecDeque::new();
    let outcome = frontier_reconcile::reconcile_seen_frontier_with_kernel(
        frontier_reconcile::FrontierReconcileReason::Periodic,
        traffic,
        state,
        metrics,
        &mut SwarmActionDispatcher::Buffered(&mut buffered),
        stall_warn_after,
    )
    .await
    .expect("reconciliation should succeed");
    let flush = matches!(
        buffered.pop_front(),
        Some(SwarmAction::FlushDeferredHeardBlocks)
    );
    assert!(buffered.is_empty());
    (outcome, flush)
}

#[tokio::test]
async fn effect_lag_dropping_heighted_seen_resyncs_frontier_and_releases_deferred_block() {
    use frontier_reconcile::{
        run_frontier_reconcile_task, triage_kernel_effect_recv, KernelEffectRecv,
    };
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    state.lock().await.first_negative = 10;
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![Some(heaviest_chain_scry_result(10, 30_010))],
        vec![PokeResult::Ack],
    )
    .await;
    let (_temp, app) = setup_nockapp("test-ker.jam").await;
    let base = app.get_handle();
    let (sender, receiver) = tokio::sync::broadcast::channel(4);
    let handle = NockAppHandle {
        io_sender: base.io_sender.clone(),
        effect_sender: Arc::new(sender),
        effect_receiver: Mutex::new(receiver),
        metrics: base.metrics.clone(),
        exit: base.exit.clone(),
    };
    handle
        .effect_sender
        .send(seen_block_effect_slab(30_010, 10))
        .unwrap();
    for seed in 0..8 {
        handle
            .effect_sender
            .send(seen_tx_effect_slab(40_000 + seed))
            .unwrap();
    }
    let lag = Arc::new(tokio::sync::Notify::new());
    assert!(matches!(
        triage_kernel_effect_recv(handle.next_effect().await, &metrics, &lag),
        KernelEffectRecv::Lagged
    ));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    for _ in 0..4 {
        let KernelEffectRecv::Effect(effect) =
            triage_kernel_effect_recv(handle.next_effect().await, &metrics, &lag)
        else {
            panic!("newest effects should survive");
        };
        handle_effect_with_dispatcher(
            effect,
            &mut SwarmActionDispatcher::Channel(&tx),
            vec![],
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            state.clone(),
            metrics.clone(),
            PeerExclusions::default(),
        )
        .await
        .unwrap();
    }
    let (block, _) = heard_block_fact_with_tx_ids(11, &[]);
    route_response_fact(
        PeerId::random(),
        block,
        &traffic.traffic,
        &metrics,
        &state,
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(state.lock().await.first_negative, 10);
    assert_eq!(state.lock().await.deferred_heard_block_heights(), vec![11]);
    assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 0);
    let task = tokio::spawn(run_frontier_reconcile_task(
        traffic.traffic.clone(),
        state.clone(),
        metrics.clone(),
        tx.clone(),
        lag,
        None,
    ));
    let action = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(matches!(action, SwarmAction::FlushDeferredHeardBlocks));
    assert_eq!(state.lock().await.first_negative, 11);
    let mut swarm = deferred_flush_test_swarm();
    let running = Arc::new(AtomicBool::new(false));
    let mut workers = TrackedJoinSet::new();
    dispatch_deferred_flush(
        &mut swarm, &tx, &running, &mut workers, &state, &metrics, &traffic.traffic,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), workers.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 1);
    assert!(state.lock().await.deferred_heard_block_heights().is_empty());
    assert!(!running.load(Ordering::Acquire));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn frontier_reconcile_only_moves_frontier_forward_and_flushes_ready_blocks() {
    use frontier_reconcile::FrontierReconcileOutcome;
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 2)));
    state.lock().await.first_negative = 10;
    let mut empty = NounSlab::new();
    let noun = T(&mut empty, &[D(0), D(0)]);
    empty.set_root(noun);
    let mut malformed = NounSlab::new();
    malformed.set_root(D(42));
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![
            Some(heaviest_chain_scry_result(10, 30_010)),
            Some(heaviest_chain_scry_result(10, 30_010)),
            Some(heaviest_chain_scry_result(5, 30_005)),
            Some(empty),
            Some(malformed),
            None,
        ],
        vec![],
    )
    .await;
    let peer = PeerId::random();
    {
        let mut guard = state.lock().await;
        for height in [11, 13] {
            let (fact, _) = heard_block_fact_with_tx_ids(height, &[]);
            let NockchainFact::HeardBlock(id, _) = &fact else {
                unreachable!()
            };
            assert!(guard.defer_heard_block(peer, height, id.clone(), fact));
        }
    }
    let (outcome, flush) =
        reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300)).await;
    assert!(matches!(
        outcome,
        FrontierReconcileOutcome::Advanced {
            previous_frontier: 10,
            frontier: 11,
            kernel_heaviest: 10,
        }
    ));
    assert!(flush);
    let ready = state.lock().await.take_ready_deferred_heard_block_entries();
    assert_eq!(ready.len(), 1);
    for entry in &ready {
        state
            .lock()
            .await
            .complete_deferred_heard_block_entry(entry);
    }
    for height in [10, 5] {
        assert_eq!(
            reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300),)
                .await,
            (
                FrontierReconcileOutcome::InSync {
                    frontier: 11,
                    kernel_heaviest: height
                },
                false
            )
        );
    }
    for _ in 0..3 {
        assert_eq!(
            reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300),)
                .await,
            (FrontierReconcileOutcome::Unavailable, false)
        );
    }
    assert_eq!(state.lock().await.first_negative, 11);
    assert_eq!(metrics.highest_block_height_seen.load(), 10.0);
    assert_eq!(state.lock().await.deferred_heard_block_heights(), vec![13]);
}

#[tokio::test]
async fn frontier_reconcile_flags_frontier_stalled_below_deferred_blocks() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    state.lock().await.first_negative = 11;
    let (fact, _) = heard_block_fact_with_tx_ids(12, &[]);
    let NockchainFact::HeardBlock(id, _) = &fact else {
        unreachable!()
    };
    state
        .lock()
        .await
        .defer_heard_block(PeerId::random(), 12, id.clone(), fact);
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![
            Some(heaviest_chain_scry_result(10, 30_010)),
            Some(heaviest_chain_scry_result(10, 30_010)),
            Some(heaviest_chain_scry_result(12, 30_012)),
        ],
        vec![],
    )
    .await;
    reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300)).await;
    assert_eq!(metrics.deferred_heights_above_stalled_frontier.load(), 0.0);
    reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::ZERO).await;
    assert_eq!(metrics.deferred_heights_above_stalled_frontier.load(), 1.0);
    reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::ZERO).await;
    assert_eq!(metrics.deferred_heights_above_stalled_frontier.load(), 0.0);
}

#[tokio::test]
async fn frontier_reconcile_periodic_only_and_lag_retry_without_periodic() {
    for periodic in [false, true] {
        let metrics = isolated_test_metrics();
        let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
        state.lock().await.first_negative = 10;
        let mut results = Vec::new();
        if !periodic {
            results.push(None);
        }
        results.push(Some(heaviest_chain_scry_result(10, 30_010)));
        let traffic =
            build_scripted_traffic_cop(DriverTranscript::default(), results, vec![]).await;
        let (fact, _) = heard_block_fact_with_tx_ids(11, &[]);
        let NockchainFact::HeardBlock(id, _) = &fact else {
            unreachable!()
        };
        state
            .lock()
            .await
            .defer_heard_block(PeerId::random(), 11, id.clone(), fact);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let lag = Arc::new(tokio::sync::Notify::new());
        if !periodic {
            lag.notify_one();
        }
        let task = tokio::spawn(frontier_reconcile::run_frontier_reconcile_task(
            traffic.traffic.clone(),
            state.clone(),
            metrics,
            tx,
            lag,
            periodic.then_some(Duration::from_millis(20)),
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap(),
            Some(SwarmAction::FlushDeferredHeardBlocks)
        ));
        task.abort();
        let _ = task.await;
        assert_eq!(state.lock().await.first_negative, 11);
        assert_eq!(
            traffic.peek_count.load(Ordering::SeqCst),
            if periodic { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn heaviest_chain_peek_path_is_served_by_kernel() {
    let live = build_live_traffic_cop(start_nockchain_app().await);
    let result = live
        .traffic
        .peek(None, kernel_io::heaviest_chain_scry_slab())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        kernel_io::heaviest_chain_height_from_scry(&result).unwrap(),
        None
    );
}

#[test]
fn heaviest_chain_scry_decodes_tip_height() {
    assert_eq!(
        kernel_io::heaviest_chain_height_from_scry(&heaviest_chain_scry_result(165_915, 30_010))
            .unwrap(),
        Some(165_915),
    );
    let mut malformed = NounSlab::new();
    let result = T(&mut malformed, &[D(0), D(0), D(7)]);
    malformed.set_root(result);
    assert!(kernel_io::heaviest_chain_height_from_scry(&malformed).is_err());
}

/// Keep kernel replies under test control while using the real traffic queues.
async fn controlled_frontier_traffic() -> (
    traffic_cop::TrafficCop,
    tokio::sync::mpsc::Receiver<IOAction>,
    TrackedJoinSet<Result<(), NockAppError>>,
) {
    let (_temp, app) = setup_nockapp("test-ker.jam").await;
    let base = app.get_handle();
    let (io_sender, receiver) = tokio::sync::mpsc::channel(16);
    let handle = NockAppHandle {
        io_sender,
        effect_sender: base.effect_sender.clone(),
        effect_receiver: Mutex::new(base.effect_sender.subscribe()),
        metrics: base.metrics.clone(),
        exit: base.exit.clone(),
    };
    let mut tasks = TrackedJoinSet::new();
    let traffic = traffic_cop::TrafficCop::new(handle, &mut tasks, Duration::from_secs(1));
    (traffic, receiver, tasks)
}

async fn next_controlled_kernel_action(
    receiver: &mut tokio::sync::mpsc::Receiver<IOAction>,
) -> IOAction {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("kernel action should arrive")
        .expect("kernel queue should remain open")
}

#[tokio::test]
async fn frontier_reconcile_retries_failed_peek_and_retains_inflight_notification() {
    let (traffic, mut kernel, _tasks) = controlled_frontier_traffic().await;
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    state.lock().await.first_negative = 10;
    let (fact, _) = heard_block_fact_with_tx_ids(12, &[]);
    let NockchainFact::HeardBlock(id, _) = &fact else {
        unreachable!()
    };
    state
        .lock()
        .await
        .defer_heard_block(PeerId::random(), 12, id.clone(), fact);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let lag = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn(frontier_reconcile::run_frontier_reconcile_task(
        traffic,
        state.clone(),
        metrics,
        tx,
        lag.clone(),
        None,
    ));
    lag.notify_one();
    let IOAction::Peek { result_channel, .. } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected tip peek");
    };
    // A dropped reply is a failed peek, not an empty chain. No second lag or
    // periodic timer is available to recover the consumed notification.
    drop(result_channel);
    let failed_at = tokio::time::Instant::now();
    let IOAction::Peek { result_channel, .. } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected delayed retry");
    };
    assert!(failed_at.elapsed() >= Duration::from_millis(900));
    assert_eq!(state.lock().await.first_negative, 10);
    // A new lag while this snapshot is in flight must survive its success.
    lag.notify_one();
    result_channel
        .send(Some(heaviest_chain_scry_result(10, 30_010)))
        .unwrap();
    let IOAction::Peek { result_channel, .. } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected another pass for in-flight lag");
    };
    result_channel
        .send(Some(heaviest_chain_scry_result(11, 30_011)))
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(SwarmAction::FlushDeferredHeardBlocks)
    ));
    assert_eq!(state.lock().await.first_negative, 12);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn frontier_reconcile_full_queue_releases_lock_and_cannot_regress_seen_gauge() {
    let (traffic, mut kernel, _tasks) = controlled_frontier_traffic().await;
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    state.lock().await.first_negative = 10;
    let (fact, _) = heard_block_fact_with_tx_ids(11, &[]);
    let NockchainFact::HeardBlock(id, _) = &fact else {
        unreachable!()
    };
    state
        .lock()
        .await
        .defer_heard_block(PeerId::random(), 11, id.clone(), fact);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    tx.send(SwarmAction::FlushDeferredHeardBlocks)
        .await
        .unwrap();
    let lag = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn(frontier_reconcile::run_frontier_reconcile_task(
        traffic,
        state.clone(),
        metrics.clone(),
        tx,
        lag.clone(),
        None,
    ));
    lag.notify_one();
    let IOAction::Peek { result_channel, .. } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected tip peek");
    };
    // Queue our mutex waiter behind reconciliation's acquisition, then prove
    // the state stays usable while its flush dispatch awaits a full queue.
    let guard = state.lock().await;
    result_channel
        .send(Some(heaviest_chain_scry_result(10, 30_010)))
        .unwrap();
    let_worker_park().await;
    drop(guard);
    let guard = tokio::time::timeout(Duration::from_secs(5), state.lock())
        .await
        .expect("full action queue must not retain state lock");
    assert_eq!(guard.first_negative, 11);
    drop(guard);
    let mut buffered = VecDeque::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        handle_effect_with_dispatcher(
            seen_block_effect_slab(30_020, 20),
            &mut SwarmActionDispatcher::Buffered(&mut buffered),
            vec![],
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            state.clone(),
            metrics.clone(),
            PeerExclusions::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(metrics.highest_block_height_seen.load(), 20.0);
    assert!(matches!(
        rx.recv().await,
        Some(SwarmAction::FlushDeferredHeardBlocks)
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(SwarmAction::FlushDeferredHeardBlocks)
    ));
    assert_eq!(state.lock().await.first_negative, 21);
    assert_eq!(metrics.highest_block_height_seen.load(), 20.0);
    // A stale snapshot arriving after a newer ordinary %seen also stays put.
    lag.notify_one();
    let IOAction::Peek { result_channel, .. } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected stale tip peek");
    };
    result_channel
        .send(Some(heaviest_chain_scry_result(5, 30_005)))
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap(),
        Some(SwarmAction::FlushDeferredHeardBlocks)
    ));
    assert_eq!(state.lock().await.first_negative, 21);
    assert_eq!(metrics.highest_block_height_seen.load(), 20.0);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn frontier_reconcile_does_not_create_prefetch_demand_or_peer_credit() {
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let peer = PeerId::random();
    let other = PeerId::random();
    let (fact, _) = heard_block_fact_with_tx_ids(11, &[]);
    let NockchainFact::HeardBlock(id, _) = &fact else {
        unreachable!()
    };
    {
        let mut guard = state.lock().await;
        guard.first_negative = 10;
        seed_connected_peer(&mut guard, peer, 900);
        seed_connected_peer(&mut guard, other, 901);
        let (tip_fact, _) = heard_block_fact_with_block_seed(20, 30_020, &[]);
        let NockchainFact::HeardBlock(tip_id, _) = tip_fact else {
            unreachable!()
        };
        guard.track_block_id_str_and_peer(tip_id.clone(), peer);
        guard.validate_and_untrack_block_sources(tip_id);
        assert!(guard.defer_heard_block_with_source(
            peer,
            11,
            id.clone(),
            fact,
            BlockSource::Prefetch,
        ));
    }
    let traffic = build_scripted_traffic_cop(
        DriverTranscript::default(),
        vec![
            Some(heaviest_chain_scry_result(20, 30_020)),
            Some(heaviest_chain_scry_result(20, 30_020)),
        ],
        vec![],
    )
    .await;
    assert!(
        !reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300),)
            .await
            .1
    );
    {
        let mut guard = state.lock().await;
        assert_eq!(guard.first_negative, 21);
        assert!(!guard.has_kernel_block_height_request(11));
        assert!(!guard.has_ready_deferred_heard_blocks());
        let selected = (0..2)
            .map(|_| {
                guard
                    .select_block_height_request_peer(vec![peer, other], &PeerExclusions::default())
                    .unwrap()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            selected,
            BTreeSet::from([peer, other]),
            "a height-only snapshot must not give the validated supplier acceptance priority"
        );
        guard.note_kernel_block_height_requested(11);
    }
    assert!(
        reconcile_frontier_once(&traffic.traffic, &state, &metrics, Duration::from_secs(300),)
            .await
            .1
    );
    assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deferred_flush_runs_off_driver_loop_and_keeps_height_order() {
    let (traffic, mut kernel, _tasks) = controlled_frontier_traffic().await;
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    {
        let mut guard = state.lock().await;
        guard.first_negative = 11;
        for height in [11, 12] {
            let (fact, _) = heard_block_fact_with_tx_ids(height, &[]);
            let NockchainFact::HeardBlock(id, _) = &fact else {
                unreachable!()
            };
            assert!(guard.defer_heard_block(PeerId::random(), height, id.clone(), fact));
        }
    }
    let mut swarm = deferred_flush_test_swarm();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let running = Arc::new(AtomicBool::new(false));
    let mut workers = TrackedJoinSet::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        dispatch_deferred_flush(
            &mut swarm, &tx, &running, &mut workers, &state, &metrics, &traffic,
        ),
    )
    .await
    .expect("action consumer must not wait for the kernel");
    let IOAction::Poke {
        poke, ack_channel, ..
    } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected heard-block 11");
    };
    assert_eq!(heard_block_height_from_fact_poke(&poke).unwrap(), 11);
    let first_ack = ack_channel;
    let mut buffered = VecDeque::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        handle_effect_with_dispatcher(
            seen_block_effect_slab(30_011, 11),
            &mut SwarmActionDispatcher::Buffered(&mut buffered),
            vec![],
            false,
            PrefetchConfig::disabled(),
            runtime_limits_from_config(&LIBP2P_CONFIG),
            state.clone(),
            metrics.clone(),
            PeerExclusions::default(),
        ),
    )
    .await
    .expect("effect handling must not wait for the held poke")
    .unwrap();
    assert_eq!(state.lock().await.first_negative, 12);
    assert!(matches!(
        buffered.pop_front(),
        Some(SwarmAction::FlushDeferredHeardBlocks)
    ));
    assert!(buffered.is_empty());
    tokio::time::timeout(
        Duration::from_secs(5),
        dispatch_deferred_flush(
            &mut swarm, &tx, &running, &mut workers, &state, &metrics, &traffic,
        ),
    )
    .await
    .expect("coalesced action must remain nonblocking");
    assert!(
        kernel.try_recv().is_err(),
        "height 12 cannot overtake held height 11"
    );
    first_ack.send(PokeResult::Ack).unwrap();
    let IOAction::Poke {
        poke, ack_channel, ..
    } = next_controlled_kernel_action(&mut kernel).await
    else {
        panic!("expected heard-block 12");
    };
    assert_eq!(heard_block_height_from_fact_poke(&poke).unwrap(), 12);
    ack_channel.send(PokeResult::Ack).unwrap();
    tokio::time::timeout(Duration::from_secs(5), workers.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        workers.join_next().await.is_none(),
        "only the release worker owns delivery"
    );
    assert!(
        kernel.try_recv().is_err(),
        "each height must be delivered once"
    );
    assert!(state.lock().await.deferred_heard_block_heights().is_empty());
    assert!(!running.load(Ordering::Acquire));
    assert!(rx.try_recv().is_err());
}

/// Genesis supplies a genuine accepted kernel tip without mining. Its replay
/// is already frontier-ready at height zero: this checks positive tip repair
/// and real-kernel flush delivery, not successor-height eligibility.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frontier_reconcile_live_accepted_genesis_resyncs_and_flushes_replay() {
    use frontier_reconcile::{FrontierReconcileOutcome, FrontierReconcileReason};

    let live = build_live_traffic_cop(start_nockchain_app().await);
    seed_fakenet_pre_genesis(&live.effect_handle).await;
    send_born(&live.effect_handle).await;
    let metrics = isolated_test_metrics();
    let state = Arc::new(Mutex::new(P2PState::new(metrics.clone(), 100)));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let peer = PeerId::random();
    let genesis = fake_genesis_block_message_fact();
    let NockchainFact::HeardBlock(genesis_id, _) = &genesis else {
        unreachable!()
    };
    let genesis_id = genesis_id.clone();
    route_response_fact(peer, genesis.clone(), &live.traffic, &metrics, &state, &tx)
        .await
        .expect("genuine fakenet genesis should reach the live kernel");
    assert_eq!(metrics.responses_acked_heard_block.fetch_add(0), 1);

    // No kernel effects are handed to the driver state. Acceptance must be
    // established by the actual positive scry, rather than inferred from Ack.
    let tip = live
        .traffic
        .peek(None, kernel_io::heaviest_chain_scry_slab())
        .await
        .expect("live tip peek should complete")
        .expect("live scry result");
    assert_eq!(
        kernel_io::heaviest_chain_height_from_scry(&tip).unwrap(),
        Some(0)
    );
    {
        let mut guard = state.lock().await;
        assert_eq!(guard.first_negative, 0);
        assert!(guard.defer_heard_block(peer, 0, genesis_id.clone(), genesis));
        assert!(
            guard.has_ready_deferred_heard_blocks(),
            "genesis replay is already eligible; recovery supplies its flush wakeup"
        );
    }
    assert!(rx.try_recv().is_err());
    let outcome = frontier_reconcile::reconcile_seen_frontier_with_kernel(
        FrontierReconcileReason::EffectLag,
        &live.traffic,
        &state,
        &metrics,
        &mut SwarmActionDispatcher::Channel(&tx),
        Duration::from_secs(300),
    )
    .await
    .expect("real accepted-tip reconciliation should succeed");
    assert_eq!(
        outcome,
        FrontierReconcileOutcome::Advanced {
            previous_frontier: 0,
            frontier: 1,
            kernel_heaviest: 0,
        }
    );
    assert_eq!(metrics.seen_frontier_resync_total.fetch_add(0), 1);
    let action = rx.recv().await.expect("reconcile should queue a flush");
    assert!(matches!(action, SwarmAction::FlushDeferredHeardBlocks));
    let mut swarm = deferred_flush_test_swarm();
    let running = Arc::new(AtomicBool::new(false));
    let mut workers = TrackedJoinSet::new();
    let mut equix_builder = equix::EquiXBuilder::new();
    let mut pending = BTreeMap::new();
    process_swarm_action(
        action,
        &mut swarm,
        &tx,
        &running,
        &mut workers,
        &state,
        &metrics,
        &PeerExclusions::default(),
        &mut equix_builder,
        &mut pending,
        runtime_limits_from_config(&LIBP2P_CONFIG),
        &live.traffic,
    )
    .await
    .expect("production action consumer should start release flush worker");
    tokio::time::timeout(Duration::from_secs(30), workers.join_next())
        .await
        .expect("live-kernel flush should finish")
        .expect("one release worker")
        .expect("worker must not panic")
        .expect("valid genesis replay must flush without error");
    assert!(workers.join_next().await.is_none());
    assert_eq!(
        metrics.responses_acked_heard_block.fetch_add(0),
        2,
        "the release worker must actually poke the real kernel once, not only drain state"
    );
    assert!(state.lock().await.deferred_heard_block_heights().is_empty());
    assert!(!running.load(Ordering::Acquire));
    assert!(rx.try_recv().is_err());
    let tip = live
        .traffic
        .peek(None, kernel_io::heaviest_chain_scry_slab())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        kernel_io::heaviest_chain_height_from_scry(&tip).unwrap(),
        Some(0)
    );
    assert_eq!(state.lock().await.first_negative, 1);
    println!(
        "test_evidence live_accepted_genesis kernel_tip=0 frontier=1 resyncs=1 \
         real_kernel_block_acks=2 replay_flushes=1 deferred_remaining=0 \
         successor_eligibility_proven=false"
    );
}
