use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser};
use nockapp::kernel::boot::{NockStackSize, PmaSize};

// TODO: command-line/configure
/** Path to read current node's identity from */
pub const IDENTITY_PATH: &str = ".nockchain_identity";

/** Path to read current node's peer ID from */
pub const PEER_ID_EXTENSION: &str = ".peer_id";

// TODO: command-line/configure
/** Extension for peer ID files */
pub const PEER_ID_FILE_EXTENSION: &str = "peerid";

// Libp2p multiaddrs don't support const construction, so we have to put strings literals and parse them at startup
/** Backbone nodes for our testnet */
pub const TESTNET_BACKBONE_NODES: &[&str] = &[];

// Realnet backbone peers now live in `crate::backbone`, which selects a
// round-robin subset of them at startup instead of dialing a single default.

/** How often we should affirmatively ask other nodes for their heaviest chain */
pub const CHAIN_INTERVAL: Duration = Duration::from_secs(20);

/// The height of the bitcoin block that we want to sync our genesis block to
/// Currently, this is the height of an existing block for testing. It will be
/// switched to a future block for launch.
pub const GENESIS_HEIGHT: u64 = 897767;

/// Validated ASERT fakenet override for one puzzle (ZK or AI). Constructible only
/// via `into_config` on the corresponding args struct.
#[derive(Debug, Clone, Copy)]
pub struct FakenetAsertConfig {
    pub phase: u64,
    pub anchor_height: u64,
    pub anchor_target_bex: u64,
    /// Optional per-puzzle ideal block time (seconds). None = keep the default.
    pub ideal_block_time: Option<u64>,
    /// Optional per-puzzle ASERT half-life (seconds). None = keep the default. A
    /// short half-life makes retargeting visible on a short fakenet run.
    pub half_life: Option<u64>,
}

const FAKENET_ASERT_MAX_BEX: u64 = 512;

/// Largest AI ASERT anchor-target exponent that stays minable. See
/// [`AsertPuzzle::max_anchor_target_bex`].
const AI_ASERT_MAX_BEX: u64 = 232;

/// Which puzzle a fakenet ASERT override targets. Every puzzle pins to the
/// final pre-activation block (`anchor_height + 1 == phase`), allowing the
/// shared schedule to recover its timestamp before the puzzle's first block.
#[derive(Clone, Copy)]
enum AsertPuzzle {
    Zk,
    Ai,
}

impl AsertPuzzle {
    /// Flag family used in error text (`--fakenet-{label}-*`).
    fn label(self) -> &'static str {
        match self {
            AsertPuzzle::Zk => "asert",
            AsertPuzzle::Ai => "ai-asert",
        }
    }

    /// The one anchor-height that is valid for `phase`, plus the suffix that
    /// describes the relation in an error message. `None` means no anchor-height
    /// can satisfy this phase (e.g. ZK with `phase == 0`).
    fn expected_anchor_height(self, phase: u64) -> (Option<u64>, &'static str) {
        match self {
            AsertPuzzle::Zk | AsertPuzzle::Ai => (phase.checked_sub(1), " minus 1"),
        }
    }

    /// Largest `anchor-target-bex` this puzzle admits, with the reason for the
    /// error message.
    ///
    /// The AI ceiling is not cosmetic: an %ai-pow target is scaled by the tile
    /// shape work factor (up to 2^24) before the 256-bit jackpot is compared
    /// against it, and that product is fail-closed. An anchor above 2^232 is
    /// therefore UNMINABLE rather than easy — every AI block is rejected, and
    /// because the AI ASERT only advances on an accepted AI block the fakenet's
    /// AI puzzle never recovers. Mirrors the Hoon `max-ai-target-atom` and
    /// `ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET`.
    fn max_anchor_target_bex(self) -> (u64, &'static str) {
        match self {
            AsertPuzzle::Zk => (FAKENET_ASERT_MAX_BEX, ""),
            AsertPuzzle::Ai => (
                AI_ASERT_MAX_BEX,
                " (above this the shape-scaled jackpot threshold overflows 256 bits \
                 and every AI block is rejected)",
            ),
        }
    }
}

