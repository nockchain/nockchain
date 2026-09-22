use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use iroh::endpoint::{Connection, Incoming, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey};
use libp2p_identity::Keypair;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::messages::{NockchainRequest, NockchainResponse};
use crate::transport::{
    transport_channels, ConnectionCloseCause, DiscoveryEvent, DiscoveryFailure, DiscoverySource,
    TransportActorChannels, TransportCommand, TransportEvent, TransportHandle,
};
use crate::types::{
    ConnectionDirection, ConnectionId, DialFailure, InboundFailure, InboundRequestId, NodeId,
    PeerAddress, RequestFailure, RequestId,
};

pub const REQUEST_RESPONSE_ALPN: &[u8] = crate::transport::REQUEST_RESPONSE_PROTOCOL.as_bytes();
pub const DISCOVERY_ALPN: &[u8] = b"/nockchain/discovery/1";

const DEFAULT_COMMAND_CAPACITY: usize = 256;
const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
const DEFAULT_DISCOVERY_K: usize = 20;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ROUTING_ENTRIES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrohPeer {
    pub node_id: NodeId,
    pub addresses: Vec<PeerAddress>,
}

pub struct IrohTransportConfig {
    pub keypair: Keypair,
    pub bind: SocketAddr,
    pub advertised_addresses: Vec<PeerAddress>,
    pub bootstrap_peers: Vec<IrohPeer>,
    pub command_capacity: usize,
    pub max_frame_size: usize,
    pub discovery_k: usize,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
}

