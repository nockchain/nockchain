use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;
use std::{fmt, fs};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BRIDGE_BIN_ENV: &str = "BRIDGE_E2E_BRIDGE_BIN";
pub const NODE_BIN_ENV: &str = "BRIDGE_E2E_NODE_BIN";
pub const MINER_BIN_ENV: &str = "BRIDGE_E2E_MINER_BIN";
pub const WALLET_BIN_ENV: &str = "BRIDGE_E2E_WALLET_BIN";
pub const CTL_BIN_ENV: &str = "BRIDGE_E2E_CTL_BIN";
pub const BRIDGE_JAM_ENV: &str = "BRIDGE_E2E_BRIDGE_JAM";
pub const ROSWELL_JAM_ENV: &str = "BRIDGE_E2E_ROSWELL_JAM";
pub const FAKENET_GENESIS_ENV: &str = "BRIDGE_E2E_FAKENET_GENESIS_JAM";

const DEFAULT_BUILD_PROGRAM: &str = "bazel";
const DEFAULT_BUILD_ARGS: &[&str] = &[
    "build", "//crates/bridge:bridge-bin",
    "//crates/nockchain-bridge-sequencer:nockchain-bridge-sequencer",
    "//crates/nockchain-bridge-sequencer:nockchain-bridge-sequencer-ctl", "//assets:bridge",
    "//assets:dumb", "//assets:miner", "//assets:wal", "//assets:roswell",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    BridgeBinary,
    MinerBinary,
    NodeBinary,
    WalletBinary,
    SequencerCtlBinary,
    BridgeJam,
    RoswellJam,
    FakenetGenesisJam,
}

