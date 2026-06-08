use libp2p::PeerId;
use nockchain_libp2p_io::test_support::{
    jam_block_by_height_request, request_pow_verifies_at, solve_authenticated_gossip,
    solve_batch_request, solve_block_by_height_request, BatchRequestItem, NockchainRequest,
};
use serde_bytes::ByteBuf;

#[test]
fn req_res_pow_binds_singleton_to_peer_pair_and_message() {
    let sender = PeerId::random();
    let receiver = PeerId::random();
    let other_sender = PeerId::random();
    let other_receiver = PeerId::random();
    let request = solve_block_by_height_request(&sender, &receiver, 42);

    assert!(request_pow_verifies_at(&request, &receiver, &sender));
    assert!(!request_pow_verifies_at(&request, &receiver, &other_sender));
    assert!(!request_pow_verifies_at(&request, &other_receiver, &sender));

    let NockchainRequest::Request { pow, nonce, .. } = request else {
        panic!("singleton solver should produce a singleton request");
    };
    let tampered = NockchainRequest::Request {
        pow,
        nonce,
        message: ByteBuf::from(jam_block_by_height_request(43)),
    };
    assert!(!request_pow_verifies_at(&tampered, &receiver, &sender));
}

#[test]
fn req_res_pow_binds_batch_to_peer_pair_items_and_order() {
    let sender = PeerId::random();
    let receiver = PeerId::random();
    let other_sender = PeerId::random();
    let other_receiver = PeerId::random();
    let items = vec![
        BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(b"first".to_vec()),
        },
        BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(b"second".to_vec()),
        },
    ];
    let request =
        solve_batch_request(&sender, &receiver, items).expect("batch PoW should be solved");

    assert!(request_pow_verifies_at(&request, &receiver, &sender));
    assert!(!request_pow_verifies_at(&request, &receiver, &other_sender));
    assert!(!request_pow_verifies_at(&request, &other_receiver, &sender));

    let NockchainRequest::BatchRequest { pow, nonce, items } = request else {
        panic!("batch solver should produce a batch request");
    };
    let mut tampered_items = items.clone();
    tampered_items[0].message = ByteBuf::from(b"tampered".to_vec());
    let tampered_payload = NockchainRequest::BatchRequest {
        pow,
        nonce,
        items: tampered_items,
    };
    assert!(!request_pow_verifies_at(
        &tampered_payload, &receiver, &sender
    ));

    let mut reversed_items = items;
    reversed_items.reverse();
    let reordered = NockchainRequest::BatchRequest {
        pow,
        nonce,
        items: reversed_items,
    };
    assert!(!request_pow_verifies_at(&reordered, &receiver, &sender));
}

#[test]
fn req_res_pow_leaves_gossip_out_of_powork() {
    let receiver = PeerId::random();
    let sender = PeerId::random();
    let request = NockchainRequest::Gossip {
        message: ByteBuf::from(b"legacy gossip has no powork".to_vec()),
    };

    assert!(request_pow_verifies_at(&request, &receiver, &sender));
}

#[test]
fn req_res_pow_binds_authenticated_gossip_to_peer_pair_and_message() {
    let sender = PeerId::random();
    let receiver = PeerId::random();
    let other_sender = PeerId::random();
    let other_receiver = PeerId::random();
    let request = solve_authenticated_gossip(&sender, &receiver, b"authenticated gossip");

    assert!(request_pow_verifies_at(&request, &receiver, &sender));
    assert!(!request_pow_verifies_at(&request, &receiver, &other_sender));
    assert!(!request_pow_verifies_at(&request, &other_receiver, &sender));

    let NockchainRequest::AuthenticatedGossip { pow, nonce, .. } = request else {
        panic!("authenticated gossip solver should produce authenticated gossip");
    };
    let tampered = NockchainRequest::AuthenticatedGossip {
        pow,
        nonce,
        message: ByteBuf::from(b"tampered gossip".to_vec()),
    };
    assert!(!request_pow_verifies_at(&tampered, &receiver, &sender));
}
