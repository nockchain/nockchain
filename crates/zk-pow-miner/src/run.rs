//! Run loop — wires [`NodeClient`] ↔ [`Pool`].
//!
//! Sequence:
//! 1. Build the worker pool once (heavy: spawns N `SerfThread`s, each
//!    loaded with `assets/miner.jam`).
//! 2. (re)connect to the node with backoff. On success:
//!    a. Poke `set-mining-key-advanced`.
//!    b. Subscribe to `watch_candidates` — **before** enabling mining
//!       so the initial candidate isn't lost to a race.
//!    c. Poke `enable-mining(true)` — kernel's post-poke
//!       `update-candidate-block` then emits the first `%mine-zk` effect
//!       on the now-active stream.
//! 3. Inner loop (select):
//!    - shutdown → cancel and bounded-drain the pool
//!    - new candidate → supersede stale work and dispatch every idle worker
//!    - worker result → submit a winner once, continue retry work, or fail
//!      closed after repeated worker faults
//! 4. Transport loss or an acknowledgement-unknown submission → cancel work,
//!    back off with jitter, reconnect, reconfigure, and resubscribe.
//! 5. A valid candidate marks the session healthy and resets the consecutive
//!    reconnect-failure budget.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nockapp::nockapp::wire::Wire;
use nockchain_mining_common::{
    MiningCandidate, MiningPkhConfig, NodeClient, NodeClientError, PokeTransportOutcome,
};
use rand::Rng;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(feature = "cuda")]
use crate::cuda::{device_count, CudaWorker, SharedProver};
use crate::pool::Pool;
use crate::wire::ZkPowMinerWire;
use crate::worker::{
    build_candidate_poke, random_nonce, MineResult, SerfWorker, Worker, WorkerError, WorkerId,
};

const MAX_CONSECUTIVE_CANDIDATE_DECODE_ERRORS: u32 = 3;
const MAX_CONSECUTIVE_WORKER_ERRORS: u32 = 3;

#[derive(Debug, Clone)]
pub struct MinerConfig {
    /// `http://127.0.0.1:5555` by default.
    pub node_addr: String,
    /// v1 pubkey-hash reward configs. Required.
    pub mining_pkh_configs: Vec<MiningPkhConfig>,
    /// Worker pool size.
    pub num_threads: u64,
    /// Deadline for connecting, configuring, subscribing, and receiving a
    /// poke acknowledgement.
    pub rpc_timeout: Duration,
    /// Grace period for cooperative worker cancellation before task abort.
    pub worker_shutdown_timeout: Duration,
    pub reconnect_backoff_initial: Duration,
    pub reconnect_backoff_max: Duration,
    pub reconnect_max_attempts: u32,
}

impl MinerConfig {
    /// Convenience builder with safe defaults: required v1 mining-pkh configs,
    /// num_cpus-1 threads (min 1), 30s RPCs, 10s worker shutdown, and
    /// jittered 1s→30s reconnect backoff with 5 consecutive failures.
    pub fn new(node_addr: String, mining_pkh_configs: Vec<MiningPkhConfig>) -> Self {
        let num_threads = num_cpus::get().saturating_sub(1).max(1) as u64;
        Self {
            node_addr,
            mining_pkh_configs,
            num_threads,
            rpc_timeout: Duration::from_secs(30),
            worker_shutdown_timeout: Duration::from_secs(10),
            reconnect_backoff_initial: Duration::from_secs(1),
            reconnect_backoff_max: Duration::from_secs(30),
            reconnect_max_attempts: 5,
        }
    }

