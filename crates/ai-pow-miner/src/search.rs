//! Bounded, ordered batch search backends for AI-PoW ticket work.
//!
//! A backend receives only prepared, fully validated work and returns at most
//! the lowest winning ordinal and jackpot. The caller owns target classification,
//! checked-ticket reconstruction, and certificate construction.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use ai_pow::pearl_compat::{
    PearlCompatError, PreparedPearlPatternJob, PreparedPearlPatternScratch,
};
use ai_pow::tile_hash::hash_le_target;
use thiserror::Error;

#[cfg(feature = "node")]
use crate::canonical::{PreparedCanonicalMoeScratch, PreparedCanonicalMoeTemplate};

/// Maximum logical attempts submitted to a backend at once.
///
/// Cancellation and deadlines are observed between batches, so this bounds
/// their latency independently of a backend's throughput.
pub const DEFAULT_SEARCH_BATCH_ATTEMPTS: u64 = 256;

/// Interval between production search-throughput reports.
pub const THROUGHPUT_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Number of adjacent ticket ordinals assigned to a worker at a time.
///
/// Each attempt has substantial BLAKE3 and matrix work; small contiguous chunks
/// preserve locality while allowing the fixed worker pool to balance tails.
const CPU_SEARCH_CHUNK_ATTEMPTS: u64 = 8;

/// One contiguous, ordered search range with an inclusive effective threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBatch {
    pub start: u64,
    pub len: u64,
    pub threshold: [u8; 32],
}

impl SearchBatch {
    pub fn new(start: u64, len: u64, threshold: [u8; 32]) -> Result<Self, SearchBackendError> {
        if len == 0 {
            return Err(SearchBackendError::EmptyBatch);
        }
        start
            .checked_add(len)
            .ok_or(SearchBackendError::OrdinalRangeOverflow)?;
        Ok(Self {
            start,
            len,
            threshold,
        })
    }

    pub fn end_exclusive(self) -> u64 {
        self.start + self.len
    }
}

/// The lowest ticket ordinal accepted by a backend, with its exact hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchWinner {
    pub ordinal: u64,
    pub jackpot_hash: [u8; 32],
}

/// Terminal condition for an ordered search schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScheduleEnd {
    BudgetExhausted { max: u64 },
    AttemptSpaceExhausted,
}

/// Logical accounting for a contiguous attempt space.
///
/// Work inside one emitted batch may complete out of order, but the scheduler
/// reports attempts only through the lowest returned winner.
#[derive(Debug, Clone)]
pub struct OrderedBatchScheduler {
    start: u64,
    next: u64,
    end_exclusive: u64,
    max_attempts: Option<u64>,
    batch_attempts: u64,
}

impl OrderedBatchScheduler {
    pub fn new(
        start: u64,
        end_exclusive: u64,
        max_attempts: Option<u64>,
        batch_attempts: u64,
    ) -> Result<Self, SearchBackendError> {
        if batch_attempts == 0 {
            return Err(SearchBackendError::EmptyBatch);
        }
        Ok(Self {
            start,
            next: start,
            end_exclusive,
            max_attempts,
            batch_attempts,
        })
    }

    pub fn attempts_tried(&self) -> u64 {
        self.next - self.start
    }

    pub fn next_batch(&self, threshold: [u8; 32]) -> Result<SearchBatch, SearchScheduleEnd> {
        if let Some(max) = self.max_attempts {
            if self.attempts_tried() >= max {
                return Err(SearchScheduleEnd::BudgetExhausted { max });
            }
        }
        if self.next >= self.end_exclusive {
            return Err(SearchScheduleEnd::AttemptSpaceExhausted);
        }
        let remaining_space = self.end_exclusive - self.next;
        let remaining_budget = self
            .max_attempts
            .map(|max| max - self.attempts_tried())
            .unwrap_or(u64::MAX);
        let len = self
            .batch_attempts
            .min(remaining_space)
            .min(remaining_budget);
        Ok(SearchBatch::new(self.next, len, threshold)
            .expect("nonempty bounded scheduler range is representable"))
    }

    pub fn record_miss(&mut self, batch: SearchBatch) -> Result<(), SearchBackendError> {
        if batch.start != self.next || batch.end_exclusive() > self.end_exclusive {
            return Err(SearchBackendError::UnexpectedBatch);
        }
        self.next = batch.end_exclusive();
        Ok(())
    }

