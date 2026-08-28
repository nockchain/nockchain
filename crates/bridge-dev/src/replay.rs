use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::actions::{WithdrawalActionScenarioV1, WithdrawalFaultTrace, ACTION_SCENARIO_SCHEMA_ID};
use crate::artifacts::{ArtifactOverrides, ArtifactResolveOptions, ArtifactResolver, E2eArtifacts};
use crate::evidence::{
    EvidenceCollector, EvidenceEnvironmentMode, EvidenceRunStatus, ExternalArtifactReference,
    WithdrawalEvidenceCapsuleV1, WITHDRAWAL_EVIDENCE_SCHEMA_ID,
};
use crate::redaction::SecretRedactor;

pub const REPLAY_REPORT_SCHEMA_VERSION: u64 = 1;
static REPLAY_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct ReplaySource {
    pub input_path: PathBuf,
    pub capsule_path: Option<PathBuf>,
    pub scenario_path: PathBuf,
    pub capsule: Option<WithdrawalEvidenceCapsuleV1>,
    pub scenario: WithdrawalFaultTrace,
    pub partial_original: bool,
}

impl ReplaySource {
    pub fn load(path: &Path) -> Result<Self, ReplayError> {
        let input_path = path.to_path_buf();
        if !path.exists() {
            return Err(ReplayError::MissingReplayInput(path.to_path_buf()));
        }
        let candidate = if path.is_dir() {
            let capsule = path.join("safe-evidence/report.json");
            let scenario = path.join("scenario.json");
            if capsule.is_file() {
                capsule
            } else if scenario.is_file() {
                scenario
            } else {
                return Err(ReplayError::MissingReplayInput(path.to_path_buf()));
            }
        } else {
            path.to_path_buf()
        };
        let bytes = fs::read(&candidate).map_err(|source| ReplayError::Read {
            path: candidate.clone(),
            source,
        })?;
        let header: Value = serde_json::from_slice(&bytes).map_err(|source| ReplayError::Json {
            path: candidate.clone(),
            source,
        })?;
        if header.get("schema_id").and_then(Value::as_str) == Some(WITHDRAWAL_EVIDENCE_SCHEMA_ID) {
            let capsule = WithdrawalEvidenceCapsuleV1::from_json(
                std::str::from_utf8(&bytes)
                    .map_err(|_| ReplayError::InvalidUtf8(candidate.clone()))?,
            )?;
            let scenario_path = referenced_scenario_path(&candidate, &capsule)?;
            let scenario = read_scenario(&scenario_path)?;
            validate_capsule_scenario_binding(&capsule, &scenario)?;
            let partial_original =
                capsule.run.status != EvidenceRunStatus::Passed || capsule.terminal.is_none();
            Ok(Self {
                input_path,
                capsule_path: Some(candidate),
                scenario_path,
                capsule: Some(capsule),
                scenario,
                partial_original,
            })
        } else {
            let scenario = read_scenario(&candidate)?;
            Ok(Self {
                input_path,
                capsule_path: None,
                scenario_path: candidate,
                capsule: None,
                scenario,
                partial_original: true,
            })
        }
    }

