//! Plonky3 SNARK circuit for the `ai-pow` tiling matmul puzzle.
//!
//! Mirrors Pearl's `zk-pow` role at the proof-certificate layer: where Pearl
//! uses a compact proof to certify the opened useful-work statement, this
//! crate uses Plonky3 over Goldilocks + Tip5 + FRI plus recursion to build
//! Nockchain's AI-PoW proof stack. The selected production recursive-proof
//! direction is the compact final-layer batch-STARK route over a fast
//! statement-bound L1 proof. The older full batch-STARK checkpoint is retained
//! only as a regression/diagnostic path because it is too large for the
//! production wire budget. Native terminal compression experiments were removed
//! from the AI-PoW API after the compact route hit the relaxed target. The
//! plain `MatmulProof` remains a miner-side diagnostic/pre-ZKP target-hit
//! check, not the persisted block artifact.
//!
//! ## Attempt Reuse Boundary
//!
//! Production AI-PoW is intentionally minimal-reuse across nonce attempts.
//! Changing the opaque AI-PoW nonce must force fresh transcript-derived
//! commitments, noise, noised matrix strips, tile states, jackpot preimages,
//! and proof witness data. Cache-friendly attempt reuse is a vulnerability,
//! not a desired trait or optimization target.
//!
//! ## Public API
//!
//! Production callers normally use `ai_pow::zk_bridge`, not this crate
//! directly. Lower-level callers that already performed chain statement
//! verification should use:
//!
//! - [`recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_sx`]
//!   — selected compact final-layer batch-STARK route (the `_sx` stripe-major
//!   variant is the one the production bridge calls). It proves the
//!   statement-bound L1 proof inside a compact L2 proof, carries only the final
//!   compact body plus an explicit verifier-key digest, and verifies against
//!   verifier-owned metadata/setup and public values.
//! - [`recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache_sx`]
//!   — same proof path with reusable prover-side setup.
//! - [`recursion::verify_compact_batch_recursive_certificate_with_context`]
//!   — verifier entrypoint for compact certificates. The context and expected
//!   verifier-key digest must come from verifier-owned configuration.
//!
//! Layer-0 APIs remain public for bridge internals and tests:
//!
//! - [`composite_proof::composite_prove_pinned_logup`] /
//!   [`composite_proof::composite_verify_pinned_logup`] — Layer-0
//!   composite STARK primitives. These are intermediate inputs to the
//!   recursive certificate and are not persisted proof artifacts by
//!   themselves.
//! - [`composite_proof::composite_prove`] / [`composite_proof::composite_verify`]
//!   — dev-only unpinned prove + verify pair, wrapping `p3-uni-stark`.
//! - [`composite_full_air::CompositeFullAir`] — the top-level AIR over
//!   `TOTAL_TRACE_WIDTH` columns. 10 chips share every row, gated by
//!   selector bits packed into `CONTROL_PREP`.
//! - [`composite_full_air_with_lookups::CompositeFullAirWithLookups`] —
//!   same AIR with 7 LogUp buses reified via `p3-batch-stark`. Used
//!   when full cross-chip soundness is required.
//! - [`composite_trace::CompositeTrace`] — trace generator with
//!   `place_*` helpers for each instruction type (matmul step, BLAKE3
//!   hash, jackpot rotation).
//! - [`composite_public::CompositePublicInputs`] — typed 60-element PI
//!   vector: cumsum, jackpot state, matrix commitments, nonce-bound job key,
//!   jackpot key, and jackpot hash.
//! - [`params::ZkParams`] — circuit parameter shape (rows / cols of the
//!   matmul puzzle).
//! - [`circuit::AiPowStarkConfig`] / [`circuit::CircuitConfig`] — the
//!   STARK config factory (Goldilocks + Tip5 + FRI parameters per
//!   profile).
//!
//! ## Architectural note
//!
//! `ai-pow-zk` is intentionally **standalone** — it does **not** depend
//! on `ai-pow`. The proving crate (`ai-pow`) is the consumer; making
//! `ai-pow-zk` depend back on it would introduce a circular workspace
//! dep. The caller in `ai-pow` constructs [`ZkParams`] and the trace +
//! PIs from its own types at the call site.
//!
//! ## Consensus binding
//!
//! The production artifact is the compact final-layer batch-STARK certificate.
//! Consensus reaches it through the mandatory `ai-pow-verify` jet, which
//! decodes a bounded noun, derives the public statement and canonical opened
//! schedule from the candidate, selects verifier-owned setup by the recomputed
//! trace height, and verifies the compact certificate plus its target-bound
//! jackpot.
//!
//! The proven useful-work statement is Pearl's selected ticket tile, including
//! commitment-keyed noise, matmul accumulation, tile-state fold, and jackpot.
//! It does not claim a raw Layer-0 proof or the oversized L1 checkpoint as a
//! block certificate. The P0/D6 statement-digest fold binds the canonical
//! Layer-0 program; setup and verifier-key digests come from the node rather
//! than the prover.

