use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "gpu")]
use std::sync::Mutex;
use std::time::Duration;

#[cfg(feature = "gpu")]
use ai_pow::pearl_compat::PearlIncompleteBlockHeader;
#[cfg(feature = "gpu")]
use ai_pow_miner::canonical::PreparedCanonicalDenseTemplate;
#[cfg(feature = "gpu")]
use ai_pow_miner::gemma4::GEMMA4_NATIVE_PARAMS;
use ai_pow_miner::gemma4::{Gemma4Checkpoint, GEMMA4_CHECKPOINT_CONTENT_DIGEST};
#[cfg(feature = "gpu")]
use ai_pow_miner::gemma4_cuda::Gemma4CudaSession;
use ai_pow_miner::inference::{
    build_gemma4_mining_job, IdleMiningBackend, IdleMiningWorker, InferenceMiningRpc,
    InferenceProofSender, InferenceSchedulerState,
};
#[cfg(feature = "gpu")]
use ai_pow_miner::inference::{InferenceProofRequest, OpenedDenseWitness};
use ai_pow_miner::run::{inference_proof_channel, run_inference_node, InferenceNodeConfig};
use anyhow::{bail, Context, Result};
use clap::Parser;
use nockchain_mining_common::MiningPkhConfig;
use tokio_util::sync::CancellationToken;
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

    /// Node's private gRPC URL. Supplying it enables proof-producing mining.
    #[arg(long)]
    node_addr: Option<String>,

    /// Single-recipient v1 mining public-key hash.
    #[arg(long, conflicts_with = "mining_pkh_adv")]
    mining_pkh: Option<String>,

    /// Multi-recipient v1 mining configurations. Each entry is `share,pkh`.
    #[arg(long, value_parser = clap::value_parser!(MiningPkhConfig), num_args = 1..)]
    mining_pkh_adv: Option<Vec<MiningPkhConfig>>,

    #[arg(long, default_value_t = 3)]
    mock_idle_batch_ms: u64,

    /// Use a persistent native Gemma CUDA session for idle random-matrix work.
    #[arg(long)]
    cuda_device: Option<usize>,

    /// Validated Gemma checkpoint directory used by the CUDA runtime.
    #[arg(long)]
    checkpoint_path: Option<PathBuf>,

    /// Validate the checkpoint content and exit before CUDA initialization.
    #[arg(long)]
    verify_checkpoint_only: bool,
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
    a: Arc<Vec<i8>>,
    b: Arc<Vec<i8>>,
    candidate_generation: u64,
    extranonce: u32,
    rpc: Option<InferenceMiningRpc>,
    proof_sender: Option<InferenceProofSender>,
}