    pub fn record_winner(
        &mut self,
        batch: SearchBatch,
        winner: SearchWinner,
    ) -> Result<u64, SearchBackendError> {
        if batch.start != self.next
            || winner.ordinal < batch.start
            || winner.ordinal >= batch.end_exclusive()
        {
            return Err(SearchBackendError::WinnerOutsideBatch {
                winner: winner.ordinal,
                batch_start: batch.start,
                batch_end_exclusive: batch.end_exclusive(),
            });
        }
        self.next = winner
            .ordinal
            .checked_add(1)
            .ok_or(SearchBackendError::OrdinalRangeOverflow)?;
        Ok(self.attempts_tried())
    }
}

#[derive(Debug, Error)]
pub enum SearchBackendError {
    #[error("search batch length must be nonzero")]
    EmptyBatch,
    #[error("search batch ordinal range overflows u64")]
    OrdinalRangeOverflow,
    #[error("backend result does not correspond to the current search batch")]
    UnexpectedBatch,
    #[error(
        "backend winner ordinal {winner} is outside current batch [{batch_start}, {batch_end_exclusive})"
    )]
    WinnerOutsideBatch {
        winner: u64,
        batch_start: u64,
        batch_end_exclusive: u64,
    },
    #[error("canonical search ordinal {0} exceeds the u32 extranonce space")]
    CanonicalOrdinalOutOfRange(u64),
    #[error("dense search ordinal {0} is outside the prepared offset space")]
    DenseOrdinalOutOfRange(u64),
    #[error("prepared dense ticket evaluation: {0}")]
    DenseEvaluation(#[from] PearlCompatError),
    #[error("search backend is unavailable: {0}")]
    BackendUnavailable(String),
    #[error("CPU search worker count must be nonzero")]
    ZeroWorkerCount,
    #[error("could not spawn CPU search worker: {0}")]
    CpuWorkerSpawn(String),
    #[error("CPU search dispatch lock was poisoned")]
    DispatchPoisoned,
    #[error("CPU search worker response lock was poisoned")]
    WorkerResponsePoisoned,
    #[error("a CPU search worker stopped before reporting its batch result")]
    WorkerDisconnected,
}

