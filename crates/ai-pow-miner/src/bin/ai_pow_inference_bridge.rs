use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(feature = "gpu")]
use std::sync::Mutex;
use std::time::Duration;

#[cfg(feature = "gpu")]
use ai_pow_miner::canonical::PreparedCanonicalDenseTemplate;
#[cfg(feature = "gpu")]
use ai_pow_miner::gemma4::GEMMA4_NATIVE_PARAMS;
#[cfg(feature = "gpu")]
use ai_pow_miner::gemma4_cuda::Gemma4CudaSession;
use ai_pow_miner::inference::{
    IdleMiningBackend, IdleMiningWorker, InferenceMiningRpc, InferenceSchedulerState,
};
use anyhow::{bail, Context, Result};
use clap::Parser;
use nockapp_grpc_proto::pb::ai_pow::v1::MiningJob;
use tonic::transport::Server;

#[derive(Debug, Parser)]
#[command(name = "ai-pow-inference-bridge")]
#[command(about = "Typed vLLM control plane with mining-first idle scheduling")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:5590")]
    listen: SocketAddr,

    #[arg(long, default_value_t = 1)]
    candidate_generation: u64,

    #[arg(long, default_value_t = 3)]
    certificate_version: u32,

    #[arg(long, default_value_t = 3)]
    mock_idle_batch_ms: u64,

    /// Use a persistent native Gemma CUDA session for idle random-matrix work.
    #[arg(long)]
    cuda_device: Option<usize>,
}

enum BridgeIdleBackend {
    Timed {
        duration: Duration,
    },
    #[cfg(feature = "gpu")]
    Cuda(Mutex<CudaIdleState>),
}

#[cfg(feature = "gpu")]
struct CudaIdleState {
    session: Gemma4CudaSession,
    template: PreparedCanonicalDenseTemplate,
    extranonce: u32,
    target: [u8; 32],
}

impl IdleMiningBackend for BridgeIdleBackend {
    fn mine_one_batch(&self) -> Result<()> {
        match self {
            Self::Timed { duration } => {
                std::thread::sleep(*duration);
                Ok(())
            }
            #[cfg(feature = "gpu")]
            Self::Cuda(state) => {
                let mut state = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("CUDA idle session lock poisoned"))?;
                let prepared = state.template.prepare_search(state.extranonce)?;
                state.session.prepare(prepared.sigma(), prepared.mu())?;
                let total_tickets = state.session.total_tickets();
                let target = state.target;
                let result = state.session.search(0, total_tickets, &target)?;
                if result.winner.is_some() {
                    bail!("zero-target idle batch returned a winner");
                }
                state.extranonce = state.extranonce.wrapping_add(1);
                Ok(())
            }
        }
    }
}

fn timed_backend(args: &Args) -> (BridgeIdleBackend, Vec<u8>) {
    (
        BridgeIdleBackend::Timed {
            duration: Duration::from_millis(args.mock_idle_batch_ms),
        },
        vec![0; 76],
    )
}

#[cfg(feature = "gpu")]
fn cuda_backend(device: usize) -> Result<(BridgeIdleBackend, Vec<u8>)> {
    let params = GEMMA4_NATIVE_PARAMS;
    let (a, b) = ai_pow::synth::synth_matrices(b"nockchain-gemma4-idle-v1", &params);
    let a = Arc::new(a);
    let b = Arc::new(b);
    let template =
        PreparedCanonicalDenseTemplate::new(&params, [0x77; 32], Arc::clone(&a), Arc::clone(&b))?;
    let first = template.prepare_search(0)?;
    let session =
        Gemma4CudaSession::new_source(device, params.m as usize, params.n as usize, &a, &b)?;
    Ok((
        BridgeIdleBackend::Cuda(Mutex::new(CudaIdleState {
            session,
            template,
            extranonce: 0,
            target: [0; 32],
        })),
        first.sigma().to_vec(),
    ))
}

fn build_backend(args: &Args) -> Result<(BridgeIdleBackend, Vec<u8>)> {
    match args.cuda_device {
        Some(device) => {
            #[cfg(feature = "gpu")]
            {
                cuda_backend(device)
            }
            #[cfg(not(feature = "gpu"))]
            {
                let _ = device;
                bail!("--cuda-device requires the gpu feature")
            }
        }
        None => Ok(timed_backend(args)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let state = Arc::new(InferenceSchedulerState::default());
    let (backend, incomplete_header) = build_backend(&args)?;
    let mining_job = MiningJob {
        candidate_generation: args.candidate_generation,
        incomplete_header,
        // Zero target keeps integration runs deterministic and hit-free.
        target_le: vec![0; 32],
        certificate_version: args.certificate_version,
    };
    let rpc = InferenceMiningRpc::new(Arc::clone(&state), mining_job)?;
    let idle = IdleMiningWorker::spawn(Arc::clone(&state), Arc::new(backend));

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ai_pow_miner=info".into()),
        )
        .init();
    tracing::info!(listen = %args.listen, "starting AI-PoW inference bridge");

    let result = Server::builder()
        .add_service(rpc.into_server())
        .serve_with_shutdown(args.listen, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serve inference mining gRPC");
    idle.stop()?;
    result
}
