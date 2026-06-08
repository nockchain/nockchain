mod harness;

use std::fs;
use std::time::{Duration, Instant};

use futures::StreamExt;
use harness::{
    build_test_peer, build_test_peer_with_keypair, connect_peers, default_test_config,
    disconnect_peers, drain_pending_events, expected_common_protocol, expected_outbound_generation,
    init_tracing, run_request_until_disconnect_cleanup_failure, run_round_trip,
    run_round_trip_observing_request, wait_for_listen_addr, Transcript, TranscriptGuard,
};
use libp2p::request_response;
use libp2p::swarm::SwarmEvent;
use nockchain_libp2p_io::config::LibP2PConfig;
use nockchain_libp2p_io::test_support::{
    build_selective_batch_retry_requests, build_unsupported_protocol_fallback_replay,
    build_unsupported_protocol_fallback_requests, is_block_by_height_message,
    jam_block_by_height_request, jam_heard_tx_response, jam_raw_tx_request,
    request_pow_verifies_at, request_response_protocol_summary, solve_authenticated_gossip,
    BatchErrorClass, BatchRequestItem, BatchResultItem, BatchResultStatus, NockchainRequest,
    NockchainResponse, ReqResProtocolSupportSummary, ReqResTestEvent, ResponseEnvelope,
};
use serde::Serialize;
use serde_bytes::ByteBuf;

fn recorded_protocols(peer: &harness::TestPeer, operation: &'static str) -> Vec<String> {
    peer.protocol_trace
        .snapshot()
        .into_iter()
        .filter(|entry| entry.actor == peer.name && entry.operation == operation)
        .map(|entry| entry.protocol)
        .collect()
}

fn request_message_bytes(request: &NockchainRequest) -> Vec<u8> {
    match request {
        NockchainRequest::Request { message, .. } => message.clone().into_vec(),
        other => panic!("expected singleton Request, got {other:?}"),
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

async fn assert_gen1_result_round_trip(
    requester: &mut harness::TestPeer,
    responder: &mut harness::TestPeer,
    responder_peer_id: libp2p::PeerId,
    request_message: &'static [u8],
    response_message: &'static [u8],
    transcript: &Transcript,
) {
    let response = run_round_trip(
        requester,
        responder,
        responder_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(request_message.to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(response_message.to_vec()),
        },
        transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::Result {
            message: ByteBuf::from(response_message.to_vec()),
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
async fn req_res_gen1_only_e2e_round_trip_emits_transcript() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen1/gen1 gossip round-trip expected_generation={} expected_common_protocol={:?}",
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
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"req-res-gen1-e2e".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;

    assert_eq!(response, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("received response_id="));
}

/// Gen1-only peers exchanging a data Request (not Gossip).  The existing gen1
/// test only covers Gossip messages; this verifies that
/// NockchainRequest::Request round-trips work between two gen1-only peers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen1_only_data_request_round_trip() {
    init_tracing();

    let config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "gen1_data_request");
    transcript.record(
        "scenario",
        format!(
            "gen1/gen1 data Request round-trip expected_common_protocol={:?}",
            expected_common_protocol(&config, &config),
        ),
    );

    let mut requester = build_test_peer("requester", config.clone());
    let mut responder = build_test_peer("responder", config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"gen1-data-request".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-data-response".to_vec()),
        },
        &transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-data-response".to_vec()),
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "gen1-only peers must negotiate gen1; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("shape=request"),
        "transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=result"), "transcript:\n{rendered}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_batch_round_trip_emits_transcript() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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

    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();
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
async fn req_res_gen2_batching_keeps_gossip_singleton() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2 batch then gossip expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let batch_response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(b"gen2-batch-before-gossip".to_vec()),
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
        batch_response,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 7,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let gossip_response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"gen2-gossip-after-batch".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(gossip_response, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=gossip"));
    assert!(rendered.contains("shape=ack"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_authenticated_gossip_round_trip() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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
async fn req_res_gen2_sender_gen1_only_responder_reconnects_cleanly() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2-preferred requester talking to gen1-only responder expected_common_protocol={:?}",
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

    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"req-res-gen2-fallback-first".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(first, NockchainResponse::Ack { acked: true });

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;

    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"req-res-gen2-fallback-second".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(second, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("connection closed with"));
}

/// Explicit experiment mode: a node with `accept_enabled=false` and
/// `send_enabled=true` must keep its own outbound requests on gen2 while
/// forcing reverse-direction requests back to gen1 because it does not accept
/// inbound gen2 streams.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_outbound_only_sender_reconnects_and_keeps_gen1_inbound() {
    init_tracing();

    let sender_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let full_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "outbound_only_reconnect");
    transcript.record(
        "scenario",
        format!(
            "outbound-only sender reconnects: sender_to_full={:?} full_to_sender={:?}",
            expected_common_protocol(&sender_config, &full_config),
            expected_common_protocol(&full_config, &sender_config),
        ),
    );

    let mut sender = build_test_peer("sender", sender_config.clone());
    let mut full = build_test_peer("full", full_config.clone());
    let sender_peer_id = *sender.swarm.local_peer_id();
    let full_peer_id = *full.swarm.local_peer_id();

    let _sender_addr = wait_for_listen_addr(&mut sender, &transcript).await;
    let full_addr = wait_for_listen_addr(&mut full, &transcript).await;
    connect_peers(&mut sender, &mut full, &full_addr, &transcript).await;

    assert_batch_ack_round_trip(
        &mut sender, &mut full, full_peer_id, 1, b"outbound-only-phase1", &transcript,
    )
    .await;

    disconnect_peers(
        &mut sender, &mut full, full_peer_id, sender_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut sender, &mut full, &full_addr, &transcript).await;

    assert_batch_ack_round_trip(
        &mut sender, &mut full, full_peer_id, 2, b"outbound-only-phase2", &transcript,
    )
    .await;

    assert_gen1_result_round_trip(
        &mut full, &mut sender, sender_peer_id, b"full-to-outbound-only",
        b"full-to-outbound-only-response", &transcript,
    )
    .await;

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    assert_eq!(
        recorded_protocols(&sender, "write_request"),
        vec![gen2.clone(), gen2.clone()],
        "outbound-only sender must renegotiate gen2 after reconnect"
    );
    assert_eq!(
        recorded_protocols(&full, "read_request"),
        vec![gen2.clone(), gen2.clone()],
        "full peer must keep reading outbound-only requests on gen2"
    );
    assert_eq!(
        recorded_protocols(&full, "write_request"),
        vec![gen1.clone()],
        "full peer must not advertise inbound gen2 support against outbound-only sender"
    );
    assert_eq!(
        recorded_protocols(&sender, "read_request"),
        vec![gen1.clone()],
        "reverse request to outbound-only sender must arrive over gen1"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_full=Some(\"/nockchain-2-req-res\")"),
        "outbound-only outbound path should prefer gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("full_to_sender=Some(\"/nockchain-1-req-res\")"),
        "reverse direction must negotiate gen1 because outbound-only sender does not accept gen2; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_disconnect_during_inflight_request_recovers_after_reconnect() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_protocol_renegotiation_upgrades_after_responder_restart() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_gen1_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };
    let responder_gen2_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "restart responder with same peer identity, before={:?} after={:?}",
            expected_common_protocol(&requester_config, &responder_gen1_config),
            expected_common_protocol(&requester_config, &responder_gen2_config),
        ),
    );

    let responder_keypair = libp2p::identity::Keypair::generate_ed25519();
    let mut requester = build_test_peer("requester", requester_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let mut responder = build_test_peer_with_keypair(
        "responder",
        responder_gen1_config.clone(),
        responder_keypair.clone(),
    );
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"req-res-regen-first".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(first, NockchainResponse::Ack { acked: true });

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(responder);

    let mut responder = build_test_peer_with_keypair(
        "responder",
        responder_gen2_config.clone(),
        responder_keypair,
    );
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 99,
                message: ByteBuf::from(b"req-res-regen-second".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 99,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 99,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(rendered.contains("before=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("after=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
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
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
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

// ---------------------------------------------------------------------------
// Nous upgrade-path scenarios (bd-dyz epic)
// ---------------------------------------------------------------------------

/// bd-5s3: The explicit accept-only rollout stage uses accept_enabled=true,
/// send_enabled=false.  A gen2 sender must be able to send a BatchRequest to
/// these accept-only peers and receive a valid BatchResult over gen2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_sender_to_accept_only_responder_succeeds() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    // The accept-only stage: accept gen2 inbound, do NOT send gen2 outbound.
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2 sender -> accept-only responder \
             expected_generation={} expected_common_protocol={:?}",
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
            items: vec![
                BatchRequestItem {
                    item_id: 1,
                    message: ByteBuf::from(b"accept-only-item-1".to_vec()),
                },
                BatchRequestItem {
                    item_id: 2,
                    message: ByteBuf::from(b"accept-only-item-2".to_vec()),
                },
            ],
        },
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 1,
                    status: BatchResultStatus::Ack,
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
                    status: BatchResultStatus::Ack,
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
        }
    );

    let rendered = transcript.render();
    // Accept-only responder should negotiate gen2 inbound, so the protocol
    // should be /nockchain-2-req-res (not gen1).
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"),
        "accept-only responder must negotiate gen2 inbound; transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
}

/// bd-r5q: An accept-only responder must renegotiate gen2 inbound after a
/// reconnect while still keeping outbound requests on gen1 when
/// `send_enabled=false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_accept_only_responder_reconnects_and_keeps_gen1_outbound() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "accept-only responder reconnects: sender_to_responder={:?} responder_to_sender={:?}",
            expected_common_protocol(&requester_config, &responder_config),
            expected_common_protocol(&responder_config, &requester_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"accept-only-reconnect-phase1".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        first,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"accept-only-reconnect-phase2".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let reverse = run_round_trip(
        &mut responder,
        &mut requester,
        requester_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"accept-only-outbound-gen1".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"accept-only-outbound-gen1-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        reverse,
        NockchainResponse::Result {
            message: ByteBuf::from(b"accept-only-outbound-gen1-response".to_vec()),
        }
    );

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone(), gen2.clone()],
        "gen2 sender should keep renegotiating gen2 after reconnect"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen2.clone(), gen2.clone()],
        "accept-only responder should keep reading inbound requests on gen2 after reconnect"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_request"),
        vec![gen1.clone()],
        "accept-only responder must keep outbound requests on gen1 when send is disabled"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_request"),
        vec![gen1.clone()],
        "reverse request from the accept-only responder should arrive over gen1"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_responder=Some(\"/nockchain-2-req-res\")"),
        "accept-only inbound path should prefer gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("responder_to_sender=Some(\"/nockchain-1-req-res\")"),
        "accept-only outbound path should stay gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen1_requester_to_accept_only_responder_negotiates_gen1() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen1 requester -> accept-only responder expected_common_protocol={:?}",
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
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"gen1-requester-accept-only".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-requester-accept-only-response".to_vec()),
        },
        &transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-requester-accept-only-response".to_vec()),
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "gen1 requester should stay on gen1 with accept-only responder; transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

