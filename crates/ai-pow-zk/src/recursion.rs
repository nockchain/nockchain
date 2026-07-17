//! §recursion — integrate the ai-pow-zk composite proof with the
//! vendored `Plonky3-recursion` substrate.
//!
//! Feature-gated behind `recursion`. This module is the *caller* side
//! of a generic API: `p3_recursion`'s verifier entrypoints are generic
//! over the inner AIR, and here they are instantiated with the
//! concrete `CompositeFullAirWithLookupsPinned` + `AiPowStarkConfig`.
//! The recursion substrate stays application-agnostic.
//!
//! Staging:
//! - S2 — cross-workspace build path established.
//! - S3a — composite AIR confirmed `RecursiveAir`-conformant.
//! - S3b/c — `build_composite_l1_verifier_circuit`: the composite's
//!   batch-STARK proof is verified in-circuit by `verify_batch_circuit`.
//!   The composite is a single LogUp AIR proven by `p3_batch_stark`, so
//!   it routes through the lookup-aware batch entrypoint with the
//!   composite AIR as the single generic `A` (the de-risk's path 3a).
//!
//! ## Recommended entrypoints
//!
//! The production bridge should enter this module only after it has verified
//! the Layer-0 statement against chain-owned data and constructed a
//! [`ChainVerifiedCompositeProof`]. From there, use:
//!
//! - [`prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof`]
//! - [`prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache`]
//! - [`verify_compact_batch_recursive_certificate_with_context`]
//! - [`encode_compact_batch_recursive_certificate`]
//! - [`decode_compact_batch_recursive_certificate`]
//!
//! The non-compact L1 checkpoint functions are hidden from normal rustdoc.
//! They remain available to bridge internals and regression tests, but are too
//! large for the selected production wire artifact.

use p3_batch_stark::{BatchProof, CommonData};
use p3_circuit::ops::{
    generate_recompose_trace, generate_tip5_trace, NpoTypeId, Tip5Config, Tip5Goldilocks,
};
use p3_circuit::{CircuitBuilder, NonPrimitiveOpId};
use p3_field::{BasedVectorSpace, PrimeCharacteristicRing, PrimeField64};
use p3_lookup::logup::LogUpGadget;
use p3_recursion::pcs::fri::{
    FriProofTargets, FriVerifierParams, InputProofTargets, MerkleCapTargets, RecExtensionValMmcs,
    RecValMmcs, Witness,
};
use p3_recursion::pcs::set_fri_mmcs_private_data;
use p3_recursion::public_inputs::BatchStarkVerifierInputsBuilder;
use p3_recursion::{verify_batch_circuit, ObservableCommitment, RecursiveAir, VerificationError};
use p3_symmetric::Permutation;
use p3_tip5_circuit_air::Tip5Perm as RecTip5Perm;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::circuit::{Challenge, Tip5Compress, Tip5Sponge};
use crate::{AiPowStarkConfig, CompositeFullAirWithLookupsPinned, Val};

/// Outer circuit-prover proof produced after recursively verifying Layer 0.
/// Public because the boot-setup table caches `(AiPowL1OuterProof, metadata)` per
/// bucket and rebuilds the (large) verifier context at boot via
/// [`rebuild_compact_verifier_context`] — the small-cache fast-boot path.
pub type AiPowL1OuterProof =
    p3_circuit_prover::BatchStarkProof<p3_circuit_prover::config::GoldilocksTipsConfig>;

/// Native final-layer L2 proof over the L1 verifier circuit.
type AiPowL2FinalProof =
    p3_circuit_prover::BatchStarkProof<p3_circuit_prover::config::GoldilocksBlake3Config>;

/// Tip5 digest identifying the verifier-owned compact batch recursive setup.
///
/// This is carried by the compact certificate only as a route/setup selector.
/// The verifier recomputes it from trusted metadata and setup-derived FRI
/// shape before accepting the compact body.
pub type AiPowCompactBatchVerifierKeyDigest = [Val; DIGEST_ELEMS];

/// Canonical recursive certificate for Nockchain's AI proof-of-work puzzle
/// statement.
///
/// The outer proof alone is not a production certificate: its verifier would
/// otherwise trust proof-carried circuit metadata. The canonical certificate
/// carries the Layer-0 proof and pinned program so verification can rebuild the
/// exact L1 verifier circuit, run that verifier against the embedded Layer-0
/// proof, reject outer proof metadata that does not match the rebuilt canonical
/// circuit shape, and cryptographically verify the submitted outer proof body.
///
/// Consensus code must still derive and check the statement metadata
/// externally before accepting this certificate.
#[doc(hidden)]
#[derive(Serialize, Deserialize)]
pub struct AiPowRecursiveCertificate {
    /// Layer-0 pinned LogUp proof recursively verified by the L1 circuit.
    l0_proof: BatchProof<AiPowStarkConfig>,
    /// Canonical pinned Layer-0 program used to rebuild the L1 verifier
    /// circuit and its expected outer proof binding.
    l0_program: crate::AiPowProgram,
    /// Outer D=2 circuit-prover proof of the L1 verifier circuit execution.
    l1_outer_proof: AiPowL1OuterProof,
}

impl AiPowRecursiveCertificate {
    /// Construct the batch-STARK recursive checkpoint certificate from
    /// chain-verified Layer-0 proof parts and the corresponding L1 outer proof.
    fn new(
        l0_proof: BatchProof<AiPowStarkConfig>,
        l0_program: crate::AiPowProgram,
        l1_outer_proof: AiPowL1OuterProof,
    ) -> Self {
        Self {
            l0_proof,
            l0_program,
            l1_outer_proof,
        }
    }

    /// The outer proof, exposed for diagnostics and size accounting only.
    ///
    /// Checkpoint verification must call [`verify_recursive_certificate`], which
    /// rebuilds and runs the canonical L1 verifier circuit, checks this proof's
    /// stable circuit metadata, and verifies the submitted proof body.
    pub fn l1_outer_proof(&self) -> &AiPowL1OuterProof {
        &self.l1_outer_proof
    }

    /// The embedded Layer-0 proof, exposed for diagnostics and size accounting
    /// only.
    ///
    /// Checkpoint verification must call [`verify_recursive_certificate`], which
    /// verifies this proof inside the rebuilt L1 verifier circuit.
    pub fn l0_proof(&self) -> &BatchProof<AiPowStarkConfig> {
        &self.l0_proof
    }

    /// **Opened-statement binding.** Returns `true` iff the certificate's embedded
    /// Layer-0 program equals `expected`.
    ///
    /// [`verify_recursive_certificate`] proves the Layer-0 statement for *this*
    /// certificate's `l0_program` but does **not** bind that program to any public
    /// statement — a malicious prover can embed a program that opened a
    /// prover-favorable strip. A sound node MUST recompute the canonical program
    /// from the *public* opened schedule
    /// ([`crate::canonical::canonical_program_for_strip_schedule`]) and require
    /// this to return `true`, so the certificate is proven over exactly the
    /// claimed opened rows/columns. Compares the preprocessed program matrix
    /// (shape + every cell), which pins the entire strip schedule + selectors.
    pub fn l0_program_matches(&self, expected: &crate::AiPowProgram) -> bool {
        self.l0_program.width == expected.width && self.l0_program.values == expected.values
    }
}

/// Compact final-layer batch-STARK recursive proof candidate for AI-PoW.
///
/// This is the production-candidate wire body for the committed compact L2
/// route. It carries a small verifier-key/setup digest plus the final L2
/// compact proof body. The verifier must provide a verifier-owned
/// [`AiPowCompactBatchVerifierContext`] for the canonical setup, metadata, FRI
/// shape, and public-value binding. Accepting those values from the prover
/// would make this object unsound.
#[derive(Serialize, Deserialize)]
pub struct AiPowCompactBatchRecursiveCertificate {
    verifier_key_digest: AiPowCompactBatchVerifierKeyDigest,
    l2_compact_body: p3_circuit_prover::GoldilocksBlake3PathPrunedCompactBatchStarkProofBody,
}

impl AiPowCompactBatchRecursiveCertificate {
    fn new(
        verifier_key_digest: AiPowCompactBatchVerifierKeyDigest,
        l2_compact_body: p3_circuit_prover::GoldilocksBlake3PathPrunedCompactBatchStarkProofBody,
    ) -> Self {
        Self {
            verifier_key_digest,
            l2_compact_body,
        }
    }

    pub const fn verifier_key_digest(&self) -> &AiPowCompactBatchVerifierKeyDigest {
        &self.verifier_key_digest
    }

    pub fn l2_compact_body(
        &self,
    ) -> &p3_circuit_prover::GoldilocksBlake3PathPrunedCompactBatchStarkProofBody {
        &self.l2_compact_body
    }
}

/// Verifier-owned setup for the compact final-layer batch-STARK route.
///
/// This context is not serialized with [`AiPowCompactBatchRecursiveCertificate`].
/// Production must derive or pin it from trusted code/config/verifier-key state.
/// The compact certificate verifier treats all fields here as verifier-owned
/// and binds statement-specific public values separately.
#[derive(Serialize, Deserialize)]
pub struct AiPowCompactBatchVerifierContext {
    verifier_key_digest: AiPowCompactBatchVerifierKeyDigest,
    metadata: p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata,
    // The circuit prover data serializes in its VERIFIER-ONLY projection
    // (CommonData + preprocessed columns; prover-only LDEs reconstructed empty) —
    // see `p3_circuit_prover::CircuitProverData`'s serde. The `Arc` is
    // transparently (de)serialized via its inner value (no serde `rc` feature).
    #[serde(with = "serde_arc_circuit_prover_data")]
    circuit_prover_data: std::sync::Arc<
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksBlake3Config>,
    >,
    fri_shape: p3_circuit_prover::GoldilocksBlake3FriShape,
}

/// (De)serialize `Arc<CircuitProverData>` via its inner value (serde `rc` feature
/// is not enabled; the setup table is deterministic so structural sharing across
/// entries is not needed).
mod serde_arc_circuit_prover_data {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    type Cpd =
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksBlake3Config>;

    pub(super) fn serialize<S>(value: &Arc<Cpd>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_ref().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Arc<Cpd>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Arc::new(Cpd::deserialize(deserializer)?))
    }
}

impl AiPowCompactBatchVerifierContext {
    pub const fn verifier_key_digest(&self) -> &AiPowCompactBatchVerifierKeyDigest {
        &self.verifier_key_digest
    }

    /// The (small) proof metadata — cache this alongside the L1 outer proof to
    /// rebuild the context at boot via [`rebuild_compact_verifier_context`].
    pub const fn metadata(&self) -> &p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata {
        &self.metadata
    }

    /// An OWNED copy of the proof metadata, obtained by a serde round-trip (the
    /// metadata's `stark_common` is not `Clone`). This is exactly the projection
    /// the boot-setup cache round-trips (the dropped `stark_common.lookups` are not
    /// needed by the rebuild — see [`rebuild_compact_verifier_context`]), so it is
    /// the correct owned metadata to place in an [`AiPowCompactVerifierSetupSeed`].
    pub fn metadata_owned(
        &self,
    ) -> Result<p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata, VerificationError> {
        let bytes = bincode::serde::encode_to_vec(&self.metadata, bincode::config::standard())
            .map_err(|e| {
                VerificationError::InvalidProofShape(format!("metadata serialize: {e:?}"))
            })?;
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
            .map(|(m, _)| m)
            .map_err(|e| {
                VerificationError::InvalidProofShape(format!("metadata deserialize: {e:?}"))
            })
    }
}

/// Tip5 digest width (`DIGEST_ELEMS`), sponge `WIDTH`, sponge `RATE` —
/// the ai-pow-zk Layer-0 MMCS parameters (`circuit.rs`).
const DIGEST_ELEMS: usize = 5;
const WIDTH: usize = 16;
const RATE: usize = 10;
const GOLDILOCKS_MODULUS: u64 = 0xffff_ffff_0000_0001;

pub const AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES: usize = DIGEST_ELEMS * 8;

#[derive(Debug, Error)]
pub enum CompactBatchVerifierKeyDigestEncodingError {
    #[error("compact batch verifier-key digest has {actual} bytes, expected {expected}")]
    InvalidLength { expected: usize, actual: usize },
    #[error("compact batch verifier-key digest limb {index} is not canonical Goldilocks: {value}")]
    NonCanonicalLimb { index: usize, value: u64 },
}

/// Canonical byte encoding for production verifier-key/setup digest config.
///
/// The digest is five Goldilocks elements, encoded as canonical little-endian
/// `u64` limbs. This is deliberately separate from postcard certificate
/// encoding so verifier configuration can pin a stable 40-byte value without
/// depending on Rust field-element construction at call sites.
pub fn compact_batch_verifier_key_digest_to_bytes(
    digest: &AiPowCompactBatchVerifierKeyDigest,
) -> [u8; AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES] {
    let mut out = [0u8; AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES];
    for (i, limb) in digest.iter().enumerate() {
        out[i * 8..(i + 1) * 8].copy_from_slice(&limb.as_canonical_u64().to_le_bytes());
    }
    out
}

/// Decode the canonical byte form produced by
/// [`compact_batch_verifier_key_digest_to_bytes`].
pub fn compact_batch_verifier_key_digest_from_bytes(
    bytes: &[u8],
) -> Result<AiPowCompactBatchVerifierKeyDigest, CompactBatchVerifierKeyDigestEncodingError> {
    if bytes.len() != AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES {
        return Err(CompactBatchVerifierKeyDigestEncodingError::InvalidLength {
            expected: AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES,
            actual: bytes.len(),
        });
    }
    let mut digest = [Val::ZERO; DIGEST_ELEMS];
    for (i, chunk) in bytes.chunks_exact(8).enumerate() {
        let limb = u64::from_le_bytes(chunk.try_into().expect("chunk width checked"));
        if limb >= GOLDILOCKS_MODULUS {
            return Err(
                CompactBatchVerifierKeyDigestEncodingError::NonCanonicalLimb {
                    index: i,
                    value: limb,
                },
            );
        }
        digest[i] = Val::from_u64(limb);
    }
    Ok(digest)
}

pub const COMPACT_BATCH_L1_LOG_BLOWUP: usize = 3;
pub const COMPACT_BATCH_L1_NUM_QUERIES: usize = 20;
pub const COMPACT_BATCH_L1_CAP_HEIGHT: usize = 4;
pub const COMPACT_BATCH_L1_LOG_FINAL_POLY_LEN: usize = 2;
pub const COMPACT_BATCH_L1_ALU_LANES: usize = 4;
pub const COMPACT_BATCH_L1_HORNER_PACK_K: usize = 5;
pub const COMPACT_BATCH_L2_LOG_BLOWUP: usize = 5;
pub const COMPACT_BATCH_L2_NUM_QUERIES: usize = 12;
pub const COMPACT_BATCH_L2_CAP_HEIGHT: usize = 4;
pub const COMPACT_BATCH_L2_LOG_FINAL_POLY_LEN: usize = 2;
pub const COMPACT_BATCH_L2_MAX_LOG_ARITY: usize = 3;
pub const COMPACT_BATCH_L2_ALU_LANES: usize = 8;
pub const COMPACT_BATCH_L2_HORNER_PACK_K: usize = 5;
pub const COMPACT_BATCH_L2_RECOMPOSE_LANES: usize = 2;

