//! BLAKE3 chip for the M10.1c composite AIR.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/blake3/` — Pearl's
//! one-round-per-row BLAKE3 chip. Spreads one BLAKE3 compression
//! across 8 trace rows × ~1k cols, matching the composite AIR's
//! BLAKE3_ROUND block (`composite_layout::BLAKE3_ROUND_LEN = 1056`).
//!
//! ## Module layout (mirrors Pearl's `chip/blake3/`)
//!
//! | Pearl file | Our module | Phase | Status |
//! |---|---|---|---|
//! | `blake3_compress.rs` (scalar reference) | [`compress`] | 7 | landed |
//! | `blake3_layout.rs` (per-round columns) | [`layout`] | 7 | landed |
//! | `logic.rs` (per-row instruction types) | [`logic`] | 7 | landed |
//! | `trace.rs` (trace-side generator)       | [`trace`] | 8 | pending |
//! | `constraints.rs` (AIR-side eval)        | [`constraints`] | 8 | pending |
//! | `program.rs` (instruction compilation)  | [`program`] | 8 | pending |
//! | `blake3_air.rs` (chip struct + setup)   | [`chip`] | 8 | pending |
//!
//! ## Pearl byte-equivalence (validated)
//!
//! [`compress::blake3_compress`] produces the same output as
//! `blake3::Hasher::new_keyed(...).update(...).finalize()` for the
//! single-block keyed-root case. See the
//! `matches_blake3_crate_keyed` test in [`compress`] for the KAT.

pub mod chip;
pub mod compress;
pub mod layout;
pub mod logic;
pub mod round_air;
pub mod round_ops;

// Phase 8 modules (re-exports staged here for forward-compat):
//
// pub mod constraints;
// pub mod program;
// pub mod trace;
//
// pub use chip::Blake3Chip;
