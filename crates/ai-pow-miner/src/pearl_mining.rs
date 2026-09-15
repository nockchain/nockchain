//! Pearl-compatible merge-mining ticket loop.
//!
//! One iteration evaluates one explicit Pearl-valid `t_rows` / `t_cols` ticket
//! attempt and returns after the shared Pearl jackpot digest clears either the
//! Pearl target or the Nockchain target. Recursive proof construction and
//! canonical `%ai-pow` artifact submission happen only after a Nockchain hit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ai_pow::params::MatmulParams;
#[cfg(test)]
use ai_pow::pearl_compat::PearlPeriodicPattern;
use ai_pow::pearl_compat::{
    evaluate_pearl_merge_checked_ticket_attempt, prepare_pearl_pattern_job, PearlCompatError,
    PearlIncompleteBlockHeader, PearlMergeCheckedTicketAttempt, PearlMiningConfig,
    PearlNockchainAux, PearlPublicProofParams,
};
use ai_pow::tile_hash::hash_le_target;

use crate::search::{
    CpuSearchBackend, OrderedBatchScheduler, SearchBackend, SearchBackendError, SearchScheduleEnd,
};
use crate::{DifficultyTarget, MiningCancel, MiningStats};

/// One Pearl-compatible merge-mining job.
pub struct PearlMergeMiningJob<'a> {
    pub header: &'a PearlIncompleteBlockHeader,
    pub config: &'a PearlMiningConfig,
    pub params: &'a MatmulParams,
    pub nockchain_target: DifficultyTarget,
    pub a: &'a [i8],
    pub b: &'a [i8],
    pub max_pattern_len: usize,
    pub aux: PearlNockchainAux,
}

/// Mining-loop tuning for Pearl-compatible ticket attempts.
#[derive(Clone, Debug)]
pub struct PearlMergeMineOptions {
    /// Start at this linear position in the lexicographic list of Pearl-valid
    /// `(t_rows, t_cols)` pairs.
    pub attempt_start: u64,
    /// Stop after this many ticket attempts. `None` scans all valid pairs.
    pub max_attempts: Option<u64>,
    pub deadline: Option<Instant>,
    pub progress_interval: Option<Duration>,
}

impl Default for PearlMergeMineOptions {
    fn default() -> Self {
        Self {
            attempt_start: 0,
            max_attempts: None,
            deadline: None,
            progress_interval: Some(Duration::from_secs(2)),
        }
    }
}

/// Returned on a successful Pearl-compatible shared ticket.
#[derive(Debug, Clone)]
pub struct PearlMergeMinedTicket {
    pub attempt: PearlMergeCheckedTicketAttempt,
    pub pearl_target_hit: bool,
    pub nockchain_target_hit: bool,
    pub stats: MiningStats,
}

