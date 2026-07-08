//! Fold chip — Pearl §4.5 rotate-left-13 XOR fold (HIGH-2.2 §4.B,
//! Option B2).
//!
//! ## Property enforced
//!
//! The fold is a **pure function of a per-stripe `X_STEP`
//! sequence** (see `ai-pow::matmul::TileState::from_x_steps` and
//! `2026-05-15_HIGH2_2_DESIGN.md` §4.0). Row `t` (0-indexed) is the fold of
//! stripe `t` into slot `t mod 16`:
//!
//! ```text
//!   M_next[slot] = rotl13(M_cur[slot]) XOR X_STEP[t]      (slot = t mod 16)
//!   M_next[s]    = M_cur[s]                               (s != slot)
//!   first row:   M == 0
//! ```
//!
//! `FOLD_STATE` carries the 16-word state *entering* the row
//! (`M_cur`). After the last fold row the final state propagates
//! unchanged (every `SLOT_SEL` bit is 0 on padding rows, so all 16
//! slots pass through), so the last trace row holds the final `M`
//! — exactly what HIGH-2.2 §4.D's keystone reads.
//!
//! This is **Option B2**: a direct per-stripe fold. It is *not*
//! Pearl's rotate-on-load bit-serial RAM machine (no
//! `CUMSUM_BUFFER`, no `SHIFT3` back-shift compensation) — those
//! exist in Pearl only to service its concurrent scheduling, and
//! our SNARKs are deliberately not trace-byte-equivalent to Pearl
//! (`2026-05-15_HIGH2_2_DESIGN.md` §9.5). The XOR+rotate core reuses the
//! audited `blake3::round_ops::xor_32_shift_if` gadget.
//!
//! The accumulator → `X_STEP` reduction and its binding to the
//! committed matrices is HIGH-2.2 §4.C's obligation; this chip
//! only proves the fold given `X_STEP`.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// Pearl §4.5 left-rotation amount (`LROT_PER_TILE`).
pub const LROT: usize = 13;
/// `JACKPOT_SIZE` — number of `u32` words in the fold state.
pub const STATE_LEN: usize = 16;

/// Chip-local column offsets. Independent of `composite_layout`;
/// HIGH-2.2 §4.A/§4.C stitch these into the global trace.
pub mod cols {
    use super::STATE_LEN;

    /// 1 = active fold row, 0 = passthrough/padding.
    pub const IS_FOLD: usize = 0;
    /// One-hot slot selector (`= IS_FOLD` when summed).
    pub const SLOT_SEL: usize = IS_FOLD + 1;
    pub const SLOT_SEL_LEN: usize = STATE_LEN; // 16
    /// `X_STEP` value (u32 reinterpretation of the i32 fold input).
    pub const XSTEP: usize = SLOT_SEL + SLOT_SEL_LEN; // 17
    /// 32 little-endian bits of `XSTEP`.
    pub const XSTEP_BITS: usize = XSTEP + 1; // 18
    pub const XSTEP_BITS_LEN: usize = 32;
    /// 16-word fold state *entering* this row (`M_cur`), u32 each.
    pub const FOLD_STATE: usize = XSTEP_BITS + XSTEP_BITS_LEN; // 50
    pub const FOLD_STATE_LEN: usize = STATE_LEN; // 16
    /// 32 little-endian bits of the *current* addressed slot
    /// (`M_cur[slot]`) — the operand `xor_32_shift_if` rotates.
    pub const MCUR_BITS: usize = FOLD_STATE + FOLD_STATE_LEN; // 66
    pub const MCUR_BITS_LEN: usize = 32;
    /// Materialised `XSTEP ⊕ rotl13(M_cur[slot])` (u32) — keeps
    /// the fold transition degree ≤ 2 (see `composite_layout::
    /// FOLD_XOR_OUT` / HIGH2_2_DESIGN §4.A).
    pub const XOR_OUT: usize = MCUR_BITS + MCUR_BITS_LEN; // 98
    pub const ROW_W: usize = XOR_OUT + 1; // 99
}

