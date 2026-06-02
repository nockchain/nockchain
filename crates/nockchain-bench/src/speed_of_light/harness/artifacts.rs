use std::io::Write;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::case::{RequestedCase, ResolvedCase};
use super::docker::ContainerStats;
use super::execute::{CompletedRun, CpuProfileArtifact, RunRecord};
use super::provenance::{HostEnvSnapshot, Provenance};
use super::summary::{RunSummary, Verdict};
use super::validate::ValidationRecord;
use super::{HarnessError, TRUSTED_OUTPUT_SCHEMA_VERSION};

pub fn write_schema_version(root: &Path) -> Result<(), HarnessError> {
    std::fs::create_dir_all(root)?;
    std::fs::write(
        root.join("schema_version.txt"),
        format!("{TRUSTED_OUTPUT_SCHEMA_VERSION}\n"),
    )?;
    Ok(())
}

pub fn write_requested_case(root: &Path, requested: &RequestedCase) -> Result<(), HarnessError> {
    write_json(root.join("requested_case.json"), requested)
}

pub fn write_resolved_case(root: &Path, resolved: &ResolvedCase) -> Result<(), HarnessError> {
    write_json(root.join("resolved_case.json"), resolved)
}

pub fn write_provenance(root: &Path, provenance: &Provenance) -> Result<(), HarnessError> {
    write_json(root.join("provenance.json"), provenance)
}

pub fn write_host_env(root: &Path, host_env: &HostEnvSnapshot) -> Result<(), HarnessError> {
    let raw_dir = root.join("raw");
    std::fs::create_dir_all(&raw_dir)?;
    write_json(raw_dir.join("host_env.json"), host_env)
}

pub fn write_summary(root: &Path, summary: &RunSummary) -> Result<(), HarnessError> {
    write_json(root.join("summary.json"), summary)
}

pub fn write_verdict(root: &Path, verdict: &Verdict) -> Result<(), HarnessError> {
    write_json(root.join("verdict.json"), verdict)
}

pub fn write_validation(root: &Path, validation: &ValidationRecord) -> Result<(), HarnessError> {
    write_json(root.join("validation.json"), validation)
}

pub fn write_cpu_profile_artifact(
    case_root: &Path,
    artifact: &CpuProfileArtifact,
) -> Result<(), HarnessError> {
    std::fs::create_dir_all(case_root)?;
    write_json(case_root.join("cpu_profile.json"), artifact)
}

pub fn read_cpu_profile_artifact(case_root: &Path) -> Result<CpuProfileArtifact, HarnessError> {
    read_json(case_root.join("cpu_profile.json"))
}

pub fn write_container_samples(
    run_dir: &Path,
    samples: &[ContainerStats],
) -> Result<(), HarnessError> {
    std::fs::create_dir_all(run_dir)?;
    write_ndjson(run_dir.join("container_samples.ndjson"), samples)
}

pub fn write_run_artifacts(run_dir: &Path, run: &CompletedRun) -> Result<(), HarnessError> {
    std::fs::create_dir_all(run_dir)?;
    if let Some(record) = &run.trusted_orchestrate_record {
        write_json(run_dir.join("result.json"), record)?;
    } else {
        write_json(run_dir.join("result.json"), &run.record)?;
    }

    if let Some(profile) = &run.profile {
        write_json(run_dir.join("profile.json"), profile)?;
    }

    std::fs::write(run_dir.join("stdout.log"), "")?;
    let stderr = run
        .trusted_orchestrate_record
        .as_ref()
        .and_then(|record| record.error.as_deref())
        .or(run.record.error.as_deref())
        .map(|error| format!("{error}\n"))
        .unwrap_or_default();
    std::fs::write(run_dir.join("stderr.log"), stderr)?;

    Ok(())
}

