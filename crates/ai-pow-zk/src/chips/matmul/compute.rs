//! Scalar reference for Pearl's tile-accumulator update.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of the matmul logic at
//! `Pearl zk-pow chip/matmul/compute_cumsum.rs` —
//! specifically the per-row tile dot product and the
//! reset/update/pass-through cumsum semantics.
//!
//! ## Layout
//!
//! `A` is `TILE_H` rows × `TILE_D` cols of i8; `B` is `TILE_H` rows
//! × `TILE_D` cols of i8 (Pearl uses column-major B internally; we
//! follow Pearl's row-major-in-trace convention here, with B
//! treated as 2 rows of length-16 i8 vectors).
//!
//! `CUMSUM_TILE` is a `TILE_H × TILE_H` i32 matrix, row-major,
//! holding the running dot-products for each (i, j) output cell.
//!
//! ## Per-row semantics
//!
//! Given `(is_reset, is_update)` boolean flags (mutually
//! exclusive; both off ⇒ pass-through):
//!
//! ```text
//!   for each (i, j) in TILE_H × TILE_H:
//!       dot[i][j] = sum_{d=0..TILE_D} A[i][d] * B[j][d]
//!       if is_reset:
//!           cumsum_new[i][j] = dot[i][j]
//!       elif is_update:
//!           cumsum_new[i][j] = cumsum_old[i][j] + dot[i][j]
//!       else:
//!           cumsum_new[i][j] = cumsum_old[i][j]
//! ```
//!
//! This module exposes the scalar arithmetic that the AIR's
//! constraint generator and the trace-side filler both rely on.

use crate::composite_layout::{TILE_D, TILE_H};

/// Number of cells in a `CUMSUM_TILE`: 2 × 2 = 4. Matches Pearl's
/// `pearl_columns::CUMSUM_TILE_LEN`.
pub const CUMSUM_LEN: usize = TILE_H * TILE_H;

/// Dot product of two length-`TILE_D` i8 vectors as i32.
///
/// Worst case for `TILE_D = 16` and i8 inputs in `[-128, 127]`:
/// `16 × 128 × 127 ≈ 2.6 × 10^5`, well within i32 range.
#[inline]
pub fn tile_dot(a: &[i8; TILE_D], b: &[i8; TILE_D]) -> i32 {
    let mut acc: i32 = 0;
    for d in 0..TILE_D {
        acc += (a[d] as i32) * (b[d] as i32);
    }
    acc
}

/// Compute the full TILE_H × TILE_H block of dot products for one
/// row. `a` is `TILE_H × TILE_D` row-major; `b` is `TILE_H × TILE_D`
/// row-major (where the i-th row of `b` is "column j" of the original
/// matmul). Returns the dot products as a `CUMSUM_LEN`-element vector
/// indexed `i * TILE_H + j`.
#[inline]
pub fn tile_dot_block(a: &[[i8; TILE_D]; TILE_H], b: &[[i8; TILE_D]; TILE_H]) -> [i32; CUMSUM_LEN] {
    let mut out = [0i32; CUMSUM_LEN];
    for i in 0..TILE_H {
        for j in 0..TILE_H {
            out[i * TILE_H + j] = tile_dot(&a[i], &b[j]);
        }
    }
    out
}

/// Apply the cumsum update for one row.
///
/// Returns `cumsum_new` per Pearl's reset/update/pass-through
/// semantics. The caller is responsible for ensuring `is_reset`
/// and `is_update` are mutually exclusive (the AIR's constraint
/// allows them to violate this only at the cost of accepting
/// arbitrary `cumsum_new` values — the chip alone doesn't prevent
/// the violation; the boolean + exclusive selectors are pinned by
/// the control chip + CONTROL_PREP unpacking).
#[inline]
pub fn apply_cumsum_update(
    cumsum_old: &[i32; CUMSUM_LEN],
    dot: &[i32; CUMSUM_LEN],
    is_reset: bool,
    is_update: bool,
) -> [i32; CUMSUM_LEN] {
    let mut out = [0i32; CUMSUM_LEN];
    for k in 0..CUMSUM_LEN {
        out[k] = if is_reset {
            dot[k]
        } else if is_update {
            cumsum_old[k].wrapping_add(dot[k])
        } else {
            cumsum_old[k]
        };
    }
    out
}