    pub fn validate(&self) -> Result<(), MinerError> {
        validate_mining_pkh_configs(&self.mining_pkh_configs)?;
        if self.num_threads == 0 {
            return Err(MinerError::InvalidConfig(
                "num_threads must be nonzero".to_string(),
            ));
        }
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
        if self.reconnect_backoff_initial > self.reconnect_backoff_max {
            return Err(MinerError::InvalidConfig(
                "reconnect_backoff_initial must not exceed reconnect_backoff_max".to_string(),
            ));
        }
        if self.rpc_timeout.is_zero() {
            return Err(MinerError::InvalidConfig(
                "rpc_timeout must be nonzero".to_string(),
            ));
        }
        if self.worker_shutdown_timeout.is_zero() {
            return Err(MinerError::InvalidConfig(
                "worker_shutdown_timeout must be nonzero".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(feature = "cuda")]
#[derive(Debug, Clone)]
pub struct CudaConfig {
    /// CUDA device ordinals. Empty selects every visible device.
    pub devices: Vec<u32>,
    /// Serf prover workers shared round-robin across CUDA devices.
    pub prover_count: u32,
    /// Nonces evaluated by each device before checking cancellation.
    pub batch_size: u64,
    /// CUDA blocks launched per streaming multiprocessor.
    pub blocks_per_sm: u32,
    /// CUDA threads per block. Must be a multiple of 32.
    pub threads_per_block: u32,
}

#[cfg(feature = "cuda")]
impl Default for CudaConfig {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            prover_count: 1,
            batch_size: 262_144,
            blocks_per_sm: 12,
            threads_per_block: 512,
        }
    }
}

#[cfg(feature = "cuda")]
impl CudaConfig {
    fn validate(&self) -> Result<(), MinerError> {
        if self.prover_count == 0 {
            return Err(MinerError::InvalidConfig(
                "CUDA prover-count must be nonzero".to_owned(),
            ));
        }
        if self.batch_size == 0 {
            return Err(MinerError::InvalidConfig(
                "CUDA batch-size must be nonzero".to_owned(),
            ));
        }
        if self.blocks_per_sm == 0 {
            return Err(MinerError::InvalidConfig(
                "CUDA blocks-per-sm must be nonzero".to_owned(),
            ));
        }
        if self.threads_per_block < 32
            || self.threads_per_block > 1024
            || !self.threads_per_block.is_multiple_of(32)
        {
            return Err(MinerError::InvalidConfig(
                "CUDA threads-per-block must be a multiple of 32 between 32 and 1024".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum MinerError {
    #[error("invalid miner configuration: {0}")]
    InvalidConfig(String),
    #[error("worker spawn failed: {0}")]
    WorkerSpawn(String),
    #[error("kernel configuration failed: {0}")]
    Configure(String),
    #[error("gave up after {count} consecutive connection/session failures")]
    TooManyReconnects { count: u32 },
    #[error("worker {worker} failed after {attempts} consecutive errors: {error}")]
    WorkerFailed {
        worker: WorkerId,
        attempts: u32,
        error: String,
    },
    #[error("worker pool did not stop within {timeout_ms}ms during reconnect")]
    WorkerShutdownTimedOut { timeout_ms: u128 },
}

/// Production entry point. Builds the worker pool then runs the main
/// loop. Returns `Ok(())` on clean shutdown, `Err` on unrecoverable
/// failure.
pub async fn run(cfg: MinerConfig, shutdown: CancellationToken) -> Result<(), MinerError> {
    cfg.validate()?;
    info!(
        node = %cfg.node_addr,
        threads = cfg.num_threads,
        "zk-pow-miner: spawning worker pool"
    );
    let pool = build_pool(cfg.num_threads).await?;
    info!("zk-pow-miner: pool ready; entering main loop");
    run_with_pool(cfg, pool, shutdown).await
}

#[cfg(feature = "cuda")]
pub async fn run_cuda(
    cfg: MinerConfig,
    cuda: CudaConfig,
    shutdown: CancellationToken,
) -> Result<(), MinerError> {
    cfg.validate()?;
    cuda.validate()?;
    let devices = if cuda.devices.is_empty() {
        let count = device_count().map_err(|error| MinerError::WorkerSpawn(error.to_string()))?;
        (0..count).collect::<Vec<_>>()
    } else {
        cuda.devices.clone()
    };
    if devices.is_empty() {
        return Err(MinerError::InvalidConfig(
            "CUDA backend selected but no CUDA devices are visible".to_owned(),
        ));
    }
    let mut unique_devices = devices.clone();
    unique_devices.sort_unstable();
    unique_devices.dedup();
    if unique_devices.len() != devices.len() {
        return Err(MinerError::InvalidConfig(
            "CUDA device list contains duplicate ordinals".to_owned(),
        ));
    }
    if cuda.prover_count as usize > devices.len() {
        return Err(MinerError::InvalidConfig(
            "CUDA prover-count cannot exceed the selected device count".to_owned(),
        ));
    }
    info!(
        node = %cfg.node_addr,
        devices = ?devices,
        provers = cuda.prover_count,
        batch_size = cuda.batch_size,
        blocks_per_sm = cuda.blocks_per_sm,
        threads_per_block = cuda.threads_per_block,
        "zk-pow-miner: spawning CUDA worker pool"
    );
    let pool = build_cuda_pool(&devices, &cuda).await?;
    info!("zk-pow-miner: CUDA pool ready; entering main loop");
    run_with_pool(cfg, pool, shutdown).await
}

#[cfg(feature = "cuda")]
async fn build_cuda_pool(devices: &[u32], cuda: &CudaConfig) -> Result<Pool, MinerError> {
    let hot_state = zkvm_jetpack::hot::produce_prover_hot_state();
    let mut provers = Vec::with_capacity(cuda.prover_count as usize);
    for id in 0..cuda.prover_count {
        let prover = SharedProver::spawn(u64::from(id), hot_state.clone())
            .await
            .map_err(|error| {
                MinerError::WorkerSpawn(format!("shared CUDA prover {id}: {error}"))
            })?;
        provers.push(Arc::new(prover));
    }

    let mut workers: Vec<Arc<dyn Worker>> = Vec::with_capacity(devices.len());
    for (id, &device) in devices.iter().enumerate() {
        let prover = Arc::clone(&provers[id % provers.len()]);
        let worker = CudaWorker::spawn(
            id as u64, device, cuda.batch_size, cuda.blocks_per_sm, cuda.threads_per_block, prover,
        )
        .await
        .map_err(|error| {
            MinerError::WorkerSpawn(format!("CUDA worker {id} on device {device}: {error}"))
        })?;
        workers.push(Arc::new(worker));
    }
    Ok(Pool::new(workers))
}

async fn build_pool(num_threads: u64) -> Result<Pool, MinerError> {
    let hot_state = zkvm_jetpack::hot::produce_prover_hot_state();
    let mut workers: Vec<Arc<dyn Worker>> = Vec::with_capacity(num_threads as usize);
    for id in 0..num_threads {
        let w = SerfWorker::spawn(id, hot_state.clone())
            .await
            .map_err(|e| MinerError::WorkerSpawn(format!("worker {id}: {e}")))?;
        workers.push(Arc::new(w));
    }
    Ok(Pool::new(workers))
}

/// Inner entry point. Takes a pre-built `Pool`. Tests call this
/// directly with a stub-worker-backed pool to avoid spawning Nock VMs.
pub async fn run_with_pool(
    cfg: MinerConfig,
    mut pool: Pool,
    shutdown: CancellationToken,
) -> Result<(), MinerError> {
    cfg.validate()?;
    let mut reconnect = ReconnectState::new(&cfg);

    'outer: loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }

        // The configured endpoint supplies a bounded handshake plus TCP and
        // HTTP/2 keepalives. Cancellation keeps Ctrl-C responsive even while
        // DNS or a connect attempt is pending.
        let connect = NodeClient::connect_with_timeout(&cfg.node_addr, cfg.rpc_timeout);
        tokio::pin!(connect);
        let mut client = match tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            result = &mut connect => result,
        } {
            Ok(client) => client,
            Err(error) => {
                if !reconnect
                    .wait(&cfg, &shutdown, format!("connect failed: {error}"))
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
        };

        // Subscribe before enable-mining so the initial candidate emitted by
        // the node's post-poke update lands on a live stream. Configuration
        // pokes are idempotent: transport ambiguity is retried after reconnect,
        // while an explicit kernel NACK is a permanent configuration failure.
        match call_with_deadline(
            cfg.rpc_timeout,
            &shutdown,
            client.set_mining_key(
                ZkPowMinerWire::SetPubKey.to_wire(),
                Vec::new(),
                cfg.mining_pkh_configs.clone(),
            ),
        )
        .await
        {
            TimedCall::Completed(Ok(())) => {}
            TimedCall::Completed(Err(error @ NodeClientError::PokeRejected { .. })) => {
                return Err(MinerError::Configure(format!("set_mining_key: {error}")));
            }
            TimedCall::Completed(Err(error)) => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!("set_mining_key transport failed: {error}"),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::TimedOut => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!(
                            "set_mining_key exceeded {}ms deadline",
                            cfg.rpc_timeout.as_millis()
                        ),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::Shutdown => return Ok(()),
        }

        let mut candidates = match call_with_deadline(
            cfg.rpc_timeout,
            &shutdown,
            client.watch_candidates(vec![b"mine-zk".to_vec()]),
        )
        .await
        {
            TimedCall::Completed(Ok(stream)) => stream,
            TimedCall::Completed(Err(error)) => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!("watch_candidates setup failed: {error}"),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::TimedOut => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!(
                            "watch_candidates setup exceeded {}ms deadline",
                            cfg.rpc_timeout.as_millis()
                        ),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::Shutdown => return Ok(()),
        };

        match call_with_deadline(
            cfg.rpc_timeout,
            &shutdown,
            client.enable_mining(ZkPowMinerWire::Enable.to_wire(), true),
        )
        .await
        {
            TimedCall::Completed(Ok(())) => {}
            TimedCall::Completed(Err(error @ NodeClientError::PokeRejected { .. })) => {
                return Err(MinerError::Configure(format!(
                    "enable_mining(true): {error}"
                )));
            }
            TimedCall::Completed(Err(error)) => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!("enable_mining(true) transport failed: {error}"),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::TimedOut => {
                if !reconnect
                    .wait(
                        &cfg,
                        &shutdown,
                        format!(
                            "enable_mining(true) exceeded {}ms deadline",
                            cfg.rpc_timeout.as_millis()
                        ),
                    )
                    .await?
                {
                    return Ok(());
                }
                continue;
            }
            TimedCall::Shutdown => return Ok(()),
        }
        info!("zk-pow-miner: subscribed + mining enabled; awaiting candidates");

        let mut current_candidate: Option<MiningCandidate> = None;
        let mut current_generation = 0u64;
        let mut candidate_decode_errors = 0u32;
        let mut worker_errors: HashMap<WorkerId, u32> = HashMap::new();
        let inner_result: InnerOutcome = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break InnerOutcome::Shutdown,
                maybe_c = candidates.next() => {
                    let Some(candidate_result) = maybe_c else {
                        break InnerOutcome::Reconnect(
                            "watch_candidates stream ended".to_string(),
                        );
                    };
                    let candidate = match candidate_result {
                        Ok(candidate) => candidate,
                        Err(NodeClientError::Grpc(error)) => {
                            break InnerOutcome::Reconnect(format!(
                                "watch_candidates stream failed: {error}"
                            ));
                        }
                        Err(error) => {
                            candidate_decode_errors =
                                candidate_decode_errors.saturating_add(1);
                            current_generation = current_generation.wrapping_add(1);
                            current_candidate = None;
                            pool.cancel_all();
                            warn!(
                                consecutive_errors = candidate_decode_errors,
                                error = %error,
                                "invalid candidate superseded current work"
                            );
                            if candidate_decode_errors
                                >= MAX_CONSECUTIVE_CANDIDATE_DECODE_ERRORS
                            {
                                break InnerOutcome::Reconnect(format!(
                                    "{candidate_decode_errors} consecutive invalid candidates"
                                ));
                            }
                            continue;
                        }
                    };

                    reconnect.mark_healthy(&cfg);
                    candidate_decode_errors = 0;
                    worker_errors.clear();
                    current_generation = current_generation.wrapping_add(1);
                    info!(
                        pow_len = candidate.pow_len,
                        generation = current_generation,
                        "new candidate; superseding stale work"
                    );
                    pool.cancel_all();
                    current_candidate = Some(candidate);
                    let current = current_candidate.as_ref().expect("candidate just stored");
                    pool.dispatch_to_idle(current_generation, || {
                        build_candidate_poke(current, random_nonce())
                    });
                }
                Some((worker, generation, result)) = pool.next_result(), if pool.busy_count() > 0 => {
                    let Some(current) = &current_candidate else {
                        debug!(worker, generation, "result has no current candidate; leaving worker idle");
                        continue;
                    };
                    if generation != current_generation {
                        debug!(
                            worker,
                            generation,
                            current_generation,
                            "dropping stale mining result after candidate supersede"
                        );
                        pool.dispatch_one(
                            worker,
                            current_generation,
                            build_candidate_poke(current, random_nonce()),
                        );
                        continue;
                    }

                    match result {
                        Ok(MineResult::Success { poke_slab, .. }) => {
                            worker_errors.remove(&worker);
                            let prepared = NodeClient::prepare_poke_wire(
                                ZkPowMinerWire::Mined.to_wire(),
                                poke_slab,
                            );
                            info!(worker, generation, "found a block; submitting via gRPC");
                            let outcome = client
                                .send_prepared_poke_with_timeout_or_cancel(
                                    prepared,
                                    cfg.rpc_timeout,
                                    shutdown.cancelled(),
                                )
                                .await;
                            let elapsed_ms = outcome.elapsed().as_millis();
                            match outcome {
                                PokeTransportOutcome::Ack { .. } => {
                                    info!(
                                        worker,
                                        generation,
                                        elapsed_ms,
                                        "submission acknowledged by node; awaiting replacement candidate"
                                    );
                                    current_candidate = None;
                                    pool.cancel_all();
                                }
                                PokeTransportOutcome::Nack { code, message, .. } => {
                                    warn!(
                                        worker,
                                        generation,
                                        elapsed_ms,
                                        code,
                                        message,
                                        "node rejected submission; dropping candidate"
                                    );
                                    current_candidate = None;
                                    pool.cancel_all();
                                }
                                PokeTransportOutcome::FailureBeforeSend { error, .. } => {
                                    break InnerOutcome::Reconnect(format!(
                                        "submission failed before send: {error}"
                                    ));
                                }
                                PokeTransportOutcome::AckUnknown { error, .. } => {
                                    if shutdown.is_cancelled() {
                                        break InnerOutcome::Shutdown;
                                    }
                                    break InnerOutcome::Reconnect(format!(
                                        "submission acknowledgement unknown after {elapsed_ms}ms: {error}"
                                    ));
                                }
                            }
                        }
                        Ok(MineResult::Retry { next_nonce }) => {
                            worker_errors.remove(&worker);
                            pool.dispatch_one(
                                worker,
                                current_generation,
                                build_candidate_poke(current, next_nonce),
                            );
                        }
                        Err(error @ WorkerError::Candidate(_)) => {
                            warn!(
                                worker,
                                generation,
                                error = %error,
                                "worker rejected candidate; dropping it"
                            );
                            current_candidate = None;
                            pool.cancel_all();
                        }
                        Err(error) if retryable_worker_error(&error) => {
                            let attempts = worker_errors
                                .entry(worker)
                                .and_modify(|count| *count = count.saturating_add(1))
                                .or_insert(1);
                            if *attempts >= MAX_CONSECUTIVE_WORKER_ERRORS {
                                break InnerOutcome::Fatal(MinerError::WorkerFailed {
                                    worker,
                                    attempts: *attempts,
                                    error: error.to_string(),
                                });
                            }
                            warn!(
                                worker,
                                generation,
                                consecutive_errors = *attempts,
                                error = %error,
                                "worker attempt failed; retrying current candidate"
                            );
                            pool.dispatch_one(
                                worker,
                                current_generation,
                                build_candidate_poke(current, random_nonce()),
                            );
                        }
                        Err(error) => {
                            break InnerOutcome::Fatal(MinerError::WorkerFailed {
                                worker,
                                attempts: 1,
                                error: error.to_string(),
                            });
                        }
                    }
                }
            }
        };

        let forced_abort = quiesce_pool(&mut pool, cfg.worker_shutdown_timeout).await;

        // enable-mining is node-global, not a lease owned by this process.
        // Disabling it here would stop every other miner sharing the node.
        match inner_result {
            InnerOutcome::Shutdown => return Ok(()),
            InnerOutcome::Reconnect(reason) => {
                if forced_abort {
                    return Err(MinerError::WorkerShutdownTimedOut {
                        timeout_ms: cfg.worker_shutdown_timeout.as_millis(),
                    });
                }
                if !reconnect.wait(&cfg, &shutdown, reason).await? {
                    return Ok(());
                }
                continue 'outer;
            }
            InnerOutcome::Fatal(error) => return Err(error),
        }
    }
}

enum TimedCall<T> {
    Completed(T),
    TimedOut,
    Shutdown,
}

async fn call_with_deadline<T, F>(
    timeout: Duration,
    shutdown: &CancellationToken,
    future: F,
) -> TimedCall<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        _ = shutdown.cancelled() => TimedCall::Shutdown,
        result = tokio::time::timeout(timeout, future) => match result {
            Ok(value) => TimedCall::Completed(value),
            Err(_) => TimedCall::TimedOut,
        },
    }
}

struct ReconnectState {
    consecutive_failures: u32,
    backoff: Duration,
}

impl ReconnectState {
    fn new(cfg: &MinerConfig) -> Self {
        Self {
            consecutive_failures: 0,
            backoff: cfg.reconnect_backoff_initial,
        }
    }

    fn mark_healthy(&mut self, cfg: &MinerConfig) {
        if self.consecutive_failures > 0 {
            info!(
                previous_failures = self.consecutive_failures,
                "received valid candidate; reconnect budget reset"
            );
        }
        self.consecutive_failures = 0;
        self.backoff = cfg.reconnect_backoff_initial;
    }

    async fn wait(
        &mut self,
        cfg: &MinerConfig,
        shutdown: &CancellationToken,
        reason: String,
    ) -> Result<bool, MinerError> {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= cfg.reconnect_max_attempts {
            warn!(
                attempt = self.consecutive_failures,
                reason, "connection/session failure budget exhausted"
            );
            return Err(MinerError::TooManyReconnects {
                count: self.consecutive_failures,
            });
        }

        let delay = jittered_backoff(self.backoff);
        warn!(
            attempt = self.consecutive_failures,
            reconnect_in_ms = delay.as_millis(),
            reason,
            "connection/session failure; backing off"
        );
        let retry = tokio::select! {
            _ = shutdown.cancelled() => false,
            _ = tokio::time::sleep(delay) => true,
        };
        if retry {
            self.backoff = self
                .backoff
                .saturating_mul(2)
                .min(cfg.reconnect_backoff_max);
        }
        Ok(retry)
    }
}

fn jittered_backoff(backoff: Duration) -> Duration {
    let upper_nanos = backoff.as_nanos().min(u128::from(u64::MAX)) as u64;
    let lower_nanos = (upper_nanos / 2).max(1).min(upper_nanos);
    Duration::from_nanos(rand::rng().random_range(lower_nanos..=upper_nanos))
}

fn retryable_worker_error(error: &WorkerError) -> bool {
    matches!(
        error,
        WorkerError::Poke(_) | WorkerError::Cuda(_) | WorkerError::Cancelled
    )
}

async fn quiesce_pool(pool: &mut Pool, timeout: Duration) -> bool {
    pool.cancel_all();
    let drain = async {
        while pool.busy_count() > 0 {
            if pool.next_result().await.is_none() {
                break;
            }
        }
    };
    if tokio::time::timeout(timeout, drain).await.is_ok() {
        return false;
    }

    warn!(
        timeout_ms = timeout.as_millis(),
        busy_workers = pool.busy_count(),
        "worker cancellation deadline expired; aborting tasks"
    );
    pool.abort_all().await;
    true
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

enum InnerOutcome {
    Shutdown,
    Reconnect(String),
    Fatal(MinerError),
}

// ────────────────────────────── tests ──────────────────────────────

#[cfg(test)]
mod tests {
    //! Integration tests for the run loop.
    //!
    //! Strategy: stand up a private `NockAppService` gRPC server on an
    //! ephemeral port (the same fixture pattern as the `WatchEffects`
    //! test in `crates/nockapp-grpc/src/tests.rs`), drive
    //! [`run_with_pool`] against it using a StubWorker-backed pool, push
    //! synthetic `%mine-zk` effects, and assert the miner pokes
    //! `ZkPowMinerWire::Mined` back at the server within a tight timeout.

    use std::collections::VecDeque;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use nockapp::driver::{IOAction, NockAppHandle};
    use nockapp::noun::slab::NounSlab;
    use nockapp::NockAppExit;
    use nockapp_grpc::services::private_nockapp::server::PrivateNockAppGrpcServer;
    use nockvm::noun::{NounAllocator, D, T};
    use nockvm_macros::tas;
    use once_cell::sync::Lazy;
    use tokio::sync::{broadcast, mpsc, Mutex as TMutex};

    use super::*;
    use crate::worker::{MineResult, Worker, WorkerError, WorkerId};

    // Shared NockAppMetrics across tests — gnort rejects double-registration.
    static METRICS: Lazy<Arc<nockapp::nockapp::metrics::NockAppMetrics>> = Lazy::new(|| {
        Arc::new(
            nockapp::nockapp::metrics::NockAppMetrics::register(gnort::global_metrics_registry())
                .expect("register NockAppMetrics"),
        )
    });

    #[derive(Debug)]
    enum MockPokeResponse {
        Ack,
        Nack,
        DelayedAck(Duration),
    }

    #[derive(Clone, Copy)]
    enum MockPokeKind {
        SetKey,
        Enable,
        Mined,
        Other,
    }

    #[derive(Default)]
    struct MockResponseQueues {
        set_key: VecDeque<MockPokeResponse>,
        enable: VecDeque<MockPokeResponse>,
        mined: VecDeque<MockPokeResponse>,
    }

    impl MockResponseQueues {
        fn push(&mut self, kind: MockPokeKind, response: MockPokeResponse) {
            match kind {
                MockPokeKind::SetKey => self.set_key.push_back(response),
                MockPokeKind::Enable => self.enable.push_back(response),
                MockPokeKind::Mined => self.mined.push_back(response),
                MockPokeKind::Other => panic!("cannot script an unclassified poke"),
            }
        }

        fn pop(&mut self, kind: MockPokeKind) -> MockPokeResponse {
            let queue = match kind {
                MockPokeKind::SetKey => Some(&mut self.set_key),
                MockPokeKind::Enable => Some(&mut self.enable),
                MockPokeKind::Mined => Some(&mut self.mined),
                MockPokeKind::Other => None,
            };
            queue
                .and_then(VecDeque::pop_front)
                .unwrap_or(MockPokeResponse::Ack)
        }
    }

    /// Restartable private gRPC server backed by channels instead of a kernel.
    /// Tests can script ACK/NACK/delayed-ACK responses per wire.
    struct MockNode {
        addr: SocketAddr,
        action_tx: mpsc::Sender<IOAction>,
        effect_tx: Arc<broadcast::Sender<NounSlab>>,
        exit: NockAppExit,
        responses: Arc<TMutex<MockResponseQueues>>,
        pokes_observed: Arc<AtomicU64>,
        mined_pokes: Arc<TMutex<Vec<NounSlab>>>,
        set_key_pokes: Arc<TMutex<Vec<NounSlab>>>,
        enable_pokes: Arc<TMutex<Vec<NounSlab>>>,
        server_task: Option<tokio::task::JoinHandle<nockapp_grpc::error::Result<()>>>,
        action_drainer: tokio::task::JoinHandle<()>,
    }

    impl MockNode {
        async fn spawn() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().expect("local_addr");
            drop(listener);
            Self::spawn_on(addr).await
        }

        async fn spawn_on(addr: SocketAddr) -> Self {
            let (action_tx, mut action_rx) = mpsc::channel::<IOAction>(64);
            let (effect_tx, _seed_rx) = broadcast::channel::<NounSlab>(64);
            let effect_tx = Arc::new(effect_tx);
            let (exit, _exit_rx) = NockAppExit::new();
            let responses = Arc::new(TMutex::new(MockResponseQueues::default()));
            let pokes_observed = Arc::new(AtomicU64::new(0));
            let mined_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));
            let set_key_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));
            let enable_pokes: Arc<TMutex<Vec<NounSlab>>> = Arc::new(TMutex::new(Vec::new()));