pub fn read_run_artifacts(run_dir: &Path) -> Result<CompletedRun, HarnessError> {
    let result_path = run_dir.join("result.json");
    let result_value: serde_json::Value = read_json(&result_path)?;
    let trusted_orchestrate_record = if result_value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(crate::speed_of_light::RUN_RESULT_SCHEMA_VERSION)
    {
        Some(serde_json::from_value::<
            crate::speed_of_light::orchestrate_execute::RunRecord,
        >(result_value)?)
    } else {
        None
    };
    let mut record = if let Some(trusted) = &trusted_orchestrate_record {
        RunRecord {
            run_id: trusted.run_id.clone(),
            success: trusted.success,
            error: trusted.error.clone(),
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
        }
    } else {
        read_json(run_dir.join("result.json"))?
    };
    let profile = if run_dir.join("profile.json").exists() {
        Some(read_json(run_dir.join("profile.json"))?)
    } else {
        trusted_orchestrate_record
            .as_ref()
            .and_then(|record| record.memory_profile.clone())
    };
    if let Some(profile) = &profile {
        record.peak_process_rss_bytes = profile
            .samples
            .iter()
            .map(|sample| sample.vm_rss_kb.saturating_mul(1024) as f64)
            .max_by(|left, right| left.total_cmp(right));
        record.minor_faults_total = total_fault_delta(
            profile.samples.first().map(|sample| sample.minor_faults),
            profile.samples.last().map(|sample| sample.minor_faults),
        );
        record.major_faults_total = total_fault_delta(
            profile.samples.first().map(|sample| sample.major_faults),
            profile.samples.last().map(|sample| sample.major_faults),
        );
    }

    let block_timings = Vec::new();

    let invalid_reasons = trusted_invalid_reasons(&record, trusted_orchestrate_record.as_ref());

    Ok(CompletedRun {
        record,
        trusted_orchestrate_record,
        invalid_reasons,
        block_timings,
        profile,
        bench_results: None,
    })
}

fn total_fault_delta(first: Option<u64>, last: Option<u64>) -> Option<f64> {
    Some(last?.saturating_sub(first?) as f64)
}

fn trusted_invalid_reasons(
    record: &RunRecord,
    trusted: Option<&crate::speed_of_light::orchestrate_execute::RunRecord>,
) -> Vec<String> {
    if let Some(trusted) = trusted {
        return trusted.invalid_reasons.clone();
    }
    record
        .final_tip_validation
        .as_ref()
        .and_then(|validation| validation.invalid_reason.clone())
        .into_iter()
        .collect()
}

