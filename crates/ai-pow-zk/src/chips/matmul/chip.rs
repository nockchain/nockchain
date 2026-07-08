//! Matmul AIR — enforces the tile-accumulator update.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of Pearl `zk-pow chip/matmul/
//! compute_cumsum.rs` constraint. One constraint per `(i, j)` cell
//! in `CUMSUM_TILE`, asserting the reset / update / pass-through
//! semantics:
//!
//! ```text
//!   cumsum_new = (is_reset + is_update) · dot + (1 − is_reset) · cumsum_old
//! ```
//!
//! Equivalent boolean truth table (selectors mutually exclusive):
//!
//! | is_reset | is_update | cumsum_new |
//! |---|---|---|
//! | 0 | 0 | cumsum_old |
//! | 0 | 1 | cumsum_old + dot |
//! | 1 | 0 | dot |
//! | 1 | 1 | dot + cumsum_old (degenerate — both fire) |
//!
//! The control chip + CONTROL_PREP unpacking ensure the boolean
//! check + selector exclusivity. The matmul chip itself doesn't
//! enforce exclusivity; the row formula collapses cleanly only
//! when the boundaries are pinned upstream.
//!
//! ## Chip-local layout
//!
//! For Phase 9, the chip uses its own column layout (independent
//! of `composite_layout`). Phase 12 will wire it into the global
//! trace. The local layout maps:
//!
//! ```text
//!   col 0..32   : A_UNPACK[i][d]  i ∈ {0, 1}, d ∈ {0..16}  (row-major)
//!   col 32..64  : B_UNPACK[j][d]  j ∈ {0, 1}, d ∈ {0..16}  (row-major)
//!   col 64..68  : CUMSUM[i][j]    i ∈ {0, 1}, j ∈ {0, 1}   (row-major)
//!   col 68      : IS_RESET_CUMSUM
//!   col 69      : IS_UPDATE_CUMSUM
//!   col 70      : padding
//! ```
//!
//! Cumsum NEW value is read from the next row's CUMSUM cells via
//! `when_transition()`, so an N-row trace encodes N-1 update
//! steps. The last row's CUMSUM is the **final** accumulator
//! value; the proof verifies it against the public input by
//! virtue of the next row's CUMSUM (= row 0 wrap, treated as
//! "after the last step") — but Phase 9 only validates the
//! per-step constraints. Phase 12 wires CUMSUM to public outputs.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use super::compute::{compute_row, CUMSUM_LEN};
use crate::composite_layout::{TILE_D, TILE_H};
use crate::Val;

/// Chip-local column offsets. Independent of `composite_layout` —
/// Phase 12 stitches them into the global trace.
pub mod cols {
    use crate::composite_layout::{TILE_D, TILE_H};

    pub const A_UNPACK: usize = 0;
    pub const A_UNPACK_LEN: usize = TILE_H * TILE_D; // 32

    pub const B_UNPACK: usize = A_UNPACK + A_UNPACK_LEN; // 32
    pub const B_UNPACK_LEN: usize = TILE_H * TILE_D; // 32

    pub const CUMSUM: usize = B_UNPACK + B_UNPACK_LEN; // 64
    pub const CUMSUM_LEN: usize = TILE_H * TILE_H; // 4

    pub const IS_RESET_CUMSUM: usize = CUMSUM + CUMSUM_LEN; // 68
    pub const IS_UPDATE_CUMSUM: usize = IS_RESET_CUMSUM + 1; // 69

    /// One unused column to keep ROW_W as a clean even number for
    /// future extension. Padded out so trace allocations are
    /// power-of-2 friendly.
    pub const PADDING: usize = IS_UPDATE_CUMSUM + 1; // 70

    pub const ROW_W: usize = PADDING + 1; // 71
}

/// Matmul cumsum-update chip.
#[derive(Copy, Clone, Debug, Default)]
pub struct MatmulCumsumChip;

impl<F> BaseAir<F> for MatmulCumsumChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

/// Column-offset bundle for [`MatmulCumsumChip::eval_at`]. The
/// chip's eval body operates on **arbitrary** offsets so it can
/// be reused both standalone (chip-local layout via
/// [`cols`]) and as part of the composite AIR.
#[derive(Copy, Clone, Debug)]
pub struct MatmulOffsets {
    pub a_unpack_start: usize,
    pub b_unpack_start: usize,
    pub cumsum_start: usize,
    pub is_reset_col: usize,
    pub is_update_col: usize,
}

