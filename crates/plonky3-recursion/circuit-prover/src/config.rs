//! STARK proving configurations.
//!
//! This module provides STARK configurations for different prime fields.
//!
//! # Quick Start
//!
//! ```ignore
//! use p3_circuit_prover::config;
//!
//! // Use a preconfigured setup
//! let config = config::baby_bear();
//! ```

use p3_baby_bear::{BabyBear, Poseidon2BabyBear, default_babybear_poseidon2_16};
use p3_blake3::Blake3;
use p3_challenger::{DuplexChallenger, HashChallenger, SerializingChallenger64};
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::extension::BinomialExtensionField;
use p3_field::{Field, PrimeCharacteristicRing, PrimeField64, TwoAdicField};
use p3_fri::{FriParameters, TwoAdicFriPcs};
use p3_goldilocks::{Goldilocks, Poseidon2Goldilocks};
use p3_koala_bear::{KoalaBear, Poseidon2KoalaBear, default_koalabear_poseidon2_16};
use p3_merkle_tree::MerkleTreeMmcs;
use p3_symmetric::{
    CompressionFunctionFromHasher, CryptographicPermutation, PaddingFreeSponge, SerializingHasher,
    TruncatedPermutation,
};
use p3_tip5_circuit_air::Tip5Perm;
use p3_uni_stark::StarkConfig;

/// Compression function arity (number of inputs per compression).
const COMPRESS_ARITY: usize = 2;

/// A STARK configuration with all cryptographic primitives specified.
///
/// ### Type Parameters
/// - `F`: Base field.
/// - `PermHash`: Permutation function used for sponge hashing (leaves, transcript absorption).
/// - `PermCompress`: Permutation function used for Merkle tree compression.
/// - `HASH_PERM_WIDTH`: Width of the hash permutation state.
/// - `COMPRESS_PERM_WIDTH`: Width of the compression permutation state.
/// - `RATE`: Number of field elements absorbed per permutation in sponge mode.
/// - `OUT`: Number of output elements squeezed per permutation.
/// - `COMPRESS_CHUNK`: Number of elements per compression chunk in Merkle commitments.
/// - `CHALLENGE_DEGREE`: Extension field degree.
pub type Config<
    F,
    PermHash,
    PermCompress,
    const HASH_PERM_WIDTH: usize,
    const COMPRESS_PERM_WIDTH: usize,
    const RATE: usize,
    const OUT: usize,
    const COMPRESS_CHUNK: usize,
    const CHALLENGE_DEGREE: usize,
> = StarkConfig<
    TwoAdicFriPcs<
        F,
        Radix2DitParallel<F>,
        MerkleTreeMmcs<
            F,
            F,
            PaddingFreeSponge<PermHash, HASH_PERM_WIDTH, RATE, OUT>,
            TruncatedPermutation<PermCompress, COMPRESS_ARITY, COMPRESS_CHUNK, COMPRESS_PERM_WIDTH>,
            2,
            COMPRESS_CHUNK,
        >,
        ExtensionMmcs<
            F,
            BinomialExtensionField<F, CHALLENGE_DEGREE>,
            MerkleTreeMmcs<
                F,
                F,
                PaddingFreeSponge<PermHash, HASH_PERM_WIDTH, RATE, OUT>,
                TruncatedPermutation<
                    PermCompress,
                    COMPRESS_ARITY,
                    COMPRESS_CHUNK,
                    COMPRESS_PERM_WIDTH,
                >,
                2,
                COMPRESS_CHUNK,
            >,
        >,
    >,
    BinomialExtensionField<F, CHALLENGE_DEGREE>,
    DuplexChallenger<F, PermHash, HASH_PERM_WIDTH, RATE>,
>;