/// Zero-sized chip type.
#[derive(Debug, Default, Clone, Copy)]
pub struct FoldChip;

/// Column-offset bundle so the eval body can run both standalone
/// (chip-local layout) and, later, at composite-layout offsets.
#[derive(Copy, Clone, Debug)]
pub struct FoldOffsets {
    pub is_fold: usize,
    pub slot_sel: usize,
    pub xstep: usize,
    pub xstep_bits: usize,
    pub fold_state: usize,
    pub mcur_bits: usize,
    pub xor_out: usize,
}

impl FoldChip {
    pub const LOCAL_OFFSETS: FoldOffsets = FoldOffsets {
        is_fold: cols::IS_FOLD,
        slot_sel: cols::SLOT_SEL,
        xstep: cols::XSTEP,
        xstep_bits: cols::XSTEP_BITS,
        fold_state: cols::FOLD_STATE,
        mcur_bits: cols::MCUR_BITS,
        xor_out: cols::XOR_OUT,
    };

    /// Composite-trace offsets (HIGH-2.2 §4.A/§4.D wiring) — the
    /// FoldChip's columns at their `composite_layout` positions.
    pub const COMPOSITE_OFFSETS: FoldOffsets = FoldOffsets {
        is_fold: crate::composite_layout::FOLD_IS_FOLD,
        slot_sel: crate::composite_layout::FOLD_SLOT_SEL_START,
        xstep: crate::composite_layout::FOLD_XSTEP,
        xstep_bits: crate::composite_layout::FOLD_XSTEP_BITS_START,
        fold_state: crate::composite_layout::FOLD_STATE_START,
        mcur_bits: crate::composite_layout::FOLD_MCUR_BITS_START,
        xor_out: crate::composite_layout::FOLD_XOR_OUT,
    };

