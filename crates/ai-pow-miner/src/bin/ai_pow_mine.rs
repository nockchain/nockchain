//! `ai-pow-mine` — standalone AI-PoW (matmul puzzle) block miner.
//!
//! Mirrors `zk-pow-mine` in shape: connects to a `nockchain` node's
//! private NockAppService gRPC, subscribes to `%mine-ai` candidate
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

use ai_pow_miner::cli::{init_tracing, CommonArgs};
use ai_pow_miner::run::{run_canonical_with_backend, run_with_backend, MinerError};
use ai_pow_miner::search::{CpuSearchBackend, SearchBackend};
use clap::{Args as ClapArgs, Parser};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

/// `ai-pow-mine` — standalone AI-PoW block miner.
#[derive(Parser, Debug)]
#[command(
    name = "ai-pow-mine",
    about = "Standalone AI-PoW block miner. Mines Pearl-compatible tickets and submits recursive %ai-pow commands to a nockchain node.",
    version
)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    accelerator: AcceleratorArgs,
}

#[derive(ClapArgs, Debug)]
struct AcceleratorArgs {
    /// Use CUDA for Pearl opened-tile GEMM search.
    #[cfg(feature = "gpu")]
    #[arg(long)]
    gpu: bool,

    /// CUDA device ordinal. The current backend supports device 0.
    #[cfg(feature = "gpu")]
    #[arg(long, default_value_t = 0)]
    cuda_device: usize,

    /// Attempts dispatched through CUDA before the scheduler checks cancellation.
    #[cfg(feature = "gpu")]
    #[arg(long, default_value_t = 256)]
    gpu_batch_attempts: u64,
}

fn search_backend(args: &Args) -> Result<Arc<dyn SearchBackend>, String> {
    #[cfg(feature = "gpu")]
    if args.accelerator.gpu {
        let backend = ai_pow_miner::gpu::GpuSearchBackend::new(
            args.accelerator.cuda_device, args.accelerator.gpu_batch_attempts,
        )
        .map_err(|error| error.to_string())?;
        info!(
            cuda_device = backend.device_ordinal(),
            gpu_batch_attempts = backend.batch_attempts(),
            "ai-pow-mine: CUDA search enabled"
        );
        return Ok(Arc::new(backend));
    }
    let workers = args
        .common
        .mining_threads()
        .map_err(|error| error.to_string())?;
    CpuSearchBackend::new(workers)
        .map(|backend| Arc::new(backend) as Arc<dyn SearchBackend>)
        .map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(&args.common.log);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("ai-pow-mine: failed to build tokio runtime: {error}");
            return ExitCode::from(1);
        }
    };

    let result = if args.common.canonical {
        let backend = match search_backend(&args) {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("ai-pow-mine: cannot initialize search backend: {error}");
                return ExitCode::from(1);
            }
        };
        let pkh_configs = match args.common.mining_pkh_configs() {
            Ok(configs) => configs,
            Err(error) => {
                eprintln!("ai-pow-mine: {error:#}");
                return ExitCode::from(1);
            }
        };
        let node_addr = args.common.node_addr.clone();
        rt.block_on(async move {
            info!(node = %node_addr, "ai-pow-mine: starting canonical miner");
            let shutdown = CancellationToken::new();
            let shutdown_clone = shutdown.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    info!("ai-pow-mine: SIGINT received; shutting down");
                    shutdown_clone.cancel();
                }
            });
            run_canonical_with_backend(node_addr, pkh_configs, shutdown, backend).await
        })
    } else {
        let cfg = match args.common.build_miner_config() {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("ai-pow-mine: invalid puzzle config: {error:#}");
                return ExitCode::from(1);
            }
        };
        let backend = match search_backend(&args) {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("ai-pow-mine: cannot initialize search backend: {error}");
                return ExitCode::from(1);
            }
        };
        rt.block_on(async {
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
            run_with_backend(cfg, shutdown, backend).await
        })
    };

    match result {
        Ok(()) => {
            info!("ai-pow-mine: clean shutdown");
            ExitCode::SUCCESS
        }
        Err(MinerError::TooManyReconnects { count }) => {
            error!("ai-pow-mine: gave up after {count} consecutive reconnect failures");
            ExitCode::from(2)
        }
        Err(error) => {
            error!(error = %error, "ai-pow-mine: unrecoverable error");
            ExitCode::from(1)
        }
    }
}