/// Builds a STARK configuration directly from the hash and compression permutations.
///
/// This replaces the former `ConfigBuilder` (which was only ever used internally,
/// immediately followed by `.build()`): the field factories below call this and
/// return the concrete config, so callers no longer chain `.build()`.
#[allow(clippy::type_complexity)]
fn build_poseidon2_stark_config<
    F,
    PermHash,
    PermCompress,
    const HASH_PERM_WIDTH: usize,
    const COMPRESS_PERM_WIDTH: usize,
    const RATE: usize,
    const OUT: usize,
    const COMPRESS_CHUNK: usize,
    const CHALLENGE_DEGREE: usize,
>(
    perm_hash: PermHash,
    perm_compress: PermCompress,
) -> Config<
    F,
    PermHash,
    PermCompress,
    HASH_PERM_WIDTH,
    COMPRESS_PERM_WIDTH,
    RATE,
    OUT,
    COMPRESS_CHUNK,
    CHALLENGE_DEGREE,
>
where
    F: Field,
    PermHash: Clone + CryptographicPermutation<[F; HASH_PERM_WIDTH]>,
    PermCompress: Clone + CryptographicPermutation<[F; COMPRESS_PERM_WIDTH]>,
{
    type Hash<Perm, const PERM_WIDTH: usize, const RATE: usize, const OUT: usize> =
        PaddingFreeSponge<Perm, PERM_WIDTH, RATE, OUT>;
    type Compress<Perm, const PERM_WIDTH: usize, const COMPRESS_CHUNK: usize> =
        TruncatedPermutation<Perm, COMPRESS_ARITY, COMPRESS_CHUNK, PERM_WIDTH>;

    let hash = Hash::<PermHash, HASH_PERM_WIDTH, RATE, OUT>::new(perm_hash.clone());
    let compress =
        Compress::<PermCompress, COMPRESS_PERM_WIDTH, COMPRESS_CHUNK>::new(perm_compress);
    let val_mmcs = MerkleTreeMmcs::new(hash, compress, 3);
    let challenge_mmcs = ExtensionMmcs::new(val_mmcs.clone());
    let dft = Radix2DitParallel::default();
    let fri_params = FriParameters::new_benchmark_high_arity(challenge_mmcs);
    let pcs = TwoAdicFriPcs::new(dft, val_mmcs, fri_params);
    let challenger = DuplexChallenger::new(perm_hash);

    StarkConfig::new(pcs, challenger)
}

/// Creates a standard BabyBear configuration.
///
/// BabyBear is a 31-bit prime field (2^31 - 2^27 + 1).
///
/// # Parameters
/// - **Hash permutation width**: 16 (appropriate for 32-bit fields)
/// - **Compression permutation width**: 16
/// - **Rate**: 8 (256 bits / 32 bits per element)
/// - **Output size**: 8 (256 bits / 32 bits per element)
/// - **Challenge degree**: 4
///
/// # Examples
///
/// ```ignore
/// let config = config::baby_bear();
/// let prover = BatchStarkProver::new(config);
/// ```
#[inline]
pub fn baby_bear() -> BabyBearConfig {
    let perm = default_babybear_poseidon2_16();
    build_poseidon2_stark_config(perm.clone(), perm)
}

/// Creates a standard KoalaBear configuration.
///
/// KoalaBear is a 31-bit prime field (2^31 - 2^24 + 1).
///
/// # Parameters
/// - **Hash permutation width**: 16 (appropriate for 32-bit fields)
/// - **Compression permutation width**: 16
/// - **Rate**: 8 (256 bits / 32 bits per element)
/// - **Output size**: 8 (256 bits / 32 bits per element)
/// - **Challenge degree**: 4
///
/// # Examples
///
/// ```ignore
/// let config = config::koala_bear();
/// let prover = BatchStarkProver::new(config);
/// ```
#[inline]
pub fn koala_bear() -> KoalaBearConfig {
    let perm = default_koalabear_poseidon2_16();
    build_poseidon2_stark_config(perm.clone(), perm)
}

