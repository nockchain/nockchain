use std::convert::Infallible;

use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{ConnectionId, NetworkBehaviour};
use libp2p::{
    allow_block_list, connection_limits, identify, kad, memory_connection_limits, ping,
    request_response, Multiaddr, PeerId,
};

use crate::config::LibP2PConfig;
use crate::ip_block::{self, PeerExclusions};
use crate::messages::{NockchainRequest, NockchainResponse};

pub(crate) mod codec;
mod recovery;

/// Application response with local-only accounting for a successfully decoded stream.
#[derive(Debug, Clone)]
pub struct TransportResponse {
    pub message: NockchainResponse,
    pub received_bytes: Option<usize>,
}

impl TransportResponse {
    pub fn outgoing(message: NockchainResponse) -> Self {
        Self {
            message,
            received_bytes: None,
        }
    }
}

pub(crate) fn request_response_protocols(
    _libp2p_config: &LibP2PConfig,
) -> Vec<(libp2p::StreamProtocol, request_response::ProtocolSupport)> {
    vec![(
        libp2p::StreamProtocol::new(LibP2PConfig::req_res_protocol_version()),
        request_response::ProtocolSupport::Full,
    )]
}

pub(crate) fn build_request_response_behaviour(
    libp2p_config: &LibP2PConfig,
) -> request_response::Behaviour<codec::NockchainCodec> {
    let request_response_config = request_response::Config::default()
        .with_max_concurrent_streams(libp2p_config.request_response_max_concurrent_streams())
        .with_request_timeout(libp2p_config.request_response_timeout());
    let request_response_codec = codec::NockchainCodec::default()
        .set_request_size_maximum(libp2p_config.gen2_batch_max_bytes() as u64)
        .set_response_size_maximum(libp2p_config.gen2_batch_max_bytes() as u64);

    request_response::Behaviour::with_codec(
        request_response_codec,
        request_response_protocols(libp2p_config),
        request_response_config,
    )
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "NockchainEvent")]
/** Composed [NetworkBehaviour] implementation for Nockchain */
pub(crate) struct NockchainBehaviour {
    pub recovery: recovery::RecoverySlot,
    /// Allows nodes to connect via just IP and port and exchange pubkeys
    identify: identify::Behaviour,
    /// Connectivity testing
    ping: ping::Behaviour,
    /// Peer discovery via a DHT
    pub kad: ip_block::IpFilteredKad,
    /// Peer banning (by peer id)
    pub allow_block_list: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    /// Connection gating by IP (deny banned IPs regardless of peer id)
    pub ip_block: ip_block::Behaviour,
    /// Peer whitelisting
    pub allow_peers: Toggle<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>,
    /// Connection limiting
    connection_limits: connection_limits::Behaviour,
    /// Memory connection limits
    memory_connection_limits: Toggle<memory_connection_limits::Behaviour>,
    /// Peer store for tracking peer information (including addresses)
    pub peer_store: libp2p::peer_store::Behaviour<libp2p::peer_store::memory_store::MemoryStore>,
    /// Keep request-response after all admission gates: its established hook
    /// records a connection before returning the handler.
    pub request_response: request_response::Behaviour<codec::NockchainCodec>,
}

