use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use tracing::{debug, warn};

use crate::bridge_status::BridgeStatus;
use crate::errors::BridgeError;
use crate::ingress::proto::bridge_ingress_client::BridgeIngressClient;
use crate::ingress::proto::HealthCheckRequest;
use crate::tui::types::AlertSeverity;
use crate::types::NodeConfig;

#[derive(Clone, Debug)]
pub struct PeerEndpoint {
    pub node_id: u64,
    pub address: String,
}

#[derive(Clone, Debug)]
pub enum NodeHealthStatus {
    Healthy,
    Unreachable { error: String },
}

#[derive(Clone, Debug)]
pub struct NodeHealthSnapshot {
    pub node_id: u64,
    pub address: String,
    pub status: NodeHealthStatus,
    pub latency_ms: Option<u128>,
    pub peer_uptime_ms: Option<u64>,
    pub last_updated: SystemTime,
}

pub type SharedHealthState = Arc<RwLock<Vec<NodeHealthSnapshot>>>;

#[derive(Clone)]
pub struct HealthMonitorConfig {
    pub self_node_id: u64,
    pub self_address: String,
    pub peers: Vec<PeerEndpoint>,
    pub poll_interval: Duration,
    pub request_timeout: Duration,
    pub bridge_status: Option<BridgeStatus>,
}

pub fn derive_peer_endpoints(config: &NodeConfig, self_node_id: u64) -> Vec<PeerEndpoint> {
    config
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            let node_id = idx as u64;
            if node_id == self_node_id {
                return None;
            }
            let address = normalize_endpoint(&node.ip);
            Some(PeerEndpoint { node_id, address })
        })
        .collect()
}

pub fn initialize_health_state(peers: &[PeerEndpoint]) -> SharedHealthState {
    let snapshots = peers
        .iter()
        .map(|peer| NodeHealthSnapshot {
            node_id: peer.node_id,
            address: peer.address.clone(),
            status: NodeHealthStatus::Unreachable {
                error: "pending".into(),
            },
            latency_ms: None,
            peer_uptime_ms: None,
            last_updated: SystemTime::now(),
        })
        .collect();
    Arc::new(RwLock::new(snapshots))
}