/// bd-x4d: Two peers running the shipped Nous default
/// (`accept_enabled=true`, `send_enabled=false`) must still register inbound-
/// only gen2 while negotiating gen1 in both outbound directions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_two_accept_only_peers_keep_gen1_bidirectionally() {
    init_tracing();

    let config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();
    let expected_registration = vec![
        ReqResProtocolSupportSummary {
            protocol: gen1.clone(),
            inbound: true,
            outbound: true,
        },
        ReqResProtocolSupportSummary {
            protocol: gen2.clone(),
            inbound: true,
            outbound: false,
        },
    ];
    let protocol_summary = request_response_protocol_summary(&config);
    assert_eq!(protocol_summary, expected_registration);

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "accept-only <-> accept-only lane \
             left_to_right={:?} right_to_left={:?} registration={protocol_summary:?}",
            expected_common_protocol(&config, &config),
            expected_common_protocol(&config, &config),
        ),
    );

    let mut left = build_test_peer("left", config.clone());
    let mut right = build_test_peer("right", config.clone());
    let right_peer_id = *right.swarm.local_peer_id();
    let left_peer_id = *left.swarm.local_peer_id();

    let _left_addr = wait_for_listen_addr(&mut left, &transcript).await;
    let right_addr = wait_for_listen_addr(&mut right, &transcript).await;
    connect_peers(&mut left, &mut right, &right_addr, &transcript).await;

    assert_gen1_result_round_trip(
        &mut left, &mut right, right_peer_id, b"accept-only-left-to-right",
        b"accept-only-left-to-right-response", &transcript,
    )
    .await;
    assert_gen1_result_round_trip(
        &mut right, &mut left, left_peer_id, b"accept-only-right-to-left",
        b"accept-only-right-to-left-response", &transcript,
    )
    .await;

    assert_eq!(
        recorded_protocols(&left, "write_request"),
        vec![gen1.clone()],
        "left peer must keep outbound traffic on gen1 in the accept-only stage"
    );
    assert_eq!(
        recorded_protocols(&right, "write_request"),
        vec![gen1.clone()],
        "right peer must keep outbound traffic on gen1 in the accept-only stage"
    );
    assert_eq!(
        recorded_protocols(&left, "read_request"),
        vec![gen1.clone()],
        "left peer must receive reverse outbound traffic on gen1"
    );
    assert_eq!(
        recorded_protocols(&right, "read_request"),
        vec![gen1.clone()],
        "right peer must receive reverse outbound traffic on gen1"
    );
    assert_eq!(
        recorded_protocols(&left, "write_response"),
        vec![gen1.clone()],
        "left peer must answer reverse requests on gen1"
    );
    assert_eq!(
        recorded_protocols(&right, "write_response"),
        vec![gen1.clone()],
        "right peer must answer reverse requests on gen1"
    );
    assert_eq!(
        recorded_protocols(&left, "read_response"),
        vec![gen1.clone()],
        "left peer must receive its response on gen1"
    );
    assert_eq!(
        recorded_protocols(&right, "read_response"),
        vec![gen1.clone()],
        "right peer must receive its response on gen1"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("left_to_right=Some(\"/nockchain-1-req-res\")"),
        "left outbound should negotiate gen1; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("right_to_left=Some(\"/nockchain-1-req-res\")"),
        "right outbound should negotiate gen1; transcript:\n{rendered}"
    );
    assert!(rendered.matches("shape=request").count() >= 4);
    assert!(rendered.matches("shape=result").count() >= 4);
}

/// bd-5yd: A gen2-preferred sender connected to a gen1-only peer negotiates
/// gen1 and can still exchange gen1 data requests (not just Gossip).  The
/// existing fallback test only covers Gossip; this verifies that
/// NockchainRequest::Request messages work over the degraded path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_preferred_sender_falls_back_to_gen1_data_request() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2-preferred sender -> gen1-only responder with data Request \
             expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Send a gen1 data Request (not Gossip) — this is what the driver would
    // decompose a BatchRequest into when falling back to gen1.
    let response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"gen1-fallback-data-request".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-fallback-data-response".to_vec()),
        },
        &transcript,
    )
    .await;

    assert_eq!(
        response,
        NockchainResponse::Result {
            message: ByteBuf::from(b"gen1-fallback-data-response".to_vec()),
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "gen1-only responder must negotiate gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

/// The live req-res harness negotiates gen1 directly with a legacy peer, so it
/// cannot surface the driver's internal `UnsupportedProtocols` queue on its
/// own. This test closes that gap by using the shipped fallback builder to
/// derive the ordered singleton replay from one synthetic gen2 batch, then
/// proving those replayed requests succeed end-to-end on the real two-swarm
/// harness and leave the connection healthy for follow-up traffic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_driver_fallback_replay_decomposes_batch_into_ordered_gen1_singletons() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "driver_fallback_replay");
    transcript.record(
        "scenario",
        format!(
            "driver fallback replay for gen2 batch -> gen1-only responder expected_common_protocol={:?}",
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

    let fallback_heights = [11u64, 22, 33];
    let original_items = fallback_heights
        .iter()
        .enumerate()
        .map(|(idx, height)| BatchRequestItem {
            item_id: idx as u32 + 1,
            message: ByteBuf::from(jam_block_by_height_request(*height)),
        })
        .collect::<Vec<_>>();
    let replay = build_unsupported_protocol_fallback_replay(
        requester_peer_id,
        responder_peer_id,
        original_items.clone(),
    )
    .expect("driver fallback replay should build");
    transcript.record(
        "scenario",
        format!(
            "fallback_replay_metric_total={} original_batch_items={}",
            replay.fallback_metric_total,
            original_items.len(),
        ),
    );

    assert_eq!(replay.fallback_metric_total, original_items.len() as u64);
    assert_eq!(replay.fallback_requests.len(), original_items.len());

    for ((idx, height), request) in fallback_heights
        .iter()
        .enumerate()
        .zip(replay.fallback_requests.iter())
    {
        let NockchainRequest::Request { message, .. } = request else {
            panic!("driver fallback must replay singleton requests");
        };
        assert_eq!(
            message.as_ref(),
            original_items[idx].message.as_ref(),
            "fallback replay must preserve original item ordering and payload bytes",
        );
        assert!(
            is_block_by_height_message(message.as_ref()),
            "fallback replay item {idx} should remain a block-by-height request",
        );
        transcript.record(
            "scenario",
            format!("replaying fallback item_id={} height={height}", idx + 1),
        );

        let expected_response = NockchainResponse::Result {
            message: ByteBuf::from(format!("fallback-height-{height}-response").into_bytes()),
        };
        let observed = run_round_trip(
            &mut requester,
            &mut responder,
            responder_peer_id,
            request.clone(),
            expected_response.clone(),
            &transcript,
        )
        .await;
        assert_eq!(observed, expected_response);
    }

    let follow_up = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(jam_raw_tx_request(9_999)),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"post-fallback-health-check".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        follow_up,
        NockchainResponse::Result {
            message: ByteBuf::from(b"post-fallback-health-check".to_vec()),
        }
    );

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let expected_protocols = vec![gen1.clone(); fallback_heights.len() + 1];
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        expected_protocols,
        "requester should emit only ordered gen1 singleton requests during fallback replay",
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen1.clone(); fallback_heights.len() + 1],
        "legacy responder should receive fallback replay on gen1 only",
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen1.clone(); fallback_heights.len() + 1],
        "legacy responder should return gen1 singleton responses for the replayed items",
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen1.clone(); fallback_heights.len() + 1],
        "requester should receive gen1 singleton responses for replayed fallback items",
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "fallback replay must run over the legacy gen1 protocol; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("fallback_replay_metric_total=3"),
        "transcript should record fallback evidence relevant to rollout; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("replaying fallback item_id=1 height=11"),
        "transcript should show ordered singleton replay details; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("shape=request"),
        "transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=result"), "transcript:\n{rendered}");
}

