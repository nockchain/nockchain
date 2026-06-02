use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ValueEnum;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;

use super::artifacts::{write_json, write_schema_version, write_verdict};
use super::case::default_fsync_enabled;
use super::docker::execute_docker_trusted_run;
use super::docker_image::{prefetch_docker_image, DockerImageSource, DockerImageVariant};
use super::native::execute_native_trusted_run;
use super::orchestrate::{prepare_output_root, TrustedRunResult};
use super::provenance::{BackendRuntimeFacts, Provenance};
use super::summary::{Validity, ValueStats, Verdict};
use super::{
    ExecutionRequest, HarnessError, RequestedCase, ResolvedCase, WorkDirMode,
    COMPARISON_SCHEMA_VERSION, VERDICT_SCHEMA_VERSION,
};
use crate::speed_of_light::orchestrate_execute::StepResultRow;
use crate::speed_of_light::{BootSourceInput, PeekMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AxisValue {
    Boolean(bool),
    Integer(i64),
    String(String),
    Object(serde_json::Map<String, Value>),
}

impl AxisValue {
    fn slug_value(&self) -> String {
        match self {
            Self::Boolean(value) => value.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::String(value) => sanitize_slug(value),
            Self::Object(value) => sanitize_slug(&Value::Object(value.clone()).to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepMatrix {
    pub base_case: RequestedCase,
    pub axes: BTreeMap<String, Vec<AxisValue>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum CpuProfilerKind {
    Samply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuProfilerConfig {
    pub kind: CpuProfilerKind,
    pub sample_rate_hz: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpandedCase {
    pub case_index: usize,
    pub case_id: String,
    pub axis_assignments: BTreeMap<String, AxisValue>,
    pub requested_case: RequestedCase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleMode {
    Sequential,
    Interleaved,
    Randomized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepSchedule {
    pub mode: ScheduleMode,
    pub case_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepRunOptions {
    pub schedule_mode: ScheduleMode,
    pub random_seed: Option<u64>,
    pub comparison_markdown: bool,
    pub allow_debug_benchmark: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_profiler: Option<CpuProfilerConfig>,
}

impl Default for SweepRunOptions {
    fn default() -> Self {
        Self {
            schedule_mode: ScheduleMode::Sequential,
            random_seed: None,
            comparison_markdown: false,
            allow_debug_benchmark: false,
            cpu_profiler: None,
        }
    }
}

#[derive(Debug)]
pub struct SweepCaseRun {
    pub expanded_case: ExpandedCase,
    pub output_root: PathBuf,
    pub result: TrustedRunResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepCaseComparison {
    pub case_id: String,
    pub axis_assignments: BTreeMap<String, AxisValue>,
    pub output_root: PathBuf,
    pub resolved_case: ResolvedCase,
    pub summary: super::summary::RunSummary,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepComparison {
    #[serde(default = "comparison_schema_version")]
    pub schema_version: String,
    #[serde(default, skip_serializing)]
    pub axis_names: Vec<String>,
    #[serde(default, skip_serializing)]
    pub case_count: usize,
    #[serde(default, skip_serializing)]
    pub cases: Vec<SweepCaseComparison>,
    #[serde(default, skip_serializing)]
    pub failed_cases: Vec<SweepCaseFailure>,
    #[serde(default, skip_serializing)]
    pub invariant_violations: Vec<String>,
    #[serde(default)]
    pub aggregate: BTreeMap<String, BTreeMap<String, ValueStats>>,
    #[serde(default)]
    pub by_step_type: BTreeMap<String, BTreeMap<String, BTreeMap<String, ValueStats>>>,
    #[serde(default)]
    pub common_steps: Vec<SweepCommonStep>,
    #[serde(default)]
    pub non_comparable_metrics: Vec<NonComparableMetric>,
    #[serde(default, skip_serializing)]
    pub backend_groups: Vec<SweepComparisonGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepComparisonGroup {
    pub backend: String,
    pub case_count: usize,
    pub cases: Vec<SweepCaseComparison>,
    #[serde(default)]
    pub failed_cases: Vec<SweepCaseFailure>,
    pub invariant_violations: Vec<String>,
    #[serde(default)]
    pub aggregate: BTreeMap<String, BTreeMap<String, ValueStats>>,
    #[serde(default)]
    pub by_step_type: BTreeMap<String, BTreeMap<String, BTreeMap<String, ValueStats>>>,
    #[serde(default)]
    pub common_steps: Vec<SweepCommonStep>,
    #[serde(default)]
    pub non_comparable_metrics: Vec<NonComparableMetric>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepCommonStep {
    pub step_id: String,
    #[serde(rename = "type")]
    pub step_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    pub per_case: BTreeMap<String, SweepCommonStepCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepCommonStepCase {
    pub duration_ms_median: Option<f64>,
    pub outcome_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonComparableMetric {
    pub metric: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepCaseFailure {
    pub case_id: String,
    pub axis_assignments: BTreeMap<String, AxisValue>,
    pub output_root: PathBuf,
    pub error: String,
}

fn comparison_schema_version() -> String {
    COMPARISON_SCHEMA_VERSION.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepResult {
    pub expanded_cases: Vec<ExpandedCase>,
    pub schedule: SweepSchedule,
    pub comparison: SweepComparison,
    pub verdict: Verdict,
}

pub trait SweepExecutor {
    fn preflight_case(
        &self,
        _requested_case: &RequestedCase,
        _cpu_profiler: Option<&CpuProfilerConfig>,
    ) -> Result<(), HarnessError> {
        Ok(())
    }

    fn execute_case<'a>(
        &'a mut self,
        requested_case: RequestedCase,
        output_root: &'a Path,
        allow_debug_benchmark: bool,
        cpu_profiler: Option<CpuProfilerConfig>,
    ) -> futures::future::BoxFuture<'a, Result<TrustedRunResult, HarnessError>>;
}

pub struct HarnessSweepExecutor;

impl SweepExecutor for HarnessSweepExecutor {
    fn preflight_case(
        &self,
        requested_case: &RequestedCase,
        cpu_profiler: Option<&CpuProfilerConfig>,
    ) -> Result<(), HarnessError> {
        let variant = match cpu_profiler {
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                ..
            }) => DockerImageVariant::Profiling,
            None => DockerImageVariant::Standard,
        };
        let ExecutionRequest::Docker { image, .. } = &requested_case.execution else {
            return Ok(());
        };
        prefetch_docker_image(image, variant)
    }

    fn execute_case<'a>(
        &'a mut self,
        requested_case: RequestedCase,
        output_root: &'a Path,
        allow_debug_benchmark: bool,
        cpu_profiler: Option<CpuProfilerConfig>,
    ) -> futures::future::BoxFuture<'a, Result<TrustedRunResult, HarnessError>> {
        async move {
            match requested_case.execution.clone() {
                ExecutionRequest::Native => execute_native_trusted_run(
                    requested_case, output_root, allow_debug_benchmark, cpu_profiler,
                )
                .await
                .map(|result| TrustedRunResult {
                    resolved: result.resolved,
                    provenance: result.provenance,
                    summary: result.summary,
                    verdict: result.verdict,
                }),
                ExecutionRequest::Docker { .. } => {
                    execute_docker_trusted_run(
                        requested_case, output_root, allow_debug_benchmark, cpu_profiler,
                    )
                    .await
                }
            }
        }
        .boxed()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SweepMatrixFile {
    Internal(SweepMatrix),
    Spec(SweepMatrixSpec),
}

impl SweepMatrixFile {
    pub fn into_matrix(self) -> Result<SweepMatrix, HarnessError> {
        match self {
            Self::Internal(matrix) => Ok(matrix),
            Self::Spec(spec) => spec.into_matrix(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweepMatrixSpec {
    pub benchmark: String,
    pub base: SweepBaseCase,
    pub axes: BTreeMap<String, Vec<AxisValue>>,
}

impl SweepMatrixSpec {
    fn into_matrix(self) -> Result<SweepMatrix, HarnessError> {
        if self.benchmark != "sol-orchestrate" {
            return Err(HarnessError::InvalidRequestedCase(format!(
                "unsupported sweep benchmark `{}`; trusted sweeps support only sol-orchestrate",
                self.benchmark
            )));
        }

        Ok(SweepMatrix {
            base_case: self.base.into_requested_case()?,
            axes: self.axes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepBaseCase {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotPair>,
    #[serde(default = "default_kernel_path")]
    pub kernel: PathBuf,
    #[serde(default)]
    pub start_height: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default)]
    pub peek_mode: PeekMode,
    #[serde(default)]
    pub blocks: u64,
    #[serde(default)]
    pub skip_genesis: bool,
    #[serde(default)]
    pub profile_memory: bool,
    #[serde(default = "default_profile_interval_ms")]
    pub profile_interval_ms: u64,
    #[serde(default = "default_fsync_enabled")]
    pub fsync: bool,
    #[serde(default = "default_threads")]
    pub threads: u32,
    #[serde(default = "default_warmup_runs")]
    pub warmup_runs: u32,
    #[serde(default = "default_measured_runs")]
    pub measured_runs: u32,
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub mode: SweepModeInput,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct SweepBaseCaseSerde {
    #[serde(default)]
    fixture: Option<PathBuf>,
    #[serde(default)]
    plan: Option<PathBuf>,
    #[serde(default)]
    checkpoint: Option<PathBuf>,
    #[serde(default)]
    snapshot: Option<SnapshotPair>,
    #[serde(default = "default_kernel_path")]
    kernel: PathBuf,
    #[serde(default)]
    start_height: u64,
    #[serde(default)]
    end_height: Option<u64>,
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    peek_mode: PeekMode,
    #[serde(default)]
    blocks: u64,
    #[serde(default)]
    skip_genesis: bool,
    #[serde(default)]
    profile_memory: bool,
    #[serde(default = "default_profile_interval_ms")]
    profile_interval_ms: u64,
    #[serde(default = "default_fsync_enabled")]
    fsync: bool,
    #[serde(default = "default_threads")]
    threads: u32,
    #[serde(default = "default_warmup_runs")]
    warmup_runs: u32,
    #[serde(default = "default_measured_runs")]
    measured_runs: u32,
    #[serde(default = "default_cooldown_secs")]
    cooldown_secs: u64,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    mode: SweepModeInput,
}

impl<'de> Deserialize<'de> for SweepBaseCase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = SweepBaseCaseSerde::deserialize(deserializer)?;

        Ok(Self {
            fixture: helper.fixture,
            plan: helper.plan,
            checkpoint: helper.checkpoint,
            snapshot: helper.snapshot,
            kernel: helper.kernel,
            start_height: helper.start_height,
            end_height: helper.end_height,
            count: helper.count,
            peek_mode: helper.peek_mode,
            blocks: helper.blocks,
            skip_genesis: helper.skip_genesis,
            profile_memory: helper.profile_memory,
            profile_interval_ms: helper.profile_interval_ms,
            fsync: helper.fsync,
            threads: helper.threads,
            warmup_runs: helper.warmup_runs,
            measured_runs: helper.measured_runs,
            cooldown_secs: helper.cooldown_secs,
            label: helper.label,
            mode: helper.mode,
        })
    }
}

impl SweepBaseCase {
    fn into_requested_case(self) -> Result<RequestedCase, HarnessError> {
        let source_count = self.fixture.is_some() as u8
            + self.plan.is_some() as u8
            + self.checkpoint.is_some() as u8
            + self.snapshot.is_some() as u8;
        if source_count != 1 {
            return Err(HarnessError::InvalidRequestedCase(
                "sweep base must specify exactly one of `fixture`, `plan`, `checkpoint`, or `snapshot`"
                    .to_string(),
            ));
        }
        let mut requested = RequestedCase::native(self.fixture.clone().unwrap_or_default());
        requested.blocks = self.blocks;
        requested.skip_genesis = self.skip_genesis;
        requested.orchestrate = if let Some(plan_path) = self.plan {
            if self.blocks != 0 || self.skip_genesis {
                return Err(HarnessError::InvalidRequestedCase(
                    "sweep plan base cannot specify replay shorthand fields".to_string(),
                ));
            }
            super::case::RequestedOrchestrate::PlanFile { plan_path }
        } else if let Some(checkpoint_path) = self.checkpoint {
            if self.blocks != 0 || self.skip_genesis {
                return Err(HarnessError::InvalidRequestedCase(
                    "sweep read base cannot specify replay shorthand fields".to_string(),
                ));
            }
            super::case::RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Checkpoint {
                    checkpoint: checkpoint_path,
                },
                kernel_path: self.kernel,
                start_height: self.start_height,
                end_height: self.end_height,
                count: self.count,
                peek_mode: self.peek_mode,
            }
        } else if let Some(snapshot) = self.snapshot {
            if self.blocks != 0 || self.skip_genesis {
                return Err(HarnessError::InvalidRequestedCase(
                    "sweep read base cannot specify replay shorthand fields".to_string(),
                ));
            }
            super::case::RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot {
                    pma: snapshot.pma,
                    manifest: snapshot.manifest,
                },
                kernel_path: self.kernel,
                start_height: self.start_height,
                end_height: self.end_height,
                count: self.count,
                peek_mode: self.peek_mode,
            }
        } else {
            super::case::RequestedOrchestrate::GeneratedReplay {
                fixture_path: requested.fixture_path.clone(),
                blocks: Some(self.blocks),
                skip_genesis: self.skip_genesis,
            }
        };
        requested.profile_memory = self.profile_memory;
        requested.profile_interval_ms = self.profile_interval_ms;
        requested.set_fsync_enabled(self.fsync);
        requested.threads = self.threads;
        requested.warmup_runs = self.warmup_runs;
        requested.measured_runs = self.measured_runs;
        requested.cooldown_secs = self.cooldown_secs;
        requested.label = self.label;
        requested.execution = self.mode.into_execution_request()?;
        Ok(requested)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPair {
    pub pma: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SweepModeInput {
    #[serde(default)]
    pub native: Option<Value>,
    #[serde(default)]
    pub docker: Option<SweepDockerModeInput>,
}

impl SweepModeInput {
    fn into_execution_request(self) -> Result<ExecutionRequest, HarnessError> {
        match (self.native.is_some(), self.docker) {
            (false, None) | (true, None) => Ok(ExecutionRequest::Native),
            (false, Some(docker)) => Ok(ExecutionRequest::Docker {
                image: docker
                    .image
                    .ok_or_else(|| {
                        HarnessError::InvalidRequestedCase(
                            "sweep docker mode requires `image`".to_string(),
                        )
                    })?
                    .into_image_source()?,
                memory_limit: docker.memory_limit.unwrap_or_default(),
                cpuset: docker.cpuset,
                cpu_quota: docker.cpu_quota,
                cpu_period: docker.cpu_period,
                work_dir_mode: docker.work_dir_mode.unwrap_or(WorkDirMode::DockerTmpfs),
                allow_version_skew: docker.allow_version_skew,
            }),
            (true, Some(_)) => Err(HarnessError::InvalidRequestedCase(
                "sweep base mode must specify either native or docker, not both".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SweepDockerModeInput {
    #[serde(default)]
    pub image: Option<SweepDockerImageInput>,
    #[serde(default)]
    pub memory_limit: Option<String>,
    #[serde(default)]
    pub cpuset: Option<String>,
    #[serde(default)]
    pub cpu_quota: Option<i64>,
    #[serde(default)]
    pub cpu_period: Option<i64>,
    #[serde(default)]
    pub work_dir_mode: Option<WorkDirMode>,
    #[serde(default)]
    pub allow_version_skew: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SweepDockerImageInput {
    #[serde(default)]
    pub provided: Option<SweepProvidedImageInput>,
    #[serde(default)]
    pub auto_build: Option<SweepAutoBuildImageInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepProvidedImageInput {
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepAutoBuildImageInput {
    pub tag: String,
}

impl SweepDockerImageInput {
    fn into_image_source(self) -> Result<DockerImageSource, HarnessError> {
        match (self.provided, self.auto_build) {
            (Some(provided), None) => Ok(DockerImageSource::Provided {
                reference: provided.reference,
            }),
            (None, Some(auto_build)) => Ok(DockerImageSource::AutoBuild {
                tag: auto_build.tag,
            }),
            (None, None) => Err(HarnessError::InvalidRequestedCase(
                "sweep docker image requires either `provided` or `auto_build`".to_string(),
            )),
            (Some(_), Some(_)) => Err(HarnessError::InvalidRequestedCase(
                "sweep docker image must not specify both `provided` and `auto_build`".to_string(),
            )),
        }
    }
}

pub fn parse_matrix_value(value: Value) -> Result<SweepMatrix, HarnessError> {
    serde_json::from_value::<SweepMatrixFile>(value)?.into_matrix()
}

pub fn expand_matrix(matrix: &SweepMatrix) -> Result<Vec<ExpandedCase>, HarnessError> {
    if matrix.axes.is_empty() {
        return Err(HarnessError::InvalidRequestedCase(
            "sweep matrix requires at least one axis".to_string(),
        ));
    }

    let mut assignments = vec![BTreeMap::new()];
    for (axis_name, values) in &matrix.axes {
        if values.is_empty() {
            return Err(HarnessError::InvalidRequestedCase(format!(
                "sweep axis `{axis_name}` requires at least one value"
            )));
        }
        let mut next = Vec::new();
        for assignment in &assignments {
            for value in values {
                let mut assignment = assignment.clone();
                assignment.insert(axis_name.clone(), value.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }

    assignments
        .into_iter()
        .enumerate()
        .map(|(case_index, axis_assignments)| {
            let mut requested_case = matrix.base_case.clone();
            apply_axis_assignments(&mut requested_case, &axis_assignments)?;
            let case_slug = axis_assignments
                .iter()
                .map(|(axis, value)| {
                    let value_slug = axis_value_slug(axis, value, &matrix.axes)?;
                    Ok(format!("{}_{}", sanitize_slug(axis), value_slug))
                })
                .collect::<Result<Vec<_>, HarnessError>>()?
                .join("-");
            Ok(ExpandedCase {
                case_index,
                case_id: format!("case-{case_index:03}-{case_slug}"),
                axis_assignments,
                requested_case,
            })
        })
        .collect()
}

fn axis_value_slug(
    axis: &str,
    value: &AxisValue,
    axes: &BTreeMap<String, Vec<AxisValue>>,
) -> Result<String, HarnessError> {
    if axis != "snapshot" {
        return Ok(value.slug_value());
    }
    let values = axes.get(axis).ok_or_else(|| {
        HarnessError::InvalidRequestedCase("snapshot axis metadata missing".to_string())
    })?;
    snapshot_axis_slug(value, values)
}

fn snapshot_axis_slug(value: &AxisValue, values: &[AxisValue]) -> Result<String, HarnessError> {
    let snapshots = values
        .iter()
        .enumerate()
        .map(|(index, value)| Ok((index, snapshot_pair_value("snapshot", value)?)))
        .collect::<Result<Vec<_>, HarnessError>>()?;
    let current = snapshot_pair_value("snapshot", value)?;
    let current_index = snapshots
        .iter()
        .find_map(|(index, pair)| (pair == &current).then_some(*index))
        .ok_or_else(|| {
            HarnessError::InvalidRequestedCase("unknown snapshot axis value".to_string())
        })?;

    let stem = path_stem_label(&current.manifest);
    let stem_count = snapshots
        .iter()
        .filter(|(_, pair)| path_stem_label(&pair.manifest) == stem)
        .count();
    if stem_count == 1 {
        return Ok(sanitize_slug(&stem));
    }

    let parent = current
        .manifest
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let parent_stem = if parent.is_empty() {
        stem.clone()
    } else {
        format!("{parent}-{stem}")
    };
    let parent_stem_count = snapshots
        .iter()
        .filter(|(_, pair)| {
            let other_stem = path_stem_label(&pair.manifest);
            let other_parent = pair
                .manifest
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or("");
            let other_parent_stem = if other_parent.is_empty() {
                other_stem
            } else {
                format!("{other_parent}-{other_stem}")
            };
            other_parent_stem == parent_stem
        })
        .count();
    if parent_stem_count == 1 {
        return Ok(sanitize_slug(&parent_stem));
    }

    Ok(sanitize_slug(&format!("{parent_stem}-{current_index}")))
}

pub fn build_schedule(
    expanded_cases: &[ExpandedCase],
    mode: ScheduleMode,
    seed: Option<u64>,
) -> Result<SweepSchedule, HarnessError> {
    let case_ids = match mode {
        ScheduleMode::Sequential => expanded_cases
            .iter()
            .map(|case| case.case_id.clone())
            .collect::<Vec<_>>(),
        ScheduleMode::Interleaved => {
            let mut cases = expanded_cases.to_vec();
            cases.sort_by_key(interleave_sort_key);
            cases.into_iter().map(|case| case.case_id).collect()
        }
        ScheduleMode::Randomized => {
            let mut case_ids = expanded_cases
                .iter()
                .map(|case| case.case_id.clone())
                .collect::<Vec<_>>();
            deterministic_shuffle(&mut case_ids, seed.unwrap_or(0));
            case_ids
        }
    };

    if case_ids.is_empty() {
        return Err(HarnessError::InvalidRequestedCase(
            "sweep schedule requires at least one expanded case".to_string(),
        ));
    }

    Ok(SweepSchedule { mode, case_ids })
}

fn apply_axis_assignments(
    requested_case: &mut RequestedCase,
    axis_assignments: &BTreeMap<String, AxisValue>,
) -> Result<(), HarnessError> {
    for (axis, value) in axis_assignments {
        if apply_general_axis(requested_case, axis, value)? {
            continue;
        }

        if is_docker_axis(axis) {
            apply_docker_axis(requested_case, axis, value)?;
            continue;
        }

        return Err(HarnessError::InvalidRequestedCase(format!(
            "unsupported sweep axis `{axis}`"
        )));
    }

    Ok(())
}

fn apply_general_axis(
    requested_case: &mut RequestedCase,
    axis: &str,
    value: &AxisValue,
) -> Result<bool, HarnessError> {
    match axis {
        "mode" => requested_case.execution = mode_axis_value(axis, value)?,
        "threads" => requested_case.threads = integer_to_u32(axis, value)?,
        "allow_degraded_cold" => requested_case.allow_degraded_cold = boolean_value(axis, value)?,
        "allow_debug_benchmark" => {
            return Err(HarnessError::InvalidRequestedCase(
                "sweep axis `allow_debug_benchmark` is run-level policy; pass it as a sweep option"
                    .to_string(),
            ));
        }
        "plan" => {
            requested_case.orchestrate = super::case::RequestedOrchestrate::PlanFile {
                plan_path: path_value(axis, value)?,
            };
        }
        "checkpoint" => {
            requested_case.orchestrate = super::case::RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Checkpoint {
                    checkpoint: path_value(axis, value)?,
                },
                kernel_path: read_kernel_path(requested_case),
                start_height: read_start_height(requested_case),
                end_height: read_end_height(requested_case),
                count: read_count(requested_case),
                peek_mode: read_peek_mode(requested_case),
            };
        }
        "snapshot" => {
            let snapshot = snapshot_pair_value(axis, value)?;
            requested_case.orchestrate = super::case::RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot {
                    pma: snapshot.pma,
                    manifest: snapshot.manifest,
                },
                kernel_path: read_kernel_path(requested_case),
                start_height: read_start_height(requested_case),
                end_height: read_end_height(requested_case),
                count: read_count(requested_case),
                peek_mode: read_peek_mode(requested_case),
            };
        }
        "snapshot.pma" | "snapshot.manifest" => {
            return Err(HarnessError::InvalidRequestedCase(
                "sweep snapshot axes must vary the atomic `snapshot` object".to_string(),
            ));
        }
        "kernel" => sync_generated_read_source(
            requested_case,
            Some(path_value(axis, value)?),
            None,
            None,
            None,
            None,
        )?,
        "start_height" => sync_generated_read_source(
            requested_case,
            None,
            Some(integer_to_u64(axis, value)?),
            None,
            None,
            None,
        )?,
        "end_height" => sync_generated_read_source(
            requested_case,
            None,
            None,
            Some(Some(integer_to_u64(axis, value)?)),
            Some(None),
            None,
        )?,
        "count" => sync_generated_read_source(
            requested_case,
            None,
            None,
            Some(None),
            Some(Some(integer_to_u64(axis, value)?)),
            None,
        )?,
        "peek_mode" => sync_generated_read_source(
            requested_case,
            None,
            None,
            None,
            None,
            Some(peek_mode_value(axis, value)?),
        )?,
        "blocks" => {
            ensure_replay_axis(requested_case, axis)?;
            requested_case.blocks = integer_to_u64(axis, value)?;
            sync_generated_replay_source(requested_case);
        }
        "skip_genesis" => {
            ensure_replay_axis(requested_case, axis)?;
            requested_case.skip_genesis = boolean_value(axis, value)?;
            sync_generated_replay_source(requested_case);
        }
        "profile_memory" => requested_case.profile_memory = boolean_value(axis, value)?,
        "profile_interval_ms" => requested_case.profile_interval_ms = integer_to_u64(axis, value)?,
        "fsync" => requested_case.set_fsync_enabled(boolean_value(axis, value)?),
        "warmup_runs" => requested_case.warmup_runs = integer_to_u32(axis, value)?,
        "measured_runs" => requested_case.measured_runs = integer_to_u32(axis, value)?,
        "cooldown_secs" => requested_case.cooldown_secs = integer_to_u64(axis, value)?,
        "fixture" => {
            ensure_replay_axis(requested_case, axis)?;
            requested_case.fixture_path = path_value(axis, value)?;
            sync_generated_replay_source(requested_case);
        }
        "label" => requested_case.label = Some(string_value(axis, value)?),
        _ => return Ok(false),
    }

    Ok(true)
}

fn sync_generated_replay_source(requested_case: &mut RequestedCase) {
    if matches!(
        requested_case.orchestrate,
        super::case::RequestedOrchestrate::GeneratedReplay { .. }
    ) {
        requested_case.orchestrate = super::case::RequestedOrchestrate::GeneratedReplay {
            fixture_path: requested_case.fixture_path.clone(),
            blocks: Some(requested_case.blocks),
            skip_genesis: requested_case.skip_genesis,
        };
    }
}

fn ensure_replay_axis(requested_case: &RequestedCase, axis: &str) -> Result<(), HarnessError> {
    if matches!(
        requested_case.orchestrate,
        super::case::RequestedOrchestrate::GeneratedReplay { .. }
    ) {
        return Ok(());
    }
    Err(HarnessError::InvalidRequestedCase(format!(
        "sweep axis `{axis}` requires a replay fixture base"
    )))
}

fn integer_to_u32(axis: &str, value: &AxisValue) -> Result<u32, HarnessError> {
    let value = integer_value(axis, value)?;
    u32::try_from(value).map_err(|_| {
        HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires a non-negative 32-bit integer"
        ))
    })
}

fn integer_to_u64(axis: &str, value: &AxisValue) -> Result<u64, HarnessError> {
    let value = integer_value(axis, value)?;
    u64::try_from(value).map_err(|_| {
        HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires a non-negative 64-bit integer"
        ))
    })
}

fn integer_value(axis: &str, value: &AxisValue) -> Result<i64, HarnessError> {
    match value {
        AxisValue::Integer(value) => Ok(*value),
        _ => Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires an integer value"
        ))),
    }
}

fn string_value(_axis: &str, value: &AxisValue) -> Result<String, HarnessError> {
    match value {
        AxisValue::String(value) => Ok(value.clone()),
        AxisValue::Integer(value) => Ok(value.to_string()),
        AxisValue::Boolean(value) => Ok(value.to_string()),
        AxisValue::Object(_) => Err(HarnessError::InvalidRequestedCase(
            "sweep string axis requires a scalar value".to_string(),
        )),
    }
}

fn path_value(axis: &str, value: &AxisValue) -> Result<PathBuf, HarnessError> {
    Ok(PathBuf::from(string_value(axis, value)?))
}

fn snapshot_pair_value(axis: &str, value: &AxisValue) -> Result<SnapshotPair, HarnessError> {
    let AxisValue::Object(object) = value else {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires an object with `pma` and `manifest`"
        )));
    };
    serde_json::from_value::<SnapshotPair>(Value::Object(object.clone())).map_err(|source| {
        HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires snapshot `pma` and `manifest`: {source}"
        ))
    })
}

fn path_stem_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn boolean_value(axis: &str, value: &AxisValue) -> Result<bool, HarnessError> {
    match value {
        AxisValue::Boolean(value) => Ok(*value),
        _ => Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires a boolean value"
        ))),
    }
}

fn work_dir_mode_value(axis: &str, value: &AxisValue) -> Result<WorkDirMode, HarnessError> {
    let normalized = string_value(axis, value)?
        .replace(['_', '-'], "")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "hostbind" => Ok(WorkDirMode::HostBind),
        "dockervolume" => Ok(WorkDirMode::DockerVolume),
        "dockertmpfs" => Ok(WorkDirMode::DockerTmpfs),
        _ => Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires a valid work dir mode"
        ))),
    }
}

fn peek_mode_value(axis: &str, value: &AxisValue) -> Result<PeekMode, HarnessError> {
    match string_value(axis, value)?.replace('-', "_").as_str() {
        "warm" => Ok(PeekMode::Warm),
        "cold_each" => Ok(PeekMode::ColdEach),
        _ => Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires `warm` or `cold_each`"
        ))),
    }
}

fn mode_axis_value(axis: &str, value: &AxisValue) -> Result<ExecutionRequest, HarnessError> {
    let AxisValue::Object(object) = value else {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires an object value"
        )));
    };
    serde_json::from_value::<SweepModeInput>(Value::Object(object.clone()))?
        .into_execution_request()
}

fn read_kernel_path(requested_case: &RequestedCase) -> PathBuf {
    match &requested_case.orchestrate {
        super::case::RequestedOrchestrate::GeneratedRead { kernel_path, .. } => kernel_path.clone(),
        _ => default_kernel_path(),
    }
}

fn read_start_height(requested_case: &RequestedCase) -> u64 {
    match &requested_case.orchestrate {
        super::case::RequestedOrchestrate::GeneratedRead { start_height, .. } => *start_height,
        _ => 0,
    }
}

fn read_end_height(requested_case: &RequestedCase) -> Option<u64> {
    match &requested_case.orchestrate {
        super::case::RequestedOrchestrate::GeneratedRead { end_height, .. } => *end_height,
        _ => None,
    }
}

fn read_count(requested_case: &RequestedCase) -> Option<u64> {
    match &requested_case.orchestrate {
        super::case::RequestedOrchestrate::GeneratedRead { count, .. } => *count,
        _ => None,
    }
}

fn read_peek_mode(requested_case: &RequestedCase) -> PeekMode {
    match &requested_case.orchestrate {
        super::case::RequestedOrchestrate::GeneratedRead { peek_mode, .. } => *peek_mode,
        _ => PeekMode::Warm,
    }
}

fn sync_generated_read_source(
    requested_case: &mut RequestedCase,
    kernel_path: Option<PathBuf>,
    start_height: Option<u64>,
    end_height: Option<Option<u64>>,
    count: Option<Option<u64>>,
    peek_mode: Option<PeekMode>,
) -> Result<(), HarnessError> {
    let super::case::RequestedOrchestrate::GeneratedRead {
        boot,
        kernel_path: existing_kernel,
        start_height: existing_start,
        end_height: existing_end,
        count: existing_count,
        peek_mode: existing_peek_mode,
    } = &requested_case.orchestrate
    else {
        return Err(HarnessError::InvalidRequestedCase(
            "read sweep axes require a checkpoint/read base".to_string(),
        ));
    };
    requested_case.orchestrate = super::case::RequestedOrchestrate::GeneratedRead {
        boot: boot.clone(),
        kernel_path: kernel_path.unwrap_or_else(|| existing_kernel.clone()),
        start_height: start_height.unwrap_or(*existing_start),
        end_height: end_height.unwrap_or(*existing_end),
        count: count.unwrap_or(*existing_count),
        peek_mode: peek_mode.unwrap_or(*existing_peek_mode),
    };
    Ok(())
}

fn is_docker_axis(axis: &str) -> bool {
    matches!(
        axis,
        "image"
            | "memory_limit"
            | "cpuset"
            | "cpu_quota"
            | "cpu_period"
            | "work_dir_mode"
            | "allow_version_skew"
    )
}

fn apply_docker_axis(
    requested_case: &mut RequestedCase,
    axis: &str,
    value: &AxisValue,
) -> Result<(), HarnessError> {
    match &mut requested_case.execution {
        ExecutionRequest::Docker {
            image,
            memory_limit,
            cpuset,
            cpu_quota,
            cpu_period,
            work_dir_mode,
            allow_version_skew,
        } => match axis {
            "image" => *image = docker_image_axis_value(axis, value)?,
            "memory_limit" => *memory_limit = string_value(axis, value)?,
            "cpuset" => *cpuset = Some(string_value(axis, value)?),
            "cpu_quota" => *cpu_quota = Some(integer_value(axis, value)?),
            "cpu_period" => *cpu_period = Some(integer_value(axis, value)?),
            "work_dir_mode" => *work_dir_mode = work_dir_mode_value(axis, value)?,
            "allow_version_skew" => *allow_version_skew = boolean_value(axis, value)?,
            other => {
                return Err(HarnessError::InvalidRequestedCase(format!(
                    "unsupported sweep axis `{other}`"
                )));
            }
        },
        ExecutionRequest::Native => {
            return Err(HarnessError::InvalidRequestedCase(format!(
                "sweep axis `{axis}` requires Docker execution"
            )));
        }
    }

    Ok(())
}

fn docker_image_axis_value(
    axis: &str,
    value: &AxisValue,
) -> Result<DockerImageSource, HarnessError> {
    let AxisValue::Object(object) = value else {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "sweep axis `{axis}` requires an object value"
        )));
    };
    let image = serde_json::from_value::<SweepDockerImageInput>(Value::Object(object.clone()))?
        .into_image_source()?;
    if matches!(image, DockerImageSource::AutoBuild { .. }) {
        return Err(HarnessError::InvalidRequestedCase(
            "sweep axis `image` only accepts provided image values".to_string(),
        ));
    }
    Ok(image)
}

fn interleave_sort_key(expanded_case: &ExpandedCase) -> Vec<String> {
    let mut reversed = expanded_case
        .axis_assignments
        .iter()
        .rev()
        .map(|(axis, value)| format!("{axis}={}", value.slug_value()))
        .collect::<Vec<_>>();
    reversed.push(format!("{:06}", expanded_case.case_index));
    reversed
}

fn deterministic_shuffle<T>(values: &mut [T], seed: u64) {
    if values.len() < 2 {
        return;
    }

    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    for index in (1..values.len()).rev() {
        state = xorshift64(state);
        let swap_index = (state as usize) % (index + 1);
        values.swap(index, swap_index);
    }
}

fn xorshift64(mut state: u64) -> u64 {
    if state == 0 {
        state = 0x4d59_5df4_d0f3_3173;
    }
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

fn sanitize_slug(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut previous_separator = false;
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() || ch == '_' {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if normalized == '-' {
            if !previous_separator {
                slug.push(normalized);
            }
            previous_separator = true;
        } else {
            slug.push(normalized);
            previous_separator = false;
        }
    }
    slug.trim_matches('-').to_string()
}

pub async fn execute_sweep<E: SweepExecutor>(
    matrix_json: &Value,
    matrix: SweepMatrix,
    output_root: &Path,
    options: &SweepRunOptions,
    executor: &mut E,
) -> Result<SweepResult, HarnessError> {
    validate_sweep_profiling_support(&matrix, options)?;
    prepare_output_root(output_root)?;
    std::fs::create_dir_all(output_root)?;
    write_schema_version(output_root)?;

    let expanded_cases = expand_matrix(&matrix)?;
    let schedule = build_schedule(&expanded_cases, options.schedule_mode, options.random_seed)?;
    let mut prefetched = BTreeSet::new();
    for expanded_case in &expanded_cases {
        let key = serde_json::to_string(&(
            expanded_case.requested_case.execution.clone(),
            options.cpu_profiler.clone(),
        ))?;
        if prefetched.insert(key) {
            executor
                .preflight_case(&expanded_case.requested_case, options.cpu_profiler.as_ref())?;
        }
    }

    write_json(output_root.join("matrix.json"), matrix_json)?;
    write_json(output_root.join("matrix_expanded.json"), &expanded_cases)?;
    write_json(output_root.join("schedule.json"), &schedule)?;

    let cases_root = output_root.join("cases");
    std::fs::create_dir_all(&cases_root)?;
    let expanded_by_id = expanded_cases
        .iter()
        .cloned()
        .map(|case| (case.case_id.clone(), case))
        .collect::<BTreeMap<_, _>>();

    let mut case_runs = Vec::with_capacity(schedule.case_ids.len());
    let mut failed_cases = Vec::new();
    for (index, case_id) in schedule.case_ids.iter().enumerate() {
        let expanded_case = expanded_by_id.get(case_id).cloned().ok_or_else(|| {
            HarnessError::InvalidRequestedCase(format!("unknown scheduled case `{case_id}`"))
        })?;
        let case_output_root = cases_root.join(case_id);
        std::fs::create_dir_all(&case_output_root)?;
        match executor
            .execute_case(
                expanded_case.requested_case.clone(),
                &case_output_root,
                options.allow_debug_benchmark,
                options.cpu_profiler.clone(),
            )
            .await
        {
            Ok(result) => case_runs.push(SweepCaseRun {
                expanded_case: expanded_case.clone(),
                output_root: case_output_root.clone(),
                result,
            }),
            Err(error) => {
                let error_message = error.to_string();
                persist_failed_sweep_verdict(
                    &case_output_root,
                    format!("case {case_id} failed: {error_message}"),
                )?;
                failed_cases.push(SweepCaseFailure {
                    case_id: expanded_case.case_id.clone(),
                    axis_assignments: expanded_case.axis_assignments.clone(),
                    output_root: case_output_root.clone(),
                    error: error_message,
                });
            }
        }

        if index + 1 < schedule.case_ids.len() && expanded_case.requested_case.cooldown_secs > 0 {
            sleep(Duration::from_secs(
                expanded_case.requested_case.cooldown_secs,
            ))
            .await;
        }
    }

    let comparison = build_comparison_with_failures(&case_runs, &failed_cases)?;
    write_json(output_root.join("comparison.json"), &comparison)?;
    if options.comparison_markdown {
        std::fs::write(
            output_root.join("comparison.md"),
            render_comparison_markdown(&comparison),
        )?;
    }

    let verdict = derive_sweep_verdict(&comparison);
    write_verdict(output_root, &verdict)?;

    Ok(SweepResult {
        expanded_cases,
        schedule,
        comparison,
        verdict,
    })
}

fn validate_sweep_profiling_support(
    _matrix: &SweepMatrix,
    options: &SweepRunOptions,
) -> Result<(), HarnessError> {
    if options.cpu_profiler.is_some() {
        return Err(HarnessError::InvalidRequestedCase(
            "trusted sol-orchestrate sweeps do not support CPU profiling in the first release"
                .to_string(),
        ));
    }
    Ok(())
}

pub fn build_comparison(case_runs: &[SweepCaseRun]) -> Result<SweepComparison, HarnessError> {
    build_comparison_with_failures(case_runs, &[])
}

fn build_comparison_with_failures(
    case_runs: &[SweepCaseRun],
    failed_cases: &[SweepCaseFailure],
) -> Result<SweepComparison, HarnessError> {
    if case_runs.is_empty() && failed_cases.is_empty() {
        return Err(HarnessError::InvalidRequestedCase(
            "sweep comparison requires at least one case".to_string(),
        ));
    }

    let axis_names = case_runs
        .first()
        .map(|case_run| {
            case_run
                .expanded_case
                .axis_assignments
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            failed_cases.first().map(|failed_case| {
                failed_case
                    .axis_assignments
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
        })
        .expect("comparison requires at least one successful or failed case");
    let axis_name_set = axis_names.iter().cloned().collect::<BTreeSet<_>>();
    let backend_groups = build_backend_groups(case_runs, failed_cases, &axis_name_set)?;
    let mixed_backends = backend_groups.len() > 1;
    let mut invariant_violations = Vec::new();

    if !mixed_backends {
        if let Some(baseline) = case_runs.first() {
            for case_run in case_runs.iter().skip(1) {
                compare_case_run_invariants(
                    &mut invariant_violations, &axis_name_set, baseline, case_run,
                );
            }
        }
    }

    let cases = case_runs
        .iter()
        .map(case_comparison_from_run)
        .collect::<Vec<_>>();

    let aggregate = if mixed_backends {
        BTreeMap::new()
    } else {
        comparison_aggregate(&cases)
    };
    let by_step_type = if mixed_backends {
        BTreeMap::new()
    } else {
        comparison_by_step_type(&cases)
    };
    let common_steps = if mixed_backends {
        Vec::new()
    } else {
        build_common_steps(case_runs)?
    };
    let non_comparable_metrics = non_comparable_metrics(&cases, &axis_name_set, mixed_backends);

    Ok(SweepComparison {
        schema_version: COMPARISON_SCHEMA_VERSION.to_string(),
        axis_names,
        case_count: cases.len(),
        cases,
        failed_cases: failed_cases.to_vec(),
        invariant_violations,
        aggregate,
        by_step_type,
        common_steps,
        non_comparable_metrics,
        backend_groups,
    })
}

fn build_backend_groups(
    case_runs: &[SweepCaseRun],
    _failed_cases: &[SweepCaseFailure],
    axis_name_set: &BTreeSet<String>,
) -> Result<Vec<SweepComparisonGroup>, HarnessError> {
    let mut grouped = BTreeMap::<String, Vec<&SweepCaseRun>>::new();
    for case_run in case_runs {
        grouped
            .entry(backend_group_label(&case_run.result.provenance.backend).to_string())
            .or_default()
            .push(case_run);
    }
    if grouped.len() <= 1 {
        return Ok(Vec::new());
    }

    grouped
        .into_iter()
        .map(|(backend, runs)| {
            let mut invariant_violations = Vec::new();
            if let Some(baseline) = runs.first() {
                for case_run in runs.iter().skip(1) {
                    compare_case_run_invariants(
                        &mut invariant_violations, axis_name_set, baseline, case_run,
                    );
                }
            }
            let cases = runs
                .iter()
                .map(|case_run| case_comparison_from_run(case_run))
                .collect::<Vec<_>>();
            let aggregate = comparison_aggregate(&cases);
            let by_step_type = comparison_by_step_type(&cases);
            let common_steps = build_common_steps_from_refs(&runs)?;
            let non_comparable_metrics = non_comparable_metrics(&cases, axis_name_set, false);
            Ok(SweepComparisonGroup {
                backend,
                case_count: cases.len(),
                cases,
                failed_cases: Vec::new(),
                invariant_violations,
                aggregate,
                by_step_type,
                common_steps,
                non_comparable_metrics,
            })
        })
        .collect()
}

fn compare_case_run_invariants(
    invariant_violations: &mut Vec<String>,
    axis_name_set: &BTreeSet<String>,
    baseline: &SweepCaseRun,
    current: &SweepCaseRun,
) {
    compare_requested_case_invariants(
        invariant_violations, axis_name_set, &baseline.result.resolved, &current.result.resolved,
        &current.expanded_case.case_id,
    );
    compare_binary_identity_invariants(
        invariant_violations, axis_name_set, &baseline.result.resolved, &current.result.resolved,
        &current.expanded_case.case_id,
    );
    compare_git_identity_invariants(
        invariant_violations, axis_name_set, &baseline.result.provenance,
        &current.result.provenance, &current.expanded_case.case_id,
    );
    compare_host_and_pma_invariants(
        invariant_violations, axis_name_set, &baseline.result.provenance,
        &current.result.provenance, &current.expanded_case.case_id,
    );
    compare_resolved_docker_invariants(
        invariant_violations,
        axis_name_set,
        baseline.result.resolved.docker.as_ref(),
        current.result.resolved.docker.as_ref(),
        &current.expanded_case.case_id,
    );
    compare_backend_invariants(
        invariant_violations, axis_name_set, &baseline.result.provenance.backend,
        &current.result.provenance.backend, &current.expanded_case.case_id,
    );
}

fn case_comparison_from_run(case_run: &SweepCaseRun) -> SweepCaseComparison {
    SweepCaseComparison {
        case_id: case_run.expanded_case.case_id.clone(),
        axis_assignments: case_run.expanded_case.axis_assignments.clone(),
        output_root: case_run.output_root.clone(),
        resolved_case: case_run.result.resolved.clone(),
        summary: case_run.result.summary.clone(),
        verdict: case_run.result.verdict.clone(),
    }
}

fn backend_group_label(backend: &BackendRuntimeFacts) -> &'static str {
    match backend {
        BackendRuntimeFacts::Native => "native",
        BackendRuntimeFacts::Docker { .. } => "docker",
    }
}

fn comparison_aggregate(
    cases: &[SweepCaseComparison],
) -> BTreeMap<String, BTreeMap<String, ValueStats>> {
    let mut aggregate = BTreeMap::new();
    for case in cases {
        insert_metric(
            &mut aggregate,
            "total_step_time_secs",
            case.summary.total_step_time_secs.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "steps_per_second",
            case.summary.steps_per_second.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "pokes_per_second",
            case.summary.pokes_per_second.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "peeks_per_second",
            case.summary.peeks_per_second.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "cold_peeks_per_second",
            case.summary.cold_peeks_per_second.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "peak_process_rss_bytes",
            case.summary.peak_process_rss_bytes.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "minor_faults_total",
            case.summary.minor_faults_total.clone(),
            &case.case_id,
        );
        insert_metric(
            &mut aggregate,
            "major_faults_total",
            case.summary.major_faults_total.clone(),
            &case.case_id,
        );
    }
    aggregate
}

fn insert_metric(
    aggregate: &mut BTreeMap<String, BTreeMap<String, ValueStats>>,
    metric: &str,
    value: Option<ValueStats>,
    case_id: &str,
) {
    if let Some(stats) = value {
        aggregate
            .entry(metric.to_string())
            .or_default()
            .insert(case_id.to_string(), stats);
    }
}

fn comparison_by_step_type(
    cases: &[SweepCaseComparison],
) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, ValueStats>>> {
    let mut by_step_type = BTreeMap::new();
    for case in cases {
        for (step_type, metrics) in &case.summary.by_step_type {
            for (metric, stats) in step_type_stats(metrics) {
                by_step_type
                    .entry(step_type.clone())
                    .or_insert_with(BTreeMap::new)
                    .entry(metric)
                    .or_insert_with(BTreeMap::new)
                    .insert(case.case_id.clone(), stats);
            }
        }
    }
    by_step_type
}

fn step_type_stats(summary: &super::summary::StepTypeSummary) -> Vec<(String, ValueStats)> {
    [
        ("duration_ms", summary.duration_ms.clone()),
        (
            "throughput_per_second",
            summary.throughput_per_second.clone(),
        ),
        ("error_count", summary.error_count.clone()),
        ("success_count", summary.success_count.clone()),
        ("missing_count", summary.missing_count.clone()),
        ("cold_verified_count", summary.cold_verified_count.clone()),
        (
            "cold_unverified_count",
            summary.cold_unverified_count.clone(),
        ),
        ("minflt_delta", summary.minflt_delta.clone()),
        ("majflt_delta", summary.majflt_delta.clone()),
    ]
    .into_iter()
    .filter_map(|(name, stats)| stats.map(|stats| (name.to_string(), stats)))
    .collect()
}

fn build_common_steps(case_runs: &[SweepCaseRun]) -> Result<Vec<SweepCommonStep>, HarnessError> {
    let case_run_refs = case_runs.iter().collect::<Vec<_>>();
    build_common_steps_from_refs(&case_run_refs)
}

fn build_common_steps_from_refs(
    case_runs: &[&SweepCaseRun],
) -> Result<Vec<SweepCommonStep>, HarnessError> {
    if case_runs.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows_by_case = Vec::new();
    for case_run in case_runs {
        rows_by_case.push((
            case_run.expanded_case.case_id.clone(),
            read_step_rows(&case_run.output_root)?,
        ));
    }
    let mut common_ids: Option<BTreeSet<String>> = None;
    for (_, rows) in &rows_by_case {
        let ids = rows
            .iter()
            .map(|row| row.step_id.clone())
            .collect::<BTreeSet<_>>();
        common_ids = Some(match common_ids {
            Some(existing) => existing.intersection(&ids).cloned().collect(),
            None => ids,
        });
    }
    let mut common_steps = Vec::new();
    for step_id in common_ids.unwrap_or_default() {
        let mut step_type = String::new();
        let mut height = None;
        let mut per_case = BTreeMap::new();
        for (case_id, rows) in &rows_by_case {
            let matching = rows
                .iter()
                .filter(|row| row.step_id == step_id)
                .collect::<Vec<_>>();
            if let Some(first) = matching.first() {
                step_type = first.step_type.clone();
                height = height.or(first.height);
            }
            let mut durations = matching
                .iter()
                .map(|row| row.duration_ms)
                .collect::<Vec<_>>();
            let mut outcome_counts = BTreeMap::new();
            for row in matching {
                *outcome_counts.entry(row.outcome.clone()).or_insert(0) += 1;
            }
            per_case.insert(
                case_id.clone(),
                SweepCommonStepCase {
                    duration_ms_median: median(&mut durations),
                    outcome_counts,
                },
            );
        }
        common_steps.push(SweepCommonStep {
            step_id,
            step_type,
            height,
            per_case,
        });
    }
    Ok(common_steps)
}

fn read_step_rows(output_root: &Path) -> Result<Vec<StepResultRow>, HarnessError> {
    let runs_dir = output_root.join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let path = entry.path().join("steps.ndjson");
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(path)?;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            rows.push(serde_json::from_str(line)?);
        }
    }
    Ok(rows)
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Some(values[middle])
    }
}

fn non_comparable_metrics(
    cases: &[SweepCaseComparison],
    axis_names: &BTreeSet<String>,
    mixed_backends: bool,
) -> Vec<NonComparableMetric> {
    let mut metrics = Vec::new();
    if mixed_backends {
        metrics.push(NonComparableMetric {
            metric: "aggregate".to_string(),
            reason: "native and Docker backends are split into separate comparison groups"
                .to_string(),
        });
    }
    if step_identity_axis_can_vary(axis_names) && step_type_counts_differ(cases) {
        metrics.push(NonComparableMetric {
            metric: "aggregate".to_string(),
            reason: "step identity axis changed step-type counts".to_string(),
        });
    }
    if step_signatures_differ(cases) {
        metrics.push(NonComparableMetric {
            metric: "common_steps".to_string(),
            reason: "step signatures differ across cases".to_string(),
        });
    }
    if cases.iter().any(|case| match &case.verdict.validity {
        Validity::Partial { reasons } | Validity::Invalid { reasons } => {
            reasons.iter().any(|reason| reason.contains("cold"))
        }
        Validity::Valid => false,
    }) {
        metrics.push(NonComparableMetric {
            metric: "cold_peeks_per_second".to_string(),
            reason: "one or more cases have degraded or invalid cold evidence".to_string(),
        });
    }
    metrics
}

fn step_identity_axis_can_vary(axis_names: &BTreeSet<String>) -> bool {
    step_identity_axes()
        .iter()
        .any(|axis_name| axis_names.contains(*axis_name))
}

fn step_identity_axes() -> &'static [&'static str] {
    &[
        "step_signature", "fixture", "plan", "source-plan", "source_plan", "checkpoint",
        "snapshot", "kernel", "archive",
    ]
}

fn step_signatures_differ(cases: &[SweepCaseComparison]) -> bool {
    let Some(first) = cases.first() else {
        return false;
    };
    let baseline = &first.resolved_case.orchestrate.step_signature_sha256_hex;
    cases
        .iter()
        .skip(1)
        .any(|case| &case.resolved_case.orchestrate.step_signature_sha256_hex != baseline)
}

fn step_type_counts_differ(cases: &[SweepCaseComparison]) -> bool {
    let Some(first) = cases.first() else {
        return false;
    };
    let baseline = step_type_counts(&first.summary);
    cases
        .iter()
        .skip(1)
        .any(|case| step_type_counts(&case.summary) != baseline)
}

fn step_type_counts(summary: &super::summary::RunSummary) -> BTreeMap<String, u64> {
    summary
        .by_step_type
        .iter()
        .map(|(step_type, summary)| (step_type.clone(), summary.count_per_run))
        .collect()
}

fn compare_requested_case_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: &ResolvedCase,
    current: &ResolvedCase,
    case_id: &str,
) {
    compare_invariant(
        invariant_violations, axis_names, "fixture", "fixture_sha256_hex",
        &baseline.fixture_sha256_hex, &current.fixture_sha256_hex, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "fixture", "fixture_manifest",
        &baseline.fixture_manifest, &current.fixture_manifest, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "threads", "threads", &baseline.requested.threads,
        &current.requested.threads, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "profile_memory", "profile_memory",
        &baseline.requested.profile_memory, &current.requested.profile_memory, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "blocks", "blocks", &baseline.requested.blocks,
        &current.requested.blocks, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "skip_genesis", "skip_genesis",
        &baseline.requested.skip_genesis, &current.requested.skip_genesis, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "profile_interval_ms", "profile_interval_ms",
        &baseline.requested.profile_interval_ms, &current.requested.profile_interval_ms, case_id,
    );
    compare_invariant(
        invariant_violations,
        axis_names,
        "fsync",
        "fsync",
        &baseline.requested.fsync_enabled(),
        &current.requested.fsync_enabled(),
        case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "warmup_runs", "warmup_runs",
        &baseline.requested.warmup_runs, &current.requested.warmup_runs, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "measured_runs", "measured_runs",
        &baseline.requested.measured_runs, &current.requested.measured_runs, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "cooldown_secs", "cooldown_secs",
        &baseline.requested.cooldown_secs, &current.requested.cooldown_secs, case_id,
    );
    compare_invariant_any_axis(
        invariant_violations,
        axis_names,
        step_identity_axes(),
        "orchestrate.step_signature_sha256_hex",
        &baseline.orchestrate.step_signature_sha256_hex,
        &current.orchestrate.step_signature_sha256_hex,
        case_id,
    );
    compare_invariant_any_axis(
        invariant_violations,
        axis_names,
        &[
            "fixture", "plan", "source-plan", "source_plan", "checkpoint", "snapshot", "kernel",
            "archive",
        ],
        "orchestrate.input_hashes",
        &input_hash_inventory(baseline),
        &input_hash_inventory(current),
        case_id,
    );
}

