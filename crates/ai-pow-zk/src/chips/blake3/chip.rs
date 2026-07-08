//! Top-level BLAKE3 chip — selector-gated dispatch of Phase 8a/b
//! primitives across an 8-row hash instruction.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/blake3_air.rs` —
//! the chip-level `eval` that wires `verify_init_state`,
//! `verify_round`, and `finalize_blake` to per-row selector bits.
//! Pearl spreads one BLAKE3 hash across `ROUNDS_PER_BLAKE_INSTRUCTION
//! = 8` rows: 7 mixing rounds (rows 0..7) + 1 finalize row (row 7).
//!
//! For this phase the chip operates on a **chip-local column
//! layout** (see [`cols`]). Phase 12 will wire it into the
//! composite layout's BLAKE3 block; until then we want the chip
//! and its tests to stay independent of the global trace.
//!
//! ## Per-row dispatch
//!
//! | Selector | Constraint fires |
//! |---|---|
//! | `is_new_blake = 1` | `verify_init_state` on `STATE0` |
//! | `is_round_active = 1 - is_last_round` | `verify_round` on `STATE0..STATE3` + `next.STATE0` |
//! | `is_last_round = 1` | `finalize_blake` on `STATE0..STATE1`, output → `CV_OUT` |
//!
//! Boolean checks on `is_new_blake` and `is_last_round` are
//! enforced (they otherwise leak into the gating arithmetic).
//!
//! ## Validation strategy
//!
//! The chip-level test [`prove_and_verify_one_hash`] builds the
//! complete 8-row trace of a single BLAKE3 compression call from
//! [`compress::blake3_compress`]'s scalar reference, then proves
//! and verifies it against this chip's AIR. Tamper tests then
//! mutate individual cells to confirm the constraint detects each
//! kind of attack:
//!   * Wrong initial CV (would let a miner skip the keyed-hash bind).
//!   * Wrong round-internal state (would let a miner shortcut work).
//!   * Wrong `is_new_blake` selector (would skip init constraint).
//!   * Wrong `is_last_round` selector (would skip finalize).
//!   * Wrong CV_OUT (would let a miner forge the output).

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use super::compress::{
    blake3_permute_msg, compress_full_state, round_with_snapshots, Blake3Tweak, BLAKE3_IV,
};
use super::layout::LIMBS_PER_STATE_SNAPSHOT;
use super::round_air::{finalize_blake, verify_init_state, verify_round, Blake3State};

/// Chip-local column offsets. Used by both [`Blake3Chip::eval`]
/// and [`Blake3Chip::fill_one_hash`] so the AIR and the trace
/// generator can't drift.
pub mod cols {
    use super::LIMBS_PER_STATE_SNAPSHOT;

    /// 264 cells per state snapshot (4 packed + 4×32 bits + 4 packed
    /// + 4×32 bits). Matches `super::layout::LIMBS_PER_STATE_SNAPSHOT`.
    pub const STATE_W: usize = LIMBS_PER_STATE_SNAPSHOT;

    pub const STATE0: usize = 0;
    pub const STATE1: usize = STATE0 + STATE_W; // 264
    pub const STATE2: usize = STATE1 + STATE_W; // 528
    pub const STATE3: usize = STATE2 + STATE_W; // 792

    /// 16 packed u32 message words for this row's round.
    pub const MSG: usize = STATE3 + STATE_W; // 1056
    pub const MSG_LEN: usize = 16;

    /// 8 packed u32 CV words (the chaining value entering BLAKE3
    /// for this hash instruction). Replicated across all 8 rows of
    /// the instruction so the gated `verify_init_state` constraint
    /// fires correctly on the first row.
    pub const CV_IN: usize = MSG + MSG_LEN; // 1072
    pub const CV_IN_LEN: usize = 8;