/// bd-2rj: A requester that was previously gen1-only enables gen2 send,
/// reconnects to a gen2-capable responder, and successfully negotiates gen2.
/// This is the reverse of the existing responder-restart renegotiation test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_protocol_renegotiation_upgrades_after_requester_restart() {
    init_tracing();

    let requester_gen1_config = LibP2PConfig {
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };
    let requester_gen2_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "requester restarts with gen2 enabled, same identity before={:?} after={:?}",
            expected_common_protocol(&requester_gen1_config, &responder_config),
            expected_common_protocol(&requester_gen2_config, &responder_config),
        ),
    );

    let requester_keypair = libp2p::identity::Keypair::generate_ed25519();
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();
    let mut requester = build_test_peer_with_keypair(
        "requester",
        requester_gen1_config.clone(),
        requester_keypair.clone(),
    );
    let requester_peer_id = *requester.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Phase 1: gen1 — requester prefers gen1 outbound
    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"requester-upgrade-phase1".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(first, NockchainResponse::Ack { acked: true });

    // Disconnect and restart requester with gen2 enabled
    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(requester);

    let mut requester = build_test_peer_with_keypair(
        "requester",
        requester_gen2_config.clone(),
        requester_keypair,
    );
    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Phase 2: gen2 — requester now prefers gen2 outbound
    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 42,
                message: ByteBuf::from(b"requester-upgrade-phase2".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 42,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 42,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(rendered.contains("before=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("after=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
}

/// bd-4hg: A node that was sending gen2 disables send_enabled, reconnects,
/// and correctly reverts to gen1-only outbound.  The responder is still
/// gen2-capable, but the requester's gen1-first outbound ordering must win.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_rollback_reverts_outbound_to_gen1() {
    init_tracing();

    let gen2_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let rollback_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "requester rolls back gen2 send: before={:?} after={:?}",
            expected_common_protocol(&gen2_config, &responder_config),
            expected_common_protocol(&rollback_config, &responder_config),
        ),
    );

    let requester_keypair = libp2p::identity::Keypair::generate_ed25519();
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();
    let mut requester =
        build_test_peer_with_keypair("requester", gen2_config.clone(), requester_keypair.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Phase 1: gen2 active — BatchRequest round-trip succeeds
    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"rollback-phase1".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        first,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // Disconnect and restart requester with send disabled (the rollback lever)
    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(requester);

    let mut requester =
        build_test_peer_with_keypair("requester", rollback_config.clone(), requester_keypair);
    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // Phase 2: rolled back — gen1 Gossip round-trip over gen1 negotiation
    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"rollback-phase2-gen1".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(second, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(
        rendered.contains("before=Some(\"/nockchain-2-req-res\")"),
        "phase 1 should negotiate gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("after=Some(\"/nockchain-1-req-res\")"),
        "phase 2 (rollback) should negotiate gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
}

/// bd-hat: A gen1-only peer (accept_enabled=false) connected to a gen2
/// sender: protocol negotiation degrades to gen1, and a BatchRequest sent over
/// gen1 negotiation still arrives (since the CBOR codec is shared).  Verify
/// the connection stays healthy for subsequent gen1 traffic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_batch_to_gen1_only_peer_negotiates_gen1() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2 sender -> gen1-only peer: BatchRequest negotiates gen1 \
             expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // A BatchRequest sent when protocol negotiation selects gen1 — it still
    // arrives because the CBOR codec is version-agnostic.  In production,
    // a true gen1-only binary wouldn't have the BatchRequest variant, so the
    // driver decomposes to gen1 singletons; that path is unit-tested in
    // test_build_unsupported_protocol_fallback_contexts_*.
    let response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"gen1-negotiated-batch".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
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
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // Verify a gen1 Gossip also works — connection is healthy after the batch.
    let gossip = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"gen1-follow-up".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(gossip, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "gen1-only responder must negotiate gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=gossip"));
}

/// The raw req-res transcript harness uses a version-agnostic CBOR codec, so
/// the production fallback path must be injected from the driver side rather
/// than discovered automatically by protocol negotiation alone. This scenario
/// starts from one outbound gen2 batch, reuses the driver's unsupported-
/// protocol decomposition helper, and then proves the live degraded transport
/// emits ordered gen1 singleton exchanges over `/nockchain-1-req-res`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_batch_fallback_decomposes_to_ordered_gen1_singletons() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "ordered_gen1_fallback");
    transcript.record(
        "scenario",
        format!(
            "driver fallback decomposition for outbound gen2 batch against gen1-only peer \
             expected_common_protocol={:?}",
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

    let batch_items = vec![
        BatchRequestItem {
            item_id: 11,
            message: ByteBuf::from(jam_raw_tx_request(50_001)),
        },
        BatchRequestItem {
            item_id: 12,
            message: ByteBuf::from(jam_raw_tx_request(50_002)),
        },
        BatchRequestItem {
            item_id: 13,
            message: ByteBuf::from(jam_raw_tx_request(50_003)),
        },
    ];
    let expected_messages = batch_items
        .iter()
        .map(|item| item.message.as_ref().to_vec())
        .collect::<Vec<_>>();
    let batch_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 9,
        items: batch_items,
    };
    let fallback_requests = build_unsupported_protocol_fallback_requests(
        &requester_peer_id, &responder_peer_id, batch_request,
    )
    .expect("driver fallback decomposition should build ordered singleton requests");

    let fallback_item_ids = fallback_requests
        .iter()
        .map(|request| request.item_id)
        .collect::<Vec<_>>();
    transcript.record(
        "driver",
        format!("unsupported-protocol fallback queued ordered item_ids={fallback_item_ids:?}"),
    );
    assert_eq!(fallback_item_ids, vec![11, 12, 13]);

    let mut observed_messages = Vec::new();
    for (fallback, response_seed) in fallback_requests
        .iter()
        .zip([60_001u64, 60_002u64, 60_003u64])
    {
        let response_message = jam_heard_tx_response(response_seed, 32);
        let expected_response = NockchainResponse::Result {
            message: ByteBuf::from(response_message),
        };
        let (observed_request, response) = run_round_trip_observing_request(
            &mut requester,
            &mut responder,
            responder_peer_id,
            fallback.request.clone(),
            expected_response.clone(),
            &transcript,
        )
        .await;

        let observed_message = request_message_bytes(&observed_request);
        transcript.record(
            "driver",
            format!(
                "observed fallback singleton item_id={} message_bytes={}",
                fallback.item_id,
                observed_message.len(),
            ),
        );
        observed_messages.push(observed_message);
        assert_eq!(response, expected_response);
    }

    assert_gen1_result_round_trip(
        &mut requester, &mut responder, responder_peer_id, b"post-fallback-health-check",
        b"post-fallback-health-check-response", &transcript,
    )
    .await;

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen1.clone(); 4],
        "fallback requester should emit only gen1 writes after decomposition"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen1.clone(); 4],
        "gen1-only responder should observe all fallback requests on gen1"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen1.clone(); 4],
        "fallback responses should stay on gen1"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen1.clone(); 4],
        "requester should receive all fallback responses on gen1"
    );
    assert_eq!(
        observed_messages, expected_messages,
        "live fallback requests must preserve original batch wire order"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "fallback transport must degrade to gen1; transcript:\n{rendered}"
    );
    assert!(
        !rendered.contains("disconnecting from"),
        "degraded connection should stay healthy across ordered fallback singletons; transcript:\n{rendered}"
    );
    assert!(rendered.matches("shape=request").count() >= 8);
    assert!(rendered.matches("shape=result").count() >= 8);
}

/// The driver already unit-tests retry-context construction for transient
/// failures. This E2E extends the ordered fallback transcript case by forcing a
/// retryable failure on one degraded gen1 singleton and proving the retried
/// work stays behind its later already-queued same-peer sibling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_fallback_retry_preserves_ordered_gen1_queue() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "ordered_gen1_fallback_retry");
    transcript.record(
        "scenario",
        format!(
            "degraded gen1 fallback retry keeps per-peer order \
             expected_common_protocol={:?}",
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

    let batch_items = vec![
        BatchRequestItem {
            item_id: 21,
            message: ByteBuf::from(jam_raw_tx_request(61_001)),
        },
        BatchRequestItem {
            item_id: 22,
            message: ByteBuf::from(jam_raw_tx_request(61_002)),
        },
        BatchRequestItem {
            item_id: 23,
            message: ByteBuf::from(jam_raw_tx_request(61_003)),
        },
    ];
    let expected_messages = batch_items
        .iter()
        .map(|item| item.message.as_ref().to_vec())
        .collect::<Vec<_>>();
    let batch_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 19,
        items: batch_items,
    };
    let fallback_requests = build_unsupported_protocol_fallback_requests(
        &requester_peer_id, &responder_peer_id, batch_request,
    )
    .expect("driver fallback decomposition should build ordered singleton requests");

    let fallback_item_ids = fallback_requests
        .iter()
        .map(|request| request.item_id)
        .collect::<Vec<_>>();
    transcript.record(
        "driver",
        format!("unsupported-protocol fallback queued ordered item_ids={fallback_item_ids:?}"),
    );
    assert_eq!(fallback_item_ids, vec![21, 22, 23]);

    let mut observed_messages = Vec::new();

    let first_response = NockchainResponse::Result {
        message: ByteBuf::from(jam_heard_tx_response(71_001, 32)),
    };
    let (first_observed_request, first_observed_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        fallback_requests[0].request.clone(),
        first_response.clone(),
        &transcript,
    )
    .await;
    observed_messages.push(request_message_bytes(&first_observed_request));
    assert_eq!(first_observed_response, first_response);

    let failure_observation = run_request_until_disconnect_cleanup_failure(
        &mut requester,
        &mut responder,
        responder_peer_id,
        requester_peer_id,
        fallback_requests[1].request.clone(),
        &transcript,
    )
    .await;
    assert!(matches!(
        &failure_observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));
    let failed_observed_request = failure_observation
        .observed_request
        .expect("responder should observe the failed fallback singleton");
    observed_messages.push(request_message_bytes(&failed_observed_request));
    transcript.record(
        "driver",
        format!(
            "fallback item_id={} hit retryable failure={:?}; keeping queued sibling item_id={} ahead of delayed retry",
            fallback_requests[1].item_id,
            failure_observation.requester_error,
            fallback_requests[2].item_id,
        ),
    );

    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let third_response = NockchainResponse::Result {
        message: ByteBuf::from(jam_heard_tx_response(71_003, 32)),
    };
    let (third_observed_request, third_observed_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        fallback_requests[2].request.clone(),
        third_response.clone(),
        &transcript,
    )
    .await;
    observed_messages.push(request_message_bytes(&third_observed_request));
    assert_eq!(third_observed_response, third_response);

    transcript.record(
        "driver",
        format!(
            "requeued retry for fallback item_id={}",
            fallback_requests[1].item_id
        ),
    );
    let retry_response = NockchainResponse::Result {
        message: ByteBuf::from(jam_heard_tx_response(71_022, 32)),
    };
    let (retried_observed_request, retried_observed_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        fallback_requests[1].request.clone(),
        retry_response.clone(),
        &transcript,
    )
    .await;
    observed_messages.push(request_message_bytes(&retried_observed_request));
    assert_eq!(retried_observed_response, retry_response);

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen1.clone(); 4],
        "fallback requester should keep every original and retried request on gen1"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen1.clone(); 4],
        "gen1-only responder should observe all fallback traffic on gen1"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen1.clone(); 3],
        "successful fallback responses should stay on gen1"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen1.clone(); 3],
        "requester should receive successful fallback responses on gen1"
    );
    assert_eq!(
        observed_messages,
        vec![
            expected_messages[0].clone(),
            expected_messages[1].clone(),
            expected_messages[2].clone(),
            expected_messages[1].clone(),
        ],
        "retried fallback singleton must stay behind later same-peer work that was already queued"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "fallback transport must remain degraded to gen1 across reconnect and retry; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.matches("shape=request").count() >= 8);
    assert!(rendered.matches("shape=result").count() >= 6);
}

