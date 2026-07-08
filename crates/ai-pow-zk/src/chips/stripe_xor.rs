//! Stripe-XOR transport chip — HIGH-2.2 §6(b) cross-row binding.
//!
//! ## What it proves
//!
//! The matmul sub-block-major sweep (HIGH-2.2 §6(b) spike GATE 2)
//! produces, for each `(sub_block sb, stripe step)`, a 4-cell
//! `CUMSUM` accumulator-after-step. Pearl §4.5's per-stripe scalar
//! is `x_steps[step] = ⊕` over **all** `t·t` accumulator cells
//! after stripe `step` — i.e. the XOR, across every sub-block, of
//! that sub-block's 4 accumulator cells at stripe `step`.
//!
//! Because the sweep is sub-block-major, the 16 sub-blocks'
//! accumulator-after-`step` values land on 16 **non-adjacent**
//! rows. This chip is the cross-row transport that reduces them to
//! the per-stripe scalar without a lookup bus: a carried
//! `STATE_LEN`-lane i32 register `XR`, where row `r` folds its
//! 4-cell input `IN` into the lane selected by a one-hot
//! `LANE_SEL` (the stripe index, schedule-pinned in the composite
//! via the §6(a) `CONTROL_PREP` pattern):
//!
//! ```text
//!   XR_next[lane] = XR[lane] ⊕ IN[0] ⊕ IN[1] ⊕ IN[2] ⊕ IN[3]
//!   XR_next[s]    = XR[s]                              (s ≠ lane)
//!   first row:    XR == 0
//! ```
//!
//! After every sub-block has been folded, `XR[step]` holds
//! `x_steps[step]` (XOR is associative/commutative, so the
//! sub-block-major visitation order is irrelevant). The composite
//! wiring (§6(b) next step) feeds `IN` from the swept `CUMSUM`
//! cells and binds the final `XR[step]` to the FoldChip's
//! `FOLD_XSTEP[step]`.
//!
//! ## Why per-bit parity, not a lookup
//!
//! XOR is not field-native. Each output bit is the parity (sum
//! mod 2) of the 5 contributing bits (`XR[lane]` + the 4 `IN`
//! cells) at that position. We enforce
//!
//! ```text
//!   XR[lane]_bit[i] + Σ_{c<4} IN_bit[c][i]  ==  NEW_bit[i] + 2·Q[i]
//! ```
//!
//! with `NEW_bit[i]` boolean and `Q[i]` range-bounded to
//! `{0,1,2}` (column sum ≤ 5 ⇒ `NEW_bit + 2·Q ≤ 5` ⇒ `Q ≤ 2`).
//!
//! **2026-05-21 width reduction.** `Q[i]` is one value column per
//! output-bit position, range-constrained by the cubic
//! `Q·(Q−1)·(Q−2) = 0` — replacing the prior 2-boolean-column
//! decomposition (which over-provisioned the range to `{0,1,2,3}`
//! and cost double the width). 32 columns reclaimed. The cubic is
//! degree 3 — within the composite AIR's budget (Pearl pins
//! `constraint_degree = 3`). All constraints live in plain
//! `p3-uni-stark`.
//!
//! Self-contained: `ai-pow-zk` must NOT depend on `ai-pow`
//! (dependency cycle). The reference is a local hand-rolled
//! XOR-by-lane fold; cross-crate parity vs
//! `ai-pow::matmul::compute_tile_trace`'s real `x_steps` is
//! asserted from the `ai-pow` side under the `zk` feature (the
//! legal direction), exactly like the FoldChip / XStepChip.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// HIGH-2.2 §6(b)-G2 — number of per-stripe lanes
/// (`STRIPE_MAX`). **Decoupled from the FoldChip's Pearl-fixed 16
/// M-slots** (`JACKPOT_SIZE`): a tile has `num_stripes = k/r`
/// per-stripe `X_STEP`s, and the fold consumes each distinctly
/// (folding into M-slot `stripe % 16`), so the XOR register needs
/// one lane *per stripe*, not per M-slot. 64 covers every
/// single-Layer-0 params set (rectangular `llm_shape` `k/r = 20`;
/// PROD `k/r = 64`). A shorter schedule leaves the unused high
/// lanes at 0.
pub const STATE_LEN: usize = 64;
/// Accumulator cells folded per row (the in-circuit micro-tile's
/// `CUMSUM_TILE_LEN`).
pub const IN_LEN: usize = 4;