/// Shared validation for a fakenet ASERT override. The
/// phase/anchor-height/anchor-target-bex trio is all-or-nothing; ideal-block-time
/// and half-life are independent optional tweaks that require the trio. The
/// anchor-height invariant is puzzle-specific (see [`AsertPuzzle`]).
fn build_fakenet_asert(
    puzzle: AsertPuzzle,
    phase: Option<u64>,
    anchor_height: Option<u64>,
    anchor_target_bex: Option<u64>,
    ideal_block_time: Option<u64>,
    half_life: Option<u64>,
) -> Result<Option<FakenetAsertConfig>, String> {
    let label = puzzle.label();
    match (phase, anchor_height, anchor_target_bex) {
        (None, None, None) => {
            if ideal_block_time.is_some() || half_life.is_some() {
                return Err(format!(
                    "--fakenet-{label}-ideal-block-time / --fakenet-{label}-half-life require the \
                     --fakenet-{label}-phase / -anchor-height / -anchor-target-bex trio to be set"
                ));
            }
            Ok(None)
        }
        (Some(phase), Some(anchor_height), Some(bex)) => {
            let (expected, relation) = puzzle.expected_anchor_height(phase);
            if phase == 0 || Some(anchor_height) != expected {
                return Err(format!(
                    "--fakenet-{label}-anchor-height ({anchor_height}) must equal \
                     --fakenet-{label}-phase ({phase}){relation}"
                ));
            }
            let (max_bex, why) = puzzle.max_anchor_target_bex();
            if bex > max_bex {
                return Err(format!(
                    "--fakenet-{label}-anchor-target-bex ({bex}) must be <= {max_bex}{why}"
                ));
            }
            Ok(Some(FakenetAsertConfig {
                phase,
                anchor_height,
                anchor_target_bex: bex,
                ideal_block_time,
                half_life,
            }))
        }
        _ => Err(format!(
            "--fakenet-{label}-phase, --fakenet-{label}-anchor-height, and \
             --fakenet-{label}-anchor-target-bex must all be specified together or not at all"
        )),
    }
}

/// CLI surface for the ZK-puzzle ASERT fakenet overrides. The trio must be
/// supplied together or not at all. Aliased `--fakenet-zk-asert-*` for symmetry
/// with the AI flags; `--fakenet-asert-*` is retained for compatibility.
///
/// Every arg carries an explicit `id`: this struct and `FakenetAiAsertArgs` share
/// Rust field names, and clap derives arg ids from field names — without unique
/// ids the two flatten into colliding args (the AI flags would cross-wire into the
/// ZK config).
#[derive(Args, Debug, Clone, Default)]
pub struct FakenetAsertArgs {
    #[arg(
        id = "fakenet_zk_asert_phase",
        long = "fakenet-asert-phase",
        visible_alias = "fakenet-zk-asert-phase",
        help = "ZK ASERT phase (aserti3-2d activation height) on fakenet. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub phase: Option<u64>,
    #[arg(
        id = "fakenet_zk_asert_anchor_height",
        long = "fakenet-asert-anchor-height",
        visible_alias = "fakenet-zk-asert-anchor-height",
        help = "ZK ASERT anchor height on fakenet. Must equal phase - 1. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_height: Option<u64>,
    #[arg(
        id = "fakenet_zk_asert_anchor_target_bex",
        long = "fakenet-asert-anchor-target-bex",
        visible_alias = "fakenet-zk-asert-anchor-target-bex",
        help = "ZK ASERT anchor target by bex exponent on fakenet (target = 2^bex). Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_target_bex: Option<u64>,
    #[arg(
        id = "fakenet_zk_asert_ideal_block_time",
        long = "fakenet-asert-ideal-block-time",
        visible_alias = "fakenet-zk-asert-ideal-block-time",
        help = "ZK ASERT ideal block time (seconds) on fakenet. Requires the ZK ASERT trio.",
        requires = "fakenet"
    )]
    pub ideal_block_time: Option<u64>,
    #[arg(
        id = "fakenet_zk_asert_half_life",
        long = "fakenet-asert-half-life",
        visible_alias = "fakenet-zk-asert-half-life",
        help = "ZK ASERT half-life (seconds) on fakenet; short => faster retarget. Requires the ZK ASERT trio.",
        requires = "fakenet"
    )]
    pub half_life: Option<u64>,
}

