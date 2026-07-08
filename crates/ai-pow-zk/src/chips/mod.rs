//! Per-chip constraint generators for the M10.1c composite AIR.
//!
//! Each submodule ports one of Pearl `zk-pow chip/*`
//! sub-modules. The chips are **not standalone AIRs** — they're
//! constraint generators invoked from [`crate::composite_full_air`]'s
//! top-level `eval`. This mirrors Pearl's `pearl_air.rs:46-89`
//! pattern (every chip exposes `eval_constraints(...)` taking a
//! `RowView` / `&mut E`).
//!
//! ## Per-chip module shape
//!
//! Each chip exposes:
//!
//!   * `pub struct <Name>Chip;` — zero-sized type, holds no state.
//!     (Chip-level config that lives across rows goes in
//!     `Phase 12`'s `CompositeChips` struct.)
//!   * `pub fn eval_constraints<AB: AirBuilder>(&self, builder:
//!     &mut AB)` — adds this chip's constraints to the builder.
//!   * `pub fn fill_row(&self, row_idx: usize, row: &mut [Val])` —
//!     trace-side helper used by `composite_trace`.
//!
//! ## Testing pattern
//!
//! Each chip ships with a tiny [`TestWrapperAir`] that drops the chip
//! into a 1- or 2-column trace and runs Plonky3's `prove` / `verify`
//! end-to-end under `CircuitConfig::TEST_PEARL`. Tampering with
//! specific cells must reject. This pattern is reusable across every
//! chip; the wrapper lives in each chip's `tests` module.
//!
//! [`TestWrapperAir`]: stark_row::tests

pub mod blake3;
pub mod control;
pub mod fold;
pub mod i8u8;
pub mod input;
pub mod jackpot;
pub mod matmul;
pub mod range_table;
pub mod stark_row;
pub mod stripe_xor;
pub mod xstep;