/// Convenience: one-call full row computation. Returns
/// `cumsum_new`.
#[inline]
pub fn compute_row(
    a: &[[i8; TILE_D]; TILE_H],
    b: &[[i8; TILE_D]; TILE_H],
    cumsum_old: &[i32; CUMSUM_LEN],
    is_reset: bool,
    is_update: bool,
) -> [i32; CUMSUM_LEN] {
    let dot = tile_dot_block(a, b);
    apply_cumsum_update(cumsum_old, &dot, is_reset, is_update)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_dot_simple() {
        let a: [i8; TILE_D] = core::array::from_fn(|i| (i + 1) as i8);
        let b: [i8; TILE_D] = core::array::from_fn(|i| (i + 1) as i8);
        // 1^2 + 2^2 + ... + 16^2 = 16*17*33/6 = 1496.
        assert_eq!(tile_dot(&a, &b), 1496);
    }

    #[test]
    fn tile_dot_zero_when_either_zero() {
        let a: [i8; TILE_D] = [0; TILE_D];
        let b: [i8; TILE_D] = core::array::from_fn(|i| (i + 1) as i8);
        assert_eq!(tile_dot(&a, &b), 0);
        assert_eq!(tile_dot(&b, &a), 0);
    }

    #[test]
    fn tile_dot_signs() {
        let a: [i8; TILE_D] = core::array::from_fn(|i| if i % 2 == 0 { 1 } else { -1 });
        let b: [i8; TILE_D] = [1; TILE_D];
        // 8 × +1 + 8 × −1 = 0.
        assert_eq!(tile_dot(&a, &b), 0);
    }

    #[test]
    fn tile_dot_extreme_values() {
        let a: [i8; TILE_D] = [127; TILE_D];
        let b: [i8; TILE_D] = [127; TILE_D];
        // 16 × 127 × 127 = 258064.
        assert_eq!(tile_dot(&a, &b), 16 * 127 * 127);
    }

    #[test]
    fn tile_dot_block_indexing() {
        // a[0] = all 1s, a[1] = all 2s. b[0] = all 1s, b[1] = all 3s.
        // Expected dot[i][j]:
        //   (0,0) = 16 × 1 × 1 = 16
        //   (0,1) = 16 × 1 × 3 = 48
        //   (1,0) = 16 × 2 × 1 = 32
        //   (1,1) = 16 × 2 × 3 = 96
        let mut a = [[0i8; TILE_D]; TILE_H];
        let mut b = [[0i8; TILE_D]; TILE_H];
        for d in 0..TILE_D {
            a[0][d] = 1;
            a[1][d] = 2;
            b[0][d] = 1;
            b[1][d] = 3;
        }
        let out = tile_dot_block(&a, &b);
        assert_eq!(out, [16, 48, 32, 96]);
    }

    #[test]
    fn apply_cumsum_reset_overrides() {
        let old: [i32; CUMSUM_LEN] = [1000, 2000, 3000, 4000];
        let dot: [i32; CUMSUM_LEN] = [10, 20, 30, 40];
        let out = apply_cumsum_update(&old, &dot, /*reset*/ true, /*update*/ false);
        assert_eq!(out, dot);
    }

    #[test]
    fn apply_cumsum_update_accumulates() {
        let old: [i32; CUMSUM_LEN] = [1000, 2000, 3000, 4000];
        let dot: [i32; CUMSUM_LEN] = [10, 20, 30, 40];
        let out = apply_cumsum_update(&old, &dot, /*reset*/ false, /*update*/ true);
        assert_eq!(out, [1010, 2020, 3030, 4040]);
    }

    #[test]
    fn apply_cumsum_passthrough_when_both_off() {
        let old: [i32; CUMSUM_LEN] = [1000, 2000, 3000, 4000];
        let dot: [i32; CUMSUM_LEN] = [10, 20, 30, 40];
        let out = apply_cumsum_update(&old, &dot, /*reset*/ false, /*update*/ false);
        assert_eq!(out, old);
    }

    #[test]
    fn compute_row_end_to_end_reset_then_update() {
        let mut a = [[0i8; TILE_D]; TILE_H];
        let mut b = [[0i8; TILE_D]; TILE_H];
        for d in 0..TILE_D {
            a[0][d] = (d as i8 + 1) % 4;
            a[1][d] = (d as i8 + 1) % 5;
            b[0][d] = ((d as i8 + 2) % 6) - 3;
            b[1][d] = ((d as i8 + 3) % 7) - 3;
        }
        // First row: reset.
        let zero: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let after_reset = compute_row(&a, &b, &zero, true, false);
        // Second row: update with same (a, b).
        let after_update = compute_row(&a, &b, &after_reset, false, true);
        // After update, each cell should be exactly 2× the dot product.
        let dot = tile_dot_block(&a, &b);
        for k in 0..CUMSUM_LEN {
            assert_eq!(after_update[k], dot[k].wrapping_mul(2));
        }
    }

    #[test]
    fn cumsum_len_matches_tile_h_squared() {
        assert_eq!(CUMSUM_LEN, TILE_H * TILE_H);
        // Pinned at 4 — TILE_H = 2.
        assert_eq!(CUMSUM_LEN, 4);
    }
}