#[derive(thiserror::Error, Debug)]
pub enum PearlMergeMiningError {
    #[error("Pearl merge mining statement: {0}")]
    Pearl(#[from] PearlCompatError),
    #[error("cancelled by caller")]
    Cancelled,
    #[error("deadline elapsed without a solution")]
    DeadlineElapsed,
    #[error("ticket-attempt budget exhausted ({max} attempts)")]
    BudgetExhausted { max: u64 },
    #[error("Pearl-valid ticket offset space exhausted")]
    AttemptSpaceExhausted,
    #[error("search backend: {0}")]
    Backend(#[from] SearchBackendError),
    #[error("search backend reported a winner that fails both effective targets")]
    BackendWinnerTargetMismatch,
    #[error("recursive certificate build failed: {0}")]
    CertificateBuild(String),
}

/// Run Pearl-compatible ticket mining.
///
/// Every counted attempt evaluates exactly one `(t_rows, t_cols)` pair. Misses
/// return no proof artifact and no recursive certificate work is performed here.
/// Run Pearl-compatible ticket mining with the private dedicated CPU backend.
pub fn run(
    job: &PearlMergeMiningJob<'_>,
    opts: &PearlMergeMineOptions,
    cancel: MiningCancel,
) -> Result<PearlMergeMinedTicket, PearlMergeMiningError> {
    let backend = CpuSearchBackend::default();
    run_with_backend(job, opts, cancel, &backend)
}

/// Run Pearl-compatible ticket mining through a caller-owned bounded backend.
///
/// The backend receives only the prepared ticket evaluator and a maximum target.
/// Every returned winner is reconstructed through the checked scalar oracle
/// before its target class or recursive-certificate path is observable.
pub fn run_with_backend(
    job: &PearlMergeMiningJob<'_>,
    opts: &PearlMergeMineOptions,
    cancel: MiningCancel,
    backend: &dyn SearchBackend,
) -> Result<PearlMergeMinedTicket, PearlMergeMiningError> {
    if cancel.is_cancelled() {
        return Err(PearlMergeMiningError::Cancelled);
    }
    if let Some(deadline) = opts.deadline {
        if Instant::now() >= deadline {
            return Err(PearlMergeMiningError::DeadlineElapsed);
        }
    }
    let prepared = Arc::new(prepare_pearl_pattern_job(
        job.header, job.config, job.params, job.a, job.b, job.max_pattern_len,
    )?);
    let total_attempts = u64::try_from(prepared.row_offsets().len())
        .map_err(|_| PearlMergeMiningError::AttemptSpaceExhausted)?
        .checked_mul(
            u64::try_from(prepared.col_offsets().len())
                .map_err(|_| PearlMergeMiningError::AttemptSpaceExhausted)?,
        )
        .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
    let mut scheduler = OrderedBatchScheduler::new(
        opts.attempt_start,
        total_attempts,
        opts.max_attempts,
        backend.batch_attempts(),
    )?;
    let started = Instant::now();
    let mut last_progress = started;

    loop {
        if cancel.is_cancelled() {
            return Err(PearlMergeMiningError::Cancelled);
        }
        if let Some(deadline) = opts.deadline {
            if Instant::now() >= deadline {
                return Err(PearlMergeMiningError::DeadlineElapsed);
            }
        }
        let unthresholded_batch = match scheduler.next_batch([0; 32]) {
            Ok(batch) => batch,
            Err(SearchScheduleEnd::BudgetExhausted { max }) => {
                return Err(PearlMergeMiningError::BudgetExhausted { max });
            }
            Err(SearchScheduleEnd::AttemptSpaceExhausted) => {
                return Err(PearlMergeMiningError::AttemptSpaceExhausted);
            }
        };
        let (t_rows, t_cols) = prepared
            .offsets_at_ordinal(unthresholded_batch.start)
            .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
        let target_params = PearlPublicProofParams {
            block_header: *job.header,
            mining_config: *job.config,
            hash_a: prepared.commitments().h_a,
            hash_b: prepared.commitments().h_b,
            hash_jackpot: [0; 32],
            m: job.params.m,
            n: job.params.n,
            t_rows,
            t_cols,
        };
        let pearl_target = target_params.pearl_adjusted_target()?;
        let nockchain_target = target_params.nockchain_adjusted_target(&job.nockchain_target)?;
        let threshold = max_target_le(pearl_target, nockchain_target);
        let batch = crate::search::SearchBatch::new(
            unthresholded_batch.start, unthresholded_batch.len, threshold,
        )?;
        let winner = backend.search_dense(Arc::clone(&prepared), batch)?;

        if cancel.is_cancelled() {
            return Err(PearlMergeMiningError::Cancelled);
        }
        if let Some(deadline) = opts.deadline {
            if Instant::now() >= deadline {
                return Err(PearlMergeMiningError::DeadlineElapsed);
            }
        }
        let stats = match winner {
            Some(winner) => {
                let matmul_attempts_tried = scheduler.record_winner(batch, winner)?;
                MiningStats {
                    matmul_attempts_tried,
                    elapsed: started.elapsed(),
                }
            }
            None => {
                scheduler.record_miss(batch)?;
                let stats = MiningStats {
                    matmul_attempts_tried: scheduler.attempts_tried(),
                    elapsed: started.elapsed(),
                };
                if let Some(interval) = opts.progress_interval {
                    if last_progress.elapsed() >= interval {
                        tracing::trace!(
                            target: "ai_pow_miner",
                            pearl_ticket_attempts = stats.matmul_attempts_tried,
                            elapsed_s = stats.elapsed.as_secs_f64(),
                            matmul_attempt_rate = stats.matmul_attempt_rate_per_sec(),
                            "Pearl merge mining progress"
                        );
                        last_progress = Instant::now();
                    }
                }
                continue;
            }
        };

        let winner = winner.expect("winner branch constructed statistics");
        let (t_rows, t_cols) = prepared
            .offsets_at_ordinal(winner.ordinal)
            .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
        let checked_attempt = evaluate_pearl_merge_checked_ticket_attempt(
            job.header,
            job.config,
            job.params,
            t_rows,
            t_cols,
            job.a,
            job.b,
            &job.nockchain_target,
            job.max_pattern_len,
            job.aux.clone(),
        )?;
        let attempt = checked_attempt.attempt();
        if attempt.ticket.jackpot_hash != winner.jackpot_hash {
            return Err(PearlMergeMiningError::Pearl(
                PearlCompatError::JackpotHashMismatch,
            ));
        }
        let pearl_target_hit = attempt
            .public_params
            .check_pearl_jackpot_difficulty()
            .is_ok();
        let nockchain_target_hit = attempt
            .public_params
            .check_nockchain_jackpot_target(&job.nockchain_target)
            .is_ok();
        if !pearl_target_hit && !nockchain_target_hit {
            return Err(PearlMergeMiningError::BackendWinnerTargetMismatch);
        }
        return Ok(PearlMergeMinedTicket {
            attempt: checked_attempt,
            pearl_target_hit,
            nockchain_target_hit,
            stats,
        });
    }
}

fn max_target_le(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    if hash_le_target(&left, &right) {
        right
    } else {
        left
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct PatternOffsetSpace<'a> {
    pattern: &'a PearlPeriodicPattern,
    max_offset_exclusive: u32,
    period: u32,
    valid_per_period: u32,
    len: u64,
}

#[cfg(test)]
impl<'a> PatternOffsetSpace<'a> {
    fn new(
        pattern: &'a PearlPeriodicPattern,
        total_dimension: u32,
        max_pattern_len: usize,
    ) -> Result<Self, PearlCompatError> {
        let indices = pattern.to_list_bounded(max_pattern_len)?;
        let max_index = indices.into_iter().max().unwrap_or(0);
        let Some(max_offset_exclusive) = total_dimension.checked_sub(max_index) else {
            return Ok(Self::empty(pattern));
        };
        let period = pattern.period()?;
        if period == 0 {
            return Ok(Self::empty(pattern));
        }

        let valid_per_period = count_valid_offsets(pattern, period);
        if valid_per_period == 0 {
            return Ok(Self::empty(pattern));
        }
        let full_periods = max_offset_exclusive / period;
        let tail = max_offset_exclusive % period;
        let valid_in_tail = count_valid_offsets(pattern, tail);
        let len = u64::from(full_periods)
            .checked_mul(u64::from(valid_per_period))
            .and_then(|base| base.checked_add(u64::from(valid_in_tail)))
            .ok_or(PearlCompatError::PatternPeriodTooLarge)?;

        Ok(Self {
            pattern,
            max_offset_exclusive,
            period,
            valid_per_period,
            len,
        })
    }

    fn empty(pattern: &'a PearlPeriodicPattern) -> Self {
        Self {
            pattern,
            max_offset_exclusive: 0,
            period: 0,
            valid_per_period: 0,
            len: 0,
        }
    }

    fn len(&self) -> u64 {
        self.len
    }

    fn offset_at(&self, ordinal: u64) -> Option<u32> {
        if ordinal >= self.len || self.valid_per_period == 0 || self.period == 0 {
            return None;
        }

        let period_ordinal = ordinal / u64::from(self.valid_per_period);
        let within_period = u32::try_from(ordinal % u64::from(self.valid_per_period)).ok()?;
        let base = period_ordinal.checked_mul(u64::from(self.period))?;
        let remaining = u64::from(self.max_offset_exclusive).checked_sub(base)?;
        let upper_exclusive = u32::try_from(remaining.min(u64::from(self.period))).ok()?;
        let local = nth_valid_offset(self.pattern, upper_exclusive, within_period)?;
        u32::try_from(base.checked_add(u64::from(local))?).ok()
    }
}

#[cfg(test)]
fn count_valid_offsets(pattern: &PearlPeriodicPattern, upper_exclusive: u32) -> u32 {
    (0..upper_exclusive)
        .filter(|offset| pattern.offset_is_valid(*offset))
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
fn nth_valid_offset(
    pattern: &PearlPeriodicPattern,
    upper_exclusive: u32,
    ordinal: u32,
) -> Option<u32> {
    let mut seen = 0u32;
    for offset in 0..upper_exclusive {
        if pattern.offset_is_valid(offset) {
            if seen == ordinal {
                return Some(offset);
            }
            seen = seen.checked_add(1)?;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use ai_pow::pearl_compat::{
        verify_pearl_merge_public_statement_bytes, PearlIncompleteBlockHeader, PearlMiningConfig,
        PearlNockchainAux, PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE,
        PEARL_MMA_INT7XINT7_TO_INT32,
    };
    use ai_pow::synth::synth_matrices;

    use super::*;
    use crate::search::{SearchBackend, SearchBackendError, SearchBatch, SearchWinner};

    fn pearl_test_pattern(length: u32) -> PearlPeriodicPattern {
        PearlPeriodicPattern {
            shape: [(1, length), (length, 1), (length, 1)],
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
            m: 128,
            k: 1024,
            n: 128,
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

    fn pearl_job<'a>(
        header: &'a PearlIncompleteBlockHeader,
        config: &'a PearlMiningConfig,
        params: &'a MatmulParams,
        a: &'a [i8],
        b: &'a [i8],
        target: [u8; 32],
    ) -> PearlMergeMiningJob<'a> {
        PearlMergeMiningJob {
            header,
            config,
            params,
            nockchain_target: target,
            a,
            b,
            max_pattern_len: 16,
            aux: pearl_test_aux(),
        }
    }

    struct NoHitBackend {
        batches: Mutex<Vec<SearchBatch>>,
    }

    impl SearchBackend for NoHitBackend {
        fn search_dense(
            &self,
            _: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
            batch: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            self.batches.lock().expect("test lock").push(batch);
            Ok(None)
        }

        #[cfg(feature = "node")]
        fn search_canonical(
            &self,
            _: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
            _: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            Ok(None)
        }
    }

    struct CancellingBackend {
        cancel: MiningCancel,
    }

    impl SearchBackend for CancellingBackend {
        fn search_dense(
            &self,
            _: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
            _: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            self.cancel.cancel();
            Ok(None)
        }

        #[cfg(feature = "node")]
        fn search_canonical(
            &self,
            _: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
            _: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            self.cancel.cancel();
            Ok(None)
        }
    }

    struct CorruptDenseBackend;

    impl SearchBackend for CorruptDenseBackend {
        fn search_dense(
            &self,
            template: Arc<ai_pow::pearl_compat::PreparedPearlPatternJob>,
            batch: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            let (t_rows, t_cols) = template
                .offsets_at_ordinal(batch.start)
                .expect("test batch starts in prepared offset space");
            let mut scratch = template.scratch();
            let mut jackpot_hash = template
                .evaluate(t_rows, t_cols, &mut scratch)
                .expect("test prepared ticket")
                .jackpot_hash;
            jackpot_hash[0] ^= 1;
            Ok(Some(SearchWinner {
                ordinal: batch.start,
                jackpot_hash,
            }))
        }

        #[cfg(feature = "node")]
        fn search_canonical(
            &self,
            _: Arc<crate::canonical::PreparedCanonicalMoeTemplate>,
            _: SearchBatch,
        ) -> Result<Option<SearchWinner>, SearchBackendError> {
            Ok(None)
        }
    }

    fn assert_offset_space_matches_reference(
        pattern: &PearlPeriodicPattern,
        total_dimension: u32,
        max_pattern_len: usize,
    ) {
        let space =
            PatternOffsetSpace::new(pattern, total_dimension, max_pattern_len).expect("space");
        let max_index = pattern
            .to_list_bounded(max_pattern_len)
            .expect("bounded pattern")
            .into_iter()
            .max()
            .unwrap_or(0);
        let expected: Vec<u32> = total_dimension
            .checked_sub(max_index)
            .map(|upper| {
                (0..upper)
                    .filter(|offset| pattern.offset_is_valid(*offset))
                    .collect()
            })
            .unwrap_or_default();

        assert_eq!(space.len(), expected.len() as u64);
        for (ordinal, offset) in expected.iter().enumerate() {
            assert_eq!(space.offset_at(ordinal as u64), Some(*offset));
        }
        assert_eq!(space.offset_at(expected.len() as u64), None);
    }

    #[test]
    fn pattern_offset_space_enumerates_valid_offsets_without_pair_materialization() {
        let pattern = pearl_test_pattern(8);
        let space = PatternOffsetSpace::new(&pattern, 128, 16).expect("build offset space");

        assert_eq!(space.len(), 16);
        assert_eq!(space.offset_at(0), Some(0));
        assert_eq!(space.offset_at(1), Some(8));
        assert_eq!(space.offset_at(15), Some(120));
        assert_eq!(space.offset_at(16), None);
    }

    #[test]
    fn pattern_offset_space_matches_reference_scan_on_tail_boundaries() {
        assert_offset_space_matches_reference(&pearl_test_pattern(8), 25, 16);
        assert_offset_space_matches_reference(&pearl_test_pattern(8), 16, 16);
        assert_offset_space_matches_reference(&pearl_test_pattern(8), 8, 16);

        let noncontiguous = PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73])
            .expect("representable noncontiguous Pearl pattern");
        assert_offset_space_matches_reference(&noncontiguous, 128, 16);
        assert_offset_space_matches_reference(&noncontiguous, 80, 16);
        assert_offset_space_matches_reference(&noncontiguous, 72, 16);
    }

    #[test]
    fn pattern_offset_space_handles_noncontiguous_periodic_patterns() {
        let pattern = PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73])
            .expect("representable noncontiguous Pearl pattern");
        let space = PatternOffsetSpace::new(&pattern, 128, 16).expect("build offset space");

        assert_eq!(space.len(), 16);
        assert_eq!(space.offset_at(0), Some(0));
        assert_eq!(space.offset_at(1), Some(2));
        assert_eq!(space.offset_at(7), Some(22));
        assert_eq!(space.offset_at(15), Some(54));
        assert_eq!(space.offset_at(16), None);
    }

    #[test]
    fn pearl_merge_mining_returns_ticket_before_any_proof_artifact() {
        let params = pearl_test_params();
        let header = PearlIncompleteBlockHeader {
            timestamp: 0x6677_889d,
            ..pearl_test_header()
        };
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-success-v2", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());

        let mined = run(&job, &PearlMergeMineOptions::default(), MiningCancel::new())
            .expect("trivial targets should mine first Pearl ticket");

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
        assert!(mined.nockchain_target_hit);
        assert!(mined.pearl_target_hit);
        assert_eq!(mined.attempt.public_params.t_rows, 0);
        assert_eq!(mined.attempt.public_params.t_cols, 0);
        verify_pearl_merge_public_statement_bytes(
            &job.aux.nock_block_commitment,
            &mined.attempt.statement.to_bytes().unwrap(),
            &a,
            &b,
            &job.nockchain_target,
            job.max_pattern_len,
        )
        .expect("mined ticket statement verifies");
    }

    #[test]
    fn pearl_merge_mining_requires_nockchain_target_not_pearl_target() {
        let params = pearl_test_params();
        let mut header = pearl_test_header();
        header.nbits = 0x1d00_0000; // zero Pearl target after compact decode.
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-nockchain-only", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());

        let mined = run(&job, &PearlMergeMineOptions::default(), MiningCancel::new())
            .expect("Nockchain target hit should not require Pearl target hit");

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
        assert!(!mined.pearl_target_hit);
        assert!(mined.nockchain_target_hit);
        mined
            .attempt
            .public_params
            .check_nockchain_jackpot_target(&job.nockchain_target)
            .expect("ticket satisfies Nockchain target");
        assert!(
            mined
                .attempt
                .public_params
                .check_pearl_jackpot_difficulty()
                .is_err(),
            "fixture must prove the miner did not require Pearl target satisfaction"
        );
    }

    #[test]
    fn pearl_merge_mining_returns_pearl_only_target_hits() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-pearl-only", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0; 32]);

        let mined = run(&job, &PearlMergeMineOptions::default(), MiningCancel::new())
            .expect("Pearl target hit should be independently mineable");

        // With the max valid compact Pearl target (~2^255 at this fixture's
        // factor), the loop grinds a few offsets before a jackpot clears it.
        assert!(mined.stats.matmul_attempts_tried >= 1);
        assert!(mined.pearl_target_hit);
        assert!(!mined.nockchain_target_hit);
        mined
            .attempt
            .public_params
            .check_pearl_jackpot_difficulty()
            .expect("ticket satisfies Pearl target");
        assert!(
            mined
                .attempt
                .public_params
                .check_nockchain_jackpot_target(&job.nockchain_target)
                .is_err(),
            "fixture must prove the miner did not require Nockchain target satisfaction"
        );
    }

    #[test]
    fn pearl_merge_mining_misses_do_not_emit_ticket_artifacts() {
        let params = pearl_test_params();
        let mut header = pearl_test_header();
        header.nbits = 0x1d00_0000; // zero Pearl target after compact decode.
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-miss", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0; 32]);
        let opts = PearlMergeMineOptions {
            max_attempts: Some(2),
            ..PearlMergeMineOptions::default()
        };

        assert!(matches!(
            run(&job, &opts, MiningCancel::new()),
            Err(PearlMergeMiningError::BudgetExhausted { max: 2 })
        ));
    }

    #[test]
    fn pearl_scheduler_caps_final_batch_at_remaining_budget() {
        let mut params = pearl_test_params();
        params.m = 136;
        params.n = 136;
        let mut header = pearl_test_header();
        header.nbits = 0x1d00_0000;
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-batch-budget", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0; 32]);
        let backend = NoHitBackend {
            batches: Mutex::new(Vec::new()),
        };
        let opts = PearlMergeMineOptions {
            max_attempts: Some(257),
            progress_interval: None,
            ..PearlMergeMineOptions::default()
        };

        assert!(matches!(
            run_with_backend(&job, &opts, MiningCancel::new(), &backend),
            Err(PearlMergeMiningError::BudgetExhausted { max: 257 })
        ));
        let batches = backend.batches.lock().expect("test lock");
        assert_eq!(batches.len(), 2);
        assert_eq!((batches[0].start, batches[0].len), (0, 256));
        assert_eq!((batches[1].start, batches[1].len), (256, 1));
    }

    #[test]
    fn pearl_scheduler_discards_completed_batch_after_cancellation() {
        let params = pearl_test_params();
        let mut header = pearl_test_header();
        header.nbits = 0x1d00_0000;
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-batch-cancel", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0; 32]);
        let cancel = MiningCancel::new();
        let backend = CancellingBackend {
            cancel: cancel.clone(),
        };

        assert!(matches!(
            run_with_backend(&job, &PearlMergeMineOptions::default(), cancel, &backend),
            Err(PearlMergeMiningError::Cancelled)
        ));
    }

    #[test]
    fn dense_backend_winner_must_match_checked_ticket_oracle() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-corrupt-backend", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());

        assert!(matches!(
            run_with_backend(
                &job,
                &PearlMergeMineOptions::default(),
                MiningCancel::new(),
                &CorruptDenseBackend,
            ),
            Err(PearlMergeMiningError::Pearl(
                PearlCompatError::JackpotHashMismatch
            ))
        ));
    }

