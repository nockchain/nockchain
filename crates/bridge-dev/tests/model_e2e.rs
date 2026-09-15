use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bridge_dev::actions::{
    execute_fault_trace, ActionExecutionError, ActionSutResult, ExpectedActionOutcome,
    WithdrawalActionIntent, WithdrawalActionSpec, WithdrawalActionSut,
};
use bridge_dev::generated_scenario::{
    classify_execution_error, generate_scenario, shrink_failing_trace, write_minimized_trace,
    GeneratedFailureClass, GeneratedScenarioOptions,
};
use bridge_dev::model::WithdrawalModelState;
use proptest::prelude::*;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn fixed_seed_is_byte_deterministic_and_covers_core_lifecycle() {
    let first = generate_scenario(options(42, 64)).expect("first scenario");
    let second = generate_scenario(options(42, 64)).expect("second scenario");
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_vec(&first).expect("serialize first"),
        serde_json::to_vec(&second).expect("serialize second")
    );
    first.trace.validate().expect("generated trace validates");
    for phase in ["pending", "ready", "submitted", "sequencer_confirmed", "terminal"] {
        assert!(
            first.coverage.lifecycle_phases.contains_key(phase),
            "phase={phase}"
        );
    }
}

proptest! {
    #[test]
    fn arbitrary_seed_and_budget_generate_valid_bounded_traces(
        seed in any::<u64>(),
        max_actions in 1_usize..70,
        negative_percent in 0_u8..=100,
    ) {
        let mut options = options(seed, max_actions);
        options.negative_action_percent = negative_percent;
        let generated = generate_scenario(options).expect("generated scenario");
        prop_assert!(generated.trace.actions.len() <= max_actions);
        prop_assert!(generated.trace.validate().is_ok());
        prop_assert_eq!(generated.trace.actions[0].id.as_str(), "a000-provision");
    }
}

#[tokio::test]
async fn aligned_fake_sut_matches_model_at_observation_barriers() {
    let generated = generate_scenario(options(7, 64)).expect("scenario");
    let mut sut = AlignedSut::default();
    let execution = execute_fault_trace(&generated.trace, &mut sut)
        .await
        .expect("aligned execution");
    assert_eq!(execution.actions.len(), generated.trace.actions.len());
    assert_eq!(
        execution.final_model_state_sha256,
        sut.state.state_sha256().expect("SUT hash")
    );
}

#[tokio::test]
async fn injected_sut_divergence_is_classified_as_invariant_failure() {
    let generated = generate_scenario(options(8, 64)).expect("scenario");
    let mut sut = DivergentSut::default();
    let error = execute_fault_trace(&generated.trace, &mut sut)
        .await
        .expect_err("divergent SUT must fail");
    assert!(matches!(
        error,
        ActionExecutionError::InvariantMismatch { .. }
    ));
    assert_eq!(
        classify_execution_error(&error),
        GeneratedFailureClass::Invariant
    );
}

#[test]
fn deliberate_failure_shrinks_to_minimal_valid_replayable_trace() {
    let mut trace = generate_scenario(options(11, 64)).expect("scenario").trace;
    let index = trace.actions.len();
    trace.actions.push(WithdrawalActionSpec {
        id: format!("a{index:03}-bug-trigger"),
        label: "bug-trigger".to_owned(),
        timeout_ms: 1_000,
        expected: ExpectedActionOutcome::Success,
        intent: WithdrawalActionIntent::QueryFacts,
    });
    trace.validate().expect("bug trace validates");
    let minimized = shrink_failing_trace(&trace, |candidate| {
        candidate
            .actions
            .iter()
            .any(|action| action.label == "bug-trigger")
    })
    .expect("shrink failure");
    assert_eq!(minimized.actions.len(), 2);
    assert!(matches!(
        minimized.actions[0].intent,
        WithdrawalActionIntent::Provision { .. }
    ));
    assert_eq!(minimized.actions[1].label, "bug-trigger");
    minimized.validate().expect("minimized trace validates");

    let root = preserved_root("minimized");
    let path = root.join("minimized.json");
    write_minimized_trace(&path, &minimized).expect("write minimized trace");
    let replay = bridge_dev::actions::WithdrawalFaultTrace::from_json(
        &std::fs::read_to_string(&path).expect("read minimized trace"),
    )
    .expect("replay minimized trace");
    assert_eq!(replay, minimized);
    assert!(write_minimized_trace(&path, &minimized).is_err());
}

