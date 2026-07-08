//! Execution state and private data for Poseidon1 permutation operations.

use alloc::vec::Vec;

use crate::ops::poseidon1_perm::trace::Poseidon1CircuitRow;

/// Private data for Poseidon1 permutation.
///
/// Only used for Merkle mode operations. `sibling` holds extension limbs copied into the
/// capacity portion of the sponge state (length ≤ `capacity_ext` for the configured perm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poseidon1PermPrivateData<F> {
    pub sibling: Vec<F>,
}

/// Execution state for Poseidon1 permutation operations.
#[derive(Debug, Default)]
pub(crate) struct Poseidon1ExecutionState<F> {
    pub last_output_normal: Option<Vec<F>>,
    pub last_output_merkle: Option<Vec<F>>,
    /// Circuit rows captured during execution.
    pub rows: Vec<Poseidon1CircuitRow<F>>,
}