impl IrohTransportConfig {
    pub fn new(keypair: Keypair, bind: SocketAddr) -> Self {
        Self {
            keypair,
            bind,
            advertised_addresses: Vec::new(),
            bootstrap_peers: Vec::new(),
            command_capacity: DEFAULT_COMMAND_CAPACITY,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            discovery_k: DEFAULT_DISCOVERY_K,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PeerRecord {
    #[serde(with = "serde_bytes")]
    node_id: Vec<u8>,
    addresses: Vec<SocketAddr>,
}

impl PeerRecord {
    fn new(node_id: NodeId, addresses: impl IntoIterator<Item = PeerAddress>) -> Self {
        Self {
            node_id: node_id.to_bytes(),
            addresses: addresses
                .into_iter()
                .map(|address| address.socket)
                .collect(),
        }
    }

    fn validate(self) -> Result<IrohPeer, String> {
        let node_id = NodeId::from_bytes(&self.node_id)
            .map_err(|error| format!("invalid canonical node id: {error}"))?;
        let expected_key = iroh_public_key(&node_id)?;
        let actual_key = peer_id_public_key(&node_id)?;
        if expected_key.as_bytes() != &actual_key {
            return Err(String::from(
                "canonical node id does not embed its Ed25519 key",
            ));
        }
        let addresses = self
            .addresses
            .into_iter()
            .map(PeerAddress::new)
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(String::from("peer record contains no direct address"));
        }
        Ok(IrohPeer { node_id, addresses })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RequestEnvelope {
    sender: PeerRecord,
    request: NockchainRequest,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryRequest {
    sender: PeerRecord,
    #[serde(with = "serde_bytes")]
    target: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiscoveryResponse {
    peers: Vec<PeerRecord>,
}

struct ConnectionEntry {
    peer: NodeId,
    connection: Connection,
    address: Option<PeerAddress>,
    direction: ConnectionDirection,
    announced: bool,
}

struct PendingInbound {
    peer: NodeId,
    response: SendStream,
}

enum InternalEvent {
    AcceptedRequestConnection {
        peer: NodeId,
        connection: Connection,
    },
    IncomingRequest {
        connection: ConnectionId,
        authenticated_peer: NodeId,
        envelope: RequestEnvelope,
        response: SendStream,
    },
    ConnectionClosed {
        connection: ConnectionId,
        reason: String,
    },
    DialFinished {
        peer: NodeId,
        address: PeerAddress,
        result: Result<Connection, DialFailure>,
    },
    OutboundFinished {
        id: RequestId,
        peer: NodeId,
        result: Result<NockchainResponse, RequestFailure>,
    },
    DiscoveryQuery {
        authenticated_peer: NodeId,
        request: DiscoveryRequest,
        response: SendStream,
        connection: Connection,
    },
    DiscoveryFinished {
        source: DiscoverySource,
        result: Result<Vec<IrohPeer>, DiscoveryFailure>,
    },
    AcceptFailed(String),
}

pub async fn spawn_iroh_transport(
    config: IrohTransportConfig,
) -> Result<TransportHandle, Box<dyn std::error::Error + Send + Sync>> {
    let secret_key = iroh_secret_key(config.keypair)?;
    let local_node_id = node_id_from_iroh(secret_key.public())?;
    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
        .net_report_config(iroh::NetReportConfig::minimal())
        .secret_key(secret_key)
        .alpns(vec![
            REQUEST_RESPONSE_ALPN.to_vec(),
            DISCOVERY_ALPN.to_vec(),
        ])
        .bind_addr(config.bind)?
        .bind()
        .await?;

    let advertised_addresses = if config.advertised_addresses.is_empty() {
        endpoint
            .bound_sockets()
            .into_iter()
            .map(|socket| {
                let socket = if config.bind.ip().is_unspecified() {
                    socket
                } else {
                    SocketAddr::new(config.bind.ip(), socket.port())
                };
                PeerAddress::new(socket)
            })
            .collect()
    } else {
        config.advertised_addresses
    };
    let local_record = PeerRecord::new(local_node_id, advertised_addresses.clone());
    let (handle, channels) = transport_channels(local_node_id, config.command_capacity.max(1));

    tokio::spawn(run_iroh_transport(
        endpoint,
        local_record,
        advertised_addresses,
        config.bootstrap_peers,
        config.max_frame_size.max(1),
        config.discovery_k.max(1),
        config.connect_timeout,
        config.request_timeout,
        channels,
    ));
    Ok(handle)
}

#[allow(clippy::too_many_arguments)]
async fn run_iroh_transport(
    endpoint: Endpoint,
    local_record: PeerRecord,
    advertised_addresses: Vec<PeerAddress>,
    bootstrap_peers: Vec<IrohPeer>,
    max_frame_size: usize,
    discovery_k: usize,
    connect_timeout: Duration,
    request_timeout: Duration,
    mut channels: TransportActorChannels,
) {
    let (internal_tx, mut internal_rx) = mpsc::channel(256);
    let mut next_connection_id = 1u64;
    let mut next_inbound_id = 1u64;
    let mut connections = HashMap::<ConnectionId, ConnectionEntry>::new();
    let mut peer_connections = HashMap::<NodeId, BTreeSet<ConnectionId>>::new();
    let mut pending_inbound = HashMap::<InboundRequestId, PendingInbound>::new();
    let mut routing = BTreeMap::<NodeId, BTreeSet<PeerAddress>>::new();
    let mut dialing = HashSet::<NodeId>::new();

    for peer in &bootstrap_peers {
        upsert_routing(
            &mut routing,
            peer.clone(),
            local_record_node_id(&local_record),
        );
    }
    for address in advertised_addresses {
        send_event(&channels, TransportEvent::Listening { address }).await;
    }

    loop {
        tokio::select! {
            command = channels.commands.recv() => {
                let Some(command) = command else { break; };
                if process_command(
                    command,
                    &endpoint,
                    &local_record,
                    &bootstrap_peers,
                    &mut routing,
                    &mut dialing,
                    &mut connections,
                    &mut peer_connections,
                    &mut pending_inbound,
                    max_frame_size,
                    discovery_k,
                    connect_timeout,
                    request_timeout,
                    &internal_tx,
                    &channels,
                ).await {
                    break;
                }
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    send_event(&channels, TransportEvent::Fatal {
                        error: String::from("Iroh endpoint stopped accepting connections"),
                    }).await;
                    break;
                };
                spawn_incoming_handshake(incoming, internal_tx.clone());
            }
            event = internal_rx.recv() => {
                let Some(event) = event else { break; };
                process_internal_event(
                    event,
                    &endpoint,
                    &local_record,
                    &mut next_connection_id,
                    &mut next_inbound_id,
                    &mut connections,
                    &mut peer_connections,
                    &mut pending_inbound,
                    &mut routing,
                    &mut dialing,
                    max_frame_size,
                    discovery_k,
                    &internal_tx,
                    &channels,
                ).await;
            }
        }
    }

    for entry in connections.into_values() {
        entry.connection.close(0u8.into(), b"network shutdown");
    }
    for (id, pending) in pending_inbound {
        send_event(
            &channels,
            TransportEvent::InboundFailed {
                id,
                peer: pending.peer,
                failure: InboundFailure::ConnectionClosed,
            },
        )
        .await;
    }
    endpoint.close().await;
}

#[allow(clippy::too_many_arguments)]
async fn process_command(
    command: TransportCommand,
    endpoint: &Endpoint,
    local_record: &PeerRecord,
    bootstrap_peers: &[IrohPeer],
    routing: &mut BTreeMap<NodeId, BTreeSet<PeerAddress>>,
    dialing: &mut HashSet<NodeId>,
    connections: &mut HashMap<ConnectionId, ConnectionEntry>,
    peer_connections: &mut HashMap<NodeId, BTreeSet<ConnectionId>>,
    pending_inbound: &mut HashMap<InboundRequestId, PendingInbound>,
    max_frame_size: usize,
    discovery_k: usize,
    connect_timeout: Duration,
    request_timeout: Duration,
    internal_tx: &mpsc::Sender<InternalEvent>,
    channels: &TransportActorChannels,
) -> bool {
    match command {
        TransportCommand::Dial { peer, addresses } => {
            begin_dial(
                endpoint, peer, addresses, dialing, peer_connections, connect_timeout, internal_tx,
            );
        }
        TransportCommand::DialBootstrap => {
            for peer in bootstrap_peers {
                begin_dial(
                    endpoint,
                    peer.node_id,
                    peer.addresses.clone(),
                    dialing,
                    peer_connections,
                    connect_timeout,
                    internal_tx,
                );
            }
        }
        TransportCommand::DialKnown { peers } => {
            for peer in peers {
                let addresses = routing
                    .get(&peer)
                    .map(|addresses| addresses.iter().copied().collect())
                    .unwrap_or_default();
                begin_dial(
                    endpoint, peer, addresses, dialing, peer_connections, connect_timeout,
                    internal_tx,
                );
            }
        }
        TransportCommand::SendRequest { id, peer, request } => {
            let connection = peer_connections
                .get(&peer)
                .and_then(|ids| ids.iter().find_map(|id| connections.get(id)))
                .map(|entry| entry.connection.clone());
            let Some(connection) = connection else {
                send_event(
                    channels,
                    TransportEvent::RequestFailed {
                        id,
                        peer,
                        failure: RequestFailure::NotConnected,
                    },
                )
                .await;
                return false;
            };
            let envelope = RequestEnvelope {
                sender: local_record.clone(),
                request,
            };
            spawn_outbound_request(
                connection,
                id,
                peer,
                envelope,
                max_frame_size,
                request_timeout,
                internal_tx.clone(),
            );
        }
        TransportCommand::CompleteInbound { id, response } => {
            let Some(mut pending) = pending_inbound.remove(&id) else {
                return false;
            };
            match response {
                Some(response) => {
                    if let Err(error) = write_message(&mut pending.response, &response).await {
                        send_event(
                            channels,
                            TransportEvent::InboundFailed {
                                id,
                                peer: pending.peer,
                                failure: InboundFailure::Io(error),
                            },
                        )
                        .await;
                    }
                }
                None => {
                    let _ = pending.response.finish();
                    send_event(
                        channels,
                        TransportEvent::InboundFailed {
                            id,
                            peer: pending.peer,
                            failure: InboundFailure::ResponseOmission,
                        },
                    )
                    .await;
                }
            }
        }
        TransportCommand::DisconnectPeer { peer } => {
            let ids = peer_connections.remove(&peer).unwrap_or_default();
            for id in ids {
                if let Some(entry) = connections.remove(&id) {
                    entry.connection.close(0u8.into(), b"disconnect peer");
                    send_event(
                        channels,
                        TransportEvent::ConnectionClosed {
                            connection: id,
                            peer,
                            cause: Some(ConnectionCloseCause::Local),
                        },
                    )
                    .await;
                }
            }
        }
        TransportCommand::CloseConnection { connection } => {
            if let Some(entry) = connections.remove(&connection) {
                if let Some(ids) = peer_connections.get_mut(&entry.peer) {
                    ids.remove(&connection);
                    if ids.is_empty() {
                        peer_connections.remove(&entry.peer);
                    }
                }
                entry.connection.close(0u8.into(), b"close connection");
                send_event(
                    channels,
                    TransportEvent::ConnectionClosed {
                        connection,
                        peer: entry.peer,
                        cause: Some(ConnectionCloseCause::Local),
                    },
                )
                .await;
            }
        }
        TransportCommand::BootstrapDiscovery => {
            if bootstrap_peers.is_empty() {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::BootstrapFailed {
                        failure: DiscoveryFailure::NoKnownPeers,
                    }),
                )
                .await;
            } else {
                for peer in bootstrap_peers {
                    spawn_discovery_request(
                        endpoint.clone(),
                        peer.clone(),
                        local_record.clone(),
                        local_record_node_id(local_record),
                        max_frame_size,
                        connect_timeout,
                        DiscoverySource::Bootstrap,
                        internal_tx.clone(),
                    );
                }
            }
        }
        TransportCommand::RefreshDiscovery => {
            let target = local_record_node_id(local_record);
            let peers = closest_peers(routing, target, discovery_k);
            if peers.is_empty() {
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::BootstrapFailed {
                        failure: DiscoveryFailure::NoKnownPeers,
                    }),
                )
                .await;
            }
            for peer in peers {
                spawn_discovery_request(
                    endpoint.clone(),
                    peer,
                    local_record.clone(),
                    target,
                    max_frame_size,
                    connect_timeout,
                    DiscoverySource::RoutingTable,
                    internal_tx.clone(),
                );
            }
        }
        TransportCommand::RemoveDiscoveredAddress { peer, address } => {
            if let Some(peer) = peer {
                if let Some(addresses) = routing.get_mut(&peer) {
                    addresses.remove(&address);
                    if addresses.is_empty() {
                        routing.remove(&peer);
                        send_event(
                            channels,
                            TransportEvent::Discovery(DiscoveryEvent::PeerRemoved { peer }),
                        )
                        .await;
                    }
                }
            } else {
                let empty = routing
                    .iter_mut()
                    .filter_map(|(peer, addresses)| {
                        addresses.remove(&address);
                        addresses.is_empty().then_some(*peer)
                    })
                    .collect::<Vec<_>>();
                for peer in empty {
                    routing.remove(&peer);
                    send_event(
                        channels,
                        TransportEvent::Discovery(DiscoveryEvent::PeerRemoved { peer }),
                    )
                    .await;
                }
            }
        }
        TransportCommand::Shutdown => return true,
    }
    false
}

#[allow(clippy::too_many_arguments)]
async fn process_internal_event(
    event: InternalEvent,
    endpoint: &Endpoint,
    local_record: &PeerRecord,
    next_connection_id: &mut u64,
    next_inbound_id: &mut u64,
    connections: &mut HashMap<ConnectionId, ConnectionEntry>,
    peer_connections: &mut HashMap<NodeId, BTreeSet<ConnectionId>>,
    pending_inbound: &mut HashMap<InboundRequestId, PendingInbound>,
    routing: &mut BTreeMap<NodeId, BTreeSet<PeerAddress>>,
    dialing: &mut HashSet<NodeId>,
    max_frame_size: usize,
    discovery_k: usize,
    internal_tx: &mpsc::Sender<InternalEvent>,
    channels: &TransportActorChannels,
) {
    match event {
        InternalEvent::AcceptedRequestConnection { peer, connection } => {
            let id = ConnectionId::new(*next_connection_id);
            *next_connection_id = next_connection_id.saturating_add(1);
            connections.insert(
                id,
                ConnectionEntry {
                    peer,
                    connection: connection.clone(),
                    address: None,
                    direction: ConnectionDirection::Inbound,
                    announced: false,
                },
            );
            peer_connections.entry(peer).or_default().insert(id);
            spawn_request_accept_loop(connection, id, peer, max_frame_size, internal_tx.clone());
        }
        InternalEvent::IncomingRequest {
            connection,
            authenticated_peer,
            envelope,
            response,
        } => {
            let Ok(sender) = envelope.sender.validate() else {
                return;
            };
            if sender.node_id != authenticated_peer {
                return;
            }
            let Some(address) = sender.addresses.first().copied() else {
                return;
            };
            let source = if connections
                .get(&connection)
                .is_some_and(|entry| entry.direction == ConnectionDirection::Inbound)
            {
                DiscoverySource::InboundConnection
            } else {
                DiscoverySource::RoutingTable
            };
            let changed =
                upsert_routing(routing, sender.clone(), local_record_node_id(local_record));
            if changed {
                emit_peer_upsert(channels, &sender, source).await;
            }
            if let Some(entry) = connections.get_mut(&connection) {
                entry.address = Some(address);
                if !entry.announced {
                    entry.announced = true;
                    send_event(
                        channels,
                        TransportEvent::ConnectionEstablished {
                            connection,
                            peer: authenticated_peer,
                            address,
                            direction: entry.direction,
                        },
                    )
                    .await;
                }
            }
            let id = InboundRequestId::new(*next_inbound_id);
            *next_inbound_id = next_inbound_id.saturating_add(1);
            pending_inbound.insert(
                id,
                PendingInbound {
                    peer: authenticated_peer,
                    response,
                },
            );
            send_event(
                channels,
                TransportEvent::IncomingRequest {
                    id,
                    connection,
                    peer: authenticated_peer,
                    request: envelope.request,
                },
            )
            .await;
        }
        InternalEvent::ConnectionClosed { connection, reason } => {
            if let Some(entry) = connections.remove(&connection) {
                if let Some(ids) = peer_connections.get_mut(&entry.peer) {
                    ids.remove(&connection);
                    if ids.is_empty() {
                        peer_connections.remove(&entry.peer);
                    }
                }
                if entry.announced {
                    send_event(
                        channels,
                        TransportEvent::ConnectionClosed {
                            connection,
                            peer: entry.peer,
                            cause: Some(ConnectionCloseCause::Transport(reason)),
                        },
                    )
                    .await;
                }
            }
        }
        InternalEvent::DialFinished {
            peer,
            address,
            result,
        } => {
            dialing.remove(&peer);
            match result {
                Ok(connection) => {
                    let id = ConnectionId::new(*next_connection_id);
                    *next_connection_id = next_connection_id.saturating_add(1);
                    connections.insert(
                        id,
                        ConnectionEntry {
                            peer,
                            connection: connection.clone(),
                            address: Some(address),
                            direction: ConnectionDirection::Outbound,
                            announced: true,
                        },
                    );
                    peer_connections.entry(peer).or_default().insert(id);
                    let discovered = IrohPeer {
                        node_id: peer,
                        addresses: vec![address],
                    };
                    if upsert_routing(
                        routing,
                        discovered.clone(),
                        local_record_node_id(local_record),
                    ) {
                        emit_peer_upsert(channels, &discovered, DiscoverySource::RoutingTable)
                            .await;
                    }
                    send_event(
                        channels,
                        TransportEvent::ConnectionEstablished {
                            connection: id,
                            peer,
                            address,
                            direction: ConnectionDirection::Outbound,
                        },
                    )
                    .await;
                    spawn_request_accept_loop(
                        connection,
                        id,
                        peer,
                        max_frame_size,
                        internal_tx.clone(),
                    );
                }
                Err(failure) => {
                    send_event(
                        channels,
                        TransportEvent::DialFailed {
                            peer: Some(peer),
                            address: Some(address),
                            failure,
                        },
                    )
                    .await;
                }
            }
        }
        InternalEvent::OutboundFinished { id, peer, result } => match result {
            Ok(response) => {
                send_event(channels, TransportEvent::Response { id, peer, response }).await;
            }
            Err(failure) => {
                send_event(
                    channels,
                    TransportEvent::RequestFailed { id, peer, failure },
                )
                .await;
            }
        },
        InternalEvent::DiscoveryQuery {
            authenticated_peer,
            request,
            mut response,
            connection: _connection,
        } => {
            let Ok(sender) = request.sender.validate() else {
                return;
            };
            if sender.node_id != authenticated_peer {
                return;
            }
            if upsert_routing(routing, sender.clone(), local_record_node_id(local_record)) {
                emit_peer_upsert(channels, &sender, DiscoverySource::InboundConnection).await;
            }
            let target = NodeId::from_bytes(&request.target)
                .unwrap_or_else(|_| local_record_node_id(local_record));
            let mut peers = closest_peers(routing, target, discovery_k)
                .into_iter()
                .map(|peer| PeerRecord::new(peer.node_id, peer.addresses))
                .collect::<Vec<_>>();
            peers.insert(0, local_record.clone());
            if write_message(&mut response, &DiscoveryResponse { peers })
                .await
                .is_ok()
            {
                let _ = timeout(Duration::from_secs(5), response.stopped()).await;
            }
        }
        InternalEvent::DiscoveryFinished { source, result } => match result {
            Ok(peers) => {
                let mut discovered = 0usize;
                for peer in peers {
                    if peer.node_id == local_record_node_id(local_record) {
                        continue;
                    }
                    if upsert_routing(routing, peer.clone(), local_record_node_id(local_record)) {
                        discovered = discovered.saturating_add(1);
                        emit_peer_upsert(channels, &peer, source).await;
                    }
                }
                send_event(
                    channels,
                    TransportEvent::Discovery(DiscoveryEvent::RoutingTableStats {
                        entries: routing.len(),
                    }),
                )
                .await;
                if source == DiscoverySource::Bootstrap {
                    send_event(
                        channels,
                        TransportEvent::Discovery(DiscoveryEvent::BootstrapFinished { discovered }),
                    )
                    .await;
                }
            }
            Err(failure) => {
                if source == DiscoverySource::Bootstrap {
                    send_event(
                        channels,
                        TransportEvent::Discovery(DiscoveryEvent::BootstrapFailed { failure }),
                    )
                    .await;
                }
            }
        },
        InternalEvent::AcceptFailed(error) => {
            tracing::debug!(%error, "Ignoring rejected Iroh connection");
        }
    }

    let _ = endpoint;
}

fn begin_dial(
    endpoint: &Endpoint,
    peer: NodeId,
    addresses: Vec<PeerAddress>,
    dialing: &mut HashSet<NodeId>,
    peer_connections: &HashMap<NodeId, BTreeSet<ConnectionId>>,
    connect_timeout: Duration,
    internal_tx: &mpsc::Sender<InternalEvent>,
) {
    if peer_connections
        .get(&peer)
        .is_some_and(|connections| !connections.is_empty())
        || !dialing.insert(peer)
    {
        return;
    }
    let endpoint = endpoint.clone();
    let internal_tx = internal_tx.clone();
    tokio::spawn(async move {
        let result = dial_request_connection(&endpoint, peer, &addresses, connect_timeout).await;
        let address = addresses
            .first()
            .copied()
            .unwrap_or_else(|| PeerAddress::new(SocketAddr::from(([0, 0, 0, 0], 0))));
        let _ = internal_tx
            .send(InternalEvent::DialFinished {
                peer,
                address,
                result,
            })
            .await;
    });
}

async fn dial_request_connection(
    endpoint: &Endpoint,
    peer: NodeId,
    addresses: &[PeerAddress],
    connect_timeout: Duration,
) -> Result<Connection, DialFailure> {
    if addresses.is_empty() {
        return Err(DialFailure::NoAddress);
    }
    let public_key = iroh_public_key(&peer).map_err(DialFailure::Transport)?;
    let endpoint_addr = addresses
        .iter()
        .fold(EndpointAddr::new(public_key), |addr, address| {
            addr.with_ip_addr(address.socket)
        });
    match timeout(
        connect_timeout,
        endpoint.connect(endpoint_addr, REQUEST_RESPONSE_ALPN),
    )
    .await
    {
        Ok(Ok(connection)) => {
            let obtained =
                node_id_from_iroh(connection.remote_id()).map_err(DialFailure::Transport)?;
            if obtained != peer {
                return Err(DialFailure::WrongNodeId {
                    expected: peer,
                    obtained,
                });
            }
            Ok(connection)
        }
        Ok(Err(error)) => Err(DialFailure::Transport(error.to_string())),
        Err(_) => Err(DialFailure::Timeout),
    }
}

fn spawn_incoming_handshake(incoming: Incoming, internal_tx: mpsc::Sender<InternalEvent>) {
    tokio::spawn(async move {
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(error) => {
                let _ = internal_tx
                    .send(InternalEvent::AcceptFailed(format!(
                        "Iroh incoming handshake failed: {error}"
                    )))
                    .await;
                return;
            }
        };
        let peer = match node_id_from_iroh(connection.remote_id()) {
            Ok(peer) => peer,
            Err(error) => {
                connection.close(1u8.into(), b"unsupported identity");
                let _ = internal_tx.send(InternalEvent::AcceptFailed(error)).await;
                return;
            }
        };
        match connection.alpn() {
            REQUEST_RESPONSE_ALPN => {
                let _ = internal_tx
                    .send(InternalEvent::AcceptedRequestConnection { peer, connection })
                    .await;
            }
            DISCOVERY_ALPN => {
                spawn_discovery_accept(connection, peer, internal_tx);
            }
            _ => connection.close(1u8.into(), b"unsupported ALPN"),
        }
    });
}

