use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::StreamExt;
use libp2p::identify::Event as IdentifyEvent;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, Message, ResponseChannel};
use libp2p::swarm::{ConnectionId as NativeConnectionId, SwarmEvent};
use libp2p::{kad, Multiaddr, PeerId, Swarm};
use tracing::{debug, trace, warn};

use crate::behaviour::{NockchainBehaviour, NockchainEvent};
use crate::messages::NockchainResponse;
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_util::MultiaddrExt;
use crate::peer_policy::PeerExclusions;
use crate::transport::{
    transport_channels, ConnectionCloseCause, DiscoveryEvent, DiscoveryFailure, DiscoverySource,
    TransportActorChannels, TransportCommand, TransportEvent, TransportHandle,
};
use crate::types::{
    ConnectionDirection, ConnectionId, DialFailure, InboundFailure, InboundRequestId, PeerAddress,
    RequestFailure, RequestId,
};

struct PendingInbound {
    native_id: request_response::InboundRequestId,
    channel: ResponseChannel<NockchainResponse>,
}

pub(crate) fn spawn_libp2p_transport(
    swarm: Swarm<NockchainBehaviour>,
    peer_exclusions: PeerExclusions,
    metrics: Arc<NockchainP2PMetrics>,
    bootstrap_peers: Vec<Multiaddr>,
    command_capacity: usize,
) -> TransportHandle {
    let local_node_id = *swarm.local_peer_id();
    let (handle, channels) = transport_channels(local_node_id, command_capacity);
    tokio::spawn(run_libp2p_transport(
        swarm, peer_exclusions, metrics, bootstrap_peers, channels,
    ));
    handle
}

