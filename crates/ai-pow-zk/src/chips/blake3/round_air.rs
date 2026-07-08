//! BLAKE3 round-AIR composition.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/blake3_air.rs`:
//! `Blake3State`, `half_g`, `verify_round`, `finalize_blake`, and
//! `verify_init_state`. Together these encode the BLAKE3
//! compression function over Pearl's one-round-per-row trace
//! layout.
//!
//! Constraint primitives come from [`super::round_ops`] (Phase 8a).
//!
//! ## Per-row layout recap
//!
//! Each trace row holds 5 conceptual state snapshots: the row's
//! `INPUT_STATE` and three intermediate states (`STATE1`, `STATE2`,
//! `STATE3`) computed within the row's G-function application,
//! plus the NEXT row's `INPUT_STATE` (which equals this row's
//! "output state"). Pearl's 5-element `states` array threads
//! these together; we mirror the same pattern.
//!
//! ## What each function enforces
//!
//! * `half_g(a, b, c, d, m, flag, expected_*, is_activated)` —
//!   one BLAKE3 quarter-round half-step. 4 underlying constraints:
//!     `expected_a = a + b + m (mod 2^32)`
//!     `expected_d = (d XOR expected_a).rotate_right(rot_1)`
//!     `expected_c = c + expected_d (mod 2^32)`
//!     `expected_b = (b XOR expected_c).rotate_right(rot_2)`
//!   where `(rot_1, rot_2) = (16, 12)` if `flag = false`, else
//!   `(8, 7)`. Pearl's `xor_32_shift_if` direction makes these the
//!   exact BLAKE3 G-function steps.
//!
//! * `verify_round(states[0..5], msg, is_activated)` — 16 half-G
//!   calls (4 column-G half 1 + 4 column-G half 2 + 4 diagonal-G
//!   half 1 + 4 diagonal-G half 2). Pearl's per-round structure
//!   from `blake3_air.rs:76-147`.
//!
//! * `finalize_blake(states, is_activated)` — round 8's
//!   "feed-forward XOR": output[i] = state[0].row1[i] XOR
//!   state[0].row3[i] for i in 0..4 and similar for row2/row4.
//!   Repurposes `states[1].row2` / `row4` as bit decompositions of
//!   `states[0].row1` / `row3` (Pearl's "abuse" comment at
//!   `blake3_air.rs:160-176`).
//!
//! * `verify_init_state(init, is_new_blake, cv, blake3_tweak)` —
//!   initial state at round 1: row1 = cv[0..4], row2 (packed bits)
//!   = cv[4..8], row3 = BLAKE3_IV[0..4], row4 = bit-packed tweak.

use p3_air::AirBuilder;
use p3_field::PrimeCharacteristicRing;

use super::compress::BLAKE3_IV;
use super::round_ops::{add2_unchecked, add3_unchecked, xor_32_packed, xor_32_shift_if};

/// 16-word BLAKE3 state, laid out per Pearl's
/// `chip/blake3/blake3_layout.rs`:
///
/// ```text
///   row1: 4 packed values (cells 0, 4, 8, 12 of the BLAKE3 state)
///   row2: 4 × 32 boolean bits (cells 4, 5, 6, 7 — bit-decomposed)
///   row3: 4 packed values (cells 8, 9, 10, 11)
///   row4: 4 × 32 boolean bits (cells 12, 13, 14, 15)
/// ```
///
/// Slot meanings under BLAKE3's G function:
///   * `row1[i]` is the "a" operand (cumulative add target).
///   * `row2[i]` is the "b" operand (XOR + rotate target).
///   * `row3[i]` is the "c" operand (cumulative add target).
///   * `row4[i]` is the "d" operand (XOR + rotate target).
///
/// The bit-decomposition for row2 / row4 is what lets the AIR
/// express the XOR + rotate steps via `xor_32_shift_if`.
#[derive(Copy, Clone, Debug)]
pub struct Blake3State<'a, V: Copy> {
    pub row1: [V; 4],
    pub row2: [&'a [V]; 4],
    pub row3: [V; 4],
    pub row4: [&'a [V]; 4],
}

impl<'a, V: Copy> Blake3State<'a, V> {
    /// Construct a state from a contiguous slice of 264 trace
    /// cells. Layout: 4 + (4 × 32) + 4 + (4 × 32) = 264.
    pub fn from_slice(s: &'a [V]) -> Self {
        assert_eq!(
            s.len(),
            super::layout::LIMBS_PER_STATE_SNAPSHOT,
            "Blake3State::from_slice expects a 264-cell snapshot"
        );
        let mut off = 0;
        let row1: [V; 4] = core::array::from_fn(|_| {
            let v = s[off];
            off += 1;
            v
        });
        let row2: [&'a [V]; 4] = core::array::from_fn(|_| {
            let r = &s[off..off + 32];
            off += 32;
            r
        });
        let row3: [V; 4] = core::array::from_fn(|_| {
            let v = s[off];
            off += 1;
            v
        });
        let row4: [&'a [V]; 4] = core::array::from_fn(|_| {
            let r = &s[off..off + 32];
            off += 32;
            r
        });
        debug_assert_eq!(off, super::layout::LIMBS_PER_STATE_SNAPSHOT);
        Self {
            row1,
            row2,
            row3,
            row4,
        }
    }
}