fn production_l1_table_packing(public_binding_lanes: usize) -> p3_circuit_prover::TablePacking {
    p3_circuit_prover::TablePacking::new(DIGEST_ELEMS, 8)
        .with_public_binding_lanes(public_binding_lanes)
        .with_horner_pack_k(5)
}

fn production_l1_stark_config() -> p3_circuit_prover::config::GoldilocksTipsConfig {
    p3_circuit_prover::config::goldilocks_tip5_60bit()
}

fn compact_batch_l1_table_packing(public_binding_lanes: usize) -> p3_circuit_prover::TablePacking {
    p3_circuit_prover::TablePacking::new(DIGEST_ELEMS, COMPACT_BATCH_L1_ALU_LANES)
        .with_public_binding_lanes(public_binding_lanes)
        .with_horner_pack_k(COMPACT_BATCH_L1_HORNER_PACK_K)
}

fn compact_batch_l2_table_packing(public_binding_lanes: usize) -> p3_circuit_prover::TablePacking {
    p3_circuit_prover::TablePacking::new(public_binding_lanes, COMPACT_BATCH_L2_ALU_LANES)
        .with_public_binding_lanes(public_binding_lanes)
        .with_fri_params(
            COMPACT_BATCH_L2_LOG_FINAL_POLY_LEN, COMPACT_BATCH_L2_LOG_BLOWUP,
        )
        .with_horner_pack_k(COMPACT_BATCH_L2_HORNER_PACK_K)
        .with_npo_lanes(NpoTypeId::recompose(), COMPACT_BATCH_L2_RECOMPOSE_LANES)
        .with_npo_lanes(
            NpoTypeId::recompose_with_coeff_lookups(),
            COMPACT_BATCH_L2_RECOMPOSE_LANES,
        )
}

fn compact_batch_l1_stark_config() -> p3_circuit_prover::config::GoldilocksTipsConfig {
    p3_circuit_prover::config::goldilocks_tip5_pure_query_60bit_with_shape_and_cap(
        COMPACT_BATCH_L1_LOG_BLOWUP, COMPACT_BATCH_L1_NUM_QUERIES, COMPACT_BATCH_L1_CAP_HEIGHT,
    )
}

fn compact_batch_l2_stark_config() -> p3_circuit_prover::config::GoldilocksBlake3Config {
    p3_circuit_prover::config::goldilocks_blake3_with_fri_shape(
        COMPACT_BATCH_L2_LOG_BLOWUP, COMPACT_BATCH_L2_NUM_QUERIES,
        COMPACT_BATCH_L2_LOG_FINAL_POLY_LEN, COMPACT_BATCH_L2_MAX_LOG_ARITY,
        COMPACT_BATCH_L2_CAP_HEIGHT,
    )
}

fn compact_batch_l1_fri_verifier_params() -> FriVerifierParams {
    FriVerifierParams::with_mmcs(
        COMPACT_BATCH_L1_LOG_BLOWUP,
        COMPACT_BATCH_L1_LOG_FINAL_POLY_LEN,
        p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
        p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
        Tip5Config::GOLDILOCKS_W16,
    )
}

fn compact_batch_l2_fri_shape() -> p3_circuit_prover::GoldilocksBlake3FriShape {
    p3_circuit_prover::GoldilocksBlake3FriShape {
        log_blowup: COMPACT_BATCH_L2_LOG_BLOWUP,
        log_final_poly_len: COMPACT_BATCH_L2_LOG_FINAL_POLY_LEN,
        max_log_arity: COMPACT_BATCH_L2_MAX_LOG_ARITY,
        num_queries: COMPACT_BATCH_L2_NUM_QUERIES,
        commit_pow_bits:
            p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
        query_pow_bits:
            p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
        cap_height: COMPACT_BATCH_L2_CAP_HEIGHT,
    }
}

fn append_len_prefixed_bytes_as_fields(out: &mut Vec<Val>, bytes: &[u8]) {
    out.push(Val::from_u64(bytes.len() as u64));
    for chunk in bytes.chunks(7) {
        let mut limb = 0u64;
        for (shift, byte) in chunk.iter().enumerate() {
            limb |= u64::from(*byte) << (8 * shift);
        }
        out.push(Val::from_u64(limb));
    }
}

fn compact_batch_verifier_key_digest_from_parts(
    metadata: &p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata,
    fri_shape: p3_circuit_prover::GoldilocksBlake3FriShape,
) -> Result<AiPowCompactBatchVerifierKeyDigest, postcard::Error> {
    let route_params = (
        COMPACT_BATCH_L1_LOG_BLOWUP, COMPACT_BATCH_L1_NUM_QUERIES, COMPACT_BATCH_L1_CAP_HEIGHT,
        COMPACT_BATCH_L1_LOG_FINAL_POLY_LEN, COMPACT_BATCH_L1_ALU_LANES,
        COMPACT_BATCH_L1_HORNER_PACK_K, COMPACT_BATCH_L2_LOG_BLOWUP, COMPACT_BATCH_L2_NUM_QUERIES,
        COMPACT_BATCH_L2_CAP_HEIGHT, COMPACT_BATCH_L2_LOG_FINAL_POLY_LEN,
        COMPACT_BATCH_L2_MAX_LOG_ARITY, COMPACT_BATCH_L2_ALU_LANES, COMPACT_BATCH_L2_HORNER_PACK_K,
    );
    let route_params = postcard::to_allocvec(&route_params)?;
    let metadata = postcard::to_allocvec(metadata)?;
    let fri_shape = postcard::to_allocvec(&fri_shape)?;

    let mut inputs = Vec::new();
    append_len_prefixed_bytes_as_fields(&mut inputs, b"ai-pow-compact-batch-blake3-v1");
    append_len_prefixed_bytes_as_fields(&mut inputs, &route_params);
    append_len_prefixed_bytes_as_fields(&mut inputs, &metadata);
    append_len_prefixed_bytes_as_fields(&mut inputs, &fri_shape);

    let mut state = [Val::ZERO; WIDTH];
    for chunk in inputs.chunks(RATE) {
        for (i, slot) in state.iter_mut().take(RATE).enumerate() {
            *slot = chunk.get(i).copied().unwrap_or(Val::ZERO);
        }
        state = RecTip5Perm.permute(state);
    }
    Ok(state[..DIGEST_ELEMS]
        .try_into()
        .expect("digest slice width is fixed"))
}

fn statement_public_digest(public_values: &[Val]) -> Vec<Val> {
    let mut state = [Val::ZERO; WIDTH];
    for chunk in public_values.chunks(RATE) {
        for i in 0..RATE {
            state[i] = chunk.get(i).copied().unwrap_or(Val::ZERO);
        }
        state = RecTip5Perm.permute(state);
    }
    state[..DIGEST_ELEMS].to_vec()
}

fn compact_batch_l1_public_values_for_statement(
    public_values: &[Val],
    l0_program_commitment: &[Val],
) -> Vec<Val> {
    // P0/D6: fold the L0 program commitment into the statement-digest preimage,
    // exactly as the in-circuit sponge does. The caller supplies the *canonical*
    // commitment (verifier-derived via `logup_common_for`), so the L2 proof only
    // verifies if the prover used the canonical program's opened schedule.
    let mut preimage = public_values.to_vec();
    preimage.extend_from_slice(l0_program_commitment);
    statement_public_digest(&preimage)
        .into_iter()
        .flat_map(|value| {
            let lifted = Challenge::from(value);
            <Challenge as BasedVectorSpace<Val>>::as_basis_coefficients_slice(&lifted).to_vec()
        })
        .collect()
}

/// Base-field flatten of an L0 program's preprocessed commitment (P0/D6).
///
/// Uses the same order the recursive verifier target-side uses
/// (`MerkleCapTargets::get_values`), base-extracted (each element is a base value
/// lifted to the extension field, so coefficient 0 is lossless). Empty if the
/// program has no preprocessed data.
fn l0_program_commitment_vals(common_data: &CommonData<AiPowStarkConfig>) -> Vec<Val> {
    match common_data.preprocessed.as_ref() {
        Some(prep) => {
            <CompositeComm as p3_recursion::Recursive<Challenge>>::get_values(&prep.commitment)
                .into_iter()
                .map(|c| <Challenge as BasedVectorSpace<Val>>::as_basis_coefficients_slice(&c)[0])
                .collect()
        }
        None => Vec::new(),
    }
}

/// Preimage of the compact L1 statement digest: the L0 public values followed by
/// the L0 program commitment (P0/D6). Both the in-circuit sponge
/// (`build_composite_l1_verifier_circuit`) and the node's expected digest fold
/// this identically; the node derives `common_data` witness-free from the
/// canonical program via `logup_common_for`.
fn compact_batch_l1_statement_digest_preimage(
    public_values: &[Val],
    common_data: &CommonData<AiPowStarkConfig>,
) -> Vec<Val> {
    let mut preimage = public_values.to_vec();
    preimage.extend(l0_program_commitment_vals(common_data));
    preimage
}

/// Derive the **canonical** L0 program commitment (base-field flatten) the node
/// folds into the compact statement digest to pin the opened schedule (P0/D6).
///
/// Witness-free: needs only the canonical `program` (rebuilt by the verifier from
/// the public opened schedule via `canonical_program_for_strip_schedule`) and the
/// config. Uses the compact prove path's `sx_bound = true`. MoE is handled for
/// free — a MoE canonical program is just a different `Program`.
pub fn canonical_l0_program_commitment_vals(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    program: &crate::AiPowProgram,
) -> Vec<Val> {
    let cfg = crate::composite_proof::build_config(zk_params, profile);
    let pd = crate::composite_proof::logup_common_for(&cfg, program, true);
    l0_program_commitment_vals(&pd.common)
}

fn compact_batch_l1_public_values_from_built(built: &BuiltCompositeL1) -> Vec<Val> {
    built
        .public_inputs
        .iter()
        .take(DIGEST_ELEMS)
        .flat_map(|value| {
            <Challenge as BasedVectorSpace<Val>>::as_basis_coefficients_slice(value)
                .iter()
                .copied()
        })
        .collect()
}

fn compact_batch_l2_public_values_for_l1(
    l1: &AiPowL1OuterProof,
    statement_digest_public_values: &[Val],
) -> Result<Vec<Vec<Val>>, VerificationError> {
    use p3_circuit::ops::PrimitiveOpType;
    use p3_circuit_prover::batch_stark_prover::NUM_PRIMITIVE_TABLES;

    let expected_public_values = l1.public_binding_lanes * l1.ext_degree;
    if statement_digest_public_values.len() != expected_public_values {
        return Err(VerificationError::InvalidProofShape(format!(
            "compact batch L2 expected {expected_public_values} L1 statement public values, got {}",
            statement_digest_public_values.len()
        )));
    }
    let mut public_values = Vec::with_capacity(NUM_PRIMITIVE_TABLES + l1.non_primitives.len());
    public_values.resize_with(NUM_PRIMITIVE_TABLES, Vec::new);
    public_values[PrimitiveOpType::Public as usize] = statement_digest_public_values.to_vec();
    public_values.extend(
        l1.non_primitives
            .iter()
            .map(|entry| entry.public_values.clone()),
    );
    Ok(public_values)
}

fn compact_batch_l2_statement_public_values_for_l1(
    statement_digest_public_values: &[Val],
) -> Vec<Val> {
    let basis_dim = <Challenge as BasedVectorSpace<Val>>::DIMENSION;
    let mut public_values = Vec::with_capacity(statement_digest_public_values.len() * basis_dim);
    for &value in statement_digest_public_values {
        let lifted = Challenge::from(value);
        public_values.extend_from_slice(
            <Challenge as BasedVectorSpace<Val>>::as_basis_coefficients_slice(&lifted),
        );
    }
    public_values
}

fn tip5_recompose_table_provers_for_compact_l2(
) -> Vec<Box<dyn p3_circuit_prover::TableProver<p3_circuit_prover::config::GoldilocksTipsConfig>>> {
    use p3_circuit_prover::{recompose_table_provers, ConstraintProfile, TableProver, Tip5Prover};

    let mut provers: Vec<Box<dyn TableProver<p3_circuit_prover::config::GoldilocksTipsConfig>>> =
        vec![Box::new(Tip5Prover::new(
            Tip5Config::GOLDILOCKS_W16,
            ConstraintProfile::Standard,
        ))];
    provers.extend(recompose_table_provers::<
        p3_circuit_prover::config::GoldilocksTipsConfig,
        2,
    >(1, true));
    provers
}

fn non_primitive_metadata_eq(
    left: &[p3_circuit_prover::NonPrimitiveTableEntry<
        p3_circuit_prover::config::GoldilocksTipsConfig,
    >],
    right: &[p3_circuit_prover::NonPrimitiveTableEntry<
        p3_circuit_prover::config::GoldilocksTipsConfig,
    >],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.op_type == right.op_type
                && left.rows == right.rows
                && left.lanes == right.lanes
                && left.public_values == right.public_values
                && left.air_variant == right.air_variant
        })
}

/// The recursion `OpeningProof` target type for ai-pow-zk's Layer-0
/// `TwoAdicFriPcs` (the `InnerFriGeneric` alias from the recursion test
/// suite, instantiated with ai-pow-zk's own MMCS hash/compress).
type InnerFri = FriProofTargets<
    Val,
    Challenge,
    RecExtensionValMmcs<
        Val,
        Challenge,
        DIGEST_ELEMS,
        RecValMmcs<Val, DIGEST_ELEMS, Tip5Sponge, Tip5Compress>,
    >,
    InputProofTargets<Val, Challenge, RecValMmcs<Val, DIGEST_ELEMS, Tip5Sponge, Tip5Compress>>,
    Witness<Val>,
>;

struct CompactBatchRecScalarValMmcs<const DIGEST_ELEMS: usize, H, C>(
    core::marker::PhantomData<(H, C)>,
);

impl<const DIGEST_ELEMS: usize, H, C> p3_recursion::RecursiveMmcs<Val, Challenge>
    for CompactBatchRecScalarValMmcs<DIGEST_ELEMS, H, C>
where
    H: p3_symmetric::CryptographicHasher<Val, [Val; DIGEST_ELEMS]> + Sync,
    C: p3_symmetric::PseudoCompressionFunction<[Val; DIGEST_ELEMS], 2> + Sync,
    [Val; DIGEST_ELEMS]: serde::Serialize + for<'a> serde::Deserialize<'a>,
{
    type Input = p3_merkle_tree::MerkleTreeMmcs<Val, Val, H, C, 2, DIGEST_ELEMS>;
    type Commitment = MerkleCapTargets<Val, DIGEST_ELEMS>;
    type Proof = p3_recursion::pcs::fri::HashProofTargets<Val, DIGEST_ELEMS>;
}

type CompactBatchL2Hash = p3_symmetric::PaddingFreeSponge<RecTip5Perm, WIDTH, RATE, DIGEST_ELEMS>;
type CompactBatchL2Compress =
    p3_symmetric::TruncatedPermutation<RecTip5Perm, 2, DIGEST_ELEMS, WIDTH>;