fn input_hash_inventory(resolved: &ResolvedCase) -> BTreeMap<(String, String), String> {
    resolved
        .orchestrate
        .inputs
        .iter()
        .map(|input| {
            (
                (format!("{:?}", input.role), input.input_id.clone()),
                input.sha256_hex.clone(),
            )
        })
        .collect()
}

fn compare_binary_identity_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: &ResolvedCase,
    current: &ResolvedCase,
    case_id: &str,
) {
    compare_invariant(
        invariant_violations, axis_names, "version", "binary.version", &baseline.binary.version,
        &current.binary.version, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "git_commit", "binary.git_commit",
        &baseline.binary.git_commit, &current.binary.git_commit, case_id,
    );
    compare_invariant(
        invariant_violations, axis_names, "build_profile", "binary.build_profile",
        &baseline.binary.build_profile, &current.binary.build_profile, case_id,
    );
}

fn compare_git_identity_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: &Provenance,
    current: &Provenance,
    case_id: &str,
) {
    compare_invariant(
        invariant_violations,
        axis_names,
        "git_commit",
        "provenance.git.commit",
        &baseline.git.as_ref().and_then(|git| git.commit.clone()),
        &current.git.as_ref().and_then(|git| git.commit.clone()),
        case_id,
    );
    compare_invariant(
        invariant_violations,
        axis_names,
        "git_dirty",
        "provenance.git.dirty",
        &baseline.git.as_ref().map(|git| git.dirty),
        &current.git.as_ref().map(|git| git.dirty),
        case_id,
    );
}

