use std::collections::BTreeSet;

use async_trait::async_trait;
use bridge_dev::actions::{
    execute_fault_trace, ActionCapability, ActionComponent, ActionEnvironment, ActionSchemaError,
    ActionSutResult, ExpectedActionOutcome, FaultTraceExecution, WithdrawalActionIntent,
    WithdrawalActionSpec, WithdrawalActionSut, WithdrawalFaultTrace, FAULT_TRACE_SCHEMA_VERSION,
};
use bridge_dev::model::{ModelNoteName, ModelPublicState, WithdrawalModelAction};

#[test]
fn action_schema_round_trips_json_and_yaml() {
    let trace = happy_trace();
    let json = serde_json::to_string_pretty(&trace).expect("serialize JSON trace");
    assert_eq!(
        WithdrawalFaultTrace::from_json(&json).expect("parse JSON trace"),
        trace
    );
    let yaml = trace.to_yaml().expect("serialize YAML trace");
    assert_eq!(
        WithdrawalFaultTrace::from_yaml(&yaml).expect("parse YAML trace"),
        trace
    );
    assert!(json.contains("submit_canonical_burn"));
    assert!(yaml.contains("schema_version"));
}

#[test]
fn old_schema_duplicate_ids_missing_provision_and_timeout_fail_before_execution() {
    let mut old = happy_trace();
    old.schema_version = 0;
    assert!(matches!(
        old.validate(),
        Err(ActionSchemaError::UnsupportedSchema(0))
    ));

    let mut duplicate = happy_trace();
    duplicate.actions[1].id = duplicate.actions[0].id.clone();
    assert!(matches!(
        duplicate.validate(),
        Err(ActionSchemaError::DuplicateActionId(_))
    ));

    let mut missing_provision = happy_trace();
    missing_provision.actions.remove(0);
    assert!(missing_provision.validate().is_err());

    let mut timeout = happy_trace();
    timeout.actions[1].timeout_ms = timeout.overall_timeout_ms + 1;
    assert!(timeout.validate().is_err());
}

#[test]
fn unsupported_capability_component_and_partition_are_rejected() {
    let mut unsupported = happy_trace();
    unsupported
        .environment
        .capabilities
        .remove(&ActionCapability::Base);
    assert!(matches!(
        unsupported.validate(),
        Err(ActionSchemaError::UnsupportedAction { .. })
    ));

    let mut component = process_trace();
    component.actions.push(spec(
        "stop-invalid",
        WithdrawalActionIntent::Stop {
            component: ActionComponent::Bridge { node_id: 5 },
        },
    ));
    assert!(component.validate().is_err());

    let mut partition = partition_trace();
    partition.actions.push(spec(
        "partition-overlap",
        WithdrawalActionIntent::PartitionPeers {
            left: BTreeSet::from([0, 1]),
            right: BTreeSet::from([1, 2]),
        },
    ));
    assert!(partition.validate().is_err());
}

#[test]
fn process_and_journal_prerequisite_graph_is_validated() {
    let mut trace = process_trace();
    trace.actions.extend([
        spec(
            "stop-bridge",
            WithdrawalActionIntent::Stop {
                component: ActionComponent::Bridge { node_id: 3 },
            },
        ),
        expected_failure(
            "stop-bridge-again",
            "already_stopped",
            WithdrawalActionIntent::Stop {
                component: ActionComponent::Bridge { node_id: 3 },
            },
        ),
        spec(
            "start-bridge",
            WithdrawalActionIntent::Start {
                component: ActionComponent::Bridge { node_id: 3 },
            },
        ),
        spec("journal-down", WithdrawalActionIntent::FailJournalEndpoint),
        spec(
            "restart-sequencer",
            WithdrawalActionIntent::Restart {
                component: ActionComponent::Sequencer,
            },
        ),
        spec(
            "journal-up",
            WithdrawalActionIntent::RecoverJournalEndpoint { generation: 1 },
        ),
    ]);
    trace.validate().expect("valid process prerequisite graph");

    let mut invalid = process_trace();
    invalid.actions.push(spec(
        "journal-up",
        WithdrawalActionIntent::RecoverJournalEndpoint { generation: 1 },
    ));
    assert!(invalid.validate().is_err());
}