async fn run_libp2p_transport(
    mut swarm: Swarm<NockchainBehaviour>,
    peer_exclusions: PeerExclusions,
    metrics: Arc<NockchainP2PMetrics>,
    bootstrap_peers: Vec<Multiaddr>,
    mut channels: TransportActorChannels,
) {
    let mut next_connection_id = 1u64;
    let mut next_inbound_id = 1u64;
    let mut native_connections = HashMap::<NativeConnectionId, ConnectionId>::new();
    let mut local_connections = HashMap::<ConnectionId, NativeConnectionId>::new();
    let mut outbound_requests = HashMap::<request_response::OutboundRequestId, RequestId>::new();
    let mut pending_inbound = HashMap::<InboundRequestId, PendingInbound>::new();
    let mut native_inbound = HashMap::<request_response::InboundRequestId, InboundRequestId>::new();

    loop {
        tokio::select! {
            command = channels.commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                if process_command(
                    command,
                    &mut swarm,
                    &bootstrap_peers,
                    &mut outbound_requests,
                    &mut pending_inbound,
                    &mut native_inbound,
                    &local_connections,
                    &channels,
                ).await {
                    break;
                }
            }
            event = swarm.next() => {
                let Some(event) = event else {
                    let _ = channels.events.send(TransportEvent::Fatal {
                        error: String::from("libp2p swarm event stream ended"),
                    }).await;
                    break;
                };
                process_swarm_event(
                    event,
                    &mut swarm,
                    &peer_exclusions,
                    &metrics,
                    &mut next_connection_id,
                    &mut next_inbound_id,
                    &mut native_connections,
                    &mut local_connections,
                    &mut outbound_requests,
                    &mut pending_inbound,
                    &mut native_inbound,
                    &channels,
                ).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_command(
    command: TransportCommand,
    swarm: &mut Swarm<NockchainBehaviour>,
    bootstrap_peers: &[Multiaddr],
    outbound_requests: &mut HashMap<request_response::OutboundRequestId, RequestId>,
    pending_inbound: &mut HashMap<InboundRequestId, PendingInbound>,
    native_inbound: &mut HashMap<request_response::InboundRequestId, InboundRequestId>,
    local_connections: &HashMap<ConnectionId, NativeConnectionId>,
    channels: &TransportActorChannels,
) -> bool {
    match command {
        TransportCommand::Dial { peer, addresses } => {
            for address in addresses {
                let multiaddr = socket_to_multiaddr(address, Some(peer));
                if let Err(error) = swarm.dial(multiaddr) {
                    let _ = channels
                        .events
                        .send(TransportEvent::DialFailed {
                            peer: Some(peer),
                            address: Some(address),
                            failure: normalize_dial_error(&error),
                        })
                        .await;
                }
            }
        }
        TransportCommand::DialBootstrap => {
            for address in bootstrap_peers {
                if let Err(error) = swarm.dial(address.clone()) {
                    let _ = channels
                        .events
                        .send(TransportEvent::DialFailed {
                            peer: peer_id_from_multiaddr(address),
                            address: multiaddr_to_peer_address(address),
                            failure: normalize_dial_error(&error),
                        })
                        .await;
                }
            }
        }
        TransportCommand::DialKnown { peers } => {
            let peers = peers.into_iter().collect::<HashSet<_>>();
            for address in bootstrap_peers {
                if peer_id_from_multiaddr(address).is_some_and(|peer| peers.contains(&peer)) {
                    if let Err(error) = swarm.dial(address.clone()) {
                        let _ = channels
                            .events
                            .send(TransportEvent::DialFailed {
                                peer: peer_id_from_multiaddr(address),
                                address: multiaddr_to_peer_address(address),
                                failure: normalize_dial_error(&error),
                            })
                            .await;
                    }
                }
            }
        }
        TransportCommand::SendRequest { id, peer, request } => {
            let native_id = swarm
                .behaviour_mut()
                .request_response
                .send_request(&peer, request);
            outbound_requests.insert(native_id, id);
        }
        TransportCommand::CompleteInbound { id, response } => {
            if let Some(pending) = pending_inbound.remove(&id) {
                native_inbound.remove(&pending.native_id);
                if let Some(response) = response {
                    let _ = swarm
                        .behaviour_mut()
                        .request_response
                        .send_response(pending.channel, response);
                }
            }
        }
        TransportCommand::DisconnectPeer { peer } => {
            let _ = swarm.disconnect_peer_id(peer);
        }
        TransportCommand::CloseConnection { connection } => {
            if let Some(native) = local_connections.get(&connection) {
                swarm.close_connection(*native);
            }
        }
        TransportCommand::BootstrapDiscovery | TransportCommand::RefreshDiscovery => {
            if swarm.behaviour_mut().kad.bootstrap().is_err() {
                let _ = channels
                    .events
                    .send(TransportEvent::Discovery(DiscoveryEvent::BootstrapFailed {
                        failure: DiscoveryFailure::NoKnownPeers,
                    }))
                    .await;
            }
        }
        TransportCommand::RemoveDiscoveredAddress { peer, address } => {
            let removals = {
                let mut removals = Vec::new();
                for bucket in swarm.behaviour_mut().kad.kbuckets() {
                    for entry in bucket.iter() {
                        let candidate_peer = entry.node.key.into_preimage();
                        if peer.is_some_and(|peer| peer != candidate_peer) {
                            continue;
                        }
                        removals.extend(
                            entry
                                .node
                                .value
                                .iter()
                                .filter(|candidate| {
                                    multiaddr_to_peer_address(candidate) == Some(address)
                                })
                                .cloned()
                                .map(|candidate| (candidate_peer, candidate)),
                        );
                    }
                }
                removals
            };
            for (peer, address) in removals {
                let _ = swarm.behaviour_mut().kad.remove_address(&peer, &address);
                let _ = swarm
                    .behaviour_mut()
                    .peer_store
                    .store_mut()
                    .remove_address(&peer, &address);
            }
        }
        TransportCommand::Shutdown => return true,
    }
    false
}

#[allow(clippy::too_many_arguments)]
async fn process_swarm_event(
    event: SwarmEvent<NockchainEvent>,
    swarm: &mut Swarm<NockchainBehaviour>,
    peer_exclusions: &PeerExclusions,
    metrics: &NockchainP2PMetrics,
    next_connection_id: &mut u64,
    next_inbound_id: &mut u64,
    native_connections: &mut HashMap<NativeConnectionId, ConnectionId>,
    local_connections: &mut HashMap<ConnectionId, NativeConnectionId>,
    outbound_requests: &mut HashMap<request_response::OutboundRequestId, RequestId>,
    pending_inbound: &mut HashMap<InboundRequestId, PendingInbound>,
    native_inbound: &mut HashMap<request_response::InboundRequestId, InboundRequestId>,
    channels: &TransportActorChannels,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            if let Some(address) = multiaddr_to_peer_address(&address) {
                send_event(channels, TransportEvent::Listening { address }).await;
            }
        }
        SwarmEvent::ConnectionEstablished {
            connection_id,
            peer_id,
            endpoint,
            ..
        } => {
            let Some(address) = multiaddr_to_peer_address(endpoint.get_remote_address()) else {
                warn!(peer = %peer_id, remote = %endpoint.get_remote_address(), "Ignoring connection without a direct socket address");
                swarm.close_connection(connection_id);
                return;
            };
            let local_id = ConnectionId::new(*next_connection_id);
            *next_connection_id = next_connection_id.saturating_add(1);
            native_connections.insert(connection_id, local_id);
            local_connections.insert(local_id, connection_id);
            let direction = if endpoint.is_listener() {
                ConnectionDirection::Inbound
            } else {
                ConnectionDirection::Outbound
            };
            send_event(
                channels,
                TransportEvent::ConnectionEstablished {
                    connection: local_id,
                    peer: peer_id,
                    address,
                    direction,
                },
            )
            .await;
        }
        SwarmEvent::ConnectionClosed {
            connection_id,
            peer_id,
            cause,
            ..
        } => {
            if let Some(local_id) = native_connections.remove(&connection_id) {
                local_connections.remove(&local_id);
                send_event(
                    channels,
                    TransportEvent::ConnectionClosed {
                        connection: local_id,
                        peer: peer_id,
                        cause: cause.map(|error| normalize_close_cause(&error)),
                    },
                )
                .await;
            }
        }
        SwarmEvent::IncomingConnectionError {
            error: libp2p::swarm::ListenError::Denied { cause },
            ..
        } => {
            if cause
                .downcast::<libp2p::connection_limits::Exceeded>()
                .is_ok()
            {
                send_event(channels, TransportEvent::InboundConnectionLimitReached).await;
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            let address = dial_error_address(&error).and_then(multiaddr_to_peer_address);
            send_event(
                channels,
                TransportEvent::DialFailed {
                    peer: peer_id,
                    address,
                    failure: normalize_dial_error(&error),
                },
            )
            .await;
        }
        SwarmEvent::Behaviour(NockchainEvent::Identify(IdentifyEvent::Received {
            peer_id,
            info,
            ..
        })) => {
            swarm.add_external_address(info.observed_addr.clone());
            if let Some(ip) = info.observed_addr.ip_addr() {
                peer_exclusions.record_positive_ip(ip);
            }
            let mut addresses = Vec::new();
            for address in info.listen_addrs {
                if peer_exclusions.is_address_excluded(&address, Some(peer_id)) {
                    metrics.identify_addresses_skipped_for_exclusion.increment();
                    continue;
                }
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, address.clone());
                if let Some(address) = multiaddr_to_peer_address(&address) {
                    addresses.push(address);
                }
            }
            if !addresses.is_empty() {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::PeerUpserted {
                        peer: peer_id,
                        addresses,
                        source: DiscoverySource::Identify,
                    }),
                )
                .await;
            }
        }
        SwarmEvent::Behaviour(NockchainEvent::Kad(event)) => {
            handle_kad_event(event, swarm, channels).await;
        }
        SwarmEvent::Behaviour(NockchainEvent::RequestResponse(event)) => match event {
            request_response::Event::Message {
                connection_id,
                peer,
                message,
            } => match message {
                Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    let Some(connection) = native_connections.get(&connection_id).copied() else {
                        trace!(peer = %peer, ?connection_id, "Dropping request for untracked connection");
                        return;
                    };
                    let id = InboundRequestId::new(*next_inbound_id);
                    *next_inbound_id = next_inbound_id.saturating_add(1);
                    native_inbound.insert(request_id, id);
                    pending_inbound.insert(
                        id,
                        PendingInbound {
                            native_id: request_id,
                            channel,
                        },
                    );
                    send_event(
                        channels,
                        TransportEvent::IncomingRequest {
                            id,
                            connection,
                            peer,
                            request,
                        },
                    )
                    .await;
                }
                Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(id) = outbound_requests.remove(&request_id) {
                        send_event(channels, TransportEvent::Response { id, peer, response }).await;
                    }
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if let Some(id) = outbound_requests.remove(&request_id) {
                    send_event(
                        channels,
                        TransportEvent::RequestFailed {
                            id,
                            peer,
                            failure: normalize_request_failure(&error),
                        },
                    )
                    .await;
                }
            }
            request_response::Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if let Some(id) = native_inbound.remove(&request_id) {
                    pending_inbound.remove(&id);
                    send_event(
                        channels,
                        TransportEvent::InboundFailed {
                            id,
                            peer,
                            failure: normalize_inbound_failure(&error),
                        },
                    )
                    .await;
                }
            }
            request_response::Event::ResponseSent { .. } => {}
        },
        SwarmEvent::Behaviour(NockchainEvent::Ping(event)) => {
            if let Some(connection) = native_connections.get(&event.connection).copied() {
                let result = event.result.map_err(|error| error.to_string());
                send_event(
                    channels,
                    TransportEvent::Liveness {
                        connection,
                        peer: event.peer,
                        result,
                    },
                )
                .await;
            }
        }
        other => trace!(?other, "Unhandled libp2p transport event"),
    }
}