fn compare_host_and_pma_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: &Provenance,
    current: &Provenance,
    case_id: &str,
) {
    let baseline_pma = baseline.pma_replay_provenance();
    let current_pma = current.pma_replay_provenance();

    compare_invariant(
        invariant_violations, axis_names, "host_identity", "provenance.host", &baseline.host,
        &current.host, case_id,
    );
    compare_invariant(
        invariant_violations,
        axis_names,
        "runtime_flavor",
        "provenance.runtime_flavor",
        &baseline_pma
            .as_ref()
            .and_then(|pma| pma.runtime_flavor.clone()),
        &current_pma
            .as_ref()
            .and_then(|pma| pma.runtime_flavor.clone()),
        case_id,
    );
    compare_invariant(
        invariant_violations,
        axis_names,
        "boot_source",
        "provenance.boot_source",
        &baseline_pma
            .as_ref()
            .and_then(|pma| pma.boot_source.clone()),
        &current_pma.as_ref().and_then(|pma| pma.boot_source.clone()),
        case_id,
    );
    compare_invariant_any_axis(
        invariant_violations,
        axis_names,
        &["pma_work_dir_mode", "work_dir_mode"],
        "provenance.pma_work_dir_mode",
        &baseline_pma
            .as_ref()
            .and_then(|pma| pma.pma_work_dir_mode.clone()),
        &current_pma
            .as_ref()
            .and_then(|pma| pma.pma_work_dir_mode.clone()),
        case_id,
    );
    compare_invariant_any_axis(
        invariant_violations,
        axis_names,
        &["boot_event_num", "fixture"],
        "provenance.boot_event_num",
        &baseline_pma.as_ref().and_then(|pma| pma.boot_event_num),
        &current_pma.as_ref().and_then(|pma| pma.boot_event_num),
        case_id,
    );
}

