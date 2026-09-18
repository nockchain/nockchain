mod harness;

use std::time::Duration;

use futures::StreamExt;
use harness::{
    build_test_peer, connect_peers, default_test_config, drain_pending_events,
    expected_common_protocol, expected_outbound_generation, init_tracing,
    run_request_until_disconnect_cleanup_failure, run_round_trip, run_round_trip_observing_request,
    wait_for_listen_addr, Transcript, TranscriptGuard,
};
use libp2p::request_response;
use libp2p::swarm::SwarmEvent;
use nockchain_network::config::LibP2PConfig;
use nockchain_network::test_support::{
    is_block_by_height_message, jam_block_by_height_request, jam_raw_tx_request,
    request_pow_verifies_at, solve_authenticated_gossip, BatchErrorClass, BatchRequestItem,
    BatchResultItem, BatchResultStatus, NockchainRequest, NockchainResponse, ReqResTestEvent,
    ResponseEnvelope,
};
use serde_bytes::ByteBuf;

fn recorded_protocols(peer: &harness::TestPeer, operation: &'static str) -> Vec<String> {
    peer.protocol_trace
        .snapshot()
        .into_iter()
        .filter(|entry| entry.actor == peer.name && entry.operation == operation)
        .map(|entry| entry.protocol)
        .collect()
}

fn batch_request_item_ids(request: &NockchainRequest) -> Vec<u32> {
    match request {
        NockchainRequest::BatchRequest { items, .. } => {
            items.iter().map(|item| item.item_id).collect()
        }
        other => panic!("expected BatchRequest, got {other:?}"),
    }
}

