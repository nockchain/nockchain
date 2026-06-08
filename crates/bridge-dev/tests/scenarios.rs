#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, thread};

use anyhow::{anyhow, bail, Context, Result};
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::Client as S3Client;
use sha2::{Digest, Sha256};
use tempfile::{Builder as TempDirBuilder, TempDir};

const E2E_ENABLE_ENV: &str = "BRIDGE_DEV_RUN_E2E";
const E2E_PORT_OFFSET_ENV: &str = "BRIDGE_DEV_E2E_PORT_OFFSET";
const TEST_RUN_ROOT_ENV: &str = "BRIDGE_DEV_TEST_RUN_ROOT";
const PORT_OFFSET_ENV: &str = "BRIDGE_DEV_PORT_OFFSET";
const FAKENET_GENESIS_JAM_ENV: &str = "BRIDGE_DEV_FAKENET_GENESIS_JAM";
const FAKENET_POW_LEN_ENV: &str = "BRIDGE_DEV_FAKENET_POW_LEN";
const FAKENET_LOG_DIFFICULTY_ENV: &str = "BRIDGE_DEV_FAKENET_LOG_DIFFICULTY";
const BASE_BLOCKS_CHUNK_ENV: &str = "BRIDGE_DEV_BASE_BLOCKS_CHUNK";
const BRIDGE_SAVE_INTERVAL_MILLIS_ENV: &str = "BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS";
const NOCK_OBSERVER_POLL_MILLIS_ENV: &str = "BRIDGE_NOCK_OBSERVER_POLL_MILLIS";
const RUST_LOG_ENV: &str = "RUST_LOG";
const BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV: &str = "BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED";
const R2_E2E_ENABLE_ENV: &str = "BRIDGE_R2_RUN_E2E";
const R2_E2E_URL_ENV: &str = "BRIDGE_R2_TEST_URL";
const R2_E2E_ENDPOINT_ENV: &str = "BRIDGE_R2_TEST_ENDPOINT";
const R2_E2E_BUCKET_ENV: &str = "BRIDGE_R2_TEST_BUCKET";
const R2_E2E_REGION_ENV: &str = "BRIDGE_R2_TEST_REGION";
const R2_E2E_PREFIX_ENV: &str = "BRIDGE_R2_TEST_PREFIX";
const R2_E2E_ACCESS_KEY_ID_ENV: &str = "BRIDGE_R2_TEST_ACCESS_KEY_ID";
const R2_E2E_SECRET_ACCESS_KEY_ENV: &str = "BRIDGE_R2_TEST_SECRET_ACCESS_KEY";
const R2_E2E_TOKEN_ENV: &str = "BRIDGE_R2_TEST_TOKEN";
const R2_E2E_KEEP_OBJECTS_ENV: &str = "BRIDGE_R2_KEEP_OBJECTS";
const E2E_FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES: &str =
    "nockchain/jams/fakenet-genesis-pow-2-bex-1.jam";