impl MatmulCumsumChip {
    /// Chip-local offsets — used by [`MatmulCumsumChip::eval`].
    pub const LOCAL_OFFSETS: MatmulOffsets = MatmulOffsets {
        a_unpack_start: cols::A_UNPACK,
        b_unpack_start: cols::B_UNPACK,
        cumsum_start: cols::CUMSUM,
        is_reset_col: cols::IS_RESET_CUMSUM,
        is_update_col: cols::IS_UPDATE_CUMSUM,
    };

    /// Composite-trace offsets (Phase 12b wiring). Reads cells
    /// at `composite_layout::*` positions.
    pub const COMPOSITE_OFFSETS: MatmulOffsets = MatmulOffsets {
        a_unpack_start: crate::composite_layout::A_NOISED_UNPACK_START,
        b_unpack_start: crate::composite_layout::B_NOISED_UNPACK_START,
        cumsum_start: crate::composite_layout::CUMSUM_TILE_START,
        is_reset_col: crate::composite_layout::IS_RESET_CUMSUM,
        is_update_col: crate::composite_layout::IS_UPDATE_CUMSUM,
    };

    /// Emit the cumsum-update constraints at the given column
    /// offsets. Reads selectors, A/B unpack cells, and CUMSUM
    /// cells; emits booleanity + cross-row equality constraints.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &MatmulOffsets) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        // ---- Selector reads + boolean checks ----
        let is_reset_var = cur[off.is_reset_col];
        let is_update_var = cur[off.is_update_col];
        let is_reset: AB::Expr = is_reset_var.into();
        let is_update: AB::Expr = is_update_var.into();
        builder.assert_bool(is_reset_var);
        builder.assert_bool(is_update_var);

        // ---- Tile dot product (degree 2) ----
        let mut dot: [AB::Expr; CUMSUM_LEN] =
            core::array::from_fn(|_| <AB::Expr as PrimeCharacteristicRing>::ZERO);
        for i in 0..TILE_H {
            for j in 0..TILE_H {
                let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
                for d in 0..TILE_D {
                    let a_cell: AB::Expr = cur[off.a_unpack_start + i * TILE_D + d].into();
                    let b_cell: AB::Expr = cur[off.b_unpack_start + j * TILE_D + d].into();
                    acc = acc + a_cell * b_cell;
                }
                dot[i * TILE_H + j] = acc;
            }
        }

        // ---- Cross-row cumsum update ----
        {
            let mut tb = builder.when_transition();
            for k in 0..CUMSUM_LEN {
                let cur_cumsum: AB::Expr = cur[off.cumsum_start + k].into();
                let nxt_cumsum: AB::Expr = nxt[off.cumsum_start + k].into();

                let one_minus_reset: AB::Expr =
                    <AB::Expr as PrimeCharacteristicRing>::ONE - is_reset.clone();
                let reset_plus_update: AB::Expr = is_reset.clone() + is_update.clone();

                let rhs = reset_plus_update * dot[k].clone() + one_minus_reset * cur_cumsum;

                tb.assert_eq(nxt_cumsum, rhs);
            }
        }
    }

    /// Composite-layout entry point: equivalent to
    /// `eval_at(builder, &COMPOSITE_OFFSETS)`. Called from
    /// [`crate::composite_full_air::CompositeFullAir::eval`].
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);
    }
}

impl<AB: AirBuilder> Air<AB> for MatmulCumsumChip {
    fn eval(&self, builder: &mut AB) {
        MatmulCumsumChip::eval_at(builder, &MatmulCumsumChip::LOCAL_OFFSETS);
    }
}

/// Fill one row's matmul cells from a known (A, B, cumsum, selectors)
/// tuple. The caller is responsible for ensuring `cumsum` matches
/// the AIR's transition equation given the **previous** row's
/// (A, B, selectors) — this helper only writes the row, not the
/// inter-row chain.
pub fn fill_row(
    dest: &mut [Val],
    a: &[[i8; TILE_D]; TILE_H],
    b: &[[i8; TILE_D]; TILE_H],
    cumsum: &[i32; CUMSUM_LEN],
    is_reset: bool,
    is_update: bool,
) {
    use p3_field::integers::QuotientMap;

    assert_eq!(dest.len(), cols::ROW_W);

    // A_UNPACK rows.
    for i in 0..TILE_H {
        for d in 0..TILE_D {
            dest[cols::A_UNPACK + i * TILE_D + d] =
                <Val as QuotientMap<i64>>::from_int(a[i][d] as i64);
        }
    }
    // B_UNPACK rows.
    for j in 0..TILE_H {
        for d in 0..TILE_D {
            dest[cols::B_UNPACK + j * TILE_D + d] =
                <Val as QuotientMap<i64>>::from_int(b[j][d] as i64);
        }
    }
    // CUMSUM cells.
    for k in 0..CUMSUM_LEN {
        dest[cols::CUMSUM + k] = <Val as QuotientMap<i64>>::from_int(cumsum[k] as i64);
    }
    // Selectors.
    dest[cols::IS_RESET_CUMSUM] = <Val as QuotientMap<u64>>::from_int(if is_reset { 1 } else { 0 });
    dest[cols::IS_UPDATE_CUMSUM] =
        <Val as QuotientMap<u64>>::from_int(if is_update { 1 } else { 0 });
    dest[cols::PADDING] = Val::default();
}

