//! R-b — stripe-major per-stripe XOR reduce (Pearl-parity
//! arbitrary num_stripes).
//!
//! ## What it proves
//!
//! R-b reorders the matmul sweep to STRIPE-MAJOR and holds the full
//! `h·w` tile accumulator across stripes (bounded by `h·w ≤ 256`), so
//! each stripe's per-stripe scalar
//!
//! ```text
//!   x_step[s] = ⊕ over all t·t accumulator cells after stripe s
//! ```
//!
//! (the exact quantity `ai-pow::matmul::compute_tile_trace` computes at
//! `matmul.rs:301-305`) can be folded into the 16-slot FoldChip
//! IMMEDIATELY — with no 64-lane register to overflow, so num_stripes is
//! unbounded (up to Pearl's degree-bits ceiling). This chip is the
//! reduce: as the stripe-major sweep finalizes each sub-block's 4-cell
//! accumulator-after-stripe-`s`, it XORs those 4 cells into a SINGLE
//! running lane `XSTEP` that RESETS at each stripe boundary. After the
//! stripe's last sub-block, `XSTEP` holds `x_step[s]` and the composite
//! feeds it to `FOLD_XSTEP`.
//!
//! It is a 1-lane specialization of [`super::stripe_xor::StripeXorChip`]
//! (`STATE_LEN = 64`, reset per SEGMENT) collapsed to `STATE_LEN = 1`
//! with a per-STRIPE reset — which is exactly what stripe-major makes
//! possible: x_steps are produced and consumed one stripe at a time, so
//! only one lane is ever live.
//!
//! ## Why per-bit parity, not a lookup
//!
//! XOR is not field-native. Each output bit is the parity of the 5
//! contributing bits (`XSTEP[lane]` + the 4 `IN` cells) at that
//! position — identical to StripeXor:
//!
//! ```text
//!   XSTEP_bit[i] + Σ_{c<4} IN_bit[c][i]  ==  NEW_bit[i] + 2·Q[i]
//! ```
//!
//! with `NEW_bit[i]` boolean and `Q[i] ∈ {0,1,2}` (column sum ≤ 5),
//! range-bounded by the cubic `Q·(Q−1)·(Q−2) = 0`.
//!
//! Self-contained: `ai-pow-zk` must NOT depend on `ai-pow`. The
//! reference is a local hand-rolled XOR fold; cross-crate parity vs
//! `compute_tile_trace`'s real `x_steps` is asserted from the `ai-pow`
//! side under the `zk` feature (the legal direction), like FoldChip /
//! StripeXor.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// Accumulator cells folded per row (one sub-block's `CUMSUM_TILE`).
pub const IN_LEN: usize = 4;

/// Chip-local column offsets.
pub mod cols {
    use super::IN_LEN;

    /// 1 = active reduce row (a sub-block's stripe-complete accumulator),
    /// 0 = padding/passthrough.
    pub const IS_ACTIVE: usize = 0;
    /// 1 = first active row of a stripe — the running `XSTEP` entering it
    /// is forced to 0 (a fresh stripe). Verifier-fixed in the composite.
    pub const STRIPE_RESET: usize = IS_ACTIVE + 1;
    /// The `IN_LEN` accumulator cells folded this row (u32 view of the
    /// i32 sub-block `CUMSUM` after this stripe).
    pub const IN: usize = STRIPE_RESET + 1;
    /// 32 LE bits per `IN` cell.
    pub const IN_BITS: usize = IN + IN_LEN;
    pub const IN_BITS_LEN: usize = IN_LEN * 32;
    /// The single running XOR lane ENTERING this row.
    pub const XSTEP: usize = IN_BITS + IN_BITS_LEN;
    /// 32 LE bits of `XSTEP` (the operand the parity reads).
    pub const XSTEP_BITS: usize = XSTEP + 1;
    pub const XSTEP_BITS_LEN: usize = 32;
    /// New value for the lane (`= XSTEP ⊕ ⊕IN`).
    pub const NEW: usize = XSTEP_BITS + XSTEP_BITS_LEN;
    /// 32 LE bits of `NEW`.
    pub const NEW_BITS: usize = NEW + 1;
    pub const NEW_BITS_LEN: usize = 32;
    /// Per output-bit parity quotient `Q[i] ∈ {0,1,2}` (cubic-bounded).
    pub const Q: usize = NEW_BITS + NEW_BITS_LEN;
    pub const Q_LEN: usize = 32;
    pub const ROW_W: usize = Q + Q_LEN;
}

