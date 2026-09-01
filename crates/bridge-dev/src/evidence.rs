use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::artifacts::{ArtifactFile, ArtifactRole, BinaryArchitecture, E2eArtifacts};
use crate::iris_artifact::IrisArtifactFacts;
use crate::iris_driver::BurnSubmissionProof;
use crate::nockchain_probe::{NockchainTransactionFacts, NoteNameFacts};
use crate::redaction::{RedactionError, SecretRedactor};
use crate::settlement_oracle::{
    BridgeKernelTerminalFacts, PublicWithdrawalTerminalFacts, ReservationTerminalFacts,
    SequencerTerminalFacts, SettlementConservationProof, TerminalWithdrawalProof,
    TimedTerminalFact,
};

pub const WITHDRAWAL_EVIDENCE_SCHEMA_ID: &str = "nockchain.bridge.withdrawal-e2e-evidence";
pub const WITHDRAWAL_EVIDENCE_SCHEMA_VERSION: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRunStatus {
    Running,
    Passed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRunFacts {
    pub run_id: String,
    pub scenario: String,
    pub seed: u64,
    pub status: EvidenceRunStatus,
    pub error: Option<String>,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceEnvironmentMode {
    Hermetic,
    BaseSepoliaFork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEnvironmentFacts {
    pub mode: EvidenceEnvironmentMode,
    pub environment_id: String,
    pub source_manifest_sha256: Option<String>,
    pub source_chain_id: Option<u64>,
    pub source_block_number: Option<u64>,
    pub source_block_hash: Option<String>,
    pub local_chain_id: u64,
    pub rpc_endpoint_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArtifactIdentity {
    pub role: String,
    pub sha256: String,
    pub size_bytes: String,
    pub architecture: Option<String>,
}

impl EvidenceArtifactIdentity {
    fn from_artifact(file: &ArtifactFile) -> Self {
        let role = match file.role {
            ArtifactRole::BridgeBinary => "bridge_binary",
            ArtifactRole::NodeBinary => "node_binary",
            ArtifactRole::SequencerCtlBinary => "sequencer_ctl_binary",
            ArtifactRole::BridgeJam => "bridge_jam",
            ArtifactRole::RoswellJam => "roswell_jam",
            ArtifactRole::FakenetGenesisJam => "fakenet_genesis_jam",
        };
        let architecture = file.architecture.map(|architecture| match architecture {
            BinaryArchitecture::Arm64 => "arm64",
            BinaryArchitecture::X86_64 => "x86_64",
            BinaryArchitecture::Universal => "universal",
        });
        Self {
            role: role.to_owned(),
            sha256: file.sha256.clone(),
            size_bytes: file.size_bytes.to_string(),
            architecture: architecture.map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArtifacts {
    pub bridge_runtime: E2eArtifacts,
    pub iris: Option<IrisArtifactFacts>,
    pub nockswap_bundle: Option<ExternalArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDeploymentFacts {
    pub environment_id: String,
    pub addresses: BTreeMap<String, String>,
    pub runtime_code_hashes: BTreeMap<String, String>,
    pub pristine_state: Option<BTreeMap<String, Value>>,
    pub overrides: Vec<EvidenceOverrideFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceOverrideFacts {
    pub kind: String,
    pub before: String,
    pub after: String,
    pub transaction_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceStep {
    pub index: u64,
    pub action: String,
    pub status: String,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    pub duration_ms: u64,
    pub frontier_before: Option<EvidenceFrontier>,
    pub frontier_after: Option<EvidenceFrontier>,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAssertion {
    pub assertion: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFrontier {
    pub base_height: Option<u64>,
    pub base_block_hash: Option<String>,
    pub nock_height: Option<u64>,
    pub nock_block_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSequencerFacts {
    pub proposal_hash: Option<String>,
    pub journal_id: Option<String>,
    pub sequencer: TimedTerminalFact<SequencerTerminalFacts>,
    pub reservations: TimedTerminalFact<ReservationTerminalFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceNockchainFacts {
    pub transaction_id: String,
    pub inclusion_height: u64,
    pub inclusion_block_id: String,
    pub tip_height: u64,
    pub confirmation_depth: u64,
    pub selected_inputs: Vec<EvidenceNockNote>,
    pub outputs: Vec<EvidenceNockNote>,
    pub raw_spend_fees_nicks: Vec<String>,
    pub transaction_fee_nicks: String,
    pub total_input_nicks: String,
    pub total_output_nicks: String,
    pub unaccounted_nicks: String,
    pub matching_recipient_output_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceNockNote {
    pub index: usize,
    pub name: NoteNameFacts,
    pub note_version: u64,
    pub assets_nicks: String,
    pub lock_root: String,
    pub origin_height: u64,
    pub origin_transaction_id: Option<String>,
    pub origin_is_coinbase: Option<bool>,
}

impl From<&NockchainTransactionFacts> for EvidenceNockchainFacts {
    fn from(facts: &NockchainTransactionFacts) -> Self {
        Self {
            transaction_id: facts.transaction_id.clone(),
            inclusion_height: facts.inclusion.height,
            inclusion_block_id: facts.inclusion.block_id.clone(),
            tip_height: facts.tip_height,
            confirmation_depth: facts.confirmation_depth,
            selected_inputs: facts
                .selected_inputs
                .iter()
                .enumerate()
                .map(|(index, input)| EvidenceNockNote {
                    index,
                    name: input.name.clone(),
                    note_version: input.note_version,
                    assets_nicks: input.assets_nicks.to_string(),
                    lock_root: input.name.first.clone(),
                    origin_height: input.origin_height,
                    origin_transaction_id: input.origin_transaction_id.clone(),
                    origin_is_coinbase: input.origin_is_coinbase,
                })
                .collect(),
            outputs: facts
                .outputs
                .iter()
                .map(|output| EvidenceNockNote {
                    index: output.index,
                    name: output.name.clone(),
                    note_version: output.note_version,
                    assets_nicks: output.assets_nicks.to_string(),
                    lock_root: output.lock_root.clone(),
                    origin_height: output.origin_height,
                    origin_transaction_id: Some(output.origin_transaction_id.clone()),
                    origin_is_coinbase: Some(output.origin_is_coinbase),
                })
                .collect(),
            raw_spend_fees_nicks: facts
                .raw_transaction
                .spends
                .iter()
                .map(|spend| spend.fee_nicks.to_string())
                .collect(),
            transaction_fee_nicks: facts.transaction_fee_nicks.to_string(),
            total_input_nicks: facts.total_input_nicks.to_string(),
            total_output_nicks: facts.total_output_nicks.to_string(),
            unaccounted_nicks: facts.unaccounted_nicks.to_string(),
            matching_recipient_output_indices: facts.matching_recipient_output_indices.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceKernelFacts {
    pub observed_unix_ms: u64,
    pub nodes: Vec<BridgeKernelTerminalFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceTerminalFacts {
    pub withdrawal_id: String,
    pub base_event_id: String,
    pub transaction_id: String,
    pub stable_observations: u64,
    pub chain_inclusion_height: u64,
    pub chain_inclusion_block_id: String,
    pub source_observed_unix_ms: BTreeMap<String, u64>,
    pub source_names: BTreeMap<String, String>,
    pub source_correlation_groups: BTreeMap<String, String>,
}

impl From<&TerminalWithdrawalProof> for EvidenceTerminalFacts {
    fn from(proof: &TerminalWithdrawalProof) -> Self {
        Self {
            withdrawal_id: proof.target.withdrawal_id.clone(),
            base_event_id: proof.target.base_event_id.clone(),
            transaction_id: proof.target.transaction_id.clone(),
            stable_observations: proof.stable_observations,
            chain_inclusion_height: proof.chain.facts.inclusion.height,
            chain_inclusion_block_id: proof.chain.facts.inclusion.block_id.clone(),
            source_observed_unix_ms: BTreeMap::from([
                ("chain".to_owned(), proof.chain.observed_unix_ms),
                ("kernels".to_owned(), proof.kernels.observed_unix_ms),
                ("public".to_owned(), proof.public.observed_unix_ms),
                (
                    "reservations".to_owned(),
                    proof.reservations.observed_unix_ms,
                ),
                ("sequencer".to_owned(), proof.sequencer.observed_unix_ms),
            ]),
            source_names: BTreeMap::from([
                ("chain".to_owned(), proof.chain.source_name.clone()),
                ("kernels".to_owned(), proof.kernels.source_name.clone()),
                ("public".to_owned(), proof.public.source_name.clone()),
                (
                    "reservations".to_owned(),
                    proof.reservations.source_name.clone(),
                ),
                ("sequencer".to_owned(), proof.sequencer.source_name.clone()),
            ]),
            source_correlation_groups: BTreeMap::from([
                ("chain".to_owned(), proof.chain.correlation_group.clone()),
                (
                    "kernels".to_owned(),
                    proof.kernels.correlation_group.clone(),
                ),
                ("public".to_owned(), proof.public.correlation_group.clone()),
                (
                    "reservations".to_owned(),
                    proof.reservations.correlation_group.clone(),
                ),
                (
                    "sequencer".to_owned(),
                    proof.sequencer.correlation_group.clone(),
                ),
            ]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionDeclaration {
    pub policy: String,
    pub removed_secret_classes: Vec<String>,
    pub raw_logs_embedded: bool,
    pub external_artifacts_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalArtifactReference {
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawalEvidenceCapsuleV1 {
    pub schema_id: String,
    pub schema_version: u64,
    pub run: EvidenceRunFacts,
    pub environment: EvidenceEnvironmentFacts,
    pub artifacts: Option<EvidenceArtifacts>,
    pub deployment: Option<EvidenceDeploymentFacts>,
    pub steps: Vec<EvidenceStep>,
    pub assertions: Vec<EvidenceAssertion>,
    pub base: Option<BurnSubmissionProof>,
    pub sequencer: Option<EvidenceSequencerFacts>,
    pub nockchain: Option<EvidenceNockchainFacts>,
    pub kernels: Option<EvidenceKernelFacts>,
    pub public: Option<TimedTerminalFact<PublicWithdrawalTerminalFacts>>,
    pub conservation: Option<SettlementConservationProof>,
    pub terminal: Option<EvidenceTerminalFacts>,
    pub external_artifacts: Vec<ExternalArtifactReference>,
    pub redaction: RedactionDeclaration,
    pub normalized_evidence_sha256: Option<String>,
}

impl WithdrawalEvidenceCapsuleV1 {
    pub fn new(
        run: EvidenceRunFacts,
        environment: EvidenceEnvironmentFacts,
        redaction: RedactionDeclaration,
    ) -> Self {
        Self {
            schema_id: WITHDRAWAL_EVIDENCE_SCHEMA_ID.to_owned(),
            schema_version: WITHDRAWAL_EVIDENCE_SCHEMA_VERSION,
            run,
            environment,
            artifacts: None,
            deployment: None,
            steps: Vec::new(),
            assertions: Vec::new(),
            base: None,
            sequencer: None,
            nockchain: None,
            kernels: None,
            public: None,
            conservation: None,
            terminal: None,
            external_artifacts: Vec::new(),
            redaction,
            normalized_evidence_sha256: None,
        }
    }

    pub fn model_trace(
        &self,
    ) -> Result<crate::model_trace::ModelTraceV1, crate::model_trace::ModelTraceError> {
        crate::model_trace::map_evidence_capsule(self)
    }

    pub fn check_model_conformance(
        &self,
    ) -> Result<crate::model_trace::ModelTraceConformance, crate::model_trace::ModelTraceError>
    {
        let trace = self.model_trace()?;
        crate::model_trace::check_model_trace(&trace)
    }

    pub fn from_json(input: &str) -> Result<Self, EvidenceSchemaError> {
        let header: Value = serde_json::from_str(input)?;
        let schema_id = header.get("schema_id").and_then(Value::as_str);
        let schema_version = header.get("schema_version").and_then(Value::as_u64);
        if schema_id != Some(WITHDRAWAL_EVIDENCE_SCHEMA_ID)
            || schema_version != Some(WITHDRAWAL_EVIDENCE_SCHEMA_VERSION)
        {
            return Err(EvidenceSchemaError::UnsupportedSchema {
                schema_id: schema_id.unwrap_or("<missing>").to_owned(),
                schema_version,
            });
        }
        let capsule: Self = serde_json::from_value(header)?;
        capsule.validate()?;
        Ok(capsule)
    }

    pub fn validate(&self) -> Result<(), EvidenceSchemaError> {
        if self.schema_id != WITHDRAWAL_EVIDENCE_SCHEMA_ID
            || self.schema_version != WITHDRAWAL_EVIDENCE_SCHEMA_VERSION
        {
            return Err(EvidenceSchemaError::UnsupportedSchema {
                schema_id: self.schema_id.clone(),
                schema_version: Some(self.schema_version),
            });
        }
        if self.run.run_id.trim().is_empty() || self.run.scenario.trim().is_empty() {
            return Err(EvidenceSchemaError::Invalid("run identity is missing"));
        }
        if self.environment.local_chain_id != 31_338
            || self.environment.rpc_endpoint_class != "loopback_anvil"
        {
            return Err(EvidenceSchemaError::Invalid(
                "local environment is not dedicated loopback Anvil",
            ));
        }
        validate_external_artifacts(&self.external_artifacts)?;
        if self.run.status == EvidenceRunStatus::Passed
            && (self.artifacts.is_none()
                || self.deployment.is_none()
                || self.base.is_none()
                || self.sequencer.is_none()
                || self.nockchain.is_none()
                || self.kernels.is_none()
                || self.public.is_none()
                || self.conservation.is_none()
                || self.terminal.is_none())
        {
            return Err(EvidenceSchemaError::IncompletePassedCapsule);
        }
        if let Some(expected) = &self.normalized_evidence_sha256 {
            let observed = self.normalized_sha256()?;
            if expected != &observed {
                return Err(EvidenceSchemaError::NormalizedHashMismatch {
                    expected: expected.clone(),
                    observed,
                });
            }
        }
        Ok(())
    }

    pub fn seal_normalized_hash(&mut self) -> Result<String, EvidenceSchemaError> {
        let hash = self.normalized_sha256()?;
        self.normalized_evidence_sha256 = Some(hash.clone());
        Ok(hash)
    }

    pub fn normalized_sha256(&self) -> Result<String, EvidenceSchemaError> {
        let normalized = self.normalized_value()?;
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(
            &normalized,
        )?)))
    }

    pub fn normalized_value(&self) -> Result<Value, EvidenceSchemaError> {
        let artifact_identities = self.artifacts.as_ref().map(|artifacts| {
            let runtime = &artifacts.bridge_runtime;
            let mut identities = vec![
                EvidenceArtifactIdentity::from_artifact(&runtime.bridge),
                EvidenceArtifactIdentity::from_artifact(&runtime.node),
                EvidenceArtifactIdentity::from_artifact(&runtime.bridge_jam),
                EvidenceArtifactIdentity::from_artifact(&runtime.roswell_jam),
                EvidenceArtifactIdentity::from_artifact(&runtime.fakenet_genesis_jam),
            ];
            if let Some(ctl) = &runtime.sequencer_ctl {
                identities.push(EvidenceArtifactIdentity::from_artifact(ctl));
            }
            identities.sort_by(|left, right| left.role.cmp(&right.role));
            let iris = artifacts.iris.as_ref().map(|iris| {
                serde_json::json!({
                    "package_name": iris.package_name,
                    "package_version": iris.package_version,
                    "git_revision": iris.git_revision,
                    "tarball_sha256": iris.tarball_sha256,
                    "npm_integrity": iris.npm_integrity,
                })
            });
            serde_json::json!({"runtime": identities, "iris": iris})
        });
        let steps = self
            .steps
            .iter()
            .map(|step| {
                serde_json::json!({
                    "index": step.index,
                    "action": step.action,
                    "status": step.status,
                    "frontier_before": step.frontier_before,
                    "frontier_after": step.frontier_after,
                    "detail": normalize_nested_value(step.detail.clone()),
                })
            })
            .collect::<Vec<_>>();
        let base = self
            .base
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?
            .map(|mut value| {
                remove_volatile_fields(&mut value);
                value
            });
        let external_artifacts = self
            .external_artifacts
            .iter()
            .map(|artifact| {
                serde_json::json!({
                    "kind": artifact.kind,
                    "sha256": artifact.sha256,
                    "size_bytes": artifact.size_bytes,
                    "media_type": artifact.media_type,
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "schema_id": self.schema_id,
            "schema_version": self.schema_version,
            "scenario": self.run.scenario,
            "seed": self.run.seed,
            "status": self.run.status,
            "environment": self.environment,
            "artifacts": artifact_identities,
            "deployment": self.deployment,
            "steps": steps,
            "assertions": self.assertions,
            "base": base,
            "sequencer": self.sequencer.as_ref().map(|facts| serde_json::json!({
                "proposal_hash": facts.proposal_hash,
                "journal_id": facts.journal_id,
                "sequencer": facts.sequencer.facts,
                "reservations": facts.reservations.facts,
            })),
            "nockchain": self.nockchain,
            "kernels": self.kernels.as_ref().map(|facts| &facts.nodes),
            "public": self.public.as_ref().map(|facts| &facts.facts),
            "conservation": self.conservation,
            "terminal": self.terminal.as_ref().map(|facts| serde_json::json!({
                "withdrawal_id": facts.withdrawal_id,
                "base_event_id": facts.base_event_id,
                "transaction_id": facts.transaction_id,
                "stable_observations": facts.stable_observations,
                "chain_inclusion_height": facts.chain_inclusion_height,
                "chain_inclusion_block_id": facts.chain_inclusion_block_id,
            })),
            "external_artifacts": external_artifacts,
            "redaction": self.redaction,
        }))
    }
}

fn normalize_nested_value(value: Option<Value>) -> Option<Value> {
    value.map(|mut value| {
        remove_volatile_fields(&mut value);
        value
    })
}

fn remove_volatile_fields(value: &mut Value) {
    const VOLATILE_FIELDS: &[&str] = &[
        "duration_ms", "finished_at_unix_ms", "observed_unix_ms", "run_dir", "run_id",
        "started_at_unix_ms", "tarball_path", "driver_path",
    ];
    match value {
        Value::Object(object) => {
            for field in VOLATILE_FIELDS {
                object.remove(*field);
            }
            for child in object.values_mut() {
                remove_volatile_fields(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                remove_volatile_fields(child);
            }
        }
        _ => {}
    }
}

fn validate_external_artifacts(
    artifacts: &[ExternalArtifactReference],
) -> Result<(), EvidenceSchemaError> {
    let mut identities = BTreeSet::new();
    for artifact in artifacts {
        if artifact.kind.trim().is_empty()
            || artifact.path.trim().is_empty()
            || artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || artifact.size_bytes.parse::<u64>().is_err()
            || !identities.insert((artifact.kind.clone(), artifact.sha256.clone()))
        {
            return Err(EvidenceSchemaError::Invalid(
                "external artifact reference is malformed or duplicated",
            ));
        }
    }
    Ok(())
}

const MAX_SAFE_EMBEDDED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeArtifactRecord {
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeArtifactIndex {
    pub schema_version: u64,
    pub root_sha256: String,
    pub index_excluded_from_root: bool,
    pub artifacts: Vec<SafeArtifactRecord>,
}

#[derive(Debug, Clone)]
pub struct EvidenceCollectionResult {
    pub safe_capsule: WithdrawalEvidenceCapsuleV1,
    pub report_path: PathBuf,
    pub normalized_report_path: PathBuf,
    pub artifact_index_path: PathBuf,
    pub root_sha256: String,
}

pub struct EvidenceCollector {
    run_dir: PathBuf,
    safe_dir: PathBuf,
    private_dir: PathBuf,
    redactor: SecretRedactor,
    artifacts: BTreeMap<String, SafeArtifactRecord>,
    checkpoint_index: u64,
}

impl EvidenceCollector {
    pub fn new(run_dir: &Path, redactor: SecretRedactor) -> Result<Self, EvidenceCollectionError> {
        fs::create_dir_all(run_dir)?;
        let safe_dir = run_dir.join("safe-evidence");
        let private_dir = run_dir.join("private-evidence");
        fs::create_dir_all(&safe_dir)?;
        fs::create_dir_all(&private_dir)?;
        set_private_directory(&safe_dir)?;
        set_private_directory(&private_dir)?;
        let collector = Self {
            run_dir: run_dir.to_path_buf(),
            safe_dir,
            private_dir,
            redactor,
            artifacts: BTreeMap::new(),
            checkpoint_index: 0,
        };
        collector.detect_partial_writes()?;
        Ok(collector)
    }

    pub fn checkpoint(
        &mut self,
        phase: &str,
        capsule: &WithdrawalEvidenceCapsuleV1,
    ) -> Result<PathBuf, EvidenceCollectionError> {
        let phase = sanitize_artifact_segment(phase)?;
        let safe_capsule = self.safe_capsule(capsule)?;
        let path = PathBuf::from(format!(
            "checkpoints/{:03}-{phase}-report.json",
            self.checkpoint_index
        ));
        self.checkpoint_index = self
            .checkpoint_index
            .checked_add(1)
            .ok_or(EvidenceCollectionError::CheckpointOverflow)?;
        self.write_safe_json(&path, &safe_capsule)?;
        Ok(self.safe_dir.join(path))
    }

    pub fn write_safe_text(
        &mut self,
        relative_path: &Path,
        text: &str,
        media_type: &str,
    ) -> Result<PathBuf, EvidenceCollectionError> {
        let redacted = self.redactor.redact_text(text);
        self.write_safe_bytes(relative_path, redacted.as_bytes(), media_type)
    }

    pub fn write_safe_json<T: Serialize>(
        &mut self,
        relative_path: &Path,
        value: &T,
    ) -> Result<PathBuf, EvidenceCollectionError> {
        let mut value = serde_json::to_value(value)?;
        self.redactor.redact_json(&mut value);
        let bytes = serde_json::to_vec_pretty(&value)?;
        self.write_safe_bytes(relative_path, &bytes, "application/json")
    }

    pub fn write_safe_toml(
        &mut self,
        relative_path: &Path,
        input: &str,
    ) -> Result<PathBuf, EvidenceCollectionError> {
        let mut value: toml::Value = toml::from_str(input)
            .map_err(|error| EvidenceCollectionError::Toml(error.to_string()))?;
        self.redactor.redact_toml(&mut value);
        let output = toml::to_string_pretty(&value)
            .map_err(|error| EvidenceCollectionError::Toml(error.to_string()))?;
        self.write_safe_bytes(relative_path, output.as_bytes(), "application/toml")
    }

    pub fn collect_log(
        &mut self,
        source: &Path,
        relative_path: &Path,
    ) -> Result<Option<ExternalArtifactReference>, EvidenceCollectionError> {
        validate_relative_path(relative_path)?;
        let bytes = fs::read(source)?;
        match std::str::from_utf8(&bytes) {
            Ok(text) => {
                self.write_safe_text(relative_path, text, "text/plain")?;
                Ok(None)
            }
            Err(_) => {
                let private_path = self.private_dir.join(relative_path);
                atomic_write_new(&private_path, &bytes)?;
                set_private_file(&private_path)?;
                Ok(Some(ExternalArtifactReference {
                    kind: "private_binary_log".to_owned(),
                    path: private_path.display().to_string(),
                    sha256: hex::encode(Sha256::digest(&bytes)),
                    size_bytes: bytes.len().to_string(),
                    media_type: "application/octet-stream".to_owned(),
                }))
            }
        }
    }

    pub fn collect_process_logs(
        &mut self,
        paths: &[PathBuf],
    ) -> Result<Vec<ExternalArtifactReference>, EvidenceCollectionError> {
        let mut private = Vec::new();
        for (index, source) in paths.iter().enumerate() {
            if !source.is_file() {
                continue;
            }
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("process.log");
            let relative = PathBuf::from(format!("logs/{index:03}-{name}"));
            if let Some(reference) = self.collect_log(source, &relative)? {
                private.push(reference);
            }
        }
        Ok(private)
    }

    pub fn write_private_raw(
        &self,
        relative_path: &Path,
        bytes: &[u8],
    ) -> Result<PathBuf, EvidenceCollectionError> {
        validate_relative_path(relative_path)?;
        let path = self.private_dir.join(relative_path);
        atomic_write_new(&path, bytes)?;
        set_private_file(&path)?;
        Ok(path)
    }

    pub fn finish(
        mut self,
        capsule: &WithdrawalEvidenceCapsuleV1,
    ) -> Result<EvidenceCollectionResult, EvidenceCollectionError> {
        self.detect_partial_writes()?;
        let safe_capsule = self.safe_capsule(capsule)?;
        let normalized = safe_capsule.normalized_value()?;
        let report_path = self.write_safe_json(Path::new("report.json"), &safe_capsule)?;
        let normalized_report_path =
            self.write_safe_json(Path::new("normalized-report.json"), &normalized)?;
        let index = self.artifact_index();
        let artifact_index_path = self.safe_dir.join("artifact-index.json");
        let bytes = serde_json::to_vec_pretty(&index)?;
        self.redactor.assert_safe(&bytes)?;
        atomic_write_new(&artifact_index_path, &bytes)?;
        self.scan_safe_outputs()?;
        Ok(EvidenceCollectionResult {
            safe_capsule,
            report_path,
            normalized_report_path,
            artifact_index_path,
            root_sha256: index.root_sha256,
        })
    }

    pub fn detect_partial_writes(&self) -> Result<(), EvidenceCollectionError> {
        let partials = partial_files(&self.safe_dir)?;
        if partials.is_empty() {
            Ok(())
        } else {
            Err(EvidenceCollectionError::PartialWrites(partials))
        }
    }

    fn safe_capsule(
        &self,
        capsule: &WithdrawalEvidenceCapsuleV1,
    ) -> Result<WithdrawalEvidenceCapsuleV1, EvidenceCollectionError> {
        let mut value = serde_json::to_value(capsule)?;
        self.redactor.redact_json(&mut value);
        value["normalized_evidence_sha256"] = Value::Null;
        let mut capsule: WithdrawalEvidenceCapsuleV1 = serde_json::from_value(value)?;
        capsule.redaction.removed_secret_classes = self.redactor.categories();
        capsule.redaction.raw_logs_embedded = false;
        capsule.redaction.external_artifacts_only = true;
        capsule.seal_normalized_hash()?;
        capsule.validate()?;
        Ok(capsule)
    }

    fn write_safe_bytes(
        &mut self,
        relative_path: &Path,
        bytes: &[u8],
        media_type: &str,
    ) -> Result<PathBuf, EvidenceCollectionError> {
        validate_relative_path(relative_path)?;
        if bytes.len() > MAX_SAFE_EMBEDDED_BYTES {
            return Err(EvidenceCollectionError::ArtifactTooLarge {
                path: relative_path.to_path_buf(),
                size: bytes.len(),
            });
        }
        self.redactor.assert_safe(bytes)?;
        let path = self.safe_dir.join(relative_path);
        atomic_write_new(&path, bytes)?;
        let relative = path
            .strip_prefix(&self.run_dir)
            .map_err(|_| EvidenceCollectionError::UnsafePath(relative_path.to_path_buf()))?
            .to_string_lossy()
            .replace('\\', "/");
        let record = SafeArtifactRecord {
            relative_path: relative.clone(),
            sha256: hex::encode(Sha256::digest(bytes)),
            size_bytes: bytes.len().to_string(),
            media_type: media_type.to_owned(),
        };
        if self.artifacts.insert(relative, record).is_some() {
            return Err(EvidenceCollectionError::DuplicateArtifact(
                relative_path.to_path_buf(),
            ));
        }
        Ok(path)
    }

    fn artifact_index(&self) -> SafeArtifactIndex {
        let artifacts = self.artifacts.values().cloned().collect::<Vec<_>>();
        let mut digest = Sha256::new();
        for artifact in &artifacts {
            digest.update(artifact.relative_path.as_bytes());
            digest.update([0]);
            digest.update(artifact.sha256.as_bytes());
            digest.update([0]);
            digest.update(artifact.size_bytes.as_bytes());
            digest.update([0]);
        }
        SafeArtifactIndex {
            schema_version: 1,
            root_sha256: hex::encode(digest.finalize()),
            index_excluded_from_root: true,
            artifacts,
        }
    }

    fn scan_safe_outputs(&self) -> Result<(), EvidenceCollectionError> {
        for path in regular_files(&self.safe_dir)? {
            self.redactor.assert_safe(&fs::read(path)?)?;
        }
        Ok(())
    }
}

fn atomic_write_new(path: &Path, bytes: &[u8]) -> Result<(), EvidenceCollectionError> {
    if path.exists() {
        return Err(EvidenceCollectionError::RefuseOverwrite(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| EvidenceCollectionError::UnsafePath(path.to_path_buf()))?;
    let partial = path.with_file_name(format!(".{file_name}.partial-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&partial, path)?;
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), EvidenceCollectionError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(EvidenceCollectionError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn sanitize_artifact_segment(segment: &str) -> Result<String, EvidenceCollectionError> {
    let sanitized = segment
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        Err(EvidenceCollectionError::UnsafePath(PathBuf::from(segment)))
    } else {
        Ok(sanitized)
    }
}

fn regular_files(root: &Path) -> Result<Vec<PathBuf>, EvidenceCollectionError> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

fn partial_files(root: &Path) -> Result<Vec<PathBuf>, EvidenceCollectionError> {
    Ok(regular_files(root)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".partial-"))
        })
        .collect())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), EvidenceCollectionError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), EvidenceCollectionError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), EvidenceCollectionError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), EvidenceCollectionError> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum EvidenceCollectionError {
    #[error("unsafe evidence path: {0}")]
    UnsafePath(PathBuf),
    #[error("refusing to overwrite evidence artifact: {0}")]
    RefuseOverwrite(PathBuf),
    #[error("duplicate evidence artifact: {0}")]
    DuplicateArtifact(PathBuf),
    #[error("evidence artifact {path} is too large to embed safely: {size} bytes")]
    ArtifactTooLarge { path: PathBuf, size: usize },
    #[error("partial evidence writes detected: {0:?}")]
    PartialWrites(Vec<PathBuf>),
    #[error("evidence checkpoint sequence overflow")]
    CheckpointOverflow,
    #[error("invalid TOML evidence: {0}")]
    Toml(String),
    #[error(transparent)]
    Filesystem(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Schema(#[from] EvidenceSchemaError),
    #[error(transparent)]
    Redaction(#[from] RedactionError),
}

#[derive(Debug, Error)]
pub enum EvidenceSchemaError {
    #[error("unsupported evidence schema {schema_id} version {schema_version:?}")]
    UnsupportedSchema {
        schema_id: String,
        schema_version: Option<u64>,
    },
    #[error("invalid evidence capsule: {0}")]
    Invalid(&'static str),
    #[error("passed evidence capsule is missing a required section")]
    IncompletePassedCapsule,
    #[error("normalized evidence hash mismatch: expected {expected}, observed {observed}")]
    NormalizedHashMismatch { expected: String, observed: String },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