type CompactBatchL2ValMmcs = p3_merkle_tree::MerkleTreeMmcs<
    Val,
    Val,
    CompactBatchL2Hash,
    CompactBatchL2Compress,
    2,
    DIGEST_ELEMS,
>;
type CompactBatchL2ChallengeMmcs = p3_commit::ExtensionMmcs<Val, Challenge, CompactBatchL2ValMmcs>;
type CompactBatchL2Comm = MerkleCapTargets<Val, DIGEST_ELEMS>;
type CompactBatchL2RecValMmcs =
    CompactBatchRecScalarValMmcs<DIGEST_ELEMS, CompactBatchL2Hash, CompactBatchL2Compress>;
type CompactBatchL2InputProof = InputProofTargets<Val, Challenge, CompactBatchL2RecValMmcs>;
type CompactBatchL2InnerFri = FriProofTargets<
    Val,
    Challenge,
    RecExtensionValMmcs<Val, Challenge, DIGEST_ELEMS, CompactBatchL2RecValMmcs>,
    CompactBatchL2InputProof,
    Witness<Val>,
>;

/// The recursion `Comm`/commitment target type.
type CompositeComm = MerkleCapTargets<Val, DIGEST_ELEMS>;
/// The recursion `InputProof` target type.
type CompositeInputProof =
    InputProofTargets<Val, Challenge, RecValMmcs<Val, DIGEST_ELEMS, Tip5Sponge, Tip5Compress>>;

/// `Tip5Perm` lifted to act on `Challenge` (`BinomialExtensionField<
/// Goldilocks, 2>`) lanes — reads each lane's constant basis
/// coefficient, runs the base-field scalar Tip5 permutation, and
/// re-embeds with only the constant coefficient set. This is the
/// in-circuit-challenger counterpart of ai-pow-zk's native
/// `DuplexChallenger<Goldilocks, Tip5Perm, 16, 10>`; the in-circuit
/// Tip5 NPO witnesses exactly this. It uses the recursion's
/// `p3_tip5_circuit_air::Tip5Perm`, which is KAT-anchored byte-for-byte
/// to `nockchain_math::tip5::permute_5round` (the permutation ai-pow-zk's
/// native `Tip5Perm` wraps), so the in-circuit transcript matches the
/// native proof's transcript.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiftTip5;

impl Permutation<[Challenge; 16]> for LiftTip5 {
    fn permute(&self, input: [Challenge; 16]) -> [Challenge; 16] {
        let bases: [Val; 16] = core::array::from_fn(|i| {
            <Challenge as BasedVectorSpace<Val>>::as_basis_coefficients_slice(&input[i])[0]
        });
        let out = RecTip5Perm.permute(bases);
        core::array::from_fn(|i| {
            <Challenge as BasedVectorSpace<Val>>::from_basis_coefficients_fn(|j| {
                if j == 0 {
                    out[i]
                } else {
                    Val::ZERO
                }
            })
        })
    }

    fn permute_mut(&self, input: &mut [Challenge; 16]) {
        *input = Permutation::permute(self, *input);
    }
}

/// A fully-built L1 verifier circuit for a composite proof, plus
/// everything needed to run it.
#[doc(hidden)]
pub struct BuiltCompositeL1 {
    /// The L1 verifier circuit (proves "I verified the composite proof").
    pub circuit: p3_circuit::Circuit<Challenge>,
    /// Layer-0 AI-PoW statement values that are exposed and bound by the L1
    /// outer certificate.
    pub statement_public_values: Vec<Val>,
    /// Public inputs for the runner.
    pub public_inputs: Vec<Challenge>,
    /// Private inputs for the runner (opened values etc.).
    pub private_inputs: Vec<Challenge>,
    /// MMCS op ids needing FRI Merkle sibling private data.
    pub mmcs_op_ids: Vec<NonPrimitiveOpId>,
}

/// S3b/c — build the L1 recursive-verification circuit for a composite
/// `BatchProof`.
///
/// The composite (`CompositeFullAirWithLookupsPinned`) is a single
/// LogUp AIR proven by `p3_batch_stark::prove_batch`; its proof is a
/// bare `p3_batch_stark::BatchProof`. It is verified in-circuit by
/// `verify_batch_circuit` with the composite AIR as the single generic
/// `A` (vs the circuit-prover multi-table path of
/// `verify_p3_batch_proof_circuit`).
///
/// `profile` MUST be the same `CircuitConfig` the composite proof was
/// produced under: the L1 verifier circuit's FRI parameters
/// (`log_blowup`, `commit/query_pow_bits`) are derived from it and
/// must match the proof's transcript exactly, or the in-circuit
/// challenger desynchronizes. (`num_queries` is intrinsic to the
/// proof shape and need not be threaded.)
#[doc(hidden)]
pub fn build_composite_l1_verifier_circuit(
    config: &AiPowStarkConfig,
    composite_air: &CompositeFullAirWithLookupsPinned,
    proof: &BatchProof<AiPowStarkConfig>,
    common_data: &CommonData<AiPowStarkConfig>,
    public_values: &[Val],
    profile: &crate::circuit::CircuitConfig,
) -> Result<BuiltCompositeL1, VerificationError> {
    build_composite_l1_verifier_circuit_with_recompose_coeff_ctl(
        config, composite_air, proof, common_data, public_values, profile, true,
    )
}

fn build_composite_l1_verifier_circuit_with_recompose_coeff_ctl(
    config: &AiPowStarkConfig,
    composite_air: &CompositeFullAirWithLookupsPinned,
    proof: &BatchProof<AiPowStarkConfig>,
    common_data: &CommonData<AiPowStarkConfig>,
    public_values: &[Val],
    profile: &crate::circuit::CircuitConfig,
    recompose_coeff_ctl_for_decompose_links: bool,
) -> Result<BuiltCompositeL1, VerificationError> {
    let mut cb = CircuitBuilder::<Challenge>::new();
    // In-circuit Tip5 permutation NPO + the recompose link (mirror of
    // the validated Layer-0 verifier circuit, `test_tip5_layer0_
    // recursion.rs`).
    cb.enable_tip5_perm::<Tip5Goldilocks, _>(
        generate_tip5_trace::<Challenge, Tip5Goldilocks>, LiftTip5,
    );
    cb.enable_recompose::<Val>(generate_recompose_trace::<Val, Challenge>);
    cb.set_recompose_coeff_ctl_for_decompose_links(recompose_coeff_ctl_for_decompose_links);

    // ai-pow-zk Layer-0 FRI verifier params — derived from the same
    // `CircuitConfig` `build_stark_config` used to prove the
    // composite. This mapping MUST mirror `build_stark_config`:
    // `log_final_poly_len = 0` (fixed there), and BOTH the commit-
    // and query-phase PoW tiers take `config.pow_bits`. Hard-coding
    // the PoW bits to 0 (as an earlier revision did) desynchronizes
    // the in-circuit challenger from any `pow_bits > 0` proof —
    // `check_pow_witness` early-returns at 0 bits, skipping the
    // observe+sample the prover's transcript performed.
    let fri_verifier_params = FriVerifierParams::with_mmcs(
        profile.log_blowup as usize,
        0,
        profile.pow_bits as usize,
        profile.pow_bits as usize,
        Tip5Config::GOLDILOCKS_W16,
    );

    // The composite is a single AIR instance.
    let air_public_counts = [public_values.len()];

    let statement_digest_targets = cb.alloc_public_inputs(DIGEST_ELEMS, "statement digest");

    let verifier_inputs =
        BatchStarkVerifierInputsBuilder::<AiPowStarkConfig, CompositeComm, InnerFri>::allocate(
            &mut cb, proof, common_data, &air_public_counts,
        );

    let mmcs_op_ids = verify_batch_circuit::<
        CompositeFullAirWithLookupsPinned,
        AiPowStarkConfig,
        CompositeComm,
        CompositeInputProof,
        InnerFri,
        LogUpGadget,
        Tip5Config,
        WIDTH,
        RATE,
    >(
        config,
        core::slice::from_ref(composite_air),
        &mut cb,
        &verifier_inputs.proof_targets,
        &verifier_inputs.air_public_targets,
        &fri_verifier_params,
        &verifier_inputs.common_data,
        &LogUpGadget,
        Tip5Config::GOLDILOCKS_W16,
    )?;

    // P0/D6: fold the L0 program's preprocessed commitment into the statement
    // digest, so the node can pin the opened schedule to the canonical program
    // (the commitment is witness-free-derivable by the verifier via
    // `logup_common_for`; see the compact-parity doc §4 P0). The target-side
    // flatten (`to_observation_targets`) matches the value-side flatten
    // (`MerkleCapTargets::get_values`) used to build the expected digest below.
    let mut digest_input: Vec<p3_circuit::ExprId> = verifier_inputs.air_public_targets[0].clone();
    if let Some(l0_program_commitment) = verifier_inputs.common_data.preprocessed_commitment() {
        digest_input.extend(l0_program_commitment.to_observation_targets());
    }

    let mut digest_state = [None; WIDTH];
    for (block_idx, chunk) in digest_input.chunks(RATE).enumerate() {
        let mut inputs = [None; WIDTH];
        for i in 0..RATE {
            inputs[i] = Some(chunk.get(i).copied().unwrap_or(p3_circuit::ExprId::ZERO));
        }
        let outputs = cb.add_tip5_perm_for_challenger_base(
            Tip5Config::GOLDILOCKS_W16,
            block_idx == 0,
            inputs,
        )?;
        digest_state = outputs.map(Some);
    }
    for (target, digest_limb) in statement_digest_targets
        .iter()
        .zip(digest_state.iter().take(DIGEST_ELEMS))
    {
        cb.connect(
            *target,
            digest_limb.expect("statement digest limb must exist"),
        );
    }

    let circuit = cb.build()?;
    // P0/D6: the expected statement digest must fold in the same commitment the
    // in-circuit sponge absorbs above — the VALUE flatten of the L0 program's
    // preprocessed commitment (`get_values`), base-extracted to match the base
    // sponge. Node-side (`compact_batch_l1_statement_digest_preimage`) recomputes
    // this from the canonical program.
    let statement_digest_preimage =
        compact_batch_l1_statement_digest_preimage(public_values, common_data);
    let statement_public_values = statement_public_digest(&statement_digest_preimage);
    let (verifier_public_inputs, private_inputs) =
        verifier_inputs.pack_values(&[public_values.to_vec()], proof, common_data);
    let mut public_inputs = statement_public_values
        .iter()
        .copied()
        .map(Challenge::from)
        .collect::<Vec<_>>();
    public_inputs.extend(verifier_public_inputs);

    Ok(BuiltCompositeL1 {
        circuit,
        statement_public_values,
        public_inputs,
        private_inputs,
        mmcs_op_ids,
    })
}

/// Run a built composite-L1 verifier circuit against the composite
/// proof's FRI opening data. `Ok(())` iff the in-circuit verification
/// accepts.
#[doc(hidden)]
pub fn run_composite_l1_verifier(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
) -> Result<(), VerificationError> {
    run_composite_l1_verifier_traces(built, proof)?;
    Ok(())
}

fn run_composite_l1_verifier_traces(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
) -> Result<p3_circuit::tables::Traces<Challenge>, VerificationError> {
    let mut runner = built.circuit.runner();
    runner
        .set_public_inputs(&built.public_inputs)
        .map_err(VerificationError::Circuit)?;
    runner
        .set_private_inputs(&built.private_inputs)
        .map_err(VerificationError::Circuit)?;
    set_fri_mmcs_private_data::<
        Val,
        Challenge,
        crate::circuit::ChallengeMmcs,
        crate::circuit::ValMmcs,
        Tip5Sponge,
        Tip5Compress,
        DIGEST_ELEMS,
    >(
        &mut runner,
        &built.mmcs_op_ids,
        &proof.opening_proof,
        Tip5Config::GOLDILOCKS_W16,
    )
    .map_err(|e| VerificationError::InvalidProofShape(e.to_string()))?;
    runner.run().map_err(VerificationError::Circuit)
}

// Exclusive to `verify_recursive_certificate_inner`; gated with it so a release
// build (no `test`/`test-support`) does not carry an unused checkpoint helper.
#[cfg(any(test, feature = "test-support"))]
fn production_l1_circuit_prover_data(
    built: &BuiltCompositeL1,
) -> Result<
    (
        p3_circuit_prover::TablePacking,
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksTipsConfig>,
    ),
    VerificationError,
> {
    production_l1_circuit_prover_data_with_public_binding_lanes(built, 0)
}

#[cfg(any(test, feature = "test-support"))]
fn production_l1_circuit_prover_data_with_public_binding_lanes(
    built: &BuiltCompositeL1,
    public_binding_lanes: usize,
) -> Result<
    (
        p3_circuit_prover::TablePacking,
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksTipsConfig>,
    ),
    VerificationError,
> {
    l1_circuit_prover_data_with_config_and_public_binding_lanes(
        built,
        &production_l1_stark_config(),
        public_binding_lanes,
    )
}

#[cfg(any(test, feature = "test-support"))]
fn l1_circuit_prover_data_with_config_and_public_binding_lanes(
    built: &BuiltCompositeL1,
    outer_config: &p3_circuit_prover::config::GoldilocksTipsConfig,
    public_binding_lanes: usize,
) -> Result<
    (
        p3_circuit_prover::TablePacking,
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksTipsConfig>,
    ),
    VerificationError,
> {
    let table_packing = production_l1_table_packing(public_binding_lanes);
    l1_circuit_prover_data_with_config_and_table_packing(built, outer_config, table_packing)
}

fn l1_circuit_prover_data_with_config_and_table_packing(
    built: &BuiltCompositeL1,
    outer_config: &p3_circuit_prover::config::GoldilocksTipsConfig,
    table_packing: p3_circuit_prover::TablePacking,
) -> Result<
    (
        p3_circuit_prover::TablePacking,
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksTipsConfig>,
    ),
    VerificationError,
> {
    use p3_batch_stark::ProverData;
    use p3_circuit_prover::common::{get_airs_and_degrees_with_prep, NpoPreprocessor};
    use p3_circuit_prover::{
        config, recompose_air_builders, strip_public_binding_for_lookup_metadata,
        tip5_air_builders, CircuitProverData, ConstraintProfile, RecomposePreprocessor,
        Tip5Preprocessor,
    };

    type OuterConfig = config::GoldilocksTipsConfig;

    let npo_prep: Vec<Box<dyn NpoPreprocessor<Val>>> =
        vec![Box::new(Tip5Preprocessor), Box::new(RecomposePreprocessor::new(true))];
    let mut air_builders = tip5_air_builders::<OuterConfig, 2>();
    air_builders.extend(recompose_air_builders::<OuterConfig, 2>(1, true));

    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<OuterConfig, Challenge, 2>(
            &built.circuit,
            &table_packing,
            &npo_prep,
            &air_builders,
            ConstraintProfile::Standard,
        )
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "composite L1 outer cert — get_airs_and_degrees: {e:?}"
            ))
        })?;
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();

    let lookup_metadata_airs = airs
        .iter()
        .map(strip_public_binding_for_lookup_metadata)
        .collect::<Vec<_>>();
    let prover_data =
        ProverData::from_airs_and_degrees(outer_config, &lookup_metadata_airs, &degrees);
    Ok((
        table_packing,
        CircuitProverData::new(prover_data, primitive_columns, non_primitive_columns),
    ))
}

