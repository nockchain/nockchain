use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use bridge::shared::e2e_environment::{BASE_SEPOLIA_E2E_CHAIN_ID, BASE_SEPOLIA_E2E_ENVIRONMENT_ID};
use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Instant};

use crate::base_backend::{BaseBackend, BaseBackendError, SnapshotId};
use crate::environment::BaseE2eEnvironment;
use crate::nonproduction_guard::{LoopbackBaseRpcUrl, NonproductionGuard, NonproductionGuardError};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CAPTURED_OUTPUT_BYTES: usize = 256 * 1024;
static RESERVED_PORTS: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Clone)]
pub enum AnvilMode {
    Empty,
    Fork {
        source_rpc_url: String,
        block_number: u64,
    },
}

impl AnvilMode {
    fn id(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Fork { .. } => "fork",
        }
    }

    fn fork_block_number(&self) -> Option<u64> {
        match self {
            Self::Empty => None,
            Self::Fork { block_number, .. } => Some(*block_number),
        }
    }

    fn sensitive_values(&self) -> Vec<String> {
        match self {
            Self::Empty => Vec::new(),
            Self::Fork { source_rpc_url, .. } => vec![source_rpc_url.clone()],
        }
    }
}

impl fmt::Debug for AnvilMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Fork { block_number, .. } => formatter
                .debug_struct("Fork")
                .field("source_rpc_url", &"<redacted>")
                .field("block_number", block_number)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnvilConfig {
    pub binary: PathBuf,
    pub mode: AnvilMode,
    pub chain_id: u64,
    pub port: Option<u16>,
    pub startup_timeout: Duration,
}

