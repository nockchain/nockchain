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
//! 4. Inner loop (single worker for v1):
//!    - shutdown → cancel current attempt + best-effort
//!      `enable_mining(false)` + exit.
//!    - new candidate -> cancel any in-flight attempt, derive the
//!      Pearl-compatible mining job, and spawn the worker.
//!    - worker result:
//!      - success → build the recursive certificate artifact only after a
//!        target hit, then poke the node with a `%ai-pow` command on
//!        [`crate::wire::AiPowMinerWire::Mined`].
//!      - error → log + idle.
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
//! the Nockchain `%ai-pow` command. The kernel remains fail-closed until
//! recursive certificate verification is wired.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    pearl_nbits_to_target_le, validate_pearl_merge_config_for_recursive_prover,
    verify_pearl_aux_inclusion, PearlAuxInclusionProof, PearlCompatError,
    PearlIncompleteBlockHeader, PearlMergeTicketAttempt, PearlMiningConfig, PearlNockchainAux,
    PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES, PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH,
    PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
};
use ai_pow::zk_bridge::{
    AiPowCompactRecursiveCertificateRun, AiPowCompactRecursiveProverCache,
    AiPowRecursiveCertificateRun, ZkPublicCommitments,
};
use ai_pow_zk::{CompositePublicInputs, ZkParams};
use futures::StreamExt;
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::NounSlab;
use nockchain_mining_common::{MiningCandidate, MiningCandidateKind, MiningPkhConfig, NodeClient};
use nockvm::noun::{NounAllocator, D, T};
use nockvm_macros::tas;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::certificate_noun::{
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run,
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node,
    build_ai_pow_pearl_merge_artifact_noun_from_ticket_recursive_run,
    decode_ai_pow_pearl_merge_artifact_metadata_slab, AiProofNode, CertificateNounError,
    CertificateNounLimits,
};
use crate::pearl_mining::{
    self, PearlMergeMineOptions, PearlMergeMinedTicket, PearlMergeMiningError, PearlMergeMiningJob,
};
use crate::pearl_plain_proof::PearlPlainProof;
use crate::wire::AiPowMinerWire;
use crate::{DifficultyTarget, MiningCancel};

// Covers a base64-encoded max-size coinbase inclusion plus the Pearl header,
// target, and JSON-RPC envelope while still bounding untrusted Gateway input.
const PEARL_GATEWAY_MAX_RESPONSE_LINE_BYTES: usize = 160 * 1024;
const MAX_CHAIN_TARGET_U32_LIMBS: usize = 10;
const AI_POW_MINE_CANDIDATE_VERSION: u64 = 3;

