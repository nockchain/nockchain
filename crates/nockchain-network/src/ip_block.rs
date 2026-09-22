//! libp2p connection and Kademlia hooks for the transport-neutral peer policy.

use std::convert::Infallible;
use std::fmt;
use std::net::IpAddr;
use std::ops::{Deref, DerefMut};
use std::task::{Context, Poll};

use libp2p::core::transport::PortUse;
use libp2p::core::{Endpoint, Multiaddr};
use libp2p::identity::PeerId;
use libp2p::kad;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{
    dummy, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
    THandlerOutEvent, ToSwarm,
};

use crate::p2p_util::MultiaddrExt;
use crate::peer_policy::{AddressKey, PeerExclusions, PolicyAddress, TransportKind};
use crate::types::PeerAddress;

impl PolicyAddress for Multiaddr {
    fn address_key(&self, expected_peer: Option<PeerId>) -> Option<AddressKey> {
        let mut ip = None;
        let mut transport = TransportKind::Other;
        let mut port = None;
        let mut peer = expected_peer;

        for protocol in self.iter() {
            match protocol {
                Protocol::Ip4(addr) => ip = Some(IpAddr::V4(addr)),
                Protocol::Ip6(addr) => ip = Some(IpAddr::V6(addr)),
                Protocol::Tcp(value) => {
                    transport = TransportKind::Tcp;
                    port = Some(value);
                }
                Protocol::Udp(value) => {
                    transport = TransportKind::Udp;
                    port = Some(value);
                }
                Protocol::P2p(peer_id) if peer.is_none() => peer = Some(peer_id),
                _ => {}
            }
        }

        ip.map(|ip| AddressKey {
            ip,
            transport,
            port,
            expected_peer: peer,
        })
    }

    fn peer_address(&self) -> Option<PeerAddress> {
        let key = self.address_key(None)?;
        Some(PeerAddress::new(std::net::SocketAddr::new(
            key.ip, key.port?,
        )))
    }
}

/// A connection was refused because its remote endpoint is under exclusion.
#[derive(Debug)]
pub(crate) struct BlockedEndpoint {
    addr: Multiaddr,
}

impl fmt::Display for BlockedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "endpoint {} is temporarily excluded", self.addr)
    }
}

impl std::error::Error for BlockedEndpoint {}

fn enforce_not_excluded(
    exclusions: &PeerExclusions,
    addr: &Multiaddr,
    expected_peer: Option<PeerId>,
) -> Result<(), ConnectionDenied> {
    if exclusions.is_address_excluded(addr, expected_peer) {
        return Err(ConnectionDenied::new(BlockedEndpoint {
            addr: addr.clone(),
        }));
    }
    Ok(())
}

fn enforce_ip_not_excluded(
    exclusions: &PeerExclusions,
    addr: &Multiaddr,
) -> Result<(), ConnectionDenied> {
    if addr
        .ip_addr()
        .is_some_and(|ip| exclusions.is_ip_excluded(&ip))
    {
        return Err(ConnectionDenied::new(BlockedEndpoint {
            addr: addr.clone(),
        }));
    }
    Ok(())
}

/// Kademlia plus IP/endpoint filtering for the addresses Kademlia contributes
/// to dials.
pub(crate) struct IpFilteredKad {
    inner: kad::Behaviour<kad::store::MemoryStore>,
    exclusions: PeerExclusions,
}

impl IpFilteredKad {
    pub(crate) fn new(
        inner: kad::Behaviour<kad::store::MemoryStore>,
        exclusions: PeerExclusions,
    ) -> Self {
        Self { inner, exclusions }
    }

    fn is_allowed_addr(&self, addr: &Multiaddr, expected_peer: Option<PeerId>) -> bool {
        !self.exclusions.is_address_excluded(addr, expected_peer)
    }
}

impl Deref for IpFilteredKad {
    type Target = kad::Behaviour<kad::store::MemoryStore>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for IpFilteredKad {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl NetworkBehaviour for IpFilteredKad {
    type ConnectionHandler =
        <kad::Behaviour<kad::store::MemoryStore> as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = <kad::Behaviour<kad::store::MemoryStore> as NetworkBehaviour>::ToSwarm;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.inner
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.inner
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let addresses = self.inner.handle_pending_outbound_connection(
            connection_id, maybe_peer, addresses, effective_role,
        )?;
        Ok(addresses
            .into_iter()
            .filter(|addr| self.is_allowed_addr(addr, maybe_peer))
            .collect())
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        enforce_not_excluded(&self.exclusions, addr, Some(peer))?;
        self.inner.handle_established_outbound_connection(
            connection_id, peer, addr, role_override, port_use,
        )
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        self.inner.on_swarm_event(event);
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        self.inner
            .on_connection_handler_event(peer_id, connection_id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        self.inner.poll(cx)
    }
}

/// A [`NetworkBehaviour`] that denies IP-excluded connections and applies
/// endpoint cooldowns to outbound dials.
pub(crate) struct Behaviour {
    exclusions: PeerExclusions,
}

impl Behaviour {
    pub(crate) fn new(exclusions: PeerExclusions) -> Self {
        Self { exclusions }
    }

    fn enforce(
        &self,
        addr: &Multiaddr,
        expected_peer: Option<PeerId>,
    ) -> Result<(), ConnectionDenied> {
        enforce_not_excluded(&self.exclusions, addr, expected_peer)
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        _: ConnectionId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        enforce_ip_not_excluded(&self.exclusions, remote_addr)
    }

    fn handle_established_inbound_connection(
        &mut self,
        _: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        enforce_ip_not_excluded(&self.exclusions, remote_addr)?;
        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        _: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        if addresses.is_empty() {
            return Ok(vec![]);
        }

        if addresses
            .iter()
            .any(|addr| !self.exclusions.is_address_excluded(addr, maybe_peer))
        {
            return Ok(vec![]);
        }

        self.enforce(&addresses[0], maybe_peer)?;
        Ok(vec![])
    }

    fn handle_established_outbound_connection(
        &mut self,
        _: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        _: Endpoint,
        _: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.enforce(addr, Some(peer))?;
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, _event: FromSwarm) {}

    fn on_connection_handler_event(
        &mut self,
        _: PeerId,
        _: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}
