//! `ai-pow-mine` — standalone AI-PoW (matmul puzzle) block miner.
//!
//! Mirrors `zk-pow-mine` in shape: connects to a `nockchain` node's
//! private NockAppService gRPC, subscribes to `%mine` candidate
//! effects, searches Pearl-compatible tickets, builds the recursive
//! certificate only after a Nockchain target hit, and submits
//! `[%command %pow %ai-pow nonce cert]` on the `AiPowMinerWire::Mined` wire
//! (`SOURCE = "ai-pow-miner"`, `VERSION = 1`). The production submission path
//! fails closed for multi-tile configurations until the recursive statement
//! binds a full-matrix aggregate.
//!
//! Quick start (assuming a fakenet node on `127.0.0.1:5555` and Pearl Gateway
//! on `/tmp/pearlgw.sock`):
//!
//!   ai-pow-mine \
//!       --mining-pkh 9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV
//!
//! The CLI defaults to Pearl-compatible submission with single-tile,
//! production-envelope smoke parameters for local Layer-0 development. The
//! Pearl work source is Pearl Gateway miner RPC; the endpoint defaults to the
//! Unix socket `/tmp/pearlgw.sock`. Use `--pearl-gateway tcp://host:port` for a
//! TCP gateway or `--pearl-gateway /path/to.sock` for a different Unix socket.
//! Rewards must be configured with v1 pubkey-hash configs via `--mining-pkh`
//! or `--mining-pkh-adv`.
//! The production profile
//! derives canonical seeds from the nonce-keyed chunk commitments bound by the
//! recursive proof as `HASH_A` / `HASH_B`; larger production shapes remain
//! closed until full-matrix aggregation is implemented.
//!
//! ## AI puzzle inputs (local config)
//! The chain's `%mine-ai` effect carries the candidate block commitment,
//! target, and pow-len. The miner additionally owns fixed matmul `params`,
//! fixed local smoke-profile matrices, and Rust-only Pearl transcript fields.
//! Hoon still receives only the opaque `%ai-pow` nonce plus recursive
//! certificate.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    validate_pearl_merge_config_for_recursive_prover, PearlMiningConfig, PearlNockchainAux,
    PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow_miner::pearl_mining::PearlMergeMineOptions;
use ai_pow_miner::run::{
    run, AiPuzzleInputs, MinerConfig, MinerError, PearlGatewayMinerRpcConfig,
    PearlGatewayTransport, PearlMergeSubmissionConfig,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use nockchain_mining_common::MiningPkhConfig;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

const DEFAULT_PEARL_NOCKCHAIN_CHAIN_ID: &str = "nockchain";
const DEFAULT_PEARL_GATEWAY_ENDPOINT: &str = "unix:/tmp/pearlgw.sock";
const DEFAULT_PEARL_GATEWAY_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_PEARL_GATEWAY_REFRESH_MS: u64 = 1_000;
const DEFAULT_RECONNECT_BACKOFF_INITIAL_MS: u64 = 1_000;
const DEFAULT_RECONNECT_BACKOFF_MAX_MS: u64 = 30_000;
const DEFAULT_RECONNECT_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_SYNTH_SEED: &str = "ai-pow-prod-v1";
const DEFAULT_MATMUL_PARAMS: MatmulParams = MatmulParams {
    m: 8,
    k: 1024,
    n: 8,
    noise_rank: 32,
    tile: 8,
    spot_checks: 1,
    difficulty_bits: 0,
};

/// `ai-pow-mine` — standalone AI-PoW block miner.
#[derive(Parser, Debug)]
#[command(
    name = "ai-pow-mine",
    about = "Standalone AI-PoW block miner. Mines Pearl-compatible tickets and submits recursive %ai-pow commands to a nockchain node.",
    version
)]
struct Args {
    /// Node's private gRPC URL.
    #[arg(long, default_value = "http://127.0.0.1:5555")]
    node_addr: String,