async fn handle_kad_event(
    event: kad::Event,
    swarm: &mut Swarm<NockchainBehaviour>,
    channels: &TransportActorChannels,
) {
    match event {
        kad::Event::RoutingUpdated {
            peer,
            addresses,
            old_peer,
            ..
        } => {
            let addresses = addresses
                .iter()
                .filter_map(multiaddr_to_peer_address)
                .collect::<Vec<_>>();
            if !addresses.is_empty() {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::PeerUpserted {
                        peer,
                        addresses,
                        source: DiscoverySource::RoutingTable,
                    }),
                )
                .await;
            }
            if let Some(peer) = old_peer {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::PeerRemoved { peer }),
                )
                .await;
            }
        }
        kad::Event::RoutablePeer { peer, address } => {
            if let Some(address) = multiaddr_to_peer_address(&address) {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::PeerUpserted {
                        peer,
                        addresses: vec![address],
                        source: DiscoverySource::RoutingTable,
                    }),
                )
                .await;
            }
        }
        _ => {}
    }

    let entries = swarm
        .behaviour_mut()
        .kad
        .kbuckets()
        .map(|bucket| bucket.num_entries())
        .sum();
    send_event(
        channels,
        TransportEvent::Discovery(DiscoveryEvent::RoutingTableStats { entries }),
    )
    .await;
}