pub fn derive_sweep_verdict(comparison: &SweepComparison) -> Verdict {
    let mut invalid_reasons = comparison.invariant_violations.clone();
    let mut partial_reasons = Vec::new();

    for group in &comparison.backend_groups {
        invalid_reasons.extend(
            group
                .invariant_violations
                .iter()
                .map(|reason| format!("{} backend: {reason}", group.backend)),
        );
    }

    for case in &comparison.cases {
        match &case.verdict.validity {
            Validity::Valid => {}
            Validity::Partial { reasons } => {
                partial_reasons.extend(
                    reasons
                        .iter()
                        .map(|reason| format!("{}: {reason}", case.case_id)),
                );
            }
            Validity::Invalid { reasons } => {
                invalid_reasons.extend(
                    reasons
                        .iter()
                        .map(|reason| format!("{}: {reason}", case.case_id)),
                );
            }
        }
    }

    invalid_reasons.extend(
        comparison
            .failed_cases
            .iter()
            .map(|failed_case| format!("{}: {}", failed_case.case_id, failed_case.error)),
    );

    if !invalid_reasons.is_empty() {
        Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: 0.10,
            validity: Validity::Invalid {
                reasons: invalid_reasons,
            },
        }
    } else if !partial_reasons.is_empty() {
        Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: 0.10,
            validity: Validity::Partial {
                reasons: partial_reasons,
            },
        }
    } else {
        Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: 0.10,
            validity: Validity::Valid,
        }
    }
}

