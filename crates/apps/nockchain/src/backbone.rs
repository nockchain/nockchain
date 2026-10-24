//! Backbone peers that nodes dial by default on realnet.
//!
//! Libp2p multiaddrs don't support const construction, so these are stored as
//! string literals and parsed at startup. Rather than dialing every backbone on
//! every attempt, the libp2p driver dials [`DEFAULT_BACKBONE_PEER_COUNT`] of
//! them round-robin, advancing the window on each dial so successive attempts
//! spread their load across the whole set instead of hammering the same few.

/// Full set of realnet backbone peers, as libp2p multiaddr strings.
pub const BACKBONE_NODES: &[&str] = &[
    "/ip4/216.158.74.98/udp/33000/quic-v1", "/ip4/23.252.122.178/udp/33000/quic-v1",
    "/ip4/173.231.48.98/udp/33000/quic-v1", "/dnsaddr/nockchain-backbone.nockbox.org",
    "/dnsaddr/public.nockblocks.com", "/ip4/216.158.95.10/udp/33000/quic-v1",
    "/ip4/34.150.94.224/udp/3006/quic-v1",
];

/// How many backbone peers to dial per attempt.
pub const DEFAULT_BACKBONE_PEER_COUNT: usize = 3;

#[cfg(test)]
mod tests {
    use libp2p::multiaddr::Multiaddr;

    use super::*;

    #[test]
    fn backbone_nodes_are_valid_multiaddrs() {
        for node in BACKBONE_NODES {
            node.parse::<Multiaddr>()
                .unwrap_or_else(|e| panic!("invalid backbone multiaddr {node}: {e}"));
        }
    }

    #[test]
    fn backbone_nodes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for node in BACKBONE_NODES {
            assert!(seen.insert(*node), "duplicate backbone node {node}");
        }
    }

    #[test]
    fn at_least_one_dial_window_available() {
        assert!(BACKBONE_NODES.len() >= DEFAULT_BACKBONE_PEER_COUNT);
    }
}
