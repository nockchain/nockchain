use std::collections::BTreeSet;

use bridge_dev::actions::{
    ActionCapability, ActionEnvironment, ExpectedActionOutcome, WithdrawalActionIntent,
    WithdrawalActionSpec, WithdrawalFaultTrace, FAULT_TRACE_SCHEMA_VERSION,
};
use bridge_dev::evidence::{
    EvidenceEnvironmentFacts, EvidenceEnvironmentMode, EvidenceRunFacts, EvidenceRunStatus,
    EvidenceStep, RedactionDeclaration, WithdrawalEvidenceCapsuleV1,
};
use bridge_dev::model::{ModelNoteName, ModelPublicState, WithdrawalModelAction};
use bridge_dev::model_trace::{
    check_model_trace, map_evidence_capsule, map_fault_trace, AppliedModelEventKind,
    FormalTransition, ModelTraceError, ModelTraceEventKind, ModelTraceEventV1, ModelTraceSource,
    ModelTraceV1, OperationalStutterKind, TerminalConformance, MODEL_TRACE_SCHEMA_VERSION,
};

#[test]
fn happy_trace_reaches_terminal_conformance() {
    let trace = ModelTraceV1::new(true, happy_events());
    let report = check_model_trace(&trace).expect("happy trace conforms");
    assert_eq!(report.terminal, TerminalConformance::Conformant);
    assert!(report.final_state.terminal);
    assert_eq!(report.records.len(), trace.events.len());
}

#[test]
fn restart_trace_restores_generation_and_reservations() {
    let mut events = through_authorization();
    let index = events.len() as u64;
    events.extend([
        transition(
            index,
            FormalTransition::RestartSequencer,
            WithdrawalModelAction::Restart {
                component: "sequencer".to_owned(),
            },
        ),
        transition(
            index + 1,
            FormalTransition::RestoreReservations,
            WithdrawalModelAction::RestoreReservations,
        ),
        transition(
            index + 2,
            FormalTransition::ReplayJournal,
            WithdrawalModelAction::ReplayJournal { generation: 1 },
        ),
    ]);
    events.extend(post_authorization_events(index + 3));
    let report =
        check_model_trace(&ModelTraceV1::new(true, events)).expect("restart trace conforms");
    assert_eq!(report.final_state.journal_generation, 1);
    assert!(!report.final_state.replay_required);
    assert!(report.final_state.terminal);
}

#[test]
fn shallow_reinclusion_preserves_raw_transaction_identity() {
    let mut events = through_sequencer_confirmation();
    let index = events.len() as u64;
    events.push(transition(
        index,
        FormalTransition::ShallowNockReinclusion,
        WithdrawalModelAction::NockReorg {
            deep: false,
            reinclusion_height: Some(11),
            reinclusion_block_id: Some("block-11".to_owned()),
        },
    ));
    events.push(transition(
        index + 1,
        FormalTransition::Confirm,
        WithdrawalModelAction::Confirm {
            transaction_id: "tx-1".to_owned(),
            height: 11,
            block_id: "block-11".to_owned(),
        },
    ));
    events.push(transition(
        index + 2,
        FormalTransition::PublishPublicState,
        WithdrawalModelAction::Publish {
            state: ModelPublicState::SequencerConfirmed,
        },
    ));
    events.extend(terminal_events(index + 3));
    let report =
        check_model_trace(&ModelTraceV1::new(true, events)).expect("reinclusion trace conforms");
    let inclusion = report.final_state.inclusion.expect("inclusion");
    assert_eq!(inclusion.transaction_id, "tx-1");
    assert_eq!(inclusion.height, 11);
    assert_eq!(inclusion.block_id, "block-11");
}

#[test]
fn deep_reorg_hold_and_partial_trace_do_not_claim_terminal() {
    let mut events = through_authorization();
    events.push(transition(
        events.len() as u64,
        FormalTransition::DeepBaseFork,
        WithdrawalModelAction::BaseReorg { deep: true },
    ));
    let report = check_model_trace(&ModelTraceV1::new(false, events))
        .expect("deep hold is a legal partial trace");
    assert_eq!(report.terminal, TerminalConformance::NotClaimed);
    assert!(report.final_state.hold.is_some());
    assert!(!report.final_state.terminal);
}

