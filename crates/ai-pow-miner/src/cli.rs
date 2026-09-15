//! Shared node-miner command-line configuration.
//!
//! Both CPU and CUDA miner binaries flatten [`CommonArgs`], so their work,
//! reward, and node configuration is byte-for-byte identical. A search backend
//! is selected by the binary; this module never silently substitutes one.

use std::sync::Arc;
use std::time::Duration;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    validate_pearl_merge_config_for_recursive_prover, PearlMiningConfig, PearlNockchainAux,
    PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use nockchain_mining_common::MiningPkhConfig;
use tracing_subscriber::{fmt, EnvFilter};

use crate::pearl_mining::PearlMergeMineOptions;
use crate::run::{
    AiPuzzleInputs, MinerConfig, PearlGatewayMinerRpcConfig, PearlGatewayTransport,
    PearlMergeSubmissionConfig,
};
use crate::search::CpuSearchBackend;
use crate::DENSE_PRODUCTION_PARAMS;

const DEFAULT_PEARL_NOCKCHAIN_CHAIN_ID: &str = "nockchain";
const DEFAULT_PEARL_GATEWAY_ENDPOINT: &str = "unix:/tmp/pearlgw.sock";
const DEFAULT_PEARL_GATEWAY_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_PEARL_GATEWAY_REFRESH_MS: u64 = 1_000;
const DEFAULT_RECONNECT_BACKOFF_INITIAL_MS: u64 = 1_000;
const DEFAULT_RECONNECT_BACKOFF_MAX_MS: u64 = 30_000;
const DEFAULT_RECONNECT_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_MATMUL_PARAMS: MatmulParams = MatmulParams {
    m: 8,
    k: 1024,
    n: 8,
    noise_rank: 32,
    tile: 8,
    spot_checks: 1,
    difficulty_bits: 0,
};

/// Node and AI-PoW work arguments common to every miner backend.
#[derive(Args, Debug)]
pub struct CommonArgs {
    /// Node's private gRPC URL.
    #[arg(long, default_value = "http://127.0.0.1:5555")]
    pub node_addr: String,

    /// Single-recipient v1 mining pubkey hash. Mutually exclusive with --mining-pkh-adv.
    #[arg(long, conflicts_with = "mining_pkh_adv")]
    pub mining_pkh: Option<String>,

    /// Multi-recipient v1 mining pkh configs. Each entry is `share,pkh`.
    #[arg(long, value_parser = clap::value_parser!(MiningPkhConfig), num_args = 1..)]
    pub mining_pkh_adv: Option<Vec<MiningPkhConfig>>,

    /// Pearl Gateway miner RPC endpoint. Requires Gateway `getMiningInfo` jobs
    /// with `cert_version = 3`; submissions carry the same version. Accepts
    /// `unix:/path/to.sock`, `/path/to.sock`, `tcp:host:port`, `tcp://host:port`,
    /// or `host:port`. Ignored in --canonical mode.
    #[arg(long, value_name = "ENDPOINT", default_value = DEFAULT_PEARL_GATEWAY_ENDPOINT)]
    pub pearl_gateway: String,

    /// Gateway-free mode: prove a CANONICAL AI-PoW block bound to each
    /// %mine-ai candidate. CUDA mode selects the dense production backend;
    /// CPU mode retains the small diagnostic profile.
    #[arg(long)]
    pub canonical: bool,

    /// Pearl-compatible dense production benchmark shape (`m=512,k=1024,n=512,r=64,tile=8`).
    #[arg(long, conflicts_with = "canonical")]
    pub dense_production: bool,

    /// Dedicated CPU ticket-search workers. Defaults to physical core count.
    #[arg(long, value_name = "N")]
    pub mining_threads: Option<usize>,

    /// Log filter (env-filter syntax). Override with the `RUST_LOG` env var.
    #[arg(
        long,
        default_value = "info,ai_pow_miner=info,nockchain_mining_common=info"
    )]
    pub log: String,
}

impl CommonArgs {
    /// Resolve and validate the configured CPU worker count.
    pub fn mining_threads(&self) -> Result<usize> {
        let threads = self
            .mining_threads
            .unwrap_or_else(CpuSearchBackend::default_worker_count);
        if threads == 0 {
            bail!("--mining-threads must be nonzero");
        }
        Ok(threads)
    }

    /// Resolve the required v1 reward configurations.
    pub fn mining_pkh_configs(&self) -> Result<Vec<MiningPkhConfig>> {
        if let Some(pkh) = &self.mining_pkh {
            return Ok(vec![MiningPkhConfig {
                share: 1,
                pkh: pkh.clone(),
            }]);
        }
        self.mining_pkh_adv.clone().ok_or_else(|| {
            anyhow!("must supply --mining-pkh <HASH> or --mining-pkh-adv \"share,pkh\"")
        })
    }

