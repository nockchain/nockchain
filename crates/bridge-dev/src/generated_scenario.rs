use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::actions::{
    ActionCapability, ActionComponent, ActionEnvironment, ActionExecutionError,
    ExpectedActionOutcome, FaultTraceExecution, WithdrawalActionIntent, WithdrawalActionSpec,
    WithdrawalFaultTrace, FAULT_TRACE_SCHEMA_VERSION,
};
use crate::model::{ModelNoteName, ModelPublicState, WithdrawalModelAction};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedScenarioOptions {
    pub seed: u64,
    pub max_actions: usize,
    pub max_runs: u64,
    pub action_timeout_ms: u64,
    pub overall_timeout_ms: u64,
    pub negative_action_percent: u8,
    pub environment_id: String,
    pub backend: String,
}

impl GeneratedScenarioOptions {
    pub fn validate(&self) -> Result<(), GeneratedScenarioError> {
        if self.max_actions == 0
            || self.max_runs == 0
            || self.action_timeout_ms == 0
            || self.overall_timeout_ms == 0
            || self.action_timeout_ms > self.overall_timeout_ms
            || self.negative_action_percent > 100
            || self.environment_id.trim().is_empty()
            || self.backend.trim().is_empty()
        {
            Err(GeneratedScenarioError::InvalidBudget)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedCoverage {
    pub lifecycle_phases: BTreeMap<String, u64>,
    pub components: BTreeMap<String, u64>,
    pub fault_types: BTreeMap<String, u64>,
    pub recovery_outcomes: BTreeMap<String, u64>,
    pub intentional_precondition_failures: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedScenario {
    pub options: GeneratedScenarioOptions,
    pub trace: WithdrawalFaultTrace,
    pub coverage: GeneratedCoverage,
}

pub fn generate_scenario(
    options: GeneratedScenarioOptions,
) -> Result<GeneratedScenario, GeneratedScenarioError> {
    options.validate()?;
    let mut rng = StdRng::seed_from_u64(options.seed);
    let choices = GenerationChoices {
        intentional_negative: rng.random_range(0_u8..100) < options.negative_action_percent,
        base_reorg: rng.random_bool(0.5),
        process_restart: rng.random_bool(0.5),
        journal_failure: rng.random_bool(0.5),
        partition: rng.random_bool(0.5),
        nock_reinclusion: rng.random_bool(0.5),
        duplicate_observation: rng.random_bool(0.5),
        duplicate_request: rng.random_bool(0.5),
    };
    let mut builder = TraceBuilder::new(&options);
    builder.push(success(WithdrawalActionIntent::Provision { reset: true }));
    if choices.intentional_negative {
        builder.push(expected_precondition(
            "confirm-before-submit",
            WithdrawalActionIntent::ModelTransition {
                transition: WithdrawalModelAction::Confirm {
                    transaction_id: "tx-1".to_owned(),
                    height: 1,
                    block_id: "block-1".to_owned(),
                },
            },
        ));
    }
    builder.push(success(WithdrawalActionIntent::SubmitCanonicalBurn {
        withdrawal_id: "withdrawal-1".to_owned(),
        nonce: 1,
        amount_nicks: "6553600000".to_owned(),
        destination_lock_root: "recipient-root".to_owned(),
    }));
    if choices.base_reorg {
        builder.push_group(vec![
            success(WithdrawalActionIntent::InjectBaseFork {
                depth: 1,
                deep: false,
            }),
            model(WithdrawalModelAction::ReadmitBurn),
        ]);
    }
    if choices.duplicate_observation {
        builder.push(success(WithdrawalActionIntent::DuplicateBaseObservation {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
            block_number: 1,
            times: 2,
        }));
    }
    if choices.duplicate_request {
        builder.push(success(
            WithdrawalActionIntent::DuplicateAuthenticatedRequest {
                method: "bridge.withdrawal_status".to_owned(),
                times: 2,
            },
        ));
    }
    if choices.partition {
        builder.push_group(vec![
            success(WithdrawalActionIntent::PartitionPeers {
                left: BTreeSet::from([0, 1]),
                right: BTreeSet::from([2, 3, 4]),
            }),
            success(WithdrawalActionIntent::HealPeers),
        ]);
    }
    for transition in [
        WithdrawalModelAction::Assemble {
            epoch: 1,
            handoff: 0,
            proposal_hash: "proposal-1".to_owned(),
            selected_inputs: selected_inputs(),
        },
        WithdrawalModelAction::Prepare,
        WithdrawalModelAction::Canonicalize,
        WithdrawalModelAction::Reserve {
            owner: "withdrawal-1".to_owned(),
            inputs: selected_inputs(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Ready,
        },
    ] {
        builder.push(model(transition));
    }
    if choices.process_restart {
        builder.push_group(vec![
            success(WithdrawalActionIntent::Stop {
                component: ActionComponent::Bridge { node_id: 4 },
            }),
            success(WithdrawalActionIntent::Start {
                component: ActionComponent::Bridge { node_id: 4 },
            }),
        ]);
    }
    if choices.journal_failure {
        builder.push_group(vec![
            success(WithdrawalActionIntent::FailJournalEndpoint),
            success(WithdrawalActionIntent::Restart {
                component: ActionComponent::Sequencer,
            }),
            model(WithdrawalModelAction::RestoreReservations),
            success(WithdrawalActionIntent::RecoverJournalEndpoint { generation: 1 }),
        ]);
    }
    for transition in [
        WithdrawalModelAction::Authorize {
            epoch: 1,
            transaction_id: "tx-1".to_owned(),
        },
        WithdrawalModelAction::Submit {
            transaction_id: "tx-1".to_owned(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Submitted,
        },
        WithdrawalModelAction::Include {
            transaction_id: "tx-1".to_owned(),
            height: 10,
            block_id: "block-10".to_owned(),
        },
        WithdrawalModelAction::Confirm {
            transaction_id: "tx-1".to_owned(),
            height: 10,
            block_id: "block-10".to_owned(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::SequencerConfirmed,
        },
    ] {
        builder.push(model(transition));
    }
    if choices.nock_reinclusion {
        builder.push_group(vec![
            success(WithdrawalActionIntent::InjectNockOrphan {
                deep: false,
                reinclusion_height: Some(11),
                reinclusion_block_id: Some("block-11".to_owned()),
            }),
            model(WithdrawalModelAction::Confirm {
                transaction_id: "tx-1".to_owned(),
                height: 11,
                block_id: "block-11".to_owned(),
            }),
            model(WithdrawalModelAction::Publish {
                state: ModelPublicState::SequencerConfirmed,
            }),
        ]);
    }
    for node_id in 0..5 {
        builder.push(model(WithdrawalModelAction::SettleKernel { node_id }));
    }
    for transition in [
        WithdrawalModelAction::RecordPayout,
        WithdrawalModelAction::ReleaseReservations {
            owner: "withdrawal-1".to_owned(),
            inputs: selected_inputs(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Terminal,
        },
    ] {
        builder.push(model(transition));
    }
    builder.push(success(WithdrawalActionIntent::QueryFacts));
    builder.push(success(WithdrawalActionIntent::AssertTerminal));

    let trace = builder.finish();
    trace.validate()?;
    let coverage = coverage(&trace);
    Ok(GeneratedScenario {
        options,
        trace,
        coverage,
    })
}

#[derive(Debug, Clone, Copy)]
struct GenerationChoices {
    intentional_negative: bool,
    base_reorg: bool,
    process_restart: bool,
    journal_failure: bool,
    partition: bool,
    nock_reinclusion: bool,
    duplicate_observation: bool,
    duplicate_request: bool,
}

struct TraceBuilder<'a> {
    options: &'a GeneratedScenarioOptions,
    actions: Vec<WithdrawalActionSpec>,
}

impl<'a> TraceBuilder<'a> {
    fn new(options: &'a GeneratedScenarioOptions) -> Self {
        Self {
            options,
            actions: Vec::with_capacity(options.max_actions),
        }
    }

    fn push(&mut self, action: PendingAction) {
        if self.actions.len() < self.options.max_actions {
            let index = self.actions.len();
            self.actions
                .push(action.into_spec(index, self.options.action_timeout_ms));
        }
    }

    fn push_group(&mut self, actions: Vec<PendingAction>) {
        if self.actions.len() + actions.len() <= self.options.max_actions {
            for action in actions {
                self.push(action);
            }
        }
    }

    fn finish(self) -> WithdrawalFaultTrace {
        WithdrawalFaultTrace {
            schema_version: FAULT_TRACE_SCHEMA_VERSION,
            seed: self.options.seed,
            environment: ActionEnvironment {
                environment_id: self.options.environment_id.clone(),
                backend: self.options.backend.clone(),
                capabilities: BTreeSet::from([
                    ActionCapability::Provision,
                    ActionCapability::Base,
                    ActionCapability::Nockchain,
                    ActionCapability::BridgeProcess,
                    ActionCapability::SequencerProcess,
                    ActionCapability::NetworkPartition,
                    ActionCapability::Journal,
                    ActionCapability::AuthenticatedRpc,
                    ActionCapability::ModelObservation,
                    ActionCapability::TerminalAssertion,
                ]),
            },
            overall_timeout_ms: self.options.overall_timeout_ms,
            actions: self.actions,
        }
    }
}

struct PendingAction {
    label: String,
    expected: ExpectedActionOutcome,
    intent: WithdrawalActionIntent,
}

impl PendingAction {
    fn into_spec(self, index: usize, timeout_ms: u64) -> WithdrawalActionSpec {
        let id_label = self
            .label
            .chars()
            .map(|character| {
                if character.is_ascii_lowercase() || character.is_ascii_digit() {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        WithdrawalActionSpec {
            id: format!("a{index:03}-{id_label}"),
            label: self.label,
            timeout_ms,
            expected: self.expected,
            intent: self.intent,
        }
    }
}

fn success(intent: WithdrawalActionIntent) -> PendingAction {
    PendingAction {
        label: intent_label(&intent).to_owned(),
        expected: ExpectedActionOutcome::Success,
        intent,
    }
}

fn model(transition: WithdrawalModelAction) -> PendingAction {
    PendingAction {
        label: transition.name().to_owned(),
        expected: ExpectedActionOutcome::Success,
        intent: WithdrawalActionIntent::ModelTransition { transition },
    }
}

fn expected_precondition(label: &str, intent: WithdrawalActionIntent) -> PendingAction {
    PendingAction {
        label: label.to_owned(),
        expected: ExpectedActionOutcome::PreconditionFailure {
            code: "precondition".to_owned(),
        },
        intent,
    }
}

fn intent_label(intent: &WithdrawalActionIntent) -> &'static str {
    match intent {
        WithdrawalActionIntent::Provision { .. } => "provision",
        WithdrawalActionIntent::SubmitCanonicalBurn { .. } => "submit-burn",
        WithdrawalActionIntent::InjectBaseFork { .. } => "base-reorg",
        WithdrawalActionIntent::DuplicateBaseObservation { .. } => "duplicate-base-observation",
        WithdrawalActionIntent::PartitionPeers { .. } => "partition-peers",
        WithdrawalActionIntent::HealPeers => "heal-peers",
        WithdrawalActionIntent::Stop { .. } => "stop-component",
        WithdrawalActionIntent::Start { .. } => "start-component",
        WithdrawalActionIntent::Restart { .. } => "restart-component",
        WithdrawalActionIntent::FailJournalEndpoint => "fail-journal",
        WithdrawalActionIntent::RecoverJournalEndpoint { .. } => "recover-journal",
        WithdrawalActionIntent::InjectNockOrphan { .. } => "nock-reorg",
        WithdrawalActionIntent::QueryFacts => "query-facts",
        WithdrawalActionIntent::AssertTerminal => "assert-terminal",
        _ => "generated-action",
    }
}

fn selected_inputs() -> BTreeSet<ModelNoteName> {
    BTreeSet::from([ModelNoteName {
        first: "first-1".to_owned(),
        last: "last-1".to_owned(),
    }])
}

pub fn coverage(trace: &WithdrawalFaultTrace) -> GeneratedCoverage {
    let mut result = GeneratedCoverage::default();
    for action in &trace.actions {
        match &action.intent {
            WithdrawalActionIntent::ModelTransition { transition } => {
                let phase = match transition {
                    WithdrawalModelAction::Assemble { .. } => Some("pending"),
                    WithdrawalModelAction::Canonicalize => Some("ready"),
                    WithdrawalModelAction::Submit { .. } => Some("submitted"),
                    WithdrawalModelAction::Confirm { .. } => Some("sequencer_confirmed"),
                    WithdrawalModelAction::Publish {
                        state: ModelPublicState::Terminal,
                    } => Some("terminal"),
                    _ => None,
                };
                if let Some(phase) = phase {
                    increment(&mut result.lifecycle_phases, phase);
                }
            }
            WithdrawalActionIntent::Stop { component }
            | WithdrawalActionIntent::Start { component }
            | WithdrawalActionIntent::Restart { component } => {
                increment(&mut result.components, &component.label());
            }
            WithdrawalActionIntent::InjectBaseFork { .. } => {
                increment(&mut result.fault_types, "base_reorg")
            }
            WithdrawalActionIntent::InjectNockOrphan { .. } => {
                increment(&mut result.fault_types, "nock_reorg")
            }
            WithdrawalActionIntent::PartitionPeers { .. } => {
                increment(&mut result.fault_types, "partition")
            }
            WithdrawalActionIntent::FailJournalEndpoint => {
                increment(&mut result.fault_types, "journal")
            }
            WithdrawalActionIntent::DuplicateAuthenticatedRequest { .. } => {
                increment(&mut result.fault_types, "authenticated_retry")
            }
            WithdrawalActionIntent::HealPeers
            | WithdrawalActionIntent::RecoverJournalEndpoint { .. } => {
                increment(&mut result.recovery_outcomes, "recovered")
            }
            _ => {}
        }
        if matches!(
            action.expected,
            ExpectedActionOutcome::PreconditionFailure { .. }
        ) {
            result.intentional_precondition_failures += 1;
        }
    }
    result
}

fn increment(counts: &mut BTreeMap<String, u64>, key: &str) {
    *counts.entry(key.to_owned()).or_insert(0) += 1;
}

pub fn shrink_failing_trace<F>(
    trace: &WithdrawalFaultTrace,
    mut still_fails: F,
) -> Result<WithdrawalFaultTrace, GeneratedScenarioError>
where
    F: FnMut(&WithdrawalFaultTrace) -> bool,
{
    trace.validate()?;
    if !still_fails(trace) {
        return Err(GeneratedScenarioError::FailureDoesNotReproduce);
    }
    let mut minimized = trace.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for index in (1..minimized.actions.len()).rev() {
            let mut candidate = minimized.clone();
            candidate.actions.remove(index);
            if candidate.validate().is_ok() && still_fails(&candidate) {
                minimized = candidate;
                changed = true;
            }
        }
    }
    Ok(minimized)
}

pub fn write_minimized_trace(
    path: &Path,
    trace: &WithdrawalFaultTrace,
) -> Result<PathBuf, GeneratedScenarioError> {
    trace.validate()?;
    if path.exists() {
        return Err(GeneratedScenarioError::RefuseOverwrite(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(trace)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(path.to_path_buf())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedFailureClass {
    Precondition,
    Infrastructure,
    Timeout,
    Invariant,
}

pub fn classify_execution_error(error: &ActionExecutionError) -> GeneratedFailureClass {
    match error {
        ActionExecutionError::Schema(_)
        | ActionExecutionError::Model { .. }
        | ActionExecutionError::UnexpectedSuccess(_) => GeneratedFailureClass::Precondition,
        ActionExecutionError::Sut { .. } => GeneratedFailureClass::Infrastructure,
        ActionExecutionError::Timeout(_) => GeneratedFailureClass::Timeout,
        ActionExecutionError::InvariantMismatch { .. } | ActionExecutionError::ModelState(_) => {
            GeneratedFailureClass::Invariant
        }
    }
}

pub fn execution_coverage(execution: &FaultTraceExecution) -> BTreeMap<String, u64> {
    let mut result = BTreeMap::new();
    for action in &execution.actions {
        increment(&mut result, &action.status);
    }
    result
}

#[derive(Debug, Error)]
pub enum GeneratedScenarioError {
    #[error("invalid generated scenario budget")]
    InvalidBudget,
    #[error("failing trace predicate does not reproduce")]
    FailureDoesNotReproduce,
    #[error("refusing to overwrite minimized trace {0}")]
    RefuseOverwrite(PathBuf),
    #[error(transparent)]
    Schema(#[from] crate::actions::ActionSchemaError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Filesystem(#[from] std::io::Error),
}
