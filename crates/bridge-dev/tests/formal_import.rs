use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bridge_dev::actions::{
    execute_fault_trace, ActionSutResult, WithdrawalActionScenarioV1, WithdrawalActionSpec,
    WithdrawalActionSut,
};
use bridge_dev::formal_import::{
    import_formal_counterexample, FormalImportError, FormalImportOptions,
};
use bridge_dev::model::WithdrawalModelState;
use bridge_dev::model_trace::{check_model_trace, map_fault_trace, AppliedModelEventKind};
use bridge_dev::replay::ReplaySource;
use serde_json::{json, Value};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn reservation_counterexample_imports_and_replays_through_fake_harness() {
    let root = preserved_root("reservation");
    let source = root.join("reservation.itf.json");
    write_itf(&source, reservation_states());
    let result = import_formal_counterexample(
        &source,
        &root.join("imports"),
        options("oneReservationOwnerInv", "reservation-owner"),
    )
    .expect("import reservation counterexample");
    assert!(result.original_trace_path.is_file());
    assert!(result.scenario_path.is_file());
    assert!(result.report_path.is_file());
    assert_eq!(
        fs::read(&result.original_trace_path).expect("original copy"),
        fs::read(&source).expect("source")
    );
    let round_trip = WithdrawalActionScenarioV1::from_json(
        &fs::read_to_string(&result.scenario_path).expect("read scenario"),
    )
    .expect("parse scenario envelope");
    assert_eq!(round_trip.provenance.property, "oneReservationOwnerInv");
    assert_eq!(round_trip.provenance.counterexample_id, "reservation-owner");
    assert_eq!(round_trip, result.scenario);

    let replay_source = ReplaySource::load(&result.scenario_path).expect("load imported replay");
    assert_eq!(replay_source.scenario, round_trip.trace);
    let mapped = map_fault_trace(&round_trip.trace).expect("map imported trace");
    let conformance = check_model_trace(&mapped).expect("counterexample path is replayable");
    assert_eq!(
        conformance.records.last().expect("last record").applied,
        AppliedModelEventKind::ObservationStutter
    );
    assert!(conformance
        .records
        .iter()
        .any(|record| { record.applied == AppliedModelEventKind::ExpectedPrecondition }));

    let mut harness = FormalFakeHarness::default();
    execute_fault_trace(&round_trip.trace, &mut harness)
        .await
        .expect("fake harness replays imported scenario");
}

#[test]
fn authorized_identity_counterexample_is_deterministic() {
    let root = preserved_root("identity");
    let source = root.join("identity.itf.json");
    write_itf(&source, identity_states());
    let first = import_formal_counterexample(
        &source,
        &root.join("first"),
        options("retryIdentityInv", "identity-replacement"),
    )
    .expect("first import");
    let second = import_formal_counterexample(
        &source,
        &root.join("second"),
        options("retryIdentityInv", "identity-replacement"),
    )
    .expect("second import");
    assert_eq!(first.scenario, second.scenario);
    assert_eq!(
        serde_json::to_vec(&first.scenario).expect("first bytes"),
        serde_json::to_vec(&second.scenario).expect("second bytes")
    );
    assert!(first
        .scenario
        .trace
        .actions
        .iter()
        .any(|action| { action.label == "replace-authorized-raw-transaction" }));
}

#[test]
fn proper_deep_hold_imports_but_skipped_hold_is_explicitly_unsupported() {
    let root = preserved_root("hold");
    let proper_path = root.join("proper-hold.itf.json");
    write_itf(&proper_path, proper_hold_states());
    let proper = import_formal_counterexample(
        &proper_path,
        &root.join("proper"),
        options("unsafeForkHoldsInv", "proper-deep-hold"),
    )
    .expect("proper hold import");
    assert!(proper
        .scenario
        .trace
        .actions
        .iter()
        .any(|action| action.label == "deep-base-reorg"));

    let skipped_path = root.join("skipped-hold.itf.json");
    write_itf(&skipped_path, skipped_hold_states());
    assert!(matches!(
        import_formal_counterexample(
            &skipped_path,
            &root.join("skipped"),
            options("unsafeForkHoldsInv", "skipped-deep-hold"),
        ),
        Err(FormalImportError::UnsupportedAbstraction { state_index: 2, ref reason })
            if reason.contains("no safe runtime action")
    ));
}