            let responses_clone = responses.clone();
            let pokes_clone = pokes_observed.clone();
            let mined_clone = mined_pokes.clone();
            let set_key_clone = set_key_pokes.clone();
            let enable_clone = enable_pokes.clone();
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
                            let kind = if wire.source != ZkPowMinerWire::SOURCE {
                                MockPokeKind::Other
                            } else if wire.tags.iter().any(|tag| {
                                matches!(tag, nockapp::wire::WireTag::String(value) if value == "mined")
                            }) {
                                mined_clone.lock().await.push(poke);
                                MockPokeKind::Mined
                            } else if wire.tags.iter().any(|tag| {
                                matches!(tag, nockapp::wire::WireTag::String(value) if value == "setpubkey")
                            }) {
                                set_key_clone.lock().await.push(poke);
                                MockPokeKind::SetKey
                            } else if wire.tags.iter().any(|tag| {
                                matches!(tag, nockapp::wire::WireTag::String(value) if value == "enable")
                            }) {
                                enable_clone.lock().await.push(poke);
                                MockPokeKind::Enable
                            } else {
                                MockPokeKind::Other
                            };

                            let response = responses_clone.lock().await.pop(kind);
                            use nockapp::driver::PokeResult;
                            match response {
                                MockPokeResponse::Ack => {
                                    let _ = ack_channel.send(PokeResult::Ack);
                                }
                                MockPokeResponse::Nack => {
                                    let _ = ack_channel.send(PokeResult::Nack);
                                }
                                MockPokeResponse::DelayedAck(delay) => {
                                    tokio::spawn(async move {
                                        tokio::time::sleep(delay).await;
                                        let _ = ack_channel.send(PokeResult::Ack);
                                    });
                                }
                            }
                        }
                        IOAction::Peek { .. } => {}
                    }
                }
            });

            let mut node = Self {
                addr,
                action_tx,
                effect_tx,
                exit,
                responses,
                pokes_observed,
                mined_pokes,
                set_key_pokes,
                enable_pokes,
                server_task: None,
                action_drainer,
            };
            node.start_server().await;
            node
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }

        fn build_handle(&self) -> NockAppHandle {
            NockAppHandle {
                io_sender: self.action_tx.clone(),
                effect_sender: self.effect_tx.clone(),
                effect_receiver: TMutex::new(self.effect_tx.subscribe()),
                metrics: METRICS.clone(),
                exit: self.exit.clone(),
            }
        }

        async fn start_server(&mut self) {
            assert!(self.server_task.is_none(), "mock server already running");
            let server = PrivateNockAppGrpcServer::new(self.build_handle());
            let addr = self.addr;
            self.server_task = Some(tokio::spawn(async move { server.serve(addr).await }));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        async fn stop_server(&mut self) {
            if let Some(task) = self.server_task.take() {
                task.abort();
                let _ = task.await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        async fn queue_response(&self, kind: MockPokeKind, response: MockPokeResponse) {
            self.responses.lock().await.push(kind, response);
        }

        fn publish_synth_mine_effect(&self, header_seed: u64, target_seed: u64, pow_len: u64) {
            let mut slab = NounSlab::new();
            let head = D(tas!(b"mine-zk"));
            let version = D(0);
            let commit = T(
                &mut slab,
                &[
                    D(header_seed),
                    D(header_seed + 1),
                    D(header_seed + 2),
                    D(header_seed + 3),
                    D(header_seed + 4),
                ],
            );
            let target_list = T(&mut slab, &[D(target_seed), D(0)]);
            let target = T(&mut slab, &[D(tas!(b"bn")), target_list]);
            let plen = D(pow_len);
            let effect = T(&mut slab, &[head, version, commit, target, plen]);
            slab.set_root(effect);
            self.effect_tx.send(slab).expect("publish %mine-zk effect");
        }

        fn publish_malformed_mine_effect(&self) {
            let mut slab = NounSlab::new();
            let effect = T(&mut slab, &[D(tas!(b"mine-zk")), D(0)]);
            slab.set_root(effect);
            self.effect_tx
                .send(slab)
                .expect("publish malformed %mine-zk effect");
        }

        async fn shutdown(mut self) {
            self.stop_server().await;
            self.action_drainer.abort();
            let _ = self.action_drainer.await;
        }
    }

    // ── StubWorker for run-loop tests ──
    enum StubAction {
        SuccessImmediate,
        SuccessWithPowAfterDelay { pow: u64, delay: Duration },
        WaitForCancel,
        PokeError,
        Panic,
        IgnoreCancel,
    }

    struct ScriptedStubWorker {
        id: WorkerId,
        cancels: AtomicU64,
        scripts: tokio::sync::Mutex<Vec<StubAction>>,
        attempts: AtomicU64,
    }

    impl ScriptedStubWorker {
        fn new(id: WorkerId, scripts: Vec<StubAction>) -> Arc<Self> {
            Arc::new(Self {
                id,
                cancels: AtomicU64::new(0),
                scripts: tokio::sync::Mutex::new(scripts),
                attempts: AtomicU64::new(0),
            })
        }
    }

    #[async_trait]
    impl Worker for ScriptedStubWorker {
        fn id(&self) -> WorkerId {
            self.id
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
        async fn mine_attempt(&self, _poke: NounSlab) -> Result<MineResult, WorkerError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let action = {
                let mut q = self.scripts.lock().await;
                if q.is_empty() {
                    StubAction::WaitForCancel
                } else {
                    q.remove(0)
                }
            };
            match action {
                StubAction::SuccessImmediate => stub_success_result(42),
                StubAction::SuccessWithPowAfterDelay { pow, delay } => {
                    tokio::time::sleep(delay).await;
                    stub_success_result(pow)
                }
                StubAction::WaitForCancel => {
                    let cancel_baseline = self.cancels.load(Ordering::SeqCst);
                    while self.cancels.load(Ordering::SeqCst) == cancel_baseline {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(WorkerError::Poke("stub cancelled".into()))
                }
                StubAction::PokeError => Err(WorkerError::Poke("scripted failure".into())),
                StubAction::Panic => Err(WorkerError::Panicked),
                StubAction::IgnoreCancel => {
                    std::future::pending::<Result<MineResult, WorkerError>>().await
                }
            }
        }
    }

    fn stub_success_result(pow_payload: u64) -> Result<MineResult, WorkerError> {
        let mut hash_slab = NounSlab::new();
        hash_slab.set_root(D(0));
        let mut poke_slab = NounSlab::new();
        let cmd = T(
            &mut poke_slab,
            &[D(tas!(b"command")), D(tas!(b"pow")), D(pow_payload)],
        );
        poke_slab.set_root(cmd);
        Ok(MineResult::Success {
            hash_slab,
            poke_slab,
        })
    }

    fn test_config(node_addr: String) -> MinerConfig {
        use nockchain_mining_common::MiningPkhConfig;
        MinerConfig {
            node_addr,
            mining_pkh_configs: vec![MiningPkhConfig {
                share: 1,
                pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
            }],
            rpc_timeout: Duration::from_millis(250),
            worker_shutdown_timeout: Duration::from_millis(100),
            num_threads: 1,
            reconnect_backoff_initial: Duration::from_millis(50),
            reconnect_backoff_max: Duration::from_millis(200),
            reconnect_max_attempts: 3,
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

    fn submitted_pow_payload_atom(poke: &NounSlab) -> u64 {
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
        pow_cell
            .tail()
            .as_atom()
            .expect("pow payload atom")
            .as_u64()
            .expect("pow payload fits u64")
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

    async fn wait_for_poke_count(
        pokes: &TMutex<Vec<NounSlab>>,
        expected: usize,
        description: &str,
    ) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if pokes.lock().await.len() >= expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {description}"));
    }

    async fn wait_for_attempt_count(worker: &ScriptedStubWorker, expected: u64) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while worker.attempts.load(Ordering::SeqCst) < expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for worker attempt");
    }

    fn enable_poke_value(poke: &NounSlab) -> bool {
        let space = poke.noun_space();
        let root = unsafe { *poke.root() };
        let command = root.in_space(&space).as_cell().expect("command cell");
        assert!(command.head().eq_bytes("command"));
        let verb = command
            .tail()
            .noun()
            .in_space(&space)
            .as_cell()
            .expect("enable-mining cell");
        assert!(verb.head().eq_bytes("enable-mining"));
        verb.tail()
            .as_atom()
            .expect("enable flag atom")
            .as_u64()
            .expect("enable flag fits u64")
            == 0
    }

    #[test]
    fn miner_config_preflight_rejects_missing_pkh_configs() {
        let mut cfg = test_config("http://127.0.0.1:1".to_string());
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
        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs[0].share = 0;
        let err = cfg
            .validate()
            .expect_err("miner config must reject zero-share PKH config");
        assert!(
            err.to_string().contains("share must be nonzero"),
            "unexpected error: {err}"
        );

        let mut cfg = test_config("http://127.0.0.1:1".to_string());
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
    fn miner_config_preflight_rejects_zero_worker_or_reconnect_settings() {
        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.num_threads = 0;
        let err = cfg
            .validate()
            .expect_err("miner config must reject zero worker count");
        assert!(
            err.to_string().contains("num_threads must be nonzero"),
            "unexpected error: {err}"
        );

        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.reconnect_backoff_initial = Duration::ZERO;
        let err = cfg
            .validate()
            .expect_err("miner config must reject zero reconnect backoff");
        assert!(
            err.to_string()
                .contains("reconnect_backoff_initial must be nonzero"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn miner_config_preflight_rejects_invalid_deadlines_and_backoff_order() {
        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.rpc_timeout = Duration::ZERO;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message)) if message == "rpc_timeout must be nonzero"
        ));

        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.worker_shutdown_timeout = Duration::ZERO;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "worker_shutdown_timeout must be nonzero"
        ));

        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.reconnect_backoff_initial = Duration::from_millis(201);
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "reconnect_backoff_initial must not exceed reconnect_backoff_max"
        ));
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_config_preflight_rejects_invalid_launch_settings() {
        let mut cfg = CudaConfig::default();
        cfg.prover_count = 0;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "CUDA prover-count must be nonzero"
        ));

        cfg.prover_count = 1;
        cfg.batch_size = 0;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "CUDA batch-size must be nonzero"
        ));

        cfg.batch_size = 1;
        cfg.blocks_per_sm = 0;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "CUDA blocks-per-sm must be nonzero"
        ));

        cfg.blocks_per_sm = 1;
        cfg.threads_per_block = 33;
        assert!(matches!(
            cfg.validate(),
            Err(MinerError::InvalidConfig(message))
                if message == "CUDA threads-per-block must be a multiple of 32 between 32 and 1024"
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_with_pool_rejects_invalid_config_before_connecting() {
        let mut cfg = test_config("http://127.0.0.1:1".to_string());
        cfg.mining_pkh_configs.clear();
        let pool = Pool::new(Vec::new());
        let err = run_with_pool(cfg, pool, CancellationToken::new())
            .await
            .expect_err("invalid config should fail before connect");
        assert!(
            err.to_string().contains("at least one mining PKH"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_against_mock_node_submits_mined() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());

        // Pool with one stub worker that wins on its first attempt.
        let worker = ScriptedStubWorker::new(0, vec![StubAction::SuccessImmediate]);
        let workers: Vec<Arc<dyn Worker>> = vec![worker.clone()];
        let pool = Pool::new(workers);

        // Run with a shutdown token we'll trigger after observing the poke.
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        // Brief pause for the miner to connect + configure + subscribe.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_node_received_pkh_only_set_key(&node).await;
        // Publish one synthetic %mine effect.
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);

        // Poll for the %mined poke. Allow up to 2s.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut got_mined = false;
        while std::time::Instant::now() < deadline {
            if !node.mined_pokes.lock().await.is_empty() {
                got_mined = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            got_mined,
            "miner did not submit a %mined poke within 2s; observed {} total pokes",
            node.pokes_observed.load(Ordering::SeqCst)
        );

        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), mining_task)
            .await
            .expect("miner task did not exit");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_drops_stale_success_after_candidate_supersede() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        let worker = ScriptedStubWorker::new(
            0,
            vec![
                StubAction::SuccessWithPowAfterDelay {
                    pow: 100,
                    delay: Duration::from_millis(250),
                },
                StubAction::SuccessWithPowAfterDelay {
                    pow: 200,
                    delay: Duration::ZERO,
                },
            ],
        );
        let workers: Vec<Arc<dyn Worker>> = vec![worker.clone()];
        let pool = Pool::new(workers);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_node_received_pkh_only_set_key(&node).await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        tokio::time::sleep(Duration::from_millis(50)).await;
        node.publish_synth_mine_effect(200, 0xFFFF_FFFF, 2);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let pokes = node.mined_pokes.lock().await;
            if let Some(poke) = pokes.first() {
                assert_eq!(
                    submitted_pow_payload_atom(poke),
                    200,
                    "stale generation success must not be submitted"
                );
                assert_eq!(pokes.len(), 1);
                break;
            }
            drop(pokes);
            assert!(
                std::time::Instant::now() < deadline,
                "fresh generation success was not submitted after stale result was dropped"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(worker.attempts.load(Ordering::SeqCst) >= 2);

        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), mining_task)
            .await
            .expect("miner task did not exit");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_reconnects_when_node_unreachable_then_exits() {
        // No node — point at an unused localhost port that nothing's listening on.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);
        let cfg = MinerConfig {
            node_addr: format!("http://{addr}"),
            mining_pkh_configs: vec![nockchain_mining_common::MiningPkhConfig {
                share: 1,
                pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
            }],
            rpc_timeout: Duration::from_millis(250),
            worker_shutdown_timeout: Duration::from_millis(100),
            num_threads: 1,
            reconnect_backoff_initial: Duration::from_millis(20),
            reconnect_backoff_max: Duration::from_millis(80),
            reconnect_max_attempts: 3,
        };
        let worker = ScriptedStubWorker::new(0, vec![]);
        let workers: Vec<Arc<dyn Worker>> = vec![worker];
        let pool = Pool::new(workers);
        let shutdown = CancellationToken::new();
        // Expect TooManyReconnects within ~500ms.
        let r = tokio::time::timeout(Duration::from_secs(2), run_with_pool(cfg, pool, shutdown))
            .await
            .expect("run_with_pool didn't terminate");
        match r {
            Err(MinerError::TooManyReconnects { count }) => {
                assert!(count >= 3, "expected at least 3 attempts, got {count}");
            }
            other => panic!("expected TooManyReconnects, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_loop_force_aborts_worker_that_ignores_shutdown() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        // This worker never observes cancel(); the run loop must abort its task
        // after worker_shutdown_timeout instead of hanging forever.
        let worker = ScriptedStubWorker::new(0, vec![StubAction::IgnoreCancel]);
        let workers: Vec<Arc<dyn Worker>> = vec![worker.clone()];
        let pool = Pool::new(workers);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });
        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(50, 0xFFFF_FFFF, 2);
        wait_for_attempt_count(&worker, 1).await;
        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(3), mining_task)
            .await
            .expect("miner did not exit within 3s")
            .expect("miner panicked");
        assert!(r.is_ok(), "expected clean Ok on shutdown, got {r:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn explicit_configuration_nack_is_fatal_without_retry() {
        let node = MockNode::spawn().await;
        node.queue_response(MockPokeKind::SetKey, MockPokeResponse::Nack)
            .await;
        let cfg = test_config(node.url());

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_with_pool(cfg, Pool::new(Vec::new()), CancellationToken::new()),
        )
        .await
        .expect("configuration NACK did not terminate miner");
        assert!(matches!(
            result,
            Err(MinerError::Configure(message)) if message.contains("kernel rejected poke")
        ));
        assert_eq!(node.set_key_pokes.lock().await.len(), 1);
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repeated_post_connect_timeouts_exhaust_session_failure_budget() {
        let node = MockNode::spawn().await;
        for _ in 0..3 {
            node.queue_response(
                MockPokeKind::Enable,
                MockPokeResponse::DelayedAck(Duration::from_millis(500)),
            )
            .await;
        }
        let mut cfg = test_config(node.url());
        cfg.rpc_timeout = Duration::from_millis(40);
        cfg.reconnect_backoff_initial = Duration::from_millis(5);
        cfg.reconnect_backoff_max = Duration::from_millis(10);

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_with_pool(cfg, Pool::new(Vec::new()), CancellationToken::new()),
        )
        .await
        .expect("setup timeouts did not terminate miner");
        assert!(matches!(
            result,
            Err(MinerError::TooManyReconnects { count: 3 })
        ));
        assert_eq!(node.set_key_pokes.lock().await.len(), 3);
        assert_eq!(node.enable_pokes.lock().await.len(), 3);
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn miner_recovers_when_node_appears_during_backoff() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);

        let mut cfg = test_config(format!("http://{addr}"));
        cfg.reconnect_max_attempts = 50;
        cfg.reconnect_backoff_initial = Duration::from_millis(10);
        cfg.reconnect_backoff_max = Duration::from_millis(30);
        let worker = ScriptedStubWorker::new(0, vec![StubAction::SuccessImmediate]);
        let pool = Pool::new(vec![worker as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        tokio::time::sleep(Duration::from_millis(75)).await;
        let node = MockNode::spawn_on(addr).await;
        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke after delayed startup").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "submission after delayed startup").await;

        shutdown.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), mining_task)
            .await
            .expect("miner did not stop")
            .expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test]
    async fn effect_stream_data_loss_forces_reconnect_and_resubscribe() {
        let node = MockNode::spawn().await;
        node.queue_response(
            MockPokeKind::Mined,
            MockPokeResponse::DelayedAck(Duration::from_millis(250)),
        )
        .await;
        let mut cfg = test_config(node.url());
        cfg.rpc_timeout = Duration::from_secs(1);
        cfg.reconnect_max_attempts = 10;
        cfg.reconnect_backoff_initial = Duration::from_millis(10);
        cfg.reconnect_backoff_max = Duration::from_millis(30);
        let worker = ScriptedStubWorker::new(0, vec![StubAction::SuccessImmediate]);
        let pool = Pool::new(vec![worker as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "initial enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "submission blocking stream polling").await;

        // The run loop is waiting for the delayed submission ACK. Overrun the
        // bounded effect stream while it cannot poll, forcing DATA_LOSS.
        for seed in 1_000..1_512 {
            node.publish_synth_mine_effect(seed, 0xFFFF_FFFF, 2);
        }
        wait_for_poke_count(&node.set_key_pokes, 2, "set-key poke after stream loss").await;
        wait_for_poke_count(&node.enable_pokes, 2, "enable poke after stream loss").await;

        shutdown.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), mining_task)
            .await
            .expect("miner did not stop")
            .expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn malformed_candidate_supersedes_old_work_and_stream_recovers() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        let worker = ScriptedStubWorker::new(
            0,
            vec![
                StubAction::SuccessWithPowAfterDelay {
                    pow: 100,
                    delay: Duration::from_millis(150),
                },
                StubAction::SuccessWithPowAfterDelay {
                    pow: 200,
                    delay: Duration::ZERO,
                },
            ],
        );
        let pool = Pool::new(vec![worker.clone() as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_attempt_count(&worker, 1).await;
        node.publish_malformed_mine_effect();
        node.publish_synth_mine_effect(200, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "post-malformed submission").await;

        let pokes = node.mined_pokes.lock().await;
        assert_eq!(pokes.len(), 1);
        assert_eq!(submitted_pow_payload_atom(&pokes[0]), 200);
        drop(pokes);
        assert_eq!(node.set_key_pokes.lock().await.len(), 1);

        shutdown.cancel();
        let result = mining_task.await.expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submission_nack_drops_candidate_without_reconnecting() {
        let node = MockNode::spawn().await;
        node.queue_response(MockPokeKind::Mined, MockPokeResponse::Nack)
            .await;
        let cfg = test_config(node.url());
        let worker = ScriptedStubWorker::new(
            0,
            vec![
                StubAction::SuccessWithPowAfterDelay {
                    pow: 100,
                    delay: Duration::ZERO,
                },
                StubAction::SuccessWithPowAfterDelay {
                    pow: 200,
                    delay: Duration::ZERO,
                },
            ],
        );
        let pool = Pool::new(vec![worker.clone() as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "NACKed submission").await;
        tokio::time::sleep(Duration::from_millis(75)).await;
        assert_eq!(
            worker.attempts.load(Ordering::SeqCst),
            1,
            "NACKed candidate must not be mined again"
        );

        node.publish_synth_mine_effect(200, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 2, "next-candidate submission").await;
        let pokes = node.mined_pokes.lock().await;
        assert_eq!(submitted_pow_payload_atom(&pokes[0]), 100);
        assert_eq!(submitted_pow_payload_atom(&pokes[1]), 200);
        drop(pokes);
        assert_eq!(node.set_key_pokes.lock().await.len(), 1);

        shutdown.cancel();
        let result = mining_task.await.expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn acknowledgement_timeout_reconnects_without_resubmitting_proof() {
        let node = MockNode::spawn().await;
        node.queue_response(
            MockPokeKind::Mined,
            MockPokeResponse::DelayedAck(Duration::from_millis(500)),
        )
        .await;
        let mut cfg = test_config(node.url());
        cfg.rpc_timeout = Duration::from_millis(60);
        cfg.reconnect_max_attempts = 10;
        cfg.reconnect_backoff_initial = Duration::from_millis(10);
        cfg.reconnect_backoff_max = Duration::from_millis(30);
        let worker = ScriptedStubWorker::new(
            0,
            vec![
                StubAction::SuccessWithPowAfterDelay {
                    pow: 100,
                    delay: Duration::ZERO,
                },
                StubAction::SuccessWithPowAfterDelay {
                    pow: 200,
                    delay: Duration::ZERO,
                },
            ],
        );
        let pool = Pool::new(vec![worker as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "initial enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "ack-unknown submission").await;
        wait_for_poke_count(&node.set_key_pokes, 2, "set-key poke after ack timeout").await;
        wait_for_poke_count(&node.enable_pokes, 2, "enable poke after ack timeout").await;

        node.publish_synth_mine_effect(200, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 2, "submission after ack timeout").await;
        let pokes = node.mined_pokes.lock().await;
        assert_eq!(pokes.len(), 2, "ambiguous proof must not be resubmitted");
        assert_eq!(submitted_pow_payload_atom(&pokes[0]), 100);
        assert_eq!(submitted_pow_payload_atom(&pokes[1]), 200);
        drop(pokes);

        shutdown.cancel();
        let result = mining_task.await.expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_cancels_pending_submission_ack_wait() {
        let node = MockNode::spawn().await;
        node.queue_response(
            MockPokeKind::Mined,
            MockPokeResponse::DelayedAck(Duration::from_secs(5)),
        )
        .await;
        let mut cfg = test_config(node.url());
        cfg.rpc_timeout = Duration::from_secs(2);
        let worker = ScriptedStubWorker::new(0, vec![StubAction::SuccessImmediate]);
        let pool = Pool::new(vec![worker as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 1, "pending submission").await;
        shutdown.cancel();

        let result = tokio::time::timeout(Duration::from_millis(500), mining_task)
            .await
            .expect("shutdown waited for the submission deadline")
            .expect("miner panicked");
        assert!(result.is_ok(), "unexpected miner result: {result:?}");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn repeated_worker_errors_fail_instead_of_hot_looping() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        let worker = ScriptedStubWorker::new(
            7,
            vec![StubAction::PokeError, StubAction::PokeError, StubAction::PokeError],
        );
        let pool = Pool::new(vec![worker.clone() as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let mining_task = tokio::spawn(async move { run_with_pool(cfg, pool, shutdown).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        let result = tokio::time::timeout(Duration::from_secs(2), mining_task)
            .await
            .expect("worker failure budget did not terminate miner")
            .expect("miner panicked");
        assert!(matches!(
            result,
            Err(MinerError::WorkerFailed {
                worker: 7,
                attempts: 3,
                ..
            })
        ));
        assert_eq!(worker.attempts.load(Ordering::SeqCst), 3);
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn worker_panic_is_attributed_and_fatal() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        let worker = ScriptedStubWorker::new(9, vec![StubAction::Panic]);
        let pool = Pool::new(vec![worker as Arc<dyn Worker>]);
        let shutdown = CancellationToken::new();
        let mining_task = tokio::spawn(async move { run_with_pool(cfg, pool, shutdown).await });

        assert_node_received_pkh_only_set_key(&node).await;
        wait_for_poke_count(&node.enable_pokes, 1, "enable poke").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        let result = tokio::time::timeout(Duration::from_secs(2), mining_task)
            .await
            .expect("worker panic did not terminate miner")
            .expect("run loop task panicked");
        assert!(matches!(
            result,
            Err(MinerError::WorkerFailed {
                worker: 9,
                attempts: 1,
                ..
            })
        ));
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stopping_one_miner_does_not_disable_shared_node_mining() {
        let node = MockNode::spawn().await;
        let first_worker = ScriptedStubWorker::new(
            1,
            vec![StubAction::SuccessWithPowAfterDelay {
                pow: 101,
                delay: Duration::ZERO,
            }],
        );
        let second_worker = ScriptedStubWorker::new(
            2,
            vec![
                StubAction::SuccessWithPowAfterDelay {
                    pow: 201,
                    delay: Duration::ZERO,
                },
                StubAction::SuccessWithPowAfterDelay {
                    pow: 202,
                    delay: Duration::ZERO,
                },
            ],
        );
        let first_pool = Pool::new(vec![first_worker as Arc<dyn Worker>]);
        let second_pool = Pool::new(vec![second_worker as Arc<dyn Worker>]);
        let first_shutdown = CancellationToken::new();
        let second_shutdown = CancellationToken::new();
        let first_task = {
            let cfg = test_config(node.url());
            let shutdown = first_shutdown.clone();
            tokio::spawn(async move { run_with_pool(cfg, first_pool, shutdown).await })
        };
        let second_task = {
            let cfg = test_config(node.url());
            let shutdown = second_shutdown.clone();
            tokio::spawn(async move { run_with_pool(cfg, second_pool, shutdown).await })
        };

        wait_for_poke_count(&node.set_key_pokes, 2, "two set-key pokes").await;
        wait_for_poke_count(&node.enable_pokes, 2, "two enable pokes").await;
        node.publish_synth_mine_effect(100, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 2, "two first-candidate submissions").await;

        first_shutdown.cancel();
        let first_result = first_task.await.expect("first miner panicked");
        assert!(
            first_result.is_ok(),
            "unexpected first miner result: {first_result:?}"
        );
        {
            let enables = node.enable_pokes.lock().await;
            assert!(
                enables.iter().all(enable_poke_value),
                "a miner shutdown must not send global enable-mining(false)"
            );
        }

        node.publish_synth_mine_effect(200, 0xFFFF_FFFF, 2);
        wait_for_poke_count(&node.mined_pokes, 3, "surviving miner submission").await;
        assert_eq!(
            submitted_pow_payload_atom(node.mined_pokes.lock().await.last().unwrap()),
            202
        );

        second_shutdown.cancel();
        let second_result = second_task.await.expect("second miner panicked");
        assert!(
            second_result.is_ok(),
            "unexpected second miner result: {second_result:?}"
        );
        assert!(node.enable_pokes.lock().await.iter().all(enable_poke_value));
        node.shutdown().await;
    }
}
