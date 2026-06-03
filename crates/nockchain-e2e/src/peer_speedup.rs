use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::process::Command;
use tokio::time::sleep;

const GET_PEER_STATS_METHOD: &str = "nockchain.public.v2.NockchainMetricsService/GetPeerStats";

const PEER_ARRAY_ALIASES: &[&str] = &["peers", "peerStats", "peer_stats", "stats", "items"];
const PEER_ID_ALIASES: &[&str] = &["peerId", "peer_id", "peer", "peerInfo", "peer_info"];
const GENERATION_ALIASES: &[&str] = &[
    "protocolGeneration", "protocol_generation", "generation", "reqResGeneration",
    "req_res_generation",
];
const TOTAL_BYTES_ALIASES: &[&str] = &[
    "bytesTransferred", "bytes_transferred", "totalBytesTransferred", "total_bytes_transferred",
    "totalBytes", "total_bytes",
];
const BYTES_SENT_ALIASES: &[&str] = &["bytesSent", "bytes_sent", "outboundBytes", "outbound_bytes"];
const BYTES_RECEIVED_ALIASES: &[&str] =
    &["bytesReceived", "bytes_received", "inboundBytes", "inbound_bytes"];
const REQUEST_COUNT_ALIASES: &[&str] =
    &["requestCount", "request_count", "requests", "totalRequests", "total_requests"];
const REQUESTS_SENT_ALIASES: &[&str] =
    &["requestsSent", "requests_sent", "outboundRequests", "outbound_requests"];
const REQUESTS_RECEIVED_ALIASES: &[&str] =
    &["requestsReceived", "requests_received", "inboundRequests", "inbound_requests"];
const RTT_ALIASES: &[&str] = &[
    "avgRoundTripMs", "avg_round_trip_ms", "averageRoundTripMs", "average_round_trip_ms",
    "avgLatencyMs", "avg_latency_ms", "latencyMs", "latency_ms", "roundTripMs", "round_trip_ms",
];

