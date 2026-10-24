use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use nockapp::NockAppError;
use nockvm::noun::{Noun, NounSpace};
use tracing::warn;

// The warn logs are specifically constructed for fail2ban
// Changing these breaks the integration with the fail2ban regex
pub fn log_fail2ban_ipv4(peer_id: &PeerId, ip: &Ipv4Addr) {
    warn!("fail2ban: Blocked peer {peer_id} with IPv4 address: {ip}");
}
pub fn log_fail2ban_ipv6(peer_id: &PeerId, ip: &Ipv6Addr) {
    warn!("fail2ban: Blocked peer {peer_id} with IPv6 address: {ip}");
}

pub(crate) fn multiaddr_without_p2p(addr: &Multiaddr) -> Multiaddr {
    addr.iter()
        .filter(|protocol| !matches!(protocol, Protocol::P2p(_)))
        .collect()
}

pub trait PeerIdExt {
    fn from_noun(noun: Noun, space: &NounSpace) -> Result<PeerId, NockAppError>;
}

impl PeerIdExt for PeerId {
    fn from_noun(noun: Noun, space: &NounSpace) -> Result<PeerId, NockAppError> {
        let peer_id_bytes = noun.in_space(space).as_atom()?.to_bytes_until_nul()?;
        let peer_id_str = String::from_utf8(peer_id_bytes)?;
        PeerId::from_str(&peer_id_str)
            .map_err(|_| NockAppError::OtherError(String::from("Failed to parse PeerId from noun")))
    }
}

pub trait MultiaddrExt {
    /// Normalize IPv4-mapped IPv6 so exclusion and per-IP accounting keys agree.
    fn canonical_ip_addr(&self) -> Option<IpAddr>;
}

/// Use one identity for an IPv4 endpoint even when a transport reports its
/// IPv4-mapped IPv6 form. Exclusion, allowlist and IP-bucket keys must agree.
pub(crate) fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

pub(crate) fn quic_socket_addr(address: &Multiaddr) -> Option<SocketAddr> {
    let mut protocols = address.iter();
    let ip = match protocols.next()? {
        Protocol::Ip4(ip) => IpAddr::V4(ip),
        Protocol::Ip6(ip) => canonical_ip(IpAddr::V6(ip)),
        _ => return None,
    };
    let Protocol::Udp(port) = protocols.next()? else {
        return None;
    };
    if port == 0 || !matches!(protocols.next(), Some(Protocol::QuicV1)) {
        return None;
    }
    match protocols.next() {
        None => Some(SocketAddr::new(ip, port)),
        Some(Protocol::P2p(_)) if protocols.next().is_none() => Some(SocketAddr::new(ip, port)),
        _ => None,
    }
}

pub(crate) fn same_endpoint_without_p2p(left: &Multiaddr, right: &Multiaddr) -> bool {
    left.iter()
        .filter(|protocol| !matches!(protocol, Protocol::P2p(_)))
        .eq(right
            .iter()
            .filter(|protocol| !matches!(protocol, Protocol::P2p(_))))
}

impl MultiaddrExt for Multiaddr {
    fn canonical_ip_addr(&self) -> Option<IpAddr> {
        self.iter().find_map(|component| match component {
            libp2p::multiaddr::Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
            libp2p::multiaddr::Protocol::Ip6(ip) => Some(canonical_ip(IpAddr::V6(ip))),
            _ => None,
        })
    }
}
