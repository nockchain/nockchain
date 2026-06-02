use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::time::sleep;

use super::artifacts::{
    write_host_env, write_json, write_provenance, write_requested_case, write_resolved_case,
    write_schema_version, write_summary, write_verdict,
};
use super::case::{ExecutionRequest, RequestedCase, RequestedOrchestrate};
use super::execute::CompletedRun;
use super::provenance::{build_provenance, capture_host_env, BackendRuntimeFacts, Provenance};
use super::summary::{
    evaluate_verdict, stats, summarize_runs, RunFailure, RunMetrics, RunSummary, RunSummaryInput,
    StepSummary, StepTypeSummary, Verdict,
};
use super::validate::BackendValidationOutcome;
use super::{
    is_release_build, resolve_requested_case, HarnessError, ResolvedCase,
    DEFAULT_THROUGHPUT_CV_THRESHOLD,
};
use crate::speed_of_light::kernel_utils::{
    init_boot_source_backed_nockapp, peek_heaviest_chain_or_block,
};
use crate::speed_of_light::types::SolHeight;
use crate::speed_of_light::{
    build_generated_read_plan, build_generated_replay_plan, load_plan_input, normalize_plan,
    GeneratedReadOptions, GeneratedReplayOptions, PeekRangeRequest, SolArchiveReader, TrustedStep,
};

#[derive(Debug)]
pub struct TrustedRunResult {
    pub resolved: ResolvedCase,
    pub provenance: Provenance,
    pub summary: RunSummary,
    pub verdict: Verdict,
}

pub trait TrustedBackend {
    fn prepare<'a>(
        &'a mut self,
        resolved: &'a mut ResolvedCase,
        output_root: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>>;

    fn capture_runtime_facts(&self) -> Result<BackendRuntimeFacts, HarnessError>;

    fn validation_outcome(&self) -> BackendValidationOutcome {
        BackendValidationOutcome::default()
    }

    fn execute_run<'a>(
        &'a mut self,
        resolved: &'a ResolvedCase,
        run_id: &'a str,
        run_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<CompletedRun, HarnessError>>;

    fn capture_raw_evidence<'a>(
        &'a self,
        raw_dir: &'a Path,
    ) -> futures::future::BoxFuture<'a, Result<(), HarnessError>>;

    fn cleanup<'a>(&'a mut self) -> futures::future::BoxFuture<'a, Result<(), HarnessError>>;
}

pub async fn execute_trusted_run<B: TrustedBackend>(
    mut backend: B,
    requested: RequestedCase,
    output_root: &Path,
    allow_debug_benchmark: bool,
) -> Result<TrustedRunResult, HarnessError> {
    prepare_output_root(output_root)?;
    std::fs::create_dir_all(output_root)?;
    let mut resolved = resolve_requested_case(&requested)?;
    write_requested_case(output_root, &requested)?;
    resolve_trusted_plan_artifact(&requested, &mut resolved, output_root).await?;
    if resolved.orchestrate.contains_cold_steps
        && matches!(requested.execution, ExecutionRequest::Native)
    {
        if let Some(reason) = native_cold_runtime_rejection(&BackendRuntimeFacts::Native) {
            prune_output_root_to_requested_case(output_root)?;
            return Err(HarnessError::InvalidRequestedCase(reason));
        }
    }
    let runs_root = output_root.join("runs");
    let raw_dir = output_root.join("raw");
    std::fs::create_dir_all(&runs_root)?;
    std::fs::create_dir_all(&raw_dir)?;
    if let Err(error) = backend.prepare(&mut resolved, output_root).await {
        return fail_after_prepare(&mut backend, &raw_dir, error).await;
    }
    let runtime_facts_result = backend.capture_runtime_facts();
    let runtime_facts = fail_with_cleanup(&mut backend, runtime_facts_result).await?;
    let validation_outcome = backend.validation_outcome();
    let provenance = build_provenance(
        &resolved,
        runtime_facts,
        validation_outcome.pma_replay_proven(),
    );

    fail_with_cleanup(
        &mut backend,
        write_trusted_run_scaffold(output_root, &requested, &resolved, &provenance),
    )
    .await?;
    let raw_evidence_result = backend.capture_raw_evidence(&raw_dir).await;
    fail_with_cleanup(&mut backend, raw_evidence_result).await?;

    let release_build = is_release_build();
    let (invalid_reasons, partial_reasons) = trusted_policy_reasons(
        &resolved, &provenance, &validation_outcome, allow_debug_benchmark,
    );
    if !invalid_reasons.is_empty() {
        let summary = summarize_runs(&[], &[], requested.measured_runs);
        let verdict = evaluate_verdict(&RunSummaryInput {
            measured_run_count: requested.measured_runs,
            run_failures: Vec::new(),
            throughput_cv: None,
            cv_threshold: requested
                .cv_threshold
                .unwrap_or(DEFAULT_THROUGHPUT_CV_THRESHOLD),
            release_build,
            allow_debug_benchmark,
            allow_version_skew: requested.allow_version_skew,
            allow_degraded_cold: requested.allow_degraded_cold,
            invalid_reasons,
            partial_reasons,
        });
        fail_with_cleanup(&mut backend, write_summary(output_root, &summary)).await?;
        fail_with_cleanup(&mut backend, write_verdict(output_root, &verdict)).await?;
        backend.cleanup().await?;
        return Ok(TrustedRunResult {
            resolved,
            provenance,
            summary,
            verdict,
        });
    }

    for index in 0..requested.warmup_runs {
        let run_id = format!("warmup-{index}");
        let run_dir = runs_root.join(&run_id);
        let warmup_result = backend.execute_run(&resolved, &run_id, &run_dir).await;
        fail_with_cleanup(&mut backend, warmup_result).await?;
    }

    let mut run_failures = Vec::new();
    let mut run_metrics = Vec::new();
    let mut run_invalid_reasons = Vec::new();
    for index in 0..requested.measured_runs {
        let run_id = format!("run-{index}");
        let run_dir = runs_root.join(&run_id);
        let run_result = backend.execute_run(&resolved, &run_id, &run_dir).await;
        let completed = fail_with_cleanup(&mut backend, run_result).await?;
        for reason in completed.invalid_reasons.clone() {
            if !run_invalid_reasons.contains(&reason) {
                run_invalid_reasons.push(reason);
            }
        }
        if completed.record.success {
            run_metrics.push(completed_run_into_metrics(&completed));
        } else {
            run_failures.push(RunFailure {
                run_id,
                reason: completed
                    .record
                    .error
                    .clone()
                    .unwrap_or_else(|| "run failed".to_string()),
            });
        }

        if index + 1 < requested.measured_runs && requested.cooldown_secs > 0 {
            sleep(Duration::from_secs(requested.cooldown_secs)).await;
        }
    }

    let run_metrics: Vec<_> = run_metrics.into_iter().flatten().collect();
    let mut summary = summarize_runs(&run_metrics, &run_failures, requested.measured_runs);
    populate_step_summaries(output_root, &mut summary)?;
    let mut invalid_reasons = Vec::new();
    let mut partial_reasons = partial_reasons;
    if requested.measured_runs < 3 {
        invalid_reasons.push("trusted sol bench requires at least 3 measured runs".to_string());
    }
    for reason in run_invalid_reasons {
        if !invalid_reasons.contains(&reason) {
            invalid_reasons.push(reason);
        }
    }
    partial_reasons.extend(missing_peek_partial_reasons(&summary));

    let verdict = evaluate_verdict(&RunSummaryInput {
        measured_run_count: requested.measured_runs,
        run_failures: run_failures.clone(),
        throughput_cv: primary_cv(&summary, &resolved),
        cv_threshold: requested
            .cv_threshold
            .unwrap_or(DEFAULT_THROUGHPUT_CV_THRESHOLD),
        release_build,
        allow_debug_benchmark,
        allow_version_skew: requested.allow_version_skew,
        allow_degraded_cold: requested.allow_degraded_cold,
        invalid_reasons,
        partial_reasons,
    });

    fail_with_cleanup(&mut backend, write_summary(output_root, &summary)).await?;
    fail_with_cleanup(&mut backend, write_verdict(output_root, &verdict)).await?;

    backend.cleanup().await?;

    Ok(TrustedRunResult {
        resolved,
        provenance,
        summary,
        verdict,
    })
}