impl NockchainBehaviour {
    pub(crate) fn pre_new(
        libp2p_config: LibP2PConfig,
        allowed: Option<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>,
        limits: connection_limits::ConnectionLimits,
        memory_limits: Option<memory_connection_limits::Behaviour>,
        peer_exclusions: PeerExclusions,
    ) -> impl FnOnce(&libp2p::identity::Keypair) -> Self {
        move |keypair: &libp2p::identity::Keypair| {
            let peer_id = libp2p::identity::PeerId::from_public_key(&keypair.public());

            let identify_config = identify::Config::new(
                String::from(LibP2PConfig::identify_protocol_version()),
                keypair.public(),
            )
            .with_interval(libp2p_config.identify_interval())
            .with_hide_listen_addrs(true); // Advertise only externally corroborated addresses.
            let identify_behaviour = identify::Behaviour::new(identify_config);

            let memory_store = kad::store::MemoryStore::new(peer_id);

            let mut kad_config = kad::Config::new(libp2p::StreamProtocol::new(
                LibP2PConfig::kad_protocol_version(),
            ));
            kad_config.set_max_packet_size(16 * 1024 * 4);
            let kad_behaviour = kad::Behaviour::with_config(peer_id, memory_store, kad_config);

            let request_response_behaviour = build_request_response_behaviour(&libp2p_config);
            let connection_limits_behaviour = connection_limits::Behaviour::new(limits);
            let memory_connection_limits =
                Toggle::<memory_connection_limits::Behaviour>::from(memory_limits);

            let allow_peers =
                Toggle::<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>::from(
                    allowed,
                );
            let peer_store_config = libp2p::peer_store::memory_store::Config::default();
            let record_capacity = libp2p_config.peer_store_record_capacity;
            let peer_store_config = peer_store_config.set_record_capacity(record_capacity);
            let peer_store_memory =
                libp2p::peer_store::memory_store::MemoryStore::new(peer_store_config);

            let peer_store_behaviour = libp2p::peer_store::Behaviour::new(peer_store_memory);
            NockchainBehaviour {
                recovery: recovery::RecoverySlot::new(libp2p_config.connection_timeout()),
                ping: ping::Behaviour::default(),
                identify: identify_behaviour,
                kad: ip_block::IpFilteredKad::new(kad_behaviour, peer_exclusions.clone()),
                allow_block_list: allow_block_list::Behaviour::default(),
                ip_block: ip_block::Behaviour::new(peer_exclusions),
                allow_peers,
                request_response: request_response_behaviour,
                connection_limits: connection_limits_behaviour,
                memory_connection_limits,
                peer_store: peer_store_behaviour,
            }
        }
    }
}

// TODO: We need to box identify::Event but we are on stable so no boxed patterns.
#[derive(Debug)]
#[allow(dead_code)]
#[allow(clippy::large_enum_variant)]
/** Events that can be emitted by the swarm running [NockchainBehaviour] */
pub enum NockchainEvent {
    /// Received or sent identify message
    Identify(identify::Event),
    /// Received or failed ping
    Ping(ping::Event),
    /// DHT state changes
    Kad(kad::Event),
    /// Request or response received from peer
    RequestResponse(request_response::Event<NockchainRequest, TransportResponse>),
    /// Peer store events
    PeerStore(libp2p::peer_store::memory_store::Event),
    /// Transport resolved or changed the remote connection endpoint.
    ConnectionAddressChanged {
        peer_id: PeerId,
        connection_id: ConnectionId,
        address: Multiaddr,
    },
}

impl From<identify::Event> for NockchainEvent {
    fn from(event: identify::Event) -> Self {
        Self::Identify(event)
    }
}

impl From<ping::Event> for NockchainEvent {
    fn from(event: ping::Event) -> Self {
        Self::Ping(event)
    }
}

impl From<kad::Event> for NockchainEvent {
    fn from(event: kad::Event) -> Self {
        Self::Kad(event)
    }
}

impl From<Infallible> for NockchainEvent {
    fn from(i: Infallible) -> Self {
        match i {}
    }
}

impl From<recovery::ConnectionAddressChange> for NockchainEvent {
    fn from(event: recovery::ConnectionAddressChange) -> Self {
        Self::ConnectionAddressChanged {
            peer_id: event.peer_id,
            connection_id: event.connection_id,
            address: event.address,
        }
    }
}

impl From<request_response::Event<NockchainRequest, TransportResponse>> for NockchainEvent {
    fn from(event: request_response::Event<NockchainRequest, TransportResponse>) -> Self {
        Self::RequestResponse(event)
    }
}

impl From<libp2p::peer_store::memory_store::Event> for NockchainEvent {
    fn from(event: libp2p::peer_store::memory_store::Event) -> Self {
        Self::PeerStore(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_response_supports_only_gen2() {
        let protocols = request_response_protocols(&LibP2PConfig::default());

        assert_eq!(protocols.len(), 1);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_protocol_version()
        );
        assert!(protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
    }
}
