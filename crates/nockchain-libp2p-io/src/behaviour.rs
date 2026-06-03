use std::convert::Infallible;

use libp2p::request_response::cbor;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{
    allow_block_list, connection_limits, identify, kad, memory_connection_limits, ping,
    request_response,
};
use tracing::info;

use crate::config::LibP2PConfig;
use crate::ip_block::{self, PeerExclusions};
use crate::messages::{NockchainRequest, NockchainResponse};

pub(crate) fn request_response_protocols(
    libp2p_config: &LibP2PConfig,
) -> Vec<(libp2p::StreamProtocol, request_response::ProtocolSupport)> {
    let mut protocols = Vec::with_capacity(2);
    let gen1 = (
        libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen1_protocol_version()),
        request_response::ProtocolSupport::Full,
    );
    let gen2_support = match (
        libp2p_config.req_res_gen2_accept_enabled, libp2p_config.req_res_gen2_send_enabled,
    ) {
        (true, true) => Some(request_response::ProtocolSupport::Full),
        (true, false) => Some(request_response::ProtocolSupport::Inbound),
        (false, true) => Some(request_response::ProtocolSupport::Outbound),
        (false, false) => None,
    };

    if libp2p_config.req_res_gen2_send_enabled {
        if let Some(gen2_support) = gen2_support {
            protocols.push((
                libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen2_protocol_version()),
                gen2_support,
            ));
        }
        protocols.push(gen1);
    } else {
        protocols.push(gen1);
        if let Some(gen2_support) = gen2_support {
            protocols.push((
                libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen2_protocol_version()),
                gen2_support,
            ));
        }
    }

    let protocol_summary: Vec<String> = protocols
        .iter()
        .map(|(proto, support)| {
            let mode = match (support.inbound(), support.outbound()) {
                (true, true) => "full",
                (true, false) => "inbound-only",
                (false, true) => "outbound-only",
                (false, false) => "none",
            };
            format!("{} ({})", proto.as_ref(), mode)
        })
        .collect();
    info!(
        gen2_accept = libp2p_config.req_res_gen2_accept_enabled,
        gen2_send = libp2p_config.req_res_gen2_send_enabled,
        protocols = %protocol_summary.join(", "),
        "Nous req-res protocol registration"
    );

    protocols
}