/// Extends the accept-only mixed-generation reconnect path with a partial
/// BatchResult so the wire transcript proves only retryable and missing items
/// are rebatched after renegotiation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_partial_batch_result_retries_only_selected_items_after_reconnect() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "partial_batch_result_selective_retry");
    transcript.record(
        "scenario",
        format!(
            "partial BatchResult selective retry after reconnect sender_to_responder={:?} responder_to_sender={:?}",
            expected_common_protocol(&requester_config, &responder_config),
            expected_common_protocol(&responder_config, &requester_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let initial_batch = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 41,
        items: vec![
            BatchRequestItem {
                item_id: 31,
                message: ByteBuf::from(jam_raw_tx_request(81_001)),
            },
            BatchRequestItem {
                item_id: 32,
                message: ByteBuf::from(jam_raw_tx_request(81_002)),
            },
            BatchRequestItem {
                item_id: 33,
                message: ByteBuf::from(jam_raw_tx_request(81_003)),
            },
        ],
    };
    let partial_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 31,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-81-001"),
                    jam_heard_tx_response(91_001, 32),
                )),
            },
            BatchResultItem {
                item_id: 32,
                status: BatchResultStatus::Error,
                error: Some(BatchErrorClass::Backpressure),
                envelope: None,
            },
        ],
    };

    let (observed_initial_request, observed_partial_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        initial_batch.clone(),
        partial_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(
        batch_request_item_ids(&observed_initial_request),
        vec![31, 32, 33]
    );
    assert_eq!(observed_partial_response, partial_response);

    let selective_retries = build_selective_batch_retry_requests(
        &requester_peer_id,
        &responder_peer_id,
        initial_batch,
        0,
        &[32, 33],
    )
    .expect("driver selective retry builder should preserve only retryable and missing items");
    let selective_retry_item_ids = selective_retries
        .iter()
        .map(|retry| retry.item_ids.clone())
        .collect::<Vec<_>>();
    transcript.record(
        "driver",
        format!(
            "partial BatchResult scheduled selective retry item_ids={selective_retry_item_ids:?}"
        ),
    );
    assert_eq!(selective_retries.len(), 1);
    assert_eq!(selective_retries[0].retry_count, 1);
    assert_eq!(selective_retries[0].item_ids, vec![32, 33]);

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let retry_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 32,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-81-002"),
                    jam_heard_tx_response(91_002, 24),
                )),
            },
            BatchResultItem {
                item_id: 33,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-81-003"),
                    jam_heard_tx_response(91_003, 24),
                )),
            },
        ],
    };
    let (observed_retry_request, observed_retry_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        selective_retries[0].request.clone(),
        retry_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(
        batch_request_item_ids(&observed_retry_request),
        vec![32, 33]
    );
    assert_eq!(observed_retry_response, retry_response);

    assert_batch_ack_round_trip(
        &mut requester, &mut responder, responder_peer_id, 34,
        b"partial-batch-follow-up-health-check", &transcript,
    )
    .await;

    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should keep every original, retried, and follow-up batch on gen2"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "accept-only responder should keep seeing the retried batch on gen2 after reconnect"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should receive the partial, retried, and follow-up batch results on gen2"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "responder should answer all batch traffic on gen2 once the sender negotiated it"
    );
    assert!(
        recorded_protocols(&responder, "write_request").is_empty(),
        "accept-only responder should not originate outbound requests in this scenario"
    );
    assert!(
        recorded_protocols(&requester, "read_request").is_empty(),
        "requester should not receive reverse-path requests in this scenario"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_responder=Some(\"/nockchain-2-req-res\")"),
        "accept-only inbound path should negotiate gen2 before and after reconnect; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("responder_to_sender=Some(\"/nockchain-1-req-res\")"),
        "mixed-generation asymmetry should remain visible in the transcript; transcript:\n{rendered}"
    );
    assert!(rendered.contains("partial BatchResult scheduled selective retry item_ids=[[32, 33]]"));
    assert!(rendered.contains("disconnecting from"));
    assert_eq!(rendered.matches("shape=batch-request").count(), 6);
    assert_eq!(rendered.matches("shape=batch-result").count(), 6);
}

