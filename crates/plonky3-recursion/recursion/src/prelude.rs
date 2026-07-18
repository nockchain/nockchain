//! Prelude module for common imports.
//!
//! This module re-exports the most commonly used items from the crate,
//! allowing users to write:
//!
//! ```ignore
//! use p3_recursion::prelude::*;
//! ```
//!
//! Instead of importing each item individually.

pub use crate::Target;
pub use crate::challenger::CircuitChallenger;
pub use crate::generation::{GenerationError, PcsGeneration};
pub use crate::pcs::fri::FriVerifierParams;
pub use crate::public_inputs::{
    CommitmentOpening, FriVerifierInputs, PublicInputBuilder, StarkVerifierInputs,
    StarkVerifierInputsBuilder,
};
pub use crate::traits::{
    ComsWithOpeningsTargets, Recursive, RecursiveAir, RecursiveChallenger, RecursiveExtensionMmcs,
    RecursiveMmcs, RecursivePcs,
};
pub use crate::types::{
    CommitmentTargets, OpenedValuesTargets, ProofTargets, RecursiveLagrangeSelectors,
    StarkChallenges,
};
pub use crate::verifier::{ObservableCommitment, VerificationError, verify_p3_uni_proof_circuit};
