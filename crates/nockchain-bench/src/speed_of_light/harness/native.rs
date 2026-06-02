use std::path::Path;

use futures::FutureExt;

use super::artifacts::write_cpu_profile_artifact;
use super::case::{RequestedCase, ResolvedCase};
use super::execute::{cpu_profile_output_relative_path, execute_once, CpuProfileExecutionKind};
use super::orchestrate::{
    execute_trusted_run, prepare_output_root, TrustedBackend, TrustedRunResult,
};
use super::profiler::{
    build_run_once_command, cpu_profile_symbol_binary_relative_path,
    cpu_profile_symbol_dir_relative_path, ensure_samply_profiled_binary,
    invalidate_verdict_for_cpu_profiling_failure, CpuProfilerLaunchRequest, CpuProfilerLauncher,
    SystemCpuProfilerLauncher,
};
use super::provenance::{BackendRuntimeFacts, Provenance};
use super::summary::{RunSummary, Verdict};
use super::{CpuProfilerConfig, HarnessError};

#[derive(Debug)]
pub struct NativeRunResult {
    pub resolved: ResolvedCase,
    pub provenance: Provenance,
    pub summary: RunSummary,
    pub verdict: Verdict,
}

impl From<TrustedRunResult> for NativeRunResult {
    fn from(value: TrustedRunResult) -> Self {
        Self {
            resolved: value.resolved,
            provenance: value.provenance,
            summary: value.summary,
            verdict: value.verdict,
        }
    }
}

pub async fn execute_native_trusted_run(
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
    cpu_profiler: Option<CpuProfilerConfig>,
) -> Result<NativeRunResult, HarnessError> {
    execute_native_trusted_run_with_backend_and_profiler(
        NativeBackend, SystemCpuProfilerLauncher, requested, output_root, allow_debug_benchmark,
        cpu_profiler,
    )
    .await
}

pub async fn execute_native_cpu_profile(
    output_root: &Path,
    cpu_profiler: CpuProfilerConfig,
) -> Result<super::execute::CpuProfileArtifact, HarnessError> {
    let request = build_native_profiler_request(output_root, cpu_profiler)?;
    let mut launcher = SystemCpuProfilerLauncher;
    launcher.preflight(&request).await?;
    launcher.launch(&request).await
}

pub async fn execute_native_cpu_profile_for_resolved_case(
    output_root: &Path,
    resolved_case_path: &Path,
    cpu_profiler: CpuProfilerConfig,
) -> Result<super::execute::CpuProfileArtifact, HarnessError> {
    let request = build_native_profiler_request_with_resolved_case(
        output_root, resolved_case_path, cpu_profiler,
    )?;
    let mut launcher = SystemCpuProfilerLauncher;
    launcher.preflight(&request).await?;
    launcher.launch(&request).await
}

#[cfg(test)]
async fn execute_native_trusted_run_with_backend<B: TrustedBackend>(
    backend: B,
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
) -> Result<NativeRunResult, HarnessError> {
    execute_native_trusted_run_with_backend_and_profiler(
        backend, SystemCpuProfilerLauncher, requested, output_root, allow_debug_benchmark, None,
    )
    .await
}

async fn execute_native_trusted_run_with_backend_and_profiler<
    B: TrustedBackend,
    P: CpuProfilerLauncher,
>(
    backend: B,
    profiler_launcher: P,
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
    cpu_profiler: Option<CpuProfilerConfig>,
) -> Result<NativeRunResult, HarnessError> {
    execute_native_trusted_run_with_backend_and_profiling_hooks(
        backend, profiler_launcher, build_native_profiler_request, write_cpu_profile_artifact,
        requested, output_root, allow_debug_benchmark, cpu_profiler,
    )
    .await
}

async fn execute_native_trusted_run_with_backend_and_profiling_hooks<
    B: TrustedBackend,
    P: CpuProfilerLauncher,
    R,
    W,
