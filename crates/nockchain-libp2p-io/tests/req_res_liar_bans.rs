use std::collections::BTreeSet;

use libp2p::PeerId;
use nockchain_libp2p_io::test_support::ReqResLiarBanProbe;

fn peer_set(peers: &[PeerId]) -> BTreeSet<PeerId> {
    peers.iter().copied().collect()
}

#[test]
fn req_res_liar_bans_only_peers_tracked_for_bad_block_id() {
    let bad_peer_a = PeerId::random();
    let bad_peer_b = PeerId::random();
    let good_peer = PeerId::random();
    let bad_block_id = String::from("bad-block-id");
    let good_block_id = String::from("good-block-id");

    let mut probe = ReqResLiarBanProbe::new();
    probe.track_block_id_from_peer(bad_block_id.clone(), bad_peer_a);
    probe.track_block_id_from_peer(bad_block_id.clone(), bad_peer_b);
    probe.track_block_id_from_peer(good_block_id.clone(), good_peer);
    probe.defer_dummy_heard_block(bad_peer_a, 9, bad_block_id.clone());
    probe.defer_dummy_heard_block(good_peer, 10, good_block_id.clone());
    assert_eq!(probe.deferred_heard_block_total(), 2);

    let banned = probe.mark_liar_block_id(&bad_block_id);
    assert_eq!(peer_set(&banned), peer_set(&[bad_peer_a, bad_peer_b]));
    assert!(!probe.is_tracking_peer(bad_peer_a));
    assert!(!probe.is_tracking_peer(bad_peer_b));
    assert!(probe.is_tracking_peer(good_peer));
    assert_eq!(
        probe.block_ids_for_peer(good_peer),
        vec![good_block_id.clone()]
    );
    assert_eq!(probe.deferred_heard_block_total(), 1);
    assert!(!probe.has_deferred_block_at_height(9));
    assert!(probe.has_deferred_block_at_height(10));
}

#[test]
fn req_res_liar_bans_repeated_block_id_report_is_idempotent() {
    let bad_peer = PeerId::random();
    let bad_block_id = String::from("bad-block-id-repeat");

    let mut probe = ReqResLiarBanProbe::new();
    probe.track_block_id_from_peer(bad_block_id.clone(), bad_peer);
    probe.defer_dummy_heard_block(bad_peer, 12, bad_block_id.clone());

    let first_ban = probe.mark_liar_block_id(&bad_block_id);
    assert_eq!(first_ban, vec![bad_peer]);
    assert!(!probe.is_tracking_peer(bad_peer));
    assert_eq!(probe.deferred_heard_block_total(), 0);

    let second_ban = probe.mark_liar_block_id(&bad_block_id);
    assert!(second_ban.is_empty());
    assert_eq!(probe.deferred_heard_block_total(), 0);
}