impl ArtifactRole {
    fn label(self) -> &'static str {
        match self {
            Self::BridgeBinary => "bridge binary",
            Self::NodeBinary => "sequencer/node binary",
            Self::MinerBinary => "miner binary",
            Self::WalletBinary => "wallet binary",
            Self::SequencerCtlBinary => "sequencer ctl binary",
            Self::BridgeJam => "bridge jam",
            Self::RoswellJam => "roswell jam",
            Self::FakenetGenesisJam => "fakenet genesis jam",
        }
    }

    fn is_binary(self) -> bool {
        matches!(
            self,
            Self::BridgeBinary
                | Self::NodeBinary
                | Self::MinerBinary
                | Self::WalletBinary
                | Self::SequencerCtlBinary
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryArchitecture {
    Arm64,
    X86_64,
    Universal,
}

impl BinaryArchitecture {
    fn host() -> Option<Self> {
        match std::env::consts::ARCH {
            "aarch64" => Some(Self::Arm64),
            "x86_64" => Some(Self::X86_64),
            _ => None,
        }
    }

    fn supports_host(self) -> bool {
        self == Self::Universal || Self::host().is_none_or(|host| host == self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactFile {
    pub role: ArtifactRole,
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub modified_unix_seconds: Option<u64>,
    pub architecture: Option<BinaryArchitecture>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactBuildMetadata {
    pub package_version: String,
    pub git_revision: Option<String>,
    pub target_arch: String,
    pub target_os: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eArtifacts {
    pub bridge: ArtifactFile,
    pub node: ArtifactFile,
    pub miner: ArtifactFile,
    pub wallet: ArtifactFile,
    pub sequencer_ctl: Option<ArtifactFile>,
    pub bridge_jam: ArtifactFile,
    pub roswell_jam: ArtifactFile,
    pub fakenet_genesis_jam: ArtifactFile,
    pub build: ArtifactBuildMetadata,
}

impl E2eArtifacts {
    pub fn environment_overrides(&self) -> Vec<(String, String)> {
        let mut values = vec![
            env_path(BRIDGE_BIN_ENV, &self.bridge.path),
            env_path(NODE_BIN_ENV, &self.node.path),
            env_path(MINER_BIN_ENV, &self.miner.path),
            env_path(WALLET_BIN_ENV, &self.wallet.path),
            env_path(BRIDGE_JAM_ENV, &self.bridge_jam.path),
            env_path(ROSWELL_JAM_ENV, &self.roswell_jam.path),
            env_path(FAKENET_GENESIS_ENV, &self.fakenet_genesis_jam.path),
        ];
        if let Some(ctl) = &self.sequencer_ctl {
            values.push(env_path(CTL_BIN_ENV, &ctl.path));
        }
        values
    }
}

fn env_path(name: &str, path: &Path) -> (String, String) {
    (name.to_owned(), path.display().to_string())
}

#[derive(Debug, Clone, Default)]
pub struct ArtifactOverrides {
    pub bridge: Option<PathBuf>,
    pub miner: Option<PathBuf>,
    pub node: Option<PathBuf>,
    pub wallet: Option<PathBuf>,
    pub sequencer_ctl: Option<PathBuf>,
    pub bridge_jam: Option<PathBuf>,
    pub roswell_jam: Option<PathBuf>,
    pub fakenet_genesis_jam: Option<PathBuf>,
}

impl ArtifactOverrides {
    pub fn from_env() -> Self {
        Self {
            bridge: env_path_override(BRIDGE_BIN_ENV),
            miner: env_path_override(MINER_BIN_ENV),
            node: env_path_override(NODE_BIN_ENV),
            wallet: env_path_override(WALLET_BIN_ENV),
            sequencer_ctl: env_path_override(CTL_BIN_ENV),
            bridge_jam: env_path_override(BRIDGE_JAM_ENV),
            roswell_jam: env_path_override(ROSWELL_JAM_ENV),
            fakenet_genesis_jam: env_path_override(FAKENET_GENESIS_ENV),
        }
    }
}

fn env_path_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Clone)]
pub struct ArtifactBuildCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl Default for ArtifactBuildCommand {
    fn default() -> Self {
        Self {
            program: PathBuf::from(DEFAULT_BUILD_PROGRAM),
            args: DEFAULT_BUILD_ARGS.iter().map(OsString::from).collect(),
            env: Vec::new(),
        }
    }
}

impl ArtifactBuildCommand {
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(OsStr::to_string_lossy)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactResolveOptions {
    pub workspace_root: PathBuf,
    pub overrides: ArtifactOverrides,
    pub require_ctl: bool,
    pub build: bool,
    pub build_command: ArtifactBuildCommand,
    pub supplemental_build_commands: Vec<ArtifactBuildCommand>,
}

impl ArtifactResolveOptions {
    pub fn new(workspace_root: PathBuf) -> Self {
        let source_dumb_jam = workspace_root.join("assets/dumb.jam");
        let dumb_jam = if source_dumb_jam.is_file() {
            source_dumb_jam
        } else {
            workspace_root.join("bazel-bin/assets/dumb.jam")
        };
        let source_wallet_jam = workspace_root.join("assets/wal.jam");
        let wallet_jam = if source_wallet_jam.is_file() {
            source_wallet_jam
        } else {
            workspace_root.join("bazel-bin/assets/wal.jam")
        };
        let source_miner_jam = workspace_root.join("assets/miner.jam");
        let miner_jam = if source_miner_jam.is_file() {
            source_miner_jam
        } else {
            workspace_root.join("bazel-bin/assets/miner.jam")
        };
        Self {
            workspace_root,
            overrides: ArtifactOverrides::from_env(),
            require_ctl: false,
            build: false,
            build_command: ArtifactBuildCommand::default(),
            supplemental_build_commands: vec![
                ArtifactBuildCommand {
                    program: PathBuf::from("cargo"),
                    args: [
                        "build", "--release", "-p", "nockchain-bridge-sequencer", "--bin",
                        "nockchain-bridge-sequencer",
                    ]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                    env: vec![(OsString::from("KERNEL_JAM_PATH"), dumb_jam.into_os_string())],
                },
                ArtifactBuildCommand {
                    program: PathBuf::from("cargo"),
                    args: [
                        "build", "--release", "-p", "nockchain-wallet", "--bin", "nockchain-wallet",
                    ]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
                    env: vec![(
                        OsString::from("KERNEL_JAM_PATH"),
                        wallet_jam.into_os_string(),
                    )],
                },
                ArtifactBuildCommand {
                    program: PathBuf::from("cargo"),
                    args: ["build", "--release", "-p", "zk-pow-miner", "--bin", "zk-pow-mine"]
                        .into_iter()
                        .map(OsString::from)
                        .collect(),
                    env: vec![(
                        OsString::from("KERNEL_JAM_PATH"),
                        miner_jam.into_os_string(),
                    )],
                },
            ],
        }
    }
}

pub struct ArtifactResolver;

impl ArtifactResolver {
    pub fn resolve(options: &ArtifactResolveOptions) -> Result<E2eArtifacts, ArtifactResolveError> {
        if options.build {
            run_build(options)?;
            resolve_once(options).map_err(|mut error| {
                error.build_attempted = true;
                error
            })
        } else {
            resolve_once(options)
        }
    }
}

fn resolve_once(options: &ArtifactResolveOptions) -> Result<E2eArtifacts, ArtifactResolveError> {
    let root = &options.workspace_root;
    let mut problems = Vec::new();
    let bridge = resolve_required(
        ArtifactRole::BridgeBinary,
        options.overrides.bridge.as_ref(),
        &[
            root.join("target/release/bridge"),
            root.join("bazel-bin/crates/bridge/bridge-bin"),
        ],
        &mut problems,
    );
    let node = resolve_required(
        ArtifactRole::NodeBinary,
        options.overrides.node.as_ref(),
        &[
            root.join("target/release/nockchain-bridge-sequencer"),
            root.join("bazel-bin/crates/nockchain-bridge-sequencer/nockchain-bridge-sequencer"),
        ],
        &mut problems,
    );
    let miner = resolve_required(
        ArtifactRole::MinerBinary,
        options.overrides.miner.as_ref(),
        &[root.join("target/release/zk-pow-mine")],
        &mut problems,
    );
    let wallet = resolve_required(
        ArtifactRole::WalletBinary,
        options.overrides.wallet.as_ref(),
        &[root.join("target/release/nockchain-wallet")],
        &mut problems,
    );
    let ctl_candidates = [
        root.join("target/release/nockchain-bridge-sequencer-ctl"),
        root.join("bazel-bin/crates/nockchain-bridge-sequencer/nockchain-bridge-sequencer-ctl"),
    ];
    let sequencer_ctl = if options.require_ctl {
        resolve_required(
            ArtifactRole::SequencerCtlBinary,
            options.overrides.sequencer_ctl.as_ref(),
            &ctl_candidates,
            &mut problems,
        )
    } else {
        resolve_optional(
            ArtifactRole::SequencerCtlBinary,
            options.overrides.sequencer_ctl.as_ref(),
            &ctl_candidates,
            &mut problems,
        )
    };
    let bridge_jam = resolve_required(
        ArtifactRole::BridgeJam,
        options.overrides.bridge_jam.as_ref(),
        &[root.join("assets/bridge.jam"), root.join("bazel-bin/assets/bridge.jam")],
        &mut problems,
    );
    let roswell_jam = resolve_required(
        ArtifactRole::RoswellJam,
        options.overrides.roswell_jam.as_ref(),
        &[root.join("assets/roswell.jam"), root.join("bazel-bin/assets/roswell.jam")],
        &mut problems,
    );
    let fakenet_genesis_jam = resolve_required(
        ArtifactRole::FakenetGenesisJam,
        options.overrides.fakenet_genesis_jam.as_ref(),
        &[
            root.join("crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam"),
            root.join("open/crates/nockchain/jams/fakenet-genesis-pow-2-bex-1.jam"),
        ],
        &mut problems,
    );

    if !problems.is_empty() {
        return Err(ArtifactResolveError {
            problems,
            remediation: build_remediation(options),
            build_attempted: false,
            build_failure: None,
        });
    }

    let bridge = require_resolved(bridge, ArtifactRole::BridgeBinary, options)?;
    let node = require_resolved(node, ArtifactRole::NodeBinary, options)?;
    let miner = require_resolved(miner, ArtifactRole::MinerBinary, options)?;
    let wallet = require_resolved(wallet, ArtifactRole::WalletBinary, options)?;
    let bridge_jam = require_resolved(bridge_jam, ArtifactRole::BridgeJam, options)?;
    let roswell_jam = require_resolved(roswell_jam, ArtifactRole::RoswellJam, options)?;
    let fakenet_genesis_jam = require_resolved(
        fakenet_genesis_jam,
        ArtifactRole::FakenetGenesisJam,
        options,
    )?;
    Ok(E2eArtifacts {
        bridge,
        node,
        miner,
        wallet,
        sequencer_ctl,
        bridge_jam,
        roswell_jam,
        fakenet_genesis_jam,
        build: ArtifactBuildMetadata {
            package_version: env!("CARGO_PKG_VERSION").to_owned(),
            git_revision: option_env!("VERGEN_GIT_SHA")
                .or(option_env!("GIT_SHA"))
                .map(ToOwned::to_owned),
            target_arch: std::env::consts::ARCH.to_owned(),
            target_os: std::env::consts::OS.to_owned(),
        },
    })
}

fn require_resolved(
    artifact: Option<ArtifactFile>,
    role: ArtifactRole,
    options: &ArtifactResolveOptions,
) -> Result<ArtifactFile, ArtifactResolveError> {
    artifact.ok_or_else(|| ArtifactResolveError {
        problems: vec![ArtifactProblem {
            role,
            attempted_paths: Vec::new(),
            reasons: vec!["resolver lost a required artifact".to_owned()],
        }],
        remediation: build_remediation(options),
        build_attempted: false,
        build_failure: None,
    })
}

fn resolve_required(
    role: ArtifactRole,
    explicit: Option<&PathBuf>,
    candidates: &[PathBuf],
    problems: &mut Vec<ArtifactProblem>,
) -> Option<ArtifactFile> {
    resolve_candidate(role, explicit, candidates, true, problems)
}

fn resolve_optional(
    role: ArtifactRole,
    explicit: Option<&PathBuf>,
    candidates: &[PathBuf],
    problems: &mut Vec<ArtifactProblem>,
) -> Option<ArtifactFile> {
    resolve_candidate(role, explicit, candidates, explicit.is_some(), problems)
}

fn resolve_candidate(
    role: ArtifactRole,
    explicit: Option<&PathBuf>,
    candidates: &[PathBuf],
    required: bool,
    problems: &mut Vec<ArtifactProblem>,
) -> Option<ArtifactFile> {
    let attempted = explicit
        .map(|path| vec![path.clone()])
        .unwrap_or_else(|| candidates.to_vec());
    let mut invalid = Vec::new();
    for path in &attempted {
        if !path.exists() {
            continue;
        }
        match inspect_artifact(role, path) {
            Ok(artifact) => return Some(artifact),
            Err(reason) => invalid.push(format!("{}: {reason}", path.display())),
        }
    }
    if required || !invalid.is_empty() {
        problems.push(ArtifactProblem {
            role,
            attempted_paths: attempted,
            reasons: if invalid.is_empty() {
                vec!["not found".to_owned()]
            } else {
                invalid
            },
        });
    }
    None
}

fn inspect_artifact(role: ArtifactRole, path: &Path) -> Result<ArtifactFile, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".to_owned());
    }
    if metadata.len() == 0 {
        return Err("file is empty".to_owned());
    }
    if role.is_binary() && !is_executable(&metadata) {
        return Err("binary is not executable".to_owned());
    }
    let architecture = if role.is_binary() {
        let architecture = detect_binary_architecture(path)?;
        if !architecture.supports_host() {
            return Err(format!(
                "binary architecture {architecture:?} does not match host {}",
                std::env::consts::ARCH
            ));
        }
        Some(architecture)
    } else {
        None
    };
    let sha256 = hash_file(path)?;
    let modified_unix_seconds = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    Ok(ArtifactFile {
        role,
        path: path.to_path_buf(),
        sha256,
        size_bytes: metadata.len(),
        modified_unix_seconds,
        architecture,
    })
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn detect_binary_architecture(path: &Path) -> Result<BinaryArchitecture, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0u8; 32];
    file.read_exact(&mut header)
        .map_err(|_| "binary header is too short".to_owned())?;
    if &header[..4] == b"\x7fELF" {
        let machine = match header[5] {
            1 => u16::from_le_bytes([header[18], header[19]]),
            2 => u16::from_be_bytes([header[18], header[19]]),
            _ => return Err("ELF header has unknown byte order".to_owned()),
        };
        return match machine {
            0x3e => Ok(BinaryArchitecture::X86_64),
            0xb7 => Ok(BinaryArchitecture::Arm64),
            _ => Err(format!("unsupported ELF machine 0x{machine:x}")),
        };
    }
    let magic = u32::from_be_bytes(header[..4].try_into().map_err(|_| "invalid magic")?);
    if matches!(magic, 0xcafebabe | 0xcafebabf) {
        return Ok(BinaryArchitecture::Universal);
    }
    let little_magic = u32::from_le_bytes(header[..4].try_into().map_err(|_| "invalid magic")?);
    if little_magic == 0xfeedfacf {
        let cpu = u32::from_le_bytes(header[4..8].try_into().map_err(|_| "invalid cpu")?);
        return match cpu {
            0x0100_0007 => Ok(BinaryArchitecture::X86_64),
            0x0100_000c => Ok(BinaryArchitecture::Arm64),
            _ => Err(format!("unsupported Mach-O cpu 0x{cpu:x}")),
        };
    }
    Err("unrecognized executable format".to_owned())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn build_remediation(options: &ArtifactResolveOptions) -> String {
    std::iter::once(&options.build_command)
        .chain(options.supplemental_build_commands.iter())
        .map(ArtifactBuildCommand::display)
        .collect::<Vec<_>>()
        .join(" && ")
}

fn run_build(options: &ArtifactResolveOptions) -> Result<(), ArtifactResolveError> {
    for command in
        std::iter::once(&options.build_command).chain(options.supplemental_build_commands.iter())
    {
        let output = Command::new(&command.program)
            .args(&command.args)
            .current_dir(&options.workspace_root)
            .stdin(Stdio::null())
            .envs(command.env.iter().cloned())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| ArtifactResolveError {
                problems: Vec::new(),
                remediation: build_remediation(options),
                build_attempted: true,
                build_failure: Some(format!("{}: {error}", command.display())),
            })?;
        if !output.status.success() {
            return Err(ArtifactResolveError {
                problems: Vec::new(),
                remediation: build_remediation(options),
                build_attempted: true,
                build_failure: Some(format!(
                    "{} exited with {}",
                    command.display(),
                    output.status
                )),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactProblem {
    pub role: ArtifactRole,
    pub attempted_paths: Vec<PathBuf>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactResolveError {
    pub problems: Vec<ArtifactProblem>,
    pub remediation: String,
    pub build_attempted: bool,
    pub build_failure: Option<String>,
}

impl fmt::Display for ArtifactResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "bridge E2E artifacts are incomplete or invalid:")?;
        for problem in &self.problems {
            writeln!(formatter, "- {}:", problem.role.label())?;
            for path in &problem.attempted_paths {
                writeln!(formatter, "  attempted {}", path.display())?;
            }
            for reason in &problem.reasons {
                writeln!(formatter, "  {reason}")?;
            }
        }
        if let Some(failure) = &self.build_failure {
            writeln!(formatter, "build attempt failed: {failure}")?;
        }
        write!(formatter, "remediation: {}", self.remediation)
    }
}

impl std::error::Error for ArtifactResolveError {}