/// Creates a standard Goldilocks configuration.
///
/// Goldilocks is a 64-bit prime field (2^64 - 2^32 + 1).
///
/// # Parameters
/// - **Hash permutation width**: 8 (appropriate for 64-bit fields)
/// - **Compression permutation width**: 8
/// - **Rate**: 4 (256 bits / 64 bits per element)
/// - **Output size**: 4 (256 bits / 64 bits per element)
/// - **Challenge degree**: 2
///
/// # Examples
///
/// ```ignore
/// let config = config::goldilocks();
/// let prover = BatchStarkProver::new(config);
/// ```
#[inline]
pub fn goldilocks() -> GoldilocksConfig {
    use rand::SeedableRng;
    let mut rng = rand::rngs::SmallRng::seed_from_u64(1);
    let perm = p3_goldilocks::Poseidon2Goldilocks::<8>::new_from_rng_128(&mut rng);
    build_poseidon2_stark_config(perm.clone(), perm)
}

/// Goldilocks configuration with **`log_blowup = 2` (FRI tier B = 4)**
/// using **Tip5 throughout** for MMCS + Fiat-Shamir challenger.
///
/// **2026-05-20 (M-S5b S1.B Poseidon2-removal P5)**: this builder
/// was previously Poseidon2-Goldilocks<8>-based; now uses
/// `p3-tip5-circuit-air::Tip5Perm` (width 16, rate 10, digest 5)
/// — matching the inner ai-pow-zk STARK's choice for architectural
/// uniformity (analogous to Pearl's BLAKE3-throughout pattern).
///
/// Compatibility alias for the production Tip5 outer-cert config.
///
/// This function used to select a tiny `FriParameters::new_testing`
/// profile (`log_blowup = 2`, `num_queries = 2`, about five FRI bits).
/// That path is deliberately removed: Tip5 recursive certificates now
/// have a single maintained profile, [`goldilocks_tip5_60bit`].
#[inline]
pub fn goldilocks_tip5() -> GoldilocksTipsConfig {
    goldilocks_tip5_60bit()
}