    /// BLAKE3 compression tweak (counter_low || counter_high[0..16]
    /// || flags[0..8] || block_len[0..7]) packed via `polyval(...,
    /// 2)`. Replicated across the 8 rows.
    pub const TWEAK: usize = CV_IN + CV_IN_LEN; // 1080

    /// 8 packed u32 CV words — the BLAKE3 output. Only meaningful
    /// on the finalize row; free on other rows.
    pub const CV_OUT: usize = TWEAK + 1; // 1081
    pub const CV_OUT_LEN: usize = 8;

    /// Per-row selectors. Boolean. Only one of the two should be 1
    /// in any given row; both can be 0 (a non-instruction row).
    pub const IS_NEW_BLAKE: usize = CV_OUT + CV_OUT_LEN; // 1089
    pub const IS_LAST_ROUND: usize = IS_NEW_BLAKE + 1; // 1090

    /// Total width of the chip's local row.
    pub const ROW_W: usize = IS_LAST_ROUND + 1; // 1091
}

/// BLAKE3 round-AIR chip.
///
/// Zero-sized; all per-hash state is in the trace. Conceptually
/// equivalent to Pearl's `Blake3Air` (a thin wrapper around the
/// constraint dispatch).
#[derive(Copy, Clone, Debug, Default)]
pub struct Blake3Chip;

impl<F> BaseAir<F> for Blake3Chip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

/// Column-offset bundle for [`Blake3Chip::eval_at`]. Lets the
/// same eval body work over both the chip-local layout and the
/// composite layout.
#[derive(Copy, Clone, Debug)]
pub struct Blake3Offsets {
    /// Offset of the row's first state snapshot (4 contiguous
    /// snapshots × 264 cells = 1056 cells starting here).
    pub state_start: usize,
    /// Offset of the BLAKE3 message (16 packed u32 words).
    pub msg_start: usize,
    /// Offset of the CV (8 packed u32 words).
    pub cv_start: usize,
    /// Offset of the packed tweak column.
    pub tweak_col: usize,
    /// Offset of the row's CV_OUT (8 packed u32 words).
    pub cv_out_start: usize,
    /// Column of the IS_NEW_BLAKE selector.
    pub is_new_blake_col: usize,
    /// Column of the IS_LAST_ROUND selector.
    pub is_last_round_col: usize,
    /// §M-S5b Path A column-overlay — optional pair of activity
    /// selector columns that additionally exclude a row from the
    /// `verify_round` constraint. In the composite these are the
    /// matmul `IS_RESET_CUMSUM` / `IS_UPDATE_CUMSUM` selectors
    /// (pinned via the CRIT-1 `CONTROL_PREP`): StripeXor activity is
    /// co-located on the matmul sweep, and the StripeXor
    /// bit-decomposition columns are aliased onto a BLAKE3-round
    /// sub-window, so `verify_round` must be vacuous on matmul rows.
    /// `None` for the standalone chip (no matmul selectors exist).
    pub round_gate_excl: Option<(usize, usize)>,
}

impl Blake3Chip {
    /// Chip-local offsets. Used by [`Blake3Chip::eval`].
    pub const LOCAL_OFFSETS: Blake3Offsets = Blake3Offsets {
        state_start: cols::STATE0,
        msg_start: cols::MSG,
        cv_start: cols::CV_IN,
        tweak_col: cols::TWEAK,
        cv_out_start: cols::CV_OUT,
        is_new_blake_col: cols::IS_NEW_BLAKE,
        is_last_round_col: cols::IS_LAST_ROUND,
        round_gate_excl: None,
    };