async fn send_event(channels: &TransportActorChannels, event: TransportEvent) {
    if channels.events.send(event).await.is_err() {
        debug!("Transport event receiver closed");
    }
}

fn multiaddr_to_peer_address(address: &Multiaddr) -> Option<PeerAddress> {
    let ip = address.ip_addr()?;
    let port = address.iter().find_map(|protocol| match protocol {
        Protocol::Udp(port) | Protocol::Tcp(port) => Some(port),
        _ => None,
    })?;
    Some(PeerAddress::new((ip, port).into()))
}

fn socket_to_multiaddr(address: PeerAddress, peer: Option<PeerId>) -> Multiaddr {
    let mut multiaddr = Multiaddr::empty();
    match address.socket.ip() {
        std::net::IpAddr::V4(ip) => multiaddr.push(Protocol::Ip4(ip)),
        std::net::IpAddr::V6(ip) => multiaddr.push(Protocol::Ip6(ip)),
    }
    multiaddr.push(Protocol::Udp(address.socket.port()));
    multiaddr.push(Protocol::QuicV1);
    if let Some(peer) = peer {
        multiaddr.push(Protocol::P2p(peer));
    }
    multiaddr
}

fn peer_id_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer) => Some(peer),
        _ => None,
    })
}

fn dial_error_address(error: &libp2p::swarm::DialError) -> Option<&Multiaddr> {
    match error {
        libp2p::swarm::DialError::WrongPeerId { address, .. }
        | libp2p::swarm::DialError::LocalPeerId { address } => Some(address),
        _ => None,
    }
}

