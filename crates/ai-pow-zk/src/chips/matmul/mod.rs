//! Matmul chip for the M10.1c composite AIR.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/matmul/` — the chip that
//! enforces the **inner accumulation step** of Pearl's tiled INT8
//! `(A + E) · (B + F)` matrix multiplication. Each row consumes a
//! TILE_H × TILE_D × TILE_H tile-block and updates a TILE_H × TILE_H
//! cumulative-sum accumulator.
//!
//! ## Module layout (mirrors Pearl's `chip/matmul/`)
//!
//! | Pearl file | Our module | Phase | Status |
//! |---|---|---|---|
//! | scalar reference (`tile dot product`)     | [`compute`] | 9a | landed |
//! | trace + AIR (`compute_cumsum.rs`)         | [`chip`] | 9b | in progress |
//!
//! ## Per-row contract
//!
//! Each row carries:
//!   * `A_NOISED_UNPACK`: 32 i8 cells = TILE_H × TILE_D (2 × 16).
//!   * `B_NOISED_UNPACK`: 32 i8 cells = TILE_H × TILE_D.
//!   * `CUMSUM_TILE`: 4 i32 cells = TILE_H × TILE_H (2 × 2).
//!   * `IS_RESET_CUMSUM`, `IS_UPDATE_CUMSUM` (selectors).
//!
//! Cross-row contract: `CUMSUM_TILE_NEW = func(IS_RESET, IS_UPDATE,
//! CUMSUM_TILE_OLD, dot_product(A_NOISED, B_NOISED))`.

pub mod chip;
pub mod compute;