    /// Composite-trace offsets. Maps each of the chip's column
    /// blocks onto `composite_layout::*` positions. Note we read
    /// CV from `BLAKE3_CV_START` (the value "ready for BLAKE3"
    /// on this row) rather than `CV_IN_START` (the value pulled
    /// in from a previous hash via LogUp).
    pub const COMPOSITE_OFFSETS: Blake3Offsets = Blake3Offsets {
        state_start: crate::composite_layout::BLAKE3_ROUND_START,
        msg_start: crate::composite_layout::BLAKE3_MSG_START,
        cv_start: crate::composite_layout::BLAKE3_CV_START,
        tweak_col: crate::composite_layout::CV_OR_TWEAK_PREP,
        cv_out_start: crate::composite_layout::CV_OUT_START,
        is_new_blake_col: crate::composite_layout::IS_NEW_BLAKE,
        is_last_round_col: crate::composite_layout::IS_LAST_ROUND,
        round_gate_excl: Some((
            crate::composite_layout::IS_RESET_CUMSUM,
            crate::composite_layout::IS_UPDATE_CUMSUM,
        )),
    };

    /// Emit the BLAKE3 chip's constraints at the given column
    /// offsets. The constraint logic is identical to the
    /// chip-local case; only the column read positions change.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &Blake3Offsets) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        // ---- Selector reads + boolean checks ----
        let is_new_blake_var = cur[off.is_new_blake_col];
        let is_last_round_var = cur[off.is_last_round_col];
        let is_new_blake: AB::Expr = is_new_blake_var.into();
        let is_last_round: AB::Expr = is_last_round_var.into();

        // Booleanity.
        builder.assert_bool(is_new_blake_var);
        builder.assert_bool(is_last_round_var);

        // ---- State slicing ----
        let state_w = cols::STATE_W;
        let s0_cells = &cur[off.state_start..off.state_start + state_w];
        let s1_cells = &cur[off.state_start + state_w..off.state_start + 2 * state_w];
        let s2_cells = &cur[off.state_start + 2 * state_w..off.state_start + 3 * state_w];
        let s3_cells = &cur[off.state_start + 3 * state_w..off.state_start + 4 * state_w];
        let s4_cells = &nxt[off.state_start..off.state_start + state_w];

        let s0 = Blake3State::from_slice(s0_cells);
        let s1 = Blake3State::from_slice(s1_cells);
        let s2 = Blake3State::from_slice(s2_cells);
        let s3 = Blake3State::from_slice(s3_cells);
        let s4 = Blake3State::from_slice(s4_cells);
        let states = [s0, s1, s2, s3, s4];

        // ---- Message + CV_IN + Tweak ----
        let msg: Vec<AB::Expr> = (0..16).map(|i| cur[off.msg_start + i].into()).collect();
        let cv_in: Vec<AB::Expr> = (0..8).map(|i| cur[off.cv_start + i].into()).collect();
        let tweak: AB::Expr = cur[off.tweak_col].into();

