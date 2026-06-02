pub mod artifacts;
pub mod case;
pub mod docker;
pub mod docker_image;
pub mod execute;
pub mod native;
pub mod orchestrate;
pub mod profiler;
pub mod provenance;
pub mod summary;
pub mod sweep;
pub mod validate;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub use case::{
    current_binary_identity, default_fsync_enabled, fsync_mode_label, resolve_requested_case,
    BinaryIdentity, DockerResolvedConfig, ExecutionConfig, ExecutionRequest, RequestedCase,
    RequestedOrchestrate, ResolvedCase, ResolvedOrchestrate, WorkDirMode, DEFAULT_FSYNC_ENABLED,
};
pub use docker::{execute_docker_trusted_run, execute_docker_validation};
pub use docker_image::{DockerImageSource, DockerImageVariant, ResolvedDockerImage};
pub use execute::{
    cpu_profile_output_relative_path, execute_once, execute_once_with_options,
    execute_once_with_work_dir, BlockTimingRecord, CompletedRun, CpuProfileArtifact,
    CpuProfileExecutionKind, ExecuteOptions, RunRecord,
};
pub use native::{
    execute_native_cpu_profile, execute_native_cpu_profile_for_resolved_case,
    execute_native_trusted_run,
};
pub use orchestrate::{execute_trusted_run, TrustedBackend, TrustedRunResult};
pub use profiler::{
    build_samply_record_command, preflight_samply_profiler, run_samply_record_command,
    CpuProfilerLaunchRequest, CpuProfilerLauncher, ExternalCommand, SystemCpuProfilerLauncher,
};
pub use provenance::{
    capture_host_env, capture_native_provenance, BackendRuntimeFacts, GitIdentity, HostEnvSnapshot,
    HostIdentity, Provenance,
};
pub use summary::{
    evaluate_verdict, summarize_runs, RunFailure, RunMetrics, RunSummary, RunSummaryInput,
    StepTypeSummary, Validity, ValueStats, Verdict,
};
pub use sweep::{
    build_comparison, build_schedule, derive_sweep_verdict, execute_sweep, expand_matrix,
    parse_matrix_value, AxisValue, CpuProfilerConfig, CpuProfilerKind, ExpandedCase,
    HarnessSweepExecutor, ScheduleMode, SweepComparison, SweepExecutor, SweepMatrix,
    SweepMatrixFile, SweepResult, SweepRunOptions, SweepSchedule,
};
use thiserror::Error;
pub use validate::{
    evaluate_validation_probe, find_cached_validation, persist_validation_record,
    read_validation_cache, read_validation_record, run_validation_probe,
    upsert_validation_cache_record, validate_cached_or_run, validation_cache_path,
    ValidationCacheFile, ValidationCacheKey, ValidationProbeResult, ValidationRecord,
    ValidationStatus, VALIDATION_PROBE_VERSION,
};

pub const TRUSTED_OUTPUT_SCHEMA_VERSION: &str = "trusted-sol-orchestrate-output/v1";
pub const REQUESTED_CASE_SCHEMA_VERSION: &str = "requested-case/v2";
pub const RESOLVED_CASE_SCHEMA_VERSION: &str = "resolved-case/v2";
pub const PROVENANCE_SCHEMA_VERSION: &str = "provenance/v2";
pub const SUMMARY_SCHEMA_VERSION: &str = "summary/v1";
pub const VERDICT_SCHEMA_VERSION: &str = "verdict/v1";
pub const COMPARISON_SCHEMA_VERSION: &str = "comparison/v1";
pub const DEFAULT_THROUGHPUT_CV_THRESHOLD: f64 = 0.10;

pub(super) fn cgroup_v2_path_from_proc_cgroup(contents: &str) -> Option<PathBuf> {
    for line in contents.lines() {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next();
        let _controllers = parts.next();
        let path = parts.next();
        if hierarchy == Some("0") {
            let relative = path.unwrap_or_default().trim_start_matches('/');
            let root = PathBuf::from("/sys/fs/cgroup");
            return Some(if relative.is_empty() {
                root
            } else {
                root.join(relative)
            });
        }
    }
    None
}