impl FakenetAsertArgs {
    /// Validate + convert. `Ok(None)` when unset; `Err` on a broken trio / cap.
    pub fn into_config(self) -> Result<Option<FakenetAsertConfig>, String> {
        build_fakenet_asert(
            AsertPuzzle::Zk,
            self.phase,
            self.anchor_height,
            self.anchor_target_bex,
            self.ideal_block_time,
            self.half_life,
        )
    }
}

/// CLI surface for AI-puzzle ASERT fakenet overrides. Its phase is the AI
/// admission boundary: it sets activation when the explicit activation flag is
/// absent, and must equal that flag when both are present.
#[derive(Args, Debug, Clone, Default)]
pub struct FakenetAiAsertArgs {
    #[arg(
        id = "fakenet_ai_asert_phase",
        long = "fakenet-ai-asert-phase",
        help = "AI ASERT phase and AI-PoW activation height on fakenet. Sets activation unless --fakenet-ai-pow-activation-height is supplied; both must match. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub phase: Option<u64>,
    #[arg(
        id = "fakenet_ai_asert_anchor_height",
        long = "fakenet-ai-asert-anchor-height",
        help = "AI ASERT anchor height on fakenet. Must equal phase - 1. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_height: Option<u64>,
    #[arg(
        id = "fakenet_ai_asert_anchor_target_bex",
        long = "fakenet-ai-asert-anchor-target-bex",
        help = "AI ASERT anchor target by bex exponent on fakenet (target = 2^bex). Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_target_bex: Option<u64>,
    #[arg(
        id = "fakenet_ai_asert_ideal_block_time",
        long = "fakenet-ai-asert-ideal-block-time",
        help = "AI ASERT ideal block time (seconds) on fakenet. Requires the AI ASERT trio.",
        requires = "fakenet"
    )]
    pub ideal_block_time: Option<u64>,
    #[arg(
        id = "fakenet_ai_asert_half_life",
        long = "fakenet-ai-asert-half-life",
        help = "AI ASERT half-life (seconds) on fakenet; short => faster retarget. Requires the AI ASERT trio.",
        requires = "fakenet"
    )]
    pub half_life: Option<u64>,
}

impl FakenetAiAsertArgs {
    /// Validate + convert. `Ok(None)` when unset; `Err` on a broken trio / cap.
    pub fn into_config(self) -> Result<Option<FakenetAsertConfig>, String> {
        build_fakenet_asert(
            AsertPuzzle::Ai,
            self.phase,
            self.anchor_height,
            self.anchor_target_bex,
            self.ideal_block_time,
            self.half_life,
        )
    }
}

/// Fakenet phase defaults applied by the CLI when no explicit `--fakenet-*-phase`
/// override is passed.  Exported so the E2E harness can configure scenarios to
/// match whatever the node will actually use, rather than duplicating magic
/// numbers in test code.
pub const DEFAULT_FAKENET_V1_PHASE: u64 = 1;
pub const DEFAULT_FAKENET_BYTHOS_PHASE: u64 = 1;

