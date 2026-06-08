mod harness;

use harness::{
    build_test_peer, connect_peers, default_test_config, drain_pending_events,
    expected_common_protocol, init_tracing, run_request_until_outbound_failure,
    run_request_until_outbound_failure_with_action,
    run_request_until_outbound_failure_with_actions, run_request_until_outbound_timeout,
    run_round_trip, wait_for_listen_addr, FailureRequesterAction, FailureResponderAction,
    Transcript, TranscriptGuard,
};
use libp2p::request_response;
use nockchain_libp2p_io::config::LibP2PConfig;
use nockchain_libp2p_io::peer_stats::PeerReqResGeneration;
use nockchain_libp2p_io::test_support::{
    BatchRequestItem, BatchResultItem, BatchResultStatus, NockchainRequest, NockchainResponse,
    ReqResFailureObservabilityProbe, ReqResGeneration, ResponseEnvelope,
};
use serde_bytes::ByteBuf;

fn limited_gen2_config(max_bytes: usize) -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        gen2_batch_max_bytes: max_bytes,
        ..default_test_config()
    }
}

fn encoded_request_bytes(request: &NockchainRequest) -> usize {
    cbor4ii::serde::to_vec(Vec::new(), request)
        .expect("request should encode")
        .len()
}

fn encoded_response_bytes(response: &NockchainResponse) -> usize {
    cbor4ii::serde::to_vec(Vec::new(), response)
        .expect("response should encode")
        .len()
}

fn timeout_gen2_config(timeout_secs: u64) -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: true,
        req_res_gen2_send_enabled: true,
        request_response_timeout_secs: timeout_secs,
        ..default_test_config()
    }
}

fn timeout_gen1_config(timeout_secs: u64) -> LibP2PConfig {
    LibP2PConfig {
        req_res_gen2_accept_enabled: false,
        req_res_gen2_send_enabled: false,
        request_response_timeout_secs: timeout_secs,
        ..default_test_config()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_oversize_request_fails_on_codec_boundary() {
    init_tracing();

    let max_bytes = 128;
    let requester_config = limited_gen2_config(max_bytes);
    let responder_config = limited_gen2_config(max_bytes);
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0xAA; 256]),
        }],
    };
    let encoded_bytes = encoded_request_bytes(&request);
    assert!(encoded_bytes > max_bytes);

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "oversize request expected_common_protocol={:?} generation=gen2 observed_bytes={encoded_bytes} configured_cap={max_bytes} reject_reason=codec_request_too_large",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config);
    let mut responder = build_test_peer("responder", responder_config);
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure(
        &mut requester, &mut responder, responder_peer_id, request, None, &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    let rendered = transcript.render();
    assert!(rendered.contains("observed_bytes="));
    assert!(rendered.contains("configured_cap=128"));
    assert!(rendered.contains("reject_reason=codec_request_too_large"));
    assert!(rendered.contains("outbound failure"));
    assert!(!rendered.contains("received request_id="));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_oversize_response_fails_on_codec_boundary() {
    init_tracing();

    let max_bytes = 128;
    let requester_config = limited_gen2_config(max_bytes);
    let responder_config = limited_gen2_config(max_bytes);
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 7,
            message: ByteBuf::from(b"small-request".to_vec()),
        }],
    };
    let response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(
                String::from("oversize-tx"),
                vec![0xBB; 256],
            )),
        }],
    };
    let encoded_bytes = encoded_response_bytes(&response);
    assert!(encoded_bytes > max_bytes);

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "oversize response expected_common_protocol={:?} generation=gen2 observed_bytes={encoded_bytes} configured_cap={max_bytes} reject_reason=codec_response_too_large",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config);
    let mut responder = build_test_peer("responder", responder_config);
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        Some(response),
        &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    let rendered = transcript.render();
    assert!(rendered.contains("observed_bytes="));
    assert!(rendered.contains("configured_cap=128"));
    assert!(rendered.contains("reject_reason=codec_response_too_large"));
    assert!(rendered.contains("sent response for request_id="));
    assert!(rendered.contains("outbound failure"));
}

