use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bridge_dev::actions::WithdrawalFaultTrace;
use bridge_dev::fault_campaign::{
    build_candidates, run_fault_campaign, select_pairwise, CampaignCandidate,
    CampaignExecutionFailure, CampaignExecutor, CampaignOptions, CampaignRunStatus, DimensionValue,
};
use bridge_dev::generated_scenario::{generate_scenario, GeneratedScenarioOptions};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn candidate_selection_is_deterministic_for_fixed_seed() {
    let options = options(4);
    let (first, first_duplicates) = build_candidates(&options).expect("first candidates");
    let (second, second_duplicates) = build_candidates(&options).expect("second candidates");
    assert_eq!(first_duplicates, second_duplicates);
    assert_eq!(
        first
            .iter()
            .map(|candidate| (&candidate.seed, &candidate.trace_sha256, &candidate.dimensions))
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|candidate| (&candidate.seed, &candidate.trace_sha256, &candidate.dimensions))
            .collect::<Vec<_>>()
    );
    let selected_first = select_pairwise(&first, options.runs);
    let selected_second = select_pairwise(&second, options.runs);
    assert_eq!(selected_first, selected_second);
    assert_eq!(selected_first.selected_indices.len(), options.runs);
}

#[test]
fn greedy_pairwise_selection_maximizes_new_pairs_deterministically() {
    let candidates = vec![
        synthetic_candidate(1, ["a=1|b=1", "a=1|c=1"]),
        synthetic_candidate(2, ["a=1|b=1", "b=1|c=1"]),
        synthetic_candidate(3, ["a=1|c=1", "b=1|c=1"]),
    ];
    let selection = select_pairwise(&candidates, 2);
    assert_eq!(selection.selected_indices, vec![0, 1]);
    assert_eq!(selection.covered_pairs.len(), 3);
    assert!(!selection.uncovered_pairs.is_empty());
}

#[tokio::test]
async fn fail_fast_stops_and_links_a_minimized_replayable_trace() {
    let options = options(3);
    let (candidates, _) = build_candidates(&options).expect("candidates");
    let selection = select_pairwise(&candidates, options.runs);
    let first = &candidates[selection.selected_indices[0]];
    let mut executor = SeedFailureExecutor::stable(first.seed);
    let root = preserved_root("fail-fast");
    let result = run_fault_campaign(options, &root, &mut executor)
        .await
        .expect("campaign report");
    assert_eq!(result.report.executed_runs, 1);
    assert_eq!(result.report.runs[0].status, CampaignRunStatus::Failed);
    assert_eq!(result.report.runs[0].failure_reproduced, Some(true));
    let path = PathBuf::from(
        result.report.runs[0]
            .minimized_scenario_path
            .as_ref()
            .expect("minimized path"),
    );
    let minimized = WithdrawalFaultTrace::from_json(
        &std::fs::read_to_string(&path).expect("read minimized trace"),
    )
    .expect("parse minimized trace");
    assert!(minimized.actions.len() < first.scenario.trace.actions.len());
    assert!(result.report_path.is_file());
}

#[tokio::test]
async fn continue_mode_runs_budget_and_reports_uncovered_pairs() {
    let mut options = options(3);
    options.fail_fast = false;
    let (candidates, _) = build_candidates(&options).expect("candidates");
    let selection = select_pairwise(&candidates, options.runs);
    let failing_seed = candidates[selection.selected_indices[0]].seed;
    let mut executor = SeedFailureExecutor::stable(failing_seed);
    let result = run_fault_campaign(options, &preserved_root("continue"), &mut executor)
        .await
        .expect("campaign report");
    assert_eq!(result.report.executed_runs, 3);
    assert_eq!(
        result
            .report
            .runs
            .iter()
            .filter(|run| run.status == CampaignRunStatus::Failed)
            .count(),
        1
    );
    assert!(!result.report.covered_pairs.is_empty());
    assert!(!result.report.uncovered_pairs.is_empty());
    assert!(result.report.covered_dimensions.contains_key("fault_class"));
}

#[tokio::test]
async fn one_flaky_trace_is_reported_without_false_minimization_claim() {
    let options = options(1);
    let (candidates, _) = build_candidates(&options).expect("candidates");
    let selected = select_pairwise(&candidates, 1);
    let seed = candidates[selected.selected_indices[0]].seed;
    let mut executor = SeedFailureExecutor::flaky(seed);
    let result = run_fault_campaign(options, &preserved_root("flaky"), &mut executor)
        .await
        .expect("campaign report");
    assert_eq!(result.report.runs[0].status, CampaignRunStatus::Failed);
    assert_eq!(result.report.runs[0].failure_reproduced, Some(false));
    assert!(result.report.runs[0].minimized_scenario_path.is_some());
}

#[derive(Default)]
struct SeedFailureExecutor {
    failing_seed: u64,
    flaky: bool,
    attempts: BTreeMap<u64, usize>,
}

impl SeedFailureExecutor {
    fn stable(seed: u64) -> Self {
        Self {
            failing_seed: seed,
            flaky: false,
            attempts: BTreeMap::new(),
        }
    }

    fn flaky(seed: u64) -> Self {
        Self {
            failing_seed: seed,
            flaky: true,
            attempts: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl CampaignExecutor for SeedFailureExecutor {
    async fn execute(
        &mut self,
        seed: u64,
        _trace: &WithdrawalFaultTrace,
    ) -> Result<(), CampaignExecutionFailure> {
        let attempts = self.attempts.entry(seed).or_default();
        *attempts += 1;
        if seed == self.failing_seed && (!self.flaky || *attempts == 1) {
            Err(CampaignExecutionFailure {
                class: "fixture_failure".to_owned(),
                message: "deliberate fixture failure".to_owned(),
            })
        } else {
            Ok(())
        }
    }
}

fn options(runs: usize) -> CampaignOptions {
    CampaignOptions {
        seed: 42,
        runs,
        max_actions: 64,
        action_timeout_ms: 1_000,
        overall_timeout_ms: 60_000,
        negative_action_percent: 25,
        environment_id: "campaign-test".to_owned(),
        backend: "hermetic".to_owned(),
        fail_fast: true,
    }
}

fn synthetic_candidate<const N: usize>(seed: u64, pairs: [&str; N]) -> CampaignCandidate {
    let scenario = generate_scenario(GeneratedScenarioOptions {
        seed,
        max_actions: 64,
        max_runs: 3,
        action_timeout_ms: 1_000,
        overall_timeout_ms: 60_000,
        negative_action_percent: 0,
        environment_id: "synthetic".to_owned(),
        backend: "hermetic".to_owned(),
    })
    .expect("synthetic scenario");
    CampaignCandidate {
        seed,
        scenario,
        dimensions: BTreeSet::from([DimensionValue {
            dimension: "synthetic".to_owned(),
            value: seed.to_string(),
        }]),
        pairs: pairs.into_iter().map(str::to_owned).collect(),
        trace_sha256: seed.to_string(),
    }
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-campaign-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
