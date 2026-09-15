use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bridge_dev::actions::{
    ActionCapability, ActionEnvironment, ExpectedActionOutcome, WithdrawalActionIntent,
    WithdrawalActionSpec, WithdrawalFaultTrace, FAULT_TRACE_SCHEMA_VERSION,
};
use bridge_dev::artifacts::{ArtifactBuildMetadata, ArtifactFile, ArtifactRole, E2eArtifacts};
use bridge_dev::evidence::{
    EvidenceAssertion, EvidenceEnvironmentFacts, EvidenceEnvironmentMode, EvidenceRunFacts,
    EvidenceRunStatus, ExternalArtifactReference, RedactionDeclaration,
    WithdrawalEvidenceCapsuleV1,
};
use bridge_dev::replay::{
    compare_artifacts, compare_semantics, run_replay, ReplayArtifactResolution, ReplayError,
    ReplayExecutionContext, ReplayExecutor, ReplaySource, SemanticComparisonClass,
};
use sha2::{Digest, Sha256};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn semantic_comparison_distinguishes_exact_volatile_and_divergent() {
    let original = capsule(EvidenceRunStatus::Failed);
    let exact = compare_semantics(Some(&original), &original).expect("exact comparison");
    assert_eq!(exact.class, SemanticComparisonClass::ExactMatch);

    let mut volatile = original.clone();
    volatile.run.run_id = "new-run".to_owned();
    volatile.run.started_at_unix_ms += 100;
    volatile.run.finished_at_unix_ms = Some(999);
    let comparison = compare_semantics(Some(&original), &volatile).expect("volatile comparison");
    assert_eq!(
        comparison.class,
        SemanticComparisonClass::AllowedVolatileDifference
    );

    let mut divergent = volatile;
    divergent.assertions.push(EvidenceAssertion {
        assertion: "terminal proof".to_owned(),
        status: "failed".to_owned(),
        detail: Some("different semantic outcome".to_owned()),
    });
    let comparison = compare_semantics(Some(&original), &divergent).expect("divergence");
    assert_eq!(
        comparison.class,
        SemanticComparisonClass::SemanticDivergence
    );
    assert!(comparison
        .differing_paths
        .iter()
        .any(|path| path.contains("assertions")));
}

#[test]
fn artifact_substitution_requires_approval_and_is_labeled() {
    let expected = artifacts("a");
    let observed = artifacts("b");
    assert!(matches!(
        compare_artifacts(Some(&expected), Some(&observed), false),
        Err(ReplayError::ArtifactSubstitutionRequiresApproval)
    ));
    let resolution =
        compare_artifacts(Some(&expected), Some(&observed), true).expect("approved substitution");
    assert!(
        matches!(
            &resolution,
            ReplayArtifactResolution::Substituted { differences }
                if !differences.is_empty()
                    && differences.iter().all(|difference| {
                        difference.expected_sha256.as_deref() == Some("a")
                            && difference.observed_sha256.as_deref() == Some("b")
                    })
        ),
        "approved replacement was not labeled: {resolution:?}"
    );
}

#[tokio::test]
async fn linked_replay_is_new_and_original_capsule_is_unchanged() {
    let root = preserved_root("linked");
    let source = write_source(&root, EvidenceRunStatus::Failed);
    let original_path = source.capsule_path.clone().expect("capsule path");
    let original_before = fs::read(&original_path).expect("read original before replay");
    let replay_capsule = source.capsule.clone().expect("source capsule");
    let output_root = root.join("linked-replays");
    let mut executor = StaticExecutor {
        output: replay_capsule,
    };
    let result = run_replay(
        source,
        None,
        ReplayArtifactResolution::NotRecorded,
        None,
        &output_root,
        &mut executor,
    )
    .await
    .expect("run replay");
    assert_eq!(
        result.report.comparison.class,
        SemanticComparisonClass::ExactMatch
    );
    assert!(result.report_path.is_file());
    assert!(result.linked_capsule_path.is_file());
    assert_ne!(result.linked_capsule_path, original_path);
    assert_eq!(
        fs::read(&original_path).expect("read original after replay"),
        original_before
    );
    let linked = WithdrawalEvidenceCapsuleV1::from_json(
        &fs::read_to_string(&result.linked_capsule_path).expect("read linked capsule"),
    )
    .expect("parse linked capsule");
    assert!(linked
        .external_artifacts
        .iter()
        .any(|artifact| artifact.kind == "replay_source"));
}