/// **C3 / M-S5 vertical-recursion cert** — Goldilocks outer-cert
/// config at the **paper-anchored Johnson tier** (paper
/// Ben-Sasson, Carmon, Habock, Kopparty, Saraf,
/// *"On Proximity Gaps for Reed–Solomon Codes"*, IACR ePrint
/// 2025/2055, Theorem 1.5 + §1.3.2).
///
/// **2026-05-20 (M-S5b S1.B Poseidon2-removal P5)**: flipped from
/// `Poseidon2Goldilocks<8>` (width 8, x⁷ S-box, degree 7) to
/// `Tip5Perm` (width 16, rate 10, digest 5, degree 4) for the MMCS
/// hash + Fiat-Shamir challenger permutation. Eliminates the
/// dual-hash architectural defect identified in the routes audit
/// `crates/ai-pow-zk/docs/2026-05-20_PROOF_SIZE_REDUCTION_ROUTES_AUDIT.md`
/// § 3.2.0. User-accepted per "I'm not concerned at present.
/// I'm not willing to use Poseidon2." (2026-05-20).
///
/// **2026-05-21 paper-anchored bits-target relaxation:** the
/// maintainer's prior 80-bit floor (with `nq=20` ⇒ 82 bits at
/// Johnson) was reanchored after reading IACR ePrint 2025/2055
/// §§ 1.4, 6, 8 carefully. The paper provides two end-points for
/// our config:
///
///   | End-point | Formula | Bits at lb=4, n≤2^22 | Status |
///   |---|---|---:|---|
///   | Known **insecure** at γ ≥ LDR (Thm 1.17 CYCLE-SUM) | `log₂(n) + O(1)` | ~22 | constructive attack |
///   | Known **secure** at γ < J(δ)−η (Thm 1.5) | `lb·nq + pow` | 80+ | proven, paper |
///
/// The Plonky3 `CapacityBound::log_eta` heuristic claims `~2·lb`
/// bits/query (≈ 98 bits at `nq=12`) at γ ≈ 1−ρ, but that sits
/// in the no-mans-land between Johnson (proven) and LDR
/// (attacked) where the paper provides **neither** a positive
/// theorem nor a constructive attack against generic codes —
/// only counterexamples for specific code families (Thm 1.6
/// char-2; Thm 1.13 Mersenne primes). The heuristic is therefore
/// not adopted as the production soundness model.
///
/// **Anchored-between policy (2026-05-21):** the bits target is
/// placed in the (22, 80) interval, **proven via Theorem 1.5**
/// at the chosen `(lb, nq)`. Targeting **60 bits Johnson** with
/// `nq = 9` gives:
///
///   `bits_anchored = lb·nq + query_pow
///                  = 4 · 9 + 24 = 60` bits Johnson, proven.
///
/// This is 40 bits above the known-insecure floor and 20 bits
/// below the prior conservative 80-bit ceiling — "reasonable and
/// optimistic." 25% fewer queries than the prior `nq=20` 82-bit
/// PROD on **paper-proven Johnson footing**, no conjectural
/// soundness model.
///
/// **Time-bounded threat model (rationale for the relaxed floor):**
/// the 80-bit floor was an offline-cryptographic-security threshold;
/// PoW forgery in this chain is **time-bounded by the 2.5-min block
/// cadence**. An attacker must produce a forging proof within ~150
/// seconds, otherwise a fresh honest block obsoletes the target.
/// At 60 bits: 2^60 ops ≈ 1.2·10^18 ops in 150 s ⇒ ~8·10^15 ops/sec
/// (~30 PetaOps) sustained throughput required. FRI verification is
/// dominated by random Merkle-path opens (not matmul) — the workload
/// favors CPU patterns over GPU/ASIC, putting the wall-clock budget
/// well beyond the 2.5-min window even for state-actor-scale compute.
/// The 80-bit margin would only matter against an offline /
/// long-horizon attacker, which the 2.5-min cadence forecloses.
/// Maintainer 2026-05-21: "an attacker has 2.5 minutes to make a
/// proof in our context, hence our optimism."
///
/// **Cumulative lever stack (each independently validated in
/// `2026-05-20_RECURSIVE_PROOF_SIZE_INVESTIGATION.md` + this 2026-05-21
/// reanchoring):**
///
///  | Lever                       | Effect on L1 | Soundness effect            |
///  |-----------------------------|-------------:|-----------------------------|
///  | `log_blowup: 2 → 4`         | −46%         | 16× LDE; bits = 4·nq + query_pow |
///  | `num_queries: 42 → 20 → 15 → 10 → 9` | query bytes ↓ | bits = 4·9 + 24 = 60 (Johnson, proven) |
///  | `mmcs cap: 0 → 5`           | cuts paths   | cuts Merkle-path depth      |
///  | `max_log_arity: 1 → 3`      | −5% (sep.)   | none — fold-shape only      |
///  | `log_final_poly_len: 0 → 2` | (combined)   | none — final-poly tail only |
///
/// **FRI parameters (current production):** `log_blowup = 4,
/// num_queries = 9, max_log_arity = 3, log_final_poly_len = 2,
/// commit_proof_of_work_bits = 1, query_proof_of_work_bits = 24,
/// cap_height = 5, digest = 5`. Unconditional Johnson soundness
/// = `log_blowup · num_queries + query_pow
/// = 4 · 9 + 24 = 60` bits — anchored between the known
/// insecure (22) and the prior conservative ceiling (80), with
/// a 60-bit floor target.
///
/// **Trade-off:** `log_blowup = 4` ⇒ 16× LDE (vs the pre-2026-05-20
/// 4×) ⇒ ~4× prover memory + slower proving wall (the dominant
/// operational cost; size win is at the prover's expense, not the
/// verifier's). Per-block PoW at 2.5-min cadence does not need the
/// 120/128-bit long-horizon margin; see
/// `crates/ai-pow-zk/docs/2026-05-19_M_S5B_TERMINAL_COMPRESSION_DESIGN.md`
/// §1.4.
///
/// Proximity testing stays at γ < J(δ)−η (Johnson radius J(δ) =
/// 1 − √(1/16) = 0.75 at this rate; never beyond — paper IACR
/// ePrint 2025/2055 §1.4 + §8 attacks avoided by virtue of the
/// DEEP-FRI analysis choosing `γ_FRI` strictly inside Johnson).
///
/// **Function-name caveat:** the `_80bit` suffix is historical
/// (the function targeted 80 bits before the 2026-05-21 anchored
/// reanchoring). Renaming pending across call-sites; the
/// authoritative bit count is in this doc-comment + the inline
/// `bits = lb·nq+query_pow` formula.
pub const GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP: usize = 4;
pub const GOLDILOCKS_TIP5_RECURSIVE_LOG_FINAL_POLY_LEN: usize = 2;
pub const GOLDILOCKS_TIP5_RECURSIVE_MAX_LOG_ARITY: usize = 3;
pub const GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES: usize = 9;
pub const GOLDILOCKS_TIP5_RECURSIVE_COMMIT_POW_BITS: usize = 1;
pub const GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS: usize = 24;
pub const GOLDILOCKS_TIP5_RECURSIVE_CAP_HEIGHT: usize = 5;
pub const GOLDILOCKS_TIP5_RECURSIVE_JOHNSON_BITS: usize = GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP
    * GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES
    + GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS;

pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_BLOWUP: usize = 4;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_FINAL_POLY_LEN: usize = 2;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_MAX_LOG_ARITY: usize = 3;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_NUM_QUERIES: usize = 15;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS: usize = 0;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS: usize = 0;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_CAP_HEIGHT: usize = 5;
pub const GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_JOHNSON_BITS: usize =
    GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_BLOWUP
        * GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_NUM_QUERIES;

#[inline]
fn goldilocks_tip5_with_fri_params(
    log_blowup: usize,
    log_final_poly_len: usize,
    max_log_arity: usize,
    num_queries: usize,
    commit_proof_of_work_bits: usize,
    query_proof_of_work_bits: usize,
    cap_height: usize,
) -> GoldilocksTipsConfig {
    let perm = Tip5Perm;
    let hash = PaddingFreeSponge::<_, 16, 10, 5>::new(perm);
    let compress = TruncatedPermutation::<_, COMPRESS_ARITY, 5, 16>::new(perm);
    let val_mmcs = MerkleTreeMmcs::new(hash, compress, cap_height);
    let challenge_mmcs = ExtensionMmcs::new(val_mmcs.clone());
    let dft = Radix2DitParallel::default();
    let fri_params = FriParameters {
        log_blowup,
        log_final_poly_len,
        max_log_arity,
        num_queries,
        commit_proof_of_work_bits,
        query_proof_of_work_bits,
        mmcs: challenge_mmcs,
    };
    let pcs = TwoAdicFriPcs::new(dft, val_mmcs, fri_params);
    let challenger = DuplexChallenger::new(perm);
    StarkConfig::new(pcs, challenger)
}

#[inline]
pub fn goldilocks_tip5_60bit() -> GoldilocksTipsConfig {
    // Paper-anchored Johnson-radius soundness (IACR ePrint
    // 2025/2055 Theorem 1.5):
    //   bits = log_blowup · num_queries + query_pow
    //        = 4 · 9 + 24 = 60 bits, proven.
    // Anchored between the known-insecure log₂(n) ≈ 22 bits at
    // γ ≥ LDR (Thm 1.17 CYCLE-SUM) and the prior conservative
    // 80-bit ceiling at nq=20. Maintainer 2026-05-21 targeted
    // a 60-bit floor; nq=9 with query PoW 24 lands exactly there
    // while cutting query-sized proof material.
    // mla=3 + lfp=2 + cap=5 are soundness-neutral compression
    // levers (fold shape / final poly / Merkle cap).
    goldilocks_tip5_with_fri_params(
        GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP,
        GOLDILOCKS_TIP5_RECURSIVE_LOG_FINAL_POLY_LEN,
        GOLDILOCKS_TIP5_RECURSIVE_MAX_LOG_ARITY,
        GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES,
        GOLDILOCKS_TIP5_RECURSIVE_COMMIT_POW_BITS,
        GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS,
        GOLDILOCKS_TIP5_RECURSIVE_CAP_HEIGHT,
    )
}