/// Chip-local column offsets.
pub mod cols {
    use super::{IN_LEN, STATE_LEN};

    /// 1 = active fold row, 0 = padding/passthrough.
    pub const IS_ACTIVE: usize = 0;
    /// One-hot lane selector (`Σ == IS_ACTIVE`).
    pub const LANE_SEL: usize = IS_ACTIVE + 1;
    pub const LANE_SEL_LEN: usize = STATE_LEN;
    /// The `IN_LEN` accumulator cells folded this row (u32 view of
    /// the i32 `CUMSUM`).
    pub const IN: usize = LANE_SEL + LANE_SEL_LEN;
    /// 32 LE bits per `IN` cell.
    pub const IN_BITS: usize = IN + IN_LEN;
    pub const IN_BITS_LEN: usize = IN_LEN * 32;
    /// `STATE_LEN`-lane register *entering* this row.
    pub const XR: usize = IN_BITS + IN_BITS_LEN;
    pub const XR_LEN: usize = STATE_LEN;
    /// 32 LE bits of the *selected* lane value
    /// (`Σ_s LANE_SEL[s]·XR[s]`).
    pub const XR_SEL_BITS: usize = XR + XR_LEN;
    pub const XR_SEL_BITS_LEN: usize = 32;
    /// The new value for the selected lane (`= XR[lane] ⊕ ⊕IN`).
    pub const NEW_SEL: usize = XR_SEL_BITS + XR_SEL_BITS_LEN;
    /// 32 LE bits of `NEW_SEL`.
    pub const NEW_SEL_BITS: usize = NEW_SEL + 1;
    pub const NEW_SEL_BITS_LEN: usize = 32;
    /// Parity quotient per output bit position. `Q[i] ∈ {0,1,2}`
    /// (the column sum of 5 contributing bits is ≤ 5, and
    /// `col_sum = NEW_bit + 2·Q` with `NEW_bit ∈ {0,1}` ⇒ `Q ≤ 2`).
    ///
    /// **2026-05-21 width reduction.** Stored as ONE value column
    /// per output-bit position, range-constrained by the cubic
    /// `Q·(Q−1)·(Q−2) = 0`. This replaces the prior `QBITS = 2`
    /// boolean columns per position (`Q_BITS_LEN = 64`), which
    /// over-provisioned the range to `{0,1,2,3}` and cost 2× the
    /// width. The cubic is degree-3 — within the composite AIR's
    /// degree budget (Pearl pins `constraint_degree = 3`; the
    /// `TEST_PEARL` `log_blowup = 2` tier admits degree 4). Net:
    /// `Q_LEN = 32` vs the prior 64 — 32 columns reclaimed.
    pub const Q: usize = NEW_SEL_BITS + NEW_SEL_BITS_LEN;
    pub const Q_LEN: usize = 32;
    pub const ROW_W: usize = Q + Q_LEN;
}

/// Zero-sized chip type.
#[derive(Debug, Default, Clone, Copy)]
pub struct StripeXorChip;

/// Column-offset bundle so the eval body runs both standalone and
/// (later) at composite-layout offsets.
#[derive(Copy, Clone, Debug)]
pub struct StripeXorOffsets {
    pub is_active: usize,
    pub lane_sel: usize,
    pub in_cells: usize,
    pub in_bits: usize,
    pub xr: usize,
    pub xr_sel_bits: usize,
    pub new_sel: usize,
    pub new_sel_bits: usize,
    pub q: usize,
}

impl StripeXorChip {
    pub const LOCAL_OFFSETS: StripeXorOffsets = StripeXorOffsets {
        is_active: cols::IS_ACTIVE,
        lane_sel: cols::LANE_SEL,
        in_cells: cols::IN,
        in_bits: cols::IN_BITS,
        xr: cols::XR,
        xr_sel_bits: cols::XR_SEL_BITS,
        new_sel: cols::NEW_SEL,
        new_sel_bits: cols::NEW_SEL_BITS,
        q: cols::Q,
    };

