use std::path::PathBuf;
use std::time::Duration;

use clap::{value_parser, ArgAction, Args, Parser};
use nockchain_types::tx_engine::common::Hash;

use crate::mining::MiningPkhConfig;

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

/// Validated ASERT fakenet trio. Only constructible via [`FakenetAsertArgs::into_config`].
#[derive(Debug, Clone, Copy)]
pub struct FakenetAsertConfig {
    pub phase: u64,
    pub anchor_height: u64,
    pub anchor_target_bex: u64,
}

/// CLI surface for the three ASERT fakenet overrides. All three must be supplied together or not
/// at all; call [`into_config`][FakenetAsertArgs::into_config] to enforce the invariant and obtain
/// a [`FakenetAsertConfig`].
#[derive(Args, Debug, Clone, Default)]
pub struct FakenetAsertArgs {
    #[arg(
        long = "fakenet-asert-phase",
        help = "Override the asert-phase (aserti3-2d activation height) when running on fakenet. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub phase: Option<u64>,
    #[arg(
        long = "fakenet-asert-anchor-height",
        help = "Override the asert-anchor-height when running on fakenet. Must equal asert-phase - 1. Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_height: Option<u64>,
    #[arg(
        long = "fakenet-asert-anchor-target-bex",
        help = "Override asert-anchor-target-atom by bex exponent when running on fakenet (target = 2^bex). Requires --fakenet.",
        requires = "fakenet"
    )]
    pub anchor_target_bex: Option<u64>,
}

impl FakenetAsertArgs {
    /// Validates the trio invariant and converts to [`FakenetAsertConfig`].
    ///
    /// Returns `Ok(None)` when none of the three flags are set.
    /// Returns `Err` when only some are set, when `anchor_height + 1 != phase`, or when
    /// `anchor_target_bex` exceeds the cap.
    pub fn into_config(self) -> Result<Option<FakenetAsertConfig>, String> {
        match (self.phase, self.anchor_height, self.anchor_target_bex) {
            (None, None, None) => Ok(None),
            (Some(phase), Some(anchor_height), Some(bex)) => {
                if phase == 0 || Some(phase) != anchor_height.checked_add(1) {
                    return Err(format!(
                        "--fakenet-asert-anchor-height ({anchor_height}) must equal \
                         --fakenet-asert-phase ({phase}) minus 1"
                    ));
                }
                const MAX_BEX: u64 = 512;
                if bex > MAX_BEX {
                    return Err(format!(
                        "--fakenet-asert-anchor-target-bex ({bex}) must be <= {MAX_BEX}"
                    ));
                }
                Ok(Some(FakenetAsertConfig {
                    phase,
                    anchor_height,
                    anchor_target_bex: bex,
                }))
            }
            _ => Err("--fakenet-asert-phase, --fakenet-asert-anchor-height, and \
                 --fakenet-asert-anchor-target-bex must all be specified together or not at all"
                .to_string()),
        }
    }
}

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
        help = "Pubkey hash to mine to (mutually exclusive with --mining-pkh-adv)"
    )]
    pub mining_pkh: Option<String>,
    #[arg(
        long,
        help = "Advanced mining pubkey hash configuration (mutually exclusive with --mining-pkh). Format: share,pkh",
        value_parser = value_parser!(MiningPkhConfig),
        num_args = 1..,
    )]
    pub mining_pkh_adv: Option<Vec<MiningPkhConfig>>,
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
    #[command(flatten)]
    pub fakenet_asert: FakenetAsertArgs,
    #[arg(long, help = "Path to fake genesis block jam file")]
    pub fakenet_genesis_jam_path: Option<PathBuf>,
    #[arg(long, help = "Public gRPC binding address (off by default), recommended value = \"127.0.0.1:5555\"", value_parser = clap::value_parser!(std::net::SocketAddr))]
    pub bind_public_grpc_addr: Option<std::net::SocketAddr>,
    #[arg(long, default_value = "5555")]
    pub bind_private_grpc_port: u16,
    #[arg(long, default_value = "false")]
    pub fast_sync: bool,
}