/// Goldilocks + Tip5 recursive STARK profile with a 60-bit Johnson floor from
/// queries alone.
///
/// This keeps the same soundness-neutral compression levers as
/// [`goldilocks_tip5_60bit`] (`max_log_arity = 3`,
/// `log_final_poly_len = 2`, `cap_height = 5`) but replaces the mixed
/// `4 * 9 + 24` query/PoW accounting with `4 * 15 + 0`. It is not the default
/// batch-STARK checkpoint profile; callers use it when evaluating production
/// candidates that must not count verifier-accepted proof-system PoW grinding.
#[inline]
pub fn goldilocks_tip5_pure_query_60bit() -> GoldilocksTipsConfig {
    goldilocks_tip5_pure_query_60bit_with_shape(
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_BLOWUP,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_NUM_QUERIES,
    )
}

/// Build a query-only 60-bit Goldilocks + Tip5 recursive profile for a
/// specific `(log_blowup, num_queries)` shape.
///
/// The caller supplies only the Johnson-relevant query shape. The
/// soundness-neutral compression levers and Merkle cap match
/// [`goldilocks_tip5_pure_query_60bit`]. This intentionally panics unless
/// `log_blowup * num_queries >= 60`, so diagnostics cannot accidentally
/// compare a weaker profile as a production candidate.
#[inline]
pub fn goldilocks_tip5_pure_query_60bit_with_shape(
    log_blowup: usize,
    num_queries: usize,
) -> GoldilocksTipsConfig {
    goldilocks_tip5_pure_query_60bit_with_shape_and_cap(
        log_blowup,
        num_queries,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_CAP_HEIGHT,
    )
}

/// Build a query-only 60-bit Goldilocks + Tip5 recursive profile for a
/// specific `(log_blowup, num_queries, cap_height)` shape.
///
/// This is a diagnostic/profiling variant of
/// [`goldilocks_tip5_pure_query_60bit_with_shape`]. The Merkle cap height is a
/// proof-size/prover-time trade-off and is not counted toward Johnson
/// soundness, so the same at-least-60-bit query-only assertion is retained.
#[inline]
pub fn goldilocks_tip5_pure_query_60bit_with_shape_and_cap(
    log_blowup: usize,
    num_queries: usize,
    cap_height: usize,
) -> GoldilocksTipsConfig {
    goldilocks_tip5_pure_query_60bit_with_fri_shape(
        log_blowup,
        num_queries,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_LOG_FINAL_POLY_LEN,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_MAX_LOG_ARITY,
        cap_height,
    )
}

/// Build a query-only 60-bit Goldilocks + Tip5 recursive profile for a
/// specific FRI proof-shape diagnostic.
///
/// `log_final_poly_len`, `max_log_arity`, and `cap_height` change proof shape
/// and size but are not counted as Johnson soundness bits. The function retains
/// the `log_blowup * num_queries >= 60` assertion so callers cannot
/// accidentally benchmark a weaker no-PoW candidate as production-equivalent.
#[inline]
pub fn goldilocks_tip5_pure_query_60bit_with_fri_shape(
    log_blowup: usize,
    num_queries: usize,
    log_final_poly_len: usize,
    max_log_arity: usize,
    cap_height: usize,
) -> GoldilocksTipsConfig {
    assert!(
        log_blowup * num_queries >= GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_JOHNSON_BITS,
        "pure-query recursive Tip5 profile must provide at least 60 Johnson bits"
    );
    goldilocks_tip5_with_fri_params(
        log_blowup,
        log_final_poly_len,
        max_log_arity,
        num_queries,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
        GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
        cap_height,
    )
}

