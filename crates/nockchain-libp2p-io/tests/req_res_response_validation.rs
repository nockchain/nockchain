mod harness;

use harness::{
    build_test_peer, connect_peers, default_test_config, disconnect_peers,
    expected_common_protocol, init_tracing, run_round_trip_observing_request, wait_for_listen_addr,
    Transcript, TranscriptGuard,
};
use nockchain_libp2p_io::config::LibP2PConfig;
use nockchain_libp2p_io::test_support::{
    build_selective_batch_retry_requests, bundled_block_for_height,
    jam_block_range_with_txs_request, jam_heard_tx_response, jam_raw_tx_request,
    validated_batch_response_retry_item_ids, BatchRequestItem, BatchResultItem, BatchResultStatus,
    NockchainRequest, NockchainResponse, ResponseEnvelope,
};
use serde_bytes::ByteBuf;

fn gen2_config() -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    }
}

fn batch_request_item_ids(request: &NockchainRequest) -> Vec<u32> {
    match request {
        NockchainRequest::BatchRequest { items, .. } => {
            items.iter().map(|item| item.item_id).collect()
        }
        other => panic!("expected BatchRequest, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_response_validation_retries_mismatched_and_missing_raw_tx_items() {
    init_tracing();

    let requester_config = gen2_config();
    let responder_config = gen2_config();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "raw_tx_response_validation_retry");
    transcript.record(
        "scenario",
        format!(
            "raw-tx response validation over gen2 expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let initial_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 91,
        items: vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(jam_raw_tx_request(91_001)),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(jam_raw_tx_request(91_002)),
            },
        ],
    };
    let invalid_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 1,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(
                String::from("wrong-tx-id"),
                jam_heard_tx_response(91_101, 32),
            )),
        }],
    };

    let (observed_request, observed_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        initial_request.clone(),
        invalid_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(observed_request, initial_request);
    assert_eq!(observed_response, invalid_response);

    let retry_item_ids =
        validated_batch_response_retry_item_ids(&observed_request, &observed_response)
            .expect("response validation should classify retry items");
    transcript.record(
        "driver",
        format!("semantic response validation scheduled retry item_ids={retry_item_ids:?}"),
    );
    assert_eq!(retry_item_ids, vec![1, 2]);

    let retry_requests = build_selective_batch_retry_requests(
        &requester_peer_id, &responder_peer_id, observed_request, 0, &retry_item_ids,
    )
    .expect("selective retry request should build from validation result");
    assert_eq!(retry_requests.len(), 1);
    assert_eq!(retry_requests[0].item_ids, vec![1, 2]);

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let retry_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::NotFound,
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
    let (observed_retry_request, observed_retry_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        retry_requests[0].request.clone(),
        retry_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(batch_request_item_ids(&observed_retry_request), vec![1, 2]);
    assert_eq!(observed_retry_response, retry_response);

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("semantic response validation scheduled retry item_ids=[1, 2]"));
    assert!(rendered.contains("disconnecting from"));
    assert_eq!(rendered.matches("shape=batch-request").count(), 4);
    assert_eq!(rendered.matches("shape=batch-result").count(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_response_validation_classifies_non_contiguous_block_range() {
    init_tracing();

    let requester_config = gen2_config();
    let responder_config = gen2_config();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "block_range_response_validation");
    transcript.record(
        "scenario",
        format!(
            "block-range response validation over gen2 expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let initial_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 92,
        items: vec![BatchRequestItem {
            item_id: 7,
            message: ByteBuf::from(
                jam_block_range_with_txs_request(50, 2).expect("range request should encode"),
            ),
        }],
    };
    let non_contiguous_range = ResponseEnvelope::heard_block_range_with_txs(vec![
        bundled_block_for_height(50, &[]),
        bundled_block_for_height(52, &[]),
    ]);
    let invalid_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(non_contiguous_range),
        }],
    };

    let (observed_request, observed_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        initial_request.clone(),
        invalid_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(observed_request, initial_request);
    assert_eq!(observed_response, invalid_response);

    let retry_item_ids =
        validated_batch_response_retry_item_ids(&observed_request, &observed_response)
            .expect("range response validation should classify retry items");
    transcript.record(
        "driver",
        format!("semantic range validation scheduled retry item_ids={retry_item_ids:?}"),
    );
    assert_eq!(retry_item_ids, vec![7]);

    let retry_requests = build_selective_batch_retry_requests(
        requester.swarm.local_peer_id(),
        &responder_peer_id,
        observed_request,
        0,
        &retry_item_ids,
    )
    .expect("range selective retry decision should not error");
    assert!(retry_requests.is_empty());

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("semantic range validation scheduled retry item_ids=[7]"));
    assert_eq!(rendered.matches("shape=batch-request").count(), 2);
    assert_eq!(rendered.matches("shape=batch-result").count(), 2);
}