/// Zero-sized chip type.
#[derive(Debug, Default, Clone, Copy)]
pub struct TileReduceChip;

/// Column-offset bundle so the eval body runs both standalone and (later)
/// at composite-layout offsets.
#[derive(Copy, Clone, Debug)]
pub struct TileReduceOffsets {
    pub is_active: usize,
    pub stripe_reset: usize,
    pub in_cells: usize,
    pub in_bits: usize,
    pub xstep: usize,
    pub xstep_bits: usize,
    pub new: usize,
    pub new_bits: usize,
    pub q: usize,
}

impl TileReduceChip {
    pub const LOCAL_OFFSETS: TileReduceOffsets = TileReduceOffsets {
        is_active: cols::IS_ACTIVE,
        stripe_reset: cols::STRIPE_RESET,
        in_cells: cols::IN,
        in_bits: cols::IN_BITS,
        xstep: cols::XSTEP,
        xstep_bits: cols::XSTEP_BITS,
        new: cols::NEW,
        new_bits: cols::NEW_BITS,
        q: cols::Q,
    };

    /// Composite-trace offsets (R-b). Vacuous when all TR_* are 0.
    pub const COMPOSITE_OFFSETS: TileReduceOffsets = TileReduceOffsets {
        is_active: crate::composite_layout::TR_IS_ACTIVE,
        stripe_reset: crate::composite_layout::TR_STRIPE_RESET,
        in_cells: crate::composite_layout::TR_IN_START,
        in_bits: crate::composite_layout::TR_IN_BITS_START,
        xstep: crate::composite_layout::TR_XSTEP,
        xstep_bits: crate::composite_layout::TR_XSTEP_BITS_START,
        new: crate::composite_layout::TR_NEW,
        new_bits: crate::composite_layout::TR_NEW_BITS_START,
        q: crate::composite_layout::TR_Q_START,
    };

