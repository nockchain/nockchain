//! Lib-level prove/verify wrappers for the composite AIR.
//!
//! ## Entrypoints — three tiers (pick by use case)
//!
//! | Family | AIR | Prover | Use |
//! |---|---|---|---|
//! | [`composite_prove`] / [`composite_verify`] | `CompositeFullAir` (unpinned) | uni-stark | unit / constraint-logic dev only — **not sound for PoW** (CRIT-1: a prover can zero selectors) |
//! | [`composite_prove_pinned`] / [`composite_verify_pinned`] / [`composite_verify_pow_pinned`] | `CompositeFullAirPinned` | uni-stark | CRIT-1 program-pinned, **no LogUp**. Lighter; backs the `crit1_*` / `high2_*` constraint-logic regression suite. Not the production path (matmul reads unbound — §4.C). |
//! | [`composite_prove_pinned_logup`] / [`composite_verify_pinned_logup`] / [`composite_verify_pow_pinned_logup`] | `CompositeFullAirWithLookupsPinned` | **batch-stark** | Layer-0 Route A (HIGH-2.2 §4.C). CRIT-1 program-pin **and** the `noised_packed`/range/i8u8/cv-routing LogUp enforced in one proof. Used by [`ai-pow::zk_bridge`] and as the inner proof for the recursive certificate. ≈1.23x the uni-stark pinned cost. |
//!
//! New Layer-0 callers should use the **`*_pinned_logup`** family.
//! Nockchain production consensus callers should not stop at this
//! layer: the canonical block/wire certificate is the recursive L1
//! certificate produced by
//! `crate::recursion::prove_canonical_ai_pow_certificate`. The
//! uni-stark `*_pinned` family is retained as the lighter no-LogUp
//! variant + the home of the CRIT-1/HIGH-2 constraint-logic
//! adversarial suite. The unpinned `composite_prove`/`verify` is
//! dev-only and PoW-unsound.
//!
//! ## CRIT-1 trust model (all `*_pinned*` families)
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

use p3_commit::Pcs;
#[cfg(any(test, feature = "dev-unsafe"))]
use p3_uni_stark::{
    prove, prove_with_preprocessed, setup_preprocessed, verify, verify_with_preprocessed,
    PreprocessedProverData, PreprocessedVerifierKey, Proof,
};
use p3_uni_stark::{StarkGenericConfig, Val, VerificationError};
use thiserror::Error;

use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
use crate::composite_full_air::{extract_program, program_degree_bits, ProgramShapeError};
#[cfg(any(test, feature = "dev-unsafe"))]
use crate::composite_full_air::{CompositeFullAir, CompositeFullAirPinned};
use crate::composite_public::CompositePublicInputs;
use crate::composite_trace::CompositeTrace;
use crate::params::ZkParams;

/// Concrete type of the underlying STARK verification error for the composite AIR.
/// Equivalent to `VerificationError<PcsError<AiPowStarkConfig>>`.
pub type StarkVerificationError = VerificationError<
    <<AiPowStarkConfig as StarkGenericConfig>::Pcs as Pcs<
        <AiPowStarkConfig as StarkGenericConfig>::Challenge,
        <AiPowStarkConfig as StarkGenericConfig>::Challenger,
    >>::Error,
>;