// NOTE: a `goldilocks_tip5_60bit_higharity()` sibling existed at
// this position through 2026-05-20 — it diverged from
// `goldilocks_tip5_60bit()` only in `max_log_arity = 3,
// log_final_poly_len = 2`. The 2026-05-20 in-substrate stacking
// flip rolled those soundness-neutral levers into the production
// `goldilocks_tip5_60bit()` itself, making the sibling byte-
// identical. The sibling is now deleted; downstream callers that
// referenced it should use `goldilocks_tip5_60bit()` directly.

/// Type alias for BabyBear STARK configuration.
pub type BabyBearConfig =
    Config<BabyBear, Poseidon2BabyBear<16>, Poseidon2BabyBear<16>, 16, 16, 8, 8, 8, 4>;

/// Type alias for KoalaBear STARK configuration.
pub type KoalaBearConfig =
    Config<KoalaBear, Poseidon2KoalaBear<16>, Poseidon2KoalaBear<16>, 16, 16, 8, 8, 8, 4>;

/// Type alias for Goldilocks STARK configuration.
pub type GoldilocksConfig =
    Config<Goldilocks, Poseidon2Goldilocks<8>, Poseidon2Goldilocks<8>, 8, 8, 4, 4, 4, 2>;

pub type GoldilocksBlake3Challenge = BinomialExtensionField<Goldilocks, 2>;
type GoldilocksBlake3Hash = SerializingHasher<Blake3>;
type GoldilocksBlake3Compress = CompressionFunctionFromHasher<Blake3, 2, 32>;
pub type GoldilocksBlake3ValMmcs =
    MerkleTreeMmcs<Goldilocks, u8, GoldilocksBlake3Hash, GoldilocksBlake3Compress, 2, 32>;
type GoldilocksBlake3ChallengeMmcs =
    ExtensionMmcs<Goldilocks, GoldilocksBlake3Challenge, GoldilocksBlake3ValMmcs>;
type GoldilocksBlake3Challenger =
    SerializingChallenger64<Goldilocks, HashChallenger<u8, Blake3, 32>>;

/// Goldilocks final-layer STARK config with BLAKE3 MMCS commitments and a
/// BLAKE3 byte-serializing Fiat-Shamir transcript.
///
/// This config is intended for native-only final recursive layers. It is not
/// recursion friendly: verifier circuits that must verify this proof would need
/// BLAKE3 transcript and MMCS gadgets. Native verification remains a standard
/// Plonky3 FRI STARK over Goldilocks with caller-supplied FRI parameters.
pub type GoldilocksBlake3Config = StarkConfig<
    TwoAdicFriPcs<
        Goldilocks,
        Radix2DitParallel<Goldilocks>,
        GoldilocksBlake3ValMmcs,
        GoldilocksBlake3ChallengeMmcs,
    >,
    GoldilocksBlake3Challenge,
    GoldilocksBlake3Challenger,
>;

#[inline]
pub fn goldilocks_blake3_val_mmcs(cap_height: usize) -> GoldilocksBlake3ValMmcs {
    let hash = SerializingHasher::new(Blake3);
    let compress = CompressionFunctionFromHasher::<Blake3, 2, 32>::new(Blake3);
    MerkleTreeMmcs::new(hash, compress, cap_height)
}

#[inline]
pub fn goldilocks_blake3_with_fri_shape(
    log_blowup: usize,
    num_queries: usize,
    log_final_poly_len: usize,
    max_log_arity: usize,
    cap_height: usize,
) -> GoldilocksBlake3Config {
    let val_mmcs = goldilocks_blake3_val_mmcs(cap_height);
    let challenge_mmcs = ExtensionMmcs::new(val_mmcs.clone());
    let dft = Radix2DitParallel::default();
    let fri_params = FriParameters {
        log_blowup,
        log_final_poly_len,
        max_log_arity,
        num_queries,
        commit_proof_of_work_bits: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_COMMIT_POW_BITS,
        query_proof_of_work_bits: GOLDILOCKS_TIP5_RECURSIVE_PURE_QUERY_QUERY_POW_BITS,
        mmcs: challenge_mmcs,
    };
    let pcs = TwoAdicFriPcs::new(dft, val_mmcs, fri_params);
    let challenger = SerializingChallenger64::from_hasher(
        b"p3-goldilocks-blake3-final-layer-v1".to_vec(),
        Blake3,
    );
    StarkConfig::new(pcs, challenger)
}

