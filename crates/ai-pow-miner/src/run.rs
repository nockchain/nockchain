//! Out-of-process node-connecting run loop for the AI-PoW miner.
//!
//! Mirrors `zk-pow-miner::run` in shape: the miner runs as a separate
//! OS process and talks to a `nockchain` node over the node's private
//! [`nockapp_grpc`] `NockAppService`. The substrate (connect /
//! `set-mining-key` / `enable-mining` / `WatchEffects` / `submit`)
//! is shared via [`nockchain_mining_common::NodeClient`]; only the
//! puzzle-specific bits (the AI-PoW matmul prover and
//! [`crate::wire::AiPowMinerWire`] submission wire) live here.
//!
//! ## Lifecycle
//! 1. Build the [`crate::run::MinerConfig`] with AI-puzzle parameters, matrices, and the
//!    Pearl header source.
//! 2. (re)connect to the node with backoff.
//! 3. `set_mining_key` → `watch_candidates` → `enable_mining(true)`
//!    (subscribe before enable to avoid the candidate-emit race).
//! 4. Inner loop:
//!    - shutdown → cancel/drain candidate, Gateway, and mining workers, then
//!      best-effort `enable_mining(false)` + exit.
//!    - new candidate → bump the generation, cancel current mining, ingest
//!      candidate data, resolve Pearl Gateway work, and spawn the mining worker.
//!    - worker results are accepted only for the current generation; target hits
//!      carry a prepared `%ai-pow` poke for [`crate::wire::AiPowMinerWire::Mined`].
//!    - worker errors fail closed for certificate construction and otherwise log.
//! 5. Stream drop → outer loop reconnects.
//!
//! ## Note on submission
//! The payload shape is a `%ai-pow` noun carrying an opaque
//! Rust-owned nonce and the recursive AI-PoW certificate noun. The plain
//! `MatmulProof` and tile index are mining internals; they are not submitted
//! to the kernel as the block proof. In Pearl-compatible mode the run loop
//! constructs the Rust-owned `AIP1` nonce and, when a Pearl Gateway work item
//! hits Pearl's target, submits Pearl's `PlainProof` wire payload to Gateway.
//! If the same attempt hits Nockchain's target, the miner separately submits
//! the Nockchain `%ai-pow` command, which consensus verifies via the
//! `%ai-pow-verify` jet before admitting the block.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ai_pow::params::MatmulParams;
#[cfg(feature = "gpu")]
use ai_pow::pearl_compat::PearlWorkCommitments;
use ai_pow::pearl_compat::{
    pearl_nbits_to_target_le, validate_pearl_merge_config_for_recursive_prover,
    verify_pearl_aux_inclusion, PearlAuxInclusionProof, PearlCompatError,
    PearlIncompleteBlockHeader, PearlMergeCheckedTicketAttempt, PearlMergeTicketAttempt,
    PearlMiningConfig, PearlNockchainAux, PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES,
    PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH, PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::tile_hash::hash_le_target;
use ai_pow::zk_bridge::{
    prove_pearl_merge_compact_recursive_certificate_checked, AiPowCompactRecursiveCertificateRun,
    AiPowRecursiveCertificateRun, ZkPublicCommitments,
};
use ai_pow_zk::{CompositePublicInputs, ZkParams};
use futures::StreamExt;
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::NounSlab;
use nockchain_mining_common::{
    MiningCandidate, MiningCandidateKind, MiningPkhConfig, NodeClient, NodeClientError,
    PokeTransportOutcome, PokeTransportStatus, PreparedPoke,
};
use nockvm::noun::{NounAllocator, D, T};
use nockvm_macros::tas;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(feature = "gpu")]
use crate::canonical::PreparedCanonicalDenseSearch;
use crate::canonical::{
    canonical_dense_incomplete_header, evaluate_canonical_moe_jackpot,
    prove_canonical_moe_block_at_for_miner, CanonicalBlock, CanonicalDenseBlock,
    CanonicalProveError, PreparedCanonicalDenseTemplate, PreparedCanonicalMoeTemplate,
};
use crate::certificate_noun::{
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run,
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node,
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_recursive_run,
    build_ai_pow_pearl_merge_moe_artifact_noun_from_node,
    decode_ai_pow_pearl_merge_artifact_metadata_slab, AiProofNode, CertificateNounError,
    CertificateNounLimits,
};
use crate::inference::{
    build_gemma4_mining_job, InferenceMiningRpc, InferenceProofRequest, InferenceProofSender,
    OpenedDenseWitness,
};
#[cfg(feature = "gpu")]
use crate::peak::MultiGpuPeakSearchBackend;
use crate::pearl_mining::{
    self, PearlMergeMineOptions, PearlMergeMinedTicket, PearlMergeMiningError, PearlMergeMiningJob,
};
use crate::pearl_plain_proof::PearlPlainProof;
#[cfg(feature = "gpu")]
use crate::search::MeteredSearchBackend;
#[cfg(feature = "gpu")]
use crate::search::SearchBatch;
use crate::search::{CpuSearchBackend, OrderedBatchScheduler, SearchBackend, SearchScheduleEnd};
use crate::wire::AiPowMinerWire;
#[cfg(feature = "gpu")]
use crate::PEAK_PRODUCTION_PARAMS;
use crate::{DifficultyTarget, MiningCancel};

// Covers a base64-encoded max-size coinbase inclusion plus the Pearl header,
// target, and JSON-RPC envelope while still bounding untrusted Gateway input.
const PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES: usize = 160 * 1024;
const MAX_CHAIN_TARGET_U32_LIMBS: usize = 10;
const PEARL_GATEWAY_CERTIFICATE_VERSION_V3: u32 = 3;
const AI_POW_MINE_CANDIDATE_VERSION: u64 = 4;
const NODE_POKE_ACK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiPowProveTimings {
    pub certificate_build_ms: u128,
    pub poke_build_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiPowSubmitTimings {
    pub jam_ms: u128,
    pub transport_ms: u128,
    pub status: PokeTransportStatus,
    pub send_started: bool,
}

impl AiPowSubmitTimings {
    fn from_outcome(jam_elapsed: Duration, outcome: &PokeTransportOutcome) -> Self {
        Self {
            jam_ms: jam_elapsed.as_millis(),
            transport_ms: outcome.elapsed().as_millis(),
            status: outcome.status(),
            send_started: outcome.send_started(),
        }
    }
}

type AiPowPearlMergeCertificateBuilder = dyn Fn(
        &PearlMergeCheckedTicketAttempt,
    ) -> Result<PearlMergeCertificateProof, AiPowCertificateBuildError>
    + Send
    + Sync
    + 'static;

/// Recursive proof data produced only after a Pearl-compatible ticket clears
/// Nockchain's target.
///
/// This stays crate-internal so external callers cannot inject synthetic proof
/// nodes or route around the recursive prover selected by
/// [`PearlMergeSubmissionConfig::new_compact_recursive`]. Tests inside this crate may
/// still inject synthetic proof nodes to exercise the surrounding noun and
/// run-loop plumbing without running the prover.
#[derive(Debug, Clone)]
pub(crate) struct PearlMergeCertificateProof {
    zk_params: ZkParams,
    found_idx: u32,
    commitments: ZkPublicCommitments,
    public_inputs: CompositePublicInputs,
    trace_height: usize,
    certificate: AiProofNode,
}

impl PearlMergeCertificateProof {
    pub(crate) fn from_compact_recursive_run(
        run: &AiPowCompactRecursiveCertificateRun,
    ) -> Result<Self, AiPowCertificateBuildError> {
        let certificate =
            crate::certificate_noun::compact_recursive_certificate_to_node(run.certificate())
                .map_err(|e| AiPowCertificateBuildError(e.to_string()))?;
        Ok(Self {
            zk_params: run.zk_params(),
            found_idx: run.found_idx(),
            commitments: run.commitments(),
            public_inputs: run.public_inputs().clone(),
            trace_height: run.trace_height(),
            certificate,
        })
    }
}

/// Rust-only Nockchain submission settings for Pearl-compatible mining.
///
/// Hoon still receives only the opaque `AIP1` nonce bytes and recursive
/// certificate; these Pearl fields are used only by the miner to construct the
/// shared attempt transcript and aux commitment.
#[derive(Clone)]
pub struct PearlMergeSubmissionConfig {
    gateway: PearlGatewayMinerRpcConfig,
    mining_config: PearlMiningConfig,
    aux_template: PearlNockchainAux,
    max_pattern_len: usize,
    mine_opts: PearlMergeMineOptions,
    certificate_builder: Arc<AiPowPearlMergeCertificateBuilder>,
}

impl PearlMergeSubmissionConfig {
    /// Build the canonical production Pearl-compatible Nockchain submission
    /// config. The certificate builder is fixed to the selected compact
    /// recursive prover, so external callers cannot accidentally install a
    /// plain-proof or synthetic certificate path.
    pub fn new_compact_recursive(
        gateway: PearlGatewayMinerRpcConfig,
        mining_config: PearlMiningConfig,
        aux_template: PearlNockchainAux,
        max_pattern_len: usize,
        mine_opts: PearlMergeMineOptions,
        params: MatmulParams,
        a: Arc<Vec<i8>>,
        b: Arc<Vec<i8>>,
    ) -> Self {
        let certificate_builder = Arc::new(move |attempt: &PearlMergeCheckedTicketAttempt| {
            let run = prove_pearl_merge_compact_recursive_certificate_checked(
                attempt,
                &params,
                a.as_slice(),
                b.as_slice(),
            )
            .map_err(|e| {
                AiPowCertificateBuildError(format!(
                    "refusing to build Pearl-compatible recursive certificate before successful Nockchain target check: {e}"
                ))
            })?;
            let proof = PearlMergeCertificateProof::from_compact_recursive_run(&run)?;
            Ok(proof)
        });

        Self {
            gateway,
            mining_config,
            aux_template,
            max_pattern_len,
            mine_opts,
            certificate_builder,
        }
    }

    pub(crate) fn build_certificate_for_attempt(
        &self,
        attempt: &PearlMergeCheckedTicketAttempt,
    ) -> Result<PearlMergeCertificateProof, AiPowCertificateBuildError> {
        (self.certificate_builder)(attempt)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PearlGatewayTransport {
    UnixSocket { path: String },
    Tcp { host: String, port: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PearlGatewayMinerRpcConfig {
    pub transport: PearlGatewayTransport,
    pub request_timeout: Duration,
    pub refresh_interval: Duration,
}

#[derive(Debug, Error)]
#[error("AI-PoW recursive certificate build failed: {0}")]
pub struct AiPowCertificateBuildError(pub String);

#[derive(Debug, Error)]
pub enum AiPowCertificatePokeError {
    #[error("Pearl merge AI-PoW artifact: {0}")]
    PearlMergeArtifact(#[from] CertificateNounError),
}

impl From<String> for AiPowCertificateBuildError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AiPowCertificateBuildError {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// AI puzzle inputs: the Rust-owned local state required for Pearl-compatible
/// ticket search. The chain's `%mine-ai` effect supplies the candidate block
/// commitment, target, and pow-len; the miner combines that with these matrices
/// and the Pearl submission config to build the shared attempt transcript.
///
/// These come from operator config (CLI / config file). In a future
/// chain-AI integration these may be derived from chain state (e.g.
/// layer/epoch); the substrate is structured so that a follow-up can
/// swap the derivation in without changing the run loop.
#[derive(Clone)]
pub struct AiPuzzleInputs {
    pub params: MatmulParams,
    /// Reference matmul inputs. `Arc` so the spawn-blocking worker can
    /// hold a cheap clone without copying the bytes.
    pub a: Arc<Vec<i8>>,
    pub b: Arc<Vec<i8>>,
    /// Pearl-format-compatible Nockchain submission configuration. This is the
    /// only production submission path.
    pub pearl_merge: PearlMergeSubmissionConfig,
}

impl AiPuzzleInputs {
    /// Production node-mining preflight: do not spend matmul work unless the
    /// configured puzzle can be converted into the recursive certificate
    /// artifact expected at the block boundary.
    pub fn validate_canonical_submission_ready(&self) -> Result<(), MinerError> {
        let pearl = &self.pearl_merge;
        validate_pearl_merge_config_for_recursive_prover(
            &pearl.mining_config,
            &self.params,
            pearl.max_pattern_len,
        )
        .map_err(|e| {
            MinerError::CanonicalCertificateUnavailable(format!(
                "configured Pearl merge AI-PoW params/config cannot produce a recursive certificate artifact: {e}"
            ))
        })?;
        pearl.aux_template.to_bytes().map_err(|e| {
            MinerError::CanonicalCertificateUnavailable(format!(
                "Pearl aux template is not canonical: {e}"
            ))
        })?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct MinerConfig {
    /// Node's private gRPC URL, e.g. `http://127.0.0.1:5555`.
    pub node_addr: String,
    /// v1 pubkey-hash reward configs. Required.
    pub mining_pkh_configs: Vec<MiningPkhConfig>,
    /// AI puzzle local-state inputs (matrices, params, Pearl work source).
    pub puzzle: AiPuzzleInputs,
    /// Dedicated CPU ticket-search workers. Defaults to physical core count.
    pub mining_threads: usize,
    pub reconnect_backoff_initial: Duration,
    pub reconnect_backoff_max: Duration,
    pub reconnect_max_attempts: u32,
}

impl MinerConfig {
    /// Convenience builder for the common case: required v1 mining-pkh
    /// configs, default backoff (1s→30s, 5 retries).
    pub fn new(
        node_addr: String,
        mining_pkh_configs: Vec<MiningPkhConfig>,
        puzzle: AiPuzzleInputs,
    ) -> Self {
        Self {
            node_addr,
            mining_pkh_configs,
            puzzle,
            reconnect_backoff_initial: Duration::from_secs(1),
            reconnect_backoff_max: Duration::from_secs(30),
            reconnect_max_attempts: 5,
            mining_threads: CpuSearchBackend::default_worker_count(),
        }
    }

    pub fn validate(&self) -> Result<(), MinerError> {
        validate_mining_pkh_configs(&self.mining_pkh_configs)?;
        if self.reconnect_max_attempts == 0 {
            return Err(MinerError::InvalidConfig(
                "reconnect_max_attempts must be nonzero".to_string(),
            ));
        }
        if self.reconnect_backoff_initial.is_zero() {
            return Err(MinerError::InvalidConfig(
                "reconnect_backoff_initial must be nonzero".to_string(),
            ));
        }
        if self.reconnect_backoff_max.is_zero() {
            return Err(MinerError::InvalidConfig(
                "reconnect_backoff_max must be nonzero".to_string(),
            ));
        }
        if self.mining_threads == 0 {
            return Err(MinerError::InvalidConfig(
                "mining_threads must be nonzero".to_string(),
            ));
        }

        let gateway = &self.puzzle.pearl_merge.gateway;
        if gateway.request_timeout.is_zero() {
            return Err(MinerError::InvalidConfig(
                "Pearl Gateway request_timeout must be nonzero".to_string(),
            ));
        }
        if gateway.refresh_interval.is_zero() {
            return Err(MinerError::InvalidConfig(
                "Pearl Gateway refresh_interval must be nonzero".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum MinerError {
    #[error("invalid miner configuration: {0}")]
    InvalidConfig(String),
    #[error("kernel configuration failed: {0}")]
    Configure(String),
    #[error("gave up after {count} consecutive connect attempts")]
    TooManyReconnects { count: u32 },
    #[error("candidate decode failed: {0}")]
    CandidateDecode(String),
    #[error("worker join failed: {0}")]
    WorkerJoin(String),
    #[error("search backend setup failed: {0}")]
    SearchBackend(#[from] crate::search::SearchBackendError),
    #[error("{0}")]
    CertificateBuild(String),
    #[error("{0}")]
    CanonicalCertificateUnavailable(String),
    #[error("production AI-PoW worker failed: {0}")]
    ProductionWorker(String),
}

/// Production entry point using a physical-core CPU search backend.
pub async fn run(cfg: MinerConfig, shutdown: CancellationToken) -> Result<(), MinerError> {
    cfg.validate()?;
    cfg.puzzle.validate_canonical_submission_ready()?;
    let backend: Arc<dyn SearchBackend> = Arc::new(CpuSearchBackend::new(cfg.mining_threads)?);
    run_with_backend(cfg, shutdown, backend).await
}

/// Production entry point with an owned ticket-search backend.
pub async fn run_with_backend(
    cfg: MinerConfig,
    shutdown: CancellationToken,
    backend: Arc<dyn SearchBackend>,
) -> Result<(), MinerError> {
    cfg.validate()?;
    cfg.puzzle.validate_canonical_submission_ready()?;
    info!(
        node = %cfg.node_addr,
        params = ?cfg.puzzle.params,
        "ai-pow-miner: entering main loop"
    );

    let mut consecutive_failures: u32 = 0;
    let mut backoff = cfg.reconnect_backoff_initial;

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        // ── (re)connect ──
        let mut client = match NodeClient::connect(&cfg.node_addr).await {
            Ok(c) => {
                consecutive_failures = 0;
                backoff = cfg.reconnect_backoff_initial;
                c
            }
            Err(e) => {
                consecutive_failures += 1;
                warn!(
                    attempt = consecutive_failures,
                    backoff_ms = backoff.as_millis() as u64,
                    error = %e,
                    "connect failed; backing off"
                );
                if consecutive_failures >= cfg.reconnect_max_attempts {
                    return Err(MinerError::TooManyReconnects {
                        count: consecutive_failures,
                    });
                }
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(cfg.reconnect_backoff_max);
                continue;
            }
        };

        // ── configure ──
        // Order matters: subscribe BEFORE enable_mining so the initial
        // candidate (which the post-poke update-candidate-block emits)
        // lands on a live stream.
        if let Err(e) = client
            .set_mining_key(
                AiPowMinerWire::SetPubKey.to_wire(),
                Vec::new(),
                cfg.mining_pkh_configs.clone(),
            )
            .await
        {
            return Err(MinerError::Configure(format!("set_mining_key: {e}")));
        }
        let mut candidates = match client.watch_candidates(vec![b"mine-ai".to_vec()]).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "watch_candidates failed; reconnect");
                consecutive_failures += 1;
                continue;
            }
        };
        if let Err(e) = client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), true)
            .await
        {
            return Err(MinerError::Configure(format!("enable_mining(true): {e}")));
        }
        info!("ai-pow-miner: subscribed + mining enabled; awaiting candidates");

        // ── inner loop ──
        // Pearl Gateway fetches, mining, Gateway submission, recursive proof
        // construction, and poke serialization run outside this async loop. A
        // generation guards every worker result so superseded candidates cannot
        // submit stale proofs after cancellation races.
        let mut workers: JoinSet<(u64, Result<PearlMergeWorkerOutput, PearlMergeMiningError>)> =
            JoinSet::new();
        let mut candidate_workers: JoinSet<(u64, Result<NockchainCandidateInputs, String>)> =
            JoinSet::new();
        let mut gateway_workers: JoinSet<(
            u64,
            PearlGatewayWorkKind,
            NockchainCandidateInputs,
            Result<PearlMergeCandidateJob, String>,
        )> = JoinSet::new();
        let mut current_cancel: Option<MiningCancel> = None;
        let mut current_generation = 0_u64;
        let mut latest_candidate: Option<NockchainCandidateInputs> = None;
        let mut current_pearl_header: Option<PearlIncompleteBlockHeader> = None;
        let mut candidate_gateway_generation: Option<u64> = None;
        let mut refresh_gateway_generation: Option<u64> = None;
        let mut next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
        let mut pearl_refresh = tokio::time::interval(pearl_work_refresh_interval(&cfg));
        pearl_refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let inner_result: InnerOutcome = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    if let Err(e) = abort_and_drain_candidate_workers(&mut candidate_workers).await {
                        break InnerOutcome::Fatal(e);
                    }
                    if let Err(e) = abort_and_drain_pearl_gateway_workers(&mut gateway_workers).await {
                        break InnerOutcome::Fatal(e);
                    }
                    if let Err(e) = cancel_and_drain_pearl_workers(&mut workers, &mut current_cancel).await {
                        break InnerOutcome::Fatal(e);
                    }
                    break InnerOutcome::Shutdown;
                }
                maybe_c = candidates.next() => {
                    let Some(c_res) = maybe_c else {
                        warn!("watch_candidates stream ended; will reconnect");
                        break InnerOutcome::StreamLost;
                    };
                    let candidate = match c_res {
                        Ok(c) => c,
                        Err(NodeClientError::Grpc(e)) => {
                            warn!(error = %e, "watch_candidates stream failed; will reconnect");
                            break InnerOutcome::StreamLost;
                        }
                        Err(e) => break InnerOutcome::Fatal(MinerError::CandidateDecode(format!("{e}"))),
                    };
                    current_generation = current_generation.wrapping_add(1);
                    latest_candidate = None;
                    current_pearl_header = None;
                    refresh_gateway_generation = None;
                    candidate_gateway_generation = None;
                    next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
                    if let Some(cancel) = current_cancel.take() {
                        cancel.cancel();
                    }
                    spawn_candidate_ingestion(&mut candidate_workers, current_generation, candidate);
                }
                _ = pearl_refresh.tick() => {
                    let Some(candidate_inputs) = latest_candidate else {
                        continue;
                    };
                    if candidate_gateway_generation == Some(current_generation)
                        || refresh_gateway_generation == Some(current_generation)
                    {
                        continue;
                    }
                    refresh_gateway_generation = Some(current_generation);
                    spawn_pearl_gateway_work(
                        &mut gateway_workers,
                        current_generation,
                        PearlGatewayWorkKind::Refresh,
                        &cfg,
                        candidate_inputs,
                    );
                }
                joined = candidate_workers.join_next(), if !candidate_workers.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    let joined: CandidateWorkerJoin = joined;
                    let (generation, result) = match joined {
                        Ok(joined) => joined,
                        Err(e) => break InnerOutcome::Fatal(MinerError::WorkerJoin(format!("{e}"))),
                    };
                    if generation != current_generation {
                        debug!(
                            generation,
                            current_generation,
                            "dropping stale candidate ingestion result"
                        );
                        continue;
                    }
                    let candidate_inputs = match result {
                        Ok(candidate_inputs) => candidate_inputs,
                        Err(e) => {
                            warn!(error = %e, "could not derive Nockchain candidate inputs; skipping");
                            continue;
                        }
                    };
                    latest_candidate = Some(candidate_inputs);
                    candidate_gateway_generation = Some(current_generation);
                    spawn_pearl_gateway_work(
                        &mut gateway_workers,
                        current_generation,
                        PearlGatewayWorkKind::Candidate,
                        &cfg,
                        candidate_inputs,
                    );
                }
                joined = gateway_workers.join_next(), if !gateway_workers.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    let joined: PearlGatewayWorkerJoin = joined;
                    let (generation, kind, candidate_inputs, result) = match joined {
                        Ok(joined) => joined,
                        Err(e) => break InnerOutcome::Fatal(MinerError::WorkerJoin(format!("{e}"))),
                    };
                    if kind == PearlGatewayWorkKind::Refresh
                        && refresh_gateway_generation == Some(generation)
                    {
                        refresh_gateway_generation = None;
                    }
                    if kind == PearlGatewayWorkKind::Candidate
                        && candidate_gateway_generation == Some(generation)
                    {
                        candidate_gateway_generation = None;
                    }
                    if generation != current_generation {
                        debug!(
                            generation,
                            current_generation,
                            "dropping stale Pearl Gateway work result"
                        );
                        continue;
                    }
                    let pearl_job = match result {
                        Ok(pearl_job) => pearl_job,
                        Err(e) => {
                            current_pearl_header = None;
                            match kind {
                                PearlGatewayWorkKind::Candidate => {
                                    warn!(error = %e, "could not derive Pearl merge job inputs from candidate");
                                }
                                PearlGatewayWorkKind::Refresh => {
                                    warn!(error = %e, "could not refresh Pearl Gateway work for current Nockchain candidate");
                                }
                            }
                            continue;
                        }
                    };
                    if kind == PearlGatewayWorkKind::Refresh {
                        if current_pearl_header == Some(pearl_job.header) {
                            continue;
                        }
                        current_generation = current_generation.wrapping_add(1);
                        if let Some(cancel) = current_cancel.take() {
                            cancel.cancel();
                        }
                        next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
                        info!(pow_len = candidate_inputs.pow_len, "Pearl Gateway work changed; redispatching ai-pow attempt for current Nockchain candidate");
                    } else {
                        info!(pow_len = candidate_inputs.pow_len, "new candidate; dispatching Pearl-compatible ai-pow attempt");
                    }
                    current_pearl_header = Some(pearl_job.header);
                    let cancel = MiningCancel::new();
                    spawn_pearl_merge_attempt(
                        &mut workers,
                        current_generation,
                        &cfg,
                        backend.clone(),
                        pearl_job,
                        pearl_mine_opts_with_attempt_start(&cfg, next_pearl_attempt_start),
                        cancel.clone(),
                    );
                    current_cancel = Some(cancel);
                }
                joined = workers.join_next(), if !workers.is_empty() => {
                    let Some(joined) = joined else {
                        continue;
                    };
                    let joined: PearlMergeWorkerJoin = joined;
                    let (generation, result) = match joined {
                        Ok(joined) => joined,
                        Err(e) => break InnerOutcome::Fatal(MinerError::WorkerJoin(format!("{e}"))),
                    };
                    if generation != current_generation {
                        debug!(
                            generation,
                            current_generation,
                            "dropping stale Pearl-compatible worker result"
                        );
                        continue;
                    }
                    current_cancel = None;
                    match result {
                        Ok(PearlMergeWorkerOutput::PearlOnly {
                            mined,
                            pearl_submit_failed,
                        }) => {
                            info!(
                                matmul_attempts = mined.ticket.stats.matmul_attempts_tried,
                                elapsed_s = mined.ticket.stats.elapsed.as_secs_f64(),
                                matmul_attempt_rate = mined.ticket.stats.matmul_attempt_rate_per_sec(),
                                pearl_target_hit = mined.ticket.pearl_target_hit,
                                nockchain_target_hit = mined.ticket.nockchain_target_hit,
                                "ai-pow-miner: Pearl-compatible solution found"
                            );
                            if pearl_submit_failed {
                                current_pearl_header = None;
                                continue;
                            }
                            next_pearl_attempt_start = match next_pearl_attempt_start
                                .checked_add(mined.ticket.stats.matmul_attempts_tried)
                            {
                                Some(next) => next,
                                None => {
                                    warn!(
                                        attempt_start = next_pearl_attempt_start,
                                        attempts_tried = mined.ticket.stats.matmul_attempts_tried,
                                        "Pearl-compatible attempt offset overflow; waiting for refreshed work"
                                    );
                                    current_pearl_header = None;
                                    continue;
                                }
                            };
                            let pearl_job = PearlMergeCandidateJob {
                                header: mined.ticket.attempt.public_params.block_header,
                                gateway_mining_job: mined.gateway_mining_job.clone(),
                                aux_inclusion: mined.aux_inclusion.clone(),
                                target: mined.ticket.attempt.nockchain_target,
                                aux: mined.ticket.attempt.aux.clone(),
                            };
                            current_pearl_header = Some(pearl_job.header);
                            let cancel = MiningCancel::new();
                            info!(
                                attempt_start = next_pearl_attempt_start,
                                "Pearl-only solution submitted; continuing Nockchain search on same Pearl work"
                            );
                            spawn_pearl_merge_attempt(
                                &mut workers,
                                current_generation,
                                &cfg,
                                backend.clone(),
                                pearl_job,
                                pearl_mine_opts_with_attempt_start(&cfg, next_pearl_attempt_start),
                                cancel.clone(),
                            );
                            current_cancel = Some(cancel);
                        }
                        Ok(PearlMergeWorkerOutput::NockchainPrepared(submission)) => {
                            info!(
                                matmul_attempts = submission.mined.ticket.stats.matmul_attempts_tried,
                                elapsed_s = submission.mined.ticket.stats.elapsed.as_secs_f64(),
                                matmul_attempt_rate = submission.mined.ticket.stats.matmul_attempt_rate_per_sec(),
                                pearl_target_hit = submission.mined.ticket.pearl_target_hit,
                                nockchain_target_hit = submission.mined.ticket.nockchain_target_hit,
                                "ai-pow-miner: Pearl-compatible solution found"
                            );
                            let outcome = client
                                .send_prepared_poke_with_timeout_or_cancel(
                                    submission.prepared,
                                    NODE_POKE_ACK_TIMEOUT,
                                    shutdown.cancelled(),
                                )
                                .await;
                            let submit_timings =
                                AiPowSubmitTimings::from_outcome(submission.jam_elapsed, &outcome);
                            match outcome.into_result() {
                                Ok(()) => info!(
                                    certificate_build_ms = submission.prove_timings.certificate_build_ms,
                                    poke_build_ms = submission.prove_timings.poke_build_ms,
                                    jam_ms = submit_timings.jam_ms,
                                    transport_ms = submit_timings.transport_ms,
                                    submit_status = ?submit_timings.status,
                                    send_started = submit_timings.send_started,
                                    "Pearl-compatible ai-pow certificate submission acked by node"
                                ),
                                Err(e) => warn!(
                                    error = %e,
                                    certificate_build_ms = submission.prove_timings.certificate_build_ms,
                                    poke_build_ms = submission.prove_timings.poke_build_ms,
                                    jam_ms = submit_timings.jam_ms,
                                    transport_ms = submit_timings.transport_ms,
                                    submit_status = ?submit_timings.status,
                                    send_started = submit_timings.send_started,
                                    "submit Pearl-compatible ai-pow certificate poke failed (likely stale candidate)"
                                ),
                            }
                            latest_candidate = None;
                            current_pearl_header = None;
                        }
                        Err(PearlMergeMiningError::Cancelled) => {
                            debug!("Pearl-compatible worker cancelled (expected on candidate supersede / shutdown)");
                        }
                        Err(PearlMergeMiningError::CertificateBuild(e)) => {
                            // A build failure is deterministic for this
                            // candidate's statement, not for the miner: log,
                            // drop the candidate, and wait for refreshed work
                            // instead of aborting the process.
                            warn!(
                                error = %e,
                                "Pearl-compatible certificate build failed; dropping candidate"
                            );
                            latest_candidate = None;
                            current_pearl_header = None;
                        }
                        Err(e) => {
                            warn!(error = %e, "Pearl-compatible ai-pow attempt terminated without solution");
                        }
                    }
                }
            }
        };

        // ── cleanup before reconnect or exit ──
        abort_and_drain_candidate_workers(&mut candidate_workers).await?;
        abort_and_drain_pearl_gateway_workers(&mut gateway_workers).await?;
        cancel_and_drain_pearl_workers(&mut workers, &mut current_cancel).await?;
        let _ = client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), false)
            .await;

        match inner_result {
            InnerOutcome::Shutdown => return Ok(()),
            InnerOutcome::StreamLost => {
                consecutive_failures += 1;
                if consecutive_failures >= cfg.reconnect_max_attempts {
                    return Err(MinerError::TooManyReconnects {
                        count: consecutive_failures,
                    });
                }
                tokio::select! {
                    _ = shutdown.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(cfg.reconnect_backoff_max);
            }
            InnerOutcome::Fatal(e) => return Err(e),
        }
    }

    Ok(())
}

/// Canonical block shape for the gateway-free CPU miner. Matches the
/// `ai_pow_accept_e2e` integration test and a member of the node's production
/// verifier-setup bucket set (heights 2^13..2^19), so a node's boot-installed
/// setup verifies it. See [`run_canonical`].
const CANONICAL_MATMUL_PARAMS: MatmulParams = MatmulParams {
    m: 64,
    k: 1024,
    n: 64,
    noise_rank: 64,
    tile: 8,
    spot_checks: 1,
    difficulty_bits: 0,
};
const CANONICAL_HW: u32 = 8;
const CANONICAL_E: usize = 2;
const CANONICAL_TOP_K: usize = 1;

/// Build the `[%command %pow [%ai-pow nonce cert]]` poke from a proved canonical
/// (MoE / `AIM1`) block, mirroring `ai_pow_accept_e2e::artifact_for_block` +
/// `pow_poke_from_artifact`. The artifact is wrapped directly (no dense-`AIP1`
/// metadata self-check — that would reject the MoE nonce magic).
fn build_canonical_poke(block: &CanonicalBlock) -> Result<NounSlab, MinerError> {
    let artifact = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
        &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
        block.certificate.found_idx, block.certificate.trace_height,
        &block.certificate.commitments, &block.certificate.public_inputs,
        &block.certificate.certificate,
    )
    .map_err(|e| MinerError::CertificateBuild(format!("canonical moe artifact: {e}")))?;
    let artifact_space = artifact.noun_space();
    let mut slab = NounSlab::new();
    let art = slab.copy_into(unsafe { *artifact.root() }, &artifact_space);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), art]);
    slab.set_root(payload);
    Ok(slab)
}

