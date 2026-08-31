//! NockApp candidate subscription, inference winner reconstruction, and submission.

use std::sync::Arc;
use std::time::Duration;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::PearlMergeCheckedTicketAttempt;
use futures::StreamExt;
use nockapp::nockapp::wire::Wire;
use nockchain_mining_common::{
    MiningCandidateKind, MiningPkhConfig, NodeClient, NodeClientError, PreparedPoke,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::canonical::{
    canonical_dense_incomplete_header, CanonicalDenseBlock, CanonicalProveError,
    PreparedCanonicalDenseTemplate,
};
use crate::inference::{
    build_gemma4_mining_job, InferenceMiningRpc, InferenceProofRequest, InferenceProofSender,
    OpenedDenseWitness,
};
use crate::run::{
    build_dense_poke, derive_nockchain_candidate_inputs, MinerError, NockchainCandidateInputs,
    NODE_POKE_ACK_TIMEOUT, PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
};
use crate::wire::AiPowMinerWire;

pub(crate) const INFERENCE_DENSE_TILE: u32 = 16;
pub struct InferenceNodeConfig {
    pub node_addr: String,
    pub mining_pkh_configs: Vec<MiningPkhConfig>,
}

impl InferenceNodeConfig {
    pub fn validate(&self) -> Result<(), MinerError> {
        if self.node_addr.trim().is_empty() {
            return Err(MinerError::InvalidConfig(
                "inference node address must not be empty".to_string(),
            ));
        }
        if self.mining_pkh_configs.is_empty() {
            return Err(MinerError::InvalidConfig(
                "inference mining requires at least one reward public-key hash".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn inference_proof_channel() -> (
    InferenceProofSender,
    tokio::sync::mpsc::Receiver<InferenceProofRequest>,
) {
    tokio::sync::mpsc::channel(1)
}

type InferenceProofWorkerResult = (
    u64,
    tokio::sync::oneshot::Sender<Result<String, String>>,
    Result<PreparedPoke, String>,
);

enum InferenceProofOutcome {
    Joined(Result<InferenceProofWorkerResult, tokio::task::JoinError>),
    Pending,
}

async fn await_inference_proof_worker(
    worker: &mut Option<JoinHandle<InferenceProofWorkerResult>>,
) -> InferenceProofOutcome {
    match worker.as_mut() {
        Some(handle) => InferenceProofOutcome::Joined(handle.await),
        None => {
            std::future::pending::<()>().await;
            InferenceProofOutcome::Pending
        }
    }
}

async fn reject_and_await_inference_proof_worker(
    worker: &mut Option<JoinHandle<InferenceProofWorkerResult>>,
    detail: &str,
) -> Result<(), MinerError> {
    if let Some(handle) = worker.take() {
        let (_, response, _) = handle
            .await
            .map_err(|error| MinerError::WorkerJoin(error.to_string()))?;
        let _ = response.send(Err(detail.to_string()));
    }
    Ok(())
}

pub(crate) fn publish_inference_job(
    rpc: &InferenceMiningRpc,
    candidate: InferenceNodeCandidate,
) -> Result<(), MinerError> {
    let job = build_gemma4_mining_job(
        candidate.generation, candidate.incomplete_header, candidate.inputs.target,
        PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
    )
    .map_err(|error| MinerError::Configure(format!("build inference mining job: {error}")))?;
    rpc.set_mining_job(job)
        .map_err(|error| MinerError::Configure(format!("publish inference mining job: {error}")))
}

pub(crate) fn invalidate_inference_job(
    rpc: &InferenceMiningRpc,
    generation: u64,
) -> Result<(), MinerError> {
    let incomplete_header = canonical_dense_incomplete_header([0; 32], 0)
        .map_err(|error| MinerError::Configure(error.to_string()))?;
    let job = build_gemma4_mining_job(
        generation, incomplete_header, [0; 32], PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
    )
    .map_err(|error| MinerError::Configure(format!("build invalid inference job: {error}")))?;
    rpc.set_mining_job(job)
        .map_err(|error| MinerError::Configure(format!("invalidate inference mining job: {error}")))
}

/// Subscribe the inference bridge to `%mine-ai`, prove scalar-rechecked native
/// winners, and submit the canonical `%ai-pow` command.
pub async fn run_inference_node(
    cfg: InferenceNodeConfig,
    rpc: InferenceMiningRpc,
    mut proof_requests: tokio::sync::mpsc::Receiver<InferenceProofRequest>,
    shutdown: CancellationToken,
) -> Result<(), MinerError> {
    cfg.validate()?;
    rpc.set_production_enabled(true);
    rpc.set_node_connected(false);
    let mut generation = rpc.candidate_generation();
    let mut active_candidate: Option<InferenceNodeCandidate> = None;
    info!(node = %cfg.node_addr, "inference bridge: entering production node loop");

    loop {
        if shutdown.is_cancelled() {
            rpc.set_node_connected(false);
            return Ok(());
        }
        let mut client = match NodeClient::connect(&cfg.node_addr).await {
            Ok(client) => client,
            Err(error) => {
                warn!(error = %error, "inference bridge node connect failed; retrying in 2s");
                tokio::select! {
                    _ = shutdown.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
                continue;
            }
        };
        client
            .set_mining_key(
                AiPowMinerWire::SetPubKey.to_wire(),
                Vec::new(),
                cfg.mining_pkh_configs.clone(),
            )
            .await
            .map_err(|error| MinerError::Configure(format!("set_mining_key: {error}")))?;
        let mut candidates = match client.watch_candidates(vec![b"mine-ai".to_vec()]).await {
            Ok(candidates) => candidates,
            Err(error) => {
                warn!(error = %error, "inference bridge watch_candidates failed; reconnecting");
                continue;
            }
        };
        client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), true)
            .await
            .map_err(|error| MinerError::Configure(format!("enable_mining(true): {error}")))?;
        rpc.set_node_connected(true);
        info!("inference bridge: subscribed + mining enabled; awaiting %mine-ai candidates");

        let mut proof_worker: Option<JoinHandle<InferenceProofWorkerResult>> = None;
        let reconnect = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    rpc.set_node_connected(false);
                    reject_and_await_inference_proof_worker(
                        &mut proof_worker,
                        "inference proof cancelled during shutdown",
                    ).await?;
                    let _ = client
                        .enable_mining(AiPowMinerWire::Enable.to_wire(), false)
                        .await;
                    return Ok(());
                }
                maybe_candidate = candidates.next() => {
                    let Some(candidate_result) = maybe_candidate else {
                        warn!("inference bridge candidate stream ended; reconnecting");
                        break true;
                    };
                    let candidate = match candidate_result {
                        Ok(candidate) => candidate,
                        Err(NodeClientError::Grpc(error)) => {
                            warn!(error = %error, "inference bridge candidate stream failed; reconnecting");
                            break true;
                        }
                        Err(error) => {
                            warn!(error = %error, "inference bridge candidate decode error; skipping");
                            continue;
                        }
                    };
                    if candidate.kind != MiningCandidateKind::Ai {
                        continue;
                    }
                    let inputs = match derive_nockchain_candidate_inputs(&candidate) {
                        Ok(inputs) => inputs,
                        Err(error) => {
                            warn!(error = %error, "inference bridge rejected node candidate");
                            continue;
                        }
                    };
                    generation = generation.checked_add(1).ok_or_else(|| {
                        MinerError::Configure("inference candidate generation overflow".to_string())
                    })?;
                    let incomplete_header = canonical_dense_incomplete_header(
                        inputs.nock_block_commitment,
                        0,
                    ).map_err(|error| MinerError::Configure(error.to_string()))?;
                    let candidate = InferenceNodeCandidate {
                        generation,
                        inputs,
                        incomplete_header,
                    };
                    publish_inference_job(&rpc, candidate)?;
                    active_candidate = Some(candidate);
                    info!(
                        generation,
                        commit = %hex::encode(inputs.nock_block_commitment),
                        pow_len = inputs.pow_len,
                        target = %hex::encode(inputs.target),
                        "inference bridge published %mine-ai candidate"
                    );
                }
                maybe_request = proof_requests.recv() => {
                    let Some(request) = maybe_request else {
                        break false;
                    };
                    let Some(candidate) = active_candidate else {
                        let _ = request.response.send(Err(
                            "no active %mine-ai candidate".to_string()
                        ));
                        continue;
                    };
                    if request.witness.candidate_generation != candidate.generation {
                        let _ = request.response.send(Err(format!(
                            "stale inference witness generation {}; current generation is {}",
                            request.witness.candidate_generation,
                            candidate.generation
                        )));
                        continue;
                    }
                    if proof_worker.is_some() {
                        let _ = request.response.send(Err(
                            "an inference winner proof is already in progress".to_string()
                        ));
                        continue;
                    }
                    proof_worker = Some(tokio::task::spawn_blocking(move || {
                        prepare_inference_proof(candidate, request)
                    }));
                }
                outcome = await_inference_proof_worker(&mut proof_worker) => {
                    let InferenceProofOutcome::Joined(joined) = outcome else {
                        continue;
                    };
                    proof_worker = None;
                    let (proof_generation, response, prepared) = joined
                        .map_err(|error| MinerError::WorkerJoin(error.to_string()))?;
                    if proof_generation != generation {
                        let _ = response.send(Err(format!(
                            "candidate generation changed from {proof_generation} to {generation} while proving"
                        )));
                        continue;
                    }
                    let prepared = match prepared {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            let _ = response.send(Err(format!(
                                "inference winner proof rejected: {error}"
                            )));
                            continue;
                        }
                    };
                    let outcome = client
                        .send_prepared_poke_with_timeout_or_cancel(
                            prepared,
                            NODE_POKE_ACK_TIMEOUT,
                            shutdown.cancelled(),
                        )
                        .await;
                    let response_result = outcome.into_result().map(
                        |_| "canonical %ai-pow submission acknowledged by node".to_string()
                    ).map_err(
                        |error| format!("canonical %ai-pow submission failed: {error}")
                    );
                    let _ = response.send(response_result);
                }
            }
        };

        rpc.set_node_connected(false);
        reject_and_await_inference_proof_worker(
            &mut proof_worker, "candidate stream disconnected while proving",
        )
        .await?;
        generation = generation.checked_add(1).ok_or_else(|| {
            MinerError::Configure("inference candidate generation overflow".to_string())
        })?;
        active_candidate = None;
        invalidate_inference_job(&rpc, generation)?;
        let _ = client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), false)
            .await;
        if !reconnect {
            return Ok(());
        }
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct InferenceNodeCandidate {
    pub(crate) generation: u64,
    pub(crate) inputs: NockchainCandidateInputs,
    pub(crate) incomplete_header: [u8; ai_pow::pearl_compat::PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
}

pub(crate) fn reconstruct_inference_dense_attempt(
    candidate: InferenceNodeCandidate,
    witness: OpenedDenseWitness,
) -> Result<
    (
        PreparedCanonicalDenseTemplate,
        PearlMergeCheckedTicketAttempt,
    ),
    CanonicalProveError,
> {
    if witness.candidate_generation != candidate.generation {
        return Err(CanonicalProveError(format!(
            "stale inference witness generation {}; current generation is {}",
            witness.candidate_generation, candidate.generation
        )));
    }
    let params = MatmulParams {
        m: witness.a_rows,
        k: witness.common_dim,
        n: witness.b_columns,
        noise_rank: witness.noise_rank,
        tile: INFERENCE_DENSE_TILE,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let ordinal = opened_dense_ordinal(&witness, &params)?;
    let template = PreparedCanonicalDenseTemplate::new(
        &params,
        candidate.inputs.nock_block_commitment,
        Arc::clone(&witness.a),
        Arc::clone(&witness.b_transposed),
    )?;
    let expected_base =
        canonical_dense_incomplete_header(candidate.inputs.nock_block_commitment, 0)?;
    if candidate.incomplete_header != expected_base {
        return Err(CanonicalProveError(
            "active inference candidate has a noncanonical base header".to_string(),
        ));
    }
    let prepared = template.prepare_search(witness.extranonce)?;
    let expected_header = canonical_dense_incomplete_header(
        candidate.inputs.nock_block_commitment, witness.extranonce,
    )?;
    if prepared.sigma() != expected_header {
        return Err(CanonicalProveError(
            "opened witness header does not match its candidate extranonce".to_string(),
        ));
    }
    let attempt = template.checked_search_winner(&prepared, ordinal, &candidate.inputs.target)?;
    if attempt.commitments.s_a != witness.noise_seed_a
        || attempt.commitments.s_b != witness.noise_seed_b
    {
        return Err(CanonicalProveError(
            "opened witness noise seeds disagree with scalar reconstruction".to_string(),
        ));
    }
    Ok((template, attempt))
}

fn prove_inference_dense_block(
    candidate: InferenceNodeCandidate,
    witness: OpenedDenseWitness,
) -> Result<CanonicalDenseBlock, CanonicalProveError> {
    let (template, attempt) = reconstruct_inference_dense_attempt(candidate, witness)?;
    template.prove(attempt)
}

fn opened_dense_ordinal(
    witness: &OpenedDenseWitness,
    params: &MatmulParams,
) -> Result<u64, CanonicalProveError> {
    let tile = params.tile as usize;
    let valid_tile = |indices: &[u32], dimension: u32| {
        indices.len() == tile
            && indices
                .first()
                .is_some_and(|start| start.is_multiple_of(params.tile))
            && indices
                .iter()
                .copied()
                .enumerate()
                .all(|(offset, value)| value == indices[0] + offset as u32 && value < dimension)
    };
    if !valid_tile(&witness.a_row_indices, params.m)
        || !valid_tile(&witness.b_column_indices, params.n)
    {
        return Err(CanonicalProveError(
            "opened witness indices do not describe one in-range dense tile".to_string(),
        ));
    }
    let row_ticket = u64::from(witness.a_row_indices[0] / params.tile);
    let col_ticket = u64::from(witness.b_column_indices[0] / params.tile);
    let col_tickets = u64::from(params.n / params.tile);
    row_ticket
        .checked_mul(col_tickets)
        .and_then(|base| base.checked_add(col_ticket))
        .ok_or_else(|| CanonicalProveError("opened witness ordinal overflow".to_string()))
}

fn prepare_inference_proof(
    candidate: InferenceNodeCandidate,
    request: InferenceProofRequest,
) -> (
    u64,
    tokio::sync::oneshot::Sender<Result<String, String>>,
    Result<PreparedPoke, String>,
) {
    let generation = request.witness.candidate_generation;
    let result = prove_inference_dense_block(candidate, request.witness)
        .map_err(|error| error.to_string())
        .and_then(|block| {
            build_dense_poke(&block, INFERENCE_DENSE_TILE as usize)
                .map_err(|error| error.to_string())
        })
        .map(|poke| NodeClient::prepare_poke_wire(AiPowMinerWire::Mined.to_wire(), poke));
    (generation, request.response, result)
}
