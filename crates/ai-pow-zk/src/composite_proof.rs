//! Lib-level prove/verify wrappers for the composite AIR.
//!
//! ## Entrypoints — three tiers (pick by use case)
//!
//! | Family | AIR | Prover | Use |
//! |---|---|---|---|
//! | [`composite_prove`] / [`composite_verify`] | `CompositeFullAir` (unpinned) | uni-stark | unit / constraint-logic dev only — **not sound for PoW** (a prover can zero selectors) |
//! | [`composite_prove_pinned`] / [`composite_verify_pinned`] / [`composite_verify_pow_pinned`] | `CompositeFullAirPinned` | uni-stark | program-pinned, **no LogUp**. Lighter; backs the `crit1_*` / `high2_*` constraint-logic regression suite. Not the production path (matmul reads unbound). |
//! | [`composite_prove_pinned_logup`] / [`composite_verify_pinned_logup`] / [`composite_verify_pow_pinned_logup`] | `CompositeFullAirWithLookupsPinned` | **batch-stark** | Layer-0 Route A. The canonical-program pin **and** the `noised_packed`/range/i8u8/cv-routing LogUp enforced in one proof. Used by [`ai-pow::zk_bridge`] and as the inner proof for the recursive certificate. ≈1.23x the uni-stark pinned cost. |
//!
//! New Layer-0 callers should use the **`*_pinned_logup`** family.
//! Nockchain production consensus callers should not stop at this
//! layer: the canonical block/wire certificate is the recursive L1
//! certificate produced by
//! `crate::recursion::prove_canonical_ai_pow_certificate`. The
//! uni-stark `*_pinned` family is retained as the lighter no-LogUp
//! variant + the home of the constraint-logic
//! adversarial suite. The unpinned `composite_prove`/`verify` is
//! dev-only and PoW-unsound.
//!
//! ## Trust model (all `*_pinned*` families)
//!
//! The verifier rebuilds the canonical `program` from the
//! trusted per-block shape (a pure function of `ctx`/`params`),
//! **never** from the proof, and checks the proof against that
//! program's preprocessed commitment. A forged trace whose
//! program differs is rejected. See `ai-pow::zk_bridge`.
//!
//! ## Public-input shape
//!
//! [`CompositePublicInputs`] — 60 field elements:
//! `cumsum(4) + jackpot(16) + hash_a(8) + hash_b(8) +
//! job_key(8) + commitment_hash(8) + hash_jackpot(8)`. The
//! cumsum/jackpot values are bound on the trace's last row; the
//! hash/key values are bound by selector-gated rows. See
//! [`crate::composite_public`] for the layout and the
//! `CompositePublicInputs::derive_from_trace` helper that snapshots
//! the values from a generated trace.

#[cfg(any(test, feature = "dev-unsafe"))]
use p3_uni_stark::{
    prove, prove_with_preprocessed, setup_preprocessed, verify, verify_with_preprocessed,
    PreprocessedProverData, PreprocessedVerifierKey, Proof,
};
use p3_uni_stark::{StarkGenericConfig, Val};
use thiserror::Error;

use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
use crate::composite_full_air::{extract_program, program_degree_bits, ProgramShapeError};
#[cfg(any(test, feature = "dev-unsafe"))]
use crate::composite_full_air::{CompositeFullAir, CompositeFullAirPinned};
use crate::composite_public::CompositePublicInputs;
use crate::composite_trace::CompositeTrace;
use crate::params::ZkParams;

/// Concrete STARK verification failures are stringified at this API boundary because
/// the uni-STARK and batch-STARK paths return different verifier error enums.
pub type StarkVerificationError = String;

#[derive(Debug, Error)]
pub enum CompositeVerificationError {
    #[error("invalid verifier program: {0}")]
    InvalidProgram(#[from] ProgramShapeError),
    #[error("stark verification failed: {0}")]
    Stark(StarkVerificationError),
}

/// Build the composite STARK config for the given parameters +
/// profile. Re-export of [`build_stark_config`] for ergonomics.
pub fn build_config(params: &ZkParams, profile: &CircuitConfig) -> AiPowStarkConfig {
    build_stark_config(params, profile)
}

/// Prove the composite AIR against a given trace + public inputs.
///
/// DEV-UNSAFE: this is compiled only for this crate's tests or with the
/// explicit `dev-unsafe` feature. It must not be used as a proof-of-work
/// verifier primitive because the unpinned AIR lets a malicious trace
/// disable selectors.
///
/// `trace` must be a [`CompositeTrace`] whose internal matrix has
/// width [`crate::composite_layout::TOTAL_TRACE_WIDTH`] and height
/// a power of 2 ≥ `MIN_STARK_LEN`. `public_inputs` must match the
/// trace's last-row CUMSUM_TILE / JACKPOT_MSG cells — the AIR
/// enforces this binding.
///
/// The returned [`Proof`] can be serialised via [`bincode`] for
/// transport.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_prove(
    config: &AiPowStarkConfig,
    trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
) -> Proof<AiPowStarkConfig> {
    let pis = public_inputs.to_vec();
    prove::<AiPowStarkConfig, _>(config, &CompositeFullAir, trace.matrix, &pis)
}

/// Verify a composite proof against the claimed public inputs.
///
/// DEV-UNSAFE: this is compiled only for this crate's tests or with the
/// explicit `dev-unsafe` feature. It verifies a local AIR statement, not a
/// sound proof of work.
///
/// Returns `Ok(())` if valid; otherwise a
/// [`CompositeVerificationError`] describing the failure.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_verify(
    config: &AiPowStarkConfig,
    proof: &Proof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
) -> Result<(), CompositeVerificationError> {
    let pis = public_inputs.to_vec();
    verify::<AiPowStarkConfig, _>(config, &CompositeFullAir, proof, &pis)
        .map_err(|e| CompositeVerificationError::Stark(format!("{e:?}")))
}

/// Encode the 8×u32 `HASH_JACKPOT` PI as a 32-byte little-endian
/// u256, byte-identical to a BLAKE3 digest (`bytes[4i..4i+4] =
/// word[i].to_le_bytes()`). Matches the encoding
/// `place_matrix_hash` uses (CV_OUT word i = LE u32 of digest
/// bytes 4i..4i+4), so the inverse reconstructs the digest.
pub fn hash_jackpot_le_bytes(hash_jackpot: &[u32; 8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&hash_jackpot[i].to_le_bytes());
    }
    out
}

/// 256-bit unsigned `hash <= target`, both little-endian 32-byte.
/// Identical comparison to `ai-pow::tile_hash::hash_le_target` —
/// kept local so `ai-pow-zk` stays standalone.
fn le_u256_le(hash: &[u8; 32], target: &[u8; 32]) -> bool {
    for k in (0..32).rev() {
        match hash[k].cmp(&target[k]) {
            core::cmp::Ordering::Less => return true,
            core::cmp::Ordering::Greater => return false,
            core::cmp::Ordering::Equal => continue,
        }
    }
    true
}

/// Error from a proof-of-work wrapper: either the STARK proof is invalid,
/// or it is valid but the proven `HASH_JACKPOT` does not clear the
/// difficulty target.
#[derive(Debug)]
pub enum PowVerifyError {
    /// The underlying STARK proof failed verification.
    Stark(CompositeVerificationError),
    /// STARK valid, but `HASH_JACKPOT > target` (tile not a winner).
    DifficultyNotMet,
}

impl core::fmt::Display for PowVerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PowVerifyError::Stark(e) => write!(f, "stark verification failed: {e}"),
            PowVerifyError::DifficultyNotMet => {
                write!(f, "HASH_JACKPOT does not clear the difficulty target")
            }
        }
    }
}

impl std::error::Error for PowVerifyError {}

/// C2 — full proof-of-work verification.
///
/// Pearl's Layer-0 STARK does **not** enforce the difficulty
/// inequality `BLAKE3(M, key=jackpot_key) ≤ target` in-circuit; it is
/// checked outside (block validation / higher recursion layers,
/// see `pearl_circuit.rs`). `ai-pow-zk` is a single STARK with no
/// recursion layers, so this wrapper performs the Pearl-equivalent
/// check after STARK verification, against the **bound**
/// `HASH_JACKPOT` public input (C4). Soundness rests on
/// HASH_JACKPOT being a selector-gated bound PI — the verifier
/// compares the *proven* tile-state keyed hash against `target`,
/// not an unconstrained claim. An in-AIR 256-bit comparator was
/// considered and rejected: it is strictly more than Pearl does
/// at Layer-0, costs a dedicated chip, and recursion (M12) would
/// absorb the external check anyway.
///
/// `target` is the 32-byte little-endian difficulty bound
/// (`ai-pow::tile_hash::difficulty_target` produces it).
///
/// ## Target-check obligation (caller-enforced)
///
/// `target` is **not** absorbed into the Fiat-Shamir transcript and
/// **not** an AIR public input (Pearl-Layer-0-faithful: difficulty
/// is external by design). This wrapper is therefore the
/// *unhardened primitive*: the difficulty bound is only meaningful
/// if the caller passes a `target` it **derived itself from the
/// chain-pinned params** (`difficulty_target(params)`) and never a
/// counterparty-supplied value. The canonical-program pin guarantees the other
/// precondition — `HASH_JACKPOT` is a genuinely bound PI.
/// Production callers MUST go through
/// `ai_pow::zk_bridge::prove_and_verify_for_block`, which recomputes
/// the target internally so it cannot be forged.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_verify_pow(
    config: &AiPowStarkConfig,
    proof: &Proof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    target: &[u8; 32],
) -> Result<(), PowVerifyError> {
    composite_verify(config, proof, public_inputs).map_err(PowVerifyError::Stark)?;
    let hj = hash_jackpot_le_bytes(&public_inputs.hash_jackpot);
    if le_u256_le(&hj, target) {
        Ok(())
    } else {
        Err(PowVerifyError::DifficultyNotMet)
    }
}

// ───────────────────────────────────────────────────────────────
//  Program-pinned prove / verify
//
//  The unit `composite_prove`/`composite_verify` above prove the
//  *unpinned* `CompositeFullAir` — a malicious prover can zero
//  every selector and forge a winning proof (no preprocessed
//  commitment ⇒ no verifier-fixed program). The pinned API below
//  commits `PROGRAM_COLS` as a *preprocessed* trace whose
//  commitment goes in the verifying key; the AIR forces the
//  prover's in-trace `*_PREP` cells to equal it. The verifier
//  rebuilds the canonical program from the trusted shape (never
//  from the proof) — see `ai-pow::zk_bridge`. This is the
//  production path.
// ───────────────────────────────────────────────────────────────

type Program = p3_matrix::dense::RowMajorMatrix<Val<AiPowStarkConfig>>;

fn checked_program_degree_bits(program: &Program) -> Result<usize, ProgramShapeError> {
    program_degree_bits(program)
}

/// Commit a program matrix as a preprocessed trace, returning the
/// reusable prover data + verifying key. Deterministic in
/// `program`: prover and verifier independently arrive at the
/// same commitment iff they use the same canonical program.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_setup(
    config: &AiPowStarkConfig,
    program: &Program,
) -> (
    PreprocessedProverData<AiPowStarkConfig>,
    PreprocessedVerifierKey<AiPowStarkConfig>,
) {
    let degree_bits =
        checked_program_degree_bits(program).expect("canonical program shape already validated");
    let air = CompositeFullAirPinned::try_new(program.clone())
        .expect("canonical program shape validated");
    setup_preprocessed(config, &air, degree_bits)
        .expect("CompositeFullAirPinned always has preprocessed columns")
}

/// Program-pinned prove (uni-stark, **no LogUp** — lighter
/// variant + the `crit1_*`/`high2_*` constraint-logic harness).
/// **Production should call [`composite_prove_pinned_logup`]**
/// (Route A) so the `noised_packed` matrix binding is enforced.
///
/// Derives the canonical program from the (honest) trace's
/// `*_PREP` columns, commits it, and proves. Returns the proof
/// **and** the program — the caller hands the program to the
/// verifier *out of band from a trusted source* (params), never
/// lets the verifier take it from the proof.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_prove_pinned(
    config: &AiPowStarkConfig,
    trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
) -> (Proof<AiPowStarkConfig>, Program) {
    let program = extract_program(&trace.matrix);
    let air = CompositeFullAirPinned::new(program.clone());
    let (pp, _vk) = composite_setup(config, &program);
    let pis = public_inputs.to_vec();
    let proof = prove_with_preprocessed(config, &air, trace.matrix, &pis, Some(&pp));
    (proof, program)
}

/// Program-pinned verify. `program` MUST be the canonical program
/// for the agreed `ZkParams`, rebuilt by the verifier from the
/// trusted shape — never extracted from the prover's proof. The
/// preprocessed commitment in the derived VK pins the prover's
/// selector schedule; a forged trace whose `*_PREP` columns
/// differ fails the in-AIR equality.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_verify_pinned(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &Proof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
) -> Result<(), CompositeVerificationError> {
    checked_program_degree_bits(program)?;
    let air = CompositeFullAirPinned::try_new(program.clone())?;
    let (_pp, vk) = composite_setup(config, program);
    let pis = public_inputs.to_vec();
    verify_with_preprocessed(config, &air, proof, &pis, Some(&vk))
        .map_err(|e| CompositeVerificationError::Stark(format!("{e:?}")))
}

/// Program-pinned full PoW verify: pinned STARK verify + the C2
/// difficulty check against the bound `HASH_JACKPOT`.
#[cfg(any(test, feature = "dev-unsafe"))]
pub fn composite_verify_pow_pinned(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &Proof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    target: &[u8; 32],
) -> Result<(), PowVerifyError> {
    composite_verify_pinned(config, program, proof, public_inputs)
        .map_err(PowVerifyError::Stark)?;
    let hj = hash_jackpot_le_bytes(&public_inputs.hash_jackpot);
    if le_u256_le(&hj, target) {
        Ok(())
    } else {
        Err(PowVerifyError::DifficultyNotMet)
    }
}

