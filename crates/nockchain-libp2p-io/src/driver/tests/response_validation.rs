use libp2p::core::ConnectedPoint;

use super::*;
use crate::p2p_state::IpBucket;

#[tokio::test]
async fn response_validation_cooldown_follows_the_actual_sdk_connection() {
    for request_is_range in [false, true] {
        for reverse in [false, true] {
            for close_before_consumer in [false, true] {
                let mut rejected_ips = BTreeSet::new();
                let mut rejected_non_first = false;
                for invalid_round in 0..2 {
                    let config = LibP2PConfig::default();
                    let exclusions = PeerExclusions::default();
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
                    let (mut server, _trace, _raw_request, raw_response) =
                        crate::test_support::build_req_res_test_swarm_with_protocol_trace(
                            "validation-server",
                            config.clone(),
                            Keypair::generate_ed25519(),
                            vec![
                                "/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap(),
                                "/ip6/::1/udp/0/quic-v1".parse().unwrap(),
                            ],
                        )
                        .unwrap();
                    let peer = *server.local_peer_id();
                    let local_peer = *client.local_peer_id();
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
                    for address in addresses {
                        client.dial(address).unwrap();
                        loop {
                            tokio::select! {
                                event = client.select_next_some() => match event {
                                    SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                                        assert_eq!(peer_id, peer);
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
                    let snapshot = client.behaviour().recovery.request_endpoints(&peer).unwrap();
                    assert_eq!(snapshot.len(), 2);
                    assert_ne!(snapshot[0].endpoint.ip(), snapshot[1].endpoint.ip());
                    let first = state.lock().await.peer_first_address(&peer)
                        .and_then(|address| quic_socket_addr(&address)).unwrap();
                    for round in 0..=invalid_round {
                        let before: Vec<_> = {
                            let guard = state.lock().await;
                            snapshot.iter().map(|candidate| (
                                candidate.endpoint,
                                guard.prefetch_bucket_window_bytes(IpBucket::from_ip(candidate.endpoint.ip())),
                            )).collect()
                        };
                        send_outbound_request_now(&mut client, &state, &metrics,
                            OutboundRequestContext::new(peer, NockchainRequest::BatchRequest {
                                pow: [0; 16], nonce: round as u64,
                                items: vec![BatchRequestItem {
                                    item_id: 7,
                                    message: if request_is_range {
                                        block_range_with_txs_request_message(50, 2).unwrap()
                                    } else {
                                        crate::messages::block_with_txs_by_height_request_message(50).unwrap()
                                    },
                                }],
                            })).await;
                        let mut expected_socket = None;
                        let mut expected_response_bytes = None;
                        let (request_id, connection_id, selected, message) = loop {
                            tokio::select! {
                                event = client.select_next_some() => match event {
                                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                                        request_response::Event::Message {
                                            peer: responding_peer, connection_id,
                                            message: request_response::Message::Response { request_id, response },
                                        }
                                    )) => {
                                        assert_eq!(responding_peer, peer);
                                        let bytes = response.received_bytes.expect("codec measurement");
                                        let (canonical_bytes, padded_bytes) = expected_response_bytes
                                            .take().expect("server must inject the response first");
                                        assert!(padded_bytes > canonical_bytes);
                                        assert_eq!(bytes, padded_bytes);
                                        let receipt = client.behaviour().recovery.connection_endpoint(&peer, connection_id);
                                        let mut guard = state.lock().await;
                                        let charge = guard.charge_received_response(
                                            request_id, peer, connection_id, receipt, bytes,
                                        ).unwrap();
                                        let selected = guard.outbound_request_context(request_id).unwrap()
                                            .response_endpoint.unwrap();
                                        assert_eq!(selected.connection_id, connection_id);
                                        assert_eq!(Some(selected.endpoint), expected_socket.take());
                                        assert_eq!(charge.is_some(), request_is_range);
                                        if let Some(charge) = charge {
                                            assert_eq!(selected.endpoint, charge.endpoint);
                                        }
                                        for (endpoint, prior) in &before {
                                            let expected = prior + if request_is_range && *endpoint == selected.endpoint { bytes } else { 0 };
                                            assert_eq!(guard.prefetch_bucket_window_bytes(IpBucket::from_ip(endpoint.ip())), expected);
                                        }
                                        break (request_id, connection_id, selected,
                                            request_response::Message::Response { request_id, response });
                                    }
                                    SwarmEvent::Behaviour(NockchainEvent::RequestResponse(
                                        request_response::Event::OutboundFailure { error, .. }
                                    )) => panic!("response exchange failed: {error}"),
                                    _ => {}
                                },
                                event = server.select_next_some() => match event {
                                    SwarmEvent::ConnectionEstablished {
                                        connection_id, endpoint: ConnectedPoint::Listener { local_addr, .. }, ..
                                    } => {
                                        server_sockets.insert(connection_id, quic_socket_addr(&local_addr).unwrap());
                                    }
                                    SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                                        request_response::Event::Message {
                                            connection_id,
                                            message: request_response::Message::Request { channel, .. }, ..
                                        }
                                    )) => {
                                        expected_socket = Some(server_sockets[&connection_id]);
                                        let result = if round == invalid_round {
                                            BatchResultItem {
                                                item_id: 7, status: BatchResultStatus::Result, error: None,
                                                envelope: Some(ResponseEnvelope::heard_block_range_with_txs(vec![
                                                    bundled_block_for_height(50, &[]),
                                                    bundled_block_for_height(52, &[]),
                                                ])),
                                            }
                                        } else {
                                            BatchResultItem {
                                                item_id: 7, status: BatchResultStatus::NotFound, error: None, envelope: None,
                                            }
                                        };
                                        let response = NockchainResponse::BatchResult { results: vec![result] };
                                        let mut encoded = cbor4ii_transport::serde::to_vec(Vec::new(), &response).unwrap();
                                        let canonical_bytes = encoded.len();
                                        encoded.resize(canonical_bytes + 257, 0xff);
                                        assert!(encoded.len() <= config.gen2_batch_max_bytes());
                                        expected_response_bytes = Some((canonical_bytes, encoded.len()));
                                        raw_response.inject_once(encoded);
                                        server.behaviour_mut().request_response.send_response(
                                            channel, crate::behaviour::TransportResponse::outgoing(response),
                                        ).unwrap();
                                    }
                                    _ => {}
                                }
                            }
                        };
                        if round != invalid_round {
                            state.lock().await.remove_outbound_request(request_id);
                            continue;
                        }
                        rejected_ips.insert(selected.endpoint.ip());
                        rejected_non_first |= selected.endpoint != first;
                        if close_before_consumer {
                            close_fallback_test_peer(&mut client, &mut server, &state, connection_id).await;
                            assert!(state.lock().await.connection_address(connection_id).is_none());
                            assert!(client.is_connected(&peer), "the other connection remains useful");
                        }
                        let traffic = build_scripted_traffic_cop(
                            DriverTranscript::default(), Vec::new(), Vec::new(),
                        ).await;
                        let (swarm_tx, mut swarm_rx) = tokio::sync::mpsc::channel(8);
                        let mut equix = equix::EquiXBuilder::new();
                        handle_request_response(
                            peer, connection_id, message, swarm_tx, &mut equix, local_peer,
                            traffic.traffic.clone(), metrics.clone(), state.clone(),
                            runtime_limits_from_config(&LIBP2P_CONFIG), exclusions.clone(),
                        ).await.unwrap();
                        let mut cooldowns = 0;
                        let mut fallbacks = 0;
                        let mut retries = 0;
                        while let Ok(action) = swarm_rx.try_recv() {
                            match action {
                                SwarmAction::RecordExclusionOutcome { outcome, related_peers } => {
                                    assert_eq!(related_peers, vec![peer]);
                                    let cooldown = outcome.address_cooldown.unwrap();
                                    assert_eq!(quic_socket_addr(&cooldown.address), Some(selected.endpoint));
                                    assert_eq!(cooldown.key.expected_peer, Some(peer));
                                    assert!(outcome.ip_exclusion.is_none());
                                    cooldowns += 1;
                                }
                                SwarmAction::QueueKernelRequest { peer_id, request_message } => {
                                    assert_eq!(peer_id, peer);
                                    assert!(matches!(
                                        crate::messages::decode_request_item_message(&request_message).unwrap(),
                                        NockchainDataRequest::BlockByHeight(50)
                                    ));
                                    fallbacks += 1;
                                }
                                SwarmAction::RetryRequests { requests, .. } => {
                                    assert!(!request_is_range);
                                    assert_eq!(requests.len(), 1);
                                    assert_eq!(requests[0].peer_id, peer);
                                    retries += requests.len();
                                }
                                other => panic!("unexpected validation action: {other:?}"),
                            }
                        }
                        assert_eq!(cooldowns, 1);
                        assert_eq!(fallbacks, usize::from(request_is_range));
                        assert_eq!(retries, usize::from(!request_is_range));
                        assert_eq!(traffic.poke_count.load(Ordering::SeqCst), 0,
                            "non-contiguous responses must not reach kernel admission");
                        assert!(state.lock().await.outbound_request_context(request_id).is_none());
                        for candidate in &snapshot {
                            let mut address = libp2p::Multiaddr::empty();
                            address.push(match candidate.endpoint.ip() {
                                std::net::IpAddr::V4(ip) => libp2p::multiaddr::Protocol::Ip4(ip),
                                std::net::IpAddr::V6(ip) => libp2p::multiaddr::Protocol::Ip6(ip),
                            });
                            address.push(libp2p::multiaddr::Protocol::Udp(candidate.endpoint.port()));
                            address.push(libp2p::multiaddr::Protocol::QuicV1);
                            assert_eq!(exclusions.is_address_excluded(&address, Some(peer)),
                                candidate.endpoint == selected.endpoint);
                            assert!(!exclusions.is_ip_excluded(&candidate.endpoint.ip()));
                        }
                    }
                }).await.expect("dual-connection validation must complete");
                }
                assert_eq!(
                    rejected_ips.len(),
                    2,
                    "both SDK-selected numeric endpoints must be covered"
                );
                assert!(
                    rejected_non_first,
                    "the response must also exercise a non-first peer address"
                );
            }
        }
    }
}