pub async fn run_health_monitor(
    cfg: HealthMonitorConfig,
    shared: SharedHealthState,
) -> Result<(), BridgeError> {
    if cfg.peers.is_empty() {
        return Ok(());
    }

    // Track consecutive failures per peer
    let mut failure_counts: HashMap<u64, u32> = HashMap::new();

    let mut interval = tokio::time::interval(cfg.poll_interval);
    loop {
        interval.tick().await;
        for peer in &cfg.peers {
            let reading = check_peer(peer, &cfg).await;

            // Get previous state before recording new reading
            let was_healthy = {
                if let Ok(guard) = shared.read() {
                    guard
                        .iter()
                        .find(|s| s.node_id == peer.node_id)
                        .map(|s| matches!(s.status, NodeHealthStatus::Healthy))
                        .unwrap_or(false)
                } else {
                    false
                }
            };

            record_reading(&shared, peer, reading.clone());

            // Handle alerts if TUI state is available
            if let Some(ref bridge_status) = cfg.bridge_status {
                match &reading.status {
                    NodeHealthStatus::Healthy => {
                        // Peer recovered
                        if !was_healthy {
                            let count = failure_counts.get(&peer.node_id).unwrap_or(&0);
                            if *count > 0 {
                                bridge_status.push_alert(
                                    AlertSeverity::Info,
                                    format!("Peer {} Recovered", peer.node_id),
                                    format!(
                                        "Peer {} ({}) is now healthy",
                                        peer.node_id, peer.address
                                    ),
                                    "health_monitor".to_string(),
                                );
                            }
                            failure_counts.insert(peer.node_id, 0);
                        }
                    }
                    NodeHealthStatus::Unreachable { error } => {
                        // Peer became unreachable or remains unreachable
                        let count = failure_counts.entry(peer.node_id).or_insert(0);
                        *count += 1;

                        if was_healthy {
                            // Just became unreachable - Warning
                            bridge_status.push_alert(
                                AlertSeverity::Warning,
                                format!("Peer {} Unreachable", peer.node_id),
                                format!("Peer {} ({}): {}", peer.node_id, peer.address, error),
                                "health_monitor".to_string(),
                            );
                        } else if *count >= 3 {
                            // Persistent failure - escalate to Error every 3rd consecutive failure
                            if (*count).is_multiple_of(3) {
                                bridge_status.push_alert(
                                    AlertSeverity::Error,
                                    format!("Peer {} Persistently Down", peer.node_id),
                                    format!(
                                        "Peer {} ({}) has failed {} consecutive health checks: {}",
                                        peer.node_id, peer.address, count, error
                                    ),
                                    "health_monitor".to_string(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PeerReading {
    status: NodeHealthStatus,
    latency_ms: Option<u128>,
    peer_uptime_ms: Option<u64>,
}

async fn check_peer(peer: &PeerEndpoint, cfg: &HealthMonitorConfig) -> PeerReading {
    let start = Instant::now();
    let mut client = match BridgeIngressClient::connect(peer.address.clone()).await {
        Ok(client) => client,
        Err(err) => {
            return PeerReading {
                status: NodeHealthStatus::Unreachable {
                    error: format!("connect failed: {}", err),
                },
                latency_ms: None,
                peer_uptime_ms: None,
            };
        }
    };
    let request = HealthCheckRequest {
        requester_node_id: cfg.self_node_id,
        requester_address: cfg.self_address.clone(),
    };
    let response = tokio::time::timeout(cfg.request_timeout, client.health_check(request)).await;

    match response {
        Ok(Ok(resp)) => {
            let latency = start.elapsed().as_millis();
            let body = resp.into_inner();
            debug!(
                target: "bridge.health",
                peer_id=peer.node_id,
                latency_ms=latency,
                uptime_ms=body.uptime_millis,
                "peer responded to health check"
            );
            PeerReading {
                status: NodeHealthStatus::Healthy,
                latency_ms: Some(latency),
                peer_uptime_ms: Some(body.uptime_millis),
            }
        }
        Ok(Err(status)) => {
            warn!(
                target: "bridge.health",
                peer_id=peer.node_id,
                error=%status,
                "health check RPC failed"
            );
            PeerReading {
                status: NodeHealthStatus::Unreachable {
                    error: status.to_string(),
                },
                latency_ms: None,
                peer_uptime_ms: None,
            }
        }
        Err(_) => {
            warn!(
                target: "bridge.health",
                peer_id=peer.node_id,
                "health check timed out"
            );
            PeerReading {
                status: NodeHealthStatus::Unreachable {
                    error: "timeout".into(),
                },
                latency_ms: None,
                peer_uptime_ms: None,
            }
        }
    }
}

fn record_reading(state: &SharedHealthState, peer: &PeerEndpoint, reading: PeerReading) {
    if let Ok(mut guard) = state.write() {
        if let Some(entry) = guard
            .iter_mut()
            .find(|snapshot| snapshot.node_id == peer.node_id)
        {
            entry.status = reading.status;
            entry.latency_ms = reading.latency_ms;
            entry.peer_uptime_ms = reading.peer_uptime_ms;
            entry.last_updated = SystemTime::now();
        } else {
            guard.push(NodeHealthSnapshot {
                node_id: peer.node_id,
                address: peer.address.clone(),
                status: reading.status,
                latency_ms: reading.latency_ms,
                peer_uptime_ms: reading.peer_uptime_ms,
                last_updated: SystemTime::now(),
            });
        }
    }
}

/// Normalize an endpoint address by adding http:// prefix if missing.
/// This is required for tonic gRPC clients which expect a full URI.
pub fn normalize_endpoint(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("http://{}", raw)
    }
}

/// Bridge operational status based on number of healthy nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationLevel {
    /// 5/5 or 4/5 healthy - full operational capacity
    Normal,
    /// 3/5 healthy - operational but degraded (at threshold)
    Degraded,
    /// <3/5 healthy - bridge halted (below multisig threshold)
    Critical,
}

/// Check the bridge's operational health level based on healthy peer count.
///
/// The bridge requires 3-of-5 signatures for multisig operations.
/// This function returns the degradation level based on how many nodes are healthy.
///
/// # Arguments
/// * `healthy_count` - Number of currently healthy nodes (including self)
/// * `total_nodes` - Total number of nodes in the bridge (typically 5)
///
/// # Returns
/// Degradation level indicating operational capacity
pub fn check_degradation(healthy_count: usize, total_nodes: usize) -> DegradationLevel {
    let threshold = 3; // 3-of-5 multisig threshold

    if healthy_count >= total_nodes.saturating_sub(1) {
        // 5/5 or 4/5 - full capacity
        DegradationLevel::Normal
    } else if healthy_count >= threshold {
        // 3/5 - at threshold, degraded but operational
        DegradationLevel::Degraded
    } else {
        // <3/5 - below threshold, cannot operate
        DegradationLevel::Critical
    }
}

/// Count healthy nodes from a health snapshot.
///
/// # Arguments
/// * `state` - Shared health state to check
/// * `include_self` - Whether to count self as healthy (typically true)
///
/// # Returns
/// Number of healthy nodes
pub fn count_healthy(state: &SharedHealthState, include_self: bool) -> usize {
    let mut count = if include_self { 1 } else { 0 };

    if let Ok(guard) = state.read() {
        count += guard
            .iter()
            .filter(|s| matches!(s.status, NodeHealthStatus::Healthy))
            .count();
    }

    count
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash as NockPkh;

    use super::*;
    use crate::types::{AtomBytes, NodeConfig, NodeInfo, SchnorrSecretKey};

    #[test]
    fn derive_peers_skips_self() {
        let config = NodeConfig {
            node_id: 1,
            nodes: vec![
                NodeInfo {
                    ip: "localhost:8001".into(),
                    eth_pubkey: AtomBytes(vec![]),
                    // Fake test PKH (valid format placeholder)
                    nock_pkh: NockPkh::from_base58(
                        "2222222222222222222222222222222222222222222222222222",
                    )
                    .expect("pkh"),
                },
                NodeInfo {
                    ip: "localhost:8002".into(),
                    eth_pubkey: AtomBytes(vec![]),
                    // Fake test PKH (valid format placeholder)
                    nock_pkh: NockPkh::from_base58(
                        "3333333333333333333333333333333333333333333333333333",
                    )
                    .expect("pkh"),
                },
            ],
            my_eth_key: AtomBytes(vec![]),
            my_nock_key: SchnorrSecretKey([Belt(0); 8]),
        };
        let peers = derive_peer_endpoints(&config, 1);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_id, 0);
        assert_eq!(peers[0].address, "http://localhost:8001");
    }

    #[test]
    fn test_check_degradation_normal() {
        // 5/5 - full capacity
        assert_eq!(check_degradation(5, 5), DegradationLevel::Normal);

        // 4/5 - one node down but still healthy
        assert_eq!(check_degradation(4, 5), DegradationLevel::Normal);
    }

    #[test]
    fn test_check_degradation_degraded() {
        // 3/5 - at threshold, degraded but operational
        assert_eq!(check_degradation(3, 5), DegradationLevel::Degraded);
    }

    #[test]
    fn test_check_degradation_critical() {
        // 2/5 - below threshold, cannot operate
        assert_eq!(check_degradation(2, 5), DegradationLevel::Critical);

        // 1/5 - severely degraded
        assert_eq!(check_degradation(1, 5), DegradationLevel::Critical);

        // 0/5 - all nodes down
        assert_eq!(check_degradation(0, 5), DegradationLevel::Critical);
    }

    #[test]
    fn test_count_healthy() {
        let peers = vec![
            PeerEndpoint {
                node_id: 0,
                address: "http://localhost:8001".into(),
            },
            PeerEndpoint {
                node_id: 2,
                address: "http://localhost:8002".into(),
            },
        ];
        let state = initialize_health_state(&peers);

        // Initially all unreachable, only self is healthy
        assert_eq!(count_healthy(&state, true), 1);
        assert_eq!(count_healthy(&state, false), 0);

        // Make one peer healthy
        {
            let mut guard = state.write().unwrap();
            guard[0].status = NodeHealthStatus::Healthy;
        }

        assert_eq!(count_healthy(&state, true), 2);
        assert_eq!(count_healthy(&state, false), 1);

        // Make both peers healthy
        {
            let mut guard = state.write().unwrap();
            guard[1].status = NodeHealthStatus::Healthy;
        }

        assert_eq!(count_healthy(&state, true), 3);
        assert_eq!(count_healthy(&state, false), 2);
    }
}
