use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::actions::{ActionComponent, WithdrawalActionIntent, WithdrawalFaultTrace};
use crate::generated_scenario::{
    generate_scenario, write_minimized_trace, GeneratedScenario, GeneratedScenarioOptions,
};
use crate::model::{ModelPublicState, WithdrawalModelAction};

pub const FAULT_CAMPAIGN_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DimensionValue {
    pub dimension: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct CampaignCandidate {
    pub seed: u64,
    pub scenario: GeneratedScenario,
    pub dimensions: BTreeSet<DimensionValue>,
    pub pairs: BTreeSet<String>,
    pub trace_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignSelection {
    pub selected_indices: Vec<usize>,
    pub covered_pairs: BTreeSet<String>,
    pub uncovered_pairs: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignOptions {
    pub seed: u64,
    pub runs: usize,
    pub max_actions: usize,
    pub action_timeout_ms: u64,
    pub overall_timeout_ms: u64,
    pub negative_action_percent: u8,
    pub environment_id: String,
    pub backend: String,
    pub fail_fast: bool,
}

impl CampaignOptions {
    pub fn validate(&self) -> Result<(), FaultCampaignError> {
        if self.runs == 0
            || self.max_actions == 0
            || self.action_timeout_ms == 0
            || self.overall_timeout_ms == 0
            || self.action_timeout_ms > self.overall_timeout_ms
            || self.negative_action_percent > 100
            || self.environment_id.trim().is_empty()
            || !matches!(self.backend.as_str(), "hermetic" | "base-sepolia-fork")
        {
            return Err(FaultCampaignError::InvalidOptions);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignRunStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignRunRecord {
    pub seed: u64,
    pub trace_sha256: String,
    pub status: CampaignRunStatus,
    pub duration_ms: u64,
    pub failure_class: Option<String>,
    pub failure_message: Option<String>,
    pub failure_reproduced: Option<bool>,
    pub minimized_scenario_path: Option<String>,
    pub dimensions: BTreeSet<DimensionValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultCampaignReport {
    pub schema_version: u64,
    pub seed: u64,
    pub requested_runs: usize,
    pub selected_runs: usize,
    pub executed_runs: usize,
    pub max_actions: usize,
    pub backend: String,
    pub fail_fast: bool,
    pub total_duration_ms: u64,
    pub covered_dimensions: BTreeMap<String, BTreeSet<String>>,
    pub covered_pairs: BTreeSet<String>,
    pub uncovered_pairs: BTreeSet<String>,
    pub duplicate_equivalent_candidates: usize,
    pub runs: Vec<CampaignRunRecord>,
}

#[derive(Debug, Clone)]
pub struct CampaignResult {
    pub report: FaultCampaignReport,
    pub report_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignExecutionFailure {
    pub class: String,
    pub message: String,
}

#[async_trait]
pub trait CampaignExecutor: Send {
    async fn execute(
        &mut self,
        seed: u64,
        trace: &WithdrawalFaultTrace,
    ) -> Result<(), CampaignExecutionFailure>;
}

pub fn build_candidates(
    options: &CampaignOptions,
) -> Result<(Vec<CampaignCandidate>, usize), FaultCampaignError> {
    options.validate()?;
    let candidate_budget = options.runs.saturating_mul(8).max(options.runs);
    let attempt_budget = candidate_budget.saturating_mul(64).max(64);
    let mut rng = StdRng::seed_from_u64(options.seed);
    let mut seeds = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    let mut candidates = Vec::with_capacity(candidate_budget);
    let mut duplicate_equivalent = 0usize;
    let mut duplicate_streak = 0usize;
    for _ in 0..attempt_budget {
        if candidates.len() >= candidate_budget {
            break;
        }
        let seed = rng.random::<u64>();
        if !seeds.insert(seed) {
            continue;
        }
        let scenario = generate_scenario(GeneratedScenarioOptions {
            seed,
            max_actions: options.max_actions,
            max_runs: options.runs as u64,
            action_timeout_ms: options.action_timeout_ms,
            overall_timeout_ms: options.overall_timeout_ms,
            negative_action_percent: options.negative_action_percent,
            environment_id: options.environment_id.clone(),
            backend: options.backend.clone(),
        })?;
        let mut normalized_trace = scenario.trace.clone();
        normalized_trace.seed = 0;
        let bytes = serde_json::to_vec(&normalized_trace)?;
        let trace_sha256 = hex::encode(Sha256::digest(bytes));
        if !hashes.insert(trace_sha256.clone()) {
            duplicate_equivalent = duplicate_equivalent.saturating_add(1);
            duplicate_streak = duplicate_streak.saturating_add(1);
            if duplicate_streak >= 1_024 {
                break;
            }
            continue;
        }
        duplicate_streak = 0;
        let dimensions = trace_dimensions(&scenario.trace);
        let pairs = dimension_pairs(&dimensions);
        candidates.push(CampaignCandidate {
            seed,
            scenario,
            dimensions,
            pairs,
            trace_sha256,
        });
    }
    if candidates.is_empty() {
        return Err(FaultCampaignError::NoCandidates);
    }
    Ok((candidates, duplicate_equivalent))
}

pub fn select_pairwise(candidates: &[CampaignCandidate], runs: usize) -> CampaignSelection {
    let expected = expected_pair_universe();
    let mut remaining = (0..candidates.len()).collect::<BTreeSet<_>>();
    let mut selected = Vec::new();
    let mut covered = BTreeSet::new();
    while selected.len() < runs && !remaining.is_empty() {
        let Some(best) = remaining.iter().copied().max_by(|left, right| {
            let left_gain = candidates[*left].pairs.difference(&covered).count();
            let right_gain = candidates[*right].pairs.difference(&covered).count();
            left_gain
                .cmp(&right_gain)
                .then_with(|| candidates[*right].seed.cmp(&candidates[*left].seed))
        }) else {
            break;
        };
        remaining.remove(&best);
        covered.extend(candidates[best].pairs.iter().cloned());
        selected.push(best);
    }
    CampaignSelection {
        selected_indices: selected,
        covered_pairs: covered.clone(),
        uncovered_pairs: expected.difference(&covered).cloned().collect(),
    }
}

pub async fn run_fault_campaign<E: CampaignExecutor>(
    options: CampaignOptions,
    output_root: &Path,
    executor: &mut E,
) -> Result<CampaignResult, FaultCampaignError> {
    let started = Instant::now();
    let (candidates, duplicate_equivalent_candidates) = build_candidates(&options)?;
    let selection = select_pairwise(&candidates, options.runs);
    let run_dir = create_campaign_dir(output_root)?;
    let mut records = Vec::with_capacity(selection.selected_indices.len());
    let mut covered_dimensions = BTreeMap::<String, BTreeSet<String>>::new();
    for index in &selection.selected_indices {
        let candidate = &candidates[*index];
        for value in &candidate.dimensions {
            covered_dimensions
                .entry(value.dimension.clone())
                .or_default()
                .insert(value.value.clone());
        }
        let run_started = Instant::now();
        match executor
            .execute(candidate.seed, &candidate.scenario.trace)
            .await
        {
            Ok(()) => records.push(CampaignRunRecord {
                seed: candidate.seed,
                trace_sha256: candidate.trace_sha256.clone(),
                status: CampaignRunStatus::Passed,
                duration_ms: duration_ms(run_started.elapsed()),
                failure_class: None,
                failure_message: None,
                failure_reproduced: None,
                minimized_scenario_path: None,
                dimensions: candidate.dimensions.clone(),
            }),
            Err(failure) => {
                let minimized = minimize_failure(
                    executor, candidate.seed, &candidate.scenario.trace, &failure.class,
                )
                .await;
                let path = run_dir.join(format!("failure-{}.json", candidate.seed));
                write_minimized_trace(&path, &minimized.trace)?;
                records.push(CampaignRunRecord {
                    seed: candidate.seed,
                    trace_sha256: candidate.trace_sha256.clone(),
                    status: CampaignRunStatus::Failed,
                    duration_ms: duration_ms(run_started.elapsed()),
                    failure_class: Some(failure.class),
                    failure_message: Some(failure.message),
                    failure_reproduced: Some(minimized.reproduced),
                    minimized_scenario_path: Some(path.display().to_string()),
                    dimensions: candidate.dimensions.clone(),
                });
                if options.fail_fast {
                    break;
                }
            }
        }
    }
    let report = FaultCampaignReport {
        schema_version: FAULT_CAMPAIGN_SCHEMA_VERSION,
        seed: options.seed,
        requested_runs: options.runs,
        selected_runs: selection.selected_indices.len(),
        executed_runs: records.len(),
        max_actions: options.max_actions,
        backend: options.backend,
        fail_fast: options.fail_fast,
        total_duration_ms: duration_ms(started.elapsed()),
        covered_dimensions,
        covered_pairs: selection.covered_pairs,
        uncovered_pairs: selection.uncovered_pairs,
        duplicate_equivalent_candidates,
        runs: records,
    };
    let report_path = run_dir.join("campaign-report.json");
    write_new_json(&report_path, &report)?;
    Ok(CampaignResult {
        report,
        report_path,
    })
}

struct MinimizedFailure {
    trace: WithdrawalFaultTrace,
    reproduced: bool,
}

async fn minimize_failure<E: CampaignExecutor>(
    executor: &mut E,
    seed: u64,
    trace: &WithdrawalFaultTrace,
    failure_class: &str,
) -> MinimizedFailure {
    if !executor
        .execute(seed, trace)
        .await
        .is_err_and(|failure| failure.class == failure_class)
    {
        return MinimizedFailure {
            trace: trace.clone(),
            reproduced: false,
        };
    }
    let mut minimized = trace.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for index in (1..minimized.actions.len()).rev() {
            let mut candidate = minimized.clone();
            candidate.actions.remove(index);
            if candidate.validate().is_err() {
                continue;
            }
            if executor
                .execute(seed, &candidate)
                .await
                .is_err_and(|failure| failure.class == failure_class)
            {
                minimized = candidate;
                changed = true;
            }
        }
    }
    MinimizedFailure {
        trace: minimized,
        reproduced: true,
    }
}

pub fn trace_dimensions(trace: &WithdrawalFaultTrace) -> BTreeSet<DimensionValue> {
    let mut values = BTreeSet::new();
    let mut phase = "pre_burn";
    let mut boundary = "pre_authorization";
    for action in &trace.actions {
        let fault = fault_dimensions(&action.intent);
        if let Some((actor, class)) = fault {
            insert_dimension(&mut values, "lifecycle_phase", phase);
            insert_dimension(&mut values, "actor", actor);
            insert_dimension(&mut values, "fault_class", class);
            insert_dimension(&mut values, "safety_boundary", boundary);
        }
        match action.intent.model_action() {
            Some(WithdrawalModelAction::ObserveBurn { .. }) => phase = "pending",
            Some(WithdrawalModelAction::Canonicalize) => phase = "ready",
            Some(WithdrawalModelAction::Authorize { .. }) => boundary = "post_authorization",
            Some(WithdrawalModelAction::Submit { .. }) => phase = "submitted",
            Some(WithdrawalModelAction::Confirm { .. }) => phase = "sequencer_confirmed",
            Some(WithdrawalModelAction::Publish {
                state: ModelPublicState::Terminal,
            }) => phase = "terminal",
            _ => {}
        }
        match &action.intent {
            WithdrawalActionIntent::HealPeers
            | WithdrawalActionIntent::RecoverJournalEndpoint { .. } => {
                insert_dimension(&mut values, "recovery_outcome", "recovered")
            }
            WithdrawalActionIntent::InjectNockOrphan {
                reinclusion_height: Some(_),
                reinclusion_block_id: Some(_),
                ..
            } => insert_dimension(&mut values, "recovery_outcome", "recovered"),
            WithdrawalActionIntent::InjectBaseFork { deep: true, .. }
            | WithdrawalActionIntent::InjectNockOrphan { deep: true, .. } => {
                insert_dimension(&mut values, "recovery_outcome", "held")
            }
            WithdrawalActionIntent::AssertTerminal => {
                insert_dimension(&mut values, "recovery_outcome", "terminal")
            }
            _ => {}
        }
    }
    values
}

fn fault_dimensions(intent: &WithdrawalActionIntent) -> Option<(&'static str, &'static str)> {
    match intent {
        WithdrawalActionIntent::Restart { component } => Some((
            match component {
                ActionComponent::Bridge { .. } => "bridge",
                ActionComponent::Sequencer => "sequencer",
                ActionComponent::NockchainNode => "nockchain",
            },
            "restart",
        )),
        WithdrawalActionIntent::PartitionPeers { .. } => Some(("network", "partition")),
        WithdrawalActionIntent::DuplicateAuthenticatedRequest { .. }
        | WithdrawalActionIntent::DuplicateBaseObservation { .. } => Some(("base", "duplicate")),
        WithdrawalActionIntent::FailJournalEndpoint => Some(("journal", "failure")),
        WithdrawalActionIntent::InjectBaseFork { .. } => Some(("base", "reorg")),
        WithdrawalActionIntent::InjectNockOrphan { .. } => Some(("nockchain", "reorg")),
        _ => None,
    }
}

fn dimension_pairs(values: &BTreeSet<DimensionValue>) -> BTreeSet<String> {
    let values = values.iter().collect::<Vec<_>>();
    let mut pairs = BTreeSet::new();
    for left in 0..values.len() {
        for right in (left + 1)..values.len() {
            if values[left].dimension != values[right].dimension {
                pairs.insert(pair_key(values[left], values[right]));
            }
        }
    }
    pairs
}

fn expected_pair_universe() -> BTreeSet<String> {
    let domains = BTreeMap::from([
        (
            "lifecycle_phase",
            vec!["pending", "ready", "submitted", "sequencer_confirmed"],
        ),
        (
            "actor",
            vec!["bridge", "sequencer", "base", "nockchain", "journal", "network"],
        ),
        (
            "fault_class",
            vec!["restart", "partition", "duplicate", "failure", "reorg"],
        ),
        ("recovery_outcome", vec!["recovered", "held", "terminal"]),
        (
            "safety_boundary",
            vec!["pre_authorization", "post_authorization"],
        ),
    ]);
    let dimensions = domains.keys().copied().collect::<Vec<_>>();
    let mut pairs = BTreeSet::new();
    for left in 0..dimensions.len() {
        for right in (left + 1)..dimensions.len() {
            for left_value in &domains[dimensions[left]] {
                for right_value in &domains[dimensions[right]] {
                    pairs.insert(pair_key(
                        &DimensionValue {
                            dimension: dimensions[left].to_owned(),
                            value: (*left_value).to_owned(),
                        },
                        &DimensionValue {
                            dimension: dimensions[right].to_owned(),
                            value: (*right_value).to_owned(),
                        },
                    ));
                }
            }
        }
    }
    pairs
}

fn pair_key(left: &DimensionValue, right: &DimensionValue) -> String {
    if left.dimension <= right.dimension {
        format!(
            "{}={}|{}={}",
            left.dimension, left.value, right.dimension, right.value
        )
    } else {
        format!(
            "{}={}|{}={}",
            right.dimension, right.value, left.dimension, left.value
        )
    }
}

fn insert_dimension(values: &mut BTreeSet<DimensionValue>, dimension: &str, value: &str) {
    values.insert(DimensionValue {
        dimension: dimension.to_owned(),
        value: value.to_owned(),
    });
}

fn create_campaign_dir(root: &Path) -> Result<PathBuf, FaultCampaignError> {
    fs::create_dir_all(root)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| FaultCampaignError::Clock)?;
    let path = root.join(format!(
        "campaign-{}-{}",
        now.as_millis(),
        std::process::id()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), FaultCampaignError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Error)]
pub enum FaultCampaignError {
    #[error("invalid fault campaign options")]
    InvalidOptions,
    #[error("system clock precedes Unix epoch")]
    Clock,
    #[error(transparent)]
    Generated(#[from] crate::generated_scenario::GeneratedScenarioError),
    #[error("fault generator produced no distinct candidate traces")]
    NoCandidates,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Filesystem(#[from] std::io::Error),
}