    /// Single-recipient v1 mining pubkey hash. Mutually exclusive with --mining-pkh-adv.
    #[arg(long, conflicts_with = "mining_pkh_adv")]
    mining_pkh: Option<String>,

    /// Multi-recipient v1 mining pkh configs. Each entry is `share,pkh`.
    #[arg(long, value_parser = clap::value_parser!(MiningPkhConfig), num_args = 1..)]
    mining_pkh_adv: Option<Vec<MiningPkhConfig>>,

    /// Pearl Gateway miner RPC endpoint. Accepts `unix:/path/to.sock`, `/path/to.sock`,
    /// `tcp:host:port`, `tcp://host:port`, or `host:port`.
    #[arg(long, value_name = "ENDPOINT", default_value = DEFAULT_PEARL_GATEWAY_ENDPOINT)]
    pearl_gateway: String,

    /// Log filter (env-filter syntax). Override with the `RUST_LOG` env var.
    #[arg(
        long,
        default_value = "info,ai_pow_miner=info,nockchain_mining_common=info"
    )]
    log: String,
}

fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(&args.log);

    let Some(pkh_configs) = build_pkh_configs(&args) else {
        eprintln!("ai-pow-mine: must supply --mining-pkh <HASH> or --mining-pkh-adv \"share,pkh\"");
        return ExitCode::from(1);
    };

    let puzzle = match build_puzzle_inputs(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ai-pow-mine: invalid puzzle config: {e:#}");
            return ExitCode::from(1);
        }
    };

    let cfg = MinerConfig {
        node_addr: args.node_addr,
        mining_pkh_configs: pkh_configs,
        puzzle,
        reconnect_backoff_initial: Duration::from_millis(DEFAULT_RECONNECT_BACKOFF_INITIAL_MS),
        reconnect_backoff_max: Duration::from_millis(DEFAULT_RECONNECT_BACKOFF_MAX_MS),
        reconnect_max_attempts: DEFAULT_RECONNECT_MAX_ATTEMPTS,
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ai-pow-mine: failed to build tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    let r: Result<(), MinerError> = rt.block_on(async {
        info!(
            node = %cfg.node_addr,
            puzzle_m = cfg.puzzle.params.m,
            puzzle_k = cfg.puzzle.params.k,
            puzzle_n = cfg.puzzle.params.n,
            "ai-pow-mine: starting"
        );
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("ai-pow-mine: SIGINT received; shutting down");
                shutdown_clone.cancel();
            }
        });
        run(cfg, shutdown).await
    });

    match r {
        Ok(()) => {
            info!("ai-pow-mine: clean shutdown");
            ExitCode::from(0)
        }
        Err(MinerError::TooManyReconnects { count }) => {
            error!("ai-pow-mine: gave up after {count} consecutive reconnect failures");
            ExitCode::from(2)
        }
        Err(e) => {
            error!(error = %e, "ai-pow-mine: unrecoverable error");
            ExitCode::from(1)
        }
    }
}

fn init_tracing(filter: &str) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    let _ = fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}

fn build_pkh_configs(args: &Args) -> Option<Vec<MiningPkhConfig>> {
    if let Some(pkh) = &args.mining_pkh {
        Some(vec![MiningPkhConfig {
            share: 1,
            pkh: pkh.clone(),
        }])
    } else if let Some(adv) = &args.mining_pkh_adv {
        Some(adv.clone())
    } else {
        None
    }
}

fn build_puzzle_inputs(args: &Args) -> Result<AiPuzzleInputs> {
    let params = DEFAULT_MATMUL_PARAMS;
    params
        .validate()
        .map_err(|e| anyhow!("matmul params invalid: {e}"))?;
    validate_pearl_recursive_cli_params(params)?;

    let (a, b) = ai_pow::synth::synth_matrices(DEFAULT_SYNTH_SEED.as_bytes(), &params);

    let a = Arc::new(a);
    let b = Arc::new(b);

    let pearl_merge = build_pearl_merge_submission_config(args, params, &a, &b)?;
    Ok(AiPuzzleInputs {
        params,
        a,
        b,
        pearl_merge,
    })
}

