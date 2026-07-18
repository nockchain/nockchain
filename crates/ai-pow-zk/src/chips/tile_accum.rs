//! §6(b)-R-b — held `h·w` tile accumulator (stripe-major).
//!
//! ## What it proves
//!
//! R-b sweeps STRIPE-MAJOR and holds the FULL `h·w` tile accumulator
//! across stripes, so each sub-block's cumulative accumulator survives
//! from one stripe to the next (bounded by `h·w ≤ 256`). Each active row
//! is one `(stripe, sub-block)` micro-step: it adds that sub-block's
//! stripe dot into the sub-block's 4 cells of the held accumulator `ACC`
//! and passes every other cell through. The per-sub-block reset fires on
//! stripe 0 (`IS_RESET`), so `ACC[sub-block]` starts at that stripe's dot
//! and accumulates over stripes 1..num_stripes — exactly
//! `ai-pow::matmul::compute_tile_trace`'s `c_blk` recurrence
//! (matmul.rs:296-298), just visited stripe-major.
//!
//! The additive recurrence per selected cell mirrors the matmul chip's
//! `cumsum_new = (is_reset+is_update)·dot + (1−is_reset)·cumsum_old`
//! (`chips/matmul/chip.rs:14`); here it is per-cell, gated by a one-hot
//! sub-block selector, with every non-selected cell held:
//!
//! ```text
//!   ACC_next[c] = ACC[c]·(1 − SB_SEL[c/4]·IS_RESET) + SB_SEL[c/4]·DOT[c%4]
//! ```
//!
//! (degree 3 — within the pinned budget). The reduce
//! ([`super::tile_reduce`]) then XORs each sub-block's post-update cells
//! into the per-stripe `x_step`; the composite feeds `DOT` from the real
//! noised matmul (the dot machinery stays in the matmul chip).
//!
//! Self-contained: `ai-pow-zk` must NOT depend on `ai-pow`; the reference
//! is a local hand-rolled accumulate, and cross-crate `c_blk` parity is
//! asserted from the `ai-pow` side under the `zk` feature.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// Cells per sub-block (`TILE_H × TILE_H` = 2×2).
pub const SB_CELLS: usize = 4;
/// Max sub-blocks the standalone chip lays out (`h·w / 4 ≤ 256/4 = 64`).
pub const MAX_SB: usize = 64;
/// Max held cells (`h·w ≤ 256`).
pub const MAX_CELLS: usize = MAX_SB * SB_CELLS;

pub mod cols {
    use super::{MAX_CELLS, MAX_SB, SB_CELLS};

    /// 1 = active `(stripe, sub-block)` micro-step, 0 = padding.
    pub const IS_ACTIVE: usize = 0;
    /// 1 = stripe-0 row (reset the selected sub-block's cells).
    pub const IS_RESET: usize = IS_ACTIVE + 1;
    /// One-hot sub-block selector (`Σ == IS_ACTIVE`).
    pub const SB_SEL: usize = IS_RESET + 1;
    pub const SB_SEL_LEN: usize = MAX_SB;
    /// The selected sub-block's stripe dot (`SB_CELLS` i32 cells).
    pub const DOT: usize = SB_SEL + SB_SEL_LEN;
    /// The held `h·w` accumulator ENTERING this row.
    pub const ACC: usize = DOT + SB_CELLS;
    pub const ACC_LEN: usize = MAX_CELLS;
    pub const ROW_W: usize = ACC + ACC_LEN;
}

/// Zero-sized chip type.
#[derive(Debug, Default, Clone, Copy)]
pub struct TileAccumChip;

#[derive(Copy, Clone, Debug)]
pub struct TileAccumOffsets {
    pub is_active: usize,
    pub is_reset: usize,
    pub sb_sel: usize,
    pub dot: usize,
    pub acc: usize,
    /// Number of sub-blocks actually laid out (`h·w/4`); cells beyond
    /// `n_sb·SB_CELLS` are inert (held 0). `SB_SEL` beyond `n_sb` is 0.
    pub n_sb: usize,
}

impl TileAccumChip {
    pub const LOCAL_OFFSETS: TileAccumOffsets = TileAccumOffsets {
        is_active: cols::IS_ACTIVE,
        is_reset: cols::IS_RESET,
        sb_sel: cols::SB_SEL,
        dot: cols::DOT,
        acc: cols::ACC,
        n_sb: MAX_SB,
    };