/// **ADDITIVE (M-S5b S1.B Poseidon2-removal P1).** Tip5-unified
/// Goldilocks STARK configuration: recursive Tip5Perm (5-round, width 16, rate
/// 10, digest 5) for both MMCS hashing and Fiat-Shamir duplexing
/// challenger. Mirrors the inner ai-pow-zk STARK's Tip5 choice
/// (`crates/ai-pow-zk/src/circuit.rs:186, 203`); eliminates the
/// dual-hash architectural defect identified in
/// `crates/ai-pow-zk/docs/2026-05-20_PROOF_SIZE_REDUCTION_ROUTES_AUDIT.md`
/// § 3.2.0 + spec'd in
/// `crates/ai-pow-zk/docs/2026-05-20_POSEIDON2_REMOVAL_SPEC.md`.
///
/// **Predicted savings:** ~8–12 KB opened-values from eliminating
/// the Poseidon2 perm AIR sub-circuit + a structural max-constraint-
/// degree drop from 7 (Poseidon2 x⁷) to 2 (Tip5 lookup-table post-L4)
/// that further shrinks the quotient polynomial.
///
/// **Soundness:** identical to `GoldilocksConfig` at the FRI side
/// (same FRI params). Recursive proving uses the paper-spec 5-round
/// Tip5 variant only; Nockchain's canonical non-recursive hash path
/// remains the separate 7-round `nockchain_math::tip5::permute`.
pub type GoldilocksTipsConfig = Config<Goldilocks, Tip5Perm, Tip5Perm, 16, 16, 10, 5, 5, 2>;

/// Trait bounds for STARK-compatible fields.
pub trait StarkField: Field + PrimeCharacteristicRing + TwoAdicField + PrimeField64 {}

impl<F> StarkField for F where F: Field + PrimeCharacteristicRing + TwoAdicField + PrimeField64 {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_fields_configs_compile() {
        let _bb: BabyBearConfig = baby_bear();
        let _kb: KoalaBearConfig = koala_bear();
        let _gl: GoldilocksConfig = goldilocks();
    }

    /// M-S5b S1.B P1 — the new GoldilocksTipsConfig builder compiles
    /// and constructs without panic. Validates that Tip5Perm satisfies
    /// the Plonky3 Permutation + CryptographicPermutation trait bounds
    /// that the Config<...> type alias requires.
    #[test]
    fn goldilocks_tip5_unified_compiles() {
        let _c: GoldilocksTipsConfig = goldilocks_tip5_60bit();
    }

    #[test]
    fn goldilocks_tip5_recursive_profile_meets_60_bit_floor() {
        assert_eq!(GOLDILOCKS_TIP5_RECURSIVE_LOG_BLOWUP, 4);
        assert_eq!(GOLDILOCKS_TIP5_RECURSIVE_NUM_QUERIES, 9);
        assert_eq!(GOLDILOCKS_TIP5_RECURSIVE_QUERY_POW_BITS, 24);
        assert_eq!(GOLDILOCKS_TIP5_RECURSIVE_JOHNSON_BITS, 60);
        assert!(GOLDILOCKS_TIP5_RECURSIVE_JOHNSON_BITS >= 60);
    }

    #[test]
    fn pure_query_profile_accepts_stronger_johnson_bits() {
        let _ = goldilocks_tip5_pure_query_60bit_with_fri_shape(7, 9, 2, 3, 4);
    }

    #[test]
    #[should_panic(expected = "at least 60 Johnson bits")]
    fn pure_query_profile_rejects_weaker_johnson_bits() {
        let _ = goldilocks_tip5_pure_query_60bit_with_fri_shape(7, 8, 2, 3, 4);
    }
}