    pub fn environment_mode(&self) -> Option<EvidenceEnvironmentMode> {
        self.capsule
            .as_ref()
            .map(|capsule| capsule.environment.mode)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayArtifactResolution {
    NotRecorded,
    Exact,
    Substituted {
        differences: Vec<ArtifactDifference>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDifference {
    pub role: String,
    pub expected_sha256: Option<String>,
    pub observed_sha256: Option<String>,
}

pub fn compare_artifacts(
    expected: Option<&E2eArtifacts>,
    observed: Option<&E2eArtifacts>,
    allow_substitution: bool,
) -> Result<ReplayArtifactResolution, ReplayError> {
    match (expected, observed) {
        (None, None) => Ok(ReplayArtifactResolution::NotRecorded),
        (Some(_), None) => Err(ReplayError::MissingOriginalArtifacts),
        (None, Some(_)) if allow_substitution => Ok(ReplayArtifactResolution::Substituted {
            differences: artifact_differences(None, observed),
        }),
        (None, Some(_)) => Err(ReplayError::ArtifactSubstitutionRequiresApproval),
        (Some(expected), Some(observed)) => {
            let differences = artifact_differences(Some(expected), Some(observed));
            if differences.is_empty() {
                Ok(ReplayArtifactResolution::Exact)
            } else if allow_substitution {
                Ok(ReplayArtifactResolution::Substituted { differences })
            } else {
                Err(ReplayError::ArtifactSubstitutionRequiresApproval)
            }
        }
    }
}

pub fn resolve_replay_artifacts(
    workspace_root: &Path,
    expected: Option<&E2eArtifacts>,
    replacement_manifest: Option<&Path>,
    allow_substitution: bool,
) -> Result<(Option<E2eArtifacts>, ReplayArtifactResolution), ReplayError> {
    let replacement = replacement_manifest
        .map(read_artifact_manifest)
        .transpose()?;
    let selected = replacement.as_ref().or(expected);
    let Some(selected) = selected else {
        return Ok((None, ReplayArtifactResolution::NotRecorded));
    };
    let mut options = ArtifactResolveOptions::new(workspace_root.to_path_buf());
    options.require_ctl = selected.sequencer_ctl.is_some();
    options.overrides = ArtifactOverrides {
        bridge: Some(selected.bridge.path.clone()),
        node: Some(selected.node.path.clone()),
        sequencer_ctl: selected
            .sequencer_ctl
            .as_ref()
            .map(|file| file.path.clone()),
        bridge_jam: Some(selected.bridge_jam.path.clone()),
        roswell_jam: Some(selected.roswell_jam.path.clone()),
        fakenet_genesis_jam: Some(selected.fakenet_genesis_jam.path.clone()),
    };
    let resolved = ArtifactResolver::resolve(&options)?;
    let resolution = compare_artifacts(expected, Some(&resolved), allow_substitution)?;
    Ok((Some(resolved), resolution))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticComparisonClass {
    NoBaseline,
    ExactMatch,
    AllowedVolatileDifference,
    SemanticDivergence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticComparison {
    pub class: SemanticComparisonClass,
    pub differing_paths: Vec<String>,
    pub original_normalized_sha256: Option<String>,
    pub replay_normalized_sha256: String,
}

pub fn compare_semantics(
    original: Option<&WithdrawalEvidenceCapsuleV1>,
    replay: &WithdrawalEvidenceCapsuleV1,
) -> Result<SemanticComparison, ReplayError> {
    let replay_normalized = normalized_semantics(replay)?;
    let replay_hash = hash_json(&replay_normalized)?;
    let Some(original) = original else {
        return Ok(SemanticComparison {
            class: SemanticComparisonClass::NoBaseline,
            differing_paths: Vec::new(),
            original_normalized_sha256: None,
            replay_normalized_sha256: replay_hash,
        });
    };
    let original_normalized = normalized_semantics(original)?;
    let original_hash = hash_json(&original_normalized)?;
    let exact = exact_semantics(original)? == exact_semantics(replay)?;
    let class = if exact {
        SemanticComparisonClass::ExactMatch
    } else if original_normalized == replay_normalized {
        SemanticComparisonClass::AllowedVolatileDifference
    } else {
        SemanticComparisonClass::SemanticDivergence
    };
    let differing_paths = if class == SemanticComparisonClass::SemanticDivergence {
        differing_paths(&original_normalized, &replay_normalized)
    } else {
        Vec::new()
    };
    Ok(SemanticComparison {
        class,
        differing_paths,
        original_normalized_sha256: Some(original_hash),
        replay_normalized_sha256: replay_hash,
    })
}

#[derive(Debug, Clone)]
pub struct ReplayExecutionContext {
    pub run_dir: PathBuf,
    pub source: ReplaySource,
    pub artifacts: Option<E2eArtifacts>,
    pub artifact_resolution: ReplayArtifactResolution,
    pub archive_rpc_url: Option<String>,
}

#[async_trait]
pub trait ReplayExecutor: Send {
    async fn execute(
        &mut self,
        context: &ReplayExecutionContext,
    ) -> Result<WithdrawalEvidenceCapsuleV1, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    pub schema_version: u64,
    pub source_capsule: Option<String>,
    pub source_scenario: String,
    pub source_sha256: String,
    pub linked_capsule: String,
    pub artifact_resolution: ReplayArtifactResolution,
    pub comparison: SemanticComparison,
    pub partial_original: bool,
    pub failure_reproduced: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub report_path: PathBuf,
    pub linked_capsule_path: PathBuf,
    pub report: ReplayReport,
}

pub async fn run_replay<E: ReplayExecutor>(
    source: ReplaySource,
    artifacts: Option<E2eArtifacts>,
    artifact_resolution: ReplayArtifactResolution,
    archive_rpc_url: Option<String>,
    output_root: &Path,
    executor: &mut E,
) -> Result<ReplayResult, ReplayError> {
    validate_archive_source(&source, archive_rpc_url.as_deref()).await?;
    verify_recorded_iris_artifact(source.capsule.as_ref())?;
    let run_dir = create_replay_run_dir(output_root)?;
    let context = ReplayExecutionContext {
        run_dir: run_dir.clone(),
        source,
        artifacts,
        artifact_resolution: artifact_resolution.clone(),
        archive_rpc_url,
    };
    let mut replay_capsule = executor
        .execute(&context)
        .await
        .map_err(ReplayError::Executor)?;
    replay_capsule.validate()?;
    let comparison = compare_semantics(context.source.capsule.as_ref(), &replay_capsule)?;
    let failure_reproduced = context
        .source
        .capsule
        .as_ref()
        .filter(|capsule| capsule.run.status == EvidenceRunStatus::Failed)
        .map(|_| replay_capsule.run.status == EvidenceRunStatus::Failed);
    let source_path = context
        .source
        .capsule_path
        .as_ref()
        .unwrap_or(&context.source.scenario_path);
    let source_reference = external_reference("replay_source", source_path)?;
    replay_capsule.external_artifacts.push(source_reference);
    replay_capsule.normalized_evidence_sha256 = None;

    let redactor = SecretRedactor::new(Vec::new())?;
    let collector = EvidenceCollector::new(&run_dir, redactor)?;
    let collected = collector.finish(&replay_capsule)?;
    let report = ReplayReport {
        schema_version: REPLAY_REPORT_SCHEMA_VERSION,
        source_capsule: context
            .source
            .capsule_path
            .as_ref()
            .map(|path| path.display().to_string()),
        source_scenario: context.source.scenario_path.display().to_string(),
        source_sha256: sha256_file(source_path)?,
        linked_capsule: collected.report_path.display().to_string(),
        artifact_resolution,
        comparison,
        partial_original: context.source.partial_original,
        failure_reproduced,
    };
    let report_path = run_dir.join("replay-result.json");
    write_new_json(&report_path, &report)?;
    Ok(ReplayResult {
        report_path,
        linked_capsule_path: collected.report_path,
        report,
    })
}

pub async fn validate_archive_source(
    source: &ReplaySource,
    archive_rpc_url: Option<&str>,
) -> Result<(), ReplayError> {
    match source.environment_mode() {
        Some(EvidenceEnvironmentMode::Hermetic) | None => {
            if archive_rpc_url.is_some() {
                return Err(ReplayError::ArchiveRpcOnlyForFork);
            }
            Ok(())
        }
        Some(EvidenceEnvironmentMode::BaseSepoliaFork) => {
            let rpc_url = archive_rpc_url.ok_or(ReplayError::ArchiveRpcRequired)?;
            let capsule = source
                .capsule
                .as_ref()
                .ok_or(ReplayError::ArchiveRpcRequired)?;
            let number = capsule
                .environment
                .source_block_number
                .ok_or(ReplayError::MissingForkIdentity)?;
            let expected_hash = capsule
                .environment
                .source_block_hash
                .as_deref()
                .ok_or(ReplayError::MissingForkIdentity)?;
            let block = archive_block(rpc_url, number).await?;
            if block.number != number || !block.hash.eq_ignore_ascii_case(expected_hash) {
                return Err(ReplayError::ForkBlockMismatch {
                    expected_number: number,
                    expected_hash: expected_hash.to_owned(),
                    observed_number: block.number,
                    observed_hash: block.hash,
                });
            }
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize)]
struct ArchiveBlock {
    number: String,
    hash: String,
}

#[derive(Debug)]
struct ParsedArchiveBlock {
    number: u64,
    hash: String,
}

async fn archive_block(rpc_url: &str, number: u64) -> Result<ParsedArchiveBlock, ReplayError> {
    let response = reqwest::Client::new()
        .post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getBlockByNumber",
            "params": [format!("0x{number:x}"), false]
        }))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| ReplayError::ForkProviderUnavailable { source })?;
    let value: Value = response
        .json()
        .await
        .map_err(|source| ReplayError::ForkProviderUnavailable { source })?;
    if value.get("error").is_some() {
        return Err(ReplayError::ForkProviderResponse);
    }
    let block: ArchiveBlock = serde_json::from_value(
        value
            .get("result")
            .cloned()
            .ok_or(ReplayError::ForkProviderResponse)?,
    )
    .map_err(|_| ReplayError::ForkProviderResponse)?;
    let number = u64::from_str_radix(block.number.trim_start_matches("0x"), 16)
        .map_err(|_| ReplayError::ForkProviderResponse)?;
    Ok(ParsedArchiveBlock {
        number,
        hash: block.hash,
    })
}

fn referenced_scenario_path(
    capsule_path: &Path,
    capsule: &WithdrawalEvidenceCapsuleV1,
) -> Result<PathBuf, ReplayError> {
    let reference = capsule
        .external_artifacts
        .iter()
        .find(|artifact| artifact.kind == "fault_trace" || artifact.kind == "scenario")
        .ok_or_else(|| {
            if capsule
                .artifacts
                .as_ref()
                .and_then(|artifacts| artifacts.nockswap_bundle.as_ref())
                .is_some()
            {
                ReplayError::BrowserReplayUnavailable
            } else {
                ReplayError::ScenarioReferenceMissing
            }
        })?;
    let capsule_dir = capsule_path.parent().unwrap_or_else(|| Path::new("."));
    let base = if capsule_dir.file_name().and_then(|name| name.to_str()) == Some("safe-evidence") {
        capsule_dir.parent().unwrap_or(capsule_dir)
    } else {
        capsule_dir
    };
    let path = if Path::new(&reference.path).is_absolute() {
        PathBuf::from(&reference.path)
    } else {
        safe_join(base, Path::new(&reference.path))?
    };
    let metadata = fs::metadata(&path).map_err(|source| ReplayError::Read {
        path: path.clone(),
        source,
    })?;
    if metadata.len().to_string() != reference.size_bytes || sha256_file(&path)? != reference.sha256
    {
        return Err(ReplayError::ScenarioIdentityMismatch(path));
    }
    Ok(path)
}

fn read_scenario(path: &Path) -> Result<WithdrawalFaultTrace, ReplayError> {
    let input = fs::read_to_string(path).map_err(|source| ReplayError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let header: Value = serde_json::from_str(&input)?;
    if header.get("schema_id").and_then(Value::as_str) == Some(ACTION_SCENARIO_SCHEMA_ID) {
        Ok(WithdrawalActionScenarioV1::from_json(&input)?.trace)
    } else {
        Ok(WithdrawalFaultTrace::from_json(&input)?)
    }
}

fn validate_capsule_scenario_binding(
    capsule: &WithdrawalEvidenceCapsuleV1,
    scenario: &WithdrawalFaultTrace,
) -> Result<(), ReplayError> {
    if capsule.run.seed != scenario.seed
        || capsule.environment.environment_id != scenario.environment.environment_id
    {
        return Err(ReplayError::ScenarioCapsuleMismatch);
    }
    let expected_backend = match capsule.environment.mode {
        EvidenceEnvironmentMode::Hermetic => "hermetic",
        EvidenceEnvironmentMode::BaseSepoliaFork => "base-sepolia-fork",
    };
    if scenario.environment.backend != expected_backend && scenario.environment.backend != "fake" {
        return Err(ReplayError::ScenarioCapsuleMismatch);
    }
    Ok(())
}

fn artifact_differences(
    expected: Option<&E2eArtifacts>,
    observed: Option<&E2eArtifacts>,
) -> Vec<ArtifactDifference> {
    let expected = expected.map(artifact_identity_map).unwrap_or_default();
    let observed = observed.map(artifact_identity_map).unwrap_or_default();
    expected
        .keys()
        .chain(observed.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|role| {
            let left = expected.get(role).cloned();
            let right = observed.get(role).cloned();
            (left != right).then(|| ArtifactDifference {
                role: role.clone(),
                expected_sha256: left,
                observed_sha256: right,
            })
        })
        .collect()
}

fn artifact_identity_map(artifacts: &E2eArtifacts) -> BTreeMap<String, String> {
    let mut values = BTreeMap::from([
        ("bridge".to_owned(), artifacts.bridge.sha256.clone()),
        ("node".to_owned(), artifacts.node.sha256.clone()),
        ("bridge_jam".to_owned(), artifacts.bridge_jam.sha256.clone()),
        (
            "roswell_jam".to_owned(),
            artifacts.roswell_jam.sha256.clone(),
        ),
        (
            "fakenet_genesis_jam".to_owned(),
            artifacts.fakenet_genesis_jam.sha256.clone(),
        ),
    ]);
    if let Some(ctl) = &artifacts.sequencer_ctl {
        values.insert("sequencer_ctl".to_owned(), ctl.sha256.clone());
    }
    values
}
fn verify_recorded_iris_artifact(
    capsule: Option<&WithdrawalEvidenceCapsuleV1>,
) -> Result<(), ReplayError> {
    let Some(iris) = capsule
        .and_then(|capsule| capsule.artifacts.as_ref())
        .and_then(|artifacts| artifacts.iris.as_ref())
    else {
        return Ok(());
    };
    if !iris.tarball_path.is_file() {
        return Err(ReplayError::MissingIrisArtifact(iris.tarball_path.clone()));
    }
    let observed = sha256_file(&iris.tarball_path)?;
    if observed != iris.tarball_sha256 {
        return Err(ReplayError::IrisArtifactMismatch {
            expected: iris.tarball_sha256.clone(),
            observed,
        });
    }
    Ok(())
}

fn read_artifact_manifest(path: &Path) -> Result<E2eArtifacts, ReplayError> {
    let input = fs::read(path).map_err(|source| ReplayError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&input).map_err(|source| ReplayError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn normalized_semantics(capsule: &WithdrawalEvidenceCapsuleV1) -> Result<Value, ReplayError> {
    let mut value = capsule.normalized_value()?;
    if let Some(object) = value.as_object_mut() {
        object.remove("artifacts");
        object.remove("external_artifacts");
    }
    Ok(value)
}

fn exact_semantics(capsule: &WithdrawalEvidenceCapsuleV1) -> Result<Value, ReplayError> {
    let mut value = serde_json::to_value(capsule)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("artifacts");
        object.remove("external_artifacts");
        object.remove("normalized_evidence_sha256");
        if let Some(run) = object.get_mut("run").and_then(Value::as_object_mut) {
            run.remove("run_id");
        }
    }
    Ok(value)
}

fn differing_paths(left: &Value, right: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    collect_differences("$", left, right, &mut paths);
    paths.sort();
    paths.truncate(64);
    paths
}

fn collect_differences(prefix: &str, left: &Value, right: &Value, paths: &mut Vec<String>) {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        collect_differences(&format!("{prefix}.{key}"), left, right, paths)
                    }
                    _ => paths.push(format!("{prefix}.{key}")),
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            let length = left.len().max(right.len());
            for index in 0..length {
                match (left.get(index), right.get(index)) {
                    (Some(left), Some(right)) => {
                        collect_differences(&format!("{prefix}[{index}]"), left, right, paths)
                    }
                    _ => paths.push(format!("{prefix}[{index}]")),
                }
            }
        }
        _ if left != right => paths.push(prefix.to_owned()),
        _ => {}
    }
}

fn safe_join(base: &Path, relative: &Path) -> Result<PathBuf, ReplayError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(ReplayError::UnsafeScenarioPath(relative.to_path_buf()));
    }
    Ok(base.join(relative))
}