    /// Composite-layout entry point: `eval_at(builder,
    /// &COMPOSITE_OFFSETS)` **plus** the §6(b)-G2 per-fold-row
    /// stripe-selector well-formedness (composite-only — the
    /// standalone FoldChip has no stripe selector). Each
    /// `FOLD_STRIPE_SEL[s]` is boolean and `Σ == FOLD_IS_FOLD`
    /// (one-hot on fold rows, all-zero off them), so the 6-bit
    /// stripe index `ControlChip` pins into `CONTROL_PREP` (CRIT-1)
    /// faithfully addresses exactly one `SX_XR` lane in the §6(b)
    /// keystone. Called from `CompositeFullAir::eval`.
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);

        let main = builder.main();
        let cur = main.current_slice();
        let is_fold = cur[crate::composite_layout::FOLD_IS_FOLD];
        let mut sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        for s in 0..crate::composite_layout::FOLD_STRIPE_SEL_LEN {
            let sel = cur[crate::composite_layout::FOLD_STRIPE_SEL_START + s];
            builder.assert_bool(sel);
            sum = sum + sel.into();
        }
        builder.assert_eq(sum, is_fold.into());
    }

    /// Emit the fold constraints at the given column offsets.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &FoldOffsets) {
        let two = <AB::F as PrimeCharacteristicRing>::TWO;

        // ---- Per-row structural constraints ----
        {
            let main = builder.main();
            let cur = main.current_slice();

            let is_fold = cur[off.is_fold];
            builder.assert_bool(is_fold);

            // SLOT_SEL: each boolean, and Σ == IS_FOLD (one-hot on
            // fold rows, all-zero on padding).
            let mut sel_sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::SLOT_SEL_LEN {
                let sel = cur[off.slot_sel + s];
                builder.assert_bool(sel);
                sel_sum = sel_sum + sel.into();
            }
            builder.assert_eq(sel_sum, cur[off.is_fold].into());

            // XSTEP == Σ XSTEP_BITS[i]·2^i (bits boolean-checked).
            let mut x_acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..cols::XSTEP_BITS_LEN {
                let bit = cur[off.xstep_bits + i];
                builder.assert_bool(bit);
                x_acc = x_acc + bit.into() * pow.clone();
                pow = pow * two.clone();
            }
            builder.assert_eq(cur[off.xstep].into(), x_acc);

            // MCUR_BITS must be the bit-decomposition of the
            // *currently addressed* slot value:
            //   Σ MCUR_BITS[i]·2^i == Σ_s SLOT_SEL[s]·FOLD_STATE[s]
            // (booleanity of MCUR_BITS is also enforced by
            // `xor_32_shift_if` below, but assert here too so the
            // selection constraint is self-contained).
            let mut m_acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pow_m: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for i in 0..cols::MCUR_BITS_LEN {
                let bit = cur[off.mcur_bits + i];
                builder.assert_bool(bit);
                m_acc = m_acc + bit.into() * pow_m.clone();
                pow_m = pow_m * two.clone();
            }
            let mut sel_val: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::SLOT_SEL_LEN {
                sel_val = sel_val + cur[off.slot_sel + s].into() * cur[off.fold_state + s].into();
            }
            builder.assert_eq(m_acc, sel_val);

            // Materialise the fold's XOR result into FOLD_XOR_OUT
            // (degree 2 — the `2·a·b` term), so the cross-row
            // selected-slot binding below stays degree 2 instead
            // of degree 3 (§4.A root cause: batch-stark rejects a
            // non-zero degree-3 constraint that uni-stark
            // accepts). `xor_out == Σ_p (xstep_bits[p] ⊕
            // mcur_bits[(p+19) mod 32])·2^p`, i.e. `XSTEP ⊕
            // (M_cur[slot] <<< 13)`. Unconditional: on non-fold
            // rows xstep_bits = mcur_bits = 0 ⇒ xor_out = 0.
            let mut xacc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut powx2: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for p in 0..32 {
                let a = cur[off.xstep_bits + p];
                let b = cur[off.mcur_bits + (p + 32 - LROT) % 32];
                // a XOR b = a + b - 2ab (a, b boolean-checked above).
                let two_ab: AB::Expr =
                    a.into() * b.into() * <AB::F as PrimeCharacteristicRing>::TWO;
                let xor_bit: AB::Expr = a.into() + b.into() - two_ab;
                xacc = xacc + xor_bit * powx2.clone();
                powx2 = powx2 * two.clone();
            }
            builder.assert_eq(cur[off.xor_out].into(), xacc);
        }

        // ---- Boundary: first row state is zero ----
        {
            let main = builder.main();
            let cur = main.current_slice();
            let mut fr = builder.when_first_row();
            for s in 0..cols::FOLD_STATE_LEN {
                fr.assert_zero(cur[off.fold_state + s]);
            }
        }

        // ---- Cross-row fold ----
        //
        // Selected slot:  M_next[slot] = X_STEP XOR (M_cur[slot] <<< 13)
        // Other slots:    M_next[s]    = M_cur[s]
        {
            // Snapshot owned arrays before opening the sub-builder.
            let (sel, cur_state, is_fold, xor_out): (
                [AB::Var; cols::SLOT_SEL_LEN],
                [AB::Var; cols::FOLD_STATE_LEN],
                AB::Expr,
                AB::Expr,
            ) = {
                let main = builder.main();
                let cur = main.current_slice();
                (
                    core::array::from_fn(|s| cur[off.slot_sel + s]),
                    core::array::from_fn(|s| cur[off.fold_state + s]),
                    cur[off.is_fold].into(),
                    cur[off.xor_out].into(),
                )
            };
            let nxt_state: [AB::Var; cols::FOLD_STATE_LEN] = {
                let main = builder.main();
                let nxt = main.next_slice();
                core::array::from_fn(|s| nxt[off.fold_state + s])
            };

            let mut tb = builder.when_transition();

            // Selected slot binding (degree 2): on a fold row
            // exactly one SLOT_SEL is 1, so Σ_s SLOT_SEL[s]·
            // M_next[s] = M_next[slot]; force it to
            // is_fold·FOLD_XOR_OUT (= X_STEP ⊕ rotl13(M_cur[slot])
            // on fold rows, 0 otherwise). `sel·nxt` and
            // `is_fold·xor_out` are both degree 2 — the deg-3
            // `is_fold·(res_sel − acc)` form is gone (§4.A fix).
            let mut res_sel: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..cols::FOLD_STATE_LEN {
                res_sel = res_sel + sel[s].into() * nxt_state[s].into();
            }
            tb.assert_eq(res_sel, is_fold * xor_out);

            // Non-selected slots pass through unchanged. When
            // IS_FOLD = 0 every SLOT_SEL is 0, so all 16 slots pass
            // through ⇒ the final state propagates to the last row.
            for s in 0..cols::FOLD_STATE_LEN {
                let one_minus_sel: AB::Expr =
                    <AB::Expr as PrimeCharacteristicRing>::ONE - sel[s].into();
                let diff: AB::Expr = nxt_state[s].into() - cur_state[s].into();
                tb.assert_zero(one_minus_sel * diff);
            }
        }
    }
}

