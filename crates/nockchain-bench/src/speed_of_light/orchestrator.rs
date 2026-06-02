use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nockapp::nockapp::NockApp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::archive::{ArchiveError, SolArchiveReader};
use super::boot_source::{BootSourceError, BootSourceInput, ResolvedBootSource};
use super::checkpoint::CheckpointLoadError;
use super::harness::fsync_mode_label;
use super::kernel_utils::{
    init_boot_source_backed_nockapp, peek_heaviest_chain_or_block, sol_replay_wire,
    BootSourceBackedInitError, KernelInitError,
};
use super::peek_bench::PeekResultKind;
use super::poke::{poke_archive_block, PokeStepError};
use super::types::SolHeight;

type OrchestratorColdRuntime = crate::speed_of_light::cold_peek::ColdRuntime;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuickOrchestratePlan {
    pub boot: BootSourceInput,
    pub kernel: PathBuf,
    #[serde(default)]
    pub steps: Vec<QuickOrchestrateStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdMode {
    Strict,
    Soft,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuickOrchestrateStep {
    PokeArchiveBlock {
        archive: PathBuf,
        height: u64,
        #[serde(default)]
        label: Option<String>,
    },
    PeekHeight {
        height: u64,
        #[serde(default)]
        label: Option<String>,
    },
    ForceCold {
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        tolerance_pages: Option<u64>,
        #[serde(default)]
        max_attempts: Option<u32>,
    },
    PeekHeightCold {
        height: u64,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        tolerance_pages: Option<u64>,
        #[serde(default)]
        max_attempts: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StepOutcome {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "error")]
    Error,
}

impl StepOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Success => "success",
            Self::Missing => "missing",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StepType {
    PokeArchiveBlock,
    PeekHeight,
    ForceCold,
    PeekHeightCold,
}

impl StepType {
    fn as_str(self) -> &'static str {
        match self {
            Self::PokeArchiveBlock => "poke_archive_block",
            Self::PeekHeight => "peek_height",
            Self::ForceCold => "force_cold",
            Self::PeekHeightCold => "peek_height_cold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreparedColdStepOptions {
    tolerance_pages: u64,
    max_attempts: u32,
}

#[derive(Debug, Clone, Copy)]
struct StepMeasurement {
    duration: Duration,
    minflt_delta: Option<u64>,
    majflt_delta: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct PeekMeasurement {
    sample: super::peek_bench::PeekResultSample,
    measurement: StepMeasurement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    label: String,
    step_type: StepType,
    height: Option<u64>,
    outcome: StepOutcome,
    duration: Duration,
    error_message: Option<String>,
    minflt_delta: Option<u64>,
    majflt_delta: Option<u64>,
    cold_verified: Option<bool>,
    cold_force_duration: Option<Duration>,
    residency_pages_after: Option<u64>,
    residency_total_pages: Option<u64>,
    cold_attempts: Option<u32>,
    degraded_reason: Option<String>,
    cold_target: Option<crate::speed_of_light::cold_peek::ColdTargetKind>,
    cold_evidence: Option<crate::speed_of_light::cold_peek::ColdEvidenceDetails>,
    peek_completed: Option<bool>,
    peek_outcome: Option<StepOutcome>,
    raw_tx_pokes_completed: Option<u64>,
    block_poke_duration: Option<Duration>,
    raw_tx_poke_duration: Option<Duration>,
    slab_prebuild_duration: Option<Duration>,
    block_slab_prebuild_duration: Option<Duration>,
    raw_tx_slab_prebuild_duration: Option<Duration>,
    raw_tx_slabs_prebuilt: Option<u64>,
    raw_tx_payload_bytes_prebuilt: Option<u64>,
    slab_prebuild_start_rss_bytes: Option<u64>,
    slab_prebuild_peak_rss_bytes: Option<u64>,
}

#[derive(Serialize)]
struct StepResultWire<'a> {
    label: &'a str,
    #[serde(rename = "type")]
    step_type: StepType,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u64>,
    outcome: StepOutcome,
    duration_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none", rename = "error")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    minflt_delta: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    majflt_delta: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_force_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    residency_pages_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    residency_total_pages: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    degraded_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_target: Option<crate::speed_of_light::cold_peek::ColdTargetKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cold_evidence: Option<&'a crate::speed_of_light::cold_peek::ColdEvidenceDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peek_completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peek_outcome: Option<StepOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_tx_pokes_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_poke_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_tx_poke_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slab_prebuild_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_slab_prebuild_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_tx_slab_prebuild_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_tx_slabs_prebuilt: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_tx_payload_bytes_prebuilt: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slab_prebuild_start_rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slab_prebuild_peak_rss_bytes: Option<u64>,
}

impl StepResult {
    fn new(
        label: String,
        step_type: StepType,
        height: Option<u64>,
        outcome: StepOutcome,
        duration: Duration,
        error_message: Option<String>,
    ) -> Self {
        Self {
            label,
            step_type,
            height,
            outcome,
            duration,
            error_message,
            minflt_delta: None,
            majflt_delta: None,
            cold_verified: None,
            cold_force_duration: None,
            residency_pages_after: None,
            residency_total_pages: None,
            cold_attempts: None,
            degraded_reason: None,
            cold_target: None,
            cold_evidence: None,
            peek_completed: None,
            peek_outcome: None,
            raw_tx_pokes_completed: None,
            block_poke_duration: None,
            raw_tx_poke_duration: None,
            slab_prebuild_duration: None,
            block_slab_prebuild_duration: None,
            raw_tx_slab_prebuild_duration: None,
            raw_tx_slabs_prebuilt: None,
            raw_tx_payload_bytes_prebuilt: None,
            slab_prebuild_start_rss_bytes: None,
            slab_prebuild_peak_rss_bytes: None,
        }
    }

    fn ok(label: String, step_type: StepType, height: Option<u64>, duration: Duration) -> Self {
        Self::new(label, step_type, height, StepOutcome::Ok, duration, None)
    }

    fn with_outcome(
        label: String,
        step_type: StepType,
        height: Option<u64>,
        outcome: StepOutcome,
        duration: Duration,
    ) -> Self {
        Self::new(label, step_type, height, outcome, duration, None)
    }

    fn error(
        label: String,
        step_type: StepType,
        height: Option<u64>,
        duration: Duration,
        error_message: String,
    ) -> Self {
        Self::new(
            label,
            step_type,
            height,
            StepOutcome::Error,
            duration,
            Some(error_message),
        )
    }

    fn wire(&self) -> StepResultWire<'_> {
        StepResultWire {
            label: &self.label,
            step_type: self.step_type,
            height: self.height,
            outcome: self.outcome,
            duration_ms: duration_ms(self.duration),
            error: self.error_message.as_deref(),
            minflt_delta: self.minflt_delta,
            majflt_delta: self.majflt_delta,
            cold_verified: self.cold_verified,
            cold_force_duration_ms: self.cold_force_duration.map(duration_ms),
            residency_pages_after: self.residency_pages_after,
            residency_total_pages: self.residency_total_pages,
            cold_attempts: self.cold_attempts,
            degraded_reason: self.degraded_reason.as_deref(),
            cold_target: self.cold_target,
            cold_evidence: self.cold_evidence.as_ref(),
            peek_completed: self.peek_completed,
            peek_outcome: self.peek_outcome,
            raw_tx_pokes_completed: self.raw_tx_pokes_completed,
            block_poke_duration_ms: self.block_poke_duration.map(duration_ms),
            raw_tx_poke_duration_ms: self.raw_tx_poke_duration.map(duration_ms),
            slab_prebuild_duration_ms: self.slab_prebuild_duration.map(duration_ms),
            block_slab_prebuild_duration_ms: self.block_slab_prebuild_duration.map(duration_ms),
            raw_tx_slab_prebuild_duration_ms: self.raw_tx_slab_prebuild_duration.map(duration_ms),
            raw_tx_slabs_prebuilt: self.raw_tx_slabs_prebuilt,
            raw_tx_payload_bytes_prebuilt: self.raw_tx_payload_bytes_prebuilt,
            slab_prebuild_start_rss_bytes: self.slab_prebuild_start_rss_bytes,
            slab_prebuild_peak_rss_bytes: self.slab_prebuild_peak_rss_bytes,
        }
    }

    fn with_step_measurement(mut self, measurement: StepMeasurement) -> Self {
        self.minflt_delta = measurement.minflt_delta;
        self.majflt_delta = measurement.majflt_delta;
        self
    }

    fn with_cold_force_result(
        mut self,
        cold: crate::speed_of_light::cold_peek::ColdForceResult,
    ) -> Self {
        self.cold_verified = Some(cold.cold_verified);
        self.cold_force_duration = Some(self.duration);
        self.residency_pages_after = Some(cold.residency_pages_after);
        self.residency_total_pages = Some(cold.residency_total_pages);
        self.cold_attempts = Some(cold.cold_attempts);
        self.degraded_reason = cold.degraded_reason;
        self.cold_target = Some(cold.cold_target);
        self.cold_evidence = Some(cold.evidence);
        self
    }

    fn with_cold_force_duration(mut self, duration: Duration) -> Self {
        self.cold_force_duration = Some(duration);
        self
    }

    fn with_peek_result(mut self, outcome: StepOutcome) -> Self {
        self.peek_completed = Some(!matches!(outcome, StepOutcome::Error));
        self.peek_outcome = Some(outcome);
        self
    }

    fn with_archive_poke_timings(
        mut self,
        timings: crate::speed_of_light::poke::ArchivePokeTimings,
    ) -> Self {
        self.raw_tx_pokes_completed = Some(timings.raw_tx_pokes_completed);
        self.block_poke_duration = Some(timings.block_duration);
        self.raw_tx_poke_duration = Some(timings.raw_tx_duration);
        self.slab_prebuild_duration = Some(timings.slab_prebuild_duration);
        self.block_slab_prebuild_duration = Some(timings.block_slab_prebuild_duration);
        self.raw_tx_slab_prebuild_duration = Some(timings.raw_tx_slab_prebuild_duration);
        self.raw_tx_slabs_prebuilt = Some(timings.raw_tx_slabs_prebuilt);
        self.raw_tx_payload_bytes_prebuilt = Some(timings.raw_tx_payload_bytes_prebuilt);
        self.slab_prebuild_start_rss_bytes = timings.slab_prebuild_start_rss_bytes;
        self.slab_prebuild_peak_rss_bytes = timings.slab_prebuild_peak_rss_bytes;
        self
    }

    fn with_cold_verify_failure(
        mut self,
        cold_target: crate::speed_of_light::cold_peek::ColdTargetKind,
        residency_pages_after: u64,
        residency_total_pages: u64,
        cold_attempts: u32,
    ) -> Self {
        self.cold_verified = Some(false);
        self.cold_force_duration = Some(self.duration);
        self.residency_pages_after = Some(residency_pages_after);
        self.residency_total_pages = Some(residency_total_pages);
        self.cold_attempts = Some(cold_attempts);
        self.cold_target = Some(cold_target);
        self
    }
}

impl Serialize for StepResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.wire().serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FinalTip {
    height: u64,
    hash: String,
}

#[derive(Debug, Clone)]
pub struct QuickOrchestrateResults {
    boot_source: BootSourceInput,
    kernel_path: PathBuf,
    fsync: bool,
    init_time: Duration,
    steps: Vec<StepResult>,
    failed_step_index: Option<usize>,
    final_tip: Option<FinalTip>,
}

#[derive(Serialize)]
struct BootWire<'a> {
    source: &'a BootSourceInput,
    kernel: &'a str,
    fsync: &'static str,
    init_time_secs: f64,
}

#[derive(Serialize)]
struct QuickOrchestrateResultsWire<'a> {
    boot: BootWire<'a>,
    steps: &'a [StepResult],
}

