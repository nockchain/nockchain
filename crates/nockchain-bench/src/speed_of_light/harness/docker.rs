use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use bollard::container::Stats;
use bollard::Docker;
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinError;
use tokio::time::sleep;

use super::artifacts::{
    read_run_artifacts, write_container_samples, write_cpu_profile_artifact, write_host_env,
    write_requested_case, write_resolved_case, write_schema_version,
};
use super::case::{BinaryIdentity, ExecutionRequest, RequestedCase, ResolvedCase, WorkDirMode};
use super::docker_image::{
    resolve_docker_image, DockerImageSource, DockerImageVariant, ResolvedDockerImage,
};
use super::execute::{cpu_profile_output_relative_path, CompletedRun, CpuProfileExecutionKind};
use super::orchestrate::{
    execute_trusted_run, prepare_output_root, TrustedBackend, TrustedRunResult,
};
use super::profiler::{
    augment_perf_permission_guidance, build_run_once_command,
    cpu_profile_symbol_binary_relative_path, cpu_profile_symbol_dir_relative_path,
    invalidate_verdict_for_cpu_profiling_failure, validate_profiled_run, CpuProfilerLaunchRequest,
    CpuProfilerLauncher,
};
use super::provenance::{capture_host_env, BackendRuntimeFacts};
use super::validate::{
    persist_validation_record, read_validation_record, validate_cached_or_run,
    BackendValidationOutcome, ValidationCacheKey, ValidationProbeResult, ValidationRecord,
    ValidationStatus, VALIDATION_PROBE_VERSION,
};
use super::{
    cgroup_v2_path_from_proc_cgroup, resolve_requested_case, unix_timestamp_ms, CpuProfilerConfig,
    CpuProfilerKind, HarnessError,
};
use crate::speed_of_light::{ResolvedInput, TrustedPlan};

const CGROUP_V2_MEMORY_MAX_PATH: &str = "/sys/fs/cgroup/memory.max";
const CGROUP_V2_MEMORY_CURRENT_PATH: &str = "/sys/fs/cgroup/memory.current";
const CGROUP_V2_CPU_MAX_PATH: &str = "/sys/fs/cgroup/cpu.max";
const CGROUP_V2_CPUSET_EFFECTIVE_PATH: &str = "/sys/fs/cgroup/cpuset.cpus.effective";
const CGROUP_V2_CPUSET_PATH: &str = "/sys/fs/cgroup/cpuset.cpus";
const COLD_CGROUP_PARENT_ENV: &str = "NOCKCHAIN_BENCH_COLD_CGROUP_PARENT=/sys/fs/cgroup";
const UNRESOLVED_IMAGE_DIGEST: &str = "<unresolved>";