#[test]
fn unsupported_transition_and_liveness_trace_fail_without_shell_fabrication() {
    let root = preserved_root("unsupported");
    let path = root.join("unsupported.itf.json");
    let mut states = vec![absent_state(), pending_state()];
    let mut unsupported = pending_state();
    set_variant(&mut unsupported, "publicState", "PublicTerminal");
    states.push(unsupported);
    write_itf(&path, states);
    assert!(matches!(
        import_formal_counterexample(
            &path,
            &root.join("imports"),
            options("publicStateInv", "unsupported-public-jump"),
        ),
        Err(FormalImportError::UnsupportedAbstraction { .. })
    ));
    assert!(matches!(
        import_formal_counterexample(
            &path,
            &root.join("liveness"),
            options("healthyEventuallyTerminal", "liveness-cycle"),
        ),
        Err(FormalImportError::UnsupportedAbstraction { state_index: 0, .. })
    ));
}

#[test]
fn old_or_malformed_itf_schema_is_rejected() {
    let root = preserved_root("schema");
    let path = root.join("old.itf.json");
    fs::create_dir_all(&root).expect("create schema root");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "#meta": {"format": "ITF-v0"},
            "vars": ["state"],
            "states": [{"state": absent_state()}, {"state": pending_state()}]
        }))
        .expect("serialize malformed ITF"),
    )
    .expect("write malformed ITF");
    assert!(matches!(
        import_formal_counterexample(
            &path,
            &root.join("imports"),
            options("allSafety", "old-schema"),
        ),
        Err(FormalImportError::InvalidItf(_))
    ));
}

#[derive(Default)]
struct FormalFakeHarness {
    state: WithdrawalModelState,
}

#[async_trait]
impl WithdrawalActionSut for FormalFakeHarness {
    async fn execute_action(
        &mut self,
        action: &WithdrawalActionSpec,
    ) -> Result<ActionSutResult, String> {
        if let Some(model_action) = action.intent.model_action() {
            self.state
                .apply(&model_action)
                .map_err(|error| error.to_string())?;
        }
        Ok(ActionSutResult {
            status: "passed".to_owned(),
            detail: None,
        })
    }

    async fn observe_model_state(&mut self) -> Result<Option<WithdrawalModelState>, String> {
        Ok(Some(self.state.clone()))
    }
}

fn reservation_states() -> Vec<Value> {
    let absent = absent_state();
    let pending = pending_state();
    let assembling = assembling_state();
    let mut prepared = assembling.clone();
    set_variant(&mut prepared, "phase", "Prepared");
    let mut canonical = prepared.clone();
    set_variant(&mut canonical, "phase", "PeerCanonical");
    set_variant(&mut canonical, "publicState", "PublicReady");
    set_reservations(&mut canonical, [(0, "burn-1"), (1, "burn-1")]);
    let mut bad = canonical.clone();
    set_reservations(&mut bad, [(0, "burn-1"), (1, "other-withdrawal")]);
    vec![absent, pending, assembling, prepared, canonical, bad]
}

fn identity_states() -> Vec<Value> {
    let mut states = reservation_states();
    states.pop();
    let canonical = states.last().expect("canonical state").clone();
    let mut authorized = canonical.clone();
    set_variant(&mut authorized, "phase", "Authorized");
    set_string(&mut authorized, "authorizedTxIdentity", "raw-tx-1");
    states.push(authorized.clone());
    let mut bad = authorized;
    set_string(&mut bad, "rawTxIdentity", "replacement-raw-tx");
    states.push(bad);
    states
}