#[derive(Debug, Error)]
pub enum CompositeVerificationError {
    #[error("invalid verifier program: {0}")]
    InvalidProgram(#[from] ProgramShapeError),
    #[error("stark verification failed: {0:?}")]
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
        .map_err(CompositeVerificationError::Stark)
}

/// Encode the 8×u32 `HASH_JACKPOT` PI as a 32-byte little-endian
/// u256, byte-identical to a BLAKE3 digest (`bytes[4i..4i+4] =
/// word[i].to_le_bytes()`). Matches the encoding M52's
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
/// ## MED-3 obligation (caller-enforced)
///
/// `target` is **not** absorbed into the Fiat-Shamir transcript and
/// **not** an AIR public input (Pearl-Layer-0-faithful: difficulty
/// is external by design). This wrapper is therefore the
/// *unhardened primitive*: the difficulty bound is only meaningful
/// if the caller passes a `target` it **derived itself from the
/// chain-pinned params** (`difficulty_target(params)`) and never a
/// counterparty-supplied value. CRIT-1 (fixed) guarantees the other
/// precondition — `HASH_JACKPOT` is a genuinely bound PI.
/// Production callers MUST go through the MED-3-hardened
/// `ai_pow::zk_bridge::prove_and_verify_for_block`, which recomputes
/// the target internally so it cannot be forged. See
/// `2026-05-15_ZKP_SECURITY_REPORT.md` §MED-3.
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
//  CRIT-1: program-pinned prove / verify (ZKP_SECURITY_REPORT)
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
/// differ fails the in-AIR equality (CRIT-1 closed).
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
        .map_err(CompositeVerificationError::Stark)
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
//  HIGH-2.2 §4.C Route A: pinned + LogUp Layer-0 prove/verify
//
//  The uni-stark `composite_*_pinned` above enforce the CRIT-1
//  program-pin but NOT the cross-chip LogUp — so the matmul
//  `A_NOISED`/`B_NOISED` reads are unbound vs the C3/HASH_A
//  canonical store (§4.C gap). These prove/verify the
//  `CompositeFullAirWithLookupsPinned` AIR via `p3-batch-stark`,
//  which enforces the CRIT-1 pin AND the `noised_packed`
//  (+ range / i8u8 / cv-routing) LogUp simultaneously. Spike
//  measured ~1.23x the uni-stark pinned prover cost
//  (2026-05-15_HIGH2_2_DESIGN.md §4.C.10), vs naive Route C's ~10x.
//
//  Same CRIT-1 trust model: the verifier rebuilds the canonical
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
/// proof — identical CRIT-1 discipline to `composite_prove_pinned`).
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

/// [`composite_prove_pinned_logup`] with an explicit HIGH-2.2
/// §6(b) keystone flag. `sx_bound` MUST be derived by the
/// verifier from the trusted block params (`num_stripes ≤ 16`),
/// never from the proof. `true` = production / TEST_SMALL path;
/// `false` = `num_stripes > 16` legacy path (rectangular / PROD),
/// the documented §6(b) wider-register residual.
pub fn composite_prove_pinned_logup_sx(
    config: &AiPowStarkConfig,
    mut trace: CompositeTrace,
    public_inputs: &CompositePublicInputs,
    sx_bound: bool,
) -> (p3_batch_stark::BatchProof<AiPowStarkConfig>, Program) {
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
    (proof, program)
}

/// Verifier-side `CommonData` for the canonical `program` —
/// rebuilt witness-free from the program + its (public) height.
/// `pub(crate)` so the §recursion integration can obtain the
/// `CommonData` the recursive verifier needs.
pub(crate) fn logup_common_for(
    config: &AiPowStarkConfig,
    program: &Program,
    sx_bound: bool,
) -> p3_batch_stark::ProverData<AiPowStarkConfig> {
    use p3_batch_stark::ProverData;
    let log_ext_db = checked_program_degree_bits(program)
        .expect("canonical program shape already validated")
        + config.is_zk() as usize;
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

/// [`composite_verify_pinned_logup`] with an explicit §6(b)
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
    .map_err(CompositeVerificationError::Stark)
}

/// Route-A pinned full PoW verify: pinned+LogUp STARK verify +
/// the C2 difficulty check against the bound `HASH_JACKPOT`.
pub fn composite_verify_pow_pinned_logup(
    config: &AiPowStarkConfig,
    program: &Program,
    proof: &p3_batch_stark::BatchProof<AiPowStarkConfig>,
    public_inputs: &CompositePublicInputs,
    target: &[u8; 32],
) -> Result<(), PowVerifyError> {
    composite_verify_pow_pinned_logup_sx(config, program, proof, public_inputs, target, true)
}

/// [`composite_verify_pow_pinned_logup`] with an explicit §6(b)
/// keystone flag. `sx_bound` MUST be derived by the verifier from
/// the trusted block params (`num_stripes ≤ 16`), never the proof.
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
fn _suppress_unused_val_import(_v: Val<AiPowStarkConfig>) {}

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
            profile.johnson_fri_bits(),
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
        let bincode_global_lookup_data =
            bincode_len(&proof.global_lookup_data, "Layer-0 global lookup data");
        let bincode_component_sum = bincode_commitments
            + bincode_opened_values
            + bincode_opening_proof
            + bincode_global_lookup_data;

