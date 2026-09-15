use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use bridge_dev::evidence::{
    EvidenceCollector, EvidenceEnvironmentFacts, EvidenceEnvironmentMode, EvidenceRunFacts,
    EvidenceRunStatus, SafeArtifactIndex, WithdrawalEvidenceCapsuleV1,
};
use bridge_dev::redaction::{SecretRedactor, SecretValue};
use bridge_dev::settlement_oracle::ExactNicks;
use serde_json::json;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const CANARY: &str = "super-secret/?+=";

#[test]
fn every_forced_failure_phase_emits_partial_report_and_index() {
    for phase in ["provision", "start", "client", "base", "nock", "oracle", "report"] {
        let root = preserved_root(phase);
        let mut capsule = partial_capsule(phase, 1_000, 2_000);
        let mut collector = EvidenceCollector::new(&root, redactor()).expect("collector");
        let checkpoint = collector
            .checkpoint(phase, &capsule)
            .expect("failure checkpoint");
        assert!(checkpoint.is_file());
        let parsed = WithdrawalEvidenceCapsuleV1::from_json(
            &fs::read_to_string(&checkpoint).expect("read checkpoint"),
        )
        .expect("parse checkpoint");
        assert_eq!(parsed.run.status, EvidenceRunStatus::Failed);

        capsule.run.finished_at_unix_ms = Some(2_001);
        let result = collector.finish(&capsule).expect("finish failure evidence");
        assert!(result.report_path.is_file());
        assert!(result.normalized_report_path.is_file());
        assert!(result.artifact_index_path.is_file());
        let index: SafeArtifactIndex = serde_json::from_str(
            &fs::read_to_string(result.artifact_index_path).expect("read index"),
        )
        .expect("parse index");
        assert_eq!(index.root_sha256, result.root_sha256);
        assert!(index.index_excluded_from_root);
        assert!(index.artifacts.len() >= 3);
    }
}

#[test]
fn canary_secrets_are_removed_from_text_json_toml_urls_and_encoded_forms() {
    let root = preserved_root("redaction");
    let mut collector = EvidenceCollector::new(&root, redactor()).expect("collector");
    let encoded = STANDARD.encode(CANARY.as_bytes());
    let percent = "super-secret%2F%3F%2B%3D";
    let private_field = ["private", "key"].join("_");
    let rpc_field = ["rpc", "url"].join("_");
    let credentialed_url =
        ["https://", "user:", CANARY, "@host/path?", "session=", CANARY].concat();
    let text = format!("plain={CANARY} percent={percent} base64={encoded} url={credentialed_url}");
    collector
        .write_safe_text(Path::new("logs/plain.log"), &text, "text/plain")
        .expect("redact text");
    let mut json_config = json!({
        "nock_token": "0x0000000000000000000000000000000000000001",
        "nested": {"value": encoded},
    });
    let object = json_config
        .as_object_mut()
        .expect("fixture JSON is an object");
    object.insert(private_field.clone(), json!(CANARY));
    object.insert(rpc_field.clone(), json!(credentialed_url));
    collector
        .write_safe_json(Path::new("config/config.json"), &json_config)
        .expect("redact json");
    collector
        .write_safe_toml(
            Path::new("config/config.toml"),
            &format!("{private_field} = \"{CANARY}\"\n{rpc_field} = \"{credentialed_url}\"\n"),
        )
        .expect("redact TOML");
    let source_log = root.join("source.log");
    fs::write(&source_log, format!("source secret {CANARY} {encoded}")).expect("write source log");
    collector
        .collect_process_logs(&[source_log])
        .expect("collect process log");

    let mut capsule = partial_capsule("redaction", 1_000, 2_000);
    let result = collector.finish(&capsule).expect("finish evidence");
    capsule = result.safe_capsule;
    assert!(capsule
        .redaction
        .removed_secret_classes
        .contains(&"private_key".to_owned()));
    for path in regular_files(&root.join("safe-evidence")) {
        let bytes = fs::read(path).expect("read safe output");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(CANARY));
        assert!(!text.contains(percent));
        assert!(!text.contains(&encoded));
    }
    let config = fs::read_to_string(root.join("safe-evidence/config/config.json"))
        .expect("read safe config");
    assert!(config.contains("0x0000000000000000000000000000000000000001"));
}

#[test]
fn binary_logs_stay_private_and_are_only_referenced_by_safe_hash() {
    let root = preserved_root("binary");
    let mut collector = EvidenceCollector::new(&root, redactor()).expect("collector");
    let source = root.join("binary.log");
    fs::write(&source, [0xff, 0x00, 0xfe, 0x01]).expect("write binary log");
    let reference = collector
        .collect_log(&source, Path::new("logs/binary.log"))
        .expect("collect binary log")
        .expect("binary log is external");
    assert!(Path::new(&reference.path).starts_with(root.join("private-evidence")));
    assert_eq!(reference.sha256.len(), 64);
    let mut capsule = partial_capsule("binary", 1_000, 2_000);
    capsule.external_artifacts.push(reference);
    let result = collector.finish(&capsule).expect("finish binary evidence");
    let report = fs::read(result.report_path).expect("read report");
    assert!(!report
        .windows(4)
        .any(|window| window == [0xff, 0x00, 0xfe, 0x01]));
}