/// Extends the accept-only mixed-generation reconnect path with a malformed
/// `ResponseEnvelope` so the transcript proves only the malformed and missing
/// item ids are rebatched after renegotiation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_malformed_response_envelope_retries_only_selected_items_after_reconnect() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "malformed_batch_result_selective_retry");
    transcript.record(
        "scenario",
        format!(
            "malformed ResponseEnvelope selective retry after reconnect sender_to_responder={:?} responder_to_sender={:?}",
            expected_common_protocol(&requester_config, &responder_config),
            expected_common_protocol(&responder_config, &requester_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let initial_batch = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 43,
        items: vec![
            BatchRequestItem {
                item_id: 41,
                message: ByteBuf::from(jam_raw_tx_request(82_001)),
            },
            BatchRequestItem {
                item_id: 42,
                message: ByteBuf::from(jam_raw_tx_request(82_002)),
            },
            BatchRequestItem {
                item_id: 43,
                message: ByteBuf::from(jam_raw_tx_request(82_003)),
            },
        ],
    };
    let mut malformed_envelope =
        ResponseEnvelope::heard_tx(String::from("tx-82-002"), jam_heard_tx_response(92_002, 24));
    malformed_envelope.block_id = Some(String::from("unexpected-block-id"));
    transcript.record(
        "driver",
        "item_id=42 carries malformed heard-tx envelope metadata; item_id=43 is omitted to force selective retry",
    );
    let partial_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 41,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-82-001"),
                    jam_heard_tx_response(92_001, 32),
                )),
            },
            BatchResultItem {
                item_id: 42,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(malformed_envelope),
            },
        ],
    };

    let (observed_initial_request, observed_partial_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        initial_batch.clone(),
        partial_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(
        batch_request_item_ids(&observed_initial_request),
        vec![41, 42, 43]
    );
    assert_eq!(observed_partial_response, partial_response);

    let selective_retries = build_selective_batch_retry_requests(
        &requester_peer_id,
        &responder_peer_id,
        initial_batch,
        0,
        &[42, 43],
    )
    .expect("driver selective retry builder should preserve only malformed and missing items");
    let selective_retry_item_ids = selective_retries
        .iter()
        .map(|retry| retry.item_ids.clone())
        .collect::<Vec<_>>();
    transcript.record(
        "driver",
        format!(
            "malformed ResponseEnvelope scheduled selective retry item_ids={selective_retry_item_ids:?}"
        ),
    );
    assert_eq!(selective_retries.len(), 1);
    assert_eq!(selective_retries[0].retry_count, 1);
    assert_eq!(selective_retries[0].item_ids, vec![42, 43]);

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let retry_response = NockchainResponse::BatchResult {
        results: vec![
            BatchResultItem {
                item_id: 42,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-82-002-retry"),
                    jam_heard_tx_response(92_102, 24),
                )),
            },
            BatchResultItem {
                item_id: 43,
                status: BatchResultStatus::Result,
                error: None,
                envelope: Some(ResponseEnvelope::heard_tx(
                    String::from("tx-82-003"),
                    jam_heard_tx_response(92_003, 24),
                )),
            },
        ],
    };
    let (observed_retry_request, observed_retry_response) = run_round_trip_observing_request(
        &mut requester,
        &mut responder,
        responder_peer_id,
        selective_retries[0].request.clone(),
        retry_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(
        batch_request_item_ids(&observed_retry_request),
        vec![42, 43]
    );
    assert_eq!(observed_retry_response, retry_response);

    assert_batch_ack_round_trip(
        &mut requester, &mut responder, responder_peer_id, 44,
        b"malformed-batch-follow-up-health-check", &transcript,
    )
    .await;

    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should keep the initial, retried, and follow-up batches on gen2"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "accept-only responder should keep seeing the malformed retry batch on gen2 after reconnect"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "requester should receive the malformed, retried, and follow-up batch results on gen2"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_response"),
        vec![gen2.clone(), gen2.clone(), gen2.clone()],
        "responder should answer all batch traffic on gen2 once the sender negotiated it"
    );
    assert!(
        recorded_protocols(&responder, "write_request").is_empty(),
        "accept-only responder should not originate outbound requests in this scenario"
    );
    assert!(
        recorded_protocols(&requester, "read_request").is_empty(),
        "requester should not receive reverse-path requests in this scenario"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_responder=Some(\"/nockchain-2-req-res\")"),
        "accept-only inbound path should negotiate gen2 before and after reconnect; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("responder_to_sender=Some(\"/nockchain-1-req-res\")"),
        "mixed-generation asymmetry should remain visible in the transcript; transcript:\n{rendered}"
    );
    assert!(rendered.contains("malformed heard-tx envelope metadata"));
    assert!(rendered
        .contains("malformed ResponseEnvelope scheduled selective retry item_ids=[[42, 43]]"));
    assert!(rendered.contains("disconnecting from"));
    assert_eq!(rendered.matches("shape=batch-request").count(), 6);
    assert_eq!(rendered.matches("shape=batch-result").count(), 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_full_disable_peer_stays_gen1_only_in_gen2_network() {
    init_tracing();

    let active_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let fully_disabled_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "full-disable node in gen2-active network expected_common_protocol={:?}",
            expected_common_protocol(&active_config, &fully_disabled_config),
        ),
    );

    let mut active = build_test_peer("active", active_config.clone());
    let mut disabled = build_test_peer("disabled", fully_disabled_config.clone());
    let active_peer_id = *active.swarm.local_peer_id();
    let disabled_peer_id = *disabled.swarm.local_peer_id();

    let _active_addr = wait_for_listen_addr(&mut active, &transcript).await;
    let disabled_addr = wait_for_listen_addr(&mut disabled, &transcript).await;
    connect_peers(&mut active, &mut disabled, &disabled_addr, &transcript).await;

    let first = run_round_trip(
        &mut active,
        &mut disabled,
        disabled_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"active-to-disabled".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"active-to-disabled-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        first,
        NockchainResponse::Result {
            message: ByteBuf::from(b"active-to-disabled-response".to_vec()),
        }
    );

    let second = run_round_trip(
        &mut disabled,
        &mut active,
        active_peer_id,
        NockchainRequest::Gossip {
            message: ByteBuf::from(b"disabled-to-active".to_vec()),
        },
        NockchainResponse::Ack { acked: true },
        &transcript,
    )
    .await;
    assert_eq!(second, NockchainResponse::Ack { acked: true });

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
    assert!(rendered.contains("shape=gossip"));
    assert!(rendered.contains("shape=ack"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_fallback_connection_handles_sequential_batches() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "gen2 sender falls back to gen1 and reuses the connection for sequential batches expected_common_protocol={:?}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let first_response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![
                BatchRequestItem {
                    item_id: 11,
                    message: ByteBuf::from(b"fallback-batch-1-item-1".to_vec()),
                },
                BatchRequestItem {
                    item_id: 12,
                    message: ByteBuf::from(b"fallback-batch-1-item-2".to_vec()),
                },
            ],
        },
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 11,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 12,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
            ],
        },
        &transcript,
    )
    .await;

    assert_eq!(
        first_response,
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 11,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 12,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
            ],
        }
    );

    let second_response = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![
                BatchRequestItem {
                    item_id: 21,
                    message: ByteBuf::from(b"fallback-batch-2-item-1".to_vec()),
                },
                BatchRequestItem {
                    item_id: 22,
                    message: ByteBuf::from(b"fallback-batch-2-item-2".to_vec()),
                },
                BatchRequestItem {
                    item_id: 23,
                    message: ByteBuf::from(b"fallback-batch-2-item-3".to_vec()),
                },
            ],
        },
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 21,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 22,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 23,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
            ],
        },
        &transcript,
    )
    .await;

    assert_eq!(
        second_response,
        NockchainResponse::BatchResult {
            results: vec![
                BatchResultItem {
                    item_id: 21,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 22,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
                BatchResultItem {
                    item_id: 23,
                    status: BatchResultStatus::Ack,
                    error: None,
                    envelope: None,
                },
            ],
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"),
        "fallback responder must negotiate gen1; transcript:\n{rendered}"
    );
    assert!(
        !rendered.contains("disconnecting from"),
        "transcript:\n{rendered}"
    );
    assert!(rendered.matches("shape=batch-request").count() >= 4);
    assert!(rendered.matches("shape=batch-result").count() >= 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn req_res_multi_peer_mixed_generation_fallback_stays_peer_scoped() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let legacy_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };
    let modern_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "multi_peer_mixed_generation_isolation");
    transcript.record(
        "scenario",
        format!(
            "one requester connected to legacy and modern peers concurrently \
             legacy_expected_protocol={:?} modern_expected_protocol={:?}",
            expected_common_protocol(&requester_config, &legacy_config),
            expected_common_protocol(&requester_config, &modern_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let mut legacy = build_test_peer("legacy", legacy_config.clone());
    let legacy_peer_id = *legacy.swarm.local_peer_id();
    let mut modern = build_test_peer("modern", modern_config.clone());
    let modern_peer_id = *modern.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let legacy_addr = wait_for_listen_addr(&mut legacy, &transcript).await;
    let modern_addr = wait_for_listen_addr(&mut modern, &transcript).await;

    connect_peers(&mut requester, &mut legacy, &legacy_addr, &transcript).await;
    connect_peers(&mut requester, &mut modern, &modern_addr, &transcript).await;
    transcript.record(
        "scenario",
        format!(
            "concurrent topology requester={requester_peer_id} legacy={legacy_peer_id} modern={modern_peer_id}"
        ),
    );

    let modern_first = run_round_trip(
        &mut requester,
        &mut modern,
        modern_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"modern-before-legacy-reconnect".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        modern_first,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let legacy_first = run_round_trip(
        &mut requester,
        &mut legacy,
        legacy_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"legacy-fallback-before-reconnect".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-fallback-before-reconnect-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        legacy_first,
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-fallback-before-reconnect-response".to_vec()),
        }
    );

    disconnect_peers(
        &mut requester, &mut legacy, legacy_peer_id, requester_peer_id, &transcript,
    )
    .await;
    connect_peers(&mut requester, &mut legacy, &legacy_addr, &transcript).await;

    let legacy_second = run_round_trip(
        &mut requester,
        &mut legacy,
        legacy_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"legacy-fallback-after-reconnect".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-fallback-after-reconnect-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        legacy_second,
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-fallback-after-reconnect-response".to_vec()),
        }
    );

    let modern_second = run_round_trip(
        &mut requester,
        &mut modern,
        modern_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"modern-after-legacy-reconnect".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        modern_second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("legacy_expected_protocol=Some(\"/nockchain-1-req-res\")"),
        "legacy peer should stay on gen1; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("modern_expected_protocol=Some(\"/nockchain-2-req-res\")"),
        "modern peer should stay on gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("disconnecting from {legacy_peer_id}")),
        "legacy reconnect should be visible in the transcript:\n{rendered}"
    );
    assert!(
        !rendered.contains(&format!("disconnecting from {modern_peer_id}")),
        "legacy reconnect must not force the modern peer off its connection:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-request").count() >= 4,
        "modern peer should keep handling batch traffic on gen2 before and after legacy reconnect; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-result").count() >= 4,
        "modern peer should keep returning batch results on gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=request").count() >= 4,
        "legacy peer should keep serving singleton gen1 fallback requests; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=result").count() >= 4,
        "legacy peer should keep returning singleton gen1 fallback responses; transcript:\n{rendered}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn req_res_gen2_four_node_rolling_rollback_stays_peer_scoped() {
    init_tracing();

    let gen2_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let rollback_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "four_node_rolling_rollback");

    let anchor_keypair = libp2p::identity::Keypair::generate_ed25519();
    let full_a_keypair = libp2p::identity::Keypair::generate_ed25519();
    let full_b_keypair = libp2p::identity::Keypair::generate_ed25519();
    let miner_b_keypair = libp2p::identity::Keypair::generate_ed25519();

    let mut anchor =
        build_test_peer_with_keypair("miner-a", gen2_config.clone(), anchor_keypair.clone());
    let anchor_peer_id = *anchor.swarm.local_peer_id();
    let mut full_a =
        build_test_peer_with_keypair("full-a", gen2_config.clone(), full_a_keypair.clone());
    let full_a_peer_id = *full_a.swarm.local_peer_id();
    let mut full_b =
        build_test_peer_with_keypair("full-b", gen2_config.clone(), full_b_keypair.clone());
    let full_b_peer_id = *full_b.swarm.local_peer_id();
    let mut miner_b =
        build_test_peer_with_keypair("miner-b", gen2_config.clone(), miner_b_keypair.clone());
    let miner_b_peer_id = *miner_b.swarm.local_peer_id();

    let _anchor_addr = wait_for_listen_addr(&mut anchor, &transcript).await;
    let full_a_addr = wait_for_listen_addr(&mut full_a, &transcript).await;
    let full_b_addr = wait_for_listen_addr(&mut full_b, &transcript).await;
    let miner_b_addr = wait_for_listen_addr(&mut miner_b, &transcript).await;

    connect_peers(&mut anchor, &mut full_a, &full_a_addr, &transcript).await;
    connect_peers(&mut anchor, &mut full_b, &full_b_addr, &transcript).await;
    connect_peers(&mut anchor, &mut miner_b, &miner_b_addr, &transcript).await;

    transcript.record(
        "scenario",
        format!(
            "4-node rolling rollback anchor={anchor_peer_id} full_a={full_a_peer_id} \
             full_b={full_b_peer_id} miner_b={miner_b_peer_id}"
        ),
    );
    transcript.record(
        "phase",
        format!(
            "baseline all outbound gen2 anchor_to_full_a={:?} full_a_to_anchor={:?} \
             full_b_to_anchor={:?} miner_b_to_anchor={:?}",
            expected_common_protocol(&gen2_config, &gen2_config),
            expected_common_protocol(&gen2_config, &gen2_config),
            expected_common_protocol(&gen2_config, &gen2_config),
            expected_common_protocol(&gen2_config, &gen2_config),
        ),
    );

    assert_batch_ack_round_trip(
        &mut anchor, &mut full_a, full_a_peer_id, 1, b"anchor-before-rollback", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut full_a, &mut anchor, anchor_peer_id, 11, b"full-a-before-rollback", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut full_b, &mut anchor, anchor_peer_id, 21, b"full-b-before-rollback", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut miner_b, &mut anchor, anchor_peer_id, 31, b"miner-b-before-rollback", &transcript,
    )
    .await;

    let full_a_gen2_before_rollback = recorded_protocols(&full_a, "write_request");
    assert_eq!(
        full_a_gen2_before_rollback,
        vec![LibP2PConfig::req_res_gen2_protocol_version().to_string()],
        "full-a should start with gen2 outbound before rollback"
    );

    disconnect_peers(
        &mut anchor, &mut full_a, full_a_peer_id, anchor_peer_id, &transcript,
    )
    .await;
    drop(full_a);
    transcript.record(
        "rollback", "full-a dropped; rebuilding with same keypair and gen1-first outbound",
    );

    let mut full_a =
        build_test_peer_with_keypair("full-a", rollback_config.clone(), full_a_keypair);
    assert_eq!(
        *full_a.swarm.local_peer_id(),
        full_a_peer_id,
        "restarted full-a must retain its identity"
    );
    let full_a_addr = wait_for_listen_addr(&mut full_a, &transcript).await;
    connect_peers(&mut anchor, &mut full_a, &full_a_addr, &transcript).await;

    transcript.record(
        "phase",
        format!(
            "after full-a rollback full_a_to_anchor={:?} anchor_to_full_a={:?} \
             full_b_to_anchor={:?} miner_b_to_anchor={:?}",
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&gen2_config, &rollback_config),
            expected_common_protocol(&gen2_config, &gen2_config),
            expected_common_protocol(&gen2_config, &gen2_config),
        ),
    );

    assert_gen1_result_round_trip(
        &mut full_a, &mut anchor, anchor_peer_id, b"full-a-after-rollback",
        b"full-a-after-rollback-response", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut full_b, &mut anchor, anchor_peer_id, 22, b"full-b-during-full-a-rollback", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut miner_b, &mut anchor, anchor_peer_id, 32, b"miner-b-during-full-a-rollback",
        &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut anchor, &mut full_a, full_a_peer_id, 2, b"anchor-still-gen2-to-full-a", &transcript,
    )
    .await;

    let full_b_gen2_before_rollback = recorded_protocols(&full_b, "write_request");
    assert_eq!(
        full_b_gen2_before_rollback,
        vec![
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
        ],
        "full-b should remain on gen2 until its own rollback"
    );

    disconnect_peers(
        &mut anchor, &mut full_b, full_b_peer_id, anchor_peer_id, &transcript,
    )
    .await;
    drop(full_b);
    transcript.record(
        "rollback", "full-b dropped; rebuilding with same keypair and gen1-first outbound",
    );

    let mut full_b =
        build_test_peer_with_keypair("full-b", rollback_config.clone(), full_b_keypair);
    assert_eq!(
        *full_b.swarm.local_peer_id(),
        full_b_peer_id,
        "restarted full-b must retain its identity"
    );
    let full_b_addr = wait_for_listen_addr(&mut full_b, &transcript).await;
    connect_peers(&mut anchor, &mut full_b, &full_b_addr, &transcript).await;

    transcript.record(
        "phase",
        format!(
            "after full-b rollback full_a_to_anchor={:?} full_b_to_anchor={:?} \
             miner_b_to_anchor={:?} anchor_to_full_a={:?}",
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&gen2_config, &gen2_config),
            expected_common_protocol(&gen2_config, &rollback_config),
        ),
    );

    assert_gen1_result_round_trip(
        &mut full_a, &mut anchor, anchor_peer_id, b"full-a-stays-gen1-after-full-b-rollback",
        b"full-a-stays-gen1-after-full-b-rollback-response", &transcript,
    )
    .await;
    assert_gen1_result_round_trip(
        &mut full_b, &mut anchor, anchor_peer_id, b"full-b-after-rollback",
        b"full-b-after-rollback-response", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut miner_b, &mut anchor, anchor_peer_id, 33, b"miner-b-during-full-b-rollback",
        &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut anchor, &mut full_a, full_a_peer_id, 3, b"anchor-still-gen2-after-full-b-rollback",
        &transcript,
    )
    .await;

    let miner_b_gen2_before_rollback = recorded_protocols(&miner_b, "write_request");
    assert_eq!(
        miner_b_gen2_before_rollback,
        vec![
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
        ],
        "miner-b should stay on gen2 until its own rollback"
    );

    disconnect_peers(
        &mut anchor, &mut miner_b, miner_b_peer_id, anchor_peer_id, &transcript,
    )
    .await;
    drop(miner_b);
    transcript.record(
        "rollback", "miner-b dropped; rebuilding with same keypair and gen1-first outbound",
    );

    let mut miner_b =
        build_test_peer_with_keypair("miner-b", rollback_config.clone(), miner_b_keypair);
    assert_eq!(
        *miner_b.swarm.local_peer_id(),
        miner_b_peer_id,
        "restarted miner-b must retain its identity"
    );
    let miner_b_addr = wait_for_listen_addr(&mut miner_b, &transcript).await;
    connect_peers(&mut anchor, &mut miner_b, &miner_b_addr, &transcript).await;

    transcript.record(
        "phase",
        format!(
            "after miner-b rollback full_a_to_anchor={:?} full_b_to_anchor={:?} \
             miner_b_to_anchor={:?} anchor_to_full_a={:?}",
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&rollback_config, &gen2_config),
            expected_common_protocol(&gen2_config, &rollback_config),
        ),
    );

    assert_gen1_result_round_trip(
        &mut full_a, &mut anchor, anchor_peer_id, b"full-a-stays-gen1-after-miner-b-rollback",
        b"full-a-stays-gen1-after-miner-b-rollback-response", &transcript,
    )
    .await;
    assert_gen1_result_round_trip(
        &mut full_b, &mut anchor, anchor_peer_id, b"full-b-stays-gen1-after-miner-b-rollback",
        b"full-b-stays-gen1-after-miner-b-rollback-response", &transcript,
    )
    .await;
    assert_gen1_result_round_trip(
        &mut miner_b, &mut anchor, anchor_peer_id, b"miner-b-after-rollback",
        b"miner-b-after-rollback-response", &transcript,
    )
    .await;
    assert_batch_ack_round_trip(
        &mut anchor, &mut full_a, full_a_peer_id, 4, b"anchor-still-gen2-after-miner-b-rollback",
        &transcript,
    )
    .await;

    let anchor_gen2_before_rollback = recorded_protocols(&anchor, "write_request");
    assert_eq!(
        anchor_gen2_before_rollback,
        vec![
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
            LibP2PConfig::req_res_gen2_protocol_version().to_string(),
        ],
        "anchor should stay on gen2 until the final rollback stage"
    );

    disconnect_peers(
        &mut anchor, &mut full_a, full_a_peer_id, anchor_peer_id, &transcript,
    )
    .await;
    drop(anchor);
    transcript.record(
        "rollback", "anchor dropped; rebuilding with same keypair and gen1-first outbound",
    );

    let mut anchor =
        build_test_peer_with_keypair("miner-a", rollback_config.clone(), anchor_keypair);
    assert_eq!(
        *anchor.swarm.local_peer_id(),
        anchor_peer_id,
        "restarted anchor must retain its identity"
    );
    let _anchor_addr = wait_for_listen_addr(&mut anchor, &transcript).await;
    connect_peers(&mut anchor, &mut full_a, &full_a_addr, &transcript).await;

    transcript.record(
        "phase",
        format!(
            "after anchor rollback anchor_to_full_a={:?} full_a_to_anchor={:?}",
            expected_common_protocol(&rollback_config, &rollback_config),
            expected_common_protocol(&rollback_config, &rollback_config),
        ),
    );

    assert_gen1_result_round_trip(
        &mut anchor, &mut full_a, full_a_peer_id, b"anchor-after-rollback",
        b"anchor-after-rollback-response", &transcript,
    )
    .await;
    assert_gen1_result_round_trip(
        &mut full_a, &mut anchor, anchor_peer_id, b"full-a-final-all-gen1",
        b"full-a-final-all-gen1-response", &transcript,
    )
    .await;

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    assert_eq!(
        recorded_protocols(&full_a, "write_request"),
        vec![gen1.clone(), gen1.clone(), gen1.clone(), gen1.clone()],
        "full-a should stay on gen1 for every outbound request after rollback"
    );
    assert_eq!(
        recorded_protocols(&full_a, "read_request"),
        vec![gen2.clone(), gen2.clone(), gen2.clone(), gen1.clone()],
        "full-a should keep accepting gen2 inbound until the anchor rolls back"
    );
    assert_eq!(
        recorded_protocols(&full_b, "write_request"),
        vec![gen1.clone(), gen1.clone()],
        "full-b should switch to gen1 once rolled back"
    );
    assert_eq!(
        recorded_protocols(&miner_b, "write_request"),
        vec![gen1.clone()],
        "miner-b should switch to gen1 once rolled back"
    );
    assert_eq!(
        recorded_protocols(&anchor, "write_request"),
        vec![gen1.clone()],
        "anchor should switch to gen1 after the final rollback stage"
    );
    assert_eq!(
        recorded_protocols(&anchor, "read_request"),
        vec![gen1.clone()],
        "final fully rolled back traffic should reach the anchor over gen1"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("4-node rolling rollback"),
        "transcript should capture the staged topology:\n{rendered}"
    );
    assert!(
        rendered.contains("full_a_to_anchor=Some(\"/nockchain-1-req-res\")"),
        "rolled-back full-a should be visible as gen1 outbound in the transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("full_b_to_anchor=Some(\"/nockchain-1-req-res\")"),
        "rolled-back full-b should be visible as gen1 outbound in the transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("miner_b_to_anchor=Some(\"/nockchain-1-req-res\")"),
        "rolled-back miner-b should be visible as gen1 outbound in the transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("anchor_to_full_a=Some(\"/nockchain-2-req-res\")"),
        "the anchor should keep gen2 outbound to rolled-back peers until its own rollback:\n{rendered}"
    );
    assert!(
        rendered.contains("anchor_to_full_a=Some(\"/nockchain-1-req-res\")"),
        "the final stage should show the anchor rolled back to gen1:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("disconnecting from {full_a_peer_id}")),
        "full-a rollback should disconnect and reconnect explicitly:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("disconnecting from {full_b_peer_id}")),
        "full-b rollback should disconnect and reconnect explicitly:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("disconnecting from {miner_b_peer_id}")),
        "miner-b rollback should disconnect and reconnect explicitly:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-request").count() >= 10,
        "gen2 traffic should stay live while only part of the topology rolls back:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-result").count() >= 10,
        "gen2 responses should stay live while only part of the topology rolls back:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=request").count() >= 8,
        "gen1 rollback traffic should accumulate across the staged topology:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=result").count() >= 8,
        "gen1 responses should accumulate across the staged topology:\n{rendered}"
    );
}