#[test]
fn fault_mapping_classifies_duplicate_and_observability_stutters() {
    let trace = WithdrawalFaultTrace {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: 7,
        environment: ActionEnvironment {
            environment_id: "trace-test".to_owned(),
            backend: "fake".to_owned(),
            capabilities: BTreeSet::from([
                ActionCapability::Provision,
                ActionCapability::Base,
                ActionCapability::ModelObservation,
                ActionCapability::AuthenticatedRpc,
            ]),
        },
        overall_timeout_ms: 10_000,
        actions: vec![
            action(
                "provision",
                WithdrawalActionIntent::Provision { reset: true },
            ),
            action(
                "submit-burn",
                WithdrawalActionIntent::SubmitCanonicalBurn {
                    withdrawal_id: "withdrawal-1".to_owned(),
                    nonce: 1,
                    amount_nicks: "6553600000".to_owned(),
                    destination_lock_root: "lock-root".to_owned(),
                },
            ),
            action(
                "duplicate-burn",
                WithdrawalActionIntent::DuplicateBaseObservation {
                    withdrawal_id: "withdrawal-1".to_owned(),
                    nonce: 1,
                    block_number: 10,
                    times: 2,
                },
            ),
            action(
                "duplicate-request",
                WithdrawalActionIntent::DuplicateAuthenticatedRequest {
                    method: "withdrawal_status".to_owned(),
                    times: 2,
                },
            ),
            action("query", WithdrawalActionIntent::QueryFacts),
        ],
    };
    let mapped = map_fault_trace(&trace).expect("map fault trace");
    let report = check_model_trace(&mapped).expect("mapped trace conforms");
    assert_eq!(report.records.len(), 6);
    assert_eq!(
        report.records[0].applied,
        AppliedModelEventKind::OperationalStutter
    );
    assert_eq!(report.records[1].applied, AppliedModelEventKind::Transition);
    assert_eq!(
        report.records[2].applied,
        AppliedModelEventKind::IdempotentStutter
    );
    assert_eq!(
        report.records[3].applied,
        AppliedModelEventKind::IdempotentStutter
    );
    assert_eq!(
        report.records[4].applied,
        AppliedModelEventKind::OperationalStutter
    );
    assert_eq!(
        report.records[5].applied,
        AppliedModelEventKind::ObservationStutter
    );
}

#[test]
fn raw_transaction_replacement_is_rejected_at_first_bad_event() {
    let mut events = through_authorization();
    let bad_index = events.len() as u64;
    events.push(transition(
        bad_index,
        FormalTransition::Submit,
        WithdrawalModelAction::Submit {
            transaction_id: "replacement-tx".to_owned(),
        },
    ));
    assert_illegal_index(ModelTraceV1::new(false, events), bad_index);
}

#[test]
fn reservation_double_owner_is_rejected_at_first_bad_event() {
    let mut events = through_authorization();
    let bad_index = events.len() as u64;
    events.push(transition(
        bad_index,
        FormalTransition::Reserve,
        WithdrawalModelAction::Reserve {
            owner: "another-withdrawal".to_owned(),
            inputs: selected_inputs(),
        },
    ));
    assert_illegal_index(ModelTraceV1::new(false, events), bad_index);
}

#[test]
fn premature_terminal_claim_is_rejected() {
    let events = through_authorization();
    let trace = ModelTraceV1::new(true, events);
    assert!(matches!(
        check_model_trace(&trace),
        Err(ModelTraceError::TerminalClaimBeforeModelTerminal { .. })
    ));
}

#[test]
fn evidence_mapping_uses_versioned_model_details_and_partial_failure_boundary() {
    let mut capsule = capsule();
    capsule.steps.push(evidence_transition(
        0,
        "observe-burn",
        WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        },
    ));
    capsule.steps.push(EvidenceStep {
        index: 1,
        action: "query_facts".to_owned(),
        status: "passed".to_owned(),
        started_at_unix_ms: 2,
        finished_at_unix_ms: 3,
        duration_ms: 1,
        frontier_before: None,
        frontier_after: None,
        detail: None,
    });
    capsule.steps.push(EvidenceStep {
        index: 2,
        action: "backend_timeout".to_owned(),
        status: "failed".to_owned(),
        started_at_unix_ms: 3,
        finished_at_unix_ms: 4,
        duration_ms: 1,
        frontier_before: None,
        frontier_after: None,
        detail: None,
    });
    let trace = map_evidence_capsule(&capsule).expect("map evidence");
    let report = check_model_trace(&trace).expect("partial evidence conforms");
    assert_eq!(report.terminal, TerminalConformance::NotClaimed);
    assert_eq!(report.records.len(), 3);
    assert!(!report.final_state.terminal);
}

