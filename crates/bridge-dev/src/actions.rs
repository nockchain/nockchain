use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::time::timeout;

use crate::model::{WithdrawalModelAction, WithdrawalModelError, WithdrawalModelState};

pub const FAULT_TRACE_SCHEMA_VERSION: u64 = 1;
pub const ACTION_SCENARIO_SCHEMA_ID: &str = "nockchain.bridge.withdrawal-action-scenario";
pub const ACTION_SCENARIO_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionCapability {
    Provision,
    Base,
    Nockchain,
    BridgeProcess,
    SequencerProcess,
    NetworkPartition,
    Journal,
    AuthenticatedRpc,
    ModelObservation,
    TerminalAssertion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionEnvironment {
    pub environment_id: String,
    pub backend: String,
    pub capabilities: BTreeSet<ActionCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalFaultTrace {
    pub schema_version: u64,
    pub seed: u64,
    pub environment: ActionEnvironment,
    pub overall_timeout_ms: u64,
    pub actions: Vec<WithdrawalActionSpec>,
}

impl WithdrawalFaultTrace {
    pub fn from_json(input: &str) -> Result<Self, ActionSchemaError> {
        let trace: Self = serde_json::from_str(input)?;
        trace.validate()?;
        Ok(trace)
    }

    pub fn from_yaml(input: &str) -> Result<Self, ActionSchemaError> {
        let trace: Self = serde_yaml::from_str(input)
            .map_err(|error| ActionSchemaError::Yaml(error.to_string()))?;
        trace.validate()?;
        Ok(trace)
    }

    pub fn to_yaml(&self) -> Result<String, ActionSchemaError> {
        serde_yaml::to_string(self).map_err(|error| ActionSchemaError::Yaml(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), ActionSchemaError> {
        if self.schema_version != FAULT_TRACE_SCHEMA_VERSION {
            return Err(ActionSchemaError::UnsupportedSchema(self.schema_version));
        }
        if self.environment.environment_id.trim().is_empty()
            || self.environment.backend.trim().is_empty()
            || self.overall_timeout_ms == 0
            || self.actions.is_empty()
        {
            return Err(ActionSchemaError::InvalidTrace(
                "environment, timeout, and actions are required",
            ));
        }
        let mut ids = HashSet::with_capacity(self.actions.len());
        let mut process = ValidationProcessState::default();
        let mut model = WithdrawalModelState::default();
        for (index, action) in self.actions.iter().enumerate() {
            validate_action_id(&action.id)?;
            if !ids.insert(action.id.clone()) {
                return Err(ActionSchemaError::DuplicateActionId(action.id.clone()));
            }
            if action.timeout_ms == 0 || action.timeout_ms > self.overall_timeout_ms {
                return Err(ActionSchemaError::InvalidAction {
                    id: action.id.clone(),
                    reason: "action timeout is zero or exceeds overall timeout",
                });
            }
            let capability = action.intent.capability();
            if !self.environment.capabilities.contains(&capability) {
                return Err(ActionSchemaError::UnsupportedAction {
                    id: action.id.clone(),
                    capability,
                    backend: self.environment.backend.clone(),
                });
            }
            let process_before = process.clone();
            if let Err(error) = process.validate_and_apply(index, action) {
                if matches!(
                    action.expected,
                    ExpectedActionOutcome::PreconditionFailure { .. }
                ) {
                    process = process_before;
                    continue;
                }
                return Err(error);
            }
            if let Some(model_action) = action.intent.model_action() {
                let result = model.apply(&model_action);
                match (&action.expected, result) {
                    (ExpectedActionOutcome::Success, Ok(_)) => {}
                    (ExpectedActionOutcome::PreconditionFailure { .. }, Err(WithdrawalModelError::Precondition { .. })) => {}
                    (ExpectedActionOutcome::PreconditionFailure { .. }, Ok(_)) => {
                        return Err(ActionSchemaError::InvalidAction {
                            id: action.id.clone(),
                            reason: "action expects a model precondition failure but transition is legal",
                        })
                    }
                    (_, Err(error)) => {
                        return Err(ActionSchemaError::ModelPrecondition {
                            id: action.id.clone(),
                            reason: error.to_string(),
                        })
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionScenarioProvenance {
    pub source_kind: String,
    pub source_path: String,
    pub source_sha256: String,
    pub property: String,
    pub counterexample_id: String,
    pub model_schema_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalActionScenarioV1 {
    pub schema_id: String,
    pub schema_version: u64,
    pub provenance: ActionScenarioProvenance,
    pub trace: WithdrawalFaultTrace,
}

impl WithdrawalActionScenarioV1 {
    pub fn new(provenance: ActionScenarioProvenance, trace: WithdrawalFaultTrace) -> Self {
        Self {
            schema_id: ACTION_SCENARIO_SCHEMA_ID.to_owned(),
            schema_version: ACTION_SCENARIO_SCHEMA_VERSION,
            provenance,
            trace,
        }
    }

    pub fn from_json(input: &str) -> Result<Self, ActionSchemaError> {
        let scenario: Self = serde_json::from_str(input)?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn validate(&self) -> Result<(), ActionSchemaError> {
        if self.schema_id != ACTION_SCENARIO_SCHEMA_ID
            || self.schema_version != ACTION_SCENARIO_SCHEMA_VERSION
        {
            return Err(ActionSchemaError::InvalidTrace(
                "unsupported action scenario envelope",
            ));
        }
        if self.provenance.source_kind.trim().is_empty()
            || self.provenance.source_path.trim().is_empty()
            || self.provenance.source_sha256.len() != 64
            || !self
                .provenance
                .source_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.provenance.property.trim().is_empty()
            || self.provenance.counterexample_id.trim().is_empty()
            || self.provenance.model_schema_version == 0
        {
            return Err(ActionSchemaError::InvalidTrace(
                "action scenario provenance is invalid",
            ));
        }
        self.trace.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalActionSpec {
    pub id: String,
    pub label: String,
    pub timeout_ms: u64,
    pub expected: ExpectedActionOutcome,
    pub intent: WithdrawalActionIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedActionOutcome {
    Success,
    PreconditionFailure { code: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "component", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionComponent {
    Bridge { node_id: u64 },
    Sequencer,
    NockchainNode,
}

impl ActionComponent {
    pub fn label(&self) -> String {
        match self {
            Self::Bridge { node_id } => format!("bridge-{node_id}"),
            Self::Sequencer => "sequencer".to_owned(),
            Self::NockchainNode => "node".to_owned(),
        }
    }

    fn validate(&self, action_id: &str) -> Result<(), ActionSchemaError> {
        if matches!(self, Self::Bridge { node_id } if *node_id >= 5) {
            Err(ActionSchemaError::InvalidAction {
                id: action_id.to_owned(),
                reason: "bridge node id must be 0..4",
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleObservation {
    Pending,
    Ready,
    Submitted,
    SequencerConfirmed,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalFailurePoint {
    AppendRemoteBeforeLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WithdrawalActionIntent {
    Provision {
        reset: bool,
    },
    Reset,
    SubmitCanonicalBurn {
        withdrawal_id: String,
        nonce: u64,
        amount_nicks: String,
        destination_lock_root: String,
    },
    MineBase {
        blocks: u64,
    },
    WaitLifecycle {
        phase: LifecycleObservation,
    },
    ObserveLifecycle {
        phase: LifecycleObservation,
    },
    Stop {
        component: ActionComponent,
    },
    Start {
        component: ActionComponent,
    },
    Restart {
        component: ActionComponent,
    },
    PartitionPeers {
        left: BTreeSet<u64>,
        right: BTreeSet<u64>,
    },
    HealPeers,
    DuplicateAuthenticatedRequest {
        method: String,
        times: u64,
    },
    DuplicateBaseObservation {
        withdrawal_id: String,
        nonce: u64,
        block_number: u64,
        times: u64,
    },
    FailJournalEndpoint,
    RecoverJournalEndpoint {
        generation: u64,
    },
    SetJournalFailurePoint {
        point: JournalFailurePoint,
    },
    InjectBaseFork {
        depth: u64,
        deep: bool,
    },
    InjectNockOrphan {
        deep: bool,
        reinclusion_height: Option<u64>,
        reinclusion_block_id: Option<String>,
    },
    AdvanceNockchain {
        blocks: u64,
    },
    AdvanceObserverFrontiers {
        blocks: u64,
    },
    ModelTransition {
        transition: WithdrawalModelAction,
    },
    QueryFacts,
    AssertModelInvariant {
        invariant: String,
    },
    AssertTerminal,
}

impl WithdrawalActionIntent {
    pub fn capability(&self) -> ActionCapability {
        match self {
            Self::Provision { .. } | Self::Reset => ActionCapability::Provision,
            Self::SubmitCanonicalBurn { .. }
            | Self::MineBase { .. }
            | Self::DuplicateBaseObservation { .. }
            | Self::InjectBaseFork { .. } => ActionCapability::Base,
            Self::WaitLifecycle { .. }
            | Self::ObserveLifecycle { .. }
            | Self::AdvanceObserverFrontiers { .. }
            | Self::QueryFacts => ActionCapability::ModelObservation,
            Self::Stop { component } | Self::Start { component } | Self::Restart { component } => {
                match component {
                    ActionComponent::Bridge { .. } => ActionCapability::BridgeProcess,
                    ActionComponent::Sequencer => ActionCapability::SequencerProcess,
                    ActionComponent::NockchainNode => ActionCapability::Nockchain,
                }
            }
            Self::PartitionPeers { .. } | Self::HealPeers => ActionCapability::NetworkPartition,
            Self::DuplicateAuthenticatedRequest { .. } => ActionCapability::AuthenticatedRpc,
            Self::FailJournalEndpoint
            | Self::RecoverJournalEndpoint { .. }
            | Self::SetJournalFailurePoint { .. } => ActionCapability::Journal,
            Self::InjectNockOrphan { .. } | Self::AdvanceNockchain { .. } => {
                ActionCapability::Nockchain
            }
            Self::ModelTransition { .. } | Self::AssertModelInvariant { .. } => {
                ActionCapability::ModelObservation
            }
            Self::AssertTerminal => ActionCapability::TerminalAssertion,
        }
    }

    pub fn model_action(&self) -> Option<WithdrawalModelAction> {
        match self {
            Self::SubmitCanonicalBurn {
                withdrawal_id,
                nonce,
                ..
            }
            | Self::DuplicateBaseObservation {
                withdrawal_id,
                nonce,
                ..
            } => Some(WithdrawalModelAction::ObserveBurn {
                withdrawal_id: withdrawal_id.clone(),
                nonce: *nonce,
            }),
            Self::Restart { component } => Some(WithdrawalModelAction::Restart {
                component: component.label(),
            }),
            Self::RecoverJournalEndpoint { generation } => {
                Some(WithdrawalModelAction::ReplayJournal {
                    generation: *generation,
                })
            }
            Self::InjectBaseFork { deep, .. } => {
                Some(WithdrawalModelAction::BaseReorg { deep: *deep })
            }
            Self::InjectNockOrphan {
                deep,
                reinclusion_height,
                reinclusion_block_id,
            } => Some(WithdrawalModelAction::NockReorg {
                deep: *deep,
                reinclusion_height: *reinclusion_height,
                reinclusion_block_id: reinclusion_block_id.clone(),
            }),
            Self::ModelTransition { transition } => Some(transition.clone()),
            _ => None,
        }
    }

    pub fn is_alignment_barrier(&self) -> bool {
        matches!(
            self,
            Self::WaitLifecycle { .. }
                | Self::ObserveLifecycle { .. }
                | Self::ModelTransition { .. }
                | Self::QueryFacts
                | Self::AssertModelInvariant { .. }
                | Self::AssertTerminal
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSutResult {
    pub status: String,
    pub detail: Option<Value>,
}

#[async_trait]
pub trait WithdrawalActionSut: Send {
    async fn execute_action(
        &mut self,
        action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String>;

    async fn observe_model_state(&mut self) -> Result<Option<WithdrawalModelState>, String> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionExecutionRecord {
    pub id: String,
    pub label: String,
    pub status: String,
    pub model_state_sha256: String,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultTraceExecution {
    pub schema_version: u64,
    pub seed: u64,
    pub final_model_state_sha256: String,
    pub actions: Vec<ActionExecutionRecord>,
}

pub async fn execute_fault_trace<S: WithdrawalActionSut>(
    trace: &WithdrawalFaultTrace,
    sut: &mut S,
) -> Result<FaultTraceExecution, ActionExecutionError> {
    trace.validate()?;
    timeout(
        Duration::from_millis(trace.overall_timeout_ms),
        execute_validated_fault_trace(trace, sut),
    )
    .await
    .map_err(|_| ActionExecutionError::Timeout("overall trace".to_owned()))?
}

async fn execute_validated_fault_trace<S: WithdrawalActionSut>(
    trace: &WithdrawalFaultTrace,
    sut: &mut S,
) -> Result<FaultTraceExecution, ActionExecutionError> {
    let mut model = WithdrawalModelState::default();
    let mut records = Vec::with_capacity(trace.actions.len());
    for action in &trace.actions {
        let before = model.clone();
        let model_result = action
            .intent
            .model_action()
            .map(|transition| model.apply(&transition));
        if let Some(Err(error)) = &model_result {
            model = before;
            if matches!(
                action.expected,
                ExpectedActionOutcome::PreconditionFailure { .. }
            ) && matches!(error, WithdrawalModelError::Precondition { .. })
            {
                records.push(ActionExecutionRecord {
                    id: action.id.clone(),
                    label: action.label.clone(),
                    status: "expected_precondition_failure".to_owned(),
                    model_state_sha256: model.state_sha256()?,
                    detail: Some(serde_json::json!({"model_error": error.to_string()})),
                });
                continue;
            }
            return Err(ActionExecutionError::Model {
                id: action.id.clone(),
                reason: error.to_string(),
            });
        }
        let result = timeout(
            Duration::from_millis(action.timeout_ms),
            sut.execute_action(action),
        )
        .await
        .map_err(|_| ActionExecutionError::Timeout(action.id.clone()))?;
        match (result, &action.expected) {
            (Ok(result), ExpectedActionOutcome::Success) => {
                if action.intent.is_alignment_barrier() {
                    if let Some(observed) = timeout(
                        Duration::from_millis(action.timeout_ms),
                        sut.observe_model_state(),
                    )
                    .await
                    .map_err(|_| ActionExecutionError::Timeout(action.id.clone()))?
                    .map_err(|reason| ActionExecutionError::Sut {
                        id: action.id.clone(),
                        reason,
                    })? {
                        if observed != model {
                            return Err(ActionExecutionError::InvariantMismatch {
                                id: action.id.clone(),
                                expected: model.state_sha256()?,
                                observed: observed.state_sha256()?,
                            });
                        }
                    }
                }
                records.push(ActionExecutionRecord {
                    id: action.id.clone(),
                    label: action.label.clone(),
                    status: result.status,
                    model_state_sha256: model.state_sha256()?,
                    detail: result.detail,
                });
            }
            (Err(reason), ExpectedActionOutcome::PreconditionFailure { code })
                if reason.contains(code) =>
            {
                model = before;
                records.push(ActionExecutionRecord {
                    id: action.id.clone(),
                    label: action.label.clone(),
                    status: "expected_precondition_failure".to_owned(),
                    model_state_sha256: model.state_sha256()?,
                    detail: Some(serde_json::json!({"sut_error": reason})),
                });
            }
            (Err(reason), _) => {
                return Err(ActionExecutionError::Sut {
                    id: action.id.clone(),
                    reason,
                });
            }
            (Ok(_), ExpectedActionOutcome::PreconditionFailure { .. }) => {
                return Err(ActionExecutionError::UnexpectedSuccess(action.id.clone()));
            }
        }
    }
    Ok(FaultTraceExecution {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: trace.seed,
        final_model_state_sha256: model.state_sha256()?,
        actions: records,
    })
}

#[derive(Clone, Default)]
struct ValidationProcessState {
    provisioned: bool,
    terminal_asserted: bool,
    partitioned: bool,
    journal_failed: bool,
    components: BTreeMap<ActionComponent, bool>,
}

impl ValidationProcessState {
    fn validate_and_apply(
        &mut self,
        index: usize,
        action: &WithdrawalActionSpec,
    ) -> Result<(), ActionSchemaError> {
        match &action.intent {
            WithdrawalActionIntent::Provision { .. } => {
                if index != 0 || self.provisioned {
                    return invalid_action(
                        action, "provision must be the first and only provision",
                    );
                }
                self.provisioned = true;
                self.components.insert(ActionComponent::Sequencer, true);
                self.components.insert(ActionComponent::NockchainNode, true);
                for node_id in 0..5 {
                    self.components
                        .insert(ActionComponent::Bridge { node_id }, true);
                }
            }
            _ if !self.provisioned => {
                return invalid_action(action, "action requires prior provision")
            }
            WithdrawalActionIntent::Stop { component } => {
                component.validate(&action.id)?;
                if self.components.get(component) != Some(&true) {
                    return invalid_action(action, "component is unavailable or already stopped");
                }
                self.components.insert(component.clone(), false);
            }
            WithdrawalActionIntent::Start { component } => {
                component.validate(&action.id)?;
                if self.components.get(component) != Some(&false) {
                    return invalid_action(action, "component is unavailable or already running");
                }
                self.components.insert(component.clone(), true);
            }
            WithdrawalActionIntent::Restart { component } => {
                component.validate(&action.id)?;
                if self.components.get(component) != Some(&true) {
                    return invalid_action(action, "component is unavailable or stopped");
                }
            }
            WithdrawalActionIntent::PartitionPeers { left, right } => {
                let valid_node = |node: &u64| *node < 5;
                if self.partitioned
                    || left.is_empty()
                    || right.is_empty()
                    || !left.is_disjoint(right)
                    || !left.iter().all(valid_node)
                    || !right.iter().all(valid_node)
                {
                    return invalid_action(action, "partition groups are invalid");
                }
                self.partitioned = true;
            }
            WithdrawalActionIntent::HealPeers => {
                if !self.partitioned {
                    return invalid_action(action, "no active partition to heal");
                }
                self.partitioned = false;
            }
            WithdrawalActionIntent::FailJournalEndpoint => {
                if self.journal_failed {
                    return invalid_action(action, "journal endpoint already failed");
                }
                self.journal_failed = true;
            }
            WithdrawalActionIntent::RecoverJournalEndpoint { .. } => {
                if !self.journal_failed {
                    return invalid_action(action, "journal endpoint is not failed");
                }
                self.journal_failed = false;
            }
            WithdrawalActionIntent::InjectBaseFork { .. }
            | WithdrawalActionIntent::InjectNockOrphan { .. }
                if self.terminal_asserted =>
            {
                return invalid_action(action, "fork injection after terminal assertion is invalid")
            }
            WithdrawalActionIntent::AssertTerminal => self.terminal_asserted = true,
            WithdrawalActionIntent::SubmitCanonicalBurn {
                amount_nicks,
                destination_lock_root,
                withdrawal_id,
                ..
            } => {
                if amount_nicks.parse::<u64>().is_err()
                    || amount_nicks == "0"
                    || destination_lock_root.trim().is_empty()
                    || withdrawal_id.trim().is_empty()
                {
                    return invalid_action(action, "canonical burn fields are invalid");
                }
            }
            WithdrawalActionIntent::MineBase { blocks }
            | WithdrawalActionIntent::AdvanceNockchain { blocks }
            | WithdrawalActionIntent::AdvanceObserverFrontiers { blocks }
                if *blocks == 0 =>
            {
                return invalid_action(action, "block count must be positive")
            }
            WithdrawalActionIntent::DuplicateAuthenticatedRequest { method, times } => {
                if method.trim().is_empty() || *times < 2 {
                    return invalid_action(action, "duplicate RPC needs a method and times >= 2");
                }
            }
            WithdrawalActionIntent::DuplicateBaseObservation { times, .. } if *times < 2 => {
                return invalid_action(action, "duplicate observation times must be >= 2")
            }
            WithdrawalActionIntent::AssertModelInvariant { invariant }
                if invariant.trim().is_empty() =>
            {
                return invalid_action(action, "model invariant name is empty")
            }
            _ => {}
        }
        Ok(())
    }
}

fn validate_action_id(id: &str) -> Result<(), ActionSchemaError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Err(ActionSchemaError::InvalidActionId(id.to_owned()))
    } else {
        Ok(())
    }
}

fn invalid_action<T>(
    action: &WithdrawalActionSpec,
    reason: &'static str,
) -> Result<T, ActionSchemaError> {
    Err(ActionSchemaError::InvalidAction {
        id: action.id.clone(),
        reason,
    })
}

#[derive(Debug, Error)]
pub enum ActionSchemaError {
    #[error("unsupported fault trace schema version {0}")]
    UnsupportedSchema(u64),
    #[error("invalid fault trace: {0}")]
    InvalidTrace(&'static str),
    #[error("invalid action id {0:?}")]
    InvalidActionId(String),
    #[error("duplicate action id {0}")]
    DuplicateActionId(String),
    #[error("invalid action {id}: {reason}")]
    InvalidAction { id: String, reason: &'static str },
    #[error("action {id} requires capability {capability:?}, unsupported by {backend}")]
    UnsupportedAction {
        id: String,
        capability: ActionCapability,
        backend: String,
    },
    #[error("action {id} fails model precondition: {reason}")]
    ModelPrecondition { id: String, reason: String },
    #[error("invalid YAML action trace: {0}")]
    Yaml(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum ActionExecutionError {
    #[error(transparent)]
    Schema(#[from] ActionSchemaError),
    #[error("action {0} timed out")]
    Timeout(String),
    #[error("action {id} failed model transition: {reason}")]
    Model { id: String, reason: String },
    #[error("action {id} failed SUT execution: {reason}")]
    Sut { id: String, reason: String },
    #[error("action {0} unexpectedly succeeded")]
    UnexpectedSuccess(String),
    #[error("action {id} invariant mismatch: expected {expected}, observed {observed}")]
    InvariantMismatch {
        id: String,
        expected: String,
        observed: String,
    },
    #[error(transparent)]
    ModelState(#[from] WithdrawalModelError),
}