#[derive(Serialize)]
struct TwoPeerLatencySample {
    label: String,
    generation: String,
    item_count: usize,
    payload_len: usize,
    response_bytes: usize,
    total_ms: f64,
    per_item_ms: f64,
    protocol: String,
}

#[derive(Serialize)]
struct TwoPeerLatencyReport {
    schema_version: &'static str,
    scenario: &'static str,
    samples: Vec<TwoPeerLatencySample>,
}

fn maybe_write_report_json<T: Serialize>(report: &T) {
    let Ok(path) = std::env::var("REQ_RES_GEN2_REPORT_JSON") else {
        return;
    };
    let json = serde_json::to_vec_pretty(report).expect("benchmark report should serialize");
    fs::write(&path, json).expect("benchmark report should write");
    println!("json report: {path}");
}

fn encoded_response_bytes(response: &NockchainResponse) -> usize {
    cbor4ii::serde::to_vec(Vec::new(), response)
        .expect("response should encode")
        .len()
}

fn gen1_latency_config() -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    }
}

fn gen2_latency_config() -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    }
}

fn gen1_request(item_id: usize) -> NockchainRequest {
    NockchainRequest::Request {
        pow: Default::default(),
        nonce: 0,
        message: ByteBuf::from(format!("latency-gen1-{item_id}").into_bytes()),
    }
}