    /// Composite-trace offsets (HIGH-2.2 §6(b) wiring) — the
    /// StripeXorChip's columns at their `composite_layout`
    /// positions.
    pub const COMPOSITE_OFFSETS: StripeXorOffsets = StripeXorOffsets {
        is_active: crate::composite_layout::SX_IS_ACTIVE,
        lane_sel: crate::composite_layout::SX_LANE_SEL_START,
        in_cells: crate::composite_layout::SX_IN_START,
        in_bits: crate::composite_layout::SX_IN_BITS_START,
        xr: crate::composite_layout::SX_XR_START,
        xr_sel_bits: crate::composite_layout::SX_XR_SEL_BITS_START,
        new_sel: crate::composite_layout::SX_NEW_SEL,
        new_sel_bits: crate::composite_layout::SX_NEW_SEL_BITS_START,
        q: crate::composite_layout::SX_Q_START,
    };

    /// Composite-layout entry point: the chip-internal XOR
    /// transport **plus** the cross-chip binding that ties each
    /// active row's `SX_IN` to the matmul chip's
    /// accumulator-*after*-step. The matmul chip forces
    /// `nxt.CUMSUM_TILE == compute_row(cur)` on every transition,
    /// so binding `SX_IN[k] == nxt.CUMSUM_TILE[k]` makes `SX_IN`
    /// exactly the genuine swept accumulator — closing
    /// `committed A/B → CUMSUM → SX_IN → XR → FOLD_XSTEP`. Vacuous
    /// on every non-stripe-xor row (`SX_IS_ACTIVE = 0`), so all
    /// existing traces (all-zero SX columns) are unaffected.
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);

        // SX_IN ← matmul accumulator-after-step (cross-row, gated).
        let off = &Self::COMPOSITE_OFFSETS;
        let (is_active, in_cells): (AB::Expr, [AB::Var; IN_LEN]) = {
            let main = builder.main();
            let cur = main.current_slice();
            (
                cur[off.is_active].into(),
                core::array::from_fn(|k| cur[off.in_cells + k]),
            )
        };
        let nxt_cumsum: [AB::Var; IN_LEN] = {
            let main = builder.main();
            let nxt = main.next_slice();
            core::array::from_fn(|k| nxt[crate::composite_layout::CUMSUM_TILE_START + k])
        };
        let mut tb = builder.when_transition();
        for k in 0..IN_LEN {
            tb.assert_zero(is_active.clone() * (in_cells[k].into() - nxt_cumsum[k].into()));
        }
    }

    /// Emit the stripe-XOR constraints at the given offsets.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &StripeXorOffsets) {
        let two = <AB::F as PrimeCharacteristicRing>::TWO;

        // ---- IS_ACTIVE boolean — UNGATED ----
        // The activity selector gates the data-validation
        // constraints below and implicitly drives the ungated
        // cross-row passthrough; it must be unconditionally boolean.
        {
            let main = builder.main();
            let cur = main.current_slice();
            builder.assert_bool(cur[off.is_active]);
        }

        // ---- LANE_SEL structural constraints — UNGATED ----
        // LANE_SEL is boolean + one-hot (Σ == IS_ACTIVE) on every
        // row. The cross-row register passthrough (also ungated,
        // below) reads LANE_SEL to pick the updating lane; gating
        // LANE_SEL would let a prover set LANE_SEL = 1 on an
        // inactive row and corrupt the XR passthrough — a forgery
        // of the §6(b) binding. Ungated, IS_ACTIVE = 0 on an
        // inactive row forces Σ LANE_SEL = 0 ⇒ all LANE_SEL = 0,
        // exactly the zero-default the passthrough relies on.
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut sel_sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::LANE_SEL_LEN {
                let sel = cur[off.lane_sel + s];
                builder.assert_bool(sel);
                sel_sum = sel_sum + sel.into();
            }
            builder.assert_eq(sel_sum, cur[off.is_active].into());
        }

        // ---- Q range check — UNGATED (degree-3 cubic) ----
        // Q[i] ∈ {0,1,2} via the cubic Q·(Q−1)·(Q−2) = 0. This is
        // already degree 3 — the composite's pinned constraint-
        // degree budget (`circuit.rs` `CircuitConfig`) — so it
        // CANNOT be wrapped in a degree-1 IS_ACTIVE gate without
        // overflowing to degree 4 (Path A / §7.1 finding D3). It
        // stays ungated; the Q columns are therefore NOT overlay-
        // eligible and must stay dedicated + zero on inactive rows
        // (which keeps this ungated cubic trivially satisfied).
        {
            let main = builder.main();
            let cur = main.current_slice();
            for i in 0..32 {
                let q: AB::Expr = cur[off.q + i].into();
                let one = <AB::Expr as PrimeCharacteristicRing>::ONE;
                builder.assert_zero(q.clone() * (q.clone() - one) * (q.clone() - two.clone()));
            }
        }

        // ---- Per-row data-validation constraints — GATED by IS_ACTIVE ----
        // The column groups validated here — IN, IN_BITS,
        // XR_SEL_BITS, NEW_SEL, NEW_SEL_BITS — are checked intra-row
        // only and read by no ungated cross-row constraint (the
        // SX_IN transport in `eval_composite` is itself IS_ACTIVE-
        // gated). Gating is therefore sound: on an inactive row this
        // data is both unconstrained and unread. It also makes the
        // bit-decomposition groups overlay-eligible (Path A / §7.1
        // stage O2). Each constraint here is degree ≤ 2 ungated ⇒
        // ≤ 3 gated — within the composite's degree-3 budget.
        {
            let is_active: AB::Expr = {
                let main = builder.main();
                main.current_slice()[off.is_active].into()
            };
            let mut b = builder.when(is_active);
            let main = b.main();
            let cur = main.current_slice();

            // Each IN cell is the **signed i32** matmul accumulator
            // value (the matmul chip stores `CUMSUM` via
            // `QuotientMap<i64>`, so the §6(b) `SX_IN ==
            // nxt.CUMSUM_TILE` binding must use the same signed
            // field encoding). IN_BITS is the 32-bit two's-complement
            // pattern; the signed value is
            //   Σ_{i<32} bit_i·2^i  −  bit_31·2^32
            // (= unsigned u32 reinterpretation minus 2^32 when the
            // sign bit is set). The XOR below uses the raw bit
            // pattern, so it is sign-agnostic and unaffected.
            let two_pow_32 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << 32);
            for c in 0..IN_LEN {
                let mut recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
                let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
                for i in 0..32 {
                    let bit = cur[off.in_bits + c * 32 + i];
                    b.assert_bool(bit);
                    recon = recon + bit.into() * pow.clone();
                    pow = pow * two.clone();
                }
                let sign: AB::Expr = cur[off.in_bits + c * 32 + 31].into() * two_pow_32.clone();
                b.assert_eq(cur[off.in_cells + c].into(), recon - sign);
            }

            // XR_SEL_BITS must be the bit-decomposition of the
            // selected lane's XR value:
            //   Σ XR_SEL_BITS[i]·2^i == Σ_s LANE_SEL[s]·XR[s]
            let mut sel_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut powx: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                let bit = cur[off.xr_sel_bits + i];
                b.assert_bool(bit);
                sel_recon = sel_recon + bit.into() * powx.clone();
                powx = powx * two.clone();
            }
            let mut sel_val: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::XR_LEN {
                sel_val = sel_val + cur[off.lane_sel + s].into() * cur[off.xr + s].into();
            }
            b.assert_eq(sel_recon, sel_val);

            // NEW_SEL == Σ NEW_SEL_BITS[i]·2^i (bits boolean).
            let mut new_recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pown: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..32 {
                let bit = cur[off.new_sel_bits + i];
                b.assert_bool(bit);
                new_recon = new_recon + bit.into() * pown.clone();
                pown = pown * two.clone();
            }
            b.assert_eq(cur[off.new_sel].into(), new_recon);

            // Per output bit i: parity of the 5 contributing bits
            // (selected XR lane + the IN_LEN input cells) ==
            // NEW_SEL_BITS[i] + 2·Q[i]. Q is range-bounded by the
            // ungated cubic above; this parity equation then pins
            // NEW_SEL_BITS to the true XOR. Degree 1 ⇒ 2 gated.
            for i in 0..32 {
                let mut col_sum: AB::Expr = cur[off.xr_sel_bits + i].into();
                for c in 0..IN_LEN {
                    col_sum = col_sum + cur[off.in_bits + c * 32 + i].into();
                }
                let nbit = cur[off.new_sel_bits + i];
                let q: AB::Expr = cur[off.q + i].into();
                b.assert_eq(col_sum, nbit.into() + q * two.clone());
            }
        }

        // ---- Boundary: first row register is zero ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut fr = builder.when_first_row();
            for s in 0..cols::XR_LEN {
                fr.assert_zero(cur[off.xr + s]);
            }
        }

        // ---- Cross-row register update ----
        //
        //   XR_next[lane] = NEW_SEL          (= XR[lane] ⊕ ⊕IN)
        //   XR_next[s]    = XR[s]            (s ≠ lane; incl. all
        //                                     lanes on padding rows)
        {
            let (sel, cur_xr, new_sel): (
                [AB::Var; cols::LANE_SEL_LEN],
                [AB::Var; cols::XR_LEN],
                AB::Expr,
            ) = {
                let main = builder.main();
                let cur = main.current_slice();
                (
                    core::array::from_fn(|s| cur[off.lane_sel + s]),
                    core::array::from_fn(|s| cur[off.xr + s]),
                    cur[off.new_sel].into(),
                )
            };
            let nxt_xr: [AB::Var; cols::XR_LEN] = {
                let main = builder.main();
                let nxt = main.next_slice();
                core::array::from_fn(|s| nxt[off.xr + s])
            };

            let mut tb = builder.when_transition();

            // Selected lane: Σ_s LANE_SEL[s]·XR_next[s] == NEW_SEL
            // (on an active row exactly one LANE_SEL is 1; on a
            // padding row the sum is 0 and NEW_SEL recomposes 0).
            let mut res_sel: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::XR_LEN {
                res_sel = res_sel + sel[s].into() * nxt_xr[s].into();
            }
            tb.assert_eq(res_sel, new_sel);

            // Non-selected lanes pass through unchanged. On a
            // padding row every LANE_SEL is 0 ⇒ all lanes pass
            // through ⇒ the final register propagates to the last
            // row (where the §6(b) binding reads it).
            for s in 0..cols::XR_LEN {
                let one_minus_sel: AB::Expr =
                    <AB::Expr as PrimeCharacteristicRing>::ONE - sel[s].into();
                let diff: AB::Expr = nxt_xr[s].into() - cur_xr[s].into();
                tb.assert_zero(one_minus_sel * diff);
            }
        }
    }
}

