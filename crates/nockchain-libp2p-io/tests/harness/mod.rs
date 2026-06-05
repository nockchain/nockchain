#![allow(dead_code)]

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

fn test_timeout() -> Duration {
    let secs: u64 = std::env::var("NOCKCHAIN_TEST_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    Duration::from_secs(secs)
}

use futures::StreamExt;
use libp2p::swarm::SwarmEvent;
use libp2p::{request_response, Multiaddr, PeerId};
use nockchain_libp2p_io::config::LibP2PConfig;
use nockchain_libp2p_io::test_support::{
    build_req_res_test_swarm_with_protocol_trace, first_common_outbound_protocol, NockchainRequest,
    NockchainResponse, ProtocolTrace, RawRequestInjection, RawResponseInjection, ReqResTestEvent,
    ReqResTestSwarm,
};
use tracing_subscriber::EnvFilter;

static TRACING_INIT: OnceLock<()> = OnceLock::new();

pub fn init_tracing() {
    let _ = TRACING_INIT.get_or_init(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(
                "info,nockchain_libp2p_io=trace,libp2p_swarm=debug,libp2p_request_response=trace",
            )
        });
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

#[derive(Clone)]
pub struct Transcript {
    start: Instant,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Default for Transcript {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Transcript {
    pub fn record(&self, actor: &str, message: impl Into<String>) {
        let line = format!(
            "[{elapsed:>6}ms] {actor}: {message}",
            elapsed = self.start.elapsed().as_millis(),
            actor = actor,
            message = message.into(),
        );
        println!("{line}");
        self.lines
            .lock()
            .expect("transcript mutex poisoned")
            .push(line);
    }

    pub fn render(&self) -> String {
        self.lines
            .lock()
            .expect("transcript mutex poisoned")
            .join("\n")
    }
}

pub struct TranscriptGuard<'a> {
    transcript: &'a Transcript,
    label: &'static str,
}

impl<'a> TranscriptGuard<'a> {
    pub fn new(transcript: &'a Transcript, label: &'static str) -> Self {
        Self { transcript, label }
    }
}

impl Drop for TranscriptGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "\n=== TRANSCRIPT DUMP ({}) ===\n{}\n=== END TRANSCRIPT ===\n",
                self.label,
                self.transcript.render()
            );
        }
    }
}

pub async fn drain_pending_events(peer: &mut TestPeer, transcript: &Transcript) {
    let drain_timeout = Duration::from_millis(50);
    while let Ok(event) = tokio::time::timeout(drain_timeout, peer.swarm.select_next_some()).await {
        transcript.record(peer.name, format!("drain: {event:?}"));
    }
}

pub fn loopback_quic_addr() -> Multiaddr {
    "/ip4/127.0.0.1/udp/0/quic-v1"
        .parse()
        .expect("loopback quic address should parse")
}

pub fn default_test_config() -> LibP2PConfig {
    LibP2PConfig::default()
}

pub fn expected_outbound_generation(config: &LibP2PConfig) -> &'static str {
    if config.req_res_gen2_send_enabled {
        "gen2"
    } else {
        "gen1"
    }
}

pub fn expected_common_protocol(local: &LibP2PConfig, remote: &LibP2PConfig) -> Option<String> {
    first_common_outbound_protocol(local, remote)
}

pub struct TestPeer {
    pub name: &'static str,
    pub protocol_trace: ProtocolTrace,
    pub raw_request_injection: RawRequestInjection,
    pub raw_response_injection: RawResponseInjection,
    pub swarm: ReqResTestSwarm,
}

pub fn build_test_peer(name: &'static str, config: LibP2PConfig) -> TestPeer {
    build_test_peer_with_keypair(name, config, libp2p::identity::Keypair::generate_ed25519())
}

pub fn build_test_peer_with_keypair(
    name: &'static str,
    config: LibP2PConfig,
    keypair: libp2p::identity::Keypair,
) -> TestPeer {
    let (swarm, protocol_trace, raw_request_injection, raw_response_injection) =
        build_req_res_test_swarm_with_protocol_trace(
            name,
            config,
            keypair,
            vec![loopback_quic_addr()],
        )
        .expect("req-res test swarm should build");
    TestPeer {
        name,
        protocol_trace,
        raw_request_injection,
        raw_response_injection,
        swarm,
    }
}

