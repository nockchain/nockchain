use libp2p::PeerId;
use nockchain_libp2p_io::test_support::{
    deferred_heard_block_per_peer_bytes_cap, deferred_heard_block_per_peer_cap, jam_raw_tx_request,
    solve_authenticated_gossip, BatchRequestItem, NockchainRequest, ReplayProbeAdmission,
    ReqResStateBoundsProbe,
};
use serde_bytes::ByteBuf;

fn batch_request(nonce: u64, seed: u64) -> NockchainRequest {
    NockchainRequest::BatchRequest {
        pow: [0; 16],
        nonce,
        items: vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(jam_raw_tx_request(seed)),
        }],
    }
}

#[test]
fn req_res_state_bounds_replay_survives_reconnect_and_obeys_window() {
    let mut probe = ReqResStateBoundsProbe::new(2);
    let first = batch_request(1, 11_001);
    let second = batch_request(2, 11_002);
    let third = batch_request(3, 11_003);

    let first_connection =
        probe.connect_peer("/ip4/127.0.0.1/tcp/4001".parse().expect("valid addr"));
    assert_eq!(
        probe
            .admit_replay_key(&first)
            .expect("first replay key should derive"),
        ReplayProbeAdmission::Accepted
    );
    assert_eq!(
        probe
            .admit_replay_key(&first)
            .expect("duplicate replay key should derive"),
        ReplayProbeAdmission::Replayed
    );

    assert_eq!(probe.disconnect(first_connection), 0);
    assert!(
        probe
            .has_replay_key(&first)
            .expect("replay key should still derive"),
        "disconnect must not clear the exact replay window for peer {}",
        probe.peer_id()
    );

    let _second_connection =
        probe.connect_peer("/ip4/127.0.0.1/tcp/4002".parse().expect("valid addr"));
    assert_eq!(
        probe
            .admit_replay_key(&first)
            .expect("reconnected replay key should derive"),
        ReplayProbeAdmission::Replayed
    );

    assert_eq!(
        probe
            .admit_replay_key(&second)
            .expect("second replay key should derive"),
        ReplayProbeAdmission::Accepted
    );
    assert_eq!(
        probe
            .admit_replay_key(&third)
            .expect("third replay key should derive"),
        ReplayProbeAdmission::Accepted
    );
    assert!(
        !probe
            .has_replay_key(&first)
            .expect("evicted replay key should derive"),
        "oldest replay key should be evicted once the per-peer window is full",
    );
    assert!(probe
        .has_replay_key(&second)
        .expect("second replay key should derive"));
    assert!(probe
        .has_replay_key(&third)
        .expect("third replay key should derive"));

    probe.remove_peer();
    assert!(
        !probe
            .has_replay_key(&second)
            .expect("removed replay key should derive"),
        "explicit peer removal should clear retained replay keys",
    );
}

#[test]
fn req_res_state_bounds_replay_tracks_authenticated_gossip() {
    let mut probe = ReqResStateBoundsProbe::new(8);
    let sender = PeerId::random();
    let receiver = PeerId::random();
    let request = solve_authenticated_gossip(&sender, &receiver, b"authenticated replay key");

    assert_eq!(
        probe
            .admit_replay_key(&request)
            .expect("authenticated gossip replay key should derive"),
        ReplayProbeAdmission::Accepted
    );
    assert_eq!(
        probe
            .admit_replay_key(&request)
            .expect("authenticated gossip duplicate key should derive"),
        ReplayProbeAdmission::Replayed
    );
    assert!(probe
        .has_replay_key(&request)
        .expect("authenticated gossip retained key should derive"));
}

#[test]
fn req_res_state_bounds_future_block_defer_stays_bounded_per_peer() {
    let mut probe = ReqResStateBoundsProbe::new(8);
    let cap = deferred_heard_block_per_peer_cap();
    let tail_height = cap as u64 + 7;

    for height in 0..=tail_height {
        assert!(probe.defer_dummy_heard_block(height, format!("future-block-{height}")));
    }

    assert_eq!(probe.deferred_heard_block_total(), cap);
    assert!(!probe.has_deferred_block_at_height(0));
    assert!(probe.has_deferred_block_at_height(tail_height));
    assert!(
        !probe.defer_dummy_heard_block(tail_height, format!("future-block-{tail_height}")),
        "duplicate future block id at the same height should not grow the buffer",
    );
    assert_eq!(probe.deferred_heard_block_total(), cap);
}

#[test]
fn req_res_state_bounds_future_block_defer_obeys_byte_budget() {
    let mut probe = ReqResStateBoundsProbe::new(8);
    assert!(probe.defer_dummy_heard_block(0, String::from("future-block-a")));
    let retained_per_block = probe.deferred_heard_block_total_bytes();
    assert!(retained_per_block > 0);
    assert!(retained_per_block < deferred_heard_block_per_peer_bytes_cap());

    let per_peer_cap = retained_per_block * 2;
    probe.set_deferred_heard_block_byte_caps(per_peer_cap, per_peer_cap * 4);

    assert!(probe.defer_dummy_heard_block(1, String::from("future-block-b")));
    assert!(probe.defer_dummy_heard_block(2, String::from("future-block-c")));

    assert_eq!(probe.deferred_heard_block_total(), 2);
    assert!(probe.deferred_heard_block_total_bytes() <= per_peer_cap);
    assert!(!probe.has_deferred_block_at_height(0));
    assert!(probe.has_deferred_block_at_height(1));
    assert!(probe.has_deferred_block_at_height(2));
}