impl<F> BaseAir<F> for FoldChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for FoldChip {
    fn eval(&self, builder: &mut AB) {
        FoldChip::eval_at(builder, &FoldChip::LOCAL_OFFSETS);
    }
}

/// Reference fold (matches `ai-pow::matmul::TileState::fold`),
/// duplicated here so the chip crate has no dependency on
/// `ai-pow`. Tested for byte-equivalence in the test module.
#[inline]
fn rotl13_xor(m_slot: u32, x: u32) -> u32 {
    m_slot.rotate_left(LROT as u32) ^ x
}

/// Build a standalone fold trace from a per-stripe `x` sequence
/// (the i32 values from `ai-pow::matmul::compute_tile_trace`).
///
/// Row `t` folds `x_steps[t]` into slot `t mod 16`; rows after the
/// last fold carry the final state unchanged. Padded to the next
/// power of two (≥ 4).
pub fn build_trace(x_steps: &[i32]) -> RowMajorMatrix<Val> {
    use p3_field::integers::QuotientMap;

    assert!(!x_steps.is_empty(), "x_steps must be non-empty");
    let len = x_steps.len();
    let n = (len + 1).next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let set_u32 = |row: &mut [Val], at: usize, v: u32| {
        row[at] = <Val as QuotientMap<u64>>::from_int(v as u64);
    };
    let set_bits = |row: &mut [Val], at: usize, v: u32| {
        for i in 0..32 {
            row[at + i] = <Val as QuotientMap<u64>>::from_int(((v >> i) & 1) as u64);
        }
    };

    let mut m = [0u32; STATE_LEN];

    for t in 0..len {
        let slot = t % STATE_LEN;
        let x = x_steps[t] as u32;
        let row = &mut flat[t * cols::ROW_W..(t + 1) * cols::ROW_W];

        row[cols::IS_FOLD] = <Val as QuotientMap<u64>>::from_int(1);
        row[cols::SLOT_SEL + slot] = <Val as QuotientMap<u64>>::from_int(1);
        set_u32(row, cols::XSTEP, x);
        set_bits(row, cols::XSTEP_BITS, x);
        for s in 0..STATE_LEN {
            set_u32(row, cols::FOLD_STATE + s, m[s]);
        }
        set_bits(row, cols::MCUR_BITS, m[slot]);
        let folded = rotl13_xor(m[slot], x); // = XSTEP ⊕ rotl13(M_cur[slot])
        set_u32(row, cols::XOR_OUT, folded);
        m[slot] = folded;
    }

    // Rows [len, n): final state, all selectors 0 ⇒ full
    // passthrough, MCUR_BITS = 0 (selection sum is 0).
    for t in len..n {
        let row = &mut flat[t * cols::ROW_W..(t + 1) * cols::ROW_W];
        for s in 0..STATE_LEN {
            set_u32(row, cols::FOLD_STATE + s, m[s]);
        }
    }

    RowMajorMatrix::new(flat, cols::ROW_W)
}