fn compare_invariant<T: PartialEq>(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    axis_name: &str,
    field_name: &str,
    baseline: &T,
    current: &T,
    case_id: &str,
) {
    compare_invariant_any_axis(
        invariant_violations,
        axis_names,
        &[axis_name],
        field_name,
        baseline,
        current,
        case_id,
    );
}

fn compare_invariant_any_axis<T: PartialEq>(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    axis_names_to_ignore: &[&str],
    field_name: &str,
    baseline: &T,
    current: &T,
    case_id: &str,
) {
    if axis_names_to_ignore
        .iter()
        .any(|axis_name| axis_names.contains(*axis_name))
        || baseline == current
    {
        return;
    }
    invariant_violations.push(format!(
        "case {case_id} changed non-axis field `{field_name}`"
    ));
}

fn compare_resolved_docker_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: Option<&crate::speed_of_light::harness::case::DockerResolvedConfig>,
    current: Option<&crate::speed_of_light::harness::case::DockerResolvedConfig>,
    case_id: &str,
) {
    match (baseline, current) {
        (None, None) => {}
        (Some(baseline), Some(current)) => {
            compare_invariant(
                invariant_violations, axis_names, "cpuset", "docker.cpuset", &baseline.cpuset,
                &current.cpuset, case_id,
            );
            compare_invariant(
                invariant_violations, axis_names, "cpu_quota", "docker.cpu_quota",
                &baseline.cpu_quota, &current.cpu_quota, case_id,
            );
            compare_invariant(
                invariant_violations, axis_names, "cpu_period", "docker.cpu_period",
                &baseline.cpu_period, &current.cpu_period, case_id,
            );
            compare_invariant(
                invariant_violations, axis_names, "work_dir_mode", "docker.work_dir_mode",
                &baseline.work_dir_mode, &current.work_dir_mode, case_id,
            );
            compare_invariant(
                invariant_violations, axis_names, "allow_version_skew",
                "docker.allow_version_skew", &baseline.allow_version_skew,
                &current.allow_version_skew, case_id,
            );
        }
        _ => invariant_violations.push(format!(
            "case {case_id} changed non-axis field `resolved.docker`"
        )),
    }
}

fn compare_backend_invariants(
    invariant_violations: &mut Vec<String>,
    axis_names: &BTreeSet<String>,
    baseline: &BackendRuntimeFacts,
    current: &BackendRuntimeFacts,
    case_id: &str,
) {
    match (baseline, current) {
        (BackendRuntimeFacts::Native, BackendRuntimeFacts::Native) => {}
        (
            BackendRuntimeFacts::Docker {
                host_binary: baseline_host_binary,
                container_binary: baseline_container_binary,
                image_digest: baseline_image_digest,
                realized_cpuset: baseline_cpuset,
                realized_cpu_max: baseline_cpu_max,
                ..
            },
            BackendRuntimeFacts::Docker {
                host_binary: current_host_binary,
                container_binary: current_container_binary,
                image_digest: current_image_digest,
                realized_cpuset: current_cpuset,
                realized_cpu_max: current_cpu_max,
                ..
            },
        ) => {
            compare_invariant_any_axis(
                invariant_violations,
                axis_names,
                &["image"],
                "backend.image_digest",
                baseline_image_digest,
                current_image_digest,
                case_id,
            );
            compare_invariant_any_axis(
                invariant_violations,
                axis_names,
                &[],
                "backend.host_binary",
                baseline_host_binary,
                current_host_binary,
                case_id,
            );
            compare_invariant_any_axis(
                invariant_violations,
                axis_names,
                &[],
                "backend.container_binary",
                baseline_container_binary,
                current_container_binary,
                case_id,
            );
            compare_invariant(
                invariant_violations, axis_names, "cpuset", "backend.realized_cpuset",
                baseline_cpuset, current_cpuset, case_id,
            );
            compare_invariant_any_axis(
                invariant_violations,
                axis_names,
                &["cpu_quota", "cpu_period"],
                "backend.realized_cpu_max",
                baseline_cpu_max,
                current_cpu_max,
                case_id,
            );
        }
        _ => invariant_violations.push(format!(
            "case {case_id} changed non-axis field `execution_mode`"
        )),
    }
}

fn render_comparison_markdown(comparison: &SweepComparison) -> String {
    let mut output = String::from("# SOL Sweep Comparison\n\n");
    output.push_str(
        "| Case | Axes | Verdict | Plan Time Median | Poke/s | Peek/s | Cold Peek/s | Evidence Notes |\n",
    );
    output.push_str("| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for case in &comparison.cases {
        let axes = case
            .axis_assignments
            .iter()
            .map(|(axis, value)| format!("{axis}={}", value.slug_value()))
            .collect::<Vec<_>>()
            .join(", ");
        let verdict = match &case.verdict.validity {
            Validity::Valid => "Valid".to_string(),
            Validity::Partial { .. } => "Partial".to_string(),
            Validity::Invalid { .. } => "Invalid".to_string(),
        };
        let plan_time = format_stats(&case.summary.total_step_time_secs);
        let poke_rate = format_stats(&case.summary.pokes_per_second);
        let peek_rate = format_stats(&case.summary.peeks_per_second);
        let cold_peek_rate = format_stats(&case.summary.cold_peeks_per_second);
        let notes = match &case.verdict.validity {
            Validity::Valid => "-".to_string(),
            Validity::Partial { reasons } | Validity::Invalid { reasons } => reasons.join("; "),
        };
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            case.case_id, axes, verdict, plan_time, poke_rate, peek_rate, cold_peek_rate, notes
        ));
    }
    for failed_case in &comparison.failed_cases {
        let axes = failed_case
            .axis_assignments
            .iter()
            .map(|(axis, value)| format!("{axis}={}", value.slug_value()))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "| {} | {} | Invalid | - | - | - | - | {} |\n",
            failed_case.case_id, axes, failed_case.error
        ));
    }
    output
}

fn format_stats(stats: &Option<ValueStats>) -> String {
    stats
        .as_ref()
        .map(|stats| format!("{:.2}", stats.median))
        .unwrap_or_else(|| "-".to_string())
}

fn persist_failed_sweep_verdict(output_root: &Path, reason: String) -> Result<(), HarnessError> {
    write_verdict(
        output_root,
        &Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: 0.10,
            validity: Validity::Invalid {
                reasons: vec![reason],
            },
        },
    )
}

fn default_profile_interval_ms() -> u64 {
    500
}

fn default_kernel_path() -> PathBuf {
    PathBuf::from("assets/dumb.jam")
}

fn default_threads() -> u32 {
    1
}

fn default_warmup_runs() -> u32 {
    1
}

fn default_measured_runs() -> u32 {
    5
}