    /// Composite entry point. Vacuous on all pre-R-b traces.
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);
    }

    /// Emit the reduce constraints at the given offsets.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &TileReduceOffsets) {
        let two = <AB::F as PrimeCharacteristicRing>::TWO;

        // ---- IS_ACTIVE + STRIPE_RESET boolean (UNGATED) ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            builder.assert_bool(cur[off.is_active]);
        }
        {
            let main = builder.main();
            let cur = main.current_slice();
            builder.assert_bool(cur[off.stripe_reset]);
        }

        // ---- STRIPE_RESET ⇒ lane entering this row is 0 ----
        // (degree 2). Verifier-fixed reset ⇒ each stripe's XSTEP starts
        // fresh; a prior stripe's value never carries in.
        {
            let (reset, xstep): (AB::Var, AB::Var) = {
                let main = builder.main();
                let cur = main.current_slice();
                (cur[off.stripe_reset], cur[off.xstep])
            };
            builder.assert_zero(reset.into() * xstep.into());
        }

        // ---- Q range check — UNGATED (degree-3 cubic) ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            for i in 0..32 {
                let q: AB::Expr = cur[off.q + i].into();
                let one = <AB::Expr as PrimeCharacteristicRing>::ONE;
                builder.assert_zero(q.clone() * (q.clone() - one) * (q.clone() - two.clone()));
            }
        }

        // ---- Per-row data validation — GATED by IS_ACTIVE ----
        {
            let is_active: AB::Expr = {
                let main = builder.main();
                main.current_slice()[off.is_active].into()
            };
            let mut b = builder.when(is_active);
            let main = b.main();
            let cur = main.current_slice();

            // IN cells are signed-i32 (the matmul CUMSUM encoding); IN_BITS
            // is the two's-complement pattern. The XOR uses the raw bit
            // pattern, so it is sign-agnostic.
            let two_pow_32 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << 32);
            for c in 0..IN_LEN {
                let mut recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
                let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
                for i in 0..32 {
                    let bit = cur[off.in_bits + c * 32 + i];
                    b.assert_bool(bit);
                    recon += bit.into() * pow.clone();
                    pow *= two.clone();
                }
                let sign: AB::Expr = cur[off.in_bits + c * 32 + 31].into() * two_pow_32.clone();
                b.assert_eq(cur[off.in_cells + c].into(), recon - sign);
            }

            // XSTEP_BITS is the bit-decomposition of the lane entering.
            let mut x_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut powx: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                let bit = cur[off.xstep_bits + i];
                b.assert_bool(bit);
                x_recon += bit.into() * powx.clone();
                powx *= two.clone();
            }
            b.assert_eq(x_recon, cur[off.xstep].into());

            // NEW == Σ NEW_BITS[i]·2^i (bits boolean).
            let mut new_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pown: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                let bit = cur[off.new_bits + i];
                b.assert_bool(bit);
                new_recon += bit.into() * pown.clone();
                pown *= two.clone();
            }
            b.assert_eq(cur[off.new].into(), new_recon);

            // Per output bit i: parity of the 5 contributing bits
            // (XSTEP + the IN_LEN cells) == NEW_BITS[i] + 2·Q[i].
            for i in 0..32 {
                let mut col_sum: AB::Expr = cur[off.xstep_bits + i].into();
                for c in 0..IN_LEN {
                    col_sum += cur[off.in_bits + c * 32 + i].into();
                }
                let nbit = cur[off.new_bits + i];
                let q: AB::Expr = cur[off.q + i].into();
                b.assert_eq(col_sum, nbit.into() + q * two.clone());
            }
        }

        // ---- Boundary: first row lane is zero ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut fr = builder.when_first_row();
            fr.assert_zero(cur[off.xstep]);
        }

        // ---- Cross-row lane update ----
        //
        //   XSTEP_next = NEW                        (active, not reset-next)
        //   XSTEP_next = XSTEP                      (padding, not reset-next)
        //   XSTEP_next forced 0 by the reset constraint on a reset-next row
        {
            let (is_active, xstep, new): (AB::Var, AB::Var, AB::Var) = {
                let main = builder.main();
                let cur = main.current_slice();
                (cur[off.is_active], cur[off.xstep], cur[off.new])
            };
            let (nxt_xstep, nxt_reset): (AB::Var, AB::Var) = {
                let main = builder.main();
                let nxt = main.next_slice();
                (nxt[off.xstep], nxt[off.stripe_reset])
            };
            let mut tb = builder.when_transition();
            // Not into a reset row: active ⇒ next = NEW; inactive ⇒ next = XSTEP.
            let not_reset: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ONE - nxt_reset.into();
            // active: (1-reset_next)·is_active·(nxt - NEW) = 0
            tb.assert_zero(not_reset.clone() * is_active.into() * (nxt_xstep.into() - new.into()));
            // inactive: (1-reset_next)·(1-is_active)·(nxt - XSTEP) = 0
            let one_minus_active: AB::Expr =
                <AB::Expr as PrimeCharacteristicRing>::ONE - is_active.into();
            tb.assert_zero(not_reset * one_minus_active * (nxt_xstep.into() - xstep.into()));
        }
    }
}

impl<F> BaseAir<F> for TileReduceChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for TileReduceChip {
    fn eval(&self, builder: &mut AB) {
        TileReduceChip::eval_at(builder, &TileReduceChip::LOCAL_OFFSETS);
    }
}