fn validate_pearl_recursive_cli_params(params: MatmulParams) -> Result<()> {
    if params.difficulty_bits != 0 || params.spot_checks != 1 {
        bail!(
            "Pearl-compatible recursive certificates require difficulty_bits 0 and spot_checks 1"
        );
    }
    params
        .validate_prod_envelope()
        .map_err(|e| anyhow!("Pearl-compatible params are not production-admissible: {e}"))?;
    if params.num_tiles() > 1 {
        bail!(
            "Pearl-compatible recursive certificates require exactly one tile; current params have {} tiles",
            params.num_tiles()
        );
    }
    Ok(())
}

fn build_pearl_merge_submission_config(
    args: &Args,
    params: MatmulParams,
    a: &Arc<Vec<i8>>,
    b: &Arc<Vec<i8>>,
) -> Result<PearlMergeSubmissionConfig> {
    validate_pearl_recursive_cli_params(params)?;
    let max_pattern_len = params.tile as usize;

    let rows_pattern = contiguous_pearl_pattern(params.tile)?;
    let cols_pattern = contiguous_pearl_pattern(params.tile)?;
    let mining_config = PearlMiningConfig {
        common_dim: params.k,
        rank: u16::try_from(params.noise_rank)
            .map_err(|_| anyhow!("fixed noise_rank does not fit Pearl mining config u16"))?,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern,
        cols_pattern,
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    validate_pearl_merge_config_for_recursive_prover(&mining_config, &params, max_pattern_len)
        .map_err(|e| anyhow!("Pearl mining config is not supported for recursive proofs: {e}"))?;

    let request_timeout = Duration::from_millis(DEFAULT_PEARL_GATEWAY_TIMEOUT_MS);
    let refresh_interval = Duration::from_millis(DEFAULT_PEARL_GATEWAY_REFRESH_MS);
    let gateway = PearlGatewayMinerRpcConfig {
        transport: resolve_pearl_gateway_transport(args)?,
        request_timeout,
        refresh_interval,
    };
    let aux_template = PearlNockchainAux {
        nockchain_chain_id: DEFAULT_PEARL_NOCKCHAIN_CHAIN_ID.as_bytes().to_vec(),
        nock_block_commitment: [0u8; 32],
        nockchain_target_epoch_or_height: 0,
        extra_domain_data: Vec::new(),
    };
    aux_template
        .to_bytes()
        .map_err(|e| anyhow!("Pearl aux template is not canonical: {e}"))?;

    let mine_opts = PearlMergeMineOptions::default();

    Ok(PearlMergeSubmissionConfig::new_compact_recursive(
        gateway,
        mining_config,
        aux_template,
        max_pattern_len,
        mine_opts,
        params,
        a.clone(),
        b.clone(),
    ))
}

fn resolve_pearl_gateway_transport(args: &Args) -> Result<PearlGatewayTransport> {
    parse_pearl_gateway_endpoint(&args.pearl_gateway)
}

fn parse_pearl_gateway_endpoint(endpoint: &str) -> Result<PearlGatewayTransport> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        bail!("--pearl-gateway endpoint must not be empty");
    }

    if let Some(path) = endpoint
        .strip_prefix("unix://")
        .or_else(|| endpoint.strip_prefix("uds://"))
        .or_else(|| endpoint.strip_prefix("unix:"))
        .or_else(|| endpoint.strip_prefix("uds:"))
    {
        if path.is_empty() {
            bail!("--pearl-gateway unix endpoint path must not be empty");
        }
        return Ok(PearlGatewayTransport::UnixSocket {
            path: path.to_string(),
        });
    }

    if endpoint.starts_with('/') {
        return Ok(PearlGatewayTransport::UnixSocket {
            path: endpoint.to_string(),
        });
    }

    let tcp = endpoint
        .strip_prefix("tcp://")
        .or_else(|| endpoint.strip_prefix("tcp:"))
        .unwrap_or(endpoint);
    let Some((host, port)) = tcp.rsplit_once(':') else {
        bail!("--pearl-gateway must be unix:/path, /path, tcp:host:port, or host:port");
    };
    if host.is_empty() {
        bail!("--pearl-gateway TCP host must not be empty");
    }
    let port = port
        .parse::<u16>()
        .with_context(|| "--pearl-gateway TCP port must be a u16")?;
    Ok(PearlGatewayTransport::Tcp {
        host: host.to_string(),
        port,
    })
}