fn primary_cv(summary: &RunSummary, resolved: &ResolvedCase) -> Option<f64> {
    let has_poke = summary.by_step_type.contains_key("poke_archive_block");
    let has_warm_peek = summary.by_step_type.contains_key("peek_height");
    let has_cold_peek = summary.by_step_type.contains_key("peek_height_cold");
    let families = [has_poke, has_warm_peek, has_cold_peek]
        .into_iter()
        .filter(|present| *present)
        .count();
    let selected = if families > 1 {
        summary.total_step_time_secs.as_ref()
    } else if has_cold_peek || resolved.orchestrate.contains_cold_steps {
        summary.cold_peeks_per_second.as_ref()
    } else if has_warm_peek {
        summary.peeks_per_second.as_ref()
    } else {
        summary.pokes_per_second.as_ref()
    };
    selected.map(|stats| stats.cv)
}

fn missing_peek_partial_reasons(summary: &RunSummary) -> Vec<String> {
    ["peek_height", "peek_height_cold"]
        .into_iter()
        .filter_map(|step_type| {
            let missing = summary
                .by_step_type
                .get(step_type)?
                .missing_count
                .as_ref()?;
            (missing.max > 0.0).then(|| {
                format!(
                    "{step_type} reported missing peeks (median {:.0}, max {:.0} per measured run)",
                    missing.median, missing.max
                )
            })
        })
        .collect()
}

fn populate_step_summaries(
    output_root: &Path,
    summary: &mut RunSummary,
) -> Result<(), HarnessError> {
    let runs_dir = output_root.join("runs");
    if !runs_dir.exists() {
        return Ok(());
    }
    #[derive(Default)]
    struct StepAggregate {
        step_index: usize,
        step_type: String,
        height: Option<u64>,
        duration_ms: Vec<f64>,
        outcomes: std::collections::BTreeMap<String, u64>,
    }

    #[derive(Default)]
    struct StepTypeRunAggregate {
        count: u64,
        duration_ms: Vec<f64>,
        errors: u64,
        successes: u64,
        missing: u64,
        cold_verified: u64,
        cold_unverified: u64,
        minflt_delta: Vec<f64>,
        majflt_delta: Vec<f64>,
    }

    let mut by_step = std::collections::BTreeMap::<String, StepAggregate>::new();
    let mut by_type_run = std::collections::BTreeMap::<
        String,
        std::collections::BTreeMap<String, StepTypeRunAggregate>,
    >::new();
    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        if !file_name.to_string_lossy().starts_with("run-") {
            continue;
        }
        let path = entry.path().join("steps.ndjson");
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(path)?;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let row: crate::speed_of_light::orchestrate_execute::StepResultRow =
                serde_json::from_str(line)?;
            let step = by_step
                .entry(row.step_id.clone())
                .or_insert_with(|| StepAggregate {
                    step_index: row.step_index,
                    step_type: row.step_type.clone(),
                    height: row.height,
                    ..StepAggregate::default()
                });
            step.duration_ms.push(row.duration_ms);
            *step.outcomes.entry(row.outcome.clone()).or_default() += 1;

            let type_run = by_type_run
                .entry(row.step_type.clone())
                .or_default()
                .entry(row.run_id.clone())
                .or_default();
            type_run.count += 1;
            type_run.duration_ms.push(row.duration_ms);
            if row.outcome == "error" {
                type_run.errors += 1;
            }
            if row.outcome == "success" || row.outcome == "ok" {
                type_run.successes += 1;
            }
            if row.outcome == "missing" {
                type_run.missing += 1;
            }
            if row.step_type == "peek_height_cold"
                || row.step_type == "force_cold"
                || row.cold_evidence_id.is_some()
            {
                if row.trusted_metric_valid == Some(true) {
                    type_run.cold_verified += 1;
                } else {
                    type_run.cold_unverified += 1;
                }
            }
            if let Some(delta) = row.minflt_delta {
                type_run.minflt_delta.push(delta as f64);
            }
            if let Some(delta) = row.majflt_delta {
                type_run.majflt_delta.push(delta as f64);
            }
        }
    }
    summary.steps = by_step
        .into_iter()
        .filter_map(|(step_id, aggregate)| {
            Some(StepSummary {
                step_index: aggregate.step_index,
                step_id,
                step_type: aggregate.step_type,
                height: aggregate.height,
                duration_ms: stats(aggregate.duration_ms.into_iter()),
                outcomes: aggregate.outcomes,
            })
        })
        .collect();
    summary.by_step_type = by_type_run
        .into_iter()
        .map(|(step_type, by_run)| {
            let count_per_run = by_run
                .values()
                .map(|run| run.count)
                .max()
                .unwrap_or_default();
            let duration_ms = by_run
                .values()
                .flat_map(|run| run.duration_ms.iter().copied())
                .collect::<Vec<_>>();
            let throughput_per_second = by_run
                .values()
                .filter_map(|run| {
                    let total_secs = run.duration_ms.iter().copied().sum::<f64>() / 1000.0;
                    let numerator =
                        if matches!(step_type.as_str(), "peek_height" | "peek_height_cold") {
                            run.successes
                        } else {
                            run.count
                        };
                    (numerator > 0 && total_secs > 0.0 && total_secs.is_finite())
                        .then_some(numerator as f64 / total_secs)
                })
                .collect::<Vec<_>>();
            let minflt_delta = by_run
                .values()
                .flat_map(|run| run.minflt_delta.iter().copied())
                .collect::<Vec<_>>();
            let majflt_delta = by_run
                .values()
                .flat_map(|run| run.majflt_delta.iter().copied())
                .collect::<Vec<_>>();
            (
                step_type,
                StepTypeSummary {
                    count_per_run,
                    duration_ms: stats(duration_ms.into_iter()),
                    throughput_per_second: stats(throughput_per_second.into_iter()),
                    error_count: stats(by_run.values().map(|run| run.errors as f64)),
                    success_count: stats(by_run.values().map(|run| run.successes as f64)),
                    missing_count: stats(by_run.values().map(|run| run.missing as f64)),
                    cold_verified_count: stats(by_run.values().map(|run| run.cold_verified as f64)),
                    cold_unverified_count: stats(
                        by_run.values().map(|run| run.cold_unverified as f64),
                    ),
                    minflt_delta: stats(minflt_delta.into_iter()),
                    majflt_delta: stats(majflt_delta.into_iter()),
                },
            )
        })
        .collect();
    Ok(())
}