        // ---- Round constraint gate ----
        //
        // `verify_round` is a CROSS-ROW constraint: `states[4]` is
        // the next row's STATE0 and it asserts
        // `next.STATE0 == Round(this row's STATE0..3, this msg)`.
        // It must fire only when this row→next row is a genuine
        // intra-blake round step, i.e. BOTH:
        //
        //   (1) this row is not the finalize row   : 1 − is_last_round
        //   (2) the next row continues the same hash: 1 − next_is_new_blake
        //
        // Factor (1) alone (the historical gate) wrongly left the
        // round ACTIVE on the non-blake row immediately preceding a
        // blake block (`is_last_round = 0` there), demanding
        // `next.STATE0 == Round(that row's state)` — which a fresh
        // blake init STATE0 cannot satisfy. That made a block only
        // verifiable when contiguous from trace row 0 (no leading
        // boundary). Factor (2) disables the round at that leading
        // boundary because the next row is `is_new_blake = 1`. See
        // `2026-05-15_BLAKE3_CHIP_ROUND_GATE_BUG.md`. Factor (1) is KEPT so the
        // trailing boundary (finalize → following row) stays
        // disabled exactly as before. `verify_init_state`
        // (gated by `is_new_blake`) independently pins the block's
        // first-row STATE0, so dropping the leading round link does
        // not unconstrain it.
        let next_is_new_blake: AB::Expr = nxt[off.is_new_blake_col].into();
        // §M-S5b Path A column-overlay — `verify_round` is a
        // CROSS-ROW constraint: it reads this row's STATE0..3 *and*
        // the next row's STATE0. The StripeXor bit-decomposition
        // columns are aliased onto a BLAKE3-round sub-window and
        // StripeXor activity is co-located on the matmul sweep, so
        // on a matmul row the BLAKE3-round columns hold StripeXor
        // data, not BLAKE3 state. `verify_round` must therefore be
        // vacuous on any transition that touches a matmul row —
        // both when *this* row is matmul (`first_factor`) and when
        // the *next* row is matmul (`next_factor`, since the
        // constraint reads `next.STATE0`). The matmul selectors
        // (`IS_RESET_CUMSUM` / `IS_UPDATE_CUMSUM`) are pinned via
        // the CRIT-1 `CONTROL_PREP` and 0 on every genuine BLAKE3
        // round row, so this never weakens a real round step; each
        // factor stays degree 1 (extra subtraction terms only).
        let mut first_factor: AB::Expr =
            <AB::Expr as PrimeCharacteristicRing>::ONE - is_last_round.clone();
        let mut next_factor: AB::Expr =
            <AB::Expr as PrimeCharacteristicRing>::ONE - next_is_new_blake;
        if let Some((excl_a, excl_b)) = off.round_gate_excl {
            first_factor = first_factor - cur[excl_a].into() - cur[excl_b].into();
            next_factor = next_factor - nxt[excl_a].into() - nxt[excl_b].into();
        }
        let is_round_active: AB::Expr = first_factor * next_factor;

        // verify_round only makes sense across two consecutive rows,
        // so guard with when_transition() (skips the last trace row).
        {
            let mut tb = builder.when_transition();
            verify_round::<_>(&mut tb, &states, &msg, is_round_active);
        }

        // ---- Init constraint, gated by is_new_blake ----
        //
        // First row of each hash sets STATE0 to (cv[0..4],
        // bit-decomp(cv[4..8]), IV[0..4], bit-decomp(tweak)).
        verify_init_state::<_>(builder, &states[0], is_new_blake, &cv_in, tweak);

        // ---- Finalize constraint, gated by is_last_round ----
        //
        // Last row of each hash applies the BLAKE3 feed-forward XOR
        // using STATE0 and STATE1 (with Pearl's bit-decomp reuse
        // trick on STATE1.row2 / STATE1.row4).
        let out = finalize_blake::<_>(builder, &states, is_last_round.clone());

        // CV_OUT[i] == out[i] when is_last_round = 1.
        for i in 0..8 {
            let cv_out_cell: AB::Expr = cur[off.cv_out_start + i].into();
            builder.assert_zero(is_last_round.clone() * (out[i].clone() - cv_out_cell));
        }
    }

    /// Composite-layout entry point. Called from
    /// [`crate::composite_full_air::CompositeFullAir::eval`].
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);
    }
}

impl<AB: AirBuilder> Air<AB> for Blake3Chip {
    fn eval(&self, builder: &mut AB) {
        Blake3Chip::eval_at(builder, &Blake3Chip::LOCAL_OFFSETS);
    }
}

