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
//! - [`recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof`]
//!   — selected compact final-layer batch-STARK route. It proves the
//!   statement-bound L1 proof inside a compact L2 proof, carries only the final
//!   compact body plus an explicit verifier-key digest, and verifies against
//!   verifier-owned metadata/setup and public values.
//! - [`recursion::prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache`]
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
//! ## Status
//!
//! M10.1c is the Layer-0 composite pipeline. The production recursive target is
//! a recursive certificate, not the raw Layer-0 proof and not the oversized
//! batch-STARK L1 checkpoint. The selected direction is compact final-layer
//! batch-STARK L2; it still needs Hoon verifier wiring and a production source
//! for the verifier-key/setup digest before consensus readiness is claimed.
//! Earlier M9.1 / M10.1b prototypes were retired once M10.1c had full LogUp +
//! PI binding + bench data.
//! See `docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md` for the
//! current production pipeline.
//!
//! ## What's not yet bound
//!
//! - **Full-matmul recursive statement.** The current recursive certificate
//!   verifies one selected tile. Production block admission therefore remains
//!   closed until the recursive statement binds a full-matmul aggregate or
//!   equivalent full-work certificate.
//! - **Hoon noun verifier hook.** The structured recursive-certificate noun
//!   encoder exists in the miner crate, but consensus still needs the jet /
//!   wiring that decodes the noun at the block boundary, reconstructs the
//!   verifier-derived statement from block data, and calls the Rust verifier.
//!   Until that hook is wired, the Hoon/kernel path remains fail-closed.

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