/// Read the 16-word fold state from a built trace's last row
/// (the value HIGH-2.2 §4.D's keystone binds).
pub fn final_state(trace: &RowMajorMatrix<Val>) -> [u32; STATE_LEN] {
    use p3_field::PrimeField64;
    let h = trace.values.len() / cols::ROW_W;
    let base = (h - 1) * cols::ROW_W;
    core::array::from_fn(|s| trace.values[base + cols::FOLD_STATE + s].as_canonical_u64() as u32)
}

#[cfg(test)]
mod tests {
    //! Exhaustive FoldChip tests. Self-contained: `ai-pow-zk` must
    //! NOT depend on `ai-pow` (dependency cycle — see Cargo.toml).
    //! The reference is a local hand-rolled rotl13-XOR fold; the
    //! ai-pow `compute_tile_trace`/`from_x_steps` byte-equivalence
    //! parity is tested from the `ai-pow` side under the `zk`
    //! feature (HIGH-2.2 §4.A/§7), where `ai-pow` → `ai-pow-zk` is
    //! the legal direction.

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

    /// Local reference fold — the exact Pearl §4.5 recurrence
    /// `ai-pow::matmul::TileState::from_x_steps` implements.
    fn ref_fold(x_steps: &[i32]) -> [u32; STATE_LEN] {
        let mut m = [0u32; STATE_LEN];
        for (t, &x) in x_steps.iter().enumerate() {
            let slot = t % STATE_LEN;
            m[slot] = m[slot].rotate_left(LROT as u32) ^ (x as u32);
        }
        m
    }

    /// The chip's proven final state matches the reference for a
    /// spread of sequence lengths: 1, exactly STATE_LEN (no wrap),
    /// the real TEST_SMALL `num_stripes`=16, and > 16 (slot wrap).
    #[test]
    fn final_state_matches_reference_over_lengths() {
        for len in [1usize, 8, 16, 17, 40, 64] {
            let xs: Vec<i32> = (0..len as i32)
                .map(|i| i.wrapping_mul(0x9E37_79B1u32 as i32) ^ (i << 5) ^ 0x5A5A)
                .collect();
            assert_eq!(
                final_state(&build_trace(&xs)),
                ref_fold(&xs),
                "len {len}: chip final state vs reference fold"
            );
        }
    }

    /// The honest trace proves and verifies across the same
    /// length spread (1, 16, > 16 wrapping).
    #[test]
    fn honest_trace_proves_and_verifies() {
        let cfg = cfg();
        for xs in [
            vec![0x1234_5678i32],
            (0..16i32)
                .map(|i| i.wrapping_mul(2_000_003))
                .collect::<Vec<_>>(),
            (0..40i32)
                .map(|i| i.wrapping_mul(0x9E37_79B1u32 as i32) ^ (i << 5))
                .collect::<Vec<_>>(),
        ] {
            let trace = build_trace(&xs);
            let proof = prove::<AiPowStarkConfig, _>(&cfg, &FoldChip, trace, &[]);
            verify::<AiPowStarkConfig, _>(&cfg, &FoldChip, &proof, &[]).unwrap_or_else(|e| {
                panic!("honest fold trace (len {}) must verify: {e:?}", xs.len())
            });
        }
    }

    /// Independent re-derivation against an explicit per-slot
    /// rotate-then-xor (guards against a shared bug between
    /// `build_trace` and the chip constraints).
    #[test]
    fn final_state_matches_manual_fold() {
        let xs: Vec<i32> = (0..37i32).map(|i| (i << 11) ^ i.wrapping_mul(7)).collect();
        let mut m = [0u32; STATE_LEN];
        for (t, &x) in xs.iter().enumerate() {
            let slot = t % STATE_LEN;
            m[slot] = m[slot].rotate_left(13) ^ (x as u32);
        }
        assert_eq!(final_state(&build_trace(&xs)), m);
    }