#[derive(Clone, PartialEq)]
struct CompactBatchL1CircuitShape {
    witness_count: u32,
    ops: Vec<p3_circuit::ops::Op<Challenge>>,
    public_rows: Vec<p3_circuit::WitnessId>,
    public_flat_len: usize,
    private_input_rows: Vec<p3_circuit::WitnessId>,
    private_flat_len: usize,
    enabled_op_types: Vec<NpoTypeId>,
    expr_to_widx: Vec<(p3_circuit::ExprId, p3_circuit::WitnessId)>,
    trace_generator_order: Vec<NpoTypeId>,
    trace_generator_types: Vec<NpoTypeId>,
    tag_to_witness: Vec<(String, p3_circuit::WitnessId)>,
    tag_to_op_id: Vec<(String, NonPrimitiveOpId)>,
    witness_rewrite: Option<Vec<(p3_circuit::WitnessId, p3_circuit::WitnessId)>>,
}

fn compact_batch_l1_circuit_shape(
    circuit: &p3_circuit::Circuit<Challenge>,
) -> CompactBatchL1CircuitShape {
    let mut enabled_op_types = circuit.enabled_ops.keys().cloned().collect::<Vec<_>>();
    enabled_op_types.sort();
    let mut trace_generator_types = circuit
        .non_primitive_trace_generators
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    trace_generator_types.sort();
    let mut expr_to_widx = circuit
        .expr_to_widx
        .iter()
        .map(|(&expr, &witness)| (expr, witness))
        .collect::<Vec<_>>();
    expr_to_widx.sort_by_key(|(expr, _)| *expr);
    let mut tag_to_witness = circuit
        .tag_to_witness
        .iter()
        .map(|(tag, &witness)| (tag.clone(), witness))
        .collect::<Vec<_>>();
    tag_to_witness.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut tag_to_op_id = circuit
        .tag_to_op_id
        .iter()
        .map(|(tag, &op_id)| (tag.clone(), op_id))
        .collect::<Vec<_>>();
    tag_to_op_id.sort_by(|(left, _), (right, _)| left.cmp(right));
    let witness_rewrite = circuit.witness_rewrite.as_ref().map(|rewrite| {
        let mut entries = rewrite
            .iter()
            .map(|(&from, &to)| (from, to))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(from, _)| *from);
        entries
    });

    CompactBatchL1CircuitShape {
        witness_count: circuit.witness_count,
        ops: circuit.ops.clone(),
        public_rows: circuit.public_rows.clone(),
        public_flat_len: circuit.public_flat_len,
        private_input_rows: circuit.private_input_rows.clone(),
        private_flat_len: circuit.private_flat_len,
        enabled_op_types,
        expr_to_widx,
        trace_generator_order: circuit.non_primitive_trace_generator_order.clone(),
        trace_generator_types,
        tag_to_witness,
        tag_to_op_id,
        witness_rewrite,
    }
}

struct CompactBatchL1Prep {
    circuit_shape: CompactBatchL1CircuitShape,
    table_packing: p3_circuit_prover::TablePacking,
    circuit_prover_data: std::sync::Arc<
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksTipsConfig>,
    >,
    prover: p3_circuit_prover::BatchStarkProver<p3_circuit_prover::config::GoldilocksTipsConfig>,
}

fn build_compact_batch_l1_prep(
    built: &BuiltCompositeL1,
) -> Result<CompactBatchL1Prep, VerificationError> {
    use p3_circuit_prover::BatchStarkProver;

    let table_packing = compact_batch_l1_table_packing(DIGEST_ELEMS);
    let (table_packing, circuit_prover_data) =
        l1_circuit_prover_data_with_config_and_table_packing(
            built,
            &compact_batch_l1_stark_config(),
            table_packing,
        )?;
    let mut prover = BatchStarkProver::new(compact_batch_l1_stark_config())
        .with_table_packing(table_packing.clone());
    prover.register_tip5_table::<2>(Tip5Config::GOLDILOCKS_W16);
    prover.register_recompose_table::<2>(true);

    Ok(CompactBatchL1Prep {
        circuit_shape: compact_batch_l1_circuit_shape(&built.circuit),
        table_packing,
        circuit_prover_data: std::sync::Arc::new(circuit_prover_data),
        prover,
    })
}

fn ensure_compact_batch_l1_prep_matches_built(
    prep: &CompactBatchL1Prep,
    built: &BuiltCompositeL1,
) -> Result<(), VerificationError> {
    if prep.table_packing != compact_batch_l1_table_packing(DIGEST_ELEMS) {
        return Err(VerificationError::InvalidProofShape(
            "compact batch L1 prep table-packing mismatch".to_string(),
        ));
    }
    if prep.circuit_shape != compact_batch_l1_circuit_shape(&built.circuit) {
        return Err(VerificationError::InvalidProofShape(
            "compact batch L1 prep was built for a different verifier circuit/setup shape"
                .to_string(),
        ));
    }
    Ok(())
}

fn prove_compact_batch_l1_with_prep(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
    prep: &CompactBatchL1Prep,
) -> Result<AiPowL1OuterProof, VerificationError> {
    ensure_compact_batch_l1_prep_matches_built(prep, built)?;
    let traces = run_composite_l1_verifier_traces(built, proof)?;
    prep.prover
        .prove_all_tables(&traces, prep.circuit_prover_data.as_ref())
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch L1 outer cert — prove_all_tables: {e:?}"
            ))
        })
}

/// S5 — produce the **L1 outer certificate** for a composite proof:
/// prove the composite-L1 verifier circuit itself as a D=2 batch-STARK
/// (`prove_all_tables`). This is the outer recursive proof object for the
/// statement "I verified the composite proof".
///
/// Mirrors the validated `outer_cert_layer0` machinery
/// (`Plonky3-recursion` `test_tip5_layer0_recursion.rs`) — D=2,
/// Tip5 NPO (D=1 perm) + recompose with split coeff tables — with the
/// composite-L1 circuit in place of the Fibonacci-L0 one.
///
/// Returns the L1 outer proof on accept; an `Err` if the L1 verifier circuit
/// runner rejects before outer proving.
#[doc(hidden)]
pub fn prove_composite_l1_outer_cert(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
) -> Result<AiPowL1OuterProof, VerificationError> {
    prove_composite_l1_outer_cert_with_public_binding_lanes(built, proof, 0)
}

fn prove_composite_l1_outer_cert_with_public_binding_lanes(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
    public_binding_lanes: usize,
) -> Result<AiPowL1OuterProof, VerificationError> {
    prove_composite_l1_outer_cert_with_config_and_public_binding_lanes(
        built,
        proof,
        production_l1_stark_config(),
        public_binding_lanes,
    )
}

fn prove_composite_l1_outer_cert_with_config_and_public_binding_lanes(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
    outer_config: p3_circuit_prover::config::GoldilocksTipsConfig,
    public_binding_lanes: usize,
) -> Result<AiPowL1OuterProof, VerificationError> {
    let table_packing = production_l1_table_packing(public_binding_lanes);
    prove_composite_l1_outer_cert_with_config_and_table_packing(
        built, proof, outer_config, table_packing,
    )
}

fn prove_composite_l1_outer_cert_with_config_and_table_packing(
    built: &BuiltCompositeL1,
    proof: &BatchProof<AiPowStarkConfig>,
    outer_config: p3_circuit_prover::config::GoldilocksTipsConfig,
    table_packing: p3_circuit_prover::TablePacking,
) -> Result<AiPowL1OuterProof, VerificationError> {
    use p3_circuit_prover::BatchStarkProver;

    let (table_packing, circuit_prover_data) =
        l1_circuit_prover_data_with_config_and_table_packing(built, &outer_config, table_packing)?;
    let traces = run_composite_l1_verifier_traces(built, proof)?;
    let mut prover = BatchStarkProver::new(outer_config).with_table_packing(table_packing);
    prover.register_tip5_table::<2>(Tip5Config::GOLDILOCKS_W16);
    prover.register_recompose_table::<2>(true);

    let batch_proof = prover
        .prove_all_tables(&traces, &circuit_prover_data)
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "composite L1 outer cert — prove_all_tables: {e:?}"
            ))
        })?;
    Ok(batch_proof)
}

/// Verify the batch-STARK recursive checkpoint certificate against the
/// verifier-derived Layer-0 AI-PoW public inputs and chain-pinned proving
/// parameters.
///
/// This is the hardened batch-STARK checkpoint verifier. It rejects outer
/// proofs whose circuit-prover metadata is merely self-consistent by rebuilding
/// the canonical L1 verifier circuit from the certificate's Layer-0
/// proof/program, running that circuit against the verifier-derived public
/// inputs, comparing stable rebuilt outer metadata to the submitted outer
/// proof, and verifying the submitted outer proof with the production
/// batch-STARK verifier.
///
/// **Not a production path, and not compiled into production builds.** The
/// consensus accept path is the COMPACT verifier (`verify_ai_pow_block_artifact`
/// -> the `_compact_` verifiers), which binds the canonical Layer-0 program via
/// the P0/D6 opened-schedule commitment fold. This standalone checkpoint verifier
/// is retained only as a regression/benchmark intermediate, so it is gated behind
/// `test`/`test-support` and does not exist in a release binary. A caller MUST
/// bind the certificate's `l0_program` to the canonical program from the public
/// opened schedule (see `AiPowRecursiveCertificate::l0_program_matches`) before
/// trusting the result — this verifier proves the statement for the certificate's
/// own embedded program.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn verify_recursive_certificate(
    cert: &AiPowRecursiveCertificate,
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    public_inputs: &crate::composite_public::CompositePublicInputs,
) -> Result<(), VerificationError> {
    verify_recursive_certificate_inner(cert, zk_params, profile, &public_inputs.to_vec())
}

#[cfg(any(test, feature = "test-support"))]
fn verify_recursive_certificate_inner(
    cert: &AiPowRecursiveCertificate,
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    public_values: &[Val],
) -> Result<(), VerificationError> {
    use p3_circuit_prover::BatchStarkProver;

    if public_values.len() != crate::composite_public::NUM_PUBLIC_VALUES {
        return Err(VerificationError::InvalidProofShape(format!(
            "AI-PoW recursive certificate verification requires exactly {} \
                 verifier-derived public inputs; got {}",
            crate::composite_public::NUM_PUBLIC_VALUES,
            public_values.len()
        )));
    }

    let cfg = crate::composite_proof::build_config(zk_params, profile);
    // §6(b)-R-b: match the L0 keystone flag (sx_bound=false for the
    // num_stripes>STRIPE_MAX stripe-major path). Verifier-derived from params.
    let sx_bound =
        (zk_params.k / zk_params.noise_rank) as usize <= crate::composite_layout::STRIPE_MAX;
    let air = CompositeFullAirWithLookupsPinned::new_with(cert.l0_program.clone(), sx_bound);
    let pd = crate::composite_proof::logup_common_for(&cfg, &cert.l0_program, sx_bound);
    let built = build_composite_l1_verifier_circuit(
        &cfg, &air, &cert.l0_proof, &pd.common, public_values, profile,
    )?;

    let traces = run_composite_l1_verifier_traces(&built, &cert.l0_proof)?;

    let (expected_circuit_packing, expected_circuit_prover_data) =
        production_l1_circuit_prover_data(&built)?;

    let mut expected_outer_prover = BatchStarkProver::new(production_l1_stark_config())
        .with_table_packing(expected_circuit_packing.clone());
    expected_outer_prover.register_tip5_table::<2>(Tip5Config::GOLDILOCKS_W16);
    expected_outer_prover.register_recompose_table::<2>(true);
    let expected_outer_proof = expected_outer_prover
        .prove_all_tables(&traces, &expected_circuit_prover_data)
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "AI-PoW recursive certificate verifier could not rebuild canonical \
                 L1 outer proof metadata: {e:?}"
            ))
        })?;
    let outer = &cert.l1_outer_proof;
    if outer.rows != expected_outer_proof.rows
        || outer.alu_variant != expected_outer_proof.alu_variant
        || outer.ext_degree != expected_outer_proof.ext_degree
        || outer.w_binomial != expected_outer_proof.w_binomial
        || outer.alu_quintic_trinomial != expected_outer_proof.alu_quintic_trinomial
        || !non_primitive_metadata_eq(&outer.non_primitives, &expected_outer_proof.non_primitives)
    {
        return Err(VerificationError::InvalidProofShape(
            "AI-PoW recursive certificate outer proof metadata is not the \
             canonical L1 verifier circuit shape for the supplied Layer-0 \
             proof, program, parameters, and public inputs"
                .to_string(),
        ));
    }
    if !p3_circuit_prover::common_preprocessed_binding_eq(
        &outer.stark_common, &expected_outer_proof.stark_common,
    ) {
        return Err(VerificationError::InvalidProofShape(
            "AI-PoW recursive certificate outer proof preprocessed commitment \
             binding is not the canonical L1 verifier circuit preprocessed binding"
                .to_string(),
        ));
    }

    let expected_public_binding_lanes = 0;
    let expected_packing = production_l1_table_packing(expected_public_binding_lanes);
    if outer.ext_degree != 2 {
        return Err(VerificationError::InvalidProofShape(format!(
            "AI-PoW recursive certificate uses extension degree {}; expected 2",
            outer.ext_degree
        )));
    }
    if expected_circuit_packing != expected_packing {
        return Err(VerificationError::InvalidProofShape(format!(
            "rebuilt AI-PoW recursive verifier circuit uses table packing {:?}; \
             expected production packing {:?}",
            expected_circuit_packing, expected_packing
        )));
    }
    if outer.table_packing != expected_packing {
        return Err(VerificationError::InvalidProofShape(format!(
            "AI-PoW recursive certificate uses non-production table packing {:?}; \
             expected {:?}",
            outer.table_packing, expected_packing
        )));
    }
    if outer.public_binding_lanes != expected_public_binding_lanes {
        return Err(VerificationError::InvalidProofShape(format!(
            "AI-PoW recursive certificate binds {} L1 public values; expected {}",
            outer.public_binding_lanes, expected_public_binding_lanes
        )));
    }
    if outer.alu_quintic_trinomial {
        return Err(VerificationError::InvalidProofShape(
            "AI-PoW recursive certificate unexpectedly selected quintic ALU".to_string(),
        ));
    }
    expected_outer_prover
        .verify_all_tables(outer)
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "AI-PoW recursive certificate outer proof failed production \
             batch-STARK verification: {e:?}"
            ))
        })?;
    Ok(())
}

