use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::{sleep, timeout, timeout_at, Instant};

use crate::artifacts::{ArtifactOverrides, ArtifactResolveOptions, ArtifactResolver, E2eArtifacts};
use crate::evidence::{
    EvidenceCollector, EvidenceEnvironmentFacts, EvidenceEnvironmentMode, EvidenceRunFacts,
    EvidenceRunStatus, RedactionDeclaration, WithdrawalEvidenceCapsuleV1,
};
use crate::iris_artifact::IrisArtifactInput;
use crate::iris_driver::BurnSubmissionProof;
use crate::redaction::SecretRedactor;
use crate::settlement_oracle::{SettlementConservationProof, TerminalWithdrawalProof};

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum E2eBaseMode {
    Hermetic,
    BaseSepoliaFork,
}

impl E2eBaseMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Hermetic => "hermetic",
            Self::BaseSepoliaFork => "base-sepolia-fork",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum E2eClientMode {
    RustReference,
    Iris,
}

impl E2eClientMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::RustReference => "rust-reference",
            Self::Iris => "iris",
        }
    }
}

#[derive(Debug, Clone)]
pub struct E2eRunConfig {
    pub workspace_root: PathBuf,
    pub run_root: Option<PathBuf>,
    pub artifact_manifest: Option<PathBuf>,
    pub report_path: Option<PathBuf>,
    pub build_artifacts: bool,
    pub require_ctl: bool,
    pub keep_artifacts: bool,
    pub timeout: Duration,
    pub base: E2eBaseMode,
    pub archive_rpc_url: Option<String>,
    pub iris_artifact: Option<IrisArtifactInput>,
    pub client: E2eClientMode,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub struct E2eRunContext {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub artifacts: E2eArtifacts,
    pub base: E2eBaseMode,
    pub archive_rpc_url: Option<String>,
    pub iris_artifact: Option<IrisArtifactInput>,
    pub client: E2eClientMode,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioExecution {
    pub steps_executed: u64,
    pub facts: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreWithdrawalPhase {
    Pending,
    Ready,
    Submitted,
    SequencerConfirmed,
    Terminal,
}

impl CoreWithdrawalPhase {
    const ORDER: [Self; 5] = [
        Self::Pending,
        Self::Ready,
        Self::Submitted,
        Self::SequencerConfirmed,
        Self::Terminal,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreWithdrawalEvidence {
    pub burn: BurnSubmissionProof,
    pub terminal: TerminalWithdrawalProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedCoreWithdrawalFacts {
    pub client_mode: String,
    pub iris_git_revision: String,
    pub iris_tarball_sha256: String,
    pub calldata_hex: String,
    pub amount_base_units: String,
    pub amount_nicks: String,
    pub commitment: String,
    pub recipient_lock_root: String,
    pub withdrawal_id: String,
    pub nock_transaction_id: String,
    pub nock_inclusion_height: u64,
    pub nock_inclusion_block_id: String,
    pub settlement: SettlementConservationProof,
}

#[derive(Debug, Default)]
pub struct CoreWithdrawalProgress {
    phases: Vec<CoreWithdrawalPhase>,
}

impl CoreWithdrawalProgress {
    pub fn record(&mut self, phase: CoreWithdrawalPhase) -> Result<(), CoreWithdrawalError> {
        let expected = CoreWithdrawalPhase::ORDER
            .get(self.phases.len())
            .copied()
            .ok_or(CoreWithdrawalError::PhaseAfterTerminal)?;
        if phase != expected {
            return Err(CoreWithdrawalError::UnexpectedPhase {
                expected,
                observed: phase,
            });
        }
        self.phases.push(phase);
        Ok(())
    }

    pub fn finish(
        self,
        evidence: CoreWithdrawalEvidence,
    ) -> Result<ScenarioExecution, CoreWithdrawalError> {
        if self.phases != CoreWithdrawalPhase::ORDER {
            return Err(CoreWithdrawalError::IncompletePhases(self.phases));
        }
        validate_core_evidence(&evidence)?;
        Ok(ScenarioExecution {
            steps_executed: CoreWithdrawalPhase::ORDER.len() as u64,
            facts: serde_json::to_value(evidence)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreWithdrawalPrerequisites {
    pub anvil: bool,
    pub bridge_artifacts: bool,
    pub iris_artifact: bool,
}

impl CoreWithdrawalPrerequisites {
    pub fn require(self) -> Result<(), CoreWithdrawalError> {
        let missing = [
            (!self.anvil).then_some("Anvil"),
            (!self.bridge_artifacts).then_some("bridge artifacts"),
            (!self.iris_artifact).then_some("Iris artifact"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CoreWithdrawalError::MissingPrerequisites(missing))
        }
    }
}

pub fn ordinary_burn_calldata(
    full_withdrawal_calldata: &[u8],
) -> Result<Vec<u8>, CoreWithdrawalError> {
    if full_withdrawal_calldata.len() != 116 {
        return Err(CoreWithdrawalError::InvalidFullCalldataLength(
            full_withdrawal_calldata.len(),
        ));
    }
    Ok(full_withdrawal_calldata[..68].to_vec())
}

pub fn normalize_core_withdrawal_evidence(
    evidence: &CoreWithdrawalEvidence,
) -> Result<NormalizedCoreWithdrawalFacts, CoreWithdrawalError> {
    validate_core_evidence(evidence)?;
    let artifact = evidence
        .burn
        .client
        .artifact
        .as_ref()
        .ok_or(CoreWithdrawalError::OfficialIrisRequired)?;
    Ok(NormalizedCoreWithdrawalFacts {
        client_mode: "iris_sdk".to_owned(),
        iris_git_revision: artifact.git_revision.clone(),
        iris_tarball_sha256: artifact.tarball_sha256.clone(),
        calldata_hex: evidence.burn.client.calldata_hex.clone(),
        amount_base_units: evidence.burn.event.amount_base_units.clone(),
        amount_nicks: evidence.burn.event.amount_nicks.clone(),
        commitment: evidence.burn.event.commitment.clone(),
        recipient_lock_root: evidence.burn.event.lock_root.clone(),
        withdrawal_id: evidence.terminal.target.withdrawal_id.clone(),
        nock_transaction_id: evidence.terminal.target.transaction_id.clone(),
        nock_inclusion_height: evidence.terminal.chain.facts.inclusion.height,
        nock_inclusion_block_id: evidence.terminal.chain.facts.inclusion.block_id.clone(),
        settlement: evidence.terminal.settlement.clone(),
    })
}

fn validate_core_evidence(evidence: &CoreWithdrawalEvidence) -> Result<(), CoreWithdrawalError> {
    if !evidence.burn.client.official_client
        || evidence.burn.client.client_mode != crate::client_driver::WithdrawalClientMode::IrisSdk
        || evidence.burn.client.artifact.is_none()
    {
        return Err(CoreWithdrawalError::OfficialIrisRequired);
    }
    if evidence.terminal.stable_observations < 2
        || evidence.terminal.target.base_event_id != evidence.burn.event.base_event_id
        || evidence.terminal.settlement.base_event_id != evidence.burn.event.base_event_id
        || evidence.terminal.settlement.nock_transaction_id
            != evidence.terminal.target.transaction_id
        || !evidence
            .terminal
            .settlement
            .transaction_conservation
            .verdict
        || !evidence.terminal.settlement.burn_to_payout.verdict
    {
        return Err(CoreWithdrawalError::TerminalProofMismatch);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum CoreWithdrawalError {
    #[error("missing core withdrawal prerequisites: {0:?}")]
    MissingPrerequisites(Vec<&'static str>),
    #[error("unexpected phase: expected {expected:?}, observed {observed:?}")]
    UnexpectedPhase {
        expected: CoreWithdrawalPhase,
        observed: CoreWithdrawalPhase,
    },
    #[error("phase recorded after terminal")]
    PhaseAfterTerminal,
    #[error("core withdrawal phases are incomplete: {0:?}")]
    IncompletePhases(Vec<CoreWithdrawalPhase>),
    #[error("core withdrawal requires official Iris artifact evidence")]
    OfficialIrisRequired,
    #[error("terminal proof does not join the Base burn and Nockchain transaction")]
    TerminalProofMismatch,
    #[error("full withdrawal calldata must be 116 bytes, observed {0}")]
    InvalidFullCalldataLength(usize),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[async_trait]
pub trait E2eScenarioExecutor: Send {
    async fn provision(&mut self, context: &E2eRunContext) -> Result<(), String>;
    async fn execute(&mut self, context: &E2eRunContext) -> Result<ScenarioExecution, String>;
    async fn shutdown(&mut self, context: &E2eRunContext) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct E2eReport {
    pub schema_version: u64,
    pub run_id: String,
    pub run_dir: PathBuf,
    pub environment: String,
    pub client: String,
    pub seed: u64,
    pub status: String,
    pub steps_executed: u64,
    pub facts: Value,
    pub errors: Vec<String>,
    pub shutdown_attempted: bool,
    pub artifacts_preserved: bool,
    pub keep_artifacts_requested: bool,
    pub artifacts: E2eArtifacts,
}

#[derive(Debug, Clone)]
pub struct E2eRunOutcome {
    pub report: E2eReport,
    pub report_path: PathBuf,
}

impl E2eRunOutcome {
    pub fn success(&self) -> bool {
        self.report.status == "passed"
    }

    pub fn stable_output_lines(&self) -> [String; 5] {
        [
            format!("run_id={}", self.report.run_id),
            format!("run_dir={}", self.report.run_dir.display()),
            format!("environment={}", self.report.environment),
            format!("seed={}", self.report.seed),
            format!("report={}", self.report_path.display()),
        ]
    }
}

pub struct E2eRunner;

impl E2eRunner {
    pub async fn run<E: E2eScenarioExecutor + ?Sized>(
        config: E2eRunConfig,
        executor: &mut E,
        mut cancellation: watch::Receiver<bool>,
    ) -> Result<E2eRunOutcome, E2eRunnerError> {
        if config.timeout.is_zero() {
            return Err(E2eRunnerError::InvalidTimeout);
        }
        let (run_id, run_dir) = allocate_run_dir(&config)?;
        let artifacts = match resolve_artifacts(&config) {
            Ok(artifacts) => artifacts,
            Err(error) => {
                write_early_failure_evidence(&config, &run_id, &run_dir, &error.to_string())?;
                return Err(error);
            }
        };
        let context = E2eRunContext {
            run_id: run_id.clone(),
            run_dir: run_dir.clone(),
            artifacts: artifacts.clone(),
            base: config.base,
            archive_rpc_url: config.archive_rpc_url.clone(),
            iris_artifact: config.iris_artifact.clone(),
            client: config.client,
            seed: config.seed,
        };
        let deadline = Instant::now() + config.timeout;
        let mut errors = Vec::new();
        let mut steps_executed = 0;
        let mut facts = Value::Null;
        let provision = run_phase(
            "provision",
            deadline,
            &mut cancellation,
            executor.provision(&context),
        )
        .await;
        if let Err(error) = provision {
            errors.push(error);
        } else {
            match run_phase(
                "execute",
                deadline,
                &mut cancellation,
                executor.execute(&context),
            )
            .await
            {
                Ok(execution) if execution.steps_executed > 0 => {
                    steps_executed = execution.steps_executed;
                    facts = execution.facts;
                }
                Ok(_) => errors.push("execute: selected scenario executed zero steps".to_owned()),
                Err(error) => errors.push(error),
            }
        }

        let shutdown_attempted = true;
        match timeout(Duration::from_secs(5), executor.shutdown(&context)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(format!("shutdown: {error}")),
            Err(_) => errors.push("shutdown: timed out".to_owned()),
        }
        let status = if errors.is_empty() {
            "passed"
        } else {
            "failed"
        };
        let report = E2eReport {
            schema_version: 1,
            run_id,
            run_dir: run_dir.clone(),
            environment: config.base.id().to_owned(),
            client: config.client.id().to_owned(),
            seed: config.seed,
            status: status.to_owned(),
            steps_executed,
            facts,
            errors,
            shutdown_attempted,
            artifacts_preserved: true,
            keep_artifacts_requested: config.keep_artifacts,
            artifacts,
        };
        let report_path = config
            .report_path
            .unwrap_or_else(|| run_dir.join("report.json"));
        write_report(&report_path, &report)?;
        Ok(E2eRunOutcome {
            report,
            report_path,
        })
    }
}

async fn run_phase<T, F>(
    phase: &'static str,
    deadline: Instant,
    cancellation: &mut watch::Receiver<bool>,
    future: F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    if *cancellation.borrow() {
        return Err(format!("{phase}: cancelled"));
    }
    let selected = async {
        tokio::select! {
            result = future => result.map_err(|error| format!("{phase}: {error}")),
            changed = cancellation.changed() => {
                match changed {
                    Ok(()) if *cancellation.borrow() => Err(format!("{phase}: cancelled")),
                    Ok(()) => Err(format!("{phase}: cancellation channel changed without cancellation")),
                    Err(_) => Err(format!("{phase}: cancellation channel closed")),
                }
            }
        }
    };
    timeout_at(deadline, selected)
        .await
        .map_err(|_| format!("{phase}: timed out"))?
}

fn resolve_artifacts(config: &E2eRunConfig) -> Result<E2eArtifacts, E2eRunnerError> {
    let mut options = ArtifactResolveOptions::new(config.workspace_root.clone());
    options.build = config.build_artifacts;
    options.require_ctl = config.require_ctl;
    if let Some(path) = &config.artifact_manifest {
        let input = fs::read(path).map_err(|source| E2eRunnerError::ArtifactManifestRead {
            path: path.clone(),
            source,
        })?;
        let expected: E2eArtifacts = serde_json::from_slice(&input).map_err(|source| {
            E2eRunnerError::ArtifactManifestJson {
                path: path.clone(),
                source,
            }
        })?;
        options.overrides = ArtifactOverrides {
            bridge: Some(expected.bridge.path.clone()),
            node: Some(expected.node.path.clone()),
            sequencer_ctl: expected
                .sequencer_ctl
                .as_ref()
                .map(|file| file.path.clone()),
            bridge_jam: Some(expected.bridge_jam.path.clone()),
            roswell_jam: Some(expected.roswell_jam.path.clone()),
            fakenet_genesis_jam: Some(expected.fakenet_genesis_jam.path.clone()),
        };
        let observed = ArtifactResolver::resolve(&options)?;
        if artifact_hashes(&observed) != artifact_hashes(&expected) {
            return Err(E2eRunnerError::ArtifactManifestDrift);
        }
        Ok(observed)
    } else {
        ArtifactResolver::resolve(&options).map_err(Into::into)
    }
}

fn artifact_hashes(artifacts: &E2eArtifacts) -> Vec<&str> {
    let mut hashes = vec![
        artifacts.bridge.sha256.as_str(),
        artifacts.node.sha256.as_str(),
        artifacts.bridge_jam.sha256.as_str(),
        artifacts.roswell_jam.sha256.as_str(),
        artifacts.fakenet_genesis_jam.sha256.as_str(),
    ];
    if let Some(ctl) = &artifacts.sequencer_ctl {
        hashes.push(ctl.sha256.as_str());
    }
    hashes
}

fn allocate_run_dir(config: &E2eRunConfig) -> Result<(String, PathBuf), E2eRunnerError> {
    let base = config
        .run_root
        .clone()
        .unwrap_or_else(|| config.workspace_root.join("target/bridge-e2e-runs"));
    fs::create_dir_all(&base).map_err(|source| E2eRunnerError::RunDirectory {
        path: base.clone(),
        source,
    })?;
    for _ in 0..32 {
        let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let run_id = format!("withdrawal-{timestamp}-{}-{counter}", std::process::id());
        let run_dir = base.join(&run_id);
        match fs::create_dir(&run_dir) {
            Ok(()) => {
                set_private_permissions(&run_dir)?;
                return Ok((run_id, run_dir));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(E2eRunnerError::RunDirectory {
                    path: run_dir,
                    source,
                });
            }
        }
    }
    Err(E2eRunnerError::RunIdExhausted)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), E2eRunnerError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|source| E2eRunnerError::RunDirectory {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).map_err(|source| E2eRunnerError::RunDirectory {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), E2eRunnerError> {
    Ok(())
}

fn write_early_failure_evidence(
    config: &E2eRunConfig,
    run_id: &str,
    run_dir: &Path,
    error: &str,
) -> Result<(), E2eRunnerError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let mode = match config.base {
        E2eBaseMode::Hermetic => EvidenceEnvironmentMode::Hermetic,
        E2eBaseMode::BaseSepoliaFork => EvidenceEnvironmentMode::BaseSepoliaFork,
    };
    let capsule = WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: run_id.to_owned(),
            scenario: "withdrawal".to_owned(),
            seed: config.seed,
            status: EvidenceRunStatus::Failed,
            error: Some(error.to_owned()),
            started_at_unix_ms: now,
            finished_at_unix_ms: Some(now),
        },
        EvidenceEnvironmentFacts {
            mode,
            environment_id: config.base.id().to_owned(),
            source_manifest_sha256: None,
            source_chain_id: (config.base == E2eBaseMode::BaseSepoliaFork).then_some(84_532),
            source_block_number: None,
            source_block_hash: None,
            local_chain_id: 31_338,
            rpc_endpoint_class: "loopback_anvil".to_owned(),
        },
        RedactionDeclaration {
            policy: "withdrawal-e2e-redaction-v1".to_owned(),
            removed_secret_classes: Vec::new(),
            raw_logs_embedded: false,
            external_artifacts_only: true,
        },
    );
    let redactor = SecretRedactor::new(Vec::new())?;
    let mut collector = EvidenceCollector::new(run_dir, redactor)?;
    collector.checkpoint("artifact-resolution", &capsule)?;
    collector.finish(&capsule)?;
    Ok(())
}

fn write_report(path: &Path, report: &E2eReport) -> Result<(), E2eRunnerError> {
    let bytes = serde_json::to_vec_pretty(report)?;
    fs::write(path, bytes).map_err(|source| E2eRunnerError::ReportWrite {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, Error)]
pub enum E2eRunnerError {
    #[error("E2E timeout must be nonzero")]
    InvalidTimeout,
    #[error(transparent)]
    Artifacts(#[from] crate::artifacts::ArtifactResolveError),
    #[error("failed to read artifact manifest {path}")]
    ArtifactManifestRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse artifact manifest {path}")]
    ArtifactManifestJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("artifact manifest hashes do not match current files")]
    ArtifactManifestDrift,
    #[error("failed to create E2E run directory {path}")]
    RunDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to allocate a unique E2E run id")]
    RunIdExhausted,
    #[error("failed to write E2E report {path}")]
    ReportWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Evidence(#[from] crate::evidence::EvidenceCollectionError),
    #[error(transparent)]
    Redaction(#[from] crate::redaction::RedactionError),
    #[error(transparent)]
    ReportJson(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedPlan {
    Success,
    ProvisionFailure,
    AssertionFailure,
    ShutdownFailure,
    ZeroSteps,
    WaitForCancellation,
}

pub struct ScriptedE2eExecutor {
    plan: ScriptedPlan,
}

impl ScriptedE2eExecutor {
    pub fn new(plan: ScriptedPlan) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl E2eScenarioExecutor for ScriptedE2eExecutor {
    async fn provision(&mut self, context: &E2eRunContext) -> Result<(), String> {
        fs::write(context.run_dir.join("provisioned"), b"1").map_err(|error| error.to_string())?;
        if self.plan == ScriptedPlan::ProvisionFailure {
            Err("scripted provision failure".to_owned())
        } else {
            Ok(())
        }
    }

    async fn execute(&mut self, context: &E2eRunContext) -> Result<ScenarioExecution, String> {
        match self.plan {
            ScriptedPlan::AssertionFailure => Err("scripted assertion failure".to_owned()),
            ScriptedPlan::ZeroSteps => Ok(ScenarioExecution {
                steps_executed: 0,
                facts: Value::Null,
            }),
            ScriptedPlan::WaitForCancellation => {
                sleep(Duration::from_secs(60)).await;
                Ok(ScenarioExecution {
                    steps_executed: 1,
                    facts: json!({"unexpected": "wait completed"}),
                })
            }
            _ => {
                fs::write(context.run_dir.join("executed"), b"1")
                    .map_err(|error| error.to_string())?;
                Ok(ScenarioExecution {
                    steps_executed: 1,
                    facts: json!({"scripted": true}),
                })
            }
        }
    }

    async fn shutdown(&mut self, context: &E2eRunContext) -> Result<(), String> {
        fs::write(context.run_dir.join("shutdown"), b"1").map_err(|error| error.to_string())?;
        if self.plan == ScriptedPlan::ShutdownFailure {
            Err("scripted shutdown failure".to_owned())
        } else {
            Ok(())
        }
    }
}

pub struct UnavailableE2eExecutor;

#[async_trait]
impl E2eScenarioExecutor for UnavailableE2eExecutor {
    async fn provision(&mut self, _context: &E2eRunContext) -> Result<(), String> {
        Err("selected withdrawal backend is not wired into the cluster runner yet".to_owned())
    }

    async fn execute(&mut self, _context: &E2eRunContext) -> Result<ScenarioExecution, String> {
        Err("withdrawal execution is unavailable without provisioning".to_owned())
    }

    async fn shutdown(&mut self, _context: &E2eRunContext) -> Result<(), String> {
        Ok(())
    }
}
