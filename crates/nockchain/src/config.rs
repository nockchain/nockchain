use std::path::PathBuf;
use std::time::Duration;

use clap::{arg, command, value_parser, ArgAction, Parser};

use crate::mining::MiningKeyConfig;

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

// Libp2p multiaddrs don't support const construction, so we have to put strings literals and parse them at startup
// TODO: feature flag testnet/realnet
/** Backbone nodes for our realnet */
#[allow(dead_code)]
pub const REALNET_BACKBONE_NODES: &[&str] = &["/dnsaddr/nockchain-backbone.zorp.io"];

/** How often we should affirmatively ask other nodes for their heaviest chain */
pub const CHAIN_INTERVAL: Duration = Duration::from_secs(20);

/// The height of the bitcoin block that we want to sync our genesis block to
/// Currently, this is the height of an existing block for testing. It will be
/// switched to a future block for launch.
pub const GENESIS_HEIGHT: u64 = 897767;

/// Command line arguments
#[derive(Parser, Debug, Clone)]
#[command(name = "nockchain")]
pub struct NockchainCli {
    #[command(flatten)]
    pub nockapp_cli: nockapp::kernel::boot::Cli,
    #[arg(long, help = "Mine in-kernel", default_value = "false")]
    pub mine: bool,
    #[arg(
        long,
        help = "Pubkey to mine to (mutually exclusive with --mining-key-adv)"
    )]
    pub mining_pubkey: Option<String>,
    #[arg(
        long,
        help = "Advanced mining key configuration (mutually exclusive with --mining-pubkey). Format: share,m:key1,key2,key3",
        value_parser = value_parser!(MiningKeyConfig),
        num_args = 1..,
    )]
    pub mining_key_adv: Option<Vec<MiningKeyConfig>>,
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
    pub bind: Vec<String>,
    #[arg(
        long,
        help = "Don't generate a new peer ID, keep the existing one",
        default_value = "false"
    )]
    pub no_new_peer_id: bool,
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
    #[arg(long, help = "Number of threads to mine with defaults to one less than the number of cpus available.", default_value = None)]
    pub num_threads: Option<u64>,
    #[arg(
        long,
        help = "Size of Proof of Work puzzle for mining on fakenet. Mainnet uses 64. Must be a power of 2. Defaults to 2. Ignored on mainnet.",
        default_value = "2"
    )]
    pub fakenet_pow_len: Option<u64>,
    #[arg(
        long,
        help = "log target difficulty for mining on fakenet. Defaults to 2 (so 2^2 attempts on average find a block). Ignored on mainnet.",
        default_value = "1"
    )]
    pub fakenet_log_difficulty: Option<u64>,
    #[arg(long, help = "Path to fake genesis block jam file")]
    pub fakenet_genesis_jam_path: Option<PathBuf>,
}

impl NockchainCli {
    pub fn validate(&self) -> Result<(), String> {
        if self.mine && !(self.mining_pubkey.is_some() || self.mining_key_adv.is_some()) {
            return Err(
                "Cannot specify mine without either mining_pubkey or mining_key_adv".to_string(),
            );
        }

        if self.mining_pubkey.is_some() && self.mining_key_adv.is_some() {
            return Err(
                "Cannot specify both mining_pubkey and mining_key_adv at the same time".to_string(),
            );
        }

        Ok(())
    }
}