/// Per-stage instrumentation of one end-to-end composite→L1 recursion run.
///
/// `l1_cert` is the batch-STARK recursive checkpoint certificate. The Layer-0
/// proof and pinned program are intentionally owned by that certificate so
/// verification can rebuild and bind the exact L1 verifier circuit.
#[doc(hidden)]
pub struct L1RecursionRun {
    /// Composite (Layer-0) STARK trace height — the dominant cost
    /// and memory driver.
    pub composite_trace_height: usize,
    /// Composite trace width (`composite_layout::TOTAL_TRACE_WIDTH`).
    pub composite_trace_width: usize,
    /// Wall-clock (ms) to prove the composite batch-STARK (L0).
    pub composite_prove_ms: u128,
    /// Wall-clock (ms) to build the L1 recursive-verifier circuit.
    pub l1_circuit_build_ms: u128,
    /// Wall-clock (ms) to run the L1 verifier circuit — the
    /// in-circuit accept check (S3).
    pub l1_in_circuit_verify_ms: u128,
    /// Wall-clock (ms) to outer-prove the L1 verifier circuit as a
    /// D=2 batch-STARK + `verify_all_tables` — the L1 certificate (S5).
    pub l1_outer_cert_ms: u128,
    /// Public inputs bound by the composite proof that the L1 certificate
    /// recursively verifies.
    pub public_inputs: crate::composite_public::CompositePublicInputs,
    /// The L1 recursive certificate.
    ///
    /// This is the batch-STARK recursive checkpoint artifact.
    pub l1_cert: AiPowRecursiveCertificate,
}

/// Timings and certificate for recursively certifying an already-built
/// Layer-0 composite proof.
///
/// This is useful for callers that already used the ai-pow bridge to build
/// the canonical Layer-0 proof and pinned program from a mining solution.
/// The returned `l1_cert` is the recursive proof artifact; consensus admission
/// still belongs to the outer ai-pow statement verifier.
#[doc(hidden)]
pub struct L1CertificateRun {
    /// Wall-clock (ms) to build the L1 recursive-verifier circuit.
    pub l1_circuit_build_ms: u128,
    /// Wall-clock (ms) to run the L1 verifier circuit.
    pub l1_in_circuit_verify_ms: u128,
    /// Wall-clock (ms) to outer-prove the L1 verifier circuit.
    pub l1_outer_cert_ms: u128,
    /// The batch-STARK recursive checkpoint certificate.
    pub l1_cert: AiPowRecursiveCertificate,
}

/// Timings, compact certificate, and verifier-owned context for the committed
/// compact final-layer batch-STARK route.
///
/// The certificate is the only wire candidate. The verifier context is returned
/// here for tests and local verification; production must derive or pin an
/// equivalent context out of band instead of accepting it from a miner. The
/// certificate carries only a digest of that verifier-owned context.
pub struct CompactBatchCertificateRun {
    pub l1_circuit_build_ms: u128,
    pub l1_outer_cert_ms: u128,
    pub l2_prep_ms: u128,
    pub l2_prove_ms: u128,
    pub l2_compact_ms: u128,
    pub l2_compact_verify_ms: u128,
    pub compact_cert: AiPowCompactBatchRecursiveCertificate,
    pub verifier_context: AiPowCompactBatchVerifierContext,
    /// The L1 outer proof — the COMPACT input from which the (large) verifier
    /// context is rebuilt via [`rebuild_compact_verifier_context`]. Caching THIS
    /// (small) + `verifier_context.metadata` lets the boot table rebuild the
    /// per-bucket verifier setup cheaply, instead of serializing the ~866 MB
    /// preprocessed Merkle tree the context carries.
    pub l1_outer_proof: AiPowL1OuterProof,
    /// Newly-built reusable L2 setup, present only when this run did not use
    /// a caller-supplied cache.
    pub prover_cache: Option<AiPowCompactBatchProverCache>,
}

/// Rebuild the compact verifier context WITHOUT proving, from the SMALL cached
/// inputs — the boot-time primitive for the cached fast-boot verifier-setup table.
///
/// The cached inputs per trace-height bucket are all small (KB-MB): the L0 program
/// (canonical, params-pure), the L0 composite proof, its public inputs, the
/// `sx_bound` flag, the L1 outer proof, and the L2 proof metadata. This rebuilds:
/// (1) the L1 verifier circuit (`build_composite_l1_verifier_circuit`, circuit
/// compilation — no proving), (2) its `CommonData` INCLUDING the per-AIR `lookups`
/// (`l1_circuit_prover_data_...` → `Lookups::from_air`), which serde drops from the
/// L1 proof (`Lookups` is not Serialize) but which the L2 build needs, (3) the L2
/// verifier circuit + its preprocessed commitment (`build_compact_batch_l2_over_l1_prep`,
/// the ~866 MB tree — deterministic, rebuilt in memory, never cached), then the
/// FRI shape (constant) + derived digest. All fast (compile + Merkle commit, no FRI).
#[allow(clippy::too_many_arguments)]
pub fn rebuild_compact_verifier_context(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    l0_program: &crate::AiPowProgram,
    l0_proof: &BatchProof<AiPowStarkConfig>,
    l0_public_inputs: &crate::composite_public::CompositePublicInputs,
    sx_bound: bool,
    l1_outer_proof: AiPowL1OuterProof,
    metadata: p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata,
) -> Result<AiPowCompactBatchVerifierContext, VerificationError> {
    // (1) Rebuild the L1 verifier circuit (compile-only, no proving).
    let cfg = crate::composite_proof::build_config(zk_params, profile);
    let air = CompositeFullAirWithLookupsPinned::new_with(l0_program.clone(), sx_bound);
    let pd = crate::composite_proof::logup_common_for(&cfg, l0_program, sx_bound);
    let built = build_composite_l1_verifier_circuit(
        &cfg,
        &air,
        l0_proof,
        &pd.common,
        &l0_public_inputs.to_vec(),
        profile,
    )?;
    // (2) Rebuild the L1 CommonData WITH lookups (Lookups::from_air) and install it
    // on the (serde-lossy) cached L1 proof, so the L2 build has what it needs.
    let (_, l1_cpd) = l1_circuit_prover_data_with_config_and_table_packing(
        &built,
        &compact_batch_l1_stark_config(),
        compact_batch_l1_table_packing(DIGEST_ELEMS),
    )?;
    let mut l1 = l1_outer_proof;
    l1.stark_common = l1_cpd.prover_data.common;
    // (3) Rebuild the L2 preprocessed commitment + assemble the context.
    let l2_prep = build_compact_batch_l2_over_l1_prep(&l1)?;
    let fri_shape = compact_batch_l2_fri_shape();
    let verifier_key_digest = compact_batch_verifier_key_digest_from_parts(&metadata, fri_shape)
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "rebuild: compact batch verifier-key digest construction failed: {e:?}"
            ))
        })?;
    // This context only ever VERIFIES (it is the boot verifier setup): drop the
    // prove-only raw preprocessed columns, which the path-pruned compact verifier
    // never reads, to cut resident memory. The preprocessed Merkle tree in
    // `prover_data` — which verification DOES need to restore omitted openings — is
    // kept, so verification is bit-identical. The Arc is freshly built on this
    // rebuild path (refcount 1) so `try_unwrap` succeeds; the shared fallback keeps
    // the full data (still correct, just not slimmed).
    let circuit_prover_data = match std::sync::Arc::try_unwrap(l2_prep.circuit_prover_data) {
        Ok(cpd) => std::sync::Arc::new(cpd.into_verifier_only()),
        Err(shared) => shared,
    };
    Ok(AiPowCompactBatchVerifierContext {
        verifier_key_digest,
        metadata,
        circuit_prover_data,
        fri_shape,
    })
}

/// The SMALL, serializable per-bucket seed for the boot verifier-setup table.
///
/// It carries exactly the inputs [`rebuild_compact_verifier_context`] needs to
/// rebuild the (large, ~866 MB) compact verifier context WITHOUT proving: the L0
/// program + proof + public inputs and the L1 outer proof + metadata. Sized in
/// KB-MB (the L0 program + proof dominate), so a full 8-bucket table caches in
/// tens of MB rather than gigabytes.
///
/// `sx_bound` and the FRI/circuit profile are pure functions of
/// `(zk_params, trace_height)` and are DERIVED at rebuild, not stored — so a
/// cached seed can never disagree with the prover about them. The trace height is
/// the L0 program height (the preprocessed program has one row per trace row).
#[derive(Serialize, Deserialize)]
pub struct AiPowCompactVerifierSetupSeed {
    pub zk_params: crate::params::ZkParams,
    pub l0_program: crate::AiPowProgram,
    pub l0_proof: BatchProof<AiPowStarkConfig>,
    pub l0_public_inputs: crate::composite_public::CompositePublicInputs,
    pub l1_outer_proof: AiPowL1OuterProof,
    pub metadata: p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata,
    /// The canonical 40-byte compact verifier-key/setup digest bytes (cached so a
    /// loader need not re-derive it; the rebuild re-derives + must match).
    pub verifier_key_digest_bytes: Vec<u8>,
}

impl AiPowCompactVerifierSetupSeed {
    /// The Layer-0 trace height this seed's setup verifies (= L0 program height).
    pub fn trace_height(&self) -> usize {
        use p3_matrix::Matrix;
        self.l0_program.height()
    }

    /// Assemble a seed from a chain-verified L0 bundle + the run's L1 outer proof
    /// and metadata. Consumes `verified_l0` (moves the L0 program/proof out; clones
    /// the borrowed public inputs). Used only by the boot-setup table builder —
    /// never on the per-block mining path, which discards these parts.
    pub fn from_run(
        zk_params: &crate::params::ZkParams,
        verified_l0: ChainVerifiedCompositeProof<'_>,
        l1_outer_proof: AiPowL1OuterProof,
        metadata: p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata,
        verifier_key_digest_bytes: Vec<u8>,
    ) -> Self {
        let ChainVerifiedCompositeProof {
            program,
            proof,
            public_inputs,
        } = verified_l0;
        Self {
            zk_params: *zk_params,
            l0_program: program,
            l0_proof: proof,
            l0_public_inputs: public_inputs.clone(),
            l1_outer_proof,
            metadata,
            verifier_key_digest_bytes,
        }
    }

    /// Rebuild the full compact verifier context (NO proving — circuit compile +
    /// Merkle commit only). Consumes the seed. `sx_bound` and the profile are
    /// derived from `(zk_params, trace_height)`, matching the prover exactly.
    pub fn rebuild_context(self) -> Result<AiPowCompactBatchVerifierContext, VerificationError> {
        let profile = crate::circuit::CircuitConfig::for_layer0_trace(self.trace_height());
        let sx_bound = (self.zk_params.k / self.zk_params.noise_rank) as usize
            <= crate::composite_layout::STRIPE_MAX;
        rebuild_compact_verifier_context(
            &self.zk_params, &profile, &self.l0_program, &self.l0_proof, &self.l0_public_inputs,
            sx_bound, self.l1_outer_proof, self.metadata,
        )
    }
}

/// **Batch-STARK recursive checkpoint caller** — the full ai-pow-zk →
/// Plonky3-recursion
/// pipeline for one composite proof, end to end:
///
/// 1. prove the composite matmul-PoW batch-STARK (Layer 0);
/// 2. build the L1 recursive-verifier circuit and run it — the
///    composite proof is verified in-circuit (S3);
/// 3. outer-prove that verifier circuit as a D=2 batch-STARK and
///    `verify_all_tables` — the L1 recursive certificate (S5).
///
/// Returns per-stage timings and the canonical L1 certificate. The
/// certificate owns the Layer-0 proof/program context required for
/// verifier-side L1 circuit binding; callers must not persist or transmit
/// any separate Layer-0 proof artifact.
///
/// This is the single public entrypoint a production consumer (or a
/// measurement harness) drives; it hides the crate-internal program-pin
/// / `CommonData` plumbing. The canonical program is extracted from the
/// trace and pinned (CRIT-1), exactly as the Layer-0 proving path
/// (`composite_prove_pinned_logup`).
#[doc(hidden)]
pub fn recurse_composite_to_l1(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    trace: crate::composite_trace::CompositeTrace,
) -> Result<L1RecursionRun, VerificationError> {
    use std::time::Instant;

    let cfg = crate::composite_proof::build_config(zk_params, profile);
    let composite_trace_height = trace.height();
    let composite_trace_width = trace.width();
    let pis = crate::composite_public::CompositePublicInputs::derive_from_trace(&trace);

    let t = Instant::now();
    let (composite_proof, program) =
        crate::composite_proof::composite_prove_pinned_logup(&cfg, trace, &pis);
    let composite_prove_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
    let pd = crate::composite_proof::logup_common_for(&cfg, &program, true);
    let built = build_composite_l1_verifier_circuit(
        &cfg,
        &air,
        &composite_proof,
        &pd.common,
        &pis.to_vec(),
        profile,
    )?;
    let l1_circuit_build_ms = t.elapsed().as_millis();

    let t = Instant::now();
    run_composite_l1_verifier(&built, &composite_proof)?;
    let l1_in_circuit_verify_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let l1_outer_proof = prove_composite_l1_outer_cert(&built, &composite_proof)?;
    let l1_cert = AiPowRecursiveCertificate::new(composite_proof, program, l1_outer_proof);
    let l1_outer_cert_ms = t.elapsed().as_millis();

    Ok(L1RecursionRun {
        composite_trace_height,
        composite_trace_width,
        composite_prove_ms,
        l1_circuit_build_ms,
        l1_in_circuit_verify_ms,
        l1_outer_cert_ms,
        public_inputs: pis,
        l1_cert,
    })
}

/// Layer-0 proof parts that a caller has already checked against the
/// chain-derived AI-PoW statement.
pub struct ChainVerifiedCompositeProof<'a> {
    program: crate::AiPowProgram,
    proof: BatchProof<AiPowStarkConfig>,
    public_inputs: &'a crate::composite_public::CompositePublicInputs,
}

impl<'a> ChainVerifiedCompositeProof<'a> {
    /// Construct a recursion input after the caller has verified the
    /// Layer-0 proof against the exact chain-derived statement:
    /// canonical program, public inputs, target, selected work unit,
    /// commitments, nonce, and production/full-work admissibility.
    ///
    /// # Safety
    ///
    /// This is unsafe because the type cannot itself prove that the
    /// caller performed the chain statement verification. Constructing
    /// it from arbitrary proof parts can produce a recursive certificate
    /// for a valid STARK statement that is not a valid Nockchain AI-PoW
    /// work unit.
    pub unsafe fn from_parts_after_chain_statement_verification(
        program: crate::AiPowProgram,
        proof: BatchProof<AiPowStarkConfig>,
        public_inputs: &'a crate::composite_public::CompositePublicInputs,
    ) -> Self {
        Self {
            program,
            proof,
            public_inputs,
        }
    }

    /// The Layer-0 composite trace height (the preprocessed program has one row
    /// per trace row, so its height IS the trace length). Callers use this to
    /// pick the degree-adaptive FRI profile (`CircuitConfig::for_layer0_trace`)
    /// consistently on the prove side.
    pub fn trace_height(&self) -> usize {
        use p3_matrix::Matrix;
        self.program.height()
    }
}

struct CompactBatchL2Prep {
    l1_metadata: p3_circuit_prover::GoldilocksTip5BatchStarkProofMetadata,
    verification_circuit: p3_circuit::Circuit<Challenge>,
    verifier_inputs: BatchStarkVerifierInputsBuilder<
        p3_circuit_prover::config::GoldilocksTipsConfig,
        CompactBatchL2Comm,
        CompactBatchL2InnerFri,
    >,
    mmcs_op_ids: Vec<NonPrimitiveOpId>,
    circuit_prover_data: std::sync::Arc<
        p3_circuit_prover::CircuitProverData<p3_circuit_prover::config::GoldilocksBlake3Config>,
    >,
    prover: p3_circuit_prover::BatchStarkProver<p3_circuit_prover::config::GoldilocksBlake3Config>,
    l2_statement_public_binding_lanes: usize,
}