/// Build a full matmul trace from a list of `(A, B, is_reset,
/// is_update)` tuples. Computes the consistent `CUMSUM` chain by
/// applying [`compute_row`] step-by-step. Pads to next power of 2.
pub fn build_trace(steps: &[Step]) -> RowMajorMatrix<Val> {
    let n = steps.len().next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let mut cumsum_prev: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];

    for (i, step) in steps.iter().enumerate() {
        // Compute this row's cumsum value: per Pearl's convention,
        // the CUMSUM cell on row `i` is **the cumulative sum AFTER
        // applying step i's update**. We model this as follows:
        //
        //   * `cumsum_prev` carries the value that step i sees as
        //     "old" (i.e. the value of CUMSUM on the trace row
        //     PRECEDING this one in update-order).
        //   * `cumsum_new` is the result after applying step i's
        //     reset/update/passthrough.
        //
        // For our AIR's cross-row encoding (cur.CUMSUM = old,
        // nxt.CUMSUM = new), we need to write `cumsum_prev` on row
        // `i` and `cumsum_new` on row `i + 1`. We do this by
        // pre-computing one row ahead.
        //
        // The first row (i = 0) holds `[0; 4]` as the "old" — i.e.
        // before any step has been applied.
        let row_start = i * cols::ROW_W;
        let row = &mut flat[row_start..row_start + cols::ROW_W];

        let cumsum_new = compute_row(
            &step.a, &step.b, &cumsum_prev, step.is_reset, step.is_update,
        );

        // Write this row with `cumsum_prev` as the CUMSUM cell.
        fill_row(
            row, &step.a, &step.b, &cumsum_prev, step.is_reset, step.is_update,
        );

        cumsum_prev = cumsum_new;
    }

    // After the last step, the chain advances to `cumsum_prev` (the
    // final accumulator). Phase 12 will wire this to a public output
    // column; for the chip-local test we leave the next row's CUMSUM
    // as zero (which the when_transition gating silences).

    // Pad remaining rows with all zeros (constraint satisfied
    // trivially: is_reset=0, is_update=0, cumsum_old=0, cumsum_new=0,
    // dot=0).
    let _ = cumsum_prev;
    RowMajorMatrix::new(flat, cols::ROW_W)
}

/// One step in the matmul cumsum chain — the inputs to one row.
#[derive(Clone, Debug)]
pub struct Step {
    pub a: [[i8; TILE_D]; TILE_H],
    pub b: [[i8; TILE_D]; TILE_H],
    pub is_reset: bool,
    pub is_update: bool,
}

#[cfg(test)]
mod tests {
    //! End-to-end matmul chip tests. Builds a trace from a list of
    //! steps and runs prove + verify under
    //! [`CircuitConfig::TEST_PEARL`]. Tamper tests confirm each
    //! constraint detection mode.

    use p3_field::integers::QuotientMap;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::params::ZkParams;

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

    fn make_a_pattern_1() -> [[i8; TILE_D]; TILE_H] {
        let mut a = [[0i8; TILE_D]; TILE_H];
        for d in 0..TILE_D {
            a[0][d] = (d as i8 + 1) % 5;
            a[1][d] = (d as i8 * 3) % 7 - 3;
        }
        a
    }

    fn make_b_pattern_1() -> [[i8; TILE_D]; TILE_H] {
        let mut b = [[0i8; TILE_D]; TILE_H];
        for d in 0..TILE_D {
            b[0][d] = ((d as i8 + 2) % 6) - 3;
            b[1][d] = ((d as i8 + 3) % 11) - 5;
        }
        b
    }

    /// Helper: build a 4-step trace with reset then 3 updates and
    /// run the chip.
    fn build_4step_trace() -> RowMajorMatrix<Val> {
        let a = make_a_pattern_1();
        let b = make_b_pattern_1();
        let steps = vec![
            Step {
                a,
                b,
                is_reset: true,
                is_update: false,
            },
            Step {
                a,
                b,
                is_reset: false,
                is_update: true,
            },
            Step {
                a,
                b,
                is_reset: false,
                is_update: true,
            },
            Step {
                a,
                b,
                is_reset: false,
                is_update: true,
            },
        ];
        build_trace(&steps)
    }

