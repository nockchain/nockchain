//! Per-round BLAKE3 column layout.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/blake3_layout.rs`.
//! The BLAKE3 round AIR occupies [`crate::composite_layout::BLAKE3_ROUND_LEN`]
//! = 1056 columns per row. This module defines the sub-layout *within*
//! that 1056-column block.
//!
//! ## Per-round structure
//!
//! Each row holds four state snapshots — one per stage in Pearl's
//! round-arithmetic decomposition:
//!
//! ```text
//!   INPUT_STATE: state at the start of this round (16 u32s spread
//!                across 4 state-rows: [row1: 4 limbs, row2: 128 limbs,
//!                row3: 4 limbs, row4: 128 limbs]).
//!   STATE1:      state after the first column-G half-round.
//!   STATE2:      state after the second column-G half-round.
//!   STATE3:      state after both diagonal-G half-rounds = round
//!                output (also the input to the *next* row's
//!                INPUT_STATE).
//! ```
//!
//! State storage convention (from Pearl, `blake3_layout.rs:6-25`):
//!
//!   * ROW1, ROW3 columns (4 limbs each): 16-bit packed limbs.
//!     These hold state words 0, 4, 8, 12 (ROW1) and 8, 12, …
//!     (ROW3) — i.e. the cells the G function adds to (cumulative
//!     adders, where range-checking 16-bit limbs is useful).
//!   * ROW2, ROW4 columns (128 limbs each): individual 32-bit bit
//!     decompositions. These hold the cells the G function XOR-
//!     rotates (where bit-level access is needed for the rotate
//!     operation). 128 = 32 bits × 4 cells per state row.
//!
//! Per-stage size: 4 + 128 + 4 + 128 = 264 limbs.
//! Total per row: 4 × 264 = 1056 limbs = `BLAKE3_ROUND_LEN`.

use crate::composite_layout::BLAKE3_ROUND_START;

/// Limbs in a row1-style state (ADD-side, packed 16-bit).
pub const ROW1_LIMBS: usize = 4;
/// Limbs in a row2-style state (XOR-side, bit-decomposed).
pub const ROW2_LIMBS: usize = 128;
/// Limbs in a row3-style state (ADD-side, packed 16-bit).
pub const ROW3_LIMBS: usize = 4;
/// Limbs in a row4-style state (XOR-side, bit-decomposed).
pub const ROW4_LIMBS: usize = 128;

/// Limbs per state snapshot: 4 + 128 + 4 + 128 = 264.
pub const LIMBS_PER_STATE_SNAPSHOT: usize = ROW1_LIMBS + ROW2_LIMBS + ROW3_LIMBS + ROW4_LIMBS;

/// State snapshots per round: input + state1 + state2 + state3.
pub const NUM_STATE_SNAPSHOTS_PER_ROUND: usize = 4;

/// Total columns this layout consumes inside `BLAKE3_ROUND_START..
/// BLAKE3_ROUND_END`. Pearl pins this at 1056.
pub const TOTAL_LIMBS_PER_ROUND: usize = LIMBS_PER_STATE_SNAPSHOT * NUM_STATE_SNAPSHOTS_PER_ROUND;

// =====================================================================
//  INPUT_STATE — state at the start of this round.
// =====================================================================

pub const INPUT_STATE_ROW1_START: usize = BLAKE3_ROUND_START;
pub const INPUT_STATE_ROW2_START: usize = INPUT_STATE_ROW1_START + ROW1_LIMBS;
pub const INPUT_STATE_ROW3_START: usize = INPUT_STATE_ROW2_START + ROW2_LIMBS;
pub const INPUT_STATE_ROW4_START: usize = INPUT_STATE_ROW3_START + ROW3_LIMBS;
pub const INPUT_STATE_END: usize = INPUT_STATE_ROW4_START + ROW4_LIMBS;

// =====================================================================
//  STATE1 — state after the first column-G half-round.
// =====================================================================