/// Reusable prover-side setup for the compact final-layer batch-STARK route.
///
/// This cache owns L1 prover setup when it was built by a full compact run, plus
/// L2 verifier-circuit targets, AIR setup, preprocessed prover data, and
/// table-prover registration for a fixed L1 proof shape. It is not a wire
/// artifact and is not accepted from miners. The compact certificate still
/// carries only a verifier-key/setup digest, and verification still requires
/// verifier-owned context.
pub struct AiPowCompactBatchProverCache {
    l1_prep: Option<CompactBatchL1Prep>,
    l2_prep: CompactBatchL2Prep,
}

impl AiPowCompactBatchProverCache {
    pub const fn l2_statement_public_binding_lanes(&self) -> usize {
        self.l2_prep.l2_statement_public_binding_lanes
    }
}

/// Build reusable compact-L2 prover setup from a representative canonical L1
/// recursive certificate.
///
/// The cache is guarded against stale L1 metadata before use, so a cache built
/// for a different L1 shape rejects instead of silently proving against the
/// wrong verifier circuit.
#[doc(hidden)]
pub fn build_compact_batch_prover_cache_from_l1_certificate(
    l1_cert: &AiPowRecursiveCertificate,
) -> Result<AiPowCompactBatchProverCache, VerificationError> {
    Ok(AiPowCompactBatchProverCache {
        l1_prep: None,
        l2_prep: build_compact_batch_l2_over_l1_prep(l1_cert.l1_outer_proof())?,
    })
}

/// Produce a recursive AI-PoW certificate from bridge-verified Layer-0
/// proof parts.
///
/// This function recursively verifies the Layer-0 proof in-circuit and
/// returns only the recursive L1 certificate. It does not serialize,
/// persist, or bless the Layer-0 proof as a block artifact.
#[doc(hidden)]
pub fn prove_recursive_certificate_from_chain_verified_composite_proof(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: ChainVerifiedCompositeProof<'_>,
) -> Result<L1CertificateRun, VerificationError> {
    use std::time::Instant;

    let cfg = crate::composite_proof::build_config(zk_params, profile);
    let t = Instant::now();
    // §6(b)-R-b: the L1 verifier circuit must be built over the SAME AIR
    // keystone flag the L0 proof used — `sx_bound = false` for the R-b
    // stripe-major path (`num_stripes > STRIPE_MAX`), `true` otherwise.
    // Derived from the trusted (verified) params; matches the compact path.
    let sx_bound =
        (zk_params.k / zk_params.noise_rank) as usize <= crate::composite_layout::STRIPE_MAX;
    let air = CompositeFullAirWithLookupsPinned::new_with(verified.program.clone(), sx_bound);
    let pd = crate::composite_proof::logup_common_for(&cfg, &verified.program, sx_bound);
    let built = build_composite_l1_verifier_circuit(
        &cfg,
        &air,
        &verified.proof,
        &pd.common,
        &verified.public_inputs.to_vec(),
        profile,
    )?;
    let l1_circuit_build_ms = t.elapsed().as_millis();

    let t = Instant::now();
    run_composite_l1_verifier(&built, &verified.proof)?;
    let l1_in_circuit_verify_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let l1_outer_proof = prove_composite_l1_outer_cert(&built, &verified.proof)?;
    let l1_cert = AiPowRecursiveCertificate::new(verified.proof, verified.program, l1_outer_proof);
    let l1_outer_cert_ms = t.elapsed().as_millis();

    Ok(L1CertificateRun {
        l1_circuit_build_ms,
        l1_in_circuit_verify_ms,
        l1_outer_cert_ms,
        l1_cert,
    })
}

fn build_compact_batch_l2_over_l1_prep(
    l1: &AiPowL1OuterProof,
) -> Result<CompactBatchL2Prep, VerificationError> {
    use p3_batch_stark::ProverData;
    use p3_circuit_prover::common::{get_airs_and_degrees_with_prep, NpoPreprocessor};
    use p3_circuit_prover::{
        recompose_air_builders, strip_public_binding_for_lookup_metadata, tip5_air_builders,
        BatchStarkProver, CircuitProverData, ConstraintProfile, RecomposePreprocessor,
        Tip5Preprocessor,
    };

    const TRACE_D: usize = 2;

    let l2_statement_public_binding_lanes = l1.public_binding_lanes * l1.ext_degree;
    if l2_statement_public_binding_lanes == 0 {
        return Err(VerificationError::InvalidProofShape(
            "compact batch L2 requires non-empty L1 public binding lanes".to_string(),
        ));
    }

    let mut circuit_builder = CircuitBuilder::<Challenge>::new();
    circuit_builder.enable_tip5_perm::<Tip5Goldilocks, _>(
        generate_tip5_trace::<Challenge, Tip5Goldilocks>, LiftTip5,
    );
    circuit_builder.enable_recompose::<Val>(generate_recompose_trace::<Val, Challenge>);
    circuit_builder.set_recompose_coeff_ctl_for_decompose_links(true);

    let lookup_gadget = LogUpGadget::new();
    let l1_table_provers = tip5_recompose_table_provers_for_compact_l2();
    let (verifier_inputs, mmcs_op_ids) = p3_recursion::verifier::verify_p3_batch_proof_circuit::<
        p3_circuit_prover::config::GoldilocksTipsConfig,
        CompactBatchL2Comm,
        CompactBatchL2InputProof,
        CompactBatchL2InnerFri,
        LogUpGadget,
        Tip5Config,
        WIDTH,
        RATE,
        TRACE_D,
    >(
        &compact_batch_l1_stark_config(),
        &mut circuit_builder,
        l1,
        &compact_batch_l1_fri_verifier_params(),
        &l1.stark_common,
        &lookup_gadget,
        Tip5Config::GOLDILOCKS_W16,
        &l1_table_provers,
    )
    .map_err(|e| {
        VerificationError::InvalidProofShape(format!(
            "compact batch L2 verifier circuit over L1 proof failed: {e:?}"
        ))
    })?;

    let verification_circuit = circuit_builder.build()?;
    let l2_table_packing = compact_batch_l2_table_packing(l2_statement_public_binding_lanes);
    let npo_prep: Vec<Box<dyn NpoPreprocessor<Val>>> =
        vec![Box::new(Tip5Preprocessor), Box::new(RecomposePreprocessor::new(true))];
    let mut air_builders =
        tip5_air_builders::<p3_circuit_prover::config::GoldilocksBlake3Config, 2>();
    air_builders.extend(recompose_air_builders::<
        p3_circuit_prover::config::GoldilocksBlake3Config,
        2,
    >(COMPACT_BATCH_L2_RECOMPOSE_LANES, true));

    let (airs_degrees, primitive_columns, non_primitive_columns) =
        get_airs_and_degrees_with_prep::<
            p3_circuit_prover::config::GoldilocksBlake3Config,
            Challenge,
            2,
        >(
            &verification_circuit,
            &l2_table_packing,
            &npo_prep,
            &air_builders,
            ConstraintProfile::Standard,
        )
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch L2 AIR setup failed: {e:?}"
            ))
        })?;
    let (airs, degrees): (Vec<_>, Vec<usize>) = airs_degrees.into_iter().unzip();
    let lookup_metadata_airs = airs
        .iter()
        .map(strip_public_binding_for_lookup_metadata)
        .collect::<Vec<_>>();
    let prover_data = ProverData::from_airs_and_degrees(
        &compact_batch_l2_stark_config(),
        &lookup_metadata_airs,
        &degrees,
    );
    let circuit_prover_data = std::sync::Arc::new(CircuitProverData::new(
        prover_data, primitive_columns, non_primitive_columns,
    ));
    let mut prover =
        BatchStarkProver::new(compact_batch_l2_stark_config()).with_table_packing(l2_table_packing);
    prover.register_tip5_table::<2>(Tip5Config::GOLDILOCKS_W16);
    prover.register_recompose_table::<2>(true);

    Ok(CompactBatchL2Prep {
        l1_metadata: p3_circuit_prover::GoldilocksTip5BatchStarkProofMetadata::from_proof(l1),
        verification_circuit,
        verifier_inputs,
        mmcs_op_ids,
        circuit_prover_data,
        prover,
        l2_statement_public_binding_lanes,
    })
}

fn compact_batch_l1_metadata_matches_cached_prep(
    cached: &p3_circuit_prover::GoldilocksTip5BatchStarkProofMetadata,
    l1: &AiPowL1OuterProof,
) -> bool {
    let current = p3_circuit_prover::GoldilocksTip5BatchStarkProofMetadata::from_proof(l1);
    cached.table_packing == current.table_packing
        && cached.public_binding_lanes == current.public_binding_lanes
        && cached.rows == current.rows
        && cached.alu_variant == current.alu_variant
        && cached.ext_degree == current.ext_degree
        && cached.w_binomial == current.w_binomial
        && cached.alu_quintic_trinomial == current.alu_quintic_trinomial
        && non_primitive_metadata_eq(&cached.non_primitives, &current.non_primitives)
        && p3_circuit_prover::common_preprocessed_binding_eq(
            &cached.stark_common, &current.stark_common,
        )
}

fn ensure_compact_batch_l2_prep_matches_l1(
    prep: &CompactBatchL2Prep,
    l1: &AiPowL1OuterProof,
) -> Result<(), VerificationError> {
    let expected_lanes = l1.public_binding_lanes * l1.ext_degree;
    if prep.l2_statement_public_binding_lanes != expected_lanes {
        return Err(VerificationError::InvalidProofShape(format!(
            "compact batch L2 prep public binding lane mismatch: prep has {}, L1 proof has {}",
            prep.l2_statement_public_binding_lanes, expected_lanes
        )));
    }
    if !compact_batch_l1_metadata_matches_cached_prep(&prep.l1_metadata, l1) {
        return Err(VerificationError::InvalidProofShape(
            "compact batch L2 prep was built for a different L1 proof metadata/setup shape"
                .to_string(),
        ));
    }
    Ok(())
}

/// Return whether a compact-recursion error came from trying to reuse L2
/// prover setup against a different L1 proof shape.
///
/// Callers may use this to discard a stale prover cache and rebuild setup. This
/// must not be treated as proof acceptance: the stale cache was rejected before
/// L2 proving.
pub fn is_compact_batch_prover_cache_mismatch(error: &VerificationError) -> bool {
    let VerificationError::InvalidProofShape(message) = error else {
        return false;
    };
    message.contains("compact batch L1 prep table-packing mismatch")
        || message.contains(
            "compact batch L1 prep was built for a different verifier circuit/setup shape",
        )
        || message.contains("compact batch L2 prep public binding lane mismatch")
        || message.contains(
            "compact batch L2 prep was built for a different L1 proof metadata/setup shape",
        )
}

fn prove_compact_batch_l2_with_prep(
    prep: &CompactBatchL2Prep,
    l1: &AiPowL1OuterProof,
    statement_digest_public_values: &[Val],
) -> Result<AiPowL2FinalProof, VerificationError> {
    ensure_compact_batch_l2_prep_matches_l1(prep, l1)?;
    let l1_public_values =
        compact_batch_l2_public_values_for_l1(l1, statement_digest_public_values)?;
    if statement_digest_public_values.len() != prep.l2_statement_public_binding_lanes {
        return Err(VerificationError::InvalidProofShape(format!(
            "compact batch L2 prep public binding lane mismatch: prep has {}, proof statement has {}",
            prep.l2_statement_public_binding_lanes,
            statement_digest_public_values.len()
        )));
    }
    let (public_inputs, private_inputs) = prep
        .verifier_inputs
        .pack_values(&l1_public_values, &l1.proof, &l1.stark_common);

    let mut runner = prep.verification_circuit.runner();
    runner
        .set_public_inputs(&public_inputs)
        .map_err(VerificationError::Circuit)?;
    runner
        .set_private_inputs(&private_inputs)
        .map_err(VerificationError::Circuit)?;
    set_fri_mmcs_private_data::<
        Val,
        Challenge,
        CompactBatchL2ChallengeMmcs,
        CompactBatchL2ValMmcs,
        CompactBatchL2Hash,
        CompactBatchL2Compress,
        DIGEST_ELEMS,
    >(
        &mut runner,
        &prep.mmcs_op_ids,
        &l1.proof.opening_proof,
        Tip5Config::GOLDILOCKS_W16,
    )
    .map_err(|e| VerificationError::InvalidProofShape(e.to_string()))?;
    let traces = runner.run().map_err(VerificationError::Circuit)?;
    prep.prover
        .prove_all_tables(&traces, prep.circuit_prover_data.as_ref())
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch L2 prove_all_tables failed: {e:?}"
            ))
        })
}

/// Produce the compact final-layer batch-STARK recursive candidate from
/// bridge-verified Layer-0 proof parts.
///
/// This implements the committed compact L2 route with pure-query 60-bit
/// parameters and no proof-system PoW grinding: L1 `lb=3,nq=20`, L2
/// `lb=5,nq=12`. The returned verifier context is for local verification and
/// verifier-key integration work; it is not part of the certificate wire body.
pub fn prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: &ChainVerifiedCompositeProof<'_>,
) -> Result<CompactBatchCertificateRun, VerificationError> {
    prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_inner(
        zk_params, profile, verified, None, true,
    )
}

/// §6(b)-R-b — explicit-`sx_bound` variant. The R-b stripe-major path
/// proves the composite with `sx_bound = false` (the SX 64-lane keystone
/// is inactive; the §6(b)-R-b keystone binds the fold input), so the
/// recursion's L1 verifier circuit must be built over the same AIR flag.
pub fn prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_sx(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: &ChainVerifiedCompositeProof<'_>,
    sx_bound: bool,
) -> Result<CompactBatchCertificateRun, VerificationError> {
    prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_inner(
        zk_params, profile, verified, None, sx_bound,
    )
}

/// Cached-setup variant of
/// [`prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof`].
///
/// This skips compact-L1 prover setup when present and matching, and skips
/// compact-L2 verifier/AIR setup when the supplied cache matches the freshly
/// produced L1 proof shape. The cache is verifier/prover setup only; it does
/// not weaken the certificate binding because the final compact body is still
/// checked against a verifier-key/setup digest and verifier-owned context.
pub fn prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: &ChainVerifiedCompositeProof<'_>,
    cache: &AiPowCompactBatchProverCache,
) -> Result<CompactBatchCertificateRun, VerificationError> {
    prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_inner(
        zk_params,
        profile,
        verified,
        Some(cache),
        true,
    )
}

/// §6(b)-R-b — cached-setup variant with an explicit `sx_bound`. The
/// R-b stripe-major L0 proof (`sx_bound = false`) requires the L1
/// verifier circuit be built over the same AIR flag; the prover cache
/// is setup-only and does not affect the binding.
pub fn prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_with_prover_cache_sx(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: &ChainVerifiedCompositeProof<'_>,
    cache: &AiPowCompactBatchProverCache,
    sx_bound: bool,
) -> Result<CompactBatchCertificateRun, VerificationError> {
    prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_inner(
        zk_params,
        profile,
        verified,
        Some(cache),
        sx_bound,
    )
}