impl NockchainCli {
    pub fn validate(&self) -> Result<(), String> {
        if self.mine && !(self.mining_pkh.is_some() || self.mining_pkh_adv.is_some()) {
            return Err(
                "Cannot specify mine without either mining_pkh or mining_pkh_adv".to_string(),
            );
        }

        if self.mining_pkh.is_some() && self.mining_pkh_adv.is_some() {
            return Err(
                "Cannot specify both mining_pkh and mining_pkh_adv at the same time".to_string(),
            );
        }

        if let Some(pkh) = &self.mining_pkh {
            Hash::from_base58(pkh).map_err(|err| format!("Invalid mining_pkh: {err}"))?;
        }

        if let Some(pkh_configs) = &self.mining_pkh_adv {
            for config in pkh_configs {
                Hash::from_base58(&config.pkh).map_err(|err| {
                    format!("Invalid mining_pkh_adv entry '{}': {err}", config.pkh)
                })?;
            }
        }

        self.fakenet_asert.clone().into_config().map(|_| ())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use nockapp::kernel::boot::default_boot_cli;

    use super::*;

    const VALID_V0_PUBKEY: &str = "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ";
    const VALID_MINING_PKH: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";

    fn base_cli() -> NockchainCli {
        NockchainCli {
            nockapp_cli: default_boot_cli(false),
            mine: false,
            mining_pkh: None,
            mining_pkh_adv: None,
            fakenet: false,
            peer: Vec::new(),
            force_peer: Vec::new(),
            allowed_peers_path: None,
            no_default_peers: false,
            bind: None,
            no_new_peer_id: false,
            max_established_incoming: None,
            max_established_outgoing: None,
            max_pending_incoming: None,
            max_pending_outgoing: None,
            max_established: None,
            max_established_per_peer: None,
            prune_inbound: None,
            max_system_memory_fraction: None,
            max_system_memory_bytes: None,
            num_threads: None,
            fakenet_pow_len: 2,
            fakenet_log_difficulty: 1,
            fakenet_v1_phase: None,
            fakenet_bythos_phase: None,
            fakenet_asert: FakenetAsertArgs::default(),
            fakenet_genesis_jam_path: None,
            bind_public_grpc_addr: Some("127.0.0.1:5555".parse().unwrap()),
            bind_private_grpc_port: 5555,
            fast_sync: false,
        }
    }

    #[test]
    fn validate_accepts_valid_advanced_configs() {
        let mut cli = base_cli();
        cli.mining_pkh_adv = Some(vec![MiningPkhConfig {
            share: 1,
            pkh: VALID_MINING_PKH.to_string(),
        }]);

        assert!(cli.validate().is_ok());
    }

    #[test]
    fn validate_accepts_all_three_asert_overrides() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = FakenetAsertArgs {
            phase: Some(10),
            anchor_height: Some(9),
            anchor_target_bex: Some(4),
        };
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
        cli.fakenet_asert = FakenetAsertArgs {
            phase: Some(10),
            anchor_height: None,
            anchor_target_bex: None,
        };
        let err = cli.validate().expect_err("expected partial ASERT error");
        assert!(err.contains("must all be specified together"));
    }

    #[test]
    fn validate_rejects_anchor_height_not_phase_minus_one() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = FakenetAsertArgs {
            phase: Some(10),
            anchor_height: Some(8), // should be 9
            anchor_target_bex: Some(4),
        };
        let err = cli.validate().expect_err("expected anchor invariant error");
        assert!(err.contains("must equal"));
    }

    #[test]
    fn validate_rejects_asert_phase_zero() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = FakenetAsertArgs {
            phase: Some(0),
            anchor_height: Some(0),
            anchor_target_bex: Some(4),
        };
        let err = cli.validate().expect_err("expected phase=0 error");
        assert!(err.contains("must equal"));
    }

    #[test]
    fn validate_rejects_bex_above_cap() {
        let mut cli = base_cli();
        cli.fakenet = true;
        cli.fakenet_asert = FakenetAsertArgs {
            phase: Some(10),
            anchor_height: Some(9),
            anchor_target_bex: Some(513),
        };
        let err = cli.validate().expect_err("expected bex cap error");
        assert!(err.contains("must be <="));
    }

    #[test]
    fn validate_rejects_invalid_mining_pkh_adv_entry() {
        // We specifically want to catch if users mix up v0 and v1 addresses, because they are both base58-encoded.
        // Using a base58-encoded pubkey ensures the input is base58 but not a valid hash.
        let mut cli = base_cli();
        cli.mining_pkh_adv = Some(vec![MiningPkhConfig {
            share: 1,
            pkh: VALID_V0_PUBKEY.to_string(),
        }]);

        let err = cli.validate().expect_err("expected invalid pkh adv");
        assert!(err.contains("Invalid mining_pkh_adv entry"));
    }
}