impl QuickOrchestrateResults {
    #[cfg(test)]
    pub(crate) fn test_with_steps(steps: Vec<StepResult>) -> Self {
        Self {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("checkpoint.chkjam"),
            },
            kernel_path: PathBuf::from("kernel.jam"),
            fsync: true,
            init_time: Duration::ZERO,
            failed_step_index: steps
                .iter()
                .position(|step| matches!(step.outcome, StepOutcome::Error)),
            steps,
            final_tip: None,
        }
    }

    pub fn steps(&self) -> &[StepResult] {
        &self.steps
    }

    pub fn init_time_secs(&self) -> f64 {
        self.init_time.as_secs_f64()
    }

    pub fn final_tip_parts(&self) -> Option<(u64, &str)> {
        self.final_tip
            .as_ref()
            .map(|tip| (tip.height, tip.hash.as_str()))
    }

    pub fn succeeded(&self) -> bool {
        self.failed_step_index.is_none()
    }

    pub fn has_step_failure(&self) -> bool {
        self.failed_step_index.is_some()
    }

    pub fn failure_message(&self) -> Option<&str> {
        self.failed_step_index
            .and_then(|index| self.steps.get(index))
            .and_then(|step| step.error_message.as_deref())
    }

    pub fn to_compact_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&QuickOrchestrateResultsWire {
            boot: BootWire {
                source: &self.boot_source,
                kernel: &self.kernel_path.to_string_lossy(),
                fsync: fsync_mode_label(self.fsync),
                init_time_secs: self.init_time.as_secs_f64(),
            },
            steps: &self.steps,
        })
    }

    pub fn print_summary(&self) {
        match &self.boot_source {
            BootSourceInput::Checkpoint { checkpoint } => {
                println!("Checkpoint: {}", checkpoint.display());
            }
            BootSourceInput::Snapshot { pma, manifest } => {
                println!("Snapshot PMA:      {}", pma.display());
                println!("Snapshot manifest: {}", manifest.display());
            }
        }
        println!("Kernel:     {}", self.kernel_path.display());
        println!("Boot time:  {:.3}s", self.init_time.as_secs_f64());
        for step in &self.steps {
            let height_fragment = step
                .height
                .map(|height| format!(" height={height}"))
                .unwrap_or_default();
            println!(
                "Step {label}: type={step_type}{height_fragment} duration_ms={duration_ms:.3} outcome={outcome}",
                label = step.label,
                step_type = step.step_type.as_str(),
                height_fragment = height_fragment,
                duration_ms = duration_ms(step.duration),
                outcome = step.outcome.as_str(),
            );
            if let Some(error) = &step.error_message {
                println!("  error={error}");
            }
        }
        if let Some(final_tip) = &self.final_tip {
            println!("Final tip:  {} {}", final_tip.height, final_tip.hash);
        }
    }
}

impl StepResult {
    #[cfg(test)]
    pub(crate) fn test_poke_archive_block_with_timings(
        label: impl Into<String>,
        height: u64,
        duration: Duration,
        timings: crate::speed_of_light::poke::ArchivePokeTimings,
    ) -> Self {
        Self::ok(
            label.into(),
            StepType::PokeArchiveBlock,
            Some(height),
            duration,
        )
        .with_archive_poke_timings(timings)
    }

    #[cfg(test)]
    pub(crate) fn test_poke_archive_block_error_with_timings(
        label: impl Into<String>,
        height: u64,
        duration: Duration,
        error: impl Into<String>,
        timings: crate::speed_of_light::poke::ArchivePokeTimings,
    ) -> Self {
        Self::error(
            label.into(),
            StepType::PokeArchiveBlock,
            Some(height),
            duration,
            error.into(),
        )
        .with_archive_poke_timings(timings)
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn step_type_str(&self) -> &'static str {
        self.step_type.as_str()
    }

    pub fn height(&self) -> Option<u64> {
        self.height
    }

    pub fn outcome_str(&self) -> &'static str {
        self.outcome.as_str()
    }

    pub fn duration_ms_value(&self) -> f64 {
        duration_ms(self.duration)
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn minflt_delta(&self) -> Option<u64> {
        self.minflt_delta
    }

    pub fn majflt_delta(&self) -> Option<u64> {
        self.majflt_delta
    }

    pub fn cold_verified(&self) -> Option<bool> {
        self.cold_verified
    }

    pub fn cold_attempts(&self) -> Option<u32> {
        self.cold_attempts
    }

    pub fn cold_force_duration_ms(&self) -> Option<f64> {
        self.cold_force_duration.map(duration_ms)
    }

    pub fn residency_pages_after(&self) -> Option<u64> {
        self.residency_pages_after
    }

    pub fn residency_total_pages(&self) -> Option<u64> {
        self.residency_total_pages
    }

    pub fn degraded_reason(&self) -> Option<&str> {
        self.degraded_reason.as_deref()
    }

    pub fn cold_evidence(&self) -> Option<&crate::speed_of_light::cold_peek::ColdEvidenceDetails> {
        self.cold_evidence.as_ref()
    }

    pub fn peek_completed(&self) -> Option<bool> {
        self.peek_completed
    }

    pub fn peek_outcome(&self) -> Option<&str> {
        self.peek_outcome.map(StepOutcome::as_str)
    }

    pub fn raw_tx_pokes_completed(&self) -> Option<u64> {
        self.raw_tx_pokes_completed
    }

    pub fn block_poke_duration_ms(&self) -> Option<f64> {
        self.block_poke_duration.map(duration_ms)
    }

    pub fn raw_tx_poke_duration_ms(&self) -> Option<f64> {
        self.raw_tx_poke_duration.map(duration_ms)
    }

    pub fn slab_prebuild_duration_ms(&self) -> Option<f64> {
        self.slab_prebuild_duration.map(duration_ms)
    }

    pub fn block_slab_prebuild_duration_ms(&self) -> Option<f64> {
        self.block_slab_prebuild_duration.map(duration_ms)
    }

    pub fn raw_tx_slab_prebuild_duration_ms(&self) -> Option<f64> {
        self.raw_tx_slab_prebuild_duration.map(duration_ms)
    }

    pub fn raw_tx_slabs_prebuilt(&self) -> Option<u64> {
        self.raw_tx_slabs_prebuilt
    }

    pub fn raw_tx_payload_bytes_prebuilt(&self) -> Option<u64> {
        self.raw_tx_payload_bytes_prebuilt
    }

    pub fn slab_prebuild_start_rss_bytes(&self) -> Option<u64> {
        self.slab_prebuild_start_rss_bytes
    }

    pub fn slab_prebuild_peak_rss_bytes(&self) -> Option<u64> {
        self.slab_prebuild_peak_rss_bytes
    }
}

#[derive(Debug, Clone)]
pub struct QuickOrchestrateRunner {
    plan_path: PathBuf,
    work_dir: PathBuf,
    fsync: bool,
    cold_mode: ColdMode,
}

impl QuickOrchestrateRunner {
    pub fn new(plan_path: PathBuf, work_dir: PathBuf, fsync: bool, cold_mode: ColdMode) -> Self {
        Self {
            plan_path,
            work_dir,
            fsync,
            cold_mode,
        }
    }

