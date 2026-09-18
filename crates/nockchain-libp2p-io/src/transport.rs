use std::time::Duration;

use tokio::sync::mpsc;

use crate::messages::{NockchainRequest, NockchainResponse};
use crate::types::{
    ConnectionDirection, ConnectionId, DialFailure, InboundFailure, InboundRequestId, NodeId,
    PeerAddress, RequestFailure, RequestId,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoverySource {
    Bootstrap,
    Identify,
    RoutingTable,
    InboundConnection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryFailure {
    NoKnownPeers,
    Timeout,
    Transport(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryEvent {
    PeerUpserted {
        peer: NodeId,
        addresses: Vec<PeerAddress>,
        source: DiscoverySource,
    },
    PeerRemoved {
        peer: NodeId,
    },
    BootstrapFinished {
        discovered: usize,
    },
    BootstrapFailed {
        failure: DiscoveryFailure,
    },
    RoutingTableStats {
        entries: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionCloseCause {
    Local,
    Remote,
    Idle,
    PermissionDenied,
    Transport(String),
}

#[derive(Clone, Debug)]
pub enum TransportCommand {
    Dial {
        peer: NodeId,
        addresses: Vec<PeerAddress>,
    },
    DialBootstrap,
    DialKnown {
        peers: Vec<NodeId>,
    },
    SendRequest {
        id: RequestId,
        peer: NodeId,
        request: NockchainRequest,
    },
    CompleteInbound {
        id: InboundRequestId,
        response: Option<NockchainResponse>,
    },
    DisconnectPeer {
        peer: NodeId,
    },
    CloseConnection {
        connection: ConnectionId,
    },
    BootstrapDiscovery,
    RefreshDiscovery,
    RemoveDiscoveredAddress {
        peer: Option<NodeId>,
        address: PeerAddress,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum TransportEvent {
    Listening {
        address: PeerAddress,
    },
    ConnectionEstablished {
        connection: ConnectionId,
        peer: NodeId,
        address: PeerAddress,
        direction: ConnectionDirection,
    },
    ConnectionClosed {
        connection: ConnectionId,
        peer: NodeId,
        cause: Option<ConnectionCloseCause>,
    },
    InboundConnectionLimitReached,
    IncomingRequest {
        id: InboundRequestId,
        connection: ConnectionId,
        peer: NodeId,
        request: NockchainRequest,
    },
    Response {
        id: RequestId,
        peer: NodeId,
        response: NockchainResponse,
    },
    RequestFailed {
        id: RequestId,
        peer: NodeId,
        failure: RequestFailure,
    },
    InboundFailed {
        id: InboundRequestId,
        peer: NodeId,
        failure: InboundFailure,
    },
    Liveness {
        connection: ConnectionId,
        peer: NodeId,
        result: Result<Duration, String>,
    },
    DialFailed {
        peer: Option<NodeId>,
        address: Option<PeerAddress>,
        failure: DialFailure,
    },
    Discovery(DiscoveryEvent),
    Fatal {
        error: String,
    },
}

pub struct TransportHandle {
    pub local_node_id: NodeId,
    pub commands: mpsc::Sender<TransportCommand>,
    pub events: mpsc::Receiver<TransportEvent>,
}

impl TransportHandle {
    pub async fn next_event(&mut self) -> Option<TransportEvent> {
        self.events.recv().await
    }
}

pub(crate) struct TransportActorChannels {
    pub commands: mpsc::Receiver<TransportCommand>,
    pub events: mpsc::Sender<TransportEvent>,
}

pub(crate) fn transport_channels(
    local_node_id: NodeId,
    capacity: usize,
) -> (TransportHandle, TransportActorChannels) {
    let (commands_tx, commands_rx) = mpsc::channel(capacity);
    let (events_tx, events_rx) = mpsc::channel(capacity);
    (
        TransportHandle {
            local_node_id,
            commands: commands_tx,
            events: events_rx,
        },
        TransportActorChannels {
            commands: commands_rx,
            events: events_tx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_transport_channels_preserve_command_order() {
        let node = NodeId::random();
        let peer = NodeId::random();
        let (handle, mut actor) = transport_channels(node, 2);

        handle
            .commands
            .send(TransportCommand::DisconnectPeer { peer })
            .await
            .expect("actor command receiver should be open");
        handle
            .commands
            .send(TransportCommand::Shutdown)
            .await
            .expect("actor command receiver should be open");

        assert!(matches!(
            actor.commands.recv().await,
            Some(TransportCommand::DisconnectPeer { peer: observed }) if observed == peer
        ));
        assert!(matches!(
            actor.commands.recv().await,
            Some(TransportCommand::Shutdown)
        ));
    }
}
