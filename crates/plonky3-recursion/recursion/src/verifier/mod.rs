//! STARK verification within recursive circuits.

mod batch_stark;
mod errors;
mod observable;
mod quotient;
mod stark;

pub use batch_stark::{
    CircuitTablesAir, PcsVerifierParams, verify_batch_circuit, verify_p3_batch_proof_circuit,
    verify_p3_batch_proof_circuit_profile_tag_challenger_phases_for_test_only,
    verify_p3_batch_proof_circuit_profile_skip_preprocessed_transcript_for_test_only,
};
pub use errors::VerificationError;
pub use observable::ObservableCommitment;
pub use quotient::recompose_quotient_from_chunks_circuit;
pub use stark::verify_p3_uni_proof_circuit;