        let postcard_total = postcard_len(&proof, "Layer-0 batch proof");
        let postcard_commitments = postcard_len(&proof.commitments, "Layer-0 commitments");
        let postcard_opened_values = postcard_len(&proof.opened_values, "Layer-0 opened values");
        let postcard_opening_proof = postcard_len(&proof.opening_proof, "Layer-0 opening proof");
        let postcard_global_lookup_data =
            postcard_len(&proof.global_lookup_data, "Layer-0 global lookup data");
        let postcard_component_sum = postcard_commitments
            + postcard_opened_values
            + postcard_opening_proof
            + postcard_global_lookup_data;

        let table_count = proof.opened_values.instances.len();
        let global_lookup_entries: usize = proof.global_lookup_data.iter().map(Vec::len).sum();

        eprintln!(
            "ai-pow Layer-0 pinned+LogUp size breakdown [{label}]: profile=lb{} nq{} pow{} prove_ms={} verify_ms={} tables={} global_lookup_entries={}",
            profile.log_blowup,
            profile.num_queries,
            profile.pow_bits,
            prove_ms,
            verify_ms,
            table_count,
            global_lookup_entries,
        );
        eprintln!(
            "ai-pow Layer-0 pinned+LogUp bincode bytes [{label}]: total={} commitments={} opened_values={} opening_proof={} global_lookup_data={} component_sum={} serde_overhead={}",
            bincode_total,
            bincode_commitments,
            bincode_opened_values,
            bincode_opening_proof,
            bincode_global_lookup_data,
            bincode_component_sum,
            bincode_total.saturating_sub(bincode_component_sum),
        );
        eprintln!(
            "ai-pow Layer-0 pinned+LogUp postcard bytes [{label}]: total={} commitments={} opened_values={} opening_proof={} global_lookup_data={} component_sum={} serde_overhead={}",
            postcard_total,
            postcard_commitments,
            postcard_opened_values,
            postcard_opening_proof,
            postcard_global_lookup_data,
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

    // ───── HIGH-2.2 §4.A regression: non-zero JACKPOT_MSG ─────
    //
    // A latent JackpotChip bug (the JACKPOT_MSG RAM recurrence
    // `nxt = SLOT_SEL·rotl13_xor + (1−SLOT_SEL)·cur` was
    // `when_transition` but **not** gated by `is_active`) forced
    // JACKPOT_MSG constant across all inactive rows, so the
    // inactive→active(finalize) boundary forbade a freshly-placed
    // non-zero JACKPOT_MSG. Latent for years because every
    // jackpot placement hashed an all-zero JACKPOT_MSG (0 == 0);
    // surfaced by HIGH-2.2 §4.A (the first non-zero JACKPOT_MSG —
    // the real folded `M`). Fixed by gating the recurrence with
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

    /// HIGH-2.2 §6(b) fast unit gate — the full useful-work chain
    /// placement satisfies the base `CompositeFullAir` (matmul
    /// sweep recurrence + StripeXor transport + `SX_IN ==
    /// nxt.CUMSUM_TILE` cross-chip binding + FoldChip), proven via
    /// the cheap unit prover. Isolates witness/chip-wiring bugs in
    /// seconds before the ~minutes Route-A pinned path below. (The
    /// Pinned §4.D/§6(b) keystones are exercised by
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
            .expect("§6(b) useful-work chain must verify through unit CompositeFullAir");
    }

    /// §6(b)-R-b — the STRIPE-MAJOR sweep (`place_useful_work_chain_rb`:
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

    /// §6(b)-R-b soundness — a tampered TileReduce input on a reduce row
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

    /// §6(b)-R-b ADVERSARIAL — the held TileAccum accumulator must be fed
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

    /// §6(b)-R-b — the PINNED path (CRIT-1 program pin + §4.D keystone
    /// JACKPOT_MSG==FOLD_STATE + the §6(b)-R-b keystone FOLD_XSTEP==TR_NEW)
    /// verifies for a stripe-major R-b trace at num_stripes=128 (> the old
    /// STRIPE_MAX=64), with `sx_bound=false` (the SX 64-lane keystone is
    /// inactive for R-b; the §6(b)-R-b keystone binds the fold input to the
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
        // §4.D: the jackpot-hash block sets last-row JACKPOT_MSG == M.
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