pub const STATE1_ROW1_START: usize = INPUT_STATE_END;
pub const STATE1_ROW2_START: usize = STATE1_ROW1_START + ROW1_LIMBS;
pub const STATE1_ROW3_START: usize = STATE1_ROW2_START + ROW2_LIMBS;
pub const STATE1_ROW4_START: usize = STATE1_ROW3_START + ROW3_LIMBS;
pub const STATE1_END: usize = STATE1_ROW4_START + ROW4_LIMBS;

// =====================================================================
//  STATE2 — state after the second column-G half-round.
// =====================================================================

pub const STATE2_ROW1_START: usize = STATE1_END;
pub const STATE2_ROW2_START: usize = STATE2_ROW1_START + ROW1_LIMBS;
pub const STATE2_ROW3_START: usize = STATE2_ROW2_START + ROW2_LIMBS;
pub const STATE2_ROW4_START: usize = STATE2_ROW3_START + ROW3_LIMBS;
pub const STATE2_END: usize = STATE2_ROW4_START + ROW4_LIMBS;

// =====================================================================
//  STATE3 — state after both diagonal-G half-rounds (round output).
// =====================================================================

pub const STATE3_ROW1_START: usize = STATE2_END;
pub const STATE3_ROW2_START: usize = STATE3_ROW1_START + ROW1_LIMBS;
pub const STATE3_ROW3_START: usize = STATE3_ROW2_START + ROW2_LIMBS;
pub const STATE3_ROW4_START: usize = STATE3_ROW3_START + ROW3_LIMBS;
pub const STATE3_END: usize = STATE3_ROW4_START + ROW4_LIMBS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite_layout::{BLAKE3_ROUND_LEN, BLAKE3_ROUND_START};

    #[test]
    fn per_snapshot_limbs_are_264() {
        assert_eq!(LIMBS_PER_STATE_SNAPSHOT, 264);
    }

    #[test]
    fn total_limbs_matches_blake3_round_len() {
        // Pearl pins BLAKE3_ROUND_LEN = 1056 = 4 × 264.
        assert_eq!(TOTAL_LIMBS_PER_ROUND, BLAKE3_ROUND_LEN);
        assert_eq!(TOTAL_LIMBS_PER_ROUND, 1056);
    }

    #[test]
    fn state3_end_matches_blake3_round_end() {
        // The four snapshots should fill exactly the BLAKE3_ROUND
        // column block, starting at BLAKE3_ROUND_START and ending at
        // BLAKE3_ROUND_START + BLAKE3_ROUND_LEN.
        assert_eq!(STATE3_END, BLAKE3_ROUND_START + BLAKE3_ROUND_LEN);
    }

    #[test]
    fn snapshot_offsets_are_contiguous() {
        // Within each snapshot, the four row starts strictly
        // increase. Between snapshots, the previous snapshot's
        // *_END equals the next snapshot's *_ROW1_START — they're
        // contiguous, not gapped.
        assert!(INPUT_STATE_ROW1_START < INPUT_STATE_ROW2_START);
        assert!(INPUT_STATE_ROW2_START < INPUT_STATE_ROW3_START);
        assert!(INPUT_STATE_ROW3_START < INPUT_STATE_ROW4_START);
        assert!(INPUT_STATE_ROW4_START < INPUT_STATE_END);
        assert_eq!(INPUT_STATE_END, STATE1_ROW1_START);
        assert!(STATE1_ROW1_START < STATE1_END);
        assert_eq!(STATE1_END, STATE2_ROW1_START);
        assert!(STATE2_ROW1_START < STATE2_END);
        assert_eq!(STATE2_END, STATE3_ROW1_START);
        assert!(STATE3_ROW1_START < STATE3_END);
    }

    #[test]
    fn pearl_row_widths_match() {
        // Pearl `chip/blake3/blake3_layout.rs:6-25`:
        //   INPUT_STATE_ROW1: 4, ROW2: 128, ROW3: 4, ROW4: 128.
        //   STATE1_*, STATE2_*, STATE3_*: same as INPUT.
        assert_eq!(ROW1_LIMBS, 4);
        assert_eq!(ROW2_LIMBS, 128);
        assert_eq!(ROW3_LIMBS, 4);
        assert_eq!(ROW4_LIMBS, 128);
    }
}