type AiPowPearlMergeCertificateBuilder = dyn Fn(&PearlMergeTicketAttempt) -> Result<PearlMergeCertificateProof, AiPowCertificateBuildError>
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
        let compact_prover_cache: Arc<Mutex<Option<Arc<AiPowCompactRecursiveProverCache>>>> =
            Arc::new(Mutex::new(None));
        let certificate_builder = Arc::new(move |attempt: &PearlMergeTicketAttempt| {
            let cached = compact_prover_cache
                .lock()
                .map_err(|_| {
                    AiPowCertificateBuildError(
                        "compact recursive prover cache lock poisoned".to_string(),
                    )
                })?
                .clone();
            if cached.is_some() {
                debug!("using warmed compact recursive prover cache");
            }
            let run = if let Some(cache) = cached.as_deref() {
                ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate_with_prover_cache(
                    attempt,
                    &params,
                    a.as_slice(),
                    b.as_slice(),
                    max_pattern_len,
                    cache,
                )
            } else {
                ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate(
                    attempt,
                    &params,
                    a.as_slice(),
                    b.as_slice(),
                    max_pattern_len,
                )
            }
            .map_err(|e| {
                AiPowCertificateBuildError(format!(
                    "refusing to build Pearl-compatible recursive certificate before successful Nockchain target check: {e}"
                ))
            })?;
            let proof = PearlMergeCertificateProof::from_compact_recursive_run(&run)?;
            if let Some(new_cache) = run.into_prover_cache() {
                let mut guard = compact_prover_cache.lock().map_err(|_| {
                    AiPowCertificateBuildError(
                        "compact recursive prover cache lock poisoned".to_string(),
                    )
                })?;
                debug!("stored compact recursive prover cache from certificate run");
                *guard = Some(Arc::new(new_cache));
            }
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
        attempt: &PearlMergeTicketAttempt,
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
    #[error("{0}")]
    CertificateBuild(String),
    #[error("{0}")]
    CanonicalCertificateUnavailable(String),
}

/// Production entry point. Returns `Ok(())` on clean shutdown, `Err` on
/// unrecoverable failure.
pub async fn run(cfg: MinerConfig, shutdown: CancellationToken) -> Result<(), MinerError> {
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
        // `worker` is the currently-running spawn-blocking task (if any).
        // `latest_candidate` stores only sendable decoded candidate inputs,
        // which lets Gateway-backed Pearl work refreshes restart from the same
        // Nockchain candidate when Pearl emits a newer block template. On
        // shutdown we cancel + drain.
        let mut worker: Option<MiningWorker> = None;
        let mut latest_candidate: Option<NockchainCandidateInputs> = None;
        let mut current_pearl_header: Option<PearlIncompleteBlockHeader> = None;
        let mut next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
        let mut pearl_refresh = tokio::time::interval(pearl_work_refresh_interval(&cfg));
        pearl_refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let inner_result: InnerOutcome = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break InnerOutcome::Shutdown,
                maybe_c = candidates.next() => {
                    let Some(c_res) = maybe_c else {
                        warn!("watch_candidates stream ended; will reconnect");
                        break InnerOutcome::StreamLost;
                    };
                    let candidate = match c_res {
                        Ok(c) => c,
                        Err(e) => break InnerOutcome::Fatal(MinerError::CandidateDecode(format!("{e}"))),
                    };
                    let candidate_inputs = match derive_nockchain_candidate_inputs(&candidate) {
                        Ok(x) => x,
                        Err(e) => {
                            current_pearl_header = None;
                            warn!(error = %e, "could not derive Nockchain candidate inputs; skipping");
                            continue;
                        }
                    };
                    latest_candidate = Some(candidate_inputs);
                    next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
                    // Cancel any in-flight attempt; await its join so we
                    // don't accumulate handles. Drop the result — we're
                    // moving on.
                    if let Some(w) = worker.take() {
                        w.cancel();
                        if let Err(e) = w.await_join().await {
                            debug!(error = %e, "prior worker join error (ignored)");
                        }
                    }
                    let pearl_job = match derive_pearl_merge_job_inputs_from_nockchain(&cfg, &candidate_inputs) {
                        Ok(x) => x,
                        Err(e) => {
                            current_pearl_header = None;
                            warn!(error = %e, "could not derive Pearl merge job inputs from candidate; skipping");
                            continue;
                        }
                    };
                    current_pearl_header = Some(pearl_job.header);
                    let cancel = MiningCancel::new();
                    info!(pow_len = candidate_inputs.pow_len, "new candidate; dispatching Pearl-compatible ai-pow attempt");
                    let h = spawn_pearl_merge_attempt(
                        &cfg,
                        pearl_job,
                        pearl_mine_opts_with_attempt_start(&cfg, next_pearl_attempt_start),
                        cancel.clone(),
                    );
                    worker = Some(MiningWorker::PearlMerge { handle: h, cancel });
                }
                _ = pearl_refresh.tick() => {
                    let Some(candidate_inputs) = latest_candidate else {
                        continue;
                    };
                    let pearl_job = match derive_pearl_merge_job_inputs_from_nockchain(&cfg, &candidate_inputs) {
                        Ok(x) => x,
                        Err(e) => {
                            warn!(error = %e, "could not refresh Pearl Gateway work for current Nockchain candidate");
                            continue;
                        }
                    };
                    if current_pearl_header == Some(pearl_job.header) {
                        continue;
                    }
                    if let Some(w) = worker.take() {
                        w.cancel();
                        if let Err(e) = w.await_join().await {
                            debug!(error = %e, "prior worker join error after Pearl refresh (ignored)");
                        }
                    }
                    current_pearl_header = Some(pearl_job.header);
                    next_pearl_attempt_start = cfg.puzzle.pearl_merge.mine_opts.attempt_start;
                    let cancel = MiningCancel::new();
                    info!(pow_len = candidate_inputs.pow_len, "Pearl Gateway work changed; redispatching ai-pow attempt for current Nockchain candidate");
                    let h = spawn_pearl_merge_attempt(
                        &cfg,
                        pearl_job,
                        pearl_mine_opts_with_attempt_start(&cfg, next_pearl_attempt_start),
                        cancel.clone(),
                    );
                    worker = Some(MiningWorker::PearlMerge { handle: h, cancel });
                }
                done = await_worker(&mut worker) => {
                    // worker is now None.
                    match done {
                        WorkerOutcome::None => {
                            // No worker was running; await_worker yielded
                            // immediately. Park ourselves until the next
                            // candidate / shutdown.
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        WorkerOutcome::PearlJoined(Ok(Ok(mined))) => {
                            info!(
                                matmul_attempts = mined.ticket.stats.matmul_attempts_tried,
                                elapsed_s = mined.ticket.stats.elapsed.as_secs_f64(),
                                matmul_attempt_rate = mined.ticket.stats.matmul_attempt_rate_per_sec(),
                                pearl_target_hit = mined.ticket.pearl_target_hit,
                                nockchain_target_hit = mined.ticket.nockchain_target_hit,
                                "ai-pow-miner: Pearl-compatible solution found"
                            );
                            let pearl_cfg = &cfg.puzzle.pearl_merge;
                            let mut pearl_submit_failed = false;
                            if mined.ticket.pearl_target_hit {
                                if let Err(e) = submit_pearl_solution_to_gateway(&cfg, pearl_cfg, &mined) {
                                    pearl_submit_failed = true;
                                    warn!(error = %e, "submit Pearl Gateway plain proof failed");
                                }
                            }
                            if !mined.ticket.nockchain_target_hit {
                                if pearl_submit_failed {
                                    current_pearl_header = None;
                                } else {
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
                                    let h = spawn_pearl_merge_attempt(
                                        &cfg,
                                        pearl_job,
                                        pearl_mine_opts_with_attempt_start(
                                            &cfg,
                                            next_pearl_attempt_start,
                                        ),
                                        cancel.clone(),
                                    );
                                    worker = Some(MiningWorker::PearlMerge { handle: h, cancel });
                                }
                                continue;
                            }
                            let proof = match pearl_cfg.build_certificate_for_attempt(&mined.ticket.attempt) {
                                Ok(proof) => proof,
                                Err(e) => {
                                    warn!(error = %e, "Pearl-compatible recursive AI-PoW certificate build failed");
                                    break InnerOutcome::Fatal(MinerError::CertificateBuild(e.to_string()));
                                }
                            };
                            let poke = match build_ai_pow_pearl_merge_certificate_poke_from_ticket_proof(
                                &mined.ticket.attempt,
                                &mined.aux_inclusion,
                                &cfg.puzzle.a,
                                &cfg.puzzle.b,
                                pearl_cfg.max_pattern_len,
                                &proof,
                            ) {
                                Ok(poke) => poke,
                                Err(e) => {
                                    warn!(error = %e, "canonical Pearl-compatible AI-PoW certificate poke build failed");
                                    break InnerOutcome::Fatal(MinerError::CertificateBuild(e.to_string()));
                                }
                            };
                            if let Err(e) = client
                                .poke_wire(AiPowMinerWire::Mined.to_wire(), poke)
                                .await
                            {
                                warn!(error = %e, "submit Pearl-compatible ai-pow certificate poke failed (likely stale candidate)");
                            }
                            latest_candidate = None;
                            current_pearl_header = None;
                        }
                        WorkerOutcome::PearlJoined(Ok(Err(PearlMergeMiningError::Cancelled))) => {
                            debug!("Pearl-compatible worker cancelled (expected on candidate supersede / shutdown)");
                        }
                        WorkerOutcome::PearlJoined(Ok(Err(e))) => {
                            warn!(error = %e, "Pearl-compatible ai-pow attempt terminated without solution");
                        }
                        WorkerOutcome::PearlJoined(Err(e)) => {
                            break InnerOutcome::Fatal(MinerError::WorkerJoin(format!("{e}")));
                        }
                    }
                }
            }
        };

        // ── cleanup before reconnect or exit ──
        if let Some(w) = worker.take() {
            w.cancel();
            let _ = w.await_join().await;
        }
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

enum MiningWorker {
    PearlMerge {
        handle: JoinHandle<Result<PearlMergeMinedSubmission, PearlMergeMiningError>>,
        cancel: MiningCancel,
    },
}

impl MiningWorker {
    fn cancel(&self) {
        match self {
            MiningWorker::PearlMerge { cancel, .. } => {
                cancel.cancel();
            }
        }
    }

    async fn await_join(self) -> Result<(), tokio::task::JoinError> {
        match self {
            MiningWorker::PearlMerge { handle, .. } => handle.await.map(|_| ()),
        }
    }
}

enum WorkerOutcome {
    /// No worker was running; the future returned immediately.
    None,
    /// Worker joined: outer Result = tokio JoinError, inner = mining result.
    PearlJoined(
        Result<Result<PearlMergeMinedSubmission, PearlMergeMiningError>, tokio::task::JoinError>,
    ),
}

/// Helper to make `tokio::select!` work over an `Option<JoinHandle>`.
/// If the slot is empty, returns `WorkerOutcome::None` immediately
/// (caller pauses to avoid a busy-loop). If the slot has a handle,
/// awaits it (drops it on join). Mutates `worker` in place so the
/// caller doesn't need to thread the take/put.
async fn await_worker(worker: &mut Option<MiningWorker>) -> WorkerOutcome {
    match worker.take() {
        Some(MiningWorker::PearlMerge { handle, .. }) => WorkerOutcome::PearlJoined(handle.await),
        None => WorkerOutcome::None,
    }
}

/// Derive the per-candidate job inputs the AI-PoW prover needs:
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

#[derive(Clone, Debug)]
struct PearlGatewayResolvedMiningJob {
    header: PearlIncompleteBlockHeader,
    target: serde_json::Value,
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
    if response.id != serde_json::Value::from(request_id) {
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
    header: &PearlIncompleteBlockHeader,
    target: serde_json::Value,
) -> Result<(), PearlGatewayError> {
    validate_pearl_gateway_target_uint256(&target)?;
    let request_id = 2u64;
    let incomplete_header_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
    };
    let request = json!({
        "jsonrpc": "2.0",
        "method": "submitPlainProof",
        "params": {
            "plain_proof": plain_proof_base64,
            "mining_job": {
                "incomplete_header_bytes": incomplete_header_bytes,
                "target": target,
            },
        },
        "id": request_id,
    })
    .to_string();
    let response_line = exchange_pearl_gateway_request(config, &request)?;
    let response: PearlGatewaySubmitRpcResponse = serde_json::from_str(&response_line)?;
    if response.id != serde_json::Value::from(request_id) {
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

fn decode_chain_target_bignum(target: &NounSlab) -> Result<DifficultyTarget, String> {
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

/// Spawn the Pearl-compatible ticket worker. This evaluates ticket attempts
/// only; recursive proof construction happens in the async submission path
/// after the worker returns a Nockchain-target hit.
fn spawn_pearl_merge_attempt(
    cfg: &MinerConfig,
    job_inputs: PearlMergeCandidateJob,
    mine_opts: PearlMergeMineOptions,
    cancel: MiningCancel,
) -> JoinHandle<Result<PearlMergeMinedSubmission, PearlMergeMiningError>> {
    let params = cfg.puzzle.params;
    let a = cfg.puzzle.a.clone();
    let b = cfg.puzzle.b.clone();
    let pearl = cfg.puzzle.pearl_merge.clone();
    tokio::task::spawn_blocking(move || {
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
        let ticket = pearl_mining::run(&job, &mine_opts, cancel)?;
        Ok(PearlMergeMinedSubmission {
            ticket,
            gateway_mining_job: job_inputs.gateway_mining_job,
            aux_inclusion: job_inputs.aux_inclusion,
        })
    })
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
    if gateway_job.header != mined_header {
        return Err(
            "Pearl Gateway mining job header does not match the aux-bearing mined header; \
             skipping submitPlainProof because Gateway acknowledges before its async stale-header check"
                .to_string(),
        );
    }
    let plain = PearlPlainProof::from_attempt(
        &cfg.puzzle.params, &mined.ticket.attempt, &cfg.puzzle.a, &cfg.puzzle.b,
    )
    .map_err(|e| format!("build Pearl plain proof: {e}"))?;
    let plain_proof_base64 = plain
        .to_base64_bincode1()
        .map_err(|e| format!("serialize Pearl plain proof: {e}"))?;
    let target = gateway_job.target.clone();
    validate_pearl_gateway_target_uint256(&target)
        .map_err(|e| format!("Gateway mining job target became invalid before submit: {e}"))?;
    submit_pearl_gateway_plain_proof(gateway, &plain_proof_base64, &mined_header, target)
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
    //! `%mine` effect, and assert the miner pokes an
    //! `AiPowMinerWire::Mined` slab back at the server within a
    //! generous timeout. Uses `MatmulParams::TEST_SMALL` + trivial
    //! uint256 `FF..FF` target so the real ai-pow prover wins on extranonce 0.

    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use ai_pow::params::MatmulParams;
    use ai_pow::pearl_compat::{
        evaluate_pearl_merge_ticket_attempt, verify_pearl_aux_inclusion,
        PearlIncompleteBlockHeader, PearlMiningConfig, PearlNockchainAux, PearlPeriodicPattern,
        PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
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
    use crate::certificate_noun::{
        build_ai_pow_pearl_merge_artifact_noun_from_node, decode_ai_pow_pearl_merge_artifact_noun,
        pearl_merge_recursive_certificate_parts_from_ticket,
        pearl_merge_recursive_public_inputs_from_work, AiProofNode, PearlMergePublicStatementShape,
    };
    use crate::pearl_mining::{
        self, PearlMergeMineOptions, PearlMergeMiningError, PearlMergeMiningJob,
    };
    use crate::wire::AiPowMinerWire;

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

        // Publish a synthetic %mine-ai effect (the AI miner subscribes
        // to mine-ai post Stage 4; the mock fixture publishes accordingly).
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
            self.effect_tx.send(slab).expect("publish %mine effect");
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
            nbits: 0x207f_ffff,
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
            certificate_builder: Arc::new(|attempt: &PearlMergeTicketAttempt| {
                let params = pearl_test_params();
                let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
                let parts =
                    pearl_merge_recursive_certificate_parts_from_ticket(attempt, &a, &b, 16)
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
    fn pearl_gateway_fetches_tcp_mining_info() {
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
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{}}}}}\n",
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
        submit_pearl_gateway_plain_proof(&cfg, proof_base64, &header, target)
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
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":\"123456\"}}}}\n",
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
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &mined_header, &pearl_cfg.mining_config, &params, 0, 0, &cfg.puzzle.a, &cfg.puzzle.b,
            &[0xffu8; 32], pearl_cfg.max_pattern_len, aux,
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
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
            reconnect_backoff_initial: Duration::from_millis(50),
            reconnect_backoff_max: Duration::from_millis(200),
            reconnect_max_attempts: 3,
        }
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
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
                "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{}}}}}\n",
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
            Ok(_) => panic!("AI miner must reject non-%3 mine-ai candidates"),
            Err(err) => err,
        };
        assert!(err.contains("%3"), "unexpected error: {err}");
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
            &[0xff; 32],
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
            &[0xff; 32],
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
            &[0xff; 32],
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
            &[0xff; 32],
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
            nockchain_target: [0xff; 32],
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
            &[0xff; 32],
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
        node.publish_synth_mine_effect_with_target_limbs(
            header_seed,
            &[u64::from(u32::MAX); 8],
            64,
        );

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
        pearl_cfg.certificate_builder = Arc::new(|attempt: &PearlMergeTicketAttempt| {
            let params = pearl_test_params();
            let (a, b) = synth_matrices(b"pearl-node-run-submit", &params);
            let parts = pearl_merge_recursive_certificate_parts_from_ticket(attempt, &a, &b, 16)
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
        node.publish_synth_mine_effect_with_target_limbs(701, &[u64::from(u32::MAX); 8], 64);

        let r = tokio::time::timeout(Duration::from_secs(10), mining_task)
            .await
            .expect("miner task did not exit")
            .expect("miner panicked");
        match r {
            Err(MinerError::CertificateBuild(msg)) => {
                assert!(
                    msg.contains("recursive-run.found-idx"),
                    "unexpected certificate build error: {msg}"
                );
            }
            other => panic!("expected stale recursive metadata to fail closed, got {other:?}"),
        }
        assert!(
            node.mined_pokes.lock().await.is_empty(),
            "stale recursive metadata must not be submitted to the node"
        );

        shutdown.cancel();
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
        pearl_cfg.certificate_builder = Arc::new(move |_attempt: &PearlMergeTicketAttempt| {
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
                    "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
        node.publish_synth_mine_effect_with_target_limbs(703, &[u64::from(u32::MAX); 8], 64);

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
                    "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
        node.publish_synth_mine_effect_with_target_limbs(704, &[u64::from(u32::MAX); 8], 64);

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
    async fn run_loop_pearl_only_hit_keeps_nockchain_candidate_for_new_pearl_work() {
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
                pearl_test_header(),
                pearl_test_header(),
                PearlIncompleteBlockHeader {
                    timestamp: pearl_test_header().timestamp + 1,
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
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
                                pearl_test_header(),
                            );
                        let encoded_header = {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD.encode(header.to_bytes())
                        };
                        let target = pearl_target_decimal_for_header(&header);
                        let response = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
                            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"incomplete_header_bytes\":\"{}\",\"target\":{},\"aux_inclusion\":{{\"coinbase_tx\":\"{}\",\"merkle_branch\":[]}}}}}}\n",
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
        node.publish_synth_mine_effect_with_target_limbs(706, &[u64::from(u32::MAX); 8], 64);

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
        node.publish_synth_mine_effect_with_target_limbs(100, &[u64::from(u32::MAX); 8], 64);

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
