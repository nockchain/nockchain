//! Pearl-compatible merge-mining ticket loop.
//!
//! One iteration evaluates one explicit Pearl-valid `t_rows` / `t_cols` ticket
//! attempt and returns after the shared Pearl jackpot digest clears either the
//! Pearl target or the Nockchain target. Recursive proof construction and
//! canonical `%ai-pow` artifact submission happen only after a Nockchain hit.

use std::time::{Duration, Instant};

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    evaluate_pearl_merge_ticket_attempt, PearlCompatError, PearlIncompleteBlockHeader,
    PearlMergeTicketAttempt, PearlMiningConfig, PearlNockchainAux, PearlPeriodicPattern,
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
    pub attempt: PearlMergeTicketAttempt,
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
}

/// Run Pearl-compatible ticket mining.
///
/// Every counted attempt evaluates exactly one `(t_rows, t_cols)` pair. Misses
/// return no proof artifact and no recursive certificate work is performed here.
pub fn run(
    job: &PearlMergeMiningJob<'_>,
    opts: &PearlMergeMineOptions,
    cancel: MiningCancel,
) -> Result<PearlMergeMinedTicket, PearlMergeMiningError> {
    let row_offsets =
        PatternOffsetSpace::new(&job.config.rows_pattern, job.params.m, job.max_pattern_len)?;
    let col_offsets =
        PatternOffsetSpace::new(&job.config.cols_pattern, job.params.n, job.max_pattern_len)?;
    if row_offsets.is_empty() || col_offsets.is_empty() {
        return Err(PearlMergeMiningError::AttemptSpaceExhausted);
    }
    let col_count = col_offsets.len();
    let total_attempts = row_offsets
        .len()
        .checked_mul(col_count)
        .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
    if opts.attempt_start >= total_attempts {
        return Err(PearlMergeMiningError::AttemptSpaceExhausted);
    }

    let start = Instant::now();
    let mut stats = MiningStats {
        matmul_attempts_tried: 0,
        elapsed: Duration::ZERO,
    };
    let mut last_progress = start;

    loop {
        if cancel.is_cancelled() {
            return Err(PearlMergeMiningError::Cancelled);
        }
        if let Some(deadline) = opts.deadline {
            if Instant::now() >= deadline {
                return Err(PearlMergeMiningError::DeadlineElapsed);
            }
        }
        if let Some(max) = opts.max_attempts {
            if stats.matmul_attempts_tried >= max {
                return Err(PearlMergeMiningError::BudgetExhausted { max });
            }
        }

        let linear = opts
            .attempt_start
            .checked_add(stats.matmul_attempts_tried)
            .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
        if linear >= total_attempts {
            return Err(PearlMergeMiningError::AttemptSpaceExhausted);
        }
        let t_rows = row_offsets
            .offset_at(linear / col_count)
            .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;
        let t_cols = col_offsets
            .offset_at(linear % col_count)
            .ok_or(PearlMergeMiningError::AttemptSpaceExhausted)?;

        let attempt = evaluate_pearl_merge_ticket_attempt(
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
        stats.matmul_attempts_tried += 1;
        stats.elapsed = start.elapsed();

        let pearl_target_hit = attempt
            .public_params
            .check_pearl_jackpot_difficulty()
            .is_ok();
        let nockchain_target_hit = attempt
            .public_params
            .check_nockchain_jackpot_target(&job.nockchain_target)
            .is_ok();

        if pearl_target_hit || nockchain_target_hit {
            return Ok(PearlMergeMinedTicket {
                attempt,
                pearl_target_hit,
                nockchain_target_hit,
                stats,
            });
        }

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
    }
}

#[derive(Debug, Clone, Copy)]
struct PatternOffsetSpace<'a> {
    pattern: &'a PearlPeriodicPattern,
    max_offset_exclusive: u32,
    period: u32,
    valid_per_period: u32,
    len: u64,
}

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

    fn is_empty(&self) -> bool {
        self.len == 0
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

fn count_valid_offsets(pattern: &PearlPeriodicPattern, upper_exclusive: u32) -> u32 {
    (0..upper_exclusive)
        .filter(|offset| pattern.offset_is_valid(*offset))
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

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
    use ai_pow::pearl_compat::{
        verify_pearl_merge_public_statement_bytes, PearlIncompleteBlockHeader, PearlMiningConfig,
        PearlNockchainAux, PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE,
        PEARL_MMA_INT7XINT7_TO_INT32,
    };
    use ai_pow::synth::synth_matrices;

    use super::*;

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
            nbits: 0x207f_ffff,
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
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-success", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);

        let mined = run(&job, &PearlMergeMineOptions::default(), MiningCancel::new())
            .expect("trivial targets should mine first Pearl ticket");

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
        assert!(mined.pearl_target_hit);
        assert!(mined.nockchain_target_hit);
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
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);

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

        assert_eq!(mined.stats.matmul_attempts_tried, 1);
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
    fn pearl_merge_mining_can_start_at_later_ticket_pair() {
        let params = pearl_test_params();
        let header = pearl_test_header();
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-loop-start", &params);
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);
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
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);
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
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);

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
        let job = pearl_job(&header, &config, &params, &a, &b, [0xff; 32]);
        let cancel = MiningCancel::new();
        cancel.cancel();

        assert!(matches!(
            run(&job, &PearlMergeMineOptions::default(), cancel),
            Err(PearlMergeMiningError::Cancelled)
        ));
    }
}