const E2E_DEPOSIT_AMOUNT_NICKS: &str = "6553600001";
const E2E_DEPOSIT_SPEND_TIMEOUT_SECS: u64 = 1_800;
const E2E_WITHDRAWAL_AMOUNT_NOCK: &str = "1001";
const E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS: &str = "10";
const E2E_WITHDRAWAL_PHASE_POLL_SECS: u64 = 30;
const WAIT_WITHDRAWAL_TIMEOUT_FRAGMENT: &str = "timed out waiting for withdrawal";
const REQUIRED_E2E_ENV: &[&str] = &[
    "TENDERLY_ACCESS_KEY", "TENDERLY_ACCOUNT_ID", "TENDERLY_PROJECT_SLUG",
    "TENDERLY_TEST_PRIVATE_KEY",
];
const SECRET_ENV: &[&str] = &[
    "TENDERLY_ACCESS_KEY", "TENDERLY_TEST_PRIVATE_KEY", "BRIDGE_DEV_OWNER_PRIVATE_KEY",
    R2_E2E_ACCESS_KEY_ID_ENV, R2_E2E_SECRET_ACCESS_KEY_ENV, R2_E2E_TOKEN_ENV,
    "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY",
];
const ALL_COMPONENTS: &[&str] =
    &["node", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"];
const ALL_BRIDGE_COMPONENTS: &[&str] =
    &["bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"];
const ALL_BRIDGE_NODES: &[usize] = &[0, 1, 2, 3, 4];

static E2E_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct BridgeDevScenario {
    _tempdir: TempDir,
    workspace_root: PathBuf,
    bridge_dev_bin: PathBuf,
    run_root: PathBuf,
    port_offset: u16,
    up_child: Option<Child>,
    up_stdout: PathBuf,
    up_stderr: PathBuf,
    env_overrides: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedDeposit {
    nonce: u64,
    amount: u64,
    recipient: String,
    tx_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservedDepositPhase {
    Submitted,
    Successful,
}

impl ObservedDepositPhase {
    fn cli_flag(self) -> &'static str {
        match self {
            Self::Submitted => "--submitted",
            Self::Successful => "--successful",
        }
    }

    fn output_label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Successful => "successful",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedWithdrawal {
    phase: String,
    id: String,
    as_of: String,
    base_event: String,
    nonce: String,
    proposal_status: String,
    sequenced_state: String,
    handoff_owner: String,
    transaction_name: String,
    proposal_hash: String,
    authorized_transaction_name: String,
}

#[derive(Debug, Clone)]
struct R2ScenarioJournal {
    endpoint: String,
    bucket: String,
    region: String,
    prefix: String,
    journal_id: String,
    access_key_id: String,
    secret_access_key: String,
}

impl R2ScenarioJournal {
    fn from_env(test_name: &str) -> Result<Option<Self>> {
        if !matches!(
            env::var(R2_E2E_ENABLE_ENV).ok().as_deref(),
            Some("1" | "true" | "yes")
        ) {
            eprintln!(
                "skipping R2-backed bridge-dev scenario; set {R2_E2E_ENABLE_ENV}=1 to run it"
            );
            return Ok(None);
        }
        let endpoint = r2_endpoint()?;
        let credentials = r2_credentials(&endpoint.account_id)?;
        let now = unix_now_for_test()?;
        let test_name = sanitize_key_segment(test_name);
        let run_id = format!("{now}-{}", std::process::id());
        let prefix_root = optional_env(R2_E2E_PREFIX_ENV)
            .unwrap_or_else(|| "withdrawal-sequencer-e2e/bridge-dev".to_string());
        Ok(Some(Self {
            endpoint: endpoint.endpoint,
            bucket: endpoint.bucket,
            region: optional_env(R2_E2E_REGION_ENV).unwrap_or_else(|| "auto".to_string()),
            prefix: format!("{prefix_root}/{test_name}/{run_id}"),
            journal_id: format!("bridge-dev-r2-e2e-{test_name}-{run_id}"),
            access_key_id: credentials.access_key_id,
            secret_access_key: credentials.secret_access_key,
        }))
    }

    fn env_overrides(&self) -> Vec<(String, String)> {
        vec![
            (
                BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV.to_string(),
                "1".to_string(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ENDPOINT".to_string(),
                self.endpoint.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_BUCKET".to_string(),
                self.bucket.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_REGION".to_string(),
                self.region.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_PREFIX".to_string(),
                self.prefix.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_ID".to_string(),
                self.journal_id.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_ACCESS_KEY_ID".to_string(),
                self.access_key_id.clone(),
            ),
            (
                "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY".to_string(),
                self.secret_access_key.clone(),
            ),
        ]
    }

    fn event_prefix(&self) -> String {
        format!(
            "{}/v1/journals/{}/events/",
            self.prefix.trim_matches('/'),
            self.journal_id
        )
    }

    fn list_event_keys(&self) -> Result<Vec<String>> {
        let client = self.s3_client();
        let bucket = self.bucket.clone();
        let prefix = self.event_prefix();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build R2 cleanup runtime")?;
        runtime.block_on(async move {
            let mut continuation_token = None;
            let mut keys = Vec::new();
            loop {
                let mut request = client
                    .list_objects_v2()
                    .bucket(bucket.clone())
                    .prefix(prefix.clone());
                if let Some(token) = continuation_token {
                    request = request.continuation_token(token);
                }
                let output = request
                    .send()
                    .await
                    .context("failed to list bridge-dev R2 journal objects")?;
                keys.extend(
                    output
                        .contents()
                        .iter()
                        .filter_map(|object| object.key().map(ToString::to_string)),
                );
                continuation_token = output.next_continuation_token().map(ToString::to_string);
                if continuation_token.is_none() {
                    break;
                }
            }
            Ok(keys)
        })
    }

    fn assert_has_events(&self) -> Result<()> {
        let keys = self.list_event_keys()?;
        if keys.is_empty() {
            bail!(
                "R2 journal prefix {} did not contain any event objects",
                self.event_prefix()
            );
        }
        Ok(())
    }

    fn cleanup(&self) -> Result<()> {
        let keys = self.list_event_keys()?;
        if keys.is_empty() {
            return Ok(());
        }
        let client = self.s3_client();
        let bucket = self.bucket.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build R2 cleanup runtime")?;
        runtime.block_on(async move {
            for key in keys {
                client
                    .delete_object()
                    .bucket(bucket.clone())
                    .key(key)
                    .send()
                    .await
                    .context("failed to delete bridge-dev R2 journal object")?;
            }
            Ok(())
        })
    }

    fn s3_client(&self) -> S3Client {
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(self.endpoint.clone())
            .region(Region::new(self.region.clone()))
            .credentials_provider(Credentials::new(
                self.access_key_id.clone(),
                self.secret_access_key.clone(),
                None,
                None,
                "bridge-dev-scenario",
            ))
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();
        S3Client::from_conf(config)
    }
}

impl Drop for R2ScenarioJournal {
    fn drop(&mut self) {
        if matches!(
            env::var(R2_E2E_KEEP_OBJECTS_ENV).ok().as_deref(),
            Some("1" | "true" | "yes")
        ) {
            eprintln!(
                "leaving R2 bridge-dev journal objects for prefix {} because {}=1",
                self.event_prefix(),
                R2_E2E_KEEP_OBJECTS_ENV
            );
            return;
        }
        if let Err(err) = self.cleanup() {
            eprintln!(
                "R2 bridge-dev scenario cleanup failed: {}",
                redact(&err.to_string())
            );
        }
    }
}

#[derive(Debug, Clone)]
struct R2Endpoint {
    endpoint: String,
    account_id: String,
    bucket: String,
}

#[derive(Debug, Clone)]
struct R2Credentials {
    access_key_id: String,
    secret_access_key: String,
}

impl BridgeDevScenario {
    fn new(name: &str) -> Result<Self> {
        let tempdir = scenario_tempdir().context("failed to create bridge-dev scenario tempdir")?;
        let run_root = tempdir.path().join("test_run_data");
        fs::create_dir_all(&run_root)
            .with_context(|| format!("failed to create {}", run_root.display()))?;
        let log_dir = tempdir.path().join("logs");
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create {}", log_dir.display()))?;
        Ok(Self {
            workspace_root: workspace_root()?,
            bridge_dev_bin: bridge_dev_bin(),
            run_root,
            port_offset: scenario_port_offset(name)?,
            up_child: None,
            up_stdout: log_dir.join("up.stdout.log"),
            up_stderr: log_dir.join("up.stderr.log"),
            env_overrides: Vec::new(),
            _tempdir: tempdir,
        })
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.bridge_dev_bin);
        command
            .args(args)
            .current_dir(&self.workspace_root)
            .env(TEST_RUN_ROOT_ENV, &self.run_root)
            .env(PORT_OFFSET_ENV, self.port_offset.to_string());
        if env::var_os(FAKENET_GENESIS_JAM_ENV).is_none() {
            command.env(
                FAKENET_GENESIS_JAM_ENV,
                crates_dir(&self.workspace_root).join(E2E_FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES),
            );
        }
        if env::var_os(FAKENET_POW_LEN_ENV).is_none() {
            command.env(FAKENET_POW_LEN_ENV, "2");
        }
        if env::var_os(FAKENET_LOG_DIFFICULTY_ENV).is_none() {
            command.env(FAKENET_LOG_DIFFICULTY_ENV, "1");
        }
        if env::var_os(BASE_BLOCKS_CHUNK_ENV).is_none() {
            command.env(BASE_BLOCKS_CHUNK_ENV, "10");
        }
        if env::var_os(BRIDGE_SAVE_INTERVAL_MILLIS_ENV).is_none() {
            command.env(BRIDGE_SAVE_INTERVAL_MILLIS_ENV, "1000");
        }
        if env::var_os(NOCK_OBSERVER_POLL_MILLIS_ENV).is_none() {
            command.env(NOCK_OBSERVER_POLL_MILLIS_ENV, "250");
        }
        if env::var_os(RUST_LOG_ENV).is_none() {
            command.env(RUST_LOG_ENV, "info,bridge.withdrawal=debug");
        }
        for (key, value) in &self.env_overrides {
            command.env(key, value);
        }
        command
    }

    fn extend_env_overrides(&mut self, envs: impl IntoIterator<Item = (String, String)>) {
        self.env_overrides.extend(envs);
    }

    fn run_checked(&self, args: &[&str]) -> Result<String> {
        let output = self
            .command(args)
            .output()
            .with_context(|| format!("failed to run bridge-dev {}", args.join(" ")))?;
        match checked_stdout(args, output) {
            Ok(stdout) => Ok(stdout),
            Err(err) if args.first().copied() == Some("status") => Err(err),
            Err(err) => Err(err).with_context(|| self.cluster_context()),
        }
    }

    fn run_checked_retry(&mut self, args: &[&str], timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        while Instant::now() < deadline {
            self.ensure_up_still_running()?;
            match self.run_checked(args) {
                Ok(stdout) => return Ok(stdout),
                Err(err) => {
                    last_error = Some(err);
                    thread::sleep(Duration::from_secs(5));
                }
            }
        }
        match last_error {
            Some(err) => Err(err).with_context(|| {
                format!(
                    "timed out retrying bridge-dev {} for {}s",
                    args.join(" "),
                    timeout.as_secs()
                )
            }),
            None => bail!("timed out retrying bridge-dev {}", args.join(" ")),
        }
    }

    fn spawn_fresh_cluster(&mut self) -> Result<()> {
        self.spawn_cluster(&["up", "--fresh", "--start"], Duration::from_secs(420))
    }

    fn spawn_cluster(&mut self, args: &[&str], status_timeout: Duration) -> Result<()> {
        ensure_release_binaries(&self.workspace_root)?;
        let stdout = File::create(&self.up_stdout)
            .with_context(|| format!("failed to create {}", self.up_stdout.display()))?;
        let stderr = File::create(&self.up_stderr)
            .with_context(|| format!("failed to create {}", self.up_stderr.display()))?;
        let child = self
            .command(args)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("failed to spawn bridge-dev {}", args.join(" ")))?;
        self.up_child = Some(child);
        self.wait_for_status(status_timeout)
            .map(|_| ())
            .with_context(|| format!("{}\n{}", self.up_log_context(), self.cluster_context()))
    }

    fn wait_for_status(&mut self, timeout: Duration) -> Result<String> {
        let stdout =
            self.wait_for_status_command(&["status", "--bridges", "--sequencer"], timeout)?;
        assert_contains_all(&stdout, &["bridge_streams:", "sequencer_status:"])?;
        Ok(stdout)
    }

    fn wait_for_process_status(&mut self, timeout: Duration) -> Result<String> {
        self.wait_for_status_command_allowing_endpoint_failures(&["status"], timeout)
    }

    fn wait_for_status_command(&mut self, args: &[&str], timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        while Instant::now() < deadline {
            self.ensure_up_still_running()?;
            match self.run_checked(args) {
                Ok(stdout) => {
                    assert_contains_all(&stdout, &["processes:"])?;
                    return Ok(stdout);
                }
                Err(err) => {
                    last_error = Some(err);
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
        match last_error {
            Some(err) => Err(err).context("timed out waiting for bridge-dev status"),
            None => bail!("timed out waiting for bridge-dev status"),
        }
    }

    fn wait_for_status_command_allowing_endpoint_failures(
        &mut self,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String> {
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        while Instant::now() < deadline {
            self.ensure_up_still_running()?;
            match self.run_status_command_allowing_endpoint_failures(args) {
                Ok(stdout) => {
                    assert_contains_all(&stdout, &["processes:"])?;
                    return Ok(stdout);
                }
                Err(err) => {
                    last_error = Some(err);
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
        match last_error {
            Some(err) => Err(err).context("timed out waiting for bridge-dev status"),
            None => bail!("timed out waiting for bridge-dev status"),
        }
    }

    fn run_status_command_allowing_endpoint_failures(&self, args: &[&str]) -> Result<String> {
        let output = self
            .command(args)
            .output()
            .with_context(|| format!("failed to run bridge-dev {}", args.join(" ")))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("processes:") {
            return Ok(stdout.into_owned());
        }
        checked_stdout(args, output)
    }

    fn wait_for_deposit_on_node(
        &mut self,
        phase: ObservedDepositPhase,
        node_id: usize,
        timeout_secs: u64,
    ) -> Result<ObservedDeposit> {
        self.wait_for_deposit_on_node_after(phase, node_id, timeout_secs, None)
    }

    fn wait_for_deposit_on_node_after(
        &mut self,
        phase: ObservedDepositPhase,
        node_id: usize,
        timeout_secs: u64,
        after_nonce: Option<u64>,
    ) -> Result<ObservedDeposit> {
        let mut args = vec![
            "wait".to_string(),
            "deposit".to_string(),
            phase.cli_flag().to_string(),
            "--node-id".to_string(),
            node_id.to_string(),
            "--timeout-secs".to_string(),
            timeout_secs.to_string(),
        ];
        if let Some(after_nonce) = after_nonce {
            args.push("--after-nonce".to_string());
            args.push(after_nonce.to_string());
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.run_checked(&arg_refs)?;
        parse_observed_deposit(&output, phase)
    }

    fn wait_for_withdrawal_phase(
        &mut self,
        phase: &str,
        flag: &str,
        timeout_secs: u64,
    ) -> Result<ObservedWithdrawal> {
        self.wait_for_withdrawal_phase_for(phase, flag, timeout_secs, None)
    }

    fn wait_for_withdrawal_phase_for(
        &mut self,
        phase: &str,
        flag: &str,
        timeout_secs: u64,
        target: Option<&ObservedWithdrawal>,
    ) -> Result<ObservedWithdrawal> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut last_error = None;

        loop {
            self.ensure_up_still_running()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            let chunk_secs = remaining.as_secs().clamp(1, E2E_WITHDRAWAL_PHASE_POLL_SECS);
            match self.wait_for_withdrawal_phase_once(phase, flag, chunk_secs, target) {
                Ok(withdrawal) => return Ok(withdrawal),
                Err(err) => {
                    if !err.to_string().contains(WAIT_WITHDRAWAL_TIMEOUT_FRAGMENT) {
                        return Err(err)
                            .with_context(|| format!("failed while waiting for withdrawal {phase}"))
                            .with_context(|| self.cluster_context());
                    }
                    last_error = Some(err);
                }
            }

            if Instant::now() >= deadline {
                break;
            }

            // Withdrawal handoff is measured in confirmed Base blocks, not wall-clock time.
            // Tenderly VNETs can sit at a fixed Base height while the test is otherwise idle, so
            // advance Base between short waits to let degraded/restart scenarios rotate turns.
            self.run_checked(&["advance-base", "--blocks", E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS])
                .with_context(|| {
                    format!("failed to advance Base while waiting for withdrawal {phase}")
                })?;
        }

        match last_error {
            Some(err) => Err(err)
                .with_context(|| {
                    format!("timed out waiting {timeout_secs}s for withdrawal {phase}")
                })
                .with_context(|| self.cluster_context()),
            None => bail!("timed out waiting {timeout_secs}s for withdrawal {phase}"),
        }
    }

    fn wait_for_withdrawal_phase_once(
        &self,
        phase: &str,
        flag: &str,
        timeout_secs: u64,
        target: Option<&ObservedWithdrawal>,
    ) -> Result<ObservedWithdrawal> {
        let timeout = timeout_secs.to_string();
        let mut args = vec![
            "wait".to_string(),
            "withdrawal".to_string(),
            flag.to_string(),
            "--timeout-secs".to_string(),
            timeout,
        ];
        if let Some(target) = target {
            args.extend([
                "--withdrawal-id-as-of-hex".to_string(),
                target.as_of.clone(),
                "--withdrawal-id-base-event-hex".to_string(),
                target.base_event.clone(),
                "--withdrawal-nonce".to_string(),
                target.nonce.clone(),
            ]);
        }
        let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self
            .command(&arg_refs)
            .output()
            .with_context(|| format!("failed to run bridge-dev {}", arg_refs.join(" ")))?;
        let stdout = checked_stdout(&arg_refs, output)?;
        parse_observed_withdrawal(&stdout, phase)
    }

    fn complete_deposit_on_all_nodes(&mut self) -> Result<ObservedDeposit> {
        self.complete_deposit_on_all_nodes_after(None)
    }

    fn complete_deposit_on_all_nodes_after(
        &mut self,
        after_nonce: Option<u64>,
    ) -> Result<ObservedDeposit> {
        self.run_checked_retry(
            &["deposit", "--amount-nicks", E2E_DEPOSIT_AMOUNT_NICKS],
            Duration::from_secs(E2E_DEPOSIT_SPEND_TIMEOUT_SECS),
        )?;
        let submitted = self.wait_for_deposit_on_node_after(
            ObservedDepositPhase::Submitted,
            0,
            240,
            after_nonce,
        )?;
        assert_positive_deposit(&submitted, "submitted")?;
        let successful = self.wait_for_deposit_on_node_after(
            ObservedDepositPhase::Successful,
            0,
            360,
            after_nonce,
        )?;
        assert_same_deposit_identity(
            &submitted, &successful, "node-0 submitted", "node-0 successful",
        )?;
        assert_successful_deposit_on_all_nodes_after(self, &successful, 360, after_nonce)?;
        Ok(successful)
    }

    fn request_withdrawal_after_mint(&self) -> Result<()> {
        self.run_checked(&["mint-for-burn", "--amount-nock", E2E_WITHDRAWAL_AMOUNT_NOCK])?;
        self.run_checked(&["request-withdrawal", "--amount-nock", E2E_WITHDRAWAL_AMOUNT_NOCK])?;
        self.run_checked(&["advance-base", "--blocks", E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS])?;
        Ok(())
    }

    fn wait_for_withdrawal_execution(
        &mut self,
    ) -> Result<(
        ObservedWithdrawal,
        ObservedWithdrawal,
        ObservedWithdrawal,
        ObservedWithdrawal,
    )> {
        let pending = self.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
        let ready = self.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
        let submitted =
            self.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
        let executed =
            self.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
        assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
        Ok((pending, ready, submitted, executed))
    }

    fn restart_all_bridges(&mut self) -> Result<()> {
        self.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
        let stopped = self.wait_for_process_status(Duration::from_secs(120))?;
        assert_processes_not_running(&stopped, ALL_BRIDGE_COMPONENTS)?;
        assert_bridge_reboot_state_present(self, ALL_BRIDGE_NODES)?;
        self.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
        let status = self.wait_for_status(Duration::from_secs(240))?;
        assert_cluster_available(&status)
    }

    fn bridge_data_dir(&self, node_id: usize) -> PathBuf {
        self.run_root.join(format!("bridge-{node_id}"))
    }

    fn sequencer_sqlite_path(&self) -> PathBuf {
        self.run_root
            .join("node")
            .join("nockchain")
            .join("withdrawal-state-store.sqlite")
    }

    fn remove_sequencer_sqlite(&self) -> Result<()> {
        let sqlite_path = self.sequencer_sqlite_path();
        let candidates = [
            sqlite_path.clone(),
            sqlite_path.with_extension("sqlite-wal"),
            sqlite_path.with_extension("sqlite-shm"),
        ];
        for path in candidates {
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
        }
        if self.sequencer_sqlite_path().exists() {
            bail!(
                "sequencer sqlite still exists after removal: {}",
                self.sequencer_sqlite_path().display()
            );
        }
        Ok(())
    }

    fn ensure_up_still_running(&mut self) -> Result<()> {
        if let Some(child) = &mut self.up_child {
            if let Some(status) = child
                .try_wait()
                .context("failed to inspect bridge-dev up")?
            {
                bail!(
                    "bridge-dev up exited early with {status}: {}",
                    self.up_log_context()
                );
            }
        }
        Ok(())
    }

    fn up_log_context(&self) -> String {
        format!(
            "up stdout:\n{}\nup stderr:\n{}",
            redacted_tail(&self.up_stdout),
            redacted_tail(&self.up_stderr)
        )
    }

    fn cluster_context(&self) -> String {
        let current_dir = self.run_root.join("bridge-dev/current");
        let mut context = format!("run root: {}\n", self.run_root.display());
        match self
            .command(&["status", "--bridges", "--sequencer"])
            .output()
        {
            Ok(output) => {
                context.push_str("status stdout:\n");
                context.push_str(&redact(&String::from_utf8_lossy(&output.stdout)));
                context.push_str("\nstatus stderr:\n");
                context.push_str(&redact(&String::from_utf8_lossy(&output.stderr)));
                context.push('\n');
            }
            Err(err) => {
                context.push_str(&format!("status unavailable: {err}\n"));
            }
        }
        let mut log_names = vec![
            "supervisor.log".to_string(),
            "node.stderr.log".to_string(),
            "node.stdout.log".to_string(),
        ];
        for node_id in 0..5 {
            log_names.push(format!("bridge-{node_id}.stderr.log"));
            log_names.push(format!("bridge-{node_id}.stdout.log"));
        }
        for log_name in log_names {
            let path = current_dir.join(log_name);
            context.push_str(&format!(
                "{} tail:\n{}\n",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("log"),
                redacted_tail(&path)
            ));
        }
        context
    }

    fn stop(&mut self) {
        if self.up_child.is_none() {
            return;
        }
        let _ = self.command(&["down"]).output();
        if let Some(child) = &mut self.up_child {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        self.up_child = None;
                        return;
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(250)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.up_child = None;
    }
}

impl Drop for BridgeDevScenario {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn fresh_vnet_boot_reaches_status() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("fresh-boot")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    assert_queue_drained(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_happy_path_reaches_submitted_and_successful() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("deposit-happy-path")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_replays_after_all_bridges_were_down() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("deposit-bridge-downtime")?;

    scenario.spawn_fresh_cluster()?;
    scenario.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, ALL_BRIDGE_COMPONENTS)?;
    scenario.run_checked_retry(
        &["deposit", "--amount-nicks", E2E_DEPOSIT_AMOUNT_NICKS],
        Duration::from_secs(E2E_DEPOSIT_SPEND_TIMEOUT_SECS),
    )?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    let submitted = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Submitted, 0, 360)?;
    let successful = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 0, 480)?;
    assert_same_deposit_identity(
        &submitted, &successful, "post-downtime submitted", "post-downtime successful",
    )?;
    assert_successful_deposit_on_all_nodes(&mut scenario, &successful, 480)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn deposit_state_survives_all_bridge_process_restart() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("deposit-bridge-restart")?;

    scenario.spawn_fresh_cluster()?;
    let deposit = scenario.complete_deposit_on_all_nodes()?;
    scenario.restart_all_bridges()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_successful_deposit_on_all_nodes(&mut scenario, &deposit, 360)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn multiple_deposits_are_ordered_and_visible_on_all_nodes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("multiple-deposits")?;

    scenario.spawn_fresh_cluster()?;
    let first = scenario.complete_deposit_on_all_nodes()?;
    let second = scenario.complete_deposit_on_all_nodes_after(Some(first.nonce))?;
    assert_deposit_nonce_increased(&first, &second)?;
    assert_successful_deposit_on_all_nodes_after(&mut scenario, &second, 360, Some(first.nonce))
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_happy_path_reaches_executed() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("withdrawal-happy-path")?;

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    scenario.wait_for_withdrawal_execution()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)?;
    Ok(())
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_executes_after_ready_bridge_restart() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("withdrawal-ready-restart")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    scenario.restart_all_bridges()?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_confirms_after_sequencer_restart_from_submitted() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("withdrawal-submitted-sequencer-restart")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;

    scenario.run_checked(&["stop", "node"])?;
    let stopped = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&stopped, &["node"])?;

    scenario.run_checked(&["start", "node"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;

    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials, R2 credentials, and release bridge binaries"]
fn withdrawal_sequencer_rebuilds_from_r2_after_sqlite_wipe() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let Some(r2_journal) = R2ScenarioJournal::from_env("sequencer-sqlite-wipe")? else {
        return Ok(());
    };
    let mut scenario = BridgeDevScenario::new("withdrawal-r2-sequencer-recovery")?;
    scenario.extend_env_overrides(r2_journal.env_overrides());

    scenario.spawn_fresh_cluster()?;
    let status = scenario.wait_for_status(Duration::from_secs(60))?;
    assert_cluster_available(&status)?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.request_withdrawal_after_mint()?;
    let pending = scenario.wait_for_withdrawal_phase("Pending", "--pending", 240)?;
    let ready = scenario.wait_for_withdrawal_phase_for("Ready", "--ready", 480, Some(&pending))?;
    r2_journal.assert_has_events()?;

    scenario.run_checked(&["stop", "node"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["node"])?;
    let sqlite_path = scenario.sequencer_sqlite_path();
    if !sqlite_path.exists() {
        bail!(
            "sequencer sqlite did not exist before wipe: {}",
            sqlite_path.display()
        );
    }
    scenario.remove_sequencer_sqlite()?;

    scenario.run_checked(&["start", "node"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_contains_all(&status, &["bridge_streams:", "sequencer_status:"])?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    let submitted =
        scenario.wait_for_withdrawal_phase_for("Submitted", "--submitted", 600, Some(&pending))?;
    let executed =
        scenario.wait_for_withdrawal_phase_for("Executed", "--executed", 720, Some(&pending))?;
    assert_withdrawal_progression(&pending, &ready, &submitted, &executed)?;
    r2_journal.assert_has_events()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn withdrawal_catches_up_after_all_bridge_processes_were_down() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("withdrawal-bridge-downtime")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, ALL_BRIDGE_COMPONENTS)?;
    scenario.request_withdrawal_after_mint()?;
    scenario.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(240))?;
    assert_cluster_available(&status)?;
    scenario.wait_for_withdrawal_execution()?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn two_node_degraded_withdrawal_still_executes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("two-node-degraded-withdrawal")?;

    scenario.spawn_fresh_cluster()?;
    scenario.complete_deposit_on_all_nodes()?;
    scenario.run_checked(&["stop", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["bridge-3", "bridge-4"])?;
    scenario.request_withdrawal_after_mint()?;
    scenario.wait_for_withdrawal_execution()?;
    scenario.run_checked(&["start", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_processes_running(&status, &["bridge-3", "bridge-4"])?;
    assert_sequencer_idle(&status)
}

#[test]
#[ignore = "requires Tenderly VNET credentials and release bridge binaries"]
fn two_node_degraded_deposit_still_completes() -> Result<()> {
    let _guard = e2e_guard();
    if !e2e_enabled()? {
        return Ok(());
    }
    let mut scenario = BridgeDevScenario::new("two-node-degraded-deposit")?;

    scenario.spawn_fresh_cluster()?;
    scenario.run_checked(&["stop", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_process_status(Duration::from_secs(120))?;
    assert_processes_not_running(&status, &["bridge-3", "bridge-4"])?;
    scenario.run_checked_retry(
        &["deposit", "--amount-nicks", E2E_DEPOSIT_AMOUNT_NICKS],
        Duration::from_secs(E2E_DEPOSIT_SPEND_TIMEOUT_SECS),
    )?;
    let submitted = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Submitted, 0, 240)?;
    let deposit = scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 0, 360)?;
    assert_same_deposit_identity(
        &submitted, &deposit, "degraded submitted", "degraded successful",
    )?;
    scenario.run_checked(&["start", "bridge-3", "bridge-4"])?;
    let status = scenario.wait_for_status(Duration::from_secs(120))?;
    assert_cluster_available(&status)?;
    assert_processes_running(&status, &["bridge-3", "bridge-4"])?;
    assert_same_deposit(
        &deposit,
        &scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 3, 360)?,
        "bridge-3",
    )?;
    assert_same_deposit(
        &deposit,
        &scenario.wait_for_deposit_on_node(ObservedDepositPhase::Successful, 4, 360)?,
        "bridge-4",
    )?;
    Ok(())
}

fn e2e_guard() -> MutexGuard<'static, ()> {
    E2E_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn e2e_enabled() -> Result<bool> {
    match env::var(E2E_ENABLE_ENV).ok().as_deref() {
        Some("1") | Some("true") | Some("yes") => {
            let missing = REQUIRED_E2E_ENV
                .iter()
                .copied()
                .filter(|key| {
                    env::var(key)
                        .ok()
                        .is_none_or(|value| value.trim().is_empty())
                })
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                bail!(
                    "{E2E_ENABLE_ENV}=1 but required Tenderly env vars are missing: {}",
                    missing.join(", ")
                );
            }
            Ok(true)
        }
        _ => {
            eprintln!("skipping bridge-dev scenario; set {E2E_ENABLE_ENV}=1 to run it");
            Ok(false)
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_r2_env(name: &str) -> Result<String> {
    optional_env(name).ok_or_else(|| anyhow!("{name} must be set when {R2_E2E_ENABLE_ENV}=1"))
}

fn r2_endpoint() -> Result<R2Endpoint> {
    if let Some(url) = optional_env(R2_E2E_URL_ENV) {
        let parsed = reqwest::Url::parse(&url).context("BRIDGE_R2_TEST_URL must be a valid URL")?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("BRIDGE_R2_TEST_URL must include a host"))?;
        let account_id = host
            .split_once('.')
            .map(|(account_id, _)| account_id.to_string())
            .ok_or_else(|| {
                anyhow!("BRIDGE_R2_TEST_URL host must start with the Cloudflare account id")
            })?;
        let endpoint = format!("{}://{}", parsed.scheme(), host);
        let bucket = parsed.path().trim_matches('/');
        if bucket.is_empty() || bucket.contains('/') {
            bail!("BRIDGE_R2_TEST_URL must include exactly one bucket path segment");
        }
        return Ok(R2Endpoint {
            endpoint,
            account_id,
            bucket: bucket.to_string(),
        });
    }

    let endpoint = required_r2_env(R2_E2E_ENDPOINT_ENV)?;
    let parsed =
        reqwest::Url::parse(&endpoint).context("BRIDGE_R2_TEST_ENDPOINT must be a valid URL")?;
    let account_id = parsed
        .host_str()
        .and_then(|host| {
            host.split_once('.')
                .map(|(account_id, _)| account_id.to_string())
        })
        .ok_or_else(|| {
            anyhow!("BRIDGE_R2_TEST_ENDPOINT host must start with the Cloudflare account id")
        })?;
    Ok(R2Endpoint {
        endpoint,
        account_id,
        bucket: required_r2_env(R2_E2E_BUCKET_ENV)?,
    })
}

fn cloudflare_token_id(account_id: &str, token: &str) -> Result<String> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/tokens/verify");
    let response = reqwest::blocking::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .context("failed to verify Cloudflare R2 token")?
        .error_for_status()
        .context("Cloudflare R2 token verification returned an error status")?
        .json::<serde_json::Value>()
        .context("Cloudflare R2 token verification returned invalid JSON")?;
    if !response["success"].as_bool().unwrap_or(false) {
        bail!("Cloudflare R2 token verification failed");
    }
    let status = response["result"]["status"].as_str().unwrap_or("");
    if status != "active" {
        bail!("Cloudflare R2 token is not active");
    }
    response["result"]["id"]
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("Cloudflare R2 token verification did not return a token id"))
}

fn r2_credentials(account_id: &str) -> Result<R2Credentials> {
    if let (Some(access_key_id), Some(secret_access_key)) = (
        optional_env(R2_E2E_ACCESS_KEY_ID_ENV),
        optional_env(R2_E2E_SECRET_ACCESS_KEY_ENV),
    ) {
        return Ok(R2Credentials {
            access_key_id,
            secret_access_key,
        });
    }

    if let Some(token) = optional_env(R2_E2E_TOKEN_ENV) {
        let access_key_id = cloudflare_token_id(account_id, &token)?;
        let secret_access_key = format!("{:x}", Sha256::digest(token.as_bytes()));
        return Ok(R2Credentials {
            access_key_id,
            secret_access_key,
        });
    }

    Ok(R2Credentials {
        access_key_id: required_r2_env(R2_E2E_ACCESS_KEY_ID_ENV)?,
        secret_access_key: required_r2_env(R2_E2E_SECRET_ACCESS_KEY_ENV)?,
    })
}

fn unix_now_for_test() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs())
}

fn sanitize_key_segment(raw: &str) -> String {
    raw.trim_matches('/')
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = manifest_dir
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve crates directory"))?;
    let source_root = crates_dir
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve source root"))?;
    if source_root.file_name().and_then(|name| name.to_str()) == Some("open")
        && source_root.parent().is_some_and(|parent| {
            parent.join("Cargo.toml").is_file() || parent.join("MODULE.bazel").is_file()
        })
    {
        Ok(source_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| source_root.to_path_buf()))
    } else {
        Ok(source_root.to_path_buf())
    }
}

fn crates_dir(workspace_root: &Path) -> PathBuf {
    let open_crates = workspace_root.join("open/crates");
    if open_crates.exists() {
        open_crates
    } else {
        workspace_root.join("crates")
    }
}

fn bridge_dev_bin() -> PathBuf {
    option_env!("CARGO_BIN_EXE_bridge-dev")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().unwrap().join("target/debug/bridge-dev"))
}

fn scenario_tempdir() -> Result<TempDir> {
    TempDirBuilder::new()
        .prefix("bd-")
        .tempdir_in("/tmp")
        .or_else(|_| TempDir::new())
        .context("failed to create short bridge-dev scenario tempdir")
}

fn scenario_port_offset(name: &str) -> Result<u16> {
    if let Ok(raw) = env::var(E2E_PORT_OFFSET_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed
                .parse::<u16>()
                .with_context(|| format!("{E2E_PORT_OFFSET_ENV} must be a u16 port offset"));
        }
    }
    let name_hash = name
        .bytes()
        .fold(0u16, |acc, byte| acc.wrapping_add(u16::from(byte)))
        % 10;
    Ok(10_000 + ((std::process::id() % 1_000) as u16 * 10) + name_hash)
}

fn ensure_release_binaries(workspace_root: &Path) -> Result<()> {
    for binary in ["bridge", "nockchain-bridge-sequencer", "nockchain-wallet"] {
        let path = workspace_root.join("target/release").join(binary);
        if !path.exists() {
            bail!(
                "{binary} binary not found at {}. Build with `cargo build --release -p bridge -p nockchain-bridge-sequencer -p nockchain-wallet` before running bridge-dev E2E scenarios",
                path.display()
            );
        }
    }
    Ok(())
}

fn checked_stdout(args: &[&str], output: Output) -> Result<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        return Ok(stdout.into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "bridge-dev {} exited with {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        output.status,
        redact(&stdout),
        redact(&stderr)
    )
}

fn assert_contains_all(haystack: &str, needles: &[&str]) -> Result<()> {
    for needle in needles {
        if !haystack.contains(needle) {
            bail!("output missing {needle:?}:\n{}", redact(haystack));
        }
    }
    Ok(())
}

fn process_state<'a>(status: &'a str, component_name: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        (columns.first().copied() == Some(component_name))
            .then(|| columns.get(1).copied())
            .flatten()
    })
}

fn assert_processes_running(status: &str, component_names: &[&str]) -> Result<()> {
    for component_name in component_names {
        match process_state(status, component_name) {
            Some("running") => {}
            Some(state) => bail!(
                "status output shows {component_name} as {state}, expected running:\n{}",
                redact(status)
            ),
            None => bail!(
                "status output does not include {component_name}:\n{}",
                redact(status)
            ),
        }
    }
    Ok(())
}

fn assert_processes_not_running(status: &str, component_names: &[&str]) -> Result<()> {
    for component_name in component_names {
        match process_state(status, component_name) {
            Some("running") => bail!(
                "status output still shows {component_name} running:\n{}",
                redact(status)
            ),
            Some(_) => {}
            None => bail!(
                "status output does not include {component_name}:\n{}",
                redact(status)
            ),
        }
    }
    Ok(())
}

fn bridge_stream_line(status: &str, node_id: usize) -> Option<&str> {
    let prefix = format!("bridge-{node_id} ");
    status
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(&prefix) && line.contains("running_state="))
}

