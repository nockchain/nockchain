use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use libp2p::core::transport::PortUse;
use libp2p::core::Endpoint;
use libp2p::swarm::{
    dummy, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
    THandlerOutEvent, ToSwarm,
};
use libp2p::{Multiaddr, PeerId};
use smallvec::SmallVec;

use crate::p2p_state::{RequestEndpoint, RequestEndpoints};
use crate::p2p_util::quic_socket_addr;

/// One reclaimed slot belongs to its configured dial until that dial finishes.
/// This also gates already-pending unrelated handshakes, which could otherwise
/// win the slot between ConnectionClosed and the recovery handshake.
#[derive(Default)]
pub(crate) struct RecoverySlot {
    attempts: BTreeMap<ConnectionId, Multiaddr>,
    reservation: Option<Reservation>,
    dial_timeout: Duration,
    initial_cursor: usize,
    prefer_backbone: bool,
    live: BTreeMap<PeerId, SmallVec<[TrackedEndpoint; 2]>>,
    pending_address_changes: SmallVec<[ConnectionAddressChange; 2]>,
}

#[derive(Debug)]
pub(crate) struct ConnectionAddressChange {
    pub peer_id: PeerId,
    pub connection_id: ConnectionId,
    pub address: Multiaddr,
}

struct TrackedEndpoint {
    connection_id: ConnectionId,
    endpoint: Option<SocketAddr>,
}

struct Reservation {
    closing: Option<ConnectionId>,
    dialing: Option<ConnectionId>,
    address: Multiaddr,
    expires: Instant,
}

impl RecoverySlot {
    pub(crate) fn new(dial_timeout: Duration) -> Self {
        Self {
            dial_timeout,
            ..Self::default()
        }
    }

    pub(crate) fn has_capacity(&self) -> bool {
        self.attempts.len() < 64
    }

    pub(crate) fn pending_at(&self, address: &Multiaddr) -> bool {
        self.attempts.values().any(|pending| pending == address)
    }

    pub(crate) fn pending_address(&self, id: ConnectionId) -> Option<&Multiaddr> {
        self.attempts.get(&id)
    }

    pub(crate) fn initial_offset(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            self.initial_cursor % len
        }
    }

    pub(crate) fn advance_initial(&mut self, next: usize) {
        self.initial_cursor = next;
    }

    pub(crate) fn next_pool(&mut self) -> bool {
        self.prefer_backbone = !self.prefer_backbone;
        self.prefer_backbone
    }

    /// The request-response behaviour selects from the same synchronous
    /// FromSwarm lifecycle. Async driver state may still be awaiting its event.
    pub(crate) fn request_endpoints(&self, peer: &PeerId) -> Option<RequestEndpoints> {
        let live = self.live.get(peer)?;
        if live.is_empty() {
            return None;
        }
        live.iter()
            .map(|connection| {
                Some(RequestEndpoint {
                    connection_id: connection.connection_id,
                    endpoint: connection.endpoint?,
                })
            })
            .collect()
    }

    pub(crate) fn numeric_request_endpoints(&self, peer: &PeerId) -> RequestEndpoints {
        self.live
            .get(peer)
            .into_iter()
            .flat_map(|connections| connections.iter())
            .filter_map(|connection| {
                Some(RequestEndpoint {
                    connection_id: connection.connection_id,
                    endpoint: connection.endpoint?,
                })
            })
            .collect()
    }

    pub(crate) fn connection_endpoint(
        &self,
        peer: &PeerId,
        id: ConnectionId,
    ) -> Option<SocketAddr> {
        self.live
            .get(peer)?
            .iter()
            .find(|connection| connection.connection_id == id)?
            .endpoint
    }
    pub(crate) fn track(&mut self, id: ConnectionId, address: Multiaddr) -> bool {
        if self.pending_at(&address) {
            return false;
        }
        if self.attempts.len() >= 64 {
            return false;
        }
        self.attempts.insert(id, address);
        true
    }

    pub(crate) fn finish(&mut self, id: ConnectionId) -> Option<Multiaddr> {
        if self
            .reservation
            .as_ref()
            .is_some_and(|r| r.dialing == Some(id))
        {
            self.reservation = None;
        }
        self.attempts.remove(&id)
    }

    pub(crate) fn reserved(&mut self) -> bool {
        if self
            .reservation
            .as_ref()
            .is_some_and(|r| r.expires <= Instant::now())
        {
            self.reservation = None;
        }
        self.reservation.is_some()
    }

    pub(crate) fn reserve(&mut self, closing: ConnectionId, address: Multiaddr) {
        self.reservation = Some(Reservation {
            closing: Some(closing),
            dialing: None,
            address,
            expires: Instant::now() + Duration::from_secs(30),
        });
    }

    pub(crate) fn closed(&mut self, id: ConnectionId) -> Option<Multiaddr> {
        if !self.reserved() {
            return None;
        }
        let reservation = self.reservation.as_mut()?;
        if reservation.closing != Some(id) {
            return None;
        }
        reservation.closing = None;
        Some(reservation.address.clone())
    }

    pub(crate) fn arm(&mut self, id: ConnectionId) {
        if let Some(reservation) = self.reservation.as_mut() {
            reservation.dialing = Some(id);
            reservation.expires = Instant::now() + self.dial_timeout;
        }
    }

    fn admit(&mut self, id: ConnectionId) -> Result<(), ConnectionDenied> {
        if self.reserved() && self.reservation.as_ref().and_then(|r| r.dialing) != Some(id) {
            return Err(ConnectionDenied::new(std::io::Error::other(
                "connection slot reserved for configured peer recovery",
            )));
        }
        Ok(())
    }
}