impl<F> BaseAir<F> for StripeXorChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for StripeXorChip {
    fn eval(&self, builder: &mut AB) {
        StripeXorChip::eval_at(builder, &StripeXorChip::LOCAL_OFFSETS);
    }
}

/// Reference fold (XOR of each row's 4 `IN` cells into its lane),
/// matching the sub-block-major sweep reduction. `events` is the
/// visitation order `(lane, [i32; IN_LEN])`.
pub fn ref_stripe_xor(events: &[(usize, [i32; IN_LEN])]) -> [u32; STATE_LEN] {
    let mut xr = [0u32; STATE_LEN];
    for &(lane, in4) in events {
        let mut x = 0u32;
        for &v in &in4 {
            x ^= v as u32;
        }
        xr[lane] ^= x;
    }
    xr
}

/// Build a standalone trace from a visitation sequence
/// `(lane, [i32; IN_LEN])`. One active row per event; padded to
/// the next power of two (≥ 4). The final register propagates
/// through the padding rows so the last row carries it.
pub fn build_trace(events: &[(usize, [i32; IN_LEN])]) -> RowMajorMatrix<Val> {
    use p3_field::integers::QuotientMap;

    assert!(!events.is_empty(), "events must be non-empty");
    let n = (events.len() + 1).next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let set_bits = |row: &mut [Val], at: usize, v: u32| {
        for i in 0..32 {
            row[at + i] = <Val as QuotientMap<u64>>::from_int(((v >> i) & 1) as u64);
        }
    };

    let mut xr = [0u32; STATE_LEN];
    for (idx, &(lane, in4)) in events.iter().enumerate() {
        assert!(lane < STATE_LEN, "lane out of range");
        let row = &mut flat[idx * cols::ROW_W..(idx + 1) * cols::ROW_W];

        row[cols::IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(1);
        row[cols::LANE_SEL + lane] = <Val as QuotientMap<u64>>::from_int(1);

        let mut xin = 0u32;
        for c in 0..IN_LEN {
            let u = in4[c] as u32;
            // Signed i32 cell (matches the matmul CUMSUM encoding);
            // IN_BITS is the two's-complement pattern.
            row[cols::IN + c] = <Val as QuotientMap<i64>>::from_int(in4[c] as i64);
            set_bits(row, cols::IN_BITS + c * 32, u);
            xin ^= u;
        }
        // Register state entering this row.
        for s in 0..STATE_LEN {
            row[cols::XR + s] = <Val as QuotientMap<u64>>::from_int(xr[s] as u64);
        }
        let sel_val = xr[lane];
        set_bits(row, cols::XR_SEL_BITS, sel_val);
        let new_sel = sel_val ^ xin;
        row[cols::NEW_SEL] = <Val as QuotientMap<u64>>::from_int(new_sel as u64);
        set_bits(row, cols::NEW_SEL_BITS, new_sel);

        // Q[i] = (XR_sel_bit[i] + Σ_c IN_bit[c][i] − NEW_bit[i]) / 2
        // ∈ {0,1,2}, written to a single value column per position.
        for i in 0..32 {
            let mut col_sum: u32 = (sel_val >> i) & 1;
            for c in 0..IN_LEN {
                col_sum += (in4[c] as u32 >> i) & 1;
            }
            let q = (col_sum - ((new_sel >> i) & 1)) / 2;
            row[cols::Q + i] = <Val as QuotientMap<u64>>::from_int(q as u64);
        }

        xr[lane] = new_sel;
    }

    // Padding rows: register passthrough (all selectors 0).
    for idx in events.len()..n {
        let row = &mut flat[idx * cols::ROW_W..(idx + 1) * cols::ROW_W];
        for s in 0..STATE_LEN {
            row[cols::XR + s] = <Val as QuotientMap<u64>>::from_int(xr[s] as u64);
        }
    }

    RowMajorMatrix::new(flat, cols::ROW_W)
}

/// Read the final `STATE_LEN`-lane register from a built trace's
/// last row (the value the §6(b) composite binding reads).
pub fn final_register(trace: &RowMajorMatrix<Val>) -> [u32; STATE_LEN] {
    use p3_field::PrimeField64;
    let h = trace.values.len() / cols::ROW_W;
    let base = (h - 1) * cols::ROW_W;
    core::array::from_fn(|s| trace.values[base + cols::XR + s].as_canonical_u64() as u32)
}

#[cfg(test)]
mod tests {
    //! Exhaustive standalone tests. Self-contained: `ai-pow-zk`
    //! must NOT depend on `ai-pow`. The cross-crate parity vs
    //! `compute_tile_trace`'s real per-stripe `x_steps` (over the
    //! sub-block-major sweep) is asserted from the `ai-pow` side
    //! under the `zk` feature (HIGH-2.2 §6(b)).

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

    /// Sub-block-major visitation: for each of `n_sb` sub-blocks,
    /// fold all `n_stripes` stripes (lane = stripe index). The
    /// final register must equal the reference and verify.
    #[test]
    fn final_register_matches_reference_and_verifies() {
        let c = cfg();
        // Incl. the §6(b)-G2 cases: num_stripes > 16 (rect-shaped
        // `k/r = 20`) and the full STRIPE_MAX = 64 (PROD-per-segment).
        for (n_sb, n_stripes) in
            [(1usize, 4usize), (16, 16), (4, 8), (16, 1), (4, 20), (8, 64), (1, 64)]
        {
            let raw = lcg(0xABCD ^ (n_sb as u64), n_sb * n_stripes * IN_LEN);
            let mut events = Vec::new();
            for sb in 0..n_sb {
                for step in 0..n_stripes {
                    let base = (sb * n_stripes + step) * IN_LEN;
                    let in4: [i32; IN_LEN] = core::array::from_fn(|k| raw[base + k]);
                    events.push((step, in4));
                }
            }
            let trace = build_trace(&events);
            assert_eq!(
                final_register(&trace),
                ref_stripe_xor(&events),
                "n_sb={n_sb} n_stripes={n_stripes}: register vs reference"
            );
            let proof = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, trace, &[]);
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &proof, &[])
                .unwrap_or_else(|e| panic!("honest stripe-xor trace must verify: {e:?}"));
        }
    }

    /// Independent re-derivation: XOR-by-lane via explicit per-bit
    /// parity (guards a shared bug between build_trace and chip).
    #[test]
    fn matches_manual_bit_parity() {
        let n_sb = 16;
        let n_stripes = 16;
        let raw = lcg(7, n_sb * n_stripes * IN_LEN);
        let mut events = Vec::new();
        let mut want = [0u32; STATE_LEN];
        for sb in 0..n_sb {
            for step in 0..n_stripes {
                let base = (sb * n_stripes + step) * IN_LEN;
                let in4: [i32; IN_LEN] = core::array::from_fn(|k| raw[base + k]);
                for &v in &in4 {
                    want[step] ^= v as u32;
                }
                events.push((step, in4));
            }
        }
        assert_eq!(final_register(&build_trace(&events)), want);
    }

    fn honest() -> RowMajorMatrix<Val> {
        let raw = lcg(99, 16 * 16 * IN_LEN);
        let mut events = Vec::new();
        for sb in 0..16 {
            for step in 0..16 {
                let base = (sb * 16 + step) * IN_LEN;
                events.push((step, core::array::from_fn(|k| raw[base + k])));
            }
        }
        build_trace(&events)
    }

    #[test]
    fn rejects_tampered_register() {
        let c = cfg();
        let mut t = honest();
        let h = t.values.len() / cols::ROW_W;
        // Corrupt lane 0 one row before the end.
        t.values[(h - 2) * cols::ROW_W + cols::XR] =
            <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEF);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "tampered XR must reject"
        );
    }

    #[test]
    fn rejects_nonzero_first_row_register() {
        let c = cfg();
        let mut t = honest();
        t.values[cols::XR + 3] = <Val as QuotientMap<u64>>::from_int(1);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "non-zero initial register must reject"
        );
    }

    #[test]
    fn rejects_tampered_new_sel_without_bits() {
        let c = cfg();
        let mut t = honest();
        t.values[2 * cols::ROW_W + cols::NEW_SEL] = <Val as QuotientMap<u64>>::from_int(0x7);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "NEW_SEL inconsistent with its bits must reject"
        );
    }

    #[test]
    fn rejects_double_lane_selection() {
        let c = cfg();
        let mut t = honest();
        // A second LANE_SEL bit on row 1 ⇒ Σ == IS_ACTIVE violated.
        t.values[1 * cols::ROW_W + cols::LANE_SEL + 9] = <Val as QuotientMap<u64>>::from_int(1);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "two active LANE_SEL bits must reject"
        );
    }

    #[test]
    fn rejects_out_of_range_q() {
        let c = cfg();
        let mut t = honest();
        // Q[0] = 3 is outside the valid range {0,1,2} — the cubic
        // range constraint Q·(Q−1)·(Q−2)=0 must reject it.
        t.values[cols::Q] = <Val as QuotientMap<u64>>::from_int(3);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "out-of-range Q (= 3) must reject"
        );
    }

    #[test]
    fn rejects_lane_passthrough_violation() {
        let c = cfg();
        let mut t = honest();
        // Mutate a non-selected lane across a transition: change
        // the final register on the last row only ⇒ the prior
        // transition's passthrough (nxt==cur for s≠lane) breaks.
        let h = t.values.len() / cols::ROW_W;
        t.values[(h - 1) * cols::ROW_W + cols::XR + 5] =
            <Val as QuotientMap<u64>>::from_int(0x1234);
        let p = prove::<AiPowStarkConfig, _>(&c, &StripeXorChip, t, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &StripeXorChip, &p, &[]).is_err(),
            "non-selected-lane mutation must break passthrough"
        );
    }
}
