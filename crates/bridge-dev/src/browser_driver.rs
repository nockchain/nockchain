use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const BROWSER_DRIVER_SCHEMA_VERSION: u64 = 2;
pub const BROWSER_DRIVER_CHAIN_ID: u64 = 31_338;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriverMode {
    Hermetic,
    BaseSepoliaFork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserDriverManifestV2 {
    pub schema_version: u64,
    pub run_id: String,
    pub mode: BrowserDriverMode,
    pub base_url: String,
    pub rpc_url: String,
    pub chain_id: u64,
    pub account: String,
    pub contracts: BTreeMap<String, String>,
    pub bridge_signer_pkhs: Vec<String>,
    pub bridge_threshold: u64,
    pub bridge_lock_root: String,
    pub nockswap_git_revision: String,
    pub iris_git_revision: String,
    pub iris_package_version: String,
    pub iris_tarball_sha256: String,
    pub amount_nocks: String,
    pub destination_v1_pkh: String,
    pub public_status_url: String,
    pub readiness_path: String,
    pub terminal_proof_path: PathBuf,
    pub result_path: PathBuf,
    pub artifact_dir: PathBuf,
}

impl BrowserDriverManifestV2 {
    pub fn validate(&self, run_dir: &Path) -> Result<(), BrowserDriverError> {
        if self.schema_version != BROWSER_DRIVER_SCHEMA_VERSION
            || self.run_id.trim().is_empty()
            || self.chain_id != BROWSER_DRIVER_CHAIN_ID
            || self.amount_nocks.parse::<u64>().is_err()
            || self.bridge_lock_root.trim().is_empty()
            || self.destination_v1_pkh.trim().is_empty()
            || self.readiness_path.trim().is_empty()
            || self.iris_package_version.trim().is_empty()
            || !is_hex_revision(&self.iris_git_revision)
            || !is_hex_revision(&self.nockswap_git_revision)
            || !is_hex_digest(&self.iris_tarball_sha256)
        {
            return Err(BrowserDriverError::InvalidManifest(
                "schema, identity, chain, amount, or destination is invalid",
            ));
        }
        require_loopback(&self.base_url, "base_url")?;
        require_loopback(&self.rpc_url, "rpc_url")?;
        require_loopback(&self.public_status_url, "public_status_url")?;
        if !is_evm_address(&self.account)
            || self.contracts.is_empty()
            || !self.contracts.contains_key("nock")
            || !self.contracts.contains_key("message_inbox")
            || self
                .contracts
                .values()
                .any(|address| !is_evm_address(address))
            || self.bridge_signer_pkhs.is_empty()
            || self
                .bridge_signer_pkhs
                .iter()
                .any(|pkh| pkh.trim().is_empty())
            || self.bridge_threshold == 0
            || self.bridge_threshold as usize > self.bridge_signer_pkhs.len()
        {
            return Err(BrowserDriverError::InvalidManifest(
                "account or contract allowlist is invalid",
            ));
        }
        for path in [&self.terminal_proof_path, &self.result_path, &self.artifact_dir] {
            if !path.starts_with(run_dir) {
                return Err(BrowserDriverError::PathOutsideRun(path.clone()));
            }
        }
        if self.result_path.exists() {
            return Err(BrowserDriverError::RefuseOverwrite(
                self.result_path.clone(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BrowserDriverLaunch {
    pub manifest: BrowserDriverManifestV2,
    pub manifest_path: PathBuf,
    pub nockswap_checkout: PathBuf,
    pub private_key: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWithdrawalStatus {
    Confirmed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWithdrawalResultV2 {
    pub schema_version: u64,
    pub run_id: String,
    pub nockswap_git_revision: String,
    pub status: BrowserWithdrawalStatus,
    pub account: String,
    pub chain_id: u64,
    pub amount_nocks: String,
    pub normalized_destination: String,
    pub calldata_hex: String,
    pub calldata_byte_length: u64,
    pub submitted_transaction_hash: String,
    pub transaction_hash: String,
    pub block_number: String,
    pub block_hash: String,
    pub log_index: u64,
    pub base_event_id: String,
    pub nock_transaction_id: String,
    pub nock_block_id: String,
    pub burn_count: u64,
    pub payout_count: u64,
    pub reload_count: u64,
    pub terminal_proof_observed: bool,
    pub terminal_proof_sha256: String,
    pub history_states: Vec<String>,
}

impl BrowserWithdrawalResultV2 {
    pub fn from_json(input: &str) -> Result<Self, BrowserDriverError> {
        let result: Self = serde_json::from_str(input)?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), BrowserDriverError> {
        if self.schema_version != BROWSER_DRIVER_SCHEMA_VERSION
            || self.run_id.trim().is_empty()
            || !is_hex_revision(&self.nockswap_git_revision)
            || self.chain_id != BROWSER_DRIVER_CHAIN_ID
            || !is_evm_address(&self.account)
            || self.amount_nocks.parse::<u64>().is_err()
            || self.normalized_destination.trim().is_empty()
            || self.calldata_byte_length != 116
            || !is_hex_bytes(&self.calldata_hex, 116)
            || !is_hex_bytes(&self.submitted_transaction_hash, 32)
            || !is_hex_bytes(&self.transaction_hash, 32)
            || self.block_number.parse::<u64>().is_err()
            || !is_hex_bytes(&self.block_hash, 32)
            || !is_hex_bytes(&self.base_event_id, 32)
            || self.nock_transaction_id.trim().is_empty()
            || self.nock_block_id.trim().is_empty()
            || self.burn_count != 1
            || self.payout_count != 1
            || self.reload_count == 0
            || !self.terminal_proof_observed
            || !is_hex_digest(&self.terminal_proof_sha256)
            || self.status != BrowserWithdrawalStatus::Confirmed
        {
            return Err(BrowserDriverError::InvalidResult);
        }
        if self.history_states.last().map(String::as_str) != Some("confirmed")
            || self
                .history_states
                .iter()
                .filter(|state| state.as_str() == "confirmed")
                .count()
                != 1
        {
            return Err(BrowserDriverError::InvalidResult);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserBackendEvidenceV1 {
    pub calldata_hex: String,
    pub transaction_hash: String,
    pub block_number: String,
    pub block_hash: String,
    pub log_index: u64,
    pub base_event_id: String,
    pub nock_transaction_id: String,
    pub nock_block_id: String,
    pub burn_count: u64,
    pub payout_count: u64,
    pub terminal_proof_sha256: String,
}

pub fn verify_browser_backend_parity(
    browser: &BrowserWithdrawalResultV2,
    backend: &BrowserBackendEvidenceV1,
) -> Result<(), BrowserDriverError> {
    browser.validate()?;
    let matches = browser
        .calldata_hex
        .eq_ignore_ascii_case(&backend.calldata_hex)
        && browser
            .transaction_hash
            .eq_ignore_ascii_case(&backend.transaction_hash)
        && browser.block_number == backend.block_number
        && browser.block_hash.eq_ignore_ascii_case(&backend.block_hash)
        && browser.log_index == backend.log_index
        && browser
            .base_event_id
            .eq_ignore_ascii_case(&backend.base_event_id)
        && browser.nock_transaction_id == backend.nock_transaction_id
        && browser.nock_block_id == backend.nock_block_id
        && browser.burn_count == backend.burn_count
        && browser.payout_count == backend.payout_count
        && browser
            .terminal_proof_sha256
            .eq_ignore_ascii_case(&backend.terminal_proof_sha256);
    if matches {
        Ok(())
    } else {
        Err(BrowserDriverError::BackendDivergence)
    }
}

pub fn write_browser_manifest_new(
    run_dir: &Path,
    path: &Path,
    manifest: &BrowserDriverManifestV2,
) -> Result<(), BrowserDriverError> {
    manifest.validate(run_dir)?;
    write_new(path, &serde_json::to_vec_pretty(manifest)?)
}

pub fn run_browser_driver(
    run_dir: &Path,
    launch: &BrowserDriverLaunch,
) -> Result<BrowserWithdrawalResultV2, BrowserDriverError> {
    launch.manifest.validate(run_dir)?;
    if !launch.manifest_path.is_file() {
        return Err(BrowserDriverError::ManifestMissing(
            launch.manifest_path.clone(),
        ));
    }
    if launch.timeout.is_zero() {
        return Err(BrowserDriverError::InvalidManifest(
            "browser timeout must be positive",
        ));
    }
    let stdout_path = run_dir.join("browser.stdout.log");
    let stderr_path = run_dir.join("browser.stderr.log");
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stderr_path)?;
    let child = spawn_browser_process(launch, "e2e/withdrawal.spec.ts", stdout, stderr)?;
    wait_browser_process(child, launch.timeout, stdout_path, stderr_path)?;
    let input = fs::read_to_string(&launch.manifest.result_path)?;
    let manifest_input = fs::read_to_string(&launch.manifest_path)?;
    let manifest_from_disk: BrowserDriverManifestV2 = serde_json::from_str(&manifest_input)?;
    if manifest_from_disk != launch.manifest {
        return Err(BrowserDriverError::ManifestResultMismatch);
    }
    let result = BrowserWithdrawalResultV2::from_json(&input)?;
    let proof_digest = terminal_proof_sha256(&launch.manifest.terminal_proof_path)?;
    if !result
        .terminal_proof_sha256
        .eq_ignore_ascii_case(&proof_digest)
    {
        return Err(BrowserDriverError::BackendDivergence);
    }
    if result.run_id != launch.manifest.run_id
        || result.nockswap_git_revision != launch.manifest.nockswap_git_revision
        || result.account.to_lowercase() != launch.manifest.account.to_lowercase()
        || result.amount_nocks != launch.manifest.amount_nocks
        || result.normalized_destination != launch.manifest.destination_v1_pkh
    {
        return Err(BrowserDriverError::ManifestResultMismatch);
    }
    Ok(result)
}

pub fn run_browser_failure_driver(
    run_dir: &Path,
    launch: &BrowserDriverLaunch,
) -> Result<(), BrowserDriverError> {
    run_browser_followup(
        run_dir, launch, "real-failures", "e2e/withdrawal-real-failures.spec.ts", None,
    )
}

pub fn run_browser_recovery_matrix_driver(
    run_dir: &Path,
    launch: &BrowserDriverLaunch,
) -> Result<(), BrowserDriverError> {
    run_browser_followup(
        run_dir,
        launch,
        "failure-matrix",
        "e2e/withdrawal-failures.spec.ts",
        Some(unused_loopback_status_url()?),
    )?;
    run_browser_followup(
        run_dir, launch, "lifecycle-regression", "e2e/withdrawal-lifecycle.spec.ts", None,
    )
}

fn run_browser_followup(
    run_dir: &Path,
    launch: &BrowserDriverLaunch,
    label: &str,
    spec_path: &str,
    public_status_url: Option<String>,
) -> Result<(), BrowserDriverError> {
    if !launch.manifest_path.is_file() {
        return Err(BrowserDriverError::ManifestMissing(
            launch.manifest_path.clone(),
        ));
    }
    let manifest_input = fs::read_to_string(&launch.manifest_path)?;
    let manifest_from_disk: BrowserDriverManifestV2 = serde_json::from_str(&manifest_input)?;
    if manifest_from_disk != launch.manifest {
        return Err(BrowserDriverError::ManifestResultMismatch);
    }
    let mut followup = launch.clone();
    followup.manifest_path = run_dir.join(format!("browser-{label}-manifest.json"));
    followup.manifest.result_path = run_dir.join(format!("browser-{label}-unused-result.json"));
    followup.manifest.artifact_dir = run_dir.join(format!("browser-{label}-artifacts"));
    if let Some(public_status_url) = public_status_url {
        followup.manifest.public_status_url = public_status_url;
    }
    followup.manifest.validate(run_dir)?;
    write_browser_manifest_new(run_dir, &followup.manifest_path, &followup.manifest)?;
    fs::create_dir_all(&followup.manifest.artifact_dir)?;
    let stdout_path = run_dir.join(format!("browser-{label}.stdout.log"));
    let stderr_path = run_dir.join(format!("browser-{label}.stderr.log"));
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&stderr_path)?;
    let child = spawn_browser_process(&followup, spec_path, stdout, stderr)?;
    wait_browser_process(child, followup.timeout, stdout_path, stderr_path)
}

fn unused_loopback_status_url() -> Result<String, BrowserDriverError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    Ok(format!("http://{address}/withdrawal-status"))
}

fn spawn_browser_process(
    launch: &BrowserDriverLaunch,
    spec_path: &str,
    stdout: std::fs::File,
    stderr: std::fs::File,
) -> Result<Child, BrowserDriverError> {
    let contract_allowlist = launch
        .manifest
        .contracts
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    let nock = launch
        .manifest
        .contracts
        .get("nock")
        .ok_or(BrowserDriverError::InvalidManifest(
            "browser manifest must include Nock",
        ))?;
    let message_inbox = launch.manifest.contracts.get("message_inbox").ok_or(
        BrowserDriverError::InvalidManifest("browser manifest must include MessageInbox"),
    )?;
    Command::new("npm")
        .args(["run", "test:e2e", "--", spec_path, "--project=chromium"])
        .current_dir(&launch.nockswap_checkout)
        .env("NOCKSWAP_E2E_MANIFEST", &launch.manifest_path)
        .env("NOCKSWAP_E2E_BASE_URL", &launch.manifest.base_url)
        .env("NOCKSWAP_E2E_RPC_URL", &launch.manifest.rpc_url)
        .env(
            "NOCKSWAP_E2E_CHAIN_ID",
            launch.manifest.chain_id.to_string(),
        )
        .env("NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID", "nockswap-local-e2e")
        .env("NOCKSWAP_E2E_ACCOUNT", &launch.manifest.account)
        .env("NEXT_PUBLIC_NOCKSWAP_E2E", "1")
        .env("NEXT_PUBLIC_NOCKSWAP_E2E_ORIGIN", &launch.manifest.base_url)
        .env("NEXT_PUBLIC_NOCKSWAP_E2E_RPC_URL", &launch.manifest.rpc_url)
        .env(
            "NEXT_PUBLIC_NOCKSWAP_E2E_CHAIN_ID",
            launch.manifest.chain_id.to_string(),
        )
        .env("NEXT_PUBLIC_NOCKSWAP_E2E_ACCOUNT", &launch.manifest.account)
        .env(
            "NEXT_PUBLIC_NOCKSWAP_E2E_CONTRACT_ALLOWLIST", &contract_allowlist,
        )
        .env(
            "NEXT_PUBLIC_WALLETCONNECT_CHAIN_IDS",
            launch.manifest.chain_id.to_string(),
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_CHAIN_ID",
            launch.manifest.chain_id.to_string(),
        )
        .env("NEXT_PUBLIC_BRIDGE_DEV_NOCK_TOKEN_ADDRESS", nock)
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_MESSAGE_INBOX_ADDRESS", message_inbox,
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_SIGNER_PKHS",
            launch.manifest.bridge_signer_pkhs.join(","),
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_THRESHOLD",
            launch.manifest.bridge_threshold.to_string(),
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_LOCK_ROOT", &launch.manifest.bridge_lock_root,
        )
        .env("NEXT_PUBLIC_BRIDGE_DEV_NOCKCHAIN_CONFIRMATION_DEPTH", "1")
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_PUBLIC_STATUS_URL", &launch.manifest.public_status_url,
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_WITHDRAWAL_WIRE_PROTOCOL", "WithdrawalWireV1",
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_WITHDRAWAL_POLICY_ID", "withdrawal-policy-v1",
        )
        .env(
            "NEXT_PUBLIC_BRIDGE_DEV_IRIS_SDK_VERSION", &launch.manifest.iris_package_version,
        )
        .env(
            "NOCKSWAP_E2E_TIMEOUT_MS",
            launch.timeout.as_millis().to_string(),
        )
        .env("NEXT_PUBLIC_BASE_TO_NOCK_WITHDRAWALS_ENABLED", "true")
        .env("NOCKSWAP_E2E_PRIVATE_KEY", &launch.private_key)
        .env("NOCKSWAP_E2E_CONTRACT_ALLOWLIST", contract_allowlist)
        .env("NOCKSWAP_E2E_ARTIFACT_DIR", &launch.manifest.artifact_dir)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(BrowserDriverError::Filesystem)
}

fn wait_browser_process(
    mut child: Child,
    timeout: Duration,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
) -> Result<(), BrowserDriverError> {
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(BrowserDriverError::Timeout {
                stdout: stdout_path,
                stderr: stderr_path,
            });
        }
        thread::sleep(Duration::from_millis(100));
    };
    if status.success() {
        Ok(())
    } else {
        Err(BrowserDriverError::Failed {
            status: status.code(),
            stdout: stdout_path,
            stderr: stderr_path,
        })
    }
}

pub fn terminal_proof_sha256(path: &Path) -> Result<String, BrowserDriverError> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn require_loopback(value: &str, field: &'static str) -> Result<(), BrowserDriverError> {
    let url = Url::parse(value).map_err(|_| BrowserDriverError::NonLoopback(field))?;
    if !matches!(url.scheme(), "http" | "ws")
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
    {
        return Err(BrowserDriverError::NonLoopback(field));
    }
    Ok(())
}

fn is_evm_address(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_hex_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_hex_bytes(value: &str, bytes: usize) -> bool {
    value.len() == 2 + bytes * 2
        && value.starts_with("0x")
        && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), BrowserDriverError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum BrowserDriverError {
    #[error("invalid browser driver manifest: {0}")]
    InvalidManifest(&'static str),
    #[error("browser driver URL is not loopback: {0}")]
    NonLoopback(&'static str),
    #[error("browser driver path is outside its run directory: {0}")]
    PathOutsideRun(PathBuf),
    #[error("refusing to overwrite browser result: {0}")]
    RefuseOverwrite(PathBuf),
    #[error("browser manifest is missing: {0}")]
    ManifestMissing(PathBuf),
    #[error("browser result is invalid or claims success before terminal proof")]
    InvalidResult,
    #[error("browser result does not match orchestrator manifest")]
    ManifestResultMismatch,
    #[error("browser and backend terminal evidence diverge")]
    BackendDivergence,
    #[error("browser driver timed out; stdout={stdout}, stderr={stderr}")]
    Timeout { stdout: PathBuf, stderr: PathBuf },
    #[error("browser driver failed with status {status:?}; stdout={stdout}, stderr={stderr}")]
    Failed {
        status: Option<i32>,
        stdout: PathBuf,
        stderr: PathBuf,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Filesystem(#[from] std::io::Error),
}
