use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use libp2p::{Multiaddr, PeerId};
use nockapp::{AtomExt, NockAppError};
use nockvm::noun::Noun;
use tracing::warn;

// The warn logs are specifically constructed for fail2ban
// Changing these breaks the integration with the fail2ban regex
pub fn log_fail2ban_ipv4(peer_id: &PeerId, ip: &Ipv4Addr) {
    warn!("fail2ban: Blocked peer {peer_id} with IPv4 address: {ip}");
}
pub fn log_fail2ban_ipv6(peer_id: &PeerId, ip: &Ipv6Addr) {
    warn!("fail2ban: Blocked peer {peer_id} with IPv6 address: {ip}");
}

pub trait PeerIdExt {
    fn from_noun(noun: Noun) -> Result<PeerId, NockAppError>;
}

impl PeerIdExt for PeerId {
    fn from_noun(noun: Noun) -> Result<PeerId, NockAppError> {
        let peer_id_bytes = noun.as_atom()?.to_bytes_until_nul()?;
        let peer_id_str = String::from_utf8(peer_id_bytes)?;
        PeerId::from_str(&peer_id_str)
            .map_err(|_| NockAppError::OtherError(String::from("Failed to parse PeerId from noun")))
    }
}

pub trait MultiaddrExt {
    fn ip_addr(&self) -> Option<IpAddr>;
}

impl MultiaddrExt for Multiaddr {
    fn ip_addr(&self) -> Option<IpAddr> {
        self.iter().find_map(|component| match component {
            libp2p::multiaddr::Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
            libp2p::multiaddr::Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
            _ => None,
        })
    }
}