fn assert_bridge_streams_available(status: &str, node_ids: &[usize]) -> Result<()> {
    for node_id in node_ids {
        let Some(line) = bridge_stream_line(status, *node_id) else {
            bail!(
                "status output does not include bridge-{node_id} stream:\n{}",
                redact(status)
            );
        };
        if !line.contains("running_state=Running") {
            bail!(
                "bridge-{node_id} stream is not running:\n{}",
                redact(status)
            );
        }
        if !line.contains("nockchain_api=Connected") {
            bail!(
                "bridge-{node_id} nockchain API is not connected:\n{}",
                redact(status)
            );
        }
    }
    Ok(())
}

fn assert_cluster_available(status: &str) -> Result<()> {
    assert_contains_all(
        status,
        &["processes:", "bridge_streams:", "sequencer_status:"],
    )?;
    assert_processes_running(status, ALL_COMPONENTS)?;
    assert_bridge_streams_available(status, ALL_BRIDGE_NODES)
}

fn assert_sequencer_idle(status: &str) -> Result<()> {
    assert_contains_all(status, &["reserved_inputs=0", "next_pending=none"])
}

fn assert_queue_drained(status: &str) -> Result<()> {
    assert_contains_all(
        status,
        &[
            "pending_deposits=0", "pending_withdrawals=0", "unsettled_deposits=0",
            "unsettled_withdrawals=0",
        ],
    )
}