    /// Composite-trace offsets (§6(b)-R-b). Vacuous when all TA_* are 0.
    pub const COMPOSITE_OFFSETS: TileAccumOffsets = TileAccumOffsets {
        is_active: crate::composite_layout::TA_IS_ACTIVE,
        is_reset: crate::composite_layout::TA_IS_RESET,
        sb_sel: crate::composite_layout::TA_SB_SEL_START,
        dot: crate::composite_layout::TA_DOT_START,
        acc: crate::composite_layout::TA_ACC_START,
        n_sb: MAX_SB,
    };

    /// Composite entry point. Vacuous on all pre-R-b traces.
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);
    }

    /// Emit the accumulator constraints at the given offsets.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &TileAccumOffsets) {
        // ---- IS_ACTIVE / IS_RESET boolean; SB_SEL one-hot (Σ==IS_ACTIVE) ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            builder.assert_bool(cur[off.is_active]);
        }
        {
            let main = builder.main();
            let cur = main.current_slice();
            builder.assert_bool(cur[off.is_reset]);
        }
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..off.n_sb {
                let sel = cur[off.sb_sel + s];
                builder.assert_bool(sel);
                sum += sel.into();
            }
            builder.assert_eq(sum, cur[off.is_active].into());
        }
        // IS_RESET may only fire on an active row (else a padding row could
        // wipe the accumulator): IS_RESET·(1−IS_ACTIVE) = 0.
        {
            let (reset, active): (AB::Var, AB::Var) = {
                let main = builder.main();
                let cur = main.current_slice();
                (cur[off.is_reset], cur[off.is_active])
            };
            let one = <AB::Expr as PrimeCharacteristicRing>::ONE;
            builder.assert_zero(reset.into() * (one - active.into()));
        }

        // ---- Boundary: first row accumulator is zero ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut fr = builder.when_first_row();
            for c in 0..off.n_sb * SB_CELLS {
                fr.assert_zero(cur[off.acc + c]);
            }
        }

        // ---- Cross-row selective additive update ----
        //   ACC_next[c] = ACC[c]·(1 − SB_SEL[c/4]·IS_RESET) + SB_SEL[c/4]·DOT[c%4]
        {
            let n = off.n_sb * SB_CELLS;
            let (is_reset, sb_sel, dot, acc): (
                AB::Var,
                Vec<AB::Var>,
                [AB::Var; SB_CELLS],
                Vec<AB::Var>,
            ) = {
                let main = builder.main();
                let cur = main.current_slice();
                (
                    cur[off.is_reset],
                    (0..off.n_sb).map(|s| cur[off.sb_sel + s]).collect(),
                    core::array::from_fn(|p| cur[off.dot + p]),
                    (0..n).map(|c| cur[off.acc + c]).collect(),
                )
            };
            let nxt_acc: Vec<AB::Var> = {
                let main = builder.main();
                let nxt = main.next_slice();
                (0..n).map(|c| nxt[off.acc + c]).collect()
            };
            let mut tb = builder.when_transition();
            let one = <AB::Expr as PrimeCharacteristicRing>::ONE;
            for c in 0..n {
                let sb = c / SB_CELLS;
                let pos = c % SB_CELLS;
                let sel: AB::Expr = sb_sel[sb].into();
                // keep = 1 − sel·is_reset (degree 2)
                let keep: AB::Expr = one.clone() - sel.clone() * is_reset.into();
                // ACC_next[c] == ACC[c]·keep + sel·DOT[pos]   (degree 3)
                let want: AB::Expr = acc[c].into() * keep + sel * dot[pos].into();
                tb.assert_zero(nxt_acc[c].into() - want);
            }
        }
    }
}

impl<F> BaseAir<F> for TileAccumChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for TileAccumChip {
    fn eval(&self, builder: &mut AB) {
        TileAccumChip::eval_at(builder, &TileAccumChip::LOCAL_OFFSETS);
    }
}