/// Executes bounded ticket batches without constructing proof material.
///
/// Backends must return the least ordinal within `batch` that clears the target,
/// even when later work completes first.
pub trait SearchBackend: Send + Sync {
    fn search_dense(
        &self,
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError>;

    #[cfg(feature = "node")]
    fn search_canonical(
        &self,
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError>;

    /// Preferred upper bound for one scheduler dispatch.
    ///
    /// Backends with fixed launch geometry can override this value. The caller
    /// still observes cancellation and deadlines between dispatches.
    fn batch_attempts(&self) -> u64 {
        DEFAULT_SEARCH_BATCH_ATTEMPTS
    }
}

/// Adds lock-free throughput accounting to a search backend.
///
/// A successful dispatch contributes its full batch because CPU workers and the
/// CUDA kernel complete every attempt before returning the lowest winner. The
/// hot path performs two relaxed atomic additions per batch. A dedicated
/// reporter swaps both window counters once per interval.
pub struct MeteredSearchBackend {
    inner: Arc<dyn SearchBackend>,
    counters: Arc<ThroughputCounters>,
    _reporter: ThroughputReporter,
}

impl MeteredSearchBackend {
    pub fn new(backend: &'static str, inner: Arc<dyn SearchBackend>) -> Arc<Self> {
        let counters = Arc::new(ThroughputCounters::default());
        let reporter =
            ThroughputReporter::spawn(backend, Arc::clone(&counters), THROUGHPUT_LOG_INTERVAL);
        Arc::new(Self {
            inner,
            counters,
            _reporter: reporter,
        })
    }

    fn record(&self, attempts: u64, shape_work_factor: u128) {
        let Ok(shape_work_factor) = u64::try_from(shape_work_factor) else {
            tracing::warn!(
                shape_work_factor = %shape_work_factor,
                "AI-PoW throughput counter cannot represent shape work factor"
            );
            return;
        };
        let Some(macs) = attempts.checked_mul(shape_work_factor) else {
            tracing::warn!(attempts, shape_work_factor, "AI-PoW throughput counter overflow");
            return;
        };
        self.counters.record(attempts, macs);
    }
}

impl SearchBackend for MeteredSearchBackend {
    fn search_dense(
        &self,
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let shape_work_factor = template.config().shape_work_factor()?;
        let result = self.inner.search_dense(template, batch);
        if result.is_ok() {
            self.record(batch.len, shape_work_factor);
        }
        result
    }

    #[cfg(feature = "node")]
    fn search_canonical(
        &self,
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        let shape_work_factor = template.config().shape_work_factor()?;
        let result = self.inner.search_canonical(template, batch);
        if result.is_ok() {
            self.record(batch.len, shape_work_factor);
        }
        result
    }

    fn batch_attempts(&self) -> u64 {
        self.inner.batch_attempts()
    }
}

#[derive(Debug, Default)]
struct ThroughputCounters {
    attempts: AtomicU64,
    macs: AtomicU64,
}

impl ThroughputCounters {
    fn record(&self, attempts: u64, macs: u64) {
        self.attempts.fetch_add(attempts, Ordering::Relaxed);
        self.macs.fetch_add(macs, Ordering::Relaxed);
    }

    fn take_window(&self, elapsed: Duration) -> ThroughputWindow {
        let attempts = self.attempts.swap(0, Ordering::Relaxed);
        let macs = self.macs.swap(0, Ordering::Relaxed);
        ThroughputWindow {
            elapsed,
            attempts,
            macs,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ThroughputWindow {
    elapsed: Duration,
    attempts: u64,
    macs: u64,
}

impl ThroughputWindow {
    fn attempts_per_second(self) -> f64 {
        self.attempts as f64 / self.elapsed.as_secs_f64()
    }

    fn macs_per_second(self) -> f64 {
        self.macs as f64 / self.elapsed.as_secs_f64()
    }
}

struct ThroughputReporter {
    stop: Arc<AtomicBool>,
    thread: Thread,
    join: Option<JoinHandle<()>>,
}

impl ThroughputReporter {
    fn spawn(backend: &'static str, counters: Arc<ThroughputCounters>, interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let reporter_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("ai-pow-throughput".to_string())
            .spawn(move || {
                let mut window_started = Instant::now();
                loop {
                    thread::park_timeout(interval);
                    if reporter_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let elapsed = window_started.elapsed();
                    window_started = Instant::now();
                    let window = counters.take_window(elapsed);
                    let macs_per_second = window.macs_per_second();
                    tracing::info!(
                        target: "ai_pow_miner",
                        backend,
                        window_seconds = elapsed.as_secs_f64(),
                        window_attempts = window.attempts,
                        attempts_per_second = window.attempts_per_second(),
                        window_macs = window.macs,
                        macs_per_second,
                        tera_macs_per_second = macs_per_second / 1.0e12,
                        "AI-PoW search throughput"
                    );
                }
            })
            .expect("throughput reporter thread must spawn");
        let thread = join.thread().clone();
        Self {
            stop,
            thread,
            join: Some(join),
        }
    }
}

impl Drop for ThroughputReporter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.unpark();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Fixed CPU workers that each own their reusable scratch state.
///
/// The work receiver is private to one worker, so scratch allocation and reuse
/// need neither thread-local state nor cross-worker locking.
pub struct CpuSearchBackend {
    workers: Vec<CpuWorker>,
    next: Arc<AtomicU64>,
    dispatch: Mutex<()>,
    joins: Mutex<Vec<thread::JoinHandle<()>>>,
}

struct CpuWorker {
    command: mpsc::SyncSender<WorkerCommand>,
    result: Mutex<mpsc::Receiver<Result<Option<SearchWinner>, SearchBackendError>>>,
}

enum WorkerCommand {
    Dense {
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
        next: Arc<AtomicU64>,
    },
    #[cfg(feature = "node")]
    Canonical {
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
        next: Arc<AtomicU64>,
    },
    Shutdown,
}

impl CpuSearchBackend {
    /// Default mining parallelism: physical CPU cores, never zero.
    pub fn default_worker_count() -> usize {
        num_cpus::get_physical().max(1)
    }

    pub fn new(worker_count: usize) -> Result<Self, SearchBackendError> {
        if worker_count == 0 {
            return Err(SearchBackendError::ZeroWorkerCount);
        }
        let mut workers = Vec::with_capacity(worker_count);
        let mut joins = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (command, receiver) = mpsc::sync_channel(1);
            let (result, result_receiver) = mpsc::sync_channel(1);
            let join = thread::Builder::new()
                .name(format!("ai-pow-search-{index}"))
                .spawn(move || worker_loop(receiver, result))
                .map_err(|error| SearchBackendError::CpuWorkerSpawn(error.to_string()))?;
            workers.push(CpuWorker {
                command,
                result: Mutex::new(result_receiver),
            });
            joins.push(join);
        }
        Ok(Self {
            workers,
            next: Arc::new(AtomicU64::new(0)),
            dispatch: Mutex::new(()),
            joins: Mutex::new(joins),
        })
    }

    fn validate_batch(batch: SearchBatch) -> Result<(), SearchBackendError> {
        if batch.len == 0 {
            return Err(SearchBackendError::EmptyBatch);
        }
        batch
            .start
            .checked_add(batch.len)
            .ok_or(SearchBackendError::OrdinalRangeOverflow)?;
        Ok(())
    }

    fn collect(&self) -> Result<Option<SearchWinner>, SearchBackendError> {
        let mut winner = None;
        for worker in &self.workers {
            let candidate = worker
                .result
                .lock()
                .map_err(|_| SearchBackendError::WorkerResponsePoisoned)?
                .recv()
                .map_err(|_| SearchBackendError::WorkerDisconnected)?;
            if let Some(candidate) = candidate? {
                if winner.is_none_or(|current: SearchWinner| candidate.ordinal < current.ordinal) {
                    winner = Some(candidate);
                }
            }
        }
        Ok(winner)
    }
}

impl Default for CpuSearchBackend {
    fn default() -> Self {
        Self::new(Self::default_worker_count()).expect("physical core count is nonzero")
    }
}

impl Drop for CpuSearchBackend {
    fn drop(&mut self) {
        for worker in &self.workers {
            let _ = worker.command.send(WorkerCommand::Shutdown);
        }
        if let Ok(joins) = self.joins.get_mut() {
            for join in joins.drain(..) {
                let _ = join.join();
            }
        }
    }
}

impl SearchBackend for CpuSearchBackend {
    fn search_dense(
        &self,
        template: Arc<PreparedPearlPatternJob>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Self::validate_batch(batch)?;
        let _dispatch = self
            .dispatch
            .lock()
            .map_err(|_| SearchBackendError::DispatchPoisoned)?;
        self.next.store(batch.start, Ordering::Relaxed);
        for worker in &self.workers {
            worker
                .command
                .send(WorkerCommand::Dense {
                    template: Arc::clone(&template),
                    batch,
                    next: Arc::clone(&self.next),
                })
                .map_err(|_| SearchBackendError::WorkerDisconnected)?;
        }
        self.collect()
    }

    #[cfg(feature = "node")]
    fn search_canonical(
        &self,
        template: Arc<PreparedCanonicalMoeTemplate>,
        batch: SearchBatch,
    ) -> Result<Option<SearchWinner>, SearchBackendError> {
        Self::validate_batch(batch)?;
        let _dispatch = self
            .dispatch
            .lock()
            .map_err(|_| SearchBackendError::DispatchPoisoned)?;
        self.next.store(batch.start, Ordering::Relaxed);
        for worker in &self.workers {
            worker
                .command
                .send(WorkerCommand::Canonical {
                    template: Arc::clone(&template),
                    batch,
                    next: Arc::clone(&self.next),
                })
                .map_err(|_| SearchBackendError::WorkerDisconnected)?;
        }
        self.collect()
    }
}

fn worker_loop(
    receiver: mpsc::Receiver<WorkerCommand>,
    result: mpsc::SyncSender<Result<Option<SearchWinner>, SearchBackendError>>,
) {
    let mut dense_scratch = None;
    #[cfg(feature = "node")]
    let mut canonical_scratch = None;
    while let Ok(command) = receiver.recv() {
        match command {
            WorkerCommand::Dense {
                template,
                batch,
                next,
            } => {
                let outcome =
                    search_dense_with_scratch(&template, batch, &next, &mut dense_scratch);
                let _ = result.send(outcome);
            }
            #[cfg(feature = "node")]
            WorkerCommand::Canonical {
                template,
                batch,
                next,
            } => {
                let outcome =
                    search_canonical_with_scratch(&template, batch, &next, &mut canonical_scratch);
                let _ = result.send(outcome);
            }
            WorkerCommand::Shutdown => break,
        }
    }
}

fn search_dense_with_scratch(
    template: &PreparedPearlPatternJob,
    batch: SearchBatch,
    next: &AtomicU64,
    scratch: &mut Option<PreparedPearlPatternScratch>,
) -> Result<Option<SearchWinner>, SearchBackendError> {
    if scratch
        .as_ref()
        .is_none_or(|scratch| !template.scratch_matches(scratch))
    {
        *scratch = Some(template.scratch());
    }
    let scratch = scratch
        .as_mut()
        .expect("scratch cache is initialized above");
    let mut best = None;
    let end = batch.end_exclusive();
    loop {
        let chunk_start = next.fetch_add(CPU_SEARCH_CHUNK_ATTEMPTS, Ordering::Relaxed);
        if chunk_start >= end {
            break;
        }
        let chunk_end = chunk_start
            .saturating_add(CPU_SEARCH_CHUNK_ATTEMPTS)
            .min(end);
        for ordinal in chunk_start..chunk_end {
            let (t_rows, t_cols) = template
                .offsets_at_ordinal(ordinal)
                .ok_or(SearchBackendError::DenseOrdinalOutOfRange(ordinal))?;
            let result = template.evaluate(t_rows, t_cols, scratch)?;
            if hash_le_target(&result.jackpot_hash, &batch.threshold) {
                let winner = SearchWinner {
                    ordinal,
                    jackpot_hash: result.jackpot_hash,
                };
                if best.is_none_or(|previous: SearchWinner| winner.ordinal < previous.ordinal) {
                    best = Some(winner);
                }
            }
        }
    }
    Ok(best)
}

#[cfg(feature = "node")]
fn search_canonical_with_scratch(
    template: &PreparedCanonicalMoeTemplate,
    batch: SearchBatch,
    next: &AtomicU64,
    scratch: &mut Option<PreparedCanonicalMoeScratch>,
) -> Result<Option<SearchWinner>, SearchBackendError> {
    if scratch
        .as_ref()
        .is_none_or(|scratch| !template.scratch_matches(scratch))
    {
        *scratch = Some(template.scratch());
    }
    let scratch = scratch
        .as_mut()
        .expect("scratch cache is initialized above");
    let mut best = None;
    let end = batch.end_exclusive();
    loop {
        let chunk_start = next.fetch_add(CPU_SEARCH_CHUNK_ATTEMPTS, Ordering::Relaxed);
        if chunk_start >= end {
            break;
        }
        let chunk_end = chunk_start
            .saturating_add(CPU_SEARCH_CHUNK_ATTEMPTS)
            .min(end);
        for ordinal in chunk_start..chunk_end {
            let extranonce = u32::try_from(ordinal)
                .map_err(|_| SearchBackendError::CanonicalOrdinalOutOfRange(ordinal))?;
            let result = template.evaluate(extranonce, scratch);
            if hash_le_target(&result.jackpot_hash, &batch.threshold) {
                let winner = SearchWinner {
                    ordinal,
                    jackpot_hash: result.jackpot_hash,
                };
                if best.is_none_or(|previous: SearchWinner| winner.ordinal < previous.ordinal) {
                    best = Some(winner);
                }
            }
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_window_reports_attempt_and_mac_rates() {
        let counters = ThroughputCounters::default();
        counters.record(2_000, 131_072_000);
        let window = counters.take_window(Duration::from_secs(2));
        assert_eq!(window.attempts, 2_000);
        assert_eq!(window.macs, 131_072_000);
        assert_eq!(window.attempts_per_second(), 1_000.0);
        assert_eq!(window.macs_per_second(), 65_536_000.0);

        let empty = counters.take_window(Duration::from_secs(1));
        assert_eq!(empty.attempts, 0);
        assert_eq!(empty.macs, 0);
    }

    #[test]
    fn scheduler_caps_budget_across_batches_and_counts_through_winner() {
        let mut scheduler =
            OrderedBatchScheduler::new(5, 1_000, Some(257), 256).expect("valid scheduler");
        let first = scheduler.next_batch([0; 32]).expect("first batch");
        assert_eq!((first.start, first.len), (5, 256));
        scheduler.record_miss(first).expect("first miss");
        let final_batch = scheduler.next_batch([0; 32]).expect("final batch");
        assert_eq!((final_batch.start, final_batch.len), (261, 1));
        scheduler.record_miss(final_batch).expect("final miss");
        assert_eq!(scheduler.attempts_tried(), 257);
        assert_eq!(
            scheduler.next_batch([0; 32]),
            Err(SearchScheduleEnd::BudgetExhausted { max: 257 })
        );

        let mut winner_scheduler =
            OrderedBatchScheduler::new(40, 1_000, None, 64).expect("valid scheduler");
        let batch = winner_scheduler.next_batch([0; 32]).expect("winner batch");
        let attempts = winner_scheduler
            .record_winner(
                batch,
                SearchWinner {
                    ordinal: 43,
                    jackpot_hash: [0; 32],
                },
            )
            .expect("winner belongs to batch");
        assert_eq!(attempts, 4);
    }
}