fn build_dense_poke(
    block: &CanonicalDenseBlock,
    max_pattern_len: usize,
) -> Result<NounSlab, MinerError> {
    let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
        block.attempt.attempt(),
        &block.aux_inclusion,
        &block.a,
        &block.b,
        max_pattern_len,
        &block.run,
    )
    .map_err(|e| MinerError::CertificateBuild(format!("dense artifact: {e}")))?;
    let artifact_space = artifact.noun_space();
    let mut slab = NounSlab::new();
    let art = slab.copy_into(unsafe { *artifact.root() }, &artifact_space);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), art]);
    slab.set_root(payload);
    Ok(slab)
}

#[cfg(feature = "gpu")]
fn build_peak_poke(block: &CanonicalDenseBlock) -> Result<NounSlab, MinerError> {
    build_dense_poke(block, PEAK_PRODUCTION_PARAMS.tile as usize)
}

enum GatewayFreeBlock {
    Canonical(CanonicalBlock),
    #[cfg(feature = "gpu")]
    Peak(CanonicalDenseBlock),
}

impl GatewayFreeBlock {
    fn commit(&self) -> [u8; 32] {
        match self {
            Self::Canonical(block) => block.commit,
            #[cfg(feature = "gpu")]
            Self::Peak(block) => block.commit,
        }
    }

    fn trace_height(&self) -> usize {
        match self {
            Self::Canonical(block) => block.certificate.trace_height,
            #[cfg(feature = "gpu")]
            Self::Peak(block) => block.run.trace_height(),
        }
    }

    fn build_poke(&self) -> Result<NounSlab, MinerError> {
        match self {
            Self::Canonical(block) => build_canonical_poke(block),
            #[cfg(feature = "gpu")]
            Self::Peak(block) => build_peak_poke(block),
        }
    }
}

#[derive(Clone)]
enum GatewayFreeProfile {
    Canonical,
    #[cfg(feature = "gpu")]
    Peak {
        a: Arc<Vec<i8>>,
        b: Arc<Vec<i8>>,
    },
}

impl GatewayFreeProfile {
    fn name(&self) -> &'static str {
        match self {
            Self::Canonical => "canonical-moe",
            #[cfg(feature = "gpu")]
            Self::Peak { .. } => "peak-dense",
        }
    }

    fn worker_errors_are_fatal(&self) -> bool {
        match self {
            Self::Canonical => false,
            #[cfg(feature = "gpu")]
            Self::Peak { .. } => true,
        }
    }

    fn grind(
        &self,
        commit: [u8; 32],
        target: DifficultyTarget,
        cancel: Arc<AtomicBool>,
        backend: &dyn SearchBackend,
    ) -> GrindResult {
        match self {
            Self::Canonical => grind_canonical_block_with_backend(commit, target, cancel, backend),
            #[cfg(feature = "gpu")]
            Self::Peak { a, b } => {
                grind_peak_block_with_backend(commit, target, cancel, backend, a, b)
            }
        }
    }
}

/// A grind worker returns `Ok(Some(block))` when a ticket cleared the target and
/// its certificate was proved, `Ok(None)` when the grind was cancelled or
/// exhausted, and `Err` on search, revalidation, or proof failure.
type GrindResult = Result<Option<GatewayFreeBlock>, CanonicalProveError>;

enum CanonicalOutcome {
    None,
    Joined(Result<GrindResult, tokio::task::JoinError>),
}

/// Borrows the worker handle instead of taking it: inside `tokio::select!`
/// this branch's future is dropped whenever another branch wins first, and a
/// take-based version would detach the grind task on every losing race — the
/// detached prove keeps running (multi-GB, ~30s) while the loop spawns a new
/// grind per candidate, so a candidate storm accumulates concurrent proves
/// until memory is exhausted. The handle is only consumed (`worker.take()` by
/// the caller's match arms) when this branch actually wins.
async fn await_canonical_worker(worker: &mut Option<JoinHandle<GrindResult>>) -> CanonicalOutcome {
    match worker.as_mut() {
        Some(h) => CanonicalOutcome::Joined(h.await),
        None => CanonicalOutcome::None,
    }
}

async fn cancel_and_await_canonical_worker(
    worker: &mut Option<JoinHandle<GrindResult>>,
    cancel: &AtomicBool,
) -> Result<(), MinerError> {
    cancel.store(true, Ordering::Relaxed);
    if let Some(handle) = worker.take() {
        let _ = handle
            .await
            .map_err(|e| MinerError::WorkerJoin(format!("{e}")))?;
    }
    Ok(())
}

/// MAC-equivalents one canonical grind attempt costs — the shape work factor
/// `F` consensus prices this miner's attempts at. The node's `target` prices
/// ONE MAC-equivalent, so the jackpot clears when
/// `jackpot <= target * CANONICAL_SHAPE_WORK_FACTOR`; comparing against the
/// bare target instead would silently discard every win in `(target, Theta]`
/// and cost this miner `F` times more work per block than consensus asks for.
/// See `ai_pow::difficulty`.
pub fn canonical_shape_work_factor() -> Result<u128, CanonicalProveError> {
    crate::canonical::canonical_mining_config(
        &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K,
    )
    .shape_work_factor()
    .map_err(|e| CanonicalProveError(format!("canonical shape work factor: {e}")))
}

/// The effective jackpot threshold this miner accepts against: `target · F` for
/// the canonical tile shape.
///
/// Depends only on `(target, shape)`, never on the extranonce, so the grind
/// resolves it once. A target the canonical shape cannot scale is a node /
/// consensus misconfiguration, not a grind failure — surface it rather than
/// silently spinning the whole extranonce space.
pub fn canonical_grind_threshold(
    target: &DifficultyTarget,
) -> Result<[u8; 32], CanonicalProveError> {
    let factor = canonical_shape_work_factor()?;
    ai_pow::difficulty::effective_jackpot_threshold(target, factor).map_err(|e| {
        CanonicalProveError(format!(
            "candidate target {} is outside the representable AI-PoW domain for the canonical \
             shape (factor {factor}): {e:?}",
            hex::encode(target)
        ))
    })
}

#[cfg(test)]
fn grind_canonical_block(
    commit: [u8; 32],
    target: DifficultyTarget,
    cancel: Arc<AtomicBool>,
) -> GrindResult {
    let backend = CpuSearchBackend::default();
    grind_canonical_block_with_backend(commit, target, cancel, &backend)
}

// Proof-of-work grind for the gateway-free canonical miner. Batches consecutive
// extranonces through a prepared template and returns only after a scalar-oracle
// recheck has matched the backend winner. Cancellation is observed before and
// after every bounded batch. Runs on a blocking thread.
fn grind_canonical_block_with_backend(
    commit: [u8; 32],
    target: DifficultyTarget,
    cancel: Arc<AtomicBool>,
    backend: &dyn SearchBackend,
) -> GrindResult {
    let threshold = canonical_grind_threshold(&target)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let template = Arc::new(PreparedCanonicalMoeTemplate::new(
        &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K, commit,
    )?);
    let mut scheduler =
        OrderedBatchScheduler::new(0, u64::from(u32::MAX) + 1, None, backend.batch_attempts())
            .map_err(|error| CanonicalProveError(format!("search scheduler: {error}")))?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let batch = match scheduler.next_batch(threshold) {
            Ok(batch) => batch,
            Err(SearchScheduleEnd::AttemptSpaceExhausted) => break,
            Err(SearchScheduleEnd::BudgetExhausted { .. }) => {
                return Err(CanonicalProveError(
                    "canonical grind has no configured attempt budget".to_string(),
                ));
            }
        };
        let winner = backend
            .search_canonical(Arc::clone(&template), batch)
            .map_err(|error| CanonicalProveError(format!("search backend: {error}")))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(winner) = winner else {
            scheduler
                .record_miss(batch)
                .map_err(|error| CanonicalProveError(format!("search scheduler: {error}")))?;
            continue;
        };
        scheduler
            .record_winner(batch, winner)
            .map_err(|error| CanonicalProveError(format!("search scheduler: {error}")))?;
        let extranonce = u32::try_from(winner.ordinal).map_err(|_| {
            CanonicalProveError("backend returned out-of-range extranonce".to_string())
        })?;
        let jackpot = evaluate_canonical_moe_jackpot(
            &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K, commit,
            extranonce,
        )?;
        if jackpot != winner.jackpot_hash {
            return Err(CanonicalProveError(
                "search backend jackpot disagrees with canonical scalar oracle".to_string(),
            ));
        }
        if !hash_le_target(&jackpot, &threshold) {
            return Err(CanonicalProveError(
                "search backend reported a canonical jackpot above its threshold".to_string(),
            ));
        }
        info!(
            extranonce,
            commit = %hex::encode(commit),
            "canonical AI-PoW jackpot hit; proving certificate (~25-30s)"
        );
        let proved = prove_canonical_moe_block_at_for_miner(
            &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K, commit,
            extranonce,
        )?;
        return Ok(Some(GatewayFreeBlock::Canonical(proved)));
    }
    warn!(
        commit = %hex::encode(commit),
        "canonical AI-PoW grind exhausted the u32 extranonce space with no jackpot; \
         the target is too hard for the canonical shape (raise --fakenet-ai-asert-anchor-target-bex)"
    );
    Ok(None)
}

#[cfg(feature = "gpu")]
fn grind_peak_block_with_backend(
    commit: [u8; 32],
    target: DifficultyTarget,
    cancel: Arc<AtomicBool>,
    backend: &dyn SearchBackend,
    a: &Arc<Vec<i8>>,
    b: &Arc<Vec<i8>>,
) -> GrindResult {
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let template = PreparedCanonicalDenseTemplate::new(
        &PEAK_PRODUCTION_PARAMS,
        commit,
        Arc::clone(a),
        Arc::clone(b),
    )?;

    for extranonce in 0..=u32::MAX {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let prepared = Arc::new(template.prepare_search(extranonce)?);
        let factor = prepared
            .config()
            .shape_work_factor()
            .map_err(|e| CanonicalProveError(format!("peak shape work factor: {e}")))?;
        let threshold =
            ai_pow::difficulty::effective_jackpot_threshold(&target, factor).map_err(|e| {
                CanonicalProveError(format!(
                    "candidate target {} is outside the representable AI-PoW domain for the peak \
                     shape (factor {factor}): {e:?}",
                    hex::encode(target)
                ))
            })?;
        let total_tickets = prepared.total_tickets();
        let batch = SearchBatch::new(0, total_tickets, threshold)
            .map_err(|e| CanonicalProveError(format!("peak search batch: {e}")))?;
        let outcome = backend
            .search_peak(Arc::clone(&prepared), batch)
            .map_err(|e| CanonicalProveError(format!("peak search backend: {e}")))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(winner) = outcome.winner else {
            continue;
        };
        let attempt =
            revalidate_peak_winner(&template, &prepared, winner, &outcome.commitments, &target)?;
        info!(
            extranonce,
            ordinal = winner.ordinal,
            commit = %hex::encode(commit),
            "peak dense AI-PoW jackpot hit; proving recursive certificate"
        );
        let block = template.prove(attempt)?;
        return Ok(Some(GatewayFreeBlock::Peak(block)));
    }

    warn!(
        commit = %hex::encode(commit),
        "peak AI-PoW grind exhausted the u32 extranonce space with no jackpot"
    );
    Ok(None)
}

#[cfg(feature = "gpu")]
fn revalidate_peak_winner(
    template: &PreparedCanonicalDenseTemplate,
    prepared: &PreparedCanonicalDenseSearch,
    winner: crate::search::SearchWinner,
    device_commitments: &PearlWorkCommitments,
    target: &DifficultyTarget,
) -> Result<PearlMergeCheckedTicketAttempt, CanonicalProveError> {
    let attempt = template.checked_search_winner(prepared, winner.ordinal, target)?;
    if attempt.commitments != *device_commitments {
        return Err(CanonicalProveError(
            "peak backend transcript disagrees with scalar winner recheck".to_string(),
        ));
    }
    if attempt.ticket.jackpot_hash != winner.jackpot_hash {
        return Err(CanonicalProveError(
            "peak backend jackpot disagrees with scalar winner recheck".to_string(),
        ));
    }
    Ok(attempt)
}

/// Gateway-free canonical CPU miner. It binds each MoE search to the current
/// `%mine-ai` block commitment, applies the consensus shape-work adjustment, and
/// builds the recursive certificate only after a scalar-validated hit. A new
/// candidate cancels and drains the old search before its replacement starts.
pub async fn run_canonical(
    node_addr: String,
    mining_pkh_configs: Vec<MiningPkhConfig>,
    shutdown: CancellationToken,
) -> Result<(), MinerError> {
    let backend: Arc<dyn SearchBackend> = Arc::new(CpuSearchBackend::new(
        CpuSearchBackend::default_worker_count(),
    )?);
    run_gateway_free_with_backend(
        node_addr,
        mining_pkh_configs,
        shutdown,
        backend,
        GatewayFreeProfile::Canonical,
    )
    .await
}

/// Gateway-free canonical miner with an owned ticket-search backend.
pub async fn run_canonical_with_backend(
    node_addr: String,
    mining_pkh_configs: Vec<MiningPkhConfig>,
    shutdown: CancellationToken,
    backend: Arc<dyn SearchBackend>,
) -> Result<(), MinerError> {
    run_gateway_free_with_backend(
        node_addr,
        mining_pkh_configs,
        shutdown,
        backend,
        GatewayFreeProfile::Canonical,
    )
    .await
}

#[cfg(feature = "gpu")]
pub async fn run_peak(
    node_addr: String,
    mining_pkh_configs: Vec<MiningPkhConfig>,
    shutdown: CancellationToken,
    device_ordinals: Option<Vec<usize>>,
) -> Result<(), MinerError> {
    let backend = match device_ordinals {
        Some(ordinals) => MultiGpuPeakSearchBackend::new(ordinals),
        None => MultiGpuPeakSearchBackend::all_visible(),
    }
    .map_err(|e| MinerError::Configure(format!("peak CUDA backend: {e}")))?;
    let (a, b) = ai_pow::synth::synth_matrices(
        ai_pow::synth::AI_POW_PROD_SYNTH_SEED,
        &PEAK_PRODUCTION_PARAMS,
    );
    backend
        .preflight(&a, &b)
        .map_err(|e| MinerError::Configure(format!("peak CUDA preflight: {e}")))?;
    info!(
        cuda_devices = ?backend.device_ordinals(),
        "ai-pow-miner: peak CUDA search initialized"
    );
    let backend: Arc<dyn SearchBackend> = MeteredSearchBackend::new("peak-cuda", Arc::new(backend));
    run_gateway_free_with_backend(
        node_addr,
        mining_pkh_configs,
        shutdown,
        backend,
        GatewayFreeProfile::Peak {
            a: Arc::new(a),
            b: Arc::new(b),
        },
    )
    .await
}