impl BridgeIdleBackend {
    fn configure_production(
        &mut self,
        rpc: InferenceMiningRpc,
        proof_sender: InferenceProofSender,
    ) {
        #[cfg(feature = "gpu")]
        if let Self::Cuda(state) = self {
            let state = state.get_mut().expect("new CUDA idle mutex");
            state.rpc = Some(rpc);
            state.proof_sender = Some(proof_sender);
        }
        #[cfg(not(feature = "gpu"))]
        let _ = (rpc, proof_sender);
    }
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
                let prepared = state.template.prepare_search(0)?;
                let Some(rpc) = &state.rpc else {
                    state.session.prepare(prepared.sigma(), prepared.mu())?;
                    let total_tickets = state.session.total_tickets();
                    state.session.search(0, total_tickets, &[0; 32])?;
                    return Ok(());
                };
                let mining_job = rpc.mining_job()?;
                if state.candidate_generation != mining_job.candidate_generation {
                    state.candidate_generation = mining_job.candidate_generation;
                    state.extranonce = 0;
                }
                let mut header =
                    PearlIncompleteBlockHeader::from_bytes(&mining_job.incomplete_header)?;
                header.timestamp = header.timestamp.wrapping_add(state.extranonce);
                let preparation = state
                    .session
                    .prepare(&header.to_bytes(), &mining_job.mining_config)?;
                let target: [u8; 32] = mining_job
                    .effective_target_le
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("effective target must be 32 bytes"))?;
                let total_tickets = state.session.total_tickets();
                let result = state.session.search(0, total_tickets, &target)?;
                let Some(ordinal) = result.winner else {
                    state.extranonce = state.extranonce.wrapping_add(1);
                    return Ok(());
                };
                let proof_sender = state.proof_sender.clone().ok_or_else(|| {
                    anyhow::anyhow!("idle winner proof handoff is not configured")
                })?;
                let params = GEMMA4_NATIVE_PARAMS;
                let col_tickets = u64::from(params.n / params.tile);
                let row_start = u32::try_from(ordinal / col_tickets)?
                    .checked_mul(params.tile)
                    .ok_or_else(|| anyhow::anyhow!("idle winner row overflow"))?;
                let column_start = u32::try_from(ordinal % col_tickets)?
                    .checked_mul(params.tile)
                    .ok_or_else(|| anyhow::anyhow!("idle winner column overflow"))?;
                let current_extranonce = state.extranonce;
                state.extranonce = state.extranonce.wrapping_add(1);
                let witness = OpenedDenseWitness {
                    candidate_generation: mining_job.candidate_generation,
                    work_id: 0,
                    extranonce: current_extranonce,
                    a_row_indices: (row_start..row_start + params.tile).collect(),
                    b_column_indices: (column_start..column_start + params.tile).collect(),
                    noise_seed_a: preparation.commitments.s_a,
                    noise_seed_b: preparation.commitments.s_b,
                    noise_rank: params.noise_rank,
                    a_rows: params.m,
                    b_columns: params.n,
                    common_dim: params.k,
                    a: Arc::clone(&state.a),
                    b_transposed: Arc::clone(&state.b),
                };
                let (response, response_rx) = tokio::sync::oneshot::channel();
                proof_sender
                    .blocking_send(InferenceProofRequest { witness, response })
                    .map_err(|_| anyhow::anyhow!("inference proof worker is unavailable"))?;
                match response_rx.blocking_recv()? {
                    Ok(detail) => {
                        tracing::info!(%detail, "idle inference-bridge winner submitted");
                        Ok(())
                    }
                    Err(detail) => bail!("{detail}"),
                }
            }
        }
    }
}

fn timed_backend(args: &Args) -> (BridgeIdleBackend, [u8; 76]) {
    (
        BridgeIdleBackend::Timed {
            duration: Duration::from_millis(args.mock_idle_batch_ms),
        },
        [0; 76],
    )
}

#[cfg(feature = "gpu")]
fn cuda_backend(device: usize) -> Result<(BridgeIdleBackend, [u8; 76])> {
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
            a,
            b,
            candidate_generation: 0,
            extranonce: 0,
            rpc: None,
            proof_sender: None,
        })),
        *first.sigma(),
    ))
}