/// Reference: for each stripe, XOR every sub-block's 4 cells into that
/// stripe's `x_step` (the whole tile accumulator after the stripe). One
/// `x_step` per stripe. `stripes[s]` is the list of that stripe's
/// sub-block CUMSUM-after-stripe 4-tuples (their XOR == `x_step[s]`).
pub fn ref_x_steps(stripes: &[Vec<[i32; IN_LEN]>]) -> Vec<u32> {
    stripes
        .iter()
        .map(|subblocks| {
            let mut x = 0u32;
            for sb in subblocks {
                for &v in sb {
                    x ^= v as u32;
                }
            }
            x
        })
        .collect()
}

/// Build a standalone trace: for each stripe, one active row per
/// sub-block (STRIPE_RESET on the stripe's first). Padded to the next
/// power of two (≥ 4). Returns `(trace, stripe_last_rows)` where
/// `stripe_last_rows[s]` is the row whose `NEW` == `x_step[s]`.
pub fn build_trace(stripes: &[Vec<[i32; IN_LEN]>]) -> (RowMajorMatrix<Val>, Vec<usize>) {
    use p3_field::integers::QuotientMap;

    let total: usize = stripes.iter().map(|s| s.len()).sum();
    assert!(total > 0, "stripes must contain sub-blocks");
    let n = (total + 1).next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let set_bits = |row: &mut [Val], at: usize, v: u32| {
        for i in 0..32 {
            row[at + i] = <Val as QuotientMap<u64>>::from_int(((v >> i) & 1) as u64);
        }
    };

    let mut xstep = 0u32;
    let mut row_idx = 0usize;
    let mut stripe_last = Vec::with_capacity(stripes.len());

    for subblocks in stripes {
        for (sb_i, sb) in subblocks.iter().enumerate() {
            if sb_i == 0 {
                xstep = 0; // fresh stripe
            }
            let row = &mut flat[row_idx * cols::ROW_W..(row_idx + 1) * cols::ROW_W];
            row[cols::IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(1);
            if sb_i == 0 {
                row[cols::STRIPE_RESET] = <Val as QuotientMap<u64>>::from_int(1);
            }
            let mut xin = 0u32;
            for c in 0..IN_LEN {
                let u = sb[c] as u32;
                row[cols::IN + c] = <Val as QuotientMap<i64>>::from_int(sb[c] as i64);
                set_bits(row, cols::IN_BITS + c * 32, u);
                xin ^= u;
            }
            row[cols::XSTEP] = <Val as QuotientMap<u64>>::from_int(xstep as u64);
            set_bits(row, cols::XSTEP_BITS, xstep);
            let new = xstep ^ xin;
            row[cols::NEW] = <Val as QuotientMap<u64>>::from_int(new as u64);
            set_bits(row, cols::NEW_BITS, new);
            for i in 0..32 {
                let mut col_sum: u32 = (xstep >> i) & 1;
                for c in 0..IN_LEN {
                    col_sum += (sb[c] as u32 >> i) & 1;
                }
                let q = (col_sum - ((new >> i) & 1)) / 2;
                row[cols::Q + i] = <Val as QuotientMap<u64>>::from_int(q as u64);
            }
            xstep = new;
            row_idx += 1;
        }
        stripe_last.push(row_idx - 1);
    }

    // Padding rows: lane passthrough (holds the last stripe's value).
    for r in row_idx..n {
        let row = &mut flat[r * cols::ROW_W..(r + 1) * cols::ROW_W];
        row[cols::XSTEP] = <Val as QuotientMap<u64>>::from_int(xstep as u64);
    }

    (RowMajorMatrix::new(flat, cols::ROW_W), stripe_last)
}

/// Read the `NEW` lane of a row (== `x_step[s]` on a stripe's last row).
pub fn new_at_row(trace: &RowMajorMatrix<Val>, row: usize) -> u32 {
    use p3_field::PrimeField64;
    trace.values[row * cols::ROW_W + cols::NEW].as_canonical_u64() as u32
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
                (s >> 32) as i32
            })
            .collect()
    }

    /// Stripe-major reduce: each stripe's `x_step` (read at its last row's
    /// NEW) equals the XOR of that stripe's sub-block cells, and the whole
    /// trace proves+verifies — spanning num_stripes up to Pearl's 512
    /// ceiling with an h·w=256 tile (64 sub-blocks/stripe).
    #[test]
    fn stripe_reduce_matches_reference_and_verifies() {
        let c = cfg();
        for (n_stripes, n_sb) in [(1usize, 1usize), (16, 4), (65, 4), (128, 2), (512, 1)] {
            let raw = lcg(0xC0DE ^ (n_stripes as u64), n_stripes * n_sb * IN_LEN);
            let stripes: Vec<Vec<[i32; IN_LEN]>> = (0..n_stripes)
                .map(|s| {
                    (0..n_sb)
                        .map(|sb| {
                            let base = (s * n_sb + sb) * IN_LEN;
                            core::array::from_fn(|k| raw[base + k])
                        })
                        .collect()
                })
                .collect();
            let refs = ref_x_steps(&stripes);
            let (trace, last) = build_trace(&stripes);
            for s in 0..n_stripes {
                assert_eq!(
                    new_at_row(&trace, last[s]),
                    refs[s],
                    "n_stripes={n_stripes} n_sb={n_sb}: stripe {s} x_step vs reference",
                );
            }
            let proof = prove::<AiPowStarkConfig, _>(&c, &TileReduceChip, trace, &[]);
            verify::<AiPowStarkConfig, _>(&c, &TileReduceChip, &proof, &[]).unwrap_or_else(|e| {
                panic!("honest stripe-reduce (n_stripes={n_stripes}) must verify: {e:?}")
            });
        }
    }

    fn honest() -> (RowMajorMatrix<Val>, Vec<usize>) {
        let raw = lcg(7, 16 * 4 * IN_LEN);
        let stripes: Vec<Vec<[i32; IN_LEN]>> = (0..16)
            .map(|s| {
                (0..4)
                    .map(|sb| {
                        let base = (s * 4 + sb) * IN_LEN;
                        core::array::from_fn(|k| raw[base + k])
                    })
                    .collect()
            })
            .collect();
        build_trace(&stripes)
    }

    #[test]
    fn rejects_missing_stripe_reset() {
        let c = cfg();
        let (mut t, _) = honest();
        // Clear STRIPE_RESET on stripe 1's first row (row 4) ⇒ the prior
        // stripe's lane carries in ⇒ passthrough breaks (lane != 0).
        let r = 4;
        assert_eq!(
            t.values[r * cols::ROW_W + cols::STRIPE_RESET],
            <Val as QuotientMap<u64>>::from_int(1),
            "row {r} should be stripe 1's reset row",
        );
        t.values[r * cols::ROW_W + cols::STRIPE_RESET] = <Val as QuotientMap<u64>>::from_int(0);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileReduceChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileReduceChip, &p, &[]).is_err(),
            "clearing a stripe reset must break the trace",
        );
    }

    #[test]
    fn rejects_tampered_new_without_bits() {
        let c = cfg();
        let (mut t, _) = honest();
        t.values[2 * cols::ROW_W + cols::NEW] = <Val as QuotientMap<u64>>::from_int(0x7);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileReduceChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileReduceChip, &p, &[]).is_err(),
            "NEW inconsistent with its bits must reject",
        );
    }

    #[test]
    fn rejects_out_of_range_q() {
        let c = cfg();
        let (mut t, _) = honest();
        t.values[cols::Q] = <Val as QuotientMap<u64>>::from_int(3);
        let p = prove::<AiPowStarkConfig, _>(&c, &TileReduceChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &TileReduceChip, &p, &[]).is_err(),
            "out-of-range Q (=3) must reject",
        );
    }
}