    /// §6(b)-R-b ADVERSARIAL — the keystone that REPLACES the disabled
    /// SX 64-lane keystone when `sx_bound=false`. With the SX keystone
    /// off, the ONLY thing binding each stripe's FoldChip input to the
    /// genuine per-stripe reduce is the §6(b)-R-b keystone
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
        // value ≠ the row's TR_NEW — the §6(b)-R-b keystone must catch it.
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
            "tampered FOLD_XSTEP (≠ TR_NEW) MUST reject — the §6(b)-R-b \
             keystone binds the fold input to the real reduce under sx_bound=false"
        );
    }

    /// §6(b)-R-b ADVERSARIAL — the `sx_bound` flag is load-bearing and
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

    /// §6(b)-R-b — the full Route-A LogUp path (CRIT-1 pin + the
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
        let a_id_base = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((t * k).div_ceil(8)) as u64;
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

    /// §6(b)-R-b ADVERSARIAL — the `noised_packed` LogUp bus binds the
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
        let a_id_base = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((t * k).div_ceil(8)) as u64;
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

    /// §6(b)-R-b ADVERSARIAL — the §4.D keystone `JACKPOT_MSG == FOLD_STATE`
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
        // the trace's real M (flip one lane). The §4.D keystone
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
             §4.D binds the PoW digest to the real fold-chain output",
        );
    }

    /// HIGH-2.2 §6(b) end-to-end regression via the production
    /// Route-A path: the **full useful-work chain** —
    /// `place_useful_work_chain` (sub-block-major matmul sweep +
    /// co-located StripeXor reduction) → `place_fold_chain` driven
    /// by the chip-reduced `x_steps` → `JACKPOT_MSG` = folded
    /// `TileState M` → §4.D + §6(b) keystones → jackpot-hash,
    /// proven/verified through `composite_*_pinned_logup` (CRIT-1
    /// pin + `noised_packed` LogUp via batch-stark). Exercises every
    /// new §6(b) constraint together — the matmul cross-row
    /// recurrence over the 256-row sweep, the StripeXor transport,
    /// the `SX_IN == nxt.CUMSUM_TILE` cross-chip binding, and the
    /// Pinned `FOLD_XSTEP == SX_XR[stripe]` keystone — through the
    /// batch-stark prover (the debug-assertions-OFF hazard surface).
    /// `a_prime`/`b_prime` are synthetic i8 strips: `ai-pow-zk` must
    /// not depend on `ai-pow`, and the chip math is self-consistent
    /// (cross-crate parity vs `compute_tile_trace` is asserted from
    /// the `ai-pow` side — GATEs 1/3).
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

        // M-S1 (§4.C.11) — place the positioned `noised_packed`
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
        let a_id_base = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((t * k).div_ceil(8)) as u64;
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

        // §4.D keystone precondition: last-row JACKPOT_MSG ==
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
                "§4.D precondition: JACKPOT_MSG[{s}] != FOLD_STATE[{s}]"
            );
        }
        // §6(b) keystone precondition: every fold row's FOLD_XSTEP
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
                "§6(b) precondition: FOLD_XSTEP != SX_XR @stripe {step}"
            );
        }

        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis)
            .expect("full §6(b) useful-work chain + keystones must verify under Route-A");
    }

    /// M-S1 (§4.C.11) coverage net: the producer store from
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

    /// §4.C.2 / A3.2a: the positioned store layout is a **pure
    /// function of `(t,r,num_stripes,k)`** — the `(side_a,src)`
    /// skeleton is identical for *any* `a′/b′` byte filling, so
    /// the CRIT-1 program rebuild can reconstruct each store
    /// row's `(i,l)` (hence its pinned noise) **witness-free**
    /// (the W1 unblocker). And it is consistent with M-S1: every
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

        // M-S1 consistency: every deduped store chunk is some
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

    /// M-S1 (§4.C.11) **adversarial I2**: the §6(b) sweep input is
    /// genuinely *bound* to the declared `noised_packed` store. A
    /// prover that sweeps a tile whose noised micro-tiles are NOT
    /// the published store (here: store built from the canonical
    /// `a_prime`/`b_prime`, but the sweep run on a *different*,
    /// cheaper tile) leaves the bus unbalanced ⇒ Route-A MUST
    /// reject. (This is the non-vacuity proof for the whole-
    /// micro-tile binding; store ↔ committed-matrix `HASH_A` is the
    /// separately-scoped §4.C.2 residual.)
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

    /// **CSA S4 — HIGH-2.2 §6(b)-G2 keystone (K3) explicit tamper.**
    ///
    /// The §6(b)-G2 keystone constraint (`composite_full_air.rs:318-334`)
    /// binds the FoldChip's `FOLD_XSTEP` to the StripeXorChip's
    /// `SX_XR[stripe]` lane selected by the verifier-fixed
    /// `FOLD_STRIPE_SEL` (one-hot, packed into the CRIT-1 pinned
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
    /// the one-hot `FOLD_STRIPE_SEL[0]=1` claims). The K3 constraint
    /// becomes `1 · (SX_XR[1] − SX_XR[0]) ≠ 0` ⇒ M1 rejection at
    /// `composite_verify_pinned_logup`.
    ///
    /// This is the explicit K3 regression for stripe-pin binding: a wrong
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

        // === K3 §6(b)-G2 tamper: fold-row 0 has FOLD_SLOT_SEL[0]=1
        // (one-hot stripe = 0), so the constraint asserts
        //   FOLD_XSTEP == SX_XR[0].
        // Tamper: replace FOLD_XSTEP with a value guaranteed to
        // differ from SX_XR[0] (its honest value + 1, in the field).
        // The constraint
        //   Σ_s FOLD_STRIPE_SEL[s] · (FOLD_XSTEP − SX_XR[s])
        // = FOLD_STRIPE_SEL[0] · ((SX_XR[0] + 1) − SX_XR[0])
        // = 1 ≠ 0
        // ⇒ M1 (AIR eval() violation) at verify.
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
            "K3 tamper-test precondition: row 0 of fold must be one-hot on slot 0"
        );

        // Sanity: confirm FOLD_XSTEP == SX_XR[0] before tampering
        // (i.e., the keystone is satisfied honestly).
        let sx_xr_correct = trace.matrix.values[base + SX_XR_START + 0];
        let fold_xstep_honest = trace.matrix.values[base + FOLD_XSTEP];
        assert_eq!(
            fold_xstep_honest.as_canonical_u64(),
            sx_xr_correct.as_canonical_u64(),
            "K3 tamper-test precondition: honest FOLD_XSTEP must == SX_XR[0]"
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
            "K3 tamper-test internal: +1 in Goldilocks must change value",
        );
        trace.matrix.values[base + FOLD_XSTEP] = tampered_value;

        // Derive PIs + canonical from the tampered trace. FOLD_XSTEP
        // is not in PROGRAM_COLS (the CRIT-1 pin set is row
        // metadata: control, noise pins, CV/tweak, A/B IDs, and the
        // row index). Canonical is unchanged; PIs are unchanged (they bind HASH_A/B,
        // HASH_JACKPOT, key-pin rows). The mismatch surfaces purely as
        // an AIR-constraint failure on the K3 row.
        let pis = CompositePublicInputs::derive_from_trace(&trace);
        let canonical = extract_program(&trace.matrix);
        let (proof, _) = composite_prove_pinned_logup(&cfg, trace, &pis);
        let result = composite_verify_pinned_logup(&cfg, &canonical, &proof, &pis);
        assert!(
            result.is_err(),
            "K3 §6(b)-G2 tamper (FOLD_XSTEP retargeted to SX_XR[1] != SX_XR[0]) MUST reject",
        );
    }

    /// **CSA S5 — HIGH-2.2 §6(b)-G2 keystone (K3) PRODUCER-SIDE tamper.**
    ///
    /// Cross-AIR composition test (per
    /// `2026-05-20_CSA_S5_CROSS_AIR_TAMPER_TESTS.md`).
    /// The K3 keystone binds FoldChip's `FOLD_XSTEP` (consumer side)
    /// to StripeXorChip's `SX_XR[stripe]` (producer side). The S4
    /// consumer-side test `high2_2_g2_xstep_stripe_pin_rejects`
    /// tampers `FOLD_XSTEP`. This S5 test exercises the *opposite*
    /// direction: tamper `SX_XR[0]` at the K3 row while leaving
    /// `FOLD_XSTEP` honest. The K3 constraint
    ///   `Σ_s FOLD_STRIPE_SEL[s] · (FOLD_XSTEP − SX_XR[s]) = 0`
    /// becomes `1 · (FOLD_XSTEP − (SX_XR[0] + 1)) = −1 ≠ 0` ⇒ M1.
    ///
    /// **Defense-in-depth note (R1 honest).** Tampering `SX_XR[0]`
    /// at one row also violates StripeXorChip's row-to-row
    /// passthrough constraint (StripeXor is inactive at fold rows,
    /// so the carry-forward `SX_XR[i+1] == SX_XR[i]` is enforced).
    /// Either rejection mechanism catches the tamper. This test
    /// asserts rejection without claiming which constraint fires
    /// first; the *cross-AIR claim* is that the K3 binding is
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
        // exercises the K3 producer-side path (and incidentally
        // violates StripeXor's passthrough; both ⇒ M1).
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
            "S5 K3 producer-side: row 0 must be one-hot on slot 0"
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
            "S5: tampered SX_XR[0] (producer side of K3 keystone) MUST reject"
        );
    }

    // ───────────── CRIT-1 malicious-prover regression suite ─────────────
    //
    // ZKP_SECURITY_REPORT CRIT-1: a malicious prover can zero every
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
        let jk: [u32; 8] = core::array::from_fn(|i| 0xC0FE_0000 + i as u32);
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

    /// THE CRIT-1 exploit, now blocked. A malicious prover submits
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
            "CRIT-1: forged proof must fail against the canonical program VK"
        );
        assert!(
            composite_verify_pow_pinned(&cfg, &canonical, &evil_proof, &forged, &[0xFFu8; 32])
                .is_err(),
            "CRIT-1: forged winning PoW must be rejected"
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
            "CRIT-1: forged HASH_JACKPOT must fail the C4 binding under the pinned program"
        );
    }

    /// HIGH-2: the C4-hashed `JACKPOT_MSG` is no longer
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
            .expect("zero-CUMSUM honest trace satisfies the HIGH-2 keystone");

        // Attack: plant a "winning" free jackpot message on the
        // last row while CUMSUM stays 0 — exactly the pre-HIGH-2
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
            "HIGH-2: a free (non-CUMSUM) jackpot message must be rejected"
        );
    }

    // ───────── HIGH-2.2 §4.C Route-A production suite ─────────
    //
    // The CRIT-1 / HIGH-2 adversarial regressions, re-run against
    // the batch-stark pinned+LogUp path (`*_pinned_logup`). These
    // prove the production Route-A binding keeps CRIT-1 soundness
    // and the HIGH-2 keystone *while additionally enforcing the
    // noised_packed/range LogUp*. (The noised_packed *matmul-input*
    // binding is non-vacuous only once §4.A places real matmul
    // rows — 2026-05-15_HIGH2_2_DESIGN.md §4.C.10; not overclaimed here.)

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

    /// CRIT-1 under Route A: a zeroed-selector forgery is
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
            "CRIT-1 under Route A: forged proof must fail vs canonical program"
        );
        assert!(
            composite_verify_pow_pinned_logup(
                &cfg, &canonical, &evil_proof, &forged, &[0xFFu8; 32]
            )
            .is_err(),
            "CRIT-1 under Route A: forged winning PoW rejected"
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

    /// **§4.C.10 — MAT_FREQ producer-planting adversarial test.**
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
                "§4.C.10: a planted noised_packed producer (inflated \
                 MAT_FREQ) was ACCEPTED — the bus does not bind \
                 producer multiplicity"
            ),
            Ok(Err(_)) | Err(_) => { /* rejected — correct */ }
        }
    }

    /// HIGH-2 keystone holds under Route A: a free (non-CUMSUM)
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
            "HIGH-2 under Route A: free jackpot message must be rejected"
        );
    }

    #[test]
    fn hash_jackpot_le_bytes_is_blake3_digest_order() {
        // word i ↦ bytes[4i..4i+4] little-endian — the inverse of
        // M52's `u32::from_le_bytes([digest[4i..4i+4]])`.
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
    /// = 4`, `num_queries = 15`, `pow_bits = 0` — 60 pure-query bits
    /// unconditional FRI soundness at the Johnson radius; paper IACR
    /// ePrint 2025/2055 Theorem 1.5). The baseline trace has no chip activity, so
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