/// Build one row from a (state, message, cv_in, tweak, cv_out,
/// is_new_blake, is_last_round) tuple — internal helper used by
/// the trace generator.
fn fill_row(
    dest: &mut [crate::Val],
    snap_input: &[u32; 16],
    snap1: &[u32; 16],
    snap2: &[u32; 16],
    snap3: &[u32; 16],
    msg: &[u32; 16],
    cv_in: &[u32; 8],
    tweak: u64,
    cv_out: &[u32; 8],
    is_new_blake: bool,
    is_last_round: bool,
) {
    use p3_field::integers::QuotientMap;

    use crate::Val;

    fn write_state_into(dest: &mut [Val], state: &[u32; 16]) {
        let mut off = 0;
        for i in 0..4 {
            dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
            off += 1;
        }
        for i in 4..8 {
            for bit in 0..32 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                off += 1;
            }
        }
        for i in 8..12 {
            dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
            off += 1;
        }
        for i in 12..16 {
            for bit in 0..32 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                off += 1;
            }
        }
        debug_assert_eq!(off, LIMBS_PER_STATE_SNAPSHOT);
    }

    // 4 state snapshots.
    write_state_into(
        &mut dest[cols::STATE0..cols::STATE0 + cols::STATE_W],
        snap_input,
    );
    write_state_into(&mut dest[cols::STATE1..cols::STATE1 + cols::STATE_W], snap1);
    write_state_into(&mut dest[cols::STATE2..cols::STATE2 + cols::STATE_W], snap2);
    write_state_into(&mut dest[cols::STATE3..cols::STATE3 + cols::STATE_W], snap3);

    // Message.
    for i in 0..16 {
        dest[cols::MSG + i] = <Val as QuotientMap<u64>>::from_int(msg[i] as u64);
    }
    // CV_IN.
    for i in 0..8 {
        dest[cols::CV_IN + i] = <Val as QuotientMap<u64>>::from_int(cv_in[i] as u64);
    }
    // Tweak.
    dest[cols::TWEAK] = <Val as QuotientMap<u64>>::from_int(tweak);
    // CV_OUT.
    for i in 0..8 {
        dest[cols::CV_OUT + i] = <Val as QuotientMap<u64>>::from_int(cv_out[i] as u64);
    }
    // Selectors.
    dest[cols::IS_NEW_BLAKE] =
        <Val as QuotientMap<u64>>::from_int(if is_new_blake { 1 } else { 0 });
    dest[cols::IS_LAST_ROUND] =
        <Val as QuotientMap<u64>>::from_int(if is_last_round { 1 } else { 0 });
}

/// Compute the 63-bit tweak word as `verify_init_state` expects it
/// to be packed:
///
/// ```text
///   bits 0..32  : counter_low
///   bits 32..48 : counter_high[0..16]
///   bits 48..56 : flags[0..8]
///   bits 56..63 : block_len[0..7]
/// ```
pub fn pack_tweak(t: &Blake3Tweak) -> u64 {
    let counter_low = t.counter_low as u64;
    let counter_high = (t.counter_high as u64) & 0xFFFF;
    let flags = (t.flags as u64) & 0xFF;
    let block_len = (t.block_len as u64) & 0x7F; // 7-bit slot
    counter_low | (counter_high << 32) | (flags << 48) | (block_len << 56)
}