async fn resolve_trusted_plan_artifact(
    requested: &RequestedCase,
    resolved: &mut ResolvedCase,
    output_root: &Path,
) -> Result<(), HarnessError> {
    let mut trusted_plan = match &requested.orchestrate {
        RequestedOrchestrate::GeneratedReplay {
            fixture_path,
            blocks,
            skip_genesis,
        } => {
            let generated = build_generated_replay_plan(&GeneratedReplayOptions {
                fixture_path: fixture_path.clone(),
                output_root: output_root.to_path_buf(),
                blocks: *blocks,
                skip_genesis: *skip_genesis,
            })
            .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
            normalize_plan(generated.plan_input)
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?
        }
        RequestedOrchestrate::PlanFile { plan_path } => {
            let source_plan_path = canonicalize_source_path(plan_path)?;
            let source_plan_sha256_hex = sha256_hex_for_file(&source_plan_path)?;
            resolved.orchestrate.source_plan_sha256_hex = Some(source_plan_sha256_hex);
            resolved.orchestrate.source_plan_path = Some(source_plan_path);
            let input = load_plan_input(plan_path)
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
            normalize_plan(input)
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?
        }
        RequestedOrchestrate::GeneratedRead {
            boot,
            kernel_path,
            start_height,
            end_height,
            count,
            peek_mode,
        } => {
            let work_dir = output_root.join("input/read-tip-work");
            std::fs::create_dir_all(&work_dir)?;
            let resolved_boot = boot
                .clone()
                .resolve()
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
            let mut nockapp = init_boot_source_backed_nockapp(
                &resolved_boot,
                kernel_path,
                &work_dir,
                requested.fsync_enabled(),
            )
            .await
            .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
            let tip = peek_heaviest_chain_or_block(&mut nockapp)
                .await
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?
                .ok_or_else(|| {
                    HarnessError::InvalidRequestedCase(
                        "heaviest chain tip is unavailable after boot".to_string(),
                    )
                })?;
            let generated = build_generated_read_plan(&GeneratedReadOptions {
                boot: boot.clone(),
                kernel_path: kernel_path.clone(),
                start_height: *start_height,
                range: PeekRangeRequest::from_bounds(*end_height, *count)
                    .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?,
                peek_mode: *peek_mode,
                tip_height: tip.0 .0 .0,
            })
            .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
            resolved.orchestrate.read_range_resolution =
                Some(generated.read_range_resolution.clone());
            normalize_plan(generated.plan_input)
                .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?
        }
    };

    apply_trusted_replay_policy(&mut trusted_plan)?;
    trusted_plan.boot.fsync = requested.fsync_enabled();
    crate::speed_of_light::refresh_plan_hashes(&mut trusted_plan)
        .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;
    write_json(output_root.join("trusted_plan.json"), &trusted_plan)?;
    resolved.orchestrate.normalized_plan_sha256_hex =
        Some(trusted_plan.normalized_plan_sha256_hex.clone());
    resolved.orchestrate.inputs = trusted_plan.inputs.clone();
    resolved.orchestrate.step_count = trusted_plan.steps.len();
    resolved.orchestrate.step_signature_sha256_hex =
        Some(trusted_plan.step_signature_sha256_hex.clone());
    resolved.orchestrate.contains_cold_steps = trusted_plan.steps.iter().any(|step| {
        matches!(
            step,
            TrustedStep::ForceCold { .. } | TrustedStep::PeekHeightCold { .. }
        )
    });
    Ok(())
}

fn apply_trusted_replay_policy(
    plan: &mut crate::speed_of_light::TrustedPlan,
) -> Result<(), HarnessError> {
    let mut selected_by_archive = BTreeMap::<String, Vec<u64>>::new();
    for step in &plan.steps {
        if let TrustedStep::PokeArchiveBlock {
            archive_input_id,
            height,
            ..
        } = step
        {
            selected_by_archive
                .entry(archive_input_id.clone())
                .or_default()
                .push(*height);
        }
    }
    if selected_by_archive.is_empty() {
        return Ok(());
    }

    for (archive_input_id, heights) in selected_by_archive {
        let archive_input = plan
            .inputs
            .iter()
            .find(|input| input.input_id == archive_input_id)
            .ok_or_else(|| {
                HarnessError::InvalidRequestedCase(format!(
                    "trusted plan references missing archive input {archive_input_id}"
                ))
            })?;
        let reader = SolArchiveReader::from_file(&archive_input.absolute_path)
            .map_err(|error| HarnessError::InvalidRequestedCase(error.to_string()))?;

        if plan.expected_final_tip.is_none() {
            if let Some(gap_height) = first_replay_gap(&heights) {
                return Err(HarnessError::InvalidRequestedCase(format!(
                    "replay range non-contiguous: gap at height {gap_height}"
                )));
            }
        }

        validate_archive_replay_blocks(&reader, &heights)?;
    }
    Ok(())
}

fn validate_archive_replay_blocks(
    reader: &SolArchiveReader,
    heights: &[u64],
) -> Result<(), HarnessError> {
    for height in heights {
        reader
            .get_entry_by_height(SolHeight(*height))
            .ok_or_else(|| missing_archive_block_error(*height))?;
    }
    Ok(())
}

fn missing_archive_block_error(height: u64) -> HarnessError {
    HarnessError::InvalidRequestedCase(format!(
        "trusted replay references archive block missing at height {height}"
    ))
}

fn first_replay_gap(heights: &[u64]) -> Option<u64> {
    heights.windows(2).find_map(|pair| {
        let expected = pair[0].saturating_add(1);
        (pair[1] != expected).then_some(expected)
    })
}

fn canonicalize_source_path(path: &Path) -> Result<std::path::PathBuf, HarnessError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(absolute.canonicalize()?)
}

fn sha256_hex_for_file(path: &Path) -> Result<String, HarnessError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

async fn fail_after_prepare<B: TrustedBackend>(
    backend: &mut B,
    raw_dir: &Path,
    error: HarnessError,
) -> Result<TrustedRunResult, HarnessError> {
    let _ = backend.capture_raw_evidence(raw_dir).await;
    let _ = backend.cleanup().await;
    Err(error)
}

async fn fail_with_cleanup<B: TrustedBackend, T>(
    backend: &mut B,
    result: Result<T, HarnessError>,
) -> Result<T, HarnessError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = backend.cleanup().await;
            Err(error)
        }
    }
}

