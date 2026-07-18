//! Execution state and private data for Tip5 permutation operations
//! (C2.3).
//!
//! Faithful mirror of `poseidon1_perm::state`: `Tip5PermPrivateData`
//! carries the Merkle sibling digest (≤ `capacity_ext` = 6 elements)
//! copied into the capacity portion of the sponge state, and the
//! execution state keeps separate `normal` / `merkle` chain slots so
//! the in-circuit MMCS (`add_mmcs_verify`) and the sponge / challenger
//! path do not cross-contaminate (exactly like Poseidon1 D=1).

use alloc::vec::Vec;

use p3_tip5_circuit_air::Tip5CircuitRow;

/// Private data for a Tip5 permutation row.
///
/// Only consumed in Merkle mode. `sibling` holds the base-field
/// sibling digest limbs (length ≤ `capacity_ext`) the executor copies
/// into the capacity portion of the sponge state — the in-circuit
/// counterpart of native `TruncatedPermutation`'s 2-to-1 compress
/// over a 16-wide Tip5 with 5-element digests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tip5PermPrivateData<F> {
    pub sibling: Vec<F>,
}

/// Execution state for Tip5 permutation operations.
#[derive(Debug, Default)]
pub(crate) struct Tip5ExecutionState<F> {
    /// Previous permutation output for the sponge / challenger path
    /// (full-width carry incl. capacity — matches `PaddingFreeSponge`
    /// overwrite mode).
    pub last_output_normal: Option<Vec<F>>,
    /// Previous permutation output for the Merkle / MMCS path (only
    /// the rate portion is carried forward, exactly like Poseidon1's
    /// `init_chain_state` Merkle branch).
    pub last_output_merkle: Option<Vec<F>>,
    /// Circuit rows captured during execution.
    pub rows: Vec<Tip5CircuitRow<F>>,
}