    /// Build the matrix and Pearl-Gateway configuration for a node miner.
    pub fn build_puzzle_inputs(&self) -> Result<AiPuzzleInputs> {
        let params = if self.dense_production {
            DENSE_PRODUCTION_PARAMS
        } else {
            DEFAULT_MATMUL_PARAMS
        };
        params
            .validate()
            .map_err(|e| anyhow!("matmul params invalid: {e}"))?;
        validate_pearl_recursive_cli_params(params)?;

        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let a = Arc::new(a);
        let b = Arc::new(b);
        let pearl_merge = build_pearl_merge_submission_config(self, params, &a, &b)?;
        Ok(AiPuzzleInputs {
            params,
            a,
            b,
            pearl_merge,
        })
    }

    /// Build a fully validated non-canonical node-miner configuration.
    pub fn build_miner_config(&self) -> Result<MinerConfig> {
        Ok(MinerConfig {
            node_addr: self.node_addr.clone(),
            mining_pkh_configs: self.mining_pkh_configs()?,
            puzzle: self.build_puzzle_inputs()?,
            reconnect_backoff_initial: Duration::from_millis(DEFAULT_RECONNECT_BACKOFF_INITIAL_MS),
            reconnect_backoff_max: Duration::from_millis(DEFAULT_RECONNECT_BACKOFF_MAX_MS),
            reconnect_max_attempts: DEFAULT_RECONNECT_MAX_ATTEMPTS,
            mining_threads: self.mining_threads()?,
        })
    }
}

/// Initialize the shared stderr tracing subscriber.
pub fn init_tracing(filter: &str) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    let _ = fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
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
    if params.num_tiles() > 1 && params != DENSE_PRODUCTION_PARAMS {
        bail!(
            "Pearl-compatible recursive certificates require the default single-tile shape or --dense-production's admitted shape; current params have {} tiles",
            params.num_tiles()
        );
    }
    Ok(())
}