fn normalize_dial_error(error: &libp2p::swarm::DialError) -> DialFailure {
    match error {
        libp2p::swarm::DialError::NoAddresses => DialFailure::NoAddress,
        libp2p::swarm::DialError::WrongPeerId {
            obtained, address, ..
        } => DialFailure::WrongNodeId {
            expected: peer_id_from_multiaddr(address).unwrap_or(*obtained),
            obtained: *obtained,
        },
        error if error.to_string().contains("Permission denied") => DialFailure::PermissionDenied,
        error => DialFailure::Transport(error.to_string()),
    }
}

fn normalize_request_failure(error: &request_response::OutboundFailure) -> RequestFailure {
    match error {
        request_response::OutboundFailure::Timeout => RequestFailure::Timeout,
        request_response::OutboundFailure::ConnectionClosed => RequestFailure::ConnectionClosed,
        request_response::OutboundFailure::UnsupportedProtocols => {
            RequestFailure::UnsupportedProtocol
        }
        request_response::OutboundFailure::DialFailure => RequestFailure::NotConnected,
        request_response::OutboundFailure::Io(error) => RequestFailure::Io(error.to_string()),
    }
}

fn normalize_inbound_failure(error: &request_response::InboundFailure) -> InboundFailure {
    match error {
        request_response::InboundFailure::Timeout => InboundFailure::Timeout,
        request_response::InboundFailure::ConnectionClosed => InboundFailure::ConnectionClosed,
        request_response::InboundFailure::UnsupportedProtocols => {
            InboundFailure::UnsupportedProtocol
        }
        request_response::InboundFailure::ResponseOmission => InboundFailure::ResponseOmission,
        request_response::InboundFailure::Io(error) => InboundFailure::Io(error.to_string()),
    }
}

fn normalize_close_cause(error: &libp2p::swarm::ConnectionError) -> ConnectionCloseCause {
    if error.to_string().contains("Permission denied") {
        ConnectionCloseCause::PermissionDenied
    } else if error.to_string().contains("idle") {
        ConnectionCloseCause::Idle
    } else {
        ConnectionCloseCause::Transport(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_quic_multiaddr_round_trips_through_neutral_address() {
        let peer = PeerId::random();
        let address = PeerAddress::new("127.0.0.1:34000".parse().expect("valid socket"));
        let multiaddr = socket_to_multiaddr(address, Some(peer));

        assert_eq!(multiaddr_to_peer_address(&multiaddr), Some(address));
        assert_eq!(peer_id_from_multiaddr(&multiaddr), Some(peer));
    }

    #[test]
    fn request_failure_normalization_preserves_retry_semantics() {
        assert!(
            normalize_request_failure(&request_response::OutboundFailure::Timeout).is_transient()
        );
        assert!(
            normalize_request_failure(&request_response::OutboundFailure::ConnectionClosed)
                .is_transient()
        );
        assert!(!normalize_request_failure(
            &request_response::OutboundFailure::UnsupportedProtocols
        )
        .is_transient());
        assert!(
            !normalize_request_failure(&request_response::OutboundFailure::DialFailure)
                .is_transient()
        );
    }
}