/// Unlike semantic malformed request coverage, this injects raw top-level bytes
/// that fail before any typed `BatchRequest` exists on the inbound side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_malformed_top_level_request_bytes_fail_before_decode() {
    init_tracing();

    let requester_config = limited_gen2_config(256);
    let responder_config = limited_gen2_config(256);
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 8,
            message: ByteBuf::from(b"small-request".to_vec()),
        }],
    };
    let malformed_request = vec![0xA1];

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "malformed top-level request bytes expected_common_protocol={:?} generation=gen2 raw_bytes={} reject_reason=request_decode_before_batch_request",
            expected_common_protocol(&requester_config, &responder_config),
            malformed_request.len(),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config);
    let mut responder = build_test_peer("responder", responder_config);
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure_with_actions(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        FailureRequesterAction::RawRequest(malformed_request),
        FailureResponderAction::NoResponse,
        &transcript,
    )
    .await;

    assert_eq!(observation.observed_request, None);
    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    let rendered = transcript.render();
    assert!(rendered.contains("reject_reason=request_decode_before_batch_request"));
    assert!(rendered.contains("sent raw malformed request"));
    assert!(rendered.contains("outbound failure"));
    assert!(!rendered.contains("received request_id="));
}

/// Unlike the malformed `ResponseEnvelope` coverage, this injects raw top-level
/// bytes that fail before any typed `NockchainResponse` exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_malformed_top_level_response_bytes_fail_before_decode() {
    init_tracing();

    let requester_config = limited_gen2_config(256);
    let responder_config = limited_gen2_config(256);
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 9,
            message: ByteBuf::from(b"small-request".to_vec()),
        }],
    };
    let malformed_response = vec![0xA1];

    let transcript = Transcript::default();
    transcript.record(
        "scenario",
        format!(
            "malformed top-level response bytes expected_common_protocol={:?} generation=gen2 raw_bytes={} reject_reason=response_decode_before_nockchain_response",
            expected_common_protocol(&requester_config, &responder_config),
            malformed_response.len(),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config);
    let mut responder = build_test_peer("responder", responder_config);
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure_with_action(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request.clone(),
        FailureResponderAction::RawResponse(malformed_response),
        &transcript,
    )
    .await;

    assert_eq!(observation.observed_request, Some(request));
    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    let rendered = transcript.render();
    assert!(rendered.contains("reject_reason=response_decode_before_nockchain_response"));
    assert!(rendered.contains("sent raw malformed response"));
    assert!(rendered.contains("outbound failure"));
    assert!(!rendered.contains("shape=batch-result"));
}

/// After an oversize request is rejected at the codec boundary, verify that the
/// peer pair can recover and complete a small follow-up request-response cycle.
/// The codec-layer rejection typically tears down the substream (or connection),
/// so this test reconnects before the follow-up to prove the peers are not left
/// in a permanently broken state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_oversize_request_recovery_with_small_followup() {
    init_tracing();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "oversize_request_recovery");

    let max_bytes = 128;
    let requester_config = limited_gen2_config(max_bytes);
    let responder_config = limited_gen2_config(max_bytes);

    // --- Phase 1: trigger oversize request failure ---
    let oversize_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 1,
            message: ByteBuf::from(vec![0xAA; 256]),
        }],
    };
    assert!(encoded_request_bytes(&oversize_request) > max_bytes);

    transcript.record("scenario", "phase-1: oversize request → codec rejection");

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure(
        &mut requester, &mut responder, responder_peer_id, oversize_request, None, &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    // --- Phase 2: drain residual events, reconnect, then small follow-up ---
    transcript.record("scenario", "phase-2: recovery → small follow-up round-trip");
    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;

    // Reconnect using the original listen address — the codec failure may have
    // closed the connection but the listener remains active.
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let small_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 1,
        items: vec![BatchRequestItem {
            item_id: 2,
            message: ByteBuf::from(b"tiny".to_vec()),
        }],
    };
    let small_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 2,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(String::from("small-tx"), b"ok")),
        }],
    };
    assert!(encoded_request_bytes(&small_request) <= max_bytes);
    assert!(encoded_response_bytes(&small_response) <= max_bytes);

    let got = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        small_request,
        small_response.clone(),
        &transcript,
    )
    .await;

    // Confirm the small follow-up completed successfully.
    match (&got, &small_response) {
        (
            NockchainResponse::BatchResult { results: got_r },
            NockchainResponse::BatchResult { results: exp_r },
        ) => {
            assert_eq!(got_r.len(), exp_r.len(), "result count mismatch");
            assert_eq!(got_r[0].item_id, exp_r[0].item_id);
        }
        _ => panic!("expected BatchResult, got {got:?}"),
    }

    let rendered = transcript.render();
    assert!(rendered.contains("phase-2"));
    assert!(rendered.contains("received response_id="));
}

