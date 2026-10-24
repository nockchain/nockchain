use std::sync::{Arc, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PeerReqResGeneration {
    #[default]
    Unknown,
    Gen1,
    Gen2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeerStatsEntry {
    pub peer_id: String,
    /// Best-known remote req-res capability for dashboard grouping.
    /// This remains `Unknown` until Identify or unsupported-protocol fallback
    /// settles the peer's generation; per-request traffic counters must not
    /// overwrite it with provisional send-path guesses.
    pub protocol_generation: PeerReqResGeneration,
    pub request_count: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub average_round_trip_ms: f64,
    pub average_batch_size: f64,
    pub failure_count: u64,
    pub timeout_count: u64,
    pub blocks_received: u64,
    pub average_block_propagation_ms: f64,
    pub connection_duration_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeerStatsSnapshot {
    pub collected_at_unix_ms: u64,
    pub peers: Vec<PeerStatsEntry>,
}

#[derive(Debug, Default)]
pub struct PeerStatsRegistry {
    snapshot: RwLock<PeerStatsSnapshot>,
}

impl PeerStatsRegistry {
    pub fn replace_snapshot(&self, snapshot: PeerStatsSnapshot) {
        match self.snapshot.write() {
            Ok(mut guard) => *guard = snapshot,
            Err(poisoned) => *poisoned.into_inner() = snapshot,
        }
    }

    pub fn snapshot(&self) -> PeerStatsSnapshot {
        match self.snapshot.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

pub fn global_peer_stats_registry() -> Arc<PeerStatsRegistry> {
    static REGISTRY: OnceLock<Arc<PeerStatsRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(PeerStatsRegistry::default()))
        .clone()
}

pub fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