#[test]
fn source_loader_reports_partial_old_missing_and_browser_inputs() {
    let partial_root = preserved_root("partial");
    let source = write_source(&partial_root, EvidenceRunStatus::Failed);
    assert!(source.partial_original);
    assert_eq!(source.scenario.seed, 19);

    let old_path = preserved_root("old").join("old.json");
    fs::create_dir_all(old_path.parent().expect("old parent")).expect("create old parent");
    fs::write(
        &old_path, r#"{"schema_id":"nockchain.bridge.withdrawal-e2e-evidence","schema_version":0}"#,
    )
    .expect("write old capsule");
    assert!(ReplaySource::load(&old_path).is_err());

    let missing = preserved_root("missing");
    assert!(matches!(
        ReplaySource::load(&missing),
        Err(ReplayError::MissingReplayInput(_))
    ));

    let browser_root = preserved_root("browser");
    let safe = browser_root.join("safe-evidence");
    fs::create_dir_all(&safe).expect("create safe directory");
    let mut browser = capsule(EvidenceRunStatus::Failed);
    browser.artifacts = Some(bridge_dev::evidence::EvidenceArtifacts {
        bridge_runtime: artifacts("a"),
        iris: None,
        nockswap_bundle: Some(ExternalArtifactReference {
            kind: "nockswap_bundle".to_owned(),
            path: "bundle.tar".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: "1".to_owned(),
            media_type: "application/gzip".to_owned(),
        }),
    });
    fs::write(
        safe.join("report.json"),
        serde_json::to_vec_pretty(&browser).expect("serialize browser capsule"),
    )
    .expect("write browser capsule");
    assert!(matches!(
        ReplaySource::load(&browser_root),
        Err(ReplayError::BrowserReplayUnavailable)
    ));
}

#[tokio::test]
async fn archive_rpc_rules_are_mode_specific() {
    let hermetic = write_source(&preserved_root("hermetic"), EvidenceRunStatus::Failed);
    assert!(matches!(
        bridge_dev::replay::validate_archive_source(&hermetic, Some("https://archive.invalid"))
            .await,
        Err(ReplayError::ArchiveRpcOnlyForFork)
    ));

    let fork_root = preserved_root("fork");
    let mut fork_capsule = capsule(EvidenceRunStatus::Failed);
    fork_capsule.environment.mode = EvidenceEnvironmentMode::BaseSepoliaFork;
    fork_capsule.environment.source_chain_id = Some(8453);
    fork_capsule.environment.source_block_number = Some(1);
    fork_capsule.environment.source_block_hash = Some(format!("0x{}", "1".repeat(64)));
    let fork = write_custom_source(&fork_root, fork_capsule);
    assert!(matches!(
        bridge_dev::replay::validate_archive_source(&fork, None).await,
        Err(ReplayError::ArchiveRpcRequired)
    ));
}

#[test]
fn scenario_file_without_capsule_has_no_baseline() {
    let root = preserved_root("scenario-only");
    fs::create_dir_all(&root).expect("create scenario root");
    let path = root.join("scenario.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&scenario()).expect("serialize scenario"),
    )
    .expect("write scenario");
    let loaded = ReplaySource::load(&path).expect("load scenario");
    assert!(loaded.capsule.is_none());
    assert!(loaded.partial_original);
    let replay = capsule(EvidenceRunStatus::Failed);
    let comparison = compare_semantics(None, &replay).expect("no baseline comparison");
    assert_eq!(comparison.class, SemanticComparisonClass::NoBaseline);
}

#[derive(Clone)]
struct StaticExecutor {
    output: WithdrawalEvidenceCapsuleV1,
}

#[async_trait]
impl ReplayExecutor for StaticExecutor {
    async fn execute(
        &mut self,
        context: &ReplayExecutionContext,
    ) -> Result<WithdrawalEvidenceCapsuleV1, String> {
        if context.source.scenario.actions.is_empty() {
            return Err("scenario unexpectedly empty".to_owned());
        }
        Ok(self.output.clone())
    }
}