/// Reference: `c_blk[h·w]` accumulated stripe-major. `dots[s][sb]` is
/// sub-block `sb`'s stripe-`s` dot (`SB_CELLS` i32). Reset at stripe 0.
/// Returns the final accumulator and the per-`(stripe, sub-block)`
/// post-update sub-block cells (what the reduce consumes).
pub fn ref_accumulate(
    n_sb: usize,
    dots: &[Vec<[i32; SB_CELLS]>],
) -> (Vec<i32>, Vec<Vec<[i32; SB_CELLS]>>) {
    let mut acc = vec![0i32; n_sb * SB_CELLS];
    let mut per_stripe_sb = Vec::with_capacity(dots.len());
    for (s, stripe) in dots.iter().enumerate() {
        let mut sb_cells = Vec::with_capacity(n_sb);
        for (sb, dot) in stripe.iter().enumerate() {
            for p in 0..SB_CELLS {
                let cell = sb * SB_CELLS + p;
                acc[cell] = if s == 0 {
                    dot[p]
                } else {
                    acc[cell].wrapping_add(dot[p])
                };
            }
            sb_cells.push(core::array::from_fn(|p| acc[sb * SB_CELLS + p]));
        }
        per_stripe_sb.push(sb_cells);
    }
    (acc, per_stripe_sb)
}

