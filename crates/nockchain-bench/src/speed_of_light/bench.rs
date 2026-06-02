//! Speed-of-light benchmark runner
//!
//! Pokes archived blocks into a fresh kernel as fast as possible
//! to measure maximum throughput.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use nockapp::nockapp::NockApp;
use nockapp::save::SaveableCheckpoint;
use thiserror::Error;
use tracing::info;

use super::archive::{ArchiveFilter, SolArchiveReader};
use super::checkpoint::{load_checkpoint, CheckpointLoadError};
use super::final_tip::{validate_final_tip, FinalTipValidation, ObservedFinalTip};
use super::harness::DEFAULT_FSYNC_ENABLED;
use super::kernel_utils::{
    init_nockapp, peek_heaviest_chain, sol_replay_wire, KernelInitError, PeekChainError,
};
use super::poke::poke_archive_block;
use super::profiling::{
    build_scorecard, infer_gc_events, infer_page_fault_bursts, summarize_phases, MemoryProfile,
    PhaseKind, PhaseWindow, ProcessMemoryProfiler,
};
use super::replay_window::{select_replay_window, ReplayWindowOptions};
use super::start_height::{resolve_start_height, StartHeightError};
use super::types::{ProofVersion, SolHeight};

#[derive(Debug, Error)]
pub enum BenchError {
    #[error("Archive error: {0}")]
    Archive(#[from] super::archive::ArchiveError),

    #[error("Unsupported benchmark path: {0}")]
    Unsupported(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Kernel load error: {0}")]
    KernelLoad(String),

    #[error("Checkpoint load error: {0}")]
    Checkpoint(#[from] CheckpointLoadError),

    #[error("Cue error: {0}")]
    Cue(String),

    #[error("Poke error: {0}")]
    Poke(String),

    #[error("Noun decode error: {0}")]
    NounDecode(#[from] noun_serde::NounDecodeError),

    #[error("Start height error: {0}")]
    StartHeight(#[from] StartHeightError),

    #[error("Checkpoint chain height unavailable and no explicit start height was provided")]
    CheckpointHeightUnavailable,

    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::nockapp::NockAppError),

    #[error("Kernel init error: {0}")]
    KernelInit(#[from] KernelInitError),

    #[error("Chain height peek error: {0}")]
    ChainPeek(#[from] PeekChainError),

    #[error("Memory sampling error: {0}")]
    MemorySample(String),
}

/// Configuration for the benchmark
#[derive(Debug, Clone)]
pub struct SolBenchConfig {
    /// Path to the archive file
    pub archive_path: String,
    /// Path to the kernel jam file
    pub kernel_path: String,
    /// Number of blocks to benchmark (0 = all)
    pub block_count: u64,
    /// Whether to skip genesis block (block 0)
    pub skip_genesis: bool,
    /// Optional proof version filter
    pub proof_version: Option<ProofVersion>,
    /// Optional starting checkpoint to load before benchmarking
    pub checkpoint_path: Option<String>,
    /// Optional start height override
    pub start_height: Option<SolHeight>,
    /// Whether PMA replay should keep fsync durability enabled.
    pub fsync: bool,
    /// Enable memory timeline profiling during benchmark.
    pub profile_memory: bool,
    /// Sampling interval for memory profile.
    pub profile_interval_ms: u64,
    /// Inferred GC threshold based on RSS drops.
    pub gc_drop_threshold_bytes: u64,
    /// Detect page-fault bursts by minor fault delta.
    pub page_fault_minor_burst_threshold: u64,
    /// Detect page-fault bursts by major fault delta.
    pub page_fault_major_burst_threshold: u64,
    /// Working directory for generated checkpoint files.
    pub work_dir: PathBuf,
}

impl Default for SolBenchConfig {
    fn default() -> Self {
        Self {
            archive_path: "blocks.solarch".to_string(),
            kernel_path: "assets/dumb.jam".to_string(),
            block_count: 0,
            skip_genesis: false, // Genesis is required for chain validation
            proof_version: None,
            checkpoint_path: None,
            start_height: None,
            fsync: DEFAULT_FSYNC_ENABLED,
            profile_memory: false,
            profile_interval_ms: 500,
            gc_drop_threshold_bytes: 64 * 1024 * 1024, // 64 MiB
            page_fault_minor_burst_threshold: 50_000,
            page_fault_major_burst_threshold: 1,
            work_dir: PathBuf::from("."),
        }
    }
}

/// Results from a benchmark run
#[derive(Debug, Clone)]
pub struct SolBenchResults {
    /// Total number of blocks poked
    pub blocks_poked: u64,
    /// Total time for all pokes
    pub total_poke_time: Duration,
    /// Time for kernel initialization
    pub init_time: Duration,
    /// Individual block timings (height, duration)
    pub block_timings: Vec<(SolHeight, Duration)>,
    /// Number of failed pokes
    pub failed_pokes: u64,
    /// Optional memory timeline profile and derived scorecard.
    pub memory_profile: Option<MemoryProfile>,
    /// Final-tip validation result when the selected replay window has a derivable expected tip.
    pub final_tip_validation: Option<FinalTipValidation>,
    /// Reasons this quick result is incomplete/invalid as replay evidence.
    pub invalid_reasons: Vec<String>,
}

impl SolBenchResults {
    /// Blocks per second
    pub fn blocks_per_second(&self) -> f64 {
        if self.total_poke_time.as_secs_f64() > 0.0 {
            self.blocks_poked as f64 / self.total_poke_time.as_secs_f64()
        } else {
            0.0
        }
    }

    /// Average time per block
    pub fn avg_block_time(&self) -> Duration {
        if self.blocks_poked > 0 {
            self.total_poke_time / self.blocks_poked as u32
        } else {
            Duration::ZERO
        }
    }

    /// Print a summary of the results
    pub fn print_summary(&self) {
        println!("\n=== Benchmark Results ===\n");
        println!("Blocks poked:    {}", self.blocks_poked);
        println!("Failed pokes:    {}", self.failed_pokes);
        println!("Init time:       {:.2}s", self.init_time.as_secs_f64());
        println!(
            "Total poke time: {:.2}s",
            self.total_poke_time.as_secs_f64()
        );
        println!(
            "Avg per block:   {:.2}ms",
            self.avg_block_time().as_secs_f64() * 1000.0
        );
        println!("Throughput:      {:.2} blocks/s", self.blocks_per_second());
        if let Some(validation) = &self.final_tip_validation {
            println!(
                "Expected tip:    {} {}",
                validation.expected.height, validation.expected.hash
            );
            match &validation.observed {
                Some(observed) => {
                    println!("Observed tip:    {} {}", observed.height, observed.hash)
                }
                None => println!("Observed tip:    unavailable"),
            }
            println!("Final tip valid: {}", validation.valid);
        }
        if !self.invalid_reasons.is_empty() {
            println!("Invalid reasons:");
            for reason in &self.invalid_reasons {
                println!("  - {reason}");
            }
        }
        if let Some(profile) = &self.memory_profile {
            println!("\n=== Memory Profile ===\n");
            println!("Samples:         {}", profile.samples.len());
            println!("GC events:       {}", profile.gc_events.len());
            println!("Fault bursts:    {}", profile.page_fault_bursts.len());
            println!("Peak RSS:        {:.2} MiB", profile.scorecard.peak_rss_mib);
            println!("P95 RSS:         {:.2} MiB", profile.scorecard.p95_rss_mib);
            if let Some(value) = profile.scorecard.checkpoint_peak_rss_mib {
                println!("Ckpt peak RSS:   {:.2} MiB", value);
            }
            if let Some(value) = profile.scorecard.checkpoint_seconds_per_gib {
                println!("Ckpt sec/GiB:    {:.2}", value);
            }
            if let Some(value) = profile.scorecard.gc_pause_p95_ms {
                println!("GC pause p95:    {:.1} ms", value);
            }
            println!(
                "GC / 1k blocks:  {:.2}",
                profile.scorecard.gc_events_per_1k_blocks
            );
        }
    }
}

/// Speed-of-light benchmark runner
pub struct SolBenchRunner {
    config: SolBenchConfig,
    nockapp: Option<NockApp>,
}

impl SolBenchRunner {
    /// Create a new benchmark runner
    pub fn new(config: SolBenchConfig) -> Self {
        Self {
            config,
            nockapp: None,
        }
    }

    /// Initialize a fresh kernel (no checkpoint state)
    pub async fn initialize(&mut self) -> Result<(), BenchError> {
        info!(kernel = %self.config.kernel_path, "Initializing fresh kernel for benchmark");

        let checkpoint = if let Some(path) = &self.config.checkpoint_path {
            let loaded = load_checkpoint(path)?;
            Some(SaveableCheckpoint {
                ker_hash: loaded.ker_hash,
                event_num: loaded.event_num,
                state: loaded.state,
                cold: loaded.cold,
            })
        } else {
            None
        };

        let nockapp = init_nockapp(
            std::path::Path::new(&self.config.kernel_path),
            checkpoint,
            &self.config.work_dir,
            false,
            self.config.fsync,
        )
        .await?;

        info!("Fresh kernel initialized");
        self.nockapp = Some(nockapp);
        Ok(())
    }

    /// Run the benchmark
    pub async fn run(&mut self) -> Result<SolBenchResults, BenchError> {
        // Load archive
        info!(archive = %self.config.archive_path, "Loading archive");
        let archive_bytes = std::fs::read(&self.config.archive_path)?;
        let reader = SolArchiveReader::from_bytes(archive_bytes)?;
        let archive = reader.inspect();

        info!(
            blocks = archive.block_count,
            min_height = archive.min_height.as_u64(),
            max_height = archive.max_height.as_u64(),
            "Archive loaded"
        );

        let run_start = Instant::now();
        let mut profiler = if self.config.profile_memory {
            Some(ProcessMemoryProfiler::new(self.config.profile_interval_ms))
        } else {
            None
        };
        if let Some(profiler) = profiler.as_mut() {
            profiler
                .sample_now(0)
                .map_err(|e| BenchError::MemorySample(e.to_string()))?;
        }

        let mut phase_windows = Vec::new();
        let checkpoint_profiles = Vec::new();

        // Initialize kernel
        let init_start = Instant::now();
        self.initialize().await?;
        let init_time = init_start.elapsed();
        let init_end_ms = run_start.elapsed().as_millis() as u64;
        phase_windows.push(PhaseWindow::new(PhaseKind::Init, 0, init_end_ms));
        if let Some(profiler) = profiler.as_mut() {
            profiler
                .sample_now(init_end_ms)
                .map_err(|e| BenchError::MemorySample(e.to_string()))?;
        }

        let nockapp = self.nockapp.as_mut().ok_or(BenchError::KernelLoad(
            "NockApp not initialized".to_string(),
        ))?;

        let checkpoint_height =
            if self.config.checkpoint_path.is_some() && self.config.start_height.is_none() {
                let height = peek_heaviest_chain(nockapp).await?;
                height
                    .map(|(height, _)| SolHeight(height.0 .0))
                    .ok_or(BenchError::CheckpointHeightUnavailable)
                    .map(Some)?
            } else {
                None
            };

        let start_height = resolve_start_height(self.config.start_height, checkpoint_height)?;

        let block_limit = if self.config.block_count > 0 {
            Some(self.config.block_count)
        } else {
            None
        };

        info!(
            skip_genesis = self.config.skip_genesis,
            proof_version = self.config.proof_version.map(|v| v.as_str()),
            start_height = start_height.as_u64(),
            profile_memory = self.config.profile_memory,
            "Starting benchmark"
        );

        let replay_start_ms = run_start.elapsed().as_millis() as u64;
        let mut block_timings = Vec::new();
        let mut blocks_poked = 0u64;
        let failed_pokes = 0u64;
        let poke_start = Instant::now();

        // Wire for the poke
        let wire = sol_replay_wire();

        let filter = ArchiveFilter {
            proof_version: self.config.proof_version,
            start_height: Some(start_height),
            end_height: None,
        };
        let replay_window = select_replay_window(
            &reader,
            ReplayWindowOptions {
                filter: filter.clone(),
                skip_genesis: self.config.skip_genesis,
                block_limit,
            },
        )?;
        let mut invalid_reasons = Vec::new();
        if replay_window.blocks.is_empty() {
            invalid_reasons.push("zero-block replay window".to_string());
        }
        if let Some(gap_height) = replay_window.first_gap_height {
            invalid_reasons.push(format!(
                "replay range non-contiguous: gap at height {}",
                gap_height.as_u64()
            ));
        }

        for entry in reader.iter_filtered(filter) {
            if self.config.skip_genesis && entry.height == SolHeight::ZERO {
                continue;
            }
            if let Some(limit) = block_limit {
                if blocks_poked >= limit {
                    break;
                }
            }

            if let Some(profiler) = profiler.as_mut() {
                let now_ms = run_start.elapsed().as_millis() as u64;
                profiler
                    .maybe_sample(now_ms)
                    .map_err(|e| BenchError::MemorySample(e.to_string()))?;
            }

            match poke_archive_block(nockapp, wire.clone(), &reader, entry).await {
                Ok(timings) => {
                    block_timings.push((entry.height, timings.total_duration));
                    blocks_poked += 1;

                    if let Some(profiler) = profiler.as_mut() {
                        let now_ms = run_start.elapsed().as_millis() as u64;
                        profiler
                            .maybe_sample(now_ms)
                            .map_err(|e| BenchError::MemorySample(e.to_string()))?;
                    }

                    if blocks_poked % 100 == 0 {
                        info!(
                            blocks = blocks_poked,
                            height = entry.height.as_u64(),
                            raw_txs = timings.raw_tx_pokes_completed,
                            elapsed_ms = poke_start.elapsed().as_millis(),
                            "Progress"
                        );
                    }
                }
                Err(error) => {
                    return Err(replay_poke_failure(entry.height, &error));
                }
            }
        }

        let final_tip_validation = if let Some(expected) = replay_window.expected_final_tip {
            let observed =
                peek_heaviest_chain(nockapp)
                    .await?
                    .map(|(height, hash)| ObservedFinalTip {
                        height: height.0 .0,
                        hash: hash.to_base58(),
                    });
            let validation = validate_final_tip(expected, observed);
            if let Some(reason) = &validation.invalid_reason {
                invalid_reasons.push(reason.clone());
            }
            Some(validation)
        } else {
            None
        };

        let total_poke_time = poke_start.elapsed();
        let replay_end_ms = run_start.elapsed().as_millis() as u64;
        phase_windows.push(PhaseWindow::new(
            PhaseKind::Replay,
            replay_start_ms,
            replay_end_ms,
        ));

        let memory_profile = if let Some(mut profiler) = profiler {
            profiler
                .sample_now(replay_end_ms)
                .map_err(|e| BenchError::MemorySample(e.to_string()))?;

            let mut samples = profiler.into_samples();
            samples.sort_by_key(|sample| sample.timestamp_ms);

            let gc_events = infer_gc_events(&samples, self.config.gc_drop_threshold_bytes);
            for event in &gc_events {
                phase_windows.push(PhaseWindow::new(
                    PhaseKind::Gc,
                    event.start_ms,
                    event.end_ms,
                ));
            }
            phase_windows.sort_by_key(|window| (window.start_ms, window.end_ms));

            let phase_summaries = summarize_phases(&samples, &phase_windows);
            let page_fault_bursts = infer_page_fault_bursts(
                &samples, self.config.page_fault_minor_burst_threshold,
                self.config.page_fault_major_burst_threshold,
            );
            let scorecard = build_scorecard(
                &samples, &checkpoint_profiles, &gc_events, &page_fault_bursts, blocks_poked,
                failed_pokes, total_poke_time,
            );

            Some(MemoryProfile {
                interval_ms: self.config.profile_interval_ms,
                samples,
                phase_windows,
                phase_summaries,
                checkpoint_profiles: checkpoint_profiles.clone(),
                gc_events,
                page_fault_bursts,
                scorecard,
            })
        } else {
            None
        };

        Ok(SolBenchResults {
            blocks_poked,
            total_poke_time,
            init_time,
            block_timings,
            failed_pokes,
            memory_profile,
            final_tip_validation,
            invalid_reasons,
        })
    }
}

fn replay_poke_failure(height: SolHeight, error: &dyn std::fmt::Display) -> BenchError {
    info!(
        height = height.as_u64(),
        error = %error,
        "Failed to replay archived block"
    );
    BenchError::Poke(format!(
        "failed to replay archived block at height {}: {error}",
        height.as_u64()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speed_of_light::peek_bench::{PeekSample, PeekSampleKind};

    #[test]
    fn test_bench_config_default_profile_values() {
        let config = SolBenchConfig::default();
        assert!(!config.profile_memory);
        assert!(config.fsync);
        assert_eq!(config.profile_interval_ms, 500);
        assert_eq!(config.gc_drop_threshold_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn test_peek_samples_are_usable_from_sibling_modules() {
        let _kind = PeekSampleKind::Missing;
        let _latency: fn(&PeekSample) -> u64 = PeekSample::latency_us;
    }

    #[test]
    fn replay_poke_failure_is_fatal_and_names_height() {
        let error = replay_poke_failure(SolHeight(42), &"bad raw tx");
        assert!(matches!(error, BenchError::Poke(_)));
        assert!(error
            .to_string()
            .contains("failed to replay archived block at height 42"));
    }
}