#[derive(Debug, Clone)]
pub struct AssertPeerSpeedupOptions {
    pub servers: Vec<String>,
    pub sample_interval: Duration,
    pub min_speedup_ratio: f64,
    pub json_out: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssertPeerSpeedupReport {
    pub passed: bool,
    pub method: &'static str,
    pub sample_interval_ms: u64,
    pub min_speedup_ratio: f64,
    pub primary_metric: String,
    pub primary_ratio: f64,
    pub request_ratio: Option<f64>,
    pub latency_ratio: Option<f64>,
    pub gen1: PeerGenerationAggregate,
    pub gen2: PeerGenerationAggregate,
    pub raw_samples: RawPeerSamples,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PeerGenerationAggregate {
    pub seen_peer_count: usize,
    pub active_peer_count: usize,
    pub delta_bytes: u64,
    pub delta_requests: u64,
    pub throughput_bytes_per_sec: f64,
    pub throughput_requests_per_sec: f64,
    pub avg_rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct PeerCounters {
    server: String,
    peer_id: String,
    generation: Generation,
    total_bytes: Option<u64>,
    request_count: Option<u64>,
    avg_rtt_ms: Option<f64>,
}

/// Raw before/after peer-stats snapshots preserved for post-hoc review.
#[derive(Debug, Clone, Serialize)]
pub struct RawPeerSamples {
    pub before: Vec<PeerSample>,
    pub after: Vec<PeerSample>,
}

/// A single peer observation from a `GetPeerStats` snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct PeerSample {
    pub server: String,
    pub peer_id: String,
    pub generation: String,
    pub total_bytes: Option<u64>,
    pub request_count: Option<u64>,
    pub avg_rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
enum Generation {
    Gen1,
    Gen2,
}

impl PeerCounters {
    fn to_sample(&self) -> PeerSample {
        PeerSample {
            server: self.server.clone(),
            peer_id: self.peer_id.clone(),
            generation: format!("{:?}", self.generation),
            total_bytes: self.total_bytes,
            request_count: self.request_count,
            avg_rtt_ms: self.avg_rtt_ms,
        }
    }
}

fn counters_to_samples(counters: &[PeerCounters]) -> Vec<PeerSample> {
    counters.iter().map(PeerCounters::to_sample).collect()
}

#[derive(Default)]
struct GenerationAccumulator {
    seen_peer_count: usize,
    active_peer_count: usize,
    delta_bytes: u64,
    delta_requests: u64,
    rtt_weighted_sum: f64,
    rtt_weight_total: f64,
}

impl GenerationAccumulator {
    fn add_seen(&mut self) {
        self.seen_peer_count += 1;
    }

    fn add_activity(
        &mut self,
        delta_bytes: Option<u64>,
        delta_requests: Option<u64>,
        avg_rtt_ms: Option<f64>,
    ) {
        let delta_bytes = delta_bytes.unwrap_or(0);
        let delta_requests = delta_requests.unwrap_or(0);
        if delta_bytes == 0 && delta_requests == 0 {
            return;
        }

        self.active_peer_count += 1;
        self.delta_bytes = self.delta_bytes.saturating_add(delta_bytes);
        self.delta_requests = self.delta_requests.saturating_add(delta_requests);

        let weight = if delta_requests > 0 {
            delta_requests as f64
        } else if delta_bytes > 0 {
            1.0
        } else {
            0.0
        };
        if let Some(avg_rtt_ms) = avg_rtt_ms {
            if weight > 0.0 {
                self.rtt_weighted_sum += avg_rtt_ms * weight;
                self.rtt_weight_total += weight;
            }
        }
    }

    fn finish(self, interval: Duration) -> PeerGenerationAggregate {
        let interval_secs = interval.as_secs_f64();
        PeerGenerationAggregate {
            seen_peer_count: self.seen_peer_count,
            active_peer_count: self.active_peer_count,
            delta_bytes: self.delta_bytes,
            delta_requests: self.delta_requests,
            throughput_bytes_per_sec: if interval_secs > 0.0 {
                self.delta_bytes as f64 / interval_secs
            } else {
                0.0
            },
            throughput_requests_per_sec: if interval_secs > 0.0 {
                self.delta_requests as f64 / interval_secs
            } else {
                0.0
            },
            avg_rtt_ms: if self.rtt_weight_total > 0.0 {
                Some(self.rtt_weighted_sum / self.rtt_weight_total)
            } else {
                None
            },
        }
    }
}

pub async fn assert_peer_speedup(
    options: AssertPeerSpeedupOptions,
) -> Result<AssertPeerSpeedupReport> {
    if options.servers.is_empty() {
        bail!("at least one --server is required");
    }
    if options.sample_interval.is_zero() {
        bail!("--sample-interval-ms must be greater than zero");
    }

    let first = capture_peer_snapshots(&options.servers).await?;
    sleep(options.sample_interval).await;
    let second = capture_peer_snapshots(&options.servers).await?;

    let raw_samples = RawPeerSamples {
        before: counters_to_samples(&first),
        after: counters_to_samples(&second),
    };

    let report = build_report(
        &first, &second, options.sample_interval, options.min_speedup_ratio, raw_samples,
    )?;

    if let Some(path) = &options.json_out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }

    println!("{}", serde_json::to_string_pretty(&report)?);

    if !report.passed {
        bail!(
            "gen2 speedup assertion failed: {} ratio {:.3}x below required {:.3}x",
            report.primary_metric, report.primary_ratio, report.min_speedup_ratio
        );
    }

    Ok(report)
}

async fn capture_peer_snapshots(servers: &[String]) -> Result<Vec<PeerCounters>> {
    let mut snapshots = Vec::new();
    for server in servers {
        let stdout = grpcurl(server, GET_PEER_STATS_METHOD, &json!({}))
            .await
            .with_context(|| format!("failed to fetch peer stats from {}", server))?;
        let parsed: Value = serde_json::from_str(&stdout)
            .with_context(|| format!("failed to parse peer stats JSON from {}", server))?;
        let mut peer_stats = parse_peer_counters(&parsed, server)?;
        snapshots.append(&mut peer_stats);
    }
    Ok(snapshots)
}

async fn grpcurl(server: &str, method: &str, body: &Value) -> Result<String> {
    let output = Command::new("grpcurl")
        .arg("-plaintext")
        .arg("-d")
        .arg(serde_json::to_string(body)?)
        .arg(server)
        .arg(method)
        .output()
        .await
        .context("failed to spawn grpcurl; install grpcurl or run inside nix develop")?;

    if !output.status.success() {
        return Err(anyhow!(
            "grpcurl {} on {} failed: {}",
            method,
            server,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    String::from_utf8(output.stdout).context("grpcurl returned invalid UTF-8")
}

fn parse_peer_counters(doc: &Value, server: &str) -> Result<Vec<PeerCounters>> {
    let peer_entries = peer_entries(doc).ok_or_else(|| {
        anyhow!("peer stats response from {} did not contain a peer array", server)
    })?;

    let mut peers = Vec::with_capacity(peer_entries.len());
    for entry in peer_entries {
        let Value::Object(obj) = entry else {
            continue;
        };

        let Some(peer_id) = extract_peer_id(obj) else {
            continue;
        };
        let Some(generation) = extract_generation(obj) else {
            continue;
        };

        peers.push(PeerCounters {
            server: server.to_string(),
            peer_id,
            generation,
            total_bytes: extract_total_bytes(obj),
            request_count: extract_request_count(obj),
            avg_rtt_ms: extract_f64_alias(obj, RTT_ALIASES),
        });
    }

    if peers.is_empty() {
        bail!("peer stats response from {} had no parseable peers", server);
    }

    Ok(peers)
}

fn peer_entries(doc: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = doc.as_array() {
        return Some(array);
    }
    let obj = doc.as_object()?;
    for alias in PEER_ARRAY_ALIASES {
        if let Some(value) = obj.get(*alias) {
            if let Some(array) = value.as_array() {
                return Some(array);
            }
            if let Some(array) = peer_entries(value) {
                return Some(array);
            }
        }
    }
    None
}

fn build_report(
    first: &[PeerCounters],
    second: &[PeerCounters],
    sample_interval: Duration,
    min_speedup_ratio: f64,
    raw_samples: RawPeerSamples,
) -> Result<AssertPeerSpeedupReport> {
    let mut first_map = HashMap::new();
    for counters in first {
        first_map.insert(
            (counters.server.clone(), counters.peer_id.clone()),
            counters.clone(),
        );
    }

    let mut second_map = HashMap::new();
    for counters in second {
        second_map.insert(
            (counters.server.clone(), counters.peer_id.clone()),
            counters.clone(),
        );
    }

    let mut gen1 = GenerationAccumulator::default();
    let mut gen2 = GenerationAccumulator::default();

    for counters in second_map.values() {
        match counters.generation {
            Generation::Gen1 => gen1.add_seen(),
            Generation::Gen2 => gen2.add_seen(),
        }
    }

    for (key, second_counters) in &second_map {
        let Some(first_counters) = first_map.get(key) else {
            continue;
        };
        let delta_bytes = compute_delta(first_counters.total_bytes, second_counters.total_bytes);
        let delta_requests =
            compute_delta(first_counters.request_count, second_counters.request_count);
        match second_counters.generation {
            Generation::Gen1 => {
                gen1.add_activity(delta_bytes, delta_requests, second_counters.avg_rtt_ms)
            }
            Generation::Gen2 => {
                gen2.add_activity(delta_bytes, delta_requests, second_counters.avg_rtt_ms)
            }
        }
    }

    let gen1 = gen1.finish(sample_interval);
    let gen2 = gen2.finish(sample_interval);

    if gen1.active_peer_count == 0 || gen2.active_peer_count == 0 {
        bail!(
            "did not observe active traffic for both generations (gen1 active peers: {}, gen2 active peers: {})",
            gen1.active_peer_count,
            gen2.active_peer_count
        );
    }

    let bytes_ratio = ratio(gen2.throughput_bytes_per_sec, gen1.throughput_bytes_per_sec);
    let request_ratio = ratio(
        gen2.throughput_requests_per_sec, gen1.throughput_requests_per_sec,
    );
    let latency_ratio = match (gen1.avg_rtt_ms, gen2.avg_rtt_ms) {
        (Some(gen1_rtt), Some(gen2_rtt)) if gen2_rtt > 0.0 => Some(gen1_rtt / gen2_rtt),
        _ => None,
    };

    let speedup_passes = bytes_ratio
        .map(|ratio| ratio >= min_speedup_ratio)
        .unwrap_or(false)
        || request_ratio
            .map(|ratio| ratio >= min_speedup_ratio)
            .unwrap_or(false);

    let (primary_metric, primary_ratio) = if bytes_ratio
        .map(|ratio| ratio >= min_speedup_ratio)
        .unwrap_or(false)
    {
        ("bytes_per_sec".to_string(), bytes_ratio.unwrap_or_default())
    } else if request_ratio
        .map(|ratio| ratio >= min_speedup_ratio)
        .unwrap_or(false)
    {
        (
            "requests_per_sec".to_string(),
            request_ratio.unwrap_or_default(),
        )
    } else if let Some(ratio) = bytes_ratio {
        ("bytes_per_sec".to_string(), ratio)
    } else if let Some(ratio) = request_ratio {
        ("requests_per_sec".to_string(), ratio)
    } else {
        bail!("peer stats did not expose bytes or request counters for both generations");
    };

    Ok(AssertPeerSpeedupReport {
        passed: speedup_passes,
        method: GET_PEER_STATS_METHOD,
        sample_interval_ms: sample_interval.as_millis() as u64,
        min_speedup_ratio,
        primary_metric,
        primary_ratio,
        request_ratio,
        latency_ratio,
        gen1,
        gen2,
        raw_samples,
    })
}

fn compute_delta(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    match (before, after) {
        (Some(before), Some(after)) if after >= before => Some(after - before),
        _ => None,
    }
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    if denominator > 0.0 {
        Some(numerator / denominator)
    } else {
        None
    }
}

fn extract_peer_id(obj: &Map<String, Value>) -> Option<String> {
    for alias in PEER_ID_ALIASES {
        if let Some(value) = obj.get(*alias).and_then(stringish) {
            return Some(value);
        }
    }
    None
}

fn extract_generation(obj: &Map<String, Value>) -> Option<Generation> {
    for alias in GENERATION_ALIASES {
        if let Some(value) = obj.get(*alias) {
            if let Some(generation) = generation_from_value(value) {
                return Some(generation);
            }
        }
    }
    None
}

fn extract_total_bytes(obj: &Map<String, Value>) -> Option<u64> {
    if let Some(total) = extract_u64_alias(obj, TOTAL_BYTES_ALIASES) {
        return Some(total);
    }

    let sent = extract_u64_alias(obj, BYTES_SENT_ALIASES);
    let received = extract_u64_alias(obj, BYTES_RECEIVED_ALIASES);
    match (sent, received) {
        (Some(sent), Some(received)) => Some(sent.saturating_add(received)),
        (Some(bytes), None) | (None, Some(bytes)) => Some(bytes),
        (None, None) => None,
    }
}

fn extract_request_count(obj: &Map<String, Value>) -> Option<u64> {
    if let Some(total) = extract_u64_alias(obj, REQUEST_COUNT_ALIASES) {
        return Some(total);
    }

    let sent = extract_u64_alias(obj, REQUESTS_SENT_ALIASES);
    let received = extract_u64_alias(obj, REQUESTS_RECEIVED_ALIASES);
    match (sent, received) {
        (Some(sent), Some(received)) => Some(sent.saturating_add(received)),
        (Some(count), None) | (None, Some(count)) => Some(count),
        (None, None) => None,
    }
}

fn extract_u64_alias(obj: &Map<String, Value>, aliases: &[&str]) -> Option<u64> {
    aliases
        .iter()
        .find_map(|alias| obj.get(*alias).and_then(u64ish))
}

fn extract_f64_alias(obj: &Map<String, Value>, aliases: &[&str]) -> Option<f64> {
    aliases
        .iter()
        .find_map(|alias| obj.get(*alias).and_then(f64ish))
}

fn generation_from_value(value: &Value) -> Option<Generation> {
    let string = stringish(value)?.to_ascii_lowercase();
    if string.contains("gen2") || string.ends_with('2') {
        return Some(Generation::Gen2);
    }
    if string.contains("gen1") || string.ends_with('1') {
        return Some(Generation::Gen1);
    }
    None
}

fn stringish(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(obj) => {
            for alias in ["value", "id", "peerId", "peer_id", "hash"] {
                if let Some(value) = obj.get(alias).and_then(stringish) {
                    return Some(value);
                }
            }
            None
        }
        _ => None,
    }
}

fn u64ish(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok())),
        Value::String(value) => value.parse::<u64>().ok(),
        Value::Object(obj) => {
            for alias in ["value", "count", "total"] {
                if let Some(value) = obj.get(alias).and_then(u64ish) {
                    return Some(value);
                }
            }
            None
        }
        _ => None,
    }
}

fn f64ish(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        Value::Object(obj) => {
            for alias in ["value", "avgMs", "avg_ms"] {
                if let Some(value) = obj.get(alias).and_then(f64ish) {
                    return Some(value);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(
        server: &str,
        peer_id: &str,
        generation: Generation,
        total_bytes: Option<u64>,
        request_count: Option<u64>,
        avg_rtt_ms: Option<f64>,
    ) -> PeerCounters {
        PeerCounters {
            server: server.to_string(),
            peer_id: peer_id.to_string(),
            generation,
            total_bytes,
            request_count,
            avg_rtt_ms,
        }
    }

    #[test]
    fn parse_peer_counters_accepts_common_field_aliases() {
        let doc = json!({
            "peerStats": [
                {
                    "peerId": "peer-gen1",
                    "protocolGeneration": "Gen1",
                    "bytesSent": "100",
                    "bytesReceived": 40,
                    "requestCount": "7",
                    "avgRoundTripMs": 12.5
                },
                {
                    "peer_id": { "value": "peer-gen2" },
                    "generation": "gen2",
                    "total_bytes_transferred": 900,
                    "requests": 21,
                    "avg_latency_ms": "4.0"
                }
            ]
        });

        let parsed = parse_peer_counters(&doc, "127.0.0.1:6303").expect("peer stats should parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].peer_id, "peer-gen1");
        assert_eq!(parsed[0].generation, Generation::Gen1);
        assert_eq!(parsed[0].total_bytes, Some(140));
        assert_eq!(parsed[0].request_count, Some(7));
        assert_eq!(parsed[0].avg_rtt_ms, Some(12.5));
        assert_eq!(parsed[1].peer_id, "peer-gen2");
        assert_eq!(parsed[1].generation, Generation::Gen2);
        assert_eq!(parsed[1].total_bytes, Some(900));
        assert_eq!(parsed[1].request_count, Some(21));
        assert_eq!(parsed[1].avg_rtt_ms, Some(4.0));
    }

    #[test]
    fn parse_peer_counters_accepts_nested_grpc_stats_shape() {
        let doc = json!({
            "stats": {
                "collectedAtUnixMs": "42",
                "peers": [
                    {
                        "peerId": "peer-gen2",
                        "protocolGeneration": "PEER_REQ_RES_GENERATION_GEN2",
                        "bytesSent": 10,
                        "bytesReceived": 25,
                        "requestCount": 3,
                        "averageRoundTripMs": 6.5
                    }
                ]
            }
        });

        let parsed =
            parse_peer_counters(&doc, "127.0.0.1:6304").expect("nested peer stats should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].peer_id, "peer-gen2");
        assert_eq!(parsed[0].generation, Generation::Gen2);
        assert_eq!(parsed[0].total_bytes, Some(35));
        assert_eq!(parsed[0].request_count, Some(3));
        assert_eq!(parsed[0].avg_rtt_ms, Some(6.5));
    }

    #[test]
    fn build_report_passes_when_gen2_bytes_are_faster() {
        let first = vec![
            peer(
                "full-a",
                "miner-a",
                Generation::Gen1,
                Some(1_000),
                Some(10),
                Some(18.0),
            ),
            peer(
                "full-a",
                "full-b",
                Generation::Gen2,
                Some(1_000),
                Some(10),
                Some(6.0),
            ),
        ];
        let second = vec![
            peer(
                "full-a",
                "miner-a",
                Generation::Gen1,
                Some(1_600),
                Some(16),
                Some(20.0),
            ),
            peer(
                "full-a",
                "full-b",
                Generation::Gen2,
                Some(3_400),
                Some(32),
                Some(5.0),
            ),
        ];

        let raw_samples = RawPeerSamples {
            before: counters_to_samples(&first),
            after: counters_to_samples(&second),
        };
        let report = build_report(&first, &second, Duration::from_secs(4), 1.2, raw_samples)
            .expect("report should build");
        assert!(report.passed);
        assert_eq!(report.primary_metric, "bytes_per_sec");
        assert!(report.primary_ratio > 3.0);
        assert_eq!(report.gen1.active_peer_count, 1);
        assert_eq!(report.gen2.active_peer_count, 1);
        assert_eq!(report.raw_samples.before.len(), 2);
        assert_eq!(report.raw_samples.after.len(), 2);
    }

    #[test]
    fn build_report_falls_back_to_request_rate() {
        let first = vec![
            peer(
                "full-a",
                "miner-a",
                Generation::Gen1,
                None,
                Some(10),
                Some(14.0),
            ),
            peer(
                "full-a",
                "full-b",
                Generation::Gen2,
                None,
                Some(10),
                Some(7.0),
            ),
        ];
        let second = vec![
            peer(
                "full-a",
                "miner-a",
                Generation::Gen1,
                None,
                Some(14),
                Some(14.0),
            ),
            peer(
                "full-a",
                "full-b",
                Generation::Gen2,
                None,
                Some(24),
                Some(7.0),
            ),
        ];

        let raw_samples = RawPeerSamples {
            before: counters_to_samples(&first),
            after: counters_to_samples(&second),
        };
        let report = build_report(&first, &second, Duration::from_secs(2), 1.5, raw_samples)
            .expect("report should build");
        assert!(report.passed);
        assert_eq!(report.primary_metric, "requests_per_sec");
        assert!(report.primary_ratio >= 2.0);
    }

    #[test]
    fn parse_peer_counters_skips_unknown_generation_entries() {
        let doc = json!({
            "peerStats": [
                {
                    "peerId": "peer-unknown",
                    "protocolGeneration": "unknown",
                    "requestCount": 99
                },
                {
                    "peerId": "peer-gen2",
                    "protocolGeneration": "Gen2",
                    "bytesSent": 10,
                    "bytesReceived": 5,
                    "requestCount": 3
                }
            ]
        });

        let parsed = parse_peer_counters(&doc, "127.0.0.1:6303").expect("peer stats should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].peer_id, "peer-gen2");
        assert_eq!(parsed[0].generation, Generation::Gen2);
    }

    #[test]
    fn report_json_contains_raw_samples() {
        let first = vec![
            peer("s1", "p1", Generation::Gen1, Some(100), Some(5), Some(10.0)),
            peer("s1", "p2", Generation::Gen2, Some(200), Some(8), Some(4.0)),
        ];
        let second = vec![
            peer(
                "s1",
                "p1",
                Generation::Gen1,
                Some(300),
                Some(10),
                Some(11.0),
            ),
            peer("s1", "p2", Generation::Gen2, Some(800), Some(24), Some(3.5)),
        ];
        let raw_samples = RawPeerSamples {
            before: counters_to_samples(&first),
            after: counters_to_samples(&second),
        };
        let report = build_report(&first, &second, Duration::from_secs(2), 1.0, raw_samples)
            .expect("report should build");

        let json: Value = serde_json::to_value(&report).expect("report should serialize");
        let samples = json
            .get("raw_samples")
            .expect("raw_samples field must exist");
        let before = samples
            .get("before")
            .and_then(|v| v.as_array())
            .expect("before array");
        let after = samples
            .get("after")
            .and_then(|v| v.as_array())
            .expect("after array");
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        assert_eq!(before[0]["peer_id"], "p1");
        assert_eq!(before[0]["generation"], "Gen1");
        assert_eq!(after[1]["peer_id"], "p2");
        assert_eq!(after[1]["generation"], "Gen2");
        assert_eq!(after[1]["total_bytes"], 800);
    }
}