#[test]
fn fork_after_terminal_assertion_and_invalid_duplicate_counts_fail_validation() {
    let mut trace = process_trace();
    trace
        .actions
        .push(spec("terminal", WithdrawalActionIntent::AssertTerminal));
    trace.actions.push(spec(
        "late-fork",
        WithdrawalActionIntent::InjectBaseFork {
            depth: 1,
            deep: false,
        },
    ));
    assert!(trace.validate().is_err());

    let mut duplicate = process_trace();
    duplicate.actions.push(spec(
        "duplicate-rpc",
        WithdrawalActionIntent::DuplicateAuthenticatedRequest {
            method: "get_status".to_owned(),
            times: 1,
        },
    ));
    assert!(duplicate.validate().is_err());
}

#[tokio::test]
async fn fake_harness_executes_trace_with_stable_ids_and_model_hashes() {
    let trace = happy_trace();
    let mut first_sut = FakeSut::default();
    let first = execute_fault_trace(&trace, &mut first_sut)
        .await
        .expect("execute first trace");
    let mut second_sut = FakeSut::default();
    let second = execute_fault_trace(&trace, &mut second_sut)
        .await
        .expect("execute second trace");
    assert_eq!(
        first.final_model_state_sha256,
        second.final_model_state_sha256
    );
    assert_eq!(model_hashes(&first), model_hashes(&second));
    assert_eq!(first.actions.len(), trace.actions.len());
    for (record, action) in first.actions.iter().zip(&trace.actions) {
        assert_eq!(record.id, action.id);
        assert_eq!(record.label, action.label);
    }
    assert_eq!(first_sut.executed_ids.len(), trace.actions.len());
}

#[tokio::test]
async fn model_precondition_failure_is_recorded_without_touching_sut() {
    let mut trace = process_trace();
    trace.actions.push(expected_failure(
        "confirm-too-early",
        "precondition",
        WithdrawalActionIntent::ModelTransition {
            transition: WithdrawalModelAction::Confirm {
                transaction_id: "tx-missing".to_owned(),
                height: 1,
                block_id: "block-1".to_owned(),
            },
        },
    ));
    trace
        .validate()
        .expect("expected model failure trace validates");
    let mut sut = FakeSut::default();
    let execution = execute_fault_trace(&trace, &mut sut)
        .await
        .expect("execute expected failure trace");
    assert_eq!(
        execution.actions.last().expect("record").status,
        "expected_precondition_failure"
    );
    assert_eq!(sut.executed_ids, vec!["provision"]);
}

#[test]
fn all_action_classes_have_capabilities_and_model_mapping_is_deterministic() {
    let actions = representative_actions();
    let capabilities = actions
        .iter()
        .map(WithdrawalActionIntent::capability)
        .collect::<BTreeSet<_>>();
    assert!(capabilities.contains(&ActionCapability::Provision));
    assert!(capabilities.contains(&ActionCapability::Base));
    assert!(capabilities.contains(&ActionCapability::Nockchain));
    assert!(capabilities.contains(&ActionCapability::BridgeProcess));
    assert!(capabilities.contains(&ActionCapability::SequencerProcess));
    assert!(capabilities.contains(&ActionCapability::NetworkPartition));
    assert!(capabilities.contains(&ActionCapability::Journal));
    assert!(capabilities.contains(&ActionCapability::AuthenticatedRpc));
    assert!(capabilities.contains(&ActionCapability::ModelObservation));
    assert!(capabilities.contains(&ActionCapability::TerminalAssertion));
    let first = actions
        .iter()
        .map(WithdrawalActionIntent::model_action)
        .collect::<Vec<_>>();
    let second = actions
        .iter()
        .map(WithdrawalActionIntent::model_action)
        .collect::<Vec<_>>();
    assert_eq!(first, second);
}