fn assert_successful_deposit_on_all_nodes(
    scenario: &mut BridgeDevScenario,
    expected: &ObservedDeposit,
    timeout_secs: u64,
) -> Result<()> {
    assert_successful_deposit_on_all_nodes_after(scenario, expected, timeout_secs, None)
}

fn assert_successful_deposit_on_all_nodes_after(
    scenario: &mut BridgeDevScenario,
    expected: &ObservedDeposit,
    timeout_secs: u64,
    after_nonce: Option<u64>,
) -> Result<()> {
    for node_id in ALL_BRIDGE_NODES {
        let observed = scenario.wait_for_deposit_on_node_after(
            ObservedDepositPhase::Successful,
            *node_id,
            timeout_secs,
            after_nonce,
        )?;
        assert_same_deposit(expected, &observed, &format!("bridge-{node_id}"))?;
    }
    Ok(())
}

fn assert_bridge_reboot_state_present(
    scenario: &BridgeDevScenario,
    node_ids: &[usize],
) -> Result<()> {
    for node_id in node_ids {
        let data_dir = scenario.bridge_data_dir(*node_id);
        if !bridge_data_dir_has_reboot_state(&data_dir) {
            bail!(
                "bridge-{node_id} did not write rebootable state under {}",
                data_dir.display()
            );
        }
    }
    Ok(())
}