/// Command line arguments
#[derive(Parser, Debug, Clone)]
#[command(name = "nockchain")]
pub struct NockchainCli {
    #[command(flatten)]
    pub nockapp_cli: nockapp::kernel::boot::Cli,
    #[arg(long, help = "Whether to run as fakenet", default_value_t = false)]
    pub fakenet: bool,
    #[arg(long, short, help = "Initial peer", action = ArgAction::Append)]
    pub peer: Vec<String>,
    #[arg(long, short, help = "Force peer", action = ArgAction::Append)]
    pub force_peer: Vec<String>,
    #[arg(long, help = "Allowed peer IDs file")]
    pub allowed_peers_path: Option<String>,
    #[arg(long, help = "Don't dial default peers")]
    pub no_default_peers: bool,
    #[arg(long, help = "Bind address", action = ArgAction::Append)]
    pub bind: Option<Vec<String>>,
    #[arg(
        long,
        help = "Deprecated / no-op: persisting the existing peer ID is now the default. Retained for backward compatibility; pass --new-peer-id to force a fresh identity instead.",
        default_value = "false"
    )]
    pub no_new_peer_id: bool,
    #[arg(
        long,
        help = "Generate a fresh libp2p peer ID, discarding any existing identity. By default the existing peer ID is persisted across restarts so the node keeps a stable identity.",
        default_value = "false",
        conflicts_with = "no_new_peer_id"
    )]
    pub new_peer_id: bool,
    #[arg(
        long,
        help = "Override the path to the libp2p identity key (defaults to .nockchain_identity)"
    )]
    pub identity_path: Option<PathBuf>,
    #[arg(long, help = "Maximum established incoming connections")]
    pub max_established_incoming: Option<u32>,
    #[arg(long, help = "Maximum established outgoing connections")]
    pub max_established_outgoing: Option<u32>,
    #[arg(long, help = "Maximum pending incoming connections")]
    pub max_pending_incoming: Option<u32>,
    #[arg(long, help = "Maximum pending outgoing connections")]
    pub max_pending_outgoing: Option<u32>,
    #[arg(long, help = "Maximum established connections")]
    pub max_established: Option<u32>,
    #[arg(long, help = "Maximum established connections per peer")]
    pub max_established_per_peer: Option<u32>,
    #[arg(
        long,
        help = "Prune <N> inbound connections when a peer is denied due to connection limits. (Use on boot nodes only.)"
    )]
    pub prune_inbound: Option<usize>,
    #[arg(long, help = "Maximum system memory percentage for connection limits")]
    pub max_system_memory_fraction: Option<f64>,
    #[arg(long, help = "Maximum process memory for connection limits (bytes)")]
    pub max_system_memory_bytes: Option<usize>,
    #[arg(
        long,
        help = "Size of Proof of Work puzzle for mining on fakenet. Mainnet uses 64. Must be a power of 2. Defaults to 2. Ignored on mainnet.",
        default_value = "2",
        requires = "fakenet"
    )]
    pub fakenet_pow_len: u64,
    #[arg(
        long,
        help = "log target difficulty for mining on fakenet. Defaults to 1 (so 2^1 attempts on average find a block). Ignored on mainnet.",
        default_value = "1",
        requires = "fakenet"
    )]
    pub fakenet_log_difficulty: u64,
    #[arg(
        long,
        help = "Override the v1-phase activation height when running on fakenet. Requires --fakenet.",
        default_value = "1",
        requires = "fakenet"
    )]
    pub fakenet_v1_phase: Option<u64>,
    #[arg(
        long,
        help = "Override the bythos-phase activation height when running on fakenet. Requires --fakenet.",
        default_value = "1",
        requires = "fakenet"
    )]
    pub fakenet_bythos_phase: Option<u64>,
    #[arg(
        long,
        help = "Override the AI-PoW activation height when running on fakenet. Also \
                sets phase.zk-asert-post-ai and phase.ai-asert to the same value so the \
                ZK regime switch + AI puzzle activation happen together. \
                Defaults to BlockchainConstants::DEFAULT_AI_POW_ACTIVATION_HEIGHT \
                (114300 on mainnet, effectively never on fakenet). \
                Requires --fakenet.",
        requires = "fakenet"
    )]
    pub fakenet_ai_pow_activation_height: Option<u64>,
    #[command(flatten)]
    pub fakenet_asert: FakenetAsertArgs,
    #[command(flatten)]
    pub fakenet_ai_asert: FakenetAiAsertArgs,
    #[arg(
        long,
        help = "Override the candidate timestamp update interval in seconds when running on fakenet. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub fakenet_update_candidate_interval_secs: Option<u64>,
    #[arg(long, help = "Path to fake genesis block jam file")]
    pub fakenet_genesis_jam_path: Option<PathBuf>,
    #[arg(long, help = "Public gRPC binding address (off by default), recommended value = \"127.0.0.1:5555\"", value_parser = clap::value_parser!(std::net::SocketAddr))]
    pub bind_public_grpc_addr: Option<std::net::SocketAddr>,
    #[arg(
        long,
        help = "Private gRPC binding address (e.g. 0.0.0.0:5555). \
                Overrides --bind-private-grpc-port when set. \
                Needed when the node runs inside a Docker container and the \
                test host must reach the gRPC endpoint from outside. \
                WARNING: the private gRPC endpoint is unauthenticated kernel \
                command authority; bind a trusted interface only (the default \
                is 127.0.0.1).",
        value_parser = clap::value_parser!(std::net::SocketAddr)
    )]
    pub bind_private_grpc_addr: Option<std::net::SocketAddr>,
    #[arg(long, default_value = "5555")]
    pub bind_private_grpc_port: u16,
    #[arg(
        long = "ai-pow-verifier-cache-cap",
        help = "Max resident AI-PoW verifier contexts (LRU). The production default \
                retains all 13 supported shape keys across seven trace heights, preventing \
                attacker-controlled evict/reload thrash at the measured setup-table RSS. \
                Lowering the cap reduces RSS by allowing verifier contexts to page in and \
                out, but reintroduces synchronous page-in churn under adversarial traffic. \
                Overrides AI_POW_VERIFIER_CACHE_CAP."
    )]
    pub ai_pow_verifier_cache_cap: Option<usize>,
}