fn happy_trace() -> WithdrawalFaultTrace {
    let mut actions = vec![
        spec(
            "provision",
            WithdrawalActionIntent::Provision { reset: true },
        ),
        spec(
            "burn",
            WithdrawalActionIntent::SubmitCanonicalBurn {
                withdrawal_id: "withdrawal-1".to_owned(),
                nonce: 1,
                amount_nicks: "6553600000".to_owned(),
                destination_lock_root: "recipient-root".to_owned(),
            },
        ),
    ];
    for (id, transition) in happy_model_transitions() {
        actions.push(spec(
            id,
            WithdrawalActionIntent::ModelTransition { transition },
        ));
    }
    actions.push(spec(
        "terminal-assert",
        WithdrawalActionIntent::AssertTerminal,
    ));
    WithdrawalFaultTrace {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: 42,
        environment: ActionEnvironment {
            environment_id: "hermetic".to_owned(),
            backend: "fake".to_owned(),
            capabilities: BTreeSet::from([
                ActionCapability::Provision,
                ActionCapability::Base,
                ActionCapability::ModelObservation,
                ActionCapability::TerminalAssertion,
            ]),
        },
        overall_timeout_ms: 60_000,
        actions,
    }
}

fn happy_model_transitions() -> Vec<(&'static str, WithdrawalModelAction)> {
    let mut transitions = vec![
        (
            "assemble",
            WithdrawalModelAction::Assemble {
                epoch: 1,
                handoff: 0,
                proposal_hash: "proposal-1".to_owned(),
                selected_inputs: inputs(),
            },
        ),
        ("prepare", WithdrawalModelAction::Prepare),
        ("canonicalize", WithdrawalModelAction::Canonicalize),
        (
            "reserve",
            WithdrawalModelAction::Reserve {
                owner: "withdrawal-1".to_owned(),
                inputs: inputs(),
            },
        ),
        (
            "ready",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Ready,
            },
        ),
        (
            "authorize",
            WithdrawalModelAction::Authorize {
                epoch: 1,
                transaction_id: "tx-1".to_owned(),
            },
        ),
        (
            "submit",
            WithdrawalModelAction::Submit {
                transaction_id: "tx-1".to_owned(),
            },
        ),
        (
            "submitted",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Submitted,
            },
        ),
        (
            "include",
            WithdrawalModelAction::Include {
                transaction_id: "tx-1".to_owned(),
                height: 10,
                block_id: "block-10".to_owned(),
            },
        ),
        (
            "confirm",
            WithdrawalModelAction::Confirm {
                transaction_id: "tx-1".to_owned(),
                height: 10,
                block_id: "block-10".to_owned(),
            },
        ),
        (
            "sequencer-confirmed",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::SequencerConfirmed,
            },
        ),
    ];
    for node_id in 0..5 {
        transitions.push((
            match node_id {
                0 => "settle-0",
                1 => "settle-1",
                2 => "settle-2",
                3 => "settle-3",
                _ => "settle-4",
            },
            WithdrawalModelAction::SettleKernel { node_id },
        ));
    }
    transitions.extend([
        ("payout", WithdrawalModelAction::RecordPayout),
        (
            "release",
            WithdrawalModelAction::ReleaseReservations {
                owner: "withdrawal-1".to_owned(),
                inputs: inputs(),
            },
        ),
        (
            "terminal",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Terminal,
            },
        ),
    ]);
    transitions
}

fn process_trace() -> WithdrawalFaultTrace {
    WithdrawalFaultTrace {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: 1,
        environment: ActionEnvironment {
            environment_id: "hermetic".to_owned(),
            backend: "fake".to_owned(),
            capabilities: BTreeSet::from([
                ActionCapability::Provision,
                ActionCapability::BridgeProcess,
                ActionCapability::SequencerProcess,
                ActionCapability::Journal,
                ActionCapability::TerminalAssertion,
                ActionCapability::Base,
                ActionCapability::ModelObservation,
            ]),
        },
        overall_timeout_ms: 10_000,
        actions: vec![spec(
            "provision",
            WithdrawalActionIntent::Provision { reset: true },
        )],
    }
}

fn partition_trace() -> WithdrawalFaultTrace {
    let mut trace = process_trace();
    trace
        .environment
        .capabilities
        .insert(ActionCapability::NetworkPartition);
    trace
}

