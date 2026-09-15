use std::cell::Cell;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};
use std::{env, thread};

use anyhow::{anyhow, bail, Context, Result};
use bridge::shared::types::WithdrawalPolicy;
use tempfile::{Builder as TempDirBuilder, TempDir};

use crate::artifacts::{ArtifactResolveOptions, ArtifactResolver};

pub const E2E_ENABLE_ENV: &str = "BRIDGE_DEV_RUN_E2E";
pub const E2E_PORT_OFFSET_ENV: &str = "BRIDGE_DEV_E2E_PORT_OFFSET";
pub const BRIDGE_DEV_BIN_ENV: &str = "CARGO_BIN_EXE_bridge-dev";
pub const KEEP_RUN_ROOT_ENV: &str = "BRIDGE_DEV_KEEP_RUN_ROOT";
pub const TEST_RUN_ROOT_ENV: &str = "BRIDGE_DEV_TEST_RUN_ROOT";
pub const PORT_OFFSET_ENV: &str = "BRIDGE_DEV_PORT_OFFSET";
pub const FAKENET_GENESIS_JAM_ENV: &str = "BRIDGE_DEV_FAKENET_GENESIS_JAM";
pub const FAKENET_POW_LEN_ENV: &str = "BRIDGE_DEV_FAKENET_POW_LEN";
pub const FAKENET_LOG_DIFFICULTY_ENV: &str = "BRIDGE_DEV_FAKENET_LOG_DIFFICULTY";
pub const FAKENET_BYTHOS_PHASE_ENV: &str = "BRIDGE_DEV_FAKENET_BYTHOS_PHASE";
pub const BASE_BLOCKS_CHUNK_ENV: &str = "BRIDGE_DEV_BASE_BLOCKS_CHUNK";
pub const BRIDGE_SAVE_INTERVAL_MILLIS_ENV: &str = "BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS";
pub const NOCK_OBSERVER_POLL_MILLIS_ENV: &str = "BRIDGE_NOCK_OBSERVER_POLL_MILLIS";
pub const RUST_LOG_ENV: &str = "RUST_LOG";
pub const MANUAL_SUBMIT_APPROVAL_ENV: &str = "BRIDGE_DEV_MANUAL_SUBMIT_APPROVAL";
pub const BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV: &str = "BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED";
pub const R2_E2E_ENABLE_ENV: &str = "BRIDGE_R2_RUN_E2E";
pub const R2_E2E_URL_ENV: &str = "BRIDGE_R2_TEST_URL";
pub const R2_E2E_ENDPOINT_ENV: &str = "BRIDGE_R2_TEST_ENDPOINT";
pub const R2_E2E_BUCKET_ENV: &str = "BRIDGE_R2_TEST_BUCKET";
pub const R2_E2E_REGION_ENV: &str = "BRIDGE_R2_TEST_REGION";
pub const R2_E2E_PREFIX_ENV: &str = "BRIDGE_R2_TEST_PREFIX";
pub const R2_E2E_ACCESS_KEY_ID_ENV: &str = "BRIDGE_R2_TEST_ACCESS_KEY_ID";
pub const R2_E2E_SECRET_ACCESS_KEY_ENV: &str = "BRIDGE_R2_TEST_SECRET_ACCESS_KEY";
pub const R2_E2E_TOKEN_ENV: &str = "BRIDGE_R2_TEST_TOKEN";
pub const R2_E2E_KEEP_OBJECTS_ENV: &str = "BRIDGE_R2_KEEP_OBJECTS";
pub const E2E_FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES: &str =
    "nockchain/jams/fakenet-genesis-pow-2-bex-1.jam";