pub(super) fn write_json<T: Serialize>(
    path: impl AsRef<Path>,
    value: &T,
) -> Result<(), HarnessError> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub(super) fn write_ndjson<T: Serialize>(
    path: impl AsRef<Path>,
    values: &[T],
) -> Result<(), HarnessError> {
    let mut output = std::fs::File::create(path.as_ref())?;
    for value in values {
        serde_json::to_writer(&mut output, value)?;
        output.write_all(b"\n")?;
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T, HarnessError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::sampler::buckets::MemoryAttribution;
    use crate::speed_of_light::fixture::SolFixtureManifest;
    use crate::speed_of_light::harness::case::{
        BinaryIdentity, ExecutionConfig, ResolvedOrchestrate,
    };
    use crate::speed_of_light::harness::execute::{
        cpu_profile_output_relative_path, BlockTimingRecord, CpuProfileArtifact,
        CpuProfileExecutionKind, RunRecord,
    };
    use crate::speed_of_light::harness::provenance::BackendRuntimeFacts;
    use crate::speed_of_light::harness::summary::Validity;
    use crate::speed_of_light::harness::validate::{
        ValidationCacheKey, ValidationRecord, ValidationStatus, VALIDATION_PROBE_VERSION,
    };
    use crate::speed_of_light::harness::{
        CpuProfilerKind, PROVENANCE_SCHEMA_VERSION, RESOLVED_CASE_SCHEMA_VERSION,
        SUMMARY_SCHEMA_VERSION, VERDICT_SCHEMA_VERSION,
    };
    use crate::speed_of_light::profiling::{
        MemoryProfile, PhaseKind, PhaseSummary, PhaseWindow, SolScorecard,
    };
    use crate::speed_of_light::types::SolHeight;

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
            host: super::super::provenance::HostIdentity {
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

    fn valid_validation_record() -> ValidationRecord {
        ValidationRecord {
            key: ValidationCacheKey {
                docker_engine_version: "28.0.1".to_string(),
                cgroup_version: "2".to_string(),
                image_digest: "sha256:abc".to_string(),
                memory_limit: "8g".to_string(),
                cpuset: Some("0-3".to_string()),
                cpu_quota: Some(200_000),
                cpu_period: Some(100_000),
                work_dir_mode: super::super::case::WorkDirMode::DockerTmpfs,
                probe_version: VALIDATION_PROBE_VERSION.to_string(),
            },
            status: ValidationStatus::Valid,
            from_cache: false,
            observed_probe_version: Some(VALIDATION_PROBE_VERSION.to_string()),
            probe_version_matches: Some(true),
            container_started: true,
            docker_reports_cgroup_v2: true,
            memory_max_readable: true,
            memory_current_readable: true,
            memory_limit_matches: true,
            allocation_sanity: true,
            realized_memory_max_bytes: Some(8 * 1024 * 1024 * 1024),
            allocation_request_bytes: Some(64 * 1024 * 1024),
            memory_current_before_bytes: Some(1_000),
            memory_current_peak_bytes: Some(65 * 1024 * 1024),
            memory_current_after_bytes: Some(2_000),
            recorded_cpu_max: Some("200000 100000".to_string()),
            recorded_cpuset: Some("0-3".to_string()),
            failure_reason: None,
        }
    }

    fn sorted_object_keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys = value
            .as_object()
            .expect("json object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    fn sample(ts: u64, rss_kb: u64, minor: u64, major: u64) -> MemoryAttribution {
        MemoryAttribution {
            timestamp_ms: ts,
            vm_rss_kb: rss_kb,
            vm_size_kb: rss_kb * 2,
            minor_faults: minor,
            major_faults: major,
            ..Default::default()
        }
    }

    fn memory_profile() -> MemoryProfile {
        MemoryProfile {
            interval_ms: 500,
            samples: vec![sample(0, 100, 10, 1), sample(500, 150, 25, 3)],
            phase_windows: vec![PhaseWindow::new(PhaseKind::Replay, 0, 500)],
            phase_summaries: vec![PhaseSummary {
                kind: PhaseKind::Replay,
                start_ms: 0,
                end_ms: 500,
                duration_ms: 500,
                sample_count: 2,
                peak_rss_bytes: 150 * 1024,
                avg_rss_bytes: 125 * 1024,
                peak_vm_size_bytes: 300 * 1024,
                avg_vm_size_bytes: 250 * 1024,
                minor_faults_delta: 15,
                major_faults_delta: 2,
            }],
            checkpoint_profiles: Vec::new(),
            gc_events: Vec::new(),
            page_fault_bursts: Vec::new(),
            scorecard: SolScorecard {
                peak_rss_mib: 150.0 / 1024.0,
                p95_rss_mib: 150.0 / 1024.0,
                checkpoint_peak_rss_mib: None,
                checkpoint_seconds_per_gib: None,
                gc_pause_p95_ms: None,
                gc_events_per_1k_blocks: 0.0,
                page_fault_burst_count: 0,
                blocks_per_second: 1.0,
                failed_pokes: 0,
            },
        }
    }

    #[test]
    fn harness_artifacts_write_expected_run_files() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let completed = CompletedRun {
            record: RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 10,
                failed_pokes: 0,
                init_time_secs: 1.0,
                total_replay_time_secs: 2.0,
                throughput_blocks_per_second: 5.0,
                average_block_time_ms: 200.0,
                peak_process_rss_bytes: Some(123.0),
                minor_faults_total: Some(10.0),
                major_faults_total: Some(1.0),
                final_tip_validation: None,
            },
            trusted_orchestrate_record: None,
            invalid_reasons: Vec::new(),
            block_timings: vec![BlockTimingRecord {
                height: 42,
                duration_ms: 10.0,
            }],
            profile: None,
            bench_results: None,
        };

        write_run_artifacts(&run_dir, &completed).expect("write artifacts");

        assert!(run_dir.join("result.json").exists());
        assert!(!run_dir.join("block_timings.ndjson").exists());
        assert!(run_dir.join("stdout.log").exists());
        assert!(run_dir.join("stderr.log").exists());
    }

    #[test]
    fn harness_artifacts_omits_empty_block_timings() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let completed = CompletedRun {
            record: RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
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
        };

        write_run_artifacts(&run_dir, &completed).expect("write artifacts");

        assert!(!run_dir.join("block_timings.ndjson").exists());
    }

    #[test]
    fn trusted_run_artifacts_write_run_result_schema_and_no_block_timings() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let profile = memory_profile();
        let trusted_record = crate::speed_of_light::orchestrate_execute::RunRecord {
            schema_version: crate::speed_of_light::RUN_RESULT_SCHEMA_VERSION.to_string(),
            benchmark: "sol-orchestrate".to_string(),
            run_id: "run-0".to_string(),
            success: true,
            error: None,
            boot: crate::speed_of_light::orchestrate_execute::RunBoot {
                source: crate::speed_of_light::TrustedBootSource::Checkpoint {
                    checkpoint_input_id: "checkpoint-0".to_string(),
                    event_num: Some(0),
                },
                kernel_input_id: "kernel-0".to_string(),
                fsync: true,
                init_time_secs: Some(0.25),
            },
            steps_planned: 1,
            steps_executed: 1,
            cold: crate::speed_of_light::orchestrate_execute::RunColdCounts::default(),
            counts: crate::speed_of_light::orchestrate_execute::RunCounts {
                poke_archive_block: 1,
                ..Default::default()
            },
            timing: crate::speed_of_light::orchestrate_execute::RunTiming {
                total_step_time_secs: 1.0,
                total_poke_time_secs: 1.0,
                ..Default::default()
            },
            throughput: crate::speed_of_light::orchestrate_execute::RunThroughput {
                steps_per_second: Some(1.0),
                pokes_per_second: Some(1.0),
                ..Default::default()
            },
            expected_final_tip: None,
            final_tip: None,
            final_tip_validation: None,
            invalid_reasons: Vec::new(),
            failed_step_index: None,
            memory_profile: Some(profile.clone()),
        };
        let completed = CompletedRun {
            record: RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 1,
                failed_pokes: 0,
                init_time_secs: 0.0,
                total_replay_time_secs: 1.0,
                throughput_blocks_per_second: 1.0,
                average_block_time_ms: 0.0,
                peak_process_rss_bytes: None,
                minor_faults_total: None,
                major_faults_total: None,
                final_tip_validation: None,
            },
            trusted_orchestrate_record: Some(trusted_record.clone()),
            invalid_reasons: Vec::new(),
            block_timings: vec![BlockTimingRecord {
                height: 42,
                duration_ms: 10.0,
            }],
            profile: Some(profile),
            bench_results: None,
        };

        write_run_artifacts(&run_dir, &completed).expect("write artifacts");

        let result: serde_json::Value = read_json(run_dir.join("result.json")).expect("result");
        assert_eq!(
            result
                .get("schema_version")
                .and_then(serde_json::Value::as_str),
            Some(crate::speed_of_light::RUN_RESULT_SCHEMA_VERSION)
        );
        assert!(result.get("blocks_poked").is_none());
        assert!(result.get("memory_profile").is_some());
        assert!(run_dir.join("profile.json").exists());
        assert!(!run_dir.join("block_timings.ndjson").exists());

        let loaded = read_run_artifacts(&run_dir).expect("read artifacts");
        assert_eq!(loaded.trusted_orchestrate_record, Some(trusted_record));
        assert!(loaded.profile.is_some());
        assert_eq!(loaded.record.peak_process_rss_bytes, Some(150.0 * 1024.0));
        assert_eq!(loaded.record.minor_faults_total, Some(15.0));
        assert_eq!(loaded.record.major_faults_total, Some(2.0));
        assert!(loaded.block_timings.is_empty());
    }

    #[test]
    fn harness_artifacts_read_completed_run_round_trips_files() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let completed = CompletedRun {
            record: RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 10,
                failed_pokes: 0,
                init_time_secs: 1.0,
                total_replay_time_secs: 2.0,
                throughput_blocks_per_second: 5.0,
                average_block_time_ms: 200.0,
                peak_process_rss_bytes: Some(123.0),
                minor_faults_total: Some(10.0),
                major_faults_total: Some(1.0),
                final_tip_validation: None,
            },
            trusted_orchestrate_record: None,
            invalid_reasons: Vec::new(),
            block_timings: vec![BlockTimingRecord {
                height: 42,
                duration_ms: 10.0,
            }],
            profile: None,
            bench_results: None,
        };

        write_run_artifacts(&run_dir, &completed).expect("write artifacts");
        let loaded = read_run_artifacts(&run_dir).expect("read artifacts");

        assert_eq!(loaded.record, completed.record);
        assert!(loaded.block_timings.is_empty());
        assert!(loaded.profile.is_none());
        assert!(loaded.bench_results.is_none());
    }

    #[test]
    fn cpu_profile_artifact() {
        let tempdir = tempdir().expect("tempdir");

        let native_case_root = tempdir.path().join("cases/case-000-native");
        let docker_case_root = tempdir.path().join("cases/case-001-docker");
        let profile_output_relative_path =
            cpu_profile_output_relative_path(CpuProfilerKind::Samply);

        let base_command = vec![
            "samply".to_string(),
            "record".to_string(),
            "--save-only".to_string(),
            "-o".to_string(),
            profile_output_relative_path.to_string_lossy().to_string(),
            "--".to_string(),
            "nockchain-bench".to_string(),
            "sol".to_string(),
            "run-once".to_string(),
            "--resolved-case".to_string(),
            "/bench/input/resolved_case.json".to_string(),
            "--run-dir".to_string(),
            "/bench/output/profile-run".to_string(),
            "--run-id".to_string(),
            "profile".to_string(),
        ];

        let native_artifact = CpuProfileArtifact {
            profiler_kind: CpuProfilerKind::Samply,
            sample_rate_hz: 1_000,
            execution_kind: CpuProfileExecutionKind::Native,
            profiled_command: base_command.clone(),
            output_relative_path: profile_output_relative_path.clone(),
            symbol_dir_relative_path: std::path::PathBuf::from("symbols"),
            symbol_binary_relative_path: std::path::PathBuf::from("symbols/nockchain-bench"),
        };
        let docker_artifact = CpuProfileArtifact {
            profiler_kind: CpuProfilerKind::Samply,
            sample_rate_hz: 1_000,
            execution_kind: CpuProfileExecutionKind::DockerInContainer,
            profiled_command: base_command,
            output_relative_path: profile_output_relative_path.clone(),
            symbol_dir_relative_path: std::path::PathBuf::from("symbols"),
            symbol_binary_relative_path: std::path::PathBuf::from("symbols/nockchain-bench"),
        };

        write_cpu_profile_artifact(&native_case_root, &native_artifact)
            .expect("write native cpu profile artifact");
        write_cpu_profile_artifact(&docker_case_root, &docker_artifact)
            .expect("write docker cpu profile artifact");

        let loaded_native =
            read_cpu_profile_artifact(&native_case_root).expect("read native cpu profile artifact");
        let loaded_docker =
            read_cpu_profile_artifact(&docker_case_root).expect("read docker cpu profile artifact");

        assert_eq!(loaded_native, native_artifact);
        assert_eq!(loaded_docker, docker_artifact);
        assert_eq!(
            profile_output_relative_path,
            std::path::PathBuf::from("profiles/samply-profile.json.gz")
        );
        assert!(native_case_root.join("cpu_profile.json").exists());
        assert!(docker_case_root.join("cpu_profile.json").exists());
        assert_eq!(
            loaded_native.output_relative_path,
            std::path::PathBuf::from("profiles/samply-profile.json.gz")
        );
        assert_eq!(
            loaded_docker.output_relative_path,
            std::path::PathBuf::from("profiles/samply-profile.json.gz")
        );
        assert_eq!(
            loaded_docker.symbol_dir_relative_path,
            std::path::PathBuf::from("symbols")
        );
        assert_eq!(
            loaded_docker.symbol_binary_relative_path,
            std::path::PathBuf::from("symbols/nockchain-bench")
        );
    }

    #[test]
    fn harness_artifacts_write_container_samples_ndjson() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        let samples = vec![
            super::super::docker::ContainerStats {
                timestamp_ms: 10,
                memory_usage_bytes: 20,
                memory_limit_bytes: 40,
                memory_percent: 50.0,
                memory_cache_bytes: 5,
                memory_rss_bytes: 15,
                cpu_percent: 75.0,
                minor_faults: Some(3),
                major_faults: Some(1),
            },
            super::super::docker::ContainerStats {
                timestamp_ms: 11,
                memory_usage_bytes: 25,
                memory_limit_bytes: 40,
                memory_percent: 62.5,
                memory_cache_bytes: 6,
                memory_rss_bytes: 19,
                cpu_percent: 70.0,
                minor_faults: Some(4),
                major_faults: Some(1),
            },
        ];

        write_container_samples(&run_dir, &samples).expect("write container samples");

        let payload = std::fs::read_to_string(run_dir.join("container_samples.ndjson"))
            .expect("container samples file");
        let lines: Vec<_> = payload.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: super::super::docker::ContainerStats =
            serde_json::from_str(lines[0]).expect("first sample json");
        let second: super::super::docker::ContainerStats =
            serde_json::from_str(lines[1]).expect("second sample json");
        assert_eq!(first.timestamp_ms, 10);
        assert_eq!(second.timestamp_ms, 11);
    }

    #[test]
    fn harness_artifacts_write_root_files() {
        let tempdir = tempdir().expect("tempdir");
        let root = tempdir.path();
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        let resolved = resolved_native_case(requested.clone());
        let provenance = native_provenance(&resolved);
        let host_env = HostEnvSnapshot {
            current_dir: Some(PathBuf::from("/tmp")),
            shell: Some("/bin/zsh".to_string()),
            user: Some("tester".to_string()),
            hostname_env: Some("host".to_string()),
            rust_log: None,
        };
        let summary = RunSummary {
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
        };
        let verdict = Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: false,
            allow_version_skew: false,
            allow_degraded_cold: false,
            cv_threshold: 0.10,
            validity: Validity::Valid,
        };

        write_schema_version(root).expect("schema version");
        write_requested_case(root, &requested).expect("requested");
        write_resolved_case(root, &resolved).expect("resolved");
        let resolved_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("resolved_case.json")).expect("read"))
                .expect("resolved json");
        let resolved_object = resolved_json.as_object().expect("resolved case object");
        let execution_config = resolved_object
            .get("execution_config")
            .and_then(serde_json::Value::as_object)
            .expect("execution_config object");
        assert_eq!(
            execution_config.get("gc_drop_threshold_mib"),
            Some(&serde_json::Value::from(64))
        );
        write_provenance(root, &provenance).expect("provenance");
        write_host_env(root, &host_env).expect("host env");
        write_summary(root, &summary).expect("summary");
        write_verdict(root, &verdict).expect("verdict");
        write_validation(root, &valid_validation_record()).expect("validation");

        assert!(root.join("schema_version.txt").exists());
        assert!(root.join("requested_case.json").exists());
        assert!(root.join("resolved_case.json").exists());
        assert!(root.join("provenance.json").exists());
        assert!(root.join("raw/host_env.json").exists());
        assert!(root.join("validation.json").exists());
        assert!(root.join("summary.json").exists());
        assert!(root.join("verdict.json").exists());

        let provenance_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("provenance.json")).expect("read"))
                .expect("provenance json");
        assert_eq!(
            sorted_object_keys(&provenance_json),
            vec![
                "allow_debug_benchmark", "allow_degraded_cold", "allow_version_skew", "backend",
                "binary", "capture_timestamp_ms", "cv_threshold", "fixture_manifest",
                "fixture_path", "fixture_sha256_hex", "git", "host", "schema_version",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );

        let validation_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("validation.json")).expect("read"))
                .expect("validation json");
        assert_eq!(
            sorted_object_keys(&validation_json),
            vec![
                "allocation_request_bytes", "allocation_sanity", "container_started",
                "docker_reports_cgroup_v2", "failure_reason", "from_cache", "key",
                "memory_current_after_bytes", "memory_current_before_bytes",
                "memory_current_peak_bytes", "memory_current_readable", "memory_limit_matches",
                "memory_max_readable", "observed_probe_version", "probe_version_matches",
                "realized_memory_max_bytes", "recorded_cpu_max", "recorded_cpuset", "status",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );

        let verdict_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("verdict.json")).expect("read"))
                .expect("verdict json");
        assert_eq!(
            verdict_json,
            serde_json::json!({
                "allow_debug_benchmark": false,
                "allow_degraded_cold": false,
                "allow_version_skew": false,
                "cv_threshold": 0.10,
                "schema_version": "verdict/v1",
                "validity": "Valid"
            })
        );
    }

    #[test]
    fn harness_artifacts_pma_fsync_mode_requested_and_resolved_cases_include_fsync() {
        let tempdir = tempdir().expect("tempdir");
        let root = tempdir.path();
        let requested = RequestedCase::native(PathBuf::from("fixture.soltest"));
        let resolved = resolved_native_case(requested.clone());

        write_requested_case(root, &requested).expect("requested");
        write_resolved_case(root, &resolved).expect("resolved");

        let requested_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("requested_case.json")).expect("read"))
                .expect("requested json");
        assert_eq!(requested_json.get("fsync"), Some(&serde_json::json!("on")));

        let resolved_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("resolved_case.json")).expect("read"))
                .expect("resolved json");
        let requested_object = resolved_json
            .get("requested")
            .and_then(serde_json::Value::as_object)
            .expect("resolved requested object");
        assert_eq!(
            requested_object.get("fsync"),
            Some(&serde_json::json!("on"))
        );
    }
}