impl NockchainCli {
    pub fn parse_with_default_stack_size(default_stack_size: NockStackSize) -> Self {
        Self::parse_from_with_default_stack_size(std::env::args_os(), default_stack_size)
    }

    fn parse_from_with_default_stack_size<I, T>(args: I, default_stack_size: NockStackSize) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        Self::try_parse_from_with_default_stack_size(args, default_stack_size)
            .unwrap_or_else(|err| err.exit())
    }

    fn try_parse_from_with_default_stack_size<I, T>(
        args: I,
        default_stack_size: NockStackSize,
    ) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut matches = Self::command_with_default_stack_size(default_stack_size)
            .try_get_matches_from(args.into_iter().map(Into::into))?;
        let mut cli = <Self as FromArgMatches>::from_arg_matches_mut(&mut matches)?;
        if cli.nockapp_cli.pma_initial_size.is_none() {
            cli.nockapp_cli.pma_initial_size = Some(PmaSize::from_words(
                cli.nockapp_cli.stack_size.stack_words(),
            ));
        }
        Ok(cli)
    }

    fn command_with_default_stack_size(default_stack_size: NockStackSize) -> clap::Command {
        <Self as CommandFactory>::command().mut_arg("stack_size", |arg| {
            arg.default_value(stack_size_default_arg(default_stack_size))
        })
    }
}

fn stack_size_default_arg(stack_size: NockStackSize) -> &'static str {
    match stack_size {
        NockStackSize::Tiny => "tiny",
        NockStackSize::Small => "small",
        NockStackSize::Normal => "normal",
        NockStackSize::Medium => "medium",
        NockStackSize::Large => "large",
        NockStackSize::Huge => "huge",
    }
}

