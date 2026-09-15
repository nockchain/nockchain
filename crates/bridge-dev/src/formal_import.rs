use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::actions::{
    ActionCapability, ActionEnvironment, ActionScenarioProvenance, ExpectedActionOutcome,
    WithdrawalActionIntent, WithdrawalActionScenarioV1, WithdrawalActionSpec, WithdrawalFaultTrace,
    FAULT_TRACE_SCHEMA_VERSION,
};
use crate::model::{ModelNoteName, ModelPublicState, WithdrawalModelAction};

pub const FORMAL_IMPORT_SCHEMA_VERSION: u64 = 1;
const MAX_FORMAL_TRACE_BYTES: u64 = 16 * 1024 * 1024;
static IMPORT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormalImportOptions {
    pub property: String,
    pub counterexample_id: String,
    pub environment_id: String,
    pub action_timeout_ms: u64,
    pub overall_timeout_ms: u64,
}

impl FormalImportOptions {
    pub fn validate(&self) -> Result<(), FormalImportError> {
        if self.property.trim().is_empty()
            || self.counterexample_id.trim().is_empty()
            || self.environment_id.trim().is_empty()
            || self.action_timeout_ms == 0
            || self.overall_timeout_ms == 0
            || self.action_timeout_ms > self.overall_timeout_ms
        {
            return Err(FormalImportError::InvalidOptions);
        }
        let property = self.property.to_ascii_lowercase();
        if property.contains("liveness")
            || property.contains("eventually")
            || property.contains("fair")
        {
            return Err(FormalImportError::UnsupportedAbstraction {
                state_index: 0,
                reason: "fairness/liveness counterexamples are not runtime action traces"
                    .to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalImportReport {
    pub schema_version: u64,
    pub source_path: String,
    pub source_sha256: String,
    pub property: String,
    pub counterexample_id: String,
    pub formal_state_count: usize,
    pub translated_action_count: usize,
    pub original_trace_path: String,
    pub scenario_path: String,
}

#[derive(Debug, Clone)]
pub struct FormalImportResult {
    pub output_dir: PathBuf,
    pub original_trace_path: PathBuf,
    pub scenario_path: PathBuf,
    pub report_path: PathBuf,
    pub scenario: WithdrawalActionScenarioV1,
    pub report: FormalImportReport,
}

#[derive(Debug, Deserialize)]
struct ItfTrace {
    #[serde(rename = "#meta")]
    metadata: ItfMetadata,
    vars: Vec<String>,
    states: Vec<ItfStateEnvelope>,
}

#[derive(Debug, Deserialize)]
struct ItfMetadata {
    format: String,
}

#[derive(Debug, Deserialize)]
struct ItfStateEnvelope {
    state: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FormalState {
    canonical_burn: bool,
    burn_id: String,
    phase: String,
    public_state: String,
    proposal_epoch: u64,
    proposal_hash: String,
    raw_tx: String,
    authorized_tx: String,
    inclusion_tx: String,
    inclusion_height: u64,
    inclusion_block: String,
    settled_nodes: BTreeSet<u64>,
    reservations: BTreeMap<u64, String>,
    payout_count: u64,
    compensation_count: u64,
    journal_generation: u64,
    replay_required: bool,
    hold: String,
}

pub fn import_formal_counterexample(
    source_path: &Path,
    output_root: &Path,
    options: FormalImportOptions,
) -> Result<FormalImportResult, FormalImportError> {
    options.validate()?;
    let metadata = fs::metadata(source_path).map_err(|source| FormalImportError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_FORMAL_TRACE_BYTES {
        return Err(FormalImportError::TraceTooLarge(metadata.len()));
    }
    let bytes = fs::read(source_path).map_err(|source| FormalImportError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    let itf: ItfTrace = serde_json::from_slice(&bytes)?;
    if itf.metadata.format != "ITF" || itf.vars != ["state"] || itf.states.len() < 2 {
        return Err(FormalImportError::InvalidItf(
            "expected ITF with one state variable and at least two states".to_owned(),
        ));
    }
    let states = itf
        .states
        .iter()
        .enumerate()
        .map(|(index, state)| parse_formal_state(index, &state.state))
        .collect::<Result<Vec<_>, _>>()?;
    let trace = translate_states(&states, &source_sha256, &options)?;
    let provenance = ActionScenarioProvenance {
        source_kind: "quint_itf_counterexample".to_owned(),
        source_path: source_path.display().to_string(),
        source_sha256: source_sha256.clone(),
        property: options.property.clone(),
        counterexample_id: options.counterexample_id.clone(),
        model_schema_version: 1,
    };
    let scenario = WithdrawalActionScenarioV1::new(provenance, trace);
    scenario.validate()?;

    let output_dir = create_import_dir(output_root)?;
    let original_trace_path = output_dir.join("formal-counterexample.itf.json");
    let scenario_path = output_dir.join("scenario.json");
    let report_path = output_dir.join("import-report.json");
    write_new_bytes(&original_trace_path, &bytes)?;
    write_new_json(&scenario_path, &scenario)?;
    let report = FormalImportReport {
        schema_version: FORMAL_IMPORT_SCHEMA_VERSION,
        source_path: source_path.display().to_string(),
        source_sha256,
        property: options.property,
        counterexample_id: options.counterexample_id,
        formal_state_count: states.len(),
        translated_action_count: scenario.trace.actions.len(),
        original_trace_path: original_trace_path.display().to_string(),
        scenario_path: scenario_path.display().to_string(),
    };
    write_new_json(&report_path, &report)?;
    Ok(FormalImportResult {
        output_dir,
        original_trace_path,
        scenario_path,
        report_path,
        scenario,
        report,
    })
}

fn translate_states(
    states: &[FormalState],
    source_sha256: &str,
    options: &FormalImportOptions,
) -> Result<WithdrawalFaultTrace, FormalImportError> {
    let mut builder = ImportedActionBuilder::new(options.action_timeout_ms);
    builder.push(
        "provision",
        WithdrawalActionIntent::Provision { reset: true },
        ExpectedActionOutcome::Success,
    );
    for (offset, pair) in states.windows(2).enumerate() {
        let state_index = offset + 1;
        infer_transition(state_index, &pair[0], &pair[1], &mut builder)?;
        builder.push(
            &format!("observe-formal-state-{state_index}"),
            WithdrawalActionIntent::QueryFacts,
            ExpectedActionOutcome::Success,
        );
    }
    let seed_bytes: [u8; 8] = hex::decode(&source_sha256[..16])?
        .try_into()
        .map_err(|_| FormalImportError::InvalidItf("source hash is truncated".to_owned()))?;
    let trace = WithdrawalFaultTrace {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: u64::from_be_bytes(seed_bytes),
        environment: ActionEnvironment {
            environment_id: options.environment_id.clone(),
            backend: "formal-import".to_owned(),
            capabilities: BTreeSet::from([
                ActionCapability::Provision,
                ActionCapability::ModelObservation,
            ]),
        },
        overall_timeout_ms: options.overall_timeout_ms,
        actions: builder.actions,
    };
    trace.validate()?;
    Ok(trace)
}

fn infer_transition(
    state_index: usize,
    before: &FormalState,
    after: &FormalState,
    builder: &mut ImportedActionBuilder,
) -> Result<(), FormalImportError> {
    if before.hold == "NoHold"
        && after.hold.starts_with("Deep")
        && (after.phase != "Held" || after.public_state != "PublicReorgHold")
    {
        return Err(FormalImportError::UnsupportedAbstraction {
            state_index,
            reason: "deep reorg bypasses hold atomically; no safe runtime action can reproduce it"
                .to_owned(),
        });
    }
    if before.compensation_count > 0 && after.payout_count > before.payout_count {
        builder.model(
            "payout-after-compensation",
            WithdrawalModelAction::RecordPayout,
            expected_precondition(),
        );
        return Ok(());
    }
    if before.phase == "Authorized"
        && before.authorized_tx == after.authorized_tx
        && before.raw_tx != after.raw_tx
    {
        builder.model(
            "replace-authorized-raw-transaction",
            WithdrawalModelAction::Submit {
                transaction_id: after.raw_tx.clone(),
            },
            expected_precondition(),
        );
        return Ok(());
    }
    if after.phase == "Terminal" && after.settled_nodes.len() < 5 {
        builder.model(
            "terminal-without-kernel-settlement",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Terminal,
            },
            expected_precondition(),
        );
        return Ok(());
    }
    if let Some((input, owner)) = changed_foreign_reservation(before, after) {
        builder.model(
            "reservation-double-owner",
            WithdrawalModelAction::Reserve {
                owner,
                inputs: BTreeSet::from([formal_note(input)]),
            },
            expected_precondition(),
        );
        return Ok(());
    }

    let mut mapped = false;
    if !before.canonical_burn && after.canonical_burn && after.phase == "Pending" {
        builder.model(
            "observe-canonical-burn",
            WithdrawalModelAction::ObserveBurn {
                withdrawal_id: after.burn_id.clone(),
                nonce: 0,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "Pending" && after.phase == "Assembling" {
        builder.model(
            "assemble",
            WithdrawalModelAction::Assemble {
                epoch: after.proposal_epoch,
                handoff: 0,
                proposal_hash: after.proposal_hash.clone(),
                selected_inputs: formal_inputs(after),
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "Assembling" && after.phase == "Prepared" {
        builder.model(
            "prepare",
            WithdrawalModelAction::Prepare,
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "Prepared" && after.phase == "PeerCanonical" {
        builder.model(
            "canonicalize",
            WithdrawalModelAction::Canonicalize,
            ExpectedActionOutcome::Success,
        );
        let inputs = formal_inputs(after);
        if !inputs.is_empty() {
            builder.model(
                "reserve",
                WithdrawalModelAction::Reserve {
                    owner: after.burn_id.clone(),
                    inputs,
                },
                ExpectedActionOutcome::Success,
            );
        }
        builder.model(
            "publish-ready",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Ready,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "PeerCanonical" && after.phase == "Authorized" {
        builder.model(
            "authorize",
            WithdrawalModelAction::Authorize {
                epoch: after.proposal_epoch,
                transaction_id: after.authorized_tx.clone(),
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "Authorized" && after.phase == "Submitted" {
        builder.model(
            "submit",
            WithdrawalModelAction::Submit {
                transaction_id: after.raw_tx.clone(),
            },
            ExpectedActionOutcome::Success,
        );
        builder.model(
            "publish-submitted",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Submitted,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.inclusion_tx.is_empty() && !after.inclusion_tx.is_empty() {
        builder.model(
            "include",
            WithdrawalModelAction::Include {
                transaction_id: after.inclusion_tx.clone(),
                height: after.inclusion_height,
                block_id: after.inclusion_block.clone(),
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.phase == "Submitted" && after.phase == "SequencerConfirmed" {
        builder.model(
            "confirm",
            WithdrawalModelAction::Confirm {
                transaction_id: after.inclusion_tx.clone(),
                height: after.inclusion_height,
                block_id: after.inclusion_block.clone(),
            },
            ExpectedActionOutcome::Success,
        );
        builder.model(
            "publish-sequencer-confirmed",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::SequencerConfirmed,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    for node in after.settled_nodes.difference(&before.settled_nodes) {
        builder.model(
            &format!("settle-kernel-{node}"),
            WithdrawalModelAction::SettleKernel { node_id: *node },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.payout_count < after.payout_count {
        builder.model(
            "record-payout",
            WithdrawalModelAction::RecordPayout,
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.compensation_count < after.compensation_count {
        builder.model(
            "record-compensation",
            WithdrawalModelAction::RecordRefund,
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.reservations.values().any(|owner| !owner.is_empty())
        && after.reservations.values().all(String::is_empty)
        && after.payout_count > 0
    {
        builder.model(
            "release-reservations",
            WithdrawalModelAction::ReleaseReservations {
                owner: after.burn_id.clone(),
                inputs: formal_inputs(before),
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.journal_generation < after.journal_generation {
        builder.model(
            "restart-sequencer",
            WithdrawalModelAction::Restart {
                component: "sequencer".to_owned(),
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.replay_required && !after.replay_required {
        builder.model(
            "restore-reservations",
            WithdrawalModelAction::RestoreReservations,
            ExpectedActionOutcome::Success,
        );
        builder.model(
            "replay-journal",
            WithdrawalModelAction::ReplayJournal {
                generation: after.journal_generation,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.hold == "NoHold" && after.hold == "ShallowBaseFork" {
        builder.model(
            "shallow-base-reorg",
            WithdrawalModelAction::BaseReorg { deep: false },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.hold == "NoHold" && after.hold == "DeepBaseFork" {
        builder.model(
            "deep-base-reorg",
            WithdrawalModelAction::BaseReorg { deep: true },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if before.hold == "NoHold" && after.hold == "DeepNockFork" {
        builder.model(
            "deep-nock-reorg",
            WithdrawalModelAction::NockReorg {
                deep: true,
                reinclusion_height: None,
                reinclusion_block_id: None,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if after.phase == "Terminal" {
        builder.model(
            "publish-terminal",
            WithdrawalModelAction::Publish {
                state: ModelPublicState::Terminal,
            },
            ExpectedActionOutcome::Success,
        );
        mapped = true;
    }
    if !mapped && before != after {
        return Err(FormalImportError::UnsupportedAbstraction {
            state_index,
            reason: summarize_difference(before, after),
        });
    }
    Ok(())
}

fn changed_foreign_reservation(before: &FormalState, after: &FormalState) -> Option<(u64, String)> {
    after.reservations.iter().find_map(|(input, owner)| {
        let old = before
            .reservations
            .get(input)
            .map(String::as_str)
            .unwrap_or("");
        (!owner.is_empty() && owner != &after.burn_id && owner != old)
            .then(|| (*input, owner.clone()))
    })
}

fn formal_inputs(state: &FormalState) -> BTreeSet<ModelNoteName> {
    state
        .reservations
        .keys()
        .copied()
        .map(formal_note)
        .collect()
}

fn formal_note(input: u64) -> ModelNoteName {
    ModelNoteName {
        first: format!("formal-input-{input}"),
        last: "quint-symmetry-class".to_owned(),
    }
}

fn expected_precondition() -> ExpectedActionOutcome {
    ExpectedActionOutcome::PreconditionFailure {
        code: "precondition".to_owned(),
    }
}

struct ImportedActionBuilder {
    timeout_ms: u64,
    actions: Vec<WithdrawalActionSpec>,
}

impl ImportedActionBuilder {
    fn new(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            actions: Vec::new(),
        }
    }

    fn model(
        &mut self,
        label: &str,
        action: WithdrawalModelAction,
        expected: ExpectedActionOutcome,
    ) {
        self.push(
            label,
            WithdrawalActionIntent::ModelTransition { transition: action },
            expected,
        );
    }

    fn push(
        &mut self,
        label: &str,
        intent: WithdrawalActionIntent,
        expected: ExpectedActionOutcome,
    ) {
        let index = self.actions.len();
        let normalized = label
            .chars()
            .map(|character| {
                if character.is_ascii_lowercase() || character.is_ascii_digit() {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        self.actions.push(WithdrawalActionSpec {
            id: format!("formal-{index:03}-{normalized}"),
            label: label.to_owned(),
            timeout_ms: self.timeout_ms,
            expected,
            intent,
        });
    }
}

fn parse_formal_state(index: usize, value: &Value) -> Result<FormalState, FormalImportError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_state(index, "state is not an object"))?;
    Ok(FormalState {
        canonical_burn: bool_field(index, object, "canonicalBurn")?,
        burn_id: string_field(index, object, "burnId")?,
        phase: variant_field(index, object, "phase")?,
        public_state: variant_field(index, object, "publicState")?,
        proposal_epoch: integer_field(index, object, "proposalEpoch")?,
        proposal_hash: string_field(index, object, "proposalHash")?,
        raw_tx: string_field(index, object, "rawTxIdentity")?,
        authorized_tx: string_field(index, object, "authorizedTxIdentity")?,
        inclusion_tx: string_field(index, object, "inclusionTxIdentity")?,
        inclusion_height: integer_field(index, object, "inclusionHeight")?,
        inclusion_block: string_field(index, object, "inclusionBlock")?,
        settled_nodes: integer_set_field(index, object, "settledNodes")?,
        reservations: reservation_map_field(index, object, "reservations")?,
        payout_count: integer_field(index, object, "payoutCount")?,
        compensation_count: integer_field(index, object, "compensationCount")?,
        journal_generation: integer_field(index, object, "journalGeneration")?,
        replay_required: bool_field(index, object, "replayRequired")?,
        hold: variant_field(index, object, "hold")?,
    })
}

fn bool_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<bool, FormalImportError> {
    object
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid_state(index, &format!("{name} is not bool")))
}

fn string_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<String, FormalImportError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_state(index, &format!("{name} is not string")))
}

fn variant_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<String, FormalImportError> {
    object
        .get(name)
        .and_then(Value::as_object)
        .and_then(|variant| variant.get("tag"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_state(index, &format!("{name} is not an ITF variant")))
}

fn integer_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<u64, FormalImportError> {
    parse_itf_integer(
        object
            .get(name)
            .ok_or_else(|| invalid_state(index, &format!("{name} is missing")))?,
    )
    .ok_or_else(|| invalid_state(index, &format!("{name} is not a nonnegative integer")))
}

fn parse_itf_integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .get("#bigint")
            .and_then(Value::as_str)
            .and_then(|value| value.parse().ok())
    })
}

fn integer_set_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<BTreeSet<u64>, FormalImportError> {
    let values = object
        .get(name)
        .and_then(|value| value.get("#set"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_state(index, &format!("{name} is not an ITF set")))?;
    values
        .iter()
        .map(|value| {
            parse_itf_integer(value)
                .ok_or_else(|| invalid_state(index, &format!("{name} contains a non-integer")))
        })
        .collect()
}

fn reservation_map_field(
    index: usize,
    object: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<BTreeMap<u64, String>, FormalImportError> {
    let entries = object
        .get(name)
        .and_then(|value| value.get("#map"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_state(index, &format!("{name} is not an ITF map")))?;
    entries
        .iter()
        .map(|entry| {
            let pair = entry
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| invalid_state(index, &format!("{name} has invalid entry")))?;
            let input = parse_itf_integer(&pair[0])
                .ok_or_else(|| invalid_state(index, &format!("{name} key is invalid")))?;
            let owner = pair[1]
                .as_str()
                .ok_or_else(|| invalid_state(index, &format!("{name} owner is invalid")))?;
            Ok((input, owner.to_owned()))
        })
        .collect()
}

fn summarize_difference(before: &FormalState, after: &FormalState) -> String {
    let mut fields = Vec::new();
    if before.phase != after.phase {
        fields.push(format!("phase:{}->{}", before.phase, after.phase));
    }
    if before.public_state != after.public_state {
        fields.push(format!(
            "public_state:{}->{}",
            before.public_state, after.public_state
        ));
    }
    if before.raw_tx != after.raw_tx {
        fields.push("raw_tx_identity".to_owned());
    }
    if before.reservations != after.reservations {
        fields.push("reservations".to_owned());
    }
    if before.hold != after.hold {
        fields.push(format!("hold:{}->{}", before.hold, after.hold));
    }
    if fields.is_empty() {
        "state changed only in unsupported abstract fields".to_owned()
    } else {
        format!("unsupported simultaneous transition: {}", fields.join(", "))
    }
}

fn invalid_state(index: usize, reason: &str) -> FormalImportError {
    FormalImportError::InvalidState {
        index,
        reason: reason.to_owned(),
    }
}

fn create_import_dir(root: &Path) -> Result<PathBuf, FormalImportError> {
    fs::create_dir_all(root)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| FormalImportError::Clock)?;
    let sequence = IMPORT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!(
        "formal-import-{}-{}-{sequence}",
        now.as_millis(),
        std::process::id()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), FormalImportError> {
    write_new_bytes(path, &serde_json::to_vec_pretty(value)?)
}

fn write_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), FormalImportError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum FormalImportError {
    #[error("invalid formal import options")]
    InvalidOptions,
    #[error("formal trace exceeds {MAX_FORMAL_TRACE_BYTES} bytes: {0}")]
    TraceTooLarge(u64),
    #[error("invalid ITF trace: {0}")]
    InvalidItf(String),
    #[error("invalid formal state {index}: {reason}")]
    InvalidState { index: usize, reason: String },
    #[error("unsupported formal abstraction at state {state_index}: {reason}")]
    UnsupportedAbstraction { state_index: usize, reason: String },
    #[error("system clock precedes Unix epoch")]
    Clock,
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Hex(#[from] hex::FromHexError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    ActionSchema(#[from] crate::actions::ActionSchemaError),
    #[error(transparent)]
    Filesystem(#[from] std::io::Error),
}