fn prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_inner(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    verified: &ChainVerifiedCompositeProof<'_>,
    prover_cache: Option<&AiPowCompactBatchProverCache>,
    sx_bound: bool,
) -> Result<CompactBatchCertificateRun, VerificationError> {
    use std::time::Instant;

    let cfg = crate::composite_proof::build_config(zk_params, profile);
    let t = Instant::now();
    let air = CompositeFullAirWithLookupsPinned::new_with(verified.program.clone(), sx_bound);
    let pd = crate::composite_proof::logup_common_for(&cfg, &verified.program, sx_bound);
    let built = build_composite_l1_verifier_circuit(
        &cfg,
        &air,
        &verified.proof,
        &pd.common,
        &verified.public_inputs.to_vec(),
        profile,
    )?;
    let l1_circuit_build_ms = t.elapsed().as_millis();
    let statement_digest_public_values = compact_batch_l1_public_values_from_built(&built);

    let t = Instant::now();
    let mut owned_l1_prep = None;
    let l1_prep = if let Some(cached) = prover_cache.and_then(|cache| cache.l1_prep.as_ref()) {
        ensure_compact_batch_l1_prep_matches_built(cached, &built)?;
        cached
    } else {
        owned_l1_prep = Some(build_compact_batch_l1_prep(&built)?);
        owned_l1_prep
            .as_ref()
            .expect("owned L1 prep was just initialized")
    };
    let l1_outer_proof = prove_compact_batch_l1_with_prep(&built, &verified.proof, l1_prep)?;
    let l1_outer_cert_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let mut owned_l2_prep = None;
    let l2_prep = if let Some(cached) = prover_cache {
        ensure_compact_batch_l2_prep_matches_l1(&cached.l2_prep, &l1_outer_proof)?;
        &cached.l2_prep
    } else {
        owned_l2_prep = Some(build_compact_batch_l2_over_l1_prep(&l1_outer_proof)?);
        owned_l2_prep
            .as_ref()
            .expect("owned L2 prep was just initialized")
    };
    let l2_prep_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let l2_proof = prove_compact_batch_l2_with_prep(
        l2_prep, &l1_outer_proof, &statement_digest_public_values,
    )?;
    let l2_prove_ms = t.elapsed().as_millis();

    let l2_statement_public_values =
        compact_batch_l2_statement_public_values_for_l1(&statement_digest_public_values);
    let l2_metadata =
        p3_circuit_prover::GoldilocksBlake3BatchStarkProofMetadata::from_proof(&l2_proof);
    let l2_fri_shape = compact_batch_l2_fri_shape();
    let verifier_key_digest =
        compact_batch_verifier_key_digest_from_parts(&l2_metadata, l2_fri_shape).map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch verifier-key digest construction failed: {e:?}"
            ))
        })?;
    let t = Instant::now();
    let l2_compact = l2_prep
        .prover
        .compact_goldilocks_blake3_path_pruned_preprocessed_with_public_values(
            l2_proof,
            &l2_statement_public_values,
            l2_prep.circuit_prover_data.as_ref(),
            l2_fri_shape,
        )
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch L2 body construction failed: {e:?}"
            ))
        })?;
    let l2_compact_ms = t.elapsed().as_millis();

    let compact_cert =
        AiPowCompactBatchRecursiveCertificate::new(verifier_key_digest, l2_compact.into_body());
    let verifier_context = AiPowCompactBatchVerifierContext {
        verifier_key_digest,
        metadata: l2_metadata,
        circuit_prover_data: std::sync::Arc::clone(&l2_prep.circuit_prover_data),
        fri_shape: l2_fri_shape,
    };

    let verify_bytes = encode_compact_batch_recursive_certificate(&compact_cert).map_err(|e| {
        VerificationError::InvalidProofShape(format!(
            "compact batch recursive certificate encoding failed: {e:?}"
        ))
    })?;
    let verify_cert = decode_compact_batch_recursive_certificate(&verify_bytes).map_err(|e| {
        VerificationError::InvalidProofShape(format!(
            "compact batch recursive certificate decoding failed: {e:?}"
        ))
    })?;
    let t = Instant::now();
    // Self-verify sanity: fold the prover's own L0 program commitment (equals the
    // canonical one for an honest prover). The node passes the canonical commitment
    // via `canonical_l0_program_commitment_vals` instead.
    verify_compact_batch_recursive_certificate_with_context(
        &verifier_context,
        verify_cert,
        verified.public_inputs,
        &l0_program_commitment_vals(&pd.common),
    )?;
    let l2_compact_verify_ms = t.elapsed().as_millis();

    Ok(CompactBatchCertificateRun {
        l1_circuit_build_ms,
        l1_outer_cert_ms,
        l2_prep_ms,
        l2_prove_ms,
        l2_compact_ms,
        l2_compact_verify_ms,
        compact_cert,
        verifier_context,
        l1_outer_proof,
        prover_cache: owned_l2_prep.map(|l2_prep| AiPowCompactBatchProverCache {
            l1_prep: owned_l1_prep,
            l2_prep,
        }),
    })
}

/// Verify a compact final-layer batch-STARK certificate with verifier-owned
/// context.
///
/// The context must be derived from trusted verifier-key/setup state. The
/// statement-specific final public values are derived here from trusted
/// Layer-0 public inputs, not from the proof body.
pub fn verify_compact_batch_recursive_certificate_with_context(
    context: &AiPowCompactBatchVerifierContext,
    cert: AiPowCompactBatchRecursiveCertificate,
    public_inputs: &crate::composite_public::CompositePublicInputs,
    // P0/D6: the CANONICAL L0 program commitment (verifier-derived witness-free
    // via `canonical_l0_program_commitment_vals` from the program the node
    // rebuilds from the public opened schedule). Binds the opened schedule: a
    // certificate proven over a different program fails the statement-digest
    // check. Pass the prover's own for a self-verify sanity check.
    l0_program_commitment: &[Val],
) -> Result<(), VerificationError> {
    let expected_digest =
        compact_batch_verifier_key_digest_from_parts(&context.metadata, context.fri_shape)
            .map_err(|e| {
                VerificationError::InvalidProofShape(format!(
                    "compact batch verifier-key digest reconstruction failed: {e:?}"
                ))
            })?;
    if context.verifier_key_digest != expected_digest {
        return Err(VerificationError::InvalidProofShape(
            "compact batch verifier context digest does not match its metadata/FRI shape"
                .to_string(),
        ));
    }
    if cert.verifier_key_digest != expected_digest {
        return Err(VerificationError::InvalidProofShape(
            "compact batch certificate verifier-key digest does not match verifier context"
                .to_string(),
        ));
    }

    let expected_l2_packing = compact_batch_l2_table_packing(context.metadata.public_binding_lanes);
    if context.metadata.table_packing != expected_l2_packing {
        return Err(VerificationError::InvalidProofShape(format!(
            "compact batch verifier context uses table packing {:?}; expected {:?}",
            context.metadata.table_packing, expected_l2_packing
        )));
    }
    if context.fri_shape != compact_batch_l2_fri_shape() {
        return Err(VerificationError::InvalidProofShape(format!(
            "compact batch verifier context uses FRI shape {:?}; expected {:?}",
            context.fri_shape,
            compact_batch_l2_fri_shape()
        )));
    }

    let l1_statement_public_values = compact_batch_l1_public_values_for_statement(
        &public_inputs.to_vec(),
        l0_program_commitment,
    );
    let l2_statement_public_values =
        compact_batch_l2_statement_public_values_for_l1(&l1_statement_public_values);
    let compact_context = p3_circuit_prover::GoldilocksBlake3PathPrunedCompactVerifierContext::new(
        &context.metadata, &context.circuit_prover_data, context.fri_shape,
        &l2_statement_public_values,
    );
    let mut verifier = p3_circuit_prover::BatchStarkProver::new(compact_batch_l2_stark_config())
        .with_table_packing(expected_l2_packing);
    verifier.register_tip5_table::<2>(Tip5Config::GOLDILOCKS_W16);
    verifier.register_recompose_table::<2>(true);
    verifier
        .verify_goldilocks_blake3_path_pruned_preprocessed_compact_body_with_context(
            cert.l2_compact_body, compact_context,
        )
        .map_err(|e| {
            VerificationError::InvalidProofShape(format!(
                "compact batch recursive certificate verification failed: {e:?}"
            ))
        })
}

pub fn encode_compact_batch_recursive_certificate(
    cert: &AiPowCompactBatchRecursiveCertificate,
) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(cert)
}

pub fn decode_compact_batch_recursive_certificate(
    bytes: &[u8],
) -> Result<AiPowCompactBatchRecursiveCertificate, postcard::Error> {
    let (cert, trailing): (AiPowCompactBatchRecursiveCertificate, &[u8]) =
        postcard::take_from_bytes(bytes)?;
    if !trailing.is_empty() {
        return Err(postcard::Error::DeserializeUnexpectedEnd);
    }
    Ok(cert)
}

/// Produce the hardened batch-STARK recursive AI-PoW checkpoint certificate.
///
/// This is a name-level guardrail against raw Layer-0 proof submission: the
/// returned certificate is recursive and cryptographically verifies the L1
/// verifier-circuit proof body. It is not the selected compact batch-STARK
/// artifact, because the full checkpoint certificate is too large for the wire
/// budget.
/// Consensus callers must separately derive the exact public statement and
/// reject selected-tile statements that do not prove the intended full-matmul
/// work unit.
#[doc(hidden)]
pub fn prove_canonical_ai_pow_certificate(
    zk_params: &crate::params::ZkParams,
    profile: &crate::circuit::CircuitConfig,
    trace: crate::composite_trace::CompositeTrace,
) -> Result<L1RecursionRun, VerificationError> {
    recurse_composite_to_l1(zk_params, profile, trace)
}

/// Serialize the batch-STARK recursive AI-PoW checkpoint certificate into
/// compact bytes.
///
/// This serializes the batch-STARK structured recursive checkpoint, including
/// the Layer-0 proof/program context needed to rebuild the L1 verifier circuit.
/// It does not accept or produce a standalone Layer-0 `AiPowBatchProof`,
/// because raw Layer-0 proofs are not block/wire certificates for Nockchain
/// AI-PoW. This helper is not the compact final-layer batch-STARK production
/// candidate.
#[doc(hidden)]
pub fn encode_recursive_certificate(
    cert: &AiPowRecursiveCertificate,
) -> Result<Vec<u8>, bincode::error::EncodeError> {
    bincode::serde::encode_to_vec(cert, bincode::config::standard().with_fixed_int_encoding())
}

/// Decode bytes previously produced by [`encode_recursive_certificate`].
///
/// Decoding is structural only; callers still need to verify the certificate
/// against chain-derived statement data once the verifier is wired.
#[doc(hidden)]
pub fn decode_recursive_certificate(
    bytes: &[u8],
) -> Result<AiPowRecursiveCertificate, bincode::error::DecodeError> {
    let (cert, consumed) = bincode::serde::decode_from_slice(
        bytes,
        bincode::config::standard().with_fixed_int_encoding(),
    )?;
    if consumed != bytes.len() {
        return Err(bincode::error::DecodeError::OtherString(format!(
            "recursive certificate decode left {} trailing bytes",
            bytes.len() - consumed
        )));
    }
    Ok(cert)
}

/// S3a — compile-time proof that the composite AIR satisfies the
/// recursion substrate's `RecursiveAir` bound.
fn _require_recursive_air<A>()
where
    A: RecursiveAir<Val, Challenge, LogUpGadget>,
{
}