#[tokio::test]
async fn timeout_and_infrastructure_failures_are_not_misclassified() {
    let mut trace = generate_scenario(options(1, 1))
        .expect("one-action trace")
        .trace;
    trace.actions[0].timeout_ms = 1;
    trace.overall_timeout_ms = 10;
    let mut slow = SlowSut;
    let timeout_error = execute_fault_trace(&trace, &mut slow)
        .await
        .expect_err("slow action must time out");
    assert_eq!(
        classify_execution_error(&timeout_error),
        GeneratedFailureClass::Timeout
    );

    let mut overall_options = options(2, 2);
    overall_options.negative_action_percent = 0;
    let mut overall_trace = generate_scenario(overall_options)
        .expect("overall-timeout trace")
        .trace;
    overall_trace.overall_timeout_ms = 75;
    for action in &mut overall_trace.actions {
        action.timeout_ms = 75;
    }
    let mut overall_slow = SlowSut;
    let overall_error = execute_fault_trace(&overall_trace, &mut overall_slow)
        .await
        .expect_err("whole trace must honor its budget");
    assert!(matches!(
        &overall_error,
        ActionExecutionError::Timeout(scope) if scope == "overall trace"
    ));

    let mut broken = BrokenSut;
    let infrastructure_error = execute_fault_trace(&trace, &mut broken)
        .await
        .expect_err("broken SUT must fail");
    assert_eq!(
        classify_execution_error(&infrastructure_error),
        GeneratedFailureClass::Infrastructure
    );
}

fn options(seed: u64, max_actions: usize) -> GeneratedScenarioOptions {
    GeneratedScenarioOptions {
        seed,
        max_actions,
        max_runs: 10,
        action_timeout_ms: 1_000,
        overall_timeout_ms: 60_000,
        negative_action_percent: 25,
        environment_id: "hermetic".to_owned(),
        backend: "fake".to_owned(),
    }
}

#[derive(Default)]
struct AlignedSut {
    state: WithdrawalModelState,
}

#[async_trait]
impl WithdrawalActionSut for AlignedSut {
    async fn execute_action(
        &mut self,
        action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        if let Some(transition) = action.intent.model_action() {
            self.state
                .apply(&transition)
                .map_err(|error| error.to_string())?;
        }
        Ok(ActionSutResult {
            status: "passed".to_owned(),
            detail: Some(serde_json::json!({"action_id": action.id})),
        })
    }

    async fn observe_model_state(&mut self) -> Result<Option<WithdrawalModelState>, String> {
        Ok(Some(self.state.clone()))
    }
}

#[derive(Default)]
struct DivergentSut {
    inner: AlignedSut,
}

#[async_trait]
impl WithdrawalActionSut for DivergentSut {
    async fn execute_action(
        &mut self,
        action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        self.inner.execute_action(action).await
    }

    async fn observe_model_state(&mut self) -> Result<Option<WithdrawalModelState>, String> {
        let mut state = self.inner.state.clone();
        state.refund_count = state.refund_count.saturating_add(1);
        Ok(Some(state))
    }
}

struct SlowSut;

#[async_trait]
impl WithdrawalActionSut for SlowSut {
    async fn execute_action(
        &mut self,
        _action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(ActionSutResult {
            status: "passed".to_owned(),
            detail: None,
        })
    }
}

struct BrokenSut;

#[async_trait]
impl WithdrawalActionSut for BrokenSut {
    async fn execute_action(
        &mut self,
        _action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        Err("infrastructure unavailable".to_owned())
    }
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-generated-scenario-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