async fn run_gateway_free_with_backend(
    node_addr: String,
    mining_pkh_configs: Vec<MiningPkhConfig>,
    shutdown: CancellationToken,
    backend: Arc<dyn SearchBackend>,
    profile: GatewayFreeProfile,
) -> Result<(), MinerError> {
    validate_mining_pkh_configs(&mining_pkh_configs)?;
    let profile_name = profile.name();
    info!(
        node = %node_addr,
        profile = profile_name,
        "ai-pow-miner: entering gateway-free production loop"
    );
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        let mut client = match NodeClient::connect(&node_addr).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "connect failed; retrying in 2s");
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
                continue;
            }
        };
        if let Err(e) = client
            .set_mining_key(
                AiPowMinerWire::SetPubKey.to_wire(),
                Vec::new(),
                mining_pkh_configs.clone(),
            )
            .await
        {
            return Err(MinerError::Configure(format!("set_mining_key: {e}")));
        }
        let mut candidates = match client.watch_candidates(vec![b"mine-ai".to_vec()]).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "watch_candidates failed; reconnecting");
                continue;
            }
        };
        if let Err(e) = client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), true)
            .await
        {
            return Err(MinerError::Configure(format!("enable_mining(true): {e}")));
        }
        info!(
            profile = profile_name,
            "ai-pow-miner: subscribed + mining enabled; awaiting %mine-ai candidates"
        );

        let mut worker: Option<JoinHandle<GrindResult>> = None;
        // Shared stop flag so a grind on the blocking pool bails promptly at
        // shutdown (its u32 nonce sweep could otherwise run for minutes).
        let grind_cancel = Arc::new(AtomicBool::new(false));
        let reconnect = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    cancel_and_await_canonical_worker(&mut worker, &grind_cancel).await?;
                    return Ok(());
                }
                maybe_c = candidates.next() => {
                    let Some(c_res) = maybe_c else {
                        warn!("watch_candidates stream ended; reconnecting");
                        break true;
                    };
                    let candidate = match c_res {
                        Ok(c) => c,
                        Err(NodeClientError::Grpc(e)) => {
                            warn!(error = %e, "watch_candidates stream failed; reconnecting");
                            break true;
                        }
                        Err(e) => {
                            warn!(error = %e, "candidate decode error; skipping");
                            continue;
                        }
                    };
                    if candidate.kind != MiningCandidateKind::Ai {
                        continue;
                    }
                    if worker.is_some() {
                        info!(
                            profile = profile_name,
                            "new candidate replaces active gateway-free search"
                        );
                        cancel_and_await_canonical_worker(&mut worker, &grind_cancel).await?;
                    }
                    let inputs = match derive_nockchain_candidate_inputs(&candidate) {
                        Ok(x) => x,
                        Err(e) => {
                            warn!(error = %e, "could not derive candidate inputs; skipping");
                            continue;
                        }
                    };
                    let commit = inputs.nock_block_commitment;
                    let target = inputs.target;
                    info!(
                        profile = profile_name,
                        commit = %hex::encode(commit),
                        pow_len = inputs.pow_len,
                        target = %hex::encode(target),
                        "new %mine-ai candidate; starting AI-PoW search"
                    );
                    grind_cancel.store(false, Ordering::Relaxed);
                    let cancel = grind_cancel.clone();
                    let backend = backend.clone();
                    let profile = profile.clone();
                    worker = Some(tokio::task::spawn_blocking(move || {
                        profile.grind(commit, target, cancel, &*backend)
                    }));
                }
                joined = await_canonical_worker(&mut worker) => {
                    // A Joined outcome means the grind task resolved; consume the
                    // handle so the next poll does not re-await the completed task
                    // and replay the outcome.
                    if matches!(joined, CanonicalOutcome::Joined(_)) {
                        worker = None;
                    }
                    match joined {
                        CanonicalOutcome::None => {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        CanonicalOutcome::Joined(Ok(Ok(Some(block)))) => {
                            info!(
                                profile = profile_name,
                                commit = %hex::encode(block.commit()),
                                trace_height = block.trace_height(),
                                "AI-PoW block proved; submitting %pow to node"
                            );
                            let poke_start = Instant::now();
                            match block.build_poke() {
                                Ok(poke) => {
                                    let poke_build_ms = poke_start.elapsed().as_millis();
                                    let jam_start = Instant::now();
                                    let prepared = NodeClient::prepare_poke_wire(
                                        AiPowMinerWire::Mined.to_wire(),
                                        poke,
                                    );
                                    let jam_elapsed = jam_start.elapsed();
                                    let outcome = client
                                        .send_prepared_poke_with_timeout_or_cancel(
                                            prepared,
                                            NODE_POKE_ACK_TIMEOUT,
                                            shutdown.cancelled(),
                                        )
                                        .await;
                                    let submit_timings =
                                        AiPowSubmitTimings::from_outcome(jam_elapsed, &outcome);
                                    match outcome.into_result() {
                                        Ok(()) => info!(
                                            poke_build_ms,
                                            jam_ms = submit_timings.jam_ms,
                                            transport_ms = submit_timings.transport_ms,
                                            submit_status = ?submit_timings.status,
                                            send_started = submit_timings.send_started,
                                            "gateway-free %ai-pow submission acked by node"
                                        ),
                                        Err(e) => warn!(
                                            error = %e,
                                            poke_build_ms,
                                            jam_ms = submit_timings.jam_ms,
                                            transport_ms = submit_timings.transport_ms,
                                            submit_status = ?submit_timings.status,
                                            send_started = submit_timings.send_started,
                                            "submit gateway-free %pow failed (likely stale candidate)"
                                        ),
                                    }
                                }
                                Err(e) if profile.worker_errors_are_fatal() => return Err(e),
                                Err(e) => warn!(error = %e, "build gateway-free poke failed"),
                            }
                        }
                        CanonicalOutcome::Joined(Ok(Ok(None))) => {
                            // Grind cancelled (shutdown) or exhausted the nonce space.
                        }
                        CanonicalOutcome::Joined(Ok(Err(e)))
                            if profile.worker_errors_are_fatal() =>
                        {
                            return Err(MinerError::ProductionWorker(e.to_string()));
                        }
                        CanonicalOutcome::Joined(Ok(Err(e))) => {
                            warn!(error = %e, profile = profile_name, "gateway-free AI-PoW grind/prove failed");
                        }
                        CanonicalOutcome::Joined(Err(e)) => {
                            return Err(MinerError::WorkerJoin(format!("{e}")));
                        }
                    }
                }
            }
        };

        cancel_and_await_canonical_worker(&mut worker, &grind_cancel).await?;
        let _ = client
            .enable_mining(AiPowMinerWire::Enable.to_wire(), false)
            .await;
        if reconnect {
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    Ok(())
}

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

fn publish_inference_job(
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

fn invalidate_inference_job(rpc: &InferenceMiningRpc, generation: u64) -> Result<(), MinerError> {
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

type CandidateWorkerJoin =
    Result<(u64, Result<NockchainCandidateInputs, String>), tokio::task::JoinError>;

async fn abort_and_drain_candidate_workers(
    workers: &mut JoinSet<(u64, Result<NockchainCandidateInputs, String>)>,
) -> Result<(), MinerError> {
    workers.abort_all();
    while let Some(joined) = workers.join_next().await {
        match joined {
            Ok(_) => {}
            Err(e) if e.is_cancelled() => {}
            Err(e) => return Err(MinerError::WorkerJoin(format!("{e}"))),
        }
    }
    Ok(())
}

fn validate_mining_pkh_configs(configs: &[MiningPkhConfig]) -> Result<(), MinerError> {
    if configs.is_empty() {
        return Err(MinerError::InvalidConfig(
            "at least one mining PKH config is required".to_string(),
        ));
    }
    for (idx, config) in configs.iter().enumerate() {
        if config.share == 0 {
            return Err(MinerError::InvalidConfig(format!(
                "mining PKH config {idx} share must be nonzero"
            )));
        }
        if config.pkh.trim().is_empty() {
            return Err(MinerError::InvalidConfig(format!(
                "mining PKH config {idx} pkh must not be empty"
            )));
        }
    }
    Ok(())
}

fn pearl_work_refresh_interval(cfg: &MinerConfig) -> Duration {
    cfg.puzzle.pearl_merge.gateway.refresh_interval
}

enum InnerOutcome {
    Shutdown,
    StreamLost,
    Fatal(MinerError),
}

type PearlMergeWorkerJoin =
    Result<(u64, Result<PearlMergeWorkerOutput, PearlMergeMiningError>), tokio::task::JoinError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PearlGatewayWorkKind {
    Candidate,
    Refresh,
}

type PearlGatewayWorkerJoin = Result<
    (
        u64,
        PearlGatewayWorkKind,
        NockchainCandidateInputs,
        Result<PearlMergeCandidateJob, String>,
    ),
    tokio::task::JoinError,
>;

async fn abort_and_drain_pearl_gateway_workers(
    workers: &mut JoinSet<(
        u64,
        PearlGatewayWorkKind,
        NockchainCandidateInputs,
        Result<PearlMergeCandidateJob, String>,
    )>,
) -> Result<(), MinerError> {
    workers.abort_all();
    while let Some(joined) = workers.join_next().await {
        match joined {
            Ok(_) => {}
            Err(e) if e.is_cancelled() => {}
            Err(e) => return Err(MinerError::WorkerJoin(format!("{e}"))),
        }
    }
    Ok(())
}

async fn cancel_and_drain_pearl_workers(
    workers: &mut JoinSet<(u64, Result<PearlMergeWorkerOutput, PearlMergeMiningError>)>,
    current_cancel: &mut Option<MiningCancel>,
) -> Result<(), MinerError> {
    if let Some(cancel) = current_cancel.take() {
        cancel.cancel();
    }
    while let Some(joined) = workers.join_next().await {
        let _ = joined.map_err(|e| MinerError::WorkerJoin(format!("{e}")))?;
    }
    Ok(())
}
/// the 32-byte chain difficulty target and Nockchain block commitment.
///
/// **`nck_commitment`** is `BLAKE3(jam(candidate.block_header))`, where
/// `candidate.block_header` is the kernel-emitted `block-commitment:page:t`
/// noun. The field name is inherited from the shared ZK-miner substrate; for
/// AI-PoW this is a commitment noun, not a raw block header. Hashing its
/// canonical jam gives the 32-byte value carried in the Rust-owned `AIP1`
/// nonce's Nockchain aux commitment. That Hoon commitment is the same mining
/// surface used by zk-pow: it binds the parent block id, tx-id set, coinbase
/// split, timestamp, epoch counter, target, accumulated work, height, and page
/// message before the PoW artifact is installed.
///
/// **`target`** is decoded from the kernel-side bignum noun
/// `[%bn limbs]`, where `limbs` are little-endian u32 chunks. The
/// ai-pow primitive compares BLAKE3 attempt hashes as 256-bit
/// little-endian integers, so bignum values above `2^256 - 1`
/// saturate to `FF..FF`.
fn derive_job_inputs(candidate: &MiningCandidate) -> Result<(DifficultyTarget, [u8; 32]), String> {
    // Hash the jammed block_header to a 32-byte commitment.
    let header_bytes = candidate.block_header.jam();
    let nck = *blake3::hash(&header_bytes).as_bytes();
    let target = decode_chain_target_bignum(&candidate.target)?;
    Ok((target, nck))
}

fn expect_ai_pow_candidate_version(candidate: &MiningCandidate) -> Result<(), String> {
    if candidate.kind != MiningCandidateKind::Ai {
        return Err(format!(
            "AI-PoW miner expected %mine-ai candidate, got {:?}",
            candidate.kind
        ));
    }

    let space = candidate.version.noun_space();
    let version = unsafe { *candidate.version.root() }
        .in_space(&space)
        .as_atom()
        .map_err(|_| "AI-PoW mining candidate version must be an atom".to_string())?
        .as_u64()
        .map_err(|_| "AI-PoW mining candidate version must fit in u64".to_string())?;

    if version != AI_POW_MINE_CANDIDATE_VERSION {
        return Err(format!(
            "AI-PoW miner expected %mine-ai version %{AI_POW_MINE_CANDIDATE_VERSION}, got %{version}"
        ));
    }

    Ok(())
}

struct PearlMergeCandidateJob {
    header: PearlIncompleteBlockHeader,
    gateway_mining_job: PearlGatewayResolvedMiningJob,
    aux_inclusion: PearlAuxInclusionProof,
    target: DifficultyTarget,
    aux: PearlNockchainAux,
}

struct PearlMergeMinedSubmission {
    ticket: PearlMergeMinedTicket,
    gateway_mining_job: PearlGatewayResolvedMiningJob,
    aux_inclusion: PearlAuxInclusionProof,
}

struct PearlMergePreparedSubmission {
    mined: PearlMergeMinedSubmission,
    prove_timings: AiPowProveTimings,
    jam_elapsed: Duration,
    prepared: PreparedPoke,
}

enum PearlMergeWorkerOutput {
    PearlOnly {
        mined: PearlMergeMinedSubmission,
        pearl_submit_failed: bool,
    },
    NockchainPrepared(PearlMergePreparedSubmission),
}
/// A Gateway job whose wire certificate version was validated as V3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PearlCertificateV3;

impl PearlCertificateV3 {
    const fn wire_version(self) -> u32 {
        PEARL_GATEWAY_CERTIFICATE_VERSION_V3
    }
}

#[derive(Clone, Debug)]
struct PearlGatewayResolvedMiningJob {
    header: PearlIncompleteBlockHeader,
    target: serde_json::Value,
    certificate: PearlCertificateV3,
    aux_inclusion: Option<PearlAuxInclusionProof>,
}

#[derive(Clone, Copy)]
struct NockchainCandidateInputs {
    target: DifficultyTarget,
    nock_block_commitment: [u8; 32],
    pow_len: u64,
}

fn derive_nockchain_candidate_inputs(
    candidate: &MiningCandidate,
) -> Result<NockchainCandidateInputs, String> {
    expect_ai_pow_candidate_version(candidate)?;
    let (target, nock_block_commitment) = derive_job_inputs(candidate)?;
    Ok(NockchainCandidateInputs {
        target,
        nock_block_commitment,
        pow_len: candidate.pow_len,
    })
}
const INFERENCE_DENSE_TILE: u32 = 16;

#[derive(Clone, Copy)]
struct InferenceNodeCandidate {
    generation: u64,
    inputs: NockchainCandidateInputs,
    incomplete_header: [u8; ai_pow::pearl_compat::PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
}

fn reconstruct_inference_dense_attempt(
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

#[cfg(test)]
fn derive_pearl_merge_job_inputs(
    cfg: &MinerConfig,
    candidate: &MiningCandidate,
) -> Result<PearlMergeCandidateJob, String> {
    let candidate_inputs = derive_nockchain_candidate_inputs(candidate)?;
    derive_pearl_merge_job_inputs_from_nockchain(cfg, &candidate_inputs)
}

fn derive_pearl_merge_job_inputs_from_nockchain(
    cfg: &MinerConfig,
    candidate: &NockchainCandidateInputs,
) -> Result<PearlMergeCandidateJob, String> {
    let pearl = &cfg.puzzle.pearl_merge;
    let mut aux = pearl.aux_template.clone();
    aux.nock_block_commitment = candidate.nock_block_commitment;
    let aux_commitment = aux
        .commitment()
        .map_err(|e| format!("build Nockchain aux commitment: {e}"))?;
    let (header, gateway_mining_job, aux_inclusion) = {
        let job = fetch_pearl_gateway_mining_job(&pearl.gateway, Some(&aux_commitment))
            .map_err(|e| format!("resolve Pearl work header: {e}"))?;
        let (header, aux_inclusion) = match job.aux_inclusion.clone() {
            Some(aux_inclusion) => {
                verify_pearl_aux_inclusion(&job.header, &aux_commitment, &aux_inclusion)
                    .map_err(|e| format!("verify Pearl Gateway aux inclusion: {e}"))?;
                (job.header, aux_inclusion)
            }
            None => {
                return Err(
                    "Pearl Gateway response did not include requested aux_inclusion".to_string(),
                );
            }
        };
        (header, job, aux_inclusion)
    };
    Ok(PearlMergeCandidateJob {
        header,
        gateway_mining_job,
        aux_inclusion,
        target: candidate.target,
        aux,
    })
}

#[derive(Debug, Error)]
enum PearlGatewayError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Pearl gateway returned error: {0}")]
    Rpc(String),
    #[error("Pearl gateway mining job omitted certificate version")]
    MissingCertificateVersion,
    #[error("Pearl gateway requires certificate version {expected}, got {actual}")]
    UnsupportedCertificateVersion { expected: u32, actual: u32 },
    #[error(
        "Pearl Gateway mining job header does not match the aux-bearing mined header; \
         skipping submitPlainProof because Gateway acknowledges before its async stale-header check"
    )]
    MiningJobHeaderMismatch,
    #[error("Pearl gateway response id mismatch: expected {expected}, got {actual}")]
    ResponseIdMismatch { expected: u64, actual: String },
    #[error("Pearl gateway mining job target is outside uint256")]
    TargetOverflow,
    #[error("Pearl gateway mining job target does not match header nbits")]
    TargetNbitsMismatch,
    #[error("Pearl gateway response line exceeded {limit} bytes")]
    ResponseTooLarge { limit: usize },
    #[error("Pearl gateway aux inclusion merkle branch digest has wrong length: got {0}")]
    AuxInclusionDigestLen(usize),
    #[error("Pearl gateway aux inclusion coinbase tx exceeded {limit} bytes: got {actual}")]
    AuxInclusionCoinbaseTooLarge { actual: usize, limit: usize },
    #[error("Pearl gateway aux inclusion merkle branch exceeded {limit} entries: got {actual}")]
    AuxInclusionMerkleBranchTooDeep { actual: usize, limit: usize },
    #[error("Pearl gateway header: {0}")]
    Header(#[from] PearlCompatError),
    #[cfg(not(unix))]
    #[error("Unix socket Pearl gateway transport is not supported on this platform")]
    UnixSocketUnsupported,
}

#[derive(Debug, Deserialize)]
struct PearlGatewayMiningInfoRpcResponse {
    id: serde_json::Value,
    result: Option<PearlGatewayMiningJob>,
    error: Option<PearlGatewayRpcError>,
}

#[derive(Debug, Deserialize)]
struct PearlGatewayRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PearlGatewayMiningJob {
    incomplete_header_bytes: String,
    target: serde_json::Value,
    #[serde(default)]
    cert_version: Option<u32>,
    #[serde(default)]
    aux_inclusion: Option<PearlGatewayAuxInclusion>,
}

#[derive(Debug, Deserialize)]
struct PearlGatewayAuxInclusion {
    coinbase_tx: String,
    #[serde(default)]
    merkle_branch: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PearlGatewaySubmitRpcResponse {
    id: serde_json::Value,
    result: Option<serde_json::Value>,
    error: Option<PearlGatewayRpcError>,
}

fn parse_pearl_gateway_certificate_v3(
    cert_version: Option<u32>,
) -> Result<PearlCertificateV3, PearlGatewayError> {
    let actual = cert_version.ok_or(PearlGatewayError::MissingCertificateVersion)?;
    if actual != PEARL_GATEWAY_CERTIFICATE_VERSION_V3 {
        return Err(PearlGatewayError::UnsupportedCertificateVersion {
            expected: PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
            actual,
        });
    }
    Ok(PearlCertificateV3)
}

fn fetch_pearl_gateway_mining_job(
    config: &PearlGatewayMinerRpcConfig,
    aux_commitment: Option<&[u8; 32]>,
) -> Result<PearlGatewayResolvedMiningJob, PearlGatewayError> {
    let request_id = 1u64;
    let params = match aux_commitment {
        Some(aux_commitment) => {
            let mut coinbase_aux_flags =
                Vec::with_capacity(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG.len() + 32);
            coinbase_aux_flags.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
            coinbase_aux_flags.extend_from_slice(aux_commitment);
            let coinbase_aux_flags = {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(coinbase_aux_flags)
            };
            json!({
                "coinbase_aux_flags": coinbase_aux_flags,
                "return_aux_inclusion": true,
            })
        }
        None => json!({}),
    };
    let request = json!({
        "jsonrpc": "2.0",
        "method": "getMiningInfo",
        "params": params,
        "id": request_id,
    })
    .to_string();
    let response_line = exchange_pearl_gateway_request(config, &request)?;

    let response: PearlGatewayMiningInfoRpcResponse = serde_json::from_str(&response_line)?;
    if response.id != request_id {
        return Err(PearlGatewayError::ResponseIdMismatch {
            expected: request_id,
            actual: response.id.to_string(),
        });
    }
    if let Some(error) = response.error {
        let mut msg = format!("{}: {}", error.code, error.message);
        if let Some(data) = error.data {
            msg.push_str(": ");
            msg.push_str(&data);
        }
        return Err(PearlGatewayError::Rpc(msg));
    }
    let job = response
        .result
        .ok_or_else(|| PearlGatewayError::Rpc("missing result".to_string()))?;
    let certificate = parse_pearl_gateway_certificate_v3(job.cert_version)?;
    validate_pearl_gateway_target_uint256(&job.target)?;

    let header_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(job.incomplete_header_bytes)?
    };
    let header = PearlIncompleteBlockHeader::from_bytes(&header_bytes)?;
    validate_pearl_gateway_target_matches_header_nbits(&job.target, header.nbits)?;
    Ok(PearlGatewayResolvedMiningJob {
        header,
        target: job.target,
        certificate,
        aux_inclusion: job
            .aux_inclusion
            .map(decode_pearl_gateway_aux_inclusion)
            .transpose()?,
    })
}

fn decode_pearl_gateway_aux_inclusion(
    value: PearlGatewayAuxInclusion,
) -> Result<PearlAuxInclusionProof, PearlGatewayError> {
    use base64::Engine as _;
    if value.merkle_branch.len() > PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH {
        return Err(PearlGatewayError::AuxInclusionMerkleBranchTooDeep {
            actual: value.merkle_branch.len(),
            limit: PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH,
        });
    }
    let coinbase_tx = base64::engine::general_purpose::STANDARD.decode(value.coinbase_tx)?;
    if coinbase_tx.len() > PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES {
        return Err(PearlGatewayError::AuxInclusionCoinbaseTooLarge {
            actual: coinbase_tx.len(),
            limit: PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES,
        });
    }
    let mut merkle_branch = Vec::with_capacity(value.merkle_branch.len());
    for encoded in value.merkle_branch {
        let digest = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        let digest: [u8; 32] = digest
            .as_slice()
            .try_into()
            .map_err(|_| PearlGatewayError::AuxInclusionDigestLen(digest.len()))?;
        merkle_branch.push(digest);
    }
    Ok(PearlAuxInclusionProof {
        coinbase_tx,
        merkle_branch,
    })
}

fn submit_pearl_gateway_plain_proof(
    config: &PearlGatewayMinerRpcConfig,
    plain_proof_base64: &str,
    mined_header: &PearlIncompleteBlockHeader,
    job: &PearlGatewayResolvedMiningJob,
) -> Result<(), PearlGatewayError> {
    if job.header != *mined_header {
        return Err(PearlGatewayError::MiningJobHeaderMismatch);
    }
    validate_pearl_gateway_target_uint256(&job.target)?;
    let request_id = 2u64;
    let incomplete_header_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(job.header.to_bytes())
    };
    let request = json!({
        "jsonrpc": "2.0",
        "method": "submitPlainProof",
        "params": {
            "plain_proof": plain_proof_base64,
            "mining_job": {
                "incomplete_header_bytes": incomplete_header_bytes,
                "target": &job.target,
                "cert_version": job.certificate.wire_version(),
            },
        },
        "id": request_id,
    })
    .to_string();
    let response_line = exchange_pearl_gateway_request(config, &request)?;
    let response: PearlGatewaySubmitRpcResponse = serde_json::from_str(&response_line)?;
    if response.id != request_id {
        return Err(PearlGatewayError::ResponseIdMismatch {
            expected: request_id,
            actual: response.id.to_string(),
        });
    }
    if let Some(error) = response.error {
        let mut msg = format!("{}: {}", error.code, error.message);
        if let Some(data) = error.data {
            msg.push_str(": ");
            msg.push_str(&data);
        }
        return Err(PearlGatewayError::Rpc(msg));
    }
    match response.result {
        Some(serde_json::Value::String(result)) if result == "submitted" => Ok(()),
        Some(other) => Err(PearlGatewayError::Rpc(format!(
            "unexpected submitPlainProof result: {other}"
        ))),
        None => Err(PearlGatewayError::Rpc("missing result".to_string())),
    }
}

fn exchange_pearl_gateway_request(
    config: &PearlGatewayMinerRpcConfig,
    request: &str,
) -> Result<String, PearlGatewayError> {
    match &config.transport {
        PearlGatewayTransport::Tcp { host, port } => {
            let mut stream = connect_tcp_with_timeout(host, *port, config.request_timeout)?;
            stream.set_read_timeout(Some(config.request_timeout))?;
            stream.set_write_timeout(Some(config.request_timeout))?;
            stream.write_all(request.as_bytes())?;
            stream.write_all(b"\n")?;
            stream.flush()?;
            let mut reader = BufReader::new(stream);
            read_bounded_gateway_response_line(&mut reader)
        }
        PearlGatewayTransport::UnixSocket { path } => {
            #[cfg(unix)]
            {
                let mut stream = UnixStream::connect(path)?;
                stream.set_read_timeout(Some(config.request_timeout))?;
                stream.set_write_timeout(Some(config.request_timeout))?;
                stream.write_all(request.as_bytes())?;
                stream.write_all(b"\n")?;
                stream.flush()?;
                let mut reader = BufReader::new(stream);
                read_bounded_gateway_response_line(&mut reader)
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(PearlGatewayError::UnixSocketUnsupported);
            }
        }
    }
}

fn read_bounded_gateway_response_line<R: BufRead>(
    reader: &mut R,
) -> Result<String, PearlGatewayError> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let newline_at = available.iter().position(|&b| b == b'\n');
        let consume_len = newline_at.map_or(available.len(), |idx| idx + 1);
        if bytes.len() + consume_len > PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES {
            return Err(PearlGatewayError::ResponseTooLarge {
                limit: PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES,
            });
        }
        bytes.extend_from_slice(&available[..consume_len]);
        reader.consume(consume_len);
        if newline_at.is_some() {
            break;
        }
    }
    Ok(String::from_utf8(bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?)
}