// ───────────────────────────────────────────────────────────────
//  Route A: pinned + LogUp Layer-0 prove/verify
//
//  The uni-stark `composite_*_pinned` above enforce the canonical-
//  program pin but NOT the cross-chip LogUp — so the matmul
//  `A_NOISED`/`B_NOISED` reads are unbound vs the C3/HASH_A
//  canonical store. These prove/verify the
//  `CompositeFullAirWithLookupsPinned` AIR via `p3-batch-stark`,
//  which enforces the canonical-program pin AND the `noised_packed`
//  (+ range / i8u8 / cv-routing) LogUp simultaneously. The batch path
//  keeps the prover close to the uni-stark pinned cost instead of widening
//  the preprocessed trace.
//
//  Same trust model: the verifier rebuilds the canonical
//  `program` from the trusted per-block `ctx` (never from the
//  proof), derives the preprocessed commitment from it via
//  `ProverData::from_airs_and_degrees` (witness-free — needs only
//  the program + the public trace height), and checks the proof
//  against that.
// ───────────────────────────────────────────────────────────────

/// Route-A pinned Layer-0 prove. `trace` is LogUp-balanced here
/// (`populate_lookup_freq`) so the bus argument closes. Returns
/// the batch proof + the canonical program (handed to the
/// verifier out-of-band from a trusted source, never via the
/// proof — the same pinning discipline as `composite_prove_pinned`).
///
/// This proof is the inner input to the recursive production
/// certificate. It is not itself the canonical Nockchain block/wire
/// certificate.
pub fn composite_prove_pinned_logup(
    config: &AiPowStarkConfig,
    trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
) -> (p3_batch_stark::BatchProof<AiPowStarkConfig>, Program) {
    composite_prove_pinned_logup_sx(config, trace, public_inputs, true)
}

/// [`composite_prove_pinned_logup`] with an explicit
/// keystone flag. `sx_bound` MUST be derived by the verifier from trusted
/// block parameters, never from the proof. `true` uses the StripeXor transport
/// for `num_stripes <= STRIPE_MAX`; `false` uses the R-b TileReduce predecessor
/// keystone for larger stripe-major traces.
pub fn composite_prove_pinned_logup_sx(
    config: &AiPowStarkConfig,
    trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
    sx_bound: bool,
) -> (p3_batch_stark::BatchProof<AiPowStarkConfig>, Program) {
    let (proof, program, _) =
        composite_prove_pinned_logup_sx_with_common(config, trace, public_inputs, sx_bound);
    (proof, program)
}

pub fn composite_prove_pinned_logup_sx_with_common(
    config: &AiPowStarkConfig,
    mut trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
    sx_bound: bool,
) -> (
    p3_batch_stark::BatchProof<AiPowStarkConfig>,
    Program,
    p3_batch_stark::CommonData<AiPowStarkConfig>,
) {
    use p3_batch_stark::{prove_batch, ProverData, StarkInstance};

    trace.populate_lookup_freq();
    let program = extract_program(&trace.matrix);
    let air =
        crate::composite_full_air_with_lookups::CompositeFullAirWithLookupsPinned::try_new_with(
            program.clone(),
            sx_bound,
        )
        .expect("canonical program shape validated");
    let pvs = public_inputs.to_vec();
    let instances = vec![StarkInstance {
        air: &air,
        trace: &trace.matrix,
        public_values: pvs,
    }];
    let pd = ProverData::from_instances(config, &instances);
    let proof = prove_batch(config, &instances, &pd);
    (proof, program, pd.common)
}

/// Verifier-side `CommonData` for the canonical `program` —
/// rebuilt witness-free from the program + its (public) height.
/// `pub(crate)` so the recursion integration can obtain the
/// `CommonData` the recursive verifier needs.
pub(crate) fn logup_common_for(
    config: &AiPowStarkConfig,
    program: &Program,
    sx_bound: bool,
) -> p3_batch_stark::ProverData<AiPowStarkConfig> {
    use p3_batch_stark::ProverData;
    let log_ext_db = checked_program_degree_bits(program)
        .expect("canonical program shape already validated")
        + config.is_zk();
    let air =
        crate::composite_full_air_with_lookups::CompositeFullAirWithLookupsPinned::try_new_with(
            program.clone(),
            sx_bound,
        )
        .expect("canonical program shape validated");
    ProverData::from_airs_and_degrees(config, std::slice::from_ref(&air), &[log_ext_db])
}

/// Route-A pinned verify. `program` MUST be the canonical program
/// the verifier rebuilds from the trusted shape (never from the
/// proof) — exactly as `composite_verify_pinned`.
pub fn composite_verify_pinned_logup(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &p3_batch_stark::BatchProof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
) -> Result<(), CompositeVerificationError> {
    composite_verify_pinned_logup_sx(config, program, proof, public_inputs, true)
}

/// [`composite_verify_pinned_logup`] with an explicit
/// keystone flag (verifier-set from trusted params).
pub(crate) fn composite_verify_pinned_logup_sx(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &p3_batch_stark::BatchProof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    sx_bound: bool,
) -> Result<(), CompositeVerificationError> {
    use p3_batch_stark::verify_batch;
    checked_program_degree_bits(program)?;
    let air =
        crate::composite_full_air_with_lookups::CompositeFullAirWithLookupsPinned::try_new_with(
            program.clone(),
            sx_bound,
        )?;
    let pd = logup_common_for(config, program, sx_bound);
    verify_batch(
        config,
        std::slice::from_ref(&air),
        proof,
        &[public_inputs.to_vec()],
        &pd.common,
    )
    .map_err(|e| CompositeVerificationError::Stark(format!("{e:?}")))
}

/// Route-A pinned full PoW verify: pinned+LogUp STARK verify +
/// the C2 difficulty check against the bound `HASH_JACKPOT`.
///
/// `target` is **not** absorbed into the Fiat-Shamir transcript and **not**
/// an AIR public input (Pearl-Layer-0-faithful: difficulty is external by
/// design). The bound is meaningful only if the caller derived it itself
/// from chain-pinned data (params / block header); a counterparty-supplied
/// target weaker than the real one is accepted. Every production caller
/// recomputes the target internally (see `ai_pow::zk_bridge`).
pub fn composite_verify_pow_pinned_logup(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &p3_batch_stark::BatchProof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    target: &[u8; 32],
) -> Result<(), PowVerifyError> {
    composite_verify_pow_pinned_logup_sx(config, program, proof, public_inputs, target, true)
}