pub const E2E_DEPOSIT_AMOUNT_NICKS: &str = "6553600001";
pub const E2E_DEPOSIT_SPEND_TIMEOUT_SECS: u64 = 1_800;
pub const E2E_MIXED_INPUT_WITHDRAWAL_AMOUNT_NOCK: &str = "120000";
pub const E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS: &str = "10";
pub const E2E_WITHDRAWAL_PHASE_POLL_SECS: u64 = 30;
pub const E2E_PRE_BYTHOS_WITHDRAWAL_BYTHOS_PHASE: u64 = 80;
pub const E2E_MANUAL_APPROVAL_DEFER_TIMEOUT_SECS: u64 = 90;
pub const WAIT_WITHDRAWAL_TIMEOUT_FRAGMENT: &str = "timed out waiting for withdrawal";
pub const STOP_CONDITION_LOG_MARKERS: &[&str] = &[
    "Bridge Stopped", "local stop requested", "local stop activated", "kernel-stop", "peer-stop",
    "running_state=Stopped",
];
pub const REQUIRED_E2E_ENV: &[&str] = &[
    "TENDERLY_ACCESS_KEY", "TENDERLY_ACCOUNT_ID", "TENDERLY_PROJECT_SLUG",
    "TENDERLY_TEST_PRIVATE_KEY",
];
const SECRET_ENV: &[&str] = &[
    "TENDERLY_ACCESS_KEY", "TENDERLY_PRIVATE_KEY", "TENDERLY_TEST_PRIVATE_KEY",
    "BRIDGE_DEV_OWNER_PRIVATE_KEY", R2_E2E_ACCESS_KEY_ID_ENV, R2_E2E_SECRET_ACCESS_KEY_ENV,
    R2_E2E_TOKEN_ENV, "WITHDRAWAL_SEQUENCER_JOURNAL_OBJECT_STORE_SECRET_ACCESS_KEY",
];
pub const ALL_COMPONENTS: &[&str] =
    &["node", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"];
pub const ALL_BRIDGE_COMPONENTS: &[&str] =
    &["bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"];
pub const ALL_BRIDGE_NODES: &[usize] = &[0, 1, 2, 3, 4];

pub fn core_withdrawal_amount_nocks(policy: &WithdrawalPolicy, headroom_nocks: u64) -> Result<u64> {
    let amount_nocks = policy
        .minimum_gross_nocks
        .checked_add(headroom_nocks)
        .context("core withdrawal amount overflowed NOCK units")?;
    let amount_nicks = amount_nocks
        .checked_mul(policy.nicks_per_nock)
        .context("core withdrawal amount overflowed nick units")?;
    if amount_nicks > policy.maximum_nicks
        || amount_nicks % policy.nicks_per_nock != 0
        || amount_nocks < policy.minimum_gross_nocks
    {
        bail!("core withdrawal amount does not satisfy active policy");
    }
    Ok(amount_nocks)
}

pub fn core_withdrawal_amount_nicks(policy: &WithdrawalPolicy, headroom_nocks: u64) -> Result<u64> {
    core_withdrawal_amount_nocks(policy, headroom_nocks)?
        .checked_mul(policy.nicks_per_nock)
        .context("core withdrawal amount overflowed nick units")
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioPaths {
    pub workspace_root: PathBuf,
    pub bridge_dev_bin: PathBuf,
    pub run_root: PathBuf,
    pub up_stdout: PathBuf,
    pub up_stderr: PathBuf,
    pub port_offset: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBaseEnvironment {
    pub http_url: String,
    pub ws_url: String,
    pub chain_id: u64,
    pub start_height: u64,
    pub inbox_contract: String,
    pub nock_contract: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTarget {
    Node,
    Bridge(usize),
}

impl ProcessTarget {
    pub fn name(self) -> Result<String> {
        match self {
            Self::Node => Ok("node".to_owned()),
            Self::Bridge(index) if index < 5 => Ok(format!("bridge-{index}")),
            Self::Bridge(index) => bail!("invalid bridge process target {index}; expected 0..=4"),
        }
    }
}

pub struct ScenarioHarness {
    tempdir: Option<TempDir>,
    workspace_root: PathBuf,
    bridge_dev_bin: PathBuf,
    run_root: PathBuf,
    port_offset: u16,
    up_child: Option<Child>,
    up_stdout: PathBuf,
    up_stderr: PathBuf,
    preserve_run_root: Cell<bool>,
    resolve_artifacts: bool,
    build_artifacts: bool,
    env_overrides: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedDeposit {
    pub nonce: u64,
    pub amount: u64,
    pub recipient: String,
    pub tx_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedDepositPhase {
    Submitted,
    Successful,
}

impl ObservedDepositPhase {
    pub fn cli_flag(self) -> &'static str {
        match self {
            Self::Submitted => "--submitted",
            Self::Successful => "--successful",
        }
    }

    pub fn output_label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Successful => "successful",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedWithdrawal {
    pub phase: String,
    pub id: String,
    pub as_of: String,
    pub base_event: String,
    pub nonce: String,
    pub proposal_status: String,
    pub sequenced_state: String,
    pub handoff_owner: String,
    pub transaction_name: String,
    pub proposal_hash: String,
    pub authorized_transaction_name: String,
}
impl ScenarioHarness {
    pub fn new(name: &str) -> Result<Self> {
        let mut harness = Self::with_binary(name, workspace_root()?, bridge_dev_bin())?;
        harness.resolve_artifacts = true;
        Ok(harness)
    }

    pub fn with_binary(
        name: &str,
        workspace_root: PathBuf,
        bridge_dev_bin: PathBuf,
    ) -> Result<Self> {
        let tempdir = scenario_tempdir().context("failed to create bridge-dev scenario tempdir")?;
        let run_root = tempdir.path().join("test_run_data");
        fs::create_dir_all(&run_root)
            .with_context(|| format!("failed to create {}", run_root.display()))?;
        let log_dir = tempdir.path().join("logs");
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create {}", log_dir.display()))?;
        Ok(Self {
            workspace_root,
            bridge_dev_bin,
            run_root,
            port_offset: scenario_port_offset(name)?,
            up_child: None,
            up_stdout: log_dir.join("up.stdout.log"),
            up_stderr: log_dir.join("up.stderr.log"),
            resolve_artifacts: false,
            build_artifacts: false,
            env_overrides: Vec::new(),
            preserve_run_root: Cell::new(false),
            tempdir: Some(tempdir),
        })
    }

    pub fn for_e2e_run(
        name: &str,
        workspace_root: PathBuf,
        bridge_dev_bin: PathBuf,
        run_dir: &Path,
    ) -> Result<Self> {
        let run_root = run_dir.join("cluster");
        fs::create_dir_all(&run_root)
            .with_context(|| format!("failed to create {}", run_root.display()))?;
        let log_dir = run_dir.join("cluster-logs");
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create {}", log_dir.display()))?;
        Ok(Self {
            workspace_root,
            bridge_dev_bin,
            run_root,
            port_offset: scenario_port_offset(name)?,
            up_child: None,
            up_stdout: log_dir.join("up.stdout.log"),
            up_stderr: log_dir.join("up.stderr.log"),
            resolve_artifacts: false,
            build_artifacts: false,
            env_overrides: Vec::new(),
            preserve_run_root: Cell::new(true),
            tempdir: None,
        })
    }

    pub fn with_artifact_build(&mut self, build: bool) {
        self.build_artifacts = build;
    }

    pub fn paths(&self) -> ScenarioPaths {
        ScenarioPaths {
            workspace_root: self.workspace_root.clone(),
            bridge_dev_bin: self.bridge_dev_bin.clone(),
            run_root: self.run_root.clone(),
            up_stdout: self.up_stdout.clone(),
            up_stderr: self.up_stderr.clone(),
            port_offset: self.port_offset,
        }
    }

    pub fn preserve_failure_artifacts(&self) {
        self.preserve_run_root.set(true);
    }

    pub fn command(&self, args: &[&str]) -> Command {
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

    pub fn extend_env_overrides(&mut self, envs: impl IntoIterator<Item = (String, String)>) {
        self.env_overrides.extend(envs);
    }

    pub fn with_fakenet_bythos_phase(&mut self, phase: u64) {
        self.extend_env_overrides([(FAKENET_BYTHOS_PHASE_ENV.to_string(), phase.to_string())]);
    }

    pub fn run_checked(&self, args: &[&str]) -> Result<String> {
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

    pub fn start_targets(&self, targets: &[ProcessTarget]) -> Result<String> {
        self.run_target_command("start", targets)
    }

    pub fn stop_targets(&self, targets: &[ProcessTarget]) -> Result<String> {
        self.run_target_command("stop", targets)
    }

    pub fn restart_targets(
        &mut self,
        targets: &[ProcessTarget],
        timeout: Duration,
    ) -> Result<String> {
        let names = process_target_names(targets)?;
        self.stop_targets(targets)?;
        let stopped = self.wait_for_process_status(timeout)?;
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        assert_processes_not_running(&stopped, &name_refs)?;
        self.start_targets(targets)?;
        let running = self.wait_for_status(timeout)?;
        assert_processes_running(&running, &name_refs)?;
        Ok(running)
    }

    fn run_target_command(&self, command: &str, targets: &[ProcessTarget]) -> Result<String> {
        let names = process_target_names(targets)?;
        let mut args = Vec::with_capacity(names.len() + 1);
        args.push(command);
        args.extend(names.iter().map(String::as_str));
        self.run_checked(&args)
    }

    pub fn run_checked_retry(&mut self, args: &[&str], timeout: Duration) -> Result<String> {
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

    pub fn spawn_fresh_cluster(&mut self) -> Result<()> {
        self.spawn_cluster(&["up", "--fresh", "--start"], Duration::from_secs(420))
    }

    pub fn write_local_base_environment(
        &self,
        environment: &LocalBaseEnvironment,
    ) -> Result<PathBuf> {
        if environment.chain_id != 31_338
            || !environment.http_url.starts_with("http://127.0.0.1:")
            || !environment.ws_url.starts_with("ws://127.0.0.1:")
            || environment.start_height == 0
        {
            bail!("local Base environment must bind dedicated chain 31338 to one loopback Anvil");
        }
        let path = self.run_root.join("virtual-testnet.generated.env");
        let contents = format!(
            "export BASE_RPC_URL=\"{}\"\nexport BASE_WS_URL=\"{}\"\nexport BASE_CHAIN_ID=\"{}\"\nexport BASE_START_HEIGHT=\"{}\"\nexport INBOX_CONTRACT_ADDRESS=\"{}\"\nexport NOCK_CONTRACT_ADDRESS=\"{}\"\n",
            environment.http_url,
            environment.ws_url,
            environment.chain_id,
            environment.start_height,
            environment.inbox_contract,
            environment.nock_contract,
        );
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        use std::io::Write as _;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    pub fn spawn_local_cluster(&mut self) -> Result<()> {
        self.spawn_cluster(
            &["up", "--fresh-state", "--start"],
            Duration::from_secs(420),
        )
    }
    pub fn spawn_cluster(&mut self, args: &[&str], status_timeout: Duration) -> Result<()> {
        if self.resolve_artifacts {
            let mut options = ArtifactResolveOptions::new(self.workspace_root.clone());
            options.build = self.build_artifacts;
            let artifacts = ArtifactResolver::resolve(&options).map_err(anyhow::Error::new)?;
            self.extend_env_overrides(artifacts.environment_overrides());
        }
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
        let result = self
            .wait_for_status(status_timeout)
            .map(|_| ())
            .with_context(|| format!("{}\n{}", self.up_log_context(), self.cluster_context()));
        if result.is_err() {
            self.preserve_failure_artifacts();
        }
        result
    }

    pub fn wait_for_status(&mut self, timeout: Duration) -> Result<String> {
        let stdout =
            self.wait_for_status_command(&["status", "--bridges", "--sequencer"], timeout)?;
        assert_contains_all(&stdout, &["bridge_streams:", "sequencer_status:"])?;
        Ok(stdout)
    }

    pub fn wait_for_process_status(&mut self, timeout: Duration) -> Result<String> {
        self.wait_for_status_command_allowing_endpoint_failures(&["status"], timeout)
    }

    pub fn wait_for_status_command(&mut self, args: &[&str], timeout: Duration) -> Result<String> {
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

    pub fn wait_for_status_command_allowing_endpoint_failures(
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

    pub fn run_status_command_allowing_endpoint_failures(&self, args: &[&str]) -> Result<String> {
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

    pub fn wait_for_deposit_on_node(
        &mut self,
        phase: ObservedDepositPhase,
        node_id: usize,
        timeout_secs: u64,
    ) -> Result<ObservedDeposit> {
        self.wait_for_deposit_on_node_after(phase, node_id, timeout_secs, None)
    }

    pub fn wait_for_deposit_on_node_after(
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

    pub fn wait_for_withdrawal_phase(
        &mut self,
        phase: &str,
        flag: &str,
        timeout_secs: u64,
    ) -> Result<ObservedWithdrawal> {
        self.wait_for_withdrawal_phase_for(phase, flag, timeout_secs, None)
    }

    pub fn wait_for_withdrawal_phase_for(
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

    pub fn wait_for_withdrawal_phase_once(
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

    pub fn wait_for_withdrawal_manual_approval_facts(
        &mut self,
        target: &ObservedWithdrawal,
        timeout_secs: u64,
    ) -> Result<ObservedWithdrawal> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut last_error = None;
        let mut last_observed = None;

        loop {
            self.ensure_up_still_running()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            let chunk_secs = remaining.as_secs().clamp(1, E2E_WITHDRAWAL_PHASE_POLL_SECS);
            match self.wait_for_withdrawal_phase_once("Ready", "--ready", chunk_secs, Some(target))
            {
                Ok(withdrawal) => {
                    assert_same_withdrawal(target, &withdrawal)?;
                    if !is_placeholder(&withdrawal.proposal_hash)
                        && !is_placeholder(&withdrawal.authorized_transaction_name)
                    {
                        return Ok(withdrawal);
                    }
                    last_observed = Some(withdrawal);
                }
                Err(err) => {
                    if !err.to_string().contains(WAIT_WITHDRAWAL_TIMEOUT_FRAGMENT) {
                        return Err(err)
                            .context("failed while waiting for withdrawal approval facts")
                            .with_context(|| self.cluster_context());
                    }
                    last_error = Some(err);
                }
            }

            if Instant::now() >= deadline {
                break;
            }

            self.run_checked(&["advance-base", "--blocks", E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS])
                .context("failed to advance Base while waiting for withdrawal approval facts")?;
        }

        let last_observed = last_observed
            .map(|withdrawal| format!("; last observed withdrawal={withdrawal:?}"))
            .unwrap_or_default();
        match last_error {
            Some(err) => Err(err)
                .with_context(|| {
                    format!(
                        "timed out waiting {timeout_secs}s for withdrawal approval facts{last_observed}"
                    )
                })
                .with_context(|| self.cluster_context()),
            None => bail!(
                "timed out waiting {timeout_secs}s for withdrawal approval facts{last_observed}"
            ),
        }
    }

    pub fn assert_withdrawal_not_submitted_before_manual_approval(
        &mut self,
        target: &ObservedWithdrawal,
    ) -> Result<()> {
        match self.wait_for_withdrawal_phase_for(
            "Submitted",
            "--submitted",
            E2E_MANUAL_APPROVAL_DEFER_TIMEOUT_SECS,
            Some(target),
        ) {
            Ok(submitted) => {
                bail!("withdrawal submitted before manual approval was registered: {:?}", submitted)
            }
            Err(err) => {
                let rendered = format!("{err:#}");
                if rendered.contains(WAIT_WITHDRAWAL_TIMEOUT_FRAGMENT)
                    || rendered.contains("timed out waiting")
                {
                    Ok(())
                } else {
                    Err(err)
                        .context("submitted wait failed unexpectedly before manual approval")
                        .with_context(|| self.cluster_context())
                }
            }
        }
    }

    pub fn complete_deposit_on_all_nodes(&mut self) -> Result<ObservedDeposit> {
        self.complete_deposit_on_all_nodes_after(None)
    }

    pub fn complete_deposit_on_all_nodes_after(
        &mut self,
        after_nonce: Option<u64>,
    ) -> Result<ObservedDeposit> {
        self.complete_deposit_on_all_nodes_with_amount_after(E2E_DEPOSIT_AMOUNT_NICKS, after_nonce)
    }

    pub fn complete_deposit_on_all_nodes_with_amount_after(
        &mut self,
        amount_nicks: &str,
        after_nonce: Option<u64>,
    ) -> Result<ObservedDeposit> {
        self.run_checked_retry(
            &["deposit", "--amount-nicks", amount_nicks],
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

    pub fn request_withdrawal_after_mint(&self) -> Result<()> {
        let amount_nock = core_withdrawal_amount_nocks(&WithdrawalPolicy::v1(), 1)?.to_string();
        self.request_withdrawal_after_mint_amount(&amount_nock)
    }

    pub fn request_withdrawal_after_mint_amount(&self, amount_nock: &str) -> Result<()> {
        self.run_checked(&["mint-for-burn", "--amount-nock", amount_nock])?;
        self.run_checked(&["request-withdrawal", "--amount-nock", amount_nock])?;
        self.run_checked(&["advance-base", "--blocks", E2E_WITHDRAWAL_BASE_ADVANCE_BLOCKS])?;
        Ok(())
    }

    pub fn wait_for_withdrawal_sequencer_confirmation(
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

    pub fn current_nock_height(&self) -> Result<u64> {
        let status = self.run_checked(&["status", "--bridges", "--sequencer"])?;
        parse_status_nock_height(&status)
    }

    pub fn wait_for_nock_height_at_least(&mut self, target: u64, timeout: Duration) -> Result<u64> {
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        while Instant::now() < deadline {
            self.ensure_up_still_running()?;
            match self.current_nock_height() {
                Ok(height) if height >= target => return Ok(height),
                Ok(height) => {
                    last_error = Some(anyhow!("nock height {height} is still below {target}"));
                }
                Err(err) => last_error = Some(err),
            }
            thread::sleep(Duration::from_secs(2));
        }
        match last_error {
            Some(err) => Err(err)
                .with_context(|| format!("timed out waiting for nock height >= {target}"))
                .with_context(|| self.cluster_context()),
            None => bail!("timed out waiting for nock height >= {target}"),
        }
    }

    pub fn restart_all_bridges(&mut self) -> Result<()> {
        self.run_checked(&["stop", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
        let stopped = self.wait_for_process_status(Duration::from_secs(120))?;
        assert_processes_not_running(&stopped, ALL_BRIDGE_COMPONENTS)?;
        assert_bridge_reboot_state_present(self, ALL_BRIDGE_NODES)?;
        self.run_checked(&["start", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"])?;
        let status = self.wait_for_status(Duration::from_secs(240))?;
        assert_cluster_available(&status)
    }

    pub fn bridge_data_dir(&self, node_id: usize) -> PathBuf {
        self.run_root.join(format!("bridge-{node_id}"))
    }

    pub fn sequencer_config_path(&self) -> PathBuf {
        self.run_root
            .join("bridge-configs")
            .join("sequencer-conf.toml")
    }

    pub fn sequencer_data_dir(&self) -> PathBuf {
        self.run_root.join("node")
    }

    pub fn sequencer_ctl_binary(&self) -> PathBuf {
        self.workspace_root
            .join("target/release/nockchain-bridge-sequencer-ctl")
    }

    pub fn ensure_sequencer_ctl_binary(&self) -> Result<()> {
        let path = self.sequencer_ctl_binary();
        if !path.exists() {
            bail!(
                "nockchain-bridge-sequencer-ctl binary not found at {}. Build with `cargo build --release -p nockchain-bridge-sequencer --bin nockchain-bridge-sequencer-ctl` before running the manual approval bridge-dev E2E scenario",
                path.display()
            );
        }
        Ok(())
    }

    pub fn run_sequencer_ctl_checked(&self, args: &[&str]) -> Result<String> {
        self.ensure_sequencer_ctl_binary()?;
        let binary = self.sequencer_ctl_binary();
        let output = Command::new(&binary)
            .args(args)
            .arg("--sequencer-config-path")
            .arg(self.sequencer_config_path())
            .arg("--data-dir")
            .arg(self.sequencer_data_dir())
            .output()
            .with_context(|| format!("failed to run {} {}", binary.display(), args.join(" ")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() {
            return Ok(stdout.into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{} {} exited with {}\nstdout:\n{}\nstderr:\n{}",
            binary.display(),
            args.join(" "),
            output.status,
            redact(&stdout),
            redact(&stderr)
        )
    }

    pub fn sequencer_sqlite_path(&self) -> PathBuf {
        self.run_root
            .join("node")
            .join("nockchain")
            .join("withdrawal-state-store.sqlite")
    }

    pub fn remove_sequencer_sqlite(&self) -> Result<()> {
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

    pub fn ensure_up_still_running(&mut self) -> Result<()> {
        if let Some(child) = &mut self.up_child {
            if let Some(status) = child
                .try_wait()
                .context("failed to inspect bridge-dev up")?
            {
                self.preserve_failure_artifacts();
                bail!(
                    "bridge-dev up exited early with {status}: {}",
                    self.up_log_context()
                );
            }
        }
        Ok(())
    }

    pub fn up_log_context(&self) -> String {
        format!(
            "up stdout:\n{}\nup stderr:\n{}",
            redacted_tail(&self.up_stdout),
            redacted_tail(&self.up_stderr)
        )
    }

    pub fn cluster_context(&self) -> String {
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

    pub fn current_log_paths(&self) -> Vec<PathBuf> {
        let current_dir = self.run_root.join("bridge-dev/current");
        let mut paths = vec![
            current_dir.join("supervisor.log"),
            current_dir.join("node.stderr.log"),
            current_dir.join("node.stdout.log"),
        ];
        for node_id in ALL_BRIDGE_NODES {
            paths.push(current_dir.join(format!("bridge-{node_id}.stderr.log")));
            paths.push(current_dir.join(format!("bridge-{node_id}.stdout.log")));
        }
        paths
    }

    pub fn evidence_log_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.up_stdout.clone(), self.up_stderr.clone()];
        paths.extend(self.current_log_paths());
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn assert_withdrawal_build_selected_input_count(&self, expected: usize) -> Result<()> {
        let expected_marker = format!("selected_inputs={expected}");
        let mut build_lines = Vec::new();
        for path in self.current_log_paths() {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for line in contents.lines() {
                if line.contains("requesting withdrawal proposal build from kernel") {
                    let plain_line = strip_ansi_codes(line);
                    if plain_line.contains(&expected_marker) {
                        return Ok(());
                    }
                    build_lines.push(format!(
                        "{}: {}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("log"),
                        redact(line)
                    ));
                }
            }
        }
        if build_lines.is_empty() {
            bail!(
                "did not find a withdrawal proposal build log line while checking for {expected_marker}: {}",
                self.cluster_context()
            );
        }
        bail!(
            "withdrawal proposal build did not use {expected} selected inputs; observed:\n{}",
            build_lines.join("\n")
        );
    }

    pub fn assert_no_stop_conditions_in_logs(&self) -> Result<()> {
        let mut matches = Vec::new();
        for path in self.current_log_paths() {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for (line_index, line) in contents.lines().enumerate() {
                if STOP_CONDITION_LOG_MARKERS
                    .iter()
                    .any(|marker| line.contains(marker))
                {
                    matches.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        line_index + 1,
                        redact(line)
                    ));
                }
            }
        }
        if matches.is_empty() {
            Ok(())
        } else {
            bail!(
                "found bridge stop-condition markers in scenario logs:\n{}",
                matches.join("\n")
            )
        }
    }

    pub fn stop(&mut self) {
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

impl Drop for ScenarioHarness {
    fn drop(&mut self) {
        let keep_run_root = self.preserve_run_root.get()
            || std::thread::panicking()
            || env::var(KEEP_RUN_ROOT_ENV).ok().as_deref() == Some("1");
        self.stop();
        if keep_run_root {
            if let Some(tempdir) = self.tempdir.take() {
                eprintln!(
                    "preserving bridge-dev scenario tempdir at {} after failure or explicit request",
                    tempdir.keep().display()
                );
            }
        }
    }
}
fn process_target_names(targets: &[ProcessTarget]) -> Result<Vec<String>> {
    if targets.is_empty() {
        bail!("at least one process target is required");
    }
    targets.iter().copied().map(ProcessTarget::name).collect()
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
    env::var_os(BRIDGE_DEV_BIN_ENV)
        .or_else(|| option_env!("CARGO_BIN_EXE_bridge-dev").map(std::ffi::OsString::from))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root()
                .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
                .join("target/debug/bridge-dev")
        })
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

pub fn assert_contains_all(haystack: &str, needles: &[&str]) -> Result<()> {
    for needle in needles {
        if !haystack.contains(needle) {
            bail!("output missing {needle:?}:\n{}", redact(haystack));
        }
    }
    Ok(())
}

pub fn assert_contains(haystack: &str, needle: &str) -> Result<()> {
    assert_contains_all(haystack, &[needle])
}

pub fn process_state<'a>(status: &'a str, component_name: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        (columns.first().copied() == Some(component_name))
            .then(|| columns.get(1).copied())
            .flatten()
    })
}

pub fn assert_processes_running(status: &str, component_names: &[&str]) -> Result<()> {
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

pub fn assert_processes_not_running(status: &str, component_names: &[&str]) -> Result<()> {
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

pub fn parse_status_nock_height(status: &str) -> Result<u64> {
    status
        .lines()
        .find_map(|line| {
            let line = line.trim();
            if !line.starts_with("bridge-0") {
                return None;
            }
            parse_line_u64_field(line, "nock_height=")
        })
        .ok_or_else(|| anyhow!("status output did not include bridge-0 nock_height:\n{status}"))
}

fn parse_line_u64_field(line: &str, field: &str) -> Option<u64> {
    let (_, rest) = line.split_once(field)?;
    rest.split_whitespace().next()?.parse().ok()
}

pub fn bridge_stream_line(status: &str, node_id: usize) -> Option<&str> {
    let prefix = format!("bridge-{node_id} ");
    status
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(&prefix) && line.contains("running_state="))
}

pub fn assert_bridge_streams_available(status: &str, node_ids: &[usize]) -> Result<()> {
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

pub fn assert_cluster_available(status: &str) -> Result<()> {
    assert_contains_all(
        status,
        &["processes:", "bridge_streams:", "sequencer_status:"],
    )?;
    assert_processes_running(status, ALL_COMPONENTS)?;
    assert_bridge_streams_available(status, ALL_BRIDGE_NODES)
}

pub fn assert_sequencer_idle(status: &str) -> Result<()> {
    assert_contains_all(status, &["reserved_inputs=0", "next_pending=none"])
}

pub fn assert_queue_drained(status: &str) -> Result<()> {
    assert_contains_all(
        status,
        &[
            "pending_deposits=0", "pending_withdrawals=0", "unsettled_deposits=0",
            "unsettled_withdrawals=0",
        ],
    )
}

pub fn assert_successful_deposit_on_all_nodes(
    scenario: &mut ScenarioHarness,
    expected: &ObservedDeposit,
    timeout_secs: u64,
) -> Result<()> {
    assert_successful_deposit_on_all_nodes_after(scenario, expected, timeout_secs, None)
}

pub fn assert_successful_deposit_on_all_nodes_after(
    scenario: &mut ScenarioHarness,
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
    scenario: &ScenarioHarness,
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

pub fn bridge_data_dir_has_reboot_state(data_dir: &Path) -> bool {
    checkpoint_dir_has_nonempty_checkpoint(&data_dir.join("checkpoints"))
        || pma_dir_has_nonempty_snapshot(&data_dir.join("pma"))
        || nonempty_file(&data_dir.join("event-log.sqlite3"))
}

pub fn checkpoint_dir_has_nonempty_checkpoint(checkpoint_dir: &Path) -> bool {
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

pub fn assert_same_deposit(
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

pub fn assert_same_deposit_identity(
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

pub fn assert_deposit_nonce_increased(
    first: &ObservedDeposit,
    second: &ObservedDeposit,
) -> Result<()> {
    if second.nonce <= first.nonce {
        bail!("second deposit nonce did not advance: first={:?}, second={:?}", first, second);
    }
    Ok(())
}

pub fn parse_observed_deposit(
    output: &str,
    phase: ObservedDepositPhase,
) -> Result<ObservedDeposit> {
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

pub fn parse_observed_withdrawal(output: &str, expected_phase: &str) -> Result<ObservedWithdrawal> {
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
    if is_placeholder(value) {
        bail!("withdrawal {} has placeholder {field_name}: {:?}", withdrawal.phase, withdrawal);
    }
    Ok(())
}

fn is_placeholder(value: &str) -> bool {
    value == "-" || value.trim().is_empty()
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

pub fn assert_withdrawal_progression(
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
fn redacted_tail(path: &Path) -> String {
    let Ok(contents) = fs::read_to_string(path) else {
        return "<unavailable>".to_string();
    };
    let lines = contents.lines().rev().take(80).collect::<Vec<_>>();
    let tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    redact(&tail)
}

pub fn redact(value: &str) -> String {
    SECRET_ENV.iter().fold(value.to_string(), |redacted, key| {
        let Ok(sensitive_value) = env::var(key) else {
            return redacted;
        };
        let sensitive_value = sensitive_value.trim();
        if sensitive_value.is_empty() {
            redacted
        } else {
            redacted.replace(sensitive_value, "<redacted>")
        }
    })
}

fn strip_ansi_codes(value: &str) -> String {
    let mut stripped = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            stripped.push(ch);
            continue;
        }
        if chars.next_if_eq(&'[').is_none() {
            continue;
        }
        for code in chars.by_ref() {
            if code.is_ascii_alphabetic() {
                break;
            }
        }
    }
    stripped
}