    pub async fn run(&self) -> Result<QuickOrchestrateResults, PreRunError> {
        let prepared = load_and_validate_plan(&self.plan_path)?;
        let PreparedPlan {
            boot_source,
            boot_source_input,
            kernel_path,
            steps,
            archive_cache,
            warnings,
        } = prepared;
        let has_cold_steps = steps.iter().any(PreparedStep::requires_cold_runtime);

        for warning in &warnings {
            eprintln!("quick-orchestrate warning: {warning}");
        }

        let mut cold_runtime = startup_cold_runtime(has_cold_steps, self.cold_mode)?;

        std::fs::create_dir_all(&self.work_dir).map_err(|source| BootFailure::WorkDirCreate {
            path: self.work_dir.clone(),
            source,
        })?;

        let init_started_at = Instant::now();
        let nockapp =
            init_boot_source_backed_nockapp(&boot_source, &kernel_path, &self.work_dir, self.fsync)
                .await
                .map_err(BootFailure::from)?;
        let init_time = init_started_at.elapsed();

        bind_cold_runtime_after_boot(cold_runtime.as_mut(), &self.work_dir, self.fsync)?;

        let mut context = ScenarioContext {
            nockapp,
            archive_cache,
        };

        let replay_wire = sol_replay_wire();
        let mut results = QuickOrchestrateResults {
            boot_source: boot_source_input,
            kernel_path,
            fsync: self.fsync,
            init_time,
            steps: Vec::with_capacity(steps.len()),
            failed_step_index: None,
            final_tip: None,
        };

        for (index, step) in steps.iter().enumerate() {
            let step_result =
                execute_step(&mut context, step, &replay_wire, cold_runtime.as_mut()).await;
            let failed = matches!(step_result.outcome, StepOutcome::Error);
            results.steps.push(step_result);
            if failed {
                results.failed_step_index = Some(index);
                break;
            }
        }

        results.final_tip = query_final_tip(&mut context.nockapp).await;
        Ok(results)
    }
}

struct ScenarioContext {
    nockapp: NockApp,
    archive_cache: HashMap<PathBuf, SolArchiveReader>,
}

struct PreparedPlan {
    boot_source: ResolvedBootSource,
    boot_source_input: BootSourceInput,
    kernel_path: PathBuf,
    steps: Vec<PreparedStep>,
    archive_cache: HashMap<PathBuf, SolArchiveReader>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
enum PreparedStep {
    PokeArchiveBlock {
        label: String,
        height: u64,
        archive_path: PathBuf,
    },
    PeekHeight {
        label: String,
        height: u64,
    },
    ForceCold {
        label: String,
        options: PreparedColdStepOptions,
    },
    PeekHeightCold {
        label: String,
        height: u64,
        options: PreparedColdStepOptions,
    },
}

impl PreparedStep {
    fn requires_cold_runtime(&self) -> bool {
        self.is_force_cold() || matches!(self, Self::PeekHeightCold { .. })
    }

    fn is_force_cold(&self) -> bool {
        matches!(self, Self::ForceCold { .. })
    }

    fn warm_peek_label(&self) -> Option<&str> {
        match self {
            Self::PeekHeight { label, .. } => Some(label),
            _ => None,
        }
    }

