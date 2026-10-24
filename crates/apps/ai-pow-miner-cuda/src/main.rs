//! `ai-pow-mine-cuda` — CUDA-backed standalone AI-PoW block miner.

use std::process::ExitCode;

use ai_pow_miner::cli::{init_tracing, CommonArgs};
use ai_pow_miner_cuda::CudaSearchBackend;
use anyhow::Result;
use clap::{Args, Parser};
use tracing::{error, info};

const DEFAULT_GPU_BATCH_ATTEMPTS: u64 = 4_096;

#[derive(Parser, Debug)]
#[command(
    name = "ai-pow-mine-cuda",
    about = "CUDA-backed AI-PoW block miner. CUDA startup failures are fatal; this binary never falls back to CPU search.",
    version
)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// CUDA device ordinal.
    #[arg(long, default_value_t = 0)]
    cuda_device: usize,

    /// Ticket attempts launched before cancellation is observed.
    #[arg(long, default_value_t = DEFAULT_GPU_BATCH_ATTEMPTS, value_name = "N")]
    gpu_batch_attempts: u64,
}

fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(&args.common.log);
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error!(error = %error, "ai-pow-mine-cuda: startup failed");
            ExitCode::from(1)
        }
    }
}

fn run(args: Args) -> Result<()> {
    let backend = CudaSearchBackend::new(args.cuda_device, args.gpu_batch_attempts)?;
    info!(
        cuda_device = backend.device_ordinal(),
        gpu_batch_attempts = backend.gpu_batch_attempts(),
        "ai-pow-mine-cuda: CUDA search backend initialized"
    );
    anyhow::bail!("CUDA ticket kernels are unavailable")
}
