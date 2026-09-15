use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::actions::{ExpectedActionOutcome, WithdrawalActionIntent, WithdrawalFaultTrace};
use crate::evidence::{EvidenceStep, WithdrawalEvidenceCapsuleV1};
use crate::model::{
    ModelPublicState, WithdrawalModelAction, WithdrawalModelError, WithdrawalModelState,
    WITHDRAWAL_MODEL_SCHEMA_VERSION,
};

pub const MODEL_TRACE_SCHEMA_ID: &str = "nockchain.bridge.withdrawal-model-trace";
pub const MODEL_TRACE_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTraceSource {
    FaultAction,
    EvidenceStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalTransition {
    ObserveCanonicalBurn,
    InvalidateBurn,
    ReadmitBurn,
    RecoverDeepHold,
    Assemble,
    Prepare,
    Canonicalize,
    AdvanceHandoff,
    Reserve,
    RestoreReservations,
    Authorize,
    Submit,
    Include,
    Confirm,
    SettleKernel,
    RecordPayout,
    RecordCompensation,
    ReleaseReservations,
    PublishPublicState,
    PublishTerminal,
    RestartSequencer,
    RestartComponent,
    ReplayJournal,
    ShallowBaseFork,
    DeepBaseFork,
    ShallowNockFork,
    ShallowNockReinclusion,
    DeepNockFork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStutterKind {
    Lifecycle,
    Frontier,
    Facts,
    Assertion,
    JournalStatus,
    FailedBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationalStutterKind {
    Provision,
    ChainAdvance,
    ProcessControl,
    NetworkFault,
    JournalFault,
    AuthenticatedRetry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelTraceEventKind {
    Transition {
        formal: FormalTransition,
        action: WithdrawalModelAction,
    },
    ExpectedPrecondition {
        formal: FormalTransition,
        action: WithdrawalModelAction,
    },
    ObservationStutter {
        observation: ObservationStutterKind,
    },
    OperationalStutter {
        operation: OperationalStutterKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTraceEventV1 {
    pub runtime_index: u64,
    pub runtime_name: String,
    pub source: ModelTraceSource,
    pub event: ModelTraceEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTraceV1 {
    pub schema_id: String,
    pub schema_version: u64,
    pub model_schema_version: u64,
    pub terminal_expected: bool,
    pub events: Vec<ModelTraceEventV1>,
}

impl ModelTraceV1 {
    pub fn new(terminal_expected: bool, events: Vec<ModelTraceEventV1>) -> Self {
        Self {
            schema_id: MODEL_TRACE_SCHEMA_ID.to_owned(),
            schema_version: MODEL_TRACE_SCHEMA_VERSION,
            model_schema_version: WITHDRAWAL_MODEL_SCHEMA_VERSION,
            terminal_expected,
            events,
        }
    }

    pub fn from_json(input: &str) -> Result<Self, ModelTraceError> {
        let trace: Self = serde_json::from_str(input)?;
        trace.validate()?;
        Ok(trace)
    }

    pub fn validate(&self) -> Result<(), ModelTraceError> {
        if self.schema_id != MODEL_TRACE_SCHEMA_ID
            || self.schema_version != MODEL_TRACE_SCHEMA_VERSION
        {
            return Err(ModelTraceError::UnsupportedTraceSchema {
                schema_id: self.schema_id.clone(),
                schema_version: self.schema_version,
            });
        }
        if self.model_schema_version != WITHDRAWAL_MODEL_SCHEMA_VERSION {
            return Err(ModelTraceError::UnsupportedModelSchema {
                expected: WITHDRAWAL_MODEL_SCHEMA_VERSION,
                observed: self.model_schema_version,
            });
        }
        let mut previous = None;
        for event in &self.events {
            if event.runtime_name.trim().is_empty() {
                return Err(ModelTraceError::InvalidTrace(
                    "runtime event name is empty".to_owned(),
                ));
            }
            if previous.is_some_and(|index| event.runtime_index < index) {
                return Err(ModelTraceError::InvalidTrace(
                    "runtime event indices regress".to_owned(),
                ));
            }
            validate_formal_binding(event)?;
            previous = Some(event.runtime_index);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppliedModelEventKind {
    Transition,
    IdempotentStutter,
    ObservationStutter,
    OperationalStutter,
    ExpectedPrecondition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTraceRecord {
    pub runtime_index: u64,
    pub runtime_name: String,
    pub formal: Option<FormalTransition>,
    pub applied: AppliedModelEventKind,
    pub state_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalConformance {
    NotClaimed,
    Conformant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTraceConformance {
    pub schema_version: u64,
    pub terminal: TerminalConformance,
    pub final_state: WithdrawalModelState,
    pub records: Vec<ModelTraceRecord>,
}

pub fn check_model_trace(trace: &ModelTraceV1) -> Result<ModelTraceConformance, ModelTraceError> {
    trace.validate()?;
    let mut state = WithdrawalModelState::default();
    let mut records = Vec::with_capacity(trace.events.len());
    for event in &trace.events {
        let (formal, applied) = match &event.event {
            ModelTraceEventKind::Transition { formal, action } => {
                let outcome = state
                    .apply(action)
                    .map_err(|error| illegal_event(event, &state, error))?;
                let applied = if outcome.changed {
                    AppliedModelEventKind::Transition
                } else {
                    AppliedModelEventKind::IdempotentStutter
                };
                (Some(*formal), applied)
            }
            ModelTraceEventKind::ExpectedPrecondition { formal, action } => {
                let mut candidate = state.clone();
                match candidate.apply(action) {
                    Err(WithdrawalModelError::Precondition { .. }) => {}
                    Err(error) => return Err(illegal_event(event, &state, error)),
                    Ok(_) => {
                        return Err(ModelTraceError::ExpectedPreconditionSucceeded {
                            index: event.runtime_index,
                            runtime_name: event.runtime_name.clone(),
                            state: Box::new(state),
                        })
                    }
                }
                (Some(*formal), AppliedModelEventKind::ExpectedPrecondition)
            }
            ModelTraceEventKind::ObservationStutter { .. } => {
                (None, AppliedModelEventKind::ObservationStutter)
            }
            ModelTraceEventKind::OperationalStutter { .. } => {
                (None, AppliedModelEventKind::OperationalStutter)
            }
        };
        records.push(ModelTraceRecord {
            runtime_index: event.runtime_index,
            runtime_name: event.runtime_name.clone(),
            formal,
            applied,
            state_sha256: state.state_sha256()?,
        });
    }
    if trace.terminal_expected && !state.terminal {
        return Err(ModelTraceError::TerminalClaimBeforeModelTerminal {
            state: Box::new(state),
        });
    }
    Ok(ModelTraceConformance {
        schema_version: MODEL_TRACE_SCHEMA_VERSION,
        terminal: if trace.terminal_expected {
            TerminalConformance::Conformant
        } else {
            TerminalConformance::NotClaimed
        },
        final_state: state,
        records,
    })
}

pub fn map_fault_trace(trace: &WithdrawalFaultTrace) -> Result<ModelTraceV1, ModelTraceError> {
    trace.validate()?;
    let mut events = Vec::new();
    let mut terminal_expected = false;
    for (index, action) in trace.actions.iter().enumerate() {
        terminal_expected |= matches!(action.intent, WithdrawalActionIntent::AssertTerminal);
        if let WithdrawalActionIntent::DuplicateBaseObservation { times, .. } = &action.intent {
            let model_action = action.intent.model_action().ok_or_else(|| {
                ModelTraceError::InvalidTrace("duplicate observation lost model mapping".to_owned())
            })?;
            for duplicate in 0..*times {
                events.push(transition_event(
                    index as u64,
                    format!("{}#{duplicate}", action.id),
                    ModelTraceSource::FaultAction,
                    model_action.clone(),
                    &action.expected,
                ));
            }
            continue;
        }
        if let Some(model_action) = action.intent.model_action() {
            events.push(transition_event(
                index as u64,
                action.id.clone(),
                ModelTraceSource::FaultAction,
                model_action,
                &action.expected,
            ));
        } else {
            events.push(ModelTraceEventV1 {
                runtime_index: index as u64,
                runtime_name: action.id.clone(),
                source: ModelTraceSource::FaultAction,
                event: classify_stutter(&action.intent).ok_or_else(|| {
                    ModelTraceError::InvalidTrace(format!(
                        "model-backed action {} lost its transition mapping",
                        action.id
                    ))
                })?,
            });
        }
    }
    Ok(ModelTraceV1::new(terminal_expected, events))
}

pub fn map_evidence_capsule(
    capsule: &WithdrawalEvidenceCapsuleV1,
) -> Result<ModelTraceV1, ModelTraceError> {
    capsule.validate()?;
    let mut events = Vec::new();
    for step in &capsule.steps {
        let event = map_evidence_step(step, capsule)?;
        events.push(event);
        if step.status == "failed" {
            break;
        }
    }
    Ok(ModelTraceV1::new(capsule.terminal.is_some(), events))
}

fn map_evidence_step(
    step: &EvidenceStep,
    capsule: &WithdrawalEvidenceCapsuleV1,
) -> Result<ModelTraceEventV1, ModelTraceError> {
    if step.status == "failed" {
        return Ok(ModelTraceEventV1 {
            runtime_index: step.index,
            runtime_name: step.action.clone(),
            source: ModelTraceSource::EvidenceStep,
            event: ModelTraceEventKind::ObservationStutter {
                observation: ObservationStutterKind::FailedBoundary,
            },
        });
    }
    if step.status != "passed" && step.status != "expected_precondition_failure" {
        return Err(ModelTraceError::UnmappableEvidenceEvent {
            index: step.index,
            action: step.action.clone(),
            reason: format!("unsupported evidence status {}", step.status),
        });
    }
    if let Some(detail) = step
        .detail
        .as_ref()
        .and_then(|detail| detail.get("model_trace"))
    {
        let envelope: EvidenceModelEventV1 =
            serde_json::from_value(detail.clone()).map_err(|error| {
                ModelTraceError::UnmappableEvidenceEvent {
                    index: step.index,
                    action: step.action.clone(),
                    reason: error.to_string(),
                }
            })?;
        if envelope.schema_version != MODEL_TRACE_SCHEMA_VERSION {
            return Err(ModelTraceError::UnsupportedModelSchema {
                expected: MODEL_TRACE_SCHEMA_VERSION,
                observed: envelope.schema_version,
            });
        }
        let expected = if step.status == "expected_precondition_failure" {
            ExpectedActionOutcome::PreconditionFailure {
                code: "precondition".to_owned(),
            }
        } else {
            ExpectedActionOutcome::Success
        };
        return Ok(transition_event(
            step.index,
            step.action.clone(),
            ModelTraceSource::EvidenceStep,
            envelope.action,
            &expected,
        ));
    }
    if step.action == "submit_burn" {
        let burn =
            capsule
                .base
                .as_ref()
                .ok_or_else(|| ModelTraceError::UnmappableEvidenceEvent {
                    index: step.index,
                    action: step.action.clone(),
                    reason: "submit_burn is missing Base proof".to_owned(),
                })?;
        return Ok(transition_event(
            step.index,
            step.action.clone(),
            ModelTraceSource::EvidenceStep,
            WithdrawalModelAction::ObserveBurn {
                withdrawal_id: burn.event.base_event_id.clone(),
                nonce: burn.event.log_index,
            },
            &ExpectedActionOutcome::Success,
        ));
    }
    let observation = match step.action.as_str() {
        "pending"
        | "ready"
        | "submitted"
        | "sequencer_confirmed"
        | "terminal"
        | "wait_lifecycle"
        | "observe_lifecycle" => ObservationStutterKind::Lifecycle,
        "advance_observer_frontiers" => ObservationStutterKind::Frontier,
        "query_facts" | "collect_evidence" => ObservationStutterKind::Facts,
        "assert_model_invariant" | "assert_terminal" => ObservationStutterKind::Assertion,
        "journal_status" => ObservationStutterKind::JournalStatus,
        "provision" | "reset" => {
            return Ok(ModelTraceEventV1 {
                runtime_index: step.index,
                runtime_name: step.action.clone(),
                source: ModelTraceSource::EvidenceStep,
                event: ModelTraceEventKind::OperationalStutter {
                    operation: OperationalStutterKind::Provision,
                },
            })
        }
        _ => {
            return Err(ModelTraceError::UnmappableEvidenceEvent {
                index: step.index,
                action: step.action.clone(),
                reason: "no explicit model_trace detail or stutter classification".to_owned(),
            })
        }
    };
    Ok(ModelTraceEventV1 {
        runtime_index: step.index,
        runtime_name: step.action.clone(),
        source: ModelTraceSource::EvidenceStep,
        event: ModelTraceEventKind::ObservationStutter { observation },
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceModelEventV1 {
    schema_version: u64,
    action: WithdrawalModelAction,
}

fn transition_event(
    runtime_index: u64,
    runtime_name: String,
    source: ModelTraceSource,
    action: WithdrawalModelAction,
    expected: &ExpectedActionOutcome,
) -> ModelTraceEventV1 {
    let formal = formal_transition(&action);
    let event = match expected {
        ExpectedActionOutcome::Success => ModelTraceEventKind::Transition { formal, action },
        ExpectedActionOutcome::PreconditionFailure { .. } => {
            ModelTraceEventKind::ExpectedPrecondition { formal, action }
        }
    };
    ModelTraceEventV1 {
        runtime_index,
        runtime_name,
        source,
        event,
    }
}

fn classify_stutter(intent: &WithdrawalActionIntent) -> Option<ModelTraceEventKind> {
    Some(match intent {
        WithdrawalActionIntent::WaitLifecycle { .. }
        | WithdrawalActionIntent::ObserveLifecycle { .. } => {
            ModelTraceEventKind::ObservationStutter {
                observation: ObservationStutterKind::Lifecycle,
            }
        }
        WithdrawalActionIntent::AdvanceObserverFrontiers { .. } => {
            ModelTraceEventKind::ObservationStutter {
                observation: ObservationStutterKind::Frontier,
            }
        }
        WithdrawalActionIntent::QueryFacts => ModelTraceEventKind::ObservationStutter {
            observation: ObservationStutterKind::Facts,
        },
        WithdrawalActionIntent::AssertModelInvariant { .. }
        | WithdrawalActionIntent::AssertTerminal => ModelTraceEventKind::ObservationStutter {
            observation: ObservationStutterKind::Assertion,
        },
        WithdrawalActionIntent::Provision { .. } | WithdrawalActionIntent::Reset => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::Provision,
            }
        }
        WithdrawalActionIntent::MineBase { .. }
        | WithdrawalActionIntent::AdvanceNockchain { .. } => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::ChainAdvance,
            }
        }
        WithdrawalActionIntent::Stop { .. } | WithdrawalActionIntent::Start { .. } => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::ProcessControl,
            }
        }
        WithdrawalActionIntent::PartitionPeers { .. } | WithdrawalActionIntent::HealPeers => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::NetworkFault,
            }
        }
        WithdrawalActionIntent::DuplicateAuthenticatedRequest { .. } => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::AuthenticatedRetry,
            }
        }
        WithdrawalActionIntent::FailJournalEndpoint
        | WithdrawalActionIntent::SetJournalFailurePoint { .. } => {
            ModelTraceEventKind::OperationalStutter {
                operation: OperationalStutterKind::JournalFault,
            }
        }
        WithdrawalActionIntent::SubmitCanonicalBurn { .. }
        | WithdrawalActionIntent::Restart { .. }
        | WithdrawalActionIntent::DuplicateBaseObservation { .. }
        | WithdrawalActionIntent::RecoverJournalEndpoint { .. }
        | WithdrawalActionIntent::InjectBaseFork { .. }
        | WithdrawalActionIntent::InjectNockOrphan { .. }
        | WithdrawalActionIntent::ModelTransition { .. } => return None,
    })
}

fn formal_transition(action: &WithdrawalModelAction) -> FormalTransition {
    match action {
        WithdrawalModelAction::ObserveBurn { .. } => FormalTransition::ObserveCanonicalBurn,
        WithdrawalModelAction::InvalidateBurn => FormalTransition::InvalidateBurn,
        WithdrawalModelAction::ReadmitBurn => FormalTransition::ReadmitBurn,
        WithdrawalModelAction::RecoverHold { .. } => FormalTransition::RecoverDeepHold,
        WithdrawalModelAction::Assemble { .. } => FormalTransition::Assemble,
        WithdrawalModelAction::Prepare => FormalTransition::Prepare,
        WithdrawalModelAction::Canonicalize => FormalTransition::Canonicalize,
        WithdrawalModelAction::AdvanceHandoff { .. } => FormalTransition::AdvanceHandoff,
        WithdrawalModelAction::Reserve { .. } => FormalTransition::Reserve,
        WithdrawalModelAction::RestoreReservations => FormalTransition::RestoreReservations,
        WithdrawalModelAction::Authorize { .. } => FormalTransition::Authorize,
        WithdrawalModelAction::Submit { .. } => FormalTransition::Submit,
        WithdrawalModelAction::Include { .. } => FormalTransition::Include,
        WithdrawalModelAction::Confirm { .. } => FormalTransition::Confirm,
        WithdrawalModelAction::SettleKernel { .. } => FormalTransition::SettleKernel,
        WithdrawalModelAction::RecordPayout => FormalTransition::RecordPayout,
        WithdrawalModelAction::RecordRefund => FormalTransition::RecordCompensation,
        WithdrawalModelAction::ReleaseReservations { .. } => FormalTransition::ReleaseReservations,
        WithdrawalModelAction::Publish { state } => {
            if *state == ModelPublicState::Terminal {
                FormalTransition::PublishTerminal
            } else {
                FormalTransition::PublishPublicState
            }
        }
        WithdrawalModelAction::Restart { component } => {
            if component == "sequencer" {
                FormalTransition::RestartSequencer
            } else {
                FormalTransition::RestartComponent
            }
        }
        WithdrawalModelAction::ReplayJournal { .. } => FormalTransition::ReplayJournal,
        WithdrawalModelAction::BaseReorg { deep } => {
            if *deep {
                FormalTransition::DeepBaseFork
            } else {
                FormalTransition::ShallowBaseFork
            }
        }
        WithdrawalModelAction::NockReorg {
            deep,
            reinclusion_height,
            ..
        } => {
            if *deep {
                FormalTransition::DeepNockFork
            } else if reinclusion_height.is_some() {
                FormalTransition::ShallowNockReinclusion
            } else {
                FormalTransition::ShallowNockFork
            }
        }
    }
}

fn validate_formal_binding(event: &ModelTraceEventV1) -> Result<(), ModelTraceError> {
    let binding = match &event.event {
        ModelTraceEventKind::Transition { formal, action }
        | ModelTraceEventKind::ExpectedPrecondition { formal, action } => Some((formal, action)),
        ModelTraceEventKind::ObservationStutter { .. }
        | ModelTraceEventKind::OperationalStutter { .. } => None,
    };
    if let Some((formal, action)) = binding {
        let expected = formal_transition(action);
        if *formal != expected {
            return Err(ModelTraceError::FormalBindingMismatch {
                index: event.runtime_index,
                expected,
                observed: *formal,
            });
        }
    }
    Ok(())
}

fn illegal_event(
    event: &ModelTraceEventV1,
    state: &WithdrawalModelState,
    error: WithdrawalModelError,
) -> ModelTraceError {
    ModelTraceError::IllegalEvent {
        index: event.runtime_index,
        runtime_name: event.runtime_name.clone(),
        reason: error.to_string(),
        state: Box::new(state.clone()),
    }
}

#[derive(Debug, Error)]
pub enum ModelTraceError {
    #[error("unsupported model trace schema {schema_id} version {schema_version}")]
    UnsupportedTraceSchema {
        schema_id: String,
        schema_version: u64,
    },
    #[error("unsupported model schema: expected {expected}, observed {observed}")]
    UnsupportedModelSchema { expected: u64, observed: u64 },
    #[error("invalid model trace: {0}")]
    InvalidTrace(String),
    #[error(
        "formal binding mismatch at event {index}: expected {expected:?}, observed {observed:?}"
    )]
    FormalBindingMismatch {
        index: u64,
        expected: FormalTransition,
        observed: FormalTransition,
    },
    #[error("unmappable evidence event {index} ({action}): {reason}")]
    UnmappableEvidenceEvent {
        index: u64,
        action: String,
        reason: String,
    },
    #[error("illegal model event {index} ({runtime_name}): {reason}; state={state:?}")]
    IllegalEvent {
        index: u64,
        runtime_name: String,
        reason: String,
        state: Box<WithdrawalModelState>,
    },
    #[error("expected precondition event {index} ({runtime_name}) succeeded; state={state:?}")]
    ExpectedPreconditionSucceeded {
        index: u64,
        runtime_name: String,
        state: Box<WithdrawalModelState>,
    },
    #[error("runtime trace claimed terminal before model terminal; state={state:?}")]
    TerminalClaimBeforeModelTerminal { state: Box<WithdrawalModelState> },
    #[error(transparent)]
    ActionSchema(#[from] crate::actions::ActionSchemaError),
    #[error(transparent)]
    EvidenceSchema(#[from] crate::evidence::EvidenceSchemaError),
    #[error(transparent)]
    Model(#[from] WithdrawalModelError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