    #[cfg(test)]
    fn label(&self) -> &str {
        match self {
            Self::PokeArchiveBlock { label, .. }
            | Self::PeekHeight { label, .. }
            | Self::ForceCold { label, .. }
            | Self::PeekHeightCold { label, .. } => label,
        }
    }
}

#[derive(Debug, Error)]
pub enum PlanValidationError {
    #[error("failed to read quick-orchestrate plan {path}: {source}")]
    ReadPlan {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse quick-orchestrate plan {path}: {source}")]
    ParsePlan {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("failed to resolve current working directory: {0}")]
    CurrentDir(#[source] std::io::Error),

    #[error("checkpoint path does not exist: {path}")]
    MissingCheckpoint { path: PathBuf },

    #[error("snapshot PMA path does not exist: {path}")]
    MissingSnapshotPma { path: PathBuf },

    #[error("snapshot manifest path does not exist: {path}")]
    MissingSnapshotManifest { path: PathBuf },

    #[error("kernel path does not exist: {path}")]
    MissingKernel { path: PathBuf },

    #[error("archive path does not exist: {path}")]
    MissingArchive { path: PathBuf },

    #[error("failed to canonicalize {kind} path {path}: {source}")]
    Canonicalize {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse archive {path}: {source}")]
    ArchiveParse {
        path: PathBuf,
        #[source]
        source: ArchiveError,
    },

    #[error("quick-orchestrate step {step_type} at index {index} requires PMA replay cold-runtime support")]
    #[allow(dead_code)]
    ColdStepRequiresPmaRuntimeCompat {
        index: usize,
        step_type: &'static str,
    },

    #[error(
        "quick-orchestrate peek_height step at index {index} with label {label:?} is marked cold via the case-insensitive cold- prefix but is not preceded by a qualifying force_cold step"
    )]
    ColdLabeledPeekNotAdjacent { index: usize, label: String },

    #[error("failed to resolve quick-orchestrate boot source: {0}")]
    BootSource(#[from] BootSourceError),
}

#[derive(Debug, Error)]
pub enum BootFailure {
    #[error("failed to create work dir {path}: {source}")]
    WorkDirCreate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to initialize cold runtime: {0}")]
    ColdInit(#[from] crate::speed_of_light::cold_peek::ColdInitError),

    #[error("failed to load checkpoint: {0}")]
    CheckpointLoad(#[from] CheckpointLoadError),

    #[error("failed to initialize boot-source-backed kernel: {0}")]
    KernelInit(#[from] KernelInitError),
}

impl From<BootSourceBackedInitError> for BootFailure {
    fn from(value: BootSourceBackedInitError) -> Self {
        match value {
            BootSourceBackedInitError::CheckpointLoad(source) => Self::CheckpointLoad(source),
            BootSourceBackedInitError::KernelInit(source) => Self::KernelInit(source),
        }
    }
}

fn startup_cold_runtime(
    has_cold_steps: bool,
    cold_mode: ColdMode,
) -> Result<Option<OrchestratorColdRuntime>, BootFailure> {
    crate::speed_of_light::cold_peek::ColdRuntime::startup_if_needed(has_cold_steps, cold_mode)
        .map_err(BootFailure::from)
}

fn bind_cold_runtime_after_boot(
    cold_runtime: Option<&mut OrchestratorColdRuntime>,
    work_dir: &Path,
    fsync: bool,
) -> Result<(), BootFailure> {
    if let Some(cold_runtime) = cold_runtime {
        cold_runtime
            .bind_after_boot(work_dir, fsync)
            .map_err(BootFailure::from)?;
    }
    Ok(())
}

fn poke_archive_error_step_result(
    label: &str,
    archive_path: &Path,
    height: u64,
    fallback_duration: Duration,
    source: PokeStepError,
) -> StepResult {
    let timings = source.archive_poke_timings();
    let duration = timings
        .map(|timings| timings.total_duration)
        .unwrap_or(fallback_duration);
    let result = StepResult::error(
        label.to_string(),
        StepType::PokeArchiveBlock,
        Some(height),
        duration,
        StepExecutionError::Poke {
            path: archive_path.to_path_buf(),
            height,
            source,
        }
        .to_string(),
    );
    if let Some(timings) = timings {
        result.with_archive_poke_timings(timings)
    } else {
        result
    }
}

#[derive(Debug, Error)]
enum StepExecutionError {
    #[error("block not found in archive at height {height}: {path}")]
    ArchiveMissing { path: PathBuf, height: u64 },

    #[error("failed to replay archive block from {path} at height {height}: {source}")]
    Poke {
        path: PathBuf,
        height: u64,
        #[source]
        source: PokeStepError,
    },

    #[error("failed to peek height {height}: {source}")]
    Peek {
        height: u64,
        #[source]
        source: nockapp::nockapp::NockAppError,
    },
}

#[derive(Debug, Error)]
pub enum PreRunError {
    #[error(transparent)]
    Plan(#[from] PlanValidationError),

    #[error(transparent)]
    Boot(#[from] BootFailure),
}

fn load_and_validate_plan(plan_path: &Path) -> Result<PreparedPlan, PlanValidationError> {
    let bytes = std::fs::read(plan_path).map_err(|source| PlanValidationError::ReadPlan {
        path: plan_path.to_path_buf(),
        source,
    })?;
    let plan: QuickOrchestratePlan =
        serde_json::from_slice(&bytes).map_err(|source| PlanValidationError::ParsePlan {
            path: plan_path.to_path_buf(),
            source,
        })?;

    let boot_input = resolve_boot_source_paths(plan.boot)?;
    let boot_source = boot_input.clone().resolve()?;
    let kernel_path = resolve_existing_path(&plan.kernel, "kernel")?;
    let mut archive_cache = HashMap::new();
    let mut steps = Vec::with_capacity(plan.steps.len());
    for (index, step) in plan.steps.into_iter().enumerate() {
        match step {
            QuickOrchestrateStep::PokeArchiveBlock {
                archive,
                height,
                label,
            } => {
                let archive_path = resolve_existing_path(&archive, "archive")?;
                if !archive_cache.contains_key(&archive_path) {
                    let reader = SolArchiveReader::from_file(&archive_path).map_err(|source| {
                        PlanValidationError::ArchiveParse {
                            path: archive_path.clone(),
                            source,
                        }
                    })?;
                    archive_cache.insert(archive_path.clone(), reader);
                }
                steps.push(PreparedStep::PokeArchiveBlock {
                    label: label.unwrap_or_else(|| format!("step-{index}")),
                    height,
                    archive_path,
                });
            }
            QuickOrchestrateStep::PeekHeight { height, label } => {
                steps.push(PreparedStep::PeekHeight {
                    label: label.unwrap_or_else(|| format!("step-{index}")),
                    height,
                });
            }
            QuickOrchestrateStep::ForceCold {
                label,
                tolerance_pages,
                max_attempts,
            } => {
                steps.push(PreparedStep::ForceCold {
                    label: label.unwrap_or_else(|| format!("step-{index}")),
                    options: PreparedColdStepOptions {
                        tolerance_pages: tolerance_pages.unwrap_or(0),
                        max_attempts: max_attempts.unwrap_or(3),
                    },
                });
            }
            QuickOrchestrateStep::PeekHeightCold {
                height,
                label,
                tolerance_pages,
                max_attempts,
            } => {
                steps.push(PreparedStep::PeekHeightCold {
                    label: label.unwrap_or_else(|| format!("step-{index}")),
                    height,
                    options: PreparedColdStepOptions {
                        tolerance_pages: tolerance_pages.unwrap_or(0),
                        max_attempts: max_attempts.unwrap_or(3),
                    },
                });
            }
        }
    }

    let warnings = validate_cold_step_plan(&steps)?;

    Ok(PreparedPlan {
        boot_source,
        boot_source_input: boot_input,
        kernel_path,
        steps,
        archive_cache,
        warnings,
    })
}

fn resolve_boot_source_paths(
    boot: BootSourceInput,
) -> Result<BootSourceInput, PlanValidationError> {
    match boot {
        BootSourceInput::Checkpoint { checkpoint } => {
            let checkpoint = resolve_existing_path(&checkpoint, "checkpoint")?;
            Ok(BootSourceInput::Checkpoint { checkpoint })
        }
        BootSourceInput::Snapshot { pma, manifest } => {
            let pma = resolve_existing_path(&pma, "snapshot PMA")?;
            let manifest = resolve_existing_path(&manifest, "snapshot manifest")?;
            Ok(BootSourceInput::Snapshot { pma, manifest })
        }
    }
}

fn validate_cold_step_plan(steps: &[PreparedStep]) -> Result<Vec<String>, PlanValidationError> {
    for (index, step) in steps.iter().enumerate() {
        if let Some(label) = step.warm_peek_label() {
            if is_cold_peek_label(label) && !has_prior_force_cold_context(steps, index) {
                return Err(PlanValidationError::ColdLabeledPeekNotAdjacent {
                    index,
                    label: label.to_string(),
                });
            }
        }
    }
    Ok(Vec::new())
}

fn has_prior_force_cold_context(steps: &[PreparedStep], peek_index: usize) -> bool {
    for step in steps[..peek_index].iter().rev() {
        match step {
            PreparedStep::ForceCold { .. } | PreparedStep::PeekHeightCold { .. } => return true,
            PreparedStep::PokeArchiveBlock { .. } => return false,
            PreparedStep::PeekHeight { .. } => {}
        }
    }
    false
}

fn is_cold_peek_label(label: &str) -> bool {
    label
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cold-"))
}

fn resolve_existing_path(path: &Path, kind: &'static str) -> Result<PathBuf, PlanValidationError> {
    let current_dir = std::env::current_dir().map_err(PlanValidationError::CurrentDir)?;
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };

    if !resolved.exists() {
        return Err(match kind {
            "checkpoint" => PlanValidationError::MissingCheckpoint { path: resolved },
            "snapshot PMA" => PlanValidationError::MissingSnapshotPma { path: resolved },
            "snapshot manifest" => PlanValidationError::MissingSnapshotManifest { path: resolved },
            "kernel" => PlanValidationError::MissingKernel { path: resolved },
            _ => PlanValidationError::MissingArchive { path: resolved },
        });
    }

    resolved
        .canonicalize()
        .map_err(|source| PlanValidationError::Canonicalize {
            kind,
            path: resolved,
            source,
        })
}

async fn execute_step(
    context: &mut ScenarioContext,
    step: &PreparedStep,
    replay_wire: &nockapp::nockapp::wire::WireRepr,
    cold_runtime: Option<&mut OrchestratorColdRuntime>,
) -> StepResult {
    match step {
        PreparedStep::PokeArchiveBlock {
            label,
            height,
            archive_path,
        } => execute_poke_step(context, label, *height, archive_path, replay_wire).await,
        PreparedStep::PeekHeight { label, height } => {
            execute_peek_step(context, label, *height).await
        }
        PreparedStep::ForceCold { label, options } => {
            execute_force_cold_step(label, StepType::ForceCold, None, cold_runtime, *options)
        }
        PreparedStep::PeekHeightCold {
            label,
            height,
            options,
        } => execute_cold_peek_step(context, label, *height, cold_runtime, *options).await,
    }
}

fn execute_force_cold_step(
    label: &str,
    step_type: StepType,
    height: Option<u64>,
    cold_runtime: Option<&mut OrchestratorColdRuntime>,
    options: PreparedColdStepOptions,
) -> StepResult {
    let cold_runtime = cold_runtime.expect("validated cold plan requires initialized cold runtime");
    let (cold_result, measurement) = measure_sync(|| {
        cold_runtime.force_cold(crate::speed_of_light::cold_peek::ColdStepOptions {
            tolerance_pages: options.tolerance_pages,
            max_attempts: options.max_attempts,
        })
    });
    finalize_force_cold_step(label, step_type, height, cold_result, measurement)
}

fn finalize_force_cold_step(
    label: &str,
    step_type: StepType,
    height: Option<u64>,
    cold_result: Result<
        crate::speed_of_light::cold_peek::ColdForceResult,
        crate::speed_of_light::cold_peek::ColdStepError,
    >,
    measurement: StepMeasurement,
) -> StepResult {
    match cold_result {
        Ok(cold) => StepResult::ok(label.to_string(), step_type, height, measurement.duration)
            .with_step_measurement(measurement)
            .with_cold_force_result(cold),
        Err(
            error @ crate::speed_of_light::cold_peek::ColdStepError::VerifyFailed {
                cold_target,
                residency_pages_after,
                residency_total_pages,
                cold_attempts,
                ..
            },
        ) => StepResult::error(
            label.to_string(),
            step_type,
            height,
            measurement.duration,
            error.to_string(),
        )
        .with_step_measurement(measurement)
        .with_cold_verify_failure(
            cold_target, residency_pages_after, residency_total_pages, cold_attempts,
        ),
        Err(error) => StepResult::error(
            label.to_string(),
            step_type,
            height,
            measurement.duration,
            error.to_string(),
        )
        .with_step_measurement(measurement),
    }
}

fn finalize_cold_peek_step(
    label: &str,
    height: u64,
    cold: crate::speed_of_light::cold_peek::ColdForceResult,
    cold_force_duration: Duration,
    measurement: StepMeasurement,
    outcome: StepOutcome,
) -> StepResult {
    StepResult::with_outcome(
        label.to_string(),
        StepType::PeekHeightCold,
        Some(height),
        outcome,
        measurement.duration,
    )
    .with_step_measurement(measurement)
    .with_cold_force_result(cold)
    .with_cold_force_duration(cold_force_duration)
    .with_peek_result(outcome)
}

async fn execute_poke_step(
    context: &mut ScenarioContext,
    label: &str,
    height: u64,
    archive_path: &Path,
    replay_wire: &nockapp::nockapp::wire::WireRepr,
) -> StepResult {
    let started_at = Instant::now();
    let (nockapp, archive_cache) = (&mut context.nockapp, &context.archive_cache);
    let reader = archive_cache
        .get(archive_path)
        .expect("validated archive should be cached");

    let entry = match reader.get_entry_by_height(SolHeight(height)) {
        Some(entry) => entry,
        None => {
            return StepResult::error(
                label.to_string(),
                StepType::PokeArchiveBlock,
                Some(height),
                started_at.elapsed(),
                StepExecutionError::ArchiveMissing {
                    path: archive_path.to_path_buf(),
                    height,
                }
                .to_string(),
            );
        }
    };

    match poke_archive_block(nockapp, replay_wire.clone(), reader, entry).await {
        Ok(timings) => StepResult::ok(
            label.to_string(),
            StepType::PokeArchiveBlock,
            Some(height),
            timings.total_duration,
        )
        .with_archive_poke_timings(timings),
        Err(source) => poke_archive_error_step_result(
            label,
            archive_path,
            height,
            started_at.elapsed(),
            source,
        ),
    }
}

async fn execute_peek_step(context: &mut ScenarioContext, label: &str, height: u64) -> StepResult {
    let started_at = Instant::now();

    match measure_peek(&mut context.nockapp, height).await {
        Ok(measurement) => {
            let outcome = match measurement.sample.kind {
                PeekResultKind::Success => StepOutcome::Success,
                PeekResultKind::Missing => StepOutcome::Missing,
            };
            StepResult::with_outcome(
                label.to_string(),
                StepType::PeekHeight,
                Some(height),
                outcome,
                measurement.measurement.duration,
            )
            .with_step_measurement(measurement.measurement)
        }
        Err(source) => StepResult::error(
            label.to_string(),
            StepType::PeekHeight,
            Some(height),
            started_at.elapsed(),
            StepExecutionError::Peek { height, source }.to_string(),
        ),
    }
}

async fn execute_cold_peek_step(
    context: &mut ScenarioContext,
    label: &str,
    height: u64,
    cold_runtime: Option<&mut OrchestratorColdRuntime>,
    options: PreparedColdStepOptions,
) -> StepResult {
    let cold_runtime = cold_runtime.expect("validated cold plan requires initialized cold runtime");
    let (cold_result, cold_measurement) = measure_sync(|| {
        cold_runtime.force_cold(crate::speed_of_light::cold_peek::ColdStepOptions {
            tolerance_pages: options.tolerance_pages,
            max_attempts: options.max_attempts,
        })
    });
    let cold = match cold_result {
        Ok(cold) => cold,
        Err(error) => {
            return finalize_force_cold_step(
                label,
                StepType::PeekHeightCold,
                Some(height),
                Err(error),
                cold_measurement,
            );
        }
    };

    let started_at = Instant::now();
    match measure_peek(&mut context.nockapp, height).await {
        Ok(measurement) => {
            let outcome = match measurement.sample.kind {
                PeekResultKind::Success => StepOutcome::Success,
                PeekResultKind::Missing => StepOutcome::Missing,
            };
            finalize_cold_peek_step(
                label, height, cold, cold_measurement.duration, measurement.measurement, outcome,
            )
        }
        Err(source) => StepResult::error(
            label.to_string(),
            StepType::PeekHeightCold,
            Some(height),
            started_at.elapsed(),
            StepExecutionError::Peek { height, source }.to_string(),
        )
        .with_cold_force_result(cold),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FaultCounters {
    minflt: u64,
    majflt: u64,
}

fn measure_sync<T, E>(operation: impl FnOnce() -> Result<T, E>) -> (Result<T, E>, StepMeasurement) {
    let before = getrusage_self();
    let started_at = Instant::now();
    let result = operation();
    let measurement = finish_measurement(before, started_at);
    (result, measurement)
}

async fn measure_peek(
    nockapp: &mut NockApp,
    height: u64,
) -> Result<PeekMeasurement, nockapp::nockapp::NockAppError> {
    let before = getrusage_self();
    let started_at = Instant::now();
    let sample = super::peek_bench::peek_height_result(nockapp, height).await?;
    let measurement = finish_measurement(before, started_at);

    Ok(PeekMeasurement {
        sample,
        measurement,
    })
}

fn finish_measurement(before: Option<FaultCounters>, started_at: Instant) -> StepMeasurement {
    let duration = started_at.elapsed();
    let after = getrusage_self();
    let (minflt_delta, majflt_delta) = match (before, after) {
        (Some(before), Some(after)) => (
            Some(after.minflt.saturating_sub(before.minflt)),
            Some(after.majflt.saturating_sub(before.majflt)),
        ),
        _ => (None, None),
    };

    StepMeasurement {
        duration,
        minflt_delta,
        majflt_delta,
    }
}

fn getrusage_self() -> Option<FaultCounters> {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }

    let usage = unsafe { usage.assume_init() };
    Some(FaultCounters {
        minflt: usage.ru_minflt as u64,
        majflt: usage.ru_majflt as u64,
    })
}

async fn query_final_tip(nockapp: &mut NockApp) -> Option<FinalTip> {
    match peek_heaviest_chain_or_block(nockapp).await {
        Ok(Some((height, hash))) => Some(FinalTip {
            height: height.0 .0,
            hash: hash.to_base58(),
        }),
        Ok(None) | Err(_) => None,
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use bytes::Bytes;
    use nockapp::nockapp::save::JammedCheckpointV2;
    use nockapp::JammedNoun;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::speed_of_light::archive::SolArchiveWriter;
    use crate::speed_of_light::checkpoint::load_checkpoint;
    use crate::speed_of_light::kernel_utils::{init_nockapp, peek_heaviest_chain_or_block};
    use crate::speed_of_light::types::{ProofVersion, SolHeight};

    fn checkpoint_boot(path: impl serde::Serialize) -> serde_json::Value {
        json!({ "type": "checkpoint", "checkpoint": path })
    }

    fn write_checkpoint(path: &Path, event_num: u64) {
        let checkpoint = JammedCheckpointV2::new(
            blake3::hash(b"kernel"),
            event_num,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        std::fs::write(path, checkpoint.encode().expect("encode checkpoint"))
            .expect("write checkpoint");
    }

    #[test]
    fn quick_orchestrate_plan_json_deserializes_mvp_schema() {
        let plan: QuickOrchestratePlan = serde_json::from_value(json!({
            "boot": checkpoint_boot("/tmp/0.chkjam"),
            "kernel": "/tmp/dumb.jam",
            "steps": [
                {
                    "type": "poke_archive_block",
                    "archive": "/tmp/blocks.solarch",
                    "height": 7,
                    "label": "poke-one"
                },
                {
                    "type": "peek_height",
                    "height": 7,
                    "label": "peek-one"
                }
            ]
        }))
        .expect("plan should deserialize");

        assert_eq!(plan.steps.len(), 2);
    }

    #[test]
    fn missing_step_labels_default_to_step_indexes() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());
        let archive = write_parseable_archive(temp_dir.path(), "blocks.solarch");

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "poke_archive_block",
                        "archive": archive,
                        "height": 1
                    },
                    {
                        "type": "peek_height",
                        "height": 1
                    }
                ]
            }),
        );

        let validated = load_and_validate_plan(&plan_path).expect("validation");
        assert_eq!(validated.steps[0].label(), "step-0");
        assert_eq!(validated.steps[1].label(), "step-1");
    }

    #[test]
    fn unknown_step_type_fails_validation() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "not-a-real-step",
                        "height": 1
                    }
                ]
            }),
        );

        let error = load_and_validate_plan(&plan_path)
            .err()
            .expect("validation should fail");
        assert!(error.to_string().contains("not-a-real-step"));
    }

    #[test]
    fn missing_required_fields_fail_validation() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "poke_archive_block",
                        "height": 1
                    }
                ]
            }),
        );

        let error = load_and_validate_plan(&plan_path)
            .err()
            .expect("validation should fail");
        assert!(error.to_string().contains("archive"));
    }

    #[test]
    fn validation_eagerly_parses_archives() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());
        let archive = temp_dir.path().join("broken.solarch");
        std::fs::write(&archive, "not-an-archive").expect("archive");

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "poke_archive_block",
                        "archive": archive,
                        "height": 1
                    }
                ]
            }),
        );

        let error = load_and_validate_plan(&plan_path)
            .err()
            .expect("archive should be parsed eagerly");
        assert!(error.to_string().contains("archive"));
    }

    #[test]
    fn step_outcome_serializes_to_lowercase_strings() {
        assert_eq!(
            serde_json::to_string(&StepOutcome::Ok).expect("serialize"),
            "\"ok\""
        );
        assert_eq!(
            serde_json::to_string(&StepOutcome::Success).expect("serialize"),
            "\"success\""
        );
        assert_eq!(
            serde_json::to_string(&StepOutcome::Missing).expect("serialize"),
            "\"missing\""
        );
        assert_eq!(
            serde_json::to_string(&StepOutcome::Error).expect("serialize"),
            "\"error\""
        );
    }

    #[test]
    fn quick_orchestrate_step_json_uses_type_duration_ms_and_error_fields() {
        let value = serde_json::to_value(StepResult {
            label: "poke-one".to_string(),
            step_type: StepType::PokeArchiveBlock,
            height: Some(7),
            outcome: StepOutcome::Error,
            duration: Duration::from_micros(12_345),
            error_message: Some("no block".to_string()),
            minflt_delta: None,
            majflt_delta: None,
            cold_verified: None,
            cold_force_duration: None,
            residency_pages_after: None,
            residency_total_pages: None,
            cold_attempts: None,
            degraded_reason: None,
            cold_target: None,
            cold_evidence: None,
            peek_completed: None,
            peek_outcome: None,
            raw_tx_pokes_completed: None,
            block_poke_duration: None,
            raw_tx_poke_duration: None,
            slab_prebuild_duration: None,
            block_slab_prebuild_duration: None,
            raw_tx_slab_prebuild_duration: None,
            raw_tx_slabs_prebuilt: None,
            raw_tx_payload_bytes_prebuilt: None,
            slab_prebuild_start_rss_bytes: None,
            slab_prebuild_peak_rss_bytes: None,
        })
        .expect("serialize step");

        assert_eq!(value["label"], json!("poke-one"));
        assert_eq!(value["type"], json!("poke_archive_block"));
        assert_eq!(value["height"], json!(7));
        assert_eq!(value["outcome"], json!("error"));
        assert!(value["duration_ms"].is_number());
        assert_eq!(value["error"], json!("no block"));
        assert!(value.get("step_type").is_none());
        assert!(value.get("error_message").is_none());
    }

    #[test]
    fn quick_orchestrate_step_json_exposes_archive_prebuild_metrics() {
        let value = serde_json::to_value(
            StepResult::ok(
                "poke-one".to_string(),
                StepType::PokeArchiveBlock,
                Some(7),
                Duration::from_millis(10),
            )
            .with_archive_poke_timings(
                crate::speed_of_light::poke::ArchivePokeTimings {
                    block_duration: Duration::from_millis(4),
                    raw_tx_duration: Duration::from_millis(6),
                    total_duration: Duration::from_millis(10),
                    slab_prebuild_duration: Duration::from_millis(3),
                    block_slab_prebuild_duration: Duration::from_millis(1),
                    raw_tx_slab_prebuild_duration: Duration::from_millis(2),
                    slab_prebuild_start_rss_bytes: Some(1_000),
                    slab_prebuild_peak_rss_bytes: Some(2_000),
                    raw_tx_pokes_completed: 2,
                    raw_tx_slabs_prebuilt: 2,
                    raw_tx_payload_bytes_prebuilt: 128,
                },
            ),
        )
        .expect("serialize step");

        assert_eq!(value["block_poke_duration_ms"], json!(4.0));
        assert_eq!(value["raw_tx_poke_duration_ms"], json!(6.0));
        assert_eq!(value["slab_prebuild_duration_ms"], json!(3.0));
        assert_eq!(value["block_slab_prebuild_duration_ms"], json!(1.0));
        assert_eq!(value["raw_tx_slab_prebuild_duration_ms"], json!(2.0));
        assert_eq!(value["raw_tx_slabs_prebuilt"], json!(2));
        assert_eq!(value["raw_tx_payload_bytes_prebuilt"], json!(128));
        assert_eq!(value["slab_prebuild_start_rss_bytes"], json!(1_000));
        assert_eq!(value["slab_prebuild_peak_rss_bytes"], json!(2_000));
    }

    #[test]
    fn poke_archive_error_step_result_preserves_timed_failure_metrics() {
        let timings = crate::speed_of_light::poke::ArchivePokeTimings {
            block_duration: Duration::from_millis(14),
            raw_tx_duration: Duration::from_millis(8),
            total_duration: Duration::from_millis(22),
            slab_prebuild_duration: Duration::from_millis(12),
            block_slab_prebuild_duration: Duration::from_millis(4),
            raw_tx_slab_prebuild_duration: Duration::from_millis(8),
            slab_prebuild_start_rss_bytes: Some(1_000),
            slab_prebuild_peak_rss_bytes: Some(2_000),
            raw_tx_pokes_completed: 0,
            raw_tx_slabs_prebuilt: 2,
            raw_tx_payload_bytes_prebuilt: 128,
        };
        let result = poke_archive_error_step_result(
            "poke-one",
            Path::new("/tmp/archive.solarch"),
            7,
            Duration::from_secs(999),
            PokeStepError::ArchivePoke {
                source: nockapp::nockapp::NockAppError::PokeFailed,
                timings,
            },
        );

        assert_eq!(result.duration, timings.total_duration);
        assert_eq!(result.raw_tx_pokes_completed, Some(0));
        assert_eq!(result.block_poke_duration, Some(timings.block_duration));
        assert_eq!(result.raw_tx_poke_duration, Some(timings.raw_tx_duration));
        assert_eq!(
            result.slab_prebuild_duration,
            Some(timings.slab_prebuild_duration)
        );
        assert_eq!(
            result.block_slab_prebuild_duration,
            Some(timings.block_slab_prebuild_duration)
        );
        assert_eq!(
            result.raw_tx_slab_prebuild_duration,
            Some(timings.raw_tx_slab_prebuild_duration)
        );
        assert_eq!(result.raw_tx_slabs_prebuilt, Some(2));
        assert_eq!(result.raw_tx_payload_bytes_prebuilt, Some(128));
        assert_eq!(result.slab_prebuild_start_rss_bytes, Some(1_000));
        assert_eq!(result.slab_prebuild_peak_rss_bytes, Some(2_000));
        assert!(result
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("failed to replay archive block")));
    }

    #[test]
    fn quick_orchestrate_step_json_handles_optional_cold_fields_and_optional_height() {
        let force_cold = serde_json::to_value(StepResult {
            label: "cold-prep".to_string(),
            step_type: StepType::ForceCold,
            height: None,
            outcome: StepOutcome::Ok,
            duration: Duration::from_millis(2),
            error_message: None,
            minflt_delta: Some(11),
            majflt_delta: Some(1),
            cold_verified: Some(false),
            cold_force_duration: None,
            residency_pages_after: Some(7),
            residency_total_pages: Some(100),
            cold_attempts: Some(3),
            degraded_reason: Some("macos_unsupported".to_string()),
            cold_target: Some(crate::speed_of_light::cold_peek::ColdTargetKind::Unsupported),
            cold_evidence: None,
            peek_completed: None,
            peek_outcome: None,
            raw_tx_pokes_completed: None,
            block_poke_duration: None,
            raw_tx_poke_duration: None,
            slab_prebuild_duration: None,
            block_slab_prebuild_duration: None,
            raw_tx_slab_prebuild_duration: None,
            raw_tx_slabs_prebuilt: None,
            raw_tx_payload_bytes_prebuilt: None,
            slab_prebuild_start_rss_bytes: None,
            slab_prebuild_peak_rss_bytes: None,
        })
        .expect("serialize force cold");
        let cold_peek = serde_json::to_value(StepResult {
            label: "cold-peek".to_string(),
            step_type: StepType::PeekHeightCold,
            height: Some(7),
            outcome: StepOutcome::Success,
            duration: Duration::from_millis(3),
            error_message: None,
            minflt_delta: Some(22),
            majflt_delta: Some(0),
            cold_verified: Some(true),
            cold_force_duration: None,
            residency_pages_after: Some(0),
            residency_total_pages: Some(100),
            cold_attempts: Some(1),
            degraded_reason: None,
            cold_target: Some(crate::speed_of_light::cold_peek::ColdTargetKind::NockStack),
            cold_evidence: None,
            peek_completed: None,
            peek_outcome: None,
            raw_tx_pokes_completed: None,
            block_poke_duration: None,
            raw_tx_poke_duration: None,
            slab_prebuild_duration: None,
            block_slab_prebuild_duration: None,
            raw_tx_slab_prebuild_duration: None,
            raw_tx_slabs_prebuilt: None,
            raw_tx_payload_bytes_prebuilt: None,
            slab_prebuild_start_rss_bytes: None,
            slab_prebuild_peak_rss_bytes: None,
        })
        .expect("serialize cold peek");

        assert!(force_cold.get("height").is_none());
        assert_eq!(force_cold["type"], json!("force_cold"));
        assert_eq!(force_cold["cold_verified"], json!(false));
        assert_eq!(force_cold["degraded_reason"], json!("macos_unsupported"));
        assert_eq!(force_cold["cold_target"], json!("unsupported"));
        assert_eq!(cold_peek["height"], json!(7));
        assert_eq!(cold_peek["type"], json!("peek_height_cold"));
        assert_eq!(cold_peek["cold_target"], json!("nockstack"));
    }

    #[test]
    fn strict_force_cold_failure_keeps_measurement_and_residency_metadata() {
        let step = finalize_force_cold_step(
            "cold-prep",
            StepType::ForceCold,
            None,
            Err(crate::speed_of_light::cold_peek::ColdStepError::VerifyFailed {
                cold_target: crate::speed_of_light::cold_peek::ColdTargetKind::PmaReplay,
                residency_pages_after: 4,
                residency_total_pages: 16,
                tolerance_pages: 0,
                cold_attempts: 3,
                offending_vma: Some(crate::speed_of_light::cold_peek::OffendingVmaResidency {
                    path: PathBuf::from("/tmp/replay-pma/slab-0.bin"),
                    resident_pages: 4,
                    total_pages: 16,
                }),
                message: "offending_vma=/tmp/replay-pma/slab-0.bin resident_pages=4/16; resident_pages_after=4/16 exceeded tolerance_pages=0".to_string(),
            }),
            StepMeasurement {
                duration: Duration::from_millis(7),
                minflt_delta: Some(11),
                majflt_delta: Some(2),
            },
        );

        assert_eq!(step.outcome, StepOutcome::Error);
        assert_eq!(step.duration, Duration::from_millis(7));
        assert_eq!(step.minflt_delta, Some(11));
        assert_eq!(step.majflt_delta, Some(2));
        assert_eq!(step.cold_verified, Some(false));
        assert_eq!(step.residency_pages_after, Some(4));
        assert_eq!(step.residency_total_pages, Some(16));
        assert_eq!(step.cold_attempts, Some(3));
        assert_eq!(
            step.cold_target,
            Some(crate::speed_of_light::cold_peek::ColdTargetKind::PmaReplay)
        );
        assert!(step
            .error_message
            .as_deref()
            .expect("error message")
            .contains("/tmp/replay-pma/slab-0.bin"));
    }

    #[test]
    fn prepared_steps_identify_when_cold_runtime_is_required() {
        assert!(!PreparedStep::PeekHeight {
            label: "warm".to_string(),
            height: 7,
        }
        .requires_cold_runtime());
        assert!(PreparedStep::ForceCold {
            label: "cold-prep".to_string(),
            options: PreparedColdStepOptions {
                tolerance_pages: 0,
                max_attempts: 3,
            },
        }
        .requires_cold_runtime());
    }

    #[test]
    fn cold_labeled_peek_requires_prior_force_cold_case_insensitive() {
        let steps = vec![
            PreparedStep::PeekHeight {
                label: "warm-7".to_string(),
                height: 7,
            },
            PreparedStep::PeekHeight {
                label: "CoLd-7".to_string(),
                height: 7,
            },
        ];

        let error = validate_cold_step_plan(&steps)
            .err()
            .expect("mislabeled cold peek should fail validation");
        assert!(error.to_string().contains("CoLd-7"), "{error}");
        assert!(error.to_string().contains("force_cold"), "{error}");
    }

    #[test]
    fn cold_labeled_peeks_after_force_cold_can_span_the_run() {
        let steps = vec![
            PreparedStep::ForceCold {
                label: "prep".to_string(),
                options: PreparedColdStepOptions {
                    tolerance_pages: 0,
                    max_attempts: 3,
                },
            },
            PreparedStep::PeekHeight {
                label: "cold-7".to_string(),
                height: 7,
            },
            PreparedStep::PeekHeight {
                label: "cold-8".to_string(),
                height: 8,
            },
        ];

        let warnings = validate_cold_step_plan(&steps).expect("cold run peeks are valid");
        assert!(warnings.is_empty());
    }

    #[test]
    fn non_labeled_interleaving_after_force_cold_no_longer_warns() {
        let steps = vec![
            PreparedStep::ForceCold {
                label: "prep".to_string(),
                options: PreparedColdStepOptions {
                    tolerance_pages: 0,
                    max_attempts: 3,
                },
            },
            PreparedStep::PokeArchiveBlock {
                label: "poke-7".to_string(),
                height: 7,
                archive_path: PathBuf::from("/tmp/archive.solarch"),
            },
            PreparedStep::PeekHeight {
                label: "peek-7".to_string(),
                height: 7,
            },
        ];

        let warnings = validate_cold_step_plan(&steps).expect("plan should be valid");
        assert!(warnings.is_empty());
    }

    #[test]
    fn first_step_force_cold_can_feed_a_cold_labeled_peek() {
        let steps = vec![
            PreparedStep::ForceCold {
                label: "prep".to_string(),
                options: PreparedColdStepOptions {
                    tolerance_pages: 0,
                    max_attempts: 3,
                },
            },
            PreparedStep::PeekHeight {
                label: "cold-7".to_string(),
                height: 7,
            },
        ];

        let warnings = validate_cold_step_plan(&steps).expect("adjacent cold peek is valid");
        assert!(warnings.is_empty());
    }

    #[test]
    fn terminal_force_cold_does_not_emit_an_ambiguity_warning() {
        let steps = vec![
            PreparedStep::PeekHeight {
                label: "warm-7".to_string(),
                height: 7,
            },
            PreparedStep::ForceCold {
                label: "prep".to_string(),
                options: PreparedColdStepOptions {
                    tolerance_pages: 0,
                    max_attempts: 3,
                },
            },
        ];

        let warnings = validate_cold_step_plan(&steps).expect("terminal force_cold is valid");
        assert!(warnings.is_empty());
    }

    #[test]
    fn peek_height_cold_success_uses_a_single_fused_step_result() {
        let step = finalize_cold_peek_step(
            "cold-7",
            7,
            crate::speed_of_light::cold_peek::ColdForceResult {
                cold_target: crate::speed_of_light::cold_peek::ColdTargetKind::NockStack,
                cold_verified: true,
                residency_pages_after: 0,
                residency_total_pages: 32,
                cold_attempts: 1,
                degraded_reason: None,
                evidence: crate::speed_of_light::cold_peek::ColdEvidenceDetails::default(),
            },
            Duration::from_millis(3),
            StepMeasurement {
                duration: Duration::from_millis(4),
                minflt_delta: Some(19),
                majflt_delta: Some(7),
            },
            StepOutcome::Success,
        );

        assert_eq!(step.step_type, StepType::PeekHeightCold);
        assert_eq!(step.height, Some(7));
        assert_eq!(step.outcome, StepOutcome::Success);
        assert_eq!(step.duration, Duration::from_millis(4));
        assert_eq!(step.minflt_delta, Some(19));
        assert_eq!(step.majflt_delta, Some(7));
        assert_eq!(step.cold_verified, Some(true));
        assert_eq!(step.residency_pages_after, Some(0));
        assert_eq!(step.residency_total_pages, Some(32));
        assert_eq!(step.cold_attempts, Some(1));
        assert_eq!(
            step.cold_target,
            Some(crate::speed_of_light::cold_peek::ColdTargetKind::NockStack)
        );
    }

    #[test]
    fn quick_orchestrate_fail_fast_result_json_keeps_only_executed_steps() {
        let results = QuickOrchestrateResults {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("/tmp/0.chkjam"),
            },
            kernel_path: PathBuf::from("/tmp/dumb.jam"),
            fsync: true,
            init_time: Duration::from_millis(123),
            steps: vec![
                StepResult {
                    label: "peek-one".to_string(),
                    step_type: StepType::PeekHeight,
                    height: Some(7),
                    outcome: StepOutcome::Success,
                    duration: Duration::from_millis(3),
                    error_message: None,
                    minflt_delta: None,
                    majflt_delta: None,
                    cold_verified: None,
                    cold_force_duration: None,
                    residency_pages_after: None,
                    residency_total_pages: None,
                    cold_attempts: None,
                    degraded_reason: None,
                    cold_target: None,
                    cold_evidence: None,
                    peek_completed: None,
                    peek_outcome: None,
                    raw_tx_pokes_completed: None,
                    block_poke_duration: None,
                    raw_tx_poke_duration: None,
                    slab_prebuild_duration: None,
                    block_slab_prebuild_duration: None,
                    raw_tx_slab_prebuild_duration: None,
                    raw_tx_slabs_prebuilt: None,
                    raw_tx_payload_bytes_prebuilt: None,
                    slab_prebuild_start_rss_bytes: None,
                    slab_prebuild_peak_rss_bytes: None,
                },
                StepResult {
                    label: "poke-bad".to_string(),
                    step_type: StepType::PokeArchiveBlock,
                    height: Some(99),
                    outcome: StepOutcome::Error,
                    duration: Duration::from_millis(1),
                    error_message: Some("missing".to_string()),
                    minflt_delta: None,
                    majflt_delta: None,
                    cold_verified: None,
                    cold_force_duration: None,
                    residency_pages_after: None,
                    residency_total_pages: None,
                    cold_attempts: None,
                    degraded_reason: None,
                    cold_target: None,
                    cold_evidence: None,
                    peek_completed: None,
                    peek_outcome: None,
                    raw_tx_pokes_completed: None,
                    block_poke_duration: None,
                    raw_tx_poke_duration: None,
                    slab_prebuild_duration: None,
                    block_slab_prebuild_duration: None,
                    raw_tx_slab_prebuild_duration: None,
                    raw_tx_slabs_prebuilt: None,
                    raw_tx_payload_bytes_prebuilt: None,
                    slab_prebuild_start_rss_bytes: None,
                    slab_prebuild_peak_rss_bytes: None,
                },
            ],
            failed_step_index: Some(1),
            final_tip: None,
        };

        assert!(!results.succeeded());
        let value = serde_json::from_str::<serde_json::Value>(
            &results.to_compact_json().expect("compact json"),
        )
        .expect("parse json");
        assert_eq!(value["steps"].as_array().expect("steps").len(), 2);
        assert_eq!(value["steps"][1]["error"], json!("missing"));
    }

    #[test]
    fn quick_orchestrate_results_exposes_measured_init_time_secs() {
        let results = QuickOrchestrateResults {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("/tmp/0.chkjam"),
            },
            kernel_path: PathBuf::from("/tmp/dumb.jam"),
            fsync: true,
            init_time: Duration::from_millis(123),
            steps: Vec::new(),
            failed_step_index: None,
            final_tip: None,
        };

        assert_eq!(results.init_time_secs(), 0.123);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed integration coverage; exercised in release smoke verification"]
    async fn fail_fast_runner_returns_results_after_a_successful_step() {
        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };

        let temp_dir = tempdir().expect("temp dir");
        let archive = write_parseable_archive(temp_dir.path(), "blocks.solarch");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "peek-tip"
                    },
                    {
                        "type": "poke_archive_block",
                        "archive": archive,
                        "height": tip_height + 1000,
                        "label": "poke-missing"
                    },
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "never-runs"
                    }
                ]
            }),
        );

        let Ok(Ok(results)) = tokio::time::timeout(
            Duration::from_secs(60),
            QuickOrchestrateRunner::new(
                plan_path,
                temp_dir.path().join("work"),
                true,
                ColdMode::Strict,
            )
            .run(),
        )
        .await
        else {
            return;
        };

        assert!(!results.succeeded());
        assert_eq!(results.failed_step_index, Some(1));
        assert_eq!(results.steps.len(), 2);
        assert_eq!(results.steps[0].outcome, StepOutcome::Success);
        assert_eq!(results.steps[1].outcome, StepOutcome::Error);

        let json = results.to_compact_json().expect("compact json");
        assert!(!json.contains('\n'));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed integration coverage; exercised in release smoke verification"]
    async fn peek_missing_is_nonfatal_and_serializes_as_missing() {
        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };

        let temp_dir = tempdir().expect("temp dir");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "peek_height",
                        "height": tip_height + 1
                    }
                ]
            }),
        );

        let Ok(Ok(results)) = tokio::time::timeout(
            Duration::from_secs(60),
            QuickOrchestrateRunner::new(
                plan_path,
                temp_dir.path().join("work"),
                true,
                ColdMode::Strict,
            )
            .run(),
        )
        .await
        else {
            return;
        };

        assert!(results.succeeded());
        assert_eq!(results.steps.len(), 1);
        assert_eq!(results.steps[0].outcome, StepOutcome::Missing);

        let value = serde_json::from_str::<serde_json::Value>(
            &results.to_compact_json().expect("compact json"),
        )
        .expect("parse json");
        assert_eq!(value["steps"][0]["outcome"], json!("missing"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed cold-peek smoke; requires local checkpoint/cgroup setup"]
    async fn force_cold_then_peek_records_verified_cold_metrics() {
        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };

        let temp_dir = tempdir().expect("temp dir");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "force_cold",
                        "label": "prep"
                    },
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "cold-tip"
                    }
                ]
            }),
        );

        let results = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Strict,
        )
        .run()
        .await
        .expect("runner should succeed");

        assert!(results.succeeded());
        assert_eq!(results.steps.len(), 2);
        assert_eq!(results.steps[0].step_type, StepType::ForceCold);
        assert_eq!(results.steps[0].cold_verified, Some(true));
        assert_eq!(results.steps[0].residency_pages_after, Some(0));
        assert_eq!(results.steps[1].step_type, StepType::PeekHeight);
        assert!(results.steps[1].majflt_delta.unwrap_or(0) > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed cold-peek smoke; requires local checkpoint/cgroup setup"]
    async fn warm_then_force_cold_then_peek_shows_fault_delta_contrast() {
        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };

        let temp_dir = tempdir().expect("temp dir");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "warm-tip"
                    },
                    {
                        "type": "force_cold",
                        "label": "prep"
                    },
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "cold-tip"
                    }
                ]
            }),
        );

        let results = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Strict,
        )
        .run()
        .await
        .expect("runner should succeed");

        assert!(results.succeeded());
        assert_eq!(results.steps.len(), 3);
        let warm_majflt = results.steps[0].majflt_delta.unwrap_or(0);
        let cold_majflt = results.steps[2].majflt_delta.unwrap_or(0);
        assert!(cold_majflt > 0, "expected a cold major-fault delta");
        assert!(
            cold_majflt > warm_majflt,
            "expected cold delta {cold_majflt} to exceed warm delta {warm_majflt}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed cold-peek smoke; requires local checkpoint/cgroup setup"]
    async fn peek_height_cold_sweep_verifies_all_samples() {
        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };
        if tip_height < 99 {
            return;
        }

        let start_height = tip_height - 99;
        let steps: Vec<_> = (start_height..=tip_height)
            .map(|height| {
                json!({
                    "type": "peek_height_cold",
                    "height": height,
                    "label": format!("cold-{height}")
                })
            })
            .collect();

        let temp_dir = tempdir().expect("temp dir");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": steps
            }),
        );

        let results = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Strict,
        )
        .run()
        .await
        .expect("runner should succeed");

        assert!(results.succeeded());
        assert_eq!(results.steps.len(), 100);
        assert!(results
            .steps
            .iter()
            .all(|step| step.step_type == StepType::PeekHeightCold));
        assert!(results
            .steps
            .iter()
            .all(|step| step.cold_verified == Some(true)));

        let mut majflt: Vec<u64> = results
            .steps
            .iter()
            .map(|step| step.majflt_delta.unwrap_or(0))
            .collect();
        majflt.sort_unstable();
        assert!(
            majflt[majflt.len() / 2] > 0,
            "median majflt delta should be positive"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "checkpoint-backed cold-peek smoke; requires an externally prepared residue case"]
    async fn soft_mode_residency_failure_does_not_abort_run() {
        if env::var_os("NOCKCHAIN_BENCH_RUN_SOFT_MODE_RESIDUE_TEST").is_none() {
            return;
        }

        let Some((checkpoint, kernel, tip_height)) =
            tokio::time::timeout(Duration::from_secs(60), fixture_boot_inputs())
                .await
                .ok()
                .flatten()
        else {
            return;
        };

        let temp_dir = tempdir().expect("temp dir");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "peek_height_cold",
                        "height": tip_height,
                        "label": "cold-tip"
                    },
                    {
                        "type": "peek_height",
                        "height": tip_height,
                        "label": "warm-tip"
                    }
                ]
            }),
        );

        let results = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Soft,
        )
        .run()
        .await
        .expect("soft mode should not abort");

        assert!(results.succeeded());
        assert_eq!(results.steps[0].step_type, StepType::PeekHeightCold);
        assert_eq!(results.steps[0].cold_verified, Some(false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cold_init_fails_without_delegated_memory() {
        if env::var_os("NOCKCHAIN_BENCH_RUN_COLD_INIT_NO_DELEGATED_MEMORY_TEST").is_none() {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let cgroup_parent = temp_dir.path().join("cold-init-no-memory");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());
        std::fs::create_dir_all(&cgroup_parent).expect("cgroup parent");
        std::fs::write(cgroup_parent.join("cgroup.subtree_control"), "+cpu +io")
            .expect("subtree control");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "force_cold",
                        "label": "prep"
                    }
                ]
            }),
        );
        let _override_guard = crate::speed_of_light::cold_peek::set_test_cold_init_overrides(
            Some(cgroup_parent),
            None,
        );

        let error = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Strict,
        )
        .run()
        .await
        .expect_err("startup should fail before boot");

        match error {
            PreRunError::Boot(BootFailure::ColdInit(
                crate::speed_of_light::cold_peek::ColdInitError::NoDelegatedMemory,
            )) => {}
            other => panic!("expected NoDelegatedMemory, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cold_init_reports_swappiness_key_unsupported() {
        if env::var_os("NOCKCHAIN_BENCH_RUN_COLD_INIT_SWAPPINESS_UNSUPPORTED_TEST").is_none()
            && env::var_os("NOCKCHAIN_BENCH_RUN_COLD_INIT_SWAPPINESS_UNSUPPORTED").is_none()
        {
            return;
        }

        let temp_dir = tempdir().expect("temp dir");
        let cgroup_parent = temp_dir.path().join("cold-init-swappiness");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());
        std::fs::create_dir_all(&cgroup_parent).expect("cgroup parent");
        std::fs::write(cgroup_parent.join("cgroup.subtree_control"), "+memory")
            .expect("subtree control");
        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "force_cold",
                        "label": "prep"
                    }
                ]
            }),
        );
        let _override_guard = crate::speed_of_light::cold_peek::set_test_cold_init_overrides(
            Some(cgroup_parent),
            Some(Err(
                crate::speed_of_light::cold_peek::ColdInitError::SwappinessKeyUnsupported {
                    found_kernel: "6.10.0-test".to_string(),
                },
            )),
        );

        let error = QuickOrchestrateRunner::new(
            plan_path,
            temp_dir.path().join("work"),
            true,
            ColdMode::Strict,
        )
        .run()
        .await
        .expect_err("startup should fail before boot");

        match error {
            PreRunError::Boot(BootFailure::ColdInit(
                crate::speed_of_light::cold_peek::ColdInitError::SwappinessKeyUnsupported {
                    ..
                },
            )) => {}
            other => panic!("expected SwappinessKeyUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn quick_orchestrate_plan_json_deserializes_new_cold_steps() {
        let plan: QuickOrchestratePlan = serde_json::from_value(json!({
            "boot": checkpoint_boot("/tmp/0.chkjam"),
            "kernel": "/tmp/dumb.jam",
            "steps": [
                {
                    "type": "force_cold",
                    "label": "cold-prep",
                    "tolerance_pages": 2,
                    "max_attempts": 5
                },
                {
                    "type": "peek_height_cold",
                    "height": 7,
                    "label": "cold-peek",
                    "tolerance_pages": 1,
                    "max_attempts": 4
                }
            ]
        }))
        .expect("plan should deserialize");

        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[0] {
            QuickOrchestrateStep::ForceCold {
                tolerance_pages,
                max_attempts,
                ..
            } => {
                assert_eq!(*tolerance_pages, Some(2));
                assert_eq!(*max_attempts, Some(5));
            }
            other => panic!("expected force_cold, got {other:?}"),
        }
        match &plan.steps[1] {
            QuickOrchestrateStep::PeekHeightCold {
                height,
                tolerance_pages,
                max_attempts,
                ..
            } => {
                assert_eq!(*height, 7);
                assert_eq!(*tolerance_pages, Some(1));
                assert_eq!(*max_attempts, Some(4));
            }
            other => panic!("expected peek_height_cold, got {other:?}"),
        }
    }

    #[test]
    fn quick_orchestrate_validation_accepts_force_cold_on_current_branch() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "force_cold",
                        "label": "cold-prep"
                    }
                ]
            }),
        );

        let validated = load_and_validate_plan(&plan_path).expect("validation should succeed");
        assert!(matches!(validated.steps[0], PreparedStep::ForceCold { .. }));
    }

    #[test]
    fn quick_orchestrate_validation_accepts_peek_height_cold_on_current_branch() {
        let temp_dir = tempdir().expect("temp dir");
        let (checkpoint, kernel) = write_boot_files(temp_dir.path());

        let plan_path = write_plan(
            temp_dir.path(),
            json!({
                "boot": checkpoint_boot(checkpoint),
                "kernel": kernel,
                "steps": [
                    {
                        "type": "peek_height_cold",
                        "height": 7,
                        "label": "cold-7"
                    }
                ]
            }),
        );

        let validated = load_and_validate_plan(&plan_path).expect("validation should succeed");
        assert!(matches!(
            validated.steps[0],
            PreparedStep::PeekHeightCold { .. }
        ));
    }

    fn write_plan(dir: &Path, value: serde_json::Value) -> PathBuf {
        let path = dir.join("plan.json");
        std::fs::write(&path, serde_json::to_vec(&value).expect("plan json")).expect("write plan");
        path
    }

    fn write_boot_files(dir: &Path) -> (PathBuf, PathBuf) {
        let checkpoint = dir.join("checkpoint.chkjam");
        let kernel = dir.join("kernel.jam");
        write_checkpoint(&checkpoint, 0);
        std::fs::write(&kernel, "kernel").expect("kernel");
        (checkpoint, kernel)
    }

    fn write_parseable_archive(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(1),
                dummy_hash(1),
                0,
                ProofVersion::V0,
                b"junk-jam-bytes",
            )
            .expect("add block");
        writer.write_to_file(&path).expect("write archive");
        path
    }

    async fn fixture_boot_inputs() -> Option<(PathBuf, PathBuf, u64)> {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let checkpoint = repo_root.join("checkpoints/0.chkjam");
        let kernel = repo_root.join("assets/dumb.jam");
        if !checkpoint.is_file() || !kernel.is_file() {
            return None;
        }

        let temp_dir = tempdir().expect("temp dir");
        let loaded = load_checkpoint(&checkpoint).ok()?;
        let checkpoint_state = nockapp::nockapp::save::SaveableCheckpoint {
            ker_hash: loaded.ker_hash,
            event_num: loaded.event_num,
            state: loaded.state,
            cold: loaded.cold,
        };
        let mut nockapp = init_nockapp(
            &kernel,
            Some(checkpoint_state),
            &temp_dir.path().to_path_buf(),
            false,
            true,
        )
        .await
        .ok()?;
        let (tip, _hash) = peek_heaviest_chain_or_block(&mut nockapp).await.ok()??;
        Some((checkpoint, kernel, tip.0 .0))
    }

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }
}
