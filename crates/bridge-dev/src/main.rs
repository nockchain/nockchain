#![allow(clippy::too_many_arguments)]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::network::{EthereumWallet, Network};
use alloy::primitives::{Address, U256};
use alloy::providers::fillers::{
    CachedNonceManager, ChainIdFiller, GasFiller, NonceFiller, WalletFiller,
};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use anyhow::{anyhow, bail, Context, Result};
use bridge::observability::tui::types::format_nock_from_nicks;
use bridge::observability::tui_api::proto as tui_proto;
use bridge::observability::tui_api::proto::bridge_tui_client::BridgeTuiClient;
use bridge::shared::base::encode_withdrawal_burn_calldata;
use bridge::shared::config::{
    canonical_testing_bridge_lock_root, derive_bridge_spend_authority_from_pkhs, BridgeConfigToml,
    BridgeConstantsToml, NodeInfoToml, SequencerConfigToml, SequencerJournalConfigToml,
    SequencerNodeInfoToml,
};
use bridge::shared::ingress::proto as ingress_proto;
use bridge::shared::ingress::proto::bridge_ingress_client::BridgeIngressClient;
use bridge::shared::ingress::proto::withdrawal_sequencer_client::WithdrawalSequencerClient;
use bridge::shared::proposer::withdrawal_turn_proposer;
use bridge::shared::signing::BridgeSigner;
use bridge::withdrawal::transport::withdrawal_id_from_proto;
use clap::{Args, Parser, Subcommand};
use ibig::UBig;
use nockapp_grpc::pb::common::v2::note;
use nockapp_grpc::services::public_nockchain::v2::client::{
    BalanceRequest, PublicNockchainGrpcClient,
};
use nockchain_math::crypto::cheetah::{ch_scal_big, A_GEN};
use nockchain_types::common::Hash as NockHash;
use nockchain_types::tx_engine::common::{FirstName, SchnorrPubkey};
use nockchain_types::tx_engine::v1::tx::{Lock, LockPrimitive, Pkh, SpendCondition};
use op_alloy::network::Optimism;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};
use tokio::process::{Child, Command as TokioCommand};
use tokio::time::{sleep, Instant};

const NODE_BIND_PORT: u16 = 3005;
const NODE_PUBLIC_GRPC_PORT: u16 = 5001;
const NODE_PRIVATE_GRPC_PORT: u16 = 5002;
const WITHDRAWAL_SEQUENCER_API_PORT_DELTA: u16 = 100;
const STATUS_BALANCE_TIMEOUT: Duration = Duration::from_secs(3);
const NODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const BRIDGE_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const BRIDGE_DEV_WITHDRAWAL_HANDOFF_WINDOW_BLOCKS: u64 = 10;
const BRIDGE_DEV_TEST_RUN_ROOT_ENV: &str = "BRIDGE_DEV_TEST_RUN_ROOT";
const BRIDGE_DEV_PORT_OFFSET_ENV: &str = "BRIDGE_DEV_PORT_OFFSET";
const BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV: &str = "BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED";
const BRIDGE_DEV_SEQUENCER_JOURNAL_SIGNING_KEY_ENV: &str =
    "BRIDGE_DEV_SEQUENCER_JOURNAL_SIGNING_KEY";
const BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT_ENV: &str =
    "BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT";
const BRIDGE_DEV_FAKENET_GENESIS_JAM_ENV: &str = "BRIDGE_DEV_FAKENET_GENESIS_JAM";
const BRIDGE_DEV_FAKENET_POW_LEN_ENV: &str = "BRIDGE_DEV_FAKENET_POW_LEN";
const BRIDGE_DEV_FAKENET_LOG_DIFFICULTY_ENV: &str = "BRIDGE_DEV_FAKENET_LOG_DIFFICULTY";
const BRIDGE_DEV_BASE_BLOCKS_CHUNK_ENV: &str = "BRIDGE_DEV_BASE_BLOCKS_CHUNK";
const BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV: &str = "BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS";
const FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES: &str =
    "nockchain/jams/fakenet-genesis-pow-64-bex-2.jam";
const FAKENET_POW_LEN: u64 = 64;
const FAKENET_LOG_DIFFICULTY: u64 = 2;
const CHILD_SIGINT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(120);
const FAKENET_MINING_PKH: &str = "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt";
const BRIDGE_DEV_DEFAULT_SEQUENCER_JOURNAL_SIGNING_KEY: &str =
    "0x59c6995e998f97a5a0044966f09453892d69e3f67122e7bd1c4ef5e6d8e0e6df";
const BRIDGE_INGRESS_PORTS: [u16; 5] = [8002, 8003, 8004, 8005, 8006];
const BRIDGE_ETH_KEYS: [&str; 5] = [
    "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318",
    "0x5c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362319",
    "0x6c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231a",
    "0x7c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231b",
    "0x8c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f36231c",
];
const BRIDGE_ETH_ADDRS: [&str; 5] = [
    "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23", "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9",
    "0x274BD645de480C325D618c60c661F11275eB77F1", "0x6dc59eb20f7928935c47A391e35545a2CEC51013",
    "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375",
];
const BRIDGE_NOCK_KEYS: [&str; 5] = [
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8T", "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8U",
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8V", "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8W",
    "5KZuFKrctV5iUburT54Z9fhpf3V3hv2sPf9GRQnjFR8X",
];
const BRIDGE_NOCK_PKHS: [&str; 5] = [
    "A47ZMEQ2U2x1h3bVMUNdkutKYNiyXFWMVTQZC8BWgXBmS5mc6ysAhLZ",
    "BYp766x6Zhu7DHbewMHu7ajsAenRMm1M7rgmpxUwY83BJy4RGMAG2z8",
    "2f7BtZpaaKVb9mCUFgMuYjcQXhrexfqCJs4h1es5t9jQrqdmhVgYLU6",
    "BLCg8KPPKDJPJ8hhdHSGsurxgKwBorqpF1qrHsCiojsPf96GEzwsFQ",
    "AeZ1jsSHoAg7bjBr2k4kMeRERsx85Bp68tfTMiiYZtjFRCtc4gexNWc",
];
const NICKS_PER_NOCK: u64 = 65_536;
const NOCK_BASE_PER_NICK: u128 = 152_587_890_625;

fn optional_env_string(key: &str) -> Result<Option<String>> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => bail!("{key} must be valid UTF-8"),
    }
}

fn parse_bridge_dev_port_offset(raw: Option<&str>) -> Result<u16> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    trimmed
        .parse::<u16>()
        .with_context(|| format!("{BRIDGE_DEV_PORT_OFFSET_ENV} must be a u16 port offset"))
}

fn bridge_dev_port_offset() -> Result<u16> {
    parse_bridge_dev_port_offset(optional_env_string(BRIDGE_DEV_PORT_OFFSET_ENV)?.as_deref())
}

fn offset_port_with_offset(label: &str, port: u16, offset: u16) -> Result<u16> {
    port.checked_add(offset).ok_or_else(|| {
        anyhow!("{label} port {port} overflows with {BRIDGE_DEV_PORT_OFFSET_ENV}={offset}")
    })
}

fn offset_port(label: &str, port: u16) -> Result<u16> {
    offset_port_with_offset(label, port, bridge_dev_port_offset()?)
}

fn node_bind_addr() -> Result<String> {
    Ok(format!(
        "/ip4/0.0.0.0/udp/{}/quic-v1",
        offset_port("node bind", NODE_BIND_PORT)?
    ))
}

fn node_public_grpc_addr() -> Result<String> {
    Ok(format!(
        "127.0.0.1:{}",
        offset_port("node public gRPC", NODE_PUBLIC_GRPC_PORT)?
    ))
}

fn node_private_grpc_port() -> Result<u16> {
    offset_port("node private gRPC", NODE_PRIVATE_GRPC_PORT)
}

fn node_private_grpc_addr() -> Result<String> {
    Ok(format!("127.0.0.1:{}", node_private_grpc_port()?))
}

fn sequencer_api_port_with_offset(offset: u16) -> Result<u16> {
    let base_port = NODE_PRIVATE_GRPC_PORT
        .checked_add(WITHDRAWAL_SEQUENCER_API_PORT_DELTA)
        .expect("bridge-dev sequencer base port must fit in u16");
    offset_port_with_offset("withdrawal sequencer API", base_port, offset)
}

fn sequencer_api_port() -> Result<u16> {
    sequencer_api_port_with_offset(bridge_dev_port_offset()?)
}

fn bridge_ingress_port_with_offset(node_id: usize, offset: u16) -> Result<u16> {
    let port = BRIDGE_INGRESS_PORTS
        .get(node_id)
        .copied()
        .ok_or_else(|| anyhow!("invalid bridge node index {node_id}"))?;
    offset_port_with_offset(&format!("bridge-{node_id} ingress"), port, offset)
}

fn bridge_ingress_port(node_id: usize) -> Result<u16> {
    bridge_ingress_port_with_offset(node_id, bridge_dev_port_offset()?)
}

fn parse_optional_u64_env_value(key: &str, raw: Option<&str>) -> Result<Option<u64>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .with_context(|| format!("{key} must be a u64 height"))
}

fn optional_u64_env(key: &str) -> Result<Option<u64>> {
    parse_optional_u64_env_value(key, optional_env_string(key)?.as_deref())
}

fn parse_optional_millis_env_value(key: &str, raw: Option<&str>) -> Result<Option<u64>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .with_context(|| format!("{key} must be a u64 millisecond value"))
}

fn parse_bridge_save_interval_event_time_secs(raw: Option<&str>) -> Result<Option<u64>> {
    Ok(
        parse_optional_millis_env_value(BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV, raw)?
            .map(|millis| millis.saturating_add(999) / 1000),
    )
}

fn bridge_save_interval_event_time_secs() -> Result<Option<u64>> {
    parse_bridge_save_interval_event_time_secs(
        optional_env_string(BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV)?.as_deref(),
    )
}

fn withdrawal_activation_nock_next_height() -> Result<u64> {
    Ok(optional_u64_env(BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT_ENV)?.unwrap_or(1))
}

fn fakenet_genesis_jam_path(paths: &Paths) -> Result<PathBuf> {
    resolve_fakenet_genesis_jam_path(
        &paths.workspace_root,
        &paths.crates_dir,
        optional_env_string(BRIDGE_DEV_FAKENET_GENESIS_JAM_ENV)?.as_deref(),
    )
}

fn resolve_fakenet_genesis_jam_path(
    workspace_root: &Path,
    crates_dir: &Path,
    override_path: Option<&str>,
) -> Result<PathBuf> {
    match override_path {
        Some(override_path) => {
            let path = PathBuf::from(override_path);
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(workspace_root.join(path))
            }
        }
        None => Ok(crates_dir.join(FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES)),
    }
}

fn fakenet_pow_len() -> Result<u64> {
    Ok(optional_u64_env(BRIDGE_DEV_FAKENET_POW_LEN_ENV)?.unwrap_or(FAKENET_POW_LEN))
}

fn fakenet_log_difficulty() -> Result<u64> {
    Ok(optional_u64_env(BRIDGE_DEV_FAKENET_LOG_DIFFICULTY_ENV)?.unwrap_or(FAKENET_LOG_DIFFICULTY))
}

fn base_blocks_chunk() -> Result<u64> {
    Ok(optional_u64_env(BRIDGE_DEV_BASE_BLOCKS_CHUNK_ENV)?.unwrap_or(1))
}

alloy::sol! {
    struct DevTip5Hash {
        uint64[5] limbs;
    }

    #[sol(rpc)]
    contract DevMessageInbox {
        function bridgeNodes(uint256 index) external view returns (address);
        function lastDepositNonce() external view returns (uint256);
        function withdrawalsEnabled() external view returns (bool);
        function submitDeposit(
            DevTip5Hash txId,
            DevTip5Hash nameFirst,
            DevTip5Hash nameLast,
            address recipient,
            uint256 amount,
            uint256 blockHeight,
            DevTip5Hash asOf,
            uint256 depositNonce,
            bytes[] ethSigs
        ) external;
    }

    #[sol(rpc)]
    contract DevNock {
        function inbox() external view returns (address);
        function balanceOf(address account) external view returns (uint256);
        function mint(address to, uint256 amount) external;
        function burn(uint256 amount, bytes32 lockRoot) external;
        function updateInbox(address newInbox) external;
    }
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Manual bridge harness for local cluster + Tenderly VNETs"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(
        long,
        default_value = "virtual-testnet",
        help = "Profile name expected inside the bridge-dev profile file"
    )]
    profile: String,

    #[arg(
        long,
        help = "Profile config file (default: <bridge>/scripts/environments/bridge-dev.toml)"
    )]
    profile_path: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Up(UpArgs),
    Down,
    Status(StatusArgs),
    Watch(WatchArgs),
    Wait(WaitArgs),
    Info,
    Logs(LogsArgs),
    Stop(ComponentTargetArgs),
    Start(ComponentTargetArgs),
    Restart(ComponentTargetArgs),
    Deposit(DepositArgs),
    MintForBurn(MintForBurnArgs),
    RequestWithdrawal(RequestWithdrawalArgs),
    AdvanceBase(AdvanceBaseArgs),
}

#[derive(Args, Debug)]
struct UpArgs {
    #[arg(
        long = "fresh",
        help = "Provision a fresh Tenderly VNET and reset all local state, including sequencer state"
    )]
    fresh: bool,

    #[arg(
        long = "fresh-vnet",
        help = "Provision a fresh Tenderly VNET and redeploy contracts before startup"
    )]
    fresh_vnet: bool,

    #[arg(
        long = "fresh-state",
        help = "Reset local bridge and node runtime state before startup while preserving sequencer state"
    )]
    fresh_state: bool,

    #[arg(
        long,
        help = "Pass --start to bridge processes on boot to clear persisted kernel stop state"
    )]
    start: bool,
}

impl UpArgs {
    fn fresh_vnet(&self) -> bool {
        self.fresh || self.fresh_vnet
    }

    fn fresh_state(&self) -> bool {
        self.fresh || self.fresh_state
    }
}

#[derive(Args, Debug, Default)]
struct StatusArgs {
    #[arg(
        long,
        help = "Query TUI snapshots from all five bridge nodes in addition to bridge-0"
    )]
    bridges: bool,

    #[arg(
        long,
        help = "Query the colocated withdrawal sequencer for pending withdrawal state"
    )]
    sequencer: bool,
}

#[derive(Args, Debug)]
struct LogsArgs {
    #[arg(
        default_value = "supervisor",
        help = "supervisor, node, or bridge-0..bridge-4"
    )]
    target: String,

    #[arg(long, help = "Follow the log output")]
    follow: bool,
}

#[derive(Args, Debug)]
struct WatchArgs {
    #[arg(
        long,
        default_value_t = 1000,
        help = "Polling interval in milliseconds"
    )]
    interval_ms: u64,
}

#[derive(Args, Debug)]
struct WaitArgs {
    #[command(subcommand)]
    command: WaitCommand,
}

#[derive(Subcommand, Debug)]
enum WaitCommand {
    Deposit(WaitDepositArgs),
    Withdrawal(WaitWithdrawalArgs),
}

#[derive(Args, Debug)]
struct WaitDepositArgs {
    #[arg(
        long,
        conflicts_with = "successful",
        help = "Wait until a deposit is submitted"
    )]
    submitted: bool,

    #[arg(long, help = "Wait until a deposit is successful")]
    successful: bool,

    #[arg(
        long,
        default_value_t = 0,
        help = "Bridge node id whose TUI snapshot should be queried"
    )]
    node_id: usize,

    #[arg(long, default_value_t = 60, help = "Timeout in seconds")]
    timeout_secs: u64,

    #[arg(
        long,
        help = "Only match deposits with a nonce greater than this value"
    )]
    after_nonce: Option<u64>,
}

#[derive(Args, Debug)]
struct WaitWithdrawalArgs {
    #[arg(long, conflicts_with_all = ["ready", "submitted", "executed"], help = "Wait until a withdrawal is pending")]
    pending: bool,

    #[arg(long, conflicts_with_all = ["pending", "submitted", "executed"], help = "Wait until a withdrawal is ready")]
    ready: bool,

    #[arg(long, conflicts_with_all = ["pending", "ready", "executed"], help = "Wait until a withdrawal is submitted")]
    submitted: bool,

    #[arg(long, conflicts_with_all = ["pending", "ready", "submitted"], help = "Wait until a withdrawal is executed")]
    executed: bool,

    #[arg(long, default_value_t = 120, help = "Timeout in seconds")]
    timeout_secs: u64,

    #[arg(long, help = "Expected withdrawal id.as_of hex bytes")]
    withdrawal_id_as_of_hex: Option<String>,

    #[arg(long, help = "Expected withdrawal id.base_event_id hex bytes")]
    withdrawal_id_base_event_hex: Option<String>,

    #[arg(long, help = "Expected withdrawal nonce")]
    withdrawal_nonce: Option<u64>,
}

#[derive(Args, Debug)]
struct ComponentTargetArgs {
    #[arg(num_args = 1.., help = "one or more targets: node or bridge-0..bridge-4")]
    targets: Vec<String>,
}

#[derive(Args, Debug)]
struct DepositArgs {
    #[arg(long, help = "Named deposit alias from the profile")]
    to: Option<String>,

    #[arg(long, help = "Amount in nicks")]
    amount_nicks: Option<u64>,
}

#[derive(Args, Debug)]
struct MintForBurnArgs {
    #[arg(long, help = "Named recipient alias from the profile")]
    to: Option<String>,

    #[arg(
        long,
        default_value = "1000",
        help = "Amount in NOCK, must be exactly representable in nick granularity"
    )]
    amount_nock: String,
}

#[derive(Args, Debug)]
struct RequestWithdrawalArgs {
    #[arg(long, help = "Named holder alias from the profile")]
    from: Option<String>,

    #[arg(long, help = "Named withdrawal destination alias from the profile")]
    to: Option<String>,

    #[arg(long, help = "Amount in NOCK, supports up to 16 decimals")]
    amount_nock: String,
}

#[derive(Args, Debug)]
struct AdvanceBaseArgs {
    #[arg(long)]
    blocks: u64,
}

#[derive(Debug, Clone)]
struct SourceLayout {
    workspace_root: PathBuf,
    crates_dir: PathBuf,
    bridge_dir: PathBuf,
}

impl SourceLayout {
    fn discover_from_manifest_dir(manifest_dir: &Path) -> Result<Self> {
        let crates_dir = manifest_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                anyhow!(
                    "failed to resolve crates directory from {}",
                    manifest_dir.display()
                )
            })?;
        let source_root = crates_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
            anyhow!(
                "failed to resolve source root from {}",
                crates_dir.display()
            )
        })?;
        let workspace_root = if source_root.file_name().and_then(|name| name.to_str())
            == Some("open")
            && source_root.parent().is_some_and(|parent| {
                parent.join("Cargo.toml").is_file() || parent.join("MODULE.bazel").is_file()
            }) {
            source_root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| source_root.clone())
        } else {
            source_root
        };
        let bridge_dir = crates_dir.join("bridge");
        Ok(Self {
            workspace_root,
            crates_dir,
            bridge_dir,
        })
    }
}

#[derive(Debug, Clone)]
struct Paths {
    workspace_root: PathBuf,
    crates_dir: PathBuf,
    bridge_dir: PathBuf,
    test_data_dir: PathBuf,
    current_dir: PathBuf,
    manifest_path: PathBuf,
    control_socket: PathBuf,
    supervisor_log: PathBuf,
    env_file: PathBuf,
    deploy_script: PathBuf,
    cleanup_script: PathBuf,
    advance_blocks_script: PathBuf,
    deposit_script: PathBuf,
    bin_dir: PathBuf,
}

