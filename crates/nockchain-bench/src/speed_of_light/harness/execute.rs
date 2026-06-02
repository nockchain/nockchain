use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::artifacts::write_run_artifacts;
use super::case::{ExecutionConfig, ResolvedCase};
use super::{create_temp_dir, CpuProfilerKind, HarnessError};
use crate::speed_of_light::bench::{SolBenchConfig, SolBenchResults, SolBenchRunner};
use crate::speed_of_light::final_tip::FinalTipValidation;
use crate::speed_of_light::fixture::extract_fixture_to_paths;
use crate::speed_of_light::orchestrate_execute::execute_trusted_plan_once;
use crate::speed_of_light::orchestrate_plan::TrustedPlan;
use crate::speed_of_light::profiling::{
    build_scorecard, infer_gc_events, infer_page_fault_bursts, summarize_phases,
    BestEffortProcessMemorySampler, MemoryProfile, PhaseKind, PhaseWindow,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ExecuteOptions {
    pub gc_drop_threshold_mib: u64,
    pub page_fault_minor_burst_threshold: u64,
    pub page_fault_major_burst_threshold: u64,
}

impl Default for ExecuteOptions {
    fn default() -> Self {
        Self::from(&ExecutionConfig::default())
    }
}

impl From<&ExecutionConfig> for ExecuteOptions {
    fn from(value: &ExecutionConfig) -> Self {
        Self {
            gc_drop_threshold_mib: value.gc_drop_threshold_mib,
            page_fault_minor_burst_threshold: value.page_fault_minor_burst_threshold,
            page_fault_major_burst_threshold: value.page_fault_major_burst_threshold,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockTimingRecord {
    pub height: u64,
    pub duration_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub success: bool,
    pub error: Option<String>,
    pub blocks_poked: u64,
    pub failed_pokes: u64,
    pub init_time_secs: f64,
    pub total_replay_time_secs: f64,
    pub throughput_blocks_per_second: f64,
    pub average_block_time_ms: f64,
    pub peak_process_rss_bytes: Option<f64>,
    pub minor_faults_total: Option<f64>,
    pub major_faults_total: Option<f64>,
    #[serde(default)]
    pub final_tip_validation: Option<FinalTipValidation>,
}

pub struct CompletedRun {
    pub record: RunRecord,
    pub trusted_orchestrate_record: Option<crate::speed_of_light::orchestrate_execute::RunRecord>,
    pub invalid_reasons: Vec<String>,
    pub block_timings: Vec<BlockTimingRecord>,
    pub profile: Option<MemoryProfile>,
    pub bench_results: Option<SolBenchResults>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuProfileExecutionKind {
    Native,
    DockerInContainer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuProfileArtifact {
    pub profiler_kind: CpuProfilerKind,
    pub sample_rate_hz: u32,
    pub execution_kind: CpuProfileExecutionKind,
    pub profiled_command: Vec<String>,
    pub output_relative_path: PathBuf,
    pub symbol_dir_relative_path: PathBuf,
    pub symbol_binary_relative_path: PathBuf,
}

pub fn cpu_profile_output_relative_path(profiler_kind: CpuProfilerKind) -> PathBuf {
    match profiler_kind {
        CpuProfilerKind::Samply => PathBuf::from("profiles/samply-profile.json.gz"),
    }
}

pub async fn execute_once(
    resolved: &ResolvedCase,
    run_id: &str,
    run_dir: &Path,
) -> Result<CompletedRun, HarnessError> {
    execute_once_with_work_dir(resolved, run_id, run_dir, None).await
}

pub async fn execute_once_with_work_dir(
    resolved: &ResolvedCase,
    run_id: &str,
    run_dir: &Path,
    work_dir: Option<&Path>,
) -> Result<CompletedRun, HarnessError> {
    execute_once_with_options(
        resolved,
        run_id,
        run_dir,
        work_dir,
        &ExecuteOptions::from(&resolved.execution_config),
    )
    .await
}

pub async fn execute_once_with_options(
    resolved: &ResolvedCase,
    run_id: &str,
    run_dir: &Path,
    work_dir: Option<&Path>,
    options: &ExecuteOptions,
) -> Result<CompletedRun, HarnessError> {
    if resolved.benchmark == "sol-orchestrate" && resolved.orchestrate.step_count > 0 {
        return execute_orchestrate_once(resolved, run_id, run_dir, work_dir, options).await;
    }

    let run = match run_benchmark_once(resolved, options).await {
        Ok(results) => completed_run_from_results(run_id, results),
        Err(error) => CompletedRun {
            record: RunRecord {
                run_id: run_id.to_string(),
                success: false,
                error: Some(error.to_string()),
                blocks_poked: 0,
                failed_pokes: 0,
                init_time_secs: 0.0,
                total_replay_time_secs: 0.0,
                throughput_blocks_per_second: 0.0,
                average_block_time_ms: 0.0,
                peak_process_rss_bytes: None,
                minor_faults_total: None,
                major_faults_total: None,
                final_tip_validation: None,
            },
            trusted_orchestrate_record: None,
            invalid_reasons: Vec::new(),
            block_timings: Vec::new(),
            profile: None,
            bench_results: None,
        },
    };

    write_run_artifacts(run_dir, &run)?;
    Ok(run)
}

async fn execute_orchestrate_once(
    resolved: &ResolvedCase,
    run_id: &str,
    run_dir: &Path,
    work_dir: Option<&Path>,
    options: &ExecuteOptions,
) -> Result<CompletedRun, HarnessError> {
    let output_root = run_dir.parent().and_then(Path::parent).ok_or_else(|| {
        HarnessError::InvalidRequestedCase("run_dir must be under runs/<run_id>".to_string())
    })?;
    let trusted_plan_path = output_root.join(&resolved.orchestrate.trusted_plan_relative_path);
    let plan: TrustedPlan = serde_json::from_slice(&std::fs::read(&trusted_plan_path)?)?;
    let default_work_dir = run_dir.join("work");
    let work_dir = work_dir.unwrap_or(&default_work_dir);
    let profiling_start = Instant::now();
    let sampler = if resolved.requested.profile_memory {
        Some(
            BestEffortProcessMemorySampler::start(
                profiling_start, resolved.requested.profile_interval_ms,
            )
            .map_err(|error| HarnessError::CommandFailure(error.to_string()))?,
        )
    } else {
        None
    };

    let record = match execute_trusted_plan_once(
        &plan,
        run_id,
        run_dir,
        work_dir,
        resolved.requested.fsync_enabled(),
        resolved.requested.allow_degraded_cold,
    )
    .await
    {
        Ok(record) => record,
        Err(error) => {
            let reason = error
                .invalid_reason()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            let trusted_failed = failed_trusted_orchestrate_record(
                &plan,
                run_id,
                reason.clone(),
                resolved.requested.fsync_enabled(),
            );
            let profile = finish_trusted_orchestrate_profile(
                sampler, profiling_start, &trusted_failed, resolved, options,
            )?;
            let mut trusted_failed = trusted_failed;
            trusted_failed.memory_profile = profile.clone();
            let mut failed = CompletedRun {
                record: run_record_projection_from_trusted(&trusted_failed),
                trusted_orchestrate_record: Some(trusted_failed),
                invalid_reasons: error
                    .invalid_reason()
                    .map(|reason| vec![reason.to_string()])
                    .unwrap_or_default(),
                block_timings: Vec::new(),
                profile,
                bench_results: None,
            };
            if let Some(profile) = &failed.profile {
                failed.record.apply_profile_summary(profile);
            }
            write_run_artifacts(run_dir, &failed)?;
            return Ok(failed);
        }
    };

    let profile =
        finish_trusted_orchestrate_profile(sampler, profiling_start, &record, resolved, options)?;
    let mut record = record;
    record.memory_profile = profile.clone();
    let mut projected = run_record_projection_from_trusted(&record);
    if let Some(profile) = &profile {
        projected.apply_profile_summary(profile);
    }

    let completed = CompletedRun {
        record: projected,
        trusted_orchestrate_record: Some(record),
        invalid_reasons: Vec::new(),
        block_timings: Vec::new(),
        profile,
        bench_results: None,
    };
    write_run_artifacts(run_dir, &completed)?;
    Ok(completed)
}

fn finish_trusted_orchestrate_profile(
    sampler: Option<BestEffortProcessMemorySampler>,
    profiling_start: Instant,
    record: &crate::speed_of_light::orchestrate_execute::RunRecord,
    resolved: &ResolvedCase,
    options: &ExecuteOptions,
) -> Result<Option<MemoryProfile>, HarnessError> {
    let Some(sampler) = sampler else {
        return Ok(None);
    };

    let end_ms = profiling_start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    sampler
        .sample_now(end_ms)
        .map_err(|error| HarnessError::CommandFailure(error.to_string()))?;
    let status_samples = sampler
        .finish()
        .map_err(|error| HarnessError::CommandFailure(error.to_string()))?;
    let mut samples = status_samples
        .into_iter()
        .map(|sample| sample.attribution().clone())
        .collect::<Vec<_>>();
    samples.sort_by_key(|sample| sample.timestamp_ms);

    let mut phase_windows = vec![PhaseWindow::new(PhaseKind::Replay, 0, end_ms)];
    let gc_events = infer_gc_events(
        &samples,
        options.gc_drop_threshold_mib.saturating_mul(1024 * 1024),
    );
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
        &samples, options.page_fault_minor_burst_threshold,
        options.page_fault_major_burst_threshold,
    );
    let checkpoint_profiles = Vec::new();
    let scorecard = build_scorecard(
        &samples,
        &checkpoint_profiles,
        &gc_events,
        &page_fault_bursts,
        record.counts.poke_archive_block,
        record.counts.error_steps,
        Duration::from_secs_f64(record.timing.total_poke_time_secs.max(0.0)),
    );

    Ok(Some(MemoryProfile {
        interval_ms: resolved.requested.profile_interval_ms.max(1),
        samples,
        phase_windows,
        phase_summaries,
        checkpoint_profiles,
        gc_events,
        page_fault_bursts,
        scorecard,
    }))
}

fn run_record_projection_from_trusted(
    record: &crate::speed_of_light::orchestrate_execute::RunRecord,
) -> RunRecord {
    RunRecord {
        run_id: record.run_id.clone(),
        success: record.success,
        error: record.error.clone(),
        blocks_poked: record.counts.poke_archive_block,
        failed_pokes: record.counts.error_steps,
        init_time_secs: record.boot.init_time_secs.unwrap_or(0.0),
        total_replay_time_secs: record.timing.total_poke_time_secs,
        throughput_blocks_per_second: record.throughput.pokes_per_second.unwrap_or(0.0),
        average_block_time_ms: 0.0,
        peak_process_rss_bytes: None,
        minor_faults_total: None,
        major_faults_total: None,
        final_tip_validation: None,
    }
}

impl RunRecord {
    fn apply_profile_summary(&mut self, profile: &MemoryProfile) {
        self.peak_process_rss_bytes = profile
            .samples
            .iter()
            .map(|sample| sample.vm_rss_kb.saturating_mul(1024) as f64)
            .max_by(|left, right| left.total_cmp(right));
        self.minor_faults_total = total_minor_faults(profile);
        self.major_faults_total = total_major_faults(profile);
    }
}

fn failed_trusted_orchestrate_record(
    plan: &TrustedPlan,
    run_id: &str,
    error: String,
    fsync: bool,
) -> crate::speed_of_light::orchestrate_execute::RunRecord {
    let cold_steps_planned = plan
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step,
                crate::speed_of_light::orchestrate_plan::TrustedStep::PeekHeightCold { .. }
            )
        })
        .count() as u64;
    crate::speed_of_light::orchestrate_execute::RunRecord {
        schema_version: crate::speed_of_light::RUN_RESULT_SCHEMA_VERSION.to_string(),
        benchmark: "sol-orchestrate".to_string(),
        run_id: run_id.to_string(),
        success: false,
        error: Some(error.clone()),
        boot: crate::speed_of_light::orchestrate_execute::RunBoot {
            source: plan.boot.source.clone(),
            kernel_input_id: plan.boot.kernel_input_id.clone(),
            fsync,
            init_time_secs: None,
        },
        steps_planned: plan.steps.len() as u64,
        steps_executed: 0,
        cold: crate::speed_of_light::orchestrate_execute::RunColdCounts {
            cold_steps_planned,
            cold_steps_verified: 0,
            cold_steps_unverified: 0,
        },
        counts: crate::speed_of_light::orchestrate_execute::RunCounts::default(),
        timing: crate::speed_of_light::orchestrate_execute::RunTiming::default(),
        throughput: crate::speed_of_light::orchestrate_execute::RunThroughput::default(),
        expected_final_tip: plan.expected_final_tip.clone(),
        final_tip: None,
        final_tip_validation: None,
        invalid_reasons: vec![error],
        failed_step_index: None,
        memory_profile: None,
    }
}

async fn run_benchmark_once(
    resolved: &ResolvedCase,
    options: &ExecuteOptions,
) -> Result<SolBenchResults, HarnessError> {
    struct TempDirGuard {
        path: PathBuf,
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    let temp_dir = create_temp_dir("nockchain-bench-harness")?;
    let _temp_dir_guard = TempDirGuard {
        path: temp_dir.clone(),
    };

    let checkpoint_path = temp_dir.join("fixture.chkjam");
    let archive_path = temp_dir.join("fixture.solarch");
    let kernel_path = temp_dir.join("fixture.jam");
    let work_dir = temp_dir.join("checkpoint-work");
    std::fs::create_dir_all(&work_dir)?;

    extract_fixture_to_paths(
        &resolved.absolute_fixture_path, &checkpoint_path, &archive_path, &kernel_path,
    )?;

    let config = SolBenchConfig {
        archive_path: archive_path.to_string_lossy().to_string(),
        kernel_path: kernel_path.to_string_lossy().to_string(),
        block_count: resolved.requested.blocks,
        skip_genesis: resolved.requested.skip_genesis,
        proof_version: None,
        checkpoint_path: Some(checkpoint_path.to_string_lossy().to_string()),
        start_height: Some(resolved.fixture_manifest.archive_start_height),
        fsync: resolved.requested.fsync_enabled(),
        profile_memory: resolved.requested.profile_memory,
        profile_interval_ms: resolved.requested.profile_interval_ms,
        gc_drop_threshold_bytes: options.gc_drop_threshold_mib.saturating_mul(1024 * 1024),
        page_fault_minor_burst_threshold: options.page_fault_minor_burst_threshold,
        page_fault_major_burst_threshold: options.page_fault_major_burst_threshold,
        work_dir,
    };

    let mut runner = SolBenchRunner::new(config);
    Ok(runner.run().await?)
}

fn completed_run_from_results(run_id: &str, results: SolBenchResults) -> CompletedRun {
    let block_timings = results
        .block_timings
        .iter()
        .map(|(height, duration)| BlockTimingRecord {
            height: height.as_u64(),
            duration_ms: duration.as_secs_f64() * 1000.0,
        })
        .collect();
    let profile = results.memory_profile.clone();

    CompletedRun {
        record: RunRecord {
            run_id: run_id.to_string(),
            success: true,
            error: None,
            blocks_poked: results.blocks_poked,
            failed_pokes: results.failed_pokes,
            init_time_secs: results.init_time.as_secs_f64(),
            total_replay_time_secs: results.total_poke_time.as_secs_f64(),
            throughput_blocks_per_second: results.blocks_per_second(),
            average_block_time_ms: results.avg_block_time().as_secs_f64() * 1000.0,
            peak_process_rss_bytes: profile.as_ref().and_then(|profile| {
                profile
                    .samples
                    .iter()
                    .map(|sample| sample.vm_rss_kb.saturating_mul(1024) as f64)
                    .max_by(|left, right| left.total_cmp(right))
            }),
            minor_faults_total: profile.as_ref().and_then(total_minor_faults),
            major_faults_total: profile.as_ref().and_then(total_major_faults),
            final_tip_validation: results.final_tip_validation.clone(),
        },
        trusted_orchestrate_record: None,
        invalid_reasons: results.invalid_reasons.clone(),
        block_timings,
        profile,
        bench_results: Some(results),
    }
}

fn total_minor_faults(profile: &MemoryProfile) -> Option<f64> {
    let first = profile.samples.first()?;
    let last = profile.samples.last()?;
    Some(last.minor_faults.saturating_sub(first.minor_faults) as f64)
}

fn total_major_faults(profile: &MemoryProfile) -> Option<f64> {
    let first = profile.samples.first()?;
    let last = profile.samples.last()?;
    Some(last.major_faults.saturating_sub(first.major_faults) as f64)
}