>(
    backend: B,
    mut profiler_launcher: P,
    mut request_builder: R,
    mut artifact_writer: W,
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
    cpu_profiler: Option<CpuProfilerConfig>,
) -> Result<NativeRunResult, HarnessError>
where
    R: FnMut(&Path, CpuProfilerConfig) -> Result<CpuProfilerLaunchRequest, HarnessError>,
    W: FnMut(&Path, &super::execute::CpuProfileArtifact) -> Result<(), HarnessError>,
{
    if cpu_profiler.is_some() {
        prepare_output_root(output_root)?;
    }

    let profiling_request = if let Some(config) = cpu_profiler {
        let request = match request_builder(output_root, config) {
            Ok(request) => request,
            Err(error) => {
                invalidate_verdict_for_cpu_profiling_failure(output_root, &error)?;
                return Err(error);
            }
        };
        if let Err(error) = profiler_launcher.preflight(&request).await {
            invalidate_verdict_for_cpu_profiling_failure(output_root, &error)?;
            return Err(error);
        }
        Some(request)
    } else {
        None
    };

    let run = execute_trusted_run(backend, requested, output_root, allow_debug_benchmark).await?;
    if let Some(request) = profiling_request {
        let profiling_result = async {
            let artifact = profiler_launcher.launch(&request).await?;
            artifact_writer(output_root, &artifact)
        }
        .await;

        if let Err(error) = profiling_result {
            invalidate_verdict_for_cpu_profiling_failure(output_root, &error)?;
            return Err(error);
        }
    }
    Ok(run.into())
}

fn build_native_profiler_request(
    output_root: &Path,
    config: CpuProfilerConfig,
) -> Result<CpuProfilerLaunchRequest, HarnessError> {
    build_native_profiler_request_with_resolved_case(
        output_root,
        &output_root.join("resolved_case.json"),
        config,
    )
}