impl Paths {
    fn discover() -> Result<Self> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let layout = SourceLayout::discover_from_manifest_dir(&manifest_dir)?;
        Self::from_layout(layout)
    }

    fn from_layout(layout: SourceLayout) -> Result<Self> {
        let SourceLayout {
            workspace_root,
            crates_dir,
            bridge_dir,
        } = layout;
        let test_data_dir_override = std::env::var_os(BRIDGE_DEV_TEST_RUN_ROOT_ENV)
            .filter(|value| !value.as_os_str().is_empty())
            .map(PathBuf::from);
        let test_data_dir = resolve_test_data_dir(&bridge_dir, test_data_dir_override.as_deref());
        let bridge_dev_dir = test_data_dir.join("bridge-dev");
        let current_dir = bridge_dev_dir.join("current");
        let env_file = resolve_generated_env_path(
            &bridge_dir,
            &test_data_dir,
            test_data_dir_override.is_some(),
        );
        Ok(Self {
            workspace_root: workspace_root.clone(),
            crates_dir,
            bridge_dir: bridge_dir.clone(),
            test_data_dir,
            current_dir: current_dir.clone(),
            manifest_path: current_dir.join("manifest.json"),
            control_socket: current_dir.join("control.sock"),
            supervisor_log: current_dir.join("supervisor.log"),
            env_file,
            deploy_script: bridge_dir.join("scripts/tenderly-vnet-deploy.sh"),
            cleanup_script: bridge_dir.join("scripts/tenderly-vnet-cleanup.sh"),
            advance_blocks_script: bridge_dir.join("scripts/tenderly-advance-blocks.sh"),
            deposit_script: bridge_dir.join("scripts/create-bridge-spend.sh"),
            bin_dir: workspace_root.join("target/release"),
        })
    }

    fn default_profile_path(&self) -> PathBuf {
        self.bridge_dir.join("scripts/environments/bridge-dev.toml")
    }

    fn node_dir(&self) -> PathBuf {
        self.test_data_dir.join("node")
    }

    fn wallet_dir(&self) -> PathBuf {
        self.test_data_dir.join("wallet")
    }

    fn bridge_data_dir(&self, node_id: usize) -> PathBuf {
        self.test_data_dir.join(format!("bridge-{node_id}"))
    }

    fn bridge_config_dir(&self) -> PathBuf {
        self.test_data_dir.join("bridge-configs")
    }

    fn bridge_config_path(&self, node_id: usize) -> PathBuf {
        self.bridge_config_dir()
            .join(format!("bridge-{node_id}-conf.toml"))
    }

    fn sequencer_config_path(&self) -> PathBuf {
        self.bridge_config_dir().join("sequencer-conf.toml")
    }

    fn bridge_runtime_log_dir(&self, node_id: usize) -> PathBuf {
        self.current_dir.join(format!("bridge-{node_id}-logs"))
    }

    fn stdout_log(&self, name: &str) -> PathBuf {
        self.current_dir.join(format!("{name}.stdout.log"))
    }

    fn stderr_log(&self, name: &str) -> PathBuf {
        self.current_dir.join(format!("{name}.stderr.log"))
    }

    fn last_withdrawal_target_path(&self) -> PathBuf {
        self.current_dir.join("last-withdrawal.json")
    }

    fn bridge_binary(&self) -> PathBuf {
        self.bin_dir.join("bridge")
    }

    fn node_binary(&self) -> PathBuf {
        self.bin_dir.join("nockchain-bridge-sequencer")
    }

    fn ensure_runtime_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.current_dir)
            .with_context(|| format!("failed to create {}", self.current_dir.display()))?;
        Ok(())
    }

    fn remove_paths(&self, paths: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        for path in paths {
            if path.exists() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
            }
        }
        Ok(())
    }

    fn remove_local_state_preserving_sequencer(&self) -> Result<()> {
        self.remove_paths([
            self.wallet_dir(),
            self.bridge_data_dir(0),
            self.bridge_data_dir(1),
            self.bridge_data_dir(2),
            self.bridge_data_dir(3),
            self.bridge_data_dir(4),
            self.bridge_config_dir(),
            self.current_dir.clone(),
        ])
    }

    fn remove_all_local_state(&self) -> Result<()> {
        self.remove_paths([
            self.node_dir(),
            self.wallet_dir(),
            self.bridge_data_dir(0),
            self.bridge_data_dir(1),
            self.bridge_data_dir(2),
            self.bridge_data_dir(3),
            self.bridge_data_dir(4),
            self.bridge_config_dir(),
            self.current_dir.clone(),
        ])
    }
}

fn resolve_test_data_dir(bridge_dir: &Path, override_root: Option<&Path>) -> PathBuf {
    override_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| bridge_dir.join("test_run_data"))
}