/// Build a standalone trace: stripe-major, one active row per
/// `(stripe, sub-block)`. `IS_RESET` on stripe 0. Padded to the next
/// power of two (≥ 4). Returns `(trace, n_sb)`.
pub fn build_trace(n_sb: usize, dots: &[Vec<[i32; SB_CELLS]>]) -> RowMajorMatrix<Val> {
    use p3_field::integers::QuotientMap;
    assert!(n_sb <= MAX_SB, "n_sb exceeds MAX_SB");
    let total: usize = dots.iter().map(|s| s.len()).sum();
    assert!(total > 0, "dots must contain sub-blocks");
    let n = (total + 1).next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let mut acc = vec![0i32; n_sb * SB_CELLS];
    let mut row_idx = 0usize;
    for (s, stripe) in dots.iter().enumerate() {
        for (sb, dot) in stripe.iter().enumerate() {
            let row = &mut flat[row_idx * cols::ROW_W..(row_idx + 1) * cols::ROW_W];
            row[cols::IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(1);
            if s == 0 {
                row[cols::IS_RESET] = <Val as QuotientMap<u64>>::from_int(1);
            }
            row[cols::SB_SEL + sb] = <Val as QuotientMap<u64>>::from_int(1);
            for p in 0..SB_CELLS {
                row[cols::DOT + p] = <Val as QuotientMap<i64>>::from_int(dot[p] as i64);
            }
            // ACC entering this row.
            for c in 0..n_sb * SB_CELLS {
                row[cols::ACC + c] = <Val as QuotientMap<i64>>::from_int(acc[c] as i64);
            }
            // Apply the update to `acc` for the next row.
            for p in 0..SB_CELLS {
                let cell = sb * SB_CELLS + p;
                acc[cell] = if s == 0 {
                    dot[p]
                } else {
                    acc[cell].wrapping_add(dot[p])
                };
            }
            row_idx += 1;
        }
    }
    for r in row_idx..n {
        let row = &mut flat[r * cols::ROW_W..(r + 1) * cols::ROW_W];
        for c in 0..n_sb * SB_CELLS {
            row[cols::ACC + c] = <Val as QuotientMap<i64>>::from_int(acc[c] as i64);
        }
    }
    RowMajorMatrix::new(flat, cols::ROW_W)
}

/// Read the held accumulator from a row (i32).
pub fn acc_at_row(trace: &RowMajorMatrix<Val>, row: usize, n_cells: usize) -> Vec<i32> {
    use p3_field::PrimeField64;
    const P: u64 = 0xffff_ffff_0000_0001;
    (0..n_cells)
        .map(|c| {
            let v = trace.values[row * cols::ROW_W + cols::ACC + c].as_canonical_u64();
            // Field → signed i32 (values are small i32s embedded in Goldilocks).
            if v > P / 2 {
                (v.wrapping_sub(P)) as i64 as i32
            } else {
                v as i32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use p3_field::integers::QuotientMap;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::params::ZkParams;

    fn cfg() -> AiPowStarkConfig {
        build_stark_config(
            &ZkParams {
                m: 8,
                k: 16,
                n: 8,
                noise_rank: 2,
                tile: 2,
                difficulty_bits: 0,
            },
            &CircuitConfig::TEST_PEARL,
        )
    }

    fn lcg(seed: u64, n: usize) -> Vec<i32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                // small magnitudes so wrapping sums stay well within i32/field.
                ((s >> 40) as i32) % 4096
            })
            .collect()
    }

    /// Stripe-major hold: the accumulator's final cells equal the
    /// reference `c_blk`, and the whole trace proves+verifies — spanning
    /// num_stripes up to Pearl's 512 ceiling with h·w=16 (4 sub-blocks).
    #[test]
    fn accumulator_matches_reference_and_verifies() {
        let c = cfg();
        for (n_stripes, n_sb) in [(1usize, 1usize), (16, 4), (65, 4), (128, 2), (512, 1)] {
            let raw = lcg(0xACC ^ (n_stripes as u64), n_stripes * n_sb * SB_CELLS);
            let dots: Vec<Vec<[i32; SB_CELLS]>> = (0..n_stripes)
                .map(|s| {
                    (0..n_sb)
                        .map(|sb| {
                            let base = (s * n_sb + sb) * SB_CELLS;
                            core::array::from_fn(|k| raw[base + k])
                        })
                        .collect()
                })
                .collect();
            let (want_acc, _) = ref_accumulate(n_sb, &dots);
            let trace = build_trace(n_sb, &dots);
            let h = trace.values.len() / cols::ROW_W;
            assert_eq!(
                acc_at_row(&trace, h - 1, n_sb * SB_CELLS),
                want_acc,
                "n_stripes={n_stripes} n_sb={n_sb}: final accumulator vs reference c_blk",
            );
            let proof = prove::<AiPowStarkConfig, _>(&c, &TileAccumChip, trace, &[]);
            verify::<AiPowStarkConfig, _>(&c, &TileAccumChip, &proof, &[]).unwrap_or_else(|e| {
                panic!("honest accumulator (n_stripes={n_stripes}) must verify: {e:?}")
            });
        }
    }

    fn honest() -> RowMajorMatrix<Val> {
        let raw = lcg(11, 16 * 4 * SB_CELLS);
        let dots: Vec<Vec<[i32; SB_CELLS]>> = (0..16)
            .map(|s| {
                (0..4)
                    .map(|sb| {
                        let base = (s * 4 + sb) * SB_CELLS;
                        core::array::from_fn(|k| raw[base + k])
                    })
                    .collect()
            })
            .collect();
        build_trace(4, &dots)
    }

    #[test]
    fn rejects_tampered_accumulator() {
        let c = cfg();
        let mut t = honest();
        let h = t.values.len() / cols::ROW_W;
        t.values[(h - 2) * cols::ROW_W + cols::ACC + 3] =
            <Val as QuotientMap<u64>>::from_int(0xDEAD);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileAccumChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileAccumChip, &p, &[]).is_err(),
            "tampered ACC must reject",
        );
    }

    #[test]
    fn rejects_nonselected_cell_mutation() {
        let c = cfg();
        let mut t = honest();
        // Change the final accumulator on the last row only ⇒ the prior
        // transition's passthrough for a non-selected cell breaks.
        let h = t.values.len() / cols::ROW_W;
        t.values[(h - 1) * cols::ROW_W + cols::ACC + 7] =
            <Val as QuotientMap<u64>>::from_int(0x1234);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileAccumChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileAccumChip, &p, &[]).is_err(),
            "non-selected-cell mutation must break the additive recurrence",
        );
    }

    #[test]
    fn rejects_reset_on_later_stripe() {
        let c = cfg();
        let mut t = honest();
        // Set IS_RESET on a stripe-1 row (row 4) ⇒ that sub-block's
        // accumulator is wrongly wiped ⇒ the recurrence (accumulate) breaks.
        t.values[4 * cols::ROW_W + cols::IS_RESET] = <Val as QuotientMap<u64>>::from_int(1);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileAccumChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileAccumChip, &p, &[]).is_err(),
            "spurious IS_RESET must break the honest accumulate recurrence",
        );
    }
}