impl NockchainCli {
    /// Effective AI activation for fakenet. An AI ASERT trio without an explicit
    /// activation flag sets the activation to its phase; if both are supplied,
    /// they must agree. This preserves the protocol invariant that AI admission,
    /// the AI anchor, and the post-AI ZK regime share one boundary.
    pub(crate) fn effective_fakenet_ai_activation_height(&self) -> Result<Option<u64>, String> {
        let ai_asert = self.fakenet_ai_asert.clone().into_config()?;
        let explicit = self.fakenet_ai_pow_activation_height;
        if explicit == Some(0) {
            return Err("--fakenet-ai-pow-activation-height must be >= 1".to_string());
        }
        match (explicit, ai_asert) {
            (Some(activation), Some(asert)) if activation != asert.phase => Err(format!(
                "--fakenet-ai-pow-activation-height ({activation}) must equal \
                 --fakenet-ai-asert-phase ({})",
                asert.phase
            )),
            (Some(activation), _) => Ok(Some(activation)),
            (None, Some(asert)) => Ok(Some(asert.phase)),
            (None, None) => Ok(None),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.fakenet_asert.clone().into_config().map(|_| ())?;
        self.effective_fakenet_ai_activation_height()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use nockapp::kernel::boot::{default_boot_cli, NockStackSize};
    use nockapp::utils::{NOCK_STACK_SIZE, NOCK_STACK_SIZE_MEDIUM, NOCK_STACK_SIZE_SMALL};

    use super::*;

    fn base_cli() -> NockchainCli {
        NockchainCli {
            nockapp_cli: default_boot_cli(false),
            fakenet: false,
            peer: Vec::new(),
            force_peer: Vec::new(),
            allowed_peers_path: None,
            no_default_peers: false,
            bind: None,
            no_new_peer_id: false,
            new_peer_id: false,
            identity_path: None,
            max_established_incoming: None,
            max_established_outgoing: None,
            max_pending_incoming: None,
            max_pending_outgoing: None,
            max_established: None,
            max_established_per_peer: None,
            prune_inbound: None,
            max_system_memory_fraction: None,
            max_system_memory_bytes: None,
            fakenet_pow_len: 2,
            fakenet_log_difficulty: 1,
            fakenet_v1_phase: None,
            fakenet_bythos_phase: None,
            fakenet_ai_pow_activation_height: None,
            fakenet_asert: FakenetAsertArgs::default(),
            fakenet_ai_asert: FakenetAiAsertArgs::default(),
            fakenet_update_candidate_interval_secs: None,
            fakenet_genesis_jam_path: None,
            bind_public_grpc_addr: Some("127.0.0.1:5555".parse().unwrap()),
            bind_private_grpc_addr: None,
            bind_private_grpc_port: 5555,
            ai_pow_verifier_cache_cap: None,
        }
    }

    // clap requires unique arg ids across all flattened arg structs. Because the
    // ZK and AI ASERT arg structs share Rust field names, each needs an explicit
    // `id`; this catches a regression where two args collide (silent in release,
    // panics here).
    #[test]
    fn cli_command_arg_ids_are_unique() {
        NockchainCli::command_with_default_stack_size(NockStackSize::Large).debug_assert();
    }

    // The AI and ZK ASERT flags must parse into their OWN structs — a shared arg id
    // would cross-wire them (e.g. --fakenet-ai-asert-phase leaking into the ZK
    // config). Parse a mixed command line and confirm the values land separately.
    #[test]
    fn zk_and_ai_asert_flags_parse_independently() {
        let cli = NockchainCli::parse_from_with_default_stack_size(
            [
                "nockchain", "--fakenet", "--fakenet-asert-phase", "10",
                "--fakenet-asert-anchor-height", "9", "--fakenet-asert-anchor-target-bex", "40",
                "--fakenet-ai-asert-phase", "20", "--fakenet-ai-asert-anchor-height", "19",
                "--fakenet-ai-asert-anchor-target-bex", "50",
            ],
            NockStackSize::Large,
        );
        assert_eq!(cli.fakenet_asert.phase, Some(10));
        assert_eq!(cli.fakenet_asert.anchor_height, Some(9));
        assert_eq!(cli.fakenet_asert.anchor_target_bex, Some(40));
        assert_eq!(cli.fakenet_ai_asert.phase, Some(20));
        assert_eq!(cli.fakenet_ai_asert.anchor_height, Some(19));
        assert_eq!(cli.fakenet_ai_asert.anchor_target_bex, Some(50));
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn ai_pow_verifier_cache_cap_flag_sets_resident_context_cap() {
        let cli = NockchainCli::try_parse_from_with_default_stack_size(
            ["nockchain", "--ai-pow-verifier-cache-cap", "2"],
            NockStackSize::Large,
        )
        .unwrap();
        assert_eq!(cli.ai_pow_verifier_cache_cap, Some(2));
    }

    #[test]
    fn default_stack_size_can_be_set_by_binary() {
        let cli =
            NockchainCli::parse_from_with_default_stack_size(["nockchain"], NockStackSize::Medium);
        assert!(matches!(cli.nockapp_cli.stack_size, NockStackSize::Medium));
        assert_eq!(
            cli.nockapp_cli.pma_initial_size.unwrap().words(),
            NOCK_STACK_SIZE_MEDIUM
        );
    }

    #[test]
    fn explicit_stack_size_overrides_binary_default() {
        let cli = NockchainCli::parse_from_with_default_stack_size(
            ["nockchain", "--stack-size", "normal"],
            NockStackSize::Medium,
        );
        assert!(matches!(cli.nockapp_cli.stack_size, NockStackSize::Normal));
        assert_eq!(
            cli.nockapp_cli.pma_initial_size.unwrap().words(),
            NOCK_STACK_SIZE
        );

        let cli = NockchainCli::parse_from_with_default_stack_size(
            ["nockchain", "--stack-size=small"],
            NockStackSize::Medium,
        );
        assert!(matches!(cli.nockapp_cli.stack_size, NockStackSize::Small));
        assert_eq!(
            cli.nockapp_cli.pma_initial_size.unwrap().words(),
            NOCK_STACK_SIZE_SMALL
        );
    }

    #[test]
    fn explicit_pma_initial_size_overrides_nockchain_stack_default() {
        let cli = NockchainCli::parse_from_with_default_stack_size(
            ["nockchain", "--stack-size=small", "--pma-initial-size=512MiB"],
            NockStackSize::Medium,
        );
        assert!(matches!(cli.nockapp_cli.stack_size, NockStackSize::Small));
        assert_eq!(
            cli.nockapp_cli.pma_initial_size.unwrap().words(),
            64 * 1024 * 1024
        );
    }

    fn zk_asert(
        phase: Option<u64>,
        anchor_height: Option<u64>,
        anchor_target_bex: Option<u64>,
    ) -> FakenetAsertArgs {
        FakenetAsertArgs {
            phase,
            anchor_height,
            anchor_target_bex,
            ideal_block_time: None,
            half_life: None,
        }
    }

    fn ai_asert(
        phase: Option<u64>,
        anchor_height: Option<u64>,
        anchor_target_bex: Option<u64>,
    ) -> FakenetAiAsertArgs {
        FakenetAiAsertArgs {
            phase,
            anchor_height,
            anchor_target_bex,
            ideal_block_time: None,
            half_life: None,
        }
    }

    #[test]
    fn validate_accepts_all_three_asert_overrides() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(10), Some(9), Some(4));
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn validate_accepts_no_asert_overrides() {
        let cli = base_cli();
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn validate_rejects_partial_asert_overrides() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(10), None, None);
        let err = cli.validate().expect_err("expected partial ASERT error");
        assert!(err.contains("must all be specified together"));
    }

    #[test]
    fn validate_rejects_anchor_height_not_phase_minus_one() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(10), Some(8), Some(4)); // anchor should be 9
        let err = cli.validate().expect_err("expected anchor invariant error");
        assert!(err.contains("must equal"));
    }

    #[test]
    fn validate_rejects_asert_phase_zero() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(0), Some(0), Some(4));
        let err = cli.validate().expect_err("expected phase=0 error");
        assert!(err.contains("must equal"));
    }

    #[test]
    fn validate_rejects_ai_pow_activation_height_zero() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_pow_activation_height = Some(0);
        let err = cli
            .validate()
            .expect_err("expected activation-height=0 error");
        assert!(err.contains(">= 1"));
    }

    #[test]
    fn validate_rejects_bex_above_cap() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(10), Some(9), Some(513));
        let err = cli.validate().expect_err("expected bex cap error");
        assert!(err.contains("must be <="));
    }

    // The AI pin uses the same predecessor convention as the ZK re-pin, so
    // its shared schedule is available before the first AI block.
    #[test]
    fn validate_ai_asert_phase_sets_activation_when_flag_is_omitted() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(9), Some(4));
        assert!(cli.validate().is_ok());
        assert_eq!(
            cli.effective_fakenet_ai_activation_height().unwrap(),
            Some(10)
        );
    }

    #[test]
    fn validate_rejects_ai_asert_phase_different_from_activation() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_pow_activation_height = Some(11);
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(9), Some(4));
        let err = cli
            .validate()
            .expect_err("expected AI activation/ASERT boundary mismatch");
        assert!(err.contains("must equal"));
        assert!(err.contains("ai-asert-phase"));
    }

    #[test]
    fn validate_rejects_ai_asert_anchor_at_phase() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(10), Some(4));
        let err = cli
            .validate()
            .expect_err("expected AI anchor invariant error");
        assert!(err.contains("ai-asert"));
        assert!(err.contains("must equal"));
    }

    #[test]
    fn validate_rejects_partial_ai_asert_overrides() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), None, None);
        let err = cli.validate().expect_err("expected partial AI ASERT error");
        assert!(err.contains("ai-asert"));
        assert!(err.contains("must all be specified together"));
    }

    #[test]
    fn validate_rejects_ai_asert_bex_above_cap() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(9), Some(513));
        let err = cli.validate().expect_err("expected AI bex cap error");
        assert!(err.contains("ai-asert"));
        assert!(err.contains("must be <="));
    }

    // The AI anchor ceiling is TIGHTER than the ZK one and for a different
    // reason: above 2^232 the shape-scaled jackpot threshold leaves the 256-bit
    // domain, the verifier fail-closes on every AI block, and the AI ASERT --
    // which only advances on an ACCEPTED AI block -- never retargets back down.
    // A fakenet configured there would look like "AI mining just never works".
    #[test]
    fn validate_rejects_ai_asert_bex_above_the_minable_domain() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(9), Some(AI_ASERT_MAX_BEX + 1));
        let err = cli
            .validate()
            .expect_err("expected AI minable-domain cap error");
        assert!(err.contains("ai-asert"));
        assert!(err.contains(&format!("must be <= {AI_ASERT_MAX_BEX}")));
    }

    // ...and the boundary itself is accepted, so the bound is not off by one.
    #[test]
    fn validate_accepts_ai_asert_bex_at_the_minable_domain_edge() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_ai_asert = ai_asert(Some(10), Some(9), Some(AI_ASERT_MAX_BEX));
        cli.validate()
            .expect("the maximum minable AI anchor exponent must be accepted");
    }

    // The ZK puzzle keeps the wider ceiling: its target lives in the ~2^320
    // tip5 space and is never shape-scaled.
    #[test]
    fn validate_accepts_zk_asert_bex_above_the_ai_ceiling() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = zk_asert(Some(10), Some(9), Some(AI_ASERT_MAX_BEX + 1));
        cli.validate()
            .expect("the AI ceiling must not narrow the ZK puzzle");
    }

    // ideal-block-time / half-life are optional but require the trio to be present.
    #[test]
    fn validate_accepts_ideal_and_half_life_with_trio() {
        let mut cli = base_cli();
        cli.fakenet = true;
        let mut zk = zk_asert(Some(10), Some(9), Some(4));
        zk.ideal_block_time = Some(30);
        zk.half_life = Some(60);
        cli.fakenet_asert = zk;
        let mut ai = ai_asert(Some(10), Some(9), Some(4));
        ai.ideal_block_time = Some(30);
        ai.half_life = Some(60);
        cli.fakenet_ai_asert = ai;
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn validate_rejects_ideal_or_half_life_without_trio() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = FakenetAsertArgs {
            ideal_block_time: Some(30),
            ..Default::default()
        };
        let err = cli
            .validate()
            .expect_err("expected ideal-without-trio error");
        assert!(err.contains("require the"));
    }
}