#[test]
fn evidence_model_schema_mismatch_and_unknown_event_are_actionable() {
    let mut mismatched = capsule();
    let mut step = evidence_transition(
        0,
        "observe-burn",
        WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        },
    );
    step.detail = Some(serde_json::json!({
        "model_trace": {
            "schema_version": MODEL_TRACE_SCHEMA_VERSION + 1,
            "action": {
                "action": "observe_burn",
                "withdrawal_id": "withdrawal-1",
                "nonce": 1
            }
        }
    }));
    mismatched.steps.push(step);
    assert!(matches!(
        map_evidence_capsule(&mismatched),
        Err(ModelTraceError::UnsupportedModelSchema { .. })
    ));

    let mut unknown = capsule();
    unknown.steps.push(EvidenceStep {
        index: 0,
        action: "mystery_runtime_event".to_owned(),
        status: "passed".to_owned(),
        started_at_unix_ms: 1,
        finished_at_unix_ms: 2,
        duration_ms: 1,
        frontier_before: None,
        frontier_after: None,
        detail: None,
    });
    assert!(matches!(
        map_evidence_capsule(&unknown),
        Err(ModelTraceError::UnmappableEvidenceEvent { index: 0, .. })
    ));
}

#[test]
fn formal_binding_and_trace_schema_drift_are_rejected_before_execution() {
    let mut trace = ModelTraceV1::new(
        false,
        vec![transition(
            0,
            FormalTransition::ObserveCanonicalBurn,
            WithdrawalModelAction::ObserveBurn {
                withdrawal_id: "withdrawal-1".to_owned(),
                nonce: 1,
            },
        )],
    );
    if let ModelTraceEventKind::Transition { formal, .. } = &mut trace.events[0].event {
        *formal = FormalTransition::Submit;
    }
    assert!(matches!(
        check_model_trace(&trace),
        Err(ModelTraceError::FormalBindingMismatch { index: 0, .. })
    ));
    trace.events[0] = transition(
        0,
        FormalTransition::ObserveCanonicalBurn,
        WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        },
    );
    trace.model_schema_version += 1;
    assert!(matches!(
        check_model_trace(&trace),
        Err(ModelTraceError::UnsupportedModelSchema { .. })
    ));
}

fn happy_events() -> Vec<ModelTraceEventV1> {
    let mut events = through_sequencer_confirmation();
    events.extend(terminal_events(events.len() as u64));
    events
}

fn through_sequencer_confirmation() -> Vec<ModelTraceEventV1> {
    let mut events = through_authorization();
    events.extend(post_authorization_through_confirmation(events.len() as u64));
    events
}

fn through_authorization() -> Vec<ModelTraceEventV1> {
    let inputs = selected_inputs();
    vec![
        transition(
            0,
            FormalTransition::ObserveCanonicalBurn,
            WithdrawalModelAction::ObserveBurn {
                withdrawal_id: "withdrawal-1".to_owned(),
                nonce: 1,
            },
        ),
        transition(
            1,
            FormalTransition::Assemble,
            WithdrawalModelAction::Assemble {
                epoch: 1,
                handoff: 0,
                proposal_hash: "proposal-1".to_owned(),
                selected_inputs: inputs.clone(),
            },
        ),
        transition(2, FormalTransition::Prepare, WithdrawalModelAction::Prepare),
        transition(
            3,
            FormalTransition::Canonicalize,
            WithdrawalModelAction::Canonicalize,
        ),
        transition(
            4,
            FormalTransition::Reserve,
            WithdrawalModelAction::Reserve {
                owner: "withdrawal-1".to_owned(),
                inputs,
            },
        ),
        transition(
            5,
            FormalTransition::PublishPublicState,
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Ready,
            },
        ),
        transition(
            6,
            FormalTransition::Authorize,
            WithdrawalModelAction::Authorize {
                epoch: 1,
                transaction_id: "tx-1".to_owned(),
            },
        ),
    ]
}

fn post_authorization_events(start: u64) -> Vec<ModelTraceEventV1> {
    let mut events = post_authorization_through_confirmation(start);
    events.extend(terminal_events(start + events.len() as u64));
    events
}

fn post_authorization_through_confirmation(start: u64) -> Vec<ModelTraceEventV1> {
    vec![
        transition(
            start,
            FormalTransition::Submit,
            WithdrawalModelAction::Submit {
                transaction_id: "tx-1".to_owned(),
            },
        ),
        transition(
            start + 1,
            FormalTransition::PublishPublicState,
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Submitted,
            },
        ),
        transition(
            start + 2,
            FormalTransition::Include,
            WithdrawalModelAction::Include {
                transaction_id: "tx-1".to_owned(),
                height: 10,
                block_id: "block-10".to_owned(),
            },
        ),
        transition(
            start + 3,
            FormalTransition::Confirm,
            WithdrawalModelAction::Confirm {
                transaction_id: "tx-1".to_owned(),
                height: 10,
                block_id: "block-10".to_owned(),
            },
        ),
        transition(
            start + 4,
            FormalTransition::PublishPublicState,
            WithdrawalModelAction::Publish {
                state: ModelPublicState::SequencerConfirmed,
            },
        ),
    ]
}