async fn assert_batch_ack_round_trip(
    requester: &mut harness::TestPeer,
    responder: &mut harness::TestPeer,
    responder_peer_id: libp2p::PeerId,
    item_id: u32,
    message: &'static [u8],
    transcript: &Transcript,
) {
    let response = run_round_trip(
        requester,
        responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id,
                message: ByteBuf::from(message.to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );
}

struct ConcurrentBatchRoundTripObservation {
    first_observed_request: NockchainRequest,
    second_observed_request: NockchainRequest,
    first_response: NockchainResponse,
    second_response: NockchainResponse,
    response_arrival_order: Vec<request_response::OutboundRequestId>,
}

#[allow(clippy::too_many_arguments)]
async fn run_concurrent_batch_round_trip_reversing_responses(
    requester: &mut harness::TestPeer,
    responder: &mut harness::TestPeer,
    responder_peer_id: libp2p::PeerId,
    first_request: NockchainRequest,
    first_response: NockchainResponse,
    second_request: NockchainRequest,
    second_response: NockchainResponse,
    transcript: &Transcript,
) -> ConcurrentBatchRoundTripObservation {
    struct PendingInboundResponse {
        request_id: request_response::InboundRequestId,
        channel: request_response::ResponseChannel<NockchainResponse>,
    }

    let first_expected_item_ids = batch_request_item_ids(&first_request);
    let second_expected_item_ids = batch_request_item_ids(&second_request);

    let first_request_id = requester
        .swarm
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, first_request.clone());
    transcript.record(
        requester.name,
        format!(
            "sent overlapping request_id={first_request_id:?} item_ids={first_expected_item_ids:?} toward {responder_peer_id}"
        ),
    );

    let second_request_id = requester
        .swarm
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer_id, second_request.clone());
    transcript.record(
        requester.name,
        format!(
            "sent overlapping request_id={second_request_id:?} item_ids={second_expected_item_ids:?} toward {responder_peer_id}"
        ),
    );

    tokio::time::timeout(Duration::from_secs(15), async {
        let mut first_observed_request = None;
        let mut second_observed_request = None;
        let mut first_pending = None;
        let mut second_pending = None;
        let mut first_received_response = None;
        let mut second_received_response = None;
        let mut response_arrival_order = Vec::new();
        let mut reversed_responses_sent = false;

        loop {
            tokio::select! {
                event = responder.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Request { request_id, request, channel } => {
                                    let item_ids = batch_request_item_ids(&request);
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "received overlapping request_id={request_id:?} from {peer} item_ids={item_ids:?}"
                                        ),
                                    );

                                    if item_ids == first_expected_item_ids {
                                        first_observed_request = Some(request);
                                        first_pending = Some(PendingInboundResponse { request_id, channel });
                                    } else if item_ids == second_expected_item_ids {
                                        second_observed_request = Some(request);
                                        second_pending = Some(PendingInboundResponse { request_id, channel });
                                    } else {
                                        panic!("unexpected concurrent batch request item_ids={item_ids:?}");
                                    }

                                    if !reversed_responses_sent
                                        && first_pending.is_some()
                                        && second_pending.is_some()
                                    {
                                        let first_pending = first_pending
                                            .take()
                                            .expect("first pending response should exist");
                                        let second_pending = second_pending
                                            .take()
                                            .expect("second pending response should exist");

                                        responder
                                            .swarm
                                            .behaviour_mut()
                                            .request_response
                                            .send_response(second_pending.channel, second_response.clone())
                                            .expect("second overlapping response should send");
                                        transcript.record(
                                            responder.name,
                                            format!(
                                                "sent overlapping response for request_id={:?} item_ids={:?}",
                                                second_pending.request_id,
                                                second_expected_item_ids
                                            ),
                                        );

                                        responder
                                            .swarm
                                            .behaviour_mut()
                                            .request_response
                                            .send_response(first_pending.channel, first_response.clone())
                                            .expect("first overlapping response should send");
                                        transcript.record(
                                            responder.name,
                                            format!(
                                                "sent overlapping response for request_id={:?} item_ids={:?}",
                                                first_pending.request_id,
                                                first_expected_item_ids
                                            ),
                                        );

                                        reversed_responses_sent = true;
                                    }
                                }
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "unexpected overlapping response_id={request_id:?} shape={response:?}"
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(responder.name, format!("overlap loop saw {other:?}")),
                    }
                }
                event = requester.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "received overlapping response_id={request_id:?} from {peer} shape={response:?}"
                                        ),
                                    );
                                    response_arrival_order.push(request_id);
                                    if request_id == first_request_id {
                                        first_received_response = Some(response);
                                    } else if request_id == second_request_id {
                                        second_received_response = Some(response);
                                    } else {
                                        panic!("unexpected overlapping outbound request_id={request_id:?}");
                                    }

                                    if let (
                                        Some(first_observed_request),
                                        Some(second_observed_request),
                                        Some(first_response),
                                        Some(second_response),
                                    ) = (
                                        first_observed_request.clone(),
                                        second_observed_request.clone(),
                                        first_received_response.clone(),
                                        second_received_response.clone(),
                                    ) {
                                        return ConcurrentBatchRoundTripObservation {
                                            first_observed_request,
                                            second_observed_request,
                                            first_response,
                                            second_response,
                                            response_arrival_order,
                                        };
                                    }
                                }
                                request_response::Message::Request { request_id, request, .. } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected inbound overlapping request_id={request_id:?} shape={request:?}"
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(requester.name, format!("overlap loop saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("concurrent round-trip timeout")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_batch_round_trip_emits_transcript() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2/gen2 batch round-trip expected_generation={} expected_common_protocol={:?}",
            expected_outbound_generation(&requester_config),
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(b"req-res-gen2-batch".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 7,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 7,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
}

/// A single gen2 batch response containing all four BatchResultStatus
/// variants (Result, Ack, NotFound, Error) round-trips through the transport
/// with per-item status and error fields intact.  After the mixed response the
/// connection stays healthy for a follow-up request-response cycle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_mixed_status_batch_result_round_trip() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2/gen2 mixed-status batch result expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Build a 4-item batch request.  The responder will reply with one item
    // per BatchResultStatus variant: Result, Ack, NotFound, Error.
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"mixed-result".to_vec()),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"mixed-ack".to_vec()),
            },
            BatchRequestItem {
                item_id: 3,
                message: ByteBuf::from(b"mixed-not-found".to_vec()),
            },
            BatchRequestItem {
                item_id: 4,
                message: ByteBuf::from(b"mixed-error".to_vec()),
            },
        ],
    };

    let mixed_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_block(
                    String::from("mixed-block-id"),
                    b"mixed-result-payload",
                )),
            },
            BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 3,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 4,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
        ],
    };

    let observed = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        mixed_response.clone(),
        &transcript,
    )
    .await;

    // Assert exact per-item status/error mapping survived transport.
    assert_eq!(observed, mixed_response);

    // Verify per-item fields individually for clarity on failure.
    if let NockchainResponse::BatchResult { results } = &observed {
        assert_eq!(results.len(), 4, "expected 4 batch result items");

        assert_eq!(results[0].item_id, 1);
        assert_eq!(results[0].status, BatchResultStatus::Result);
        assert!(results[0].error.is_none());
        assert!(results[0].envelope.is_some());

        assert_eq!(results[1].item_id, 2);
        assert_eq!(results[1].status, BatchResultStatus::Ack);
        assert!(results[1].error.is_none());
        assert!(results[1].envelope.is_none());

        assert_eq!(results[2].item_id, 3);
        assert_eq!(results[2].status, BatchResultStatus::NotFound);
        assert!(results[2].error.is_none());
        assert!(results[2].envelope.is_none());

        assert_eq!(results[3].item_id, 4);
        assert_eq!(results[3].status, BatchResultStatus::Error);
        assert_eq!(results[3].error, Some(BatchErrorClass::Backpressure));
        assert!(results[3].envelope.is_none());
    } else {
        panic!("expected BatchResult response");
    }

    // Follow-up round-trip proves the connection is still healthy after
    // processing a mixed-status batch.
    let followup = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 10,
                message: ByteBuf::from(b"mixed-followup".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 10,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        followup,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 10,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"),
        "mixed-status test must negotiate gen2; transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn req_res_gen2_concurrent_batch_requests_on_one_connection_route_by_request_id() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "gen2_concurrent_batch_requests_one_connection");
    transcript.record(
        "scenario",
        format!(
            "gen2/gen2 concurrent batch requests on one live connection expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let first_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![
            BatchRequestItem {
                item_id: 101,
                message: ByteBuf::from(jam_block_by_height_request(11)),
            },
            BatchRequestItem {
                item_id: 102,
                message: ByteBuf::from(jam_block_by_height_request(12)),
            },
        ],
    };
    let second_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![
            BatchRequestItem {
                item_id: 201,
                message: ByteBuf::from(jam_raw_tx_request(21)),
            },
            BatchRequestItem {
                item_id: 202,
                message: ByteBuf::from(jam_raw_tx_request(22)),
            },
        ],
    };
    let first_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 101,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
            BatchResultItem {
                item_id: 102,
                status: BatchResultStatus::NotFound,
                error: None,
                envelope: None,
            },
        ],
    };
    let second_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 201,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
            BatchResultItem {
                item_id: 202,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            },
        ],
    };

    let observation = run_concurrent_batch_round_trip_reversing_responses(
        &mut requester,
        &mut responder,
        responder_peer_id,
        first_request.clone(),
        first_response.clone(),
        second_request.clone(),
        second_response.clone(),
        &transcript,
    )
    .await;

    assert_eq!(
        batch_request_item_ids(&observation.first_observed_request),
        batch_request_item_ids(&first_request),
    );
    assert_eq!(
        batch_request_item_ids(&observation.second_observed_request),
        batch_request_item_ids(&second_request),
    );
    assert_eq!(observation.first_response, first_response);
    assert_eq!(observation.second_response, second_response);
    assert_eq!(
        observation.response_arrival_order.len(),
        2,
        "expected both overlapping responses to arrive"
    );
    assert_ne!(
        observation.response_arrival_order[0], observation.response_arrival_order[1],
        "overlapping responses should map to distinct outbound request ids"
    );

    let follow_up = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 301,
                message: ByteBuf::from(jam_block_by_height_request(31)),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 301,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        follow_up,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 301,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"),
        "concurrent batch test must negotiate gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("sent overlapping request_id="),
        "transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("received overlapping response_id="),
        "transcript:\n{rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_repeated_backpressure_batches_keep_transport_live() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2/gen2 repeated backpressure batch results keep transport live expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let repeated_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"bp-round-item-1".to_vec()),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"bp-round-item-2".to_vec()),
            },
            BatchRequestItem {
                item_id: 3,
                message: ByteBuf::from(b"bp-round-item-3".to_vec()),
            },
            BatchRequestItem {
                item_id: 4,
                message: ByteBuf::from(b"bp-round-item-4".to_vec()),
            },
        ],
    };
    let repeated_backpressure = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
            BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
            BatchResultItem {
                item_id: 3,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
            BatchResultItem {
                item_id: 4,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
        ],
    };

    for round in 1..=3 {
        transcript.record("scenario", format!("repeated backpressure round {round}"));
        let observed = run_round_trip(
            &mut requester,
            &mut responder,
            responder_peer_id,
            repeated_request.clone(),
            repeated_backpressure.clone(),
            &transcript,
        )
        .await;
        assert_eq!(
            observed, repeated_backpressure,
            "round {round} should preserve per-item backpressure over transport"
        );
    }

    assert_batch_ack_round_trip(
        &mut requester, &mut responder, responder_peer_id, 10, b"backpressure-followup",
        &transcript,
    )
    .await;

    let gen2 = LibP2PConfig::req_res_protocol_version().to_string();
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should keep emitting requests on gen2 across repeated pressure",
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone(), gen2.clone()],
        "responder should keep accepting repeated pressure rounds on gen2",
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone(), gen2.clone()],
        "responder should keep returning batch results on gen2",
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should keep receiving repeated pressure responses on gen2",
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("repeated backpressure round 3"),
        "transcript should show repeated pressure rounds; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("shape=batch-result"),
        "transcript should record repeated batch-result responses; transcript:\n{rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_authenticated_gossip_round_trip() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2 authenticated gossip expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();
    let request = solve_authenticated_gossip(
        &requester_peer_id, &responder_peer_id, b"gen2-authenticated-gossip",
    );

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let (observed_request, response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;

    assert_eq!(response, NockchainResponse::Ack { acked: true });
    assert!(matches!(
        observed_request,
        NockchainRequest::AuthenticatedGossip { .. }
    ));
    assert!(request_pow_verifies_at(
        &observed_request, &responder_peer_id, &requester_peer_id,
    ));

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("shape=authenticated-gossip"));
    assert!(rendered.contains("shape=ack"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_large_response_payload_round_trip_succeeds() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let item_count = 16usize;
    let payload_len = 2048usize;
    let request = gen2_request(item_count);
    let response = gen2_result(item_count, payload_len);
    let response_bytes = encoded_response_bytes(&response);
    assert!(
        response_bytes > 16 * 1024,
        "encoded gen2 response should be meaningfully large: {response_bytes}"
    );

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2/gen2 large response payload expected_common_protocol={:?} item_count={item_count} payload_len={payload_len} response_bytes={response_bytes}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observed = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        response.clone(),
        &transcript,
    )
    .await;

    assert_eq!(observed, response);

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("response_bytes="));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_disconnect_during_inflight_request_recovers_after_reconnect() {
    init_tracing();

    let requester_config = LibP2PConfig {
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "disconnect in-flight gen2 request then reconnect expected_common_protocol={:?}",
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

    let observation = run_request_until_disconnect_cleanup_failure(
        &mut requester,
        &mut responder,
        responder_peer_id,
        requester_peer_id,
        gen2_request(2),
        &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;

    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let recovery_response = gen2_result(2, 128);
    let observed = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        gen2_request(2),
        recovery_response.clone(),
        &transcript,
    )
    .await;

    assert_eq!(observed, recovery_response);

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("connection closed with"));
    assert!(rendered.matches("shape=batch-request").count() >= 4);
    assert!(rendered.matches("shape=batch-result").count() >= 2);
}