#[derive(Clone)]
pub enum FailureRequesterAction {
    TypedRequest,
    RawRequest(Vec<u8>),
}

#[derive(Clone)]
#[allow(clippy::enum_variant_names)]
pub enum FailureResponderAction {
    NoResponse,
    TypedResponse(NockchainResponse),
    RawResponse(Vec<u8>),
}

pub async fn wait_for_listen_addr(peer: &mut TestPeer, transcript: &Transcript) -> Multiaddr {
    tokio::time::timeout(test_timeout(), async {
        loop {
            match peer.swarm.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    transcript.record(peer.name, format!("listening on {address}"));
                    return address;
                }
                other => {
                    transcript.record(
                        peer.name,
                        format!("while waiting for listen addr saw {other:?}"),
                    );
                }
            }
        }
    })
    .await
    .expect("listen address timeout")
}

pub async fn connect_peers(
    left: &mut TestPeer,
    right: &mut TestPeer,
    right_addr: &Multiaddr,
    transcript: &Transcript,
) {
    left.swarm
        .dial(right_addr.clone())
        .expect("dial should be accepted by swarm");
    transcript.record(left.name, format!("dialing {right_addr}"));

    let left_peer = *left.swarm.local_peer_id();
    let right_peer = *right.swarm.local_peer_id();
    let mut left_connected = false;
    let mut right_connected = false;

    tokio::time::timeout(test_timeout(), async {
        while !(left_connected && right_connected) {
            tokio::select! {
                event = left.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == right_peer => {
                            left_connected = true;
                            transcript.record(left.name, format!("connection established with {peer_id}"));
                        }
                        other => transcript.record(left.name, format!("connect loop saw {other:?}")),
                    }
                }
                event = right.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == left_peer => {
                            right_connected = true;
                            transcript.record(right.name, format!("connection established with {peer_id}"));
                        }
                        other => transcript.record(right.name, format!("connect loop saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("connection timeout");
}

pub async fn disconnect_peers(
    left: &mut TestPeer,
    right: &mut TestPeer,
    right_peer_id: PeerId,
    left_peer_id: PeerId,
    transcript: &Transcript,
) {
    left.swarm
        .disconnect_peer_id(right_peer_id)
        .expect("disconnect should be accepted");
    transcript.record(left.name, format!("disconnecting from {right_peer_id}"));

    let mut left_closed = false;
    let mut right_closed = false;
    tokio::time::timeout(test_timeout(), async {
        while !(left_closed && right_closed) {
            tokio::select! {
                event = left.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == right_peer_id => {
                            left_closed = true;
                            transcript.record(left.name, format!("connection closed with {peer_id}"));
                        }
                        other => transcript.record(left.name, format!("disconnect loop saw {other:?}")),
                    }
                }
                event = right.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == left_peer_id => {
                            right_closed = true;
                            transcript.record(right.name, format!("connection closed with {peer_id}"));
                        }
                        other => transcript.record(right.name, format!("disconnect loop saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("disconnect timeout");
}

pub async fn run_round_trip_observing_request(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    response: NockchainResponse,
    transcript: &Transcript,
) -> (NockchainRequest, NockchainResponse) {
    let request_id = requester
        .swarm
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer, request.clone());
    transcript.record(
        requester.name,
        format!(
            "sent request_id={request_id:?} shape={} toward {responder_peer}",
            describe_request(&request)
        ),
    );

    tokio::time::timeout(test_timeout(), async {
        let mut observed_request = None;
        loop {
            tokio::select! {
                event = responder.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Request { request_id, request, channel } => {
                                    observed_request.get_or_insert_with(|| request.clone());
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "received request_id={request_id:?} from {peer} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                    responder
                                        .swarm
                                        .behaviour_mut()
                                        .request_response
                                        .send_response(channel, response.clone())
                                        .expect("response should send");
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "sent response for request_id={request_id:?} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                }
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "unexpected response_id={request_id:?} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(responder.name, format!("round-trip saw {other:?}")),
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
                                            "received response_id={request_id:?} from {peer} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                    return (
                                        observed_request
                                            .expect("responder must observe request before response"),
                                        response,
                                    );
                                }
                                request_response::Message::Request { request_id, request, .. } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected inbound request_id={request_id:?} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(requester.name, format!("round-trip saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("round-trip timeout")
}

pub async fn run_round_trip(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    response: NockchainResponse,
    transcript: &Transcript,
) -> NockchainResponse {
    let (_observed_request, response) = run_round_trip_observing_request(
        requester, responder, responder_peer, request, response, transcript,
    )
    .await;
    response
}

pub struct RequestFailureObservation {
    pub observed_request: Option<NockchainRequest>,
    pub requester_error: request_response::OutboundFailure,
    pub responder_inbound_failure: Option<String>,
}

async fn capture_trailing_responder_inbound_failure(
    responder: &mut TestPeer,
    transcript: &Transcript,
) -> Option<String> {
    tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            match responder.swarm.select_next_some().await {
                SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(
                    request_response::Event::InboundFailure { peer, error, .. },
                )) => {
                    transcript.record(
                        responder.name,
                        format!("inbound failure from {peer}: {error:?}"),
                    );
                    return Some(format!("{error:?}"));
                }
                other => {
                    transcript.record(responder.name, format!("trailing failure saw {other:?}"));
                }
            }
        }
    })
    .await
    .ok()
    .flatten()
}

pub async fn run_request_until_outbound_failure(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    response: Option<NockchainResponse>,
    transcript: &Transcript,
) -> RequestFailureObservation {
    let action = response
        .map(FailureResponderAction::TypedResponse)
        .unwrap_or(FailureResponderAction::NoResponse);
    run_request_until_outbound_failure_with_actions(
        requester,
        responder,
        responder_peer,
        request,
        FailureRequesterAction::TypedRequest,
        action,
        transcript,
    )
    .await
}

pub async fn run_request_until_outbound_failure_with_action(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    action: FailureResponderAction,
    transcript: &Transcript,
) -> RequestFailureObservation {
    run_request_until_outbound_failure_with_actions(
        requester,
        responder,
        responder_peer,
        request,
        FailureRequesterAction::TypedRequest,
        action,
        transcript,
    )
    .await
}

pub async fn run_request_until_outbound_failure_with_actions(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    requester_action: FailureRequesterAction,
    responder_action: FailureResponderAction,
    transcript: &Transcript,
) -> RequestFailureObservation {
    let request_id = match requester_action.clone() {
        FailureRequesterAction::TypedRequest => requester
            .swarm
            .behaviour_mut()
            .request_response
            .send_request(&responder_peer, request.clone()),
        FailureRequesterAction::RawRequest(raw_request) => {
            let raw_request_len = raw_request.len();
            requester.raw_request_injection.inject_once(raw_request);
            let request_id = requester
                .swarm
                .behaviour_mut()
                .request_response
                .send_request(&responder_peer, request.clone());
            transcript.record(
                requester.name,
                format!(
                    "sent raw malformed request for request_id={request_id:?} placeholder_shape={} bytes={raw_request_len} toward {responder_peer}",
                    describe_request(&request)
                ),
            );
            request_id
        }
    };
    if matches!(requester_action, FailureRequesterAction::TypedRequest) {
        transcript.record(
            requester.name,
            format!(
                "sent request_id={request_id:?} shape={} toward {responder_peer}",
                describe_request(&request)
            ),
        );
    }

    tokio::time::timeout(test_timeout(), async {
        let mut observed_request = None;
        let mut responder_inbound_failure = None;

        loop {
            tokio::select! {
                event = responder.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Request { request_id, request, channel } => {
                                    observed_request.get_or_insert_with(|| request.clone());
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "received request_id={request_id:?} from {peer} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                    match responder_action.clone() {
                                        FailureResponderAction::NoResponse => {}
                                        FailureResponderAction::TypedResponse(response) => {
                                            responder
                                                .swarm
                                                .behaviour_mut()
                                                .request_response
                                                .send_response(channel, response.clone())
                                                .expect("response should send");
                                            transcript.record(
                                                responder.name,
                                                format!(
                                                    "sent response for request_id={request_id:?} shape={}",
                                                    describe_response(&response)
                                                ),
                                            );
                                        }
                                        FailureResponderAction::RawResponse(raw_response) => {
                                            let raw_response_len = raw_response.len();
                                            responder.raw_response_injection.inject_once(raw_response);
                                            responder
                                                .swarm
                                                .behaviour_mut()
                                                .request_response
                                                .send_response(channel, NockchainResponse::Ack { acked: true })
                                                .expect("raw response should send");
                                            transcript.record(
                                                responder.name,
                                                format!(
                                                    "sent raw malformed response for request_id={request_id:?} bytes={raw_response_len}"
                                                ),
                                            );
                                        }
                                    }
                                }
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "unexpected response_id={request_id:?} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                }
                            }
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::InboundFailure { peer, error, .. })) => {
                            transcript.record(
                                responder.name,
                                format!("inbound failure from {peer}: {error:?}"),
                            );
                            responder_inbound_failure.get_or_insert_with(|| format!("{error:?}"));
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::ResponseSent { peer, request_id, .. })) => {
                            transcript.record(
                                responder.name,
                                format!("response sent for request_id={request_id:?} to {peer}"),
                            );
                        }
                        other => transcript.record(responder.name, format!("failure loop saw {other:?}")),
                    }
                }
                event = requester.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                            transcript.record(
                                requester.name,
                                format!(
                                    "outbound failure for request_id={request_id:?} to {peer}: {error:?}"
                                ),
                            );
                            let responder_inbound_failure = if responder_inbound_failure.is_none()
                                && matches!(requester_action, FailureRequesterAction::RawRequest(_))
                            {
                                capture_trailing_responder_inbound_failure(responder, transcript)
                                    .await
                            } else {
                                responder_inbound_failure
                            };
                            return RequestFailureObservation {
                                observed_request,
                                requester_error: error,
                                responder_inbound_failure,
                            };
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected response_id={request_id:?} from {peer} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                    panic!("expected outbound failure, got response");
                                }
                                request_response::Message::Request { request_id, request, .. } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected inbound request_id={request_id:?} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(requester.name, format!("failure loop saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("outbound failure timeout")
}

pub async fn run_request_until_outbound_timeout(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    request: NockchainRequest,
    transcript: &Transcript,
) -> RequestFailureObservation {
    let request_id = requester
        .swarm
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer, request.clone());
    transcript.record(
        requester.name,
        format!(
            "sent request_id={request_id:?} shape={} toward {responder_peer}",
            describe_request(&request)
        ),
    );

    tokio::time::timeout(test_timeout(), async {
        let mut responder_inbound_failure = None;
        let mut held_channels = Vec::new();

        loop {
            tokio::select! {
                event = responder.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Request { request_id, request, channel } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "received request_id={request_id:?} from {peer} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "holding response channel open for request_id={request_id:?}"
                                        ),
                                    );
                                    held_channels.push(channel);
                                }
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "unexpected response_id={request_id:?} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                }
                            }
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::InboundFailure { peer, error, .. })) => {
                            transcript.record(
                                responder.name,
                                format!("inbound failure from {peer}: {error:?}"),
                            );
                            responder_inbound_failure.get_or_insert_with(|| format!("{error:?}"));
                        }
                        other => transcript.record(responder.name, format!("timeout loop saw {other:?}")),
                    }
                }
                event = requester.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                            transcript.record(
                                requester.name,
                                format!(
                                    "outbound failure for request_id={request_id:?} to {peer}: {error:?}"
                                ),
                            );
                            return RequestFailureObservation {
                                observed_request: None,
                                requester_error: error,
                                responder_inbound_failure,
                            };
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected response_id={request_id:?} from {peer} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                    panic!("expected outbound timeout, got response");
                                }
                                request_response::Message::Request { request_id, request, .. } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected inbound request_id={request_id:?} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                }
                            }
                        }
                        other => transcript.record(requester.name, format!("timeout loop saw {other:?}")),
                    }
                }
            }
        }
    })
    .await
    .expect("outbound timeout")
}