// Test-only lint relaxations: `unwrap()` is fine in tests (repo convention), and the
// AIR/trace negative tests address cells by the explicit `row * WIDTH + col` index
// idiom (`0 * WIDTH + col` for row 0), which trips erasing_op/identity_op.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::erasing_op,
        clippy::identity_op,
        // Layout/offset tests deliberately assert relationships between const
        // column offsets to document + guard the trace schedule.
        clippy::assertions_on_constants
    )
)]

pub mod blake3_tree;
pub mod canonical;
pub mod chips;
pub mod circuit;
pub mod composite_full_air;
pub mod composite_full_air_with_lookups;
pub mod composite_layout;
pub mod composite_lookup_proof;
pub mod composite_lookups;
pub mod composite_preprocess;
pub mod composite_proof;
pub mod composite_public;
pub mod composite_trace;
pub mod moe_ref;
pub mod noise_ref;
pub mod params;

/// §recursion — integration of the composite proof with the vendored
/// `Plonky3-recursion` substrate. Opt-in (`--features recursion`); the
/// default build pulls no recursion crates.
#[cfg(feature = "recursion")]
pub mod recursion;

pub use p3_goldilocks::Goldilocks as Val;

pub use crate::circuit::{AiPowStarkConfig, CircuitConfig};
pub use crate::composite_full_air::{
    extract_program, CompositeFullAir, CompositeFullAirPinned, ProgramShapeError,
};
pub use crate::composite_full_air_with_lookups::{
    CompositeFullAirWithLookups, CompositeFullAirWithLookupsPinned,
};
#[cfg(any(test, feature = "dev-unsafe"))]
pub use crate::composite_proof::{
    composite_prove as dev_unpinned_prove, composite_prove_pinned as dev_pinned_no_logup_prove,
    composite_setup as dev_pinned_no_logup_setup, composite_verify as dev_unpinned_verify,
    composite_verify_pinned as dev_pinned_no_logup_verify,
    composite_verify_pow as dev_unpinned_verify_pow,
    composite_verify_pow_pinned as dev_pinned_no_logup_verify_pow,
};
pub use crate::composite_proof::{
    hash_jackpot_le_bytes, CompositeVerificationError, PowVerifyError, StarkVerificationError,
};
pub use crate::composite_public::CompositePublicInputs;
pub use crate::composite_trace::CompositeTrace;
pub use crate::params::ZkParams;

/// Concrete pinned+LogUp Layer-0 proof type.
///
/// This is an intermediate proof consumed by the recursive certificate
/// prover. It is not the canonical Nockchain recursive certificate and
/// must not be persisted in blocks or transmitted as the AI-PoW proof
/// artifact.
pub type AiPowBatchProof = p3_batch_stark::BatchProof<AiPowStarkConfig>;

/// Concrete preprocessed program matrix type used by the pinned AI PoW circuit.
pub type AiPowProgram = p3_matrix::dense::RowMajorMatrix<p3_uni_stark::Val<AiPowStarkConfig>>;