#[allow(dead_code)]
fn _composite_conforms_to_recursive_air() {
    _require_recursive_air::<CompositeFullAirWithLookupsPinned>();
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::composite_proof::{
        build_config, composite_prove_pinned_logup, composite_prove_pinned_logup_sx,
        logup_common_for,
    };
    use crate::composite_public::CompositePublicInputs;
    use crate::composite_trace::CompositeTrace;
    use crate::params::ZkParams;
    use crate::CircuitConfig;

    fn test_zk_params() -> ZkParams {
        ZkParams {
            m: 8,
            k: 16,
            n: 8,
            noise_rank: 2,
            tile: 2,
            difficulty_bits: 0,
        }
    }

    /// §6(b)-R-b — the FULL COMPACT-CERT path for num_stripes > 64. Builds a
    /// stripe-major R-b composite trace (num_stripes=96, past STRIPE_MAX=64) +
    /// its positioned producer store, proves the L0 with `sx_bound=false`, then
    /// runs the FULL recursion (`_sx` variant, sx_bound=false) to a compact
    /// batch recursive certificate, and verifies the decoded certificate
    /// against its verifier context + the canonical L0 program commitment.
    /// This is the consensus-path validation: R-b verifies end-to-end through
    /// the compact cert at num_stripes > 64.
    #[test]
    fn rb_compact_batch_recursive_certificate_at_num_stripes_over_64() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let ch: [u32; 8] = core::array::from_fn(|i| 0xB100 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 96usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline(1 << 14);
        let h = trace.height();
        let (rows_used, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        let store_chunks = CompositeTrace::enumerate_noised_chunks_positioned(
            &a_prime, &b_prime, t, r, num_stripes,
        );
        let a_id_base = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((t * k).div_ceil(8)) as u64;
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(8 + rows_used + i, &chunk.bytes, mat_id);
        }
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        // L0 proof with sx_bound = FALSE (R-b path).
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        let verified = unsafe {
            ChainVerifiedCompositeProof::from_parts_after_chain_statement_verification(
                program, proof, &pis,
            )
        };
        // Full recursion with sx_bound = FALSE.
        let run = prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof_sx(
            &zk, &profile, &verified, false,
        )
        .expect("R-b compact batch recursive certificate must prove");

        let bytes = encode_compact_batch_recursive_certificate(&run.compact_cert)
            .expect("encode R-b compact cert");
        let decoded =
            decode_compact_batch_recursive_certificate(&bytes).expect("decode R-b compact cert");
        let commit = canonical_l0_program_commitment_vals(&zk, &profile, &verified.program);
        assert!(
            !commit.is_empty(),
            "L0 program must have a preprocessed commitment"
        );
        verify_compact_batch_recursive_certificate_with_context(
            &run.verifier_context, decoded, &pis, &commit,
        )
        .expect("decoded R-b compact cert (num_stripes 96) must verify");

        // Boot-setup serialization (Stage C1/C2): the verifier context survives a
        // serialize → deserialize round-trip (the prover-only PCS data is dropped
        // and reconstructed EMPTY) and STILL verifies the cert — proving the
        // cached/embedded setup is sound and the verifier never needs prover-only
        // data. This is the linchpin of the fast-boot cached setup table.
        // Boot-setup CACHE (Stage C3', the size-practical form): the compact
        // verifier is path-pruned, so the CONTEXT carries the preprocessed Merkle
        // tree (~866 MB/bucket — absurd to cache). Instead the boot table caches
        // the SMALL (l1_outer_proof, metadata) and REBUILDS the context at boot via
        // `rebuild_compact_verifier_context` (no proving). Here we lock in the
        // SIZE win: the cached (l1_proof, metadata) is < 8 MiB (vs 866 MB).
        let small = bincode::serde::encode_to_vec(
            (&run.l1_outer_proof, run.verifier_context.metadata()),
            bincode::config::standard(),
        )
        .expect("serialize (l1_outer_proof, metadata)");
        assert!(
            small.len() < 8 * 1024 * 1024,
            "cached (l1_proof, metadata) must be small (< 8 MiB); got {} bytes",
            small.len()
        );

        // Boot-setup REBUILD (Stage C3', end-to-end): round-trip the SMALL cached
        // blob (l1_outer_proof, metadata) through serde, then rebuild the FULL
        // compact verifier context from the small cache + the (canonical, small) L0
        // program/proof/PIs — NO proving. The rebuild reconstructs the L1
        // `CommonData.lookups` (content-exact, via `Lookups::from_air`) that serde
        // drops, and rebuilds the ~866 MB preprocessed tree in memory. The decoded
        // cert MUST verify against the REBUILT context exactly as against the
        // freshly-proved one — this is the fast-boot cached-setup linchpin.
        let (l1_rt, meta_rt): (AiPowL1OuterProof, _) =
            bincode::serde::decode_from_slice(&small, bincode::config::standard())
                .expect("deserialize (l1_outer_proof, metadata)")
                .0;
        let rebuilt_ctx = rebuild_compact_verifier_context(
            &zk, &profile, &verified.program, &verified.proof, &pis, false, l1_rt, meta_rt,
        )
        .expect("rebuild compact verifier context from SMALL cache (no proving)");
        let decoded_again =
            decode_compact_batch_recursive_certificate(&bytes).expect("re-decode R-b compact cert");
        verify_compact_batch_recursive_certificate_with_context(
            &rebuilt_ctx, decoded_again, &pis, &commit,
        )
        .expect("R-b compact cert must verify against the REBUILT (cached-setup) context");
    }

    /// S3d — end-to-end: a real composite batch-STARK proof is
    /// recursively verified in-circuit by the L1 recursion verifier,
    /// and the verifier circuit **accepts**.
    ///
    /// Proves a real honest composite proof
    /// (`composite_prove_pinned_logup` over `baseline_min`), builds the
    /// L1 recursive-verifier circuit via
    /// `build_composite_l1_verifier_circuit`, and runs it. This is the
    /// `ai-pow-zk` ↔ `Plonky3-recursion` integration end-to-end:
    /// `runner.run()` succeeding means the in-circuit FRI / Tip5
    /// challenger / MMCS recompute accepted the composite proof.
    ///
    /// (Both sides use 5-round Tip5 — see `circuit::Tip5Perm` and the
    /// `Plonky3-recursion` `tip5-circuit-air`.)
    #[test]
    fn composite_recursively_verified_l1_accepts() {
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&test_zk_params(), &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        // `composite_prove_pinned_logup` extracts + returns the
        // canonical program (CRIT-1 pin); the verifier uses it.
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);

        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);

        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build the composite L1 verifier circuit");

        run_composite_l1_verifier(&built, &proof)
            .expect("L1 recursive verification of the real composite proof must accept");
    }

    /// S5 — build a real composite proof, recursively verify it in the
    /// L1 circuit, and outer-prove that verifier circuit as a D=2
    /// batch-STARK (the L1 recursive certificate). When `tamper`, one
    /// FRI-bound opened OOD trace evaluation of the composite proof is
    /// corrupted before the L1 circuit is built — the in-circuit
    /// quotient-consistency recompute must then reject. Returns the
    /// serialized certificate byte length on accept.
    fn run_composite_l1_outer_cert(tamper: bool) -> Result<usize, String> {
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&test_zk_params(), &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (mut proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);

        if tamper {
            // Corrupt a single FRI-bound opened OOD trace evaluation.
            proof.opened_values.instances[0]
                .base_opened_values
                .trace_local[0] += Challenge::ONE;
        }

        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);

        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .map_err(|e| format!("build composite L1 verifier circuit: {e:?}"))?;

        let cert = prove_composite_l1_outer_cert(&built, &proof).map_err(|e| format!("{e:?}"))?;
        let bytes =
            postcard::to_allocvec(&cert).map_err(|e| format!("serialize L1 certificate: {e}"))?;
        Ok(bytes.len())
    }

    /// S5 ACCEPT: an honest composite proof yields a valid L1 outer
    /// certificate that `verify_all_tables` (the cross-table
    /// `WitnessChecks` soundness gate) accepts.
    #[test]
    fn composite_l1_outer_cert_accepts() {
        match run_composite_l1_outer_cert(false) {
            Ok(bytes) => eprintln!(
                "[S5] composite→L1 outer certificate ACCEPTED — serialized {} bytes ({:.2} KB)",
                bytes,
                bytes as f64 / 1024.0,
            ),
            Err(e) => panic!("valid composite→L1 outer certificate was REJECTED: {e}"),
        }
    }

    #[test]
    fn compact_batch_verifier_key_digest_bytes_are_canonical() {
        let digest = [
            Val::from_u64(1),
            Val::from_u64(2),
            Val::from_u64(3),
            Val::from_u64(4),
            Val::from_u64(5),
        ];
        let bytes = compact_batch_verifier_key_digest_to_bytes(&digest);
        assert_eq!(bytes.len(), AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES);
        let decoded = compact_batch_verifier_key_digest_from_bytes(&bytes)
            .expect("canonical digest bytes decode");
        assert_eq!(decoded, digest);

        let err = compact_batch_verifier_key_digest_from_bytes(&bytes[..39])
            .expect_err("short verifier-key digest bytes must reject");
        assert!(matches!(
            err,
            CompactBatchVerifierKeyDigestEncodingError::InvalidLength {
                expected: AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES,
                actual: 39
            }
        ));

        let mut noncanonical = bytes;
        noncanonical[0..8].copy_from_slice(&GOLDILOCKS_MODULUS.to_le_bytes());
        let err = compact_batch_verifier_key_digest_from_bytes(&noncanonical)
            .expect_err("noncanonical Goldilocks limb must reject");
        assert!(matches!(
            err,
            CompactBatchVerifierKeyDigestEncodingError::NonCanonicalLimb {
                index: 0,
                value: GOLDILOCKS_MODULUS
            }
        ));
    }

    #[test]
    #[ignore = "compact batch recursive certificate route is opt-in"]
    fn compact_batch_recursive_certificate_round_trip_for_test_pearl() {
        use std::time::Instant;

        assert_eq!(
            COMPACT_BATCH_L1_LOG_BLOWUP * COMPACT_BATCH_L1_NUM_QUERIES,
            60
        );
        assert_eq!(
            COMPACT_BATCH_L2_LOG_BLOWUP * COMPACT_BATCH_L2_NUM_QUERIES,
            60
        );
        assert_eq!(
            p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
            0
        );
        assert_eq!(
            p3_circuit_prover::config::GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
            0
        );

        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let verified = unsafe {
            ChainVerifiedCompositeProof::from_parts_after_chain_statement_verification(
                program, proof, &pis,
            )
        };

        let prove_start = Instant::now();
        let run = prove_compact_batch_recursive_certificate_from_chain_verified_composite_proof(
            &zk, &profile, &verified,
        )
        .expect("compact batch recursive certificate must prove");
        let prove_wall_ms = prove_start.elapsed().as_millis();

        let bytes = encode_compact_batch_recursive_certificate(&run.compact_cert)
            .expect("encode compact batch recursive certificate");
        let decoded = decode_compact_batch_recursive_certificate(&bytes)
            .expect("decode compact batch recursive certificate");
        // P0/D6: the canonical L0 program commitment the node folds into the
        // statement digest (here == the prover's, honest program).
        let commit = canonical_l0_program_commitment_vals(&zk, &profile, &verified.program);
        assert!(
            !commit.is_empty(),
            "L0 program must have a preprocessed commitment"
        );
        verify_compact_batch_recursive_certificate_with_context(
            &run.verifier_context, decoded, &pis, &commit,
        )
        .expect("decoded compact batch recursive certificate must verify");

        // P0/D6 ADVERSARIAL: a certificate proven over one opened schedule must be
        // rejected when the node folds a DIFFERENT program's commitment — i.e. a
        // prover who opened a favorable strip cannot pass the canonical check.
        let mut wrong_commit = commit.clone();
        wrong_commit[0] += Val::ONE;
        let decoded_for_wrong_commit = decode_compact_batch_recursive_certificate(&bytes)
            .expect("decode for wrong-commitment test");
        verify_compact_batch_recursive_certificate_with_context(
            &run.verifier_context, decoded_for_wrong_commit, &pis, &wrong_commit,
        )
        .expect_err("must reject a cert whose L0 program commitment ≠ canonical (D6 binding)");

        let mut wrong_pis = pis.clone();
        wrong_pis.hash_jackpot[0] ^= 1;
        let wrong_decoded = decode_compact_batch_recursive_certificate(&bytes)
            .expect("decode compact batch recursive certificate for tamper test");
        verify_compact_batch_recursive_certificate_with_context(
            &run.verifier_context, wrong_decoded, &wrong_pis, &commit,
        )
        .expect_err("compact batch recursive certificate must reject wrong public inputs");

        let mut wrong_digest_cert = decode_compact_batch_recursive_certificate(&bytes)
            .expect("decode compact batch recursive certificate for digest test");
        wrong_digest_cert.verifier_key_digest[0] += Val::ONE;
        verify_compact_batch_recursive_certificate_with_context(
            &run.verifier_context, wrong_digest_cert, &pis, &commit,
        )
        .expect_err("compact batch recursive certificate must reject wrong verifier-key digest");

        let mut wrong_context = run.verifier_context;
        wrong_context.verifier_key_digest[0] += Val::ONE;
        let decoded_for_wrong_context = decode_compact_batch_recursive_certificate(&bytes)
            .expect("decode compact batch recursive certificate for context digest test");
        verify_compact_batch_recursive_certificate_with_context(
            &wrong_context, decoded_for_wrong_context, &pis, &commit,
        )
        .expect_err("compact batch recursive verifier must reject stale context digest");

        eprintln!(
            "compact batch recursive certificate route [TEST_PEARL]: cert={} bytes l1_build_ms={} l1_outer_ms={} l2_prep_ms={} l2_prove_ms={} l2_compact_ms={} l2_compact_verify_ms={} prove_wall_ms={}",
            bytes.len(),
            run.l1_circuit_build_ms,
            run.l1_outer_cert_ms,
            run.l2_prep_ms,
            run.l2_prove_ms,
            run.l2_compact_ms,
            run.l2_compact_verify_ms,
            prove_wall_ms,
        );

        assert!(
            bytes.len() < 150_000,
            "compact batch recursive certificate should remain inside the relaxed size gate"
        );
    }

    #[test]
    fn recursive_certificate_outer_verifier_accepts_honest_certificate() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let cert = AiPowRecursiveCertificate::new(proof, program, outer);

        verify_recursive_certificate(&cert, &zk, &profile, &pis)
            .expect("recursive certificate verifier must accept honest cert");
        verify_recursive_certificate_inner(&cert, &zk, &profile, &[])
            .expect_err("recursive verifier must reject empty statement public inputs");
    }

    #[test]
    fn recursive_certificate_fixed_bincode_round_trip_verifies() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let cert = AiPowRecursiveCertificate::new(proof, program, outer);

        let bytes = encode_recursive_certificate(&cert).expect("encode recursive certificate");
        let decoded = decode_recursive_certificate(&bytes).expect("decode recursive certificate");
        verify_recursive_certificate(&decoded, &zk, &profile, &pis)
            .expect("decoded recursive certificate must verify");

        let mut trailing = bytes;
        trailing.push(0);
        assert!(
            decode_recursive_certificate(&trailing).is_err(),
            "decoder must reject trailing bytes after certificate"
        );
    }

    #[test]
    fn recursive_certificate_outer_verifier_rejects_non_production_envelope() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let mut cert = AiPowRecursiveCertificate::new(proof, program, outer);

        cert.l1_outer_proof.ext_degree = 1;
        verify_recursive_certificate(&cert, &zk, &profile, &pis)
            .expect_err("recursive verifier must reject non-D=2 recursion envelope");
    }

    #[test]
    fn recursive_certificate_rejects_outer_circuit_metadata_tamper() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let mut cert = AiPowRecursiveCertificate::new(proof, program, outer);

        cert.l1_outer_proof.non_primitives.clear();
        verify_recursive_certificate(&cert, &zk, &profile, &pis)
            .expect_err("recursive verifier must reject non-canonical L1 circuit metadata");
    }

    #[test]
    fn recursive_certificate_rejects_outer_preprocessed_binding_tamper() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let mut cert = AiPowRecursiveCertificate::new(proof, program, outer);

        cert.l1_outer_proof.stark_common = CommonData::new(None, Vec::new());
        let err = verify_recursive_certificate(&cert, &zk, &profile, &pis)
            .expect_err("recursive verifier must reject non-canonical preprocessed binding");
        assert!(
            err.to_string().contains("preprocessed commitment"),
            "unexpected verifier error: {err}"
        );
    }

    #[test]
    fn recursive_certificate_rejects_outer_proof_body_tamper() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let mut cert = AiPowRecursiveCertificate::new(proof, program, outer);

        let first_opened_value = cert
            .l1_outer_proof
            .proof
            .opened_values
            .instances
            .get_mut(0)
            .and_then(|instance| instance.base_opened_values.trace_local.get_mut(0))
            .expect("outer proof exposes at least one trace opening");
        *first_opened_value += Val::ONE;

        verify_recursive_certificate(&cert, &zk, &profile, &pis)
            .expect_err("recursive verifier must reject tampered L1 proof body");
    }

    #[test]
    fn recursive_certificate_rejects_wrong_statement_public_inputs() {
        let zk = test_zk_params();
        let profile = CircuitConfig::TEST_PEARL;
        let cfg = build_config(&zk, &profile);

        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let pd = logup_common_for(&cfg, &program, true);
        let built = build_composite_l1_verifier_circuit(
            &cfg,
            &air,
            &proof,
            &pd.common,
            &pis.to_vec(),
            &profile,
        )
        .expect("build composite L1 verifier circuit");
        let outer =
            prove_composite_l1_outer_cert(&built, &proof).expect("honest recursive certificate");
        let cert = AiPowRecursiveCertificate::new(proof, program, outer);

        let mut wrong = pis.clone();
        wrong.job_key[0] ^= 1;
        verify_recursive_certificate(&cert, &zk, &profile, &wrong)
            .expect_err("recursive certificate must reject metadata-swapped public inputs");
    }

    /// S5 TAMPER-REJECT: a composite proof with one corrupted opened
    /// OOD trace value must NOT yield a certificate — the in-circuit
    /// FRI/quotient-consistency binding rejects it. A rejection via
    /// `Err` (in-circuit `WitnessConflict`) or a panic (debug
    /// assertion) both count; only a produced certificate fails.
    #[test]
    fn composite_l1_outer_cert_tamper_rejects() {
        let res = std::panic::catch_unwind(|| run_composite_l1_outer_cert(true));
        match res {
            Ok(Ok(bytes)) => panic!(
                "tampered composite→L1 outer certificate was ACCEPTED ({bytes} bytes) \
                 — SOUNDNESS FAILURE"
            ),
            Ok(Err(_)) | Err(_) => { /* rejected — correct */ }
        }
    }
}