pub(super) fn current_cgroup_v2_path() -> Option<PathBuf> {
    let contents = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    cgroup_v2_path_from_proc_cgroup(&contents)
}

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Fixture error: {0}")]
    Fixture(#[from] crate::speed_of_light::fixture::FixtureError),

    #[error("Bench error: {0}")]
    Bench(#[from] crate::speed_of_light::bench::BenchError),

    #[error("Docker error: {0}")]
    Docker(#[from] docker::HarnessDockerError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Command failure: {0}")]
    CommandFailure(String),

    #[error("{0}")]
    InvalidRequestedCase(String),
}

pub fn is_release_build() -> bool {
    !cfg!(debug_assertions)
}

pub fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn create_temp_dir(prefix: &str) -> Result<PathBuf, HarnessError> {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        unix_timestamp_ms()
    ));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

pub(super) fn read_trimmed_file(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub(super) fn parse_cgroup_numeric(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("max") || value.is_empty() {
        return Some(0);
    }
    value.parse::<u64>().ok()
}

#[cfg(test)]
mod cgroup_path_tests {
    use std::path::PathBuf;

    use super::cgroup_v2_path_from_proc_cgroup;

    #[test]
    fn cgroup_path_resolves_private_namespace_root() {
        assert_eq!(
            cgroup_v2_path_from_proc_cgroup("0::/\n"),
            Some(PathBuf::from("/sys/fs/cgroup"))
        );
    }

    #[test]
    fn cgroup_path_resolves_host_namespace_container_path() {
        assert_eq!(
            cgroup_v2_path_from_proc_cgroup("0::/docker/abc123\n"),
            Some(PathBuf::from("/sys/fs/cgroup/docker/abc123"))
        );
    }
}

#[cfg(test)]
mod phase4_sweep_tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::case::RequestedCase;
    use super::sweep::{build_schedule, expand_matrix, AxisValue, ScheduleMode, SweepMatrix};

    fn base_case() -> RequestedCase {
        RequestedCase::native(PathBuf::from("fixture.soltest"))
    }

    #[test]
    fn sweep_expands_single_axis_matrix_into_one_case_per_value() {
        let matrix = SweepMatrix {
            base_case: base_case(),
            axes: BTreeMap::from([(
                "threads".to_string(),
                vec![AxisValue::Integer(1), AxisValue::Integer(2), AxisValue::Integer(4)],
            )]),
        };

        let expanded = expand_matrix(&matrix).expect("expand matrix");

        assert_eq!(expanded.len(), 3);
        assert_eq!(expanded[0].case_id, "case-000-threads_1");
        assert_eq!(expanded[1].case_id, "case-001-threads_2");
        assert_eq!(expanded[2].case_id, "case-002-threads_4");
        assert_eq!(expanded[2].requested_case.threads, 4);
    }

    #[test]
    fn sweep_expands_multi_axis_matrix_into_cartesian_product() {
        let matrix = SweepMatrix {
            base_case: base_case(),
            axes: BTreeMap::from([
                (
                    "threads".to_string(),
                    vec![AxisValue::Integer(1), AxisValue::Integer(2)],
                ),
                (
                    "profile_memory".to_string(),
                    vec![AxisValue::Boolean(false), AxisValue::Boolean(true)],
                ),
            ]),
        };

        let expanded = expand_matrix(&matrix).expect("expand matrix");

        assert_eq!(expanded.len(), 4);
        assert_eq!(
            expanded
                .iter()
                .map(|case| case.case_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "case-000-profile_memory_false-threads_1",
                "case-001-profile_memory_false-threads_2",
                "case-002-profile_memory_true-threads_1", "case-003-profile_memory_true-threads_2",
            ]
        );
    }

    #[test]
    fn sweep_schedule_supports_sequential_interleaved_and_seeded_random_order() {
        let matrix = SweepMatrix {
            base_case: base_case(),
            axes: BTreeMap::from([
                (
                    "threads".to_string(),
                    vec![AxisValue::Integer(1), AxisValue::Integer(2)],
                ),
                (
                    "profile_memory".to_string(),
                    vec![AxisValue::Boolean(false), AxisValue::Boolean(true)],
                ),
            ]),
        };

        let expanded = expand_matrix(&matrix).expect("expand matrix");

        let sequential =
            build_schedule(&expanded, ScheduleMode::Sequential, None).expect("sequential schedule");
        let interleaved = build_schedule(&expanded, ScheduleMode::Interleaved, None)
            .expect("interleaved schedule");
        let randomized = build_schedule(&expanded, ScheduleMode::Randomized, Some(7))
            .expect("randomized schedule");
        let randomized_again = build_schedule(&expanded, ScheduleMode::Randomized, Some(7))
            .expect("second randomized schedule");

        assert_eq!(
            sequential.case_ids,
            vec![
                "case-000-profile_memory_false-threads_1",
                "case-001-profile_memory_false-threads_2",
                "case-002-profile_memory_true-threads_1", "case-003-profile_memory_true-threads_2",
            ]
        );
        assert_eq!(
            interleaved.case_ids,
            vec![
                "case-000-profile_memory_false-threads_1",
                "case-002-profile_memory_true-threads_1",
                "case-001-profile_memory_false-threads_2",
                "case-003-profile_memory_true-threads_2",
            ]
        );
        assert_eq!(randomized.case_ids, randomized_again.case_ids);
        assert_ne!(randomized.case_ids, sequential.case_ids);
        let mut randomized_sorted = randomized.case_ids.clone();
        randomized_sorted.sort();
        let mut sequential_sorted = sequential.case_ids.clone();
        sequential_sorted.sort();
        assert_eq!(randomized_sorted, sequential_sorted);
    }
}