fn default_cooldown_secs() -> u64 {
    10
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use futures::FutureExt;
    use tempfile::tempdir;

    use super::*;
    use crate::speed_of_light::fixture::SolFixtureManifest;
    use crate::speed_of_light::harness::case::{
        BinaryIdentity, DockerResolvedConfig, ExecutionConfig, ExecutionRequest,
        RequestedOrchestrate, ResolvedCase, ResolvedOrchestrate, WorkDirMode,
    };
    use crate::speed_of_light::harness::docker_image::{
        DockerImageSource, DockerImageVariant, ResolvedDockerImage,
    };
    use crate::speed_of_light::harness::orchestrate::TrustedRunResult;
    use crate::speed_of_light::harness::provenance::{
        BackendRuntimeFacts, HostIdentity, PmaReplayProvenance, Provenance,
    };
    use crate::speed_of_light::harness::summary::{RunSummary, Validity, ValueStats, Verdict};
    use crate::speed_of_light::harness::{
        PROVENANCE_SCHEMA_VERSION, RESOLVED_CASE_SCHEMA_VERSION, SUMMARY_SCHEMA_VERSION,
        VERDICT_SCHEMA_VERSION,
    };
    use crate::speed_of_light::types::SolHeight;

    struct FakeExecutor {
        seen_paths: Arc<Mutex<Vec<PathBuf>>>,
        seen_requested_cases: Arc<Mutex<Vec<RequestedCase>>>,
        seen_cpu_profilers: Arc<Mutex<Vec<Option<CpuProfilerConfig>>>>,
        results: Vec<Result<TrustedRunResult, HarnessError>>,
    }

    impl FakeExecutor {
        fn new(results: Vec<Result<TrustedRunResult, HarnessError>>) -> Self {
            Self {
                seen_paths: Arc::new(Mutex::new(Vec::new())),
                seen_requested_cases: Arc::new(Mutex::new(Vec::new())),
                seen_cpu_profilers: Arc::new(Mutex::new(Vec::new())),
                results,
            }
        }

        fn seen_paths(&self) -> Arc<Mutex<Vec<PathBuf>>> {
            Arc::clone(&self.seen_paths)
        }

        fn seen_requested_cases(&self) -> Arc<Mutex<Vec<RequestedCase>>> {
            Arc::clone(&self.seen_requested_cases)
        }

        fn seen_cpu_profilers(&self) -> Arc<Mutex<Vec<Option<CpuProfilerConfig>>>> {
            Arc::clone(&self.seen_cpu_profilers)
        }
    }

    impl SweepExecutor for FakeExecutor {
        fn execute_case<'a>(
            &'a mut self,
            requested_case: RequestedCase,
            output_root: &'a Path,
            _allow_debug_benchmark: bool,
            cpu_profiler: Option<CpuProfilerConfig>,
        ) -> futures::future::BoxFuture<'a, Result<TrustedRunResult, HarnessError>> {
            self.seen_paths
                .lock()
                .expect("seen paths lock")
                .push(output_root.to_path_buf());
            self.seen_requested_cases
                .lock()
                .expect("requested cases lock")
                .push(requested_case.clone());
            self.seen_cpu_profilers
                .lock()
                .expect("cpu profilers lock")
                .push(cpu_profiler.clone());
            let result = self.results.remove(0);
            async move { result }.boxed()
        }
    }

    fn fixture_manifest() -> SolFixtureManifest {
        SolFixtureManifest {
            source_archive_path: "archive.solarch".to_string(),
            source_archive_event_num: Some(1),
            checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
            checkpoint_height: SolHeight(10),
            checkpoint_event_num: 10,
            archive_start_height: SolHeight(11),
            archive_end_height: SolHeight(20),
            include_mempool: false,
            chunk_size: 8,
            kernel_hash_hex: "kernel".to_string(),
            checkpoint_hash_hex: "checkpoint".to_string(),
            archive_hash_hex: "archive".to_string(),
        }
    }

    fn resolved_docker_image() -> ResolvedDockerImage {
        ResolvedDockerImage {
            source: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            variant: DockerImageVariant::Standard,
            requested_ref: "nockchain-bench:test".to_string(),
            resolved_ref: "sha256:digest".to_string(),
            immutable_identity: "sha256:digest".to_string(),
            image_id: "sha256:digest".to_string(),
        }
    }

    fn host_identity() -> HostIdentity {
        HostIdentity {
            hostname: Some("host".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            kernel: Some("6.0".to_string()),
            cpu_count: 8,
            total_memory_bytes: Some(32 * 1024 * 1024 * 1024),
            cpu_model: Some("cpu".to_string()),
        }
    }

    fn git_identity() -> crate::speed_of_light::harness::provenance::GitIdentity {
        crate::speed_of_light::harness::provenance::GitIdentity {
            commit: Some("abc123".to_string()),
            branch: Some("main".to_string()),
            commit_date: Some("2026-03-11T00:00:00Z".to_string()),
            dirty: false,
        }
    }

    fn docker_runtime_facts(binary: &BinaryIdentity, container_id: String) -> BackendRuntimeFacts {
        BackendRuntimeFacts::Docker {
            host_binary: binary.clone(),
            container_binary: binary.clone(),
            image_source: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            requested_image_ref: "nockchain-bench:test".to_string(),
            resolved_image_ref: "sha256:digest".to_string(),
            image_digest: "sha256:digest".to_string(),
            container_id,
            docker_engine_version: "28.0".to_string(),
            docker_context: "desktop-linux".to_string(),
            cgroup_version: "2".to_string(),
            storage_driver: "overlay2".to_string(),
            realized_memory_max: 4 * 1024 * 1024 * 1024,
            realized_memory_current: 256 * 1024 * 1024,
            realized_cpuset: Some("0-3".to_string()),
            realized_cpu_max: Some("200000 100000".to_string()),
        }
    }

    fn base_provenance(resolved: &ResolvedCase, backend: BackendRuntimeFacts) -> Provenance {
        Provenance {
            schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
            capture_timestamp_ms: 1,
            host: host_identity(),
            git: Some(git_identity()),
            backend,
            allow_debug_benchmark: false,
            allow_version_skew: resolved.requested.allow_version_skew,
            allow_degraded_cold: resolved.requested.allow_degraded_cold,
            cv_threshold: resolved.requested.cv_threshold,
            runtime_flavor: None,
            boot_source: None,
            boot_event_num: None,
            pma_work_dir_mode: None,
            pma_fsync_mode: None,
            binary: resolved.binary.clone(),
            fixture_path: resolved.absolute_fixture_path.clone(),
            fixture_sha256_hex: resolved.fixture_sha256_hex.clone(),
            fixture_manifest: resolved.fixture_manifest.clone(),
        }
    }

    fn trusted_run_result(
        fixture_sha256_hex: &str,
        threads: u32,
        case_validity: Validity,
    ) -> TrustedRunResult {
        let requested = RequestedCase {
            threads,
            warmup_runs: 0,
            measured_runs: 3,
            cooldown_secs: 0,
            ..RequestedCase::native(PathBuf::from("fixture.soltest"))
        };
        let resolved = ResolvedCase {
            schema_version: RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            requested: requested.clone(),
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate::for_requested(&requested),
            absolute_fixture_path: PathBuf::from("/tmp/fixture.soltest"),
            fixture_sha256_hex: fixture_sha256_hex.to_string(),
            fixture_manifest: fixture_manifest(),
            execution_config: ExecutionConfig::default(),
            binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("abc123".to_string()),
            },
            docker: Some(DockerResolvedConfig {
                image: resolved_docker_image(),
                requested_memory_limit_bytes: 4 * 1024 * 1024 * 1024,
                cpuset: Some("0-3".to_string()),
                cpu_quota: Some(200_000),
                cpu_period: Some(100_000),
                work_dir_mode: WorkDirMode::DockerTmpfs,
                allow_version_skew: false,
            }),
        };
        TrustedRunResult {
            resolved: resolved.clone(),
            provenance: base_provenance(
                &resolved,
                docker_runtime_facts(&resolved.binary, format!("container-{threads}")),
            ),
            summary: RunSummary {
                schema_version: SUMMARY_SCHEMA_VERSION.to_string(),
                benchmark: "sol-orchestrate".to_string(),
                measured_runs_requested: 3,
                measured_runs_succeeded: 3,
                failed_runs: Vec::new(),
                aggregate: Default::default(),
                by_step_type: Default::default(),
                steps: Vec::new(),
                steps_per_second: None,
                block_pokes_per_second: None,
                pokes_per_second: Some(ValueStats {
                    median: 100.0 + threads as f64,
                    min: 90.0,
                    max: 110.0,
                    mad: 5.0,
                    stddev: 3.0,
                    cv: 0.03,
                    values: vec![90.0, 100.0 + threads as f64, 110.0],
                }),
                raw_tx_pokes_per_second: None,
                peeks_per_second: None,
                cold_peeks_per_second: None,
                init_time_secs: None,
                total_step_time_secs: None,
                average_block_time_ms: None,
                peak_process_rss_bytes: None,
                minor_faults_total: None,
                major_faults_total: None,
            },
            verdict: Verdict {
                schema_version: VERDICT_SCHEMA_VERSION.to_string(),
                allow_debug_benchmark: false,
                allow_version_skew: false,
                allow_degraded_cold: false,
                cv_threshold: 0.10,
                validity: case_validity,
            },
        }
    }

    fn native_trusted_run_result(case_validity: Validity) -> TrustedRunResult {
        let requested = RequestedCase {
            warmup_runs: 0,
            measured_runs: 3,
            cooldown_secs: 0,
            ..RequestedCase::native(PathBuf::from("fixture.soltest"))
        };
        let resolved = ResolvedCase {
            schema_version: RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            requested: requested.clone(),
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate::for_requested(&requested),
            absolute_fixture_path: PathBuf::from("/tmp/fixture.soltest"),
            fixture_sha256_hex: "fixture-a".to_string(),
            fixture_manifest: fixture_manifest(),
            execution_config: ExecutionConfig::default(),
            binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("abc123".to_string()),
            },
            docker: None,
        };
        TrustedRunResult {
            resolved: resolved.clone(),
            provenance: base_provenance(&resolved, BackendRuntimeFacts::Native)
                .with_pma_replay_provenance(PmaReplayProvenance::checkpoint(
                    resolved.fixture_manifest.checkpoint_event_num,
                )),
            summary: RunSummary {
                schema_version: SUMMARY_SCHEMA_VERSION.to_string(),
                benchmark: "sol-orchestrate".to_string(),
                measured_runs_requested: 3,
                measured_runs_succeeded: 3,
                failed_runs: Vec::new(),
                aggregate: Default::default(),
                by_step_type: Default::default(),
                steps: Vec::new(),
                steps_per_second: None,
                block_pokes_per_second: None,
                pokes_per_second: Some(ValueStats {
                    median: 100.0,
                    min: 90.0,
                    max: 110.0,
                    mad: 5.0,
                    stddev: 3.0,
                    cv: 0.03,
                    values: vec![90.0, 100.0, 110.0],
                }),
                raw_tx_pokes_per_second: None,
                peeks_per_second: None,
                cold_peeks_per_second: None,
                init_time_secs: None,
                total_step_time_secs: None,
                average_block_time_ms: None,
                peak_process_rss_bytes: None,
                minor_faults_total: None,
                major_faults_total: None,
            },
            verdict: Verdict {
                schema_version: VERDICT_SCHEMA_VERSION.to_string(),
                allow_debug_benchmark: false,
                allow_version_skew: false,
                allow_degraded_cold: false,
                cv_threshold: 0.10,
                validity: case_validity,
            },
        }
    }

    fn docker_pma_trusted_run_result(case_validity: Validity) -> TrustedRunResult {
        let mut result = trusted_run_result("fixture-a", 1, case_validity);
        let work_dir_mode = result
            .resolved
            .docker
            .as_ref()
            .expect("docker config")
            .work_dir_mode
            .clone();
        result.provenance = result.provenance.with_pma_replay_provenance(
            PmaReplayProvenance::checkpoint(result.resolved.fixture_manifest.checkpoint_event_num)
                .with_work_dir_mode(&work_dir_mode),
        );
        result
    }

    #[test]
    fn sweep_comparison_marks_non_axis_drift_invalid() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-threads_1".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(1))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-threads_2".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(2))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let case_runs = vec![
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: trusted_run_result("fixture-a", 1, Validity::Valid),
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-threads_2"),
                result: trusted_run_result("fixture-b", 2, Validity::Valid),
            },
        ];

        let comparison = build_comparison(&case_runs).expect("comparison");
        let verdict = derive_sweep_verdict(&comparison);

        assert_eq!(comparison.invariant_violations.len(), 1);
        assert!(comparison.invariant_violations[0].contains("fixture_sha256_hex"));
        assert!(matches!(verdict.validity, Validity::Invalid { .. }));
    }

    #[test]
    fn sweep_comparison_splits_mixed_backend_groups() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-execution_native".to_string(),
                axis_assignments: BTreeMap::from([(
                    "execution_mode".to_string(),
                    AxisValue::String("native".to_string()),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-execution_docker".to_string(),
                axis_assignments: BTreeMap::from([(
                    "execution_mode".to_string(),
                    AxisValue::String("docker".to_string()),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let case_runs = vec![
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-execution_native"),
                result: native_trusted_run_result(Validity::Valid),
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-execution_docker"),
                result: trusted_run_result("fixture-a", 1, Validity::Valid),
            },
        ];

        let comparison = build_comparison(&case_runs).expect("comparison");
        let groups = comparison
            .backend_groups
            .iter()
            .map(|group| (group.backend.as_str(), group.case_count))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(groups, BTreeMap::from([("docker", 1), ("native", 1)]));
        assert!(comparison.invariant_violations.is_empty());
        assert!(comparison.aggregate.is_empty());
        assert!(comparison.by_step_type.is_empty());
        assert!(comparison
            .backend_groups
            .iter()
            .all(|group| group.aggregate.contains_key("pokes_per_second")));
        assert!(comparison.common_steps.is_empty());
        assert!(comparison
            .non_comparable_metrics
            .iter()
            .any(|metric| metric.metric == "aggregate"));

        let serialized = serde_json::to_value(&comparison).expect("comparison json");
        let keys = serialized
            .as_object()
            .expect("comparison object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "aggregate".to_string(),
                "by_step_type".to_string(),
                "common_steps".to_string(),
                "non_comparable_metrics".to_string(),
                "schema_version".to_string(),
            ])
        );
    }

    #[test]
    fn sweep_verdict_includes_mixed_backend_group_invariant_violations() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-execution_native".to_string(),
                axis_assignments: BTreeMap::from([(
                    "execution_mode".to_string(),
                    AxisValue::String("native".to_string()),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-execution_docker".to_string(),
                axis_assignments: BTreeMap::from([(
                    "execution_mode".to_string(),
                    AxisValue::String("docker".to_string()),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 2,
                case_id: "case-002-execution_docker".to_string(),
                axis_assignments: BTreeMap::from([(
                    "execution_mode".to_string(),
                    AxisValue::String("docker".to_string()),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let case_runs = vec![
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-execution_native"),
                result: native_trusted_run_result(Validity::Valid),
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-execution_docker"),
                result: trusted_run_result("fixture-a", 1, Validity::Valid),
            },
            SweepCaseRun {
                expanded_case: expanded_cases[2].clone(),
                output_root: PathBuf::from("/tmp/cases/case-002-execution_docker"),
                result: trusted_run_result("fixture-b", 1, Validity::Valid),
            },
        ];

        let comparison = build_comparison(&case_runs).expect("comparison");
        let verdict = derive_sweep_verdict(&comparison);

        assert!(comparison.invariant_violations.is_empty());
        assert!(comparison
            .backend_groups
            .iter()
            .any(|group| group.backend == "docker"
                && group
                    .invariant_violations
                    .iter()
                    .any(|reason| reason.contains("fixture_sha256_hex"))));
        match verdict.validity {
            Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("docker backend")
                        && reason.contains("fixture_sha256_hex")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
    }

    #[test]
    fn sweep_comparison_allows_fixture_axis_step_signature_drift() {
        let baseline = trusted_run_result("fixture-a", 1, Validity::Valid);
        let mut varied = trusted_run_result("fixture-b", 1, Validity::Valid);
        varied.resolved.orchestrate.step_signature_sha256_hex = Some("fixture-b-steps".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-fixture_a".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-a".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-a.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-fixture_a"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-fixture_b".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-b".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-b.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-fixture_b"),
                result: varied,
            },
        ])
        .expect("comparison");
        let verdict = derive_sweep_verdict(&comparison);

        assert!(comparison.invariant_violations.is_empty());
        assert!(matches!(verdict.validity, Validity::Valid));
        assert!(comparison
            .non_comparable_metrics
            .iter()
            .any(|metric| metric.metric == "common_steps"
                && metric.reason.contains("step signatures differ")));
    }

    #[test]
    fn sweep_comparison_flags_missing_non_axis_invariants() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-threads_1".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(1))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-threads_2".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(2))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let baseline = trusted_run_result("fixture-a", 1, Validity::Valid);
        let mut drifted = trusted_run_result("fixture-a", 2, Validity::Valid);
        drifted.resolved.requested.profile_memory = true;
        drifted.resolved.binary.version = "0.2.0".to_string();
        drifted.resolved.binary.git_commit = Some("def456".to_string());
        drifted.provenance.binary.git_commit = Some("def456".to_string());
        drifted
            .resolved
            .docker
            .as_mut()
            .expect("docker config")
            .work_dir_mode = WorkDirMode::HostBind;
        drifted
            .resolved
            .docker
            .as_mut()
            .expect("docker config")
            .allow_version_skew = true;
        drifted.provenance.git.as_mut().expect("git identity").dirty = true;
        if let BackendRuntimeFacts::Docker {
            host_binary,
            container_binary,
            ..
        } = &mut drifted.provenance.backend
        {
            host_binary.version = "0.2.0".to_string();
            container_binary.version = "0.2.0".to_string();
        }

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-threads_2"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("profile_memory")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("binary.version")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("binary.git_commit")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.git.dirty")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("docker.work_dir_mode")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("docker.allow_version_skew")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("backend.host_binary")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("backend.container_binary")));
    }

    #[test]
    fn sweep_comparison_flags_non_axis_fsync_drift() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-threads_1".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(1))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-threads_2".to_string(),
                axis_assignments: BTreeMap::from([("threads".to_string(), AxisValue::Integer(2))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut drifted = native_trusted_run_result(Validity::Valid);
        drifted.resolved.requested.set_fsync_enabled(false);

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-threads_2"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("fsync")));
    }

    #[test]
    fn sweep_comparison_allows_fsync_axis_drift() {
        let expanded_cases = vec![
            ExpandedCase {
                case_index: 0,
                case_id: "case-000-fsync_true".to_string(),
                axis_assignments: BTreeMap::from([("fsync".to_string(), AxisValue::Boolean(true))]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
            ExpandedCase {
                case_index: 1,
                case_id: "case-001-fsync_false".to_string(),
                axis_assignments: BTreeMap::from([(
                    "fsync".to_string(),
                    AxisValue::Boolean(false),
                )]),
                requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            },
        ];
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut drifted = native_trusted_run_result(Validity::Valid);
        drifted.resolved.requested.set_fsync_enabled(false);

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: expanded_cases[0].clone(),
                output_root: PathBuf::from("/tmp/cases/case-000-fsync_true"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: expanded_cases[1].clone(),
                output_root: PathBuf::from("/tmp/cases/case-001-fsync_false"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("fsync")));
    }

    #[test]
    fn comparison_flags_runtime_flavor_drift_as_non_axis_change() {
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut drifted = native_trusted_run_result(Validity::Valid);
        drifted.provenance.runtime_flavor = Some("alternate-runtime".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-threads_1"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.runtime_flavor")));
    }

    #[test]
    fn comparison_flags_boot_source_drift_as_non_axis_change() {
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut drifted = native_trusted_run_result(Validity::Valid);
        drifted.provenance.boot_source = Some("snapshot".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-threads_1"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.boot_source")));
    }

    #[test]
    fn comparison_flags_boot_event_num_drift_as_non_axis_change() {
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut drifted = native_trusted_run_result(Validity::Valid);
        drifted.provenance.boot_event_num = Some(999);

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-threads_1"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.boot_event_num")));
    }

    #[test]
    fn comparison_flags_boot_event_num_drift_allows_fixture_axis() {
        let baseline = native_trusted_run_result(Validity::Valid);
        let mut varied = native_trusted_run_result(Validity::Valid);
        varied.resolved.fixture_sha256_hex = "fixture-b".to_string();
        varied.resolved.fixture_manifest.checkpoint_event_num = 11;
        varied.provenance.fixture_sha256_hex = "fixture-b".to_string();
        varied.provenance.fixture_manifest.checkpoint_event_num = 11;
        varied.provenance.boot_event_num = Some(11);

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-fixture_a".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-a".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-a.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-fixture_a"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-fixture_b".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-b".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-b.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-fixture_b"),
                result: varied,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.boot_event_num")));
    }

    #[test]
    fn comparison_flags_pma_work_dir_mode_drift_as_non_axis_change() {
        let baseline = docker_pma_trusted_run_result(Validity::Valid);
        let mut drifted = docker_pma_trusted_run_result(Validity::Valid);
        drifted.provenance.pma_work_dir_mode = Some("host_bind".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-threads_1"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.pma_work_dir_mode")));
    }

    #[test]
    fn comparison_flags_pma_work_dir_mode_drift_even_with_fixture_axis() {
        let baseline = docker_pma_trusted_run_result(Validity::Valid);
        let mut varied = docker_pma_trusted_run_result(Validity::Valid);
        varied.resolved.fixture_sha256_hex = "fixture-b".to_string();
        varied.resolved.fixture_manifest.checkpoint_event_num = 11;
        varied.provenance.fixture_sha256_hex = "fixture-b".to_string();
        varied.provenance.fixture_manifest.checkpoint_event_num = 11;
        varied.provenance.boot_event_num = Some(11);
        varied.provenance.pma_work_dir_mode = Some("docker_volume".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-fixture_a".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-a".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-a.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-fixture_a"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-fixture_b".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "fixture".to_string(),
                        AxisValue::String("fixture-b".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture-b.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-fixture_b"),
                result: varied,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.boot_event_num")));
        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.pma_work_dir_mode")));
    }

    #[test]
    fn comparison_allows_pma_work_dir_mode_drift_when_work_dir_mode_is_axis() {
        let baseline = docker_pma_trusted_run_result(Validity::Valid);
        let mut varied = docker_pma_trusted_run_result(Validity::Valid);
        varied
            .resolved
            .docker
            .as_mut()
            .expect("docker config")
            .work_dir_mode = WorkDirMode::HostBind;
        varied.provenance.pma_work_dir_mode = Some("host_bind".to_string());

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-work_dir_mode_docker_tmpfs".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "work_dir_mode".to_string(),
                        AxisValue::String("DockerTmpfs".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-work_dir_mode_docker_tmpfs"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-work_dir_mode_host_bind".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "work_dir_mode".to_string(),
                        AxisValue::String("HostBind".to_string()),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-work_dir_mode_host_bind"),
                result: varied,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("provenance.pma_work_dir_mode")));
        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("docker.work_dir_mode")));
    }

    #[test]
    fn sweep_comparison_allows_cpu_period_axis_to_change_realized_cpu_max() {
        let baseline = trusted_run_result("fixture-a", 1, Validity::Valid);
        let mut varied = trusted_run_result("fixture-a", 1, Validity::Valid);
        varied
            .resolved
            .docker
            .as_mut()
            .expect("docker config")
            .cpu_period = Some(50_000);
        if let BackendRuntimeFacts::Docker {
            realized_cpu_max, ..
        } = &mut varied.provenance.backend
        {
            *realized_cpu_max = Some("200000 50000".to_string());
        }

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-cpu_period_100000".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "cpu_period".to_string(),
                        AxisValue::Integer(100_000),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-cpu_period_100000"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-cpu_period_50000".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "cpu_period".to_string(),
                        AxisValue::Integer(50_000),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-cpu_period_50000"),
                result: varied,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("backend.realized_cpu_max")));
    }

    #[test]
    fn image_drift_is_allowed_when_image_is_the_axis() {
        let baseline = trusted_run_result("fixture-a", 1, Validity::Valid);
        let mut varied = trusted_run_result("fixture-a", 1, Validity::Valid);
        if let BackendRuntimeFacts::Docker { image_digest, .. } = &mut varied.provenance.backend {
            *image_digest = "sha256:other".to_string();
        }

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-image_a".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "image".to_string(),
                        AxisValue::Object(serde_json::Map::from_iter([(
                            "provided".to_string(),
                            serde_json::json!({
                                "ref": "ghcr.io/org/nockchain-bench@sha256:a"
                            }),
                        )])),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-image_a"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-image_b".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "image".to_string(),
                        AxisValue::Object(serde_json::Map::from_iter([(
                            "provided".to_string(),
                            serde_json::json!({
                                "ref": "ghcr.io/org/nockchain-bench@sha256:b"
                            }),
                        )])),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-image_b"),
                result: varied,
            },
        ])
        .expect("comparison");

        assert!(!comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("backend.image_digest")));
    }

    #[test]
    fn image_drift_is_invalid_when_image_is_not_the_axis() {
        let baseline = trusted_run_result("fixture-a", 1, Validity::Valid);
        let mut drifted = trusted_run_result("fixture-a", 1, Validity::Valid);
        if let BackendRuntimeFacts::Docker { image_digest, .. } = &mut drifted.provenance.backend {
            *image_digest = "sha256:other".to_string();
        }

        let comparison = build_comparison(&[
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 0,
                    case_id: "case-000-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-000-threads_1"),
                result: baseline,
            },
            SweepCaseRun {
                expanded_case: ExpandedCase {
                    case_index: 1,
                    case_id: "case-001-threads_1".to_string(),
                    axis_assignments: BTreeMap::from([(
                        "threads".to_string(),
                        AxisValue::Integer(1),
                    )]),
                    requested_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
                },
                output_root: PathBuf::from("/tmp/cases/case-001-threads_1"),
                result: drifted,
            },
        ])
        .expect("comparison");

        assert!(comparison
            .invariant_violations
            .iter()
            .any(|reason| reason.contains("backend.image_digest")));
    }

    #[tokio::test]
    async fn sweep_rejects_cpu_profiling_metadata() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("sweep");
        let matrix = SweepMatrix {
            base_case: RequestedCase {
                cooldown_secs: 0,
                ..RequestedCase::native(PathBuf::from("fixture.soltest"))
            },
            axes: BTreeMap::from([(
                "threads".to_string(),
                vec![AxisValue::Integer(1), AxisValue::Integer(2)],
            )]),
        };
        let matrix_json = serde_json::to_value(&matrix).expect("matrix json");
        let baseline_runs = vec![
            Ok(trusted_run_result("fixture-a", 1, Validity::Valid)),
            Ok(trusted_run_result("fixture-a", 2, Validity::Valid)),
        ];
        let mut baseline_executor = FakeExecutor::new(baseline_runs);
        let baseline = execute_sweep(
            &matrix_json,
            matrix.clone(),
            &output_root.join("baseline"),
            &SweepRunOptions {
                cpu_profiler: None,
                ..SweepRunOptions::default()
            },
            &mut baseline_executor,
        )
        .await
        .expect("baseline sweep");

        let mut profiled_executor = FakeExecutor::new(Vec::new());
        let error = execute_sweep(
            &matrix_json,
            matrix,
            &output_root.join("profiled"),
            &SweepRunOptions {
                cpu_profiler: Some(CpuProfilerConfig {
                    kind: CpuProfilerKind::Samply,
                    sample_rate_hz: 1000,
                }),
                ..SweepRunOptions::default()
            },
            &mut profiled_executor,
        )
        .await
        .expect_err("profiled trusted sweep should be rejected");

        assert_eq!(baseline.comparison.case_count, 2);
        assert!(
            error.to_string().contains("do not support CPU profiling"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn sweep_rejects_cpu_profiling_config_for_docker_cases() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("sweep");
        let matrix = SweepMatrix {
            base_case: RequestedCase {
                cooldown_secs: 0,
                execution: ExecutionRequest::Docker {
                    image: DockerImageSource::AutoBuild {
                        tag: "nockchain-bench:test".to_string(),
                    },
                    memory_limit: "4g".to_string(),
                    cpuset: None,
                    cpu_quota: None,
                    cpu_period: None,
                    work_dir_mode: WorkDirMode::DockerTmpfs,
                    allow_version_skew: false,
                },
                ..RequestedCase::native(PathBuf::from("fixture.soltest"))
            },
            axes: BTreeMap::from([("threads".to_string(), vec![AxisValue::Integer(1)])]),
        };
        let matrix_json = serde_json::to_value(&matrix).expect("matrix json");
        let profiler = CpuProfilerConfig {
            kind: CpuProfilerKind::Samply,
            sample_rate_hz: 1_000,
        };
        let mut executor = FakeExecutor::new(vec![Ok(trusted_run_result(
            "fixture-a",
            1,
            Validity::Valid,
        ))]);
        let error = execute_sweep(
            &matrix_json,
            matrix,
            &output_root,
            &SweepRunOptions {
                cpu_profiler: Some(profiler.clone()),
                ..SweepRunOptions::default()
            },
            &mut executor,
        )
        .await
        .expect_err("docker trusted sweep CPU profiling should be rejected");

        assert!(
            error.to_string().contains("do not support CPU profiling"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn docker_image_matrix_parses_provided_image_source() {
        let value = serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest",
                "threads": 4,
                "warmup_runs": 1,
                "measured_runs": 3,
                "cooldown_secs": 0,
                "mode": {
                    "docker": {
                        "image": {
                            "provided": {
                                "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                            }
                        },
                        "memory_limit": "8g",
                        "work_dir_mode": "DockerTmpfs"
                    }
                }
            },
            "axes": {
                "memory_limit": ["4g", "8g"]
            }
        });

        let matrix = parse_matrix_value(value).expect("parse matrix");

        assert_eq!(matrix.base_case.threads, 4);
        assert_eq!(matrix.base_case.measured_runs, 3);
        assert_eq!(
            serde_json::to_value(&matrix.base_case.execution).expect("serialize execution"),
            serde_json::json!({
                "Docker": {
                    "image": {
                        "provided": {
                            "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                        }
                    },
                    "memory_limit": "8g",
                    "cpuset": null,
                    "cpu_quota": null,
                    "cpu_period": null,
                    "work_dir_mode": "DockerTmpfs",
                    "allow_version_skew": false
                }
            })
        );
        assert_eq!(
            matrix.axes.get("memory_limit"),
            Some(&vec![
                AxisValue::String("4g".to_string()),
                AxisValue::String("8g".to_string()),
            ])
        );
    }

    #[test]
    fn sweep_matrix_parses_plan_and_read_sources() {
        let plan = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "plan": "trusted-plan.json",
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "axes": {
                "threads": [1]
            }
        }))
        .expect("plan matrix");
        assert!(matches!(
            plan.base_case.orchestrate,
            RequestedOrchestrate::PlanFile { .. }
        ));

        let read = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "checkpoint": "checkpoint.chkjam",
                "kernel": "kernel.jam",
                "start_height": 10,
                "count": 3,
                "peek_mode": "cold-each",
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "axes": {
                "peek_mode": ["warm", "cold_each"]
            }
        }))
        .expect("read matrix");
        let expanded = expand_matrix(&read).expect("expand read matrix");
        assert!(expanded.iter().any(|case| matches!(
            case.requested_case.orchestrate,
            RequestedOrchestrate::GeneratedRead {
                peek_mode: PeekMode::Warm,
                ..
            }
        )));
        assert!(expanded.iter().any(|case| matches!(
            case.requested_case.orchestrate,
            RequestedOrchestrate::GeneratedRead {
                peek_mode: PeekMode::ColdEach,
                ..
            }
        )));
    }

    #[test]
    fn sweep_matrix_parses_snapshot_read_base() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "snapshot": {
                    "pma": "snapshot.pma",
                    "manifest": "snapshot.manifest"
                },
                "kernel": "kernel.jam",
                "start_height": 10,
                "count": 3,
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "axes": {
                "peek_mode": ["warm"]
            }
        }))
        .expect("snapshot read matrix");
        let expanded = expand_matrix(&matrix).expect("expand snapshot read matrix");

        assert!(matches!(
            &expanded[0].requested_case.orchestrate,
            RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot { pma, manifest },
                kernel_path,
                ..
            } if pma == &PathBuf::from("snapshot.pma")
                && manifest == &PathBuf::from("snapshot.manifest")
                && kernel_path == &PathBuf::from("kernel.jam")
        ));
    }

    #[test]
    fn sweep_matrix_treats_snapshot_axis_as_atomic_boot_source() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "checkpoint": "checkpoint.chkjam",
                "kernel": "kernel.jam",
                "start_height": 10,
                "count": 3,
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "axes": {
                "snapshot": [
                    {
                        "pma": "snapshots/a/snapshot.pma",
                        "manifest": "snapshots/a/snapshot.manifest"
                    },
                    {
                        "pma": "snapshots/b/snapshot.pma",
                        "manifest": "snapshots/b/snapshot.manifest"
                    }
                ]
            }
        }))
        .expect("snapshot axis matrix");
        let expanded = expand_matrix(&matrix).expect("expand snapshot axis matrix");

        assert_eq!(expanded.len(), 2);
        assert!(expanded
            .iter()
            .any(|case| case.case_id == "case-000-snapshot_a-snapshot"));
        assert!(expanded
            .iter()
            .any(|case| case.case_id == "case-001-snapshot_b-snapshot"));
        assert!(matches!(
            &expanded[1].requested_case.orchestrate,
            RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot { pma, manifest },
                ..
            } if pma == &PathBuf::from("snapshots/b/snapshot.pma")
                && manifest == &PathBuf::from("snapshots/b/snapshot.manifest")
        ));
    }

    #[test]
    fn sweep_matrix_rejects_independent_snapshot_file_axes() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "checkpoint": "checkpoint.chkjam",
                "kernel": "kernel.jam",
                "start_height": 10,
                "count": 3
            },
            "axes": {
                "snapshot.pma": ["a.pma"],
                "snapshot.manifest": ["a.manifest"]
            }
        }))
        .expect("matrix");

        let error = expand_matrix(&matrix).expect_err("independent snapshot axes rejected");
        assert!(
            error
                .to_string()
                .contains("sweep snapshot axes must vary the atomic `snapshot` object"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sweep_matrix_supports_mode_and_policy_axes() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest",
                "measured_runs": 3,
                "cooldown_secs": 0
            },
            "axes": {
                "mode": [
                    { "native": {} },
                    {
                        "docker": {
                            "image": { "provided": { "ref": "nockchain-bench:local" } },
                            "memory_limit": "8g",
                            "work_dir_mode": "DockerTmpfs"
                        }
                    }
                ],
                "allow_degraded_cold": [false, true]
            }
        }))
        .expect("mixed mode matrix");
        let expanded = expand_matrix(&matrix).expect("expand matrix");

        assert!(expanded
            .iter()
            .any(|case| matches!(case.requested_case.execution, ExecutionRequest::Native)));
        assert!(expanded.iter().any(|case| matches!(
            case.requested_case.execution,
            ExecutionRequest::Docker { .. }
        )));
        assert!(expanded
            .iter()
            .any(|case| case.requested_case.allow_degraded_cold));
    }

    #[test]
    fn sweep_matrix_rejects_allow_debug_benchmark_axis() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest"
            },
            "axes": {
                "allow_debug_benchmark": [true]
            }
        }))
        .expect("matrix");

        let error = expand_matrix(&matrix).expect_err("policy axis rejected");
        assert!(
            error.to_string().contains("allow_debug_benchmark"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn docker_image_matrix_parses_auto_build_image_source() {
        let value = serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "./fixtures/test.soltest",
                "mode": {
                    "docker": {
                        "image": {
                            "auto_build": {
                                "tag": "nockchain-bench:local"
                            }
                        },
                        "memory_limit": "8g",
                        "work_dir_mode": "DockerTmpfs"
                    }
                }
            },
            "axes": {
                "memory_limit": ["8g"]
            }
        });

        let matrix = parse_matrix_value(value).expect("parse matrix");

        assert_eq!(
            serde_json::to_value(&matrix.base_case.execution).expect("serialize execution"),
            serde_json::json!({
                "Docker": {
                    "image": {
                        "auto_build": {
                            "tag": "nockchain-bench:local"
                        }
                    },
                    "memory_limit": "8g",
                    "cpuset": null,
                    "cpu_quota": null,
                    "cpu_period": null,
                    "work_dir_mode": "DockerTmpfs",
                    "allow_version_skew": false
                }
            })
        );
    }

    #[test]
    fn docker_image_matrix_rejects_ambiguous_image_source() {
        let value = serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "./fixtures/test.soltest",
                "mode": {
                    "docker": {
                        "image": {
                            "provided": {
                                "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                            },
                            "auto_build": {
                                "tag": "nockchain-bench:local"
                            }
                        },
                        "memory_limit": "8g",
                        "work_dir_mode": "DockerTmpfs"
                    }
                }
            },
            "axes": {
                "memory_limit": ["8g"]
            }
        });

        let error = parse_matrix_value(value).expect_err("ambiguous image source should fail");
        assert!(
            error
                .to_string()
                .contains("sweep docker image must not specify both `provided` and `auto_build`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn docker_image_matrix_rejects_auto_build_image_axis() {
        let value = serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "./fixtures/test.soltest",
                "mode": {
                    "docker": {
                        "image": {
                            "provided": {
                                "ref": "ghcr.io/org/nockchain-bench@sha256:abc"
                            }
                        },
                        "memory_limit": "8g",
                        "work_dir_mode": "DockerTmpfs"
                    }
                }
            },
            "axes": {
                "image": [
                    {
                        "auto_build": {
                            "tag": "nockchain-bench:local"
                        }
                    }
                ]
            }
        });

        let matrix = parse_matrix_value(value).expect("parse matrix");
        let error = expand_matrix(&matrix).expect_err("auto_build image axis should fail");
        assert!(
            error
                .to_string()
                .contains("sweep axis `image` only accepts provided image values"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sweep_base_case_sets_fsync() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest",
                "fsync": false
            },
            "axes": {
                "threads": [1]
            }
        }))
        .expect("parse matrix");

        assert!(!matrix.base_case.fsync_enabled());
    }

    #[test]
    fn sweep_base_case_defaults_fsync_on_when_field_is_missing() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest"
            },
            "axes": {
                "threads": [1]
            }
        }))
        .expect("parse matrix");

        assert!(matrix.base_case.fsync_enabled());
    }

    #[test]
    fn sweep_expands_fsync_axis() {
        let matrix = parse_matrix_value(serde_json::json!({
            "benchmark": "sol-orchestrate",
            "base": {
                "fixture": "fixture.soltest",
                "mode": { "native": {} }
            },
            "axes": {
                "fsync": [true, false]
            }
        }))
        .expect("parse matrix");

        let expanded = expand_matrix(&matrix).expect("expand matrix");

        assert_eq!(expanded.len(), 2);
        assert!(expanded
            .iter()
            .any(|case| case.requested_case.fsync_enabled()));
        assert!(expanded
            .iter()
            .any(|case| !case.requested_case.fsync_enabled()));
    }

    #[test]
    fn sweep_expand_matrix_rejects_unknown_native_axis_as_unsupported() {
        let matrix = SweepMatrix {
            base_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            axes: BTreeMap::from([("bogus".to_string(), vec![AxisValue::Integer(1)])]),
        };

        let error = expand_matrix(&matrix).expect_err("unknown axis");

        assert!(
            error.to_string().contains("unsupported sweep axis `bogus`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sweep_expand_matrix_rejects_docker_axis_for_native_execution() {
        let matrix = SweepMatrix {
            base_case: RequestedCase::native(PathBuf::from("fixture.soltest")),
            axes: BTreeMap::from([(
                "memory_limit".to_string(),
                vec![AxisValue::String("4g".to_string())],
            )]),
        };

        let error = expand_matrix(&matrix).expect_err("docker-only axis");

        assert!(
            error
                .to_string()
                .contains("sweep axis `memory_limit` requires Docker execution"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn sweep_execution_writes_top_level_artifacts_and_case_outputs() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("sweep");
        let matrix = SweepMatrix {
            base_case: RequestedCase {
                measured_runs: 3,
                cooldown_secs: 0,
                execution: ExecutionRequest::Docker {
                    image: DockerImageSource::AutoBuild {
                        tag: "nockchain-bench:test".to_string(),
                    },
                    memory_limit: "4g".to_string(),
                    cpuset: Some("0-3".to_string()),
                    cpu_quota: Some(200_000),
                    cpu_period: Some(100_000),
                    work_dir_mode: WorkDirMode::DockerTmpfs,
                    allow_version_skew: false,
                },
                ..RequestedCase::native(PathBuf::from("fixture.soltest"))
            },
            axes: BTreeMap::from([(
                "threads".to_string(),
                vec![AxisValue::Integer(1), AxisValue::Integer(2)],
            )]),
        };
        let matrix_json = serde_json::to_value(&matrix).expect("matrix json");
        let mut executor = FakeExecutor::new(vec![
            Ok(trusted_run_result("fixture-a", 1, Validity::Valid)),
            Ok(trusted_run_result(
                "fixture-a",
                2,
                Validity::Partial {
                    reasons: vec!["throughput CV high".to_string()],
                },
            )),
        ]);
        let seen_paths = executor.seen_paths();

        let result = execute_sweep(
            &matrix_json,
            matrix,
            &output_root,
            &SweepRunOptions {
                schedule_mode: ScheduleMode::Sequential,
                comparison_markdown: true,
                ..SweepRunOptions::default()
            },
            &mut executor,
        )
        .await
        .expect("execute sweep");

        assert_eq!(result.comparison.cases.len(), 2);
        assert_eq!(result.comparison.schema_version, COMPARISON_SCHEMA_VERSION);
        assert!(result.comparison.non_comparable_metrics.is_empty());
        assert!(output_root.join("schema_version.txt").exists());
        assert!(output_root.join("matrix.json").exists());
        assert!(output_root.join("matrix_expanded.json").exists());
        assert!(output_root.join("schedule.json").exists());
        assert!(output_root.join("comparison.json").exists());
        assert!(output_root.join("comparison.md").exists());
        assert!(output_root.join("verdict.json").exists());

        let seen_paths = seen_paths.lock().expect("seen paths");
        assert_eq!(
            seen_paths.as_slice(),
            &[
                output_root.join("cases/case-000-threads_1"),
                output_root.join("cases/case-001-threads_2"),
            ]
        );
        assert!(matches!(result.verdict.validity, Validity::Partial { .. }));
    }

    #[tokio::test]
    async fn sweep_execution_continues_after_case_failure_and_records_it() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("sweep");
        let matrix = SweepMatrix {
            base_case: RequestedCase {
                measured_runs: 3,
                cooldown_secs: 0,
                execution: ExecutionRequest::Docker {
                    image: DockerImageSource::AutoBuild {
                        tag: "nockchain-bench:test".to_string(),
                    },
                    memory_limit: "4g".to_string(),
                    cpuset: Some("0-3".to_string()),
                    cpu_quota: Some(200_000),
                    cpu_period: Some(100_000),
                    work_dir_mode: WorkDirMode::DockerTmpfs,
                    allow_version_skew: false,
                },
                ..RequestedCase::native(PathBuf::from("fixture.soltest"))
            },
            axes: BTreeMap::from([(
                "threads".to_string(),
                vec![AxisValue::Integer(1), AxisValue::Integer(2), AxisValue::Integer(3)],
            )]),
        };
        let matrix_json = serde_json::to_value(&matrix).expect("matrix json");
        let mut executor = FakeExecutor::new(vec![
            Ok(trusted_run_result("fixture-a", 1, Validity::Valid)),
            Err(HarnessError::CommandFailure(
                "second case failed".to_string(),
            )),
            Ok(trusted_run_result("fixture-a", 3, Validity::Valid)),
        ]);
        let seen_paths = executor.seen_paths();

        let result = execute_sweep(
            &matrix_json,
            matrix,
            &output_root,
            &SweepRunOptions {
                schedule_mode: ScheduleMode::Sequential,
                comparison_markdown: true,
                ..SweepRunOptions::default()
            },
            &mut executor,
        )
        .await
        .expect("sweep should continue");

        let seen_paths = seen_paths.lock().expect("seen paths");
        assert_eq!(
            seen_paths.as_slice(),
            &[
                output_root.join("cases/case-000-threads_1"),
                output_root.join("cases/case-001-threads_2"),
                output_root.join("cases/case-002-threads_3"),
            ]
        );
        assert_eq!(result.comparison.cases.len(), 2);
        assert_eq!(result.comparison.failed_cases.len(), 1);
        assert_eq!(
            result.comparison.failed_cases[0].case_id,
            "case-001-threads_2"
        );
        assert!(result.comparison.failed_cases[0]
            .error
            .contains("second case failed"));
        let comparison: SweepComparison = serde_json::from_slice(
            &std::fs::read(output_root.join("comparison.json")).expect("comparison artifact"),
        )
        .expect("parse comparison");
        assert!(comparison.failed_cases.is_empty());
        let comparison_markdown = std::fs::read_to_string(output_root.join("comparison.md"))
            .expect("comparison markdown");
        assert!(comparison_markdown.contains("case-001-threads_2"));
        assert!(comparison_markdown.contains("second case failed"));
        let verdict: Verdict = serde_json::from_slice(
            &std::fs::read(output_root.join("verdict.json")).expect("verdict artifact"),
        )
        .expect("parse verdict");
        match verdict.validity {
            Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("case-001-threads_2")));
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("second case failed")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
        let failed_case_verdict: Verdict = serde_json::from_slice(
            &std::fs::read(output_root.join("cases/case-001-threads_2/verdict.json"))
                .expect("failed case verdict artifact"),
        )
        .expect("parse failed case verdict");
        match failed_case_verdict.validity {
            Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("second case failed")));
            }
            other => panic!("expected invalid failed case verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sweep_execution_allows_all_cases_to_fail_and_still_writes_comparison() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("sweep");
        let matrix = SweepMatrix {
            base_case: RequestedCase {
                measured_runs: 3,
                cooldown_secs: 0,
                ..RequestedCase::native(PathBuf::from("fixture.soltest"))
            },
            axes: BTreeMap::from([(
                "threads".to_string(),
                vec![AxisValue::Integer(1), AxisValue::Integer(2)],
            )]),
        };
        let matrix_json = serde_json::to_value(&matrix).expect("matrix json");
        let mut executor = FakeExecutor::new(vec![
            Err(HarnessError::CommandFailure(
                "first case failed".to_string(),
            )),
            Err(HarnessError::CommandFailure(
                "second case failed".to_string(),
            )),
        ]);

        let result = execute_sweep(
            &matrix_json,
            matrix,
            &output_root,
            &SweepRunOptions::default(),
            &mut executor,
        )
        .await
        .expect("sweep should still complete");

        assert!(output_root.join("comparison.json").exists());
        assert!(output_root.join("verdict.json").exists());
        let comparison: SweepComparison = serde_json::from_slice(
            &std::fs::read(output_root.join("comparison.json")).expect("comparison artifact"),
        )
        .expect("parse comparison");
        assert!(comparison.failed_cases.is_empty());
        assert_eq!(result.comparison.cases.len(), 0);
        assert_eq!(result.comparison.failed_cases.len(), 2);
        assert_eq!(
            result
                .comparison
                .failed_cases
                .iter()
                .map(|failed_case| failed_case.case_id.as_str())
                .collect::<Vec<_>>(),
            vec!["case-000-threads_1", "case-001-threads_2"]
        );
        match result.verdict.validity {
            Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("case-000-threads_1")));
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("first case failed")));
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("case-001-threads_2")));
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("second case failed")));
            }
            other => panic!("expected invalid sweep verdict, got {other:?}"),
        }
    }
}