fn gen1_result(payload_len: usize) -> NockchainResponse {
    NockchainResponse::Result {
        message: ByteBuf::from(vec![0xAB; payload_len]),
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "rollout harness, run explicitly with -- --ignored --nocapture"]
async fn req_res_gen2_transport_two_peer_latency_report() {
    init_tracing();

    let workloads = [
        ("gen1-singleton-32", false, 32usize, 512usize),
        ("gen2-batch-32", true, 32usize, 512usize),
        ("gen1-singleton-128-large", false, 128usize, 2048usize),
        ("gen2-batch-128-large", true, 128usize, 2048usize),
    ];
    let mut samples = Vec::with_capacity(workloads.len());

    println!("req-res gen2 two-peer latency report");
    println!(
        "{:<24} {:<8} {:>8} {:>12} {:>14} {:>12} {:>12}",
        "workload", "gen", "items", "payload", "response_bytes", "total_ms", "per_item_ms"
    );

    for (label, use_gen2, item_count, payload_len) in workloads {
        let requester_config = if use_gen2 {
            gen2_latency_config()
        } else {
            gen1_latency_config()
        };
        let responder_config = requester_config.clone();
        let protocol = expected_common_protocol(&requester_config, &responder_config)
            .unwrap_or_else(|| String::from("none"));
        let transcript = Transcript::default();
        transcript.record(
            "scenario",
            format!(
                "{label} generation={} items={item_count} payload_len={payload_len} protocol={protocol}",
                if use_gen2 { "gen2" } else { "gen1" },
            ),
        );

        let mut requester = build_test_peer("requester", requester_config);
        let mut responder = build_test_peer("responder", responder_config);
        let responder_peer_id = *responder.swarm.local_peer_id();

        let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
        let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
        connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

        let started = Instant::now();
        let response_bytes = if use_gen2 {
            let request = gen2_request(item_count);
            let response = gen2_result(item_count, payload_len);
            let response_bytes = encoded_response_bytes(&response);
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
            response_bytes
        } else {
            let response = gen1_result(payload_len);
            let single_response_bytes = encoded_response_bytes(&response);
            for idx in 0..item_count {
                let observed = run_round_trip(
                    &mut requester,
                    &mut responder,
                    responder_peer_id,
                    gen1_request(idx),
                    response.clone(),
                    &transcript,
                )
                .await;
                assert_eq!(observed, response);
            }
            single_response_bytes * item_count
        };
        let elapsed = started.elapsed();
        let total_ms = elapsed.as_secs_f64() * 1_000.0;
        let per_item_ms = total_ms / item_count as f64;

        println!(
            "{:<24} {:<8} {:>8} {:>12} {:>14} {:>12.3} {:>12.3}",
            label,
            if use_gen2 { "gen2" } else { "gen1" },
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_ms
        );

        samples.push(TwoPeerLatencySample {
            label: label.to_string(),
            generation: if use_gen2 {
                String::from("gen2")
            } else {
                String::from("gen1")
            },
            item_count,
            payload_len,
            response_bytes,
            total_ms,
            per_item_ms,
            protocol,
        });
    }

    maybe_write_report_json(&TwoPeerLatencyReport {
        schema_version: "req_res_gen2_two_peer_latency_v1",
        scenario: "transport-two-peer-latency",
        samples,
    });
}

// ---------------------------------------------------------------------------
// Outbound-only sender restart coverage
// ---------------------------------------------------------------------------

/// An outbound-only gen2 sender (`accept_enabled=false`, `send_enabled=true`)
/// that restarts with the same peer identity must continue to use gen2 for its
/// own outbound requests while forcing reverse-direction traffic back to gen1.
///
/// Unlike the reconnect coverage
/// (`req_res_outbound_only_sender_reconnects_and_keeps_gen1_inbound`), this
/// drops and recreates the outbound-only process entirely to prove that the
/// explicit experiment support mode survives a full restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_outbound_only_sender_restart_preserves_gen2_outbound_and_gen1_inbound() {
    init_tracing();

    let sender_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let full_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "outbound_only_restart");
    transcript.record(
        "scenario",
        format!(
            "outbound-only sender restart: sender_to_full={:?} full_to_sender={:?}",
            expected_common_protocol(&sender_config, &full_config),
            expected_common_protocol(&full_config, &sender_config),
        ),
    );

    let sender_keypair = libp2p::identity::Keypair::generate_ed25519();
    let mut full = build_test_peer("full", full_config.clone());
    let full_peer_id = *full.swarm.local_peer_id();
    let mut sender =
        build_test_peer_with_keypair("sender", sender_config.clone(), sender_keypair.clone());
    let sender_peer_id = *sender.swarm.local_peer_id();

    let _sender_addr = wait_for_listen_addr(&mut sender, &transcript).await;
    let full_addr = wait_for_listen_addr(&mut full, &transcript).await;
    connect_peers(&mut sender, &mut full, &full_addr, &transcript).await;

    assert_batch_ack_round_trip(
        &mut sender, &mut full, full_peer_id, 1, b"outbound-only-restart-phase1", &transcript,
    )
    .await;

    disconnect_peers(
        &mut sender, &mut full, full_peer_id, sender_peer_id, &transcript,
    )
    .await;
    drop(sender);
    transcript.record(
        "restart", "sender dropped; rebuilding with same keypair in outbound-only mode",
    );

    let mut sender = build_test_peer_with_keypair("sender", sender_config.clone(), sender_keypair);
    assert_eq!(
        *sender.swarm.local_peer_id(),
        sender_peer_id,
        "restarted outbound-only sender must retain its identity"
    );
    let _sender_addr = wait_for_listen_addr(&mut sender, &transcript).await;
    connect_peers(&mut sender, &mut full, &full_addr, &transcript).await;

    assert_batch_ack_round_trip(
        &mut sender, &mut full, full_peer_id, 2, b"outbound-only-restart-phase2", &transcript,
    )
    .await;

    assert_gen1_result_round_trip(
        &mut full, &mut sender, sender_peer_id, b"full-to-outbound-only-restart",
        b"full-to-outbound-only-restart-response", &transcript,
    )
    .await;

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    assert_eq!(
        recorded_protocols(&sender, "write_request"),
        vec![gen2.clone()],
        "restarted outbound-only sender must renegotiate gen2 outbound"
    );
    assert_eq!(
        recorded_protocols(&full, "read_request"),
        vec![gen2.clone(), gen2.clone()],
        "full peer must read outbound-only requests on gen2 before and after restart"
    );
    assert_eq!(
        recorded_protocols(&full, "write_request"),
        vec![gen1.clone()],
        "full peer must keep reverse outbound on gen1 against restarted outbound-only sender"
    );
    assert_eq!(
        recorded_protocols(&sender, "read_request"),
        vec![gen1.clone()],
        "reverse request to restarted outbound-only sender must arrive over gen1"
    );

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_full=Some(\"/nockchain-2-req-res\")"),
        "outbound-only outbound path should prefer gen2 after restart; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("full_to_sender=Some(\"/nockchain-1-req-res\")"),
        "reverse direction must negotiate gen1 because restarted outbound-only sender does not accept gen2; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("sender dropped; rebuilding with same keypair"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

// ---------------------------------------------------------------------------
// Accept-only responder restart coverage
// ---------------------------------------------------------------------------

/// An accept-only responder that restarts with the same peer identity
/// must continue to accept inbound gen2 while keeping outbound pinned to gen1.
///
/// This covers the stage-1 rollout scenario where rolling restarts and process
/// replacement occur with the accept-only config
/// (`req_res_gen2_accept_enabled=true`, `req_res_gen2_send_enabled=false`).
/// Unlike the reconnect test (`req_res_accept_only_responder_reconnects_and_keeps_gen1_outbound`),
/// this drops and recreates the responder process entirely — reusing only the
/// keypair — to prove that support-mode registration survives a full restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_accept_only_responder_restart_preserves_inbound_gen2() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    // Accept-only stage: accept gen2 inbound, do NOT send gen2 outbound.
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "accept-only responder restart: sender_to_responder={:?} responder_to_sender={:?}",
            expected_common_protocol(&requester_config, &responder_config),
            expected_common_protocol(&responder_config, &requester_config),
        ),
    );

    // Generate keypair up front so the restarted responder keeps the same
    // peer identity.
    let responder_keypair = libp2p::identity::Keypair::generate_ed25519();

    let mut requester = build_test_peer("requester", requester_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let mut responder = build_test_peer_with_keypair(
        "responder",
        responder_config.clone(),
        responder_keypair.clone(),
    );
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // --- Phase 1: gen2 sender → accept-only responder (pre-restart) ---

    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"accept-only-restart-phase1".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        first,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // --- Restart: disconnect, drop responder, rebuild with same keypair ---

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(responder);

    let mut responder =
        build_test_peer_with_keypair("responder", responder_config.clone(), responder_keypair);
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // --- Phase 2: gen2 sender → restarted accept-only responder ---

    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"accept-only-restart-phase2".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // --- Phase 3: verify restarted responder still sends outbound on gen1 ---

    let reverse = run_round_trip(
        &mut responder,
        &mut requester,
        requester_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"accept-only-restart-outbound-gen1".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"accept-only-restart-outbound-gen1-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        reverse,
        NockchainResponse::Result {
            message: ByteBuf::from(b"accept-only-restart-outbound-gen1-response".to_vec()),
        }
    );

    // --- Protocol trace assertions ---

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone(), gen2.clone()],
        "gen2 sender must renegotiate gen2 inbound after responder restart"
    );
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        // Only the post-restart responder records; the pre-restart instance
        // was dropped, so we expect a single gen2 read here.
        vec![gen2.clone()],
        "restarted accept-only responder must read inbound requests on gen2"
    );
    assert_eq!(
        recorded_protocols(&responder, "write_request"),
        vec![gen1.clone()],
        "restarted accept-only responder must keep outbound requests on gen1"
    );
    assert_eq!(
        recorded_protocols(&requester, "read_request"),
        vec![gen1.clone()],
        "reverse request from restarted accept-only responder must arrive over gen1"
    );

    // --- Transcript assertions ---

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_responder=Some(\"/nockchain-2-req-res\")"),
        "accept-only inbound path should prefer gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("responder_to_sender=Some(\"/nockchain-1-req-res\")"),
        "accept-only outbound path should stay gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