#[test]
fn artifact_root_changes_on_semantic_mutation_but_normalized_report_ignores_volatility() {
    let first_root = preserved_root("root-first");
    let second_root = preserved_root("root-second");
    let mut first_collector = EvidenceCollector::new(&first_root, redactor()).expect("collector");
    let mut second_collector = EvidenceCollector::new(&second_root, redactor()).expect("collector");
    first_collector
        .write_safe_text(Path::new("facts/value.txt"), "semantic=a", "text/plain")
        .expect("write first semantic artifact");
    second_collector
        .write_safe_text(Path::new("facts/value.txt"), "semantic=b", "text/plain")
        .expect("write second semantic artifact");
    let first_capsule = partial_capsule("same-scenario", 1_000, 2_000);
    let mut second_capsule = partial_capsule("same-scenario", 9_000, 10_000);
    second_capsule.run.run_id = "volatile-other-run".to_owned();
    let first = first_collector
        .finish(&first_capsule)
        .expect("finish first");
    let second = second_collector
        .finish(&second_capsule)
        .expect("finish second");
    assert_ne!(first.root_sha256, second.root_sha256);
    assert_eq!(
        fs::read(first.normalized_report_path).expect("first normalized report"),
        fs::read(second.normalized_report_path).expect("second normalized report")
    );
}

#[test]
fn partial_write_and_overwrite_are_detected_without_destroying_artifacts() {
    let root = preserved_root("partial");
    fs::create_dir_all(root.join("safe-evidence")).expect("create safe dir");
    fs::write(
        root.join("safe-evidence/.report.json.partial-999"),
        b"partial",
    )
    .expect("write partial marker");
    assert!(EvidenceCollector::new(&root, redactor()).is_err());

    let clean_root = preserved_root("overwrite");
    let mut collector = EvidenceCollector::new(&clean_root, redactor()).expect("collector");
    collector
        .write_safe_text(Path::new("facts/value.txt"), "first", "text/plain")
        .expect("write first");
    assert!(collector
        .write_safe_text(Path::new("facts/value.txt"), "second", "text/plain")
        .is_err());
    assert_eq!(
        fs::read_to_string(clean_root.join("safe-evidence/facts/value.txt"))
            .expect("read preserved artifact"),
        "first"
    );
}

#[test]
fn huge_safe_artifact_is_refused() {
    let root = preserved_root("huge");
    let mut collector = EvidenceCollector::new(&root, redactor()).expect("collector");
    let huge = "x".repeat(16 * 1024 * 1024 + 1);
    assert!(collector
        .write_safe_text(Path::new("logs/huge.log"), &huge, "text/plain")
        .is_err());
}

fn partial_capsule(
    phase: &str,
    started_at_unix_ms: u64,
    finished_at_unix_ms: u64,
) -> WithdrawalEvidenceCapsuleV1 {
    let mut capsule = WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: format!("run-{phase}"),
            scenario: "core-withdrawal".to_owned(),
            seed: 7,
            status: EvidenceRunStatus::Failed,
            error: Some(format!("forced failure at {phase}")),
            started_at_unix_ms,
            finished_at_unix_ms: Some(finished_at_unix_ms),
        },
        EvidenceEnvironmentFacts {
            mode: EvidenceEnvironmentMode::Hermetic,
            environment_id: "hermetic-current-artifacts".to_owned(),
            source_manifest_sha256: None,
            source_chain_id: None,
            source_block_number: None,
            source_block_hash: None,
            local_chain_id: 31_338,
            rpc_endpoint_class: "loopback_anvil".to_owned(),
        },
        bridge_dev::evidence::RedactionDeclaration {
            policy: "withdrawal-e2e-redaction-v1".to_owned(),
            removed_secret_classes: Vec::new(),
            raw_logs_embedded: false,
            external_artifacts_only: true,
        },
    );
    capsule
        .assertions
        .push(bridge_dev::evidence::EvidenceAssertion {
            assertion: "phase_failed".to_owned(),
            status: "failed".to_owned(),
            detail: Some(ExactNicks("18446744073709551615".to_owned()).0),
        });
    capsule
}

fn redactor() -> SecretRedactor {
    SecretRedactor::new([
        SecretValue {
            category: "private_key".to_owned(),
            value: CANARY.to_owned(),
        },
        SecretValue {
            category: "credential".to_owned(),
            value: "user:super-secret".to_owned(),
        },
    ])
    .expect("valid canary declarations")
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-evidence-collection-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).expect("read safe directory") {
            let entry = entry.expect("read safe entry");
            if entry.file_type().expect("safe file type").is_dir() {
                directories.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files
}