fn spawn_request_accept_loop(
    connection: Connection,
    connection_id: ConnectionId,
    peer: NodeId,
    max_frame_size: usize,
    internal_tx: mpsc::Sender<InternalEvent>,
) {
    tokio::spawn(async move {
        loop {
            match connection.accept_bi().await {
                Ok((response, request)) => {
                    let internal_tx = internal_tx.clone();
                    tokio::spawn(async move {
                        match read_message::<RequestEnvelope>(request, max_frame_size).await {
                            Ok(envelope) => {
                                let _ = internal_tx
                                    .send(InternalEvent::IncomingRequest {
                                        connection: connection_id,
                                        authenticated_peer: peer,
                                        envelope,
                                        response,
                                    })
                                    .await;
                            }
                            Err(_) => {
                                let mut response = response;
                                let _ = response.finish();
                            }
                        }
                    });
                }
                Err(error) => {
                    let _ = internal_tx
                        .send(InternalEvent::ConnectionClosed {
                            connection: connection_id,
                            reason: error.to_string(),
                        })
                        .await;
                    break;
                }
            }
        }
    });
}

fn spawn_outbound_request(
    connection: Connection,
    id: RequestId,
    peer: NodeId,
    envelope: RequestEnvelope,
    max_frame_size: usize,
    request_timeout: Duration,
    internal_tx: mpsc::Sender<InternalEvent>,
) {
    tokio::spawn(async move {
        let result = timeout(request_timeout, async {
            let (mut request, response) = connection
                .open_bi()
                .await
                .map_err(|error| RequestFailure::Io(error.to_string()))?;
            write_message(&mut request, &envelope)
                .await
                .map_err(RequestFailure::Io)?;
            read_message::<NockchainResponse>(response, max_frame_size)
                .await
                .map_err(RequestFailure::Codec)
        })
        .await
        .unwrap_or(Err(RequestFailure::Timeout));
        let _ = internal_tx
            .send(InternalEvent::OutboundFinished { id, peer, result })
            .await;
    });
}