/// A gen2-capable requester restarts (same peer identity) against
/// an accept-only responder (accept-only stage: accept_enabled=true,
/// send_enabled=false).  After restart, the next outbound batch must still
/// negotiate gen2 inbound on the accept-only responder.  Also verifies the
/// accept-only responder keeps gen1 for reverse outbound after the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_requester_restart_against_accept_only_responder_renegotiates_gen2() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };

    // Accept-only stage: accept gen2 inbound, do NOT send gen2 outbound.
    let responder_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "requester restart against accept-only responder: \
             sender_to_responder={:?} responder_to_sender={:?}",
            expected_common_protocol(&requester_config, &responder_config),
            expected_common_protocol(&responder_config, &requester_config),
        ),
    );

    let requester_keypair = libp2p::identity::Keypair::generate_ed25519();
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();
    let mut requester = build_test_peer_with_keypair(
        "requester",
        requester_config.clone(),
        requester_keypair.clone(),
    );
    let requester_peer_id = *requester.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // --- Phase 1: gen2 sender → accept-only responder (pre-restart) ---

    let first = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"requester-restart-accept-only-phase1".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        first,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // --- Restart: disconnect, drop requester, rebuild with same keypair ---

    disconnect_peers(
        &mut requester, &mut responder, responder_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(requester);

    let mut requester =
        build_test_peer_with_keypair("requester", requester_config.clone(), requester_keypair);
    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    // --- Phase 2: restarted gen2 sender → same accept-only responder ---

    let second = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"requester-restart-accept-only-phase2".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        second,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // --- Phase 3: accept-only responder still sends outbound on gen1 ---

    let reverse = run_round_trip(
        &mut responder,
        &mut requester,
        requester_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"requester-restart-accept-only-reverse-gen1".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"requester-restart-accept-only-reverse-gen1-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        reverse,
        NockchainResponse::Result {
            message: ByteBuf::from(b"requester-restart-accept-only-reverse-gen1-response".to_vec(),),
        }
    );

    // --- Protocol trace assertions ---

    let gen1 = LibP2PConfig::req_res_gen1_protocol_version().to_string();
    let gen2 = LibP2PConfig::req_res_gen2_protocol_version().to_string();

    // Requester was dropped and rebuilt — only post-restart trace recorded.
    assert_eq!(
        recorded_protocols(&requester, "write_request"),
        vec![gen2.clone()],
        "restarted requester must renegotiate gen2 with accept-only responder"
    );
    // Accept-only responder read both pre- and post-restart requests on gen2.
    assert_eq!(
        recorded_protocols(&responder, "read_request"),
        vec![gen2.clone(), gen2.clone()],
        "accept-only responder must read both pre- and post-restart requests on gen2"
    );
    // Accept-only responder sent reverse outbound on gen1 (send_enabled=false).
    assert_eq!(
        recorded_protocols(&responder, "write_request"),
        vec![gen1.clone()],
        "accept-only responder must keep outbound on gen1 after requester restart"
    );
    // Requester received reverse request on gen1.
    assert_eq!(
        recorded_protocols(&requester, "read_request"),
        vec![gen1.clone()],
        "reverse request from accept-only responder must arrive over gen1"
    );

    // --- Transcript assertions ---

    let rendered = transcript.render();
    assert!(
        rendered.contains("sender_to_responder=Some(\"/nockchain-2-req-res\")"),
        "accept-only inbound path should prefer gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("responder_to_sender=Some(\"/nockchain-1-req-res\")"),
        "accept-only outbound path should stay gen1; transcript:\n{rendered}"
    );
    assert!(rendered.contains("disconnecting from"));
    assert!(rendered.contains("shape=batch-request"));
    assert!(rendered.contains("shape=batch-result"));
    assert!(rendered.contains("shape=request"));
    assert!(rendered.contains("shape=result"));
}

/// Restarting a legacy peer in a mixed-generation topology must not
/// collapse the concurrent modern peer's gen2 path back to gen1.  This is the
/// restart counterpart to
/// `req_res_multi_peer_mixed_generation_fallback_stays_peer_scoped` (which
/// covers reconnect isolation).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_multi_peer_restart_isolation_preserves_modern_gen2() {
    init_tracing();

    let requester_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..LibP2PConfig::default()
    };
    let legacy_config = LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        ..default_test_config()
    };
    let modern_config = LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        ..default_test_config()
    };

    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "multi_peer_restart_isolation");
    transcript.record(
        "scenario",
        format!(
            "requester connected to legacy and modern peers; legacy will restart \
             legacy_expected_protocol={:?} modern_expected_protocol={:?}",
            expected_common_protocol(&requester_config, &legacy_config),
            expected_common_protocol(&requester_config, &modern_config),
        ),
    );

    let legacy_keypair = libp2p::identity::Keypair::generate_ed25519();

    let mut requester = build_test_peer("requester", requester_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let mut legacy =
        build_test_peer_with_keypair("legacy", legacy_config.clone(), legacy_keypair.clone());
    let legacy_peer_id = *legacy.swarm.local_peer_id();
    let mut modern = build_test_peer("modern", modern_config.clone());
    let modern_peer_id = *modern.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let legacy_addr = wait_for_listen_addr(&mut legacy, &transcript).await;
    let modern_addr = wait_for_listen_addr(&mut modern, &transcript).await;

    connect_peers(&mut requester, &mut legacy, &legacy_addr, &transcript).await;
    connect_peers(&mut requester, &mut modern, &modern_addr, &transcript).await;
    transcript.record(
        "scenario",
        format!(
            "concurrent topology requester={requester_peer_id} \
             legacy={legacy_peer_id} modern={modern_peer_id}"
        ),
    );

    // --- Phase 1: baseline traffic on both peers ---

    let modern_phase1 = run_round_trip(
        &mut requester,
        &mut modern,
        modern_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"modern-before-legacy-restart".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        modern_phase1,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 1,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    let legacy_phase1 = run_round_trip(
        &mut requester,
        &mut legacy,
        legacy_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"legacy-before-restart".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-before-restart-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        legacy_phase1,
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-before-restart-response".to_vec()),
        }
    );

    // --- Phase 2: restart legacy peer (disconnect, drop, rebuild same identity) ---

    disconnect_peers(
        &mut requester, &mut legacy, legacy_peer_id, requester_peer_id, &transcript,
    )
    .await;
    drop(legacy);
    transcript.record(
        "restart", "legacy peer dropped; rebuilding with same keypair",
    );

    let mut legacy = build_test_peer_with_keypair("legacy", legacy_config.clone(), legacy_keypair);
    assert_eq!(
        *legacy.swarm.local_peer_id(),
        legacy_peer_id,
        "restarted legacy peer must retain its identity"
    );
    let legacy_addr = wait_for_listen_addr(&mut legacy, &transcript).await;
    connect_peers(&mut requester, &mut legacy, &legacy_addr, &transcript).await;

    // --- Phase 3: post-restart traffic — legacy stays gen1, modern stays gen2 ---

    let legacy_phase3 = run_round_trip(
        &mut requester,
        &mut legacy,
        legacy_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 0,
            message: ByteBuf::from(b"legacy-after-restart".to_vec()),
        },
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-after-restart-response".to_vec()),
        },
        &transcript,
    )
    .await;
    assert_eq!(
        legacy_phase3,
        NockchainResponse::Result {
            message: ByteBuf::from(b"legacy-after-restart-response".to_vec()),
        }
    );

    let modern_phase3 = run_round_trip(
        &mut requester,
        &mut modern,
        modern_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 0,
            items: vec![BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"modern-after-legacy-restart".to_vec()),
            }],
        },
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        },
        &transcript,
    )
    .await;
    assert_eq!(
        modern_phase3,
        NockchainResponse::BatchResult {
            results: vec![BatchResultItem {
                item_id: 2,
                status: BatchResultStatus::Ack,
                error: None,
                envelope: None,
            }],
        }
    );

    // --- Transcript assertions ---

    let rendered = transcript.render();
    assert!(
        rendered.contains("legacy_expected_protocol=Some(\"/nockchain-1-req-res\")"),
        "legacy peer should negotiate gen1; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("modern_expected_protocol=Some(\"/nockchain-2-req-res\")"),
        "modern peer should negotiate gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.contains("legacy peer dropped; rebuilding with same keypair"),
        "restart event should be visible in the transcript:\n{rendered}"
    );
    assert!(
        rendered.contains(&format!("disconnecting from {legacy_peer_id}")),
        "legacy disconnect before restart should be visible; transcript:\n{rendered}"
    );
    assert!(
        !rendered.contains(&format!("disconnecting from {modern_peer_id}")),
        "legacy restart must not disconnect the modern peer; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-request").count() >= 4,
        "modern peer should keep handling batch traffic on gen2 before and after legacy restart; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=batch-result").count() >= 4,
        "modern peer should keep returning batch results on gen2; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=request").count() >= 4,
        "legacy peer should serve singleton gen1 requests before and after restart; transcript:\n{rendered}"
    );
    assert!(
        rendered.matches("shape=result").count() >= 4,
        "legacy peer should return singleton gen1 responses before and after restart; transcript:\n{rendered}"
    );
}