fn proper_hold_states() -> Vec<Value> {
    let mut states = reservation_states();
    states.pop();
    let mut held = states.last().expect("canonical state").clone();
    set_variant(&mut held, "phase", "Held");
    set_variant(&mut held, "publicState", "PublicReorgHold");
    set_variant(&mut held, "hold", "DeepBaseFork");
    states.push(held);
    states
}

fn skipped_hold_states() -> Vec<Value> {
    let absent = absent_state();
    let pending = pending_state();
    let mut bad = pending.clone();
    set_variant(&mut bad, "hold", "DeepBaseFork");
    vec![absent, pending, bad]
}

fn absent_state() -> Value {
    json!({
        "canonicalBurn": false,
        "burnId": "",
        "phase": variant("Absent"),
        "publicState": variant("PublicNone"),
        "proposalEpoch": bigint(0),
        "proposalHash": "",
        "rawTxIdentity": "",
        "authorizedTxIdentity": "",
        "inclusionTxIdentity": "",
        "inclusionHeight": bigint(0),
        "inclusionBlock": "",
        "settledNodes": {"#set": []},
        "reservations": {"#map": [[bigint(0), ""], [bigint(1), ""]]},
        "payoutCount": bigint(0),
        "compensationCount": bigint(0),
        "journalGeneration": bigint(0),
        "replayRequired": false,
        "hold": variant("NoHold")
    })
}

fn pending_state() -> Value {
    let mut state = absent_state();
    set_bool(&mut state, "canonicalBurn", true);
    set_string(&mut state, "burnId", "burn-1");
    set_variant(&mut state, "phase", "Pending");
    set_variant(&mut state, "publicState", "PublicPending");
    state
}

fn assembling_state() -> Value {
    let mut state = pending_state();
    set_variant(&mut state, "phase", "Assembling");
    set_bigint(&mut state, "proposalEpoch", 1);
    set_string(&mut state, "proposalHash", "proposal-1");
    set_string(&mut state, "rawTxIdentity", "raw-tx-1");
    state
}

fn write_itf(path: &Path, states: Vec<Value>) {
    fs::create_dir_all(path.parent().expect("ITF parent")).expect("create ITF parent");
    let value = json!({
        "#meta": {"format": "ITF", "varTypes": {"state": "fixture"}},
        "vars": ["state"],
        "states": states.into_iter().map(|state| json!({"state": state})).collect::<Vec<_>>()
    });
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("serialize ITF"),
    )
    .expect("write ITF");
}

fn options(property: &str, id: &str) -> FormalImportOptions {
    FormalImportOptions {
        property: property.to_owned(),
        counterexample_id: id.to_owned(),
        environment_id: "formal-import-test".to_owned(),
        action_timeout_ms: 1_000,
        overall_timeout_ms: 60_000,
    }
}

fn variant(tag: &str) -> Value {
    json!({"tag": tag, "value": {"tag": "UNIT"}})
}

fn bigint(value: u64) -> Value {
    json!({"#bigint": value.to_string()})
}

fn set_bool(state: &mut Value, field: &str, value: bool) {
    state[field] = Value::Bool(value);
}

fn set_string(state: &mut Value, field: &str, value: &str) {
    state[field] = Value::String(value.to_owned());
}

fn set_variant(state: &mut Value, field: &str, value: &str) {
    state[field] = variant(value);
}

fn set_bigint(state: &mut Value, field: &str, value: u64) {
    state[field] = bigint(value);
}

fn set_reservations<const N: usize>(state: &mut Value, entries: [(u64, &str); N]) {
    state["reservations"] = json!({
        "#map": entries
            .into_iter()
            .map(|(input, owner)| json!([bigint(input), owner]))
            .collect::<Vec<_>>()
    });
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-formal-import-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