    #[test]
    fn pearl_merge_mining_can_start_at_later_ticket_pair() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-start", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());
        let opts = PearlMergeMineOptions {
            attempt_start: 1,
            ..PearlMergeMineOptions::default()
        };

        let mined = run(&job, &opts, MiningCancel::new()).expect("second ticket should mine");

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
        assert_eq!(mined.attempt.public_params.t_rows, 0);
        assert_eq!(mined.attempt.public_params.t_cols, 8);
    }

    #[test]
    fn pearl_merge_mining_linear_attempts_cross_row_boundaries() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-row-boundary", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());
        let opts = PearlMergeMineOptions {
            attempt_start: 16,
            ..PearlMergeMineOptions::default()
        };

        let mined = run(&job, &opts, MiningCancel::new())
            .expect("first ticket in the second row-offset block should mine");

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
        assert_eq!(mined.attempt.public_params.t_rows, 8);
        assert_eq!(mined.attempt.public_params.t_cols, 0);
    }

    #[test]
    fn pearl_merge_mining_rejects_wrong_matrix_shape() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (mut a, b) = synth_matrices(b"pearl-ticket-loop-bad-matrix", &params);
        a.pop();
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());

        assert!(matches!(
            run(&job, &PearlMergeMineOptions::default(), MiningCancel::new()),
            Err(PearlMergeMiningError::Pearl(
                PearlCompatError::InputAShape { .. }
            ))
        ));
    }

    #[test]
    fn pearl_merge_mining_returns_cancelled_before_work() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-cancel", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, crate::easy_nock_target());
        let cancel = MiningCancel::new();
        cancel.cancel();

        assert!(matches!(
            run(&job, &PearlMergeMineOptions::default(), cancel),
            Err(PearlMergeMiningError::Cancelled)
        ));
    }

    #[test]
    fn dense_search_kat_snapshot() {
        use ai_pow::matmul::{compute_pattern_tile_trace_from_slices, BlockNoise};

        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let target = crate::easy_nock_target();
        let (a, b) = synth_matrices(b"pearl-search-kat-v1", &params);
        let checked = evaluate_pearl_merge_checked_ticket_attempt(
            &header,
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &target,
            16,
            pearl_test_aux(),
        )
        .expect("checked dense ticket");
        let attempt = checked.attempt();
        let ticket = &attempt.ticket;
        let noise = BlockNoise::expand(&attempt.commitments.s_a, &attempt.commitments.s_b, &params);
        let mut a_prime = Vec::with_capacity(ticket.a_rows.len() * params.k as usize);
        let mut e_row = vec![0i8; params.k as usize];
        for &row in &ticket.a_rows {
            noise.e_row_into(row, &mut e_row);
            let offset = row as usize * params.k as usize;
            a_prime.extend(
                a[offset..offset + params.k as usize]
                    .iter()
                    .zip(&e_row)
                    .map(|(&a, &e)| (a as i16 + e as i16) as i8),
            );
        }
        let mut b_prime = Vec::with_capacity(ticket.b_cols.len() * params.k as usize);
        let mut f_col = vec![0i8; params.k as usize];
        for &col in &ticket.b_cols {
            noise.f_col_into(col, &mut f_col);
            let offset = col as usize * params.k as usize;
            b_prime.extend(
                b[offset..offset + params.k as usize]
                    .iter()
                    .zip(&f_col)
                    .map(|(&b, &f)| (b as i16 + f as i16) as i8),
            );
        }
        let trace = compute_pattern_tile_trace_from_slices(
            &a_prime,
            &b_prime,
            ticket.a_rows.len(),
            ticket.b_cols.len(),
            params.k as usize,
            params.noise_rank as usize,
            params.k as usize,
        );

        assert_eq!(
            hex::encode(header.to_bytes()),
            "040302011111111111111111111111111111111111111111111111111111111111111111222222222222222222222222222222222222222222222222222222222222222299887766ffff7f1e"
        );
        assert_eq!(
            hex::encode(config.to_bytes().expect("config bytes")),
            "00040000400000000007000000000007000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            hex::encode(attempt.commitments.kappa),
            "9b2bf4287f7a0a9d5b9ebf42e285c8d0c8f9cc1d670784a343178bd808f7aee1"
        );
        assert_eq!(
            hex::encode(attempt.commitments.h_a),
            "7ba6f6fe82d206d74600bf85d43692f8b8f41adc97af8773785ed4089cba1814"
        );
        assert_eq!(
            hex::encode(attempt.commitments.h_b),
            "596644dfe94469924c984b17b66ab056ba54dd9329ab06b526e48c513b3fccec"
        );
        assert_eq!(
            hex::encode(attempt.commitments.s_a),
            "0f40e3da544003b73df8c2923859771611247c287294dc616c3b4ee71205009d"
        );
        assert_eq!(
            hex::encode(attempt.commitments.s_b),
            "27714ab216358b4ccd3607cef77fbd920f306db4781d29804d1efafa7bbc63ad"
        );
        assert_eq!(ticket.a_rows, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(ticket.b_cols, [0, 1, 2, 3, 4, 5, 6, 7]);
        // These digest every byte of the selected noised strips in their ticket order.
        assert_eq!(
            hex::encode(
                blake3::hash(&a_prime.iter().map(|&value| value as u8).collect::<Vec<_>>())
                    .as_bytes()
            ),
            "2e2a371bdce4d04bc55c91231823f276bb779dae67f66eb2c7165bea3211ca29"
        );
        assert_eq!(
            hex::encode(
                blake3::hash(&b_prime.iter().map(|&value| value as u8).collect::<Vec<_>>())
                    .as_bytes()
            ),
            "cd43767608a59833742b26966261d9d377cc44c34bdcc750cf98e3e50f56156e"
        );
        assert_eq!(
            trace.x_steps,
            [
                37_013, 50_655, -56_693, -10_797, -22_899, -95_412, -25_779, -74_388, 197_143,
                -181_914, -32_590, -231_446, 254_655, -30_056, -234_467, -187_635,
            ]
        );
        assert_eq!(
            trace.state,
            ai_pow::matmul::TileState([
                37_013, 50_655, -56_693, -10_797, -22_899, -95_412, -25_779, -74_388, 197_143,
                -181_914, -32_590, -231_446, 254_655, -30_056, -234_467, -187_635,
            ])
        );
        assert_eq!(trace.state, ticket.tile_state);
        assert_eq!(
            hex::encode(ticket.jackpot_hash),
            "7dde5ffa39ce4d154bdc9407875d59207151bee169c82c587e648e985931cf47"
        );
        assert_eq!(
            hex::encode(attempt.pearl_target),
            "0000000000000000000000000000000000000000000000000000000000ffff7f"
        );
        assert_eq!(
            hex::encode(
                attempt
                    .public_params
                    .nockchain_adjusted_target(&target)
                    .expect("Nockchain target")
            ),
            "0000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );
        assert_eq!(attempt.public_params.t_rows, 0);
        assert_eq!(attempt.public_params.t_cols, 0);
    }
}