#[derive(Debug, Error)]
pub enum HarnessDockerError {
    #[error("Docker API error: {0}")]
    Api(#[from] bollard::errors::Error),

    #[error("Docker not available: {0}")]
    NotAvailable(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerStats {
    pub timestamp_ms: u64,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub memory_percent: f64,
    pub memory_cache_bytes: u64,
    pub memory_rss_bytes: u64,
    pub cpu_percent: f64,
    pub minor_faults: Option<u64>,
    pub major_faults: Option<u64>,
}

impl ContainerStats {
    pub fn from_docker_stats(stats: &Stats, start_time: Instant) -> Result<Self, HarnessError> {
        use bollard::container::MemoryStatsStats;

        let memory_usage = stats.memory_stats.usage.unwrap_or(0);
        let memory_limit = stats.memory_stats.limit.unwrap_or(0);
        let (memory_cache, memory_rss) = stats
            .memory_stats
            .stats
            .as_ref()
            .map(|memory_stats| match memory_stats {
                MemoryStatsStats::V1(_) => Err(HarnessError::CommandFailure(
                    "trusted Docker runs require cgroup v2 Docker stats".to_string(),
                )),
                MemoryStatsStats::V2(v2) => Ok((v2.file, v2.anon)),
            })
            .transpose()?
            .unwrap_or((0, memory_usage));

        let memory_percent = if memory_limit > 0 {
            (memory_usage as f64 / memory_limit as f64) * 100.0
        } else {
            0.0
        };

        Ok(Self {
            timestamp_ms: start_time.elapsed().as_millis() as u64,
            memory_usage_bytes: memory_usage,
            memory_limit_bytes: memory_limit,
            memory_percent,
            memory_cache_bytes: memory_cache,
            memory_rss_bytes: memory_rss,
            cpu_percent: calculate_cpu_percent(stats),
            minor_faults: None,
            major_faults: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerRunPlan {
    pub program: String,
    pub args: Vec<String>,
}

impl DockerRunPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn for_run(
        container_name: &str,
        image_ref: &str,
        fixture_path: &str,
        output_root: &str,
        input_root: &str,
        host_work_dir: Option<&str>,
        memory_limit: &str,
        cpuset: Option<&str>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        work_dir_mode: WorkDirMode,
        run_id: &str,
    ) -> Self {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container_name.to_string(),
        ];
        Self::push_common_container_args(
            &mut args, container_name, fixture_path, output_root, input_root, host_work_dir,
            memory_limit, cpuset, cpu_quota, cpu_period, work_dir_mode,
        );

        args.extend([
            image_ref.to_string(),
            "sol".to_string(),
            "run-once".to_string(),
            "--resolved-case".to_string(),
            "/bench/input/resolved_case.json".to_string(),
            "--run-dir".to_string(),
            format!("/bench/output/runs/{run_id}"),
            "--work-dir".to_string(),
            format!("/bench/work/{run_id}"),
            "--run-id".to_string(),
            run_id.to_string(),
        ]);

        Self {
            program: "docker".to_string(),
            args,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_profile(
        container_name: &str,
        image_ref: &str,
        fixture_path: &str,
        output_root: &str,
        input_root: &str,
        host_work_dir: Option<&str>,
        memory_limit: &str,
        cpuset: Option<&str>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        work_dir_mode: WorkDirMode,
        sample_rate_hz: u32,
        output_path: &str,
        profiled_run_dir: &str,
    ) -> Self {
        let mut args = vec![
            "run".to_string(),
            "--name".to_string(),
            container_name.to_string(),
            "--entrypoint".to_string(),
            "samply".to_string(),
            "--cap-add=PERFMON".to_string(),
        ];
        Self::push_common_container_args(
            &mut args, container_name, fixture_path, output_root, input_root, host_work_dir,
            memory_limit, cpuset, cpu_quota, cpu_period, work_dir_mode,
        );

        args.extend([
            image_ref.to_string(),
            "record".to_string(),
            "--save-only".to_string(),
            "--rate".to_string(),
            sample_rate_hz.to_string(),
            "--output".to_string(),
            output_path.to_string(),
            "--".to_string(),
            "nockchain-bench".to_string(),
            "sol".to_string(),
            "run-once".to_string(),
            "--resolved-case".to_string(),
            "/bench/input/resolved_case.json".to_string(),
            "--run-dir".to_string(),
            profiled_run_dir.to_string(),
            "--work-dir".to_string(),
            "/bench/work/profile".to_string(),
            "--run-id".to_string(),
            "profile".to_string(),
        ]);

        Self {
            program: "docker".to_string(),
            args,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_common_container_args(
        args: &mut Vec<String>,
        container_name: &str,
        _fixture_path: &str,
        output_root: &str,
        input_root: &str,
        host_work_dir: Option<&str>,
        memory_limit: &str,
        cpuset: Option<&str>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        work_dir_mode: WorkDirMode,
    ) {
        push_cgroup_capable_docker_args(args);
        push_container_resource_args(args, memory_limit, cpuset, cpu_quota, cpu_period);
        args.extend([
            "-v".to_string(),
            format!("{output_root}:/bench/output"),
            "-v".to_string(),
            format!("{input_root}:/bench/input:ro"),
        ]);
        push_work_dir_mount_args(args, container_name, host_work_dir, work_dir_mode);
    }
}

fn push_cgroup_capable_docker_args(args: &mut Vec<String>) {
    args.extend([
        "--privileged".to_string(),
        "--cgroupns=host".to_string(),
        "-e".to_string(),
        COLD_CGROUP_PARENT_ENV.to_string(),
    ]);
}

fn push_container_resource_args(
    args: &mut Vec<String>,
    memory_limit: &str,
    cpuset: Option<&str>,
    cpu_quota: Option<i64>,
    cpu_period: Option<i64>,
) {
    if !memory_limit.is_empty() {
        args.push(format!("--memory={memory_limit}"));
    }

    if let Some(cpuset) = cpuset {
        args.push(format!("--cpuset-cpus={cpuset}"));
    }
    if let Some(cpu_quota) = cpu_quota {
        args.push(format!("--cpu-quota={cpu_quota}"));
    }
    if let Some(cpu_period) = cpu_period {
        args.push(format!("--cpu-period={cpu_period}"));
    }
}

fn push_work_dir_mount_args(
    args: &mut Vec<String>,
    container_name: &str,
    host_work_dir: Option<&str>,
    work_dir_mode: WorkDirMode,
) {
    match work_dir_mode {
        WorkDirMode::HostBind => {
            if let Some(host_work_dir) = host_work_dir {
                args.push("-v".to_string());
                args.push(format!("{host_work_dir}:/bench/work"));
            }
        }
        WorkDirMode::DockerVolume => {
            args.push("--mount".to_string());
            args.push(format!(
                "type=volume,src={container_name}-work,dst=/bench/work"
            ));
        }
        WorkDirMode::DockerTmpfs => {
            args.push("--tmpfs".to_string());
            args.push("/bench/work".to_string());
        }
    }
}

#[derive(Debug, Clone)]
struct DockerExecutionConfig {
    image: DockerImageSource,
    image_variant: DockerImageVariant,
    memory_limit: String,
    cpuset: Option<String>,
    cpu_quota: Option<i64>,
    cpu_period: Option<i64>,
    work_dir_mode: WorkDirMode,
}

#[derive(Debug, Clone)]
struct DockerBackendState {
    container_name: String,
    container_id: String,
    image: ResolvedDockerImage,
    output_root: PathBuf,
    volume_name: Option<String>,
    host_binary: BinaryIdentity,
    validation_outcome: BackendValidationOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PendingDockerResources {
    container_name: Option<String>,
    volume_name: Option<String>,
}

#[derive(Debug, Clone)]
struct PreparedDockerInputs {
    output_root: PathBuf,
    input_root: PathBuf,
    resolved_image: ResolvedDockerImage,
    validation_key: ValidationCacheKey,
    requested_memory_limit_bytes: u64,
}

#[derive(Debug, Clone)]
struct DockerBackend {
    execution: DockerExecutionConfig,
    state: Option<DockerBackendState>,
    pending_resources: Option<PendingDockerResources>,
}

impl DockerBackend {
    fn from_requested(
        requested: &RequestedCase,
        image_variant: DockerImageVariant,
    ) -> Result<Self, HarnessError> {
        let ExecutionRequest::Docker {
            image,
            memory_limit,
            cpuset,
            cpu_quota,
            cpu_period,
            work_dir_mode,
            allow_version_skew: _,
        } = &requested.execution
        else {
            return Err(HarnessError::InvalidRequestedCase(
                "Docker backend requires ExecutionRequest::Docker".to_string(),
            ));
        };

        Ok(Self {
            execution: DockerExecutionConfig {
                image: image.clone(),
                image_variant,
                memory_limit: memory_limit.clone(),
                cpuset: cpuset.clone(),
                cpu_quota: *cpu_quota,
                cpu_period: *cpu_period,
                work_dir_mode: work_dir_mode.clone(),
            },
            state: None,
            pending_resources: None,
        })
    }

    fn prepare_inputs(
        &self,
        resolved: &mut ResolvedCase,
        output_root: &Path,
        docker_info: &Value,
    ) -> Result<PreparedDockerInputs, HarnessError> {
        let output_root = canonicalize_existing_dir(output_root)?;
        let input_root = output_root.join("input");
        std::fs::create_dir_all(&input_root)?;
        if let Some(source_plan_path) = resolved.orchestrate.source_plan_path.as_deref() {
            std::fs::copy(source_plan_path, input_root.join("source-plan.json"))?;
        }

        let unresolved_validation_key =
            build_validation_key(&self.execution, docker_info, UNRESOLVED_IMAGE_DIGEST);
        let resolved_image = with_persisted_validation_failure(
            &output_root,
            &unresolved_validation_key,
            false,
            resolve_docker_image(&self.execution.image, self.execution.image_variant),
        )?;
        if let Some(docker) = resolved.docker.as_mut() {
            docker.image = resolved_image.clone();
        }

        let mut trusted_plan = read_trusted_plan_for_container(&output_root)?;
        rewrite_trusted_inputs_for_container(&mut trusted_plan, &mut resolved.orchestrate.inputs);
        let input_files_root = input_root.join("files");
        std::fs::create_dir_all(&input_files_root)?;
        for input in &resolved.orchestrate.inputs {
            let Some(container_path) = &input.container_path else {
                continue;
            };
            let file_name = container_path.file_name().ok_or_else(|| {
                HarnessError::InvalidRequestedCase(format!(
                    "invalid trusted container input path {}",
                    container_path.display()
                ))
            })?;
            let mountpoint = input_files_root.join(file_name);
            if !mountpoint.exists() {
                std::fs::File::create(mountpoint)?;
            }
        }
        let mut container_trusted_plan = trusted_plan.clone();
        for input in &mut container_trusted_plan.inputs {
            if let Some(container_path) = input.container_path.clone() {
                input.absolute_path = container_path;
            }
        }
        std::fs::write(
            input_root.join("trusted_plan.json"),
            serde_json::to_vec_pretty(&container_trusted_plan)?,
        )?;

        let container_resolved = containerize_resolved_case(resolved);
        std::fs::write(
            input_root.join("resolved_case.json"),
            serde_json::to_vec_pretty(&container_resolved)?,
        )?;

        Ok(PreparedDockerInputs {
            output_root,
            input_root,
            validation_key: build_validation_key(
                &self.execution, docker_info, &resolved_image.immutable_identity,
            ),
            resolved_image,
            requested_memory_limit_bytes: parse_memory_limit(&self.execution.memory_limit)
                .try_into()
                .unwrap_or(0),
        })
    }

    fn create_host_work_dir(&self, output_root: &Path) -> Result<Option<PathBuf>, HarnessError> {
        match self.execution.work_dir_mode {
            WorkDirMode::HostBind => {
                let path = output_root.join("work");
                std::fs::create_dir_all(&path)?;
                Ok(Some(path))
            }
            _ => Ok(None),
        }
    }

    fn start_container(
        &mut self,
        resolved: &ResolvedCase,
        inputs: &PreparedDockerInputs,
    ) -> Result<(), HarnessError> {
        let container_name = format!(
            "nockchain-bench-{}-{}",
            std::process::id(),
            unix_timestamp_ms()
        );
        let volume_name = match self.execution.work_dir_mode {
            WorkDirMode::DockerVolume => Some(format!("{container_name}-work")),
            _ => None,
        };
        self.pending_resources = Some(PendingDockerResources {
            container_name: Some(container_name.clone()),
            volume_name: volume_name.clone(),
        });

        let host_work_dir = self.create_host_work_dir(&inputs.output_root)?;
        if let Some(volume_name) = &volume_name {
            with_persisted_validation_failure(
                &inputs.output_root,
                &inputs.validation_key,
                false,
                docker_stdout(["volume", "create", volume_name.as_str()]).map(|_| ()),
            )?;
        }

        let create_args = docker_create_args(
            &container_name,
            &self.execution,
            &inputs.resolved_image,
            &resolved.absolute_fixture_path,
            &resolved.orchestrate.inputs,
            resolved.orchestrate.source_plan_path.as_deref(),
            &inputs.output_root,
            &inputs.input_root,
            host_work_dir.as_deref(),
            volume_name.as_deref(),
        );
        let container_id = with_persisted_validation_failure(
            &inputs.output_root,
            &inputs.validation_key,
            false,
            docker_stdout_vec(create_args),
        )?
        .trim()
        .to_string();
        with_persisted_validation_failure(
            &inputs.output_root,
            &inputs.validation_key,
            false,
            docker_stdout(["start", container_name.as_str()]).map(|_| ()),
        )?;

        self.state = Some(DockerBackendState {
            container_name,
            container_id,
            image: inputs.resolved_image.clone(),
            output_root: inputs.output_root.clone(),
            volume_name,
            host_binary: resolved.binary.clone(),
            validation_outcome: BackendValidationOutcome::default(),
        });
        self.pending_resources = None;
        Ok(())
    }

    fn resolve_validation_outcome(
        &mut self,
        inputs: &PreparedDockerInputs,
        container_started: bool,
        requires_cgroup_v2: bool,
    ) -> Result<BackendValidationOutcome, HarnessError> {
        let validation = if inputs.validation_key.cgroup_version != "2" {
            if requires_cgroup_v2 {
                validate_cached_or_run(
                    &inputs.output_root,
                    inputs.validation_key.clone(),
                    false,
                    inputs.requested_memory_limit_bytes,
                    || {
                        Err(HarnessError::InvalidRequestedCase(format!(
                            "trusted Docker cold runs require cgroup v2; docker info reported CgroupVersion={}",
                            inputs.validation_key.cgroup_version
                        )))
                    },
                )?
            } else {
                return Ok(BackendValidationOutcome::default());
            }
        } else {
            let container_name = self
                .state
                .as_ref()
                .ok_or_else(|| {
                    HarnessError::InvalidRequestedCase("Docker backend not prepared".to_string())
                })?
                .container_name
                .clone();
            validate_cached_or_run(
                &inputs.output_root,
                inputs.validation_key.clone(),
                container_started,
                inputs.requested_memory_limit_bytes,
                || run_container_validation_probe(&container_name),
            )?
        };
        Ok(BackendValidationOutcome::from_validation_record(
            &validation,
        ))
    }
}

fn read_docker_info_cgroup_version(docker_info: &Value) -> String {
    docker_info
        .get("CgroupVersion")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

fn docker_image_variant(cpu_profiler: Option<&CpuProfilerConfig>) -> DockerImageVariant {
    match cpu_profiler {
        Some(CpuProfilerConfig {
            kind: CpuProfilerKind::Samply,
            ..
        }) => DockerImageVariant::Profiling,
        None => DockerImageVariant::Standard,
    }
}

pub async fn execute_docker_trusted_run(
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
    cpu_profiler: Option<CpuProfilerConfig>,
) -> Result<TrustedRunResult, HarnessError> {
    let backend =
        DockerBackend::from_requested(&requested, docker_image_variant(cpu_profiler.as_ref()))?;
    execute_docker_trusted_run_with_hooks(
        backend,
        requested,
        output_root,
        allow_debug_benchmark,
        cpu_profiler,
        |requested, config| preflight_docker_profiler(requested, config).boxed(),
        |resolved, output_root, config| profile_docker_case(resolved, output_root, config).boxed(),
    )
    .await
}

async fn execute_docker_trusted_run_with_hooks<B, PF, LF>(
    backend: B,
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
    cpu_profiler: Option<CpuProfilerConfig>,
    preflight_profiler: PF,
    profile_case: LF,
) -> Result<TrustedRunResult, HarnessError>
where
    B: TrustedBackend,
    PF: for<'a> Fn(
        &'a RequestedCase,
        CpuProfilerConfig,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>>,
    LF: for<'a> Fn(
        &'a ResolvedCase,
        &'a Path,
        CpuProfilerConfig,
    ) -> futures::future::BoxFuture<
        'a,
        Result<super::execute::CpuProfileArtifact, HarnessError>,
    >,
{
    if cpu_profiler.is_some() {
        prepare_output_root(output_root)?;
    }

    if let Some(config) = cpu_profiler.clone() {
        if let Err(error) = preflight_profiler(&requested, config).await {
            invalidate_verdict_for_cpu_profiling_failure(output_root, &error)?;
            return Err(error);
        }
    }

    let run = execute_trusted_run(backend, requested, output_root, allow_debug_benchmark).await?;
    if let Some(config) = cpu_profiler {
        let profiling_result = async {
            let artifact = profile_case(&run.resolved, output_root, config).await?;
            write_cpu_profile_artifact(output_root, &artifact)
        }
        .await;

        if let Err(error) = profiling_result {
            invalidate_verdict_for_cpu_profiling_failure(output_root, &error)?;
            return Err(error);
        }
    }
    Ok(run)
}

async fn preflight_docker_profiler(
    requested: &RequestedCase,
    config: CpuProfilerConfig,
) -> Result<(), HarnessError> {
    let execution =
        docker_execution_config_from_requested(requested, docker_image_variant(Some(&config)))?;
    let resolved_image = resolve_docker_image(&execution.image, execution.image_variant)?;
    docker_stdout_vec(docker_cpu_profiler_preflight_args(
        &execution, &resolved_image, config.sample_rate_hz,
    ))
    .map(|_| ())
    .map_err(|error| map_docker_profiling_error(error, &resolved_image.resolved_ref))
}

#[derive(Debug, Clone)]
struct DockerCpuProfilerLauncher {
    execution: DockerExecutionConfig,
    fixture_path: PathBuf,
    output_root: PathBuf,
    input_root: PathBuf,
    host_work_dir: Option<PathBuf>,
}

impl DockerCpuProfilerLauncher {
    fn new(resolved: &ResolvedCase, output_root: &Path) -> Result<Self, HarnessError> {
        let docker = resolved.docker.as_ref().ok_or_else(|| {
            HarnessError::InvalidRequestedCase(
                "Docker profiler launcher requires resolved Docker execution".to_string(),
            )
        })?;
        let ExecutionRequest::Docker {
            memory_limit,
            cpuset,
            cpu_quota,
            cpu_period,
            ..
        } = &resolved.requested.execution
        else {
            return Err(HarnessError::InvalidRequestedCase(
                "Docker profiler launcher requires Docker execution".to_string(),
            ));
        };

        let output_root = canonicalize_existing_dir(output_root)?;
        let host_work_dir = match docker.work_dir_mode {
            WorkDirMode::HostBind => {
                let path = output_root.join("work");
                std::fs::create_dir_all(&path)?;
                Some(path)
            }
            _ => None,
        };

        Ok(Self {
            execution: DockerExecutionConfig {
                image: docker.image.source.clone(),
                image_variant: docker.image.variant,
                memory_limit: memory_limit.clone(),
                cpuset: cpuset.clone(),
                cpu_quota: *cpu_quota,
                cpu_period: *cpu_period,
                work_dir_mode: docker.work_dir_mode.clone(),
            },
            fixture_path: resolved.absolute_fixture_path.clone(),
            input_root: output_root.join("input"),
            output_root,
            host_work_dir,
        })
    }

    fn ensure_samply_available(
        &self,
        resolved_image: &ResolvedDockerImage,
    ) -> Result<(), HarnessError> {
        docker_stdout([
            "run",
            "--rm",
            "--entrypoint",
            "samply",
            resolved_image.resolved_ref.as_str(),
            "--help",
        ])
        .map(|_| ())
        .map_err(|error| map_docker_profiling_error(error, &resolved_image.resolved_ref))
    }
}

impl CpuProfilerLauncher for DockerCpuProfilerLauncher {
    fn launch<'a>(
        &'a mut self,
        request: &'a CpuProfilerLaunchRequest,
    ) -> futures::future::BoxFuture<'a, Result<super::execute::CpuProfileArtifact, HarnessError>>
    {
        async move {
            let resolved_image =
                resolve_docker_image(&self.execution.image, self.execution.image_variant)?;
            self.ensure_samply_available(&resolved_image)?;

            let output_path = request.output_path();
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let container_name = format!(
                "nockchain-bench-profile-{}-{}",
                std::process::id(),
                unix_timestamp_ms()
            );
            let volume_name = matches!(self.execution.work_dir_mode, WorkDirMode::DockerVolume)
                .then(|| format!("{container_name}-work"));

            if let Some(volume_name) = &volume_name {
                let _ = docker_stdout(["volume", "create", volume_name.as_str()])?;
            }

            let host_work_dir = self
                .host_work_dir
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
            let plan = DockerRunPlan::for_profile(
                &container_name,
                &resolved_image.resolved_ref,
                &self.fixture_path.to_string_lossy(),
                &self.output_root.to_string_lossy(),
                &self.input_root.to_string_lossy(),
                host_work_dir.as_deref(),
                &self.execution.memory_limit,
                self.execution.cpuset.as_deref(),
                self.execution.cpu_quota,
                self.execution.cpu_period,
                self.execution.work_dir_mode.clone(),
                request.sample_rate_hz,
                "/bench/output/profiles/samply-profile.json.gz",
                "/bench/output/profile-run",
            );

            let profiling_result = (|| -> Result<(), HarnessError> {
                docker_stdout_vec(plan.args).map_err(|error| {
                    map_docker_profiling_error(error, &resolved_image.resolved_ref)
                })?;

                if !output_path.exists() {
                    return Err(HarnessError::CommandFailure(format!(
                        "profiler succeeded but output artifact is missing at {}",
                        output_path.display()
                    )));
                }
                validate_profiled_run(&request.profiled_run_dir)?;
                copy_profiled_symbol_binary(&container_name, &request.symbol_binary_path())?;
                Ok(())
            })();
            let cleanup_result = cleanup_docker_resources(
                None,
                Some(&PendingDockerResources {
                    container_name: Some(container_name),
                    volume_name,
                }),
                |args: &[String]| docker_stdout_vec(args.to_vec()).map(|_| ()),
            );

            finalize_profile_cleanup_results(profiling_result, cleanup_result)?;
            Ok(request.artifact())
        }
        .boxed()
    }
}

async fn profile_docker_case(
    resolved: &ResolvedCase,
    output_root: &Path,
    config: CpuProfilerConfig,
) -> Result<super::execute::CpuProfileArtifact, HarnessError> {
    let request = build_docker_profiler_request(output_root, config);
    let mut launcher = DockerCpuProfilerLauncher::new(resolved, output_root)?;
    launcher.launch(&request).await
}

fn docker_execution_config_from_requested(
    requested: &RequestedCase,
    image_variant: DockerImageVariant,
) -> Result<DockerExecutionConfig, HarnessError> {
    let ExecutionRequest::Docker {
        image,
        memory_limit,
        cpuset,
        cpu_quota,
        cpu_period,
        work_dir_mode,
        allow_version_skew: _,
    } = &requested.execution
    else {
        return Err(HarnessError::InvalidRequestedCase(
            "Docker CPU profiling requires Docker execution".to_string(),
        ));
    };

    Ok(DockerExecutionConfig {
        image: image.clone(),
        image_variant,
        memory_limit: memory_limit.clone(),
        cpuset: cpuset.clone(),
        cpu_quota: *cpu_quota,
        cpu_period: *cpu_period,
        work_dir_mode: work_dir_mode.clone(),
    })
}

fn docker_cpu_profiler_preflight_args(
    execution: &DockerExecutionConfig,
    resolved_image: &ResolvedDockerImage,
    sample_rate_hz: u32,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--cap-add=PERFMON".to_string(),
        "--entrypoint".to_string(),
        "samply".to_string(),
    ];
    push_cgroup_capable_docker_args(&mut args);
    push_container_resource_args(
        &mut args,
        &execution.memory_limit,
        execution.cpuset.as_deref(),
        execution.cpu_quota,
        execution.cpu_period,
    );

    args.extend([
        resolved_image.resolved_ref.clone(),
        "record".to_string(),
        "--save-only".to_string(),
        "--rate".to_string(),
        sample_rate_hz.to_string(),
        "--output".to_string(),
        "/tmp/samply-preflight.json".to_string(),
        "--".to_string(),
        "/bin/true".to_string(),
    ]);
    args
}

fn build_docker_profiler_request(
    output_root: &Path,
    config: CpuProfilerConfig,
) -> CpuProfilerLaunchRequest {
    CpuProfilerLaunchRequest {
        profiler_kind: config.kind,
        sample_rate_hz: config.sample_rate_hz,
        execution_kind: CpuProfileExecutionKind::DockerInContainer,
        case_root: output_root.to_path_buf(),
        output_relative_path: cpu_profile_output_relative_path(config.kind),
        symbol_dir_relative_path: cpu_profile_symbol_dir_relative_path(),
        symbol_binary_relative_path: cpu_profile_symbol_binary_relative_path(),
        profiled_run_dir: output_root.join("profile-run"),
        profiled_command: build_run_once_command(
            "nockchain-bench", "/bench/input/resolved_case.json", "/bench/output/profile-run",
            "profile",
        ),
    }
}

fn map_docker_profiling_error(error: HarnessError, image_ref: &str) -> HarnessError {
    match error {
        HarnessError::CommandFailure(message) => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("executable file not found")
                || (lower.contains("not found") && lower.contains("samply"))
            {
                return HarnessError::CommandFailure(format!(
                    "Docker CPU profiling requires `samply` on PATH inside image `{image_ref}`"
                ));
            }

            HarnessError::CommandFailure(augment_perf_permission_guidance(&message))
        }
        other => other,
    }
}

fn copy_profiled_symbol_binary(
    container_name: &str,
    destination: &Path,
) -> Result<(), HarnessError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    docker_stdout_vec(vec![
        "cp".to_string(),
        format!("{container_name}:/usr/local/bin/nockchain-bench"),
        destination.to_string_lossy().to_string(),
    ])
    .map(|_| ())
}

pub async fn execute_docker_validation(
    requested: RequestedCase,
    output_root: &Path,
) -> Result<ValidationRecord, HarnessError> {
    super::orchestrate::prepare_output_root(output_root)?;
    std::fs::create_dir_all(output_root)?;

    let mut resolved = resolve_requested_case(&requested)?;
    let raw_dir = output_root.join("raw");
    std::fs::create_dir_all(&raw_dir)?;

    let mut backend = DockerBackend::from_requested(&requested, DockerImageVariant::Standard)?;
    let prepare_result = backend.prepare(&mut resolved, output_root).await;
    write_schema_version(output_root)?;
    write_requested_case(output_root, &requested)?;
    write_resolved_case(output_root, &resolved)?;
    write_host_env(output_root, &capture_host_env())?;
    let raw_result = backend.capture_raw_evidence(&raw_dir).await;
    let cleanup_result = backend.cleanup().await;
    finalize_validation_results(prepare_result, raw_result, cleanup_result)?;

    read_validation_record(output_root)
}

impl TrustedBackend for DockerBackend {
    fn prepare<'a>(
        &'a mut self,
        resolved: &'a mut ResolvedCase,
        output_root: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async move {
            connect_docker().await?;
            let docker_info = docker_info_json()?;
            let inputs = self.prepare_inputs(resolved, output_root, &docker_info)?;
            if resolved.orchestrate.contains_cold_steps && inputs.validation_key.cgroup_version != "2" {
                return Err(HarnessError::CommandFailure(format!(
                    "trusted Docker cold runs require cgroup v2; docker info reported CgroupVersion={}",
                    inputs.validation_key.cgroup_version
                )));
            }

            self.start_container(resolved, &inputs)?;
            let validation_outcome =
                self.resolve_validation_outcome(&inputs, true, resolved.orchestrate.contains_cold_steps)?;
            if let Some(state) = self.state.as_mut() {
                state.validation_outcome = validation_outcome;
            }

            Ok(())
        }
        .boxed()
    }

    fn capture_runtime_facts(&self) -> Result<BackendRuntimeFacts, HarnessError> {
        let state = self.state.as_ref().ok_or_else(|| {
            HarnessError::InvalidRequestedCase("Docker backend not prepared".to_string())
        })?;
        let info = docker_info_json()?;
        let container_binary = inspect_container_binary(&state.container_name)?;
        let cgroup_version = read_docker_info_cgroup_version(&info);

        Ok(BackendRuntimeFacts::Docker {
            host_binary: state.host_binary.clone(),
            container_binary,
            image_source: state.image.source.clone(),
            requested_image_ref: state.image.requested_ref.clone(),
            resolved_image_ref: state.image.resolved_ref.clone(),
            image_digest: state.image.immutable_identity.clone(),
            container_id: state.container_id.clone(),
            docker_engine_version: docker_engine_version(&info),
            docker_context: docker_context()?,
            cgroup_version,
            storage_driver: info
                .get("Driver")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            realized_memory_max: read_realized_memory_max(&state.container_name)?,
            realized_memory_current: read_realized_memory_current(&state.container_name)?,
            realized_cpuset: read_realized_cpuset(&state.container_name)?,
            realized_cpu_max: read_realized_cpu_max(&state.container_name)?,
        })
    }

    fn validation_outcome(&self) -> BackendValidationOutcome {
        self.state
            .as_ref()
            .map(|state| state.validation_outcome)
            .unwrap_or_default()
    }

    fn execute_run<'a>(
        &'a mut self,
        resolved: &'a ResolvedCase,
        run_id: &'a str,
        run_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<super::execute::CompletedRun, HarnessError>> {
        async move {
            let state = self.state.as_ref().ok_or_else(|| {
                HarnessError::InvalidRequestedCase("Docker backend not prepared".to_string())
            })?;
            let run_dir = canonicalize_run_dir_parent(run_dir)?;
            let relative_run_dir = run_dir
                .strip_prefix(&state.output_root)
                .unwrap_or_else(|_| Path::new(""));
            let container_run_dir = Path::new("/bench/output").join(relative_run_dir);
            let args = docker_exec_run_once_args(&state.container_name, &container_run_dir, run_id);
            let should_capture_samples = run_id.starts_with("run-");

            if !should_capture_samples {
                let _ = docker_stdout_vec(args)?;
                return read_run_artifacts(&run_dir);
            }

            let sample_interval_ms = resolved.requested.profile_interval_ms.max(1);
            let (stop_tx, stop_rx) = watch::channel(false);
            let container_name = state.container_name.clone();
            let run_dir_for_sampler = run_dir.clone();
            let sampler = tokio::spawn(async move {
                collect_container_samples_until_stopped(
                    container_name,
                    run_dir_for_sampler,
                    Duration::from_millis(sample_interval_ms),
                    stop_rx,
                )
                .await
            });

            let command = tokio::task::spawn_blocking(move || docker_stdout_vec(args)).await;
            let _ = stop_tx.send(true);
            let samples = collect_sampler_output(sampler.await)?;
            let _ = std::fs::remove_file(run_dir.join(".benchmark.pid"));
            write_container_samples(&run_dir, &samples)?;

            match command {
                Ok(Ok(_)) => {
                    let mut completed = read_run_artifacts(&run_dir)?;
                    populate_run_resource_metrics_from_container_samples(&mut completed, &samples);
                    Ok(completed)
                }
                Ok(Err(error)) => Err(error),
                Err(error) => Err(HarnessError::CommandFailure(format!(
                    "docker run-once task join failed: {error}"
                ))),
            }
        }
        .boxed()
    }

    fn capture_raw_evidence<'a>(
        &'a self,
        raw_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async move {
            std::fs::create_dir_all(raw_dir)?;
            std::fs::write(raw_dir.join("docker_info.json"), docker_info_json_string()?)?;
            let Some(state) = self.state.as_ref() else {
                return Ok(());
            };
            std::fs::write(
                raw_dir.join("docker_inspect.json"),
                docker_stdout(["inspect", state.container_name.as_str()])?,
            )?;
            std::fs::write(
                raw_dir.join("container_env.json"),
                serde_json::to_vec_pretty(&read_container_env(&state.container_name)?)?,
            )?;
            Ok(())
        }
        .boxed()
    }

    fn cleanup<'a>(&'a mut self) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async move {
            let state = self.state.take();
            let pending = self.pending_resources.take();
            let _ =
                cleanup_docker_resources(state.as_ref(), pending.as_ref(), |args: &[String]| {
                    docker_stdout_vec(args.to_vec()).map(|_| ())
                });
            Ok(())
        }
        .boxed()
    }
}

fn docker_exec_run_once_args(
    container_name: &str,
    container_run_dir: &Path,
    run_id: &str,
) -> Vec<String> {
    vec![
        "exec".to_string(),
        container_name.to_string(),
        "nockchain-bench".to_string(),
        "sol".to_string(),
        "run-once".to_string(),
        "--resolved-case".to_string(),
        "/bench/input/resolved_case.json".to_string(),
        "--run-dir".to_string(),
        container_run_dir.to_string_lossy().to_string(),
        "--work-dir".to_string(),
        format!("/bench/work/{run_id}"),
        "--run-id".to_string(),
        run_id.to_string(),
    ]
}

fn containerize_resolved_case(resolved: &ResolvedCase) -> ResolvedCase {
    let mut container_resolved = resolved.clone();
    let source_fixture_container_path = PathBuf::from("/bench/input/source-fixture.soltest");
    let source_plan_container_path = PathBuf::from("/bench/input/source-plan.json");
    let container_path_for = |path: &Path| {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        let canonical = absolute.canonicalize().unwrap_or(absolute);
        resolved
            .orchestrate
            .inputs
            .iter()
            .find(|input| input.absolute_path == canonical)
            .and_then(|input| input.container_path.clone())
            .unwrap_or_else(|| path.to_path_buf())
    };
    container_resolved.absolute_fixture_path =
        container_path_for(&container_resolved.absolute_fixture_path);
    container_resolved.requested.fixture_path =
        container_path_for(&container_resolved.requested.fixture_path);
    match &mut container_resolved.requested.orchestrate {
        super::case::RequestedOrchestrate::PlanFile { plan_path } => {
            *plan_path = source_plan_container_path.clone();
        }
        super::case::RequestedOrchestrate::GeneratedReplay { fixture_path, .. } => {
            let original_fixture_path = fixture_path.clone();
            *fixture_path = container_path_for(fixture_path);
            if original_fixture_path == resolved.requested.fixture_path {
                *fixture_path = source_fixture_container_path.clone();
            }
        }
        super::case::RequestedOrchestrate::GeneratedRead {
            boot, kernel_path, ..
        } => {
            match boot {
                crate::speed_of_light::BootSourceInput::Checkpoint { checkpoint } => {
                    *checkpoint = container_path_for(checkpoint);
                }
                crate::speed_of_light::BootSourceInput::Snapshot { pma, manifest } => {
                    *pma = container_path_for(pma);
                    *manifest = container_path_for(manifest);
                }
            }
            *kernel_path = container_path_for(kernel_path);
        }
    }
    if let Some(source_plan_path) = &mut container_resolved.orchestrate.source_plan_path {
        *source_plan_path = source_plan_container_path;
    }
    for input in &mut container_resolved.orchestrate.inputs {
        if let Some(container_path) = input.container_path.clone() {
            input.absolute_path = container_path;
        }
    }
    container_resolved.orchestrate.trusted_plan_relative_path =
        PathBuf::from("/bench/input/trusted_plan.json");
    container_resolved
}

fn read_trusted_plan_for_container(output_root: &Path) -> Result<TrustedPlan, HarnessError> {
    Ok(serde_json::from_slice(&std::fs::read(
        output_root.join("trusted_plan.json"),
    )?)?)
}

fn trusted_container_input_path(input: &ResolvedInput) -> PathBuf {
    let extension = input
        .absolute_path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    PathBuf::from("/bench/input/files").join(format!("{}{extension}", input.input_id))
}

fn rewrite_trusted_inputs_for_container(
    trusted_plan: &mut TrustedPlan,
    resolved_inputs: &mut [ResolvedInput],
) {
    for input in &mut trusted_plan.inputs {
        input.container_path = Some(trusted_container_input_path(input));
    }
    for input in resolved_inputs {
        input.container_path = Some(trusted_container_input_path(input));
    }
}

fn docker_create_args(
    container_name: &str,
    execution: &DockerExecutionConfig,
    resolved_image: &ResolvedDockerImage,
    _fixture_path: &Path,
    referenced_inputs: &[ResolvedInput],
    _source_plan_path: Option<&Path>,
    output_root: &Path,
    input_root: &Path,
    host_work_dir: Option<&Path>,
    volume_name: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        "--name".to_string(),
        container_name.to_string(),
        "--entrypoint".to_string(),
        "sleep".to_string(),
    ];
    push_cgroup_capable_docker_args(&mut args);
    args.extend([
        format!("--memory={}", execution.memory_limit),
        "-v".to_string(),
        format!("{}:/bench/output", output_root.display()),
        "-v".to_string(),
        format!("{}:/bench/input:ro", input_root.display()),
    ]);
    for input in referenced_inputs {
        let container_path = input
            .container_path
            .clone()
            .unwrap_or_else(|| trusted_container_input_path(input));
        args.push("-v".to_string());
        args.push(format!(
            "{}:{}:ro",
            input.absolute_path.display(),
            container_path.display()
        ));
    }
    if let Some(cpuset) = &execution.cpuset {
        args.push(format!("--cpuset-cpus={cpuset}"));
    }
    if let Some(cpu_quota) = execution.cpu_quota {
        args.push(format!("--cpu-quota={cpu_quota}"));
    }
    if let Some(cpu_period) = execution.cpu_period {
        args.push(format!("--cpu-period={cpu_period}"));
    }

    match execution.work_dir_mode {
        WorkDirMode::HostBind => {
            if let Some(host_work_dir) = host_work_dir {
                args.push("-v".to_string());
                args.push(format!("{}:/bench/work", host_work_dir.display()));
            }
        }
        WorkDirMode::DockerVolume => {
            if let Some(volume_name) = volume_name {
                args.push("--mount".to_string());
                args.push(format!("type=volume,src={volume_name},dst=/bench/work"));
            }
        }
        WorkDirMode::DockerTmpfs => {
            args.push("--tmpfs".to_string());
            args.push("/bench/work".to_string());
        }
    }

    args.extend([resolved_image.resolved_ref.clone(), "infinity".to_string()]);
    args
}

fn docker_stdout<const N: usize>(args: [&str; N]) -> Result<String, HarnessError> {
    docker_stdout_vec(args.into_iter().map(str::to_string).collect())
}

fn docker_stdout_vec(args: Vec<String>) -> Result<String, HarnessError> {
    let output = Command::new("docker").args(&args).output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailure(format!(
            "docker {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn docker_info_json() -> Result<Value, HarnessError> {
    serde_json::from_str(&docker_info_json_string()?).map_err(HarnessError::from)
}

fn docker_info_json_string() -> Result<String, HarnessError> {
    docker_stdout(["info", "--format", "{{json .}}"])
}

fn docker_engine_version(info: &Value) -> String {
    info.get("ServerVersion")
        .or_else(|| info.get("Version"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
fn require_cgroup_v2(info: &Value) -> Result<String, HarnessError> {
    let cgroup_version = read_docker_info_cgroup_version(info);
    if cgroup_version == "2" {
        Ok(cgroup_version)
    } else {
        Err(HarnessError::CommandFailure(format!(
            "trusted Docker runs require cgroup v2; docker info reported CgroupVersion={cgroup_version}"
        )))
    }
}

fn docker_context() -> Result<String, HarnessError> {
    docker_stdout(["context", "show"])
}

fn inspect_container_binary(container_name: &str) -> Result<BinaryIdentity, HarnessError> {
    let payload =
        docker_stdout(["exec", container_name, "nockchain-bench", "sol", "binary-identity"])?;
    parse_binary_identity_json(&payload)
}

fn parse_binary_identity_json(payload: &str) -> Result<BinaryIdentity, HarnessError> {
    serde_json::from_str(payload).map_err(HarnessError::from)
}

fn run_container_validation_probe(
    container_name: &str,
) -> Result<ValidationProbeResult, HarnessError> {
    let payload =
        docker_stdout(["exec", container_name, "nockchain-bench", "sol", "validate-probe"])?;
    serde_json::from_str(&payload).map_err(HarnessError::from)
}

fn build_validation_key(
    execution: &DockerExecutionConfig,
    docker_info: &Value,
    image_digest: &str,
) -> ValidationCacheKey {
    ValidationCacheKey {
        docker_engine_version: docker_engine_version(docker_info),
        cgroup_version: read_docker_info_cgroup_version(docker_info),
        image_digest: image_digest.to_string(),
        memory_limit: execution.memory_limit.clone(),
        cpuset: execution.cpuset.clone(),
        cpu_quota: execution.cpu_quota,
        cpu_period: execution.cpu_period,
        work_dir_mode: execution.work_dir_mode.clone(),
        probe_version: VALIDATION_PROBE_VERSION.to_string(),
    }
}

fn cleanup_docker_resources<F>(
    state: Option<&DockerBackendState>,
    pending: Option<&PendingDockerResources>,
    mut remove: F,
) -> Result<(), HarnessError>
where
    F: FnMut(&[String]) -> Result<(), HarnessError>,
{
    let container_name = state
        .map(|state| state.container_name.as_str())
        .or_else(|| pending.and_then(|pending| pending.container_name.as_deref()));
    let volume_name = state
        .and_then(|state| state.volume_name.as_deref())
        .or_else(|| pending.and_then(|pending| pending.volume_name.as_deref()));

    if let Some(container_name) = container_name {
        remove(&["rm".to_string(), "-f".to_string(), container_name.to_string()])?;
    }
    if let Some(volume_name) = volume_name {
        remove(&[
            "volume".to_string(),
            "rm".to_string(),
            "-f".to_string(),
            volume_name.to_string(),
        ])?;
    }

    Ok(())
}

fn with_persisted_validation_failure<T>(
    output_root: &Path,
    key: &ValidationCacheKey,
    container_started: bool,
    result: Result<T, HarnessError>,
) -> Result<T, HarnessError> {
    result.map_err(|error| {
        let _ = persist_preflight_validation_failure(
            output_root,
            key.clone(),
            container_started,
            error.to_string(),
        );
        error
    })
}

fn persist_preflight_validation_failure(
    output_root: &Path,
    key: ValidationCacheKey,
    container_started: bool,
    failure_reason: String,
) -> Result<(), HarnessError> {
    persist_validation_record(
        output_root,
        &ValidationRecord {
            key: key.clone(),
            status: ValidationStatus::Invalid,
            from_cache: false,
            observed_probe_version: None,
            probe_version_matches: None,
            container_started,
            docker_reports_cgroup_v2: key.cgroup_version == "2",
            memory_max_readable: false,
            memory_current_readable: false,
            memory_limit_matches: false,
            allocation_sanity: false,
            realized_memory_max_bytes: None,
            allocation_request_bytes: None,
            memory_current_before_bytes: None,
            memory_current_peak_bytes: None,
            memory_current_after_bytes: None,
            recorded_cpu_max: None,
            recorded_cpuset: None,
            failure_reason: Some(failure_reason),
        },
    )
}

fn finalize_validation_results(
    prepare_result: Result<(), HarnessError>,
    raw_result: Result<(), HarnessError>,
    cleanup_result: Result<(), HarnessError>,
) -> Result<(), HarnessError> {
    if let Err(error) = prepare_result {
        return Err(error);
    }
    if let Err(error) = raw_result {
        return Err(error);
    }
    if let Err(error) = cleanup_result {
        return Err(error);
    }
    Ok(())
}

fn finalize_profile_cleanup_results(
    profiling_result: Result<(), HarnessError>,
    cleanup_result: Result<(), HarnessError>,
) -> Result<(), HarnessError> {
    match (profiling_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(profiling_error), Ok(())) => Err(profiling_error),
        (Err(profiling_error), Err(cleanup_error)) => Err(HarnessError::CommandFailure(format!(
            "{profiling_error}; cleanup after profiling failure also failed: {cleanup_error}"
        ))),
    }
}

fn collect_sampler_output(
    result: Result<Result<Vec<ContainerStats>, HarnessError>, JoinError>,
) -> Result<Vec<ContainerStats>, HarnessError> {
    match result {
        Ok(Ok(samples)) => Ok(samples),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(HarnessError::CommandFailure(format!(
            "docker stats sampler join failed: {error}"
        ))),
    }
}

fn populate_run_resource_metrics_from_container_samples(
    completed: &mut CompletedRun,
    samples: &[ContainerStats],
) {
    completed.record.peak_process_rss_bytes = completed
        .record
        .peak_process_rss_bytes
        .or_else(|| peak_container_rss_bytes(samples));
    completed.record.minor_faults_total = completed
        .record
        .minor_faults_total
        .or_else(|| container_fault_delta(samples, |sample| sample.minor_faults));
    completed.record.major_faults_total = completed
        .record
        .major_faults_total
        .or_else(|| container_fault_delta(samples, |sample| sample.major_faults));
}

fn peak_container_rss_bytes(samples: &[ContainerStats]) -> Option<f64> {
    samples
        .iter()
        .map(|sample| sample.memory_rss_bytes)
        .max()
        .map(|value| value as f64)
}

fn container_fault_delta(
    samples: &[ContainerStats],
    fault_count: impl Fn(&ContainerStats) -> Option<u64>,
) -> Option<f64> {
    let first = samples.iter().find_map(&fault_count)?;
    let last = samples.iter().rev().find_map(fault_count)?;
    Some(last.saturating_sub(first) as f64)
}

async fn collect_container_samples_until_stopped(
    container_name: String,
    run_dir: PathBuf,
    interval: Duration,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<Vec<ContainerStats>, HarnessError> {
    let docker = connect_docker().await?;
    let start_time = Instant::now();
    let mut samples = Vec::new();
    let mut benchmark_pid =
        wait_for_benchmark_pid(&run_dir, Duration::from_millis(250), &mut stop_rx).await;

    loop {
        if *stop_rx.borrow() {
            break;
        }

        if benchmark_pid.is_none() {
            benchmark_pid = read_benchmark_pid(&run_dir);
        }
        samples.push(
            read_container_sample(&docker, &container_name, benchmark_pid, start_time).await?,
        );

        tokio::select! {
            changed = stop_rx.changed() => {
                match changed {
                    Ok(_) if *stop_rx.borrow() => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            _ = sleep(interval) => {}
        }
    }

    Ok(samples)
}

async fn read_container_sample(
    docker: &Docker,
    container_name: &str,
    benchmark_pid: Option<u32>,
    start_time: Instant,
) -> Result<ContainerStats, HarnessError> {
    let mut stats_stream = docker.stats(
        container_name,
        Some(bollard::container::StatsOptions {
            stream: false,
            one_shot: true,
        }),
    );
    let stats = stats_stream
        .next()
        .await
        .ok_or_else(|| {
            HarnessError::CommandFailure(format!(
                "docker stats returned no sample for container {container_name}"
            ))
        })?
        .map_err(HarnessDockerError::from)
        .map_err(HarnessError::from)?;
    let mut sample = ContainerStats::from_docker_stats(&stats, start_time)?;
    sample.memory_limit_bytes = resolve_sample_memory_limit_bytes(
        sample.memory_limit_bytes,
        read_realized_memory_max(container_name).ok(),
    );
    sample.memory_percent = if sample.memory_limit_bytes > 0 {
        (sample.memory_usage_bytes as f64 / sample.memory_limit_bytes as f64) * 100.0
    } else {
        0.0
    };
    if let Some(proc_stat_path) = benchmark_proc_stat_path(benchmark_pid) {
        if let Ok(proc_stat) =
            docker_stdout(["exec", container_name, "cat", proc_stat_path.as_str()])
        {
            if let Some((minor_faults, major_faults)) = parse_proc_stat_faults(&proc_stat) {
                sample.minor_faults = Some(minor_faults);
                sample.major_faults = Some(major_faults);
            }
        }
    }
    Ok(sample)
}

fn resolve_sample_memory_limit_bytes(stats_limit: u64, realized_limit: Option<u64>) -> u64 {
    realized_limit
        .filter(|limit| *limit > 0)
        .unwrap_or(stats_limit)
}

fn read_benchmark_pid(run_dir: &Path) -> Option<u32> {
    let pid = std::fs::read_to_string(run_dir.join(".benchmark.pid")).ok()?;
    pid.trim().parse::<u32>().ok()
}

fn benchmark_proc_stat_path(pid: Option<u32>) -> Option<String> {
    pid.map(|pid| format!("/proc/{pid}/stat"))
}

async fn wait_for_benchmark_pid(
    run_dir: &Path,
    max_wait: Duration,
    stop_rx: &mut watch::Receiver<bool>,
) -> Option<u32> {
    let start = Instant::now();
    loop {
        if let Some(pid) = read_benchmark_pid(run_dir) {
            return Some(pid);
        }
        if *stop_rx.borrow() || start.elapsed() >= max_wait {
            return None;
        }
        tokio::select! {
            changed = stop_rx.changed() => {
                match changed {
                    Ok(_) if *stop_rx.borrow() => return None,
                    Ok(_) => {}
                    Err(_) => return None,
                }
            }
            _ = sleep(Duration::from_millis(10)) => {}
        }
    }
}

#[cfg(test)]
fn verify_version_skew(
    host_binary: &BinaryIdentity,
    container_binary: &BinaryIdentity,
) -> Result<(), HarnessError> {
    if host_binary.version != container_binary.version {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "host/container version skew detected: host={} container={}",
            host_binary.version, container_binary.version
        )));
    }

    if host_binary.git_commit != container_binary.git_commit {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "host/container git commit skew detected: host={:?} container={:?}",
            host_binary.git_commit, container_binary.git_commit
        )));
    }

    Ok(())
}

fn read_realized_memory_max(container_name: &str) -> Result<u64, HarnessError> {
    read_cgroup_u64(container_name, CGROUP_V2_MEMORY_MAX_PATH)
}

fn read_realized_memory_current(container_name: &str) -> Result<u64, HarnessError> {
    read_cgroup_u64(container_name, CGROUP_V2_MEMORY_CURRENT_PATH)
}

fn read_realized_cpu_max(container_name: &str) -> Result<Option<String>, HarnessError> {
    read_optional_runtime_file(container_name, CGROUP_V2_CPU_MAX_PATH)
}

fn read_realized_cpuset(container_name: &str) -> Result<Option<String>, HarnessError> {
    match read_optional_runtime_file(container_name, CGROUP_V2_CPUSET_EFFECTIVE_PATH)? {
        Some(cpuset) => Ok(Some(cpuset)),
        None => read_optional_runtime_file(container_name, CGROUP_V2_CPUSET_PATH),
    }
}

fn read_optional_runtime_file(
    container_name: &str,
    path: &str,
) -> Result<Option<String>, HarnessError> {
    optional_runtime_fact(read_optional_container_file(container_name, path))
}

fn optional_runtime_fact(
    result: Result<Option<String>, HarnessError>,
) -> Result<Option<String>, HarnessError> {
    match result {
        Ok(value) => Ok(value),
        Err(HarnessError::CommandFailure(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_cgroup_u64(container_name: &str, path: &str) -> Result<u64, HarnessError> {
    let path = resolve_container_cgroup_path(container_name, path)?;
    let value = docker_stdout(["exec", container_name, "cat", path.as_str()])?;
    parse_cgroup_numeric(&value).ok_or_else(|| {
        HarnessError::CommandFailure(format!(
            "failed to parse cgroup value `{value}` from {path}"
        ))
    })
}

fn read_optional_container_file(
    container_name: &str,
    path: &str,
) -> Result<Option<String>, HarnessError> {
    let path = resolve_container_cgroup_path(container_name, path)?;
    let output = Command::new("docker")
        .args(["exec", container_name, "cat", path.as_str()])
        .output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailure(format!(
            "docker exec {container_name} cat {path} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

fn resolve_container_cgroup_path(container_name: &str, path: &str) -> Result<String, HarnessError> {
    if !path.starts_with("/sys/fs/cgroup/") {
        return Ok(path.to_string());
    }
    let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return Ok(path.to_string());
    };
    let proc_cgroup = docker_stdout(["exec", container_name, "cat", "/proc/self/cgroup"])?;
    let Some(cgroup_path) = cgroup_v2_path_from_proc_cgroup(&proc_cgroup) else {
        return Ok(path.to_string());
    };
    Ok(cgroup_path.join(file_name).to_string_lossy().to_string())
}

fn read_container_env(container_name: &str) -> Result<BTreeMap<String, String>, HarnessError> {
    let output = docker_stdout(["exec", container_name, "env"])?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect())
}

fn parse_cgroup_numeric(value: &str) -> Option<u64> {
    super::parse_cgroup_numeric(value)
}

fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, HarnessError> {
    std::fs::canonicalize(path).map_err(HarnessError::from)
}

fn canonicalize_run_dir_parent(run_dir: &Path) -> Result<PathBuf, HarnessError> {
    std::fs::create_dir_all(run_dir)?;
    std::fs::canonicalize(run_dir).map_err(HarnessError::from)
}

pub async fn connect_docker() -> Result<Docker, HarnessDockerError> {
    let home = std::env::var("HOME").unwrap_or_default();
    let socket_paths = [
        "/var/run/docker.sock".to_string(),
        format!("{home}/.docker/desktop/docker.sock"),
        format!("{home}/.docker/run/docker.sock"),
    ];

    if let Ok(docker) = Docker::connect_with_local_defaults() {
        if docker.ping().await.is_ok() {
            return Ok(docker);
        }
    }

    for socket_path in socket_paths {
        if !Path::new(&socket_path).exists() {
            continue;
        }
        if let Ok(docker) =
            Docker::connect_with_unix(&socket_path, 120, bollard::API_DEFAULT_VERSION)
        {
            if docker.ping().await.is_ok() {
                return Ok(docker);
            }
        }
    }

    Err(HarnessDockerError::NotAvailable(
        "Cannot connect to Docker. Tried: default, /var/run/docker.sock, ~/.docker/desktop/docker.sock, ~/.docker/run/docker.sock"
            .to_string(),
    ))
}

pub fn parse_proc_stat_faults(stat: &str) -> Option<(u64, u64)> {
    let stat = stat.trim();
    if stat.is_empty() {
        return None;
    }

    let stat_after_comm = stat
        .rfind(')')
        .map(|index| &stat[index + 1..])
        .unwrap_or(stat);
    let fields: Vec<&str> = stat_after_comm.split_whitespace().collect();
    let minflt = fields.get(7).and_then(|value| value.parse::<u64>().ok())?;
    let majflt = fields.get(9).and_then(|value| value.parse::<u64>().ok())?;
    Some((minflt, majflt))
}

pub fn parse_memory_limit(value: &str) -> i64 {
    let value = value.trim().to_lowercase();

    if let Some(num) = value.strip_suffix('g') {
        num.parse::<i64>().unwrap_or(0) * 1024 * 1024 * 1024
    } else if let Some(num) = value.strip_suffix('m') {
        num.parse::<i64>().unwrap_or(0) * 1024 * 1024
    } else if let Some(num) = value.strip_suffix('k') {
        num.parse::<i64>().unwrap_or(0) * 1024
    } else {
        value.parse::<i64>().unwrap_or(0)
    }
}

pub fn calculate_cpu_percent(stats: &Stats) -> f64 {
    let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as i64
        - stats.precpu_stats.cpu_usage.total_usage as i64;
    let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as i64
        - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as i64;
    let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

    if system_delta > 0 && cpu_delta > 0 {
        (cpu_delta as f64 / system_delta as f64) * num_cpus * 100.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use futures::FutureExt;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::sync::watch;
    use tokio::time::Duration;

    use super::*;
    use crate::speed_of_light::fixture::{
        write_fixture_file, SolFixtureCheckpointKind, SolFixtureFile, SolFixtureManifest,
    };
    use crate::speed_of_light::harness::artifacts::write_run_artifacts;
    use crate::speed_of_light::harness::case::{
        BinaryIdentity, ExecutionConfig, RequestedCase, RequestedOrchestrate, ResolvedCase,
        ResolvedOrchestrate,
    };
    use crate::speed_of_light::harness::docker_image::{
        DockerImageSource, DockerImageVariant, ResolvedDockerImage,
    };
    use crate::speed_of_light::harness::execute::{BlockTimingRecord, CompletedRun, RunRecord};
    use crate::speed_of_light::harness::orchestrate::TrustedBackend;
    use crate::speed_of_light::harness::provenance::BackendRuntimeFacts;
    use crate::speed_of_light::types::SolHeight;
    use crate::speed_of_light::{BootSourceInput, CpuProfilerKind, InputRole, ResolvedInput};

    fn auto_build_image(tag: &str) -> DockerImageSource {
        DockerImageSource::AutoBuild {
            tag: tag.to_string(),
        }
    }

    fn resolved_test_image(requested_ref: &str) -> ResolvedDockerImage {
        ResolvedDockerImage {
            source: auto_build_image("nockchain-bench:test"),
            variant: DockerImageVariant::Standard,
            requested_ref: requested_ref.to_string(),
            resolved_ref: requested_ref.to_string(),
            immutable_identity: requested_ref.to_string(),
            image_id: requested_ref.to_string(),
        }
    }

    fn fixture_manifest() -> SolFixtureManifest {
        SolFixtureManifest {
            source_archive_path: "archive.solarch".to_string(),
            source_archive_event_num: Some(1),
            checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
            checkpoint_height: SolHeight(1),
            checkpoint_event_num: 1,
            archive_start_height: SolHeight(2),
            archive_end_height: SolHeight(3),
            include_mempool: false,
            chunk_size: 8,
            kernel_hash_hex: "kernel".to_string(),
            checkpoint_hash_hex: "checkpoint".to_string(),
            archive_hash_hex: "archive".to_string(),
        }
    }

    #[test]
    fn test_parse_memory_limit() {
        assert_eq!(parse_memory_limit("16g"), 16 * 1024 * 1024 * 1024);
        assert_eq!(parse_memory_limit("512m"), 512 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1024k"), 1024 * 1024);
        assert_eq!(parse_memory_limit("1073741824"), 1073741824);
        assert_eq!(parse_memory_limit("16G"), 16 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_proc_stat_faults() {
        let stat = "1 (nockchain) S 0 0 0 0 0 0 123 0 4 0 0 0 0 0 0 0 0 0 0 0 0";
        let parsed = parse_proc_stat_faults(stat).expect("expected parse");
        assert_eq!(parsed.0, 123);
        assert_eq!(parsed.1, 4);
        assert!(parse_proc_stat_faults("").is_none());
    }

    #[test]
    fn docker_run_once_command_mounts_fixture_output_and_limits() {
        let plan = DockerRunPlan::for_run(
            "bench-harness-test",
            "nockchain-bench:test",
            "/host/fixture.soltest",
            "/host/output",
            "/host/input",
            Some("/host/work"),
            "2g",
            Some("0-3"),
            Some(200_000),
            Some(100_000),
            WorkDirMode::HostBind,
            "run-0",
        );

        assert_eq!(plan.program, "docker");
        assert!(plan.args.iter().any(|arg| arg == "--privileged"));
        assert!(plan.args.iter().any(|arg| arg == "--cgroupns=host"));
        assert!(plan.args.windows(2).any(|window| {
            window
                == [
                    "-e".to_string(),
                    "NOCKCHAIN_BENCH_COLD_CGROUP_PARENT=/sys/fs/cgroup".to_string(),
                ]
        }));
        assert!(plan.args.iter().any(|arg| arg == "--memory=2g"));
        assert!(plan.args.iter().any(|arg| arg == "--cpuset-cpus=0-3"));
        assert!(plan.args.iter().any(|arg| arg == "--cpu-quota=200000"));
        assert!(plan.args.iter().any(|arg| arg == "--cpu-period=100000"));
        assert!(!plan
            .args
            .iter()
            .any(|arg| arg.contains("/bench/fixture.soltest")));
        assert!(plan
            .args
            .iter()
            .any(|arg| arg == "/host/output:/bench/output"));
        assert!(plan
            .args
            .iter()
            .any(|arg| arg == "/host/input:/bench/input:ro"));
        assert!(plan.args.iter().any(|arg| arg == "/host/work:/bench/work"));
        assert!(plan.args.ends_with(&[
            "nockchain-bench:test".to_string(),
            "sol".to_string(),
            "run-once".to_string(),
            "--resolved-case".to_string(),
            "/bench/input/resolved_case.json".to_string(),
            "--run-dir".to_string(),
            "/bench/output/runs/run-0".to_string(),
            "--work-dir".to_string(),
            "/bench/work/run-0".to_string(),
            "--run-id".to_string(),
            "run-0".to_string(),
        ]));
    }

    #[test]
    fn docker_run_once_command_uses_named_volume_for_docker_volume() {
        let plan = DockerRunPlan::for_run(
            "bench-harness-test",
            "nockchain-bench:test",
            "/host/fixture.soltest",
            "/host/output",
            "/host/input",
            None,
            "2g",
            None,
            None,
            None,
            WorkDirMode::DockerVolume,
            "run-0",
        );

        assert!(plan
            .args
            .iter()
            .any(|arg| arg == "type=volume,src=bench-harness-test-work,dst=/bench/work"));
        assert!(!plan.args.iter().any(|arg| arg == "--tmpfs"));
    }

    #[test]
    fn docker_create_args_mounts_trusted_inputs_read_only_at_stable_paths() {
        let inputs = vec![
            ResolvedInput {
                input_id: "checkpoint-0".to_string(),
                role: crate::speed_of_light::InputRole::Checkpoint,
                absolute_path: PathBuf::from("/host/checkpoint.chkjam"),
                sha256_hex: "abc".to_string(),
                size_bytes: 3,
                container_path: Some(PathBuf::from("/bench/input/files/checkpoint-0.chkjam")),
            },
            ResolvedInput {
                input_id: "kernel-0".to_string(),
                role: crate::speed_of_light::InputRole::Kernel,
                absolute_path: PathBuf::from("/host/kernel.jam"),
                sha256_hex: "def".to_string(),
                size_bytes: 3,
                container_path: Some(PathBuf::from("/bench/input/files/kernel-0.jam")),
            },
        ];
        let args = docker_create_args(
            "bench-harness-test",
            &DockerExecutionConfig {
                image: auto_build_image("nockchain-bench:test"),
                image_variant: DockerImageVariant::Standard,
                memory_limit: "2g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: WorkDirMode::DockerTmpfs,
            },
            &resolved_test_image("nockchain-bench:test"),
            Path::new("/host/fixture.soltest"),
            &inputs,
            Some(Path::new("/host/source-plan.json")),
            Path::new("/host/output"),
            Path::new("/host/output/input"),
            None,
            None,
        );

        assert!(args.iter().any(|arg| arg == "--privileged"));
        assert!(args.iter().any(|arg| arg == "--cgroupns=host"));
        assert!(args.windows(2).any(|window| {
            window
                == [
                    "-e".to_string(),
                    "NOCKCHAIN_BENCH_COLD_CGROUP_PARENT=/sys/fs/cgroup".to_string(),
                ]
        }));
        assert!(args
            .iter()
            .any(|arg| arg == "/host/checkpoint.chkjam:/bench/input/files/checkpoint-0.chkjam:ro"));
        assert!(args
            .iter()
            .any(|arg| arg == "/host/kernel.jam:/bench/input/files/kernel-0.jam:ro"));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("/bench/input/source-plan.json")));
    }

    #[test]
    fn docker_create_args_mounts_snapshot_inputs_read_only_at_stable_paths() {
        let inputs = vec![
            ResolvedInput {
                input_id: "snapshot-pma-0".to_string(),
                role: InputRole::SnapshotPma,
                absolute_path: PathBuf::from("/host/snapshot.pma"),
                sha256_hex: "pma".to_string(),
                size_bytes: 3,
                container_path: Some(PathBuf::from("/bench/input/files/snapshot-pma-0.pma")),
            },
            ResolvedInput {
                input_id: "snapshot-manifest-0".to_string(),
                role: InputRole::SnapshotManifest,
                absolute_path: PathBuf::from("/host/snapshot.manifest"),
                sha256_hex: "manifest".to_string(),
                size_bytes: 3,
                container_path: Some(PathBuf::from(
                    "/bench/input/files/snapshot-manifest-0.manifest",
                )),
            },
            ResolvedInput {
                input_id: "kernel-0".to_string(),
                role: InputRole::Kernel,
                absolute_path: PathBuf::from("/host/kernel.jam"),
                sha256_hex: "kernel".to_string(),
                size_bytes: 3,
                container_path: Some(PathBuf::from("/bench/input/files/kernel-0.jam")),
            },
        ];
        let args = docker_create_args(
            "bench-harness-test",
            &DockerExecutionConfig {
                image: auto_build_image("nockchain-bench:test"),
                image_variant: DockerImageVariant::Standard,
                memory_limit: "2g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: WorkDirMode::DockerTmpfs,
            },
            &resolved_test_image("nockchain-bench:test"),
            Path::new("/host/fixture.soltest"),
            &inputs,
            None,
            Path::new("/host/output"),
            Path::new("/host/output/input"),
            None,
            None,
        );

        assert!(args
            .iter()
            .any(|arg| arg == "/host/snapshot.pma:/bench/input/files/snapshot-pma-0.pma:ro"));
        assert!(args.iter().any(|arg| arg
            == "/host/snapshot.manifest:/bench/input/files/snapshot-manifest-0.manifest:ro"));
        assert!(args
            .iter()
            .any(|arg| arg == "/host/kernel.jam:/bench/input/files/kernel-0.jam:ro"));
    }

    #[test]
    fn containerized_generated_read_rewrites_snapshot_boot_paths() {
        let requested = RequestedCase {
            orchestrate: RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot {
                    pma: PathBuf::from("/host/snapshot.pma"),
                    manifest: PathBuf::from("/host/snapshot.manifest"),
                },
                kernel_path: PathBuf::from("/host/kernel.jam"),
                start_height: 0,
                end_height: None,
                count: Some(1),
                peek_mode: crate::speed_of_light::PeekMode::Warm,
            },
            ..RequestedCase::native(PathBuf::new())
        };
        let resolved = ResolvedCase {
            schema_version: super::super::RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            requested,
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate {
                source_kind: "generated_read".to_string(),
                source_plan_path: None,
                source_plan_sha256_hex: None,
                normalized_plan_sha256_hex: Some("plan-sha".to_string()),
                trusted_plan_relative_path: PathBuf::from("trusted_plan.json"),
                inputs: vec![
                    ResolvedInput {
                        input_id: "snapshot-pma-0".to_string(),
                        role: InputRole::SnapshotPma,
                        absolute_path: PathBuf::from("/host/snapshot.pma"),
                        sha256_hex: "pma-sha".to_string(),
                        size_bytes: 3,
                        container_path: Some(PathBuf::from(
                            "/bench/input/files/snapshot-pma-0.pma",
                        )),
                    },
                    ResolvedInput {
                        input_id: "snapshot-manifest-0".to_string(),
                        role: InputRole::SnapshotManifest,
                        absolute_path: PathBuf::from("/host/snapshot.manifest"),
                        sha256_hex: "manifest-sha".to_string(),
                        size_bytes: 3,
                        container_path: Some(PathBuf::from(
                            "/bench/input/files/snapshot-manifest-0.manifest",
                        )),
                    },
                    ResolvedInput {
                        input_id: "kernel-0".to_string(),
                        role: InputRole::Kernel,
                        absolute_path: PathBuf::from("/host/kernel.jam"),
                        sha256_hex: "kernel-sha".to_string(),
                        size_bytes: 3,
                        container_path: Some(PathBuf::from("/bench/input/files/kernel-0.jam")),
                    },
                ],
                step_count: 1,
                step_signature_sha256_hex: Some("step-sha".to_string()),
                read_range_resolution: None,
                contains_cold_steps: false,
            },
            absolute_fixture_path: PathBuf::new(),
            fixture_sha256_hex: String::new(),
            fixture_manifest: fixture_manifest(),
            execution_config: ExecutionConfig::default(),
            binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: None,
            },
            docker: None,
        };

        let containerized = containerize_resolved_case(&resolved);
        match containerized.requested.orchestrate {
            RequestedOrchestrate::GeneratedRead {
                boot: BootSourceInput::Snapshot { pma, manifest },
                kernel_path,
                ..
            } => {
                assert_eq!(pma, PathBuf::from("/bench/input/files/snapshot-pma-0.pma"));
                assert_eq!(
                    manifest,
                    PathBuf::from("/bench/input/files/snapshot-manifest-0.manifest")
                );
                assert_eq!(
                    kernel_path,
                    PathBuf::from("/bench/input/files/kernel-0.jam")
                );
            }
            other => panic!("expected generated read snapshot, got {other:?}"),
        }
    }

    #[test]
    fn containerized_plan_file_rewrites_source_plan_without_inputs_entry() {
        let tempdir = tempdir().expect("tempdir");
        let plan_path = tempdir.path().join("relative-plan.json");
        std::fs::write(&plan_path, "{}").expect("plan");
        let checkpoint_path = tempdir.path().join("checkpoint.chkjam");
        let kernel_path = tempdir.path().join("kernel.jam");
        std::fs::write(&checkpoint_path, [1, 2, 3]).expect("checkpoint");
        std::fs::write(&kernel_path, [4, 5, 6]).expect("kernel");
        let requested = RequestedCase {
            orchestrate: RequestedOrchestrate::PlanFile {
                plan_path: plan_path.clone(),
            },
            ..RequestedCase::native(PathBuf::new())
        };
        let resolved = ResolvedCase {
            schema_version: super::super::RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            requested,
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate {
                source_kind: "plan_file".to_string(),
                source_plan_path: Some(plan_path.clone()),
                source_plan_sha256_hex: Some("source-sha".to_string()),
                normalized_plan_sha256_hex: Some("plan-sha".to_string()),
                trusted_plan_relative_path: PathBuf::from("trusted_plan.json"),
                inputs: vec![
                    ResolvedInput {
                        input_id: "checkpoint-0".to_string(),
                        role: InputRole::Checkpoint,
                        absolute_path: checkpoint_path.clone(),
                        sha256_hex: "checkpoint-sha".to_string(),
                        size_bytes: 3,
                        container_path: Some(PathBuf::from(
                            "/bench/input/files/checkpoint-0.chkjam",
                        )),
                    },
                    ResolvedInput {
                        input_id: "kernel-0".to_string(),
                        role: InputRole::Kernel,
                        absolute_path: kernel_path.clone(),
                        sha256_hex: "kernel-sha".to_string(),
                        size_bytes: 3,
                        container_path: Some(PathBuf::from("/bench/input/files/kernel-0.jam")),
                    },
                ],
                step_count: 1,
                step_signature_sha256_hex: Some("step-sha".to_string()),
                read_range_resolution: None,
                contains_cold_steps: false,
            },
            absolute_fixture_path: PathBuf::new(),
            fixture_sha256_hex: String::new(),
            fixture_manifest: SolFixtureManifest {
                source_archive_path: String::new(),
                source_archive_event_num: None,
                checkpoint_kind: SolFixtureCheckpointKind::Derived,
                checkpoint_height: SolHeight(0),
                checkpoint_event_num: 0,
                archive_start_height: SolHeight(0),
                archive_end_height: SolHeight(0),
                include_mempool: false,
                chunk_size: 0,
                kernel_hash_hex: String::new(),
                checkpoint_hash_hex: String::new(),
                archive_hash_hex: String::new(),
            },
            execution_config: ExecutionConfig::default(),
            binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: None,
            },
            docker: None,
        };

        let container = containerize_resolved_case(&resolved);

        match container.requested.orchestrate {
            RequestedOrchestrate::PlanFile { plan_path } => {
                assert_eq!(plan_path, PathBuf::from("/bench/input/source-plan.json"));
            }
            other => panic!("expected plan file source, got {other:?}"),
        }
        assert_eq!(
            container.orchestrate.source_plan_path.as_deref(),
            Some(Path::new("/bench/input/source-plan.json"))
        );
        assert!(!container
            .orchestrate
            .inputs
            .iter()
            .any(|input| input.role == InputRole::SourcePlan));
        assert!(container
            .orchestrate
            .inputs
            .iter()
            .all(|input| !input.absolute_path.starts_with(tempdir.path())));
    }

    #[test]
    fn docker_run_once_command_uses_tmpfs_for_docker_tmpfs() {
        let plan = DockerRunPlan::for_run(
            "bench-harness-test",
            "nockchain-bench:test",
            "/host/fixture.soltest",
            "/host/output",
            "/host/input",
            None,
            "2g",
            None,
            None,
            None,
            WorkDirMode::DockerTmpfs,
            "run-0",
        );

        assert!(plan.args.iter().any(|arg| arg == "--tmpfs"));
        assert!(plan.args.iter().any(|arg| arg == "/bench/work"));
        assert!(plan
            .args
            .windows(2)
            .any(|window| window == ["--work-dir".to_string(), "/bench/work/run-0".to_string()]));
        assert!(!plan.args.iter().any(|arg| arg.contains("type=volume")));
    }

    #[test]
    fn docker_exec_run_once_command_uses_work_mount_for_runtime_state() {
        let args = docker_exec_run_once_args(
            "bench-harness-test",
            Path::new("/bench/output/runs/run-0"),
            "run-0",
        );

        assert!(args
            .windows(2)
            .any(|window| window == ["--work-dir".to_string(), "/bench/work/run-0".to_string()]));
    }

    #[test]
    fn docker_profile_command_mounts_fixture_output_and_limits() {
        let plan = DockerRunPlan::for_profile(
            "bench-harness-profile-test",
            "nockchain-bench:test",
            "/host/fixture.soltest",
            "/host/output",
            "/host/input",
            Some("/host/work"),
            "2g",
            Some("0-3"),
            Some(200_000),
            Some(100_000),
            WorkDirMode::HostBind,
            1_000,
            "/bench/output/profiles/samply-profile.json.gz",
            "/bench/output/profile-run",
        );

        assert_eq!(plan.program, "docker");
        assert!(plan.args.iter().any(|arg| arg == "--privileged"));
        assert!(plan.args.iter().any(|arg| arg == "--cgroupns=host"));
        assert!(plan.args.windows(2).any(|window| {
            window
                == [
                    "-e".to_string(),
                    "NOCKCHAIN_BENCH_COLD_CGROUP_PARENT=/sys/fs/cgroup".to_string(),
                ]
        }));
        assert!(!plan.args.iter().any(|arg| arg == "--rm"));
        assert!(plan.args.iter().any(|arg| arg == "--entrypoint"));
        assert!(plan.args.iter().any(|arg| arg == "samply"));
        assert!(plan.args.iter().any(|arg| arg == "--cap-add=PERFMON"));
        assert!(plan.args.iter().any(|arg| arg == "--memory=2g"));
        assert!(plan.args.iter().any(|arg| arg == "--cpuset-cpus=0-3"));
        assert!(plan.args.iter().any(|arg| arg == "--cpu-quota=200000"));
        assert!(plan.args.iter().any(|arg| arg == "--cpu-period=100000"));
        assert!(!plan
            .args
            .iter()
            .any(|arg| arg.contains("/bench/fixture.soltest")));
        assert!(plan
            .args
            .iter()
            .any(|arg| arg == "/host/output:/bench/output"));
        assert!(plan
            .args
            .iter()
            .any(|arg| arg == "/host/input:/bench/input:ro"));
        assert!(plan.args.iter().any(|arg| arg == "/host/work:/bench/work"));
        assert!(plan.args.ends_with(&[
            "nockchain-bench:test".to_string(),
            "record".to_string(),
            "--save-only".to_string(),
            "--rate".to_string(),
            "1000".to_string(),
            "--output".to_string(),
            "/bench/output/profiles/samply-profile.json.gz".to_string(),
            "--".to_string(),
            "nockchain-bench".to_string(),
            "sol".to_string(),
            "run-once".to_string(),
            "--resolved-case".to_string(),
            "/bench/input/resolved_case.json".to_string(),
            "--run-dir".to_string(),
            "/bench/output/profile-run".to_string(),
            "--work-dir".to_string(),
            "/bench/work/profile".to_string(),
            "--run-id".to_string(),
            "profile".to_string(),
        ]));
    }

    #[test]
    fn docker_profiler_request_targets_symbol_bundle_paths() {
        let request = build_docker_profiler_request(
            Path::new("/tmp/case-root"),
            CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            },
        );

        let artifact = request.artifact();
        assert_eq!(
            artifact.output_relative_path,
            PathBuf::from("profiles/samply-profile.json.gz")
        );
        assert_eq!(artifact.symbol_dir_relative_path, PathBuf::from("symbols"));
        assert_eq!(
            artifact.symbol_binary_relative_path,
            PathBuf::from("symbols/nockchain-bench")
        );
    }

    #[test]
    fn docker_profile_preflight_command_uses_perfmon_and_profiles_true() {
        let execution = DockerExecutionConfig {
            image: auto_build_image("nockchain-bench:test"),
            image_variant: DockerImageVariant::Profiling,
            memory_limit: "2g".to_string(),
            cpuset: Some("0-3".to_string()),
            cpu_quota: Some(200_000),
            cpu_period: Some(100_000),
            work_dir_mode: WorkDirMode::DockerTmpfs,
        };
        let args = docker_cpu_profiler_preflight_args(
            &execution,
            &resolved_test_image("sha256:test"),
            1_000,
        );

        assert_eq!(args[0], "run");
        assert!(args.iter().any(|arg| arg == "--privileged"));
        assert!(args.iter().any(|arg| arg == "--cgroupns=host"));
        assert!(args.windows(2).any(|window| {
            window
                == [
                    "-e".to_string(),
                    "NOCKCHAIN_BENCH_COLD_CGROUP_PARENT=/sys/fs/cgroup".to_string(),
                ]
        }));
        assert!(args.iter().any(|arg| arg == "--cap-add=PERFMON"));
        assert!(args.iter().any(|arg| arg == "--entrypoint"));
        assert!(args.iter().any(|arg| arg == "samply"));
        assert!(args.iter().any(|arg| arg == "--memory=2g"));
        assert!(args.iter().any(|arg| arg == "--cpuset-cpus=0-3"));
        assert!(args.iter().any(|arg| arg == "--cpu-quota=200000"));
        assert!(args.iter().any(|arg| arg == "--cpu-period=100000"));
        assert!(args.ends_with(&[
            "sha256:test".to_string(),
            "record".to_string(),
            "--save-only".to_string(),
            "--rate".to_string(),
            "1000".to_string(),
            "--output".to_string(),
            "/tmp/samply-preflight.json".to_string(),
            "--".to_string(),
            "/bin/true".to_string(),
        ]));
    }

    #[tokio::test]
    async fn docker_trusted_run_preflight_failure_rejects_stale_output_root() {
        let tempdir = tempdir().expect("tempdir");
        let fixture_path = write_fixture(tempdir.path());
        let requested = RequestedCase {
            cooldown_secs: 0,
            warmup_runs: 1,
            measured_runs: 3,
            execution: ExecutionRequest::Docker {
                image: auto_build_image("nockchain-bench:test"),
                memory_limit: "4g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: WorkDirMode::DockerTmpfs,
                allow_version_skew: false,
            },
            ..RequestedCase::native(fixture_path)
        };
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        std::fs::write(output_root.join("stale.txt"), "stale").expect("stale file");
        let backend = FakeDockerBackend::successful();
        let events = backend.shared_events();

        let error = execute_docker_trusted_run_with_hooks(
            backend,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
            |_requested, _config| {
                async {
                    Err(HarnessError::CommandFailure(
                        "preflight failed: samply missing from image".to_string(),
                    ))
                }
                .boxed()
            },
            |_resolved, _output_root, _config| {
                async { panic!("profile launch should not run after stale output rejection") }
                    .boxed()
            },
        )
        .await
        .expect_err("stale output root should be rejected before profiling preflight");

        assert!(error
            .to_string()
            .contains("already exists and is not empty"));
        assert!(events.lock().expect("events").is_empty());
        assert!(output_root.join("stale.txt").exists());
        assert!(!output_root.join("verdict.json").exists());
    }

    #[tokio::test]
    async fn docker_trusted_run_preflight_failure_stops_before_trusted_runs() {
        let tempdir = tempdir().expect("tempdir");
        let fixture_path = write_fixture(tempdir.path());
        let requested = RequestedCase {
            cooldown_secs: 0,
            warmup_runs: 1,
            measured_runs: 3,
            execution: ExecutionRequest::Docker {
                image: auto_build_image("nockchain-bench:test"),
                memory_limit: "4g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: WorkDirMode::DockerTmpfs,
                allow_version_skew: false,
            },
            ..RequestedCase::native(fixture_path)
        };
        let output_root = tempdir.path().join("out");
        let backend = FakeDockerBackend::successful();
        let events = backend.shared_events();

        let error = execute_docker_trusted_run_with_hooks(
            backend,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
            |_requested, _config| {
                async {
                    Err(HarnessError::CommandFailure(
                        "preflight failed: samply missing from image".to_string(),
                    ))
                }
                .boxed()
            },
            |_resolved, _output_root, _config| {
                async { panic!("profile launch should not run after preflight failure") }.boxed()
            },
        )
        .await
        .expect_err("preflight failure should fail before trusted runs");

        assert!(error.to_string().contains("preflight"));
        assert!(events.lock().expect("events").is_empty());
        let verdict = normalized_json(&output_root.join("verdict.json"));
        assert_eq!(
            verdict,
            serde_json::json!({
                "allow_debug_benchmark": false,
                "allow_degraded_cold": false,
                "allow_version_skew": false,
                "cv_threshold": 0.10,
                "schema_version": "verdict/v1",
                "validity": {
                    "Invalid": {
                        "reasons": [format!("cpu profiling failed: {error}")]
                    }
                }
            })
        );
    }

    #[test]
    fn docker_profile_errors_rewrite_missing_samply_message() {
        let error = map_docker_profiling_error(
            HarnessError::CommandFailure(
                "docker run failed: exec: \"samply\": executable file not found in $PATH"
                    .to_string(),
            ),
            "nockchain-bench:test",
        );

        match error {
            HarnessError::CommandFailure(message) => {
                assert!(message.contains("requires `samply`"));
                assert!(message.contains("nockchain-bench:test"));
            }
            other => panic!("expected command failure, got {other:?}"),
        }
    }

    #[test]
    fn docker_profile_errors_gain_perf_guidance() {
        let error = map_docker_profiling_error(
            HarnessError::CommandFailure(
                "docker run failed: perf_event_open failed: Operation not permitted".to_string(),
            ),
            "nockchain-bench:test",
        );

        match error {
            HarnessError::CommandFailure(message) => {
                assert!(message.contains("Operation not permitted"));
                assert!(message.contains("perf_event_paranoid"));
            }
            other => panic!("expected command failure, got {other:?}"),
        }
    }

    #[test]
    fn docker_run_artifact_semantics_include_container_samples() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let completed = super::super::execute::CompletedRun {
            record: super::super::execute::RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 100,
                failed_pokes: 0,
                init_time_secs: 1.0,
                total_replay_time_secs: 2.0,
                throughput_blocks_per_second: 50.0,
                average_block_time_ms: 20.0,
                peak_process_rss_bytes: Some(123.0),
                minor_faults_total: Some(4.0),
                major_faults_total: Some(0.0),
                final_tip_validation: None,
            },
            trusted_orchestrate_record: None,
            invalid_reasons: Vec::new(),
            block_timings: vec![super::super::execute::BlockTimingRecord {
                height: 1,
                duration_ms: 20.0,
            }],
            profile: None,
            bench_results: None,
        };
        let samples = vec![ContainerStats {
            timestamp_ms: 25,
            memory_usage_bytes: 1024,
            memory_limit_bytes: 2048,
            memory_percent: 50.0,
            memory_cache_bytes: 128,
            memory_rss_bytes: 768,
            cpu_percent: 90.0,
            minor_faults: Some(9),
            major_faults: Some(1),
        }];

        super::super::artifacts::write_run_artifacts(&run_dir, &completed)
            .expect("write run artifacts");
        super::super::artifacts::write_container_samples(&run_dir, &samples)
            .expect("write container samples");

        assert!(run_dir.join("result.json").exists());
        assert!(!run_dir.join("block_timings.ndjson").exists());
        assert!(run_dir.join("container_samples.ndjson").exists());
        assert!(run_dir.join("stdout.log").exists());
        assert!(run_dir.join("stderr.log").exists());
    }

    #[test]
    fn docker_container_samples_populate_run_resource_metrics() {
        let mut completed = super::super::execute::CompletedRun {
            record: super::super::execute::RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 100,
                failed_pokes: 0,
                init_time_secs: 1.0,
                total_replay_time_secs: 2.0,
                throughput_blocks_per_second: 50.0,
                average_block_time_ms: 20.0,
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
        };
        let samples = vec![
            ContainerStats {
                timestamp_ms: 0,
                memory_usage_bytes: 1024,
                memory_limit_bytes: 2048,
                memory_percent: 50.0,
                memory_cache_bytes: 128,
                memory_rss_bytes: 700,
                cpu_percent: 10.0,
                minor_faults: Some(100),
                major_faults: Some(1),
            },
            ContainerStats {
                timestamp_ms: 500,
                memory_usage_bytes: 1536,
                memory_limit_bytes: 2048,
                memory_percent: 75.0,
                memory_cache_bytes: 128,
                memory_rss_bytes: 900,
                cpu_percent: 20.0,
                minor_faults: Some(150),
                major_faults: Some(2),
            },
        ];

        populate_run_resource_metrics_from_container_samples(&mut completed, &samples);

        assert_eq!(completed.record.peak_process_rss_bytes, Some(900.0));
        assert_eq!(completed.record.minor_faults_total, Some(50.0));
        assert_eq!(completed.record.major_faults_total, Some(1.0));
    }

    fn write_fixture(root: &Path) -> PathBuf {
        let fixture_path = root.join("fixture.soltest");
        write_fixture_file(
            &fixture_path,
            &SolFixtureFile {
                manifest: SolFixtureManifest {
                    source_archive_path: "archive.solarch".to_string(),
                    source_archive_event_num: Some(1),
                    checkpoint_kind: crate::speed_of_light::SolFixtureCheckpointKind::Derived,
                    checkpoint_height: SolHeight(1),
                    checkpoint_event_num: 1,
                    archive_start_height: SolHeight(2),
                    archive_end_height: SolHeight(3),
                    include_mempool: false,
                    chunk_size: 8,
                    kernel_hash_hex: "kernel".to_string(),
                    checkpoint_hash_hex: "checkpoint".to_string(),
                    archive_hash_hex: "archive".to_string(),
                },
                checkpoint_bytes: Vec::new(),
                archive_bytes: Vec::new(),
                kernel_bytes: Vec::new(),
            },
        )
        .expect("write fixture");
        fixture_path
    }

    fn normalized_json(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
    }

    struct FakeDockerBackend {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl FakeDockerBackend {
        fn successful() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn shared_events(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.events)
        }
    }

    impl TrustedBackend for FakeDockerBackend {
        fn execute_run<'a>(
            &'a mut self,
            _resolved: &'a ResolvedCase,
            run_id: &'a str,
            run_dir: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<CompletedRun, HarnessError>> {
            self.events.lock().expect("events").push(run_id.to_string());
            let run_dir = run_dir.to_path_buf();
            async move {
                let completed = CompletedRun {
                    record: RunRecord {
                        run_id: run_id.to_string(),
                        success: true,
                        error: None,
                        blocks_poked: 1,
                        failed_pokes: 0,
                        init_time_secs: 1.0,
                        total_replay_time_secs: 2.0,
                        throughput_blocks_per_second: 10.0,
                        average_block_time_ms: 100.0,
                        peak_process_rss_bytes: Some(128.0),
                        minor_faults_total: Some(10.0),
                        major_faults_total: Some(0.0),
                        final_tip_validation: None,
                    },
                    trusted_orchestrate_record: None,
                    invalid_reasons: Vec::new(),
                    bench_results: None,
                    profile: None,
                    block_timings: vec![BlockTimingRecord {
                        height: 2,
                        duration_ms: 100.0,
                    }],
                };
                write_run_artifacts(&run_dir, &completed).expect("run artifacts");
                Ok(completed)
            }
            .boxed()
        }

        fn prepare<'a>(
            &'a mut self,
            _resolved: &'a mut ResolvedCase,
            _output_root: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
            self.events
                .lock()
                .expect("events")
                .push("prepare".to_string());
            async { Ok(()) }.boxed()
        }

        fn capture_runtime_facts(&self) -> Result<BackendRuntimeFacts, HarnessError> {
            self.events
                .lock()
                .expect("events")
                .push("runtime-facts".to_string());
            Ok(BackendRuntimeFacts::Docker {
                host_binary: BinaryIdentity {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    build_profile: "release".to_string(),
                    git_commit: Some("host".to_string()),
                },
                container_binary: BinaryIdentity {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    build_profile: "release".to_string(),
                    git_commit: Some("host".to_string()),
                },
                image_source: auto_build_image("nockchain-bench:test"),
                requested_image_ref: "nockchain-bench:test".to_string(),
                resolved_image_ref: "sha256:test".to_string(),
                image_digest: "sha256:test".to_string(),
                container_id: "container-id".to_string(),
                docker_engine_version: "29.1.3".to_string(),
                docker_context: "default".to_string(),
                cgroup_version: "2".to_string(),
                storage_driver: "overlayfs".to_string(),
                realized_memory_max: 4 * 1024 * 1024 * 1024,
                realized_memory_current: 512,
                realized_cpuset: None,
                realized_cpu_max: None,
            })
        }

        fn capture_raw_evidence<'a>(
            &'a self,
            _raw_dir: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
            self.events
                .lock()
                .expect("events")
                .push("raw-evidence".to_string());
            async { Ok(()) }.boxed()
        }

        fn cleanup<'a>(&'a mut self) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
            self.events
                .lock()
                .expect("events")
                .push("cleanup".to_string());
            async { Ok(()) }.boxed()
        }
    }

    #[test]
    fn parse_container_binary_identity_json_preserves_commit() {
        let identity = parse_binary_identity_json(
            r#"{
                "version":"0.1.0",
                "build_profile":"release",
                "git_commit":"abc123"
            }"#,
        )
        .expect("parse binary identity");

        assert_eq!(identity.version, "0.1.0");
        assert_eq!(identity.build_profile, "release");
        assert_eq!(identity.git_commit.as_deref(), Some("abc123"));
    }

    #[test]
    fn verify_version_skew_rejects_commit_mismatch() {
        let host = BinaryIdentity {
            version: "0.1.0".to_string(),
            build_profile: "release".to_string(),
            git_commit: Some("host-commit".to_string()),
        };
        let container = BinaryIdentity {
            version: "0.1.0".to_string(),
            build_profile: "release".to_string(),
            git_commit: Some("container-commit".to_string()),
        };

        let error = verify_version_skew(&host, &container).expect_err("commit mismatch");
        assert!(error
            .to_string()
            .contains("host/container git commit skew detected"));
    }

    #[test]
    fn docker_sample_prefers_realized_memory_limit() {
        let limit = resolve_sample_memory_limit_bytes(8_210_616_320, Some(8_589_934_592));
        assert_eq!(limit, 8_589_934_592);
    }

    #[test]
    fn benchmark_proc_stat_path_uses_recorded_pid() {
        assert_eq!(
            benchmark_proc_stat_path(Some(4321)),
            Some("/proc/4321/stat".to_string())
        );
        assert_eq!(benchmark_proc_stat_path(None), None);
    }

    #[test]
    fn require_cgroup_v2_accepts_v2() {
        let info = json!({ "CgroupVersion": "2" });
        assert_eq!(require_cgroup_v2(&info).expect("expected v2"), "2");
    }

    #[test]
    fn require_cgroup_v2_rejects_non_v2() {
        let info = json!({ "CgroupVersion": "1" });
        let error = require_cgroup_v2(&info).expect_err("expected cgroup v1 rejection");
        assert!(error
            .to_string()
            .contains("trusted Docker runs require cgroup v2"));
    }

    #[test]
    fn docker_validation_allows_non_cold_non_cgroup_v2() {
        let tempdir = tempdir().expect("tempdir");
        let mut backend = DockerBackend {
            execution: DockerExecutionConfig {
                image: auto_build_image("nockchain-bench:test"),
                image_variant: DockerImageVariant::Standard,
                memory_limit: "1g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: crate::speed_of_light::harness::case::WorkDirMode::DockerTmpfs,
            },
            state: None,
            pending_resources: None,
        };
        let inputs = PreparedDockerInputs {
            output_root: tempdir.path().to_path_buf(),
            input_root: tempdir.path().join("input"),
            resolved_image: resolved_test_image("nockchain-bench:test"),
            validation_key: ValidationCacheKey {
                docker_engine_version: "28.0".to_string(),
                cgroup_version: "1".to_string(),
                image_digest: "sha256:test".to_string(),
                memory_limit: "1g".to_string(),
                cpuset: None,
                cpu_quota: None,
                cpu_period: None,
                work_dir_mode: crate::speed_of_light::harness::case::WorkDirMode::DockerTmpfs,
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
            },
            requested_memory_limit_bytes: 1024 * 1024 * 1024,
        };

        let outcome = backend
            .resolve_validation_outcome(&inputs, false, false)
            .expect("non-cold cgroup v1 should skip cold validation");
        assert_eq!(outcome, BackendValidationOutcome::default());
        assert!(backend
            .resolve_validation_outcome(&inputs, false, true)
            .is_err());
    }

    #[test]
    fn cgroup_v2_paths_match_expected() {
        assert_eq!(CGROUP_V2_MEMORY_MAX_PATH, "/sys/fs/cgroup/memory.max");
        assert_eq!(
            CGROUP_V2_MEMORY_CURRENT_PATH,
            "/sys/fs/cgroup/memory.current"
        );
        assert_eq!(CGROUP_V2_CPU_MAX_PATH, "/sys/fs/cgroup/cpu.max");
        assert_eq!(
            CGROUP_V2_CPUSET_EFFECTIVE_PATH,
            "/sys/fs/cgroup/cpuset.cpus.effective"
        );
        assert_eq!(CGROUP_V2_CPUSET_PATH, "/sys/fs/cgroup/cpuset.cpus");
    }

    #[test]
    fn optional_runtime_facts_treat_read_errors_as_missing() {
        let missing = HarnessError::CommandFailure("missing".to_string());
        assert_eq!(
            optional_runtime_fact(Err(missing)).expect("missing becomes none"),
            None
        );
    }

    #[test]
    fn optional_runtime_facts_preserve_present_values() {
        assert_eq!(
            optional_runtime_fact(Ok(Some("0-3".to_string()))).expect("present value"),
            Some("0-3".to_string())
        );
        assert_eq!(
            optional_runtime_fact(Ok(None)).expect("empty value remains none"),
            None
        );
    }

    #[tokio::test]
    async fn wait_for_benchmark_pid_observes_late_pid_file() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().to_path_buf();
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let writer_dir = run_dir.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(20)).await;
            std::fs::write(writer_dir.join(".benchmark.pid"), "4321\n").expect("pid file");
            let _ = stop_tx.send(false);
        });

        let pid = wait_for_benchmark_pid(&run_dir, Duration::from_millis(200), &mut stop_rx).await;
        assert_eq!(pid, Some(4321));
    }

    #[test]
    fn docker_validation_preflight_failure_persists_partial_artifact() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        let key = ValidationCacheKey {
            docker_engine_version: "28.0.1".to_string(),
            cgroup_version: "2".to_string(),
            image_digest: UNRESOLVED_IMAGE_DIGEST.to_string(),
            memory_limit: "8g".to_string(),
            cpuset: None,
            cpu_quota: None,
            cpu_period: None,
            work_dir_mode: WorkDirMode::DockerTmpfs,
            probe_version: VALIDATION_PROBE_VERSION.to_string(),
        };

        persist_preflight_validation_failure(
            &output_root,
            key,
            false,
            HarnessError::CommandFailure("docker start failed".to_string()).to_string(),
        )
        .expect("persist validation");

        let persisted = read_validation_record(&output_root).expect("validation artifact");
        assert_eq!(persisted.status, ValidationStatus::Invalid);
        assert!(!persisted.container_started);
        assert!(persisted.docker_reports_cgroup_v2);
        assert_eq!(
            persisted.failure_reason.as_deref(),
            Some("Command failure: docker start failed")
        );
    }

    #[test]
    fn docker_validation_finalization_prefers_prepare_error() {
        let result = finalize_validation_results(
            Err(HarnessError::InvalidRequestedCase(
                "prepare failed".to_string(),
            )),
            Err(HarnessError::CommandFailure("raw failed".to_string())),
            Err(HarnessError::CommandFailure("cleanup failed".to_string())),
        )
        .expect_err("prepare error should win");

        assert!(result.to_string().contains("prepare failed"));
    }

    #[test]
    fn immutable_image_identity_validation_cache_key_changes_with_identity() {
        let execution = DockerExecutionConfig {
            image: auto_build_image("nockchain-bench:test"),
            image_variant: DockerImageVariant::Standard,
            memory_limit: "8g".to_string(),
            cpuset: None,
            cpu_quota: None,
            cpu_period: None,
            work_dir_mode: WorkDirMode::DockerTmpfs,
        };
        let docker_info = json!({
            "ServerVersion": "28.0.1",
            "CgroupVersion": "2"
        });

        let digest_key = build_validation_key(
            &execution, &docker_info, "ghcr.io/org/nockchain-bench@sha256:abc",
        );
        let image_id_key = build_validation_key(&execution, &docker_info, "sha256:local-image");

        assert_ne!(digest_key, image_id_key);
        assert_eq!(
            digest_key.image_digest,
            "ghcr.io/org/nockchain-bench@sha256:abc"
        );
        assert_eq!(image_id_key.image_digest, "sha256:local-image");
    }

    #[test]
    fn immutable_image_identity_runtime_facts_record_resolved_identity() {
        let facts = BackendRuntimeFacts::Docker {
            host_binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("host".to_string()),
            },
            container_binary: BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("host".to_string()),
            },
            image_source: auto_build_image("nockchain-bench:test"),
            requested_image_ref: "nockchain-bench:test".to_string(),
            resolved_image_ref: "sha256:local-image".to_string(),
            image_digest: "sha256:local-image".to_string(),
            container_id: "container-id".to_string(),
            docker_engine_version: "29.1.3".to_string(),
            docker_context: "desktop-linux".to_string(),
            cgroup_version: "2".to_string(),
            storage_driver: "overlayfs".to_string(),
            realized_memory_max: 8 * 1024 * 1024 * 1024,
            realized_memory_current: 512,
            realized_cpuset: Some("0-3".to_string()),
            realized_cpu_max: Some("max 100000".to_string()),
        };

        let value = serde_json::to_value(&facts).expect("serialize facts");
        let docker = value.get("Docker").expect("docker facts");
        assert_eq!(
            docker.get("requested_image_ref"),
            Some(&json!("nockchain-bench:test"))
        );
        assert_eq!(
            docker.get("resolved_image_ref"),
            Some(&json!("sha256:local-image"))
        );
        assert_eq!(
            docker.get("image_digest"),
            Some(&json!("sha256:local-image"))
        );
    }

    #[test]
    fn docker_cleanup_uses_pending_resources_before_state_exists() {
        let mut calls = Vec::new();
        cleanup_docker_resources(
            None,
            Some(&PendingDockerResources {
                container_name: Some("bench-container".to_string()),
                volume_name: Some("bench-container-work".to_string()),
            }),
            |args| {
                calls.push(args.to_vec());
                Ok(())
            },
        )
        .expect("cleanup succeeds");

        assert_eq!(
            calls,
            vec![
                vec!["rm".to_string(), "-f".to_string(), "bench-container".to_string()],
                vec![
                    "volume".to_string(),
                    "rm".to_string(),
                    "-f".to_string(),
                    "bench-container-work".to_string(),
                ],
            ]
        );
    }

    #[test]
    fn docker_profile_cleanup_failure_is_reported_alongside_profiling_failure() {
        let error = finalize_profile_cleanup_results(
            Err(HarnessError::CommandFailure(
                "profile launch failed".to_string(),
            )),
            Err(HarnessError::CommandFailure("docker rm failed".to_string())),
        )
        .expect_err("profiling failure should remain an error");

        let message = error.to_string();
        assert!(message.contains("profile launch failed"));
        assert!(message.contains("cleanup after profiling failure also failed"));
        assert!(message.contains("docker rm failed"));
    }
}
