//! Worker pool — fans candidate attempts out across N [`Worker`]s and
//! collects results.
//!
//! The pool owns its workers (as `Arc<dyn Worker>`). The run loop
//! drives the pool by:
//! 1. On a new candidate, `dispatch_to_idle(...)` to spawn one attempt
//!    per currently-idle worker.
//! 2. On `next_result()` returning, decide what to do based on the
//!    `MineResult` (success → submit + respawn the worker;
//!    retry → respawn with the returned nonce; error → respawn fresh).
//! 3. On a superseding candidate, `cancel_all()` to signal each
//!    in-flight attempt to abort; the resulting (error-tagged) results
//!    surface through `next_result()` like any other.
//!
//! The pool does not own the *current* candidate — that's the run loop's
//! concern. The pool just dispatches pre-built poke slabs and gives
//! results back.

use std::collections::HashSet;
use std::sync::Arc;

use nockapp::noun::slab::NounSlab;
use tokio::task::JoinSet;

use crate::worker::{MineResult, Worker, WorkerError, WorkerId};

pub struct Pool {
    workers: Vec<Arc<dyn Worker>>,
    attempts: JoinSet<(WorkerId, Result<MineResult, WorkerError>)>,
    busy: HashSet<WorkerId>,
}

impl Pool {
    pub fn new(workers: Vec<Arc<dyn Worker>>) -> Self {
        // Defensive: ids should be unique. If not, dispatch_to_idle's
        // bookkeeping breaks. Check at construction.
        let mut seen = HashSet::new();
        for w in &workers {
            assert!(
                seen.insert(w.id()),
                "Pool::new: duplicate worker id {}",
                w.id()
            );
        }
        Self {
            workers,
            attempts: JoinSet::new(),
            busy: HashSet::new(),
        }
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn busy_count(&self) -> usize {
        self.busy.len()
    }

    pub fn idle_count(&self) -> usize {
        self.workers.len() - self.busy.len()
    }

    /// Dispatch one attempt to each idle worker. The caller supplies a
    /// `build_poke` closure that produces a fresh poke per dispatch
    /// (each invocation typically generates a new random nonce).
    pub fn dispatch_to_idle<F>(&mut self, mut build_poke: F)
    where
        F: FnMut() -> NounSlab,
    {
        // Collect ids first to avoid borrowing self.workers while
        // we mutate self.busy.
        let idle_ids: Vec<WorkerId> = self
            .workers
            .iter()
            .filter(|w| !self.busy.contains(&w.id()))
            .map(|w| w.id())
            .collect();
        for id in idle_ids {
            let poke = build_poke();
            self.spawn_attempt(id, poke);
        }
    }

    /// Dispatch one attempt onto the specified worker. No-op if the
    /// worker is already busy or the id is unknown.
    pub fn dispatch_one(&mut self, id: WorkerId, poke: NounSlab) {
        if self.busy.contains(&id) {
            tracing::warn!(
                worker_id = id,
                "dispatch_one: worker already busy; skipping"
            );
            return;
        }
        if !self.workers.iter().any(|w| w.id() == id) {
            tracing::warn!(worker_id = id, "dispatch_one: unknown worker id; skipping");
            return;
        }
        self.spawn_attempt(id, poke);
    }

    fn spawn_attempt(&mut self, id: WorkerId, poke: NounSlab) {
        let worker = self
            .workers
            .iter()
            .find(|w| w.id() == id)
            .expect("worker id checked")
            .clone();
        self.busy.insert(id);
        self.attempts.spawn(async move {
            let result = worker.mine_attempt(poke).await;
            (id, result)
        });
    }

    /// Signal every worker to abort its current attempt. Outstanding
    /// attempts will resolve shortly with `Err(WorkerError::Poke(...))`;
    /// the caller picks them up via [`next_result`] and decides whether
    /// to respawn.
    pub fn cancel_all(&self) {
        for w in &self.workers {
            w.cancel();
        }
    }

    /// Wait for the next attempt result. Returns `None` when no
    /// attempts are in flight (the pool is fully idle). The
    /// corresponding worker is marked idle before the result is
    /// returned, so the caller can immediately re-dispatch on that id.
    pub async fn next_result(&mut self) -> Option<(WorkerId, Result<MineResult, WorkerError>)> {
        let joined = self.attempts.join_next().await?;
        let (id, result) = joined.expect("worker task panicked");
        self.busy.remove(&id);
        Some((id, result))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use nockvm::noun::D;

    use super::*;
    use crate::worker::{MineResult, Worker, WorkerError, WorkerId};

    /// Fake worker for unit tests. Holds a queue of pre-set responses;
    /// each `mine_attempt` consumes one entry. `cancel` increments a
    /// counter — tests check it.
    struct StubWorker {
        id: WorkerId,
        responses: Mutex<Vec<StubResponse>>,
        attempts: AtomicU64,
        cancels: AtomicU64,
        /// Optional sleep before resolving; lets supersede-cancel tests
        /// race a cancel against an in-flight attempt.
        attempt_delay: Duration,
    }

    enum StubResponse {
        SuccessImmediate,
        RetryImmediate,
        Error,
        /// Resolve only after the cancel signal is observed (the
        /// stub spins on `cancels` until > 0, then returns `Error`).
        WaitForCancel,
    }

    impl StubWorker {
        fn new(id: WorkerId, responses: Vec<StubResponse>) -> Arc<Self> {
            Arc::new(Self {
                id,
                responses: Mutex::new(responses),
                attempts: AtomicU64::new(0),
                cancels: AtomicU64::new(0),
                attempt_delay: Duration::ZERO,
            })
        }

        fn new_with_delay(
            id: WorkerId,
            responses: Vec<StubResponse>,
            delay: Duration,
        ) -> Arc<Self> {
            Arc::new(Self {
                id,
                responses: Mutex::new(responses),
                attempts: AtomicU64::new(0),
                cancels: AtomicU64::new(0),
                attempt_delay: delay,
            })
        }

        fn attempt_count(&self) -> u64 {
            self.attempts.load(Ordering::SeqCst)
        }

        fn cancel_count(&self) -> u64 {
            self.cancels.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Worker for StubWorker {
        fn id(&self) -> WorkerId {
            self.id
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
        async fn mine_attempt(&self, _poke: NounSlab) -> Result<MineResult, WorkerError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let resp = {
                let mut q = self.responses.lock().unwrap();
                if q.is_empty() {
                    StubResponse::Error
                } else {
                    q.remove(0)
                }
            };
            // Optional artificial delay (for cancel-races).
            if !self.attempt_delay.is_zero() {
                tokio::time::sleep(self.attempt_delay).await;
            }
            match resp {
                StubResponse::SuccessImmediate => {
                    let mut hash_slab = NounSlab::new();
                    hash_slab.set_root(D(0));
                    let mut poke_slab = NounSlab::new();
                    poke_slab.set_root(D(0));
                    Ok(MineResult::Success {
                        hash_slab,
                        poke_slab,
                    })
                }
                StubResponse::RetryImmediate => {
                    let mut next_nonce = NounSlab::new();
                    next_nonce.set_root(D(0));
                    Ok(MineResult::Retry { next_nonce })
                }
                StubResponse::Error => Err(WorkerError::Poke("stub error".into())),
                StubResponse::WaitForCancel => {
                    // Poll cancel flag until set, then return error.
                    while self.cancels.load(Ordering::SeqCst) == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(WorkerError::Poke("stub cancelled".into()))
                }
            }
        }
    }

    fn dummy_poke() -> NounSlab {
        let mut s = NounSlab::new();
        s.set_root(D(0));
        s
    }

    #[tokio::test]
    async fn new_pool_starts_idle() {
        let workers: Vec<Arc<dyn Worker>> =
            vec![StubWorker::new(1, vec![]), StubWorker::new(2, vec![])];
        let pool = Pool::new(workers);
        assert_eq!(pool.worker_count(), 2);
        assert_eq!(pool.busy_count(), 0);
        assert_eq!(pool.idle_count(), 2);
    }

    #[tokio::test]
    async fn dispatch_to_idle_spawns_one_per_worker() {
        let w1 = StubWorker::new(1, vec![StubResponse::SuccessImmediate]);
        let w2 = StubWorker::new(2, vec![StubResponse::SuccessImmediate]);
        let workers: Vec<Arc<dyn Worker>> = vec![w1.clone(), w2.clone()];
        let mut pool = Pool::new(workers);
        pool.dispatch_to_idle(dummy_poke);
        assert_eq!(pool.busy_count(), 2);
        let _r1 = pool.next_result().await.expect("result 1");
        let _r2 = pool.next_result().await.expect("result 2");
        assert_eq!(pool.busy_count(), 0);
        assert_eq!(w1.attempt_count(), 1);
        assert_eq!(w2.attempt_count(), 1);
    }

    #[tokio::test]
    async fn dispatch_skips_busy_workers() {
        let w1 = StubWorker::new(
            1,
            vec![StubResponse::WaitForCancel, StubResponse::SuccessImmediate],
        );
        let workers: Vec<Arc<dyn Worker>> = vec![w1.clone()];
        let mut pool = Pool::new(workers);
        pool.dispatch_to_idle(dummy_poke);
        assert_eq!(pool.busy_count(), 1);
        // Second dispatch should NOT spawn a duplicate (worker is busy).
        pool.dispatch_to_idle(dummy_poke);
        assert_eq!(pool.busy_count(), 1, "duplicate dispatch was suppressed");
        // Cancel + drain.
        pool.cancel_all();
        let _ = pool.next_result().await.expect("result");
        assert_eq!(pool.busy_count(), 0);
        assert_eq!(w1.attempt_count(), 1);
    }

    #[tokio::test]
    async fn cancel_all_signals_every_worker_once() {
        let w1 = StubWorker::new(1, vec![StubResponse::WaitForCancel]);
        let w2 = StubWorker::new(2, vec![StubResponse::WaitForCancel]);
        let workers: Vec<Arc<dyn Worker>> = vec![w1.clone(), w2.clone()];
        let mut pool = Pool::new(workers);
        pool.dispatch_to_idle(dummy_poke);
        assert_eq!(pool.busy_count(), 2);
        pool.cancel_all();
        // Drain both results.
        let _ = pool.next_result().await.expect("r1");
        let _ = pool.next_result().await.expect("r2");
        assert_eq!(pool.busy_count(), 0);
        assert_eq!(w1.cancel_count(), 1);
        assert_eq!(w2.cancel_count(), 1);
    }

    #[tokio::test]
    async fn dispatch_one_respawns_after_result() {
        let w1 = StubWorker::new(
            1,
            vec![StubResponse::RetryImmediate, StubResponse::SuccessImmediate],
        );
        let workers: Vec<Arc<dyn Worker>> = vec![w1.clone()];
        let mut pool = Pool::new(workers);
        pool.dispatch_to_idle(dummy_poke);
        // First result is the Retry.
        let (id, r) = pool.next_result().await.expect("first result");
        assert_eq!(id, 1);
        assert!(matches!(r, Ok(MineResult::Retry { .. })));
        assert_eq!(pool.busy_count(), 0);
        // Respawn the same worker.
        pool.dispatch_one(1, dummy_poke());
        assert_eq!(pool.busy_count(), 1);
        // Second result is the Success.
        let (id, r) = pool.next_result().await.expect("second result");
        assert_eq!(id, 1);
        assert!(matches!(r, Ok(MineResult::Success { .. })));
        assert_eq!(w1.attempt_count(), 2);
    }

    #[tokio::test]
    async fn next_result_yields_none_when_idle() {
        let workers: Vec<Arc<dyn Worker>> = vec![StubWorker::new(1, vec![])];
        let mut pool = Pool::new(workers);
        // No attempts in flight.
        assert!(pool.next_result().await.is_none());
    }

    #[tokio::test]
    async fn supersede_cancels_then_redispatches() {
        // Worker takes a moment per attempt so we can race the cancel.
        let w1 = StubWorker::new_with_delay(
            1,
            vec![
                StubResponse::WaitForCancel,    // first attempt: will be cancelled
                StubResponse::SuccessImmediate, // second attempt: succeeds
            ],
            Duration::from_millis(0), // WaitForCancel polls cancel flag itself
        );
        let workers: Vec<Arc<dyn Worker>> = vec![w1.clone()];
        let mut pool = Pool::new(workers);
        // Dispatch the first attempt; it will hang on WaitForCancel.
        pool.dispatch_to_idle(dummy_poke);
        assert_eq!(pool.busy_count(), 1);
        // Simulate supersede: cancel + drain the resulting (error) result.
        pool.cancel_all();
        let (id, r) = pool.next_result().await.expect("cancelled result");
        assert_eq!(id, 1);
        assert!(r.is_err(), "cancelled attempt should surface as Err");
        assert_eq!(pool.busy_count(), 0);
        // Dispatch on the supersede candidate.
        pool.dispatch_to_idle(dummy_poke);
        let (id, r) = pool.next_result().await.expect("second result");
        assert_eq!(id, 1);
        assert!(matches!(r, Ok(MineResult::Success { .. })));
        assert_eq!(w1.attempt_count(), 2);
        assert_eq!(w1.cancel_count(), 1);
    }

    #[test]
    #[should_panic(expected = "duplicate worker id")]
    fn duplicate_ids_panic_at_construction() {
        let workers: Vec<Arc<dyn Worker>> =
            vec![StubWorker::new(1, vec![]), StubWorker::new(1, vec![])];
        let _pool = Pool::new(workers);
    }
}
