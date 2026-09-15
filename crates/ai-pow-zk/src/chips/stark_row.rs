//! `STARK_ROW_IDX` monotonic-increment chip.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/monotonic_increment.rs`.
//! Pearl uses this as the row-address column that `CV_OUT → CV_IN`
//! LogUp routing keys on. We use it for the same purpose.
//!
//! ## Property enforced
//!
//! On the column at offset [`composite_layout::STARK_ROW_IDX`]:
//!
//! ```text
//!   first row:        STARK_ROW_IDX[0] == 0
//!   every transition: STARK_ROW_IDX[i + 1] == STARK_ROW_IDX[i] + 1
//! ```
//!
//! Combined, the column equals the row index. Future lookups (e.g.
//! the BLAKE3 CV-routing lookup) can use it to address arbitrary
//! prior rows.
//!
//! Pearl's chip is parameterised over the column offset; ours hard-
//! codes [`composite_layout::STARK_ROW_IDX`] because there's only
//! one row-counter in the M10.1c trace layout. If a future phase
//! needs a second monotonic counter we'll generalise.

use p3_air::{AirBuilder, WindowAccess};
use p3_field::PrimeCharacteristicRing;

use crate::composite_layout::STARK_ROW_IDX;

/// Zero-sized chip type. Holds no state.
#[derive(Debug, Default, Clone, Copy)]
pub struct StarkRowChip;

impl StarkRowChip {
    pub const fn new() -> Self {
        Self
    }

    /// Constraint generator. Called from the composite AIR's eval.
    pub fn eval<AB: AirBuilder>(&self, builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        let cur_idx = cur[STARK_ROW_IDX];
        let nxt_idx = nxt[STARK_ROW_IDX];

        // First row: index is zero.
        builder.when_first_row().assert_zero(cur_idx);

        // Transition: next index = current + 1.
        builder
            .when_transition()
            .assert_eq(nxt_idx, cur_idx + <AB::F as PrimeCharacteristicRing>::ONE);
    }

    /// Trace-side helper: write the correct row-counter value into
    /// the trace cell at this row. Used by `composite_trace`.
    pub fn fill_row(&self, row_idx: usize, row: &mut [crate::Val]) {
        use p3_field::integers::QuotientMap;
        row[STARK_ROW_IDX] = <crate::Val as QuotientMap<u64>>::from_int(row_idx as u64);
    }
}

#[cfg(test)]
mod tests {
    //! Test harness pattern reused across every chip:
    //!
    //! 1. Build a thin wrapper AIR that has `TOTAL_TRACE_WIDTH`
    //!    columns and invokes only the chip(s) under test.
    //! 2. Generate a tight test trace where this chip's columns
    //!    are filled correctly and everything else is zero.
    //! 3. Round-trip `prove` + `verify` through `TEST_PEARL` and
    //!    assert it succeeds.
    //! 4. Tamper specific cells; assert verification rejects.
    //!
    //! Other chips will follow the same pattern with their own
    //! `TestWrapperAir`.
    use p3_air::{Air, AirBuilder, BaseAir};
    use p3_field::integers::QuotientMap;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_layout::{MIN_STARK_LEN, STARK_ROW_IDX, TOTAL_TRACE_WIDTH};
    use crate::params::ZkParams;
    use crate::Val;

    /// Minimal AIR that wraps only the `StarkRowChip` for testing.
    /// Width matches the full composite layout (so the test trace
    /// is the same shape we'll use later) but every column other
    /// than `STARK_ROW_IDX` is unconstrained and may be zero.
    #[derive(Debug, Default)]
    struct StarkRowOnlyAir;

    impl<F> BaseAir<F> for StarkRowOnlyAir {
        fn width(&self) -> usize {
            TOTAL_TRACE_WIDTH
        }
    }

    impl<AB: AirBuilder> Air<AB> for StarkRowOnlyAir {
        fn eval(&self, builder: &mut AB) {
            StarkRowChip::new().eval(builder);
        }
    }

    /// Test parameters for the chip's standalone test trace, kept
    /// at MIN_STARK_LEN-sized traces (matches the composite-layout
    /// minimum height; ensures the chip works at production-shape
    /// row count).
    fn test_zk_params() -> ZkParams {
        ZkParams {
            m: 8,
            k: 16,
            n: 8,
            noise_rank: 2,
            tile: 2,
            difficulty_bits: 0,
        }
    }

    /// Build a valid monotonic-increment trace. Defaults to a small
    /// (16-row) trace for fast unit tests; the production-scale
    /// smoke test uses [`MIN_STARK_LEN`] explicitly.
    fn build_valid_trace_with_rows(rows: usize) -> RowMajorMatrix<Val> {
        let mut flat: Vec<Val> = vec![Val::default(); rows * TOTAL_TRACE_WIDTH];
        for r in 0..rows {
            flat[r * TOTAL_TRACE_WIDTH + STARK_ROW_IDX] =
                <Val as QuotientMap<u64>>::from_int(r as u64);
        }
        RowMajorMatrix::new(flat, TOTAL_TRACE_WIDTH)
    }

