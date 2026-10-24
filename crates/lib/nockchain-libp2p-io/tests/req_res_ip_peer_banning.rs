use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

use libp2p::{Multiaddr, PeerId};
use nockchain_libp2p_io::config::LibP2PConfig;
use nockchain_libp2p_io::test_support::ReqResIpPeerBanningProbe;

fn ip_peer_config() -> LibP2PConfig {
    LibP2PConfig {
        ip_hygiene_enabled: true,
        address_cooldown_secs: 60,
        ip_exclusion_secs: 120,
        ip_extended_exclusion_secs: 240,
        evidence_window_secs: 60,
        ip_exclusion_history_secs: 300,
        permission_denied_cooldown_secs: 30,
        wrong_peer_id_ip_threshold: 2,
        dial_failure_ip_threshold: 2,
        same_ip_kad_entry_threshold: 2,
        max_auto_exclusion_secs: 240,
        max_exclusion_entries: 32,
        request_peer_cooldown_secs: 60,
        fail2ban_on_temp_exclusion: true,
        ..LibP2PConfig::default()
    }
}

fn quic_addr(ip: Ipv4Addr, port: u16, peer: PeerId) -> Multiaddr {
    format!("/ip4/{ip}/udp/{port}/quic-v1/p2p/{peer}")
        .parse()
        .expect("test multiaddr should parse")
}

fn peer_set(peers: &[PeerId]) -> BTreeSet<PeerId> {
    peers.iter().copied().collect()
}

#[test]
fn req_res_ip_peer_banning_escalates_same_ip_peer_rotation() {
    let shared_ip = Ipv4Addr::new(198, 51, 100, 7);
    let other_ip = Ipv4Addr::new(203, 0, 113, 9);
    let peer_a = PeerId::random();
    let peer_b = PeerId::random();
    let fresh_same_ip_peer = PeerId::random();
    let other_ip_peer = PeerId::random();
    let addr_a = quic_addr(shared_ip, 31_001, peer_a);
    let addr_b = quic_addr(shared_ip, 31_002, peer_b);
    let fresh_same_ip_addr = quic_addr(shared_ip, 31_099, fresh_same_ip_peer);
    let other_ip_addr = quic_addr(other_ip, 32_001, other_ip_peer);

    let mut probe = ReqResIpPeerBanningProbe::new(ip_peer_config());
    probe.track_peer_connection(peer_a, addr_a.clone());
    probe.track_peer_connection(peer_b, addr_b.clone());
    probe.track_peer_connection(fresh_same_ip_peer, fresh_same_ip_addr.clone());
    probe.track_peer_connection(other_ip_peer, other_ip_addr.clone());

    let first = probe.record_peer_misbehavior(&addr_a, peer_a);
    assert!(first.address_cooldown_created);
    assert!(!first.ip_exclusion_created);
    assert_eq!(first.active_ip_exclusions, 0);
    assert!(probe.is_address_excluded(&addr_a, Some(peer_a)));
    assert!(!probe.is_ip_excluded(&IpAddr::V4(shared_ip)));

    let second = probe.record_peer_misbehavior(&addr_b, peer_b);
    assert!(second.address_cooldown_created);
    assert!(second.ip_exclusion_created);
    assert!(second.fail2ban);
    assert_eq!(second.active_ip_exclusions, 1);
    assert_eq!(second.active_address_cooldowns, 2);
    assert!(probe.is_ip_excluded(&IpAddr::V4(shared_ip)));
    assert!(probe.is_address_excluded(&fresh_same_ip_addr, Some(fresh_same_ip_peer)));
    assert!(!probe.is_ip_excluded(&IpAddr::V4(other_ip)));
    assert!(!probe.is_address_excluded(&other_ip_addr, Some(other_ip_peer)));

    let selected = probe.select_request_peers(vec![peer_a, fresh_same_ip_peer, other_ip_peer], 3);
    assert_eq!(selected, vec![other_ip_peer]);
}

#[test]
fn req_res_ip_peer_banning_peer_request_cooldown_prefers_healthy_peer() {
    let cooled_peer = PeerId::random();
    let healthy_peer = PeerId::random();
    let cooled_addr = quic_addr(Ipv4Addr::new(198, 51, 100, 20), 33_001, cooled_peer);
    let healthy_addr = quic_addr(Ipv4Addr::new(203, 0, 113, 20), 33_002, healthy_peer);

    let mut probe = ReqResIpPeerBanningProbe::new(ip_peer_config());
    probe.track_peer_connection(cooled_peer, cooled_addr);
    probe.track_peer_connection(healthy_peer, healthy_addr);

    assert!(probe.record_peer_request_failure(cooled_peer));
    assert!(probe.is_peer_request_cooled_down(&cooled_peer));

    let selected = probe.select_request_peers(vec![cooled_peer, healthy_peer], 2);
    assert_eq!(selected, vec![healthy_peer]);

    let fallback = probe.select_request_peers(vec![cooled_peer], 1);
    assert_eq!(fallback, vec![cooled_peer]);

    probe.record_peer_request_success(&cooled_peer);
    assert!(!probe.is_peer_request_cooled_down(&cooled_peer));
    let recovered = probe.select_request_peers(vec![cooled_peer, healthy_peer], 2);
    assert_eq!(peer_set(&recovered), peer_set(&[cooled_peer, healthy_peer]));
}
