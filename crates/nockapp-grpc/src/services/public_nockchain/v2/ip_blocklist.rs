//! IP blocklist enforced at the public gRPC API edge.
//!
//! Public API requests reach us behind a load balancer which sets the
//! `x-forwarded-for` header in the form `RECENT_IP,LOAD_BALANCER_IP`, where
//! `RECENT_IP` is the real client. This module rejects a request when that
//! client IP is on the blocklist (the load balancer's own IP is ignored).
//!
//! The blocklist is the union of:
//!   * a compiled-in default set (so the list survives a missing env var), and
//!   * any addresses supplied via the `NOCKCHAIN_API_IP_BLOCKLIST` environment
//!     variable (comma- or whitespace-separated).
//!
//! To change the blocklist operationally, set/extend `NOCKCHAIN_API_IP_BLOCKLIST`
//! and restart the API. It is read once at server startup.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tonic::service::InterceptorLayer;
use tonic::{Request, Status};
use tracing::{info, warn};

use super::metrics::NockchainGrpcApiMetrics;

/// Environment variable holding additional blocked IPs (comma/whitespace separated).
pub const BLOCKLIST_ENV_VAR: &str = "NOCKCHAIN_API_IP_BLOCKLIST";

/// Always-blocked addresses, compiled in so the list survives a missing env var.
const DEFAULT_BLOCKED_IPS: &[&str] = &["90.65.165.234"];

/// A set of client IPs that are denied access to the public gRPC API.
///
/// Cheap to clone (the underlying set is shared via `Arc`).
#[derive(Clone)]
pub struct IpBlocklist {
    blocked: Arc<HashSet<IpAddr>>,
}

impl IpBlocklist {
    /// Build the blocklist from the compiled-in defaults plus
    /// `NOCKCHAIN_API_IP_BLOCKLIST`. Unparseable entries are logged and skipped.
    pub fn from_env_and_defaults() -> Self {
        let mut blocked = HashSet::new();

        for ip in DEFAULT_BLOCKED_IPS {
            match ip.parse::<IpAddr>() {
                Ok(addr) => {
                    blocked.insert(addr);
                }
                Err(e) => warn!("Invalid compiled-in blocked IP {ip:?}: {e}"),
            }
        }

        if let Ok(raw) = std::env::var(BLOCKLIST_ENV_VAR) {
            for token in raw.split([',', ' ', '\t', '\n', '\r']) {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                match parse_ip(token) {
                    Some(addr) => {
                        blocked.insert(addr);
                    }
                    None => {
                        warn!("Ignoring unparseable entry {token:?} in {BLOCKLIST_ENV_VAR}")
                    }
                }
            }
        }

        if blocked.is_empty() {
            info!("API IP blocklist is empty");
        } else {
            info!("API IP blocklist active with {} address(es)", blocked.len());
        }

        Self {
            blocked: Arc::new(blocked),
        }
    }

    #[cfg(test)]
    pub fn from_ips(ips: impl IntoIterator<Item = IpAddr>) -> Self {
        Self {
            blocked: Arc::new(ips.into_iter().collect()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }

    /// Checks the client IP in an `x-forwarded-for` header value.
    ///
    /// Our load balancer emits exactly `RECENT_IP,LOAD_BALANCER_IP`, where
    /// `RECENT_IP` (the leftmost entry) is the real client. We therefore match
    /// only the first token and ignore the load balancer's own address.
    /// Returns the client IP if it is blocked.
    pub fn blocked_in_xff(&self, xff: &str) -> Option<IpAddr> {
        let recent_ip = parse_ip(xff.split(',').next()?.trim())?;
        self.blocked.contains(&recent_ip).then_some(recent_ip)
    }
}

/// Parse an IP from an XFF token, tolerating an optional `:port`,
/// `[v6]:port`, or bracketed `[v6]` form that some proxies emit.
fn parse_ip(token: &str) -> Option<IpAddr> {
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Ok(sa) = token.parse::<SocketAddr>() {
        return Some(sa.ip());
    }
    let unbracketed = token.trim_start_matches('[').trim_end_matches(']');
    unbracketed.parse::<IpAddr>().ok()
}

/// A tonic layer that rejects requests whose `x-forwarded-for` header contains
/// a blocked IP, before they reach any service handler.
///
/// Applied server-wide via `Server::builder().layer(..)`, so it also guards the
/// health, reflection and block-explorer/metrics services. Requests with no
/// `x-forwarded-for` (e.g. proxy/LB health probes) are passed through.
pub fn blocklist_layer(
    blocklist: IpBlocklist,
    metrics: Arc<NockchainGrpcApiMetrics>,
) -> InterceptorLayer<impl FnMut(Request<()>) -> Result<Request<()>, Status> + Clone> {
    InterceptorLayer::new(move |req: Request<()>| {
        if let Some(xff) = req
            .metadata()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(blocked) = blocklist.blocked_in_xff(xff) {
                metrics.api_request_blocked.increment();
                warn!("Rejecting blocked client IP {blocked} (x-forwarded-for: {xff:?})");
                return Err(Status::permission_denied("client IP is blocked"));
            }
        }
        Ok(req)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn default_bad_ip_is_blocked() {
        let bl = IpBlocklist::from_env_and_defaults();
        assert_eq!(
            bl.blocked_in_xff("90.65.165.234"),
            Some(ip("90.65.165.234"))
        );
    }

    #[test]
    fn matches_recent_ip_only() {
        let bl = IpBlocklist::from_ips([ip("90.65.165.234")]);
        // Real format: RECENT_IP,LOAD_BALANCER_IP — RECENT_IP is blocked.
        assert_eq!(
            bl.blocked_in_xff("90.65.165.234,10.0.0.1"),
            Some(ip("90.65.165.234"))
        );
    }

    #[test]
    fn ignores_load_balancer_ip() {
        // The blocked IP only appears as the load balancer entry; the real
        // client (RECENT_IP) is clean, so the request must pass.
        let bl = IpBlocklist::from_ips([ip("10.0.0.1")]);
        assert_eq!(bl.blocked_in_xff("90.65.165.234,10.0.0.1"), None);
    }

    #[test]
    fn clean_recent_ip_passes() {
        let bl = IpBlocklist::from_ips([ip("90.65.165.234")]);
        assert_eq!(bl.blocked_in_xff("1.2.3.4,10.0.0.1"), None);
    }

    #[test]
    fn tolerates_port_and_brackets() {
        let bl = IpBlocklist::from_ips([ip("90.65.165.234"), ip("2001:db8::1")]);
        assert_eq!(
            bl.blocked_in_xff("90.65.165.234:54321"),
            Some(ip("90.65.165.234"))
        );
        assert_eq!(bl.blocked_in_xff("[2001:db8::1]"), Some(ip("2001:db8::1")));
    }
}