fn build_backend(args: &Args) -> Result<(BridgeIdleBackend, [u8; 76])> {
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

fn checkpoint_preflight(args: &Args) -> Result<Option<[u8; 32]>> {
    let path = match args.checkpoint_path.as_ref() {
        Some(path) => path,
        None if args.cuda_device.is_none() && args.node_addr.is_none() => return Ok(None),
        None => bail!("--cuda-device or --node-addr requires --checkpoint-path"),
    };
    let checkpoint = Gemma4Checkpoint::open(path)
        .with_context(|| format!("validate Gemma checkpoint {}", path.display()))?;
    let actual = checkpoint
        .content_digest()
        .with_context(|| format!("hash Gemma checkpoint {}", path.display()))?;
    if actual != GEMMA4_CHECKPOINT_CONTENT_DIGEST {
        bail!(
            "checkpoint content digest mismatch: expected {}, got {}",
            hex::encode(GEMMA4_CHECKPOINT_CONTENT_DIGEST),
            hex::encode(actual)
        );
    }
    Ok(Some(actual))
}

fn node_config(args: &Args) -> Result<Option<InferenceNodeConfig>> {
    let mining_pkh_configs = if let Some(pkh) = &args.mining_pkh {
        vec![MiningPkhConfig {
            share: 1,
            pkh: pkh.clone(),
        }]
    } else {
        args.mining_pkh_adv.clone().unwrap_or_default()
    };
    match &args.node_addr {
        Some(node_addr) if mining_pkh_configs.is_empty() => {
            bail!("--node-addr requires --mining-pkh or --mining-pkh-adv")
        }
        Some(node_addr) => Ok(Some(InferenceNodeConfig {
            node_addr: node_addr.clone(),
            mining_pkh_configs,
        })),
        None if !mining_pkh_configs.is_empty() => {
            bail!("reward public-key hashes require --node-addr")
        }
        None => Ok(None),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let checkpoint_content_digest = checkpoint_preflight(&args)?;
    if args.verify_checkpoint_only {
        let digest = checkpoint_content_digest
            .ok_or_else(|| anyhow::anyhow!("checkpoint verification requires checkpoint inputs"))?;
        println!("{}", hex::encode(digest));
        return Ok(());
    }
    let node_config = node_config(&args)?;
    let state = Arc::new(InferenceSchedulerState::default());
    let (mut backend, incomplete_header) = build_backend(&args)?;
    let mining_job = build_gemma4_mining_job(
        args.candidate_generation, incomplete_header,
        // A zero target keeps standalone integration runs deterministic and hit-free.
        [0; 32], args.certificate_version,
    )?;
    let (proof_sender, proof_requests) = inference_proof_channel();
    let rpc = InferenceMiningRpc::new(Arc::clone(&state), mining_job)?
        .with_checkpoint_content_digest(GEMMA4_CHECKPOINT_CONTENT_DIGEST);
    let rpc = if node_config.is_some() {
        backend.configure_production(rpc.clone(), proof_sender.clone());
        rpc.with_proof_sender(proof_sender)
    } else {
        rpc
    };
    let idle = IdleMiningWorker::spawn(Arc::clone(&state), Arc::new(backend));

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ai_pow_miner=info".into()),
        )
        .init();
    tracing::info!(
        listen = %args.listen,
        production_node = node_config.is_some(),
        "starting AI-PoW inference bridge"
    );

    let shutdown = CancellationToken::new();
    let signal_shutdown = shutdown.clone();
    let signal = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_shutdown.cancel();
    });
    let server_shutdown = shutdown.clone();
    let server = Server::builder()
        .add_service(rpc.clone().into_server())
        .serve_with_shutdown(args.listen, async move {
            server_shutdown.cancelled().await;
        });
    let result = if let Some(node_config) = node_config {
        tokio::try_join!(
            async { server.await.context("serve inference mining gRPC") },
            async {
                run_inference_node(node_config, rpc, proof_requests, shutdown.clone())
                    .await
                    .context("run inference mining node integration")
            }
        )
        .map(|_| ())
    } else {
        server.await.context("serve inference mining gRPC")
    };
    shutdown.cancel();
    signal.abort();
    let idle_result = idle.stop();
    result?;
    idle_result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Args {
        Args {
            listen: "127.0.0.1:5590".parse().unwrap(),
            candidate_generation: 1,
            certificate_version: 3,
            node_addr: None,
            mining_pkh: None,
            mining_pkh_adv: None,
            mock_idle_batch_ms: 1,
            cuda_device: None,
            checkpoint_path: None,
            verify_checkpoint_only: false,
        }
    }

    #[test]
    fn checkpoint_content_digest_is_pinned() {
        assert_eq!(
            hex::encode(GEMMA4_CHECKPOINT_CONTENT_DIGEST),
            "c59cb83550f52b26893c1837133555bf32190495372ce00935d989592515ff40"
        );
    }

    #[test]
    fn cuda_backend_requires_checkpoint_preflight() {
        let mut args = args();
        args.cuda_device = Some(0);
        let error = checkpoint_preflight(&args).unwrap_err();
        assert!(error.to_string().contains("--checkpoint-path"));
    }
}