pub async fn run_request_until_disconnect_cleanup_failure(
    requester: &mut TestPeer,
    responder: &mut TestPeer,
    responder_peer: PeerId,
    requester_peer: PeerId,
    request: NockchainRequest,
    transcript: &Transcript,
) -> RequestFailureObservation {
    let request_id = requester
        .swarm
        .behaviour_mut()
        .request_response
        .send_request(&responder_peer, request.clone());
    transcript.record(
        requester.name,
        format!(
            "sent request_id={request_id:?} shape={} toward {responder_peer}",
            describe_request(&request)
        ),
    );

    tokio::time::timeout(test_timeout(), async {
        let mut disconnect_triggered = false;
        let mut observed_request = None;
        let mut requester_error = None;
        let mut responder_inbound_failure = None;
        let mut requester_closed = false;
        let mut responder_closed = false;

        loop {
            tokio::select! {
                event = responder.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Request { request_id, request, .. } => {
                                    observed_request.get_or_insert_with(|| request.clone());
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "received request_id={request_id:?} from {peer} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                    if !disconnect_triggered {
                                        responder
                                            .swarm
                                            .disconnect_peer_id(requester_peer)
                                            .expect("disconnect should be accepted");
                                        disconnect_triggered = true;
                                        transcript.record(
                                            responder.name,
                                            format!(
                                                "disconnecting from {requester_peer} with in-flight request_id={request_id:?}"
                                            ),
                                        );
                                    }
                                }
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        responder.name,
                                        format!(
                                            "unexpected response_id={request_id:?} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                }
                            }
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::InboundFailure { peer, error, .. })) => {
                            transcript.record(
                                responder.name,
                                format!("inbound failure from {peer}: {error:?}"),
                            );
                            responder_inbound_failure.get_or_insert_with(|| format!("{error:?}"));
                        }
                        SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == requester_peer => {
                            responder_closed = true;
                            transcript.record(
                                responder.name,
                                format!("connection closed with {peer_id}"),
                            );
                        }
                        other => transcript.record(
                            responder.name,
                            format!("disconnect cleanup saw {other:?}"),
                        ),
                    }
                }
                event = requester.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::OutboundFailure { peer, request_id, error, .. })) => {
                            transcript.record(
                                requester.name,
                                format!(
                                    "outbound failure for request_id={request_id:?} to {peer}: {error:?}"
                                ),
                            );
                            requester_error.get_or_insert(error);
                        }
                        SwarmEvent::Behaviour(ReqResTestEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                            match message {
                                request_response::Message::Response { request_id, response } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected response_id={request_id:?} from {peer} shape={}",
                                            describe_response(&response)
                                        ),
                                    );
                                    panic!("expected disconnect cleanup failure, got response");
                                }
                                request_response::Message::Request { request_id, request, .. } => {
                                    transcript.record(
                                        requester.name,
                                        format!(
                                            "unexpected inbound request_id={request_id:?} shape={}",
                                            describe_request(&request)
                                        ),
                                    );
                                }
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == responder_peer => {
                            requester_closed = true;
                            transcript.record(
                                requester.name,
                                format!("connection closed with {peer_id}"),
                            );
                        }
                        other => transcript.record(
                            requester.name,
                            format!("disconnect cleanup saw {other:?}"),
                        ),
                    }
                }
            }

            if disconnect_triggered && requester_closed && responder_closed {
                if let Some(requester_error) = requester_error.take() {
                    return RequestFailureObservation {
                        observed_request,
                        requester_error,
                        responder_inbound_failure,
                    };
                }
            }
        }
    })
    .await
    .expect("disconnect cleanup timeout")
}

fn describe_request(request: &NockchainRequest) -> &'static str {
    match request {
        NockchainRequest::Request { .. } => "request",
        NockchainRequest::Gossip { .. } => "gossip",
        NockchainRequest::AuthenticatedGossip { .. } => "authenticated-gossip",
        NockchainRequest::BatchRequest { .. } => "batch-request",
    }
}

fn describe_response(response: &NockchainResponse) -> &'static str {
    match response {
        NockchainResponse::Ack { .. } => "ack",
        NockchainResponse::Result { .. } => "result",
        NockchainResponse::BatchResult { .. } => "batch-result",
    }
}
