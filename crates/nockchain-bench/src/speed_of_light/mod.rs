//! Speed-of-Light Benchmark Module
//!
//! Extracts blockchain data from a checkpoint to measure maximum possible
//! throughput when not limited by network latency.
//!
//! # Overview
//!
//! The "speed of light" benchmark measures how fast we can poke blocks into
//! the serf when blocks are pre-fetched and ready, eliminating network overhead.
//!
//! This module provides:
//! - Checkpoint loading and cue'ing
//! - Archive extraction via checkpoint peek
//! - Archive format for persisting extracted data to disk
//! - Benchmark runner for injection testing

pub mod archive;
pub mod bench;
pub mod boot_source;
pub mod checkpoint;
pub mod checkpoint_builder;
pub mod cold_peek;
pub mod extractor;
pub mod final_tip;
pub mod fixture;
pub mod harness;
pub mod kernel_utils;
pub mod mempool_inspector;
mod noun_compat;
pub mod orchestrate_execute;
pub mod orchestrate_plan;
pub mod orchestrator;
pub mod peek_bench;
mod pma_replay;
pub mod poke;
pub mod profiling;
pub mod replay_window;
pub mod start_height;
pub mod types;

pub use archive::{
    slice_archive_file, ArchiveFilter, ArchiveInspect, ArchiveMetadata, ArchiveSliceResult,
    BlockEntry, ByteOffset, ByteSize, MempoolSnapshotEntry, MempoolTxEntry, RawTxEntry,
    RawTxPayload, SolArchiveReader, SolArchiveWriter,
};
pub use bench::{SolBenchConfig, SolBenchResults, SolBenchRunner};
pub use boot_source::{
    BootSourceError, BootSourceFileRole, BootSourceInput, BootSourceKind, ResolvedBootSource,
    TrustedBootSource, TrustedBootSourceFile,
};
pub use checkpoint::{checkpoint_event_num, load_checkpoint};
pub use checkpoint_builder::{
    CheckpointBuildError, CheckpointBuildMode, CheckpointBuilder, CheckpointConfig,
    CheckpointResult,
};
pub use extractor::{
    ArchiveExtractionPhase, ArchiveExtractionProgress, BlockExtractor, ExtractorConfig,
};
pub use final_tip::{validate_final_tip, ExpectedFinalTip, FinalTipValidation, ObservedFinalTip};
pub use fixture::{
    extract_fixture_to_paths, read_fixture_file, write_fixture_file, write_fixture_file_from_paths,
    FixtureError, SolFixtureCheckpointKind, SolFixtureFile, SolFixtureManifest,
};
pub use harness::docker::{
    connect_docker, execute_docker_validation, parse_memory_limit, parse_proc_stat_faults,
    ContainerStats, DockerRunPlan, HarnessDockerError,
};
pub use harness::{
    capture_native_provenance, cpu_profile_output_relative_path, current_binary_identity,
    default_fsync_enabled, evaluate_validation_probe, evaluate_verdict, execute_docker_trusted_run,
    execute_native_cpu_profile, execute_native_cpu_profile_for_resolved_case,
    execute_native_trusted_run, execute_once, execute_once_with_options,
    execute_once_with_work_dir, execute_sweep, expand_matrix, fsync_mode_label, parse_matrix_value,
    resolve_requested_case, run_validation_probe, AxisValue, CpuProfileArtifact,
    CpuProfileExecutionKind, CpuProfilerConfig, CpuProfilerKind, DockerImageSource,
    DockerImageVariant, DockerResolvedConfig, ExecuteOptions, ExecutionConfig, ExecutionRequest,
    ExpandedCase, HarnessSweepExecutor, RequestedCase, ResolvedCase, ResolvedDockerImage,
    RunFailure, RunMetrics, RunSummary, RunSummaryInput, ScheduleMode, SweepComparison,
    SweepMatrix, SweepMatrixFile, SweepResult, SweepRunOptions, SweepSchedule, Validity,
    ValueStats, Verdict, WorkDirMode, DEFAULT_FSYNC_ENABLED,
};
pub use mempool_inspector::{find_stale_ranges, InspectorError, StaleTxRange};
pub use orchestrate_execute::{
    build_run_record_from_measurements, build_run_record_from_measurements_with_policy,
    execute_trusted_plan_once, is_allowed_degraded_cold_reason, write_run_artifacts,
    ColdEvidenceRow, FinalTip, OrchestrateExecuteError, RunCounts, RunRecord, RunThroughput,
    RunTiming, StepOutcomeKind, StepResultRow, SyntheticStepMeasurement,
    COLD_EVIDENCE_SCHEMA_VERSION, RUN_RESULT_SCHEMA_VERSION, STEP_RESULT_SCHEMA_VERSION,
};
pub use orchestrate_plan::{
    build_generated_read_plan, build_generated_replay_plan, load_plan_input, normalize_plan,
    refresh_plan_hashes, step_signature_bytes, ColdTarget, GeneratedReadOptions, GeneratedReadPlan,
    GeneratedReplayOptions, GeneratedReplayPlan, InputRole, OrchestratePlanError,
    OrchestratePlanInput, PeekMode, PlanStepInput, ReadRangeResolution, ResolvedInput, TrustedPlan,
    TrustedPlanBoot, TrustedStep, ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION,
    TRUSTED_PLAN_SCHEMA_VERSION,
};
pub use orchestrator::{
    ColdMode, QuickOrchestratePlan, QuickOrchestrateResults, QuickOrchestrateRunner,
};
pub use peek_bench::{
    LatencySummaryUs, PeekBenchConfig, PeekBenchError, PeekBenchResults, PeekBenchRunner,
    PeekRangeRequest,
};
pub use profiling::{
    build_scorecard, find_recovery_ms, infer_gc_events, infer_page_fault_bursts, summarize_phases,
    CheckpointProfile, GcEvent, MemoryProfile, PageFaultBurst, PhaseKind, PhaseSummary,
    PhaseWindow, ProcessMemoryProfiler, SolScorecard,
};
pub use replay_window::{
    select_replay_window, ReplayWindow, ReplayWindowOptions, SelectedReplayBlock,
};
pub use start_height::{resolve_start_height, StartHeightError};
pub use types::{ProofVersion, SolHeight, PROOF_VERSION_1_START, PROOF_VERSION_2_START};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::harness::{
        evaluate_verdict, ExecutionRequest, RequestedCase, RunFailure, RunSummaryInput, Validity,
    };

    fn summary_input(
        release_build: bool,
        allow_debug_benchmark: bool,
        throughput_cv: Option<f64>,
        run_failures: Vec<RunFailure>,
    ) -> RunSummaryInput {
        RunSummaryInput {
            measured_run_count: 5,
            run_failures,
            throughput_cv,
            cv_threshold: super::harness::DEFAULT_THROUGHPUT_CV_THRESHOLD,
            release_build,
            allow_debug_benchmark,
            allow_version_skew: false,
            allow_degraded_cold: false,
            invalid_reasons: Vec::new(),
            partial_reasons: Vec::new(),
        }
    }

    #[test]
    fn harness_summary_uses_phase1_defaults() {
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        assert_eq!(requested.execution, ExecutionRequest::Native);
        assert_eq!(requested.warmup_runs, 1);
        assert_eq!(requested.measured_runs, 5);
        assert_eq!(requested.cooldown_secs, 10);
    }

    #[test]
    fn harness_requested_case_stays_spec_authoritative() {
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        let value = serde_json::to_value(&requested).expect("serialize requested case");
        let object = value.as_object().expect("requested case object");

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = vec![
            "allow_debug_benchmark", "allow_degraded_cold", "allow_version_skew", "benchmark",
            "cooldown_secs", "cv_threshold", "execution", "label", "measured_runs", "orchestrate",
            "fsync", "profile_interval_ms", "profile_memory", "schema_version", "threads",
            "warmup_runs",
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);

        assert_eq!(
            object.get("execution"),
            Some(&Value::String("Native".to_string()))
        );
    }

    #[test]
    fn harness_summary_marks_failed_measured_runs_partial() {
        let verdict = evaluate_verdict(&summary_input(
            true,
            false,
            Some(0.02),
            vec![RunFailure {
                run_id: "run-2".to_string(),
                reason: "poke failed".to_string(),
            }],
        ));

        match verdict.validity {
            Validity::Partial { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("run-2")));
            }
            other => panic!("expected partial verdict, got {other:?}"),
        }
    }

    #[test]
    fn harness_summary_marks_high_cv_partial() {
        let verdict = evaluate_verdict(&summary_input(true, false, Some(0.25), Vec::new()));

        match verdict.validity {
            Validity::Partial { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("throughput CV")));
            }
            other => panic!("expected partial verdict, got {other:?}"),
        }
    }

    #[test]
    fn harness_summary_rejects_debug_trusted_runs_by_default() {
        let verdict = evaluate_verdict(&summary_input(false, false, Some(0.02), Vec::new()));

        match verdict.validity {
            Validity::Invalid { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("release")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
    }

    #[test]
    fn harness_summary_rejects_debug_override_as_partial() {
        let verdict = evaluate_verdict(&summary_input(false, true, Some(0.02), Vec::new()));

        match verdict.validity {
            Validity::Partial { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("debug build")));
            }
            other => panic!("expected partial verdict, got {other:?}"),
        }
    }
}