/// After malformed top-level request bytes fail decode before any typed
/// `BatchRequest` exists, the pair should recover after reconnect and complete
/// a normal follow-up round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_malformed_top_level_request_recovery_with_small_followup() {
    init_tracing();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "malformed_top_level_request_recovery");

    let requester_config = limited_gen2_config(256);
    let responder_config = limited_gen2_config(256);

    transcript.record(
        "scenario",
        "phase-1: malformed top-level request bytes -> decode failure before typed request",
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let malformed_request_placeholder = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 10,
            message: ByteBuf::from(b"decode-me".to_vec()),
        }],
    };
    let observation = run_request_until_outbound_failure_with_actions(
        &mut requester,
        &mut responder,
        responder_peer_id,
        malformed_request_placeholder,
        FailureRequesterAction::RawRequest(vec![0xA1]),
        FailureResponderAction::NoResponse,
        &transcript,
    )
    .await;

    assert_eq!(observation.observed_request, None);
    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    transcript.record(
        "scenario", "phase-2: reconnect -> small follow-up round-trip",
    );
    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let small_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 1,
        items: vec![BatchRequestItem {
            item_id: 11,
            message: ByteBuf::from(b"tiny".to_vec()),
        }],
    };
    let small_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 11,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(String::from("small-tx"), b"ok")),
        }],
    };

    let got = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        small_request,
        small_response.clone(),
        &transcript,
    )
    .await;

    match (&got, &small_response) {
        (
            NockchainResponse::BatchResult { results: got_r },
            NockchainResponse::BatchResult { results: exp_r },
        ) => {
            assert_eq!(got_r.len(), exp_r.len(), "result count mismatch");
            assert_eq!(got_r[0].item_id, exp_r[0].item_id);
        }
        _ => panic!("expected BatchResult, got {got:?}"),
    }

    let rendered = transcript.render();
    assert!(rendered.contains("decode failure before typed request"));
    assert!(rendered.contains("sent raw malformed request"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("phase-2: reconnect -> small follow-up round-trip"));
    assert!(rendered.contains("received response_id="));
}

/// After an oversize response is rejected at the codec boundary, verify that
/// the peer pair can recover and complete a small follow-up request-response
/// cycle. The oversize response tears down the substream/connection on the
/// requester side, so reconnection is expected before the follow-up succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_oversize_response_recovery_with_small_followup() {
    init_tracing();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "oversize_response_recovery");

    let max_bytes = 128;
    let requester_config = limited_gen2_config(max_bytes);
    let responder_config = limited_gen2_config(max_bytes);

    // --- Phase 1: trigger oversize response failure ---
    let request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 7,
            message: ByteBuf::from(b"small-request".to_vec()),
        }],
    };
    let oversize_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(
                String::from("oversize-tx"),
                vec![0xBB; 256],
            )),
        }],
    };
    assert!(encoded_response_bytes(&oversize_response) > max_bytes);

    transcript.record("scenario", "phase-1: oversize response → codec rejection");

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observation = run_request_until_outbound_failure(
        &mut requester,
        &mut responder,
        responder_peer_id,
        request,
        Some(oversize_response),
        &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    // --- Phase 2: drain, reconnect, small follow-up ---
    transcript.record(
        "scenario", "phase-2: recovery → small follow-up round-trip (reconnect expected)",
    );
    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;

    // Reconnect using the original listen address.
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let small_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 1,
        items: vec![BatchRequestItem {
            item_id: 8,
            message: ByteBuf::from(b"tiny".to_vec()),
        }],
    };
    let small_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 8,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(String::from("small-tx"), b"ok")),
        }],
    };
    assert!(encoded_request_bytes(&small_request) <= max_bytes);
    assert!(encoded_response_bytes(&small_response) <= max_bytes);

    let got = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        small_request,
        small_response.clone(),
        &transcript,
    )
    .await;

    match (&got, &small_response) {
        (
            NockchainResponse::BatchResult { results: got_r },
            NockchainResponse::BatchResult { results: exp_r },
        ) => {
            assert_eq!(got_r.len(), exp_r.len(), "result count mismatch");
            assert_eq!(got_r[0].item_id, exp_r[0].item_id);
        }
        _ => panic!("expected BatchResult, got {got:?}"),
    }

    let rendered = transcript.render();
    assert!(rendered.contains("phase-2"));
    assert!(rendered.contains("reconnect expected"));
    assert!(rendered.contains("received response_id="));
}