    #[test]
    fn prove_and_verify_4_step_chain() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_4step_trace();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[])
            .expect("4-step cumsum chain must verify");
    }

    #[test]
    fn verify_rejects_tampered_cumsum() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Bump cumsum[0] on row 1 (the "after reset" position) by 1.
        let target = 1 * cols::ROW_W + cols::CUMSUM;
        let bumped = <Val as QuotientMap<i64>>::from_int(99_999);
        trace.values[target] = bumped;
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[]).is_err(),
            "tampered cumsum must reject"
        );
    }

    #[test]
    fn verify_rejects_tampered_a_cell() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Change A_UNPACK[0][0] on row 1 — the dot product changes,
        // so the cross-row cumsum no longer matches.
        let target = 1 * cols::ROW_W + cols::A_UNPACK;
        trace.values[target] = <Val as QuotientMap<i64>>::from_int(42);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[]).is_err(),
            "tampered A_UNPACK must reject"
        );
    }

    #[test]
    fn verify_rejects_tampered_b_cell() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        let target = 1 * cols::ROW_W + cols::B_UNPACK + 5;
        trace.values[target] = <Val as QuotientMap<i64>>::from_int(-100);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[]).is_err(),
            "tampered B_UNPACK must reject"
        );
    }

    #[test]
    fn verify_rejects_non_boolean_is_reset() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        let target = 0 * cols::ROW_W + cols::IS_RESET_CUMSUM;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[]).is_err(),
            "non-boolean is_reset must reject"
        );
    }

    #[test]
    fn verify_rejects_non_boolean_is_update() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        let target = 1 * cols::ROW_W + cols::IS_UPDATE_CUMSUM;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[]).is_err(),
            "non-boolean is_update must reject"
        );
    }

    /// Pass-through rows (both selectors 0) preserve CUMSUM.
    #[test]
    fn prove_and_verify_passthrough_row() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let a = make_a_pattern_1();
        let b = make_b_pattern_1();
        let steps = vec![
            Step {
                a,
                b,
                is_reset: true,
                is_update: false,
            },
            // Pass-through row: CUMSUM unchanged regardless of A, B.
            Step {
                a,
                b,
                is_reset: false,
                is_update: false,
            },
            Step {
                a,
                b,
                is_reset: false,
                is_update: true,
            },
            Step {
                a,
                b,
                is_reset: false,
                is_update: true,
            },
        ];
        let trace = build_trace(&steps);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[])
            .expect("pass-through cumsum must verify");
    }

    /// Large positive + large negative dot products produce
    /// arithmetically correct cumsum (sanity check for i32
    /// boundaries).
    #[test]
    fn prove_and_verify_extreme_values() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let a_pos = [[127i8; TILE_D]; TILE_H];
        let b_pos = [[127i8; TILE_D]; TILE_H];
        let a_neg = [[-128i8; TILE_D]; TILE_H];
        let b_neg = [[127i8; TILE_D]; TILE_H];
        let steps = vec![
            Step {
                a: a_pos,
                b: b_pos,
                is_reset: true,
                is_update: false,
            },
            Step {
                a: a_neg,
                b: b_neg,
                is_reset: false,
                is_update: true,
            },
            Step {
                a: a_pos,
                b: b_pos,
                is_reset: false,
                is_update: true,
            },
            Step {
                a: a_pos,
                b: b_pos,
                is_reset: false,
                is_update: true,
            },
        ];
        let trace = build_trace(&steps);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &MatmulCumsumChip, &proof, &[])
            .expect("extreme i8 values must verify");
    }

    #[test]
    fn build_trace_pads_to_power_of_two() {
        let a = make_a_pattern_1();
        let b = make_b_pattern_1();
        let steps = vec![Step {
            a,
            b,
            is_reset: true,
            is_update: false,
        }];
        let trace = build_trace(&steps);
        // Should pad to 4 rows minimum.
        assert_eq!(trace.values.len(), 4 * cols::ROW_W);
    }

    #[test]
    fn chip_width_pinned() {
        assert_eq!(cols::ROW_W, 71);
        assert_eq!(cols::A_UNPACK_LEN, 32);
        assert_eq!(cols::B_UNPACK_LEN, 32);
        assert_eq!(cols::CUMSUM_LEN, 4);
    }
}