fn resolve_generated_env_path(
    bridge_dir: &Path,
    test_data_dir: &Path,
    test_data_dir_overridden: bool,
) -> PathBuf {
    if test_data_dir_overridden {
        test_data_dir.join("virtual-testnet.generated.env")
    } else {
        bridge_dir.join("scripts/environments/virtual-testnet.generated.env")
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileFile {
    profile: ProfileMeta,
    vnet: VnetProfile,
    cluster: ClusterProfile,
    aliases: AliasProfileSet,
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileMeta {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct VnetProfile {
    name_prefix: String,
    cleanup_keep: usize,
    cleanup_old_before_fresh: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ClusterProfile {
    base_confirmation_depth: u64,
    nockchain_confirmation_depth: u64,
    default_deposit_amount_nicks: u64,
    default_deposit_fee_nicks: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AliasProfileSet {
    deposit_recipient: EvmAliasConfig,
    withdraw_holder: WithdrawHolderConfig,
    withdraw_dest: WithdrawDestConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct EvmAliasConfig {
    evm_address: String,
    #[serde(default)]
    source_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WithdrawHolderConfig {
    evm_address: String,
    #[serde(default)]
    source_env: Option<String>,
    private_key: String,
    #[serde(default)]
    private_key_env: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WithdrawDestConfig {
    #[serde(default)]
    nockchain_address: Option<String>,
    #[serde(default)]
    lock_root: Option<String>,
    #[serde(default)]
    lock_root_hex: Option<String>,
    #[serde(default)]
    lock_root_bytes32_hex: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedProfile {
    source_path: PathBuf,
    profile_name: String,
    cluster: ClusterProfile,
    aliases: ResolvedAliases,
}

#[derive(Debug, Clone)]
struct ResolvedAliases {
    deposit_recipient: ResolvedEvmAlias,
    withdraw_holder: ResolvedWithdrawHolder,
    withdraw_dest: ResolvedWithdrawDest,
}

#[derive(Debug, Clone)]
struct ResolvedEvmAlias {
    address: String,
}

#[derive(Debug, Clone)]
struct ResolvedWithdrawHolder {
    address: String,
    private_key: String,
}

#[derive(Debug, Clone)]
struct ResolvedWithdrawDest {
    nockchain_address: Option<String>,
    lock_root_base58: String,
    lock_root_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    profile_name: String,
    profile_path: PathBuf,
    created_at_unix: u64,
    control_socket: PathBuf,
    env_file: PathBuf,
    vnet: VnetManifest,
    aliases: AliasManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VnetManifest {
    vnet_id: Option<String>,
    base_rpc_url: String,
    base_ws_url: String,
    base_start_height: u64,
    inbox_contract_address: String,
    nock_contract_address: String,
    tenderly_public_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AliasManifest {
    deposit_recipient: String,
    withdraw_holder: String,
    withdraw_dest_nockchain_address: Option<String>,
    withdraw_dest_lock_root_base58: String,
    withdraw_dest_lock_root_hex: String,
}

impl AliasManifest {
    fn withdraw_dest_lock_root(&self) -> Result<NockHash> {
        NockHash::from_base58(&self.withdraw_dest_lock_root_base58).with_context(|| {
            format!(
                "invalid withdraw destination lock root {}",
                self.withdraw_dest_lock_root_base58
            )
        })
    }

    #[cfg(test)]
    fn withdraw_dest_first_name(&self) -> Result<String> {
        let lock_root = self.withdraw_dest_lock_root()?;
        let first_name = FirstName::from_lock_root(&lock_root).map_err(|err| {
            anyhow!(
                "failed to derive first-name from withdraw destination lock root {}: {err}",
                self.withdraw_dest_lock_root_base58
            )
        })?;
        Ok(first_name.to_base58())
    }

    fn withdraw_dest_balance_target(&self) -> Result<NockchainWalletBalanceTarget> {
        Ok(NockchainWalletBalanceTarget::new(
            "withdraw dest",
            self.withdraw_dest_lock_root()?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WalletBalanceStatus {
    first_name: String,
    nicks: u128,
    note_count: usize,
    height: Option<u64>,
}

impl WalletBalanceStatus {
    fn summary(&self) -> String {
        let height = self
            .height
            .map(|height| height.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "{} NOCK ({} nicks, notes={}, height={}, first_name={})",
            format_nock_from_nicks(self.nicks),
            self.nicks,
            self.note_count,
            height,
            self.first_name
        )
    }
}

#[derive(Debug, Clone)]
struct NockchainWalletBalanceTarget {
    label: &'static str,
    lock_root: NockHash,
}

impl NockchainWalletBalanceTarget {
    fn new(label: &'static str, lock_root: NockHash) -> Self {
        Self { label, lock_root }
    }

    fn lock_root_base58(&self) -> String {
        self.lock_root.to_base58()
    }

    fn first_name(&self) -> Result<String> {
        let first_name = FirstName::from_lock_root(&self.lock_root).map_err(|err| {
            anyhow!(
                "failed to derive first-name from {} lock root {}: {err}",
                self.label,
                self.lock_root_base58()
            )
        })?;
        Ok(first_name.to_base58())
    }

    async fn fetch_balance(&self) -> Result<WalletBalanceStatus> {
        let first_name = self.first_name()?;
        let endpoint = public_nockchain_grpc_endpoint()?;
        let mut client = PublicNockchainGrpcClient::connect(endpoint.clone())
            .await
            .map_err(|err| {
                anyhow!("failed to connect public Nockchain gRPC at {endpoint}: {err}")
            })?;
        let balance = client
            .wallet_get_balance(&BalanceRequest::FirstName(first_name.clone()))
            .await
            .map_err(|err| {
                anyhow!("failed to query {} balance from {endpoint}: {err}", self.label)
            })?;
        let nicks = balance_total_nicks(&balance)?;
        let height = balance.height.as_ref().map(|height| height.value);
        Ok(WalletBalanceStatus {
            first_name,
            nicks,
            note_count: balance.notes.len(),
            height,
        })
    }

    async fn print_balance(&self) {
        match tokio::time::timeout(STATUS_BALANCE_TIMEOUT, self.fetch_balance()).await {
            Ok(Ok(balance)) => println!("{} wallet balance: {}", self.label, balance.summary()),
            Ok(Err(err)) => println!("{} wallet balance: unavailable: {err:#}", self.label),
            Err(_) => println!(
                "{} wallet balance: unavailable: timed out after {}s",
                self.label,
                STATUS_BALANCE_TIMEOUT.as_secs()
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BaseNockBalanceStatus {
    base_units: U256,
}

impl BaseNockBalanceStatus {
    fn nicks_and_remainder(&self) -> (U256, U256) {
        let base_units_per_nick = U256::from(NOCK_BASE_PER_NICK);
        (
            self.base_units / base_units_per_nick,
            self.base_units % base_units_per_nick,
        )
    }

    fn summary(&self) -> String {
        let (nicks, remainder) = self.nicks_and_remainder();
        let nock_amount = format_nock_amount_from_u256_nicks(nicks);
        if remainder == U256::ZERO {
            return format!(
                "{} ({} nicks, {} base units)",
                nock_amount, nicks, self.base_units
            );
        }

        format!(
            "{} + {} base units ({} nicks, {} base units total)",
            nock_amount, remainder, nicks, self.base_units
        )
    }
}

#[derive(Debug, Clone)]
struct BaseNockBalanceTarget {
    label: &'static str,
    rpc_url: String,
    nock_contract: Address,
    holder: Address,
}

impl BaseNockBalanceTarget {
    fn deposit_recipient(manifest: &Manifest) -> Result<Self> {
        let holder = Address::from_str(&manifest.aliases.deposit_recipient).with_context(|| {
            format!(
                "invalid deposit recipient address {}",
                manifest.aliases.deposit_recipient
            )
        })?;
        let nock_contract =
            Address::from_str(&manifest.vnet.nock_contract_address).with_context(|| {
                format!(
                    "invalid Nock contract address {}",
                    manifest.vnet.nock_contract_address
                )
            })?;

        Ok(Self {
            label: "deposit recipient",
            rpc_url: manifest.vnet.base_rpc_url.clone(),
            nock_contract,
            holder,
        })
    }

    async fn fetch_balance(&self) -> Result<BaseNockBalanceStatus> {
        let provider = ProviderBuilder::<_, _, Optimism>::default().connect_http(
            self.rpc_url
                .parse()
                .with_context(|| format!("invalid BASE_RPC_URL {}", self.rpc_url))?,
        );
        let nock_contract = DevNock::new(self.nock_contract, &provider);
        let base_units = nock_contract
            .balanceOf(self.holder)
            .call()
            .await
            .with_context(|| format!("failed to query {} Nock.balanceOf", self.label))?;
        Ok(BaseNockBalanceStatus { base_units })
    }

    async fn print_balance(&self) {
        match tokio::time::timeout(STATUS_BALANCE_TIMEOUT, self.fetch_balance()).await {
            Ok(Ok(balance)) => println!("{} balance: {}", self.label, balance.summary()),
            Ok(Err(err)) => println!("{} balance: unavailable: {err:#}", self.label),
            Err(_) => println!(
                "{} balance: unavailable: timed out after {}s",
                self.label,
                STATUS_BALANCE_TIMEOUT.as_secs()
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct GeneratedEnv {
    values: BTreeMap<String, String>,
}

impl GeneratedEnv {
    fn require(&self, key: &str) -> Result<String> {
        self.values
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow!("missing {key} in generated env"))
    }

    fn optional(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlRequest {
    Status,
    Down,
    Stop { target: String },
    Start { target: String },
    Restart { target: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlResponse {
    Status { components: Vec<ComponentStatus> },
    DownAck,
    ComponentAck { component: ComponentStatus },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComponentStatus {
    name: String,
    pid: Option<u32>,
    state: String,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

struct ManagedChild {
    name: String,
    child: Child,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl ManagedChild {
    fn status(&mut self) -> Result<ComponentStatus> {
        let pid = self.child.id();
        let state = match self.child.try_wait()? {
            Some(status) => match status.code() {
                Some(code) => format!("exited({code})"),
                None => "terminated".to_string(),
            },
            None => "running".to_string(),
        };
        Ok(ComponentStatus {
            name: self.name.clone(),
            pid,
            state,
            stdout_log: self.stdout_log.clone(),
            stderr_log: self.stderr_log.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentTarget {
    Node,
    Bridge(usize),
}

impl ComponentTarget {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "node" => Ok(Self::Node),
            "bridge-0" => Ok(Self::Bridge(0)),
            "bridge-1" => Ok(Self::Bridge(1)),
            "bridge-2" => Ok(Self::Bridge(2)),
            "bridge-3" => Ok(Self::Bridge(3)),
            "bridge-4" => Ok(Self::Bridge(4)),
            other => bail!("unknown component target: {other}"),
        }
    }

    fn name(self) -> String {
        match self {
            Self::Node => "node".to_string(),
            Self::Bridge(node_id) => format!("bridge-{node_id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentAction {
    Stop,
    Start,
    Restart,
}

impl ComponentAction {
    fn label(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitDepositPhase {
    Submitted,
    Successful,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitWithdrawalPhase {
    Pending,
    Ready,
    Submitted,
    Executed,
}

#[derive(Debug, Clone)]
struct StableSnapshot {
    running_state: tui_proto::RunningState,
    nock_hold: bool,
    base_hold: bool,
    nock_hold_height: Option<u64>,
    base_hold_height: Option<u64>,
    base_height: u64,
    nock_height: u64,
    pending_deposits: u64,
    pending_withdrawals: u64,
    unsettled_deposit_count: u64,
    unsettled_withdrawal_count: u64,
    batch_status: String,
    nockchain_api_state: tui_proto::nockchain_api_status::State,
    nockchain_api_last_error: Option<String>,
    degradation_warning: Option<String>,
    peer_statuses: Vec<StablePeerStatus>,
    last_submitted_deposit: Option<StableDeposit>,
    last_successful_deposit: Option<StableDeposit>,
    last_submitted_proposal: Option<StableProposal>,
    pending_inbound_proposals: Vec<StableProposal>,
}

#[derive(Debug, Clone)]
struct StablePeerStatus {
    node_id: u64,
    status: tui_proto::PeerHealthStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StableDeposit {
    tx_id: Option<String>,
    nonce: u64,
    amount: u64,
    recipient: Option<String>,
    base_block_number: Option<u64>,
}

#[derive(Debug, Clone)]
struct StableProposal {
    id: String,
    proposal_type: String,
    status: tui_proto::ProposalStatus,
    signatures_collected: u32,
    signatures_required: u32,
    nonce: Option<u64>,
    source_tx_id: Option<String>,
    tx_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct TrackedWithdrawal {
    id: ingress_proto::WithdrawalId,
    nonce: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredWithdrawalTarget {
    as_of: Vec<u8>,
    base_event_id: Vec<u8>,
    nonce: u64,
}

#[derive(Debug, Clone)]
struct WithdrawalProgress {
    id_label: String,
    nonce: Option<u64>,
    current_epoch: Option<u64>,
    handoff_index: Option<u64>,
    handoff_owner: Option<String>,
    turn_started_base_height: Option<u64>,
    proposal_status: Option<String>,
    sequenced_state: Option<String>,
    transaction_name: Option<String>,
    proposal_hash: Option<String>,
    authorized_transaction_name: Option<String>,
}

#[derive(Debug, Clone)]
struct SequencerStatusView {
    reserved_inputs: usize,
    pending: Option<WithdrawalProgress>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;
    let profile_path = cli
        .profile_path
        .unwrap_or_else(|| paths.default_profile_path());
    let profile_name = cli.profile;

    match cli.command {
        Commands::Up(args) => run_up(paths, profile_path, profile_name, args).await,
        Commands::Down => run_down(paths).await,
        Commands::Status(args) => run_status(paths, args).await,
        Commands::Watch(args) => run_watch(paths, args).await,
        Commands::Wait(args) => run_wait(paths, args).await,
        Commands::Info => run_info(paths).await,
        Commands::Logs(args) => run_logs(paths, args),
        Commands::Stop(args) => run_component_action(paths, args, ComponentAction::Stop).await,
        Commands::Start(args) => run_component_action(paths, args, ComponentAction::Start).await,
        Commands::Restart(args) => {
            run_component_action(paths, args, ComponentAction::Restart).await
        }
        Commands::Deposit(args) => run_deposit(paths, profile_path, profile_name, args).await,
        Commands::MintForBurn(args) => {
            run_mint_for_burn(paths, profile_path, profile_name, args).await
        }
        Commands::RequestWithdrawal(args) => {
            run_request_withdrawal(paths, profile_path, profile_name, args).await
        }
        Commands::AdvanceBase(args) => run_advance_base(paths, args).await,
    }
}

async fn run_up(
    paths: Paths,
    profile_path: PathBuf,
    profile_name: String,
    args: UpArgs,
) -> Result<()> {
    ensure_not_running(&paths).await?;

    let fresh_state = args.fresh_state();
    let fresh_vnet = args.fresh_vnet();
    let needs_fresh_state = fresh_state || !paths.node_dir().exists();
    if args.fresh {
        paths.remove_all_local_state()?;
    } else if args.fresh_state {
        paths.remove_local_state_preserving_sequencer()?;
    }
    paths.ensure_runtime_dirs()?;
    remove_stale_socket(&paths.control_socket)?;

    if fresh_vnet || !paths.env_file.exists() {
        let raw_profile = load_profile_file(&profile_path, &profile_name)?;
        provision_vnet(&paths, &raw_profile).await?;
    }

    let env = load_generated_env(&paths.env_file)?;
    let profile = resolve_profile(
        load_profile_file(&profile_path, &profile_name)?,
        &profile_path,
        env.clone(),
    )?;
    run_preflight_checks(&profile, &env)?;
    let manifest = build_manifest(&paths, &profile, &env)?;
    write_manifest(&paths.manifest_path, &manifest)?;

    ensure_binaries(&paths)?;
    append_supervisor_log(&paths.supervisor_log, "starting bridge-dev supervisor")?;
    let listener = UnixListener::bind(&paths.control_socket)
        .with_context(|| format!("failed to bind {}", paths.control_socket.display()))?;

    let bridge_configs = write_bridge_configs(&paths, &profile, &manifest.vnet)?;
    let mut children = Vec::new();
    children.push(
        spawn_node(
            &paths, &manifest, &bridge_configs.bridge_paths[0], &bridge_configs.sequencer_path,
            needs_fresh_state,
        )
        .await?,
    );
    wait_for_port(SocketTarget::PrivateNodeGrpc, Duration::from_secs(20)).await?;
    for (node_id, config_path) in bridge_configs.bridge_paths.iter().enumerate() {
        children.push(spawn_bridge(
            &paths, node_id, config_path, needs_fresh_state, args.start,
        )?);
    }

    println!("bridge-dev is running");
    println!("run dir: {}", paths.current_dir.display());
    println!(
        "vnet: {}",
        manifest.vnet.vnet_id.as_deref().unwrap_or("unknown")
    );
    println!("base rpc: {}", manifest.vnet.base_rpc_url);
    println!("inbox: {}", manifest.vnet.inbox_contract_address);
    println!("nock: {}", manifest.vnet.nock_contract_address);
    println!("deposit recipient: {}", manifest.aliases.deposit_recipient);
    println!("withdraw holder: {}", manifest.aliases.withdraw_holder);
    println!(
        "withdraw dest: {}",
        manifest
            .aliases
            .withdraw_dest_nockchain_address
            .as_deref()
            .unwrap_or(manifest.aliases.withdraw_dest_lock_root_base58.as_str())
    );
    println!("control socket: {}", paths.control_socket.display());
    println!("press Ctrl+C or run `bridge-dev down` from another terminal");

    let shutdown_requested = supervisor_loop(&paths, &manifest, listener, &mut children).await?;
    append_supervisor_log(
        &paths.supervisor_log,
        if shutdown_requested {
            "shutting down on control request"
        } else {
            "shutting down on ctrl-c"
        },
    )?;
    shutdown_children(&mut children).await?;
    let _ = fs::remove_file(&paths.control_socket);
    Ok(())
}

async fn run_down(paths: Paths) -> Result<()> {
    let response = send_control_request(&paths.control_socket, ControlRequest::Down).await?;
    match response {
        ControlResponse::DownAck => {
            println!("bridge-dev shutdown requested");
            Ok(())
        }
        ControlResponse::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {:?}", other),
    }
}

async fn run_status(paths: Paths, args: StatusArgs) -> Result<()> {
    let manifest = read_manifest(&paths.manifest_path)?;
    println!("profile: {}", manifest.profile_name);
    println!(
        "vnet: {}",
        manifest.vnet.vnet_id.as_deref().unwrap_or("unknown")
    );
    println!("base rpc: {}", manifest.vnet.base_rpc_url);
    println!("inbox: {}", manifest.vnet.inbox_contract_address);
    println!("nock: {}", manifest.vnet.nock_contract_address);
    print_balance_status(&manifest).await;

    print_process_status(&paths).await;

    let mut status_failed = false;

    match fetch_stable_snapshot().await {
        Ok(snapshot) => {
            print_snapshot_summary(&snapshot);
        }
        Err(err) => {
            println!("tui: unavailable: {err}");
            status_failed = true;
        }
    }

    if args.bridges {
        if let Err(err) = print_bridge_stream_statuses().await {
            println!("bridge_streams: unavailable: {err}");
            status_failed = true;
        }
    }

    if args.sequencer {
        if let Err(err) = print_sequencer_status().await {
            println!("sequencer: unavailable: {err}");
            status_failed = true;
        }
    }

    if status_failed {
        bail!("failed to query one or more status endpoints");
    }
    Ok(())
}

async fn print_balance_status(manifest: &Manifest) {
    println!("deposit recipient: {}", manifest.aliases.deposit_recipient);
    match BaseNockBalanceTarget::deposit_recipient(manifest) {
        Ok(target) => target.print_balance().await,
        Err(err) => println!("deposit recipient balance: unavailable: {err:#}"),
    }

    match bridge_multisig_balance_target() {
        Ok(target) => {
            println!("bridge multisig lock root: {}", target.lock_root_base58());
            target.print_balance().await;
        }
        Err(err) => println!("bridge multisig wallet balance: unavailable: {err:#}"),
    }

    if let Some(addr) = manifest.aliases.withdraw_dest_nockchain_address.as_deref() {
        println!("withdraw dest address: {addr}");
    }
    println!(
        "withdraw dest lock root: {} ({})",
        manifest.aliases.withdraw_dest_lock_root_base58,
        manifest.aliases.withdraw_dest_lock_root_hex
    );

    match manifest.aliases.withdraw_dest_balance_target() {
        Ok(target) => target.print_balance().await,
        Err(err) => println!("withdraw dest wallet balance: unavailable: {err:#}"),
    }
}

fn bridge_multisig_balance_target() -> Result<NockchainWalletBalanceTarget> {
    let lock_root_base58 = derive_bridge_dev_lock_root()?;
    let lock_root = NockHash::from_base58(&lock_root_base58)
        .with_context(|| format!("invalid bridge multisig lock root {lock_root_base58}"))?;
    Ok(NockchainWalletBalanceTarget::new(
        "bridge multisig", lock_root,
    ))
}

fn public_nockchain_grpc_endpoint() -> Result<String> {
    Ok(format!("http://{}", node_public_grpc_addr()?))
}

fn format_nock_amount_from_u256_nicks(nicks: U256) -> String {
    if nicks <= U256::from(u128::MAX) {
        return format!("{} NOCK", format_nock_from_nicks(nicks.to::<u128>()));
    }

    format!("{nicks} nicks")
}

fn balance_total_nicks(balance: &nockapp_grpc::pb::common::v2::Balance) -> Result<u128> {
    balance.notes.iter().try_fold(0u128, |total, entry| {
        let note = entry.note.as_ref().context("balance entry missing note")?;
        let note_version = note
            .note_version
            .as_ref()
            .context("balance entry missing note version")?;
        let nicks = match note_version {
            note::NoteVersion::Legacy(note) => {
                note.assets
                    .as_ref()
                    .context("legacy balance note missing assets")?
                    .value
            }
            note::NoteVersion::V1(note) => {
                note.assets
                    .as_ref()
                    .context("v1 balance note missing assets")?
                    .value
            }
        };
        total
            .checked_add(u128::from(nicks))
            .ok_or_else(|| anyhow!("balance nicks overflow"))
    })
}

async fn run_watch(paths: Paths, args: WatchArgs) -> Result<()> {
    let manifest = read_manifest(&paths.manifest_path)?;
    let mut ticker = tokio::time::interval(Duration::from_millis(args.interval_ms.max(100)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = ticker.tick() => {
                print!("\x1b[2J\x1b[H");
                println!("profile: {}", manifest.profile_name);
                println!("vnet: {}", manifest.vnet.vnet_id.as_deref().unwrap_or("unknown"));
                println!("base rpc: {}", manifest.vnet.base_rpc_url);
                print_process_status(&paths).await;
                match fetch_stable_snapshot().await {
                    Ok(snapshot) => print_snapshot_summary(&snapshot),
                    Err(err) => println!("tui: unavailable: {err}"),
                }
                std::io::stdout().flush().context("failed to flush watch output")?;
            }
        }
    }
}

async fn run_wait(paths: Paths, args: WaitArgs) -> Result<()> {
    match args.command {
        WaitCommand::Deposit(args) => run_wait_deposit(paths, args).await,
        WaitCommand::Withdrawal(args) => run_wait_withdrawal(paths, args).await,
    }
}

async fn run_component_action(
    paths: Paths,
    args: ComponentTargetArgs,
    action: ComponentAction,
) -> Result<()> {
    let (requests, mut failures) = component_action_requests(&args.targets, action);
    for (target, request) in requests {
        match send_control_request(&paths.control_socket, request).await {
            Ok(ControlResponse::ComponentAck { component }) => {
                println!(
                    "{} {}: {} pid={}",
                    action.label(),
                    component.name,
                    component.state,
                    component
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
            }
            Ok(ControlResponse::Error { message }) => {
                failures.push(format!("{target}: {message}"));
            }
            Ok(other) => {
                failures.push(format!("{target}: unexpected response: {other:?}"));
            }
            Err(err) => {
                failures.push(format!("{target}: {err:#}"));
            }
        }
    }

    if failures.is_empty() {
        return Ok(());
    }

    for failure in &failures {
        eprintln!("{} failed: {failure}", action.label());
    }
    bail!("{} failed for {} target(s)", action.label(), failures.len())
}

fn component_action_requests(
    raw_targets: &[String],
    action: ComponentAction,
) -> (Vec<(String, ControlRequest)>, Vec<String>) {
    let mut requests = Vec::new();
    let mut failures = Vec::new();
    for raw in raw_targets {
        match ComponentTarget::parse(raw) {
            Ok(target) => {
                requests.push((target.name(), component_action_request(action, target)));
            }
            Err(err) => {
                failures.push(format!("{raw}: {err}"));
            }
        }
    }
    (requests, failures)
}

fn component_action_request(action: ComponentAction, target: ComponentTarget) -> ControlRequest {
    match action {
        ComponentAction::Stop => ControlRequest::Stop {
            target: target.name(),
        },
        ComponentAction::Start => ControlRequest::Start {
            target: target.name(),
        },
        ComponentAction::Restart => ControlRequest::Restart {
            target: target.name(),
        },
    }
}

async fn run_info(paths: Paths) -> Result<()> {
    let manifest = read_manifest(&paths.manifest_path)?;
    println!("profile: {}", manifest.profile_name);
    println!("profile file: {}", manifest.profile_path.display());
    println!("run dir: {}", paths.current_dir.display());
    println!("manifest: {}", paths.manifest_path.display());
    println!("env file: {}", manifest.env_file.display());
    println!("control socket: {}", manifest.control_socket.display());
    println!("deposit recipient: {}", manifest.aliases.deposit_recipient);
    println!("withdraw holder: {}", manifest.aliases.withdraw_holder);
    if let Some(addr) = manifest.aliases.withdraw_dest_nockchain_address.as_deref() {
        println!("withdraw dest address: {addr}");
    }
    println!(
        "withdraw dest lock root: {} ({})",
        manifest.aliases.withdraw_dest_lock_root_base58,
        manifest.aliases.withdraw_dest_lock_root_hex
    );
    println!("supervisor log: {}", paths.supervisor_log.display());
    for name in ["node", "bridge-0", "bridge-1", "bridge-2", "bridge-3", "bridge-4"] {
        println!(
            "{name} logs: stdout={} stderr={}",
            paths.stdout_log(name).display(),
            paths.stderr_log(name).display()
        );
    }
    for (node_id, pkh) in BRIDGE_NOCK_PKHS.iter().enumerate() {
        println!("bridge-{node_id} nock pkh: {pkh}");
    }
    Ok(())
}

async fn print_process_status(paths: &Paths) {
    match send_control_request(&paths.control_socket, ControlRequest::Status).await {
        Ok(ControlResponse::Status { components }) => {
            println!("processes:");
            for component in components {
                println!(
                    "  {:<10} {:<12} pid={}",
                    component.name,
                    component.state,
                    component
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
            }
        }
        Ok(ControlResponse::Error { message }) => {
            println!("processes: error: {message}");
        }
        Ok(other) => {
            println!("processes: unexpected supervisor response: {:?}", other);
        }
        Err(err) => {
            println!("processes: supervisor unavailable: {err}");
        }
    }
}

fn print_snapshot_summary(snapshot: &StableSnapshot) {
    println!(
        "bridge-0: running_state={:?} base_height={} nock_height={}",
        snapshot.running_state, snapshot.base_height, snapshot.nock_height
    );
    println!(
        "holds: base={} {:?} nock={} {:?}",
        snapshot.base_hold,
        snapshot.base_hold_height,
        snapshot.nock_hold,
        snapshot.nock_hold_height
    );
    println!(
        "queue: pending_deposits={} pending_withdrawals={} unsettled_deposits={} unsettled_withdrawals={}",
        snapshot.pending_deposits,
        snapshot.pending_withdrawals,
        snapshot.unsettled_deposit_count,
        snapshot.unsettled_withdrawal_count
    );
    println!(
        "nockchain_api: {:?} batch_status={}",
        snapshot.nockchain_api_state, snapshot.batch_status
    );
    println!(
        "peer_health: unhealthy={} total={}",
        snapshot.unhealthy_peer_count(),
        snapshot.peer_statuses.len()
    );
    let unhealthy_nodes = snapshot
        .peer_statuses
        .iter()
        .filter(|peer| peer.status != tui_proto::PeerHealthStatus::Healthy)
        .map(|peer| peer.node_id.to_string())
        .collect::<Vec<_>>();
    if !unhealthy_nodes.is_empty() {
        println!("unhealthy_peers: {}", unhealthy_nodes.join(","));
    }
    if let Some(warning) = &snapshot.degradation_warning {
        println!("degradation_warning: {warning}");
    }
    if let Some(error) = &snapshot.nockchain_api_last_error {
        println!("nockchain_api_last_error: {error}");
    }
    if let Some(deposit) = &snapshot.last_submitted_deposit {
        println!(
            "last_submitted_deposit: nonce={} amount={} recipient={}",
            deposit.nonce,
            deposit.amount,
            deposit.recipient.as_deref().unwrap_or("<unknown>")
        );
    }
    if let Some(deposit) = &snapshot.last_successful_deposit {
        println!(
            "last_successful_deposit: nonce={} amount={} recipient={}",
            deposit.nonce,
            deposit.amount,
            deposit.recipient.as_deref().unwrap_or("<unknown>")
        );
    }
    if let Some(proposal) = &snapshot.last_submitted_proposal {
        println!(
            "last_submitted_proposal: id={} type={} status={:?} signatures={}/{}",
            proposal.id,
            proposal.proposal_type,
            proposal.status,
            proposal.signatures_collected,
            proposal.signatures_required
        );
        if let Some(tx_hash) = &proposal.tx_hash {
            println!("last_submitted_proposal_tx_hash: {tx_hash}");
        }
        if let Some(nonce) = proposal.nonce {
            println!("last_submitted_proposal_nonce: {nonce}");
        }
        if let Some(source_tx_id) = &proposal.source_tx_id {
            println!("last_submitted_proposal_source_tx_id: {source_tx_id}");
        }
    }
    if !snapshot.pending_inbound_proposals.is_empty() {
        println!(
            "pending_inbound_proposals: {}",
            snapshot.pending_inbound_proposals.len()
        );
    }
}

fn print_bridge_stream_summary(node_id: usize, snapshot: &StableSnapshot) {
    println!(
        "  bridge-{node_id} running_state={:?} base_height={} nock_height={} nockchain_api={:?} batch_status={} unhealthy_peers={}",
        snapshot.running_state,
        snapshot.base_height,
        snapshot.nock_height,
        snapshot.nockchain_api_state,
        snapshot.batch_status,
        snapshot.unhealthy_peer_count()
    );
    if snapshot.base_hold || snapshot.nock_hold {
        println!(
            "    holds: base={} {:?} nock={} {:?}",
            snapshot.base_hold,
            snapshot.base_hold_height,
            snapshot.nock_hold,
            snapshot.nock_hold_height
        );
    }
    if let Some(error) = &snapshot.nockchain_api_last_error {
        println!("    nockchain_api_last_error: {error}");
    }
}

async fn print_bridge_stream_statuses() -> Result<()> {
    println!("bridge_streams:");
    let mut failures = Vec::new();
    for node_id in 0..BRIDGE_INGRESS_PORTS.len() {
        match fetch_stable_snapshot_for_node(node_id).await {
            Ok(snapshot) => print_bridge_stream_summary(node_id, &snapshot),
            Err(err) => {
                println!("  bridge-{node_id} unavailable: {err}");
                failures.push(node_id);
            }
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    bail!("failed to query {} bridge node snapshot(s)", failures.len())
}

fn print_sequencer_status_summary(status: &SequencerStatusView) {
    println!("sequencer_status:");
    println!("  reserved_inputs={}", status.reserved_inputs);
    let Some(pending) = &status.pending else {
        println!("  next_pending=none");
        return;
    };
    println!(
        "  next_pending: id={} nonce={} state={} proposal_status={} epoch={} handoff_index={} handoff_owner={}",
        pending.id_label,
        pending
            .nonce
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pending.sequenced_state.as_deref().unwrap_or("-"),
        pending.proposal_status.as_deref().unwrap_or("-"),
        pending
            .current_epoch
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pending
            .handoff_index
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pending.handoff_owner.as_deref().unwrap_or("-")
    );
    if let Some(height) = pending.turn_started_base_height {
        println!("  turn_started_base_height={height}");
    }
    if let Some(name) = &pending.transaction_name {
        println!("  transaction_name={name}");
    }
    if let Some(name) = &pending.authorized_transaction_name {
        println!("  authorized_transaction_name={name}");
    }
    if let Some(hash) = &pending.proposal_hash {
        println!("  proposal_hash={hash}");
    }
}

async fn print_sequencer_status() -> Result<()> {
    let status = fetch_sequencer_status().await?;
    print_sequencer_status_summary(&status);
    Ok(())
}

async fn run_wait_deposit(_paths: Paths, args: WaitDepositArgs) -> Result<()> {
    let phase = if args.submitted {
        WaitDepositPhase::Submitted
    } else {
        WaitDepositPhase::Successful
    };
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);

    loop {
        let snapshot = fetch_stable_snapshot_for_node(args.node_id).await?;
        ensure_wait_conditions(&snapshot)?;

        let matched = matching_deposit_for_phase_after_nonce(&snapshot, phase, args.after_nonce);
        if let Some(deposit) = matched {
            println!(
                "deposit {}: nonce={} amount={} recipient={} tx_id={}",
                match phase {
                    WaitDepositPhase::Submitted => "submitted",
                    WaitDepositPhase::Successful => "successful",
                },
                deposit.nonce,
                deposit.amount,
                deposit.recipient.as_deref().unwrap_or("<unknown>"),
                deposit.tx_id.as_deref().unwrap_or("<unknown>")
            );
            return Ok(());
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for deposit {:?}", phase);
        }
        sleep(Duration::from_secs(1)).await;
    }
}

fn matching_deposit_for_phase(
    snapshot: &StableSnapshot,
    phase: WaitDepositPhase,
) -> Option<StableDeposit> {
    match phase {
        WaitDepositPhase::Submitted => snapshot
            .last_submitted_deposit
            .clone()
            .or_else(|| snapshot.last_successful_deposit.clone()),
        WaitDepositPhase::Successful => snapshot.last_successful_deposit.clone(),
    }
}

fn matching_deposit_for_phase_after_nonce(
    snapshot: &StableSnapshot,
    phase: WaitDepositPhase,
    after_nonce: Option<u64>,
) -> Option<StableDeposit> {
    matching_deposit_for_phase(snapshot, phase)
        .filter(|deposit| after_nonce.is_none_or(|nonce| deposit.nonce > nonce))
}

async fn run_wait_withdrawal(paths: Paths, args: WaitWithdrawalArgs) -> Result<()> {
    let phase = wait_withdrawal_phase(&args);
    let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let mut tracked: Option<TrackedWithdrawal> = match withdrawal_target_from_wait_args(&args)? {
        Some(target) => Some(target),
        None => read_last_withdrawal_target(&paths)?,
    };
    let mut fixed_target = tracked.is_some();

    loop {
        let snapshot = fetch_stable_snapshot().await?;
        ensure_withdrawal_wait_conditions(&snapshot)?;

        if tracked.is_none() {
            tracked = fetch_next_pending_withdrawal().await?;
            if let Some(target) = &tracked {
                write_last_withdrawal_target(&paths, target)?;
                fixed_target = true;
            }
        }

        if let Some(target) = &tracked {
            match fetch_withdrawal_progress(target).await? {
                Some(progress) if withdrawal_phase_satisfied(phase, &progress) => {
                    let as_of_hex = hex_string(&target.id.as_of);
                    let base_event_hex = hex_string(&target.id.base_event_id);
                    println!(
                        "withdrawal {:?}: id={} as_of={} base_event={} nonce={} proposal_status={} sequenced_state={} handoff_owner={} transaction_name={} proposal_hash={} authorized_transaction_name={}",
                        phase,
                        withdrawal_id_compact_label(&target.id),
                        as_of_hex,
                        base_event_hex,
                        progress
                            .nonce
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        progress.proposal_status.as_deref().unwrap_or("-"),
                        progress.sequenced_state.as_deref().unwrap_or("-"),
                        progress.handoff_owner.as_deref().unwrap_or("-"),
                        progress.transaction_name.as_deref().unwrap_or("-"),
                        progress.proposal_hash.as_deref().unwrap_or("-"),
                        progress
                            .authorized_transaction_name
                            .as_deref()
                            .unwrap_or("-"),
                    );
                    return Ok(());
                }
                Some(_) => {}
                None => {
                    if fixed_target {
                        bail!(
                            "target withdrawal disappeared from sequencer: id={} nonce={}",
                            withdrawal_id_compact_label(&target.id),
                            target.nonce
                        );
                    }
                    tracked = None;
                    remove_last_withdrawal_target(&paths)?;
                }
            }
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for withdrawal {:?}", phase);
        }
        sleep(Duration::from_secs(1)).await;
    }
}

fn run_logs(paths: Paths, args: LogsArgs) -> Result<()> {
    let log_paths = match args.target.as_str() {
        "supervisor" => vec![paths.supervisor_log],
        "node" => vec![paths.stdout_log("node"), paths.stderr_log("node")],
        "bridge-0" | "bridge-1" | "bridge-2" | "bridge-3" | "bridge-4" => {
            vec![paths.stdout_log(&args.target), paths.stderr_log(&args.target)]
        }
        other => bail!("unknown log target: {other}"),
    };

    for log_path in &log_paths {
        if !log_path.exists() {
            bail!("log file does not exist: {}", log_path.display());
        }
    }

    let mut command = StdCommand::new("tail");
    command.arg("-n").arg("200");
    if args.follow {
        command.arg("-f");
    }
    if log_paths.len() > 1 {
        command.arg("-v");
    }
    command.args(&log_paths);
    let status = command.status().context("failed to launch tail")?;
    if !status.success() {
        bail!("tail exited with status {}", status);
    }
    Ok(())
}

async fn run_deposit(
    paths: Paths,
    profile_path: PathBuf,
    profile_name: String,
    args: DepositArgs,
) -> Result<()> {
    let env = load_generated_env(&paths.env_file)?;
    let profile = resolve_profile(
        load_profile_file(&profile_path, &profile_name)?,
        &profile_path,
        env,
    )?;
    let target = args.to.as_deref().unwrap_or("deposit_recipient");
    if target != "deposit_recipient" {
        bail!("unsupported deposit alias: {target}");
    }
    let amount = args
        .amount_nicks
        .unwrap_or(profile.cluster.default_deposit_amount_nicks);
    let active_bridge_config = BridgeConfigToml::from_file(paths.bridge_config_path(0))
        .context("failed to read bridge-0 config for active bridge lock root")?;
    let minimum_deposit_nicks = minimum_event_nicks(&active_bridge_config)?;

    let status = TokioCommand::new(&paths.deposit_script)
        .current_dir(paths.bridge_dir.join("scripts"))
        .env(
            "BRIDGE_DEPOSIT_RECIPIENT", &profile.aliases.deposit_recipient.address,
        )
        .env("BRIDGE_DEPOSIT_AMOUNT", amount.to_string())
        .env(
            "BRIDGE_DEPOSIT_FEE",
            profile.cluster.default_deposit_fee_nicks.to_string(),
        )
        .env("BRIDGE_MIN_DEPOSIT", minimum_deposit_nicks.to_string())
        .env(
            "BRIDGE_DEPOSIT_LOCK_ROOT", &active_bridge_config.bridge_lock_root,
        )
        .env("TEST_DATA_DIR", paths.test_data_dir.display().to_string())
        .env(
            "NODE_PRIVATE_GRPC_PORT",
            node_private_grpc_port()?.to_string(),
        )
        .env(
            "NODE_PUBLIC_GRPC_SERVER_ADDR",
            format!("http://{}", node_public_grpc_addr()?),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("failed to launch create-bridge-spend.sh")?;
    if !status.success() {
        bail!("deposit command failed with status {}", status);
    }
    Ok(())
}

fn minimum_event_nocks(config: &BridgeConfigToml) -> u64 {
    config
        .constants
        .clone()
        .unwrap_or_default()
        .minimum_event_nocks
}

fn minimum_event_nicks(config: &BridgeConfigToml) -> Result<u64> {
    let minimum_event_nocks = minimum_event_nocks(config);
    minimum_event_nocks
        .checked_mul(NICKS_PER_NOCK)
        .context("bridge minimum_event_nocks overflowed when converted to nicks")
}

fn ensure_withdrawal_amount_meets_floor(
    amount_nicks: u64,
    active_bridge_config: &BridgeConfigToml,
) -> Result<()> {
    let minimum_nicks = minimum_event_nicks(active_bridge_config)?;
    if amount_nicks <= minimum_nicks {
        bail!(
            "withdrawal amount {} nicks must be greater than bridge minimum event size {} nicks (minimum_event_nocks={})",
            amount_nicks,
            minimum_nicks,
            minimum_event_nocks(active_bridge_config)
        );
    }
    Ok(())
}

async fn run_mint_for_burn(
    paths: Paths,
    profile_path: PathBuf,
    profile_name: String,
    args: MintForBurnArgs,
) -> Result<()> {
    let env = load_generated_env(&paths.env_file)?;
    let profile = resolve_profile(
        load_profile_file(&profile_path, &profile_name)?,
        &profile_path,
        env.clone(),
    )?;
    let recipient_alias = args.to.as_deref().unwrap_or("withdraw_holder");
    let recipient = resolve_mint_recipient(&profile, recipient_alias)?;
    let amount_nicks = parse_nock_amount_to_nicks(&args.amount_nock)?;
    let amount_units = nicks_to_nock_base_units(amount_nicks)?;

    let rpc_url = env
        .optional("TENDERLY_RPC_URL")
        .unwrap_or(env.require("BASE_RPC_URL")?);
    ensure_mint_for_burn_target(&env, &rpc_url)?;
    let inbox = env.require("INBOX_CONTRACT_ADDRESS")?;
    let nock = env.require("NOCK_CONTRACT_ADDRESS")?;
    let inbox_address =
        Address::from_str(&inbox).with_context(|| format!("invalid inbox address {inbox}"))?;
    let nock_address =
        Address::from_str(&nock).with_context(|| format!("invalid nock address {nock}"))?;
    let owner_key = resolve_mint_for_burn_owner_key(
        std::env::var("BRIDGE_DEV_OWNER_PRIVATE_KEY")
            .ok()
            .as_deref(),
        std::env::var("TENDERLY_TEST_PRIVATE_KEY").ok().as_deref(),
    )?;
    let signer = PrivateKeySigner::from_str(owner_key.trim_start_matches("0x"))
        .context("invalid mint-for-burn owner private key")?;
    let signer_address = signer.address();
    if let Some(expected_owner) = env.optional("TENDERLY_PUBLIC_ADDRESS") {
        let expected_owner = Address::from_str(&expected_owner)
            .with_context(|| format!("invalid TENDERLY_PUBLIC_ADDRESS {expected_owner}"))?;
        if signer_address != expected_owner {
            bail!(
                "mint-for-burn owner key resolves to {signer_address}, expected Tenderly public address {expected_owner}"
            );
        }
    }
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::<_, _, Optimism>::default()
        .filler(GasFiller)
        .filler(NonceFiller::<CachedNonceManager>::default())
        .filler(ChainIdFiller::default())
        .filler(WalletFiller::new(wallet))
        .connect_http(
            rpc_url
                .parse()
                .with_context(|| format!("invalid RPC URL {rpc_url}"))?,
        );
    let nock_contract = DevNock::new(nock_address, &provider);

    let current_inbox = nock_contract
        .inbox()
        .call()
        .await
        .context("failed to query Nock.inbox before mint-for-burn")?;
    if current_inbox != inbox_address {
        bail!(
            "refusing mint-for-burn because Nock.inbox is {current_inbox}, expected configured MessageInbox {inbox_address}"
        );
    }

    println!(
        "minting {} NOCK to {} ({recipient_alias})",
        args.amount_nock, recipient
    );
    println!("owner address: {signer_address}");

    let repoint = nock_contract
        .updateInbox(signer_address)
        .from(signer_address);
    let repoint_pending_tx = repoint
        .send()
        .await
        .context("failed to submit inbox repoint tx")?;
    let repoint_tx_hash = *repoint_pending_tx.tx_hash();
    let repoint_receipt = repoint_pending_tx
        .get_receipt()
        .await
        .context("failed to fetch inbox repoint receipt")?;
    let repoint_status_ok = repoint_receipt
        .inner
        .inner
        .receipt
        .as_receipt()
        .status
        .coerce_status();
    if !repoint_status_ok {
        bail!("mint-for-burn inbox repoint reverted on-chain: tx_hash={repoint_tx_hash:?}");
    }
    let repointed_inbox = nock_contract
        .inbox()
        .call()
        .await
        .context("failed to query Nock.inbox after repoint")?;
    if repointed_inbox != signer_address {
        bail!(
            "mint-for-burn repoint tx {repoint_tx_hash:?} completed but Nock.inbox is {repointed_inbox}, expected {signer_address}"
        );
    }

    println!("repoint tx hash: {repoint_tx_hash:?}");

    let mint_result: Result<_> = async {
        let mint = nock_contract
            .mint(recipient, U256::from(amount_units))
            .from(signer_address);
        let pending_tx = mint
            .send()
            .await
            .context("failed to submit direct Nock mint tx")?;
        let tx_hash = *pending_tx.tx_hash();
        let receipt = pending_tx
            .get_receipt()
            .await
            .context("failed to fetch direct Nock mint receipt")?;
        let status_ok = receipt
            .inner
            .inner
            .receipt
            .as_receipt()
            .status
            .coerce_status();
        if !status_ok {
            bail!("mint-for-burn direct mint reverted on-chain: tx_hash={tx_hash:?}");
        }
        Ok::<_, anyhow::Error>(tx_hash)
    }
    .await;

    let mint_tx_hash = match mint_result {
        Ok(tx_hash) => tx_hash,
        Err(err) => {
            let restore_result: Result<_> = async {
                let restore = nock_contract.updateInbox(inbox_address).from(signer_address);
                let restore_pending_tx = restore
                    .send()
                    .await
                    .context("failed to submit inbox restore tx")?;
                let restore_tx_hash = *restore_pending_tx.tx_hash();
                let restore_receipt = restore_pending_tx
                    .get_receipt()
                    .await
                    .context("failed to fetch inbox restore receipt")?;
                let restore_status_ok = restore_receipt
                    .inner
                    .inner
                    .receipt
                    .as_receipt()
                    .status
                    .coerce_status();
                if !restore_status_ok {
                    bail!(
                        "mint-for-burn inbox restore reverted on-chain: tx_hash={restore_tx_hash:?}"
                    );
                }
                let restored_inbox = nock_contract
                    .inbox()
                    .call()
                    .await
                    .context("failed to query Nock.inbox after restore")?;
                if restored_inbox != inbox_address {
                    bail!(
                        "mint-for-burn restore tx {restore_tx_hash:?} completed but Nock.inbox is {restored_inbox}, expected {inbox_address}"
                    );
                }
                Ok::<_, anyhow::Error>(restore_tx_hash)
            }
            .await;

            match restore_result {
                Ok(restore_tx_hash) => {
                    bail!("{err:#}. Restored Nock.inbox with tx {restore_tx_hash:?}");
                }
                Err(restore_err) => {
                    bail!(
                        "{err:#}. Failed to restore Nock.inbox after mint-for-burn error: {restore_err:#}"
                    );
                }
            }
        }
    };

    println!("mint tx hash: {mint_tx_hash:?}");

    let restore = nock_contract
        .updateInbox(inbox_address)
        .from(signer_address);
    let restore_pending_tx = restore
        .send()
        .await
        .context("failed to submit inbox restore tx")?;
    let restore_tx_hash = *restore_pending_tx.tx_hash();
    let restore_receipt = restore_pending_tx
        .get_receipt()
        .await
        .context("failed to fetch inbox restore receipt")?;
    let restore_status_ok = restore_receipt
        .inner
        .inner
        .receipt
        .as_receipt()
        .status
        .coerce_status();
    if !restore_status_ok {
        bail!("mint-for-burn inbox restore reverted on-chain: tx_hash={restore_tx_hash:?}");
    }
    let restored_inbox = nock_contract
        .inbox()
        .call()
        .await
        .context("failed to query Nock.inbox after restore")?;
    if restored_inbox != inbox_address {
        bail!(
            "mint-for-burn restore tx {restore_tx_hash:?} completed but Nock.inbox is {restored_inbox}, expected {inbox_address}"
        );
    }
    println!("restore tx hash: {restore_tx_hash:?}");
    println!(
        "mint-for-burn submitted. Next: `bridge-dev request-withdrawal --amount-nock {}`",
        args.amount_nock
    );
    Ok(())
}

async fn run_request_withdrawal(
    paths: Paths,
    profile_path: PathBuf,
    profile_name: String,
    args: RequestWithdrawalArgs,
) -> Result<()> {
    let env = load_generated_env(&paths.env_file)?;
    let profile = resolve_profile(
        load_profile_file(&profile_path, &profile_name)?,
        &profile_path,
        env.clone(),
    )?;
    let from_alias = args.from.as_deref().unwrap_or("withdraw_holder");
    if from_alias != "withdraw_holder" {
        bail!("unsupported withdraw holder alias: {from_alias}");
    }
    let to_alias = args.to.as_deref().unwrap_or("withdraw_dest");
    if to_alias != "withdraw_dest" {
        bail!("unsupported withdraw destination alias: {to_alias}");
    }

    let amount_nicks = parse_nock_amount_to_nicks(&args.amount_nock)?;
    let active_bridge_config = BridgeConfigToml::from_file(paths.bridge_config_path(0))
        .context("failed to read bridge-0 config for active withdrawal floor")?;
    ensure_withdrawal_amount_meets_floor(amount_nicks, &active_bridge_config)?;
    let amount_units = nicks_to_nock_base_units(amount_nicks)?;
    let rpc_url = env.require("BASE_RPC_URL")?;
    let inbox = env.require("INBOX_CONTRACT_ADDRESS")?;
    let nock = env.require("NOCK_CONTRACT_ADDRESS")?;
    let inbox_address =
        Address::from_str(&inbox).with_context(|| format!("invalid inbox address {inbox}"))?;
    let nock_address =
        Address::from_str(&nock).with_context(|| format!("invalid nock address {nock}"))?;
    let holder_address =
        Address::from_str(&profile.aliases.withdraw_holder.address).with_context(|| {
            format!(
                "invalid withdraw holder address {}",
                profile.aliases.withdraw_holder.address
            )
        })?;
    let signer = PrivateKeySigner::from_str(
        profile
            .aliases
            .withdraw_holder
            .private_key
            .trim_start_matches("0x"),
    )
    .context("invalid withdraw holder private key")?;
    let signer_address = signer.address();
    if signer_address != holder_address {
        bail!(
            "withdraw holder private key resolves to {signer_address}, expected configured holder {holder_address}"
        );
    }
    let lock_root = NockHash::from_base58(&profile.aliases.withdraw_dest.lock_root_base58)
        .with_context(|| {
            format!(
                "invalid withdraw destination lock root {}",
                profile.aliases.withdraw_dest.lock_root_base58
            )
        })?;

    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::<_, _, Optimism>::default()
        .filler(GasFiller)
        .filler(NonceFiller::<CachedNonceManager>::default())
        .filler(ChainIdFiller::default())
        .filler(WalletFiller::new(wallet))
        .connect_http(
            rpc_url
                .parse()
                .with_context(|| format!("invalid BASE_RPC_URL {rpc_url}"))?,
        );
    let inbox_contract = DevMessageInbox::new(inbox_address, &provider);
    let nock_contract = DevNock::new(nock_address, &provider);

    let withdrawals_enabled = inbox_contract
        .withdrawalsEnabled()
        .call()
        .await
        .context("failed to query MessageInbox.withdrawalsEnabled")?;
    if !withdrawals_enabled {
        bail!("withdrawals are disabled on the current MessageInbox");
    }

    let balance = nock_contract
        .balanceOf(holder_address)
        .call()
        .await
        .context("failed to query Nock.balanceOf")?;
    if balance < U256::from(amount_units) {
        bail!("holder balance {} is below requested amount {}", balance, amount_units);
    }
    remove_last_withdrawal_target(&paths)?;

    let amount_raw = U256::from(amount_units);
    let burn_calldata =
        encode_withdrawal_burn_calldata(nock_address, signer_address, amount_raw, &lock_root);
    let burn = <Optimism as Network>::TransactionRequest::default()
        .from(signer_address)
        .to(nock_address)
        .input(burn_calldata.into());
    let pending_tx = provider
        .send_transaction(burn)
        .await
        .context("failed to submit withdrawal burn tx")?;
    let tx_hash = *pending_tx.tx_hash();
    let receipt = pending_tx
        .get_receipt()
        .await
        .context("failed to fetch withdrawal burn receipt")?;
    let status_ok = receipt
        .inner
        .inner
        .receipt
        .as_receipt()
        .status
        .coerce_status();
    if !status_ok {
        bail!("withdrawal burn reverted on-chain: tx_hash={tx_hash:?}");
    }
    println!("tx hash: {tx_hash:?}");
    Ok(())
}

async fn run_advance_base(paths: Paths, args: AdvanceBaseArgs) -> Result<()> {
    let env = load_generated_env(&paths.env_file)?;
    let rpc_url = env
        .optional("TENDERLY_ADMIN_RPC_URL")
        .unwrap_or(env.require("TENDERLY_RPC_URL")?);
    let status = TokioCommand::new(&paths.advance_blocks_script)
        .arg(args.blocks.to_string())
        .arg(rpc_url)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("failed to launch tenderly-advance-blocks.sh")?;
    if !status.success() {
        bail!("advance-base failed with status {}", status);
    }
    Ok(())
}

async fn ensure_not_running(paths: &Paths) -> Result<()> {
    if !paths.control_socket.exists() {
        return Ok(());
    }
    match UnixStream::connect(&paths.control_socket).await {
        Ok(_) => bail!(
            "bridge-dev already appears to be running at {}",
            paths.control_socket.display()
        ),
        Err(_) => Ok(()),
    }
}

fn remove_stale_socket(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn load_profile_file(path: &Path, expected_profile_name: &str) -> Result<ProfileFile> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let profile: ProfileFile =
        toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;
    if profile.profile.name != expected_profile_name {
        bail!(
            "profile file {} contains profile '{}' but '{}' was requested",
            path.display(),
            profile.profile.name,
            expected_profile_name
        );
    }
    Ok(profile)
}

fn resolve_profile(
    profile: ProfileFile,
    source_path: &Path,
    env: GeneratedEnv,
) -> Result<ResolvedProfile> {
    let deposit_address = resolve_env_backed_address(
        &profile.aliases.deposit_recipient.evm_address,
        profile.aliases.deposit_recipient.source_env.as_deref(),
        &env,
    );
    let withdraw_holder_address = resolve_env_backed_address(
        &profile.aliases.withdraw_holder.evm_address,
        profile.aliases.withdraw_holder.source_env.as_deref(),
        &env,
    );
    let withdraw_holder_private_key = resolve_env_backed_value(
        &profile.aliases.withdraw_holder.private_key,
        profile.aliases.withdraw_holder.private_key_env.as_deref(),
    );
    let withdraw_dest = resolve_withdraw_dest(profile.aliases.withdraw_dest)?;

    Ok(ResolvedProfile {
        source_path: source_path.to_path_buf(),
        profile_name: profile.profile.name,
        cluster: profile.cluster,
        aliases: ResolvedAliases {
            deposit_recipient: ResolvedEvmAlias {
                address: deposit_address,
            },
            withdraw_holder: ResolvedWithdrawHolder {
                address: withdraw_holder_address,
                private_key: withdraw_holder_private_key,
            },
            withdraw_dest,
        },
    })
}

fn resolve_env_backed_address(
    default_value: &str,
    env_key: Option<&str>,
    env: &GeneratedEnv,
) -> String {
    env_key
        .and_then(|key| env.optional(key))
        .or_else(|| env_key.and_then(|key| std::env::var(key).ok()))
        .unwrap_or_else(|| default_value.to_string())
}

fn resolve_env_backed_value(default_value: &str, env_key: Option<&str>) -> String {
    env_key
        .and_then(|key| std::env::var(key).ok())
        .unwrap_or_else(|| default_value.to_string())
}

fn resolve_withdraw_dest(config: WithdrawDestConfig) -> Result<ResolvedWithdrawDest> {
    if let Some(lock_root) = config.lock_root {
        let parsed = NockHash::from_base58(&lock_root)
            .with_context(|| format!("invalid withdraw_dest lock root {lock_root}"))?;
        return Ok(ResolvedWithdrawDest {
            nockchain_address: config.nockchain_address,
            lock_root_base58: parsed.to_base58(),
            lock_root_hex: nock_hash_to_limb_hex(&parsed),
        });
    }

    if let Some(lock_root_hex) = config.lock_root_hex {
        let parsed = parse_nock_lock_root_hex(&lock_root_hex)?;
        return Ok(ResolvedWithdrawDest {
            nockchain_address: config.nockchain_address,
            lock_root_base58: parsed.to_base58(),
            lock_root_hex: nock_hash_to_limb_hex(&parsed),
        });
    }

    if let Some(lock_root_bytes32_hex) = config.lock_root_bytes32_hex {
        bail!(
            "withdraw_dest.lock_root_bytes32_hex is no longer supported because Nockchain lock roots are 40 bytes; got {lock_root_bytes32_hex}. Use nockchain_address, lock_root, or lock_root_hex instead"
        );
    }

    let Some(address) = config.nockchain_address else {
        bail!("withdraw_dest must provide either nockchain_address, lock_root, or lock_root_hex");
    };
    let pkh = NockHash::from_base58(&address)
        .with_context(|| format!("invalid nockchain address {address}"))?;
    let lock_root = Lock::SpendCondition(SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(
        1,
        vec![pkh],
    ))]))
    .hash()
    .map_err(|err| anyhow!("failed to derive lock root from {address}: {err}"))?;
    Ok(ResolvedWithdrawDest {
        nockchain_address: Some(address),
        lock_root_base58: lock_root.to_base58(),
        lock_root_hex: nock_hash_to_limb_hex(&lock_root),
    })
}

fn build_manifest(
    paths: &Paths,
    profile: &ResolvedProfile,
    env: &GeneratedEnv,
) -> Result<Manifest> {
    Ok(Manifest {
        profile_name: profile.profile_name.clone(),
        profile_path: profile.source_path.clone(),
        created_at_unix: unix_now(),
        control_socket: paths.control_socket.clone(),
        env_file: paths.env_file.clone(),
        vnet: VnetManifest {
            vnet_id: env.optional("TENDERLY_VNET_ID"),
            base_rpc_url: env.require("BASE_RPC_URL")?,
            base_ws_url: env.require("BASE_WS_URL")?,
            base_start_height: env
                .require("BASE_START_HEIGHT")?
                .parse::<u64>()
                .context("invalid BASE_START_HEIGHT")?,
            inbox_contract_address: env.require("INBOX_CONTRACT_ADDRESS")?,
            nock_contract_address: env.require("NOCK_CONTRACT_ADDRESS")?,
            tenderly_public_address: env.optional("TENDERLY_PUBLIC_ADDRESS"),
        },
        aliases: AliasManifest {
            deposit_recipient: profile.aliases.deposit_recipient.address.clone(),
            withdraw_holder: profile.aliases.withdraw_holder.address.clone(),
            withdraw_dest_nockchain_address: profile
                .aliases
                .withdraw_dest
                .nockchain_address
                .clone(),
            withdraw_dest_lock_root_base58: profile.aliases.withdraw_dest.lock_root_base58.clone(),
            withdraw_dest_lock_root_hex: profile.aliases.withdraw_dest.lock_root_hex.clone(),
        },
    })
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_vec_pretty(manifest)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn read_manifest(path: &Path) -> Result<Manifest> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

fn ensure_binaries(paths: &Paths) -> Result<()> {
    for (name, path) in [
        ("bridge", paths.bridge_binary()),
        ("nockchain-bridge-sequencer", paths.node_binary()),
    ] {
        if !path.exists() {
            bail!(
                "{name} binary not found at {}. Build with `cargo build --release -p bridge -p nockchain-bridge-sequencer`",
                path.display()
            );
        }
    }
    Ok(())
}

async fn provision_vnet(paths: &Paths, profile: &ProfileFile) -> Result<()> {
    if profile.vnet.cleanup_old_before_fresh && paths.cleanup_script.exists() {
        let status = TokioCommand::new(&paths.cleanup_script)
            .arg("--keep")
            .arg(profile.vnet.cleanup_keep.to_string())
            .arg("--prefix")
            .arg(&profile.vnet.name_prefix)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("failed to launch tenderly-vnet-cleanup.sh")?;
        if !status.success() {
            bail!("tenderly-vnet-cleanup.sh failed with status {}", status);
        }
    }

    let status = TokioCommand::new(&paths.deploy_script)
        .arg("--prefix")
        .arg(&profile.vnet.name_prefix)
        .arg("--output-env")
        .arg(&paths.env_file)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .context("failed to launch tenderly-vnet-deploy.sh")?;
    if !status.success() {
        bail!("tenderly-vnet-deploy.sh failed with status {}", status);
    }
    Ok(())
}

fn load_generated_env(path: &Path) -> Result<GeneratedEnv> {
    let file = File::open(path)
        .with_context(|| format!("failed to open generated env {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut values = BTreeMap::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = raw_value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        values.insert(key.trim().to_string(), value);
    }
    Ok(GeneratedEnv { values })
}

struct BridgeDevConfigPaths {
    bridge_paths: Vec<PathBuf>,
    sequencer_path: PathBuf,
}

fn write_bridge_configs(
    paths: &Paths,
    profile: &ResolvedProfile,
    vnet: &VnetManifest,
) -> Result<BridgeDevConfigPaths> {
    let bridge_lock_root = derive_bridge_dev_lock_root()?;
    let grpc_address = format!("http://{}", node_private_grpc_addr()?);
    let sequencer_api_address = sequencer_endpoint()?;
    let nock_activation_height = withdrawal_activation_nock_next_height()?;
    let constants = BridgeConstantsToml {
        min_signers: 3,
        total_signers: 5,
        minimum_event_nocks: 1000,
        nicks_fee_per_nock: 195,
        base_blocks_chunk: base_blocks_chunk()?,
        base_start_height: vnet.base_start_height,
        nockchain_start_height: 1,
    };
    let ingress_ports = (0..5usize)
        .map(bridge_ingress_port)
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(paths.bridge_config_dir())
        .with_context(|| format!("failed to create {}", paths.bridge_config_dir().display()))?;
    let mut out = Vec::new();
    for node_id in 0..5usize {
        let config = BridgeConfigToml {
            node_id: node_id as u64,
            base_ws_url: vnet.base_ws_url.clone(),
            bridge_lock_root: bridge_lock_root.clone(),
            inbox_contract_address: Some(vnet.inbox_contract_address.clone()),
            nock_contract_address: Some(vnet.nock_contract_address.clone()),
            my_eth_key: BRIDGE_ETH_KEYS[node_id].to_string(),
            my_nock_key: BRIDGE_NOCK_KEYS[node_id].to_string(),
            grpc_address: grpc_address.clone(),
            nockchain_sequencer_api_address: Some(sequencer_api_address.clone()),
            base_confirmation_depth: profile.cluster.base_confirmation_depth,
            nockchain_confirmation_depth: profile.cluster.nockchain_confirmation_depth,
            deposit_nonce_epoch_base: None,
            deposit_nonce_epoch_start_height: None,
            deposit_nonce_epoch_start_tx_id_base58: None,
            withdrawal_activation_nock_next_height: Some(nock_activation_height),
            ingress_listen_address: Some(format!("127.0.0.1:{}", ingress_ports[node_id])),
            nodes: (0..5usize)
                .map(|peer_id| NodeInfoToml {
                    ip: format!("127.0.0.1:{}", ingress_ports[peer_id]),
                    eth_pubkey: BRIDGE_ETH_ADDRS[peer_id].to_string(),
                    nock_pkh: BRIDGE_NOCK_PKHS[peer_id].to_string(),
                })
                .collect(),
            constants: Some(constants.clone()),
        };
        let config_path = paths.bridge_config_path(node_id);
        fs::write(&config_path, toml::to_string_pretty(&config)?)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        out.push(config_path);
    }
    let sequencer_config = SequencerConfigToml {
        nock_contract_address: vnet.nock_contract_address.clone(),
        nockchain_confirmation_depth: profile.cluster.nockchain_confirmation_depth,
        nodes: (0..5usize)
            .map(|peer_id| SequencerNodeInfoToml {
                eth_pubkey: BRIDGE_ETH_ADDRS[peer_id].to_string(),
                nock_pkh: BRIDGE_NOCK_PKHS[peer_id].to_string(),
            })
            .collect(),
        sequencer_journal: bridge_dev_sequencer_journal_config()?,
        constants: Some(constants),
    };
    let sequencer_path = paths.sequencer_config_path();
    fs::write(&sequencer_path, toml::to_string_pretty(&sequencer_config)?)
        .with_context(|| format!("failed to write {}", sequencer_path.display()))?;
    Ok(BridgeDevConfigPaths {
        bridge_paths: out,
        sequencer_path,
    })
}

fn bridge_dev_sequencer_journal_config() -> Result<SequencerJournalConfigToml> {
    let enabled = matches!(
        std::env::var(BRIDGE_DEV_SEQUENCER_JOURNAL_ENABLED_ENV)
            .ok()
            .as_deref(),
        Some("1" | "true" | "yes")
    );
    let verifier_address = if enabled {
        Some(bridge_dev_sequencer_journal_verifier_address()?)
    } else {
        None
    };
    Ok(SequencerJournalConfigToml {
        enabled,
        verifier_address,
        ..SequencerJournalConfigToml::default()
    })
}

fn bridge_dev_sequencer_journal_signing_key() -> Result<String> {
    let key = std::env::var(BRIDGE_DEV_SEQUENCER_JOURNAL_SIGNING_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| BRIDGE_DEV_DEFAULT_SEQUENCER_JOURNAL_SIGNING_KEY.to_string());
    bridge_dev_sequencer_journal_verifier_address_for_key(&key)?;
    Ok(key)
}

fn bridge_dev_sequencer_journal_verifier_address() -> Result<String> {
    let signing_key = bridge_dev_sequencer_journal_signing_key()?;
    bridge_dev_sequencer_journal_verifier_address_for_key(&signing_key)
}

fn bridge_dev_sequencer_journal_verifier_address_for_key(signing_key: &str) -> Result<String> {
    let signer = PrivateKeySigner::from_str(signing_key.strip_prefix("0x").unwrap_or(signing_key))
        .with_context(|| "sequencer journal signing key must be a valid secp256k1 key")?;
    Ok(signer.address().to_string())
}

fn bridge_dev_sequencer_journal_envs(
    config: &SequencerJournalConfigToml,
) -> Result<Vec<(&'static str, String)>> {
    if !config.enabled {
        return Ok(Vec::new());
    }
    Ok(bridge_dev_sequencer_journal_envs_for_key(
        config,
        bridge_dev_sequencer_journal_signing_key()?,
    ))
}

fn bridge_dev_sequencer_journal_envs_for_key(
    config: &SequencerJournalConfigToml,
    signing_key: String,
) -> Vec<(&'static str, String)> {
    if !config.enabled {
        return Vec::new();
    }
    vec![("WITHDRAWAL_SEQUENCER_JOURNAL_SIGNING_KEY", signing_key)]
}

fn bridge_dev_sequencer_base_confirmation_depth(config_depth: u64) -> u64 {
    // The sequencer binary requires a nonzero confirmed Base height depth, even
    // though bridge-dev workers can use depth zero to make Base projection fast.
    config_depth.max(1)
}

fn derive_bridge_dev_lock_root() -> Result<String> {
    let bridge_pkhs = BRIDGE_NOCK_PKHS
        .iter()
        .map(|raw| {
            NockHash::from_base58(raw)
                .with_context(|| format!("invalid bridge-dev nock pkh {}", raw))
        })
        .collect::<Result<Vec<_>>>()?;
    let (_, derived_root) = derive_bridge_spend_authority_from_pkhs(3, bridge_pkhs)
        .map_err(|err| anyhow!(err.to_string()))?;
    let canonical_testing_root =
        canonical_testing_bridge_lock_root().map_err(|err| anyhow!(err.to_string()))?;
    if derived_root != canonical_testing_root {
        bail!(
            "bridge-dev signer set derives {}, but canonical testing bridge root is {}",
            derived_root.to_base58(),
            canonical_testing_root.to_base58()
        );
    }
    Ok(derived_root.to_base58())
}

async fn spawn_node(
    paths: &Paths,
    manifest: &Manifest,
    bridge_config_path: &Path,
    sequencer_config_path: &Path,
    fresh_state: bool,
) -> Result<ManagedChild> {
    fs::create_dir_all(paths.node_dir())
        .with_context(|| format!("failed to create {}", paths.node_dir().display()))?;
    let bridge_config = BridgeConfigToml::from_file(bridge_config_path)?;
    let sequencer_config = SequencerConfigToml::from_file(sequencer_config_path)?;
    let sequencer_base_confirmation_depth =
        bridge_dev_sequencer_base_confirmation_depth(bridge_config.base_confirmation_depth);
    let mut args = vec![
        "--fakenet".to_string(),
        "--fakenet-genesis-jam-path".to_string(),
        fakenet_genesis_jam_path(paths)?.display().to_string(),
        "--fakenet-pow-len".to_string(),
        fakenet_pow_len()?.to_string(),
        "--fakenet-log-difficulty".to_string(),
        fakenet_log_difficulty()?.to_string(),
        "--mine".to_string(),
        "--mining-pkh".to_string(),
        FAKENET_MINING_PKH.to_string(),
        "--bind".to_string(),
        node_bind_addr()?,
        "--bind-public-grpc-addr".to_string(),
        node_public_grpc_addr()?,
        "--bind-private-grpc-port".to_string(),
        node_private_grpc_port()?.to_string(),
        "--base-ws-url".to_string(),
        manifest.vnet.base_ws_url.clone(),
        "--base-confirmation-depth".to_string(),
        sequencer_base_confirmation_depth.to_string(),
        "--withdrawal-handoff-window-blocks".to_string(),
        BRIDGE_DEV_WITHDRAWAL_HANDOFF_WINDOW_BLOCKS.to_string(),
        "--sequencer-config-path".to_string(),
        sequencer_config_path.display().to_string(),
    ];
    if fresh_state {
        args.insert(0, "--new".to_string());
    }
    let mut envs = vec![("NOCKAPP_HOME", paths.node_dir().display().to_string())];
    envs.extend(bridge_dev_sequencer_journal_envs(
        &sequencer_config.sequencer_journal,
    )?);
    spawn_process(
        "node",
        &paths.node_binary(),
        &args,
        &paths.node_dir(),
        &envs,
        &paths.stdout_log("node"),
        &paths.stderr_log("node"),
    )
}

fn spawn_bridge(
    paths: &Paths,
    node_id: usize,
    config_path: &Path,
    fresh_state: bool,
    start: bool,
) -> Result<ManagedChild> {
    let name = format!("bridge-{node_id}");
    let data_dir = paths.bridge_data_dir(node_id);
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;
    let mut args = Vec::new();
    if fresh_state {
        args.push("--new".to_string());
    }
    if start {
        args.push("--start".to_string());
    }
    if let Some(save_interval_secs) = bridge_save_interval_event_time_secs()? {
        args.push("--rotating-snapshot-interval-event-time".to_string());
        args.push(save_interval_secs.to_string());
    }
    args.extend_from_slice(&[
        "--config-path".to_string(),
        config_path.display().to_string(),
        "--data-dir".to_string(),
        data_dir.display().to_string(),
        "--log-dir".to_string(),
        paths.bridge_runtime_log_dir(node_id).display().to_string(),
    ]);
    spawn_process(
        &name,
        &paths.bridge_binary(),
        &args,
        &data_dir,
        &[
            (
                "RUST_LOG",
                "info,h2=warn,hyper=warn,tower=warn,tonic=info".to_string(),
            ),
            ("NOCKAPP_HOME", data_dir.display().to_string()),
        ],
        &paths.stdout_log(&name),
        &paths.stderr_log(&name),
    )
}

fn spawn_process(
    name: &str,
    binary: &Path,
    args: &[String],
    current_dir: &Path,
    envs: &[(&str, String)],
    stdout_log: &Path,
    stderr_log: &Path,
) -> Result<ManagedChild> {
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stdout_log)
        .with_context(|| format!("failed to open {}", stdout_log.display()))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log)
        .with_context(|| format!("failed to open {}", stderr_log.display()))?;
    let mut command = TokioCommand::new(binary);
    command
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    for (key, value) in envs {
        command.env(key, value);
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn {} from {}", name, binary.display()))?;
    Ok(ManagedChild {
        name: name.to_string(),
        child,
        stdout_log: stdout_log.to_path_buf(),
        stderr_log: stderr_log.to_path_buf(),
    })
}

async fn supervisor_loop(
    paths: &Paths,
    manifest: &Manifest,
    listener: UnixListener,
    children: &mut [ManagedChild],
) -> Result<bool> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                return Ok(false);
            }
            accepted = listener.accept() => {
                let (mut stream, _) = accepted.context("failed to accept control connection")?;
                let should_shutdown = handle_control_request(paths, manifest, &mut stream, children).await?;
                if should_shutdown {
                    return Ok(true);
                }
            }
        }
    }
}

async fn handle_control_request(
    paths: &Paths,
    manifest: &Manifest,
    stream: &mut UnixStream,
    children: &mut [ManagedChild],
) -> Result<bool> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .context("failed to read control request")?;
    let request: ControlRequest =
        serde_json::from_slice(&bytes).context("failed to decode control request")?;
    let (response, shutdown) = match request {
        ControlRequest::Status => (
            ControlResponse::Status {
                components: children
                    .iter_mut()
                    .map(ManagedChild::status)
                    .collect::<Result<Vec<_>>>()?,
            },
            false,
        ),
        ControlRequest::Down => {
            append_supervisor_log(&paths.supervisor_log, "received shutdown request")?;
            (ControlResponse::DownAck, true)
        }
        ControlRequest::Stop { target } => {
            match update_component(paths, manifest, children, &target, ComponentAction::Stop).await
            {
                Ok(component) => (ControlResponse::ComponentAck { component }, false),
                Err(err) => (
                    ControlResponse::Error {
                        message: err.to_string(),
                    },
                    false,
                ),
            }
        }
        ControlRequest::Start { target } => {
            match update_component(paths, manifest, children, &target, ComponentAction::Start).await
            {
                Ok(component) => (ControlResponse::ComponentAck { component }, false),
                Err(err) => (
                    ControlResponse::Error {
                        message: err.to_string(),
                    },
                    false,
                ),
            }
        }
        ControlRequest::Restart { target } => {
            match update_component(paths, manifest, children, &target, ComponentAction::Restart)
                .await
            {
                Ok(component) => (ControlResponse::ComponentAck { component }, false),
                Err(err) => (
                    ControlResponse::Error {
                        message: err.to_string(),
                    },
                    false,
                ),
            }
        }
    };
    stream
        .write_all(&serde_json::to_vec(&response)?)
        .await
        .context("failed to write control response")?;
    Ok(shutdown)
}

async fn update_component(
    paths: &Paths,
    manifest: &Manifest,
    children: &mut [ManagedChild],
    target: &str,
    action: ComponentAction,
) -> Result<ComponentStatus> {
    let target = ComponentTarget::parse(target)?;
    let index = component_index(target);
    if index >= children.len() {
        bail!(
            "component {} is not managed by this supervisor",
            target.name()
        );
    }

    append_supervisor_log(
        &paths.supervisor_log,
        &format!("{} {}", action.label(), target.name()),
    )?;

    match action {
        ComponentAction::Stop => {
            stop_managed_child(&mut children[index]).await?;
            children[index].status()
        }
        ComponentAction::Start => {
            if child_is_running(&mut children[index])? && matches!(target, ComponentTarget::Node) {
                children[index].status()
            } else {
                if child_is_running(&mut children[index])? {
                    stop_managed_child(&mut children[index]).await?;
                }
                children[index] = spawn_component(paths, manifest, target, false, true).await?;
                wait_for_component(target, component_startup_timeout(target)).await?;
                children[index].status()
            }
        }
        ComponentAction::Restart => {
            stop_managed_child(&mut children[index]).await?;
            children[index] = spawn_component(paths, manifest, target, false, true).await?;
            wait_for_component(target, component_startup_timeout(target)).await?;
            children[index].status()
        }
    }
}

fn component_startup_timeout(target: ComponentTarget) -> Duration {
    match target {
        ComponentTarget::Node => NODE_STARTUP_TIMEOUT,
        ComponentTarget::Bridge(_) => BRIDGE_STARTUP_TIMEOUT,
    }
}

fn component_index(target: ComponentTarget) -> usize {
    match target {
        ComponentTarget::Node => 0,
        ComponentTarget::Bridge(node_id) => node_id + 1,
    }
}

fn child_is_running(child: &mut ManagedChild) -> Result<bool> {
    Ok(child.child.try_wait()?.is_none())
}

async fn stop_managed_child(child: &mut ManagedChild) -> Result<()> {
    if !child_is_running(child)? {
        return Ok(());
    }

    signal_child(child, libc::SIGINT)?;
    wait_for_child_exit(child, CHILD_SIGINT_SHUTDOWN_TIMEOUT).await?;
    if !child_is_running(child)? {
        return Ok(());
    }

    signal_child(child, libc::SIGTERM)?;
    wait_for_child_exit(child, Duration::from_secs(3)).await?;
    if !child_is_running(child)? {
        return Ok(());
    }

    child.child.start_kill().context("failed to send kill")?;
    wait_for_child_exit(child, Duration::from_secs(3)).await?;
    if child_is_running(child)? {
        bail!("{} did not exit after SIGKILL", child.name);
    }
    Ok(())
}

async fn wait_for_child_exit(child: &mut ManagedChild, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if child.child.try_wait()?.is_some() || Instant::now() >= deadline {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn spawn_component(
    paths: &Paths,
    manifest: &Manifest,
    target: ComponentTarget,
    fresh_state: bool,
    start: bool,
) -> Result<ManagedChild> {
    match target {
        ComponentTarget::Node => {
            spawn_node(
                paths,
                manifest,
                &paths.bridge_config_path(0),
                &paths.sequencer_config_path(),
                fresh_state,
            )
            .await
        }
        ComponentTarget::Bridge(node_id) => spawn_bridge(
            paths,
            node_id,
            &paths.bridge_config_path(node_id),
            fresh_state,
            start,
        ),
    }
}

async fn send_control_request(path: &Path, request: ControlRequest) -> Result<ControlResponse> {
    let mut stream = UnixStream::connect(path)
        .await
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    stream
        .write_all(&serde_json::to_vec(&request)?)
        .await
        .context("failed to write control request")?;
    stream
        .shutdown()
        .await
        .context("failed to close control write half")?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .context("failed to read control response")?;
    serde_json::from_slice(&bytes).context("failed to decode control response")
}

async fn shutdown_children(children: &mut [ManagedChild]) -> Result<()> {
    for child in children.iter_mut().rev() {
        signal_child(child, libc::SIGINT)?;
    }
    wait_for_children_exit(children, Duration::from_secs(5)).await?;

    for child in children.iter_mut().rev() {
        if child.child.try_wait()?.is_none() {
            signal_child(child, libc::SIGTERM)?;
        }
    }
    wait_for_children_exit(children, Duration::from_secs(3)).await?;

    for child in children.iter_mut().rev() {
        if child.child.try_wait()?.is_none() {
            child.child.start_kill().context("failed to send kill")?;
        }
    }
    wait_for_children_exit(children, Duration::from_secs(3)).await?;
    Ok(())
}

fn signal_child(child: &ManagedChild, signal: i32) -> Result<()> {
    let Some(pid) = child.child.id() else {
        return Ok(());
    };
    let rc = unsafe { libc::kill(pid as i32, signal) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).with_context(|| format!("failed to signal {}", child.name));
        }
    }
    Ok(())
}

async fn wait_for_children_exit(children: &mut [ManagedChild], timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut all_exited = true;
        for child in children.iter_mut() {
            if child.child.try_wait()?.is_none() {
                all_exited = false;
            }
        }
        if all_exited || Instant::now() >= deadline {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

enum SocketTarget {
    PrivateNodeGrpc,
    BridgeIngress(usize),
}

async fn wait_for_port(target: SocketTarget, timeout: Duration) -> Result<()> {
    let addr = match target {
        SocketTarget::PrivateNodeGrpc => node_private_grpc_addr()?,
        SocketTarget::BridgeIngress(node_id) => {
            format!("127.0.0.1:{}", bridge_ingress_port(node_id)?)
        }
    };
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(&addr).await {
            Ok(_) => return Ok(()),
            Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(250)).await,
            Err(err) => {
                return Err(err).with_context(|| format!("timed out waiting for {addr}"));
            }
        }
    }
}

async fn wait_for_component(target: ComponentTarget, timeout: Duration) -> Result<()> {
    match target {
        ComponentTarget::Node => wait_for_port(SocketTarget::PrivateNodeGrpc, timeout).await,
        ComponentTarget::Bridge(node_id) => {
            wait_for_port(SocketTarget::BridgeIngress(node_id), timeout).await
        }
    }
}

fn append_supervisor_log(path: &Path, message: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "[{}] {}", unix_now(), message)
        .with_context(|| format!("failed to append {}", path.display()))
}

fn parse_nock_amount_units(raw: &str) -> Result<u128> {
    let mut parts = raw.split('.');
    let whole = parts
        .next()
        .ok_or_else(|| anyhow!("missing amount"))?
        .trim();
    let frac = parts.next().unwrap_or("").trim();
    if parts.next().is_some() {
        bail!("invalid NOCK amount {raw}");
    }
    if whole.is_empty() && frac.is_empty() {
        bail!("amount cannot be empty");
    }
    if frac.len() > 16 {
        bail!("amount supports at most 16 decimal places");
    }

    let whole_value = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<u128>()
            .with_context(|| format!("invalid whole amount component {whole}"))?
    };
    let frac_value = if frac.is_empty() {
        0
    } else {
        let padded = format!("{:0<16}", frac);
        padded
            .parse::<u128>()
            .with_context(|| format!("invalid fractional amount component {frac}"))?
    };
    whole_value
        .checked_mul(10_u128.pow(16))
        .and_then(|whole_units| whole_units.checked_add(frac_value))
        .ok_or_else(|| anyhow!("amount overflow"))
}

fn parse_nock_amount_to_nicks(raw: &str) -> Result<u64> {
    let base_units = parse_nock_amount_units(raw)?;
    if base_units == 0 {
        bail!("amount must be positive");
    }
    if base_units % NOCK_BASE_PER_NICK != 0 {
        bail!(
            "amount {raw} is not aligned to nick granularity (must be divisible by {})",
            NOCK_BASE_PER_NICK
        );
    }
    let nicks = base_units / NOCK_BASE_PER_NICK;
    u64::try_from(nicks).context("amount exceeds representable nicks range")
}

fn nicks_to_nock_base_units(amount_nicks: u64) -> Result<u128> {
    u128::from(amount_nicks)
        .checked_mul(NOCK_BASE_PER_NICK)
        .ok_or_else(|| anyhow!("amount overflow"))
}

fn resolve_mint_recipient(profile: &ResolvedProfile, alias: &str) -> Result<Address> {
    let address = match alias {
        "withdraw_holder" => &profile.aliases.withdraw_holder.address,
        "deposit_recipient" => &profile.aliases.deposit_recipient.address,
        other => bail!("unsupported mint recipient alias: {other}"),
    };
    Address::from_str(address).with_context(|| format!("invalid EVM address for alias {alias}"))
}

fn ensure_mint_for_burn_target(env: &GeneratedEnv, rpc_url: &str) -> Result<()> {
    let vnet_id = env
        .optional("TENDERLY_VNET_ID")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "mint-for-burn only supports Tenderly VNET deployments; missing TENDERLY_VNET_ID in generated env"
            )
        })?;
    if !rpc_url.contains(".rpc.tenderly.co/") {
        bail!("mint-for-burn refuses to use non-Tenderly RPC URL {rpc_url} for VNET {vnet_id}");
    }
    Ok(())
}

fn resolve_mint_for_burn_owner_key(
    bridge_dev_owner_key: Option<&str>,
    tenderly_test_private_key: Option<&str>,
) -> Result<String> {
    for value in [bridge_dev_owner_key, tenderly_test_private_key]
        .into_iter()
        .flatten()
    {
        let trimmed = value.trim().trim_matches('"').trim_matches('\'');
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    bail!(
        "mint-for-burn requires BRIDGE_DEV_OWNER_PRIVATE_KEY or TENDERLY_TEST_PRIVATE_KEY in the environment"
    )
}

impl StableSnapshot {
    fn unhealthy_peer_count(&self) -> usize {
        self.peer_statuses
            .iter()
            .filter(|peer| peer.status != tui_proto::PeerHealthStatus::Healthy)
            .count()
    }
}

fn wait_withdrawal_phase(args: &WaitWithdrawalArgs) -> WaitWithdrawalPhase {
    if args.pending {
        WaitWithdrawalPhase::Pending
    } else if args.ready {
        WaitWithdrawalPhase::Ready
    } else if args.executed {
        WaitWithdrawalPhase::Executed
    } else {
        WaitWithdrawalPhase::Submitted
    }
}

async fn fetch_stable_snapshot() -> Result<StableSnapshot> {
    fetch_stable_snapshot_for_node(0).await
}

async fn fetch_stable_snapshot_for_node(node_id: usize) -> Result<StableSnapshot> {
    let endpoint = bridge_tui_endpoint(node_id)?;
    let mut client = BridgeTuiClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("failed to connect to bridge-{node_id} TUI endpoint"))?;
    let response = client
        .get_snapshot(tonic::Request::new(tui_proto::GetSnapshotRequest {
            deposit_log_view: Some(tui_proto::DepositLogView {
                offset: 0,
                limit: 0,
            }),
            alert_view: Some(tui_proto::AlertView { limit: 0 }),
        }))
        .await
        .with_context(|| format!("failed to query bridge-{node_id} TUI snapshot at {endpoint}"))?
        .into_inner();
    stable_snapshot_from_proto(response)
}

fn bridge_tui_endpoint(node_id: usize) -> Result<String> {
    let port = bridge_ingress_port(node_id)?;
    Ok(format!("http://127.0.0.1:{port}"))
}

fn stable_snapshot_from_proto(response: tui_proto::GetSnapshotResponse) -> Result<StableSnapshot> {
    let network = response
        .network_state
        .ok_or_else(|| anyhow!("snapshot missing network_state"))?;
    let proposals = response
        .proposals
        .ok_or_else(|| anyhow!("snapshot missing proposals"))?;
    let base = network
        .base
        .ok_or_else(|| anyhow!("snapshot missing base chain state"))?;
    let nockchain = network
        .nockchain
        .ok_or_else(|| anyhow!("snapshot missing nockchain chain state"))?;
    let nockchain_api_status = network
        .nockchain_api_status
        .ok_or_else(|| anyhow!("snapshot missing nockchain_api_status"))?;

    Ok(StableSnapshot {
        running_state: parse_running_state(response.running_state),
        nock_hold: response.nock_hold,
        base_hold: response.base_hold,
        nock_hold_height: response.nock_hold_height,
        base_hold_height: response.base_hold_height,
        base_height: base.height,
        nock_height: nockchain.height,
        pending_deposits: network.pending_deposits,
        pending_withdrawals: network.pending_withdrawals,
        unsettled_deposit_count: network.unsettled_deposit_count,
        unsettled_withdrawal_count: network.unsettled_withdrawal_count,
        batch_status: batch_status_label(network.batch_status),
        nockchain_api_state: parse_nockchain_api_state(nockchain_api_status.state),
        nockchain_api_last_error: nockchain_api_status.last_error,
        degradation_warning: network.degradation_warning,
        peer_statuses: response
            .peer_statuses
            .into_iter()
            .map(|peer| StablePeerStatus {
                node_id: peer.node_id,
                status: parse_peer_health_status(peer.status),
            })
            .collect(),
        last_submitted_deposit: response
            .last_submitted_deposit
            .map(stable_deposit_from_last_submitted),
        last_successful_deposit: response
            .last_successful_deposit
            .map(stable_deposit_from_successful),
        last_submitted_proposal: proposals.last_submitted.map(stable_proposal_from_proto),
        pending_inbound_proposals: proposals
            .pending_inbound
            .into_iter()
            .map(stable_proposal_from_proto)
            .collect(),
    })
}

fn parse_running_state(value: i32) -> tui_proto::RunningState {
    tui_proto::RunningState::try_from(value).unwrap_or(tui_proto::RunningState::Unspecified)
}

fn parse_nockchain_api_state(value: i32) -> tui_proto::nockchain_api_status::State {
    tui_proto::nockchain_api_status::State::try_from(value)
        .unwrap_or(tui_proto::nockchain_api_status::State::Unspecified)
}

fn parse_peer_health_status(value: i32) -> tui_proto::PeerHealthStatus {
    tui_proto::PeerHealthStatus::try_from(value).unwrap_or(tui_proto::PeerHealthStatus::Unspecified)
}

fn parse_proposal_status(value: i32) -> tui_proto::ProposalStatus {
    tui_proto::ProposalStatus::try_from(value).unwrap_or(tui_proto::ProposalStatus::Unspecified)
}

fn stable_deposit_from_last_submitted(deposit: tui_proto::LastDeposit) -> StableDeposit {
    StableDeposit {
        tx_id: deposit.tx_id.map(|hash| hash.value),
        nonce: deposit.nonce,
        amount: deposit.amount,
        recipient: deposit.recipient.map(|recipient| recipient.value),
        base_block_number: Some(deposit.base_block_number),
    }
}

fn stable_deposit_from_successful(deposit: tui_proto::SuccessfulDeposit) -> StableDeposit {
    StableDeposit {
        tx_id: deposit.tx_id.map(|hash| hash.value),
        nonce: deposit.nonce,
        amount: deposit.amount,
        recipient: deposit.recipient.map(|recipient| recipient.value),
        base_block_number: None,
    }
}

fn stable_proposal_from_proto(proposal: tui_proto::Proposal) -> StableProposal {
    StableProposal {
        id: proposal.id,
        proposal_type: proposal.proposal_type,
        status: parse_proposal_status(proposal.status),
        signatures_collected: proposal.signatures_collected,
        signatures_required: proposal.signatures_required,
        nonce: proposal.nonce,
        source_tx_id: proposal.source_tx_id,
        tx_hash: proposal.tx_hash,
    }
}

fn batch_status_label(status: Option<tui_proto::BatchStatus>) -> String {
    let Some(status) = status.and_then(|status| status.status) else {
        return "unknown".to_string();
    };
    match status {
        tui_proto::batch_status::Status::Idle(_) => "idle".to_string(),
        tui_proto::batch_status::Status::Processing(value) => {
            format!(
                "processing(batch={} progress={}%)",
                value.batch_id, value.progress_pct
            )
        }
        tui_proto::batch_status::Status::AwaitingSignatures(value) => format!(
            "awaiting_signatures(batch={} {}/{})",
            value.batch_id, value.collected, value.required
        ),
        tui_proto::batch_status::Status::Submitting(value) => {
            format!("submitting(batch={})", value.batch_id)
        }
    }
}

fn ensure_wait_conditions(snapshot: &StableSnapshot) -> Result<()> {
    if snapshot.running_state == tui_proto::RunningState::Stopped {
        bail!("bridge is stopped");
    }
    Ok(())
}

fn ensure_withdrawal_wait_conditions(snapshot: &StableSnapshot) -> Result<()> {
    ensure_wait_conditions(snapshot)?;
    if !snapshot.peer_statuses.is_empty()
        && snapshot
            .peer_statuses
            .iter()
            .all(|peer| peer.status != tui_proto::PeerHealthStatus::Healthy)
    {
        bail!("all bridge peers are unreachable from bridge-0");
    }
    Ok(())
}

fn sequencer_endpoint() -> Result<String> {
    Ok(format!("http://127.0.0.1:{}", sequencer_api_port()?))
}

async fn fetch_next_pending_withdrawal() -> Result<Option<TrackedWithdrawal>> {
    let endpoint = sequencer_endpoint()?;
    let mut client = WithdrawalSequencerClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("failed to connect to withdrawal sequencer at {endpoint}"))?;
    let response = client
        .get_next_pending_withdrawal_ordering(tonic::Request::new(
            ingress_proto::NextPendingWithdrawalOrderingRequest {},
        ))
        .await
        .context("failed to query next pending withdrawal ordering")?
        .into_inner();
    if !response.found {
        return Ok(None);
    }
    let id = response
        .withdrawal_id
        .ok_or_else(|| anyhow!("sequencer reported a pending withdrawal without an id"))?;
    Ok(Some(TrackedWithdrawal {
        id,
        nonce: response.withdrawal_nonce,
    }))
}

async fn fetch_withdrawal_progress(
    target: &TrackedWithdrawal,
) -> Result<Option<WithdrawalProgress>> {
    let sequencer_endpoint = sequencer_endpoint()?;
    let mut sequencer = WithdrawalSequencerClient::connect(sequencer_endpoint.clone())
        .await
        .with_context(|| {
            format!("failed to connect to withdrawal sequencer at {sequencer_endpoint}")
        })?;
    let sequenced = sequencer
        .get_sequenced_withdrawal_status(tonic::Request::new(
            ingress_proto::SequencedWithdrawalStatusRequest {
                withdrawal_id: Some(target.id.clone()),
            },
        ))
        .await
        .context("failed to query sequenced withdrawal status")?
        .into_inner();
    if !sequenced.found {
        return Ok(None);
    }

    let ingress_endpoint = bridge_tui_endpoint(0)?;
    let mut ingress = BridgeIngressClient::connect(ingress_endpoint.clone())
        .await
        .with_context(|| format!("failed to connect to bridge ingress at {ingress_endpoint}"))?;
    // Before canonicalization, bridge-0 may not be able to hydrate proposal artifacts yet. The
    // sequencer state is still useful progress, so treat proposal status as optional while polling.
    let proposal = ingress
        .get_withdrawal_proposal_status(tonic::Request::new(
            ingress_proto::WithdrawalProposalStatusRequest {
                withdrawal_id: Some(target.id.clone()),
                epoch: sequenced.current_epoch,
            },
        ))
        .await
        .ok()
        .map(tonic::Response::into_inner);
    let handoff_owner =
        withdrawal_handoff_owner(&target.id, sequenced.current_epoch, sequenced.handoff_index)?;

    Ok(Some(WithdrawalProgress {
        id_label: withdrawal_id_label(&target.id),
        nonce: Some(sequenced.withdrawal_nonce),
        current_epoch: Some(sequenced.current_epoch),
        handoff_index: Some(sequenced.handoff_index),
        handoff_owner: Some(handoff_owner),
        turn_started_base_height: sequenced.turn_started_base_height,
        proposal_status: proposal
            .as_ref()
            .and_then(|proposal| (!proposal.status.is_empty()).then(|| proposal.status.clone())),
        sequenced_state: if sequenced.state.is_empty() {
            None
        } else {
            Some(sequenced.state)
        },
        transaction_name: proposal.as_ref().and_then(|proposal| {
            (!proposal.transaction_name.is_empty()).then(|| proposal.transaction_name.clone())
        }),
        authorized_transaction_name: if sequenced.authorized_transaction_name.is_empty() {
            None
        } else {
            Some(sequenced.authorized_transaction_name)
        },
        proposal_hash: if !sequenced.proposal_hash.is_empty() {
            Some(sequenced.proposal_hash)
        } else {
            proposal.as_ref().and_then(|proposal| {
                (!proposal.proposal_hash.is_empty()).then(|| proposal.proposal_hash.clone())
            })
        },
    }))
}

async fn fetch_sequencer_status() -> Result<SequencerStatusView> {
    let endpoint = sequencer_endpoint()?;
    let mut client = WithdrawalSequencerClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("failed to connect to withdrawal sequencer at {endpoint}"))?;
    let reserved_inputs = client
        .get_reserved_withdrawal_inputs(tonic::Request::new(
            ingress_proto::SequencerReservedWithdrawalInputsRequest {},
        ))
        .await
        .context("failed to query reserved withdrawal inputs")?
        .into_inner()
        .reserved_inputs
        .len();
    let pending = client
        .get_next_pending_withdrawal_ordering(tonic::Request::new(
            ingress_proto::NextPendingWithdrawalOrderingRequest {},
        ))
        .await
        .context("failed to query next pending withdrawal ordering")?
        .into_inner();
    if !pending.found {
        return Ok(SequencerStatusView {
            reserved_inputs,
            pending: None,
        });
    }
    let id = pending
        .withdrawal_id
        .ok_or_else(|| anyhow!("sequencer reported a pending withdrawal without an id"))?;
    let pending = fetch_withdrawal_progress(&TrackedWithdrawal {
        id,
        nonce: pending.withdrawal_nonce,
    })
    .await?;
    Ok(SequencerStatusView {
        reserved_inputs,
        pending,
    })
}

fn withdrawal_phase_satisfied(phase: WaitWithdrawalPhase, progress: &WithdrawalProgress) -> bool {
    let proposal_status = progress.proposal_status.as_deref().unwrap_or("");
    let sequenced_state = progress.sequenced_state.as_deref().unwrap_or("");
    match phase {
        WaitWithdrawalPhase::Pending => {
            matches!(proposal_status, "persisted" | "canonicalized")
                || matches!(
                    sequenced_state,
                    "pending"
                        | "assembling"
                        | "prepared"
                        | "peer_canonical"
                        | "authorized"
                        | "mempool_accepted"
                        | "confirmed"
                )
        }
        WaitWithdrawalPhase::Ready => {
            proposal_status == "canonicalized"
                || matches!(
                    sequenced_state,
                    "peer_canonical" | "authorized" | "mempool_accepted" | "confirmed"
                )
        }
        WaitWithdrawalPhase::Submitted => {
            matches!(sequenced_state, "mempool_accepted" | "confirmed")
        }
        WaitWithdrawalPhase::Executed => sequenced_state == "confirmed",
    }
}

fn read_last_withdrawal_target(paths: &Paths) -> Result<Option<TrackedWithdrawal>> {
    if !paths.last_withdrawal_target_path().exists() {
        return Ok(None);
    }
    let bytes = fs::read(paths.last_withdrawal_target_path()).with_context(|| {
        format!(
            "failed to read {}",
            paths.last_withdrawal_target_path().display()
        )
    })?;
    let stored: StoredWithdrawalTarget = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "failed to parse {}",
            paths.last_withdrawal_target_path().display()
        )
    })?;
    Ok(Some(TrackedWithdrawal {
        id: ingress_proto::WithdrawalId {
            as_of: stored.as_of,
            base_event_id: stored.base_event_id,
        },
        nonce: stored.nonce,
    }))
}

fn withdrawal_target_from_wait_args(
    args: &WaitWithdrawalArgs,
) -> Result<Option<TrackedWithdrawal>> {
    let has_any_target_arg = args.withdrawal_id_as_of_hex.is_some()
        || args.withdrawal_id_base_event_hex.is_some()
        || args.withdrawal_nonce.is_some();
    if !has_any_target_arg {
        return Ok(None);
    }
    let as_of_hex = args.withdrawal_id_as_of_hex.as_deref().ok_or_else(|| {
        anyhow!("--withdrawal-id-as-of-hex is required when selecting a withdrawal target")
    })?;
    let base_event_hex = args
        .withdrawal_id_base_event_hex
        .as_deref()
        .ok_or_else(|| {
            anyhow!("--withdrawal-id-base-event-hex is required when selecting a withdrawal target")
        })?;
    let nonce = args.withdrawal_nonce.ok_or_else(|| {
        anyhow!("--withdrawal-nonce is required when selecting a withdrawal target")
    })?;
    Ok(Some(TrackedWithdrawal {
        id: ingress_proto::WithdrawalId {
            as_of: parse_hex_bytes(as_of_hex, "withdrawal id as_of")?,
            base_event_id: parse_hex_bytes(base_event_hex, "withdrawal id base_event_id")?,
        },
        nonce,
    }))
}

fn write_last_withdrawal_target(paths: &Paths, target: &TrackedWithdrawal) -> Result<()> {
    let stored = StoredWithdrawalTarget {
        as_of: target.id.as_of.clone(),
        base_event_id: target.id.base_event_id.clone(),
        nonce: target.nonce,
    };
    fs::write(
        paths.last_withdrawal_target_path(),
        serde_json::to_vec_pretty(&stored)?,
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            paths.last_withdrawal_target_path().display()
        )
    })
}

fn remove_last_withdrawal_target(paths: &Paths) -> Result<()> {
    let path = paths.last_withdrawal_target_path();
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn withdrawal_id_label(id: &ingress_proto::WithdrawalId) -> String {
    format!(
        "as_of={} base_event={}",
        hex_string(&id.as_of),
        hex_string(&id.base_event_id)
    )
}

fn withdrawal_id_compact_label(id: &ingress_proto::WithdrawalId) -> String {
    format!(
        "{}:{}",
        hex_string(&id.as_of),
        hex_string(&id.base_event_id)
    )
}

fn parse_hex_bytes(value: &str, label: &str) -> Result<Vec<u8>> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    if !trimmed.len().is_multiple_of(2) {
        bail!("{label} hex must have an even number of digits");
    }
    (0..trimmed.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&trimmed[offset..offset + 2], 16)
                .with_context(|| format!("invalid {label} hex"))
        })
        .collect()
}

fn bridge_dev_node_pkhs() -> Result<Vec<NockHash>> {
    BRIDGE_NOCK_PKHS
        .iter()
        .map(|pkh| {
            NockHash::from_base58(pkh).with_context(|| format!("invalid bridge-dev nock pkh {pkh}"))
        })
        .collect()
}

fn withdrawal_handoff_owner(
    id: &ingress_proto::WithdrawalId,
    epoch: u64,
    handoff_index: u64,
) -> Result<String> {
    let withdrawal_id = withdrawal_id_from_proto(id)
        .map_err(|err| anyhow!("invalid withdrawal id from sequencer: {err}"))?;
    let node_pkhs = bridge_dev_node_pkhs()?;
    let node_id = withdrawal_turn_proposer(&withdrawal_id, epoch, handoff_index, &node_pkhs);
    Ok(format!("bridge-{node_id}"))
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_nock_lock_root_hex(raw: &str) -> Result<NockHash> {
    let bytes = parse_fixed_hex::<40>(raw, "40-byte Nockchain lock root")?;
    NockHash::from_be_limb_bytes(&bytes)
        .map_err(|err| anyhow!("invalid 40-byte Nockchain lock root: {err}"))
}

fn parse_fixed_hex<const N: usize>(raw: &str, label: &str) -> Result<[u8; N]> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if hex.len() != N * 2 {
        bail!("expected {label} hex value, got {} hex chars", hex.len());
    }
    let mut out = [0u8; N];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk)
            .with_context(|| format!("{label} hex must be valid utf-8"))?;
        out[index] = u8::from_str_radix(pair, 16)
            .with_context(|| format!("invalid hex byte '{pair}' in {label} value"))?;
    }
    Ok(out)
}

fn run_preflight_checks(profile: &ResolvedProfile, env: &GeneratedEnv) -> Result<()> {
    let mut errors = Vec::new();
    for key in [
        "BASE_RPC_URL", "BASE_WS_URL", "BASE_START_HEIGHT", "INBOX_CONTRACT_ADDRESS",
        "NOCK_CONTRACT_ADDRESS",
    ] {
        if let Err(err) = env.require(key) {
            errors.push(err.to_string());
        }
    }
    if let Some(raw_height) = env.optional("BASE_START_HEIGHT") {
        if raw_height.parse::<u64>().is_err() {
            errors.push(format!(
                "invalid BASE_START_HEIGHT in generated env: {raw_height}"
            ));
        }
    }

    for (node_id, (eth_key, expected_addr)) in BRIDGE_ETH_KEYS
        .iter()
        .zip(BRIDGE_ETH_ADDRS.iter())
        .enumerate()
    {
        match BridgeSigner::new((*eth_key).to_string()) {
            Ok(signer) => {
                let actual = signer.address().to_string();
                if !actual.eq_ignore_ascii_case(expected_addr) {
                    errors.push(format!(
                        "bridge-{node_id} ethereum key derives {actual}, expected {expected_addr}"
                    ));
                }
            }
            Err(err) => errors.push(format!("bridge-{node_id} invalid ethereum key: {err}")),
        }
    }

    for (node_id, (nock_key, expected_pkh)) in BRIDGE_NOCK_KEYS
        .iter()
        .zip(BRIDGE_NOCK_PKHS.iter())
        .enumerate()
    {
        match derive_nock_pkh(nock_key) {
            Ok(actual) if actual == *expected_pkh => {}
            Ok(actual) => errors.push(format!(
                "bridge-{node_id} nock key derives {actual}, expected {expected_pkh}"
            )),
            Err(err) => errors.push(format!("bridge-{node_id} invalid nock key: {err}")),
        }
    }

    if profile.aliases.withdraw_dest.lock_root_base58.is_empty() {
        errors.push("withdraw destination lock root resolved to an empty value".to_string());
    }

    if errors.is_empty() {
        return Ok(());
    }

    for error in &errors {
        eprintln!("preflight error: {error}");
    }
    bail!("bridge-dev preflight failed with {} error(s)", errors.len())
}

fn derive_nock_pkh(secret_key_base58: &str) -> Result<String> {
    let secret_key_bytes = bs58::decode(secret_key_base58)
        .into_vec()
        .with_context(|| format!("invalid base58 nock key {secret_key_base58}"))?;
    let secret_scalar = UBig::from_be_bytes(&secret_key_bytes);
    let pubkey = SchnorrPubkey(ch_scal_big(&secret_scalar, &A_GEN).map_err(|err| {
        anyhow!("failed to derive schnorr pubkey from {secret_key_base58}: {err}")
    })?);
    let pkh = pubkey
        .pkh_hash()
        .map_err(|err| anyhow!("failed to hash schnorr pubkey for {secret_key_base58}: {err}"))?;
    Ok(pkh.to_base58())
}

fn nock_hash_to_limb_hex(hash: &NockHash) -> String {
    format!("0x{}", hex_string(&hash.to_be_limb_bytes()))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use tempfile::TempDir;

    use super::*;

    fn derive_bridge_nock_pkhs() -> Result<Vec<String>> {
        BRIDGE_NOCK_KEYS
            .iter()
            .map(|secret_key| derive_nock_pkh(secret_key))
            .collect()
    }

    fn parse_up_args(args: &[&str]) -> UpArgs {
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Up(args) => args,
            command => panic!("expected up command, got {command:?}"),
        }
    }

    fn parse_status_args(args: &[&str]) -> StatusArgs {
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Status(args) => args,
            command => panic!("expected status command, got {command:?}"),
        }
    }

    fn parse_component_target_args(args: &[&str]) -> ComponentTargetArgs {
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Stop(args) | Commands::Start(args) | Commands::Restart(args) => args,
            command => panic!("expected component command, got {command:?}"),
        }
    }

    #[test]
    fn bridge_dev_default_journal_key_derives_dedicated_verifier() {
        let verifier = bridge_dev_sequencer_journal_verifier_address_for_key(
            BRIDGE_DEV_DEFAULT_SEQUENCER_JOURNAL_SIGNING_KEY,
        )
        .unwrap();

        assert!(verifier.starts_with("0x"));
        assert_eq!(verifier.len(), 42);
        assert!(
            !BRIDGE_ETH_ADDRS
                .iter()
                .any(|bridge_addr| bridge_addr.eq_ignore_ascii_case(&verifier)),
            "bridge-dev journal key must stay separate from bridge node signing keys"
        );
    }

    #[test]
    fn bridge_dev_journal_envs_sets_sequencer_signing_key_when_enabled() {
        let envs = bridge_dev_sequencer_journal_envs_for_key(
            &SequencerJournalConfigToml {
                enabled: true,
                ..SequencerJournalConfigToml::default()
            },
            BRIDGE_DEV_DEFAULT_SEQUENCER_JOURNAL_SIGNING_KEY.to_string(),
        );

        assert_eq!(
            envs,
            vec![(
                "WITHDRAWAL_SEQUENCER_JOURNAL_SIGNING_KEY",
                BRIDGE_DEV_DEFAULT_SEQUENCER_JOURNAL_SIGNING_KEY.to_string()
            )]
        );
    }

    fn test_paths(tempdir: &TempDir) -> Paths {
        let workspace_root = tempdir.path().join("workspace");
        let crates_dir = workspace_root.join("open/crates");
        let bridge_dir = crates_dir.join("bridge");
        let test_data_dir = bridge_dir.join("test_run_data");
        let current_dir = test_data_dir.join("bridge-dev/current");
        Paths {
            workspace_root: workspace_root.clone(),
            crates_dir,
            bridge_dir: bridge_dir.clone(),
            test_data_dir,
            current_dir: current_dir.clone(),
            manifest_path: current_dir.join("manifest.json"),
            control_socket: current_dir.join("control.sock"),
            supervisor_log: current_dir.join("supervisor.log"),
            env_file: bridge_dir.join("scripts/environments/virtual-testnet.generated.env"),
            deploy_script: bridge_dir.join("scripts/tenderly-vnet-deploy.sh"),
            cleanup_script: bridge_dir.join("scripts/tenderly-vnet-cleanup.sh"),
            advance_blocks_script: bridge_dir.join("scripts/tenderly-advance-blocks.sh"),
            deposit_script: bridge_dir.join("scripts/create-bridge-spend.sh"),
            bin_dir: workspace_root.join("target/release"),
        }
    }

    fn create_state_dirs(paths: &Paths) {
        for path in [
            paths.node_dir(),
            paths.wallet_dir(),
            paths.bridge_data_dir(0),
            paths.bridge_data_dir(1),
            paths.bridge_data_dir(2),
            paths.bridge_data_dir(3),
            paths.bridge_data_dir(4),
            paths.bridge_config_dir(),
            paths.current_dir.clone(),
        ] {
            fs::create_dir_all(path).unwrap();
        }
    }

    #[test]
    fn parses_nock_amount_units() {
        assert_eq!(parse_nock_amount_units("1").unwrap(), 10_u128.pow(16));
        assert_eq!(
            parse_nock_amount_units("1.5").unwrap(),
            15_000_000_000_000_000
        );
        assert_eq!(parse_nock_amount_units("0.0000000000000001").unwrap(), 1);
    }

    #[test]
    fn parses_nock_amount_to_nicks() {
        assert_eq!(parse_nock_amount_to_nicks("1").unwrap(), 65_536);
        assert_eq!(parse_nock_amount_to_nicks("0.0000152587890625").unwrap(), 1);
        assert!(parse_nock_amount_to_nicks("1.0000000000000001").is_err());
    }

    #[test]
    fn bridge_dev_profile_uses_one_block_nock_confirmation_depth() {
        let profile: ProfileFile = toml::from_str(include_str!(
            "../../bridge/scripts/environments/bridge-dev.toml"
        ))
        .unwrap();
        assert_eq!(profile.cluster.base_confirmation_depth, 0);
        assert_eq!(
            bridge_dev_sequencer_base_confirmation_depth(profile.cluster.base_confirmation_depth),
            1
        );
        assert_eq!(profile.cluster.nockchain_confirmation_depth, 1);
    }

    #[test]
    fn derives_deposit_minimum_from_bridge_constants() {
        let config = BridgeConfigToml {
            node_id: 0,
            base_ws_url: "ws://example".to_string(),
            bridge_lock_root: "lock-root".to_string(),
            inbox_contract_address: None,
            nock_contract_address: None,
            my_eth_key: "eth-key".to_string(),
            my_nock_key: "nock-key".to_string(),
            grpc_address: "127.0.0.1:5000".to_string(),
            nockchain_sequencer_api_address: None,
            base_confirmation_depth: 0,
            nockchain_confirmation_depth: 0,
            deposit_nonce_epoch_base: None,
            deposit_nonce_epoch_start_height: None,
            deposit_nonce_epoch_start_tx_id_base58: None,
            withdrawal_activation_nock_next_height: Some(1),
            ingress_listen_address: None,
            nodes: Vec::new(),
            constants: Some(BridgeConstantsToml {
                minimum_event_nocks: 1_000,
                ..BridgeConstantsToml::default()
            }),
        };

        assert_eq!(minimum_event_nicks(&config).unwrap(), 65_536_000);
    }

    #[test]
    fn withdrawal_floor_requires_more_than_minimum_event_nocks() {
        let config = BridgeConfigToml {
            node_id: 0,
            base_ws_url: "ws://example".to_string(),
            bridge_lock_root: "lock-root".to_string(),
            inbox_contract_address: None,
            nock_contract_address: None,
            my_eth_key: "eth-key".to_string(),
            my_nock_key: "nock-key".to_string(),
            grpc_address: "127.0.0.1:5000".to_string(),
            nockchain_sequencer_api_address: None,
            base_confirmation_depth: 0,
            nockchain_confirmation_depth: 0,
            deposit_nonce_epoch_base: None,
            deposit_nonce_epoch_start_height: None,
            deposit_nonce_epoch_start_tx_id_base58: None,
            withdrawal_activation_nock_next_height: Some(1),
            ingress_listen_address: None,
            nodes: Vec::new(),
            constants: Some(BridgeConstantsToml {
                minimum_event_nocks: 1_000,
                ..BridgeConstantsToml::default()
            }),
        };

        let err = ensure_withdrawal_amount_meets_floor(65_536_000, &config).unwrap_err();
        assert!(err
            .to_string()
            .contains("must be greater than bridge minimum event size"));

        ensure_withdrawal_amount_meets_floor(65_536_001, &config).unwrap();
    }

    #[test]
    fn mint_for_burn_target_requires_tenderly_rpc() {
        let env = GeneratedEnv {
            values: BTreeMap::from([
                ("TENDERLY_VNET_ID".to_string(), "vnet-1".to_string()),
                ("BASE_RPC_URL".to_string(), "https://example".to_string()),
            ]),
        };
        let err = ensure_mint_for_burn_target(&env, "https://example").unwrap_err();
        assert!(err
            .to_string()
            .contains("mint-for-burn refuses to use non-Tenderly RPC URL"));
    }

    #[test]
    fn resolve_mint_for_burn_owner_key_prefers_bridge_dev_override() {
        let owner_key = resolve_mint_for_burn_owner_key(Some("  0xabc  "), Some("0xdef")).unwrap();
        assert_eq!(owner_key, "0xabc");
    }

    #[test]
    fn resolve_mint_for_burn_owner_key_accepts_tenderly_test_key() {
        let owner_key = resolve_mint_for_burn_owner_key(None, Some("  0xdef  ")).unwrap();
        assert_eq!(owner_key, "0xdef");
    }

    #[test]
    fn resolve_mint_for_burn_owner_key_requires_env_var() {
        let err = resolve_mint_for_burn_owner_key(None, Some("   ")).unwrap_err();
        assert!(err.to_string().contains(
            "mint-for-burn requires BRIDGE_DEV_OWNER_PRIVATE_KEY or TENDERLY_TEST_PRIVATE_KEY"
        ));
    }

    #[test]
    fn resolves_explicit_full_withdraw_lock_root_hex() {
        let lock_root_hex = format!("0x{}", "11".repeat(40));
        let resolved = resolve_withdraw_dest(WithdrawDestConfig {
            nockchain_address: None,
            lock_root: None,
            lock_root_hex: Some(lock_root_hex.clone()),
            lock_root_bytes32_hex: None,
        })
        .unwrap();
        let expected_lock_root = parse_nock_lock_root_hex(&lock_root_hex).unwrap();
        assert!(resolved.lock_root_base58.len() > 10);
        assert_eq!(
            resolved.lock_root_hex,
            nock_hash_to_limb_hex(&expected_lock_root)
        );
        assert_eq!(resolved.lock_root_hex.len(), 82);
    }

    #[test]
    fn rejects_legacy_withdraw_lock_root_bytes32_hex() {
        let err = resolve_withdraw_dest(WithdrawDestConfig {
            nockchain_address: None,
            lock_root: None,
            lock_root_hex: None,
            lock_root_bytes32_hex: Some(
                "0x1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            ),
        })
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("lock_root_bytes32_hex is no longer supported"));
    }

    #[test]
    fn resolves_address_derived_full_withdraw_lock_root() {
        let resolved = resolve_withdraw_dest(WithdrawDestConfig {
            nockchain_address: Some(
                "9phXGACnW4238oqgvn2gpwaUjG3RAqcxq2Ash2vaKp8KjzSd3MQ56Jt".to_string(),
            ),
            lock_root: None,
            lock_root_hex: None,
            lock_root_bytes32_hex: None,
        })
        .unwrap();
        let lock_root = NockHash::from_base58(&resolved.lock_root_base58).unwrap();
        assert_eq!(resolved.lock_root_hex, nock_hash_to_limb_hex(&lock_root));
        assert_eq!(resolved.lock_root_hex.len(), 82);
    }

    #[test]
    fn parses_generated_env_lines() {
        let path = std::env::temp_dir().join(format!("bridge-dev-env-{}.env", unix_now()));
        fs::write(
            &path, "export BASE_RPC_URL=\"https://example\"\nexport TENDERLY_VNET_ID=vnet-1\n",
        )
        .unwrap();
        let env = load_generated_env(&path).unwrap();
        assert_eq!(env.require("BASE_RPC_URL").unwrap(), "https://example");
        assert_eq!(env.require("TENDERLY_VNET_ID").unwrap(), "vnet-1");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_test_data_dir_stays_in_bridge_tree() {
        let bridge_dir = PathBuf::from("/workspace/open/crates/bridge");

        assert_eq!(
            resolve_test_data_dir(&bridge_dir, None),
            bridge_dir.join("test_run_data")
        );
        assert_eq!(
            resolve_generated_env_path(&bridge_dir, &bridge_dir.join("test_run_data"), false,),
            bridge_dir.join("scripts/environments/virtual-testnet.generated.env")
        );
    }

    #[test]
    fn overridden_test_data_dir_owns_generated_env_file() {
        let bridge_dir = PathBuf::from("/workspace/open/crates/bridge");
        let run_root = PathBuf::from("/tmp/bridge-dev-e2e");

        assert_eq!(
            resolve_test_data_dir(&bridge_dir, Some(&run_root)),
            run_root
        );
        assert_eq!(
            resolve_generated_env_path(&bridge_dir, &run_root, true),
            run_root.join("virtual-testnet.generated.env")
        );
    }

    #[test]
    fn source_layout_resolves_monorepo_open_layout() {
        let tempdir = TempDir::new().unwrap();
        let workspace = tempdir.path().join("workspace");
        let manifest_dir = workspace.join("open/crates/bridge-dev");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(workspace.join("Cargo.toml"), "[workspace]\n").unwrap();

        let layout = SourceLayout::discover_from_manifest_dir(&manifest_dir).unwrap();

        assert_eq!(layout.workspace_root, workspace);
        assert_eq!(
            layout.crates_dir,
            tempdir.path().join("workspace/open/crates")
        );
        assert_eq!(
            layout.bridge_dir,
            tempdir.path().join("workspace/open/crates/bridge")
        );
    }

    #[test]
    fn source_layout_resolves_public_repo_layout() {
        let tempdir = TempDir::new().unwrap();
        let workspace = tempdir.path().join("nockchain");
        let manifest_dir = workspace.join("crates/bridge-dev");
        fs::create_dir_all(&manifest_dir).unwrap();

        let layout = SourceLayout::discover_from_manifest_dir(&manifest_dir).unwrap();

        assert_eq!(layout.workspace_root, workspace);
        assert_eq!(layout.crates_dir, tempdir.path().join("nockchain/crates"));
        assert_eq!(
            layout.bridge_dir,
            tempdir.path().join("nockchain/crates/bridge")
        );
    }

    #[test]
    fn source_layout_keeps_public_repo_named_open_as_workspace() {
        let tempdir = TempDir::new().unwrap();
        let workspace = tempdir.path().join("open");
        let manifest_dir = workspace.join("crates/bridge-dev");
        fs::create_dir_all(&manifest_dir).unwrap();

        let layout = SourceLayout::discover_from_manifest_dir(&manifest_dir).unwrap();

        assert_eq!(layout.workspace_root, workspace);
        assert_eq!(layout.crates_dir, tempdir.path().join("open/crates"));
        assert_eq!(layout.bridge_dir, tempdir.path().join("open/crates/bridge"));
    }

    #[test]
    fn port_offset_helpers_offset_every_runtime_endpoint() {
        let offset = 40;

        assert_eq!(
            offset_port_with_offset("node bind", NODE_BIND_PORT, offset).unwrap(),
            3045
        );
        assert_eq!(
            offset_port_with_offset("node public gRPC", NODE_PUBLIC_GRPC_PORT, offset).unwrap(),
            5041
        );
        assert_eq!(
            offset_port_with_offset("node private gRPC", NODE_PRIVATE_GRPC_PORT, offset).unwrap(),
            5042
        );
        assert_eq!(bridge_ingress_port_with_offset(0, offset).unwrap(), 8042);
        assert_eq!(bridge_ingress_port_with_offset(4, offset).unwrap(), 8046);
        assert_eq!(sequencer_api_port_with_offset(offset).unwrap(), 5142);
    }

    #[test]
    fn port_offset_helpers_reject_invalid_values() {
        assert!(parse_bridge_dev_port_offset(Some("not-a-number")).is_err());
        assert!(offset_port_with_offset("node bind", u16::MAX, 1).is_err());
        assert!(bridge_ingress_port_with_offset(5, 0).is_err());
    }

    #[test]
    fn activation_height_overrides_parse_optional_u64s() {
        assert_eq!(
            parse_optional_u64_env_value(
                BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT_ENV,
                Some("42"),
            )
            .unwrap(),
            Some(42)
        );
        assert_eq!(
            parse_optional_u64_env_value(BRIDGE_DEV_BASE_BLOCKS_CHUNK_ENV, Some("10")).unwrap(),
            Some(10)
        );
        assert!(parse_optional_u64_env_value(
            BRIDGE_DEV_WITHDRAWAL_ACTIVATION_NOCK_NEXT_HEIGHT_ENV,
            Some("bad"),
        )
        .is_err());
    }

    #[test]
    fn parses_bridge_save_interval_override_as_millis() {
        assert_eq!(
            parse_optional_millis_env_value(BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV, None)
                .unwrap(),
            None
        );
        assert_eq!(
            parse_optional_millis_env_value(
                BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV,
                Some("1000"),
            )
            .unwrap(),
            Some(1000)
        );
        assert!(parse_optional_millis_env_value(
            BRIDGE_DEV_BRIDGE_SAVE_INTERVAL_MILLIS_ENV,
            Some("soon"),
        )
        .is_err());
    }

    #[test]
    fn converts_bridge_save_interval_to_snapshot_event_time_seconds() {
        assert_eq!(
            parse_bridge_save_interval_event_time_secs(None).unwrap(),
            None
        );
        assert_eq!(
            parse_bridge_save_interval_event_time_secs(Some("1")).unwrap(),
            Some(1)
        );
        assert_eq!(
            parse_bridge_save_interval_event_time_secs(Some("1000")).unwrap(),
            Some(1)
        );
        assert_eq!(
            parse_bridge_save_interval_event_time_secs(Some("1001")).unwrap(),
            Some(2)
        );
    }

    #[test]
    fn fakenet_genesis_override_resolves_relative_to_workspace() {
        let workspace = PathBuf::from("/workspace");
        let crates_dir = PathBuf::from("/workspace/crates");

        assert_eq!(
            resolve_fakenet_genesis_jam_path(&workspace, &crates_dir, None).unwrap(),
            crates_dir.join(FAKENET_GENESIS_JAM_RELATIVE_TO_CRATES)
        );
        assert_eq!(
            resolve_fakenet_genesis_jam_path(
                &workspace,
                &crates_dir,
                Some("open/crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam"),
            )
            .unwrap(),
            workspace.join("open/crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam")
        );
        assert_eq!(
            resolve_fakenet_genesis_jam_path(&workspace, &crates_dir, Some("/tmp/genesis.jam"))
                .unwrap(),
            PathBuf::from("/tmp/genesis.jam")
        );
    }

    #[test]
    fn derives_real_bridge_nock_pkhs() {
        let pkhs = derive_bridge_nock_pkhs().unwrap();
        assert_eq!(
            pkhs,
            BRIDGE_NOCK_PKHS
                .iter()
                .map(|pkh| pkh.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn bridge_dev_testing_lock_root_matches_canonical_testing_root() {
        assert_eq!(
            derive_bridge_dev_lock_root().unwrap(),
            canonical_testing_bridge_lock_root().unwrap().to_base58()
        );
    }

    #[test]
    fn parses_component_targets() {
        assert_eq!(
            ComponentTarget::parse("node").unwrap(),
            ComponentTarget::Node
        );
        assert_eq!(
            ComponentTarget::parse("bridge-4").unwrap(),
            ComponentTarget::Bridge(4)
        );
        assert!(ComponentTarget::parse("bridge-5").is_err());
    }

    #[test]
    fn parses_multiple_component_action_targets() {
        let args = parse_component_target_args(&["bridge-dev", "stop", "bridge-0", "bridge-2"]);

        assert_eq!(args.targets, vec!["bridge-0", "bridge-2"]);
    }

    #[test]
    fn component_action_requests_fan_out_to_single_target_requests() {
        let targets = vec!["bridge-0".to_string(), "bridge-2".to_string()];

        let (requests, failures) = component_action_requests(&targets, ComponentAction::Stop);

        assert!(failures.is_empty());
        assert_eq!(
            requests,
            vec![
                (
                    "bridge-0".to_string(),
                    ControlRequest::Stop {
                        target: "bridge-0".to_string()
                    }
                ),
                (
                    "bridge-2".to_string(),
                    ControlRequest::Stop {
                        target: "bridge-2".to_string()
                    }
                ),
            ]
        );
    }

    #[test]
    fn component_action_requests_preserve_valid_targets_when_one_is_invalid() {
        let targets = vec!["bridge-0".to_string(), "bridge-9".to_string(), "bridge-1".to_string()];

        let (requests, failures) = component_action_requests(&targets, ComponentAction::Restart);

        assert_eq!(
            requests,
            vec![
                (
                    "bridge-0".to_string(),
                    ControlRequest::Restart {
                        target: "bridge-0".to_string()
                    }
                ),
                (
                    "bridge-1".to_string(),
                    ControlRequest::Restart {
                        target: "bridge-1".to_string()
                    }
                ),
            ]
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("bridge-9"));
    }

    #[test]
    fn derives_withdrawal_handoff_owner_from_epoch_and_handoff_index() {
        let id = ingress_proto::WithdrawalId {
            as_of: NockHash::from_base58("7777777777777777777777777777777777777777777777777777")
                .unwrap()
                .to_be_limb_bytes()
                .to_vec(),
            base_event_id: (0u8..32).collect(),
        };

        let first_owner = withdrawal_handoff_owner(&id, 0, 0).unwrap();
        let second_owner = withdrawal_handoff_owner(&id, 0, 1).unwrap();

        assert!(first_owner.starts_with("bridge-"));
        assert!(second_owner.starts_with("bridge-"));
        assert_ne!(first_owner, second_owner);
    }

    #[test]
    fn parses_split_up_flags() {
        let args = parse_up_args(&["bridge-dev", "up", "--fresh-vnet", "--fresh-state"]);
        assert!(!args.fresh);
        assert!(args.fresh_vnet);
        assert!(args.fresh_state);
        assert!(args.fresh_vnet());
        assert!(args.fresh_state());
    }

    #[test]
    fn parses_legacy_up_flag() {
        let fresh = parse_up_args(&["bridge-dev", "up", "--fresh"]);
        assert!(fresh.fresh);
        assert!(fresh.fresh_vnet());
        assert!(fresh.fresh_state());
    }

    #[test]
    fn parses_up_start_flag() {
        let args = parse_up_args(&["bridge-dev", "up", "--start"]);
        assert!(args.start);
        assert!(!args.fresh);
        assert!(!args.fresh_vnet);
        assert!(!args.fresh_state);
    }

    #[test]
    fn fresh_state_cleanup_preserves_node_dir() {
        let tempdir = TempDir::new().unwrap();
        let paths = test_paths(&tempdir);
        create_state_dirs(&paths);

        paths.remove_local_state_preserving_sequencer().unwrap();

        assert!(paths.node_dir().exists());
        assert!(!paths.wallet_dir().exists());
        assert!(!paths.bridge_data_dir(0).exists());
        assert!(!paths.bridge_config_dir().exists());
        assert!(!paths.current_dir.exists());
    }

    #[test]
    fn fresh_cleanup_removes_node_dir() {
        let tempdir = TempDir::new().unwrap();
        let paths = test_paths(&tempdir);
        create_state_dirs(&paths);

        paths.remove_all_local_state().unwrap();

        assert!(!paths.node_dir().exists());
        assert!(!paths.wallet_dir().exists());
        assert!(!paths.bridge_data_dir(0).exists());
        assert!(!paths.bridge_config_dir().exists());
        assert!(!paths.current_dir.exists());
    }

    #[test]
    fn bridge_configs_are_outside_fresh_kernel_data_dirs() {
        let tempdir = TempDir::new().unwrap();
        let paths = test_paths(&tempdir);

        for node_id in 0..5usize {
            let config_path = paths.bridge_config_path(node_id);
            assert!(
                !config_path.starts_with(paths.bridge_data_dir(node_id)),
                "bridge config path {} must not be inside fresh kernel data dir {}",
                config_path.display(),
                paths.bridge_data_dir(node_id).display()
            );
        }
    }

    #[test]
    fn bridge_runtime_logs_are_outside_fresh_kernel_data_dirs() {
        let tempdir = TempDir::new().unwrap();
        let paths = test_paths(&tempdir);

        for node_id in 0..5usize {
            let log_dir = paths.bridge_runtime_log_dir(node_id);
            assert!(
                !log_dir.starts_with(paths.bridge_data_dir(node_id)),
                "bridge runtime log dir {} must not be inside fresh kernel data dir {}",
                log_dir.display(),
                paths.bridge_data_dir(node_id).display()
            );
        }
    }

    #[test]
    fn parses_status_flags() {
        let args = parse_status_args(&["bridge-dev", "status", "--bridges", "--sequencer"]);
        assert!(args.bridges);
        assert!(args.sequencer);
    }

    #[test]
    fn derives_withdraw_dest_first_name_from_manifest_lock_root() {
        let lock_root = NockHash::from_be_bytes(&[1u8; 32]);
        let aliases = AliasManifest {
            deposit_recipient: "0x0000000000000000000000000000000000000001".to_string(),
            withdraw_holder: "0x0000000000000000000000000000000000000002".to_string(),
            withdraw_dest_nockchain_address: None,
            withdraw_dest_lock_root_base58: lock_root.to_base58(),
            withdraw_dest_lock_root_hex: nock_hash_to_limb_hex(&lock_root),
        };

        assert_eq!(
            aliases.withdraw_dest_first_name().unwrap(),
            FirstName::from_lock_root(&lock_root).unwrap().to_base58()
        );
    }

    #[test]
    fn derives_bridge_multisig_balance_target_from_canonical_lock_root() {
        let target = bridge_multisig_balance_target().unwrap();
        let lock_root = canonical_testing_bridge_lock_root().unwrap();

        assert_eq!(target.lock_root_base58(), lock_root.to_base58());
        assert_eq!(
            target.first_name().unwrap(),
            FirstName::from_lock_root(&lock_root).unwrap().to_base58()
        );
    }

    #[test]
    fn summarizes_wallet_balance_status_in_nocks_and_nicks() {
        let status = WalletBalanceStatus {
            first_name: "first-name".to_string(),
            nicks: 65_537,
            note_count: 2,
            height: Some(9),
        };

        assert_eq!(
            status.summary(),
            "1.0000152587890625 NOCK (65537 nicks, notes=2, height=9, first_name=first-name)"
        );
    }

    #[test]
    fn summarizes_base_nock_balance_in_nockchain_nocks() {
        let status = BaseNockBalanceStatus {
            base_units: U256::from(nicks_to_nock_base_units(65_537).unwrap()),
        };

        assert_eq!(
            status.summary(),
            "1.0000152587890625 NOCK (65537 nicks, 10000152587890625 base units)"
        );
    }

    #[test]
    fn summarizes_unaligned_base_nock_balance_with_base_unit_remainder() {
        let status = BaseNockBalanceStatus {
            base_units: U256::from(NOCK_BASE_PER_NICK + 7),
        };

        assert_eq!(
            status.summary(),
            "0.0000152587890625 NOCK + 7 base units (1 nicks, 152587890632 base units total)"
        );
    }

    #[test]
    fn balance_total_nicks_sums_legacy_and_v1_notes() {
        fn note_with_assets(
            note_version: note::NoteVersion,
        ) -> nockapp_grpc::pb::common::v2::BalanceEntry {
            nockapp_grpc::pb::common::v2::BalanceEntry {
                note: Some(nockapp_grpc::pb::common::v2::Note {
                    note_version: Some(note_version),
                }),
                ..Default::default()
            }
        }

        let balance = nockapp_grpc::pb::common::v2::Balance {
            notes: vec![
                note_with_assets(note::NoteVersion::Legacy(
                    nockapp_grpc::pb::common::v1::Note {
                        assets: Some(nockapp_grpc::pb::common::v1::Nicks { value: 10 }),
                        ..Default::default()
                    },
                )),
                note_with_assets(note::NoteVersion::V1(
                    nockapp_grpc::pb::common::v2::NoteV1 {
                        assets: Some(nockapp_grpc::pb::common::v1::Nicks { value: 32 }),
                        ..Default::default()
                    },
                )),
            ],
            ..Default::default()
        };

        assert_eq!(balance_total_nicks(&balance).unwrap(), 42);
    }

    #[test]
    fn parses_stable_tui_subset() {
        let snapshot = stable_snapshot_from_proto(tui_proto::GetSnapshotResponse {
            running_state: tui_proto::RunningState::Running as i32,
            nock_hold: true,
            base_hold: false,
            nock_hold_height: Some(7),
            base_hold_height: Some(8),
            network_state: Some(tui_proto::NetworkState {
                base: Some(tui_proto::ChainState {
                    height: 101,
                    tip_hash: "0xabc".to_string(),
                    confirmations: 3,
                    is_syncing: false,
                    last_updated_ms: Some(123),
                }),
                nockchain: Some(tui_proto::ChainState {
                    height: 44,
                    tip_hash: "0xdef".to_string(),
                    confirmations: 2,
                    is_syncing: false,
                    last_updated_ms: Some(456),
                }),
                pending_deposits: 2,
                pending_withdrawals: 1,
                unsettled_deposit_count: 3,
                unsettled_withdrawal_count: 4,
                batch_status: Some(tui_proto::BatchStatus {
                    status: Some(tui_proto::batch_status::Status::Submitting(
                        tui_proto::BatchSubmitting { batch_id: 9 },
                    )),
                }),
                is_mainnet: Some(false),
                nockchain_api_status: Some(tui_proto::NockchainApiStatus {
                    state: tui_proto::nockchain_api_status::State::Connected as i32,
                    since_ms: Some(123),
                    attempt: Some(1),
                    last_error: Some("stale".to_string()),
                }),
                base_next_height: Some(102),
                nock_next_height: Some(45),
                degradation_warning: Some("peer degraded".to_string()),
            }),
            deposit_log: Some(tui_proto::DepositLogSnapshot {
                total_count: 0,
                first_epoch_nonce: 0,
                rows: Vec::new(),
            }),
            proposals: Some(tui_proto::ProposalState {
                last_submitted: Some(tui_proto::Proposal {
                    id: "proposal-1".to_string(),
                    proposal_type: "withdrawal".to_string(),
                    description: String::new(),
                    signatures_collected: 2,
                    signatures_required: 3,
                    signers: vec![0, 1],
                    created_at_ms: Some(99),
                    status: tui_proto::ProposalStatus::Submitted as i32,
                    data_hash: "hash".to_string(),
                    submitted_at_block: Some(42),
                    submitted_at_ms: Some(55),
                    tx_hash: Some("0xbeef".to_string()),
                    time_to_submit_ms: Some(77),
                    executed_at_block: None,
                    source_block: Some(41),
                    amount: Some("10".to_string()),
                    recipient: Some("0x1".to_string()),
                    nonce: Some(5),
                    source_tx_id: Some("tx-1".to_string()),
                    current_proposer: Some(2),
                    is_my_turn: false,
                    time_until_takeover_ms: Some(1),
                    failure_reason: None,
                }),
                pending_inbound: Vec::new(),
                history: Vec::new(),
            }),
            peer_statuses: vec![tui_proto::PeerStatus {
                node_id: 2,
                address: "127.0.0.1:8004".to_string(),
                status: tui_proto::PeerHealthStatus::Unreachable as i32,
                error: Some("timeout".to_string()),
                latency_ms: Some(100),
                peer_uptime_ms: Some(200),
                last_updated_ms: Some(300),
            }],
            last_submitted_deposit: Some(tui_proto::LastDeposit {
                tx_id: Some(tui_proto::Base58Hash {
                    value: "deposit-submitted".to_string(),
                }),
                name_first: None,
                name_last: None,
                recipient: Some(tui_proto::EthAddress {
                    value: "0x2".to_string(),
                }),
                amount: 11,
                block_height: 12,
                as_of: None,
                nonce: 13,
                base_tx_hash: "0xabc".to_string(),
                base_block_number: 14,
            }),
            last_successful_deposit: Some(tui_proto::SuccessfulDeposit {
                tx_id: Some(tui_proto::Base58Hash {
                    value: "deposit-success".to_string(),
                }),
                name_first: None,
                name_last: None,
                recipient: Some(tui_proto::EthAddress {
                    value: "0x3".to_string(),
                }),
                amount: 21,
                block_height: 22,
                as_of: None,
                nonce: 23,
            }),
            alerts: Some(tui_proto::AlertsSnapshot { alerts: Vec::new() }),
            metrics: Some(tui_proto::MetricsState {
                total_deposited: String::new(),
                total_withdrawn: String::new(),
                hourly_tx_counts: Vec::new(),
                avg_latency_secs: 0.0,
                success_rate: 0.0,
                total_fees: String::new(),
                tx_count: 0,
                latency_sum_ms: 0,
                latency_count: 0,
            }),
            transactions: Some(tui_proto::TransactionState {
                transactions: Vec::new(),
                max_transactions: 0,
            }),
            // TODO(withdrawals): Add stable withdrawal fields once bridge-dev
            // scenarios assert frontier/cache/lifecycle details directly.
            withdrawals: Some(tui_proto::WithdrawalStateSnapshot {
                local: Some(tui_proto::WithdrawalLocalSnapshot {
                    activation_ready: Some(true),
                    activation_base_next_height: Some(102),
                    activation_nock_next_height: Some(45),
                    current_base_next_height: Some(102),
                    current_nock_next_height: Some(45),
                    current_base_height: Some(101),
                    frontier_row: Some(tui_proto::WithdrawalQueueRow {
                        nonce: 5,
                        id: "withdrawal-5".to_string(),
                        state: "peer_canonical".to_string(),
                        epoch: 2,
                        amount: Some(42),
                        recipient: Some("29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV2".to_string()),
                        base_batch_end: Some(100),
                        proposal_hash: Some("proposal-hash-5".to_string()),
                        has_commit_certificate: true,
                        has_authorized_transaction: false,
                        has_submitted_transaction: false,
                        turn_started_base_height: Some(96),
                        handoff_index: 1,
                        current_responsible_node: Some(2),
                        is_my_turn: false,
                        blocks_until_handoff: Some(7),
                        blocks_until_retry: None,
                        updated_at_secs: Some(1_700_000_000),
                    }),
                    queue: vec![tui_proto::WithdrawalQueueRow {
                        nonce: 6,
                        id: "withdrawal-6".to_string(),
                        state: "pending".to_string(),
                        epoch: 2,
                        amount: Some(21),
                        recipient: Some("2t8hWkRMMWST6pDfkdE4p1pvtwS5jvBzHhnnGkXK2P22".to_string()),
                        base_batch_end: Some(101),
                        proposal_hash: None,
                        has_commit_certificate: false,
                        has_authorized_transaction: false,
                        has_submitted_transaction: false,
                        turn_started_base_height: None,
                        handoff_index: 0,
                        current_responsible_node: None,
                        is_my_turn: false,
                        blocks_until_handoff: None,
                        blocks_until_retry: None,
                        updated_at_secs: Some(1_700_000_010),
                    }],
                    cache: Some(tui_proto::WithdrawalCacheSummary {
                        proposal_count: 1,
                        signature_count: 2,
                    }),
                    lifecycle: Some(tui_proto::WithdrawalLifecycleCounts {
                        total_count: 2,
                        live_count: 2,
                        ordering_blocking_count: 1,
                        pending_count: 1,
                        assembling_count: 0,
                        prepared_count: 0,
                        peer_canonical_count: 1,
                        authorized_count: 0,
                        mempool_accepted_count: 0,
                        confirmed_count: 0,
                        below_frontier_count: 0,
                        above_frontier_count: 1,
                    }),
                    last_error: None,
                }),
                sequencer: Some(tui_proto::WithdrawalSequencerSnapshot {
                    frontier_status: tui_proto::WithdrawalFrontierStatus::Present as i32,
                    frontier_nonce: Some(5),
                    frontier_state: Some("peer_canonical".to_string()),
                    frontier_epoch: Some(2),
                    current_confirmed_base_height: Some(101),
                    handoff_window_blocks: Some(32),
                    turn_started_base_height: Some(96),
                    handoff_index: 1,
                    current_responsible_node: Some(2),
                    is_my_turn: false,
                    blocks_until_handoff: Some(7),
                    last_error: None,
                }),
            }),
        })
        .unwrap();

        assert_eq!(snapshot.base_height, 101);
        assert_eq!(snapshot.nock_height, 44);
        assert_eq!(snapshot.pending_withdrawals, 1);
        assert_eq!(snapshot.unhealthy_peer_count(), 1);
        assert_eq!(snapshot.batch_status, "submitting(batch=9)");
        assert_eq!(
            snapshot.last_submitted_proposal.as_ref().unwrap().status,
            tui_proto::ProposalStatus::Submitted
        );
        assert_eq!(
            snapshot
                .last_successful_deposit
                .as_ref()
                .unwrap()
                .tx_id
                .as_deref(),
            Some("deposit-success")
        );
        assert_eq!(
            matching_deposit_for_phase(&snapshot, WaitDepositPhase::Submitted)
                .unwrap()
                .tx_id
                .as_deref(),
            Some("deposit-submitted")
        );
        let mut successful_only = snapshot.clone();
        successful_only.last_submitted_deposit = None;
        assert_eq!(
            matching_deposit_for_phase(&successful_only, WaitDepositPhase::Submitted)
                .unwrap()
                .tx_id
                .as_deref(),
            Some("deposit-success")
        );
        assert_eq!(
            matching_deposit_for_phase_after_nonce(
                &snapshot,
                WaitDepositPhase::Successful,
                Some(22)
            )
            .unwrap()
            .nonce,
            23
        );
        assert!(matching_deposit_for_phase_after_nonce(
            &snapshot,
            WaitDepositPhase::Successful,
            Some(23)
        )
        .is_none());
    }

    #[test]
    fn matches_withdrawal_wait_phases() {
        let pending = WithdrawalProgress {
            id_label: "wd-1".to_string(),
            nonce: Some(1),
            current_epoch: None,
            handoff_index: None,
            handoff_owner: None,
            turn_started_base_height: None,
            proposal_status: Some("persisted".to_string()),
            sequenced_state: Some("pending".to_string()),
            transaction_name: None,
            proposal_hash: None,
            authorized_transaction_name: None,
        };
        let submitted = WithdrawalProgress {
            id_label: "wd-2".to_string(),
            nonce: Some(2),
            current_epoch: None,
            handoff_index: None,
            handoff_owner: None,
            turn_started_base_height: None,
            proposal_status: Some("canonicalized".to_string()),
            sequenced_state: Some("mempool_accepted".to_string()),
            transaction_name: Some("tx-1".to_string()),
            proposal_hash: None,
            authorized_transaction_name: None,
        };

        assert!(withdrawal_phase_satisfied(
            WaitWithdrawalPhase::Pending,
            &pending
        ));
        assert!(withdrawal_phase_satisfied(
            WaitWithdrawalPhase::Pending,
            &submitted
        ));
        assert!(withdrawal_phase_satisfied(
            WaitWithdrawalPhase::Ready,
            &submitted
        ));
        assert!(withdrawal_phase_satisfied(
            WaitWithdrawalPhase::Submitted,
            &submitted
        ));
        assert!(!withdrawal_phase_satisfied(
            WaitWithdrawalPhase::Executed,
            &submitted
        ));
    }
}