pub(crate) fn build_request_response_behaviour(
    libp2p_config: &LibP2PConfig,
) -> cbor::Behaviour<NockchainRequest, NockchainResponse> {
    let request_response_config = request_response::Config::default()
        .with_max_concurrent_streams(libp2p_config.request_response_max_concurrent_streams())
        .with_request_timeout(libp2p_config.request_response_timeout());
    let request_response_codec = cbor::codec::Codec::default()
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
    /// Actual comms with custom connection handler that keeps connections alive
    pub request_response: cbor::Behaviour<NockchainRequest, NockchainResponse>,
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
            .with_hide_listen_addrs(true); // Only send externally confirmed addresses so we don't send loopback addresses
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
    RequestResponse(request_response::Event<NockchainRequest, NockchainResponse>),
    /// Peer store events
    PeerStore(libp2p::peer_store::memory_store::Event),
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

impl From<request_response::Event<NockchainRequest, NockchainResponse>> for NockchainEvent {
    fn from(event: request_response::Event<NockchainRequest, NockchainResponse>) -> Self {
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
    use std::collections::BTreeSet;

    use super::*;

    fn outbound_protocol_order(config: &LibP2PConfig) -> Vec<String> {
        request_response_protocols(config)
            .into_iter()
            .filter(|(_, support)| support.outbound())
            .map(|(protocol, _)| protocol.as_ref().to_string())
            .collect()
    }

    fn inbound_protocol_set(config: &LibP2PConfig) -> BTreeSet<String> {
        request_response_protocols(config)
            .into_iter()
            .filter(|(_, support)| support.inbound())
            .map(|(protocol, _)| protocol.as_ref().to_string())
            .collect()
    }

    fn first_common_outbound_protocol(
        local: &LibP2PConfig,
        remote: &LibP2PConfig,
    ) -> Option<String> {
        let remote_inbound = inbound_protocol_set(remote);
        outbound_protocol_order(local)
            .into_iter()
            .find(|protocol| remote_inbound.contains(protocol))
    }

    #[test]
    fn test_request_response_protocols_default_enables_gen2_and_gen1() {
        let config = LibP2PConfig::default();

        let protocols = request_response_protocols(&config);

        // Gen2 ships enabled by default: gen2 is preferred (sent first) with
        // gen1 retained underneath for fallback.
        assert_eq!(protocols.len(), 2);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_gen2_protocol_version()
        );
        assert!(protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
        assert_eq!(
            protocols[1].0.as_ref(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
    }

    #[test]
    fn test_request_response_protocols_default_enables_gen2_inbound_and_outbound() {
        let config = LibP2PConfig::default();

        let outbound = outbound_protocol_order(&config);
        let inbound = inbound_protocol_set(&config);

        assert_eq!(
            outbound,
            vec![
                LibP2PConfig::req_res_gen2_protocol_version().to_string(),
                LibP2PConfig::req_res_gen1_protocol_version().to_string(),
            ]
        );
        assert!(inbound.contains(LibP2PConfig::req_res_gen1_protocol_version()));
        assert!(inbound.contains(LibP2PConfig::req_res_gen2_protocol_version()));
    }

    #[test]
    fn test_request_response_protocols_prefer_gen2_when_send_enabled() {
        let config = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };

        let protocols = request_response_protocols(&config);

        assert_eq!(protocols.len(), 2);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_gen2_protocol_version()
        );
        assert!(protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
        assert_eq!(
            protocols[1].0.as_ref(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
    }

    #[test]
    fn test_request_response_protocols_drop_gen2_when_accept_and_send_disabled() {
        let config = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        };

        let protocols = request_response_protocols(&config);

        assert_eq!(protocols.len(), 1);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
        assert!(protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
    }

    #[test]
    fn test_request_response_protocols_allow_inbound_only_gen2() {
        let config = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        };

        let protocols = request_response_protocols(&config);

        assert_eq!(protocols.len(), 2);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
        assert!(protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
        assert_eq!(
            protocols[1].0.as_ref(),
            LibP2PConfig::req_res_gen2_protocol_version()
        );
        assert!(protocols[1].1.inbound());
        assert!(!protocols[1].1.outbound());
    }

    #[test]
    fn test_request_response_protocols_allow_outbound_only_gen2() {
        let config = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };

        let protocols = request_response_protocols(&config);

        assert_eq!(protocols.len(), 2);
        assert_eq!(
            protocols[0].0.as_ref(),
            LibP2PConfig::req_res_gen2_protocol_version()
        );
        assert!(!protocols[0].1.inbound());
        assert!(protocols[0].1.outbound());
        assert_eq!(
            protocols[1].0.as_ref(),
            LibP2PConfig::req_res_gen1_protocol_version()
        );
    }

    #[test]
    fn test_dual_stack_matrix_gen1_to_gen1_negotiates_gen1() {
        let local = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        };
        let remote = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        };

        let negotiated = first_common_outbound_protocol(&local, &remote);

        assert_eq!(
            negotiated.as_deref(),
            Some(LibP2PConfig::req_res_gen1_protocol_version())
        );
    }

    #[test]
    fn test_dual_stack_matrix_gen2_to_gen2_prefers_gen2() {
        let local = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };
        let remote = LibP2PConfig {
            req_res_gen2_accept_enabled: true,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };

        let negotiated = first_common_outbound_protocol(&local, &remote);

        assert_eq!(
            negotiated.as_deref(),
            Some(LibP2PConfig::req_res_gen2_protocol_version())
        );
    }

    #[test]
    fn test_dual_stack_matrix_gen2_sender_keeps_gen1_fallback_headroom() {
        let local = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: true,
            ..LibP2PConfig::default()
        };
        let remote = LibP2PConfig {
            req_res_gen2_accept_enabled: false,
            req_res_gen2_send_enabled: false,
            ..LibP2PConfig::default()
        };

        let local_outbound = outbound_protocol_order(&local);
        let remote_inbound = inbound_protocol_set(&remote);

        assert_eq!(
            local_outbound.first().map(String::as_str),
            Some(LibP2PConfig::req_res_gen2_protocol_version())
        );
        assert!(local_outbound
            .iter()
            .any(|protocol| protocol == LibP2PConfig::req_res_gen1_protocol_version()));
        assert!(remote_inbound.contains(LibP2PConfig::req_res_gen1_protocol_version()));
        assert!(!remote_inbound.contains(LibP2PConfig::req_res_gen2_protocol_version()));
    }
}