fn bridge_data_dir_has_reboot_state(data_dir: &Path) -> bool {
    checkpoint_dir_has_nonempty_checkpoint(&data_dir.join("checkpoints"))
        || pma_dir_has_nonempty_snapshot(&data_dir.join("pma"))
        || nonempty_file(&data_dir.join("event-log.sqlite3"))
}

fn checkpoint_dir_has_nonempty_checkpoint(checkpoint_dir: &Path) -> bool {
    ["0.chkjam", "1.chkjam"]
        .into_iter()
        .map(|name| checkpoint_dir.join(name))
        .any(|path| nonempty_file(&path))
}

fn pma_dir_has_nonempty_snapshot(pma_dir: &Path) -> bool {
    ["epoch.pma", "0.pma", "1.pma"]
        .into_iter()
        .map(|name| pma_dir.join(name))
        .any(|path| nonempty_file(&path))
}

fn nonempty_file(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn assert_positive_deposit(deposit: &ObservedDeposit, label: &str) -> Result<()> {
    if deposit.amount == 0 {
        bail!("{label} deposit reported zero amount: {:?}", deposit);
    }
    if deposit.recipient == "<unknown>" || deposit.tx_id == "<unknown>" {
        bail!("{label} deposit is missing recipient or tx id: {:?}", deposit);
    }
    Ok(())
}

fn assert_same_deposit(
    expected: &ObservedDeposit,
    observed: &ObservedDeposit,
    observed_label: &str,
) -> Result<()> {
    assert_positive_deposit(observed, observed_label)?;
    if observed != expected {
        bail!(
            "{observed_label} observed a different successful deposit: expected {:?}, got {:?}",
            expected, observed
        );
    }
    Ok(())
}

fn assert_same_deposit_identity(
    first: &ObservedDeposit,
    second: &ObservedDeposit,
    first_label: &str,
    second_label: &str,
) -> Result<()> {
    assert_positive_deposit(first, first_label)?;
    assert_positive_deposit(second, second_label)?;
    if first.nonce != second.nonce
        || first.amount != second.amount
        || first.recipient != second.recipient
    {
        bail!("{second_label} did not match {first_label}: first={:?}, second={:?}", first, second);
    }
    Ok(())
}

fn assert_deposit_nonce_increased(first: &ObservedDeposit, second: &ObservedDeposit) -> Result<()> {
    if second.nonce <= first.nonce {
        bail!("second deposit nonce did not advance: first={:?}, second={:?}", first, second);
    }
    Ok(())
}

fn parse_observed_deposit(output: &str, phase: ObservedDepositPhase) -> Result<ObservedDeposit> {
    let prefix = format!("deposit {}:", phase.output_label());
    let line = output
        .lines()
        .find(|line| line.starts_with(&prefix))
        .ok_or_else(|| anyhow!("wait deposit output did not include a {prefix} line"))?;
    let mut nonce = None;
    let mut amount = None;
    let mut recipient = None;
    let mut tx_id = None;
    for token in line.split_whitespace() {
        if let Some(value) = token.strip_prefix("nonce=") {
            nonce = Some(u64::from_str(value).context("invalid deposit nonce")?);
        } else if let Some(value) = token.strip_prefix("amount=") {
            amount = Some(u64::from_str(value).context("invalid deposit amount")?);
        } else if let Some(value) = token.strip_prefix("recipient=") {
            recipient = Some(value.to_string());
        } else if let Some(value) = token.strip_prefix("tx_id=") {
            tx_id = Some(value.to_string());
        }
    }
    Ok(ObservedDeposit {
        nonce: nonce.ok_or_else(|| anyhow!("successful deposit output missing nonce"))?,
        amount: amount.ok_or_else(|| anyhow!("successful deposit output missing amount"))?,
        recipient: recipient
            .ok_or_else(|| anyhow!("successful deposit output missing recipient"))?,
        tx_id: tx_id.ok_or_else(|| anyhow!("successful deposit output missing tx_id"))?,
    })
}

fn parse_observed_withdrawal(output: &str, expected_phase: &str) -> Result<ObservedWithdrawal> {
    let prefix = format!("withdrawal {expected_phase}:");
    let line = output
        .lines()
        .find(|line| line.starts_with(&prefix))
        .ok_or_else(|| anyhow!("wait withdrawal output did not include a {prefix} line"))?;
    let field = |key: &str| -> Result<String> {
        let prefix = format!("{key}=");
        line.split_whitespace()
            .find_map(|token| token.strip_prefix(&prefix).map(ToString::to_string))
            .ok_or_else(|| anyhow!("withdrawal {expected_phase} output missing {key}"))
    };
    let id = field("id")?;
    let as_of = field("as_of")?;
    let base_event = field("base_event")?;
    let compact_id = format!("{as_of}:{base_event}");
    if id != compact_id {
        bail!(
            "withdrawal {expected_phase} output id did not match component fields: id={id} as_of={as_of} base_event={base_event}"
        );
    }
    Ok(ObservedWithdrawal {
        phase: expected_phase.to_string(),
        id,
        as_of,
        base_event,
        nonce: field("nonce")?,
        proposal_status: field("proposal_status")?,
        sequenced_state: field("sequenced_state")?,
        handoff_owner: field("handoff_owner")?,
        transaction_name: field("transaction_name")?,
        proposal_hash: field("proposal_hash")?,
        authorized_transaction_name: field("authorized_transaction_name")?,
    })
}

fn assert_not_placeholder(
    field_name: &str,
    value: &str,
    withdrawal: &ObservedWithdrawal,
) -> Result<()> {
    if value == "-" || value.trim().is_empty() {
        bail!("withdrawal {} has placeholder {field_name}: {:?}", withdrawal.phase, withdrawal);
    }
    Ok(())
}

fn assert_same_withdrawal(
    expected: &ObservedWithdrawal,
    observed: &ObservedWithdrawal,
) -> Result<()> {
    if observed.as_of != expected.as_of
        || observed.base_event != expected.base_event
        || observed.nonce != expected.nonce
    {
        bail!(
            "withdrawal phase {} changed target: expected as_of={} base_event={} nonce={}, got as_of={} base_event={} nonce={}",
            observed.phase,
            expected.as_of,
            expected.base_event,
            expected.nonce,
            observed.as_of,
            observed.base_event,
            observed.nonce
        );
    }
    Ok(())
}

fn assert_withdrawal_progression(
    pending: &ObservedWithdrawal,
    ready: &ObservedWithdrawal,
    submitted: &ObservedWithdrawal,
    executed: &ObservedWithdrawal,
) -> Result<()> {
    assert_not_placeholder("id", &pending.id, pending)?;
    assert_not_placeholder("as_of", &pending.as_of, pending)?;
    assert_not_placeholder("base_event", &pending.base_event, pending)?;
    assert_not_placeholder("nonce", &pending.nonce, pending)?;
    for observed in [ready, submitted, executed] {
        assert_same_withdrawal(pending, observed)?;
        assert_not_placeholder("handoff_owner", &observed.handoff_owner, observed)?;
    }
    assert_not_placeholder("proposal_hash", &submitted.proposal_hash, submitted)?;
    if submitted.proposal_hash != executed.proposal_hash {
        bail!(
            "executed withdrawal proposal hash changed from submitted phase: submitted={} executed={}",
            submitted.proposal_hash,
            executed.proposal_hash
        );
    }
    assert_not_placeholder(
        "authorized_transaction_name", &submitted.authorized_transaction_name, submitted,
    )?;
    if submitted.authorized_transaction_name != executed.authorized_transaction_name {
        bail!(
            "executed withdrawal authorized transaction changed from submitted phase: submitted={} executed={}",
            submitted.authorized_transaction_name,
            executed.authorized_transaction_name
        );
    }
    if executed.sequenced_state != "confirmed" {
        bail!(
            "executed withdrawal ended with unexpected sequenced_state={}: {:?}",
            executed.sequenced_state, executed
        );
    }
    Ok(())
}

#[test]
fn process_state_helpers_read_status_process_rows() {
    let status =
        "processes:\n  bridge-3   exited(0)    pid=123\n  bridge-4   running      pid=456\n";
    assert_eq!(process_state(status, "bridge-3"), Some("exited(0)"));
    assert_eq!(process_state(status, "bridge-4"), Some("running"));
    assert_processes_not_running(status, &["bridge-3"]).unwrap();
    assert_processes_running(status, &["bridge-4"]).unwrap();
    assert!(assert_processes_running(status, &["bridge-3"]).is_err());
    assert!(assert_processes_not_running(status, &["bridge-4"]).is_err());
}

#[test]
fn bridge_stream_helpers_ignore_process_rows() {
    let status = "\
processes:
  bridge-0   running      pid=123
bridge_streams:
  bridge-0 running_state=Running base_height=1 nock_height=2 nockchain_api=Connected batch_status=idle unhealthy_peers=0
";
    assert_eq!(
        bridge_stream_line(status, 0).unwrap(),
        "bridge-0 running_state=Running base_height=1 nock_height=2 nockchain_api=Connected batch_status=idle unhealthy_peers=0"
    );
    assert_bridge_streams_available(status, &[0]).unwrap();
}

#[test]
fn reboot_state_helper_accepts_checkpoint_pma_or_event_log() {
    let tempdir = TempDir::new().unwrap();
    let data_dir = tempdir.path();
    let checkpoint_dir = data_dir.join("checkpoints");
    let pma_dir = data_dir.join("pma");
    fs::create_dir_all(&checkpoint_dir).unwrap();
    fs::create_dir_all(&pma_dir).unwrap();

    assert!(!bridge_data_dir_has_reboot_state(data_dir));
    assert!(!checkpoint_dir_has_nonempty_checkpoint(&checkpoint_dir));
    fs::write(checkpoint_dir.join("0.chkjam"), []).unwrap();
    assert!(!bridge_data_dir_has_reboot_state(data_dir));
    fs::write(checkpoint_dir.join("1.chkjam"), [1u8]).unwrap();
    assert!(checkpoint_dir_has_nonempty_checkpoint(&checkpoint_dir));
    assert!(bridge_data_dir_has_reboot_state(data_dir));

    fs::remove_file(checkpoint_dir.join("1.chkjam")).unwrap();
    fs::write(pma_dir.join("epoch.pma"), [1u8]).unwrap();
    assert!(bridge_data_dir_has_reboot_state(data_dir));

    fs::remove_file(pma_dir.join("epoch.pma")).unwrap();
    fs::write(data_dir.join("event-log.sqlite3"), [1u8]).unwrap();
    assert!(bridge_data_dir_has_reboot_state(data_dir));
}

#[test]
fn parses_successful_deposit_wait_output() {
    let deposit = parse_observed_deposit(
        "deposit successful: nonce=7 amount=42 recipient=0xabc tx_id=deposit-tx\n",
        ObservedDepositPhase::Successful,
    )
    .unwrap();
    assert_eq!(
        deposit,
        ObservedDeposit {
            nonce: 7,
            amount: 42,
            recipient: "0xabc".to_string(),
            tx_id: "deposit-tx".to_string(),
        }
    );
}

#[test]
fn parses_withdrawal_wait_output() {
    let withdrawal = parse_observed_withdrawal(
        "withdrawal Executed: id=aa:bb as_of=aa base_event=bb nonce=9 proposal_status=confirmed sequenced_state=confirmed handoff_owner=bridge-2 transaction_name=tx proposal_hash=hash authorized_transaction_name=authed\n",
        "Executed",
    )
    .unwrap();
    assert_eq!(withdrawal.id, "aa:bb");
    assert_eq!(withdrawal.as_of, "aa");
    assert_eq!(withdrawal.base_event, "bb");
    assert_eq!(withdrawal.nonce, "9");
    assert_eq!(withdrawal.sequenced_state, "confirmed");
    assert_eq!(withdrawal.authorized_transaction_name, "authed");
}

fn redacted_tail(path: &Path) -> String {
    let Ok(contents) = fs::read_to_string(path) else {
        return "<unavailable>".to_string();
    };
    let lines = contents.lines().rev().take(80).collect::<Vec<_>>();
    let tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    redact(&tail)
}

fn redact(value: &str) -> String {
    SECRET_ENV.iter().fold(value.to_string(), |redacted, key| {
        let Ok(secret) = env::var(key) else {
            return redacted;
        };
        let secret = secret.trim();
        if secret.is_empty() {
            redacted
        } else {
            redacted.replace(secret, "<redacted>")
        }
    })
}