fn spec(id: &str, intent: WithdrawalActionIntent) -> WithdrawalActionSpec {
    WithdrawalActionSpec {
        id: id.to_owned(),
        label: id.replace('-', " "),
        timeout_ms: 1_000,
        expected: ExpectedActionOutcome::Success,
        intent,
    }
}

fn expected_failure(id: &str, code: &str, intent: WithdrawalActionIntent) -> WithdrawalActionSpec {
    WithdrawalActionSpec {
        id: id.to_owned(),
        label: id.replace('-', " "),
        timeout_ms: 1_000,
        expected: ExpectedActionOutcome::PreconditionFailure {
            code: code.to_owned(),
        },
        intent,
    }
}

fn representative_actions() -> Vec<WithdrawalActionIntent> {
    vec![
        WithdrawalActionIntent::Provision { reset: true },
        WithdrawalActionIntent::Reset,
        WithdrawalActionIntent::SubmitCanonicalBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
            amount_nicks: "1".to_owned(),
            destination_lock_root: "root".to_owned(),
        },
        WithdrawalActionIntent::MineBase { blocks: 1 },
        WithdrawalActionIntent::WaitLifecycle {
            phase: bridge_dev::actions::LifecycleObservation::Pending,
        },
        WithdrawalActionIntent::ObserveLifecycle {
            phase: bridge_dev::actions::LifecycleObservation::Ready,
        },
        WithdrawalActionIntent::Stop {
            component: ActionComponent::Bridge { node_id: 0 },
        },
        WithdrawalActionIntent::Start {
            component: ActionComponent::Bridge { node_id: 0 },
        },
        WithdrawalActionIntent::Restart {
            component: ActionComponent::Sequencer,
        },
        WithdrawalActionIntent::PartitionPeers {
            left: BTreeSet::from([0]),
            right: BTreeSet::from([1]),
        },
        WithdrawalActionIntent::HealPeers,
        WithdrawalActionIntent::DuplicateAuthenticatedRequest {
            method: "status".to_owned(),
            times: 2,
        },
        WithdrawalActionIntent::DuplicateBaseObservation {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
            block_number: 1,
            times: 2,
        },
        WithdrawalActionIntent::FailJournalEndpoint,
        WithdrawalActionIntent::RecoverJournalEndpoint { generation: 1 },
        WithdrawalActionIntent::SetJournalFailurePoint {
            point: bridge_dev::actions::JournalFailurePoint::AppendRemoteBeforeLocal,
        },
        WithdrawalActionIntent::InjectBaseFork {
            depth: 1,
            deep: false,
        },
        WithdrawalActionIntent::InjectNockOrphan {
            deep: false,
            reinclusion_height: None,
            reinclusion_block_id: None,
        },
        WithdrawalActionIntent::AdvanceNockchain { blocks: 1 },
        WithdrawalActionIntent::AdvanceObserverFrontiers { blocks: 1 },
        WithdrawalActionIntent::ModelTransition {
            transition: WithdrawalModelAction::ObserveBurn {
                withdrawal_id: "withdrawal-1".to_owned(),
                nonce: 1,
            },
        },
        WithdrawalActionIntent::QueryFacts,
        WithdrawalActionIntent::AssertModelInvariant {
            invariant: "single_payout".to_owned(),
        },
        WithdrawalActionIntent::AssertTerminal,
    ]
}

fn inputs() -> BTreeSet<ModelNoteName> {
    BTreeSet::from([ModelNoteName {
        first: "first-1".to_owned(),
        last: "last-1".to_owned(),
    }])
}

fn model_hashes(execution: &FaultTraceExecution) -> Vec<&str> {
    execution
        .actions
        .iter()
        .map(|record| record.model_state_sha256.as_str())
        .collect()
}

#[derive(Default)]
struct FakeSut {
    executed_ids: Vec<String>,
}

#[async_trait]
impl WithdrawalActionSut for FakeSut {
    async fn execute_action(
        &mut self,
        action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        self.executed_ids.push(action.id.clone());
        Ok(ActionSutResult {
            status: "passed".to_owned(),
            detail: Some(serde_json::json!({"action_id": action.id})),
        })
    }
}