impl NetworkBehaviour for RecoverySlot {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = ConnectionAddressChange;

    fn handle_established_inbound_connection(
        &mut self,
        id: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.admit(id)?;
        Ok(dummy::ConnectionHandler)
    }

    fn handle_established_outbound_connection(
        &mut self,
        id: ConnectionId,
        _: PeerId,
        _: &Multiaddr,
        _: Endpoint,
        _: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.admit(id)?;
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(event) => {
                self.live
                    .entry(event.peer_id)
                    .or_default()
                    .push(TrackedEndpoint {
                        connection_id: event.connection_id,
                        endpoint: quic_socket_addr(event.endpoint.get_remote_address()),
                    });
            }
            FromSwarm::ConnectionClosed(event) => {
                if let Some(connections) = self.live.get_mut(&event.peer_id) {
                    connections
                        .retain(|connection| connection.connection_id != event.connection_id);
                    if connections.is_empty() {
                        self.live.remove(&event.peer_id);
                    }
                }
                self.pending_address_changes
                    .retain(|pending| pending.connection_id != event.connection_id);
            }
            FromSwarm::AddressChange(event) => {
                if let Some(connection) =
                    self.live.get_mut(&event.peer_id).and_then(|connections| {
                        connections
                            .iter_mut()
                            .find(|connection| connection.connection_id == event.connection_id)
                    })
                {
                    connection.endpoint = quic_socket_addr(event.new.get_remote_address());
                    let address = event.new.get_remote_address();
                    if let Some(pending) = self
                        .pending_address_changes
                        .iter_mut()
                        .find(|pending| pending.connection_id == event.connection_id)
                    {
                        pending.address.clone_from(address);
                    } else {
                        // At most one owned update per live connection.
                        self.pending_address_changes.push(ConnectionAddressChange {
                            peer_id: event.peer_id,
                            connection_id: event.connection_id,
                            address: address.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        _: PeerId,
        _: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(&mut self, _: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        match self.pending_address_changes.pop() {
            Some(event) => Poll::Ready(ToSwarm::GenerateEvent(event)),
            None => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reclaimed_slot_admits_only_its_recovery_handshake_and_releases_on_completion() {
        let mut slot = RecoverySlot::new(Duration::from_secs(180));
        let closing = ConnectionId::new_unchecked(1);
        let recovery = ConnectionId::new_unchecked(2);
        let unrelated = ConnectionId::new_unchecked(3);
        let address = "/ip4/127.0.0.1/udp/3000/quic-v1"
            .parse()
            .expect("recovery address must be a valid QUIC multiaddr");
        slot.reserve(closing, address);
        assert!(slot.admit(unrelated).is_err());
        assert!(slot.closed(closing).is_some());
        slot.arm(recovery);
        assert!(slot.admit(unrelated).is_err());
        assert!(slot.admit(recovery).is_ok());
        slot.finish(recovery);
        assert!(slot.admit(unrelated).is_ok());
    }

    #[test]
    fn resolved_connection_events_are_owned_deduplicated_and_cancelled_on_close() {
        use libp2p::core::ConnectedPoint;
        use libp2p::swarm::behaviour::{AddressChange, ConnectionClosed, ConnectionEstablished};
        let mut slot = RecoverySlot::new(Duration::from_secs(180));
        let peer_id = PeerId::random();
        let connection_id = ConnectionId::new_unchecked(10);
        let endpoint = |address: &str| ConnectedPoint::Dialer {
            address: address
                .parse()
                .expect("test endpoint must be a valid multiaddr"),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        };
        let original = endpoint("/dns4/localhost/udp/3000/quic-v1");
        let numeric = endpoint("/ip4/127.0.0.1/udp/3000/quic-v1");
        let latest = endpoint("/ip4/127.0.0.1/udp/3001/quic-v1");
        slot.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id,
            connection_id,
            endpoint: &original,
            failed_addresses: &[],
            other_established: 0,
        }));
        assert!(slot.request_endpoints(&peer_id).is_none());
        slot.on_swarm_event(FromSwarm::AddressChange(AddressChange {
            peer_id,
            connection_id,
            old: &original,
            new: &numeric,
        }));
        slot.on_swarm_event(FromSwarm::AddressChange(AddressChange {
            peer_id,
            connection_id,
            old: &numeric,
            new: &latest,
        }));
        assert_eq!(slot.pending_address_changes.len(), 1);
        assert_eq!(
            slot.request_endpoints(&peer_id)
                .expect("resolved peer must have a request endpoint")[0]
                .endpoint
                .port(),
            3001
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        match slot.poll(&mut cx) {
            Poll::Ready(ToSwarm::GenerateEvent(event)) => {
                assert_eq!(event.peer_id, peer_id);
                assert_eq!(event.connection_id, connection_id);
                assert_eq!(&event.address, latest.get_remote_address());
            }
            _ => panic!("resolved endpoint must be forwarded through the behaviour"),
        }
        assert!(slot.poll(&mut cx).is_pending());
        slot.on_swarm_event(FromSwarm::AddressChange(AddressChange {
            peer_id,
            connection_id,
            old: &latest,
            new: &numeric,
        }));
        slot.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id,
            connection_id,
            endpoint: &numeric,
            cause: None,
            remaining_established: 0,
        }));
        assert!(slot.request_endpoints(&peer_id).is_none());
        assert!(slot.pending_address_changes.is_empty());
        assert!(slot.poll(&mut cx).is_pending());
    }
}
