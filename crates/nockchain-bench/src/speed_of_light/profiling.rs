//! Memory profiling primitives for speed-of-light benchmarks.
//!
//! This module provides:
//! - Process-level memory sampling timeline
//! - Phase window summaries (init/replay/checkpoint/gc)
//! - Inferred GC events from RSS drops
//! - Page-fault burst detection
//! - Candidate scorecard metrics

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::sampler::buckets::{sample_process, AttributionConfig, MemoryAttribution};
use crate::sampler::smaps::{SmapsError, SmapsParser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhaseKind {
    Init,
    Replay,
    Checkpoint,
    Gc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseWindow {
    pub kind: PhaseKind,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl PhaseWindow {
    pub fn new(kind: PhaseKind, start_ms: u64, end_ms: u64) -> Self {
        Self {
            kind,
            start_ms,
            end_ms: end_ms.max(start_ms),
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseSummary {
    pub kind: PhaseKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
    pub sample_count: u64,
    pub peak_rss_bytes: u64,
    pub avg_rss_bytes: u64,
    pub peak_vm_size_bytes: u64,
    pub avg_vm_size_bytes: u64,
    pub minor_faults_delta: u64,
    pub major_faults_delta: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcEvent {
    /// True when inferred from memory timeline (not explicit runtime instrumentation).
    pub inferred: bool,
    pub start_ms: u64,
    pub end_ms: u64,
    pub pause_ms: u64,
    pub reclaimed_bytes: u64,
    pub live_set_rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageFaultBurst {
    pub start_ms: u64,
    pub end_ms: u64,
    pub minor_faults_delta: u64,
    pub major_faults_delta: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointProfile {
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
    pub pre_checkpoint_rss_bytes: u64,
    pub post_checkpoint_rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub recovery_ms: Option<u64>,
    pub checkpoint_size_bytes: Option<u64>,
}

impl CheckpointProfile {
    pub fn throughput_mib_per_s(&self) -> Option<f64> {
        let size = self.checkpoint_size_bytes?;
        if self.duration_ms == 0 {
            return None;
        }
        let seconds = self.duration_ms as f64 / 1000.0;
        if seconds <= 0.0 {
            return None;
        }
        Some((size as f64 / 1024.0 / 1024.0) / seconds)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolScorecard {
    pub peak_rss_mib: f64,
    pub p95_rss_mib: f64,
    pub checkpoint_peak_rss_mib: Option<f64>,
    pub checkpoint_seconds_per_gib: Option<f64>,
    pub gc_pause_p95_ms: Option<f64>,
    pub gc_events_per_1k_blocks: f64,
    pub page_fault_burst_count: u64,
    pub blocks_per_second: f64,
    pub failed_pokes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryProfile {
    pub interval_ms: u64,
    pub samples: Vec<MemoryAttribution>,
    pub phase_windows: Vec<PhaseWindow>,
    pub phase_summaries: Vec<PhaseSummary>,
    pub checkpoint_profiles: Vec<CheckpointProfile>,
    pub gc_events: Vec<GcEvent>,
    pub page_fault_bursts: Vec<PageFaultBurst>,
    pub scorecard: SolScorecard,
}

pub struct ProcessMemoryProfiler {
    pid: i32,
    interval_ms: u64,
    attribution: AttributionConfig,
    samples: Vec<MemoryAttribution>,
    last_sample_ms: Option<u64>,
}

impl ProcessMemoryProfiler {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            pid: std::process::id() as i32,
            interval_ms: interval_ms.max(1),
            attribution: AttributionConfig::default(),
            samples: Vec::new(),
            last_sample_ms: None,
        }
    }

    pub fn with_attribution(mut self, attribution: AttributionConfig) -> Self {
        self.attribution = attribution;
        self
    }

    pub fn maybe_sample(&mut self, timestamp_ms: u64) -> Result<bool, SmapsError> {
        if let Some(last) = self.last_sample_ms {
            if timestamp_ms < last.saturating_add(self.interval_ms) {
                return Ok(false);
            }
        }
        self.sample_now(timestamp_ms)?;
        Ok(true)
    }

    pub fn sample_now(&mut self, timestamp_ms: u64) -> Result<(), SmapsError> {
        let sample = sample_process(self.pid, &self.attribution, timestamp_ms)?;
        self.last_sample_ms = Some(timestamp_ms);
        self.samples.push(sample);
        Ok(())
    }

    pub fn latest_rss_bytes(&self) -> Option<u64> {
        self.samples
            .last()
            .map(|sample| sample.vm_rss_kb.saturating_mul(1024))
    }

    pub fn peak_rss_between(&self, start_ms: u64, end_ms: u64) -> Option<u64> {
        self.samples
            .iter()
            .filter(|sample| sample.timestamp_ms >= start_ms && sample.timestamp_ms <= end_ms)
            .map(|sample| sample.vm_rss_kb.saturating_mul(1024))
            .max()
    }

    pub fn samples(&self) -> &[MemoryAttribution] {
        &self.samples
    }

    pub fn into_samples(self) -> Vec<MemoryAttribution> {
        self.samples
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStatusMemorySample {
    attribution: MemoryAttribution,
    minor_faults: Option<u64>,
    major_faults: Option<u64>,
}

impl ProcessStatusMemorySample {
    pub fn from_memory_attribution(
        attribution: MemoryAttribution,
        minor_faults: Option<u64>,
        major_faults: Option<u64>,
    ) -> Self {
        Self {
            attribution,
            minor_faults,
            major_faults,
        }
    }

    pub fn timestamp_ms(&self) -> u64 {
        self.attribution.timestamp_ms
    }

    pub fn rss_bytes(&self) -> u64 {
        self.attribution.vm_rss_kb.saturating_mul(1024)
    }

    pub fn minor_faults(&self) -> Option<u64> {
        self.minor_faults
    }

    pub fn major_faults(&self) -> Option<u64> {
        self.major_faults
    }

    pub fn attribution(&self) -> &MemoryAttribution {
        &self.attribution
    }
}

#[derive(Debug, Error)]
pub enum MemorySamplerError {
    #[error("memory sampler mutex poisoned")]
    MutexPoisoned,

    #[error("memory sampler thread panicked")]
    ThreadPanicked,
}

pub struct BestEffortProcessMemorySampler {
    pid: i32,
    samples: Arc<Mutex<Vec<ProcessStatusMemorySample>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<(), MemorySamplerError>>>,
}

impl BestEffortProcessMemorySampler {
    pub fn start(started_at: Instant, interval_ms: u64) -> Result<Self, MemorySamplerError> {
        let pid = std::process::id() as i32;
        let samples = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_samples = Arc::clone(&samples);
        let thread_stop = Arc::clone(&stop);
        let sleep_interval = Duration::from_millis(interval_ms.max(1));
        let handle = std::thread::spawn(move || -> Result<(), MemorySamplerError> {
            loop {
                let timestamp_ms = elapsed_ms_since(started_at);
                if let Some(sample) = sample_process_status(pid, timestamp_ms) {
                    push_process_status_sample(&thread_samples, sample)?;
                }

                if thread_stop.load(Ordering::Relaxed) {
                    break;
                }

                std::thread::sleep(sleep_interval);
            }

            Ok(())
        });

        let sampler = Self {
            pid,
            samples,
            stop,
            handle: Some(handle),
        };
        sampler.sample_now(0)?;
        Ok(sampler)
    }

    pub fn sample_now(&self, timestamp_ms: u64) -> Result<(), MemorySamplerError> {
        if let Some(sample) = sample_process_status(self.pid, timestamp_ms) {
            push_process_status_sample(&self.samples, sample)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Vec<ProcessStatusMemorySample>, MemorySamplerError> {
        self.stop.store(true, Ordering::Relaxed);

        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(MemorySamplerError::ThreadPanicked),
            }
        }

        let mut samples = self
            .samples
            .lock()
            .map_err(|_| MemorySamplerError::MutexPoisoned)?
            .clone();
        samples.sort_unstable_by_key(ProcessStatusMemorySample::timestamp_ms);
        Ok(samples)
    }
}

impl Drop for BestEffortProcessMemorySampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn sample_process_status(pid: i32, timestamp_ms: u64) -> Option<ProcessStatusMemorySample> {
    let parser = SmapsParser::new(pid);
    let status = parser.parse_status().ok()?;
    let page_faults = parser.parse_stat().ok();

    let attribution = MemoryAttribution {
        timestamp_ms,
        vm_rss_kb: status.vm_rss_kb,
        vm_size_kb: status.vm_size_kb,
        rss_anon_kb: status.rss_anon_kb,
        rss_file_kb: status.rss_file_kb,
        vm_swap_kb: status.vm_swap_kb,
        minor_faults: page_faults
            .map(|(minor_faults, _)| minor_faults)
            .unwrap_or(0),
        major_faults: page_faults
            .map(|(_, major_faults)| major_faults)
            .unwrap_or(0),
        ..MemoryAttribution::default()
    };

    Some(ProcessStatusMemorySample::from_memory_attribution(
        attribution,
        page_faults.map(|(minor_faults, _)| minor_faults),
        page_faults.map(|(_, major_faults)| major_faults),
    ))
}

fn push_process_status_sample(
    sink: &Arc<Mutex<Vec<ProcessStatusMemorySample>>>,
    sample: ProcessStatusMemorySample,
) -> Result<(), MemorySamplerError> {
    sink.lock()
        .map_err(|_| MemorySamplerError::MutexPoisoned)?
        .push(sample);
    Ok(())
}

fn elapsed_ms_since(started_at: Instant) -> u64 {
    started_at
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128)
        .try_into()
        .expect("elapsed millis capped to u64")
}

pub fn summarize_phase(samples: &[MemoryAttribution], window: PhaseWindow) -> Option<PhaseSummary> {
    let in_window: Vec<&MemoryAttribution> = samples
        .iter()
        .filter(|sample| {
            sample.timestamp_ms >= window.start_ms && sample.timestamp_ms <= window.end_ms
        })
        .collect();
    if in_window.is_empty() {
        return None;
    }

    let sample_count = in_window.len() as u64;
    let peak_rss_bytes = in_window
        .iter()
        .map(|sample| sample.vm_rss_kb.saturating_mul(1024))
        .max()
        .unwrap_or(0);
    let avg_rss_bytes = in_window
        .iter()
        .map(|sample| sample.vm_rss_kb.saturating_mul(1024))
        .sum::<u64>()
        / sample_count;
    let peak_vm_size_bytes = in_window
        .iter()
        .map(|sample| sample.vm_size_kb.saturating_mul(1024))
        .max()
        .unwrap_or(0);
    let avg_vm_size_bytes = in_window
        .iter()
        .map(|sample| sample.vm_size_kb.saturating_mul(1024))
        .sum::<u64>()
        / sample_count;

    let first = in_window.first().copied().expect("non-empty");
    let last = in_window.last().copied().expect("non-empty");
    let minor_faults_delta = last.minor_faults.saturating_sub(first.minor_faults);
    let major_faults_delta = last.major_faults.saturating_sub(first.major_faults);

    Some(PhaseSummary {
        kind: window.kind,
        start_ms: window.start_ms,
        end_ms: window.end_ms,
        duration_ms: window.duration_ms(),
        sample_count,
        peak_rss_bytes,
        avg_rss_bytes,
        peak_vm_size_bytes,
        avg_vm_size_bytes,
        minor_faults_delta,
        major_faults_delta,
    })
}

pub fn summarize_phases(
    samples: &[MemoryAttribution],
    windows: &[PhaseWindow],
) -> Vec<PhaseSummary> {
    windows
        .iter()
        .filter_map(|window| summarize_phase(samples, *window))
        .collect()
}

pub fn infer_gc_events(samples: &[MemoryAttribution], drop_threshold_bytes: u64) -> Vec<GcEvent> {
    if samples.len() < 2 {
        return Vec::new();
    }

    let mut events = Vec::new();
    for pair in samples.windows(2) {
        let prev = &pair[0];
        let next = &pair[1];
        let prev_rss = prev.vm_rss_kb.saturating_mul(1024);
        let next_rss = next.vm_rss_kb.saturating_mul(1024);
        if prev_rss <= next_rss {
            continue;
        }
        let reclaimed = prev_rss - next_rss;
        if reclaimed < drop_threshold_bytes {
            continue;
        }

        events.push(GcEvent {
            inferred: true,
            start_ms: prev.timestamp_ms,
            end_ms: next.timestamp_ms,
            pause_ms: next.timestamp_ms.saturating_sub(prev.timestamp_ms),
            reclaimed_bytes: reclaimed,
            live_set_rss_bytes: next_rss,
        });
    }

    events
}

pub fn infer_page_fault_bursts(
    samples: &[MemoryAttribution],
    minor_threshold: u64,
    major_threshold: u64,
) -> Vec<PageFaultBurst> {
    if samples.len() < 2 {
        return Vec::new();
    }

    let mut bursts = Vec::new();
    for pair in samples.windows(2) {
        let prev = &pair[0];
        let next = &pair[1];
        let minor_delta = next.minor_faults.saturating_sub(prev.minor_faults);
        let major_delta = next.major_faults.saturating_sub(prev.major_faults);

        if minor_delta >= minor_threshold || major_delta >= major_threshold {
            bursts.push(PageFaultBurst {
                start_ms: prev.timestamp_ms,
                end_ms: next.timestamp_ms,
                minor_faults_delta: minor_delta,
                major_faults_delta: major_delta,
            });
        }
    }

    bursts
}

pub fn find_recovery_ms(
    samples: &[MemoryAttribution],
    checkpoint_end_ms: u64,
    baseline_rss_bytes: u64,
    tolerance_percent: f64,
) -> Option<u64> {
    let tolerance_factor = (100.0 + tolerance_percent.max(0.0)) / 100.0;
    let target = (baseline_rss_bytes as f64 * tolerance_factor).round() as u64;

    samples
        .iter()
        .find(|sample| {
            sample.timestamp_ms >= checkpoint_end_ms
                && sample.vm_rss_kb.saturating_mul(1024) <= target
        })
        .map(|sample| sample.timestamp_ms.saturating_sub(checkpoint_end_ms))
}

pub fn build_scorecard(
    samples: &[MemoryAttribution],
    checkpoints: &[CheckpointProfile],
    gc_events: &[GcEvent],
    page_fault_bursts: &[PageFaultBurst],
    blocks_poked: u64,
    failed_pokes: u64,
    total_poke_time: Duration,
) -> SolScorecard {
    let rss_values_mib: Vec<f64> = samples
        .iter()
        .map(|sample| sample.vm_rss_kb as f64 / 1024.0)
        .collect();
    let peak_rss_mib = rss_values_mib
        .iter()
        .copied()
        .fold(0.0_f64, |acc, value| acc.max(value));
    let p95_rss_mib = percentile_f64(&rss_values_mib, 0.95).unwrap_or(0.0);

    let checkpoint_peak_rss_mib = checkpoints
        .iter()
        .map(|cp| cp.peak_rss_bytes as f64 / 1024.0 / 1024.0)
        .reduce(f64::max);

    let checkpoint_seconds_per_gib_values: Vec<f64> = checkpoints
        .iter()
        .filter_map(|cp| {
            let size = cp.checkpoint_size_bytes?;
            if size == 0 || cp.duration_ms == 0 {
                return None;
            }
            let gib = size as f64 / 1024.0 / 1024.0 / 1024.0;
            if gib <= 0.0 {
                return None;
            }
            Some((cp.duration_ms as f64 / 1000.0) / gib)
        })
        .collect();
    let checkpoint_seconds_per_gib = mean(&checkpoint_seconds_per_gib_values);

    let gc_pause_values: Vec<f64> = gc_events
        .iter()
        .map(|event| event.pause_ms as f64)
        .collect();
    let gc_pause_p95_ms = percentile_f64(&gc_pause_values, 0.95);

    let gc_events_per_1k_blocks = if blocks_poked > 0 {
        (gc_events.len() as f64 / blocks_poked as f64) * 1000.0
    } else {
        0.0
    };

    let blocks_per_second = if total_poke_time.as_secs_f64() > 0.0 {
        blocks_poked as f64 / total_poke_time.as_secs_f64()
    } else {
        0.0
    };

    SolScorecard {
        peak_rss_mib,
        p95_rss_mib,
        checkpoint_peak_rss_mib,
        checkpoint_seconds_per_gib,
        gc_pause_p95_ms,
        gc_events_per_1k_blocks,
        page_fault_burst_count: page_fault_bursts.len() as u64,
        blocks_per_second,
        failed_pokes,
    }
}

fn percentile_f64(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p = p.clamp(0.0, 1.0);
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted.get(idx).copied()
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: u64, rss_kb: u64, vm_size_kb: u64, minor: u64, major: u64) -> MemoryAttribution {
        MemoryAttribution {
            timestamp_ms: ts,
            vm_rss_kb: rss_kb,
            vm_size_kb,
            minor_faults: minor,
            major_faults: major,
            ..Default::default()
        }
    }

    #[test]
    fn test_summarize_phase_basic() {
        let samples = vec![
            sample(0, 1000, 3000, 10, 1),
            sample(100, 1500, 3200, 20, 1),
            sample(200, 1200, 3400, 30, 3),
        ];

        let summary = summarize_phase(&samples, PhaseWindow::new(PhaseKind::Replay, 0, 200))
            .expect("expected summary");
        assert_eq!(summary.sample_count, 3);
        assert_eq!(summary.peak_rss_bytes, 1500 * 1024);
        assert_eq!(summary.avg_rss_bytes, ((1000 + 1500 + 1200) * 1024) / 3);
        assert_eq!(summary.minor_faults_delta, 20);
        assert_eq!(summary.major_faults_delta, 2);
    }

    #[test]
    fn test_infer_gc_events_threshold() {
        let samples = vec![
            sample(0, 1000, 2000, 0, 0),
            sample(100, 1400, 2000, 10, 0),
            sample(200, 900, 2000, 20, 0),
            sample(300, 890, 2000, 30, 0),
        ];

        let events = infer_gc_events(&samples, 400 * 1024);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start_ms, 100);
        assert_eq!(events[0].end_ms, 200);
        assert_eq!(events[0].reclaimed_bytes, (1400 - 900) * 1024);
    }

    #[test]
    fn test_infer_page_fault_bursts() {
        let samples = vec![
            sample(0, 1000, 2000, 100, 2),
            sample(100, 1100, 2000, 150, 2),
            sample(200, 1200, 2000, 500, 3),
            sample(300, 1300, 2000, 520, 8),
        ];

        let bursts = infer_page_fault_bursts(&samples, 200, 3);
        assert_eq!(bursts.len(), 2);
        assert_eq!(bursts[0].start_ms, 100);
        assert_eq!(bursts[1].start_ms, 200);
    }

    #[test]
    fn test_find_recovery_ms() {
        let samples = vec![
            sample(0, 1000, 2000, 0, 0),
            sample(100, 1800, 2000, 0, 0),
            sample(200, 1200, 2000, 0, 0),
            sample(300, 1020, 2000, 0, 0),
        ];

        // Baseline 1000 KB -> target at 5% tolerance is 1050 KB.
        let recovery = find_recovery_ms(&samples, 120, 1000 * 1024, 5.0).expect("recovery");
        assert_eq!(recovery, 180); // sample at 300ms.
    }

    #[test]
    fn test_build_scorecard() {
        let samples = vec![
            sample(0, 1000, 2000, 10, 1),
            sample(100, 1500, 2000, 20, 1),
            sample(200, 1100, 2000, 25, 2),
            sample(300, 1300, 2000, 30, 2),
        ];
        let checkpoints = vec![CheckpointProfile {
            start_ms: 100,
            end_ms: 200,
            duration_ms: 100,
            pre_checkpoint_rss_bytes: 1500 * 1024,
            post_checkpoint_rss_bytes: 1100 * 1024,
            peak_rss_bytes: 1600 * 1024,
            recovery_ms: Some(50),
            checkpoint_size_bytes: Some(256 * 1024 * 1024),
        }];
        let gc_events = vec![GcEvent {
            inferred: true,
            start_ms: 100,
            end_ms: 200,
            pause_ms: 100,
            reclaimed_bytes: 400 * 1024,
            live_set_rss_bytes: 1100 * 1024,
        }];
        let bursts = vec![PageFaultBurst {
            start_ms: 100,
            end_ms: 200,
            minor_faults_delta: 500,
            major_faults_delta: 2,
        }];

        let score = build_scorecard(
            &samples,
            &checkpoints,
            &gc_events,
            &bursts,
            1000,
            2,
            Duration::from_secs(10),
        );

        assert!((score.peak_rss_mib - (1500.0 / 1024.0)).abs() < 1e-6);
        assert_eq!(score.page_fault_burst_count, 1);
        assert_eq!(score.gc_events_per_1k_blocks, 1.0);
        assert!((score.blocks_per_second - 100.0).abs() < 1e-6);
        assert_eq!(score.failed_pokes, 2);
        assert!(score.checkpoint_seconds_per_gib.is_some());
    }
}
