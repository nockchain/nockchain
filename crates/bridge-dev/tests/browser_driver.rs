use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bridge_dev::browser_driver::{
    terminal_proof_sha256, verify_browser_backend_parity, write_browser_manifest_new,
    BrowserBackendEvidenceV1, BrowserDriverError, BrowserDriverManifestV2, BrowserDriverMode,
    BrowserWithdrawalResultV2, BrowserWithdrawalStatus, BROWSER_DRIVER_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn loopback_manifest_is_strict_and_write_once() {
    let root = preserved_root("manifest");
    fs::create_dir_all(&root).expect("create root");
    let valid_manifest = manifest(&root);
    valid_manifest.validate(&root).expect("valid manifest");
    let path = root.join("browser-manifest.json");
    write_browser_manifest_new(&root, &path, &valid_manifest).expect("write manifest");
    assert!(path.is_file());
    assert!(write_browser_manifest_new(&root, &path, &valid_manifest).is_err());

    let mut unsafe_manifest = valid_manifest.clone();
    unsafe_manifest.rpc_url = "https://mainnet.base.org".to_owned();
    assert!(matches!(
        unsafe_manifest.validate(&root),
        Err(BrowserDriverError::NonLoopback("rpc_url"))
    ));
    let mut wrong_chain = valid_manifest;
    wrong_chain.chain_id = 8453;
    assert!(matches!(
        wrong_chain.validate(&root),
        Err(BrowserDriverError::InvalidManifest(_))
    ));

    let mut missing_contract = manifest(&root);
    missing_contract.contracts.remove("nock");
    assert!(matches!(
        missing_contract.validate(&root),
        Err(BrowserDriverError::InvalidManifest(_))
    ));
    let mut wrong_iris = manifest(&root);
    wrong_iris.iris_tarball_sha256 = "not-a-digest".to_owned();
    assert!(matches!(
        wrong_iris.validate(&root),
        Err(BrowserDriverError::InvalidManifest(_))
    ));
    let mut wrong_nockswap = manifest(&root);
    wrong_nockswap.nockswap_git_revision = "not-a-revision".to_owned();
    assert!(matches!(
        wrong_nockswap.validate(&root),
        Err(BrowserDriverError::InvalidManifest(_))
    ));
}

#[test]
fn browser_success_requires_terminal_proof_single_burn_and_single_payout() {
    let valid = result();
    valid.validate().expect("valid terminal browser result");

    let mut early = valid.clone();
    early.terminal_proof_observed = false;
    assert!(matches!(
        early.validate(),
        Err(BrowserDriverError::InvalidResult)
    ));

    let mut duplicate = valid.clone();
    duplicate.burn_count = 2;
    assert!(matches!(
        duplicate.validate(),
        Err(BrowserDriverError::InvalidResult)
    ));

    let mut duplicate_confirmed = valid;
    duplicate_confirmed
        .history_states
        .push("confirmed".to_owned());
    assert!(matches!(
        duplicate_confirmed.validate(),
        Err(BrowserDriverError::InvalidResult)
    ));
}

#[test]
fn replacement_hash_and_backend_evidence_must_match_exactly() {
    let browser = result();
    assert_ne!(
        browser.submitted_transaction_hash, browser.transaction_hash,
        "fixture exercises replacement-aware final hash"
    );
    let backend = backend(&browser);
    verify_browser_backend_parity(&browser, &backend).expect("backend parity");

    let mut divergent = backend;
    divergent.nock_transaction_id = "different-nock-tx".to_owned();
    assert!(matches!(
        verify_browser_backend_parity(&browser, &divergent),
        Err(BrowserDriverError::BackendDivergence)
    ));
}

#[test]
fn terminal_proof_digest_is_stable() {
    let root = preserved_root("proof");
    fs::create_dir_all(&root).expect("create proof root");
    let path = root.join("terminal-proof.json");
    let bytes = br#"{"terminal":true,"withdrawal_id":"w1"}"#;
    fs::write(&path, bytes).expect("write proof");
    assert_eq!(
        terminal_proof_sha256(&path).expect("proof hash"),
        hex::encode(Sha256::digest(bytes))
    );
}

fn manifest(root: &std::path::Path) -> BrowserDriverManifestV2 {
    BrowserDriverManifestV2 {
        schema_version: BROWSER_DRIVER_SCHEMA_VERSION,
        run_id: "browser-run".to_owned(),
        mode: BrowserDriverMode::Hermetic,
        iris_package_version: "0.3.2".to_owned(),
        base_url: "http://127.0.0.1:3018".to_owned(),
        rpc_url: "http://127.0.0.1:18545".to_owned(),
        chain_id: 31_338,
        account: test_address('a'),
        contracts: BTreeMap::from([
            ("nock".to_owned(), test_address('1')),
            ("message_inbox".to_owned(), test_address('2')),
        ]),
        bridge_signer_pkhs: vec!["0x1234".to_owned()],
        bridge_threshold: 1,
        bridge_lock_root: "87NEwdZCR2EX1SNY6T6fGimN9vQNvsAa75B7EvbqvRhoJZQ5TNqbcCi".to_owned(),
        nockswap_git_revision: "a".repeat(40),
        iris_git_revision: "1".repeat(40),
        iris_tarball_sha256: "2".repeat(64),
        amount_nocks: "100001".to_owned(),
        destination_v1_pkh: "AD6Mw1QUnPUrnVpyj2gW2jT6Jd6WsuZQmPn79XpZoFEocuvV12iDkvh".to_owned(),
        public_status_url: "http://127.0.0.1:19090/status".to_owned(),
        readiness_path: "/readiness".to_owned(),
        terminal_proof_path: root.join("terminal-proof.json"),
        result_path: root.join("browser-result.json"),
        artifact_dir: root.join("browser-artifacts"),
    }
}

fn result() -> BrowserWithdrawalResultV2 {
    BrowserWithdrawalResultV2 {
        schema_version: BROWSER_DRIVER_SCHEMA_VERSION,
        run_id: "browser-run".to_owned(),
        nockswap_git_revision: "a".repeat(40),
        status: BrowserWithdrawalStatus::Confirmed,
        account: test_address('a'),
        chain_id: 31_338,
        amount_nocks: "100001".to_owned(),
        normalized_destination: "AD6Mw1QUnPUrnVpyj2gW2jT6Jd6WsuZQmPn79XpZoFEocuvV12iDkvh"
            .to_owned(),
        calldata_hex: format!("0x{}", "ab".repeat(116)),
        calldata_byte_length: 116,
        submitted_transaction_hash: hex_bytes('1', 32),
        transaction_hash: hex_bytes('2', 32),
        block_number: "10".to_owned(),
        block_hash: hex_bytes('3', 32),
        log_index: 4,
        base_event_id: hex_bytes('4', 32),
        nock_transaction_id: "nock-tx-1".to_owned(),
        nock_block_id: "nock-block-1".to_owned(),
        burn_count: 1,
        payout_count: 1,
        reload_count: 1,
        terminal_proof_observed: true,
        terminal_proof_sha256: "5".repeat(64),
        history_states: vec![
            "awaiting_base".to_owned(),
            "withdrawal_pending".to_owned(),
            "confirmed".to_owned(),
        ],
    }
}

fn backend(browser: &BrowserWithdrawalResultV2) -> BrowserBackendEvidenceV1 {
    BrowserBackendEvidenceV1 {
        calldata_hex: browser.calldata_hex.clone(),
        transaction_hash: browser.transaction_hash.clone(),
        block_number: browser.block_number.clone(),
        block_hash: browser.block_hash.clone(),
        log_index: browser.log_index,
        base_event_id: browser.base_event_id.clone(),
        nock_transaction_id: browser.nock_transaction_id.clone(),
        nock_block_id: browser.nock_block_id.clone(),
        burn_count: 1,
        payout_count: 1,
        terminal_proof_sha256: browser.terminal_proof_sha256.clone(),
    }
}

fn test_address(digit: char) -> String {
    format!("0x{}", digit.to_string().repeat(40))
}

fn hex_bytes(digit: char, bytes: usize) -> String {
    format!("0x{}", digit.to_string().repeat(bytes * 2))
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-browser-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}