fn external_reference(kind: &str, path: &Path) -> Result<ExternalArtifactReference, ReplayError> {
    let metadata = fs::metadata(path).map_err(|source| ReplayError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(ExternalArtifactReference {
        kind: kind.to_owned(),
        path: path.display().to_string(),
        sha256: sha256_file(path)?,
        size_bytes: metadata.len().to_string(),
        media_type: "application/json".to_owned(),
    })
}

fn sha256_file(path: &Path) -> Result<String, ReplayError> {
    let bytes = fs::read(path).map_err(|source| ReplayError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn hash_json(value: &Value) -> Result<String, ReplayError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn create_replay_run_dir(output_root: &Path) -> Result<PathBuf, ReplayError> {
    fs::create_dir_all(output_root).map_err(|source| ReplayError::Write {
        path: output_root.to_path_buf(),
        source,
    })?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ReplayError::Clock)?;
    let sequence = REPLAY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let run_dir = output_root.join(format!(
        "replay-{}-{}-{sequence}",
        now.as_millis(),
        std::process::id()
    ));
    fs::create_dir(&run_dir).map_err(|source| ReplayError::Write {
        path: run_dir.clone(),
        source,
    })?;
    Ok(run_dir)
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), ReplayError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| ReplayError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&bytes)
        .map_err(|source| ReplayError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| ReplayError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("replay input has no safe-evidence/report.json or scenario.json: {0}")]
    MissingReplayInput(PathBuf),
    #[error("replay input is not UTF-8 JSON: {0}")]
    InvalidUtf8(PathBuf),
    #[error("evidence capsule does not reference a fault_trace/scenario artifact")]
    ScenarioReferenceMissing,
    #[error("browser replay artifact is present but browser replay is unavailable")]
    BrowserReplayUnavailable,
    #[error("unsafe replay scenario path: {0}")]
    UnsafeScenarioPath(PathBuf),
    #[error("scenario artifact identity differs from capsule reference: {0}")]
    ScenarioIdentityMismatch(PathBuf),
    #[error("scenario seed/environment differs from evidence capsule")]
    ScenarioCapsuleMismatch,
    #[error("original artifact paths are missing or unverifiable")]
    MissingOriginalArtifacts,
    #[error("artifact substitution requires an explicit replacement manifest and approval")]
    ArtifactSubstitutionRequiresApproval,
    #[error("archive RPC is required for Base Sepolia fork replay")]
    ArchiveRpcRequired,
    #[error("archive RPC is valid only for Base Sepolia fork replay")]
    ArchiveRpcOnlyForFork,
    #[error("recorded Iris artifact is missing: {0}")]
    MissingIrisArtifact(PathBuf),
    #[error("recorded Iris artifact hash mismatch: expected {expected}, observed {observed}")]
    IrisArtifactMismatch { expected: String, observed: String },
    #[error("fork capsule is missing pinned source block number/hash")]
    MissingForkIdentity,
    #[error("fork provider is unavailable")]
    ForkProviderUnavailable { source: reqwest::Error },
    #[error("fork provider returned an invalid JSON-RPC block response")]
    ForkProviderResponse,
    #[error("fork block mismatch: expected {expected_number}/{expected_hash}, observed {observed_number}/{observed_hash}")]
    ForkBlockMismatch {
        expected_number: u64,
        expected_hash: String,
        observed_number: u64,
        observed_hash: String,
    },
    #[error("replay executor failed: {0}")]
    Executor(String),
    #[error("system clock precedes Unix epoch")]
    Clock,
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(transparent)]
    JsonValue(#[from] serde_json::Error),
    #[error(transparent)]
    ActionSchema(#[from] crate::actions::ActionSchemaError),
    #[error(transparent)]
    EvidenceSchema(#[from] crate::evidence::EvidenceSchemaError),
    #[error(transparent)]
    Artifact(#[from] crate::artifacts::ArtifactResolveError),
    #[error(transparent)]
    Redaction(#[from] crate::redaction::RedactionError),
    #[error(transparent)]
    EvidenceCollection(#[from] crate::evidence::EvidenceCollectionError),
}
