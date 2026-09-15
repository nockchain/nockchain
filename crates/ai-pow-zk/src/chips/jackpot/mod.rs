//! Jackpot chip — 16-slot rotate-XOR-13 tile-state evolution.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/jackpot/` — Pearl's
//! per-tile state update from §4.5 of the Pearl whitepaper.
//!
//! The state is a 16-slot register; each row updates exactly one
//! slot via the rotate-XOR-13 primitive:
//!
//! ```text
//!   JACKPOT_MSG_NEW[selected_slot] = rotate_left_13(JACKPOT_MSG[selected_slot]) XOR X
//!   JACKPOT_MSG_NEW[i] = JACKPOT_MSG[i]    for all other i
//! ```
//!
//! Selection is encoded one-hot via `SLOT_SEL[0..16]` (one element
//! is 1, the rest are 0). `X` is the XOR-fold value injected this
//! row (in the composite layout this comes from CUMSUM_BUFFER).
//!
//! ## Module layout (mirrors Pearl's `chip/jackpot/`)
//!
//! | Pearl file | Our module | Phase | Status |
//! |---|---|---|---|
//! | scalar reference (`rotate_xor_13`)            | [`compute`] | 10 | landed |
//! | AIR + trace generator (`jackpot_chip.rs`)     | [`chip`] | 10 | landed |
//!
//! ## Single-slot primitive
//!
//! The single-slot rotate-XOR-13 is the elementary operation
//! Pearl threads through 16 state slots. This chip's AIR adds
//! the one-hot SLOT_SEL routing on top.

pub mod chip;
pub mod compute;