/// After a malformed top-level response fails decode before any typed
/// `NockchainResponse` exists, the requester should recover after reconnect
/// and complete a normal follow-up round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_malformed_top_level_response_recovery_with_small_followup() {
    init_tracing();
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "malformed_top_level_response_recovery");

    let requester_config = limited_gen2_config(256);
    let responder_config = limited_gen2_config(256);

    transcript.record(
        "scenario",
        "phase-1: malformed top-level response bytes -> decode failure before typed response",
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let responder_peer_id = *responder.swarm.local_peer_id();

    let _requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let malformed_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![BatchRequestItem {
            item_id: 10,
            message: ByteBuf::from(b"decode-me".to_vec()),
        }],
    };
    let observation = run_request_until_outbound_failure_with_action(
        &mut requester,
        &mut responder,
        responder_peer_id,
        malformed_request,
        FailureResponderAction::RawResponse(vec![0xA1]),
        &transcript,
    )
    .await;

    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Io(_)
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Timeout
    ));

    transcript.record(
        "scenario", "phase-2: reconnect -> small follow-up round-trip",
    );
    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let small_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 1,
        items: vec![BatchRequestItem {
            item_id: 11,
            message: ByteBuf::from(b"tiny".to_vec()),
        }],
    };
    let small_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 11,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(ResponseEnvelope::heard_tx(String::from("small-tx"), b"ok")),
        }],
    };

    let got = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        small_request,
        small_response.clone(),
        &transcript,
    )
    .await;

    match (&got, &small_response) {
        (
            NockchainResponse::BatchResult { results: got_r },
            NockchainResponse::BatchResult { results: exp_r },
        ) => {
            assert_eq!(got_r.len(), exp_r.len(), "result count mismatch");
            assert_eq!(got_r[0].item_id, exp_r[0].item_id);
        }
        _ => panic!("expected BatchResult, got {got:?}"),
    }

    let rendered = transcript.render();
    assert!(rendered.contains("decode failure before typed response"));
    assert!(rendered.contains("sent raw malformed response"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("phase-2: reconnect -> small follow-up round-trip"));
    assert!(rendered.contains("received response_id="));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen2_timeout_updates_observability_and_recovers() {
    init_tracing();

    let timeout_secs = 1;
    let requester_config = timeout_gen2_config(timeout_secs);
    let responder_config = timeout_gen2_config(timeout_secs);
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "gen2_timeout_observability");
    transcript.record(
        "scenario",
        format!(
            "gen2 timeout observability expected_common_protocol={:?} request_timeout_secs={timeout_secs}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observability = ReqResFailureObservabilityProbe::new(requester_peer_id, &requester_config);
    observability
        .observe_connected_peer(
            responder_peer_id,
            &responder_addr,
            &requester_addr,
            ReqResGeneration::Gen2,
        )
        .await;

    let timed_out_request = NockchainRequest::BatchRequest {
        pow: Default::default(),
        nonce: 0,
        items: vec![
            BatchRequestItem {
                item_id: 1,
                message: ByteBuf::from(b"gen2-timeout-item-1".to_vec()),
            },
            BatchRequestItem {
                item_id: 2,
                message: ByteBuf::from(b"gen2-timeout-item-2".to_vec()),
            },
        ],
    };

    let observation = run_request_until_outbound_timeout(
        &mut requester,
        &mut responder,
        responder_peer_id,
        timed_out_request.clone(),
        &transcript,
    )
    .await;
    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Timeout
    ));

    observability
        .observe_outbound_failure(
            responder_peer_id,
            ReqResGeneration::Gen2,
            timed_out_request,
            request_response::OutboundFailure::Timeout,
        )
        .await;

    let counters = observability.snapshot();
    assert_eq!(counters.request_failed, 1);
    assert_eq!(counters.gen1_outbound_failures, 0);
    assert_eq!(counters.gen1_outbound_timeouts, 0);
    assert_eq!(counters.gen2_outbound_failures, 1);
    assert_eq!(counters.gen2_outbound_timeouts, 1);

    let peer_stats = observability.peer_stats_snapshot();
    let entry = peer_stats
        .peers
        .iter()
        .find(|entry| entry.peer_id == responder_peer_id.to_base58())
        .expect("expected peer stats entry for responder");
    assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen2);
    assert_eq!(entry.request_count, 2);
    assert_eq!(entry.failure_count, 2);
    assert_eq!(entry.timeout_count, 2);

    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;

    let followup_response = NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 7,
            status: BatchResultStatus::Ack,
            error: None,
            envelope: None,
        }],
    };
    let observed = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::BatchRequest {
            pow: Default::default(),
            nonce: 1,
            items: vec![BatchRequestItem {
                item_id: 7,
                message: ByteBuf::from(b"gen2-timeout-followup".to_vec()),
            }],
        },
        followup_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(observed, followup_response);

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-2-req-res\")"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("shape=batch-result"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_res_gen1_timeout_updates_observability_and_recovers() {
    init_tracing();

    let timeout_secs = 1;
    let requester_config = timeout_gen1_config(timeout_secs);
    let responder_config = timeout_gen1_config(timeout_secs);
    let transcript = Transcript::default();
    let _guard = TranscriptGuard::new(&transcript, "gen1_timeout_observability");
    transcript.record(
        "scenario",
        format!(
            "gen1 timeout observability expected_common_protocol={:?} request_timeout_secs={timeout_secs}",
            expected_common_protocol(&requester_config, &responder_config),
        ),
    );

    let mut requester = build_test_peer("requester", requester_config.clone());
    let mut responder = build_test_peer("responder", responder_config.clone());
    let requester_peer_id = *requester.swarm.local_peer_id();
    let responder_peer_id = *responder.swarm.local_peer_id();

    let requester_addr = wait_for_listen_addr(&mut requester, &transcript).await;
    let responder_addr = wait_for_listen_addr(&mut responder, &transcript).await;
    connect_peers(&mut requester, &mut responder, &responder_addr, &transcript).await;

    let observability = ReqResFailureObservabilityProbe::new(requester_peer_id, &requester_config);
    observability
        .observe_connected_peer(
            responder_peer_id,
            &responder_addr,
            &requester_addr,
            ReqResGeneration::Gen1,
        )
        .await;

    let timed_out_request = NockchainRequest::Request {
        pow: Default::default(),
        nonce: 0,
        message: ByteBuf::from(b"gen1-timeout-request".to_vec()),
    };

    let observation = run_request_until_outbound_timeout(
        &mut requester,
        &mut responder,
        responder_peer_id,
        timed_out_request.clone(),
        &transcript,
    )
    .await;
    assert!(matches!(
        observation.requester_error,
        request_response::OutboundFailure::Timeout
    ));

    observability
        .observe_outbound_failure(
            responder_peer_id,
            ReqResGeneration::Gen1,
            timed_out_request,
            request_response::OutboundFailure::Timeout,
        )
        .await;

    let counters = observability.snapshot();
    assert_eq!(counters.request_failed, 1);
    assert_eq!(counters.gen1_outbound_failures, 1);
    assert_eq!(counters.gen1_outbound_timeouts, 1);
    assert_eq!(counters.gen2_outbound_failures, 0);
    assert_eq!(counters.gen2_outbound_timeouts, 0);

    let peer_stats = observability.peer_stats_snapshot();
    let entry = peer_stats
        .peers
        .iter()
        .find(|entry| entry.peer_id == responder_peer_id.to_base58())
        .expect("expected peer stats entry for responder");
    assert_eq!(entry.protocol_generation, PeerReqResGeneration::Gen1);
    assert_eq!(entry.request_count, 1);
    assert_eq!(entry.failure_count, 1);
    assert_eq!(entry.timeout_count, 1);

    drain_pending_events(&mut requester, &transcript).await;
    drain_pending_events(&mut responder, &transcript).await;

    let followup_response = NockchainResponse::Result {
        message: ByteBuf::from(b"gen1-timeout-followup-response".to_vec()),
    };
    let observed = run_round_trip(
        &mut requester,
        &mut responder,
        responder_peer_id,
        NockchainRequest::Request {
            pow: Default::default(),
            nonce: 1,
            message: ByteBuf::from(b"gen1-timeout-followup".to_vec()),
        },
        followup_response.clone(),
        &transcript,
    )
    .await;
    assert_eq!(observed, followup_response);

    let rendered = transcript.render();
    assert!(rendered.contains("expected_common_protocol=Some(\"/nockchain-1-req-res\")"));
    assert!(rendered.contains("outbound failure"));
    assert!(rendered.contains("shape=result"));
}