fn contiguous_pearl_pattern(tile: u32) -> Result<PearlPeriodicPattern> {
    if tile == 0 {
        bail!("fixed tile must be nonzero");
    }
    let indices: Vec<u32> = (0..tile).collect();
    PearlPeriodicPattern::from_list(&indices)
        .map_err(|e| anyhow!("contiguous Pearl pattern for tile {tile} is invalid: {e}"))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_defaults_to_pearl_gateway_source() {
        let args = Args::parse_from([
            "ai-pow-mine", "--mining-pkh",
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV",
        ]);
        assert!(build_pkh_configs(&args).is_some());
        assert_eq!(args.pearl_gateway, DEFAULT_PEARL_GATEWAY_ENDPOINT);
        assert_eq!(
            resolve_pearl_gateway_transport(&args).expect("parse default unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/tmp/pearlgw.sock".to_string()
            }
        );
        let puzzle = build_puzzle_inputs(&args).expect("default Pearl gateway config");
        assert_eq!(puzzle.params, DEFAULT_MATMUL_PARAMS);
        let (expected_a, expected_b) =
            ai_pow::synth::synth_matrices(DEFAULT_SYNTH_SEED.as_bytes(), &puzzle.params);
        assert_eq!(puzzle.a.as_slice(), expected_a.as_slice());
        assert_eq!(puzzle.b.as_slice(), expected_b.as_slice());
        puzzle
            .validate_canonical_submission_ready()
            .expect("default pearl merge submission should pass preflight");
    }

    #[test]
    fn cli_requires_v1_reward_configs() {
        let args = Args::parse_from(["ai-pow-mine"]);
        assert!(build_pkh_configs(&args).is_none());
    }

    #[test]
    fn cli_accepts_v1_reward_configs() {
        let single = Args::parse_from([
            "ai-pow-mine", "--mining-pkh",
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV",
        ]);
        let single_configs = build_pkh_configs(&single).expect("single v1 pkh config");
        assert_eq!(single_configs.len(), 1);
        assert_eq!(single_configs[0].share, 1);
        assert_eq!(
            single_configs[0].pkh,
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV"
        );

        let advanced = Args::parse_from(["ai-pow-mine", "--mining-pkh-adv", "2,first", "3,second"]);
        let advanced_configs = build_pkh_configs(&advanced).expect("advanced v1 pkh configs");
        assert_eq!(advanced_configs.len(), 2);
        assert_eq!(advanced_configs[0].share, 2);
        assert_eq!(advanced_configs[0].pkh, "first");
        assert_eq!(advanced_configs[1].share, 3);
        assert_eq!(advanced_configs[1].pkh, "second");
    }

    #[test]
    fn cli_accepts_unified_pearl_gateway_endpoint_forms() {
        let unix =
            Args::parse_from(["ai-pow-mine", "--pearl-gateway", "unix:/var/run/pearlgw.sock"]);
        assert_eq!(
            resolve_pearl_gateway_transport(&unix).expect("parse unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/var/run/pearlgw.sock".to_string()
            }
        );

        let bare_unix =
            Args::parse_from(["ai-pow-mine", "--pearl-gateway", "/var/run/pearlgw.sock"]);
        assert_eq!(
            resolve_pearl_gateway_transport(&bare_unix).expect("parse bare unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/var/run/pearlgw.sock".to_string()
            }
        );

        let tcp = Args::parse_from(["ai-pow-mine", "--pearl-gateway", "tcp://pearl.example:18443"]);
        assert_eq!(
            resolve_pearl_gateway_transport(&tcp).expect("parse tcp endpoint"),
            PearlGatewayTransport::Tcp {
                host: "pearl.example".to_string(),
                port: 18443
            }
        );
    }

    #[test]
    fn cli_rejects_malformed_unified_pearl_gateway_endpoint() {
        let args =
            Args::parse_from(["ai-pow-mine", "--pearl-gateway", "tcp://localhost:not-a-port"]);

        let err = match build_puzzle_inputs(&args) {
            Ok(_) => panic!("malformed Pearl Gateway endpoint must fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("--pearl-gateway TCP port"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn cli_help_shows_unified_gateway_endpoint_not_legacy_split_flags() {
        let help = Args::command().render_long_help().to_string();

        assert!(help.contains("--pearl-gateway <ENDPOINT>"));
        assert!(help.contains("[default: unix:/tmp/pearlgw.sock]"));
        assert!(help.contains("--node-addr <NODE_ADDR>"));
        assert!(help.contains("--mining-pkh <MINING_PKH>"));
        assert!(!help.contains("--pearl-work-source"));
        assert!(!help.contains("--pearl-gateway-transport"));
        assert!(!help.contains("--pearl-gateway-socket"));
        assert!(!help.contains("--pearl-prev-block"));
        assert!(!help.contains("--pearl-timestamp"));
        assert!(!help.contains("--pearl-nbits"));
        assert!(!help.contains("--pearl-max-attempts"));
        assert!(!help.contains("--noise-rank"));
        assert!(!help.contains("--synth-seed"));
        assert!(!help.contains("--pearl-gateway-timeout-ms"));
        assert!(!help.contains("--pearl-nockchain-chain-id"));
        assert!(!help.contains("--pearl-nockchain-target-epoch-or-height"));
        assert!(!help.contains("--pearl-extra-domain-data"));
        assert!(!help.contains("--pearl-max-pattern-len"));
        assert!(!help.contains("--reconnect-max-attempts"));
    }

    #[test]
    fn cli_rejects_legacy_pearl_gateway_split_flags() {
        let err = Args::try_parse_from([
            "ai-pow-mine", "--pearl-gateway-transport", "tcp", "--pearl-gateway-host", "127.0.0.1",
            "--pearl-gateway-port", "8337",
        ])
        .expect_err("legacy split Pearl Gateway flags should not parse");
        assert!(
            err.to_string().contains("--pearl-gateway-transport"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cli_can_build_configured_pearl_merge_submission_inputs() {
        let args = Args::parse_from(["ai-pow-mine", "--pearl-gateway", "tcp://127.0.0.1:8337"]);

        let puzzle = build_puzzle_inputs(&args).expect("pearl merge puzzle inputs");
        assert_eq!(
            resolve_pearl_gateway_transport(&args).expect("parse configured tcp endpoint"),
            PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port: 8337
            }
        );

        puzzle
            .validate_canonical_submission_ready()
            .expect("configured pearl merge submission should pass preflight");
    }

    #[test]
    fn cli_rejects_removed_pearl_aux_search_and_shape_flags() {
        for removed_flag in [
            "--pearl-nockchain-chain-id", "--pearl-nockchain-target-epoch-or-height",
            "--pearl-extra-domain-data", "--pearl-max-pattern-len", "--pearl-max-attempts",
            "--synth-seed", "--a", "--b", "--m", "--k", "--n", "--noise-rank", "--tile",
            "--spot-checks", "--difficulty-bits", "--pearl-gateway-timeout-ms",
            "--pearl-gateway-refresh-ms", "--reconnect-backoff-initial-ms",
            "--reconnect-backoff-max-ms", "--reconnect-max-attempts",
        ] {
            let err = Args::try_parse_from(["ai-pow-mine", removed_flag, "1"])
                .expect_err("removed Pearl aux/search/shape flag should not parse");
            assert!(
                err.to_string().contains(removed_flag),
                "unexpected error for {removed_flag}: {err}"
            );
        }
    }
}