impl AnvilConfig {
    pub fn empty() -> Self {
        Self {
            binary: std::env::var_os("ANVIL_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("anvil")),
            mode: AnvilMode::Empty,
            chain_id: BASE_SEPOLIA_E2E_CHAIN_ID,
            port: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
        }
    }

    pub fn fork(source_rpc_url: String, block_number: u64) -> Self {
        Self {
            mode: AnvilMode::Fork {
                source_rpc_url,
                block_number,
            },
            ..Self::empty()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnvilEvidenceFacts {
    pub binary_version: String,
    pub rpc_client_version: String,
    pub chain_id: u64,
    pub port: u16,
    pub mode: String,
    pub fork_block_number: Option<u64>,
    pub snapshot_round_trip: bool,
}

pub struct AnvilBackend {
    child: Option<Child>,
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
    output: CapturedOutput,
    _port_reservation: PortReservation,
    http_url: LoopbackBaseRpcUrl,
    ws_url: String,
    base: BaseBackend,
    facts: AnvilEvidenceFacts,
}

impl AnvilBackend {
    pub async fn start(
        config: AnvilConfig,
        environment: &BaseE2eEnvironment,
    ) -> Result<Self, AnvilStartError> {
        if matches!(config.mode, AnvilMode::Fork { .. }) {
            return Err(AnvilStartError::ForkRequiresPinnedPreflight);
        }
        Self::start_inner(config, environment).await
    }

    pub(crate) async fn start_unverified_fork(
        config: AnvilConfig,
        environment: &BaseE2eEnvironment,
    ) -> Result<Self, AnvilStartError> {
        if !matches!(config.mode, AnvilMode::Fork { .. }) {
            return Err(AnvilStartError::ExpectedForkMode);
        }
        Self::start_inner(config, environment).await
    }

    async fn start_inner(
        config: AnvilConfig,
        environment: &BaseE2eEnvironment,
    ) -> Result<Self, AnvilStartError> {
        if config.chain_id != BASE_SEPOLIA_E2E_CHAIN_ID {
            return Err(AnvilStartError::InvalidChainId {
                expected: BASE_SEPOLIA_E2E_CHAIN_ID,
                observed: config.chain_id,
            });
        }
        if config.startup_timeout.is_zero() {
            return Err(AnvilStartError::InvalidStartupTimeout);
        }

        let binary_version = read_binary_version(&config.binary).await?;
        let port_reservation = PortReservation::acquire(config.port)?;
        let port = port_reservation.port();
        let mut command = Command::new(&config.binary);
        command
            .args([
                "--silent",
                "--port",
                &port.to_string(),
                "--chain-id",
                &config.chain_id.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        if let AnvilMode::Fork {
            source_rpc_url,
            block_number,
        } = &config.mode
        {
            command.args([
                "--fork-url",
                source_rpc_url,
                "--fork-block-number",
                &block_number.to_string(),
            ]);
        }

        let mut child = command.spawn().map_err(AnvilStartError::Spawn)?;
        let output = CapturedOutput::new(config.mode.sensitive_values());
        let stdout_task = child
            .stdout
            .take()
            .map(|stdout| tokio::spawn(drain_output(stdout, output.clone())));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(drain_output(stderr, output.clone())));
        let http_raw = format!("http://127.0.0.1:{port}");
        let http_url = LoopbackBaseRpcUrl::parse(&http_raw)?;
        let deadline = Instant::now() + config.startup_timeout;

        loop {
            if let Some(status) = child.try_wait().map_err(AnvilStartError::Inspect)? {
                finish_output_tasks(stdout_task, stderr_task).await;
                return Err(AnvilStartError::ExitedBeforeReady {
                    status: status.to_string(),
                    output: output.render(),
                });
            }
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                terminate_child(&mut child).await;
                finish_output_tasks(stdout_task, stderr_task).await;
                return Err(AnvilStartError::ReadinessTimeout {
                    timeout: config.startup_timeout,
                    output: output.render(),
                });
            }
            sleep(Duration::from_millis(25)).await;
        }

        let guarded = match NonproductionGuard::acquire(
            &http_raw,
            BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
            environment.manifest(),
        )
        .await
        {
            Ok(guarded) => guarded,
            Err(error) => {
                terminate_child(&mut child).await;
                finish_output_tasks(stdout_task, stderr_task).await;
                return Err(AnvilStartError::Guard {
                    source: error,
                    output: output.render(),
                });
            }
        };
        let rpc_client_version = guarded.capabilities().client_version.clone();
        let snapshot_round_trip = guarded.capabilities().snapshot_round_trip;
        let base = BaseBackend::new(guarded)?;
        let ws_url = format!("ws://127.0.0.1:{port}");
        let facts = AnvilEvidenceFacts {
            binary_version,
            rpc_client_version,
            chain_id: config.chain_id,
            port,
            mode: config.mode.id().to_owned(),
            fork_block_number: config.mode.fork_block_number(),
            snapshot_round_trip,
        };

        Ok(Self {
            child: Some(child),
            stdout_task,
            stderr_task,
            output,
            _port_reservation: port_reservation,
            http_url,
            ws_url,
            base,
            facts,
        })
    }

    pub fn http_url(&self) -> &LoopbackBaseRpcUrl {
        &self.http_url
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub fn facts(&self) -> &AnvilEvidenceFacts {
        &self.facts
    }

    pub fn backend(&self) -> &BaseBackend {
        &self.base
    }

    pub fn captured_output(&self) -> String {
        self.output.render()
    }

    pub fn nonce_epoch(&self) -> u64 {
        self.base.nonce_epoch()
    }

    pub async fn snapshot(&self) -> Result<SnapshotId, BaseBackendError> {
        self.base.snapshot().await
    }

    pub async fn revert(&self, snapshot: &SnapshotId) -> Result<bool, BaseBackendError> {
        self.base.revert(snapshot).await
    }

    pub async fn mine(&self, blocks: u64) -> Result<(), BaseBackendError> {
        self.base.mine(blocks).await
    }

    pub async fn set_balance(
        &self,
        address: Address,
        balance: U256,
    ) -> Result<(), BaseBackendError> {
        self.base.set_balance(address, balance).await
    }

    pub async fn balance(&self, address: Address) -> Result<U256, BaseBackendError> {
        self.base.balance(address).await
    }

    pub async fn impersonate(&self, address: Address) -> Result<(), BaseBackendError> {
        self.base.impersonate(address).await
    }

    pub async fn stop_impersonating(&self, address: Address) -> Result<(), BaseBackendError> {
        self.base.stop_impersonating(address).await
    }

    pub async fn block_number(&self) -> Result<u64, BaseBackendError> {
        self.base.block_number().await
    }

    pub async fn block_hash(&self, number: u64) -> Result<B256, BaseBackendError> {
        self.base.block_hash(number).await
    }

    pub async fn shutdown(mut self) -> Result<(), AnvilShutdownError> {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child).await;
        }
        if let Some(task) = self.stdout_task.take() {
            let _ = task.await;
        }
        if let Some(task) = self.stderr_task.take() {
            let _ = task.await;
        }
        Ok(())
    }
}

impl Drop for AnvilBackend {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            terminate_child_group_now(child);
        }
        if let Some(task) = self.stdout_task.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Error)]
pub enum AnvilStartError {
    #[error("Anvil must use dedicated E2E chain id {expected}, got {observed}")]
    InvalidChainId { expected: u64, observed: u64 },
    #[error("fork mode is available only through the pinned preflight orchestrator")]
    ForkRequiresPinnedPreflight,
    #[error("internal pinned-fork start requires fork mode")]
    ExpectedForkMode,
    #[error("Anvil startup timeout must be nonzero")]
    InvalidStartupTimeout,
    #[error("failed to execute Anvil version probe")]
    VersionProbe(#[source] std::io::Error),
    #[error("Anvil version probe failed: {0}")]
    InvalidVersion(String),
    #[error("requested Anvil port {port} is unavailable")]
    PortUnavailable { port: u16 },
    #[error("failed to allocate an isolated Anvil port")]
    PortAllocation(#[source] std::io::Error),
    #[error("failed to start Anvil")]
    Spawn(#[source] std::io::Error),
    #[error("failed to inspect Anvil process")]
    Inspect(#[source] std::io::Error),
    #[error("Anvil exited before readiness with {status}; output:\n{output}")]
    ExitedBeforeReady { status: String, output: String },
    #[error("Anvil did not become ready within {timeout:?}; output:\n{output}")]
    ReadinessTimeout { timeout: Duration, output: String },
    #[error("Anvil failed nonproduction guard; output:\n{output}")]
    Guard {
        #[source]
        source: NonproductionGuardError,
        output: String,
    },
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
    #[error(transparent)]
    Endpoint(#[from] NonproductionGuardError),
}

#[derive(Debug, Error)]
#[error("failed to shut down Anvil")]
pub struct AnvilShutdownError;

async fn read_binary_version(binary: &PathBuf) -> Result<String, AnvilStartError> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(AnvilStartError::VersionProbe)?;
    if !output.status.success() {
        return Err(AnvilStartError::InvalidVersion(
            "version command exited nonzero".to_owned(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or_default().trim();
    if !first_line.starts_with("anvil Version: ") {
        return Err(AnvilStartError::InvalidVersion(
            "version output did not identify Anvil".to_owned(),
        ));
    }
    Ok(first_line.to_owned())
}

async fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn terminate_child_group_now(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.start_kill();
}

async fn finish_output_tasks(
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
) {
    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
}

async fn drain_output<R>(mut reader: R, output: CapturedOutput)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0u8; 4096];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => output.push(&buffer[..read]),
        }
    }
}

#[derive(Clone)]
struct CapturedOutput {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    sensitive_values: Arc<Vec<String>>,
}

impl CapturedOutput {
    fn new(sensitive_values: Vec<String>) -> Self {
        Self {
            bytes: Arc::new(Mutex::new(VecDeque::new())),
            sensitive_values: Arc::new(sensitive_values),
        }
    }

    fn push(&self, bytes: &[u8]) {
        let mut output = self
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        output.extend(bytes);
        while output.len() > MAX_CAPTURED_OUTPUT_BYTES {
            output.pop_front();
        }
    }

    fn render(&self) -> String {
        let output = self
            .bytes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bytes = output.iter().copied().collect::<Vec<_>>();
        let mut rendered = String::from_utf8_lossy(&bytes).into_owned();
        for sensitive_value in self.sensitive_values.iter() {
            rendered = rendered.replace(sensitive_value, "<redacted-source-rpc>");
        }
        rendered
    }
}
struct PortReservation {
    port: u16,
}

impl PortReservation {
    fn acquire(requested: Option<u16>) -> Result<Self, AnvilStartError> {
        let reserved = &*RESERVED_PORTS;
        let mut reserved = reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(port) = requested.filter(|port| *port != 0) {
            if reserved.contains(&port) || TcpListener::bind(("127.0.0.1", port)).is_err() {
                return Err(AnvilStartError::PortUnavailable { port });
            }
            reserved.insert(port);
            return Ok(Self { port });
        }

        for _ in 0..32 {
            let listener =
                TcpListener::bind("127.0.0.1:0").map_err(AnvilStartError::PortAllocation)?;
            let port = listener
                .local_addr()
                .map_err(AnvilStartError::PortAllocation)?
                .port();
            if reserved.insert(port) {
                return Ok(Self { port });
            }
        }
        Err(AnvilStartError::PortAllocation(std::io::Error::other(
            "failed to find an unreserved port",
        )))
    }

    fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for PortReservation {
    fn drop(&mut self) {
        let reserved = &*RESERVED_PORTS;
        let mut reserved = reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reserved.remove(&self.port);
    }
}