fn build_native_profiler_request_with_resolved_case(
    output_root: &Path,
    resolved_case_path: &Path,
    config: CpuProfilerConfig,
) -> Result<CpuProfilerLaunchRequest, HarnessError> {
    let current_binary = std::env::current_exe()?;
    let profiled_binary = match config.kind {
        super::CpuProfilerKind::Samply => ensure_samply_profiled_binary(&current_binary)?,
    };
    let profile_run_dir = output_root.join("profile-run");
    let output_relative_path = cpu_profile_output_relative_path(config.kind);
    let profiled_command = build_run_once_command(
        &path_string(&profiled_binary),
        &path_string(resolved_case_path),
        &path_string(&profile_run_dir),
        "profile",
    );

    Ok(CpuProfilerLaunchRequest {
        profiler_kind: config.kind,
        sample_rate_hz: config.sample_rate_hz,
        execution_kind: CpuProfileExecutionKind::Native,
        case_root: output_root.to_path_buf(),
        output_relative_path,
        symbol_dir_relative_path: cpu_profile_symbol_dir_relative_path(),
        symbol_binary_relative_path: cpu_profile_symbol_binary_relative_path(),
        profiled_run_dir: profile_run_dir,
        profiled_command,
    })
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

struct NativeBackend;

impl TrustedBackend for NativeBackend {
    fn execute_run<'a>(
        &'a mut self,
        resolved: &'a ResolvedCase,
        run_id: &'a str,
        run_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<super::execute::CompletedRun, HarnessError>> {
        execute_once(resolved, run_id, run_dir).boxed()
    }

    fn prepare<'a>(
        &'a mut self,
        _resolved: &'a mut ResolvedCase,
        _output_root: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async { Ok(()) }.boxed()
    }

    fn capture_runtime_facts(&self) -> Result<BackendRuntimeFacts, HarnessError> {
        Ok(BackendRuntimeFacts::Native)
    }

    fn capture_raw_evidence<'a>(
        &'a self,
        _raw_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async { Ok(()) }.boxed()
    }

    fn cleanup<'a>(&'a mut self) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
        async { Ok(()) }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use futures::FutureExt;
    use nockapp::nockapp::save::JammedCheckpointV2;
    use nockapp::JammedNoun;
    use tempfile::tempdir;

    use super::{
        execute_native_trusted_run_with_backend,
        execute_native_trusted_run_with_backend_and_profiling_hooks, path_string, NativeRunResult,
    };
    use crate::speed_of_light::fixture::SolFixtureManifest;
    use crate::speed_of_light::harness::artifacts::{
        read_cpu_profile_artifact, write_cpu_profile_artifact, write_run_artifacts,
    };
    use crate::speed_of_light::harness::case::{
        BinaryIdentity, ExecutionConfig, RequestedCase, RequestedOrchestrate, ResolvedCase,
        ResolvedOrchestrate,
    };
    use crate::speed_of_light::harness::execute::{
        cpu_profile_output_relative_path, BlockTimingRecord, CompletedRun, CpuProfileArtifact,
        CpuProfileExecutionKind, RunRecord,
    };
    use crate::speed_of_light::harness::orchestrate::{
        prepare_output_root, TrustedBackend, TrustedRunResult,
    };
    use crate::speed_of_light::harness::profiler::{
        build_run_once_command, cpu_profile_symbol_binary_relative_path,
        cpu_profile_symbol_dir_relative_path,
    };
    use crate::speed_of_light::harness::provenance::{
        BackendRuntimeFacts, HostIdentity, Provenance,
    };
    use crate::speed_of_light::harness::summary::{RunSummary, Validity, Verdict};
    use crate::speed_of_light::harness::{
        CpuProfilerConfig, CpuProfilerKind, CpuProfilerLaunchRequest, CpuProfilerLauncher,
        HarnessError, PROVENANCE_SCHEMA_VERSION, RESOLVED_CASE_SCHEMA_VERSION,
        SUMMARY_SCHEMA_VERSION, VERDICT_SCHEMA_VERSION,
    };
    use crate::speed_of_light::types::SolHeight;

    fn checkpoint_boot(path: &Path) -> serde_json::Value {
        serde_json::json!({ "type": "checkpoint", "checkpoint": path })
    }

    fn write_checkpoint(path: &Path, event_num: u64) {
        let checkpoint = JammedCheckpointV2::new(
            blake3::hash(b"kernel"),
            event_num,
            JammedNoun::new(Bytes::from_static(b"cold")),
            JammedNoun::new(Bytes::from_static(b"state")),
        );
        std::fs::write(path, checkpoint.encode().expect("encode checkpoint")).expect("checkpoint");
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

    fn native_binary() -> BinaryIdentity {
        BinaryIdentity {
            version: "0.1.0".to_string(),
            build_profile: "release".to_string(),
            git_commit: None,
        }
    }

    fn resolved_native_case(requested: RequestedCase) -> ResolvedCase {
        ResolvedCase {
            schema_version: RESOLVED_CASE_SCHEMA_VERSION.to_string(),
            benchmark: "sol-orchestrate".to_string(),
            orchestrate: ResolvedOrchestrate::for_requested(&requested),
            requested,
            absolute_fixture_path: PathBuf::from("/tmp/fixture.soltest"),
            fixture_sha256_hex: "abc".to_string(),
            fixture_manifest: fixture_manifest(),
            execution_config: ExecutionConfig::default(),
            binary: native_binary(),
            docker: None,
        }
    }

    fn native_provenance(resolved: &ResolvedCase) -> Provenance {
        Provenance {
            schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
            capture_timestamp_ms: 1,
            host: HostIdentity {
                hostname: Some("host".to_string()),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                kernel: None,
                cpu_count: 4,
                total_memory_bytes: None,
                cpu_model: None,
            },
            git: None,
            backend: BackendRuntimeFacts::Native,
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: None,
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

    #[test]
    fn native_run_rejects_non_empty_output_root() {
        let tempdir = tempdir().expect("tempdir");
        std::fs::write(tempdir.path().join("stale.txt"), "stale").expect("stale file");

        let error = prepare_output_root(tempdir.path()).expect_err("should reject stale output");
        assert!(error
            .to_string()
            .contains("already exists and is not empty"));
    }

    #[test]
    fn native_run_allows_empty_output_root() {
        let tempdir = tempdir().expect("tempdir");
        prepare_output_root(tempdir.path()).expect("empty dir should be allowed");
    }

    #[test]
    fn native_run_result_converts_from_trusted_run_result() {
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        let resolved = resolved_native_case(requested.clone());
        let trusted = TrustedRunResult {
            resolved: resolved.clone(),
            provenance: native_provenance(&resolved),
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
                pokes_per_second: None,
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
                validity: Validity::Valid,
            },
        };

        let native = NativeRunResult::from(trusted);

        assert_eq!(native.resolved, resolved);
        assert_eq!(native.provenance.backend, BackendRuntimeFacts::Native);
        assert_eq!(native.verdict.validity, Validity::Valid);
    }

    #[tokio::test]
    async fn native_trusted_run_preserves_artifact_semantics_after_refactor() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();
        let events = backend.shared_events();

        let result =
            execute_native_trusted_run_with_backend(backend, requested, &output_root, false)
                .await
                .expect("native trusted run result");

        assert_eq!(
            events.lock().expect("events").clone(),
            vec![
                "prepare", "runtime-facts", "raw-evidence", "warmup-0", "run-0", "run-1", "run-2",
                "cleanup",
            ]
        );
        assert_eq!(result.provenance.backend, BackendRuntimeFacts::Native);
        assert_eq!(
            result.provenance.binary.git_commit,
            result.resolved.binary.git_commit
        );
        assert_eq!(result.summary.measured_runs_requested, 3);
        assert_eq!(result.summary.measured_runs_succeeded, 3);
        assert_eq!(result.verdict.validity, Validity::Valid);

        let root_entries = sorted_relative_paths(&output_root);
        assert_eq!(root_entries, expected_trusted_artifact_tree());

        let requested_json = normalized_json(&output_root.join("requested_case.json"));
        assert_eq!(requested_json["schema_version"], "requested-case/v2");
        assert_eq!(requested_json["benchmark"], "sol-orchestrate");
        assert_eq!(requested_json["orchestrate"]["source"], "plan_file");
        assert_eq!(
            requested_json["orchestrate"]["plan_path"],
            serde_json::json!(tempdir.path().join("trusted-input-plan.json"))
        );

        let resolved_json = normalized_json(&output_root.join("resolved_case.json"));
        assert_eq!(resolved_json["schema_version"], "resolved-case/v2");
        assert_eq!(resolved_json["benchmark"], "sol-orchestrate");
        assert_eq!(resolved_json["orchestrate"]["source_kind"], "plan_file");
        assert_eq!(
            resolved_json["requested"]["orchestrate"]["plan_path"],
            serde_json::json!(tempdir.path().join("trusted-input-plan.json"))
        );
        assert_eq!(
            normalized_json(&output_root.join("summary.json")),
            serde_json::json!({
                "average_block_time_ms": uniform_stats_json(100.0),
                "benchmark": "sol-orchestrate",
                "failed_runs": [],
                "aggregate": {
                    "block_pokes_per_second": uniform_stats_json(10.0),
                    "init_time_secs": uniform_stats_json(1.0),
                    "pokes_per_second": uniform_stats_json(10.0),
                    "total_step_time_secs": uniform_stats_json(2.0)
                },
                "block_pokes_per_second": uniform_stats_json(10.0),
                "by_step_type": {},
                "init_time_secs": uniform_stats_json(1.0),
                "major_faults_total": uniform_stats_json(0.0),
                "measured_runs_requested": 3,
                "measured_runs_succeeded": 3,
                "minor_faults_total": uniform_stats_json(10.0),
                "peak_process_rss_bytes": uniform_stats_json(128.0),
                "pokes_per_second": uniform_stats_json(10.0),
                "raw_tx_pokes_per_second": null,
                "peeks_per_second": null,
                "cold_peeks_per_second": null,
                "schema_version": "summary/v1",
                "steps": [],
                "steps_per_second": null,
                "total_step_time_secs": uniform_stats_json(2.0)
            })
        );
        assert_eq!(
            normalized_json(&output_root.join("verdict.json")),
            serde_json::json!({
                "allow_debug_benchmark": false,
                "allow_degraded_cold": false,
                "allow_version_skew": false,
                "cv_threshold": 0.10,
                "schema_version": "verdict/v1",
                "validity": "Valid"
            })
        );
        let expected_provenance = {
            #[allow(unused_mut)]
            let mut value = serde_json::json!({
                "allow_debug_benchmark": false,
                "allow_degraded_cold": false,
                "allow_version_skew": false,
                "backend": "Native",
                "binary": {
                    "build_profile": crate::speed_of_light::harness::current_binary_identity().build_profile,
                    "git_commit": "<normalized>",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capture_timestamp_ms": "<normalized>",
                "cv_threshold": null,
                "fixture_manifest": {
                    "archive_end_height": 0,
                    "archive_hash_hex": "",
                    "archive_start_height": 0,
                    "checkpoint_hash_hex": "",
                    "checkpoint_event_num": 0,
                    "checkpoint_height": 0,
                    "checkpoint_kind": "derived",
                    "chunk_size": 0,
                    "include_mempool": false,
                    "kernel_hash_hex": "",
                    "source_archive_event_num": null,
                    "source_archive_path": "",
                },
                "fixture_path": "",
                "fixture_sha256_hex": "<normalized>",
                "git": "<normalized>",
                "host": "<normalized>",
                "schema_version": "provenance/v2",
            });
            {
                let object = value.as_object_mut().expect("provenance object");
                object.insert("runtime_flavor".to_string(), serde_json::json!("pma"));
                object.insert("boot_source".to_string(), serde_json::json!("checkpoint"));
                object.insert("boot_event_num".to_string(), serde_json::json!(0));
                object.insert("pma_fsync_mode".to_string(), serde_json::json!("on"));
            }
            value
        };
        assert_eq!(
            normalized_json(&output_root.join("provenance.json")),
            expected_provenance
        );
    }

    #[tokio::test]
    async fn native_trusted_run_records_pma_fsync_mode_on() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();

        let result =
            execute_native_trusted_run_with_backend(backend, requested, &output_root, false)
                .await
                .expect("native trusted run result");

        assert_eq!(result.provenance.backend, BackendRuntimeFacts::Native);

        let root_entries = sorted_relative_paths(&output_root);
        assert_eq!(root_entries, expected_trusted_artifact_tree());

        let provenance = normalized_json(&output_root.join("provenance.json"));
        assert_eq!(
            provenance.get("backend"),
            Some(&serde_json::json!("Native"))
        );
        assert_eq!(
            provenance.get("runtime_flavor"),
            Some(&serde_json::json!("pma"))
        );
        assert_eq!(
            provenance.get("boot_source"),
            Some(&serde_json::json!("checkpoint"))
        );
        assert_eq!(
            provenance.get("boot_event_num"),
            Some(&serde_json::json!(
                result.resolved.fixture_manifest.checkpoint_event_num
            ))
        );
        assert_eq!(
            provenance.get("pma_fsync_mode"),
            Some(&serde_json::json!("on"))
        );
        assert!(provenance.get("pma_work_dir_mode").is_none());
        assert_eq!(
            result.provenance.boot_event_num,
            Some(result.resolved.fixture_manifest.checkpoint_event_num)
        );
        assert_eq!(result.provenance.boot_source.as_deref(), Some("checkpoint"));
        assert_eq!(result.provenance.runtime_flavor.as_deref(), Some("pma"));
        assert_eq!(result.provenance.pma_fsync_mode.as_deref(), Some("on"));
        assert_eq!(result.provenance.pma_work_dir_mode, None);
    }

    #[tokio::test]
    async fn native_trusted_run_records_pma_fsync_mode_off() {
        let tempdir = tempdir().expect("tempdir");
        let mut requested = write_requested_case(tempdir.path());
        requested.fsync = false;
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();

        let result =
            execute_native_trusted_run_with_backend(backend, requested, &output_root, false)
                .await
                .expect("native trusted run result");

        let provenance = normalized_json(&output_root.join("provenance.json"));
        assert_eq!(
            provenance.get("pma_fsync_mode"),
            Some(&serde_json::json!("off"))
        );
        assert_eq!(result.provenance.pma_fsync_mode.as_deref(), Some("off"));
    }

    #[tokio::test]
    async fn native_trusted_run_writes_cpu_profile_artifacts() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();
        let events = backend.shared_events();
        let profiler = FakeCpuProfilerLauncher::new(events.clone());

        let result = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            profiler,
            fake_native_profiler_request,
            write_cpu_profile_artifact,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
        )
        .await
        .expect("native trusted run result");

        assert_eq!(
            events.lock().expect("events").clone(),
            vec![
                "prepare", "runtime-facts", "raw-evidence", "warmup-0", "run-0", "run-1", "run-2",
                "cleanup", "profile",
            ]
        );
        assert_eq!(result.summary.measured_runs_requested, 3);
        assert_eq!(result.summary.measured_runs_succeeded, 3);
        assert_eq!(result.verdict.validity, Validity::Valid);

        let artifact = read_cpu_profile_artifact(&output_root).expect("cpu profile artifact");
        assert_eq!(artifact.profiler_kind, CpuProfilerKind::Samply);
        assert_eq!(artifact.sample_rate_hz, 1_000);
        assert_eq!(artifact.execution_kind, CpuProfileExecutionKind::Native);
        assert_eq!(
            artifact.output_relative_path,
            cpu_profile_output_relative_path(CpuProfilerKind::Samply)
        );
        assert_eq!(artifact.symbol_dir_relative_path, Path::new("symbols"));
        assert_eq!(
            artifact.symbol_binary_relative_path,
            Path::new("symbols/nockchain-bench")
        );
        assert!(artifact
            .profiled_command
            .iter()
            .any(|arg| arg == "run-once"));
        assert!(output_root.join("cpu_profile.json").exists());
        assert!(output_root.join("profiles/samply-profile.json.gz").exists());
        assert!(output_root.join("symbols/nockchain-bench").exists());
        assert!(output_root.join("profile-run/result.json").exists());
    }

    #[tokio::test]
    async fn native_trusted_run_marks_verdict_invalid_when_cpu_profiling_fails() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();

        let error = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            FailingCpuProfilerLauncher,
            fake_native_profiler_request,
            write_cpu_profile_artifact,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
        )
        .await
        .expect_err("profiling failure should fail the case");

        assert!(error.to_string().contains("samply"));
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

    #[tokio::test]
    async fn native_trusted_run_marks_verdict_invalid_when_cpu_profile_request_build_fails() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();

        let error = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            FakeCpuProfilerLauncher::new(Arc::new(Mutex::new(Vec::new()))),
            |_output_root: &Path,
             _config: CpuProfilerConfig|
             -> Result<CpuProfilerLaunchRequest, HarnessError> {
                Err(HarnessError::CommandFailure(
                    "request build failed".to_string(),
                ))
            },
            write_cpu_profile_artifact,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
        )
        .await
        .expect_err("request build failure should fail the case");

        assert!(error.to_string().contains("request build failed"));
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

    #[tokio::test]
    async fn native_trusted_run_preflight_failure_rejects_stale_output_root() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        std::fs::create_dir_all(&output_root).expect("output root");
        std::fs::write(output_root.join("stale.txt"), "stale").expect("stale file");
        let backend = FakeNativeBackend::successful();
        let events = backend.shared_events();

        let error = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            PreflightFailingCpuProfilerLauncher,
            fake_native_profiler_request,
            write_cpu_profile_artifact,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
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
    async fn native_trusted_run_preflight_failure_stops_before_trusted_runs() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();
        let events = backend.shared_events();

        let error = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            PreflightFailingCpuProfilerLauncher,
            fake_native_profiler_request,
            write_cpu_profile_artifact,
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
        )
        .await
        .expect_err("preflight failure should fail the case before trusted runs");

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

    #[tokio::test]
    async fn native_trusted_run_marks_verdict_invalid_when_cpu_profile_artifact_write_fails() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeNativeBackend::successful();
        let events = backend.shared_events();
        let profiler = FakeCpuProfilerLauncher::new(events);

        let error = execute_native_trusted_run_with_backend_and_profiling_hooks(
            backend,
            profiler,
            fake_native_profiler_request,
            |_output_root: &Path, _artifact: &CpuProfileArtifact| -> Result<(), HarnessError> {
                Err(HarnessError::CommandFailure(
                    "persisting cpu profile artifact failed".to_string(),
                ))
            },
            requested,
            &output_root,
            false,
            Some(CpuProfilerConfig {
                kind: CpuProfilerKind::Samply,
                sample_rate_hz: 1_000,
            }),
        )
        .await
        .expect_err("artifact write failure should fail the case");

        assert!(error
            .to_string()
            .contains("persisting cpu profile artifact failed"));
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

    struct FakeNativeBackend {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl FakeNativeBackend {
        fn successful() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn shared_events(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.events)
        }
    }

    impl TrustedBackend for FakeNativeBackend {
        fn execute_run<'a>(
            &'a mut self,
            _resolved: &'a ResolvedCase,
            run_id: &'a str,
            run_dir: &'a Path,
        ) -> futures::future::BoxFuture<
            'a,
            Result<CompletedRun, crate::speed_of_light::harness::HarnessError>,
        > {
            self.events.lock().expect("events").push(run_id.to_string());
            let run_dir = run_dir.to_path_buf();
            async move {
                let completed = completed_run(run_id);
                write_run_artifacts(&run_dir, &completed).expect("run artifacts");
                Ok(completed)
            }
            .boxed()
        }

        fn prepare<'a>(
            &'a mut self,
            _resolved: &'a mut ResolvedCase,
            _output_root: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<(), crate::speed_of_light::harness::HarnessError>>
        {
            self.events
                .lock()
                .expect("events")
                .push("prepare".to_string());
            async { Ok(()) }.boxed()
        }

        fn capture_runtime_facts(
            &self,
        ) -> Result<BackendRuntimeFacts, crate::speed_of_light::harness::HarnessError> {
            self.events
                .lock()
                .expect("events")
                .push("runtime-facts".to_string());
            Ok(BackendRuntimeFacts::Native)
        }

        fn capture_raw_evidence<'a>(
            &'a self,
            _raw_dir: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<(), crate::speed_of_light::harness::HarnessError>>
        {
            self.events
                .lock()
                .expect("events")
                .push("raw-evidence".to_string());
            async { Ok(()) }.boxed()
        }

        fn cleanup<'a>(
            &'a mut self,
        ) -> futures::future::BoxFuture<'a, Result<(), crate::speed_of_light::harness::HarnessError>>
        {
            self.events
                .lock()
                .expect("events")
                .push("cleanup".to_string());
            async { Ok(()) }.boxed()
        }
    }

    struct FakeCpuProfilerLauncher {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl FakeCpuProfilerLauncher {
        fn new(events: Arc<Mutex<Vec<String>>>) -> Self {
            Self { events }
        }
    }

    impl CpuProfilerLauncher for FakeCpuProfilerLauncher {
        fn launch<'a>(
            &'a mut self,
            request: &'a CpuProfilerLaunchRequest,
        ) -> futures::future::BoxFuture<'a, Result<CpuProfileArtifact, HarnessError>> {
            self.events
                .lock()
                .expect("events")
                .push("profile".to_string());

            async move {
                let output_path = request.case_root.join(&request.output_relative_path);
                if let Some(parent) = output_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output_path, "profile")?;
                if let Some(parent) = request.symbol_binary_path().parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(request.symbol_binary_path(), "symbol-binary")?;

                write_run_artifacts(&request.profiled_run_dir, &completed_run("profile"))?;

                Ok(request.artifact())
            }
            .boxed()
        }
    }

    struct FailingCpuProfilerLauncher;

    impl CpuProfilerLauncher for FailingCpuProfilerLauncher {
        fn launch<'a>(
            &'a mut self,
            _request: &'a CpuProfilerLaunchRequest,
        ) -> futures::future::BoxFuture<'a, Result<CpuProfileArtifact, HarnessError>> {
            async {
                Err(HarnessError::CommandFailure(
                    "samply is not installed or not on PATH".to_string(),
                ))
            }
            .boxed()
        }
    }

    struct PreflightFailingCpuProfilerLauncher;

    impl CpuProfilerLauncher for PreflightFailingCpuProfilerLauncher {
        fn preflight<'a>(
            &'a self,
            _request: &'a CpuProfilerLaunchRequest,
        ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>> {
            async {
                Err(HarnessError::CommandFailure(
                    "preflight failed: samply is not installed".to_string(),
                ))
            }
            .boxed()
        }

        fn launch<'a>(
            &'a mut self,
            _request: &'a CpuProfilerLaunchRequest,
        ) -> futures::future::BoxFuture<'a, Result<CpuProfileArtifact, HarnessError>> {
            async { panic!("launch should not run after preflight failure") }.boxed()
        }
    }

    fn completed_run(run_id: &str) -> CompletedRun {
        CompletedRun {
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
            block_timings: vec![BlockTimingRecord {
                height: 2,
                duration_ms: 10.0,
            }],
            profile: None,
            bench_results: None,
        }
    }

    fn write_requested_case(root: &Path) -> RequestedCase {
        let mut requested = RequestedCase::native(PathBuf::new());
        requested.orchestrate = RequestedOrchestrate::PlanFile {
            plan_path: write_test_plan(root),
        };
        requested.warmup_runs = 1;
        requested.measured_runs = 3;
        requested.cooldown_secs = 0;
        requested
    }

    fn write_test_plan(root: &Path) -> PathBuf {
        let checkpoint_path = root.join("checkpoint.chkjam");
        let kernel_path = root.join("kernel.jam");
        write_checkpoint(&checkpoint_path, 0);
        std::fs::write(&kernel_path, [4, 5, 6]).expect("kernel");
        let plan_path = root.join("trusted-input-plan.json");
        let plan = serde_json::json!({
            "schema_version": crate::speed_of_light::ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION,
            "boot": checkpoint_boot(&checkpoint_path),
            "kernel": kernel_path,
            "steps": [{ "type": "peek_height", "height": 1 }]
        });
        std::fs::write(
            &plan_path,
            serde_json::to_vec_pretty(&plan).expect("plan json"),
        )
        .expect("plan");
        plan_path
    }

    fn fake_native_profiler_request(
        output_root: &Path,
        config: CpuProfilerConfig,
    ) -> Result<CpuProfilerLaunchRequest, HarnessError> {
        let resolved_case_path = output_root.join("resolved_case.json");
        let profile_run_dir = output_root.join("profile-run");

        Ok(CpuProfilerLaunchRequest {
            profiler_kind: config.kind,
            sample_rate_hz: config.sample_rate_hz,
            execution_kind: CpuProfileExecutionKind::Native,
            case_root: output_root.to_path_buf(),
            output_relative_path: cpu_profile_output_relative_path(config.kind),
            symbol_dir_relative_path: cpu_profile_symbol_dir_relative_path(),
            symbol_binary_relative_path: cpu_profile_symbol_binary_relative_path(),
            profiled_run_dir: profile_run_dir.clone(),
            profiled_command: build_run_once_command(
                "nockchain-bench",
                &path_string(&resolved_case_path),
                &path_string(&profile_run_dir),
                "profile",
            ),
        })
    }

    fn sorted_relative_paths(root: &Path) -> Vec<String> {
        fn visit(root: &Path, dir: &Path, entries: &mut Vec<String>) {
            let mut children: Vec<_> = std::fs::read_dir(dir)
                .expect("read dir")
                .map(|entry| entry.expect("entry").path())
                .collect();
            children.sort();
            for path in children {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative path")
                    .to_string_lossy()
                    .to_string();
                entries.push(relative);
                if path.is_dir() {
                    visit(root, &path, entries);
                }
            }
        }

        let mut entries = Vec::new();
        visit(root, root, &mut entries);
        entries
    }

    fn expected_trusted_artifact_tree() -> Vec<String> {
        vec![
            "provenance.json", "raw", "raw/host_env.json", "requested_case.json",
            "resolved_case.json", "runs", "runs/run-0", "runs/run-0/result.json",
            "runs/run-0/stderr.log", "runs/run-0/stdout.log", "runs/run-1",
            "runs/run-1/result.json", "runs/run-1/stderr.log", "runs/run-1/stdout.log",
            "runs/run-2", "runs/run-2/result.json", "runs/run-2/stderr.log",
            "runs/run-2/stdout.log", "runs/warmup-0", "runs/warmup-0/result.json",
            "runs/warmup-0/stderr.log", "runs/warmup-0/stdout.log", "schema_version.txt",
            "summary.json", "trusted_plan.json", "verdict.json",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn normalized_json(path: &Path) -> serde_json::Value {
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("json");

        if path.ends_with("resolved_case.json") || path.ends_with("provenance.json") {
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "fixture_sha256_hex".to_string(),
                    serde_json::Value::String("<normalized>".to_string()),
                );
                if let Some(binary) = object
                    .get_mut("binary")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    binary.insert(
                        "git_commit".to_string(),
                        serde_json::Value::String("<normalized>".to_string()),
                    );
                }

                if path.ends_with("provenance.json") {
                    object.insert(
                        "capture_timestamp_ms".to_string(),
                        serde_json::Value::String("<normalized>".to_string()),
                    );
                    object.insert(
                        "host".to_string(),
                        serde_json::Value::String("<normalized>".to_string()),
                    );
                    object.insert(
                        "git".to_string(),
                        serde_json::Value::String("<normalized>".to_string()),
                    );
                }
            }
        }

        value
    }

    fn uniform_stats_json(value: f64) -> serde_json::Value {
        serde_json::json!({
            "cv": 0.0,
            "mad": 0.0,
            "max": value,
            "median": value,
            "min": value,
            "stddev": 0.0,
            "values": [value, value, value]
        })
    }
}