    /// Small trace for fast tamper-detection tests. 16 rows is the
    /// smallest power of two that admits meaningful transitions
    /// under `TEST_PEARL` (`log_blowup = 2` → min height `2^2 = 4`;
    /// we use 16 for headroom).
    fn build_valid_trace() -> RowMajorMatrix<Val> {
        build_valid_trace_with_rows(16)
    }

    #[test]
    fn chip_constructs() {
        let _chip = StarkRowChip::new();
    }

    #[test]
    fn fill_row_writes_row_index() {
        let chip = StarkRowChip::new();
        let mut row = vec![Val::default(); TOTAL_TRACE_WIDTH];
        chip.fill_row(42, &mut row);
        use p3_field::PrimeField64;
        assert_eq!(row[STARK_ROW_IDX].as_canonical_u64(), 42);
    }

    #[test]
    fn prove_and_verify_valid_monotonic_trace() {
        let cfg: AiPowStarkConfig =
            build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let trace = build_valid_trace();
        let pis: Vec<Val> = vec![];
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &pis)
            .expect("valid monotonic-increment trace must verify");
    }

    /// Property: first row must be zero. Tamper row 0 to be non-zero
    /// → reject.
    #[test]
    fn verify_rejects_nonzero_first_row() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let mut trace = build_valid_trace();
        // Force row 0's STARK_ROW_IDX to be 99 instead of 0. (Don't
        // touch the subsequent rows — the transition constraint
        // then catches the issue too, but the first-row constraint
        // fires first.)
        trace.values[STARK_ROW_IDX] = <Val as QuotientMap<u64>>::from_int(99);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        let r = verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]);
        assert!(r.is_err(), "non-zero first row must reject; got {r:?}");
    }

    /// Property: each transition must increment by exactly 1. Patch
    /// row 5's value while leaving row 6 alone → transition 5→6
    /// breaks.
    #[test]
    fn verify_rejects_broken_increment() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let mut trace = build_valid_trace();
        // Replace row 5's value with 42 (should be 5). Row 6 still
        // holds 6 → transition (5_actual → 6_actual) needs +1, but
        // 6 − 42 ≠ 1, so the constraint fires.
        let row5 = 5 * TOTAL_TRACE_WIDTH + STARK_ROW_IDX;
        trace.values[row5] = <Val as QuotientMap<u64>>::from_int(42);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        let r = verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]);
        assert!(r.is_err(), "broken increment must reject; got {r:?}");
    }

    /// Property: even a single skipped row (e.g. 5, 6, 8) is caught.
    /// Mirrors the "off-by-one" attack class.
    #[test]
    fn verify_rejects_skipped_index() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let mut trace = build_valid_trace();
        // Set row 7's value to 8 (so it equals row 8's expected
        // value). Now the chain reads 0,1,2,3,4,5,6,8,8,9,... —
        // the 6→8 transition fires the constraint.
        let row7 = 7 * TOTAL_TRACE_WIDTH + STARK_ROW_IDX;
        trace.values[row7] = <Val as QuotientMap<u64>>::from_int(8);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        let r = verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]);
        assert!(r.is_err(), "skipped index must reject; got {r:?}");
    }

    /// Property: the column actually equals the row index at every
    /// row (not just the boundary cases). Trace builder should put
    /// the right values into every cell.
    #[test]
    fn valid_trace_has_correct_row_indices() {
        let trace = build_valid_trace();
        let h = trace.values.len() / TOTAL_TRACE_WIDTH;
        use p3_field::PrimeField64;
        for r in 0..h {
            let v = trace.values[r * TOTAL_TRACE_WIDTH + STARK_ROW_IDX].as_canonical_u64();
            assert_eq!(v, r as u64, "row {r} has wrong STARK_ROW_IDX");
        }
    }

    /// Property: tampering with a row index late in the trace is
    /// caught too — first-row and transition constraints together
    /// cover the full chain, not just the early rows.
    #[test]
    fn verify_rejects_late_tamper() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let mut trace = build_valid_trace();
        let h = trace.values.len() / TOTAL_TRACE_WIDTH;
        // Replace second-to-last row's value with garbage.
        let row = h - 2;
        trace.values[row * TOTAL_TRACE_WIDTH + STARK_ROW_IDX] =
            <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEF);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &[]);
        let r = verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &[]);
        assert!(r.is_err(), "late-row tamper must reject; got {r:?}");
    }

    /// Production-scale smoke test: the same constraint set holds
    /// at `MIN_STARK_LEN` (Pearl's pinned minimum). Slow (~3 s);
    /// kept separate so the fast tamper tests above don't pay
    /// this cost.
    #[test]
    fn prove_and_verify_min_stark_len_trace() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let air = StarkRowOnlyAir;
        let trace = build_valid_trace_with_rows(MIN_STARK_LEN);
        let pis: Vec<Val> = vec![];
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &air, trace, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &air, &proof, &pis)
            .expect("MIN_STARK_LEN monotonic trace must verify");
    }
}