fn connect_tcp_with_timeout(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, std::io::Error> {
    let addrs = (host, port).to_socket_addrs()?;
    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "Pearl gateway host resolved to no socket addresses",
        )
    }))
}

fn validate_pearl_gateway_target_uint256(
    target: &serde_json::Value,
) -> Result<(), PearlGatewayError> {
    match target {
        serde_json::Value::Number(number) => {
            let digits = number.to_string();
            parse_decimal_uint256_le(&digits).map(|_| ())
        }
        _ => Err(PearlGatewayError::TargetOverflow),
    }
}

fn validate_pearl_gateway_target_matches_header_nbits(
    target: &serde_json::Value,
    nbits: u32,
) -> Result<(), PearlGatewayError> {
    let serde_json::Value::Number(number) = target else {
        return Err(PearlGatewayError::TargetOverflow);
    };
    let target = parse_decimal_uint256_le(&number.to_string())?;
    let expected = pearl_nbits_to_target_le(nbits);
    if target == expected {
        Ok(())
    } else {
        Err(PearlGatewayError::TargetNbitsMismatch)
    }
}

fn parse_decimal_uint256_le(digits: &str) -> Result<[u8; 32], PearlGatewayError> {
    let trimmed = digits.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PearlGatewayError::TargetOverflow);
    }
    const MAX_UINT256_DECIMAL: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    let normalized = trimmed.trim_start_matches('0');
    if normalized.is_empty() {
        return Ok([0u8; 32]);
    }
    if normalized.len() > MAX_UINT256_DECIMAL.len()
        || (normalized.len() == MAX_UINT256_DECIMAL.len() && normalized > MAX_UINT256_DECIMAL)
    {
        return Err(PearlGatewayError::TargetOverflow);
    }

    let mut out = [0u8; 32];
    for digit in normalized.bytes() {
        let digit = digit - b'0';
        let mut carry = u16::from(digit);
        for byte in &mut out {
            let v = u16::from(*byte) * 10 + carry;
            *byte = v as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            return Err(PearlGatewayError::TargetOverflow);
        }
    }
    Ok(out)
}

/// Decode a kernel `%mine-ai` candidate's `target` noun (a `bignum` chunk list)
/// into the 32-byte little-endian consensus target.
///
/// Public so a consumer driving the real kernel — the acceptance integration
/// test — grinds against the SAME target the node handed out, rather than a
/// value reconstructed from constants.
pub fn decode_chain_target_bignum(target: &NounSlab) -> Result<DifficultyTarget, String> {
    let space = target.noun_space();
    let root = unsafe { *target.root() };
    let target_cell = root
        .in_space(&space)
        .as_cell()
        .map_err(|_| "target must be a Hoon bignum cell [%bn limbs]".to_string())?;
    if !target_cell.head().eq_bytes("bn") {
        return Err("target must have %bn bignum tag".to_string());
    }

    let mut out = [0u8; 32];
    let mut list = target_cell.tail().noun();
    let mut limb_index = 0usize;
    let mut saturate = false;

    loop {
        if noun_is_zero_atom(list, &space)? {
            break;
        }
        if limb_index >= MAX_CHAIN_TARGET_U32_LIMBS {
            return Err(format!(
                "target bignum exceeds {MAX_CHAIN_TARGET_U32_LIMBS} u32 limbs"
            ));
        }

        let limb_cell = list
            .in_space(&space)
            .as_cell()
            .map_err(|_| "target bignum limbs must be a proper list".to_string())?;
        let limb = limb_cell
            .head()
            .as_atom()
            .map_err(|_| "target bignum limb must be an atom".to_string())?
            .as_u64()
            .map_err(|_| "target bignum limb does not fit in u64".to_string())?;
        let limb =
            u32::try_from(limb).map_err(|_| "target bignum limb must fit in u32".to_string())?;

        if limb_index < 8 {
            let offset = limb_index * 4;
            out[offset..offset + 4].copy_from_slice(&limb.to_le_bytes());
        } else if limb != 0 {
            saturate = true;
        }

        list = limb_cell.tail().noun();

        limb_index += 1;
    }

    Ok(if saturate { [0xFF; 32] } else { out })
}

fn noun_is_zero_atom(
    noun: nockvm::noun::Noun,
    space: &nockvm::noun::NounSpace,
) -> Result<bool, String> {
    match noun.in_space(space).as_atom() {
        Ok(atom) => atom
            .as_u64()
            .map(|value| value == 0)
            .map_err(|_| "target bignum list terminator does not fit in u64".to_string()),
        Err(_) => Ok(false),
    }
}

fn spawn_candidate_ingestion(
    workers: &mut JoinSet<(u64, Result<NockchainCandidateInputs, String>)>,
    generation: u64,
    candidate: MiningCandidate,
) {
    workers.spawn_blocking(move || (generation, derive_nockchain_candidate_inputs(&candidate)));
}

/// Resolve Pearl Gateway work for a candidate generation off the async loop.
fn spawn_pearl_gateway_work(
    workers: &mut JoinSet<(
        u64,
        PearlGatewayWorkKind,
        NockchainCandidateInputs,
        Result<PearlMergeCandidateJob, String>,
    )>,
    generation: u64,
    kind: PearlGatewayWorkKind,
    cfg: &MinerConfig,
    candidate_inputs: NockchainCandidateInputs,
) {
    let cfg = cfg.clone();
    workers.spawn_blocking(move || {
        (
            generation,
            kind,
            candidate_inputs,
            derive_pearl_merge_job_inputs_from_nockchain(&cfg, &candidate_inputs),
        )
    });
}

/// Spawn a Pearl-compatible worker for the given candidate generation.
/// Ticket search, Pearl Gateway submission, recursive proof construction, and
/// poke jam run off the async client loop. The caller submits only the prepared
/// Nockchain poke for the still-current generation.
fn spawn_pearl_merge_attempt(
    workers: &mut JoinSet<(u64, Result<PearlMergeWorkerOutput, PearlMergeMiningError>)>,
    generation: u64,
    cfg: &MinerConfig,
    backend: Arc<dyn SearchBackend>,
    job_inputs: PearlMergeCandidateJob,
    mine_opts: PearlMergeMineOptions,
    cancel: MiningCancel,
) {
    let cfg = cfg.clone();
    workers.spawn_blocking(move || {
        let params = cfg.puzzle.params;
        let a = cfg.puzzle.a.clone();
        let b = cfg.puzzle.b.clone();
        let pearl = cfg.puzzle.pearl_merge.clone();
        let job = PearlMergeMiningJob {
            header: &job_inputs.header,
            config: &pearl.mining_config,
            params: &params,
            nockchain_target: job_inputs.target,
            a: &a,
            b: &b,
            max_pattern_len: pearl.max_pattern_len,
            aux: job_inputs.aux,
        };
        let proof_cancel = cancel.clone();
        let ticket = match pearl_mining::run_with_backend(&job, &mine_opts, cancel, &*backend) {
            Ok(ticket) => ticket,
            Err(e) => return (generation, Err(e)),
        };
        let mined = PearlMergeMinedSubmission {
            ticket,
            gateway_mining_job: job_inputs.gateway_mining_job,
            aux_inclusion: job_inputs.aux_inclusion,
        };

        let mut pearl_submit_failed = false;
        if mined.ticket.pearl_target_hit {
            if let Err(e) = submit_pearl_solution_to_gateway(&cfg, &pearl, &mined) {
                pearl_submit_failed = true;
                warn!(error = %e, "submit Pearl Gateway plain proof failed");
            }
        }

        if !mined.ticket.nockchain_target_hit {
            return (
                generation,
                Ok(PearlMergeWorkerOutput::PearlOnly {
                    mined,
                    pearl_submit_failed,
                }),
            );
        }

        if proof_cancel.is_cancelled() {
            return (generation, Err(PearlMergeMiningError::Cancelled));
        }

        let proof_start = Instant::now();
        let proof = match pearl.build_certificate_for_attempt(&mined.ticket.attempt) {
            Ok(proof) => proof,
            Err(e) => {
                return (
                    generation,
                    Err(PearlMergeMiningError::CertificateBuild(e.to_string())),
                );
            }
        };
        let certificate_build_ms = proof_start.elapsed().as_millis();
        let poke_start = Instant::now();
        let poke = match build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
            &mined.ticket.attempt, &mined.aux_inclusion, &a, &b, pearl.max_pattern_len, &proof,
        ) {
            Ok(poke) => poke,
            Err(e) => {
                return (
                    generation,
                    Err(PearlMergeMiningError::CertificateBuild(e.to_string())),
                );
            }
        };
        let prove_timings = AiPowProveTimings {
            certificate_build_ms,
            poke_build_ms: poke_start.elapsed().as_millis(),
        };
        let jam_start = Instant::now();
        let prepared = NodeClient::prepare_poke_wire(AiPowMinerWire::Mined.to_wire(), poke);
        let jam_elapsed = jam_start.elapsed();

        (
            generation,
            Ok(PearlMergeWorkerOutput::NockchainPrepared(
                PearlMergePreparedSubmission {
                    mined,
                    prove_timings,
                    jam_elapsed,
                    prepared,
                },
            )),
        )
    });
}

fn pearl_mine_opts_with_attempt_start(
    cfg: &MinerConfig,
    attempt_start: u64,
) -> PearlMergeMineOptions {
    let mut opts = cfg.puzzle.pearl_merge.mine_opts.clone();
    opts.attempt_start = attempt_start;
    opts
}

fn submit_pearl_solution_to_gateway(
    cfg: &MinerConfig,
    pearl_cfg: &PearlMergeSubmissionConfig,
    mined: &PearlMergeMinedSubmission,
) -> Result<(), String> {
    let gateway = &pearl_cfg.gateway;
    let mined_header = mined.ticket.attempt.public_params.block_header;
    let gateway_job = &mined.gateway_mining_job;
    let plain = PearlPlainProof::from_attempt(
        &cfg.puzzle.params, &mined.ticket.attempt, &cfg.puzzle.a, &cfg.puzzle.b,
    )
    .map_err(|e| format!("build Pearl plain proof: {e}"))?;
    let plain_proof_base64 = plain
        .to_base64_bincode1()
        .map_err(|e| format!("serialize Pearl plain proof: {e}"))?;
    submit_pearl_gateway_plain_proof(gateway, &plain_proof_base64, &mined_header, gateway_job)
        .map_err(|e| e.to_string())
}

/// Internal wrapper for a prebuilt Pearl-format-compatible `%ai-pow` artifact:
///
/// ```hoon
/// [%command %pow %ai-pow nonce cert]
/// ```
///
/// `artifact` must already be the Hoon-compatible `%ai-pow` artifact:
///
/// ```hoon
/// [%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]
/// ```
///
/// The helper is crate-internal so external callers cannot bypass the
/// recursive-run construction path by handing in an arbitrary prebuilt artifact.
/// It decodes the artifact tag, opaque nonce, and certificate metadata before
/// wrapping it. It deliberately does not traverse the recursive proof-node tail;
/// ticket-derived helpers construct that tail from typed recursive proof data,
/// and consensus verification performs proof-node traversal only after cheap
/// statement checks pass.
pub(crate) fn build_ai_pow_pearl_merge_certificate_poke(
    artifact: &NounSlab,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    decode_ai_pow_pearl_merge_artifact_metadata_slab(artifact, CertificateNounLimits::default())?;

    let artifact_space = artifact.noun_space();
    let mut slab = NounSlab::new();
    let artifact = slab.copy_into(unsafe { *artifact.root() }, &artifact_space);
    let payload = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), artifact]);
    slab.set_root(payload);
    Ok(slab)
}

/// Test-only poke builder from an already-serialized recursive proof node.
///
/// Production callers use
/// [`build_ai_pow_pearl_merge_certificate_poke_from_ticket_compact_recursive_run`].
#[cfg(test)]
pub(crate) fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_node(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    certificate: &AiProofNode,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    let artifact =
        crate::certificate_noun::build_ai_pow_pearl_merge_artifact_noun_from_ticket_node(
            attempt, aux_inclusion, a_row_major, b_col_major, max_pattern_len, certificate,
        )?;
    build_ai_pow_pearl_merge_certificate_poke(&artifact)
}

/// Crate-internal poke builder for the run loop after its certificate builder
/// has produced private-field [`PearlMergeCertificateProof`] data. Tests use it
/// with synthetic proof nodes; production code gets that wrapper only through
/// the recursive prover selected by [`PearlMergeSubmissionConfig::new_compact_recursive`].
#[cfg(test)]
pub(crate) fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_public_inputs_node(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    public_inputs: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node(
        attempt, aux_inclusion, a_row_major, b_col_major, max_pattern_len, public_inputs,
        certificate,
    )?;
    build_ai_pow_pearl_merge_certificate_poke(&artifact)
}

/// Crate-internal production handoff for the run loop.
///
/// The ticket-derived statement metadata is recomputed from the candidate,
/// trusted matrices, and aux inclusion. The recursive-run metadata copied into
/// `proof` must match that recomputation before the proof node is serialized
/// into a command. This catches wrong-ticket or stale-run builders before the
/// node receives a doomed block proof.
pub(crate) fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    proof: &PearlMergeCertificateProof,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node(
        attempt, aux_inclusion, a_row_major, b_col_major, max_pattern_len, &proof.public_inputs,
        &proof.certificate,
    )?;
    let decoded = decode_ai_pow_pearl_merge_artifact_metadata_slab(
        &artifact,
        CertificateNounLimits::default(),
    )?;
    if decoded.certificate.zk_params != proof.zk_params {
        return Err(AiPowCertificatePokeError::PearlMergeArtifact(
            CertificateNounError::PearlMergePublicInputMismatch("recursive-run.zk-params"),
        ));
    }
    if decoded.certificate.found_idx != proof.found_idx {
        return Err(AiPowCertificatePokeError::PearlMergeArtifact(
            CertificateNounError::PearlMergePublicInputMismatch("recursive-run.found-idx"),
        ));
    }
    if decoded.certificate.trace_height != proof.trace_height {
        return Err(AiPowCertificatePokeError::PearlMergeArtifact(
            CertificateNounError::PearlMergePublicInputMismatch("recursive-run.trace-height"),
        ));
    }
    if decoded.certificate.commitments != proof.commitments {
        return Err(AiPowCertificatePokeError::PearlMergeArtifact(
            CertificateNounError::PearlMergePublicInputMismatch("recursive-run.commitments"),
        ));
    }
    build_ai_pow_pearl_merge_certificate_poke(&artifact)
}

/// Build the production Pearl-format-compatible Nockchain consensus poke from
/// a successful shared ticket and the matching checkpoint recursive prover run.
///
/// This is retained for checkpoint/regression workflows. Production callers use
/// [`build_ai_pow_pearl_merge_certificate_poke_from_ticket_compact_recursive_run`].
#[doc(hidden)]
pub fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_recursive_run(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    run: &AiPowRecursiveCertificateRun,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_recursive_run(
        attempt, aux_inclusion, a_row_major, b_col_major, max_pattern_len, run,
    )?;
    build_ai_pow_pearl_merge_certificate_poke(&artifact)
}

/// Build the production Pearl-format-compatible Nockchain consensus poke from
/// a successful shared ticket and the matching compact recursive prover run.
pub fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_compact_recursive_run(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    run: &AiPowCompactRecursiveCertificateRun,
) -> Result<NounSlab, AiPowCertificatePokeError> {
    let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
        attempt, aux_inclusion, a_row_major, b_col_major, max_pattern_len, run,
    )?;
    build_ai_pow_pearl_merge_certificate_poke(&artifact)
}

