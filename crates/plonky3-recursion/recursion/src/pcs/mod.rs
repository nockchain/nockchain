//! Polynomial Commitment Scheme (PCS) implementations for recursive verification.

pub mod fri;
pub mod mmcs;

pub use fri::{
    BatchOpeningTargets, CommitPhaseProofStepTargets, FriProofTargets, FriVerifierParams,
    HashProofTargets, HidingFriProofTargets, HidingHashProofTargets, HidingOpenedValuesTargets,
    InputProofTargets, MerkleCapTargets, MmcsProofTargets, QueryProofTargets, RecExtensionValMmcs,
    RecValHidingMmcs, RecValMmcs, TwoAdicFriProofTargets, Witness, verify_fri_circuit,
};
pub use mmcs::{
    convert_merkle_proof_to_siblings, set_fri_mmcs_private_data, set_hiding_fri_mmcs_private_data,
    set_hiding_salted_fri_mmcs_private_data, set_salted_fri_mmcs_private_data,
    verify_batch_circuit, verify_batch_circuit_from_extension_opened,
};