fn spawn_discovery_accept(
    connection: Connection,
    authenticated_peer: NodeId,
    internal_tx: mpsc::Sender<InternalEvent>,
) {
    tokio::spawn(async move {
        let Ok((response, request)) = connection.accept_bi().await else {
            return;
        };
        let Ok(request) = read_message::<DiscoveryRequest>(request, 1024 * 1024).await else {
            return;
        };
        let _ = internal_tx
            .send(InternalEvent::DiscoveryQuery {
                authenticated_peer,
                request,
                response,
                connection,
            })
            .await;
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_discovery_request(
    endpoint: Endpoint,
    peer: IrohPeer,
    sender: PeerRecord,
    target: NodeId,
    max_frame_size: usize,
    connect_timeout: Duration,
    source: DiscoverySource,
    internal_tx: mpsc::Sender<InternalEvent>,
) {
    tokio::spawn(async move {
        let result = async {
            let public_key = iroh_public_key(&peer.node_id).map_err(DiscoveryFailure::Transport)?;
            let endpoint_addr = peer
                .addresses
                .iter()
                .fold(EndpointAddr::new(public_key), |addr, address| {
                    addr.with_ip_addr(address.socket)
                });
            let connection = timeout(
                connect_timeout,
                endpoint.connect(endpoint_addr, DISCOVERY_ALPN),
            )
            .await
            .map_err(|_| DiscoveryFailure::Timeout)?
            .map_err(|error| DiscoveryFailure::Transport(error.to_string()))?;
            let authenticated =
                node_id_from_iroh(connection.remote_id()).map_err(DiscoveryFailure::Transport)?;
            if authenticated != peer.node_id {
                return Err(DiscoveryFailure::Transport(format!(
                    "discovery identity mismatch: expected {}, obtained {}",
                    peer.node_id, authenticated
                )));
            }
            let (mut request, response) = connection
                .open_bi()
                .await
                .map_err(|error| DiscoveryFailure::Transport(error.to_string()))?;
            write_message(
                &mut request,
                &DiscoveryRequest {
                    sender,
                    target: target.to_bytes(),
                },
            )
            .await
            .map_err(DiscoveryFailure::Transport)?;
            let response = read_message::<DiscoveryResponse>(response, max_frame_size)
                .await
                .map_err(DiscoveryFailure::Transport)?;
            response
                .peers
                .into_iter()
                .map(PeerRecord::validate)
                .collect::<Result<Vec<_>, _>>()
                .map_err(DiscoveryFailure::Transport)
        }
        .await;
        let _ = internal_tx
            .send(InternalEvent::DiscoveryFinished { source, result })
            .await;
    });
}

async fn write_message<T: Serialize>(stream: &mut SendStream, value: &T) -> Result<(), String> {
    let bytes = cbor4ii::serde::to_vec(Vec::new(), value)
        .map_err(|error| format!("CBOR encode failed: {error}"))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| format!("stream write failed: {error}"))?;
    stream
        .finish()
        .map_err(|error| format!("stream finish failed: {error}"))
}

async fn read_message<T: DeserializeOwned>(
    mut stream: RecvStream,
    max_frame_size: usize,
) -> Result<T, String> {
    let bytes = stream
        .read_to_end(max_frame_size)
        .await
        .map_err(|error| format!("stream read failed: {error}"))?;
    cbor4ii::serde::from_slice(&bytes).map_err(|error| format!("CBOR decode failed: {error}"))
}

fn upsert_routing(
    routing: &mut BTreeMap<NodeId, BTreeSet<PeerAddress>>,
    peer: IrohPeer,
    local_node_id: NodeId,
) -> bool {
    if peer.node_id == local_node_id || peer.addresses.is_empty() {
        return false;
    }
    let addresses = routing.entry(peer.node_id).or_default();
    let old_len = addresses.len();
    addresses.extend(peer.addresses);
    let changed = addresses.len() != old_len;
    if routing.len() > MAX_ROUTING_ENTRIES {
        let farthest = routing
            .keys()
            .copied()
            .max_by_key(|candidate| xor_distance(*candidate, local_node_id));
        if let Some(farthest) = farthest {
            routing.remove(&farthest);
        }
    }
    changed
}

fn closest_peers(
    routing: &BTreeMap<NodeId, BTreeSet<PeerAddress>>,
    target: NodeId,
    count: usize,
) -> Vec<IrohPeer> {
    let mut peers = routing
        .iter()
        .map(|(node_id, addresses)| IrohPeer {
            node_id: *node_id,
            addresses: addresses.iter().copied().collect(),
        })
        .collect::<Vec<_>>();
    peers.sort_by_key(|peer| xor_distance(peer.node_id, target));
    peers.truncate(count);
    peers
}

fn xor_distance(left: NodeId, right: NodeId) -> [u8; 32] {
    let left = blake3::hash(&left.to_bytes());
    let right = blake3::hash(&right.to_bytes());
    let mut distance = [0u8; 32];
    for (index, byte) in distance.iter_mut().enumerate() {
        *byte = left.as_bytes()[index] ^ right.as_bytes()[index];
    }
    distance
}

async fn emit_peer_upsert(
    channels: &TransportActorChannels,
    peer: &IrohPeer,
    source: DiscoverySource,
) {
    send_event(
        channels,
        TransportEvent::Discovery(DiscoveryEvent::PeerUpserted {
            peer: peer.node_id,
            addresses: peer.addresses.clone(),
            source,
        }),
    )
    .await;
}

async fn send_event(channels: &TransportActorChannels, event: TransportEvent) {
    let _ = channels.events.send(event).await;
}

fn local_record_node_id(record: &PeerRecord) -> NodeId {
    NodeId::from_bytes(&record.node_id)
        .expect("local peer record is constructed from a valid node id")
}

fn iroh_secret_key(keypair: Keypair) -> Result<SecretKey, String> {
    let ed25519 = keypair
        .try_into_ed25519()
        .map_err(|_| String::from("Iroh backend requires an Ed25519 node identity"))?;
    let bytes: [u8; 32] = ed25519
        .secret()
        .as_ref()
        .try_into()
        .map_err(|_| String::from("invalid Ed25519 secret key length"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn node_id_from_iroh(public_key: PublicKey) -> Result<NodeId, String> {
    let public_key = libp2p_identity::ed25519::PublicKey::try_from_bytes(public_key.as_bytes())
        .map_err(|error| format!("invalid Iroh Ed25519 public key: {error}"))?;
    Ok(NodeId::from_public_key(&libp2p_identity::PublicKey::from(
        public_key,
    )))
}

fn peer_id_public_key(node_id: &NodeId) -> Result<[u8; 32], String> {
    let multihash = node_id.as_ref();
    if multihash.code() != 0 {
        return Err(String::from(
            "Iroh backend requires an inline Ed25519 canonical node id",
        ));
    }
    let public_key = libp2p_identity::PublicKey::try_decode_protobuf(multihash.digest())
        .map_err(|error| format!("invalid embedded node public key: {error}"))?
        .try_into_ed25519()
        .map_err(|_| String::from("canonical node id does not contain an Ed25519 public key"))?;
    Ok(public_key.to_bytes())
}

fn iroh_public_key(node_id: &NodeId) -> Result<PublicKey, String> {
    PublicKey::from_bytes(&peer_id_public_key(node_id)?)
        .map_err(|error| format!("invalid Iroh public key: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::contract::{assert_request_response_contract, next_matching};

    async fn spawn_test_node(bootstrap_peers: Vec<IrohPeer>) -> (TransportHandle, IrohPeer) {
        let mut config = IrohTransportConfig::new(
            Keypair::generate_ed25519(),
            SocketAddr::from(([127, 0, 0, 1], 0)),
        );
        config.bootstrap_peers = bootstrap_peers;
        let mut handle = spawn_iroh_transport(config).await.expect("bind test node");
        let local_node_id = handle.local_node_id;
        let address = match timeout(Duration::from_secs(10), handle.next_event())
            .await
            .expect("listener event timeout")
            .expect("listener event")
        {
            TransportEvent::Listening { address } => address,
            event => panic!("expected listener event, got {event:?}"),
        };
        (
            handle,
            IrohPeer {
                node_id: local_node_id,
                addresses: vec![address],
            },
        )
    }

    #[test]
    fn identity_mapping_preserves_canonical_peer_id() {
        let keypair = Keypair::generate_ed25519();
        let expected = keypair.public().to_peer_id();
        let secret = iroh_secret_key(keypair).expect("map secret key");
        let actual = node_id_from_iroh(secret.public()).expect("map public key");
        assert_eq!(actual, expected);
        assert_eq!(iroh_public_key(&actual).unwrap(), secret.public());
    }

    #[test]
    fn closest_peers_use_canonical_xor_distance() {
        let local = Keypair::generate_ed25519().public().to_peer_id();
        let peers = (0..4)
            .map(|port| IrohPeer {
                node_id: Keypair::generate_ed25519().public().to_peer_id(),
                addresses: vec![PeerAddress::new(SocketAddr::from((
                    [127, 0, 0, 1],
                    10_000 + port,
                )))],
            })
            .collect::<Vec<_>>();
        let mut routing = BTreeMap::new();
        for peer in &peers {
            upsert_routing(&mut routing, peer.clone(), local);
        }
        let mut expected = peers.clone();
        expected.sort_by_key(|peer| xor_distance(peer.node_id, local));
        expected.truncate(2);
        assert_eq!(closest_peers(&routing, local, 2), expected);
    }

    #[tokio::test]
    async fn discovery_connects_nodes_that_do_not_share_a_bootstrap_peer() {
        let (mut node_a, peer_a) = spawn_test_node(Vec::new()).await;
        let (node_b, peer_b) = spawn_test_node(vec![peer_a.clone()]).await;
        let (mut node_c, _) = spawn_test_node(vec![peer_b]).await;

        node_c
            .commands
            .send(TransportCommand::BootstrapDiscovery)
            .await
            .expect("start discovery");
        next_matching(&mut node_c, "discovered transitive peer", |event| {
            matches!(
                event,
                TransportEvent::Discovery(DiscoveryEvent::PeerUpserted { peer, .. })
                    if *peer == peer_a.node_id
            )
        })
        .await;

        node_c
            .commands
            .send(TransportCommand::DialKnown {
                peers: vec![peer_a.node_id],
            })
            .await
            .expect("dial discovered peer");
        next_matching(&mut node_c, "connected to discovered peer", |event| {
            matches!(
                event,
                TransportEvent::ConnectionEstablished { peer, .. }
                    if *peer == peer_a.node_id
            )
        })
        .await;

        assert_request_response_contract(&mut node_c, &mut node_a, peer_a.node_id).await;

        for node in [&node_a, &node_b, &node_c] {
            node.commands
                .send(TransportCommand::Shutdown)
                .await
                .expect("shutdown test node");
        }
    }
}