    /// Slot wrapping: stripe 16 must fold back into slot 0 *after*
    /// stripe 0's rotation, so swapping the two inputs changes the
    /// proven final state (order-dependence, Pearl §4.5).
    #[test]
    fn slot_wrap_is_order_dependent() {
        let mut xs = vec![0i32; 17];
        xs[0] = 0x1111_2222;
        xs[16] = 0x3333_4444;
        let a = final_state(&build_trace(&xs));
        xs[0] = 0x3333_4444;
        xs[16] = 0x1111_2222;
        let b = final_state(&build_trace(&xs));
        assert_ne!(a, b);
    }

    fn honest() -> RowMajorMatrix<Val> {
        let xs: Vec<i32> = (0..20i32)
            .map(|i| i.wrapping_mul(0x0151_5151) ^ 0x33)
            .collect();
        build_trace(&xs)
    }

    #[test]
    fn rejects_tampered_fold_state() {
        let c = cfg();
        let mut trace = honest();
        // Corrupt the final-state slot 0 one row before the end.
        let h = trace.values.len() / cols::ROW_W;
        trace.values[(h - 2) * cols::ROW_W + cols::FOLD_STATE] =
            <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEF);
        let proof = prove::<AiPowStarkConfig, _>(&c, &FoldChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &FoldChip, &proof, &[]).is_err(),
            "tampered FOLD_STATE must reject"
        );
    }

    #[test]
    fn rejects_tampered_xstep() {
        let c = cfg();
        let mut trace = honest();
        // Flip XSTEP on row 2 without fixing its bits ⇒ the
        // XSTEP == Σ bits·2^i reconstruction must fail.
        trace.values[2 * cols::ROW_W + cols::XSTEP] = <Val as QuotientMap<u64>>::from_int(0x7);
        let proof = prove::<AiPowStarkConfig, _>(&c, &FoldChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &FoldChip, &proof, &[]).is_err(),
            "tampered XSTEP must reject"
        );
    }

    #[test]
    fn rejects_nonzero_first_row_state() {
        let c = cfg();
        let mut trace = honest();
        trace.values[cols::FOLD_STATE] = <Val as QuotientMap<u64>>::from_int(1);
        let proof = prove::<AiPowStarkConfig, _>(&c, &FoldChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &FoldChip, &proof, &[]).is_err(),
            "non-zero initial M must reject"
        );
    }

    #[test]
    fn rejects_double_slot_selection() {
        let c = cfg();
        let mut trace = honest();
        // Set a second SLOT_SEL bit on row 1 ⇒ Σ SLOT_SEL == IS_FOLD
        // (=1) is violated.
        trace.values[1 * cols::ROW_W + cols::SLOT_SEL + 5] = <Val as QuotientMap<u64>>::from_int(1);
        let proof = prove::<AiPowStarkConfig, _>(&c, &FoldChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &FoldChip, &proof, &[]).is_err(),
            "two active SLOT_SEL bits must reject"
        );
    }

    #[test]
    fn rejects_passthrough_violation_on_padding() {
        let c = cfg();
        let mut trace = honest();
        // Mutate the final state on the very last row only ⇒ the
        // padding passthrough (nxt==cur) breaks at the prior
        // transition.
        let h = trace.values.len() / cols::ROW_W;
        trace.values[(h - 1) * cols::ROW_W + cols::FOLD_STATE + 3] =
            <Val as QuotientMap<u64>>::from_int(0x1234);
        let proof = prove::<AiPowStarkConfig, _>(&c, &FoldChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&c, &FoldChip, &proof, &[]).is_err(),
            "final-row tamper must break passthrough"
        );
    }
}