impl Blake3Chip {
    /// Fill the 8 rows for one BLAKE3 hash instruction.
    ///
    /// On entry `dest` must be exactly 8 rows × `cols::ROW_W`
    /// values (zero-initialized). On exit:
    ///   * Rows 0..7 contain the 4 state snapshots after each
    ///     round, plus message + CV_IN + tweak + CV_OUT (last row
    ///     only) + selector bits.
    ///   * Row 0 has `is_new_blake = 1`; row 7 has `is_last_round = 1`.
    ///
    /// Selectors are placed so the AIR's gated constraints fire
    /// correctly: init on row 0, round on rows 0..6 (transition),
    /// finalize on row 7.
    pub fn fill_one_hash(
        dest: &mut [crate::Val],
        message: &[u32; 16],
        cv_in: &[u32; 8],
        tweak: &Blake3Tweak,
    ) {
        assert_eq!(
            dest.len(),
            8 * cols::ROW_W,
            "fill_one_hash expects exactly 8 rows × ROW_W cells"
        );

        let tweak_packed = pack_tweak(tweak);
        // Run the full BLAKE3 compression once to get the canonical
        // CV_OUT for the finalize row.
        let full_state = compress_full_state(
            cv_in, message, tweak.counter_low, tweak.counter_high as u32, tweak.block_len,
            tweak.flags,
        );
        let final_cv_out: [u32; 8] = core::array::from_fn(|i| full_state[i]);

        // Build the per-row "messages" — round 1 uses message as-is,
        // rounds 2..7 use successively permuted versions. (Round 7
        // == round_idx 6 zero-indexed.)
        let mut round_msgs: Vec<[u32; 16]> = Vec::with_capacity(7);
        let mut cur_msg = *message;
        round_msgs.push(cur_msg);
        for _ in 1..7 {
            blake3_permute_msg(&mut cur_msg);
            round_msgs.push(cur_msg);
        }
        // Round 8 (finalize row) has no round, but we put the
        // last-permuted message in for consistency.
        let finalize_msg = round_msgs[6];

        // Build the initial state: cv[0..8] ++ IV[0..4] ++ tweak[0..4].
        let mut state = [0u32; 16];
        for i in 0..8 {
            state[i] = cv_in[i];
        }
        for i in 0..4 {
            state[8 + i] = BLAKE3_IV[i];
        }
        state[12] = tweak.counter_low;
        state[13] = tweak.counter_high as u32;
        state[14] = tweak.block_len;
        state[15] = tweak.flags;

        // For each of the 7 mixing-round rows, run round_with_snapshots
        // and write the 4 snapshots.
        let mut current_input_state = state;
        for r in 0..7 {
            let row_start = r * cols::ROW_W;
            let row = &mut dest[row_start..row_start + cols::ROW_W];

            let mut s = current_input_state;
            let snaps = round_with_snapshots(&mut s, &round_msgs[r]);

            fill_row(
                row,
                &current_input_state,
                &snaps[0],
                &snaps[1],
                &snaps[2],
                &round_msgs[r],
                cv_in,
                tweak_packed,
                &final_cv_out,
                r == 0, // is_new_blake on row 0
                false,  // is_last_round
            );

            // Advance input state for next row.
            current_input_state = snaps[3];
        }

        // Row 7 — finalize row. STATE0 is the state after round 7
        // (== current_input_state). STATE1 needs to encode the bit
        // decomp of STATE0.row1 (i.e. state[0..4]) in its row2, and
        // the bit decomp of STATE0.row3 (i.e. state[8..12]) in its
        // row4. STATE2 and STATE3 are unconstrained; we leave them
        // zero.
        let final_input = current_input_state;
        let mut state1_for_finalize = [0u32; 16];
        // STATE1.row1 == state[0..4] (free; we set to match for cleanness)
        // STATE1.row2 (bit-decomp slot, idx 4..8) == STATE0.row1 (== state[0..4])
        // STATE1.row3 == state[8..12] (free)
        // STATE1.row4 (bit-decomp slot, idx 12..16) == STATE0.row3 (== state[8..12])
        state1_for_finalize[0] = final_input[0];
        state1_for_finalize[1] = final_input[1];
        state1_for_finalize[2] = final_input[2];
        state1_for_finalize[3] = final_input[3];
        // CRITICAL: the bit-decomp slots are at indices [4..8] (row2 cells)
        // and [12..16] (row4 cells) of the state interpreted as 16-word array.
        state1_for_finalize[4] = final_input[0];
        state1_for_finalize[5] = final_input[1];
        state1_for_finalize[6] = final_input[2];
        state1_for_finalize[7] = final_input[3];
        state1_for_finalize[8] = final_input[8];
        state1_for_finalize[9] = final_input[9];
        state1_for_finalize[10] = final_input[10];
        state1_for_finalize[11] = final_input[11];
        state1_for_finalize[12] = final_input[8];
        state1_for_finalize[13] = final_input[9];
        state1_for_finalize[14] = final_input[10];
        state1_for_finalize[15] = final_input[11];

        let row_start = 7 * cols::ROW_W;
        let row = &mut dest[row_start..row_start + cols::ROW_W];
        fill_row(
            row, &final_input, &state1_for_finalize, &[0u32; 16], &[0u32; 16], &finalize_msg,
            cv_in, tweak_packed, &final_cv_out, false, // is_new_blake
            true,  // is_last_round
        );
    }

