use std::fmt;
use std::net::SocketAddr;

/// Stable Nockchain node identity.
///
/// This alias deliberately preserves the historical libp2p PeerId encoding
/// because those exact bytes are committed by Nous proof of work and the
/// base58 form crosses the kernel boundary. Transport backends map their
/// authenticated public key to this canonical identity before emitting
/// events to the shared driver.
pub use libp2p_identity::PeerId as NodeId;

macro_rules! local_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

local_id!(ConnectionId);
local_id!(RequestId);
local_id!(InboundRequestId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PeerAddress {
    pub socket: SocketAddr,
}

impl PeerAddress {
    pub const fn new(socket: SocketAddr) -> Self {
        Self { socket }
    }

    pub const fn ip_addr(self) -> Option<std::net::IpAddr> {
        Some(self.socket.ip())
    }
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.socket.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestFailure {
    Timeout,
    ConnectionClosed,
    UnsupportedProtocol,
    NotConnected,
    Codec(String),
    Io(String),
    Shutdown,
}

impl RequestFailure {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout | Self::ConnectionClosed | Self::Io(_))
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InboundFailure {
    ConnectionClosed,
    Timeout,
    UnsupportedProtocol,
    ResponseOmission,
    Codec(String),
    Io(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DialFailure {
    NoAddress,
    WrongNodeId { expected: NodeId, obtained: NodeId },
    PermissionDenied,
    Timeout,
    Io(String),
    Transport(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_preserves_peer_id_protocol_encoding() {
        let node_id = NodeId::random();
        let expected_bytes = node_id.to_bytes();
        let expected_base58 = node_id.to_base58();

        assert_eq!(node_id.to_bytes(), expected_bytes);
        assert_eq!(node_id.to_base58(), expected_base58);
        assert_eq!(NodeId::from_bytes(&expected_bytes), Ok(node_id));
        assert_eq!(expected_base58.parse::<NodeId>(), Ok(node_id));
    }

    #[test]
    fn request_failure_retry_classification_matches_current_policy() {
        assert!(RequestFailure::Timeout.is_transient());
        assert!(RequestFailure::ConnectionClosed.is_transient());
        assert!(RequestFailure::Io(String::from("closed")).is_transient());
        assert!(!RequestFailure::UnsupportedProtocol.is_transient());
        assert!(!RequestFailure::NotConnected.is_transient());
        assert!(!RequestFailure::Codec(String::from("bad cbor")).is_transient());
        assert!(!RequestFailure::Shutdown.is_transient());
    }
}