fn build_pearl_merge_submission_config(
    args: &CommonArgs,
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

    let gateway = PearlGatewayMinerRpcConfig {
        transport: parse_pearl_gateway_endpoint(&args.pearl_gateway)?,
        request_timeout: Duration::from_millis(DEFAULT_PEARL_GATEWAY_TIMEOUT_MS),
        refresh_interval: Duration::from_millis(DEFAULT_PEARL_GATEWAY_REFRESH_MS),
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

    Ok(PearlMergeSubmissionConfig::new_compact_recursive(
        gateway,
        mining_config,
        aux_template,
        max_pattern_len,
        PearlMergeMineOptions::default(),
        params,
        a.clone(),
        b.clone(),
    ))
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
    use clap::{CommandFactory, Parser};

    use super::*;

    #[derive(Parser, Debug)]
    #[command(name = "ai-pow-mine")]
    struct TestArgs {
        #[command(flatten)]
        common: CommonArgs,
    }

    fn parse(arguments: &[&str]) -> CommonArgs {
        TestArgs::parse_from(arguments).common
    }

    #[test]
    fn cli_defaults_to_pearl_gateway_source() {
        let args = parse(&[
            "ai-pow-mine", "--mining-pkh",
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV",
        ]);
        assert!(args.mining_pkh_configs().is_ok());
        assert_eq!(args.pearl_gateway, DEFAULT_PEARL_GATEWAY_ENDPOINT);
        assert_eq!(
            parse_pearl_gateway_endpoint(&args.pearl_gateway).expect("parse default unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/tmp/pearlgw.sock".to_string()
            }
        );
        let puzzle = args
            .build_puzzle_inputs()
            .expect("default Pearl gateway config");
        assert_eq!(puzzle.params, DEFAULT_MATMUL_PARAMS);
        let (expected_a, expected_b) =
            ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &puzzle.params);
        assert_eq!(puzzle.a.as_slice(), expected_a.as_slice());
        assert_eq!(puzzle.b.as_slice(), expected_b.as_slice());
        puzzle
            .validate_canonical_submission_ready()
            .expect("default pearl merge submission should pass preflight");
    }

    #[test]
    fn cli_dense_production_uses_only_admitted_multi_tile_shape() {
        let args = parse(&[
            "ai-pow-mine", "--dense-production", "--mining-pkh",
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV",
        ]);
        let puzzle = args
            .build_puzzle_inputs()
            .expect("dense production Pearl gateway config");
        assert_eq!(puzzle.params, DENSE_PRODUCTION_PARAMS);
        assert!(puzzle.params.num_tiles() > 1);
        puzzle
            .validate_canonical_submission_ready()
            .expect("dense production shape should pass the named preflight");
    }

    #[test]
    fn cli_requires_v1_reward_configs() {
        let args = parse(&["ai-pow-mine"]);
        assert!(args.mining_pkh_configs().is_err());
    }

    #[test]
    fn cli_accepts_v1_reward_configs() {
        let single = parse(&[
            "ai-pow-mine", "--mining-pkh",
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV",
        ]);
        let single_configs = single.mining_pkh_configs().expect("single v1 pkh config");
        assert_eq!(single_configs.len(), 1);
        assert_eq!(single_configs[0].share, 1);
        assert_eq!(
            single_configs[0].pkh,
            "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV"
        );

        let advanced = parse(&["ai-pow-mine", "--mining-pkh-adv", "2,first", "3,second"]);
        let advanced_configs = advanced
            .mining_pkh_configs()
            .expect("advanced v1 pkh configs");
        assert_eq!(advanced_configs.len(), 2);
        assert_eq!(advanced_configs[0].share, 2);
        assert_eq!(advanced_configs[0].pkh, "first");
        assert_eq!(advanced_configs[1].share, 3);
        assert_eq!(advanced_configs[1].pkh, "second");
    }

    #[test]
    fn cli_accepts_unified_pearl_gateway_endpoint_forms() {
        let unix = parse(&["ai-pow-mine", "--pearl-gateway", "unix:/var/run/pearlgw.sock"]);
        assert_eq!(
            parse_pearl_gateway_endpoint(&unix.pearl_gateway).expect("parse unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/var/run/pearlgw.sock".to_string()
            }
        );

        let bare_unix = parse(&["ai-pow-mine", "--pearl-gateway", "/var/run/pearlgw.sock"]);
        assert_eq!(
            parse_pearl_gateway_endpoint(&bare_unix.pearl_gateway)
                .expect("parse bare unix endpoint"),
            PearlGatewayTransport::UnixSocket {
                path: "/var/run/pearlgw.sock".to_string()
            }
        );

        let tcp = parse(&["ai-pow-mine", "--pearl-gateway", "tcp://pearl.example:18443"]);
        assert_eq!(
            parse_pearl_gateway_endpoint(&tcp.pearl_gateway).expect("parse tcp endpoint"),
            PearlGatewayTransport::Tcp {
                host: "pearl.example".to_string(),
                port: 18443
            }
        );
    }

    #[test]
    fn cli_can_build_configured_pearl_merge_submission_inputs() {
        let args = parse(&["ai-pow-mine", "--pearl-gateway", "tcp://127.0.0.1:8337"]);

        let puzzle = args
            .build_puzzle_inputs()
            .expect("Pearl merge puzzle inputs");
        assert_eq!(
            parse_pearl_gateway_endpoint(&args.pearl_gateway).expect("parse configured TCP"),
            PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port: 8337
            }
        );
        puzzle
            .validate_canonical_submission_ready()
            .expect("configured Pearl merge submission should pass preflight");
    }

    #[test]
    fn cli_rejects_malformed_unified_pearl_gateway_endpoint() {
        let args = parse(&["ai-pow-mine", "--pearl-gateway", "tcp://localhost:not-a-port"]);
        let err = match args.build_puzzle_inputs() {
            Ok(_) => panic!("malformed Pearl Gateway endpoint must fail"),
            Err(error) => error,
        };
        assert!(
            err.to_string().contains("--pearl-gateway TCP port"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn cli_help_shows_unified_gateway_endpoint_not_legacy_split_flags() {
        let help = TestArgs::command().render_long_help().to_string();
        assert!(help.contains("--pearl-gateway <ENDPOINT>"));
        assert!(help.contains("[default: unix:/tmp/pearlgw.sock]"));
        assert!(help.contains("--node-addr <NODE_ADDR>"));
        assert!(help.contains("--mining-pkh <MINING_PKH>"));
        assert!(help.contains("--dense-production"));
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
        let err = TestArgs::try_parse_from([
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
    fn cli_rejects_removed_pearl_aux_search_and_shape_flags() {
        for removed_flag in [
            "--pearl-nockchain-chain-id", "--pearl-nockchain-target-epoch-or-height",
            "--pearl-extra-domain-data", "--pearl-max-pattern-len", "--pearl-max-attempts",
            "--synth-seed", "--a", "--b", "--m", "--k", "--n", "--noise-rank", "--tile",
            "--spot-checks", "--difficulty-bits", "--pearl-gateway-timeout-ms",
            "--pearl-gateway-refresh-ms", "--reconnect-backoff-initial-ms",
            "--reconnect-backoff-max-ms", "--reconnect-max-attempts",
        ] {
            let err = TestArgs::try_parse_from(["ai-pow-mine", removed_flag, "1"])
                .expect_err("removed Pearl aux/search/shape flag should not parse");
            assert!(
                err.to_string().contains(removed_flag),
                "unexpected error for {removed_flag}: {err}"
            );
        }
    }
}