/// [`composite_verify_pow_pinned_logup`] with an explicit
/// keystone flag. `sx_bound` MUST be derived by the verifier from trusted block
/// parameters, never from the proof.
pub fn composite_verify_pow_pinned_logup_sx(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &p3_batch_stark::BatchProof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    target: &[u8; 32],
    sx_bound: bool,
) -> Result<(), PowVerifyError> {
    composite_verify_pinned_logup_sx(config, program, proof, public_inputs, sx_bound)
        .map_err(PowVerifyError::Stark)?;
    let hj = hash_jackpot_le_bytes(&public_inputs.hash_jackpot);
    if le_u256_le(&hj, target) {
        Ok(())
    } else {
        Err(PowVerifyError::DifficultyNotMet)
    }
}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use super::*;

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

    fn bincode_len<T: serde::Serialize>(value: &T, label: &str) -> usize {
        bincode::serde::encode_to_vec(value, bincode::config::standard())
            .unwrap_or_else(|err| panic!("{label} must bincode-serialize: {err:?}"))
            .len()
    }

    fn postcard_len<T: serde::Serialize>(value: &T, label: &str) -> usize {
        postcard::to_allocvec(value)
            .unwrap_or_else(|err| panic!("{label} must postcard-serialize: {err:?}"))
            .len()
    }

    fn measure_pinned_logup_l0_size_breakdown(label: &str, profile: CircuitConfig) {
        assert_eq!(
            profile.operational_fri_bits(),
            60,
            "{label} must remain a 60-bit pure-query diagnostic"
        );
        assert_eq!(
            profile.pow_bits, 0,
            "{label} must not count proof-system PoW"
        );

        let cfg = build_config(&test_zk_params(), &profile);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);

        let prove_start = std::time::Instant::now();
        let (proof, program) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let prove_ms = prove_start.elapsed().as_millis();

        let verify_start = std::time::Instant::now();
        composite_verify_pinned_logup(&cfg, &program, &proof, &pis)
            .expect("production Layer-0 pinned+LogUp proof must verify");
        let verify_ms = verify_start.elapsed().as_millis();

        let bincode_total = bincode_len(&proof, "Layer-0 batch proof");
        let bincode_commitments = bincode_len(&proof.commitments, "Layer-0 commitments");
        let bincode_opened_values = bincode_len(&proof.opened_values, "Layer-0 opened values");
        let bincode_opening_proof = bincode_len(&proof.opening_proof, "Layer-0 opening proof");
        let bincode_lookup_terminals =
            bincode_len(&proof.lookup_terminals, "Layer-0 lookup terminals");
        let bincode_component_sum = bincode_commitments
            + bincode_opened_values
            + bincode_opening_proof
            + bincode_lookup_terminals;

        let postcard_total = postcard_len(&proof, "Layer-0 batch proof");
        let postcard_commitments = postcard_len(&proof.commitments, "Layer-0 commitments");
        let postcard_opened_values = postcard_len(&proof.opened_values, "Layer-0 opened values");
        let postcard_opening_proof = postcard_len(&proof.opening_proof, "Layer-0 opening proof");
        let postcard_lookup_terminals =
            postcard_len(&proof.lookup_terminals, "Layer-0 lookup terminals");
        let postcard_component_sum = postcard_commitments
            + postcard_opened_values
            + postcard_opening_proof
            + postcard_lookup_terminals;

        let table_count = proof.opened_values.instances.len();
        let lookup_terminal_count = proof
            .lookup_terminals
            .iter()
            .filter(|t| t.is_some())
            .count();

        eprintln!(
            "ai-pow Layer-0 pinned+LogUp size breakdown [{label}]: profile=lb{} nq{} pow{} prove_ms={} verify_ms={} tables={} lookup_terminals={}",
            profile.log_blowup,
            profile.num_queries,
            profile.pow_bits,
            prove_ms,
            verify_ms,
            table_count,
            lookup_terminal_count,
        );
        eprintln!(
            "ai-pow Layer-0 pinned+LogUp bincode bytes [{label}]: total={} commitments={} opened_values={} opening_proof={} lookup_terminals={} component_sum={} serde_overhead={}",
            bincode_total,
            bincode_commitments,
            bincode_opened_values,
            bincode_opening_proof,
            bincode_lookup_terminals,
            bincode_component_sum,
            bincode_total.saturating_sub(bincode_component_sum),
        );
        eprintln!(
            "ai-pow Layer-0 pinned+LogUp postcard bytes [{label}]: total={} commitments={} opened_values={} opening_proof={} lookup_terminals={} component_sum={} serde_overhead={}",
            postcard_total,
            postcard_commitments,
            postcard_opened_values,
            postcard_opening_proof,
            postcard_lookup_terminals,
            postcard_component_sum,
            postcard_total.saturating_sub(postcard_component_sum),
        );
    }

    #[test]
    fn composite_prove_verify_round_trip() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);
        composite_verify(&cfg, &proof, &pis).expect("composite proof must verify");
    }

    // ───── Regression: non-zero JACKPOT_MSG ─────
    //
    // A latent JackpotChip bug (the JACKPOT_MSG RAM recurrence
    // `nxt = SLOT_SEL·rotl13_xor + (1−SLOT_SEL)·cur` was
    // `when_transition` but **not** gated by `is_active`) forced
    // JACKPOT_MSG constant across all inactive rows, so the
    // inactive→active(finalize) boundary forbade a freshly-placed
    // non-zero JACKPOT_MSG. Latent for years because every
    // jackpot placement hashed an all-zero JACKPOT_MSG (0 == 0);
    // surfaced by the first non-zero JACKPOT_MSG placement — the
    // real folded `M`. Fixed by gating the recurrence with
    // `is_active` (`chips::jackpot::chip`). These two tests pin
    // the fix; bisection scaffolding removed post-fix.

    /// `place_jackpot_hash_block` with a **non-zero** message
    /// must satisfy the (unit) composite AIR — the minimal
    /// regression for the JackpotChip `is_active`-gating fix.
    #[test]
    fn high2_2_jackpot_nonzero_msg_unit() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let msg: [u32; 16] = core::array::from_fn(|i| 0xABCD_0001u32.wrapping_mul(i as u32 + 1));
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let _ = trace.place_jackpot_hash_block(h - 8, &msg, &ch);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);
        composite_verify(&cfg, &proof, &pis)
            .expect("non-zero-message jackpot block must verify (JackpotChip is_active gate)");
    }

    /// Fast unit gate — the full useful-work chain
    /// placement satisfies the base `CompositeFullAir` (matmul
    /// sweep recurrence + StripeXor transport + `SX_IN ==
    /// nxt.CUMSUM_TILE` cross-chip binding + FoldChip), proven via
    /// the cheap unit prover. Isolates witness/chip-wiring bugs in
    /// seconds before the ~minutes Route-A pinned path below. (The
    /// pinned jackpot/fold keystones are exercised by
    /// `high2_2_fold_chain_pinned_logup`.)
    #[test]
    fn high2_2_useful_work_chain_unit() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x1234_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (t, _k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * 64) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * 64) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(8, &a_prime, &b_prime, t, r, num_stripes);
        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let m = trace.place_fold_chain(8 + rows_used + 4, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);
        composite_verify(&cfg, &proof, &pis)
            .expect("useful-work chain must verify through unit CompositeFullAir");
    }

    #[test]
    fn pinned_zeroed_sx_controls_reject() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            SX_CONTROL_PREP, SX_IS_ACTIVE, SX_LANE_SEL_START, TOTAL_TRACE_WIDTH,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x1234_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (t, r, num_stripes) = (8usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * num_stripes * r) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * num_stripes * r) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(8, &a_prime, &b_prime, t, r, num_stripes);
        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let m = trace.place_fold_chain(8 + rows_used + 4, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let canonical = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let sx_row = (0..trace.height())
            .find(|&rr| {
                trace.matrix.values[rr * TOTAL_TRACE_WIDTH + SX_IS_ACTIVE].as_canonical_u64() == 1
            })
            .expect("SX trace has an active row");
        let base = sx_row * TOTAL_TRACE_WIDTH;
        trace.matrix.values[base + SX_IS_ACTIVE] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        for lane in 0..num_stripes {
            trace.matrix.values[base + SX_LANE_SEL_START + lane] =
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        }
        assert_ne!(
            trace.matrix.values[base + SX_CONTROL_PREP].as_canonical_u64(),
            0,
            "fixture keeps the verifier-pinned SX schedule word live"
        );

        let (proof, _) = composite_prove_pinned(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned(&cfg, &canonical, &proof, &pis).is_err(),
            "zeroed SX activity/lane controls must reject against the pinned schedule"
        );
    }

    /// R-b — the STRIPE-MAJOR sweep (`place_useful_work_chain_rb`:
    /// held h·w accumulator + per-stripe reduce + interleaved fold)
    /// satisfies the base `CompositeFullAir` (matmul recurrence + noised
    /// pack-link + TileAccum + TileReduce + TA_DOT binding + FoldChip).
    /// This is the first R-b composite proof — the activation of the two
    /// standalone-validated R-b chips inside the real composite AIR.
    #[test]
    fn rb_stripe_major_useful_work_chain_unit() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (t, r) = (8usize, 4usize);
        // num_stripes 16 (≤ STRIPE_MAX, result-equivalent) AND 128, 256 —
        // both FAR past the old STRIPE_MAX=64 cap the sub-block-major
        // StripeXor path could never exceed. This is R-b's whole point:
        // arbitrary num_stripes with no 64-lane register.
        for num_stripes in [16usize, 128, 256] {
            let k = num_stripes * r;
            let a_prime: Vec<i8> = (0..(t * k) as i32)
                .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
                .collect();
            let b_prime: Vec<i8> = (0..(t * k) as i32)
                .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
                .collect();
            let mut trace = CompositeTrace::baseline_min();
            let (_rows, _m) =
                trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
            let pis = CompositePublicInputs::derive_from_trace(&trace);
            let proof = composite_prove(&cfg, trace, &pis);
            composite_verify(&cfg, &proof, &pis).unwrap_or_else(|e| {
                panic!("R-b stripe-major sweep (num_stripes={num_stripes}) must verify: {e:?}")
            });
        }
    }

    #[test]
    fn routea_zeroed_rb_ta_tr_controls_reject() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            RB_CONTROL_PREP, TA_IS_ACTIVE, TA_SB_SEL_LEN, TA_SB_SEL_START, TOTAL_TRACE_WIDTH,
            TR_IS_ACTIVE, TR_STRIPE_RESET,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x7B00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 16usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (_rows, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let program = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let reduce_row = (0..trace.height())
            .find(|&rr| {
                trace.matrix.values[rr * TOTAL_TRACE_WIDTH + TR_IS_ACTIVE].as_canonical_u64() == 1
            })
            .expect("R-b trace has a reduce row");
        let base = reduce_row * TOTAL_TRACE_WIDTH;
        trace.matrix.values[base + TA_IS_ACTIVE] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        for s in 0..TA_SB_SEL_LEN {
            trace.matrix.values[base + TA_SB_SEL_START + s] =
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        }
        trace.matrix.values[base + TR_IS_ACTIVE] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        trace.matrix.values[base + TR_STRIPE_RESET] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0);
        assert_ne!(
            trace.matrix.values[base + RB_CONTROL_PREP].as_canonical_u64(),
            0,
            "fixture keeps the verifier-pinned R-b schedule word live"
        );

        let (proof, _) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "zeroed R-b TA/TR controls must reject against the pinned schedule"
        );
    }

    /// R-b soundness — a tampered TileReduce input on a reduce row
    /// must reject. Guards the `TR_IN == TA_ACC[active_sb]` bind (+ the
    /// TileReduce bit-reconstruction): a prover cannot feed the reduce a
    /// per-stripe contribution other than the held accumulator's.
    #[test]
    fn rb_tampered_reduce_input_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{TOTAL_TRACE_WIDTH, TR_IN_START, TR_IS_ACTIVE};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (t, r, num_stripes) = (8usize, 4usize, 16usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let _ = trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        // Tamper TR_IN[0] on the first reduce row.
        let h = trace.matrix.values.len() / TOTAL_TRACE_WIDTH;
        let mut target = None;
        for rr in 0..h {
            if trace.matrix.values[rr * TOTAL_TRACE_WIDTH + TR_IS_ACTIVE].as_canonical_u64() == 1 {
                target = Some(rr);
                break;
            }
        }
        let rr = target.expect("a reduce row must exist");
        trace.matrix.values[rr * TOTAL_TRACE_WIDTH + TR_IN_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0xDEAD_BEEF);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);
        assert!(
            composite_verify(&cfg, &proof, &pis).is_err(),
            "tampered reduce input (≠ held accumulator) must reject",
        );
    }

    /// R-b ADVERSARIAL — the held TileAccum accumulator must be fed
    /// the GENUINE matmul products. `TA_DOT` on each sweep row is bound
    /// (`TA_DOT == dot(A_NOISED_UNPACK, B_NOISED_UNPACK)`) so a prover
    /// cannot inject a fabricated per-step dot into the accumulator (and
    /// thus a fake x_step). Tampering `TA_DOT` on an active sweep row MUST
    /// reject. Together with `TR_IN==TA_ACC` (accumulator→reduce) and
    /// `FOLD_XSTEP==TR_NEW` (reduce→fold), this closes the full R-b chain
    /// matmul-dot → accumulator → reduce → fold → jackpot, each link
    /// adversarially bound with the SX 64-lane keystone OFF.
    #[test]
    fn rb_tampered_dot_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{TA_DOT_START, TA_IS_ACTIVE, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (t, r, num_stripes) = (8usize, 4usize, 16usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let _ = trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        // Tamper TA_DOT[0] on the first active sweep row.
        let h = trace.matrix.values.len() / TOTAL_TRACE_WIDTH;
        let mut rr = None;
        for row in 0..h {
            if trace.matrix.values[row * TOTAL_TRACE_WIDTH + TA_IS_ACTIVE].as_canonical_u64() == 1 {
                rr = Some(row);
                break;
            }
        }
        let rr = rr.expect("R-b trace has an active TileAccum sweep row");
        let cell = rr * TOTAL_TRACE_WIDTH + TA_DOT_START;
        let orig = trace.matrix.values[cell].as_canonical_u64();
        trace.matrix.values[cell] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(orig.wrapping_add(1));
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);
        assert!(
            composite_verify(&cfg, &proof, &pis).is_err(),
            "tampered TA_DOT (≠ real matmul dot) must reject",
        );
    }

    /// R-b — the PINNED path (the canonical-program pin + the
    /// JACKPOT_MSG==FOLD_STATE keystone + the R-b keystone FOLD_XSTEP==TR_NEW)
    /// verifies for a stripe-major R-b trace at num_stripes=128 (> the old
    /// STRIPE_MAX=64), with `sx_bound=false` (the SX 64-lane keystone is
    /// inactive for R-b; the R-b keystone binds the fold input to the
    /// per-stripe reduce). This exercises the keystones the base
    /// `CompositeFullAir` proof cannot.
    #[test]
    fn rb_stripe_major_pinned_verifies() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x7B00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 128usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (_rows, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        // The jackpot-hash block sets last-row JACKPOT_MSG == M.
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let pis_v = pis.to_vec();
        let program = extract_program(&trace.matrix);
        let air = CompositeFullAirPinned::new_with(program.clone(), false);
        let (pp, vk) = composite_setup(&cfg, &program);
        let proof = prove_with_preprocessed(&cfg, &air, trace.matrix, &pis_v, Some(&pp));
        verify_with_preprocessed(&cfg, &air, &proof, &pis_v, Some(&vk))
            .expect("R-b pinned (sx_bound=false) at num_stripes 128 must verify");
    }

    /// R-b ADVERSARIAL — the keystone that REPLACES the disabled
    /// SX 64-lane keystone when `sx_bound=false`. With the SX keystone
    /// off, the ONLY thing binding each stripe's FoldChip input to the
    /// genuine per-stripe reduce is the R-b keystone
    /// `FOLD_XSTEP == TR_NEW` (the previous row's TileReduce output). If
    /// a malicious prover could fabricate `FOLD_XSTEP` (a fake x_step
    /// without the real matmul reduce), the PoW digest would not be
    /// bound to the real work. Tampering `FOLD_XSTEP` on a fold row MUST
    /// make the pinned proof fail to verify — proving the keystone is
    /// live under `sx_bound=false` (the R-b soundness linchpin).
    #[test]
    fn rb_pinned_tampered_fold_xstep_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{FOLD_IS_FOLD, FOLD_XSTEP, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x7B00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 128usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (_rows, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        // Tamper FOLD_XSTEP on the FIRST fold row (FOLD_IS_FOLD==1) to a
        // value ≠ the row's TR_NEW — the R-b keystone must catch it.
        let mut fold_row = None;
        for rr in 0..h {
            if trace.matrix.values[rr * TOTAL_TRACE_WIDTH + FOLD_IS_FOLD].as_canonical_u64() == 1 {
                fold_row = Some(rr);
                break;
            }
        }
        let rr = fold_row.expect("R-b trace has a fold row");
        let cell = rr * TOTAL_TRACE_WIDTH + FOLD_XSTEP;
        let orig = trace.matrix.values[cell].as_canonical_u64();
        trace.matrix.values[cell] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(orig ^ 0x5A5A);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let pis_v = pis.to_vec();
        let program = extract_program(&trace.matrix);
        let air = CompositeFullAirPinned::new_with(program.clone(), false);
        let (pp, vk) = composite_setup(&cfg, &program);
        let proof = prove_with_preprocessed(&cfg, &air, trace.matrix, &pis_v, Some(&pp));
        assert!(
            verify_with_preprocessed(&cfg, &air, &proof, &pis_v, Some(&vk)).is_err(),
            "tampered FOLD_XSTEP (≠ TR_NEW) MUST reject — the R-b \
             keystone binds the fold input to the real reduce under sx_bound=false"
        );
    }

    /// R-b ADVERSARIAL — the `sx_bound` flag is load-bearing and
    /// verifier-set. An R-b trace (built with the SX 64-lane keystone
    /// OFF; FOLD_STRIPE_SEL[0]=1 satisfies FoldChip's one-hot without
    /// activating the SX lanes) does NOT satisfy the `sx_bound=true` AIR
    /// (which additionally forces `FOLD_XSTEP == SX_XR[stripe]`). So an
    /// R-b proof verified under the WRONG flag (`sx_bound=true`) MUST
    /// reject — confirming the verifier's params-derived flag
    /// (`num_stripes > STRIPE_MAX ⇒ false`) is essential and a prover
    /// cannot substitute the other AIR.
    #[test]
    fn rb_pinned_wrong_sx_bound_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x7B00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 128usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (_rows, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let pis_v = pis.to_vec();
        let program = extract_program(&trace.matrix);
        // Honest R-b proof under the CORRECT flag.
        let air_false = CompositeFullAirPinned::new_with(program.clone(), false);
        let (pp, _vk) = composite_setup(&cfg, &program);
        let proof = prove_with_preprocessed(&cfg, &air_false, trace.matrix, &pis_v, Some(&pp));
        // Verifying the SAME proof under sx_bound=true MUST reject: the
        // stronger AIR's SX keystone is unsatisfied by an R-b trace.
        let air_true = CompositeFullAirPinned::new_with(program.clone(), true);
        let (_pp2, vk_true) = composite_setup(&cfg, &program);
        assert!(
            verify_with_preprocessed(&cfg, &air_true, &proof, &pis_v, Some(&vk_true)).is_err(),
            "an R-b proof (sx_bound=false) MUST NOT verify under sx_bound=true"
        );
    }

    /// R-b — the full Route-A LogUp path (the canonical-program pin + the
    /// `noised_packed`/range/i8u8 LogUp bus + keystones) for a stripe-major
    /// R-b trace at num_stripes=128, `sx_bound=false`. The R-b sweep's
    /// `place_matmul_step` writes A/B-NOISED consumers; the bus balances
    /// against the co-located producer store the sweep also emits.
    #[test]
    fn rb_stripe_major_logup_verifies() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x9C00 + i as u32);
        // num_stripes=96 (> the old STRIPE_MAX=64). The per-query positioned
        // producer store for R-b is large, so this fits a 1<<14 trace.
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
        // noised_packed producer store — the R-b sweep now emits positioned
        // MAT_IDs (place_matmul_step_with_ids), so the bus balances against the
        // same positioned chunk keys. place_noised_store_row sets only the
        // store cols, preserving the sweep's FOLD_STATE/TR/TA passthrough.
        let store_chunks = CompositeTrace::enumerate_noised_chunks_positioned(
            &a_prime, &b_prime, t, r, num_stripes,
        );
        let store_start = 8 + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
        }
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false)
            .expect("R-b Route-A LogUp at num_stripes 128 must verify");
    }
    #[test]
    fn rb_logup_jackpot_message_decoupling_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            BLAKE3_MSG_START, IS_MSG_JACKPOT, JACKPOT_MSG_START, JACKPOT_SIZE, TOTAL_TRACE_WIDTH,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x9C00 + i as u32);
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
        let store_start = 8 + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
        }

        let mut m_prime = m;
        m_prime[1] ^= 1;
        let _ = trace.place_jackpot_hash_block(h - 8, &m_prime, &ch);

        let first_hash_row = (h - 8) * TOTAL_TRACE_WIDTH;
        assert_eq!(
            trace.matrix.values[first_hash_row + BLAKE3_MSG_START + 1].as_canonical_u64(),
            m_prime[1] as u64,
        );
        assert_eq!(
            trace.matrix.values[first_hash_row + IS_MSG_JACKPOT].as_canonical_u64(),
            1,
        );
        assert_ne!(m_prime, m);

        let last_row = (h - 1) * TOTAL_TRACE_WIDTH;
        for i in 0..JACKPOT_SIZE {
            trace.matrix.values[last_row + JACKPOT_MSG_START + i] =
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(m[i] as u64);
        }
        assert_eq!(
            trace.matrix.values[last_row + JACKPOT_MSG_START + 1].as_canonical_u64(),
            m[1] as u64,
        );

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "a BLAKE3 message that differs from the folded TileState must reject",
        );
    }

    /// Shared R-b fixture: a full stripe-major chain (96 stripes, r=4) plus
    /// the positioned noised store and the final jackpot block. Returns the
    /// trace, the honest fold state `m`, and the jackpot key `ch`.
    fn rb_chain_trace() -> (CompositeTrace, [u32; 16], [u32; 8]) {
        let ch: [u32; 8] = core::array::from_fn(|i| 0x9C00 + i as u32);
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
        let store_start = 8 + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
        }
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        (trace, m, ch)
    }

    /// R-b ADVERSARIAL — fold-state mutation on a pad row between the
    /// sweep region and the jackpot block must reject: the FoldChip
    /// passthrough pins FOLD_STATE across every non-fold row, so the
    /// jackpot message cannot be decoupled from the proven fold.
    #[test]
    fn rb_logup_fold_state_pad_tamper_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{FOLD_STATE_START, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (mut trace, _m, _ch) = rb_chain_trace();
        let h = trace.height();
        // Tamper FOLD_STATE[0] on a pad row two rows above the jackpot
        // block (a non-fold row: passthrough must propagate the tamper).
        let pad = (h - 10) * TOTAL_TRACE_WIDTH;
        let cur = trace.matrix.values[pad + FOLD_STATE_START].as_canonical_u64();
        trace.matrix.values[pad + FOLD_STATE_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur + 1);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "a fold-state mutation on a pad row must reject (passthrough is constrained)",
        );
    }

    /// R-b ADVERSARIAL — feeding the TileReduce a value that is not the
    /// selected sub-block's post-update accumulator must reject: the
    /// composite binds TR_IN to the one-hot-selected TA_ACC on the next
    /// row, so a forged per-stripe x_step cannot reach the fold.
    #[test]
    fn rb_logup_tr_in_substitution_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{TOTAL_TRACE_WIDTH, TR_IN_START, TR_IS_ACTIVE};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let (mut trace, _m, _ch) = rb_chain_trace();
        let h = trace.height();
        // Find the first TR-active row and bump TR_IN[0].
        let tr_row = (0..h)
            .find(|&r| {
                trace.matrix.values[r * TOTAL_TRACE_WIDTH + TR_IS_ACTIVE].as_canonical_u64() == 1
            })
            .expect("R-b chain has TR rows");
        let base = tr_row * TOTAL_TRACE_WIDTH;
        let cur = trace.matrix.values[base + TR_IN_START].as_canonical_u64();
        trace.matrix.values[base + TR_IN_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur + 1);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "a substituted TileReduce input must reject (TR_IN is bound to TA_ACC)",
        );
    }

    /// R-b ADVERSARIAL — the `noised_packed` LogUp bus binds the
    /// R-b sweep's matmul inputs to the committed producer store. The
    /// R-b sweep consumes `(mat_id, packed A/B-NOISED)` on each step; the
    /// store publishes the producers (in production, co-located with the
    /// strip-opening ⇒ bound to HASH_A/B via C3, i.e. the committed
    /// matrix). If the two disagree the bus does not balance. Tampering
    /// ONE store chunk's published value (producer ≠ the value the sweep
    /// consumed) MUST reject — so a malicious prover cannot sweep matmul
    /// inputs that differ from the committed matrix. This audits the bus
    /// on the R-b stripe-major path (same multiset binding as ≤64, but the
    /// consumers are emitted in stripe-major order).
    #[test]
    fn rb_logup_tampered_matmul_binding_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x9C00 + i as u32);
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
        let mut store_chunks = CompositeTrace::enumerate_noised_chunks_positioned(
            &a_prime, &b_prime, t, r, num_stripes,
        );
        // FORGE: corrupt EVERY producer's PUBLISHED value (keep the mat_id
        // position keys). Each store row now produces (id, wrong) while
        // the R-b sweep still consumes (id, real). The noised_packed bus
        // key includes the value, so every CONSUMED position loses its
        // matching producer ⇒ the bus cannot balance. (Corrupting every
        // chunk, not one, sidesteps the fact that enumerate emits some
        // unconsumed producers whose MAT_FREQ is 0.)
        for c in store_chunks.iter_mut() {
            c.bytes[0] = c.bytes[0].wrapping_add(1);
        }
        let store_start = 8 + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
        }
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "a store producer ≠ the R-b sweep's consumed matmul input MUST \
             unbalance the noised_packed bus and reject",
        );
    }

    /// Cross-side committed-value substitution is rejected.
    ///
    /// A tile-height-derived `b_id_base` lets a scattered A covering-range
    /// lane collide with a B-side key: a cheating prover can sweep COMMITTED
    /// B bytes at the colliding A positions and the `noised_packed` bus
    /// still balances (the fingerprint value matches the B producer),
    /// proving a sweep the committed matrices do not justify — an XOR-delta
    /// jackpot-grinding amplifier.
    ///
    /// With span-derived bases the A and B key spaces are disjoint, so the
    /// smuggled query `(a_id, val_B)` has no producer entry and the bus
    /// cannot balance. Both traces below are honestly computed end-to-end
    /// from their inputs (every keystone and pack-link holds); the ONLY
    /// defect in the smuggled trace is that its sweep reads B's committed
    /// bytes at A positions while the producer store publishes the committed
    /// matrices. The lane pair (A lane 8, B lane 0) collides under the
    /// tile-height base.
    #[test]
    fn noised_packed_cross_side_substitution_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x9C00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 96usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        // Scattered A covering-range lanes (Pearl-pattern-like), contiguous B.
        let a_lanes: Vec<usize> = vec![0, 1, 8, 9, 64, 65, 72, 73];
        let b_lanes: Vec<usize> = (0..t).collect();
        // The producer store publishes the COMMITTED matrices (honest
        // arrays), keyed at the covering-range lanes the sweep queries.
        let store_chunks = {
            let mut cs = CompositeTrace::enumerate_noised_chunks_positioned(
                &a_prime, &b_prime, t, r, num_stripes,
            );
            for c in cs.iter_mut() {
                let lanes: &Vec<usize> = if c.side_a { &a_lanes } else { &b_lanes };
                for entry in c.src.iter_mut() {
                    if let Some((lane, l)) = *entry {
                        *entry = Some((lanes[lane as usize] as u32, l));
                    }
                }
            }
            cs
        };
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        let build = |a_sweep: &[i8]| {
            let mut trace = CompositeTrace::baseline(1 << 14);
            let h = trace.height();
            let (rows_used, m) = trace.place_useful_work_chain_rb_indexed(
                8, a_sweep, &b_prime, t, t, r, num_stripes, &a_lanes, &b_lanes,
            );
            let store_start = 8 + rows_used;
            for (i, chunk) in store_chunks.iter().enumerate() {
                let id_base = if chunk.side_a { a_id_base } else { b_id_base };
                let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                    .try_into()
                    .expect("positioned noised chunk id must fit in MAT_ID");
                trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
            }
            let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);
            let pis = CompositePublicInputs::derive_from_trace(&trace);
            let (proof, program) = composite_prove_pinned_logup_sx(&cfg, trace, &pis, false);
            (program, proof, pis)
        };
        // Honest scattered-lane sweep verifies: the fix does not false-reject
        // legitimate scattered (Pearl-merge) schedules.
        let (program, proof, pis) = build(&a_prime);
        composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false)
            .expect("honest scattered-lane sweep must verify");
        // Smuggle: the sweep reads committed B-col-0 bytes at A-row lane 8
        // (tile-local row 2) — every downstream value (matmul, fold,
        // jackpot) honestly recomputed, exactly as a grinding miner would.
        let mut a_smuggled = a_prime.clone();
        a_smuggled[2 * k..3 * k].copy_from_slice(&b_prime[0..k]);
        let (program, proof, pis) = build(&a_smuggled);
        assert!(
            composite_verify_pinned_logup_sx(&cfg, &program, &proof, &pis, false).is_err(),
            "cross-side committed-value substitution must not balance the \
             noised_packed bus",
        );
    }

    /// R-b ADVERSARIAL — the keystone `JACKPOT_MSG == FOLD_STATE`
    /// binds the PoW digest to the R-b fold-chain OUTPUT. The jackpot hash
    /// (HASH_JACKPOT, the PoW value) is computed over JACKPOT_MSG, which
    /// the keystone forces to equal the final FoldChip state M produced by
    /// the R-b sweep+fold. If a prover could hash a DIFFERENT M' (e.g. a
    /// low-difficulty message) while the trace's fold produced M, the PoW
    /// would be unbound from the work. Placing the jackpot block over a
    /// fold state ≠ the trace's real M MUST reject on the pinned path.
    #[test]
    fn rb_pinned_tampered_jackpot_msg_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x7B00 + i as u32);
        let (t, r, num_stripes) = (8usize, 4usize, 128usize);
        let k = num_stripes * r;
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let (_rows, m) =
            trace.place_useful_work_chain_rb(8, &a_prime, &b_prime, t, t, r, num_stripes);
        // FORGE: hash a jackpot message from a DIFFERENT fold state than
        // the trace's real M (flip one lane). The keystone
        // JACKPOT_MSG == FOLD_STATE must catch the mismatch.
        let mut m_forged = m;
        m_forged[0] ^= 0x1;
        let _ = trace.place_jackpot_hash_block(h - 8, &m_forged, &ch);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let pis_v = pis.to_vec();
        let program = extract_program(&trace.matrix);
        let air = CompositeFullAirPinned::new_with(program.clone(), false);
        let (pp, vk) = composite_setup(&cfg, &program);
        let proof = prove_with_preprocessed(&cfg, &air, trace.matrix, &pis_v, Some(&pp));
        assert!(
            verify_with_preprocessed(&cfg, &air, &proof, &pis_v, Some(&vk)).is_err(),
            "a jackpot message from M' ≠ the R-b fold state M MUST reject — \
             the keystone binds the PoW digest to the real fold-chain output",
        );
    }

    /// End-to-end regression via the production
    /// Route-A path: the **full useful-work chain** —
    /// `place_useful_work_chain` (sub-block-major matmul sweep +
    /// co-located StripeXor reduction) → `place_fold_chain` driven
    /// by the chip-reduced `x_steps` → `JACKPOT_MSG` = folded
    /// `TileState M` → the jackpot/fold keystones → jackpot-hash,
    /// proven/verified through `composite_*_pinned_logup` (the
    /// canonical-program pin + `noised_packed` LogUp via batch-stark).
    /// Exercises every useful-work-chain constraint together — the matmul cross-row
    /// recurrence over the 256-row sweep, the StripeXor transport,
    /// the `SX_IN == nxt.CUMSUM_TILE` cross-chip binding, and the
    /// Pinned `FOLD_XSTEP == SX_XR[stripe]` keystone — through the
    /// batch-stark prover (the debug-assertions-OFF hazard surface).
    /// `a_prime`/`b_prime` are synthetic i8 strips: `ai-pow-zk` must
    /// not depend on `ai-pow`, and the chip math is self-consistent
    /// (cross-crate parity vs `compute_tile_trace` is asserted from
    /// the `ai-pow` side).
    #[test]
    fn high2_2_fold_chain_pinned_logup() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            FOLD_SLOT_SEL_START, FOLD_XSTEP, SX_XR_START, TOTAL_TRACE_WIDTH,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();

        // Synthetic tile strips matching the e2e geometry
        // (MatmulParams::TEST_SMALL: t=8, k=64, r=4, num_stripes=16).
        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();

        let sweep_start = 8;
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(sweep_start, &a_prime, &b_prime, t, r, num_stripes);
        assert_eq!(rows_used, (t / 2) * (t / 2) * num_stripes); // 16·16 = 256

        // Place the positioned `noised_packed`
        // producer store so the now-chunked whole-micro-tile A/B
        // matmul query (`bus_emit::noised_packed`, `A_NOISED_LEN/2`
        // + `B_.../2` sub-queries per matmul-active row) is a
        // multiset of a declared canonical store. The sweep uses
        // exact per-sub-slice A_ID/B_ID values, so the positive
        // fixture must publish producer rows under the same
        // position-derived MAT_ID keys; a single shared MAT_ID=0
        // would correctly leave the bus unbalanced.
        let store_chunks = CompositeTrace::enumerate_noised_chunks_positioned(
            &a_prime, &b_prime, t, r, num_stripes,
        );
        let store_start = sweep_start + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            trace.place_noised_store_row(store_start + i, &chunk.bytes, mat_id);
        }
        let n_store = store_chunks.len();

        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let fold_start = store_start + n_store + 4;
        let m = trace.place_fold_chain(fold_start, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        // Keystone precondition: last-row JACKPOT_MSG ==
        // FOLD_STATE == M.
        let last = (h - 1) * TOTAL_TRACE_WIDTH;
        for s in 0..16 {
            let jm = trace.matrix.values[last + crate::composite_layout::JACKPOT_MSG_START + s];
            let fs = trace.matrix.values[last + crate::composite_layout::FOLD_STATE_START + s];
            assert_eq!(
                jm,
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(m[s] as u64),
                "JACKPOT_MSG[{s}] != M"
            );
            assert_eq!(
                jm, fs,
                "keystone precondition: JACKPOT_MSG[{s}] != FOLD_STATE[{s}]"
            );
        }
        // Fold-keystone precondition: every fold row's FOLD_XSTEP
        // equals the StripeXor register lane for its stripe.
        for step in 0..num_stripes {
            let base = (fold_start + step) * TOTAL_TRACE_WIDTH;
            let fx = trace.matrix.values[base + FOLD_XSTEP].as_canonical_u64();
            // one-hot slot = stripe (num_stripes ≤ STATE_LEN ⇒ 1:1).
            let mut slot = usize::MAX;
            for s in 0..16 {
                if trace.matrix.values[base + FOLD_SLOT_SEL_START + s].as_canonical_u64() == 1 {
                    slot = s;
                }
            }
            assert_eq!(slot, step % 16, "fold slot != stripe");
            let xr = trace.matrix.values[base + SX_XR_START + slot].as_canonical_u64();
            assert_eq!(
                fx, xr,
                "keystone precondition: FOLD_XSTEP != SX_XR @stripe {step}"
            );
        }

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis)
            .expect("full useful-work chain + keystones must verify under Route-A");
    }

    /// Coverage net: the producer store from
    /// `enumerate_noised_chunks` must contain **every** distinct
    /// 2-cell chunk key the `place_useful_work_chain` sweep writes
    /// into `A_NOISED`/`B_NOISED`. Guards against drift between the
    /// enumerator's index math and the sweep's `a_blk`/`b_blk`
    /// construction (they are duplicated, not shared).
    #[test]
    fn noised_store_covers_every_swept_chunk() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            A_NOISED_LEN, A_NOISED_START, B_NOISED_LEN, B_NOISED_START, IS_RESET_CUMSUM,
            IS_UPDATE_CUMSUM, TOTAL_TRACE_WIDTH,
        };

        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();

        let mut trace = CompositeTrace::baseline_min();
        let (rows_used, _xs) =
            trace.place_useful_work_chain(8, &a_prime, &b_prime, t, r, num_stripes);

        // Store key set: pack each enumerated 8-i8 chunk the way
        // `place_matmul_step` packs `A_NOISED` (base-256 polyval of
        // each 4-i8 half), in the canonical Goldilocks encoding.
        let chunks = CompositeTrace::enumerate_noised_chunks(&a_prime, &b_prime, t, r, num_stripes);
        let pack = |b: &[i8]| -> u64 {
            let mut acc = 0i64;
            let mut p = 1i64;
            for &x in b {
                acc += x as i64 * p;
                p *= 256;
            }
            <Val<AiPowStarkConfig> as QuotientMap<i64>>::from_int(acc).as_canonical_u64()
        };
        let mut store: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
        for c in &chunks {
            store.insert((pack(&c[0..4]), pack(&c[4..8])));
        }

        // Every matmul-active row's A/B chunk keys must be in the
        // store.
        for row in 8..8 + rows_used {
            let base = row * TOTAL_TRACE_WIDTH;
            let active = trace.matrix.values[base + IS_RESET_CUMSUM].as_canonical_u64()
                + trace.matrix.values[base + IS_UPDATE_CUMSUM].as_canonical_u64();
            if active == 0 {
                continue;
            }
            for j in 0..(A_NOISED_LEN / 2) {
                let key = (
                    trace.matrix.values[base + A_NOISED_START + 2 * j].as_canonical_u64(),
                    trace.matrix.values[base + A_NOISED_START + 2 * j + 1].as_canonical_u64(),
                );
                assert!(store.contains(&key), "A chunk {j}@row {row} ∉ store");
            }
            for j in 0..(B_NOISED_LEN / 2) {
                let key = (
                    trace.matrix.values[base + B_NOISED_START + 2 * j].as_canonical_u64(),
                    trace.matrix.values[base + B_NOISED_START + 2 * j + 1].as_canonical_u64(),
                );
                assert!(store.contains(&key), "B chunk {j}@row {row} ∉ store");
            }
        }
    }

    /// The positioned store layout is a **pure
    /// function of `(t,r,num_stripes,k)`** — the `(side_a,src)`
    /// skeleton is identical for *any* `a′/b′` byte filling, so
    /// the canonical-program rebuild can reconstruct each store
    /// row's `(i,l)` (hence its pinned noise) **witness-free**
    /// (which is what makes the pinned rebuild cheap). And it is
    /// consistent with the deduped store: every
    /// value-deduped `enumerate_noised_chunks` chunk appears as
    /// some positioned row's bytes (so the LogUp producer set is
    /// unchanged — dedup was only a row-count optimization).
    #[test]
    fn a3_2a_positioned_store_layout_is_witness_free_and_consistent() {
        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let mk = |salt: i32| -> Vec<i8> {
            (0..(t * k) as i32)
                .map(|i| (((i.wrapping_mul(7).wrapping_add(salt) ^ (i >> 3)) & 0x7F) - 64) as i8)
                .collect()
        };
        // Two unrelated byte fillings of the same geometry.
        let (a1, b1) = (mk(0), mk(0x11));
        let (a2, b2) = (mk(0x5A), mk(0x77));

        let p1 = CompositeTrace::enumerate_noised_chunks_positioned(&a1, &b1, t, r, num_stripes);
        let p2 = CompositeTrace::enumerate_noised_chunks_positioned(&a2, &b2, t, r, num_stripes);
        assert_eq!(p1.len(), p2.len(), "layout length is params-fixed");
        assert!(!p1.is_empty());
        for (c1, c2) in p1.iter().zip(p2.iter()) {
            // Positions/sides identical (witness-free); only bytes
            // differ between the two fillings.
            assert_eq!(c1.side_a, c2.side_a);
            assert_eq!(c1.src, c2.src);
        }
        // The witness-free skeleton matches the positioned layout.
        let skel = CompositeTrace::noised_store_layout(t, r, num_stripes, k);
        assert_eq!(skel.len(), p1.len());
        for (s, c) in skel.iter().zip(p1.iter()) {
            assert_eq!(*s, (c.side_a, c.src));
        }

        // Dedup consistency: every deduped store chunk is some
        // positioned row's bytes (producer set unchanged).
        let deduped = CompositeTrace::enumerate_noised_chunks(&a1, &b1, t, r, num_stripes);
        let positioned_bytes: std::collections::HashSet<[i8; 8]> =
            p1.iter().map(|c| c.bytes).collect();
        for ch in &deduped {
            assert!(
                positioned_bytes.contains(ch),
                "deduped store chunk missing from positioned layout"
            );
        }
        // Positioned ⊇ deduped (no dedup ⇒ ≥ as many rows).
        assert!(p1.len() >= deduped.len());
    }

    /// **Adversarial**: the sweep input is
    /// genuinely *bound* to the declared `noised_packed` store. A
    /// prover that sweeps a tile whose noised micro-tiles are NOT
    /// the published store (here: store built from the canonical
    /// `a_prime`/`b_prime`, but the sweep run on a *different*,
    /// cheaper tile) leaves the bus unbalanced ⇒ Route-A MUST
    /// reject. (This is the non-vacuity proof for the whole-
    /// micro-tile binding; store ↔ committed-matrix `HASH_A` is a
    /// separately-scoped residual.)
    #[test]
    fn high2_2_swept_tile_not_in_store_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();

        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_canon: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_canon: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();
        // The tile actually swept differs from the published store
        // (a "cheaper"/forged tile the prover would prefer).
        let a_evil: Vec<i8> = a_canon.iter().map(|&v| v ^ 0x5A).collect();
        let b_evil: Vec<i8> = b_canon.iter().map(|&v| v ^ 0x33).collect();

        let sweep_start = 8;
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(sweep_start, &a_evil, &b_evil, t, r, num_stripes);
        // Store published from the CANONICAL tile (≠ swept tile).
        let store_chunks =
            CompositeTrace::enumerate_noised_chunks(&a_canon, &b_canon, t, r, num_stripes);
        let store_start = sweep_start + rows_used;
        let n_store = trace.place_noised_store(store_start, &store_chunks, 0);

        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let fold_start = store_start + n_store + 4;
        let m = trace.place_fold_chain(fold_start, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let res = composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis);
        assert!(
            res.is_err(),
            "swept tile ∉ declared noised store MUST reject \
             (LogUp unbalanced), got Ok"
        );
    }

    /// The canonical small Route-A path must reject when the positioned
    /// `noised_packed` producer store no longer matches the matmul sweep's
    /// consumed A/B chunks.
    #[test]
    fn high2_2_logup_tampered_positioned_store_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();

        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();

        let sweep_start = 8;
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(sweep_start, &a_prime, &b_prime, t, r, num_stripes);

        let store_chunks = CompositeTrace::enumerate_noised_chunks_positioned(
            &a_prime, &b_prime, t, r, num_stripes,
        );
        let store_start = sweep_start + rows_used;
        let max_lane = |side_a: bool| {
            store_chunks
                .iter()
                .filter(|c| c.side_a == side_a)
                .filter_map(|c| c.src.iter().flatten().map(|(lane, _)| *lane as usize).max())
                .max()
                .unwrap_or(0)
        };
        let (a_id_base, b_id_base) =
            crate::composite_trace::noised_id_bases(max_lane(true), max_lane(false), k);
        for (i, chunk) in store_chunks.iter().enumerate() {
            let id_base = if chunk.side_a { a_id_base } else { b_id_base };
            let mat_id = crate::composite_trace::noised_chunk_id(id_base, k, &chunk.src)
                .try_into()
                .expect("positioned noised chunk id must fit in MAT_ID");
            let mut wrong = chunk.bytes;
            wrong[0] = 64;
            trace.place_noised_store_row(store_start + i, &wrong, mat_id);
        }

        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let fold_start = store_start + store_chunks.len() + 4;
        let m = trace.place_fold_chain(fold_start, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let program = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned_logup(&cfg, &program, &proof, &pis).is_err(),
            "tampered positioned producers must unbalance the noised_packed bus"
        );
    }

    /// **Fold keystone — explicit consumer-side tamper.**
    ///
    /// The keystone constraint (`composite_full_air.rs:318-334`)
    /// binds the FoldChip's `FOLD_XSTEP` to the StripeXorChip's
    /// `SX_XR[stripe]` lane selected by the verifier-fixed
    /// `FOLD_STRIPE_SEL` (one-hot, packed into the pinned
    /// `CONTROL_PREP`):
    ///
    /// ```text
    /// Σ_s FOLD_STRIPE_SEL[s] · (FOLD_XSTEP − SX_XR[s]) = 0   (sx_bound=true)
    /// ```
    ///
    /// Positive control: `high2_2_fold_chain_pinned_logup` — the happy
    /// path where `FOLD_XSTEP == SX_XR[slot]` at every fold-active row.
    /// This test builds the *same* trace, then tampers fold-row 0's
    /// `FOLD_XSTEP` cell to point at `SX_XR[1]` (a DIFFERENT lane than
    /// the one-hot `FOLD_STRIPE_SEL[0]=1` claims). The keystone
    /// constraint becomes `1 · (SX_XR[1] − SX_XR[0]) ≠ 0` ⇒ AIR-eval
    /// rejection at
    /// `composite_verify_pinned_logup`.
    ///
    /// This is the explicit keystone regression for stripe-pin binding: a wrong
    /// selected lane must reject at `composite_verify_pinned_logup`.
    #[test]
    fn high2_2_g2_xstep_stripe_pin_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            FOLD_SLOT_SEL_START, FOLD_XSTEP, SX_XR_START, TOTAL_TRACE_WIDTH,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();

        // Same geometry as the positive control:
        // MatmulParams::TEST_SMALL (t=8, k=64, r=4, num_stripes=16).
        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();

        let sweep_start = 8;
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(sweep_start, &a_prime, &b_prime, t, r, num_stripes);

        let store_chunks =
            CompositeTrace::enumerate_noised_chunks(&a_prime, &b_prime, t, r, num_stripes);
        let store_start = sweep_start + rows_used;
        let n_store = trace.place_noised_store(store_start, &store_chunks, 0);

        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let fold_start = store_start + n_store + 4;
        let m = trace.place_fold_chain(fold_start, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        // === Fold-keystone tamper: fold-row 0 has FOLD_SLOT_SEL[0]=1
        // (one-hot stripe = 0), so the constraint asserts
        //   FOLD_XSTEP == SX_XR[0].
        // Tamper: replace FOLD_XSTEP with a value guaranteed to
        // differ from SX_XR[0] (its honest value + 1, in the field).
        // The constraint
        //   Σ_s FOLD_STRIPE_SEL[s] · (FOLD_XSTEP − SX_XR[s])
        // = FOLD_STRIPE_SEL[0] · ((SX_XR[0] + 1) − SX_XR[0])
        // = 1 ≠ 0
        // ⇒ AIR eval() violation at verify.
        //
        // We tamper "+1" rather than "= SX_XR[other_lane]" because
        // the SX_XR lanes at fold-row 0 can happen to be equal
        // (e.g., when the synthetic XOR cancels both lanes to 0 in
        // this small test geometry).
        let tampered_row = fold_start; // step 0; slot 0 per the positive control's invariant.
        let base = tampered_row * TOTAL_TRACE_WIDTH;

        // Sanity: confirm the row is fold-active on slot 0 before tampering.
        let mut slot_check = usize::MAX;
        for s in 0..16 {
            if trace.matrix.values[base + FOLD_SLOT_SEL_START + s].as_canonical_u64() == 1 {
                slot_check = s;
            }
        }
        assert_eq!(
            slot_check, 0,
            "keystone tamper-test precondition: row 0 of fold must be one-hot on slot 0"
        );

        // Sanity: confirm FOLD_XSTEP == SX_XR[0] before tampering
        // (i.e., the keystone is satisfied honestly).
        let sx_xr_correct = trace.matrix.values[base + SX_XR_START + 0];
        let fold_xstep_honest = trace.matrix.values[base + FOLD_XSTEP];
        assert_eq!(
            fold_xstep_honest.as_canonical_u64(),
            sx_xr_correct.as_canonical_u64(),
            "keystone tamper-test precondition: honest FOLD_XSTEP must == SX_XR[0]"
        );

        // The tamper: FOLD_XSTEP ← SX_XR[0] + 1 (in Goldilocks).
        // Guaranteed to differ from SX_XR[0] because 1 ≠ 0 in the
        // field (Goldilocks has characteristic > 1).
        let tampered_value =
            sx_xr_correct + <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(1);
        // Strict inequality sanity (1 != 0 in Goldilocks).
        assert_ne!(
            tampered_value.as_canonical_u64(),
            sx_xr_correct.as_canonical_u64(),
            "keystone tamper-test internal: +1 in Goldilocks must change value",
        );
        trace.matrix.values[base + FOLD_XSTEP] = tampered_value;

        // Derive PIs + canonical from the tampered trace. FOLD_XSTEP
        // is not in PROGRAM_COLS (the pin set is row
        // metadata: control, noise pins, CV/tweak, A/B IDs, and the
        // row index). Canonical is unchanged; PIs are unchanged (they bind HASH_A/B,
        // HASH_JACKPOT, key-pin rows). The mismatch surfaces purely as
        // an AIR-constraint failure on the tampered row.
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let result = composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis);
        assert!(
            result.is_err(),
            "fold keystone tamper (FOLD_XSTEP retargeted to SX_XR[1] != SX_XR[0]) MUST reject",
        );
    }

    /// **Fold keystone — PRODUCER-SIDE tamper.**
    ///
    /// Cross-AIR composition test.
    /// The keystone binds FoldChip's `FOLD_XSTEP` (consumer side)
    /// to StripeXorChip's `SX_XR[stripe]` (producer side). The
    /// consumer-side test `high2_2_g2_xstep_stripe_pin_rejects`
    /// tampers `FOLD_XSTEP`. This test exercises the *opposite*
    /// direction: tamper `SX_XR[0]` at the fold row while leaving
    /// `FOLD_XSTEP` honest. The keystone constraint
    ///   `Σ_s FOLD_STRIPE_SEL[s] · (FOLD_XSTEP − SX_XR[s]) = 0`
    /// becomes `1 · (FOLD_XSTEP − (SX_XR[0] + 1)) = −1 ≠ 0` ⇒ AIR-eval rejection.
    ///
    /// **Defense-in-depth note.** Tampering `SX_XR[0]`
    /// at one row also violates StripeXorChip's row-to-row
    /// passthrough constraint (StripeXor is inactive at fold rows,
    /// so the carry-forward `SX_XR[i+1] == SX_XR[i]` is enforced).
    /// Either rejection mechanism catches the tamper. This test
    /// asserts rejection without claiming which constraint fires
    /// first; the *cross-AIR claim* is that the keystone binding is
    /// **symmetric** — tampering the producer or the consumer both
    /// reject, demonstrating the bidirectional integrity of the
    /// FoldChip ↔ StripeXorChip soundness boundary.
    #[test]
    fn high2_2_g2_sx_xr_producer_side_tamper_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{FOLD_SLOT_SEL_START, SX_XR_START, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();

        let (t, k, r, num_stripes) = (8usize, 64usize, 4usize, 16usize);
        let a_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(7) ^ (i >> 3)) & 0x7F) - 64) as i8)
            .collect();
        let b_prime: Vec<i8> = (0..(t * k) as i32)
            .map(|i| (((i.wrapping_mul(5) ^ (i << 1) ^ 0x2A) & 0x7F) - 64) as i8)
            .collect();

        let sweep_start = 8;
        let (rows_used, x_steps) =
            trace.place_useful_work_chain(sweep_start, &a_prime, &b_prime, t, r, num_stripes);

        let store_chunks =
            CompositeTrace::enumerate_noised_chunks(&a_prime, &b_prime, t, r, num_stripes);
        let store_start = sweep_start + rows_used;
        let n_store = trace.place_noised_store(store_start, &store_chunks, 0);

        let xs: Vec<i32> = x_steps[..num_stripes].iter().map(|&u| u as i32).collect();
        let fold_start = store_start + n_store + 4;
        let m = trace.place_fold_chain(fold_start, &xs);
        let _ = trace.place_jackpot_hash_block(h - 8, &m, &ch);

        // Tamper SX_XR[0] at fold-row 0 by +1 in Goldilocks. This
        // exercises the keystone's producer-side path (and incidentally
        // violates StripeXor's passthrough; both ⇒ AIR-eval rejection).
        let tampered_row = fold_start;
        let base = tampered_row * TOTAL_TRACE_WIDTH;

        // Sanity: confirm fold-row 0 is one-hot on slot 0.
        let mut slot_check = usize::MAX;
        for s in 0..16 {
            if trace.matrix.values[base + FOLD_SLOT_SEL_START + s].as_canonical_u64() == 1 {
                slot_check = s;
            }
        }
        assert_eq!(
            slot_check, 0,
            "producer-side keystone tamper: row 0 must be one-hot on slot 0"
        );

        let sx_xr_honest = trace.matrix.values[base + SX_XR_START + 0];
        let tampered = sx_xr_honest + <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(1);
        trace.matrix.values[base + SX_XR_START + 0] = tampered;

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let result = composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis);
        assert!(
            result.is_err(),
            "tampered SX_XR[0] (producer side of the fold keystone) MUST reject"
        );
    }

    // ───────────── Malicious-prover regression suite ─────────────
    //
    // A malicious prover can zero every
    // selector to vacate the C1/C3/C4 PI bindings and forge a
    // winning proof with no work. These tests assert the
    // program-pinned API closes that: a proof only verifies against
    // the *canonical* program's verifying key (rebuilt by the
    // verifier from the trusted shape, never from the proof).

    /// Build a representative honest/canonical trace: matrix-hash
    /// A/B (C3) + key-pin rows (C1) + final jackpot-hash block
    /// (C4). Mirrors `ai-pow::zk_bridge`'s construction. Returns
    /// the trace; `extract_program` of it is the canonical program.
    fn honest_trace() -> CompositeTrace {
        let kappa = [0xA5u8; 32];
        // JOB_KEY must equal the matrix-commitment key: the pinned AIR
        // binds the keyed compressions' CV_IN to PI_JOB_KEY.
        let jk: [u32; 8] = core::array::from_fn(|_| 0xA5A5_A5A5);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let a = vec![0x11u8; 1024];
        let b = vec![0x22u8; 1024];
        let mut t = CompositeTrace::baseline_min();
        let h = t.height();
        let (n1, _) = t.place_matrix_hash_a(0, &a, &kappa);
        let (mh_end, _) = t.place_matrix_hash_b(n1, &b, &kappa);
        t.place_key_pin_row(mh_end + 1, false, &jk);
        t.place_key_pin_row(mh_end + 2, true, &ch);
        // jackpot-hash keyed by COMMITMENT_HASH (= `ch` words).
        t.place_jackpot_hash_block(h - 8, &[0u32; 16], &ch);
        t
    }

    fn c3_activated_matrix_trace() -> CompositeTrace {
        let kappa = [0x3Cu8; 32];
        let matrix: Vec<u8> = (0..1024).map(|i| (i % 65) as u8).collect();
        let noise = vec![0i8; matrix.len()];
        let mut t = CompositeTrace::baseline_min();
        let (next, root) = t.place_matrix_strip_opening(
            0,
            &matrix,
            0,
            1,
            1,
            &[],
            &kappa,
            4, // IS_HASH_A
            Some(&noise),
            Some(crate::composite_trace::NOISED_CHUNK_ID_BASE),
        );
        // The strip opening's chunk-start block is κ-keyed; pin JOB_KEY = κ
        // so PI_JOB_KEY matches the keyed compression's CV_IN.
        let kappa_w: [u32; 8] = core::array::from_fn(|_| 0x3C3C_3C3C);
        t.place_key_pin_row(next, false, &kappa_w);
        let pis = CompositePublicInputs::derive_from_trace(&t);
        assert_eq!(pis.hash_a, root, "HASH_A binds the co-located strip root");
        t
    }

    /// Honest pinned round-trip verifies; the difficulty check is
    /// real (a 0 target rejects the non-zero keyed digest).
    #[test]
    fn crit1_honest_pinned_roundtrip() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = honest_trace();
        let canonical = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_ne!(pis.hash_jackpot, [0u32; 8], "C4 digest non-vacuous");

        let (proof, prog) = composite_prove_pinned(&cfg, trace, &pis);
        assert_eq!(prog.values, canonical.values, "prover program == canonical");

        composite_verify_pinned(&cfg, &canonical, &proof, &pis)
            .expect("honest pinned proof must verify against canonical program");
        composite_verify_pow_pinned(&cfg, &canonical, &proof, &pis, &[0xFFu8; 32])
            .expect("clears an easy target");
        // Hardest target 0: BLAKE3(0,key) > 0 ⇒ difficulty not met.
        match composite_verify_pow_pinned(&cfg, &canonical, &proof, &pis, &[0u8; 32]) {
            Err(PowVerifyError::DifficultyNotMet) => {}
            other => panic!("expected DifficultyNotMet, got {other:?}"),
        }
    }

    /// The zeroed-selector exploit, blocked by pinning. A malicious prover submits
    /// an all-zero-selector trace (no matmul, no hashing, no work)
    /// with a forged winning `HASH_JACKPOT = 0`. It is
    /// self-consistent against its *own* (all-zero) program, but
    /// the verifier uses the **canonical** program's VK — the
    /// preprocessed commitment differs, so verification fails.
    #[test]
    fn crit1_zeroed_selector_forgery_rejected() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        // Canonical program the honest verifier trusts.
        let canonical = extract_program(&honest_trace().matrix);

        // Malicious: baseline (all selectors 0) + forged zero PIs
        // (HASH_JACKPOT = 0 ≤ any target).
        let evil = CompositeTrace::baseline_min();
        let forged = CompositePublicInputs::zero();
        let (evil_proof, evil_prog) = composite_prove_pinned(&cfg, evil, &forged);
        assert_ne!(
            evil_prog.values, canonical.values,
            "attacker's program differs from canonical"
        );

        // Self-consistent against the attacker's own program
        // (the AIR is satisfied for an all-zero schedule) — this
        // is exactly why pinning to a *trusted* program matters.
        composite_verify_pinned(&cfg, &evil_prog, &evil_proof, &forged)
            .expect("attacker proof is self-consistent vs its own program");

        // Against the canonical (trusted) program: REJECTED.
        assert!(
            composite_verify_pinned(&cfg, &canonical, &evil_proof, &forged).is_err(),
            "forged proof must fail against the canonical program VK"
        );
        assert!(
            composite_verify_pow_pinned(&cfg, &canonical, &evil_proof, &forged, &[0xFFu8; 32])
                .is_err(),
            "forged winning PoW must be rejected"
        );
    }

    /// Tampering any PROGRAM_COL in an otherwise-honest trace
    /// changes the prover's committed program; verification
    /// against the canonical program rejects it.
    #[test]
    fn crit1_tampered_program_col_rejected() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_full_air::PROGRAM_COLS;
        use crate::composite_layout::TOTAL_TRACE_WIDTH;

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let canonical = extract_program(&honest_trace().matrix);

        for &col in &PROGRAM_COLS {
            let mut t = honest_trace();
            // Tamper this program column on a mid-trace row.
            let r = 50usize;
            let cur = t.matrix.values[r * TOTAL_TRACE_WIDTH + col].as_canonical_u64();
            t.matrix.values[r * TOTAL_TRACE_WIDTH + col] =
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur.wrapping_add(1));
            let pis = CompositePublicInputs::derive_from_matrix(&t.matrix);
            let (proof, _) = composite_prove_pinned(&cfg, t, &pis);
            assert!(
                composite_verify_pinned(&cfg, &canonical, &proof, &pis).is_err(),
                "tampered PROGRAM_COL {col} must be rejected vs canonical program"
            );
        }
    }

    /// Even with the *correct* canonical program, a prover cannot
    /// forge `HASH_JACKPOT`: with selectors pinned, IS_HASH_JACKPOT
    /// fires on the jackpot-hash row and the C4 binding forces
    /// CV_OUT == PI_HASH_JACKPOT (the real non-zero keyed digest),
    /// so a swapped-to-zero PI violates the constraint.
    #[test]
    fn crit1_forged_hash_jackpot_with_canonical_program_rejected() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = honest_trace();
        let canonical = extract_program(&trace.matrix);
        let mut pis = CompositePublicInputs::derive_from_trace(&trace);
        pis.hash_jackpot = [0u32; 8]; // forge a trivially-winning value

        let (proof, _) = composite_prove_pinned(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned(&cfg, &canonical, &proof, &pis).is_err(),
            "forged HASH_JACKPOT must fail the C4 binding under the pinned program"
        );
    }

    /// The C4-hashed `JACKPOT_MSG` is no longer
    /// prover-free — the pinned AIR forces last-row
    /// `JACKPOT_MSG[0..4] == CUMSUM_TILE[0..4]` (matmul-bound) and
    /// `JACKPOT_MSG[4..16] == 0`. An attacker who grinds an
    /// arbitrary winning jackpot message (the old hashcash attack,
    /// no matmul) is rejected: the planted message no longer
    /// equals the bound accumulator.
    #[test]
    fn high2_free_jackpot_message_rejected() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{JACKPOT_MSG_START, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        // Honest baseline: CUMSUM = 0, JACKPOT_MSG = 0 on the last
        // row ⇒ keystone holds (0 == 0); HASH_JACKPOT =
        // BLAKE3(0, key=jackpot_key), a fixed value the attacker cannot
        // grind. Sanity: verifies.
        let ok = honest_trace();
        let canonical = extract_program(&ok.matrix);
        let pis_ok = CompositePublicInputs::derive_from_trace(&ok);
        let (p_ok, _) = composite_prove_pinned(&cfg, ok, &pis_ok);
        composite_verify_pinned(&cfg, &canonical, &p_ok, &pis_ok)
            .expect("zero-CUMSUM honest trace satisfies the keystone");

        // Attack: plant a "winning" free jackpot message on the
        // last row while CUMSUM stays 0 — exactly the pre-keystone
        // hashcash forge (no matmul). Keystone JACKPOT_MSG ==
        // CUMSUM is violated ⇒ must be rejected.
        let mut evil = honest_trace();
        let h = evil.height();
        let last = (h - 1) * TOTAL_TRACE_WIDTH;
        evil.matrix.values[last + JACKPOT_MSG_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0xDEAD_BEEFu64);
        let pis_evil = CompositePublicInputs::derive_from_matrix(&evil.matrix);
        let (p_evil, _) = composite_prove_pinned(&cfg, evil, &pis_evil);
        assert!(
            composite_verify_pinned(&cfg, &canonical, &p_evil, &pis_evil).is_err(),
            "a free (non-CUMSUM) jackpot message must be rejected"
        );
    }

    // ───────── Route-A production suite ─────────
    //
    // The malicious-prover / free-jackpot adversarial regressions,
    // re-run against
    // the batch-stark pinned+LogUp path (`*_pinned_logup`). These
    // prove the production Route-A binding keeps the canonical-program
    // pin and the jackpot keystone *while additionally enforcing the
    // noised_packed/range LogUp*. The noised_packed matmul-input
    // binding is non-vacuous only when the statement places matmul rows.

    /// Honest pinned+LogUp round-trip verifies; the C2 difficulty
    /// check is real (0 target rejects the non-zero keyed digest,
    /// an all-FF target clears it).
    #[test]
    fn routea_honest_roundtrip_and_pow() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = honest_trace();
        let canonical = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_ne!(pis.hash_jackpot, [0u32; 8], "C4 digest non-vacuous");

        let (proof, prog) = composite_prove_pinned_logup(&cfg, trace, &pis);
        assert_eq!(prog.values, canonical.values, "prover program == canonical");

        composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis)
            .expect("Route-A honest pinned+LogUp proof must verify");
        composite_verify_pow_pinned_logup(&cfg, &canonical, &proof, &pis, &[0xFFu8; 32])
            .expect("clears an easy target");
        match composite_verify_pow_pinned_logup(&cfg, &canonical, &proof, &pis, &[0u8; 32]) {
            Err(PowVerifyError::DifficultyNotMet) => {}
            other => panic!("expected DifficultyNotMet, got {other:?}"),
        }
    }

    /// HASH_A/HASH_B are statement fields, not prover-selected metadata.
    /// Route-A verification must reject proofs whose matrix-hash public
    /// inputs differ from the pinned matrix-hash rows.
    #[test]
    fn routea_matrix_hash_public_inputs_tamper_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        let trace_a = honest_trace();
        let canonical_a = extract_program(&trace_a.matrix);
        let mut pis_a = CompositePublicInputs::derive_from_trace(&trace_a);
        assert_ne!(pis_a.hash_a, [0u32; 8], "HASH_A fixture is non-vacuous");
        pis_a.hash_a[0] ^= 1;
        let (proof_a, _) = composite_prove_pinned_logup(&cfg, trace_a, &pis_a);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical_a, &proof_a, &pis_a).is_err(),
            "tampered HASH_A public input must reject under Route A"
        );

        let trace_b = honest_trace();
        let canonical_b = extract_program(&trace_b.matrix);
        let mut pis_b = CompositePublicInputs::derive_from_trace(&trace_b);
        assert_ne!(pis_b.hash_b, [0u32; 8], "HASH_B fixture is non-vacuous");
        pis_b.hash_b[0] ^= 1;
        let (proof_b, _) = composite_prove_pinned_logup(&cfg, trace_b, &pis_b);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical_b, &proof_b, &pis_b).is_err(),
            "tampered HASH_B public input must reject under Route A"
        );
    }

    /// JOB_KEY and COMMITMENT_HASH are bound on their BLAKE3 key rows.
    #[test]
    fn routea_c1_public_inputs_tamper_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        let trace_job = honest_trace();
        let canonical_job = extract_program(&trace_job.matrix);
        let mut pis_job = CompositePublicInputs::derive_from_trace(&trace_job);
        assert_ne!(pis_job.job_key, [0u32; 8], "JOB_KEY fixture is non-vacuous");
        pis_job.job_key[0] ^= 1;
        let (proof_job, _) = composite_prove_pinned_logup(&cfg, trace_job, &pis_job);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical_job, &proof_job, &pis_job).is_err(),
            "tampered JOB_KEY public input must reject under Route A"
        );

        let trace_commitment = honest_trace();
        let canonical_commitment = extract_program(&trace_commitment.matrix);
        let mut pis_commitment = CompositePublicInputs::derive_from_trace(&trace_commitment);
        assert_ne!(
            pis_commitment.commitment_hash, [0u32; 8],
            "COMMITMENT_HASH fixture is non-vacuous"
        );
        pis_commitment.commitment_hash[0] ^= 1;
        let (proof_commitment, _) =
            composite_prove_pinned_logup(&cfg, trace_commitment, &pis_commitment);
        assert!(
            composite_verify_pinned_logup(
                &cfg, &canonical_commitment, &proof_commitment, &pis_commitment
            )
            .is_err(),
            "tampered COMMITMENT_HASH public input must reject under Route A"
        );
    }

    /// C1-keyed adversarial — the matrix-commitment compressions must
    /// hash with the statement's κ, never a prover-chosen key. The
    /// `IS_JOB_KEYED` rows (every chunk's block 0 + every chunk-Merkle
    /// parent) bind the compression `CV_IN` to `PI_JOB_KEY`; a trace
    /// whose chunk-Merkle runs under a forged key while the key-pin row
    /// claims κ violates that binding and must reject on both pinned
    /// tiers. Four chunks ⇒ the chunk-start block AND the three parent
    /// compressions are exercised.
    #[test]
    fn routea_matrix_commitment_keyed_by_job_key() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let kappa = [0xA5u8; 32];
        let forged = [0x11u8; 32];
        let jk: [u32; 8] = core::array::from_fn(|_| 0xA5A5_A5A5);
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let a = vec![0x11u8; 4096];
        let b = vec![0x22u8; 4096];

        let build = |key: &[u8; 32]| {
            let mut t = CompositeTrace::baseline_min();
            let h = t.height();
            let (n1, _) = t.place_matrix_hash_a(0, &a, key);
            let (mh_end, _) = t.place_matrix_hash_b(n1, &b, key);
            t.place_key_pin_row(mh_end + 1, false, &jk);
            t.place_key_pin_row(mh_end + 2, true, &ch);
            t.place_jackpot_hash_block(h - 8, &[0u32; 16], &ch);
            t
        };

        // Honest control: compression key == pinned JOB_KEY ⇒ verifies.
        let honest = build(&kappa);
        let canonical = extract_program(&honest.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&honest);
        let (proof, prog) = composite_prove_pinned_logup(&cfg, honest, &pis);
        assert_eq!(prog.values, canonical.values, "prover program == canonical");
        composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis)
            .expect("honest κ-keyed commitment must verify");

        // Adversarial: compressions keyed by `forged` while the statement
        // pins κ. The only AIR violation is the IS_JOB_KEYED key binding
        // (the forged root flows consistently into HASH_A/HASH_B).
        let evil = build(&forged);
        let canonical_evil = extract_program(&evil.matrix);
        let pis_evil = CompositePublicInputs::derive_from_trace(&evil);
        assert_ne!(
            pis_evil.hash_a, pis.hash_a,
            "forged key must change the chunk-Merkle root"
        );
        let (evil_proof, _) = composite_prove_pinned_logup(&cfg, evil.clone(), &pis_evil);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical_evil, &evil_proof, &pis_evil).is_err(),
            "a matrix commitment keyed by a prover-chosen key must reject (pinned+LogUp)"
        );
        let (evil_proof_uni, _) = composite_prove_pinned(&cfg, evil, &pis_evil);
        assert!(
            composite_verify_pinned(&cfg, &canonical_evil, &evil_proof_uni, &pis_evil).is_err(),
            "a matrix commitment keyed by a prover-chosen key must reject (uni-stark pinned)"
        );
    }

    #[test]
    fn routea_jackpot_hash_uses_public_commitment_key() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{CV_IN_START, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = honest_trace();
        let canonical = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let last = (trace.height() - 1) * TOTAL_TRACE_WIDTH;
        let cur = trace.matrix.values[last + CV_IN_START].as_canonical_u64();
        trace.matrix.values[last + CV_IN_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur ^ 1);

        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis).is_err(),
            "jackpot BLAKE3 must use the public COMMITMENT_HASH key"
        );
    }

    /// HASH_JACKPOT is a statement field bound by the jackpot-hash row,
    /// independent of the outer difficulty comparison.
    #[test]
    fn routea_hash_jackpot_public_input_tamper_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = honest_trace();
        let canonical = extract_program(&trace.matrix);
        let mut pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_ne!(
            pis.hash_jackpot, [0u32; 8],
            "HASH_JACKPOT fixture is non-vacuous"
        );
        pis.hash_jackpot[0] ^= 1;
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis).is_err(),
            "tampered HASH_JACKPOT public input must reject under Route A"
        );
    }

    /// The last row owns every cumsum and jackpot public-input lane.
    #[test]
    fn routea_final_boundary_public_inputs_tamper_rejects() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        for lane in 0..4 {
            let trace = honest_trace();
            let canonical = extract_program(&trace.matrix);
            let mut pis = CompositePublicInputs::derive_from_trace(&trace);
            pis.cumsum[lane] ^= 1;
            let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
            assert!(
                composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis).is_err(),
                "tampered cumsum public input lane {lane} must reject under Route A"
            );
        }

        for lane in 0..16 {
            let trace = honest_trace();
            let canonical = extract_program(&trace.matrix);
            let mut pis = CompositePublicInputs::derive_from_trace(&trace);
            pis.jackpot[lane] ^= 1;
            let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
            assert!(
                composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis).is_err(),
                "tampered jackpot public input lane {lane} must reject under Route A"
            );
        }
    }

    /// Co-located matrix leaf rows make C3 live under the pinned Route-A
    /// verifier: changing the byte view while keeping the BLAKE3 message
    /// fixed must reject.
    #[test]
    fn routea_c3_colocated_matrix_row_tamper_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            IS_MSG_MAT, IS_NEW_BLAKE, MAT_UNPACK_START, NOISED_PACKED_START, TOTAL_TRACE_WIDTH,
            UINT8_DATA_START,
        };

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = c3_activated_matrix_trace();
        let program = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);

        let active_row = (0..trace.height())
            .find(|&r| {
                let base = r * TOTAL_TRACE_WIDTH;
                trace.matrix.values[base + IS_MSG_MAT].as_canonical_u64() == 1
                    && trace.matrix.values[base + IS_NEW_BLAKE].as_canonical_u64() == 1
            })
            .expect("co-located matrix trace has a live C3 row");

        let (ok_proof, _) = composite_prove_pinned_logup(&cfg, trace.clone(), &pis);
        composite_verify_pinned_logup(&cfg, &program, &ok_proof, &pis)
            .expect("co-located C3 trace must verify under Route A");

        let mut evil = trace;
        let base = active_row * TOTAL_TRACE_WIDTH;
        evil.matrix.values[base + MAT_UNPACK_START] =
            <Val<AiPowStarkConfig> as QuotientMap<i64>>::from_int(9);
        evil.matrix.values[base + UINT8_DATA_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(9);
        let packed = 9 + 1 * 256 + 2 * 256 * 256 + 3 * 256 * 256 * 256;
        evil.matrix.values[base + NOISED_PACKED_START] =
            <Val<AiPowStarkConfig> as QuotientMap<i64>>::from_int(packed);

        let (bad_proof, _) = composite_prove_pinned_logup(&cfg, evil, &pis);
        assert!(
            composite_verify_pinned_logup(&cfg, &program, &bad_proof, &pis).is_err(),
            "C3 must bind the matrix byte view to the BLAKE3 message under Route A"
        );
    }

    #[test]
    fn routea_active_cv_route_tamper_rejects() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{CV_IN_START, IS_CV_IN, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = c3_activated_matrix_trace();
        let program = extract_program(&trace.matrix);
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let cv_row = (0..trace.height())
            .find(|&r| {
                trace.matrix.values[r * TOTAL_TRACE_WIDTH + IS_CV_IN].as_canonical_u64() == 1
            })
            .expect("co-located matrix trace has an active CV route");
        let cell = cv_row * TOTAL_TRACE_WIDTH + CV_IN_START;
        let cur = trace.matrix.values[cell].as_canonical_u64();
        trace.matrix.values[cell] = <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur ^ 1);

        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        assert!(
            composite_verify_pinned_logup(&cfg, &program, &proof, &pis).is_err(),
            "matrix BLAKE3 chaining must consume an active CV-routing bus value"
        );
    }

    #[test]
    fn routea_malformed_program_returns_error_not_panic() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = honest_trace();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);

        let bad_width = p3_matrix::dense::RowMajorMatrix::new(vec![Default::default(); 8], 1);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            composite_verify_pinned_logup(&cfg, &bad_width, &proof, &pis)
        }));
        assert!(result.is_ok(), "malformed program must not panic verifier");
        assert!(matches!(
            result.unwrap(),
            Err(CompositeVerificationError::InvalidProgram(
                ProgramShapeError::WidthMismatch { expected, actual: 1 }
            )) if expected == crate::composite_full_air::PROGRAM_COLS.len()
        ));

        let bad_height = p3_matrix::dense::RowMajorMatrix::new(
            vec![Default::default(); crate::composite_full_air::PROGRAM_COLS.len() * 3],
            crate::composite_full_air::PROGRAM_COLS.len(),
        );
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            composite_verify_pow_pinned_logup(&cfg, &bad_height, &proof, &pis, &[0xffu8; 32])
        }));
        assert!(
            result.is_ok(),
            "malformed program must not panic PoW verifier"
        );
        assert!(matches!(
            result.unwrap(),
            Err(PowVerifyError::Stark(
                CompositeVerificationError::InvalidProgram(
                    ProgramShapeError::HeightNotPowerOfTwo { height: 3 }
                )
            ))
        ));
    }

    /// Pinning under Route A: a zeroed-selector forgery is
    /// self-consistent vs its own program but REJECTED vs the
    /// canonical program's preprocessed commitment.
    #[test]
    fn routea_crit1_zeroed_selector_forgery_rejected() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let canonical = extract_program(&honest_trace().matrix);

        let evil = CompositeTrace::baseline_min();
        let forged = CompositePublicInputs::zero();
        let (evil_proof, evil_prog) = composite_prove_pinned_logup(&cfg, evil, &forged);
        assert_ne!(evil_prog.values, canonical.values);

        composite_verify_pinned_logup(&cfg, &evil_prog, &evil_proof, &forged)
            .expect("evil proof self-consistent vs its own program");
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical, &evil_proof, &forged).is_err(),
            "Route A: forged proof must fail vs canonical program"
        );
        assert!(
            composite_verify_pow_pinned_logup(
                &cfg, &canonical, &evil_proof, &forged, &[0xFFu8; 32]
            )
            .is_err(),
            "Route A: forged winning PoW rejected"
        );
    }

    /// Tampering any PROGRAM_COL is rejected vs the canonical
    /// program under Route A (full coverage of all PROGRAM_COLS).
    #[test]
    fn routea_crit1_tampered_program_col_rejected() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_full_air::PROGRAM_COLS;
        use crate::composite_layout::TOTAL_TRACE_WIDTH;

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let canonical = extract_program(&honest_trace().matrix);

        for &col in &PROGRAM_COLS {
            let mut t = honest_trace();
            let r = 50usize;
            let cur = t.matrix.values[r * TOTAL_TRACE_WIDTH + col].as_canonical_u64();
            t.matrix.values[r * TOTAL_TRACE_WIDTH + col] =
                <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur.wrapping_add(1));
            let pis = CompositePublicInputs::derive_from_matrix(&t.matrix);
            let (proof, _) = composite_prove_pinned_logup(&cfg, t, &pis);
            assert!(
                composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis).is_err(),
                "Route A: tampered PROGRAM_COL {col} must be rejected vs canonical"
            );
        }
    }

    /// **MAT_FREQ producer-planting adversarial test.**
    /// `MAT_FREQ` is a free witness column. A malicious miner could
    /// try to "plant" a phantom `noised_packed` producer — publish a
    /// `(MAT_ID, NOISED_PACKED)` table entry with no balancing
    /// consumer — by inflating a `MAT_FREQ` cell. A sound proof MUST
    /// reject: `MAT_FREQ` feeds *only* the `noised_packed` LogUp, so
    /// any value other than the one `populate_lookup_freq` computes
    /// leaves the bus's global sum non-zero ⇒ the LogUp argument
    /// fails to close ⇒ reject.
    ///
    /// The honest prover recomputes `MAT_FREQ` via
    /// `populate_lookup_freq`, so this inlines the prove and tampers
    /// `MAT_FREQ` *after* that step — exactly what a malicious
    /// prover, controlling its own frequency pass, would do.
    #[test]
    fn sec_4c10_mat_freq_planted_producer_rejected() {
        use p3_batch_stark::{prove_batch, ProverData, StarkInstance};
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_full_air_with_lookups::CompositeFullAirWithLookupsPinned;
        use crate::composite_layout::MAT_FREQ;

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let pis = CompositePublicInputs::derive_from_matrix(&honest_trace().matrix);

        // Honest control: an untampered trace proves + verifies.
        let (ok_proof, ok_prog) = composite_prove_pinned_logup(&cfg, honest_trace(), &pis);
        composite_verify_pinned_logup(&cfg, &ok_prog, &ok_proof, &pis)
            .expect("honest trace must verify");

        // Attack: run the honest `populate_lookup_freq`, then plant a
        // phantom producer by bumping `MAT_FREQ[0]`; prove WITHOUT
        // re-populating (the malicious prover keeps its forged freq).
        let mut t = honest_trace();
        t.populate_lookup_freq();
        let cur = t.matrix.values[MAT_FREQ].as_canonical_u64();
        t.matrix.values[MAT_FREQ] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(cur.wrapping_add(1));
        let program = extract_program(&t.matrix);
        let air = CompositeFullAirWithLookupsPinned::new_with(program.clone(), true);
        let instances = vec![StarkInstance {
            air: &air,
            trace: &t.matrix,
            public_values: pis.to_vec(),
        }];
        let pd = ProverData::from_instances(&cfg, &instances);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let proof = prove_batch(&cfg, &instances, &pd);
            composite_verify_pinned_logup(&cfg, &program, &proof, &pis)
        }));
        match res {
            Ok(Ok(())) => panic!(
                "a planted noised_packed producer (inflated \
                 MAT_FREQ) was ACCEPTED — the bus does not bind \
                 producer multiplicity"
            ),
            Ok(Err(_)) | Err(_) => { /* rejected — correct */ }
        }
    }

    /// The keystone holds under Route A: a free (non-CUMSUM)
    /// winning jackpot message is rejected.
    #[test]
    fn routea_high2_free_jackpot_message_rejected() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{JACKPOT_MSG_START, TOTAL_TRACE_WIDTH};

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        let ok = honest_trace();
        let canonical = extract_program(&ok.matrix);
        let pis_ok = CompositePublicInputs::derive_from_trace(&ok);
        let (p_ok, _) = composite_prove_pinned_logup(&cfg, ok, &pis_ok);
        composite_verify_pinned_logup(&cfg, &canonical, &p_ok, &pis_ok)
            .expect("zero-CUMSUM honest trace satisfies the keystone under Route A");

        let mut evil = honest_trace();
        let h = evil.height();
        let last = (h - 1) * TOTAL_TRACE_WIDTH;
        evil.matrix.values[last + JACKPOT_MSG_START] =
            <Val<AiPowStarkConfig> as QuotientMap<u64>>::from_int(0xDEAD_BEEFu64);
        let pis_evil = CompositePublicInputs::derive_from_matrix(&evil.matrix);
        let (p_evil, _) = composite_prove_pinned_logup(&cfg, evil, &pis_evil);
        assert!(
            composite_verify_pinned_logup(&cfg, &canonical, &p_evil, &pis_evil).is_err(),
            "Route A: free jackpot message must be rejected"
        );
    }

    #[test]
    fn hash_jackpot_le_bytes_is_blake3_digest_order() {
        // word i ↦ bytes[4i..4i+4] little-endian — the inverse of
        // `u32::from_le_bytes([digest[4i..4i+4]])`.
        let hj: [u32; 8] = [
            0x04030201, 0x08070605, 0x0C0B0A09, 0x100F0E0D, 0xEFBEADDE, 0xCEFAEDFE, 0xBEBAFECA,
            0x78563412,
        ];
        let bytes = hash_jackpot_le_bytes(&hj);
        assert_eq!(&bytes[0..4], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&bytes[28..32], &[0x12, 0x34, 0x56, 0x78]);
        // Round-trip back to words (the place_matrix_hash encoding).
        let back: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]])
        });
        assert_eq!(back, hj);
    }

    #[test]
    fn c2_difficulty_check_pass_and_fail() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        // Baseline trace: no IS_HASH_JACKPOT row ⇒ hash_jackpot = 0.
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_eq!(pis.hash_jackpot, [0u32; 8]);
        let proof = composite_prove(&cfg, trace, &pis);

        // hash_jackpot = 0 (all-zero LE u256). Any non-zero target
        // ⇒ 0 ≤ target ⇒ PoW check passes.
        let easy_target = [0xFFu8; 32];
        composite_verify_pow(&cfg, &proof, &pis, &easy_target)
            .expect("zero HASH_JACKPOT clears a max target");

        // Hardest possible target: 0. 0 ≤ 0 ⇒ still passes (equality).
        let zero_target = [0u8; 32];
        composite_verify_pow(&cfg, &proof, &pis, &zero_target)
            .expect("0 ≤ 0 is a pass (>= comparison is inclusive)");

        // Tamper the PI hash_jackpot so it's large, with a tiny
        // target ⇒ DifficultyNotMet (and STARK still verifies since
        // baseline has no IS_HASH_JACKPOT row, so the binding
        // constraint is vacuous and hash_jackpot is unconstrained).
        let mut big = pis.clone();
        big.hash_jackpot = [0xFFFF_FFFF; 8]; // max u256
        let big_proof = {
            let trace2 = CompositeTrace::baseline_min();
            composite_prove(&cfg, trace2, &big)
        };
        let tiny_target = {
            let mut t = [0u8; 32];
            t[0] = 1; // value = 1
            t
        };
        match composite_verify_pow(&cfg, &big_proof, &big, &tiny_target) {
            Err(PowVerifyError::DifficultyNotMet) => {}
            other => panic!("expected DifficultyNotMet, got {other:?}"),
        }
    }

    #[test]
    fn composite_proof_is_serializable() {
        // The proof type derives Serialize/Deserialize (see crates/
        // ai-pow-zk/Cargo.toml for the bincode dep). Verifying a
        // bincode round-trip is the structural soundness check
        // every lib-level consumer cares about.
        use bincode::config::standard as bincode_standard;

        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);

        let encoded = bincode::serde::encode_to_vec(&proof, bincode_standard()).expect("encode");
        let (decoded, _len) = bincode::serde::decode_from_slice::<Proof<AiPowStarkConfig>, _>(
            &encoded,
            bincode_standard(),
        )
        .expect("decode");
        composite_verify(&cfg, &decoded, &pis).expect("decoded proof verifies");
    }

    /// Two proofs over baseline traces of different sizes both
    /// verify with the same config (the config is per-params, not
    /// per-trace-size, in TEST_PEARL).
    #[test]
    fn composite_proofs_at_two_trace_sizes() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        let trace_small = CompositeTrace::baseline_min();
        let pis_small = CompositePublicInputs::derive_from_trace(&trace_small);
        let p_small = composite_prove(&cfg, trace_small, &pis_small);
        composite_verify(&cfg, &p_small, &pis_small).expect("small proof");

        let trace_big = CompositeTrace::baseline(crate::composite_layout::MIN_STARK_LEN * 2);
        let pis_big = CompositePublicInputs::derive_from_trace(&trace_big);
        let p_big = composite_prove(&cfg, trace_big, &pis_big);
        composite_verify(&cfg, &p_big, &pis_big).expect("big proof");
    }

    // =================================================================
    //  Public-input binding tests
    // =================================================================

    /// Tamper a PI element on the verifier side; verification
    /// rejects (the AIR's `when_last_row` constraint forces the
    /// trace's last-row CUMSUM_TILE to match `pis[0..4]`).
    #[test]
    fn verify_rejects_wrong_cumsum_pi() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);

        let mut bad_pis = pis.clone();
        bad_pis.cumsum[0] = 42; // baseline has 0 everywhere; 42 is wrong.

        assert!(
            composite_verify(&cfg, &proof, &bad_pis).is_err(),
            "wrong CUMSUM PI must reject"
        );
    }

    /// Tamper a JACKPOT PI element on the verifier side.
    #[test]
    fn verify_rejects_wrong_jackpot_pi() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let proof = composite_prove(&cfg, trace, &pis);

        let mut bad_pis = pis.clone();
        bad_pis.jackpot[5] = 0xDEAD_BEEF;

        assert!(
            composite_verify(&cfg, &proof, &bad_pis).is_err(),
            "wrong JACKPOT PI must reject"
        );
    }

    /// Build a trace with threaded non-zero cumsum + jackpot;
    /// PIs derived from it; prove + verify succeeds.
    #[test]
    fn prove_verify_with_threaded_state() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        // Thread a non-zero state through to the last row.
        trace.fill_cumsum_passthrough(0, &[1, -2, 3, -4]);
        let jp: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0x12345);
        trace.fill_jackpot_passthrough(0, &jp);

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        assert_eq!(pis.cumsum, [1, -2, 3, -4]);
        assert_eq!(pis.jackpot, jp);

        let proof = composite_prove(&cfg, trace, &pis);
        composite_verify(&cfg, &proof, &pis)
            .expect("threaded-state proof must verify with matching PIs");
    }

    /// PIs are part of the verification call, so swapping a
    /// proof's PIs for another proof's still rejects.
    #[test]
    fn verify_rejects_pi_substitution_across_proofs() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        // Proof A: baseline trace + zero PIs.
        let trace_a = CompositeTrace::baseline_min();
        let pis_a = CompositePublicInputs::derive_from_trace(&trace_a);
        let proof_a = composite_prove(&cfg, trace_a, &pis_a);

        // Proof B: threaded state + non-zero PIs.
        let mut trace_b = CompositeTrace::baseline_min();
        trace_b.fill_cumsum_passthrough(0, &[1, 1, 1, 1]);
        let pis_b = CompositePublicInputs::derive_from_trace(&trace_b);
        let _proof_b = composite_prove(&cfg, trace_b, &pis_b);

        // Verifying proof A against B's PIs must reject.
        assert!(
            composite_verify(&cfg, &proof_a, &pis_b).is_err(),
            "proof A with B's PIs must reject"
        );
    }

    /// PROD-shape bench. Ignored by default — run with
    /// `cargo test --release composite_proof_prod_bench -- --ignored --nocapture`.
    ///
    /// Measures prove + verify wall-clock for the baseline trace
    /// at MIN_STARK_LEN under [`CircuitConfig::PROD`] (`log_blowup
    /// = 4`, `num_queries = 15`, `pow_bits = 0` — 60 operational FRI
    /// query bits). The baseline trace has no chip activity, so
    /// this bench is a structural ceiling: real proofs with
    /// matmul / BLAKE3 activity will take longer because the
    /// dot-product / round constraints actually evaluate to
    /// non-trivial polynomials.
    #[test]
    #[ignore = "PROD bench — expensive; run with --ignored"]
    fn composite_proof_prod_bench() {
        let cfg = build_config(&test_zk_params(), &CircuitConfig::PROD);
        let trace = CompositeTrace::baseline_min();
        let pis = CompositePublicInputs::derive_from_trace(&trace);

        let t0 = std::time::Instant::now();
        let proof = composite_prove(&cfg, trace, &pis);
        let prove_ms = t0.elapsed().as_millis();

        let t1 = std::time::Instant::now();
        composite_verify(&cfg, &proof, &pis).expect("PROD verify");
        let verify_ms = t1.elapsed().as_millis();

        // Serialise to measure proof size.
        use bincode::config::standard as bincode_standard;
        let bytes = bincode::serde::encode_to_vec(&proof, bincode_standard()).expect("encode");
        let proof_bytes = bytes.len();

        println!(
            "ai-pow-zk PROD bench (composite baseline @ MIN_STARK_LEN = {} rows × {} cols):",
            crate::composite_layout::MIN_STARK_LEN,
            crate::composite_layout::TOTAL_TRACE_WIDTH
        );
        println!("  prove    : {prove_ms} ms");
        println!("  verify   : {verify_ms} ms");
        println!("  proof    : {proof_bytes} bytes");
    }

    #[test]
    #[ignore = "Layer-0 production proof size diagnostic — expensive; run with --ignored"]
    fn composite_pinned_logup_prod_l0_size_breakdown() {
        measure_pinned_logup_l0_size_breakdown("PROD_LB4_NQ15", CircuitConfig::PROD);
    }

    #[test]
    #[ignore = "Layer-0 low-blowup proof size diagnostic — expensive; run with --ignored"]
    fn composite_pinned_logup_lb2_nq30_l0_size_breakdown() {
        measure_pinned_logup_l0_size_breakdown("PURE_QUERY_LB2_NQ30", CircuitConfig::PROD_LB2_NQ30);
    }

    #[test]
    #[ignore = "Layer-0 pure-query reduced-query proof size diagnostic — expensive; run with --ignored"]
    fn composite_pinned_logup_lb6_nq10_l0_size_breakdown() {
        measure_pinned_logup_l0_size_breakdown(
            "PURE_QUERY_LB6_NQ10",
            CircuitConfig {
                log_blowup: 6,
                pow_bits: 0,
                num_queries: 10,
            },
        );
    }
}