fn write_source(root: &Path, status: EvidenceRunStatus) -> ReplaySource {
    write_custom_source(root, capsule(status))
}

fn write_custom_source(root: &Path, mut evidence: WithdrawalEvidenceCapsuleV1) -> ReplaySource {
    let safe = root.join("safe-evidence");
    fs::create_dir_all(&safe).expect("create safe evidence directory");
    let scenario_path = root.join("scenario.json");
    let scenario_bytes = serde_json::to_vec_pretty(&scenario()).expect("serialize scenario");
    fs::write(&scenario_path, &scenario_bytes).expect("write scenario");
    evidence.external_artifacts.push(ExternalArtifactReference {
        kind: "fault_trace".to_owned(),
        path: "scenario.json".to_owned(),
        sha256: hex::encode(Sha256::digest(&scenario_bytes)),
        size_bytes: scenario_bytes.len().to_string(),
        media_type: "application/json".to_owned(),
    });
    fs::write(
        safe.join("report.json"),
        serde_json::to_vec_pretty(&evidence).expect("serialize evidence"),
    )
    .expect("write evidence");
    ReplaySource::load(root).expect("load replay source")
}

fn scenario() -> WithdrawalFaultTrace {
    WithdrawalFaultTrace {
        schema_version: FAULT_TRACE_SCHEMA_VERSION,
        seed: 19,
        environment: ActionEnvironment {
            environment_id: "replay-env".to_owned(),
            backend: "fake".to_owned(),
            capabilities: BTreeSet::from([
                ActionCapability::Provision,
                ActionCapability::ModelObservation,
            ]),
        },
        overall_timeout_ms: 10_000,
        actions: vec![
            WithdrawalActionSpec {
                id: "provision".to_owned(),
                label: "provision".to_owned(),
                timeout_ms: 1_000,
                expected: ExpectedActionOutcome::Success,
                intent: WithdrawalActionIntent::Provision { reset: true },
            },
            WithdrawalActionSpec {
                id: "query".to_owned(),
                label: "query".to_owned(),
                timeout_ms: 1_000,
                expected: ExpectedActionOutcome::Success,
                intent: WithdrawalActionIntent::QueryFacts,
            },
        ],
    }
}

fn capsule(status: EvidenceRunStatus) -> WithdrawalEvidenceCapsuleV1 {
    WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: "original-run".to_owned(),
            scenario: "replay-scenario".to_owned(),
            seed: 19,
            status,
            error: Some("original failure".to_owned()),
            started_at_unix_ms: 10,
            finished_at_unix_ms: Some(20),
        },
        EvidenceEnvironmentFacts {
            mode: EvidenceEnvironmentMode::Hermetic,
            environment_id: "replay-env".to_owned(),
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

fn artifacts(hash: &str) -> E2eArtifacts {
    E2eArtifacts {
        bridge: artifact(ArtifactRole::BridgeBinary, "bridge", hash),
        node: artifact(ArtifactRole::NodeBinary, "node", hash),
        miner: artifact(ArtifactRole::MinerBinary, "miner", hash),
        wallet: artifact(ArtifactRole::WalletBinary, "wallet", hash),
        sequencer_ctl: Some(artifact(ArtifactRole::SequencerCtlBinary, "ctl", hash)),
        bridge_jam: artifact(ArtifactRole::BridgeJam, "bridge.jam", hash),
        roswell_jam: artifact(ArtifactRole::RoswellJam, "roswell.jam", hash),
        fakenet_genesis_jam: artifact(ArtifactRole::FakenetGenesisJam, "genesis.jam", hash),
        build: ArtifactBuildMetadata {
            package_version: "0.1.15".to_owned(),
            git_revision: Some("revision".to_owned()),
            target_arch: "arm64".to_owned(),
            target_os: "macos".to_owned(),
        },
    }
}

fn artifact(role: ArtifactRole, name: &str, hash: &str) -> ArtifactFile {
    ArtifactFile {
        role,
        path: PathBuf::from(name),
        sha256: hash.to_owned(),
        size_bytes: 1,
        modified_unix_seconds: Some(1),
        architecture: None,
    }
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-replay-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
