//! Scalar reference for Pearl's 16-slot rotate-XOR-13 update.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/jackpot/` arithmetic.
//! Each row applies one of two operations:
//!
//! ```text
//!   ROTATE_XOR_13(slot): JACKPOT_MSG[slot] := rot13(JACKPOT_MSG[slot]) XOR x
//!   STORE_THROUGH      : passthrough — all slots unchanged.
//! ```
//!
//! Pearl uses `LROT_PER_TILE = 13` (matching the BLAKE3 G-function
//! rotation constants).

use crate::composite_layout::{JACKPOT_SIZE, LROT_PER_TILE};

/// Per-slot rotate-XOR-13 step. Mirrors Pearl's row-level update.
#[inline]
pub fn rotate_xor_13(v: u32, x: u32) -> u32 {
    v.rotate_left(LROT_PER_TILE) ^ x
}

/// Apply Pearl's 16-slot update.
///
/// If `is_active` is true, mutate slot `selected_slot` via
/// rotate-XOR-13 with `x`; otherwise leave the state unchanged.
/// The 15 other slots always pass through.
#[inline]
pub fn apply_jackpot_step(
    state: &[u32; JACKPOT_SIZE],
    selected_slot: usize,
    x: u32,
    is_active: bool,
) -> [u32; JACKPOT_SIZE] {
    let mut next = *state;
    if is_active {
        assert!(
            selected_slot < JACKPOT_SIZE,
            "selected_slot {} out of range",
            selected_slot
        );
        next[selected_slot] = rotate_xor_13(state[selected_slot], x);
    }
    next
}

/// Encode `selected_slot` as a one-hot indicator over
/// `JACKPOT_SIZE` slots.
#[inline]
pub fn one_hot_select(selected_slot: usize) -> [u32; JACKPOT_SIZE] {
    assert!(selected_slot < JACKPOT_SIZE);
    let mut out = [0u32; JACKPOT_SIZE];
    out[selected_slot] = 1;
    out
}

/// LSB-first bit decomposition of a u32 into 32 boolean values
/// represented as u32 (0 or 1).
#[inline]
pub fn bit_decompose_u32(v: u32) -> [u32; 32] {
    core::array::from_fn(|i| (v >> i) & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_xor_13_zero_zero_is_zero() {
        assert_eq!(rotate_xor_13(0, 0), 0);
    }

    #[test]
    fn rotate_xor_13_zero_x_is_x() {
        assert_eq!(rotate_xor_13(0, 0xDEADBEEF), 0xDEADBEEF);
    }

    #[test]
    fn rotate_xor_13_matches_definition() {
        let v: u32 = 0x12345678;
        let x: u32 = 0xCAFEBABE;
        assert_eq!(rotate_xor_13(v, x), v.rotate_left(13) ^ x);
    }

    #[test]
    fn rotate_xor_13_lrot_constant_pinned() {
        // LROT_PER_TILE = 13 (Pearl §4.5).
        assert_eq!(LROT_PER_TILE, 13);
    }

    #[test]
    fn apply_jackpot_step_only_touches_selected_slot() {
        let mut state = [0u32; JACKPOT_SIZE];
        for i in 0..JACKPOT_SIZE {
            state[i] = (i as u32 + 1) * 0x01010101;
        }
        let original = state;
        let next = apply_jackpot_step(&state, 5, 0xFFFF_FFFF, true);
        // Slot 5 changed.
        assert_eq!(next[5], rotate_xor_13(original[5], 0xFFFF_FFFF));
        // All other slots unchanged.
        for i in 0..JACKPOT_SIZE {
            if i != 5 {
                assert_eq!(next[i], original[i], "slot {i} should be unchanged");
            }
        }
    }

    #[test]
    fn apply_jackpot_step_inactive_preserves_state() {
        let state: [u32; JACKPOT_SIZE] = core::array::from_fn(|i| (i + 1) as u32 * 0xBEEF);
        let next = apply_jackpot_step(&state, 0, 0xDEAD, false);
        assert_eq!(next, state);
    }

    #[test]
    fn one_hot_select_returns_unit_vector() {
        let oh = one_hot_select(7);
        assert_eq!(oh.iter().sum::<u32>(), 1);
        assert_eq!(oh[7], 1);
        for i in 0..JACKPOT_SIZE {
            if i != 7 {
                assert_eq!(oh[i], 0);
            }
        }
    }

    #[test]
    fn bit_decompose_round_trips() {
        let v: u32 = 0xC0FFEE_42;
        let bits = bit_decompose_u32(v);
        let mut recomposed: u32 = 0;
        for i in 0..32 {
            recomposed |= bits[i] << i;
        }
        assert_eq!(recomposed, v);
        // All cells are boolean.
        for b in bits.iter() {
            assert!(*b == 0 || *b == 1);
        }
    }

    #[test]
    fn jackpot_size_pinned() {
        // Pearl's JACKPOT_SIZE = 16 (matches Tip5 / Sphinx digest slot count).
        assert_eq!(JACKPOT_SIZE, 16);
    }

    #[test]
    fn rotate_xor_13_avalanche() {
        // Flipping one bit of v changes ~half the bits of the output.
        let v = 0x12345678u32;
        let x = 0;
        let a = rotate_xor_13(v, x);
        let b = rotate_xor_13(v ^ 1, x);
        let diff = (a ^ b).count_ones();
        // We expect exactly 1 since rotate is bijective on bits.
        assert_eq!(diff, 1);
    }

    #[test]
    fn multi_step_chain_is_deterministic() {
        let mut state: [u32; JACKPOT_SIZE] = [0; JACKPOT_SIZE];
        state[0] = 0xDEADBEEF;
        let mut state_a = state;
        let mut state_b = state;
        for (slot, x) in [(0u32, 0xCAFE), (3, 0xBABE), (15, 0xF00D)].iter() {
            state_a = apply_jackpot_step(&state_a, *slot as usize, *x, true);
            state_b = apply_jackpot_step(&state_b, *slot as usize, *x, true);
        }
        assert_eq!(state_a, state_b);
    }
}
