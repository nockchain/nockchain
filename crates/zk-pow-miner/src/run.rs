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
//!       `update-candidate-block` then emits the first `%mine` effect
//!       on the now-active stream.
//! 3. Inner loop (select):
//!    - shutdown → cancel pool + best-effort `enable-mining(false)` + exit
//!    - new candidate → `cancel_all()` + remember as current
//!      + `dispatch_to_idle` (cancelled workers re-dispatch on
//!      their cancelled-result tick)
//!    - worker result → submit if success / re-dispatch on current
//!      with the returned nonce (Retry) or a fresh one (Success / Err)
//! 4. Stream drop → outer loop reconnects + re-configures + re-subscribes.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nockapp::nockapp::wire::Wire;
use nockchain_mining_common::{MiningCandidate, MiningPkhConfig, NodeClient};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::pool::Pool;
use crate::wire::ZkPowMinerWire;
use crate::worker::{build_candidate_poke, random_nonce, MineResult, SerfWorker, Worker};

#[derive(Debug, Clone)]
pub struct MinerConfig {
    /// `http://127.0.0.1:5555` by default.
    pub node_addr: String,
    /// v1 pubkey-hash reward configs. Required.
    pub mining_pkh_configs: Vec<MiningPkhConfig>,
    /// Worker pool size.
    pub num_threads: u64,
    pub reconnect_backoff_initial: Duration,
    pub reconnect_backoff_max: Duration,
    pub reconnect_max_attempts: u32,
}

impl MinerConfig {
    /// Convenience builder with safe defaults: required v1 mining-pkh configs,
    /// num_cpus-1 threads (min 1), 1s→30s backoff, 5 retries.
    pub fn new(node_addr: String, mining_pkh_configs: Vec<MiningPkhConfig>) -> Self {
        let num_threads = num_cpus::get().saturating_sub(1).max(1) as u64;
        Self {
            node_addr,
            mining_pkh_configs,
            num_threads,
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
    #[error("gave up after {count} consecutive connect attempts")]
    TooManyReconnects { count: u32 },
    #[error("candidate decode failed: {0}")]
    CandidateDecode(String),
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
        // Order matters: subscribe to %mine effects BEFORE enabling mining,
        // so the initial candidate (which the post-poke
        // update-candidate-block emits on enable) lands on a live stream.
        if let Err(e) = client
            .set_mining_key(
                ZkPowMinerWire::SetPubKey.to_wire(),
                Vec::new(),
                cfg.mining_pkh_configs.clone(),
            )
            .await
        {
            return Err(MinerError::Configure(format!("set_mining_key: {e}")));
        }
        let mut candidates = match client.watch_candidates(vec![b"mine-zk".to_vec()]).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "watch_candidates failed; reconnect");
                consecutive_failures += 1;
                continue;
            }
        };
        if let Err(e) = client
            .enable_mining(ZkPowMinerWire::Enable.to_wire(), true)
            .await
        {
            return Err(MinerError::Configure(format!("enable_mining(true): {e}")));
        }
        info!("zk-pow-miner: subscribed + mining enabled; awaiting candidates");

        // ── inner loop ──
        // current_candidate is local — never shared across threads — so we
        // sidestep the MiningCandidate:!Sync problem entirely. Each select
        // branch borrows it briefly and releases before any await on
        // `client` or `pool.next_result`.
        let mut current_candidate: Option<MiningCandidate> = None;
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
                    info!(pow_len = candidate.pow_len, "new candidate; supersede + dispatch fresh");
                    pool.cancel_all();
                    current_candidate = Some(candidate);
                    let cur = current_candidate.as_ref().expect("just-stored");
                    pool.dispatch_to_idle(|| build_candidate_poke(cur, random_nonce()));
                }
                Some((wid, r)) = pool.next_result(), if pool.busy_count() > 0 => {
                    let Some(ref cur) = current_candidate else {
                        debug!(worker = wid, "result with no current candidate; idle");
                        continue;
                    };
                    match r {
                        Ok(MineResult::Success { poke_slab, .. }) => {
                            // Build the respawn poke FIRST so `cur` is no
                            // longer borrowed before we await on the client.
                            let respawn_poke = build_candidate_poke(cur, random_nonce());
                            info!(worker = wid, "found a block; submitting via gRPC");
                            if let Err(e) = client
                                .submit_mined_block(ZkPowMinerWire::Mined.to_wire(), poke_slab)
                                .await
                            {
                                warn!(worker = wid, error = %e, "submit_mined_block failed (likely stale candidate)");
                            }
                            pool.dispatch_one(wid, respawn_poke);
                        }
                        Ok(MineResult::Retry { next_nonce }) => {
                            let respawn_poke = build_candidate_poke(cur, next_nonce);
                            pool.dispatch_one(wid, respawn_poke);
                        }
                        Err(e) => {
                            debug!(worker = wid, error = %e, "worker error; respawning on current");
                            let respawn_poke = build_candidate_poke(cur, random_nonce());
                            pool.dispatch_one(wid, respawn_poke);
                        }
                    }
                }
            }
        };

        // ── cleanup before reconnect or exit ──
        pool.cancel_all();
        while pool.busy_count() > 0 {
            let _ = pool.next_result().await;
        }
        let _ = client
            .enable_mining(ZkPowMinerWire::Enable.to_wire(), false)
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