    /// Same as [`Self::fill_one_hash`], but returns a freshly-
    /// allocated row-major matrix of exactly 8 rows.
    pub fn build_trace_one_hash(
        message: &[u32; 16],
        cv_in: &[u32; 8],
        tweak: &Blake3Tweak,
    ) -> RowMajorMatrix<crate::Val> {
        let mut flat = vec![crate::Val::default(); 8 * cols::ROW_W];
        Self::fill_one_hash(&mut flat, message, cv_in, tweak);
        RowMajorMatrix::new(flat, cols::ROW_W)
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end chip tests: build a complete 8-row trace from
    //! [`compress::blake3_compress`]'s scalar reference, then prove
    //! and verify with the chip's full AIR.

    use p3_field::integers::QuotientMap;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::chips::blake3::compress::{Blake3Tweak, BLAKE3_IV};
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

    /// Build a canonical (cv, msg, tweak) test vector. The values
    /// are arbitrary but deterministic — `blake3_compress` should
    /// be the source of truth for `cv_out`.
    fn canonical_inputs() -> ([u32; 8], [u32; 16], Blake3Tweak) {
        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0x01020304);
        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B, // CHUNK_START | CHUNK_END | ROOT | KEYED_HASH
        };
        (cv, msg, tweak)
    }

    #[test]
    fn fill_one_hash_writes_full_rows() {
        let (cv, msg, tweak) = canonical_inputs();
        let trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        // 8 rows × ROW_W cells.
        assert_eq!(trace.values.len(), 8 * cols::ROW_W);

        // Row 0 has is_new_blake = 1, is_last_round = 0.
        let r0 = &trace.values[0..cols::ROW_W];
        use p3_field::PrimeField64;
        assert_eq!(r0[cols::IS_NEW_BLAKE].as_canonical_u64(), 1);
        assert_eq!(r0[cols::IS_LAST_ROUND].as_canonical_u64(), 0);
        // Row 7 has is_new_blake = 0, is_last_round = 1.
        let r7 = &trace.values[7 * cols::ROW_W..8 * cols::ROW_W];
        assert_eq!(r7[cols::IS_NEW_BLAKE].as_canonical_u64(), 0);
        assert_eq!(r7[cols::IS_LAST_ROUND].as_canonical_u64(), 1);
        // Rows 1..6 have both = 0.
        for r in 1..7 {
            let row = &trace.values[r * cols::ROW_W..(r + 1) * cols::ROW_W];
            assert_eq!(row[cols::IS_NEW_BLAKE].as_canonical_u64(), 0);
            assert_eq!(row[cols::IS_LAST_ROUND].as_canonical_u64(), 0);
        }
    }

    #[test]
    fn cv_out_matches_compress_full_state() {
        let (cv, msg, tweak) = canonical_inputs();
        let full = compress_full_state(
            &cv, &msg, tweak.counter_low, tweak.counter_high as u32, tweak.block_len, tweak.flags,
        );
        let trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        use p3_field::PrimeField64;
        let r7 = &trace.values[7 * cols::ROW_W..8 * cols::ROW_W];
        for i in 0..8 {
            assert_eq!(
                r7[cols::CV_OUT + i].as_canonical_u64() as u32,
                full[i],
                "CV_OUT[{i}] mismatch"
            );
        }
    }

    #[test]
    fn prove_and_verify_one_hash() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv, msg, tweak) = canonical_inputs();
        let trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[])
            .expect("valid 8-row BLAKE3 trace must verify");
    }

    #[test]
    fn verify_rejects_wrong_cv_out() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv, msg, tweak) = canonical_inputs();
        let mut trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        // Flip a bit in CV_OUT on the finalize row.
        let cv_out_idx = 7 * cols::ROW_W + cols::CV_OUT;
        trace.values[cv_out_idx] = <crate::Val as QuotientMap<u64>>::from_int(0xDEADBEEF);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[]).is_err(),
            "tampered CV_OUT must reject"
        );
    }

    #[test]
    fn verify_rejects_wrong_initial_cv_row1_cell() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv, msg, tweak) = canonical_inputs();
        let mut trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        // Tamper STATE0.row1[0] on row 0. The init constraint forces
        // this to equal cv_in[0], so the change must reject.
        let target = 0 * cols::ROW_W + cols::STATE0;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(0xDEAD);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[]).is_err(),
            "tampered initial STATE0.row1[0] must reject"
        );
    }

    #[test]
    fn verify_rejects_wrong_intermediate_state() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv, msg, tweak) = canonical_inputs();
        let mut trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        // Tamper STATE2.row1[0] on row 3. This breaks round 4's
        // computation; the round constraint must reject.
        let target = 3 * cols::ROW_W + cols::STATE2;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(0xCAFEBABE);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[]).is_err(),
            "tampered intermediate state must reject"
        );
    }

    #[test]
    fn verify_rejects_non_boolean_is_new_blake() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv, msg, tweak) = canonical_inputs();
        let mut trace = Blake3Chip::build_trace_one_hash(&msg, &cv, &tweak);
        // Set is_new_blake = 2 on row 0 — non-boolean.
        let target = 0 * cols::ROW_W + cols::IS_NEW_BLAKE;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[]).is_err(),
            "non-boolean is_new_blake must reject"
        );
    }

    #[test]
    fn pack_tweak_round_trips() {
        // Hand-verify pack_tweak's bit layout: counter_low at 0,
        // counter_high at 32, flags at 48, block_len at 56.
        let t = Blake3Tweak {
            counter_low: 0xCAFE_F00D,
            counter_high: 0xBABE,
            block_len: 0x42,
            flags: 0xAB,
        };
        let packed = pack_tweak(&t);
        assert_eq!(packed & 0xFFFF_FFFF, 0xCAFE_F00D);
        assert_eq!((packed >> 32) & 0xFFFF, 0xBABE);
        assert_eq!((packed >> 48) & 0xFF, 0xAB);
        assert_eq!((packed >> 56) & 0x7F, 0x42);
    }

    #[test]
    fn pack_tweak_zero_returns_zero() {
        let t = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 0,
            flags: 0,
        };
        assert_eq!(pack_tweak(&t), 0);
    }

    /// Cross-check: a 2-hash trace (16 rows) should also verify.
    /// This is the multi-instruction case.
    ///
    /// Each 8-row block stands alone — selectors on row 0 + row 7 of
    /// each block correctly fire `verify_init_state` / `finalize_blake`.
    #[test]
    fn prove_and_verify_two_hashes() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (cv_a, msg_a, tweak_a) = canonical_inputs();
        let cv_b: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i].wrapping_add(0xDEADBEEF));
        let msg_b: [u32; 16] = core::array::from_fn(|i| (i as u32) * 0x12345);
        let tweak_b = Blake3Tweak {
            counter_low: 7,
            counter_high: 0,
            block_len: 32,
            flags: 0x0B,
        };
        let mut flat = vec![crate::Val::default(); 16 * cols::ROW_W];
        Blake3Chip::fill_one_hash(&mut flat[0..8 * cols::ROW_W], &msg_a, &cv_a, &tweak_a);
        Blake3Chip::fill_one_hash(&mut flat[8 * cols::ROW_W..], &msg_b, &cv_b, &tweak_b);
        let trace = RowMajorMatrix::new(flat, cols::ROW_W);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &Blake3Chip, &proof, &[])
            .expect("two-hash 16-row trace must verify");
    }
}