// ──────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    //! Integration tests for the AI-PoW miner run loop.
    //!
    //! Strategy: stand up a private `NockAppService` gRPC server on an
    //! ephemeral port (same fixture pattern as `zk-pow-miner`'s
    //! run-loop tests), drive [`run`] against it, push a synthetic
    //! `%mine-ai` effect, and assert the miner pokes an
    //! `AiPowMinerWire::Mined` slab back at the server within a
    //! generous timeout. Uses `MatmulParams::TEST_SMALL` + trivial
    //! uint256 `FF..FF` target so the real ai-pow prover wins on extranonce 0.

    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use ai_pow::params::MatmulParams;
    use ai_pow::pearl_compat::{
        evaluate_pearl_merge_checked_ticket_attempt, evaluate_pearl_merge_ticket_attempt,
        verify_pearl_aux_inclusion, PearlIncompleteBlockHeader, PearlMiningConfig,
        PearlNockchainAux, PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE,
        PEARL_MMA_INT7XINT7_TO_INT32,
    };
    use ai_pow::synth::synth_matrices;
    use ai_pow::zk_bridge::ZkPublicCommitments;
    use ai_pow_zk::{CompositePublicInputs, ZkParams};
    use nockapp::driver::{IOAction, NockAppHandle};
    use nockapp::noun::slab::NounSlab;
    use nockapp::NockAppExit;
    use nockapp_grpc::services::private_nockapp::server::PrivateNockAppGrpcServer;
    use nockchain_mining_common::MiningPkhConfig;
    use nockvm::noun::{NounAllocator, D, T};
    use nockvm_macros::tas;
    use once_cell::sync::Lazy;
    use tokio::sync::{broadcast, mpsc, Mutex as TMutex};

    use super::*;
    use crate::canonical::prove_canonical_moe_block_at;
    use crate::certificate_noun::{
        build_ai_pow_pearl_merge_artifact_noun_from_node, decode_ai_pow_pearl_merge_artifact_noun,
        pearl_merge_recursive_certificate_parts_from_ticket,
        pearl_merge_recursive_public_inputs_from_work, AiProofNode, PearlMergePublicStatementShape,
    };
    use crate::inference::InferenceSchedulerState;
    use crate::pearl_mining::{
        self, PearlMergeMineOptions, PearlMergeMiningError, PearlMergeMiningJob,
    };
    use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};
    use crate::wire::AiPowMinerWire;

    struct CorruptCanonicalBackend {
        jackpot_hash: [u8; 32],
    }

    impl SearchBackend for CorruptCanonicalBackend {
        fn search_dense(
            &self,
            _: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
            _: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            Ok(None)
        }

        fn search_canonical(
            &self,
            _: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
            batch: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            Ok(Some(SearchWinner {
                ordinal: batch.start,
                jackpot_hash: self.jackpot_hash,
            }))
        }
    }

    /// Measure the two cost components of AI-PoW mining on THIS machine, using the
    /// exact canonical shape the run loop mines (`CANONICAL_MATMUL_PARAMS`,
    /// hw/e/top-k). AI block time = (expected grind attempts x t_attempt) + t_prove,
    /// where expected attempts ~= 2^256 / (target · F) and `F` is the canonical
    /// shape work factor. Using 2^256/target here instead overstates the attempt
    /// count by `F` (2^16 for this shape), so any difficulty tuned against it
    /// lands `F` times too easy. These numbers are what the fakenet AI difficulty
    /// is tuned to (via the AI ASERT anchor target) so the AI and ZK puzzles take
    /// comparable wall-clock per block. Ignored (slow); run:
    ///   cargo test --release -p ai-pow-miner --features node canonical_mining_costs -- --ignored --nocapture
    /// **The canonical miner and the consensus verifier must accept exactly the
    /// same jackpots.**
    ///
    /// The miner's grind threshold is derived from the mining config it puts in
    /// the statement; the verifier's is derived from the config it re-parses out
    /// of that statement. This pins them equal for the real canonical statement,
    /// so a miner that reverts to comparing the jackpot against the bare
    /// consensus target — which costs `F` times more work per block than
    /// consensus asks for, and mis-tunes every difficulty derived from the
    /// miner's measured rate — fails here instead of in production.
    #[test]
    fn canonical_grind_threshold_matches_the_consensus_verifier() {
        let commit = [0x5au8; 32];
        let public = crate::canonical::canonical_public_params(
            &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K, commit, 0,
        )
        .expect("canonical public params");

        let miner_factor = canonical_shape_work_factor().expect("miner factor");
        let verifier_factor = public
            .difficulty_adjustment_factor()
            .expect("verifier factor");
        assert_eq!(
            miner_factor, verifier_factor,
            "canonical miner and consensus verifier disagree on the shape work factor"
        );
        // The canonical shape is h=w=8, k=1024, r=64 => F = 64 * 1024 = 2^16.
        assert_eq!(miner_factor, 1 << 16);

        for target_byte_index in [20usize, 24, 28] {
            let mut target = [0u8; 32];
            target[target_byte_index] = 0x01;
            let miner = canonical_grind_threshold(&target).expect("miner threshold");
            let verifier = public
                .nockchain_adjusted_target(&target)
                .expect("verifier threshold");
            assert_eq!(
                miner,
                verifier,
                "miner and verifier thresholds diverge at target 2^{}",
                target_byte_index * 8
            );
            // And the threshold really is looser than the bare target, i.e. the
            // miner is not silently discarding valid wins.
            assert!(
                !ai_pow::tile_hash::hash_le_target(&miner, &target) || miner == target,
                "effective threshold must be >= the consensus target"
            );
        }
    }

    /// A target consensus may legitimately emit must always be scalable by the
    /// canonical shape. If this fails the canonical miner cannot mine at that
    /// difficulty at all — and since the AI ASERT only advances on accepted AI
    /// blocks, the puzzle would not recover.
    #[test]
    fn canonical_grind_threshold_covers_the_whole_consensus_target_domain() {
        let max = ai_pow::difficulty::AI_POW_MAX_CONSENSUS_TARGET;
        canonical_grind_threshold(&max)
            .expect("the canonical miner must be able to grind at the maximum consensus AI target");
    }

    #[test]
    #[ignore]
    fn canonical_mining_costs() {
        let commit = [0x5au8; 32];
        let factor = canonical_shape_work_factor().expect("canonical shape work factor");
        println!(
            "canonical shape work factor F = {factor} (2^{})",
            factor.ilog2()
        );

        // (a) Per-attempt grind cost: evaluate_canonical_moe_jackpot (matmul +
        // jackpot, no cert). This is the tunable proof-of-work unit.
        let warm =
            evaluate_canonical_moe_jackpot(&CANONICAL_MATMUL_PARAMS, 8, 2, 1, commit, 0).unwrap();
        assert_ne!(warm, [0u8; 32]);
        let attempts = 200u32;
        let t = std::time::Instant::now();
        for xn in 0..attempts {
            let _ = evaluate_canonical_moe_jackpot(&CANONICAL_MATMUL_PARAMS, 8, 2, 1, commit, xn)
                .expect("grind attempt");
        }
        let per_attempt = t.elapsed().as_secs_f64() / attempts as f64;
        println!(
            "grind attempt mean over {attempts}: {:.4}ms  ({:.0} attempts/sec)  <-- t_attempt",
            per_attempt * 1e3,
            1.0 / per_attempt
        );

        // (b) One-time certificate cost (paid once per block, on the winning nonce).
        let t = std::time::Instant::now();
        let block = prove_canonical_moe_block_at(&CANONICAL_MATMUL_PARAMS, 8, 2, 1, commit, 0)
            .expect("prove");
        let prove_seconds = t.elapsed().as_secs_f64();
        let AiProofNode::Bytes(cert_bytes) = &block.certificate.certificate else {
            panic!("production compact certificate must use the canonical byte node");
        };
        println!(
            "canonical MoE prove: {prove_seconds:.3}s compact_cert_bytes={} trace_height={}  \
             <-- t_prove",
            cert_bytes.len(),
            block.certificate.trace_height
        );
    }

    #[test]
    fn inference_candidate_publication_and_invalidation_are_generation_bound() {
        let state = Arc::new(InferenceSchedulerState::default());
        let rpc = InferenceMiningRpc::new(
            state,
            build_gemma4_mining_job(1, [0; 76], [0; 32], PEARL_GATEWAY_CERTIFICATE_VERSION_V3)
                .expect("initial mining job"),
        )
        .expect("initial inference job");
        let commit = [0x5a; 32];
        let target = crate::easy_target_for(
            &crate::canonical::canonical_dense_mining_config(&crate::gemma4::GEMMA4_NATIVE_PARAMS)
                .expect("Gemma mining config"),
        );
        let candidate = InferenceNodeCandidate {
            generation: 2,
            inputs: NockchainCandidateInputs {
                target,
                nock_block_commitment: commit,
                pow_len: 77,
            },
            incomplete_header: canonical_dense_incomplete_header(commit, 0)
                .expect("candidate header"),
        };

        publish_inference_job(&rpc, candidate).expect("publish candidate");
        let published = rpc.mining_job().expect("published job");
        let expected = build_gemma4_mining_job(
            candidate.generation, candidate.incomplete_header, target,
            PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
        )
        .expect("expected mining job");
        assert_eq!(published, expected);

        invalidate_inference_job(&rpc, 3).expect("invalidate candidate");
        let invalidated = rpc.mining_job().expect("invalidated job");
        assert_eq!(invalidated.candidate_generation, 3);
        assert_eq!(invalidated.effective_target_le, vec![0; 32]);
    }

    #[test]
    fn canonical_grind_exits_when_cancelled() {
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(
            grind_canonical_block([0u8; 32], crate::easy_nock_target(), cancel)
                .expect("cancelled grind should exit cleanly")
                .is_none()
        );
    }

    fn inference_scalar_fixture(
        extranonce: u32,
    ) -> (
        InferenceNodeCandidate,
        OpenedDenseWitness,
        PearlMergeCheckedTicketAttempt,
    ) {
        let params = MatmulParams {
            m: 32,
            k: 1024,
            n: 32,
            noise_rank: 64,
            tile: INFERENCE_DENSE_TILE,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(b"nockchain-inference-proof-kat-v1", &params);
        let a = Arc::new(a);
        let b = Arc::new(b);
        let commit = [0x5a; 32];
        let template =
            PreparedCanonicalDenseTemplate::new(&params, commit, Arc::clone(&a), Arc::clone(&b))
                .expect("inference scalar template");
        let prepared = template
            .prepare_search(extranonce)
            .expect("search transcript");
        let target = crate::easy_target_for(&prepared.config());
        let ordinal = 3;
        let expected = template
            .checked_search_winner(&prepared, ordinal, &target)
            .expect("forced scalar winner");
        let (rows, columns) = prepared
            .offsets_at_ordinal(ordinal)
            .expect("opened tile offsets");
        let candidate = InferenceNodeCandidate {
            generation: 7,
            inputs: NockchainCandidateInputs {
                target,
                nock_block_commitment: commit,
                pow_len: 99,
            },
            incomplete_header: canonical_dense_incomplete_header(commit, 0)
                .expect("candidate header"),
        };
        let witness = OpenedDenseWitness {
            candidate_generation: candidate.generation,
            extranonce,
            work_id: 11,
            a_row_indices: (rows..rows + params.tile).collect(),
            b_column_indices: (columns..columns + params.tile).collect(),
            noise_seed_a: expected.commitments.s_a,
            noise_seed_b: expected.commitments.s_b,
            noise_rank: params.noise_rank,
            a_rows: params.m,
            b_columns: params.n,
            common_dim: params.k,
            a,
            b_transposed: b,
        };
        (candidate, witness, expected)
    }

    #[test]
    fn inference_winner_reconstruction_matches_scalar_transcript() {
        let (candidate, witness, expected) = inference_scalar_fixture(1);
        let (_, reconstructed) =
            reconstruct_inference_dense_attempt(candidate, witness).expect("scalar reconstruction");
        assert_eq!(reconstructed.commitments, expected.commitments);
        assert_eq!(
            reconstructed.ticket.jackpot_hash,
            expected.ticket.jackpot_hash
        );
        assert_eq!(
            reconstructed.aux.nock_block_commitment,
            candidate.inputs.nock_block_commitment
        );
    }

    #[test]
    fn inference_winner_reconstruction_rejects_stale_and_corrupt_witnesses() {
        let (candidate, mut stale, _) = inference_scalar_fixture(0);
        stale.candidate_generation += 1;
        assert!(reconstruct_inference_dense_attempt(candidate, stale)
            .err()
            .expect("stale witness must fail")
            .to_string()
            .contains("stale inference witness generation"));

        let (candidate, mut corrupt, _) = inference_scalar_fixture(0);
        corrupt.noise_seed_b[0] ^= 1;
        assert!(reconstruct_inference_dense_attempt(candidate, corrupt)
            .err()
            .expect("corrupt witness must fail")
            .to_string()
            .contains("noise seeds disagree"));
    }

    #[test]
    #[ignore = "builds and verifies a compact recursive inference-winner certificate"]
    fn inference_forced_winner_verifies_through_consensus_entrypoint() {
        let (candidate, witness, _) = inference_scalar_fixture(0);
        let (template, attempt) =
            reconstruct_inference_dense_attempt(candidate, witness).expect("scalar reconstruction");
        let block = template.prove(attempt).expect("compact recursive proof");
        let artifact = build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
            block.attempt.attempt(),
            &block.aux_inclusion,
            &block.a,
            &block.b,
            INFERENCE_DENSE_TILE as usize,
            &block.run,
        )
        .expect("inference artifact noun");
        let digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            block.run.verifier_key_digest(),
        );
        crate::certificate_noun::verify_ai_pow_block_artifact_jam(
            &artifact.jam(),
            CertificateNounLimits::default(),
            &candidate.inputs.nock_block_commitment,
            &candidate.inputs.target,
            INFERENCE_DENSE_TILE as usize,
            block.run.verifier_context(),
            &digest,
        )
        .expect("inference artifact verifies through consensus");
    }

    #[test]
    fn canonical_backend_winner_must_match_scalar_oracle_before_proving() {
        let commit = [0x5au8; 32];
        let mut corrupt = evaluate_canonical_moe_jackpot(
            &CANONICAL_MATMUL_PARAMS, CANONICAL_HW, CANONICAL_E, CANONICAL_TOP_K, commit, 0,
        )
        .expect("scalar canonical jackpot");
        corrupt[0] ^= 1;
        let backend = CorruptCanonicalBackend {
            jackpot_hash: corrupt,
        };

        let error = match grind_canonical_block_with_backend(
            commit,
            crate::easy_nock_target(),
            Arc::new(AtomicBool::new(false)),
            &backend,
        ) {
            Ok(_) => panic!("corrupt backend result must not reach the prover"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("disagrees with canonical scalar oracle"));
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn peak_backend_winner_must_match_scalar_recheck_before_proving() {
        let params = MatmulParams {
            m: 16,
            k: 1024,
            n: 16,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let template =
            PreparedCanonicalDenseTemplate::new(&params, [0x5a; 32], Arc::new(a), Arc::new(b))
                .expect("dense template");
        let prepared = template.prepare_search(0).expect("search transcript");
        let scalar_prepared = template.prepare(0).expect("scalar preparation");
        let scalar = scalar_prepared
            .evaluate(0, 0, &mut scalar_prepared.scratch())
            .expect("scalar ticket");
        let mut corrupt = scalar.jackpot_hash;
        corrupt[0] ^= 1;
        let mut target = [0xff; 32];
        target[30] = 0;
        target[31] = 0;

        let error = revalidate_peak_winner(
            &template,
            &prepared,
            SearchWinner {
                ordinal: 0,
                jackpot_hash: corrupt,
            },
            scalar_prepared.commitments(),
            &target,
        )
        .expect_err("corrupt peak result must not reach the prover");
        assert!(error
            .to_string()
            .contains("disagrees with scalar winner recheck"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canonical_reconnect_cleanup_awaits_in_flight_worker() {
        let cancel = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicU64::new(0));
        let cancel_for_worker = cancel.clone();
        let active_for_worker = active.clone();
        let mut worker = Some(tokio::task::spawn_blocking(move || {
            active_for_worker.fetch_add(1, Ordering::SeqCst);
            while !cancel_for_worker.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(100));
            active_for_worker.fetch_sub(1, Ordering::SeqCst);
            Ok(None)
        }));

        while active.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let started = std::time::Instant::now();
        cancel_and_await_canonical_worker(&mut worker, &cancel)
            .await
            .expect("cleanup should join worker");

        assert!(worker.is_none());
        assert!(cancel.load(Ordering::SeqCst));
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "cleanup returned before the worker finished"
        );
    }

    // Shared NockAppMetrics — gnort rejects double-registration.
    static METRICS: Lazy<Arc<nockapp::nockapp::metrics::NockAppMetrics>> = Lazy::new(|| {
        Arc::new(
            nockapp::nockapp::metrics::NockAppMetrics::register(gnort::global_metrics_registry())
                .expect("register NockAppMetrics"),
        )
    });

    struct MockNode {
        addr: SocketAddr,
        effect_tx: Arc<broadcast::Sender<NounSlab>>,
        pokes_observed: Arc<AtomicU64>,
        mined_pokes: Arc<TMutex<Vec<NounSlab>>>,
        set_key_pokes: Arc<TMutex<Vec<NounSlab>>>,
        server_task: tokio::task::JoinHandle<nockapp_grpc::error::Result<()>>,
        action_drainer: tokio::task::JoinHandle<()>,
    }

    impl MockNode {
        async fn spawn() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().expect("local_addr");
            drop(listener);
            let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
            let (effect_tx, _seed_rx) = broadcast::channel::<NounSlab>(64);
            let effect_tx = Arc::new(effect_tx);
            let effect_rx_for_handle = effect_tx.subscribe();
            let (exit, _exit_rx) = NockAppExit::new();
            let handle = NockAppHandle {
                io_sender: action_tx,
                effect_sender: effect_tx.clone(),
                effect_receiver: TMutex::new(effect_rx_for_handle),
                metrics: METRICS.clone(),
                exit,
            };
            let pokes_observed = Arc::new(AtomicU64::new(0));
            let mined_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));
            let set_key_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));
            let pokes_clone = pokes_observed.clone();
            let mined_clone = mined_pokes.clone();
            let set_key_clone = set_key_pokes.clone();
            let action_drainer = tokio::spawn(async move {
                while let Some(action) = action_rx.recv().await {
                    match action {
                        IOAction::Poke {
                            wire,
                            ack_channel,
                            poke,
                            ..
                        } => {
                            pokes_clone.fetch_add(1, Ordering::SeqCst);
                            if wire.source == <AiPowMinerWire as nockapp::wire::Wire>::SOURCE {
                                if wire.tags.iter().any(|t| match t {
                                    nockapp::wire::WireTag::String(s) => s == "mined",
                                    _ => false,
                                }) {
                                    mined_clone.lock().await.push(poke);
                                } else if wire.tags.iter().any(|t| match t {
                                    nockapp::wire::WireTag::String(s) => s == "setpubkey",
                                    _ => false,
                                }) {
                                    set_key_clone.lock().await.push(poke);
                                }
                            }
                            use nockapp::driver::PokeResult;
                            let _ = ack_channel.send(PokeResult::Ack);
                        }
                        IOAction::Peek { .. } => {}
                    }
                }
            });
            let server = PrivateNockAppGrpcServer::new(handle);
            let server_task = tokio::spawn(async move { server.serve(addr).await });
            tokio::time::sleep(Duration::from_millis(100)).await;
            MockNode {
                addr,
                effect_tx,
                pokes_observed,
                mined_pokes,
                set_key_pokes,
                server_task,
                action_drainer,
            }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }

        // Publish a synthetic %mine-ai effect matching the production
        // subscription and noun shape.
        fn publish_synth_mine_effect_with_target_limbs(
            &self,
            commitment_seed: u64,
            target_limbs: &[u64],
            pow_len: u64,
        ) {
            let mut slab = NounSlab::new();
            let head = D(tas!(b"mine-ai"));
            let version = D(AI_POW_MINE_CANDIDATE_VERSION);
            let commit_source = synth_block_commitment_slab(commitment_seed);
            let commit_space = commit_source.noun_space();
            let commit = slab.copy_into(unsafe { *commit_source.root() }, &commit_space);
            let mut target_list = D(0);
            for limb in target_limbs.iter().rev() {
                target_list = T(&mut slab, &[D(*limb), target_list]);
            }
            let target = T(&mut slab, &[D(tas!(b"bn")), target_list]);
            let plen = D(pow_len);
            let effect = T(&mut slab, &[head, version, commit, target, plen]);
            slab.set_root(effect);
            self.effect_tx.send(slab).expect("publish %mine-ai effect");
        }

        async fn shutdown(self) {
            self.server_task.abort();
            self.action_drainer.abort();
            let _ = self.server_task.await;
            let _ = self.action_drainer.await;
        }
    }

    fn pearl_test_pattern(length: u32) -> PearlPeriodicPattern {
        PearlPeriodicPattern {
            shape: [(1, length), (length, 1), (length, 1)],
        }
    }

    fn assert_set_key_poke_is_pkh_only(poke: &NounSlab) {
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));

        let verb_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("set key verb cell");
        assert!(verb_cell.head().eq_bytes("set-mining-key-advanced"));

        let lists_cell = verb_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("set key lists cell");
        assert_eq!(
            lists_cell
                .head()
                .as_atom()
                .expect("legacy key list atom")
                .as_u64()
                .expect("legacy key list atom fits u64"),
            0,
            "miner must send an empty legacy mining-key list"
        );
        lists_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("PKH config list must be nonempty");
    }

    async fn assert_node_received_pkh_only_set_key(node: &MockNode) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(poke) = node.set_key_pokes.lock().await.first() {
                assert_set_key_poke_is_pkh_only(poke);
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "miner did not submit set-mining-key poke within 2s"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn pearl_test_header() -> PearlIncompleteBlockHeader {
        PearlIncompleteBlockHeader {
            version: 0x0102_0304,
            prev_block: [0x11; 32],
            merkle_root: [0x22; 32],
            timestamp: 0x6677_8899,
            nbits: 0x1e7f_ffff,
        }
    }

    fn pearl_target_decimal_for_header(header: &PearlIncompleteBlockHeader) -> String {
        let mut value = pearl_nbits_to_target_le(header.nbits);
        if value.iter().all(|&byte| byte == 0) {
            return "0".to_string();
        }

        let mut digits = Vec::new();
        while value.iter().any(|&byte| byte != 0) {
            let mut rem = 0u16;
            for byte in value.iter_mut().rev() {
                let cur = (rem << 8) + u16::from(*byte);
                *byte = (cur / 10) as u8;
                rem = cur % 10;
            }
            digits.push((b'0' + rem as u8) as char);
        }
        digits.iter().rev().collect()
    }

    fn pearl_test_config() -> PearlMiningConfig {
        PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        }
    }

    fn pearl_test_params() -> MatmulParams {
        MatmulParams {
            m: 8,
            k: 1024,
            n: 8,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    fn pearl_test_aux() -> PearlNockchainAux {
        PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet".to_vec(),
            nock_block_commitment: [0x42; 32],
            nockchain_target_epoch_or_height: 123_456,
            extra_domain_data: b"ai-pow-target-window".to_vec(),
        }
    }

    fn pearl_tcp_gateway(
        port: u16,
        request_timeout: Duration,
        refresh_interval: Duration,
    ) -> PearlGatewayMinerRpcConfig {
        PearlGatewayMinerRpcConfig {
            transport: PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            request_timeout,
            refresh_interval,
        }
    }

    fn pearl_submission_cfg() -> PearlMergeSubmissionConfig {
        PearlMergeSubmissionConfig {
            gateway: PearlGatewayMinerRpcConfig {
                transport: PearlGatewayTransport::UnixSocket {
                    path: "/tmp/pearlgw.sock".to_string(),
                },
                request_timeout: Duration::from_secs(2),
                refresh_interval: Duration::from_secs(1),
            },
            mining_config: pearl_test_config(),
            aux_template: pearl_test_aux(),
            max_pattern_len: 16,
            mine_opts: PearlMergeMineOptions {
                max_attempts: Some(1),
                ..PearlMergeMineOptions::default()
            },
            certificate_builder: Arc::new(|attempt: &PearlMergeCheckedTicketAttempt| {
                let params = pearl_test_params();
                let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
                let parts = pearl_merge_recursive_certificate_parts_from_ticket(
                    attempt.attempt(),
                    &a,
                    &b,
                    16,
                )
                .map_err(|e| AiPowCertificateBuildError(e.to_string()))?;
                Ok(PearlMergeCertificateProof {
                    zk_params: parts.zk_params,
                    found_idx: parts.found_idx,
                    commitments: parts.commitments,
                    public_inputs: parts.public_inputs,
                    trace_height: parts.trace_height,
                    certificate: AiProofNode::Unit,
                })
            }),
        }
    }

    #[test]
    fn pearl_gateway_fetches_v3_tcp_mining_info() {
        let header = pearl_test_header();
        let header_bytes = header.to_bytes();
        let target = pearl_target_decimal_for_header(&header);
        let encoded_header = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(header_bytes)
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway fixture");
        let port = listener.local_addr().expect("gateway fixture addr").port();
        let gateway = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway client");
            let mut request_line = String::new();
            {
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                std::io::BufRead::read_line(&mut reader, &mut request_line)
                    .expect("read gateway request");
            }
            let request: serde_json::Value =
                serde_json::from_str(&request_line).expect("parse gateway request");
            assert_eq!(request["method"], "getMiningInfo");
            let response = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3}}}}\n",
                encoded_header, target
            );
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write gateway response");
        });

        let source = PearlGatewayMinerRpcConfig {
            transport: PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            request_timeout: Duration::from_secs(2),
            refresh_interval: Duration::from_secs(1),
        };
        let fetched = fetch_pearl_gateway_mining_job(&source, None)
            .expect("fetch Pearl gateway mining header")
            .header;
        gateway.join().expect("gateway fixture exited");

        assert_eq!(fetched, header);
    }

    #[test]
    fn pearl_gateway_rejects_non_v3_certificate_versions() {
        assert_eq!(
            parse_pearl_gateway_certificate_v3(Some(3))
                .expect("certificate V3 Gateway job must be accepted"),
            PearlCertificateV3
        );
        assert_eq!(std::mem::size_of::<PearlCertificateV3>(), 0);
        for actual in [1, 2, 4] {
            assert!(matches!(
                parse_pearl_gateway_certificate_v3(Some(actual)),
                Err(PearlGatewayError::UnsupportedCertificateVersion {
                    expected: 3,
                    actual: rejected,
                }) if rejected == actual
            ));
        }
        assert!(matches!(
            parse_pearl_gateway_certificate_v3(None),
            Err(PearlGatewayError::MissingCertificateVersion)
        ));
    }

    #[test]
    fn pearl_gateway_submit_plain_proof_sends_gateway_wire_format() {
        let header = pearl_test_header();
        let proof_base64 = "AQIDBA==";
        let target = serde_json::Value::from(123_456u64);
        let expected_header = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway fixture");
        let port = listener.local_addr().expect("gateway fixture addr").port();
        let gateway = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway client");
            let mut request_line = String::new();
            {
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                std::io::BufRead::read_line(&mut reader, &mut request_line)
                    .expect("read gateway request");
            }
            let request: serde_json::Value =
                serde_json::from_str(&request_line).expect("parse gateway request");
            assert_eq!(request["jsonrpc"], "2.0");
            assert_eq!(request["method"], "submitPlainProof");
            assert_eq!(request["id"], 2);
            assert_eq!(request["params"]["plain_proof"], proof_base64);
            assert_eq!(
                request["params"]["mining_job"]["incomplete_header_bytes"],
                expected_header
            );
            assert_eq!(request["params"]["mining_job"]["target"], 123_456);
            assert_eq!(
                request["params"]["mining_job"]["cert_version"],
                PEARL_GATEWAY_CERTIFICATE_VERSION_V3
            );
            let response = "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"submitted\"}\n";
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write gateway response");
        });

        let cfg = PearlGatewayMinerRpcConfig {
            transport: PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            request_timeout: Duration::from_secs(2),
            refresh_interval: Duration::from_secs(1),
        };
        let job = PearlGatewayResolvedMiningJob {
            header,
            target,
            certificate: PearlCertificateV3,
            aux_inclusion: None,
        };
        submit_pearl_gateway_plain_proof(&cfg, proof_base64, &header, &job)
            .expect("submit Pearl plain proof");
        gateway.join().expect("gateway fixture exited");
    }

    #[test]
    fn pearl_gateway_times_out_silent_tcp_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent gateway fixture");
        let port = listener.local_addr().expect("silent gateway addr").port();
        let gateway = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept silent gateway client");
            std::thread::sleep(Duration::from_millis(250));
        });
        let source = PearlGatewayMinerRpcConfig {
            transport: PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            request_timeout: Duration::from_millis(50),
            refresh_interval: Duration::from_secs(1),
        };

        let started = std::time::Instant::now();
        let err = fetch_pearl_gateway_mining_job(&source, None)
            .expect_err("silent Pearl gateway must not block indefinitely");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout took too long: {:?}",
            started.elapsed()
        );
        assert!(
            matches!(err, PearlGatewayError::Io(_)),
            "unexpected error: {err}"
        );
        gateway.join().expect("silent gateway fixture exited");
    }

    #[test]
    fn pearl_gateway_response_reader_rejects_oversized_line() {
        let exact = vec![b' '; PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES];
        let mut exact_reader = std::io::Cursor::new(exact.clone());
        assert_eq!(
            read_bounded_gateway_response_line(&mut exact_reader).expect("exact cap is accepted"),
            String::from_utf8(exact).expect("ascii")
        );

        let oversized = vec![b' '; PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES + 1];
        let mut oversized_reader = std::io::Cursor::new(oversized);
        assert!(matches!(
            read_bounded_gateway_response_line(&mut oversized_reader),
            Err(PearlGatewayError::ResponseTooLarge {
                limit: PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES
            })
        ));
    }

    #[test]
    fn pearl_gateway_target_rejects_uint257_decimal_number() {
        let target: serde_json::Value = serde_json::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        )
        .expect("parse uint257 target");

        assert!(matches!(
            validate_pearl_gateway_target_uint256(&target),
            Err(PearlGatewayError::TargetOverflow)
        ));
    }

    #[test]
    fn pearl_gateway_target_rejects_non_integer_json_values() {
        for target in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(1e6),
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!("123"),
            serde_json::json!([123]),
            serde_json::json!({"target": 123}),
        ] {
            assert!(
                matches!(
                    validate_pearl_gateway_target_uint256(&target),
                    Err(PearlGatewayError::TargetOverflow)
                ),
                "target must reject non-uint256 integer JSON value: {target}"
            );
        }
    }

    #[test]
    fn pearl_gateway_target_must_match_header_nbits() {
        let header = pearl_test_header();
        let matching_target: serde_json::Value =
            serde_json::from_str(&pearl_target_decimal_for_header(&header))
                .expect("parse matching Pearl target");
        validate_pearl_gateway_target_matches_header_nbits(&matching_target, header.nbits)
            .expect("matching Gateway target and header nbits should be accepted");

        let mismatched_target = serde_json::Value::from(0u64);
        assert!(matches!(
            validate_pearl_gateway_target_matches_header_nbits(&mismatched_target, header.nbits),
            Err(PearlGatewayError::TargetNbitsMismatch)
        ));
    }

    #[test]
    fn pearl_gateway_aux_inclusion_decoder_rejects_oversized_coinbase() {
        let oversized = vec![0u8; PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES + 1];
        let encoded = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(oversized)
        };
        let err = decode_pearl_gateway_aux_inclusion(PearlGatewayAuxInclusion {
            coinbase_tx: encoded,
            merkle_branch: Vec::new(),
        })
        .expect_err("Gateway aux inclusion must reject oversized coinbase payload");

        assert!(matches!(
            err,
            PearlGatewayError::AuxInclusionCoinbaseTooLarge {
                actual,
                limit: PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES
            } if actual == PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES + 1
        ));
    }

    #[test]
    fn pearl_gateway_aux_inclusion_decoder_rejects_merkle_branch() {
        let encoded_coinbase = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode([0u8; 1])
        };
        let encoded_digest = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode([0u8; 32])
        };
        let err = decode_pearl_gateway_aux_inclusion(PearlGatewayAuxInclusion {
            coinbase_tx: encoded_coinbase,
            merkle_branch: vec![encoded_digest],
        })
        .expect_err("production Gateway aux inclusion must reject merkle branches");

        assert!(matches!(
            err,
            PearlGatewayError::AuxInclusionMerkleBranchTooDeep {
                actual: 1,
                limit: PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH
            }
        ));
    }

    #[test]
    fn pearl_gateway_rejects_string_target() {
        let header = pearl_test_header();
        let encoded_header = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway fixture");
        let port = listener.local_addr().expect("gateway fixture addr").port();
        let gateway = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway client");
            let mut request_line = String::new();
            {
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                std::io::BufRead::read_line(&mut reader, &mut request_line)
                    .expect("read gateway request");
            }
            let request: serde_json::Value =
                serde_json::from_str(&request_line).expect("parse gateway request");
            assert_eq!(request["method"], "getMiningInfo");
            let response = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":\"123456\",\"cert_version\":3}}}}\n",
                encoded_header
            );
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write gateway response");
        });

        let source = PearlGatewayMinerRpcConfig {
            transport: PearlGatewayTransport::Tcp {
                host: "127.0.0.1".to_string(),
                port,
            },
            request_timeout: Duration::from_secs(2),
            refresh_interval: Duration::from_secs(1),
        };
        let err = fetch_pearl_gateway_mining_job(&source, None)
            .expect_err("Pearl Gateway string target must be rejected");
        gateway.join().expect("gateway fixture exited");

        assert!(
            matches!(err, PearlGatewayError::TargetOverflow),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn pearl_gateway_submission_rejects_header_mismatch_before_rpc() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        {
            let pearl_cfg = &mut cfg.puzzle.pearl_merge;
            pearl_cfg.gateway =
                pearl_tcp_gateway(9, Duration::from_millis(1), Duration::from_secs(1));
        }
        let pearl_cfg = cfg.puzzle.pearl_merge.clone();

        let mut aux = pearl_test_aux();
        aux.nock_block_commitment = [0xaa; 32];
        let header_template = pearl_test_header();
        let (mined_header, aux_inclusion) =
            pearl_test_aux_inclusion(&aux.commitment().expect("aux commitment"));
        assert_ne!(
            header_template, mined_header,
            "fixture must model Gateway issuing a header without Nockchain aux"
        );

        let params = cfg.puzzle.params;
        let attempt = evaluate_pearl_merge_checked_ticket_attempt(
            &mined_header,
            &pearl_cfg.mining_config,
            &params,
            0,
            0,
            &cfg.puzzle.a,
            &cfg.puzzle.b,
            &crate::easy_nock_target(),
            pearl_cfg.max_pattern_len,
            aux,
        )
        .expect("evaluate Pearl-compatible attempt");
        let mined = PearlMergeMinedSubmission {
            ticket: PearlMergeMinedTicket {
                attempt,
                pearl_target_hit: true,
                nockchain_target_hit: false,
                stats: crate::MiningStats::default(),
            },
            gateway_mining_job: PearlGatewayResolvedMiningJob {
                header: header_template,
                target: serde_json::Value::from(123_456u64),
                certificate: PearlCertificateV3,
                aux_inclusion: None,
            },
            aux_inclusion,
        };

        let err = submit_pearl_solution_to_gateway(&cfg, &pearl_cfg, &mined)
            .expect_err("header mismatch must fail before Gateway RPC");
        assert!(
            err.contains("does not match the aux-bearing mined header"),
            "unexpected error: {err}"
        );
    }

    fn pearl_test_coinbase_tx(aux_commitment: &[u8; 32]) -> Vec<u8> {
        let mut script = Vec::from([0x01, 0x00]);
        script.extend_from_slice(ai_pow::pearl_compat::PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
        script.extend_from_slice(aux_commitment);
        let mut tx = Vec::new();
        tx.extend_from_slice(&1u32.to_le_bytes());
        tx.push(1);
        tx.extend_from_slice(&[0u8; 32]);
        tx.extend_from_slice(&u32::MAX.to_le_bytes());
        tx.push(script.len() as u8);
        tx.extend_from_slice(&script);
        tx.extend_from_slice(&u32::MAX.to_le_bytes());
        tx.push(1);
        tx.extend_from_slice(&0u64.to_le_bytes());
        tx.push(1);
        tx.push(0x51);
        tx.extend_from_slice(&0u32.to_le_bytes());
        tx
    }

    fn gateway_aux_header_and_coinbase_from_request(
        request: &serde_json::Value,
        mut header: PearlIncompleteBlockHeader,
    ) -> (PearlIncompleteBlockHeader, String) {
        assert_eq!(request["params"]["return_aux_inclusion"], true);
        let encoded_flags = request["params"]["coinbase_aux_flags"]
            .as_str()
            .expect("coinbase_aux_flags string");
        let flags = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(encoded_flags)
                .expect("decode coinbase_aux_flags")
        };
        assert_eq!(
            &flags[..PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG.len()],
            PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG
        );
        let aux_commitment: [u8; 32] = flags[PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG.len()..]
            .try_into()
            .expect("aux commitment length");
        let coinbase_tx = pearl_test_coinbase_tx(&aux_commitment);
        let mut merkle_root = ai_pow::pearl_compat::pearl_bitcoin_double_sha256_raw(&coinbase_tx);
        merkle_root.reverse();
        header.merkle_root = merkle_root;
        let encoded_coinbase = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(coinbase_tx)
        };
        (header, encoded_coinbase)
    }

    struct TestPearlGateway {
        config: PearlGatewayMinerRpcConfig,
        stop: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<()>,
    }

    impl TestPearlGateway {
        fn shutdown(self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.join().expect("gateway fixture exited");
        }
    }

    fn spawn_static_aux_pearl_gateway(
        header_template: PearlIncompleteBlockHeader,
        request_timeout: Duration,
        refresh_interval: Duration,
    ) -> TestPearlGateway {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let port = listener.local_addr().expect("gateway addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stop_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                match request["method"].as_str().expect("method string") {
                    "getMiningInfo" => {
                        let (header, encoded_coinbase) =
                            gateway_aux_header_and_coinbase_from_request(&request, header_template);
                        let encoded_header = {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                        };
                        let target = pearl_target_decimal_for_header(&header);
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                            encoded_header, target, encoded_coinbase
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway response");
                    }
                    "submitPlainProof" => {
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                            request["id"]
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway submit response");
                    }
                    other => panic!("unexpected Gateway method: {other}"),
                }
            }
        });
        TestPearlGateway {
            config: pearl_tcp_gateway(port, request_timeout, refresh_interval),
            stop,
            thread,
        }
    }

    fn pearl_test_aux_inclusion(
        aux_commitment: &[u8; 32],
    ) -> (PearlIncompleteBlockHeader, PearlAuxInclusionProof) {
        let coinbase_tx = pearl_test_coinbase_tx(aux_commitment);
        let mut merkle_root = ai_pow::pearl_compat::pearl_bitcoin_double_sha256_raw(&coinbase_tx);
        merkle_root.reverse();
        let mut header = pearl_test_header();
        header.merkle_root = merkle_root;
        (
            header,
            PearlAuxInclusionProof {
                coinbase_tx,
                merkle_branch: Vec::new(),
            },
        )
    }

    fn test_cfg(node_addr: String) -> MinerConfig {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
        let puzzle = AiPuzzleInputs {
            params,
            a: Arc::new(a),
            b: Arc::new(b),
            pearl_merge: pearl_submission_cfg(),
        };
        MinerConfig {
            node_addr,
            mining_pkh_configs: vec![MiningPkhConfig {
                share: 1,
                pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
            }],
            puzzle,
            mining_threads: 1,
            reconnect_backoff_initial: Duration::from_millis(50),
            reconnect_backoff_max: Duration::from_millis(200),
            reconnect_max_attempts: 3,
        }
    }

    /// Largest synthetic candidate target whose factor-adjusted product
    /// fits the 256-bit band for the fixture factor (2^16): 2^240 − 1 as
    /// little-endian u32 limbs (every limb must stay a direct atom).
    /// `[u32::MAX; 8]` (~2^256) is over-band and rejected by the
    /// fail-closed adjusted-target multiply.
    fn fitting_target_limbs() -> Vec<u64> {
        let mut limbs = vec![u64::from(u32::MAX); 7];
        limbs.push(0xffff);
        limbs
    }

    fn inference_fitting_target_limbs() -> Vec<u64> {
        let config =
            crate::canonical::canonical_dense_mining_config(&crate::gemma4::GEMMA4_NATIVE_PARAMS)
                .expect("Gemma mining config");
        crate::easy_target_for(&config)
            .chunks_exact(4)
            .map(|chunk| {
                u64::from(u32::from_le_bytes(
                    chunk.try_into().expect("fixed target limb"),
                ))
            })
            .collect()
    }

    fn bignum_target_slab(limbs: &[u64]) -> NounSlab {
        let mut slab = NounSlab::new();
        let mut list = D(0);
        for limb in limbs.iter().rev() {
            list = T(&mut slab, &[D(*limb), list]);
        }
        let target = T(&mut slab, &[D(tas!(b"bn")), list]);
        slab.set_root(target);
        slab
    }

    fn synth_block_commitment_slab(commitment_seed: u64) -> NounSlab {
        let mut slab = NounSlab::new();
        let commit = T(
            &mut slab,
            &[
                D(commitment_seed),
                D(commitment_seed + 1),
                D(commitment_seed + 2),
                D(commitment_seed + 3),
                D(commitment_seed + 4),
            ],
        );
        slab.set_root(commit);
        slab
    }

    fn candidate_for_target_and_commitment(
        target: NounSlab,
        commitment_seed: u64,
    ) -> MiningCandidate {
        let mut version = NounSlab::new();
        version.set_root(D(AI_POW_MINE_CANDIDATE_VERSION));
        let block_header = synth_block_commitment_slab(commitment_seed);
        MiningCandidate {
            kind: MiningCandidateKind::Ai,
            version,
            block_header,
            target,
            pow_len: 64,
        }
    }

    fn candidate_for_target(target: NounSlab) -> MiningCandidate {
        candidate_for_target_and_commitment(target, 0xCAFE)
    }

    fn candidate_with_version(
        version: NounSlab,
        target: NounSlab,
        commitment_seed: u64,
    ) -> MiningCandidate {
        MiningCandidate {
            kind: MiningCandidateKind::Ai,
            version,
            block_header: synth_block_commitment_slab(commitment_seed),
            target,
            pow_len: 64,
        }
    }

    fn expected_aux_commitment_bridge(candidate: &MiningCandidate) -> [u8; 32] {
        *blake3::hash(&candidate.block_header.jam()).as_bytes()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inference_node_tracks_real_candidate_replacement() {
        let node = MockNode::spawn().await;
        let state = Arc::new(InferenceSchedulerState::default());
        let initial_header = canonical_dense_incomplete_header([0; 32], 0).expect("initial header");
        let initial_job = build_gemma4_mining_job(
            1, initial_header, [0; 32], PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
        )
        .expect("initial mining job");
        let rpc = InferenceMiningRpc::new(state, initial_job).expect("initial inference job");
        let (proof_sender, proof_requests) = inference_proof_channel();
        let shutdown = CancellationToken::new();
        let runner_shutdown = shutdown.clone();
        let runner_rpc = rpc.clone();
        let node_url = node.url();
        let runner = tokio::spawn(async move {
            run_inference_node(
                InferenceNodeConfig {
                    node_addr: node_url,
                    mining_pkh_configs: vec![MiningPkhConfig {
                        share: 1,
                        pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
                    }],
                },
                runner_rpc,
                proof_requests,
                runner_shutdown,
            )
            .await
        });
        let _proof_sender = proof_sender;

        assert_node_received_pkh_only_set_key(&node).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        node.publish_synth_mine_effect_with_target_limbs(
            800,
            &inference_fitting_target_limbs(),
            64,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let first = loop {
            let job = rpc.mining_job().expect("active inference job");
            if job.candidate_generation > 1 {
                break job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "inference bridge did not publish the first candidate"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_ne!(first.effective_target_le, vec![0; 32]);

        node.publish_synth_mine_effect_with_target_limbs(
            801,
            &inference_fitting_target_limbs(),
            64,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let second = loop {
            let job = rpc.mining_job().expect("replacement inference job");
            if job.candidate_generation > first.candidate_generation {
                break job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "inference bridge did not replace the candidate"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_ne!(second.incomplete_header, first.incomplete_header);

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), runner)
            .await
            .expect("inference node shutdown timeout")
            .expect("inference node task")
            .expect("inference node result");
        node.shutdown().await;
    }

    #[cfg(feature = "gpu")]
    fn opened_tensor_parts(
        tensor: nockapp_grpc_proto::pb::ai_pow::v1::OpenedTensor,
        values: &[i8],
    ) -> Vec<nockapp_grpc_proto::pb::ai_pow::v1::OpenedBlockPart> {
        use nockapp_grpc_proto::pb::ai_pow::v1::{
            opened_block_part, OpenedBlockPart, OpenedTensorChunk,
        };

        // SAFETY: i8 and u8 have identical layout. The borrowed byte view does
        // not outlive `values` and is copied into owned protobuf chunks.
        let bytes =
            unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len()) };
        bytes
            .chunks(1024 * 1024)
            .enumerate()
            .map(|(index, chunk)| OpenedBlockPart {
                part: Some(opened_block_part::Part::TensorChunk(OpenedTensorChunk {
                    tensor: tensor as i32,
                    offset: (index * 1024 * 1024) as u64,
                    data: chunk.to_vec(),
                })),
            })
            .collect()
    }

    #[cfg(feature = "gpu")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "runs the full native GPU winner, recursive proof, and node submission path"]
    async fn native_inference_winner_submits_through_bridge() {
        use nockapp_grpc_proto::pb::ai_pow::v1::inference_mining_service_client::InferenceMiningServiceClient;
        use nockapp_grpc_proto::pb::ai_pow::v1::{
            opened_block_part, GetMiningJobRequest, OpenedBlockMetadata, OpenedBlockPart,
            OpenedTensor, RegisterRuntimeRequest,
        };

        let node = MockNode::spawn().await;
        let state = Arc::new(InferenceSchedulerState::default());
        let initial_header = canonical_dense_incomplete_header([0; 32], 0).expect("initial header");
        let initial_job = build_gemma4_mining_job(
            1, initial_header, [0; 32], PEARL_GATEWAY_CERTIFICATE_VERSION_V3,
        )
        .expect("initial mining job");
        let (proof_sender, proof_requests) = inference_proof_channel();
        let rpc = InferenceMiningRpc::new(state, initial_job)
            .expect("initial inference job")
            .with_proof_sender(proof_sender);

        let node_shutdown = CancellationToken::new();
        let node_runner = tokio::spawn(run_inference_node(
            InferenceNodeConfig {
                node_addr: node.url(),
                mining_pkh_configs: vec![MiningPkhConfig {
                    share: 1,
                    pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
                }],
            },
            rpc.clone(),
            proof_requests,
            node_shutdown.clone(),
        ));
        assert_node_received_pkh_only_set_key(&node).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut target_limbs = vec![u64::from(u32::MAX); 7];
        target_limbs.push(0xff);
        node.publish_synth_mine_effect_with_target_limbs(900, &target_limbs, 64);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while rpc.candidate_generation() == 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "inference bridge did not publish the GPU candidate"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind inference bridge");
        let bridge_addr = listener.local_addr().expect("inference bridge address");
        drop(listener);
        let bridge_shutdown = CancellationToken::new();
        let bridge_server_shutdown = bridge_shutdown.clone();
        let bridge_server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(rpc.clone().into_server())
                .serve_with_shutdown(bridge_addr, async move {
                    bridge_server_shutdown.cancelled().await;
                }),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut client = InferenceMiningServiceClient::connect(format!("http://{bridge_addr}"))
            .await
            .expect("connect inference bridge");
        let registered = client
            .register_runtime(RegisterRuntimeRequest {
                protocol_version: 2,
                checkpoint_layout_digest: vec![0x33; 32],
                cuda_device_uuid: vec![0x44; 16],
                process_id: std::process::id(),
            })
            .await
            .expect("register native runtime")
            .into_inner();
        let mining_job = client
            .get_mining_job(GetMiningJobRequest {
                runtime_id: registered.runtime_id.clone(),
            })
            .await
            .expect("get node mining job")
            .into_inner();

        let params = crate::gemma4::GEMMA4_NATIVE_PARAMS;
        let (a, b) =
            ai_pow::synth::synth_matrices(b"nockchain-native-proof-bridge-kat-v1", &params);
        let a = Arc::new(a);
        let b = Arc::new(b);
        let target: [u8; 32] = mining_job
            .effective_target_le
            .as_slice()
            .try_into()
            .expect("fixed effective target");
        let mut session = crate::gemma4_cuda::Gemma4CudaSession::new_source(
            0, params.m as usize, params.n as usize, &a, &b,
        )
        .expect("native Gemma session");
        let total_tickets = session.total_tickets();
        let base_header = PearlIncompleteBlockHeader::from_bytes(&mining_job.incomplete_header)
            .expect("candidate header");
        let mut winner = None;
        for extranonce in 0..32 {
            let mut header = base_header;
            header.timestamp = header.timestamp.wrapping_add(extranonce);
            let preparation = session
                .prepare(&header.to_bytes(), &mining_job.mining_config)
                .expect("prepare native transcript");
            let result = session
                .search(0, total_tickets, &target)
                .expect("native target search");
            if let Some(ordinal) = result.winner {
                winner = Some((extranonce, ordinal, preparation));
                break;
            }
        }
        let (extranonce, ordinal, preparation) =
            winner.expect("deterministic native fixture must find a consensus-target winner");
        let col_tickets = u64::from(params.n / params.tile);
        let row_start = u32::try_from(ordinal / col_tickets)
            .expect("winner row")
            .checked_mul(params.tile)
            .expect("winner row offset");
        let column_start = u32::try_from(ordinal % col_tickets)
            .expect("winner column")
            .checked_mul(params.tile)
            .expect("winner column offset");

        let metadata = OpenedBlockMetadata {
            runtime_id: registered.runtime_id,
            candidate_generation: mining_job.candidate_generation,
            work_id: 0,
            a_row_indices: (row_start..row_start + params.tile).collect(),
            b_column_indices: (column_start..column_start + params.tile).collect(),
            noise_seed_a: preparation.commitments.s_a.to_vec(),
            noise_seed_b: preparation.commitments.s_b.to_vec(),
            noise_rank: params.noise_rank,
            a_rows: params.m,
            b_columns: params.n,
            common_dim: params.k,
            extranonce,
        };
        let mut parts = vec![OpenedBlockPart {
            part: Some(opened_block_part::Part::Metadata(metadata)),
        }];
        parts.extend(opened_tensor_parts(OpenedTensor::A, &a));
        parts.extend(opened_tensor_parts(OpenedTensor::BTransposed, &b));
        let response = client
            .submit_opened_block(tokio_stream::iter(parts))
            .await
            .expect("submit native opened witness")
            .into_inner();
        assert!(response.accepted, "{}", response.detail);
        assert_eq!(node.mined_pokes.lock().await.len(), 1);

        bridge_shutdown.cancel();
        node_shutdown.cancel();
        bridge_server
            .await
            .expect("inference bridge server task")
            .expect("inference bridge server");
        node_runner
            .await
            .expect("inference node task")
            .expect("inference node result");
        node.shutdown().await;
    }

    #[test]
    fn derive_job_inputs_decodes_bignum_target_little_endian() {
        let candidate =
            candidate_for_target(bignum_target_slab(&[0x0403_0201, 0x0807_0605, 0x0c0b_0a09]));

        let (target, _) = derive_job_inputs(&candidate).expect("derive job inputs");

        assert_eq!(&target[0..12], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert!(target[12..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn derive_pearl_merge_job_inputs_binds_aux_to_candidate_block_commitment() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_secs(2),
            Duration::from_secs(1),
        );
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway = gateway.config.clone();
        let candidate_a =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xCAFE);
        let candidate_b =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xCAFF);

        let job_a = derive_pearl_merge_job_inputs(&cfg, &candidate_a).expect("derive Pearl job A");
        let job_b = derive_pearl_merge_job_inputs(&cfg, &candidate_b).expect("derive Pearl job B");

        assert_eq!(
            job_a.aux.nock_block_commitment,
            expected_aux_commitment_bridge(&candidate_a)
        );
        assert_eq!(
            job_b.aux.nock_block_commitment,
            expected_aux_commitment_bridge(&candidate_b)
        );
        assert_ne!(
            job_a.aux.nock_block_commitment, job_b.aux.nock_block_commitment,
            "distinct kernel block commitments must produce distinct AIP1 aux bindings"
        );
        assert_ne!(
            job_a.aux.nock_block_commitment,
            pearl_test_aux().nock_block_commitment,
            "candidate commitment must replace the static aux template placeholder"
        );
        gateway.shutdown();
    }

    #[test]
    fn derive_pearl_merge_job_inputs_builds_self_verifying_aux_inclusion() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_secs(2),
            Duration::from_secs(1),
        );
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway = gateway.config.clone();
        let candidate =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xD00D);

        let job = derive_pearl_merge_job_inputs(&cfg, &candidate).expect("derive Pearl job");
        let expected_aux_commitment = job.aux.commitment().expect("aux commitment");
        verify_pearl_aux_inclusion(&job.header, &expected_aux_commitment, &job.aux_inclusion)
            .expect("derived coinbase-only Pearl aux inclusion should verify");

        let mut stale_aux = job.aux.clone();
        stale_aux.nock_block_commitment = [0x99; 32];
        let stale_aux_commitment = stale_aux.commitment().expect("stale aux commitment");
        assert!(
            verify_pearl_aux_inclusion(&job.header, &stale_aux_commitment, &job.aux_inclusion)
                .is_err(),
            "aux inclusion must bind the candidate-derived Nockchain block commitment"
        );
        gateway.shutdown();
    }

    #[test]
    fn derive_pearl_merge_job_inputs_uses_gateway_returned_aux_inclusion() {
        let candidate =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xD0A1);
        let mut aux = pearl_test_aux();
        aux.nock_block_commitment = expected_aux_commitment_bridge(&candidate);
        let aux_commitment = aux.commitment().expect("aux commitment");
        let coinbase_tx = pearl_test_coinbase_tx(&aux_commitment);
        let mut merkle_root = ai_pow::pearl_compat::pearl_bitcoin_double_sha256_raw(&coinbase_tx);
        merkle_root.reverse();
        let mut gateway_header = pearl_test_header();
        gateway_header.merkle_root = merkle_root;
        let gateway_target = pearl_target_decimal_for_header(&gateway_header);

        let encoded_header = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(gateway_header.to_bytes())
        };
        let encoded_coinbase = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&coinbase_tx)
        };
        let expected_coinbase_aux_flags = {
            let mut flags =
                Vec::with_capacity(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG.len() + aux_commitment.len());
            flags.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
            flags.extend_from_slice(&aux_commitment);
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(flags)
        };

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway fixture");
        let port = listener.local_addr().expect("gateway fixture addr").port();
        let gateway = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway client");
            let mut request_line = String::new();
            {
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                std::io::BufRead::read_line(&mut reader, &mut request_line)
                    .expect("read gateway request");
            }
            let request: serde_json::Value =
                serde_json::from_str(&request_line).expect("parse gateway request");
            assert_eq!(request["method"], "getMiningInfo");
            assert_eq!(
                request["params"]["coinbase_aux_flags"],
                expected_coinbase_aux_flags
            );
            assert_eq!(request["params"]["return_aux_inclusion"], true);
            let response = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                encoded_header, gateway_target, encoded_coinbase
            );
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write gateway response");
        });

        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway =
            pearl_tcp_gateway(port, Duration::from_secs(2), Duration::from_secs(1));

        let job = derive_pearl_merge_job_inputs(&cfg, &candidate)
            .expect("derive Gateway aux-bearing Pearl job");
        gateway.join().expect("gateway fixture exited");

        assert_eq!(job.header, gateway_header);
        assert_eq!(job.gateway_mining_job.header, gateway_header);
        verify_pearl_aux_inclusion(&job.header, &aux_commitment, &job.aux_inclusion)
            .expect("Gateway-returned aux inclusion should verify");
        assert_eq!(job.aux_inclusion.coinbase_tx, coinbase_tx);
    }

    #[test]
    fn derive_pearl_merge_job_inputs_rejects_gateway_missing_aux_inclusion() {
        let candidate =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xD0A2);
        let header = pearl_test_header();
        let target = pearl_target_decimal_for_header(&header);
        let encoded_header = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
        };
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway fixture");
        let port = listener.local_addr().expect("gateway fixture addr").port();
        let gateway = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway client");
            let mut request_line = String::new();
            {
                let mut reader =
                    std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                std::io::BufRead::read_line(&mut reader, &mut request_line)
                    .expect("read gateway request");
            }
            let request: serde_json::Value =
                serde_json::from_str(&request_line).expect("parse gateway request");
            assert_eq!(request["method"], "getMiningInfo");
            assert_eq!(request["params"]["return_aux_inclusion"], true);
            let response = format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3}}}}\n",
                encoded_header, target
            );
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write gateway response");
        });

        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway =
            pearl_tcp_gateway(port, Duration::from_secs(2), Duration::from_secs(1));

        let err = match derive_pearl_merge_job_inputs(&cfg, &candidate) {
            Ok(_) => panic!("Gateway response without aux_inclusion must be rejected"),
            Err(err) => err,
        };
        gateway.join().expect("gateway fixture exited");

        assert!(
            err.contains("did not include requested aux_inclusion"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn derive_pearl_merge_job_inputs_rejects_non_ai_candidate_version() {
        let cfg = test_cfg("http://127.0.0.1:1".to_string());
        let mut version = NounSlab::new();
        version.set_root(D(0));
        let candidate =
            candidate_with_version(version, bignum_target_slab(&[u64::from(u32::MAX)]), 0xA100);

        let err = match derive_pearl_merge_job_inputs(&cfg, &candidate) {
            Ok(_) => panic!("AI miner must reject non-%4 mine-ai candidates"),
            Err(err) => err,
        };
        assert!(err.contains("%4"), "unexpected error: {err}");
    }

    #[test]
    fn derive_pearl_merge_job_inputs_rejects_wrong_candidate_kind() {
        let cfg = test_cfg("http://127.0.0.1:1".to_string());
        let mut candidate =
            candidate_for_target_and_commitment(bignum_target_slab(&[u64::from(u32::MAX)]), 0xA0FF);
        candidate.kind = MiningCandidateKind::Zk;

        let err = match derive_pearl_merge_job_inputs(&cfg, &candidate) {
            Ok(_) => panic!("AI miner must reject non-%mine-ai candidates"),
            Err(err) => err,
        };
        assert!(err.contains("%mine-ai"), "unexpected error: {err}");
    }

    #[test]
    fn derive_pearl_merge_job_inputs_rejects_malformed_candidate_version() {
        let cfg = test_cfg("http://127.0.0.1:1".to_string());
        let mut version = NounSlab::new();
        let pair = T(&mut version, &[D(3), D(0)]);
        version.set_root(pair);
        let candidate =
            candidate_with_version(version, bignum_target_slab(&[u64::from(u32::MAX)]), 0xA101);

        let err = match derive_pearl_merge_job_inputs(&cfg, &candidate) {
            Ok(_) => panic!("AI miner must reject non-atom mine-ai candidate versions"),
            Err(err) => err,
        };
        assert!(err.contains("version"), "unexpected error: {err}");
    }

    #[test]
    fn derive_job_inputs_saturates_targets_above_u256() {
        let exact_u256_max = candidate_for_target(bignum_target_slab(&[u64::from(u32::MAX); 8]));
        let (target, _) = derive_job_inputs(&exact_u256_max).expect("derive max u256 target");
        assert_eq!(target, [0xFF; 32]);

        let mut first_overflowing_limb = vec![0u64; 9];
        first_overflowing_limb[8] = 1;
        let candidate = candidate_for_target(bignum_target_slab(&first_overflowing_limb));
        let (target, _) = derive_job_inputs(&candidate).expect("derive job inputs");
        assert_eq!(target, [0xFF; 32]);

        let mut later_overflowing_limb = vec![0u64; 10];
        later_overflowing_limb[9] = 0x8;
        let candidate = candidate_for_target(bignum_target_slab(&later_overflowing_limb));
        let (target, _) = derive_job_inputs(&candidate).expect("derive job inputs");
        assert_eq!(target, [0xFF; 32]);
    }

    #[test]
    fn derive_job_inputs_rejects_malformed_target_nouns() {
        let mut atom_target = NounSlab::new();
        atom_target.set_root(D(0xFFFF));
        let err = derive_job_inputs(&candidate_for_target(atom_target))
            .expect_err("target atom is not a bignum");
        assert!(err.contains("bignum cell"), "unexpected error: {err}");

        let mut wrong_tag_target = NounSlab::new();
        let limbs = T(&mut wrong_tag_target, &[D(1), D(0)]);
        let root = T(&mut wrong_tag_target, &[D(tas!(b"not-bn")), limbs]);
        wrong_tag_target.set_root(root);
        let err = derive_job_inputs(&candidate_for_target(wrong_tag_target))
            .expect_err("target with wrong tag is not a bignum");
        assert!(err.contains("%bn"), "unexpected error: {err}");

        let mut improper_list_target = NounSlab::new();
        let limbs = T(&mut improper_list_target, &[D(1), D(7)]);
        let root = T(&mut improper_list_target, &[D(tas!(b"bn")), limbs]);
        improper_list_target.set_root(root);
        let err = derive_job_inputs(&candidate_for_target(improper_list_target))
            .expect_err("target limbs must be a proper list");
        assert!(err.contains("proper list"), "unexpected error: {err}");

        let err = derive_job_inputs(&candidate_for_target(bignum_target_slab(&[u64::from(
            u32::MAX,
        ) + 1])))
        .expect_err("u64 limb exceeds u32");
        assert!(err.contains("u32"), "unexpected error: {err}");

        let err = derive_job_inputs(&candidate_for_target(bignum_target_slab(&[0; 11])))
            .expect_err("oversized limb list is rejected");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
    }

    #[test]
    fn production_preflight_accepts_configured_pearl_merge_submission() {
        let cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.validate()
            .expect("configured miner should pass config preflight");
        cfg.puzzle.validate_canonical_submission_ready().expect(
            "configured Pearl mode should mine ticket attempts before Nockchain submission",
        );
    }

    #[test]
    fn miner_config_preflight_rejects_missing_pkh_configs() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs.clear();

        let err = cfg
            .validate()
            .expect_err("miner config must require at least one PKH config");
        assert!(
            err.to_string().contains("at least one mining PKH"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn miner_config_preflight_rejects_bad_pkh_configs() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs[0].share = 0;
        let err = cfg
            .validate()
            .expect_err("miner config must reject zero-share PKH config");
        assert!(
            err.to_string().contains("share must be nonzero"),
            "unexpected error: {err}"
        );

        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs[0].pkh = "  ".to_string();
        let err = cfg
            .validate()
            .expect_err("miner config must reject empty PKH string");
        assert!(
            err.to_string().contains("pkh must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn miner_config_preflight_rejects_zero_gateway_timing_before_timer_setup() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway.refresh_interval = Duration::ZERO;

        let err = cfg
            .validate()
            .expect_err("zero Pearl Gateway refresh interval would panic tokio interval");
        assert!(
            err.to_string().contains("refresh_interval must be nonzero"),
            "unexpected error: {err}"
        );

        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.pearl_merge.gateway.request_timeout = Duration::ZERO;
        let err = cfg
            .validate()
            .expect_err("zero Pearl Gateway timeout is invalid");
        assert!(
            err.to_string().contains("request_timeout must be nonzero"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn miner_config_rejects_zero_dedicated_search_workers() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.mining_threads = 0;

        let err = cfg
            .validate()
            .expect_err("zero dedicated search workers is invalid");
        assert!(
            err.to_string().contains("mining_threads must be nonzero"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_rejects_invalid_config_before_connecting() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs.clear();
        let err = run(cfg, CancellationToken::new())
            .await
            .expect_err("invalid config should fail before connect");
        assert!(
            err.to_string().contains("at least one mining PKH"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn production_preflight_rejects_pearl_merge_config_param_mismatch() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        let mut pearl = pearl_submission_cfg();
        pearl.mining_config.rank = 32;
        cfg.puzzle.pearl_merge = pearl;

        let err = cfg
            .puzzle
            .validate_canonical_submission_ready()
            .expect_err("Pearl mode must reject mining configs that do not match AI params");
        assert!(
            err.to_string().contains("rank does not match"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn production_preflight_rejects_pearl_merge_unsupported_recursive_params() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.params.difficulty_bits = 1;

        let err = cfg
            .puzzle
            .validate_canonical_submission_ready()
            .expect_err("Pearl mode must reject unsupported recursive params before mining");
        assert!(
            err.to_string().contains("difficulty_bits must be 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn production_preflight_accepts_pearl_merge_multi_tile_recursive_params() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.params.m = 16;
        cfg.puzzle.params.n = 16;

        cfg.puzzle
            .validate_canonical_submission_ready()
            .expect("Pearl mode supports square-contiguous multi-tile ticket params");
    }

    #[test]
    fn production_preflight_rejects_pearl_merge_wrong_spot_checks() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.params.spot_checks = 2;

        let err = cfg
            .puzzle
            .validate_canonical_submission_ready()
            .expect_err("Pearl mode must reject unsupported spot-check params before mining");
        assert!(
            err.to_string().contains("spot_checks must be 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn production_preflight_accepts_pearl_merge_noncontiguous_recursive_pattern() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        let mut pearl = pearl_submission_cfg();
        pearl.mining_config.rows_pattern =
            PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap();
        pearl.mining_config.cols_pattern =
            PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap();
        cfg.puzzle.pearl_merge = pearl;
        cfg.puzzle.params.m = 128;
        cfg.puzzle.params.n = 128;

        cfg.puzzle
            .validate_canonical_submission_ready()
            .expect("Pearl mode must accept in-bounds non-contiguous recursive patterns");
    }

    #[test]
    fn production_preflight_rejects_noncanonical_pearl_aux_template() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        let pearl = &mut cfg.puzzle.pearl_merge;
        pearl.aux_template.nockchain_chain_id.clear();

        let err = cfg
            .puzzle
            .validate_canonical_submission_ready()
            .expect_err("Pearl mode must reject noncanonical aux templates before mining");
        assert!(
            err.to_string()
                .contains("Nockchain aux chain id must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_has_kernel_command_shape() {
        let aux = PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet".to_vec(),
            nock_block_commitment: [0x42; 32],
            nockchain_target_epoch_or_height: 123_456,
            extra_domain_data: b"ai-pow-target-window".to_vec(),
        };
        let expected_aux_commitment = aux.commitment().expect("aux commitment");
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&expected_aux_commitment);
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: [0x20; ai_pow::pearl_compat::PEARL_PUBLIC_PROOF_PARAMS_SIZE],
            expected_aux_commitment,
            aux,
        };
        let params = ZkParams {
            m: 8,
            k: 512,
            n: 8,
            noise_rank: 32,
            tile: 8,
            difficulty_bits: 0,
        };
        let commitments = ZkPublicCommitments {
            h_a_chunk: [12; 32],
            h_b_chunk: [13; 32],
        };
        let pis = CompositePublicInputs::zero();
        let artifact = build_ai_pow_pearl_merge_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &params,
            0,
            8_192,
            &commitments,
            &pis,
            &AiProofNode::Unit,
        )
        .expect("build ai-pow artifact");

        let poke =
            build_ai_pow_pearl_merge_certificate_poke(&artifact).expect("build pearl merge poke");
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };

        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));

        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        assert!(pow_cell.head().eq_bytes("pow"));

        let ai_pow_noun = pow_cell.tail().noun();
        let ai_pow_cell = ai_pow_noun.in_space(&space).as_cell().expect("ai-pow cell");
        assert!(ai_pow_cell.head().eq_bytes("ai-pow"));

        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode wrapped pearl merge artifact");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert_eq!(decoded.certificate.zk_params, params);
        assert_eq!(decoded.certificate.commitments, commitments);
        assert_eq!(decoded.certificate.public_inputs, pis);
        assert_eq!(decoded.certificate.certificate, AiProofNode::Unit);
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_derives_artifact() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-ticket-poke", &params);
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &pearl_test_config(),
            &params,
            0,
            0,
            &a,
            &b,
            &crate::easy_nock_target(),
            16,
            aux,
        )
        .expect("evaluate Pearl ticket");

        let poke = build_ai_pow_pearl_merge_certificate_poke_from_ticket_node(
            &attempt,
            &aux_inclusion,
            &a,
            &b,
            16,
            &AiProofNode::Unit,
        )
        .expect("build pearl merge poke from ticket");
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));
        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        assert!(pow_cell.head().eq_bytes("pow"));
        let ai_pow_noun = pow_cell.tail().noun();
        let ai_pow_cell = ai_pow_noun.in_space(&space).as_cell().expect("ai-pow cell");
        assert!(ai_pow_cell.head().eq_bytes("ai-pow"));

        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode ticket-derived pearl merge artifact");
        assert_eq!(
            decoded.statement,
            PearlMergePublicStatementShape::from_wire_statement(&attempt.statement)
                .expect("statement shape")
        );
        assert_eq!(decoded.certificate.found_idx, 0);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert_eq!(decoded.certificate.certificate, AiProofNode::Unit);
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_preserves_proof_public_inputs() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-ticket-poke-proof-pis", &params);
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &pearl_test_config(),
            &params,
            0,
            0,
            &a,
            &b,
            &crate::easy_nock_target(),
            16,
            aux,
        )
        .expect("evaluate Pearl ticket");
        let mut public_inputs =
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket);
        public_inputs.cumsum = [5, -8, 13, -21];

        let poke = build_ai_pow_pearl_merge_certificate_poke_from_ticket_public_inputs_node(
            &attempt,
            &aux_inclusion,
            &a,
            &b,
            16,
            &public_inputs,
            &AiProofNode::Unit,
        )
        .expect("build pearl merge poke from ticket and proof public inputs");
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        let ai_pow_noun = pow_cell.tail().noun();
        let ai_pow_cell = ai_pow_noun.in_space(&space).as_cell().expect("ai-pow cell");
        assert!(ai_pow_cell.head().eq_bytes("ai-pow"));
        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode wrapped pearl merge artifact");

        assert_eq!(decoded.certificate.public_inputs, public_inputs);
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_rejects_stale_recursive_run_metadata() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-ticket-poke-stale-run", &params);
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &pearl_test_config(),
            &params,
            0,
            0,
            &a,
            &b,
            &crate::easy_nock_target(),
            16,
            aux,
        )
        .expect("evaluate Pearl ticket");
        let parts =
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16).unwrap();
        let stale = PearlMergeCertificateProof {
            zk_params: parts.zk_params,
            found_idx: parts.found_idx + 1,
            commitments: parts.commitments,
            public_inputs: parts.public_inputs.clone(),
            trace_height: parts.trace_height,
            certificate: AiProofNode::Unit,
        };

        let err = build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
            &attempt, &aux_inclusion, &a, &b, 16, &stale,
        )
        .expect_err("stale recursive-run metadata must not be submitted");
        assert!(
            err.to_string().contains("recursive-run.found-idx"),
            "unexpected error: {err}"
        );

        let mut forged_public_inputs = parts.public_inputs.clone();
        forged_public_inputs.hash_jackpot[0] ^= 1;
        let forged = PearlMergeCertificateProof {
            zk_params: parts.zk_params,
            found_idx: parts.found_idx,
            commitments: parts.commitments,
            public_inputs: forged_public_inputs,
            trace_height: parts.trace_height,
            certificate: AiProofNode::Unit,
        };
        let err = build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
            &attempt, &aux_inclusion, &a, &b, 16, &forged,
        )
        .expect_err("forged recursive-run public inputs must not be submitted");
        assert!(
            err.to_string().contains("public-inputs.hash-jackpot"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_rejects_stale_aux_inclusion() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-ticket-poke-stale-aux", &params);
        let aux = pearl_test_aux();
        let (header, _) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &pearl_test_config(),
            &params,
            0,
            0,
            &a,
            &b,
            &crate::easy_nock_target(),
            16,
            aux,
        )
        .expect("evaluate Pearl ticket");
        let parts =
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16).unwrap();
        let proof = PearlMergeCertificateProof {
            zk_params: parts.zk_params,
            found_idx: parts.found_idx,
            commitments: parts.commitments,
            public_inputs: parts.public_inputs,
            trace_height: parts.trace_height,
            certificate: AiProofNode::Unit,
        };
        let (_, stale_aux_inclusion) = pearl_test_aux_inclusion(&[0x99; 32]);

        let err = build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
            &attempt, &stale_aux_inclusion, &a, &b, 16, &proof,
        )
        .expect_err("stale aux inclusion must not be submitted");
        assert!(
            err.to_string().contains(
                "Pearl aux commitment tag is not present in the txid-committed coinbase script"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn pearl_ticket_loop_output_builds_canonical_ai_pow_poke() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-loop-to-poke", &params);
        let config = pearl_test_config();
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let job = PearlMergeMiningJob {
            header: &header,
            config: &config,
            params: &params,
            nockchain_target: crate::easy_nock_target(),
            a: &a,
            b: &b,
            max_pattern_len: 16,
            aux,
        };
        let mined = pearl_mining::run(
            &job,
            &PearlMergeMineOptions {
                max_attempts: Some(1),
                ..PearlMergeMineOptions::default()
            },
            MiningCancel::new(),
        )
        .expect("Pearl ticket loop should mine first trivial-target ticket");

        let poke = build_ai_pow_pearl_merge_certificate_poke_from_ticket_node(
            &mined.attempt,
            &aux_inclusion,
            &a,
            &b,
            job.max_pattern_len,
            &AiProofNode::Unit,
        )
        .expect("mined Pearl ticket should build ai-pow poke");
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));
        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        assert!(pow_cell.head().eq_bytes("pow"));
        let ai_pow_noun = pow_cell.tail().noun();
        let ai_pow_cell = ai_pow_noun.in_space(&space).as_cell().expect("ai-pow cell");
        assert!(ai_pow_cell.head().eq_bytes("ai-pow"));

        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode mined-ticket ai-pow artifact");
        assert_eq!(
            decoded.certificate.found_idx,
            mined.attempt.public_params.t_rows
        );
        assert_eq!(
            decoded.statement,
            PearlMergePublicStatementShape::from_wire_statement(&mined.attempt.statement)
                .expect("statement shape")
        );
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
    }

    #[test]
    fn pearl_ticket_loop_miss_cannot_build_ai_pow_poke() {
        let params = pearl_test_params();
        let (a, b) = synth_matrices(b"pearl-run-loop-miss-no-poke", &params);
        let mut header = pearl_test_header();
        header.nbits = 0;
        let config = pearl_test_config();
        let job = PearlMergeMiningJob {
            header: &header,
            config: &config,
            params: &params,
            nockchain_target: [0; 32],
            a: &a,
            b: &b,
            max_pattern_len: 16,
            aux: pearl_test_aux(),
        };

        assert!(matches!(
            pearl_mining::run(
                &job,
                &PearlMergeMineOptions {
                    max_attempts: Some(1),
                    ..PearlMergeMineOptions::default()
                },
                MiningCancel::new(),
            ),
            Err(PearlMergeMiningError::BudgetExhausted { max: 1 })
        ));
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_from_ticket_rejects_wrong_matrices() {
        let params = pearl_test_params();
        let (mut a, b) = synth_matrices(b"pearl-run-ticket-poke-wrong-matrices", &params);
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &pearl_test_config(),
            &params,
            0,
            0,
            &a,
            &b,
            &crate::easy_nock_target(),
            16,
            aux,
        )
        .expect("evaluate Pearl ticket");
        a[0] ^= 1;

        assert!(matches!(
            build_ai_pow_pearl_merge_certificate_poke_from_ticket_node(
                &attempt,
                &aux_inclusion,
                &a,
                &b,
                16,
                &AiProofNode::Unit,
            ),
            Err(AiPowCertificatePokeError::PearlMergeArtifact(
                CertificateNounError::PearlMergeStatement(
                    ai_pow::pearl_compat::PearlCompatError::PublicCommitmentMismatch
                )
            ))
        ));
    }

    #[test]
    fn build_ai_pow_pearl_merge_certificate_poke_rejects_wrong_artifact_arm() {
        let mut wrong = NounSlab::new();
        wrong.set_root(D(999));

        assert!(matches!(
            build_ai_pow_pearl_merge_certificate_poke(&wrong),
            Err(AiPowCertificatePokeError::PearlMergeArtifact(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_pearl_merge_submits_nockchain_ai_pow_after_ticket_hit() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        cfg.puzzle.pearl_merge.gateway = gateway.config.clone();

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_node_received_pkh_only_set_key(&node).await;
        let header_seed = 700;
        node.publish_synth_mine_effect_with_target_limbs(header_seed, &fitting_target_limbs(), 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let poke = loop {
            if let Some(poke) = node.mined_pokes.lock().await.pop() {
                break poke;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Pearl merge miner did not submit a %mined poke within 10s; observed {} total pokes",
                node.pokes_observed.load(Ordering::SeqCst)
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

        let expected_nock_commitment =
            *blake3::hash(&synth_block_commitment_slab(header_seed).jam()).as_bytes();
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));
        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        assert!(pow_cell.head().eq_bytes("pow"));
        let ai_pow_noun = pow_cell.tail().noun();
        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode submitted Pearl-compatible ai-pow artifact");
        assert_eq!(
            decoded.statement.aux.nock_block_commitment,
            expected_nock_commitment
        );
        assert_eq!(decoded.aux_inclusion.merkle_branch.len(), 0);
        assert_eq!(decoded.certificate.certificate, AiProofNode::Unit);

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        gateway.shutdown();
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_rejects_stale_recursive_metadata_without_submitting_poke() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        cfg.puzzle.pearl_merge.gateway = gateway.config.clone();
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.certificate_builder = Arc::new(|attempt: &PearlMergeCheckedTicketAttempt| {
            let params = pearl_test_params();
            let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
            let parts =
                pearl_merge_recursive_certificate_parts_from_ticket(attempt.attempt(), &a, &b, 16)
                    .map_err(|e| AiPowCertificateBuildError(e.to_string()))?;
            Ok(PearlMergeCertificateProof {
                zk_params: parts.zk_params,
                found_idx: parts.found_idx + 1,
                commitments: parts.commitments,
                public_inputs: parts.public_inputs,
                trace_height: parts.trace_height,
                certificate: AiProofNode::Unit,
            })
        });

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(701, &fitting_target_limbs(), 64);

        // The stale candidate's certificate build fails deterministically:
        // the miner must drop the candidate WITHOUT submitting and WITHOUT
        // dying, then keep serving the next work cycle.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            node.mined_pokes.lock().await.is_empty(),
            "stale recursive metadata must not be submitted to the node"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(10), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(
            matches!(r, Ok(())),
            "certificate build failure must be a candidate skip, not process-fatal: {r:?}"
        );
        gateway.shutdown();
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_miss_does_not_build_recursive_certificate_or_submit_poke() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        cfg.puzzle.pearl_merge.gateway = gateway.config.clone();
        let builder_calls = Arc::new(AtomicU64::new(0));
        let builder_calls_for_cfg = builder_calls.clone();
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(1),
            ..PearlMergeMineOptions::default()
        };
        pearl_cfg.certificate_builder =
            Arc::new(move |_attempt: &PearlMergeCheckedTicketAttempt| {
                builder_calls_for_cfg.fetch_add(1, Ordering::SeqCst);
                Err(AiPowCertificateBuildError(
                    "certificate builder must not be called on a target miss".to_string(),
                ))
            });

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(702, &[0], 64);
        tokio::time::sleep(Duration::from_millis(700)).await;

        assert_eq!(
            builder_calls.load(Ordering::SeqCst),
            0,
            "recursive certificate builder must only run after a ticket target hit"
        );
        assert!(
            node.mined_pokes.lock().await.is_empty(),
            "target misses must not submit %ai-pow pokes"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        gateway.shutdown();
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_refreshes_pearl_gateway_work_for_current_nockchain_candidate() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let gateway_port = listener.local_addr().expect("gateway addr").port();
        let gateway_calls = Arc::new(AtomicU64::new(0));
        let stop_gateway = Arc::new(AtomicBool::new(false));
        let gateway_calls_for_thread = gateway_calls.clone();
        let stop_gateway_for_thread = stop_gateway.clone();
        let gateway_thread = std::thread::spawn(move || {
            let headers = [
                pearl_test_header(),
                PearlIncompleteBlockHeader {
                    timestamp: pearl_test_header().timestamp + 1,
                    ..pearl_test_header()
                },
            ];
            let mut served = 0usize;
            while served < headers.len() && !stop_gateway_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                if request["method"] == "submitPlainProof" {
                    let response = format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                        request["id"]
                    );
                    std::io::Write::write_all(&mut stream, response.as_bytes())
                        .expect("write gateway submit response");
                    continue;
                }
                assert_eq!(request["method"], "getMiningInfo");
                let (header, encoded_coinbase) =
                    gateway_aux_header_and_coinbase_from_request(&request, headers[served]);
                let encoded_header = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                };
                let target = pearl_target_decimal_for_header(&header);
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                    encoded_header, target, encoded_coinbase
                );
                std::io::Write::write_all(&mut stream, response.as_bytes())
                    .expect("write gateway response");
                served += 1;
                gateway_calls_for_thread.fetch_add(1, Ordering::SeqCst);
            }
        });

        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.gateway = pearl_tcp_gateway(
            gateway_port,
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(0),
            ..PearlMergeMineOptions::default()
        };

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(703, &fitting_target_limbs(), 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while gateway_calls.load(Ordering::SeqCst) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "miner did not refresh Pearl Gateway work for the current Nockchain candidate"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        stop_gateway.store(true, Ordering::SeqCst);
        gateway_thread.join().expect("gateway fixture exited");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_does_not_redispatch_solved_candidate_on_pearl_gateway_refresh() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let gateway_port = listener.local_addr().expect("gateway addr").port();
        let gateway_calls = Arc::new(AtomicU64::new(0));
        let stop_gateway = Arc::new(AtomicBool::new(false));
        let gateway_calls_for_thread = gateway_calls.clone();
        let stop_gateway_for_thread = stop_gateway.clone();
        let gateway_thread = std::thread::spawn(move || {
            let headers = [
                pearl_test_header(),
                PearlIncompleteBlockHeader {
                    timestamp: pearl_test_header().timestamp + 1,
                    ..pearl_test_header()
                },
                PearlIncompleteBlockHeader {
                    timestamp: pearl_test_header().timestamp + 2,
                    ..pearl_test_header()
                },
            ];
            let mut served = 0usize;
            while served < headers.len() && !stop_gateway_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                if request["method"] == "submitPlainProof" {
                    let response = format!(
                        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                        request["id"]
                    );
                    std::io::Write::write_all(&mut stream, response.as_bytes())
                        .expect("write gateway submit response");
                    continue;
                }
                assert_eq!(request["method"], "getMiningInfo");
                let (header, encoded_coinbase) =
                    gateway_aux_header_and_coinbase_from_request(&request, headers[served]);
                let encoded_header = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                };
                let target = pearl_target_decimal_for_header(&header);
                let response = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                    encoded_header, target, encoded_coinbase
                );
                std::io::Write::write_all(&mut stream, response.as_bytes())
                    .expect("write gateway response");
                served += 1;
                gateway_calls_for_thread.fetch_add(1, Ordering::SeqCst);
            }
        });

        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.gateway = pearl_tcp_gateway(
            gateway_port,
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(1),
            ..PearlMergeMineOptions::default()
        };

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(704, &fitting_target_limbs(), 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !node.mined_pokes.lock().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Pearl merge miner did not submit the first %ai-pow poke"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            gateway_calls.load(Ordering::SeqCst) <= 2,
            "a solved Nockchain candidate must not keep fetching Pearl Gateway work after submission"
        );
        assert_eq!(
            node.mined_pokes.lock().await.len(),
            1,
            "a solved Nockchain candidate must produce at most one %ai-pow poke"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        stop_gateway.store(true, Ordering::SeqCst);
        gateway_thread.join().expect("gateway fixture exited");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_v3_gateway_accepts_pearl_only_plain_proof() {
        let commitment_seed = 705;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let gateway_port = listener.local_addr().expect("gateway addr").port();
        let get_calls = Arc::new(AtomicU64::new(0));
        let submit_calls = Arc::new(AtomicU64::new(0));
        let stop_gateway = Arc::new(AtomicBool::new(false));
        let get_calls_for_thread = get_calls.clone();
        let submit_calls_for_thread = submit_calls.clone();
        let stop_gateway_for_thread = stop_gateway.clone();
        let gateway_thread = std::thread::spawn(move || {
            let headers = [
                PearlIncompleteBlockHeader {
                    timestamp: 0x6677_889b,
                    ..pearl_test_header()
                },
                PearlIncompleteBlockHeader {
                    timestamp: 0x6677_889b,
                    ..pearl_test_header()
                },
                PearlIncompleteBlockHeader {
                    timestamp: 0x6677_889c,
                    ..pearl_test_header()
                },
                PearlIncompleteBlockHeader {
                    timestamp: 0x6677_889f,
                    ..pearl_test_header()
                },
            ];
            let mut served_headers = 0usize;
            while !stop_gateway_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                match request["method"].as_str().expect("method string") {
                    "getMiningInfo" => {
                        let header_template =
                            headers[served_headers.min(headers.len().saturating_sub(1))];
                        served_headers += 1;
                        let (header, encoded_coinbase) =
                            gateway_aux_header_and_coinbase_from_request(&request, header_template);
                        let encoded_header = {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                        };
                        let target = pearl_target_decimal_for_header(&header);
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                            encoded_header, target, encoded_coinbase
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway response");
                        get_calls_for_thread.fetch_add(1, Ordering::SeqCst);
                    }
                    "submitPlainProof" => {
                        assert!(
                            request["params"]["plain_proof"]
                                .as_str()
                                .expect("plain_proof string")
                                .len()
                                > 1024
                        );
                        assert!(
                            request["params"]["mining_job"]["incomplete_header_bytes"]
                                .as_str()
                                .expect("incomplete header string")
                                .len()
                                > 32
                        );
                        let expected_target: serde_json::Value = serde_json::from_str(
                            &pearl_target_decimal_for_header(&pearl_test_header()),
                        )
                        .expect("parse expected Pearl target");
                        assert_eq!(request["params"]["mining_job"]["target"], expected_target);
                        assert_eq!(
                            request["params"]["mining_job"]["cert_version"],
                            PEARL_GATEWAY_CERTIFICATE_VERSION_V3
                        );
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                            request["id"]
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway submit response");
                        submit_calls_for_thread.fetch_add(1, Ordering::SeqCst);
                    }
                    other => panic!("unexpected Gateway method: {other}"),
                }
            }
        });

        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.gateway = pearl_tcp_gateway(
            gateway_port,
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(1),
            ..PearlMergeMineOptions::default()
        };

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(commitment_seed, &[0], 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while submit_calls.load(Ordering::SeqCst) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "Pearl-only hit did not resume for changed Pearl Gateway work"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            get_calls.load(Ordering::SeqCst) >= 3,
            "Pearl-only hit should keep refreshing Gateway work for the unsolved Nockchain candidate"
        );
        assert_eq!(submit_calls.load(Ordering::SeqCst), 2);
        assert!(
            node.mined_pokes.lock().await.is_empty(),
            "Pearl-only hit must not submit a Nockchain %ai-pow poke"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        stop_gateway.store(true, Ordering::SeqCst);
        gateway_thread.join().expect("gateway fixture exited");
        node.shutdown().await;
    }

    fn v3_gateway_winning_timestamps(commitment_seed: u64) -> Vec<u32> {
        let params = pearl_test_params();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
        let nock_block_commitment =
            *blake3::hash(&synth_block_commitment_slab(commitment_seed).jam()).as_bytes();
        let mut aux = pearl_test_aux();
        aux.nock_block_commitment = nock_block_commitment;
        let (mut header, _) = pearl_test_aux_inclusion(&aux.commitment().expect("aux commitment"));
        let start = header.timestamp;
        (start..start + 16)
            .filter(|&timestamp| {
                header.timestamp = timestamp;
                evaluate_pearl_merge_ticket_attempt(
                    &header,
                    &config,
                    &params,
                    0,
                    0,
                    &a,
                    &b,
                    &[0; 32],
                    16,
                    aux.clone(),
                )
                .expect("evaluate V3 Pearl ticket")
                .public_params
                .check_pearl_jackpot_difficulty()
                .is_ok()
            })
            .collect()
    }

    /// Pearl V3 selects these Pearl-only headers for the fixed Gateway job.
    #[test]
    fn v3_gateway_header_fixture_has_pinned_pearl_only_hits() {
        assert_eq!(
            v3_gateway_winning_timestamps(705),
            [
                0x6677_889c, 0x6677_889f, 0x6677_88a0, 0x6677_88a1, 0x6677_88a2, 0x6677_88a3,
                0x6677_88a7, 0x6677_88a8,
            ]
        );
    }

    #[test]
    fn v3_gateway_retry_header_fixture_has_pinned_pearl_only_hit() {
        assert_eq!(
            v3_gateway_winning_timestamps(707),
            [
                0x6677_889d, 0x6677_889e, 0x6677_889f, 0x6677_88a0, 0x6677_88a2, 0x6677_88a5,
                0x6677_88a7, 0x6677_88a8,
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_retries_same_pearl_header_after_submit_rpc_failure() {
        let commitment_seed = 707;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let gateway_port = listener.local_addr().expect("gateway addr").port();
        let submit_calls = Arc::new(AtomicU64::new(0));
        let stop_gateway = Arc::new(AtomicBool::new(false));
        let submit_calls_for_thread = submit_calls.clone();
        let stop_gateway_for_thread = stop_gateway.clone();
        let gateway_thread = std::thread::spawn(move || {
            while !stop_gateway_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                match request["method"].as_str().expect("method string") {
                    "getMiningInfo" => {
                        let (header, encoded_coinbase) =
                            gateway_aux_header_and_coinbase_from_request(
                                &request,
                                PearlIncompleteBlockHeader {
                                    timestamp: 0x6677_889d,
                                    ..pearl_test_header()
                                },
                            );
                        let encoded_header = {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                        };
                        let target = pearl_target_decimal_for_header(&header);
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                            encoded_header, target, encoded_coinbase
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway response");
                    }
                    "submitPlainProof" => {
                        let prior = submit_calls_for_thread.fetch_add(1, Ordering::SeqCst);
                        let response = if prior == 0 {
                            format!(
                                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":-32000,\"message\":\"transient submit failure\"}}}}\n",
                                request["id"]
                            )
                        } else {
                            format!(
                                "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                                request["id"]
                            )
                        };
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway submit response");
                    }
                    other => panic!("unexpected Gateway method: {other}"),
                }
            }
        });

        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.gateway = pearl_tcp_gateway(
            gateway_port,
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(1),
            ..PearlMergeMineOptions::default()
        };

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(commitment_seed, &[0], 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while submit_calls.load(Ordering::SeqCst) < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "Pearl submit RPC failure did not retry the same Gateway header"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            node.mined_pokes.lock().await.is_empty(),
            "Pearl-only retry must not submit a Nockchain %ai-pow poke"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        stop_gateway.store(true, Ordering::SeqCst);
        gateway_thread.join().expect("gateway fixture exited");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_dual_hit_submits_pearl_plain_proof_and_nockchain_poke() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind Pearl gateway fixture");
        listener
            .set_nonblocking(true)
            .expect("set Pearl gateway fixture nonblocking");
        let gateway_port = listener.local_addr().expect("gateway addr").port();
        let submit_calls = Arc::new(AtomicU64::new(0));
        let stop_gateway = Arc::new(AtomicBool::new(false));
        let submit_calls_for_thread = submit_calls.clone();
        let stop_gateway_for_thread = stop_gateway.clone();
        let gateway_thread = std::thread::spawn(move || {
            while !stop_gateway_for_thread.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept Pearl gateway client: {e}"),
                };
                stream
                    .set_nonblocking(false)
                    .expect("set Pearl gateway stream blocking");
                let mut request_line = String::new();
                {
                    let mut reader =
                        std::io::BufReader::new(stream.try_clone().expect("clone gateway stream"));
                    std::io::BufRead::read_line(&mut reader, &mut request_line)
                        .expect("read gateway request");
                }
                let request: serde_json::Value =
                    serde_json::from_str(&request_line).expect("parse gateway request");
                match request["method"].as_str().expect("method string") {
                    "getMiningInfo" => {
                        let (header, encoded_coinbase) =
                            gateway_aux_header_and_coinbase_from_request(
                                &request,
                                pearl_test_header(),
                            );
                        let encoded_header = {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                        };
                        let target = pearl_target_decimal_for_header(&header);
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"cert_version\":3,\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
                            encoded_header, target, encoded_coinbase
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write gateway response");
                    }
                    "submitPlainProof" => {
                        assert!(
                            request["params"]["plain_proof"]
                                .as_str()
                                .expect("plain_proof string")
                                .len()
                                > 1024
                        );
                        let expected_target: serde_json::Value = serde_json::from_str(
                            &pearl_target_decimal_for_header(&pearl_test_header()),
                        )
                        .expect("parse expected Pearl target");
                        assert_eq!(request["params"]["mining_job"]["target"], expected_target);
                        assert_eq!(
                            request["params"]["mining_job"]["cert_version"],
                            PEARL_GATEWAY_CERTIFICATE_VERSION_V3
                        );
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":\"submitted\"}}\n",
                            request["id"]
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes())
                            .expect("write Gateway submit response");
                        submit_calls_for_thread.fetch_add(1, Ordering::SeqCst);
                    }
                    other => panic!("unexpected Gateway method: {other}"),
                }
            }
        });

        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let pearl_cfg = &mut cfg.puzzle.pearl_merge;
        pearl_cfg.gateway = pearl_tcp_gateway(
            gateway_port,
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        pearl_cfg.mine_opts = PearlMergeMineOptions {
            max_attempts: Some(1),
            ..PearlMergeMineOptions::default()
        };

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        node.publish_synth_mine_effect_with_target_limbs(706, &fitting_target_limbs(), 64);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let pearl_submitted = submit_calls.load(Ordering::SeqCst) == 1;
            let nockchain_submitted = !node.mined_pokes.lock().await.is_empty();
            if pearl_submitted && nockchain_submitted {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "dual target hit did not submit both Pearl and Nockchain solutions"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        assert_eq!(submit_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            node.mined_pokes.lock().await.len(),
            1,
            "dual target hit must produce exactly one Nockchain %ai-pow poke"
        );

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        stop_gateway.store(true, Ordering::SeqCst);
        gateway_thread.join().expect("gateway fixture exited");
        node.shutdown().await;
    }

    #[ignore = "real dense production proof plus mock-node endpoint; opt-in"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_dense_production_shape_reaches_mock_node() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        let node = MockNode::spawn().await;
        let params = crate::DENSE_PRODUCTION_PARAMS;
        let (a, b) = synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        let mut cfg = test_cfg(node.url());
        cfg.puzzle.params = params;
        cfg.puzzle.a = Arc::new(a);
        cfg.puzzle.b = Arc::new(b);
        cfg.puzzle.pearl_merge = PearlMergeSubmissionConfig::new_compact_recursive(
            gateway.config.clone(),
            PearlMiningConfig {
                common_dim: params.k,
                rank: params.noise_rank as u16,
                mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
                rows_pattern: pearl_test_pattern(params.tile),
                cols_pattern: pearl_test_pattern(params.tile),
                reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
            },
            pearl_test_aux(),
            params.tile as usize,
            PearlMergeMineOptions {
                max_attempts: Some(1),
                ..PearlMergeMineOptions::default()
            },
            params,
            cfg.puzzle.a.clone(),
            cfg.puzzle.b.clone(),
        );

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_node_received_pkh_only_set_key(&node).await;
        let commitment_seed = 710;
        node.publish_synth_mine_effect_with_target_limbs(
            commitment_seed,
            &fitting_target_limbs(),
            64,
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let poke = loop {
            if let Some(poke) = node.mined_pokes.lock().await.pop() {
                break poke;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "dense production route did not submit a %mined poke within 120s; observed {} total pokes",
                node.pokes_observed.load(Ordering::SeqCst)
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

        let expected_nock_commitment =
            *blake3::hash(&synth_block_commitment_slab(commitment_seed).jam()).as_bytes();
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command_cell = root.in_space(&space).as_cell().expect("poke cell");
        assert!(command_cell.head().eq_bytes("command"));
        let pow_cell = command_cell
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("pow cell");
        assert!(pow_cell.head().eq_bytes("pow"));
        let ai_pow_noun = pow_cell.tail().noun();
        let decoded = decode_ai_pow_pearl_merge_artifact_noun(
            ai_pow_noun,
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode dense production ai-pow artifact");
        assert_eq!(
            decoded.statement.aux.nock_block_commitment,
            expected_nock_commitment
        );
        assert_eq!(decoded.certificate.zk_params.m, 512);
        assert_eq!(decoded.certificate.zk_params.k, 1024);
        assert_eq!(decoded.certificate.zk_params.n, 512);
        assert_eq!(decoded.certificate.zk_params.noise_rank, 64);
        assert!(matches!(
            decoded.certificate.certificate,
            AiProofNode::Bytes(_)
        ));

        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        assert!(matches!(r, Ok(())), "unexpected miner result: {r:?}");
        node.shutdown().await;
        gateway.shutdown();
    }

    /// Heavy: runs the real ai-pow prover on TEST_SMALL with a trivial
    /// uint256 `FF..FF` target. Should complete in well under 30 s on any
    /// modern machine; marked `#[ignore]` so `cargo test` is fast by
    /// default. Run with `cargo test -p ai-pow-miner --features node
    /// --test node_run_mock_node -- --ignored`.
    #[ignore = "manual mock-node integration test; runs the real prover"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_against_mock_node_submits_ai_pow_command_when_recursive_cert_available() {
        let gateway = spawn_static_aux_pearl_gateway(
            pearl_test_header(),
            Duration::from_millis(200),
            Duration::from_millis(100),
        );
        let node = MockNode::spawn().await;
        let mut cfg = test_cfg(node.url());
        let params = cfg.puzzle.params;
        let a = cfg.puzzle.a.clone();
        let b = cfg.puzzle.b.clone();
        let pearl_cfg = cfg.puzzle.pearl_merge.clone();
        cfg.puzzle.pearl_merge = PearlMergeSubmissionConfig::new_compact_recursive(
            gateway.config.clone(),
            pearl_cfg.mining_config,
            pearl_cfg.aux_template,
            pearl_cfg.max_pattern_len,
            pearl_cfg.mine_opts,
            params,
            a,
            b,
        );

        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });

        // Brief pause for the miner to connect + configure + subscribe.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_node_received_pkh_only_set_key(&node).await;
        node.publish_synth_mine_effect_with_target_limbs(100, &fitting_target_limbs(), 64);

        // Poll for the miner wire poke. The ticket hits immediately, but the
        // production recursive certificate is built before the node submission.
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        let mut got_mined = false;
        while std::time::Instant::now() < deadline {
            if !node.mined_pokes.lock().await.is_empty() {
                got_mined = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            got_mined,
            "miner did not submit a %mined poke within 90s; observed {} total pokes",
            node.pokes_observed.load(Ordering::SeqCst)
        );

        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), mining_task)
            .await
            .expect("miner task did not exit");
        node.shutdown().await;
        gateway.shutdown();
    }

    /// Cheap: confirms the node runner fails closed before reconnect work when
    /// the configured recursive certificate is not canonical-admissible.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_loop_rejects_before_connect_when_recursive_cert_unavailable() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.params.difficulty_bits = 1;
        let shutdown = CancellationToken::new();
        let r = tokio::time::timeout(Duration::from_secs(2), run(cfg, shutdown))
            .await
            .expect("run didn't terminate");
        match r {
            Err(MinerError::CanonicalCertificateUnavailable(msg)) => {
                assert!(
                    msg.contains("difficulty_bits must be 0"),
                    "unexpected error: {msg}"
                );
            }
            other => panic!("expected CanonicalCertificateUnavailable, got {other:?}"),
        }
    }

    /// Cheap: confirms shutdown does not turn the canonical-certificate
    /// preflight failure into a successful run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_shutdown_still_reports_unavailable_recursive_cert() {
        let mut cfg = test_cfg("http://127.0.0.1:1".to_string());
        cfg.puzzle.params.difficulty_bits = 1;
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task = tokio::spawn(async move { run(cfg, shutdown_clone).await });
        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(10), mining_task)
            .await
            .expect("miner did not exit within 10s")
            .expect("miner panicked");
        assert!(
            matches!(r, Err(MinerError::CanonicalCertificateUnavailable(_))),
            "expected recursive certificate artifact to remain unavailable, got {r:?}"
        );
    }
}