/// `polyval(bits, 2)` = recompose a 32-bit decomposition into a
/// packed u32 expression. Equivalent to Pearl's
/// `eval.polyval(b, 2)`.
fn polyval_bits<AB: AirBuilder>(bits: &[AB::Var]) -> AB::Expr {
    assert_eq!(bits.len(), 32);
    let two = <AB::F as PrimeCharacteristicRing>::TWO;
    let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
    for &b in bits {
        acc = acc + b * pow.clone();
        pow = pow * two.clone();
    }
    acc
}

/// Convert a 32-bit decomposition slice into an `AB::Expr` slice
/// suitable for `xor_32_shift_if`'s `a` parameter. The XOR helper
/// takes `&[AB::Expr]` for the left operand because that operand
/// may be a sub-expression of another constraint chain (not a raw
/// trace cell).
fn bits_to_exprs<AB: AirBuilder>(bits: &[AB::Var]) -> Vec<AB::Expr> {
    bits.iter().map(|&v| v.into()).collect()
}

/// Half of a BLAKE3 quarter-round. Mirrors Pearl's `half_g`
/// (`blake3_air.rs:43-73`).
///
/// `flag = false` → rotation amounts (16, 12); `flag = true` →
/// (8, 7). The four sub-constraints together encode one BLAKE3
/// G-half:
///
/// ```text
///   expected_a = a + polyval(b, 2) + m       (mod 2^32, via add3_unchecked)
///   expected_a = d XOR expected_d.rotate_left(rot_1)
///   expected_c = c + polyval(expected_d, 2)  (mod 2^32, via add2_unchecked)
///   expected_c = b XOR expected_b.rotate_left(rot_2)
/// ```
///
/// Note Pearl's XOR convention via `xor_32_shift_if` is `res = a
/// XOR (b <<< shift)` (LEFT rotate of `b`). Combined with BLAKE3's
/// `d' = (d XOR a').rotate_right(rot)`, the constraint is
/// equivalent to BLAKE3's spec — see the discussion in
/// [`super::round_ops`].
#[allow(clippy::too_many_arguments)]
pub fn half_g<AB: AirBuilder>(
    builder: &mut AB,
    a: AB::Var,
    b: &[AB::Var],
    c: AB::Var,
    d: &[AB::Var],
    m: AB::Expr,
    flag: bool,
    expected_a: AB::Var,
    expected_b: &[AB::Var],
    expected_c: AB::Var,
    expected_d: &[AB::Var],
    is_activated: AB::Expr,
) {
    let (rot_1, rot_2) = if flag { (8usize, 7usize) } else { (16, 12) };

    // 1. expected_a = a + polyval(b, 2) + m (unchecked mod 2^32),
    //    gated by is_activated.
    let b_packed = polyval_bits::<AB>(b);
    add3_unchecked::<AB>(
        builder,
        expected_a.into(),
        a.into(),
        b_packed,
        m,
        is_activated.clone(),
    );

    // 2. expected_a = d XOR expected_d.rotate_left(rot_1).
    let d_exprs = bits_to_exprs::<AB>(d);
    xor_32_shift_if::<AB>(
        builder,
        expected_a.into(),
        &d_exprs,
        expected_d,
        is_activated.clone(),
        rot_1,
    );

    // 3. expected_c = c + polyval(expected_d, 2) (unchecked mod 2^32),
    //    gated by is_activated.
    let expected_d_packed = polyval_bits::<AB>(expected_d);
    add2_unchecked::<AB>(
        builder,
        expected_c.into(),
        c.into(),
        expected_d_packed,
        is_activated.clone(),
    );

    // 4. expected_c = b XOR expected_b.rotate_left(rot_2).
    let b_exprs = bits_to_exprs::<AB>(b);
    xor_32_shift_if::<AB>(
        builder,
        expected_c.into(),
        &b_exprs,
        expected_b,
        is_activated,
        rot_2,
    );
}