fn write_trusted_run_scaffold(
    output_root: &Path,
    requested: &RequestedCase,
    resolved: &ResolvedCase,
    provenance: &Provenance,
) -> Result<(), HarnessError> {
    write_schema_version(output_root)?;
    write_requested_case(output_root, requested)?;
    write_resolved_case(output_root, resolved)?;
    write_provenance(output_root, provenance)?;
    write_host_env(output_root, &capture_host_env())?;
    Ok(())
}

fn prune_output_root_to_requested_case(output_root: &Path) -> Result<(), HarnessError> {
    for entry in std::fs::read_dir(output_root)? {
        let entry = entry?;
        if entry.file_name() == "requested_case.json" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn trusted_policy_reasons(
    resolved: &ResolvedCase,
    provenance: &Provenance,
    _validation_outcome: &BackendValidationOutcome,
    allow_debug_benchmark: bool,
) -> (Vec<String>, Vec<String>) {
    let mut invalid_reasons = Vec::new();
    let mut partial_reasons = Vec::new();

    if let BackendRuntimeFacts::Docker {
        host_binary,
        container_binary,
        ..
    } = &provenance.backend
    {
        if !_validation_outcome.pma_replay_proven() {
            invalid_reasons.push("trusted Docker PMA run did not prove PMA replay".to_string());
        }

        if !is_trusted_release_profile(&container_binary.build_profile) {
            let reason = format!(
                "trusted Docker runs require a release build unless --allow-debug-benchmark is set (container build profile: {})",
                container_binary.build_profile
            );
            if allow_debug_benchmark {
                partial_reasons.push(reason);
            } else {
                invalid_reasons.push(reason);
            }
        }

        if let Some(reason) = version_skew_reason(host_binary, container_binary) {
            let allow_version_skew = matches!(
                &resolved.requested.execution,
                ExecutionRequest::Docker {
                    allow_version_skew: true,
                    ..
                }
            );
            if allow_version_skew {
                partial_reasons.push(format!("{reason} under --allow-version-skew override"));
            } else {
                invalid_reasons.push(reason);
            }
        }
    }

    if resolved.requested.allow_degraded_cold {
        partial_reasons.push(
            "cold verification degradation override was enabled with --allow-degraded-cold"
                .to_string(),
        );
    }

    if resolved.orchestrate.contains_cold_steps {
        if let Some(reason) = native_cold_runtime_rejection(&provenance.backend) {
            invalid_reasons.push(reason);
        }
    }

    (invalid_reasons, partial_reasons)
}

fn native_cold_runtime_rejection(backend: &BackendRuntimeFacts) -> Option<String> {
    if !matches!(backend, BackendRuntimeFacts::Native) {
        return None;
    }

    #[cfg(not(target_os = "linux"))]
    {
        Some("trusted native cold runs require cgroup v2 memory.reclaim support".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        match crate::speed_of_light::cold_peek::ColdRuntime::startup_if_needed(
            true,
            crate::speed_of_light::ColdMode::Strict,
        ) {
            Ok(_) => None,
            Err(error) => Some(format!("trusted native cold runtime cannot init: {error}")),
        }
    }
}

fn is_trusted_release_profile(build_profile: &str) -> bool {
    matches!(build_profile, "release" | "bytehound")
}

fn version_skew_reason(
    host_binary: &crate::speed_of_light::harness::BinaryIdentity,
    container_binary: &crate::speed_of_light::harness::BinaryIdentity,
) -> Option<String> {
    if host_binary.version != container_binary.version {
        return Some(format!(
            "host/container version skew detected: host={} container={}",
            host_binary.version, container_binary.version
        ));
    }

    if host_binary.git_commit != container_binary.git_commit {
        return Some(format!(
            "host/container git commit skew detected: host={:?} container={:?}",
            host_binary.git_commit, container_binary.git_commit
        ));
    }

    None
}

pub(crate) fn prepare_output_root(output_root: &Path) -> Result<(), HarnessError> {
    if !output_root.exists() {
        return Ok(());
    }

    let mut entries = std::fs::read_dir(output_root)?;
    if entries.next().is_some() {
        return Err(HarnessError::InvalidRequestedCase(format!(
            "output directory {} already exists and is not empty",
            output_root.display()
        )));
    }

    Ok(())
}

fn completed_run_into_metrics(completed: &CompletedRun) -> Option<RunMetrics> {
    if let Some(record) = &completed.trusted_orchestrate_record {
        let mut metrics = trusted_run_record_into_metrics(record)?;
        metrics.peak_process_rss_bytes = completed.record.peak_process_rss_bytes;
        metrics.minor_faults_total = completed.record.minor_faults_total;
        metrics.major_faults_total = completed.record.major_faults_total;
        return Some(metrics);
    }
    run_record_into_metrics(&completed.record)
}

fn trusted_run_record_into_metrics(
    record: &crate::speed_of_light::orchestrate_execute::RunRecord,
) -> Option<RunMetrics> {
    if !record.success {
        return None;
    }

    Some(RunMetrics {
        steps_per_second: record.throughput.steps_per_second,
        block_pokes_per_second: record.throughput.block_pokes_per_second,
        pokes_per_second: record.throughput.pokes_per_second,
        raw_tx_pokes_per_second: record.throughput.raw_tx_pokes_per_second,
        peeks_per_second: record.throughput.peeks_per_second,
        cold_peeks_per_second: record.throughput.cold_peeks_per_second,
        init_time_secs: record.boot.init_time_secs.unwrap_or(0.0),
        total_step_time_secs: record.timing.total_step_time_secs,
        average_block_time_ms: if record.counts.poke_archive_block > 0 {
            record.timing.total_poke_time_secs * 1000.0 / record.counts.poke_archive_block as f64
        } else {
            0.0
        },
        peak_process_rss_bytes: None,
        minor_faults_total: None,
        major_faults_total: None,
    })
}

fn run_record_into_metrics(record: &super::execute::RunRecord) -> Option<RunMetrics> {
    if !record.success {
        return None;
    }

    Some(RunMetrics {
        steps_per_second: None,
        block_pokes_per_second: Some(record.throughput_blocks_per_second),
        pokes_per_second: Some(record.throughput_blocks_per_second),
        raw_tx_pokes_per_second: None,
        peeks_per_second: None,
        cold_peeks_per_second: None,
        init_time_secs: record.init_time_secs,
        total_step_time_secs: record.total_replay_time_secs,
        average_block_time_ms: record.average_block_time_ms,
        peak_process_rss_bytes: record.peak_process_rss_bytes,
        minor_faults_total: record.minor_faults_total,
        major_faults_total: record.major_faults_total,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use futures::FutureExt;
    use nockapp::nockapp::save::JammedCheckpointV2;
    use nockapp::JammedNoun;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;
    use tempfile::tempdir;

    use super::{execute_trusted_run, is_trusted_release_profile, TrustedBackend};
    #[cfg(target_os = "linux")]
    use crate::speed_of_light::cold_peek::{set_test_cold_init_overrides, ColdInitError};
    use crate::speed_of_light::harness::artifacts::write_run_artifacts;
    use crate::speed_of_light::harness::docker_image::DockerImageSource;
    use crate::speed_of_light::harness::execute::{BlockTimingRecord, CompletedRun, RunRecord};
    use crate::speed_of_light::harness::provenance::BackendRuntimeFacts;
    use crate::speed_of_light::harness::summary::{summarize_runs, StepTypeSummary, ValueStats};
    use crate::speed_of_light::harness::validate::BackendValidationOutcome;
    use crate::speed_of_light::harness::{RequestedCase, RequestedOrchestrate};
    use crate::speed_of_light::{
        BootSourceInput, ExpectedFinalTip, OrchestratePlanInput, PlanStepInput, ProofVersion,
        SolArchiveWriter, SolHeight,
    };

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

    fn dummy_hash(value: u64) -> Hash {
        Hash([Belt(value), Belt(value + 1), Belt(value + 2), Belt(value + 3), Belt(value + 4)])
    }

    fn write_archive(path: &Path, blocks: &[(u64, usize)]) {
        let mut writer = SolArchiveWriter::new();
        for (height, tx_count) in blocks {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(*height),
                    dummy_hash(*height),
                    *tx_count,
                    ProofVersion::V0,
                    &[*height as u8],
                )
                .expect("add archive block");
        }
        writer.write_to_file(path).expect("write archive");
    }

    fn trusted_poke_plan(
        root: &Path,
        archive_blocks: &[(u64, usize)],
        selected_heights: &[u64],
        expected_final_tip: Option<ExpectedFinalTip>,
    ) -> crate::speed_of_light::TrustedPlan {
        let checkpoint_path = root.join(format!("checkpoint-{}.chkjam", selected_heights[0]));
        let kernel_path = root.join(format!("kernel-{}.jam", selected_heights[0]));
        let archive_path = root.join(format!("archive-{}.solarch", selected_heights[0]));
        write_checkpoint(&checkpoint_path, 0);
        std::fs::write(&kernel_path, [4, 5, 6]).expect("kernel");
        write_archive(&archive_path, archive_blocks);
        let steps = selected_heights
            .iter()
            .map(|height| PlanStepInput::PokeArchiveBlock {
                archive: archive_path.clone(),
                height: *height,
                label: None,
            })
            .collect();
        crate::speed_of_light::normalize_plan(OrchestratePlanInput {
            schema_version: Some(
                crate::speed_of_light::ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION.to_string(),
            ),
            boot: BootSourceInput::Checkpoint {
                checkpoint: checkpoint_path,
            },
            kernel: kernel_path,
            expected_final_tip,
            steps,
        })
        .expect("normalize trusted plan")
    }

    #[tokio::test]
    async fn orchestrator_captures_runtime_facts_before_measured_runs() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let backend = FakeBackend::successful();
        let events = backend.shared_events();

        let result =
            execute_trusted_run(backend, requested, &tempdir.path().join("out"), false).await;

        assert!(result.is_ok(), "orchestrator should succeed: {result:?}");
        assert_eq!(
            events.lock().expect("events").clone(),
            vec![
                "prepare", "setup", "raw-evidence", "warmup-0", "run-0", "run-1", "run-2",
                "cleanup",
            ]
        );
    }

    #[test]
    fn missing_peek_counts_create_partial_reason() {
        let mut summary = summarize_runs(&[], &[], 3);
        summary.by_step_type.insert(
            "peek_height".to_string(),
            StepTypeSummary {
                count_per_run: 100,
                duration_ms: None,
                throughput_per_second: None,
                error_count: None,
                success_count: Some(stats_value(0.0)),
                missing_count: Some(stats_value(100.0)),
                cold_verified_count: None,
                cold_unverified_count: None,
                minflt_delta: None,
                majflt_delta: None,
            },
        );

        let reasons = super::missing_peek_partial_reasons(&summary);

        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("peek_height reported missing peeks"));
    }

    fn stats_value(value: f64) -> ValueStats {
        ValueStats {
            median: value,
            min: value,
            max: value,
            mad: 0.0,
            stddev: 0.0,
            cv: 0.0,
            values: vec![value],
        }
    }

    #[test]
    fn trusted_replay_policy_accepts_transaction_blocks_with_raw_payloads() {
        let tempdir = tempdir().expect("tempdir");
        let mut plan = trusted_poke_plan(tempdir.path(), &[(1, 1)], &[1], None);

        super::apply_trusted_replay_policy(&mut plan).expect("current archive should be complete");

        assert!(plan.invalid_reasons.is_empty());
    }

    #[test]
    fn trusted_replay_policy_requires_expected_tip_for_non_contiguous_replay() {
        let tempdir = tempdir().expect("tempdir");
        let mut plan = trusted_poke_plan(tempdir.path(), &[(1, 0), (3, 0)], &[1, 3], None);

        let error = super::apply_trusted_replay_policy(&mut plan)
            .expect_err("gap without expected tip should reject");
        assert!(error
            .to_string()
            .contains("replay range non-contiguous: gap at height 2"));

        let mut plan = trusted_poke_plan(
            tempdir.path(),
            &[(1, 0), (3, 0)],
            &[1, 3],
            Some(ExpectedFinalTip {
                height: 3,
                hash: dummy_hash(3).to_base58(),
            }),
        );
        super::apply_trusted_replay_policy(&mut plan)
            .expect("explicit expected tip permits non-contiguous plan");
    }

    #[test]
    fn trusted_replay_policy_rejects_missing_archive_height() {
        let tempdir = tempdir().expect("tempdir");
        let mut plan = trusted_poke_plan(tempdir.path(), &[(1, 0)], &[2], None);

        let error = super::apply_trusted_replay_policy(&mut plan)
            .expect_err("missing height should reject");

        assert!(error
            .to_string()
            .contains("trusted replay references archive block missing at height 2"));
    }

    #[tokio::test]
    async fn orchestrator_marks_failed_measured_runs_partial() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let backend = FakeBackend::with_failure("run-1", "synthetic failure");

        let result = execute_trusted_run(backend, requested, &tempdir.path().join("out"), false)
            .await
            .expect("orchestrator result");

        assert_eq!(result.summary.measured_runs_succeeded, 2);
        match result.verdict.validity {
            crate::speed_of_light::harness::Validity::Partial { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("run-1")));
            }
            other => panic!("expected partial verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn orchestrator_writes_expected_artifact_tree() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeBackend::successful();

        execute_trusted_run(backend, requested, &output_root, false)
            .await
            .expect("orchestrator result");

        assert!(output_root.join("schema_version.txt").exists());
        assert!(output_root.join("requested_case.json").exists());
        assert!(output_root.join("resolved_case.json").exists());
        assert!(output_root.join("provenance.json").exists());
        assert!(output_root.join("raw/host_env.json").exists());
        assert!(output_root.join("raw/backend.txt").exists());
        assert!(output_root.join("runs/warmup-0/result.json").exists());
        assert!(output_root.join("runs/run-0/result.json").exists());
        assert!(output_root.join("runs/run-1/result.json").exists());
        assert!(output_root.join("runs/run-2/result.json").exists());
        assert!(output_root.join("summary.json").exists());
        assert!(output_root.join("verdict.json").exists());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn native_cold_runtime_rejection_leaves_only_requested_case() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_cold_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeBackend::successful();
        let events = backend.shared_events();
        let _guard =
            set_test_cold_init_overrides(None, Some(Err(ColdInitError::ReclaimUnsupported)));

        let error = execute_trusted_run(backend, requested, &output_root, false)
            .await
            .expect_err("native cold runtime should be rejected before runs");
        let mut entries = std::fs::read_dir(&output_root)
            .expect("output root")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();
        entries.sort();

        assert!(error
            .to_string()
            .contains("trusted native cold runtime cannot init"));
        assert_eq!(entries, vec!["requested_case.json"]);
        assert!(events.lock().expect("events").is_empty());
    }

    #[test]
    fn populate_step_summaries_writes_spec_shaped_step_rows() {
        let tempdir = tempdir().expect("tempdir");
        let run_dir = tempdir.path().join("runs/run-0");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let rows = [
            serde_json::json!({
                "schema_version": "step-result/v1",
                "run_id": "run-0",
                "step_index": 0,
                "step_id": "step-0000-poke-11",
                "label": "poke-11",
                "type": "poke_archive_block",
                "outcome": "ok",
                "duration_ms": 10.0,
                "height": 11,
                "input_id": "archive-0",
                "minflt_delta": 2,
                "majflt_delta": 0,
                "cold_evidence_id": null,
                "trusted_metric_valid": true,
                "error": null
            }),
            serde_json::json!({
                "schema_version": "step-result/v1",
                "run_id": "run-0",
                "step_index": 1,
                "step_id": "step-0001-read-11",
                "label": "read-11",
                "type": "peek_height",
                "outcome": "success",
                "duration_ms": 5.0,
                "height": 11,
                "input_id": null,
                "minflt_delta": 1,
                "majflt_delta": 0,
                "cold_evidence_id": null,
                "trusted_metric_valid": true,
                "error": null
            }),
        ];
        std::fs::write(
            run_dir.join("steps.ndjson"),
            rows.iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .expect("steps");

        let mut summary = crate::speed_of_light::harness::summary::summarize_runs(&[], &[], 1);
        super::populate_step_summaries(tempdir.path(), &mut summary).expect("summary");

        assert_eq!(summary.steps[0].step_index, 0);
        assert_eq!(summary.steps[0].height, Some(11));
        assert_eq!(summary.steps[0].outcomes.get("ok"), Some(&1));
        let poke = summary
            .by_step_type
            .get("poke_archive_block")
            .expect("poke summary");
        assert_eq!(poke.count_per_run, 1);
        assert_eq!(poke.error_count.as_ref().expect("errors").median, 0.0);
        assert_eq!(
            poke.throughput_per_second
                .as_ref()
                .expect("type throughput")
                .median,
            100.0
        );
        let peek = summary
            .by_step_type
            .get("peek_height")
            .expect("peek summary");
        assert_eq!(peek.success_count.as_ref().expect("success").median, 1.0);
        assert_eq!(peek.missing_count.as_ref().expect("missing").median, 0.0);
    }

    #[test]
    fn populate_step_summaries_excludes_warmup_runs() {
        let tempdir = tempdir().expect("tempdir");
        let measured_dir = tempdir.path().join("runs/run-0");
        let warmup_dir = tempdir.path().join("runs/warmup-0");
        std::fs::create_dir_all(&measured_dir).expect("measured dir");
        std::fs::create_dir_all(&warmup_dir).expect("warmup dir");

        let measured = serde_json::json!({
            "schema_version": "step-result/v1",
            "run_id": "run-0",
            "step_index": 0,
            "step_id": "step-0000-poke-11",
            "label": "poke-11",
            "type": "poke_archive_block",
            "outcome": "ok",
            "duration_ms": 10.0,
            "height": 11,
            "input_id": "archive-0",
            "minflt_delta": 2,
            "majflt_delta": 0,
            "cold_evidence_id": null,
            "trusted_metric_valid": true,
            "error": null
        });
        let warmup = serde_json::json!({
            "schema_version": "step-result/v1",
            "run_id": "warmup-0",
            "step_index": 0,
            "step_id": "step-0000-poke-11",
            "label": "poke-11",
            "type": "poke_archive_block",
            "outcome": "error",
            "duration_ms": 1000.0,
            "height": 11,
            "input_id": "archive-0",
            "minflt_delta": 200,
            "majflt_delta": 20,
            "cold_evidence_id": null,
            "trusted_metric_valid": false,
            "error": "warmup should be ignored"
        });
        std::fs::write(measured_dir.join("steps.ndjson"), format!("{measured}\n"))
            .expect("measured steps");
        std::fs::write(warmup_dir.join("steps.ndjson"), format!("{warmup}\n"))
            .expect("warmup steps");

        let mut summary = crate::speed_of_light::harness::summary::summarize_runs(&[], &[], 1);
        super::populate_step_summaries(tempdir.path(), &mut summary).expect("summary");

        assert_eq!(summary.steps.len(), 1);
        assert_eq!(
            summary.steps[0]
                .duration_ms
                .as_ref()
                .expect("duration")
                .median,
            10.0
        );
        assert_eq!(summary.steps[0].outcomes.get("ok"), Some(&1));
        assert_eq!(summary.steps[0].outcomes.get("error"), None);
        let poke = summary
            .by_step_type
            .get("poke_archive_block")
            .expect("poke summary");
        assert_eq!(poke.count_per_run, 1);
        assert_eq!(poke.error_count.as_ref().expect("errors").median, 0.0);
    }

    #[test]
    fn trusted_completed_run_metrics_preserve_backend_resource_metrics() {
        let trusted = crate::speed_of_light::orchestrate_execute::RunRecord {
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
                init_time_secs: Some(3.0),
            },
            steps_planned: 1,
            steps_executed: 1,
            cold: crate::speed_of_light::orchestrate_execute::RunColdCounts::default(),
            counts: crate::speed_of_light::orchestrate_execute::RunCounts {
                poke_archive_block: 1,
                ..Default::default()
            },
            timing: crate::speed_of_light::orchestrate_execute::RunTiming {
                total_step_time_secs: 2.0,
                total_poke_time_secs: 2.0,
                ..Default::default()
            },
            throughput: crate::speed_of_light::orchestrate_execute::RunThroughput {
                steps_per_second: Some(0.5),
                block_pokes_per_second: Some(0.5),
                pokes_per_second: Some(0.5),
                raw_tx_pokes_per_second: None,
                peeks_per_second: None,
                cold_peeks_per_second: None,
            },
            expected_final_tip: None,
            final_tip: None,
            final_tip_validation: None,
            invalid_reasons: Vec::new(),
            failed_step_index: None,
            memory_profile: None,
        };
        let completed = CompletedRun {
            record: RunRecord {
                run_id: "run-0".to_string(),
                success: true,
                error: None,
                blocks_poked: 1,
                failed_pokes: 0,
                init_time_secs: 3.0,
                total_replay_time_secs: 2.0,
                throughput_blocks_per_second: 0.5,
                average_block_time_ms: 2000.0,
                peak_process_rss_bytes: Some(900.0),
                minor_faults_total: Some(50.0),
                major_faults_total: Some(1.0),
                final_tip_validation: None,
            },
            trusted_orchestrate_record: Some(trusted),
            invalid_reasons: Vec::new(),
            block_timings: Vec::new(),
            profile: None,
            bench_results: None,
        };

        let metrics = super::completed_run_into_metrics(&completed).expect("metrics");

        assert_eq!(metrics.steps_per_second, Some(0.5));
        assert_eq!(metrics.peak_process_rss_bytes, Some(900.0));
        assert_eq!(metrics.minor_faults_total, Some(50.0));
        assert_eq!(metrics.major_faults_total, Some(1.0));
    }

    #[tokio::test]
    async fn explicit_plan_records_source_plan_outside_inputs() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let output_root = tempdir.path().join("out");
        let backend = FakeBackend::successful();

        execute_trusted_run(backend, requested, &output_root, false)
            .await
            .expect("orchestrator result");

        let trusted_plan: crate::speed_of_light::TrustedPlan = serde_json::from_slice(
            &std::fs::read(output_root.join("trusted_plan.json")).expect("trusted plan"),
        )
        .expect("trusted plan json");
        assert!(!trusted_plan
            .inputs
            .iter()
            .any(|input| input.role == crate::speed_of_light::InputRole::SourcePlan));

        let resolved: crate::speed_of_light::harness::ResolvedCase = serde_json::from_slice(
            &std::fs::read(output_root.join("resolved_case.json")).expect("resolved case"),
        )
        .expect("resolved case json");
        assert!(resolved.orchestrate.source_plan_path.is_some());
        assert!(resolved.orchestrate.source_plan_sha256_hex.is_some());
        assert!(!resolved
            .orchestrate
            .inputs
            .iter()
            .any(|input| input.role == crate::speed_of_light::InputRole::SourcePlan));
    }

    #[tokio::test]
    async fn orchestrator_cleans_up_when_runtime_facts_fail() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_requested_case(tempdir.path());
        let mut backend = FakeBackend::successful();
        backend.fail_runtime_facts = true;
        let events = backend.shared_events();

        let error = execute_trusted_run(backend, requested, &tempdir.path().join("out"), false)
            .await
            .expect_err("runtime facts should fail");

        assert!(error.to_string().contains("runtime facts"));
        assert_eq!(
            events.lock().expect("events").clone(),
            vec!["prepare", "setup", "cleanup"]
        );
    }

    #[tokio::test]
    async fn orchestrator_preserves_invalid_artifacts_for_docker_version_skew() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        let requested = write_docker_requested_case(tempdir.path(), false);
        let mut backend = FakeBackend::successful();
        backend.runtime_facts = docker_runtime_facts("0.1.1", "release", "container");

        let result = execute_trusted_run(backend, requested, &output_root, false)
            .await
            .expect("invalid run should still produce artifacts");

        assert!(output_root.join("requested_case.json").exists());
        assert!(output_root.join("resolved_case.json").exists());
        assert!(output_root.join("provenance.json").exists());
        assert!(output_root.join("summary.json").exists());
        assert!(output_root.join("verdict.json").exists());
        match result.verdict.validity {
            crate::speed_of_light::harness::Validity::Invalid { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("version skew")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
        assert_eq!(result.summary.measured_runs_succeeded, 0);
    }

    #[tokio::test]
    async fn orchestrator_rejects_debug_container_build_without_override() {
        let tempdir = tempdir().expect("tempdir");
        let requested = write_docker_requested_case(tempdir.path(), false);
        let mut backend = FakeBackend::successful();
        backend.runtime_facts = docker_runtime_facts("0.1.0", "debug", "host");

        let result = execute_trusted_run(backend, requested, &tempdir.path().join("out"), false)
            .await
            .expect("debug container should produce invalid verdict");

        match result.verdict.validity {
            crate::speed_of_light::harness::Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("release build")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
        assert_eq!(result.summary.measured_runs_succeeded, 0);
    }

    #[tokio::test]
    async fn docker_trusted_run_writes_additive_pma_provenance() {
        let tempdir = tempdir().expect("tempdir");
        let output_root = tempdir.path().join("out");
        let requested = write_docker_requested_case(tempdir.path(), false);
        let mut backend = FakeBackend::successful();
        backend.runtime_facts = docker_runtime_facts("0.1.0", "release", "host");
        backend.validation_outcome = BackendValidationOutcome::new(true);

        let result = execute_trusted_run(backend, requested, &output_root, false)
            .await
            .expect("pma docker run result");

        assert_eq!(result.provenance.runtime_flavor.as_deref(), Some("pma"));
        assert_eq!(result.provenance.boot_source.as_deref(), Some("checkpoint"));
        assert_eq!(
            result.provenance.boot_event_num,
            Some(result.resolved.fixture_manifest.checkpoint_event_num)
        );
        assert_eq!(
            result.provenance.pma_work_dir_mode.as_deref(),
            Some("docker_tmpfs")
        );

        let provenance = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(output_root.join("provenance.json")).expect("provenance"),
        )
        .expect("provenance json");
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
            provenance.get("pma_work_dir_mode"),
            Some(&serde_json::json!("docker_tmpfs"))
        );
    }

    #[tokio::test]
    async fn docker_trusted_run_preserves_version_skew_policy() {
        let tempdir = tempdir().expect("tempdir");

        let mut skewed_backend = FakeBackend::successful();
        skewed_backend.runtime_facts = docker_runtime_facts("0.1.1", "release", "container");
        skewed_backend.validation_outcome = BackendValidationOutcome::new(true);

        let invalid = execute_trusted_run(
            skewed_backend,
            write_docker_requested_case(tempdir.path(), false),
            &tempdir.path().join("invalid-out"),
            false,
        )
        .await
        .expect("invalid skew result");

        match invalid.verdict.validity {
            crate::speed_of_light::harness::Validity::Invalid { reasons } => {
                assert!(reasons.iter().any(|reason| reason.contains("version skew")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }

        let mut allowed_backend = FakeBackend::successful();
        allowed_backend.runtime_facts = docker_runtime_facts("0.1.1", "release", "container");
        allowed_backend.validation_outcome = BackendValidationOutcome::new(true);

        let partial = execute_trusted_run(
            allowed_backend,
            write_docker_requested_case(tempdir.path(), true),
            &tempdir.path().join("partial-out"),
            false,
        )
        .await
        .expect("partial skew result");

        match partial.verdict.validity {
            crate::speed_of_light::harness::Validity::Partial { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("--allow-version-skew")));
            }
            other => panic!("expected partial verdict, got {other:?}"),
        }

        let mut unproven_backend = FakeBackend::successful();
        unproven_backend.runtime_facts = docker_runtime_facts("0.1.0", "release", "host");
        unproven_backend.validation_outcome = BackendValidationOutcome::default();

        let unproven = execute_trusted_run(
            unproven_backend,
            write_docker_requested_case(tempdir.path(), false),
            &tempdir.path().join("unproven-out"),
            false,
        )
        .await
        .expect("unproven result");

        match unproven.verdict.validity {
            crate::speed_of_light::harness::Validity::Invalid { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|reason| reason.contains("did not prove PMA replay")));
            }
            other => panic!("expected invalid verdict, got {other:?}"),
        }
    }

    #[test]
    fn trusted_release_profiles_include_bytehound() {
        assert!(is_trusted_release_profile("release"));
        assert!(is_trusted_release_profile("bytehound"));
        assert!(!is_trusted_release_profile("debug"));
    }

    struct FakeBackend {
        events: Arc<Mutex<Vec<String>>>,
        failed_run_id: Option<String>,
        failure_message: Option<String>,
        fail_runtime_facts: bool,
        runtime_facts: BackendRuntimeFacts,
        validation_outcome: BackendValidationOutcome,
    }

    impl FakeBackend {
        fn successful() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                failed_run_id: None,
                failure_message: None,
                fail_runtime_facts: false,
                runtime_facts: BackendRuntimeFacts::Native,
                validation_outcome: BackendValidationOutcome::default(),
            }
        }

        fn with_failure(run_id: &str, message: &str) -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                failed_run_id: Some(run_id.to_string()),
                failure_message: Some(message.to_string()),
                fail_runtime_facts: false,
                runtime_facts: BackendRuntimeFacts::Native,
                validation_outcome: BackendValidationOutcome::default(),
            }
        }

        fn shared_events(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.events)
        }
    }

    impl TrustedBackend for FakeBackend {
        fn execute_run<'a>(
            &'a mut self,
            _resolved: &'a crate::speed_of_light::harness::ResolvedCase,
            run_id: &'a str,
            run_dir: &'a Path,
        ) -> futures::future::BoxFuture<
            'a,
            Result<CompletedRun, crate::speed_of_light::harness::HarnessError>,
        > {
            self.events.lock().expect("events").push(run_id.to_string());

            let should_fail = self.failed_run_id.as_deref() == Some(run_id);
            let failure_message = self.failure_message.clone();
            let run_dir = run_dir.to_path_buf();

            async move {
                let completed = CompletedRun {
                    record: RunRecord {
                        run_id: run_id.to_string(),
                        success: !should_fail,
                        error: should_fail.then(|| {
                            failure_message.unwrap_or_else(|| "synthetic failure".to_string())
                        }),
                        blocks_poked: (!should_fail) as u64,
                        failed_pokes: should_fail as u64,
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
                };
                write_run_artifacts(&run_dir, &completed).expect("run artifacts");
                Ok(completed)
            }
            .boxed()
        }

        fn prepare<'a>(
            &'a mut self,
            _resolved: &'a mut crate::speed_of_light::harness::ResolvedCase,
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
                .push("setup".to_string());
            if self.fail_runtime_facts {
                return Err(
                    crate::speed_of_light::harness::HarnessError::InvalidRequestedCase(
                        "runtime facts failed".to_string(),
                    ),
                );
            }
            Ok(self.runtime_facts.clone())
        }

        fn validation_outcome(&self) -> BackendValidationOutcome {
            self.validation_outcome
        }

        fn capture_raw_evidence<'a>(
            &'a self,
            raw_dir: &'a Path,
        ) -> futures::future::BoxFuture<'a, Result<(), crate::speed_of_light::harness::HarnessError>>
        {
            self.events
                .lock()
                .expect("events")
                .push("raw-evidence".to_string());
            let raw_dir = raw_dir.to_path_buf();
            async move {
                std::fs::create_dir_all(&raw_dir)?;
                std::fs::write(raw_dir.join("backend.txt"), "backend")?;
                Ok(())
            }
            .boxed()
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

    fn docker_runtime_facts(
        container_version: &str,
        container_build_profile: &str,
        container_commit: &str,
    ) -> BackendRuntimeFacts {
        BackendRuntimeFacts::Docker {
            host_binary: crate::speed_of_light::harness::BinaryIdentity {
                version: "0.1.0".to_string(),
                build_profile: "release".to_string(),
                git_commit: Some("host".to_string()),
            },
            container_binary: crate::speed_of_light::harness::BinaryIdentity {
                version: container_version.to_string(),
                build_profile: container_build_profile.to_string(),
                git_commit: Some(container_commit.to_string()),
            },
            image_source: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            requested_image_ref: "nockchain-bench:test".to_string(),
            resolved_image_ref: "sha256:test".to_string(),
            image_digest: "sha256:test".to_string(),
            container_id: "abc".to_string(),
            docker_engine_version: "29.1.3".to_string(),
            docker_context: "default".to_string(),
            cgroup_version: "2".to_string(),
            storage_driver: "overlayfs".to_string(),
            realized_memory_max: 1024,
            realized_memory_current: 512,
            realized_cpuset: Some("0-3".to_string()),
            realized_cpu_max: Some("max 100000".to_string()),
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

    fn write_cold_requested_case(root: &Path) -> RequestedCase {
        let mut requested = write_requested_case(root);
        requested.orchestrate = RequestedOrchestrate::PlanFile {
            plan_path: write_cold_test_plan(root),
        };
        requested
    }

    fn write_docker_requested_case(root: &Path, allow_version_skew: bool) -> RequestedCase {
        let mut requested = write_requested_case(root);
        requested.execution = crate::speed_of_light::harness::ExecutionRequest::Docker {
            image: DockerImageSource::AutoBuild {
                tag: "nockchain-bench:test".to_string(),
            },
            memory_limit: "1g".to_string(),
            cpuset: Some("0-3".to_string()),
            cpu_quota: None,
            cpu_period: None,
            work_dir_mode: crate::speed_of_light::harness::WorkDirMode::DockerTmpfs,
            allow_version_skew,
        };
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

    fn write_cold_test_plan(root: &Path) -> PathBuf {
        let checkpoint_path = root.join("cold-checkpoint.chkjam");
        let kernel_path = root.join("cold-kernel.jam");
        write_checkpoint(&checkpoint_path, 0);
        std::fs::write(&kernel_path, [4, 5, 6]).expect("kernel");
        let plan_path = root.join("cold-trusted-input-plan.json");
        let plan = serde_json::json!({
            "schema_version": crate::speed_of_light::ORCHESTRATE_PLAN_INPUT_SCHEMA_VERSION,
            "boot": checkpoint_boot(&checkpoint_path),
            "kernel": kernel_path,
            "steps": [{ "type": "peek_height_cold", "height": 1 }]
        });
        std::fs::write(
            &plan_path,
            serde_json::to_vec_pretty(&plan).expect("plan json"),
        )
        .expect("plan");
        plan_path
    }
}
