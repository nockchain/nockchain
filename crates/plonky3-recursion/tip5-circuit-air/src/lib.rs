//! # `tip5-circuit-air` — C2 / M-S4
//!
//! A **from-scratch** AIR for recursive proving's **5-round Tip5**
//! permutation, built from the native spec and KAT-anchored.
//!
//! * **Construction reference:** the authoritative Tip5 paper (IACR
//!   ePrint 2023/107).
//! * **Bit-for-bit soundness oracle:**
//!   `nockchain_math::tip5::permute_5round`, frozen into the committed
//!   golden KAT fixture
//!   `crates/ai-pow-zk/tests/fixtures/tip5_5round_golden_kat.txt`.
//!
//! The split-and-lookup S-box is arithmetized **algebraically and
//! lookup-free**: per byte, `c = ((b+1)^3 − 1) mod 257`, which the C2.0
//! theorem (`LOOKUP_TABLE[b] == ((b+1)^3−1) mod 257`, machine-proven)
//! makes *exactly equivalent* to the native table — so no LogUp /
//! multiset argument is needed for permutation correctness, and the
//! whole permutation is a single self-contained AIR provable with
//! plain `p3_uni_stark::{prove,verify}`. The only residual soundness
//! obligation — that the 8-byte split of each split-lane input is the
//! unique *canonical* one (paper §4.6) — is enforced by an
//! inverse-or-zero `< p` guard and adversarially tested.
//!
//! See `crates/ai-pow-zk/docs/2026-05-18_C2_TIP5_CIRCUIT_AIR_DESIGN.md` for the full
//! design and soundness argument.

#![no_std]

extern crate alloc;

mod air;
mod air_circuit;
mod air_lookup;
mod generation;
mod generation_lookup;
mod perm;
mod tip5_spec;

pub use air::{Tip5PermAir, tip5_perm_air_width};
pub use air_circuit::{
    TIP5_CIRCUIT_EXTRA_MAIN_COLS, TIP5_CIRCUIT_PREP_WIDTH, TIP5_CTL_PREP_COLS, TIP5_DIGEST,
    TIP5_RATE, TIP5_WIDTH, Tip5CircuitAir, Tip5CircuitRow,
    build_tip5_circuit_main_with_mmcs_bits, build_tip5_circuit_preprocessed,
    generate_tip5_circuit_main, tip5_inputs_from_rows,
};
pub use air_lookup::{TABLE_ROWS, Tip5PermLookupAir, tip5_lookup_air_width};
pub use generation::generate_trace_rows;
pub use generation_lookup::generate_lookup_trace;
pub use perm::Tip5Perm;
pub use tip5_spec::{
    LOOKUP_TABLE, MDS_FIRST_ROW, NUM_ROUNDS, NUM_SPLIT_AND_LOOKUP, P_GOLDILOCKS, ROUND_CONSTANTS,
    STATE_SIZE, mds_matrix, permute, rc_precomp,
};