/// Verify one full BLAKE3 mixing round. Calls `half_g` 16 times
/// across 4 sub-phases (column G half 1, column G half 2, diagonal
/// G half 1, diagonal G half 2). Mirrors Pearl's `verify_round`
/// (`blake3_air.rs:75-147`).
///
/// `states[0]` is this row's INPUT_STATE; `states[1..4]` are the 3
/// intermediate snapshots; `states[4]` is the NEXT row's
/// INPUT_STATE (= this row's "output state" / round output).
///
/// All constraints are gated by `is_activated` — typically
/// `next_is_same_blake = 1 - next_is_new_blake`, which fires the
/// round only when the next row is a continuation of the current
/// BLAKE3 hash.
pub fn verify_round<AB: AirBuilder>(
    builder: &mut AB,
    states: &[Blake3State<'_, AB::Var>; 5],
    msg: &[AB::Expr],
    is_activated: AB::Expr,
) {
    assert_eq!(msg.len(), 16);

    // Column G half 1 (col-wise, msg[0,2,4,6]).
    for i in 0..4 {
        half_g::<AB>(
            builder,
            states[0].row1[i],
            states[0].row2[i],
            states[0].row3[i],
            states[0].row4[i],
            msg[2 * i].clone(),
            false,
            states[1].row1[i],
            states[1].row2[i],
            states[1].row3[i],
            states[1].row4[i],
            is_activated.clone(),
        );
    }
    // Column G half 2 (col-wise, msg[1,3,5,7]).
    for i in 0..4 {
        half_g::<AB>(
            builder,
            states[1].row1[i],
            states[1].row2[i],
            states[1].row3[i],
            states[1].row4[i],
            msg[2 * i + 1].clone(),
            true,
            states[2].row1[i],
            states[2].row2[i],
            states[2].row3[i],
            states[2].row4[i],
            is_activated.clone(),
        );
    }
    // Diagonal G half 1 (msg[8,10,12,14]) — diagonal index shifts.
    for i in 0..4 {
        half_g::<AB>(
            builder,
            states[2].row1[i],
            states[2].row2[(i + 1) % 4],
            states[2].row3[(i + 2) % 4],
            states[2].row4[(i + 3) % 4],
            msg[8 + 2 * i].clone(),
            false,
            states[3].row1[i],
            states[3].row2[(i + 1) % 4],
            states[3].row3[(i + 2) % 4],
            states[3].row4[(i + 3) % 4],
            is_activated.clone(),
        );
    }
    // Diagonal G half 2 (msg[9,11,13,15]).
    for i in 0..4 {
        half_g::<AB>(
            builder,
            states[3].row1[i],
            states[3].row2[(i + 1) % 4],
            states[3].row3[(i + 2) % 4],
            states[3].row4[(i + 3) % 4],
            msg[8 + 2 * i + 1].clone(),
            true,
            states[4].row1[i],
            states[4].row2[(i + 1) % 4],
            states[4].row3[(i + 2) % 4],
            states[4].row4[(i + 3) % 4],
            is_activated.clone(),
        );
    }
    let _ = is_activated;
}

/// Round-8 finalization: compute the BLAKE3 output by XORing
/// state-row pairs. Mirrors Pearl's `finalize_blake`
/// (`blake3_air.rs:149-176`).
///
/// Pearl reuses `states[1].row2` as the bit decomposition of
/// `states[0].row1`, asserting `states[0].row1[i] ==
/// polyval(states[1].row2[i], 2)` so the XOR can be computed
/// bit-wise. Same trick for row3 / row4.
///
/// Returns the 8 output u32 expressions (4 from row1 XOR row3,
/// 4 from row2 XOR row4). Caller asserts equality with the row's
/// `CV_OUT` columns.
pub fn finalize_blake<AB: AirBuilder>(
    builder: &mut AB,
    states: &[Blake3State<'_, AB::Var>; 5],
    is_activated: AB::Expr,
) -> [AB::Expr; 8] {
    // Repurpose states[1].row2 as the bit decomposition of
    // states[0].row1.
    for i in 0..4 {
        let packed = polyval_bits::<AB>(states[1].row2[i]);
        let lhs: AB::Expr = states[0].row1[i].into();
        builder.assert_zero(is_activated.clone() * (lhs - packed));
    }
    // Same for row3 / row4.
    for i in 0..4 {
        let packed = polyval_bits::<AB>(states[1].row4[i]);
        let lhs: AB::Expr = states[0].row3[i].into();
        builder.assert_zero(is_activated.clone() * (lhs - packed));
    }

    // Compute the XOR outputs via xor_32_packed (no shift, no
    // gating — the bits are already boolean-checked elsewhere).
    let r1_xor_r3: [AB::Expr; 4] = core::array::from_fn(|i| {
        xor_32_packed::<AB>(builder, states[1].row2[i], states[1].row4[i])
    });
    let r2_xor_r4: [AB::Expr; 4] = core::array::from_fn(|i| {
        xor_32_packed::<AB>(builder, states[0].row2[i], states[0].row4[i])
    });

    core::array::from_fn(|i| {
        if i < 4 {
            r1_xor_r3[i].clone()
        } else {
            r2_xor_r4[i - 4].clone()
        }
    })
}

/// Verify the initial state of a new BLAKE3 compression. Mirrors
/// Pearl's `verify_init_state` (`blake3_air.rs:178-223`).
///
/// On `is_new_blake = 1`:
///   * `row1[i] == cv[i]` for i in 0..4
///   * `polyval(row2[i], 2) == cv[i + 4]` for i in 0..4
///   * `row3[i] == BLAKE3_IV[i]` for i in 0..4
///   * `row4` encodes the BLAKE3 tweak:
///       row4[0]      → counter_low  (32 bits)
///       row4[1][0..16] → counter_high (16 bits)
///       row4[3][0..8]  → flags        (8 bits)
///       row4[2][0..7]  → block_len    (7 bits)
///     Packed via polyval base 2 must equal `blake3_tweak`.
///   * The remaining bits of row4 (counter_high[16..32], block_len[7..32],
///     flags[8..32]) are forced to zero.
pub fn verify_init_state<AB: AirBuilder>(
    builder: &mut AB,
    init: &Blake3State<'_, AB::Var>,
    is_new_blake: AB::Expr,
    cv: &[AB::Expr],
    blake3_tweak: AB::Expr,
) {
    assert_eq!(cv.len(), 8);

    // row1[i] == cv[i].
    for i in 0..4 {
        let diff: AB::Expr = AB::Expr::from(init.row1[i]) - cv[i].clone();
        builder.assert_zero(is_new_blake.clone() * diff);
    }
    // row3[i] == BLAKE3_IV[i].
    for i in 0..4 {
        let iv_const = <AB::F as PrimeCharacteristicRing>::from_u32(BLAKE3_IV[i]);
        let diff: AB::Expr = AB::Expr::from(init.row3[i]) - AB::Expr::from(iv_const);
        builder.assert_zero(is_new_blake.clone() * diff);
    }
    // polyval(row2[i], 2) == cv[i + 4].
    for i in 0..4 {
        let packed = polyval_bits::<AB>(init.row2[i]);
        builder.assert_zero(is_new_blake.clone() * (packed - cv[i + 4].clone()));
    }

    // row4 packs (counter_low, counter_high[0..16], flags[0..8],
    // block_len[0..7]) into one 63-bit value matching blake3_tweak.
    let mut active_bits: Vec<AB::Expr> = Vec::with_capacity(63);
    for &b in init.row4[0].iter() {
        active_bits.push(b.into());
    }
    for &b in init.row4[1][..16].iter() {
        active_bits.push(b.into());
    }
    for &b in init.row4[3][..8].iter() {
        active_bits.push(b.into());
    }
    for &b in init.row4[2][..7].iter() {
        active_bits.push(b.into());
    }
    debug_assert_eq!(active_bits.len(), 63);

    let two = <AB::F as PrimeCharacteristicRing>::TWO;
    let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
    for bit in active_bits.iter() {
        acc = acc + bit.clone() * pow.clone();
        pow = pow * two.clone();
    }
    builder.assert_zero(is_new_blake.clone() * (acc - blake3_tweak));

    // Remaining bits forced to zero.
    let zero_bit_columns: [&[AB::Var]; 3] =
        [&init.row4[1][16..], &init.row4[2][7..], &init.row4[3][8..]];
    for slice in zero_bit_columns.iter() {
        for &b in slice.iter() {
            let z = is_new_blake.clone() * b;
            builder.assert_zero(z);
        }
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end round-AIR tests: run a known BLAKE3 round through
    //! the constraint system and assert prove + verify succeed.
    //! Then tamper specific intermediate-state cells and assert
    //! rejection.
    use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
    use p3_field::integers::QuotientMap;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::chips::blake3::compress::{round_with_snapshots, BLAKE3_IV};
    use crate::chips::blake3::layout::LIMBS_PER_STATE_SNAPSHOT;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::params::ZkParams;
    use crate::Val;

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

    // Trace layout for round-AIR tests:
    //
    // Per row of width `W` (= 5 * 264 = 1320 + 16 message cols):
    //   cells [0 .. 264]       : state[0] (INPUT_STATE)
    //   cells [264 .. 528]     : state[1] (STATE1)
    //   cells [528 .. 792]     : state[2] (STATE2)
    //   cells [792 .. 1056]    : state[3] (STATE3)
    //   cells [1056 .. 1320]   : state[4] (next row's INPUT_STATE; here colocated)
    //   cells [1320 .. 1336]   : message (16 packed u32 words)
    const STATE_W: usize = LIMBS_PER_STATE_SNAPSHOT; // 264
    const MSG_OFF: usize = 5 * STATE_W; // 1320
    const W: usize = 5 * STATE_W + 16; // 1336

    /// Write a 16-word BLAKE3 state into the row's state-snapshot
    /// slice. row1 and row3 are packed u32s; row2 and row4 are
    /// LSB-first 32-bit decompositions.
    fn write_state(state: &[u32; 16], dest: &mut [Val]) {
        assert_eq!(dest.len(), LIMBS_PER_STATE_SNAPSHOT);
        let mut off = 0;
        // row1: state[0..4]
        for i in 0..4 {
            dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
            off += 1;
        }
        // row2: state[4..8] as 32 bits LSB-first
        for i in 4..8 {
            for bit in 0..32 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                off += 1;
            }
        }
        // row3: state[8..12]
        for i in 8..12 {
            dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
            off += 1;
        }
        // row4: state[12..16] as 32 bits LSB-first
        for i in 12..16 {
            for bit in 0..32 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                off += 1;
            }
        }
        debug_assert_eq!(off, LIMBS_PER_STATE_SNAPSHOT);
    }

    /// AIR that runs `verify_round` on a single-row trace with
    /// `is_activated = 1`.
    #[derive(Debug, Default)]
    struct RoundAir;
    impl<F> BaseAir<F> for RoundAir {
        fn width(&self) -> usize {
            W
        }
    }
    impl<AB: AirBuilder> Air<AB> for RoundAir {
        fn eval(&self, builder: &mut AB) {
            let main = builder.main();
            let cur = main.current_slice();

            // Slice each 264-cell snapshot from the row.
            let s0 = Blake3State::from_slice(&cur[0 * STATE_W..1 * STATE_W]);
            let s1 = Blake3State::from_slice(&cur[1 * STATE_W..2 * STATE_W]);
            let s2 = Blake3State::from_slice(&cur[2 * STATE_W..3 * STATE_W]);
            let s3 = Blake3State::from_slice(&cur[3 * STATE_W..4 * STATE_W]);
            let s4 = Blake3State::from_slice(&cur[4 * STATE_W..5 * STATE_W]);
            let states = [s0, s1, s2, s3, s4];

            // Message: 16 packed u32s at cols [1320..1336].
            let msg: Vec<AB::Expr> = (0..16).map(|i| AB::Expr::from(cur[MSG_OFF + i])).collect();

            verify_round::<AB>(
                builder,
                &states,
                &msg,
                <AB::Expr as PrimeCharacteristicRing>::ONE,
            );
        }
    }

    /// Build a 4-row trace where every row contains a valid round
    /// computation. The `is_activated = 1` gating means each row
    /// must independently satisfy the round constraints. We use
    /// the same input state + message across all 4 rows for
    /// simplicity.
    fn build_valid_round_trace(
        initial_state: [u32; 16],
        message: [u32; 16],
    ) -> RowMajorMatrix<Val> {
        let rows = 4;
        let mut flat = vec![Val::default(); rows * W];

        for r in 0..rows {
            let row_start = r * W;
            let row = &mut flat[row_start..row_start + W];

            // Compute the round step-by-step.
            let mut state = initial_state;
            let snapshots = round_with_snapshots(&mut state, &message);

            // Write the 5 states.
            write_state(&initial_state, &mut row[0 * STATE_W..1 * STATE_W]);
            write_state(&snapshots[0], &mut row[1 * STATE_W..2 * STATE_W]);
            write_state(&snapshots[1], &mut row[2 * STATE_W..3 * STATE_W]);
            write_state(&snapshots[2], &mut row[3 * STATE_W..4 * STATE_W]);
            write_state(&snapshots[3], &mut row[4 * STATE_W..5 * STATE_W]);

            // Write the message.
            for i in 0..16 {
                row[MSG_OFF + i] = <Val as QuotientMap<u64>>::from_int(message[i] as u64);
            }
        }
        RowMajorMatrix::new(flat, W)
    }

    /// Build a canonical "BLAKE3-style" initial state: chaining
    /// values are first 8 IVs, plus the IV constants and a fixed
    /// counter / block_len / flags pattern.
    fn canonical_initial_state() -> [u32; 16] {
        let mut s = [0u32; 16];
        for i in 0..8 {
            s[i] = BLAKE3_IV[i];
        }
        for i in 0..4 {
            s[8 + i] = BLAKE3_IV[i];
        }
        // counter_low, counter_high, block_len, flags:
        s[12] = 0;
        s[13] = 0;
        s[14] = 64;
        s[15] = 0x1B;
        s
    }

    fn canonical_message() -> [u32; 16] {
        core::array::from_fn(|i| (i as u32 + 1) * 0x01020304)
    }

    #[test]
    fn round_with_snapshots_produces_4_snapshots() {
        let mut s = canonical_initial_state();
        let snaps = round_with_snapshots(&mut s, &canonical_message());
        // snapshots[3] equals the final state.
        assert_eq!(snaps[3], s);
        // Snapshots differ from each other (round actually does work).
        assert_ne!(snaps[0], snaps[1]);
        assert_ne!(snaps[1], snaps[2]);
        assert_ne!(snaps[2], snaps[3]);
    }

    #[test]
    fn prove_and_verify_valid_round() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_valid_round_trace(canonical_initial_state(), canonical_message());
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[])
            .expect("valid BLAKE3 round must verify");
    }

    /// Tamper an intermediate state cell to break the round
    /// constraint. STATE1.row1[0] should be the result of the
    /// first column-G half. Patching it to a wrong value must
    /// reject.
    #[test]
    fn verify_rejects_tampered_state1_row1() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_valid_round_trace(canonical_initial_state(), canonical_message());
        // STATE1 starts at offset STATE_W; row1[0] at offset 0 within state.
        trace.values[STATE_W] = <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEF);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[]).is_err(),
            "tampered STATE1.row1 must reject"
        );
    }

    /// Tamper STATE2 to break the second half-G constraint.
    #[test]
    fn verify_rejects_tampered_state2_row3() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_valid_round_trace(canonical_initial_state(), canonical_message());
        // STATE2 starts at offset 2 * STATE_W; row3[0] at offset 4 + 128 = 132 within state.
        let off = 2 * STATE_W + 4 + 128;
        trace.values[off] = <Val as QuotientMap<u64>>::from_int(0x12345678);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[]).is_err(),
            "tampered STATE2.row3 must reject"
        );
    }

    /// Tamper a message word — the round constraint depends on
    /// msg[0..16] for the additions.
    #[test]
    fn verify_rejects_tampered_message() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_valid_round_trace(canonical_initial_state(), canonical_message());
        // Flip msg[5] at row 0.
        trace.values[MSG_OFF + 5] = <Val as QuotientMap<u64>>::from_int(0xCAFEBABE);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[]).is_err(),
            "tampered message must reject"
        );
    }

    /// Tamper a row2 bit in STATE2 (bit columns must be boolean
    /// AND match the recomposed packed value).
    #[test]
    fn verify_rejects_non_boolean_bit_in_state2_row2() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_valid_round_trace(canonical_initial_state(), canonical_message());
        // STATE2 row2 starts at offset 2*STATE_W + 4 within state.
        let off = 2 * STATE_W + 4;
        trace.values[off] = <Val as QuotientMap<u64>>::from_int(2); // non-boolean
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[]).is_err(),
            "non-boolean bit must reject"
        );
    }

    /// Two rows, different message → both must verify independently.
    /// Confirms the per-row round constraint really does fire on
    /// every row.
    #[test]
    fn prove_and_verify_two_different_rounds() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let rows = 4;
        let mut flat = vec![Val::default(); rows * W];

        // Two distinct (state, message) pairs across rows.
        let states = [canonical_initial_state(), canonical_initial_state()];
        let msgs = [
            canonical_message(),
            core::array::from_fn::<u32, 16, _>(|i| (i as u32 + 1) * 0xDEAD_BEEF),
        ];

        for r in 0..rows {
            let pick = r % 2;
            let row_start = r * W;
            let row = &mut flat[row_start..row_start + W];
            let mut s = states[pick];
            let snaps = round_with_snapshots(&mut s, &msgs[pick]);
            write_state(&states[pick], &mut row[0..STATE_W]);
            write_state(&snaps[0], &mut row[STATE_W..2 * STATE_W]);
            write_state(&snaps[1], &mut row[2 * STATE_W..3 * STATE_W]);
            write_state(&snaps[2], &mut row[3 * STATE_W..4 * STATE_W]);
            write_state(&snaps[3], &mut row[4 * STATE_W..5 * STATE_W]);
            for i in 0..16 {
                row[MSG_OFF + i] = <Val as QuotientMap<u64>>::from_int(msgs[pick][i] as u64);
            }
        }
        let trace = RowMajorMatrix::new(flat, W);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &RoundAir, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &RoundAir, &proof, &[])
            .expect("two distinct rounds must verify");
    }

    #[test]
    fn blake3_state_from_slice_pins_layout() {
        // Build a 264-cell slice with distinct sentinel values per
        // section. Confirm from_slice routes them to the right
        // row1/row2/row3/row4 buckets.
        let slice: Vec<Val> = (0..LIMBS_PER_STATE_SNAPSHOT)
            .map(|i| <Val as QuotientMap<u64>>::from_int(i as u64))
            .collect();
        let s = Blake3State::from_slice(&slice);
        use p3_field::PrimeField64;
        // row1: cells 0..4.
        for i in 0..4 {
            assert_eq!(s.row1[i].as_canonical_u64(), i as u64);
        }
        // row2[0]: cells 4..36 (32 bits).
        assert_eq!(s.row2[0][0].as_canonical_u64(), 4);
        assert_eq!(s.row2[0][31].as_canonical_u64(), 35);
        // row3: cells 4 + 4*32 = 132..136.
        for i in 0..4 {
            assert_eq!(s.row3[i].as_canonical_u64(), (132 + i) as u64);
        }
        // row4[0]: cells 136..168.
        assert_eq!(s.row4[0][0].as_canonical_u64(), 136);
        assert_eq!(s.row4[3][31].as_canonical_u64(), 263);
    }
}