fn terminal_events(start: u64) -> Vec<ModelTraceEventV1> {
    let mut events = (0..5)
        .map(|node_id| {
            transition(
                start + node_id,
                FormalTransition::SettleKernel,
                WithdrawalModelAction::SettleKernel { node_id },
            )
        })
        .collect::<Vec<_>>();
    let next = start + 5;
    events.extend([
        transition(
            next,
            FormalTransition::RecordPayout,
            WithdrawalModelAction::RecordPayout,
        ),
        transition(
            next + 1,
            FormalTransition::ReleaseReservations,
            WithdrawalModelAction::ReleaseReservations {
                owner: "withdrawal-1".to_owned(),
                inputs: selected_inputs(),
            },
        ),
        transition(
            next + 2,
            FormalTransition::PublishTerminal,
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Terminal,
            },
        ),
    ]);
    events
}

fn transition(
    runtime_index: u64,
    formal: FormalTransition,
    action: WithdrawalModelAction,
) -> ModelTraceEventV1 {
    ModelTraceEventV1 {
        runtime_index,
        runtime_name: action.name().to_owned(),
        source: ModelTraceSource::FaultAction,
        event: ModelTraceEventKind::Transition { formal, action },
    }
}

fn selected_inputs() -> BTreeSet<ModelNoteName> {
    BTreeSet::from([ModelNoteName {
        first: "input-first".to_owned(),
        last: "input-last".to_owned(),
    }])
}

fn action(id: &str, intent: WithdrawalActionIntent) -> WithdrawalActionSpec {
    WithdrawalActionSpec {
        id: id.to_owned(),
        label: id.to_owned(),
        timeout_ms: 1_000,
        expected: ExpectedActionOutcome::Success,
        intent,
    }
}

fn assert_illegal_index(trace: ModelTraceV1, expected: u64) {
    let result = check_model_trace(&trace);
    assert!(
        matches!(
            &result,
            Err(ModelTraceError::IllegalEvent { index, .. }) if *index == expected
        ),
        "expected illegal event {expected}, observed {result:?}"
    );
}

fn capsule() -> WithdrawalEvidenceCapsuleV1 {
    WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: "model-trace-run".to_owned(),
            scenario: "trace-conformance".to_owned(),
            seed: 1,
            status: EvidenceRunStatus::Running,
            error: None,
            started_at_unix_ms: 1,
            finished_at_unix_ms: None,
        },
        EvidenceEnvironmentFacts {
            mode: EvidenceEnvironmentMode::Hermetic,
            environment_id: "model-trace-env".to_owned(),
            source_manifest_sha256: None,
            source_chain_id: None,
            source_block_number: None,
            source_block_hash: None,
            local_chain_id: 31_338,
            rpc_endpoint_class: "loopback_anvil".to_owned(),
        },
        RedactionDeclaration {
            policy: "e2e-secret-redaction-v1".to_owned(),
            removed_secret_classes: vec!["private_key".to_owned()],
            raw_logs_embedded: false,
            external_artifacts_only: true,
        },
    )
}

fn evidence_transition(index: u64, name: &str, action: WithdrawalModelAction) -> EvidenceStep {
    EvidenceStep {
        index,
        action: name.to_owned(),
        status: "passed".to_owned(),
        started_at_unix_ms: index + 1,
        finished_at_unix_ms: index + 2,
        duration_ms: 1,
        frontier_before: None,
        frontier_after: None,
        detail: Some(serde_json::json!({
            "model_trace": {
                "schema_version": MODEL_TRACE_SCHEMA_VERSION,
                "action": action
            }
        })),
    }
}

#[test]
fn operational_stutter_enum_is_stable_for_reports() {
    let event = ModelTraceEventV1 {
        runtime_index: 0,
        runtime_name: "provision".to_owned(),
        source: ModelTraceSource::EvidenceStep,
        event: ModelTraceEventKind::OperationalStutter {
            operation: OperationalStutterKind::Provision,
        },
    };
    let encoded = serde_json::to_string(&event).expect("serialize event");
    assert!(encoded.contains("operational_stutter"));
    assert!(encoded.contains("provision"));
}