// ---------------------------------------------------------------------------
// BlockByHeight gen2 batching coverage
// ---------------------------------------------------------------------------

/// With both peers gen2-capable, bounded `BlockByHeight` requests should
/// round-trip over the gen2 transport. This keeps the transport-level coverage
/// aligned with the production driver now that the guarded block-response
/// budget is active again.
///
/// The test verifies three things:
/// 1. A jam-encoded BlockByHeight message is correctly identified by the same
///    decode path the driver uses (`is_block_by_height_message`).
/// 2. A two-item block batch round-trips successfully over gen2 transport.
/// 3. The transcript shows the expected gen2 batch request/result wire shapes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_block_by_height_batch_round_trip_uses_gen2() {
    init_tracing();

    let block_message = jam_block_by_height_request(42);
    assert!(
        is_block_by_height_message(&block_message),
        "jam_block_by_height_request(42) must decode as BlockByHeight"
    );
    let second_block_message = jam_block_by_height_request(43);

    let gen2_config = LibP2PConfig {
        ..default_test_config()
    };
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "block_by_height_gen2_batch");
    transcript.record(
        "scenario",
        format!(
            "BlockByHeight gen2 batch path expected_common_protocol={:?}",
            expected_common_protocol(&gen2_config, &gen2_config),
        ),
    );

    let mut requester = build_test_peer("requester", gen2_config.clone());
    let mut responder = build_test_peer("responder", gen2_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(block_message.clone()),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(second_block_message),
                },
            ],
        },
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 1,
                    status: BatchResultStatus::Result,
                    error: None,
                    envelope: Some(ResponseEnvelope::heard_block(
                        String::from("block-42"),
                        b"block-42-payload",
                    )),
                },
                BatchResultItem {
                    item_id: 2,
                    status: BatchResultStatus::Result,
                    error: None,
                    envelope: Some(ResponseEnvelope::heard_block(
                        String::from("block-43"),
                        b"block-43-payload",
                    )),
                },
            ],
        },
        &transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 1,
                    status: BatchResultStatus::Result,
                    error: None,
                    envelope: Some(ResponseEnvelope::heard_block(
                        String::from("block-42"),
                        b"block-42-payload",
                    )),
                },
                BatchResultItem {
                    item_id: 2,
                    status: BatchResultStatus::Result,
                    error: None,
                    envelope: Some(ResponseEnvelope::heard_block(
                        String::from("block-43"),
                        b"block-43-payload",
                    )),
                },
            ],
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"),
        "BlockByHeight batch must use gen2 protocol; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("shape=batch-request"),
        "block request should use the batch request shape; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("shape=batch-result"),
        "block response should use the batch result shape; transcript:\n{rendered}"
    );
}

fn encoded_response_bytes(response: &NockchainResponse) -> usize {
    cbor4ii::serde::to_vec(Vec::new(), response)
        .expect("response should encode")
        .len()
}

fn gen2_request(item_count: usize) -> NockchainRequest {
    NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: (0..item_count)
            .map(|idx| BatchRequestItem {
                item_id: idx as u32 + 1,
                message: ByteBuf::from(format!("latency-gen2-{idx}").into_bytes()),
            })
            .collect(),
    }
}

fn gen2_result(item_count: usize, payload_len: usize) -> NockchainResponse {
    NockchainResponse::BatchResult {
        results: (0..item_count)
            .map(|idx| BatchResultItem {
                item_id: idx as u32 + 1,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    format!("latency-tx-{idx}"),
                    vec![0xCD; payload_len],
                )),
            })
            .collect(),
    }
}