enum InnerOutcome {
    Shutdown,
    StreamLost,
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
    //! synthetic `%mine` effects, and assert the miner pokes
    //! `ZkPowMinerWire::Mined` back at the server within a tight timeout.

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

    /// Build a `NockAppHandle` from raw channels (no real kernel). The
    /// caller drains `action_rx` to observe pokes. The returned
    /// `effect_tx` is the bus the test publishes synthetic effects on.
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
            // Bind ephemeral port; drop the listener; reuse the port for tonic.
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let addr = listener.local_addr().expect("local_addr");
            drop(listener);
            // Channels.
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
            // Drain actions; collect ZkPowMinerWire::Mined slabs.
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
                            if wire.source == ZkPowMinerWire::SOURCE {
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
                        IOAction::Peek { .. } => {
                            // unused in these tests
                        }
                    }
                }
            });
            // Spawn server.
            let server = PrivateNockAppGrpcServer::new(handle);
            let server_task = tokio::spawn(async move { server.serve(addr).await });
            // Give server a moment to come up.
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
            self.effect_tx.send(slab).expect("publish %mine effect");
        }

        async fn shutdown(self) {
            self.server_task.abort();
            self.action_drainer.abort();
            let _ = self.server_task.await;
            let _ = self.action_drainer.await;
        }
    }

    // ── StubWorker for run-loop tests ──
    enum StubAction {
        SuccessImmediate,
        WaitForCancel,
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
                StubAction::SuccessImmediate => {
                    let mut hash_slab = NounSlab::new();
                    hash_slab.set_root(D(0));
                    let mut poke_slab = NounSlab::new();
                    // Construct a small fake `%pow` poke noun: `[%command %pow 42]`.
                    let cmd = T(
                        &mut poke_slab,
                        &[D(tas!(b"command")), D(tas!(b"pow")), D(42)],
                    );
                    poke_slab.set_root(cmd);
                    Ok(MineResult::Success {
                        hash_slab,
                        poke_slab,
                    })
                }
                StubAction::WaitForCancel => {
                    while self.cancels.load(Ordering::SeqCst) == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(WorkerError::Poke("stub cancelled".into()))
                }
            }
        }
    }

    fn test_config(node_addr: String) -> MinerConfig {
        use nockchain_mining_common::MiningPkhConfig;
        MinerConfig {
            node_addr,
            mining_pkh_configs: vec![MiningPkhConfig {
                share: 1,
                pkh: "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string(),
            }],
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
    async fn run_loop_exits_cleanly_on_shutdown() {
        let node = MockNode::spawn().await;
        let cfg = test_config(node.url());
        // Worker that hangs forever on cancel.
        let worker = ScriptedStubWorker::new(0, vec![StubAction::WaitForCancel]);
        let workers: Vec<Arc<dyn Worker>> = vec![worker.clone()];
        let pool = Pool::new(workers);
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let mining_task =
            tokio::spawn(async move { run_with_pool(cfg, pool, shutdown_clone).await });
        // Let miner connect + configure + start its first attempt.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Push a candidate so the pool actually dispatches.
        node.publish_synth_mine_effect(50, 0xFFFF_FFFF, 2);
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Now cancel — miner should drain + return Ok.
        shutdown.cancel();
        let r = tokio::time::timeout(Duration::from_secs(3), mining_task)
            .await
            .expect("miner did not exit within 3s")
            .expect("miner panicked");
        assert!(r.is_ok(), "expected clean Ok on shutdown, got {r:?}");
        node.shutdown().await;
    }
}
