#![allow(clippy::needless_range_loop)]
//! Plonky3 `StarkConfig` factory for the matmul puzzle.
//!
//! Pins the cryptographic stack:
//!
//! | Slot                  | Choice                            | Why |
//! |-----------------------|-----------------------------------|-----|
//! | Trace base field      | `Goldilocks` (p3-goldilocks)      | Native 64-bit prime; matches Pearl; friendly for the 32-bit ops in `p3-blake3-air`. |
//! | FRI challenge field   | `BinomialExtensionField<Goldilocks, 2>` | 128-bit security per challenge; standard pairing for Goldilocks STARKs. |
//! | FRI compression hash  | Recursive-certificate Tip5 adapter (`Tip5Perm`) | **5-round** variant (`nockchain_math::tip5::permute_5round`); STATE_SIZE=16, RATE=10, CAPACITY=6, DIGEST_LENGTH=5. This is selected only for the AI-PoW recursive-certificate proving stack so the composite proof's transcript is recursively verifiable. The canonical Nockchain hash remains `nockchain_math::tip5::permute` (7 rounds). Plonky3 upstream does *not* ship a `p3-tip5` crate; the in-repo `nockchain-math::tip5` is the canonical source. |
//! | Merkle MMCS           | `MerkleTreeMmcs<Val, Tip5Perm, ...>` | Standard Plonky3 mixed-matrix commitment, wrapping the Tip5 permutation in `PaddingFreeSponge` + `TruncatedPermutation`. |
//! | PCS                   | `TwoAdicFriPcs<…>`                | Univariate FRI; matches `p3-uni-stark`. |
//! | Challenger            | `DuplexChallenger<Val, Tip5Perm, _, _>` | Fiat-Shamir over the same Tip5 permutation. |
//!
//! `CircuitConfig` is the tunable side (rate, query count, PoW bits).
//! Production values are pinned by the 2026-05-21 anchored-between
//! Johnson policy below.

use p3_challenger::DuplexChallenger;
use p3_commit::ExtensionMmcs;
use p3_dft::Radix2DitParallel;
use p3_field::extension::BinomialExtensionField;
use p3_field::integers::QuotientMap;
use p3_field::{Field, PrimeField64};
use p3_fri::{FriParameters, TwoAdicFriPcs};
use p3_goldilocks::Goldilocks;
use p3_merkle_tree::MerkleTreeMmcs;
use p3_symmetric::{
    CryptographicPermutation, PaddingFreeSponge, Permutation, TruncatedPermutation,
};
use p3_uni_stark::StarkConfig;

use crate::params::ZkParams;

/// Trace base field. Re-exported here so the AIR / public-input / witness
/// modules can spell `crate::circuit::Val` and never touch Plonky3 directly.
pub type Val = Goldilocks;

// `Challenge` is the FRI challenge extension field — defined as a real
// type alias below, alongside the rest of the concrete STARK stack.

/// Configuration knobs for the Plonky3 STARK over the matmul AIR.
///
/// Security model: **unconditional / provable** FRI soundness. Under the
/// Johnson-radius proximity-gap bound proven by Ben-Sasson, Carmon, Habock,
/// Kopparty, Saraf, *"On Proximity Gaps for Reed-Solomon Codes"* (`IACR
/// ePrint 2025/2055`, Nov 2025, Theorem 1.5 + §1.3.2), each query gives
/// roughly `log_blowup` bits of soundness at the Johnson radius.
/// The implementation deliberately does not rely on Plonky3's
/// `CapacityBound::log_eta` heuristic, because that heuristic targets the
/// region between the Johnson radius and the list-decoding radius where
/// the paper gives no positive theorem for generic codes. The CYCLE-SUM
/// construction in Theorem 1.17 shows constructive attacks at the
/// list-decoding radius.
///
/// **Production policy:** the maintained floor is 60 Johnson bits. For the
/// deployed `PROD` profile, `lb=4 nq=15 pow=0+0`, so
/// `bits = lb * nq + 2 * pow = 60`. The bound is above the known-insecure
/// `log₂(n) + O(1)` list-decoding-radius region for the admitted trace sizes
/// and is the value recorded in the release-assurance ledger.
///
/// **Time-bounded threat model (rationale):** the 2.5-minute block cadence is
/// an economic exposure window for public-chain attempts, not a cryptographic
/// deadline that prevents private precomputation or private-fork attacks. The
/// release claim is therefore the Johnson-bound bit count below, plus the
/// operational assumption that useful block forgery must land before honest
/// work makes the target stale.
#[derive(Debug, Clone, Copy)]
pub struct CircuitConfig {
    /// Log2 of the FRI blowup factor. The committed evaluation domain
    /// is `2^log_blowup` times the trace length. `PROD = 4` gives rate
    /// `1/16`, which gives roughly `log_blowup = 4` bits of
    /// unconditional soundness per query at the Johnson radius (paper
    /// Theorem 1.5).
    pub log_blowup: u32,
    /// FRI PoW grinding bits at the challenger. Plonky3 applies this
    /// value to both commit-time and query-time PoW, so the Johnson
    /// accounting includes `2 * pow_bits`.
    pub pow_bits: u32,
    /// Number of FRI queries. Unconditional soundness (Johnson radius,
    /// paper Theorem 1.5): `num_queries * log_blowup + 2 * pow_bits`
    /// bits as implemented in [`build_stark_config`]. `PROD = 15` at
    /// `log_blowup = 4`, `pow_bits = 0` gives 60 pure-query bits.
    pub num_queries: u32,
}

pub const PROD_JOHNSON_FLOOR_BITS: u32 = 60;

pub const PLONKY3_SOUNDNESS_REV: &str = "11cc5849a1b57a2f520d6edc608b9e516517d841";
pub const FRI_SOUNDNESS_MODEL: &str = "IACR ePrint 2025/2055 Theorem 1.5 Johnson-radius bound";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriSoundnessProfile {
    pub log_blowup: u32,
    pub log_final_poly_len: u32,
    pub max_log_arity: u32,
    pub num_queries: u32,
    pub commit_pow_bits: u32,
    pub query_pow_bits: u32,
    pub cap_height: u32,
    pub constraint_degree: u32,
    pub log_trace_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FriSoundnessError {
    ZeroField(&'static str),
    DegreeExceedsBlowup {
        constraint_degree: u32,
        log_blowup: u32,
    },
    TraceTooShort {
        log_trace_height: u32,
        log_final_poly_len: u32,
        log_blowup: u32,
    },
    FoldArityTooLarge {
        max_log_arity: u32,
        remaining_log_domain: u32,
    },
    CapHeightTooLarge {
        cap_height: u32,
        log_commitment_height: u32,
    },
    JohnsonFloor {
        bits: u32,
        floor: u32,
    },
}

impl FriSoundnessProfile {
    pub const fn from_circuit_config(
        config: CircuitConfig,
        constraint_degree: u32,
        trace_len: usize,
    ) -> Self {
        Self {
            log_blowup: config.log_blowup,
            log_final_poly_len: 0,
            max_log_arity: 1,
            num_queries: config.num_queries,
            commit_pow_bits: config.pow_bits,
            query_pow_bits: config.pow_bits,
            cap_height: 0,
            constraint_degree,
            log_trace_height: trace_len.next_power_of_two().trailing_zeros(),
        }
    }

    pub const fn johnson_bits(self) -> u32 {
        self.log_blowup * self.num_queries + self.commit_pow_bits + self.query_pow_bits
    }

    pub const fn validate(self, floor_bits: u32) -> Result<(), FriSoundnessError> {
        if self.log_blowup == 0 {
            return Err(FriSoundnessError::ZeroField("log_blowup"));
        }
        if self.max_log_arity == 0 {
            return Err(FriSoundnessError::ZeroField("max_log_arity"));
        }
        if self.num_queries == 0 {
            return Err(FriSoundnessError::ZeroField("num_queries"));
        }
        if self.constraint_degree == 0 {
            return Err(FriSoundnessError::ZeroField("constraint_degree"));
        }
        if self.log_trace_height == 0 {
            return Err(FriSoundnessError::ZeroField("log_trace_height"));
        }
        if self.constraint_degree > (1u32 << self.log_blowup) {
            return Err(FriSoundnessError::DegreeExceedsBlowup {
                constraint_degree: self.constraint_degree,
                log_blowup: self.log_blowup,
            });
        }
        if self.log_trace_height <= self.log_final_poly_len + self.log_blowup {
            return Err(FriSoundnessError::TraceTooShort {
                log_trace_height: self.log_trace_height,
                log_final_poly_len: self.log_final_poly_len,
                log_blowup: self.log_blowup,
            });
        }
        let log_commitment_height = self.log_trace_height + self.log_blowup;
        if self.cap_height > log_commitment_height {
            return Err(FriSoundnessError::CapHeightTooLarge {
                cap_height: self.cap_height,
                log_commitment_height,
            });
        }
        let remaining_log_domain = log_commitment_height - self.log_final_poly_len;
        if self.max_log_arity > remaining_log_domain {
            return Err(FriSoundnessError::FoldArityTooLarge {
                max_log_arity: self.max_log_arity,
                remaining_log_domain,
            });
        }
        let bits = self.johnson_bits();
        if bits < floor_bits {
            return Err(FriSoundnessError::JohnsonFloor {
                bits,
                floor: floor_bits,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriProfileSearch {
    pub log_blowup: core::ops::RangeInclusive<u32>,
    pub log_final_poly_len: core::ops::RangeInclusive<u32>,
    pub max_log_arity: core::ops::RangeInclusive<u32>,
    pub num_queries: core::ops::RangeInclusive<u32>,
    pub cap_height: core::ops::RangeInclusive<u32>,
    pub constraint_degree: u32,
    pub log_trace_height: u32,
    pub floor_bits: u32,
}

pub fn admissible_fri_profiles(search: FriProfileSearch) -> Vec<FriSoundnessProfile> {
    let mut out = Vec::new();
    for log_blowup in search.log_blowup {
        for log_final_poly_len in search.log_final_poly_len.clone() {
            for max_log_arity in search.max_log_arity.clone() {
                for num_queries in search.num_queries.clone() {
                    for cap_height in search.cap_height.clone() {
                        let profile = FriSoundnessProfile {
                            log_blowup,
                            log_final_poly_len,
                            max_log_arity,
                            num_queries,
                            commit_pow_bits: 0,
                            query_pow_bits: 0,
                            cap_height,
                            constraint_degree: search.constraint_degree,
                            log_trace_height: search.log_trace_height,
                        };
                        if profile.validate(search.floor_bits).is_ok() {
                            out.push(profile);
                        }
                    }
                }
            }
        }
    }
    out
}

impl CircuitConfig {
    /// Johnson-radius FRI soundness bits for this profile under the
    /// accounting implemented in [`build_stark_config`].
    pub const fn johnson_fri_bits(self) -> u32 {
        self.log_blowup * self.num_queries + 2 * self.pow_bits
    }

    /// Production defaults. **Anchored-between policy (2026-05-21):**
    /// `lb=4 nq=15 pow=0` counts no proof-system PoW grinding, so
    /// `bits = lb*nq = 4*15 = 60` pure-query Johnson bits (Theorem 1.5,
    /// proven). This leaves a ≥38-bit margin above the known-insecure
    /// CYCLE-SUM floor at the chain LDR (~22 bits at n≤2^22).
    ///
    /// **Trade-off vs the prior `lb=4 nq=20 = 82-bit` PROD:** 25%
    /// fewer queries ⇒ proportional proof-size shrinkage at the inner
    /// layer (cascades to a smaller L1 verifier circuit). LDE size
    /// unchanged at 16× trace; prover memory unchanged.
    ///
    /// Rationale: see the struct-level doc-comment for the
    /// time-bounded (2.5-min block-cadence) threat model that
    /// justifies the relaxed 60-bit floor over the prior offline-
    /// cryptographic 80-bit floor.
    pub const PROD: Self = Self {
        log_blowup: 4,
        pow_bits: 0,
        num_queries: 15,
    };

    /// **60-bit Johnson floor at `log_blowup = 2`** (`2·30 = 60`, same floor as
    /// [`PROD`]) but a **4× smaller LDE** — cheaper Merkle commit at the cost of a
    /// larger L1 verifier circuit (2× the queries). Measured 2026-07-08: at a 2¹⁶
    /// Layer-0 trace this cuts the full compact prove 95.1 s → 55.4 s (1.72×); the
    /// win grows with degree. See `PROD_ADAPTIVE`.
    pub const PROD_LB2_NQ30: Self = Self {
        log_blowup: 2,
        pow_bits: 0,
        num_queries: 30,
    };

    /// **Degree-adaptive production profile (60-bit).** The Layer-0 prove is
    /// Merkle-commit-bound and the commit scales with `2^log_blowup × trace`,
    /// while the L1 recursion cost scales with `num_queries` (independent of the
    /// Layer-0 degree). So the total-prove-optimal blowup depends on the trace
    /// size: small traces favor the commit-heavy/recursion-cheap `lb=4/nq=15`;
    /// large traces favor the commit-cheap/recursion-heavier `lb=2/nq=30`. Both
    /// hold the 60-bit Johnson floor. Crossover measured near 2¹⁵ (12-core native):
    ///
    /// | Layer-0 degree | profile | full compact prove |
    /// |---|---|---|
    /// | ≤ 14 (e.g. 2¹³ default) | `PROD` (lb=4/nq=15) | 28.8 s |
    /// | ≥ 15 (e.g. 2¹⁶ max env) | `PROD_LB2_NQ30` | 55.4 s (vs 95.1 s) |
    ///
    /// Pearl's consensus-reachable trace buckets span `2^13..=2^19`, so both
    /// regimes occur; this picks the faster profile per degree while preserving
    /// the security floor exactly.
    pub const fn prod_adaptive(stark_degree_bits: usize) -> Self {
        if stark_degree_bits >= 15 {
            Self::PROD_LB2_NQ30
        } else {
            Self::PROD
        }
    }

    /// **The production Layer-0 / recursion FRI profile for a trace of `trace_len`
    /// rows** — [`prod_adaptive`](Self::prod_adaptive) keyed by the trace's STARK
    /// degree (`log2(trace_len)`). This is the **single source of truth** every
    /// Layer-0 prove, Layer-0 verify, L1 recursion, and recursive-certificate
    /// verify MUST use so the prover and verifier derive the *same* profile from
    /// the (public, proof-bound) trace length. `trace_len` is a power of two (the
    /// STARK trace is padded to one); non-power-of-two inputs round up so the
    /// degree is well-defined.
    pub fn for_layer0_trace(trace_len: usize) -> Self {
        debug_assert!(trace_len.is_power_of_two(), "STARK trace_len must be 2^k");
        let degree_bits = trace_len.next_power_of_two().trailing_zeros() as usize;
        Self::prod_adaptive(degree_bits)
    }

    /// Small profile for unit tests once the circuit is real.
    /// Soundness is not the goal here — we just want a fast
    /// prove/verify round-trip.
    pub const TEST: Self = Self {
        log_blowup: 1,
        pow_bits: 0,
        num_queries: 8,
    };

    /// ≥ 80-bit unconditional FRI soundness with `log_blowup = 2`,
    /// requiring **45 queries** (`2 · 45 = 90` bits Johnson-provable,
    /// ~10-bit margin). The LDE is only `4×` trace size (cheapest
    /// LDE) but the proof is the fattest of the sweep because FRI
    /// opens 45 paths.
    pub const PROD_LB2: Self = Self {
        log_blowup: 2,
        pow_bits: 0,
        num_queries: 45,
    };

    /// ≥ 80-bit unconditional FRI soundness with `log_blowup = 4`,
    /// requiring **23 queries** (`4 · 23 = 92` bits Johnson-provable).
    /// LDE is `16×` trace size — bigger Merkle commit, fewer openings.
    pub const PROD_LB4: Self = Self {
        log_blowup: 4,
        pow_bits: 0,
        num_queries: 23,
    };

    /// ≥ 80-bit unconditional FRI soundness with `log_blowup = 5`,
    /// requiring **18 queries** (`5 · 18 = 90` bits Johnson-provable).
    /// LDE is `32×` trace size — the prove side pays a lot, but the
    /// proof is among the smallest of the sweep.
    pub const PROD_LB5: Self = Self {
        log_blowup: 5,
        pow_bits: 0,
        num_queries: 18,
    };

    /// ≥ 80-bit unconditional FRI soundness with `log_blowup = 6`,
    /// requiring **15 queries** (`6 · 15 = 90` bits Johnson-provable).
    /// The extreme of the sweep.
    pub const PROD_LB6: Self = Self {
        log_blowup: 6,
        pow_bits: 0,
        num_queries: 15,
    };

    /// Test profile for the M10.1c Pearl-style composite AIR.
    ///
    /// Pearl pins `constraint_degree = 3` (see
    /// `Pearl zk-pow pearl_stark.rs:208-210`); the M10.1c
    /// composite chip set inherits that degree budget because per-chip
    /// constraints get multiplied by a `is_<chip>` boolean selector
    /// before firing. Selectors are degree 1; chip-internal constraints
    /// are degree 2; gated constraints reach degree 3.
    ///
    /// `log_blowup = 1` (the standard [`TEST`] profile) only admits
    /// degree-2 constraints (quotient degree `< 2^log_blowup = 2`).
    /// `TEST_PEARL` bumps to `log_blowup = 2` so degree-3 constraints
    /// fit while keeping tests fast.
    ///
    /// `num_queries = 16` gives ~32 bits of Johnson-provable soundness
    /// (`2 · 16 = 32`) — still non-cryptographic, intended for
    /// round-trip / tamper-detection tests. `PROD` (`log_blowup = 4,
    /// num_queries = 15`, `pow_bits = 0`) handles the real 60-bit
    /// pure-query anchored-between PoUW verification (paper Theorem 1.5).
    pub const TEST_PEARL: Self = Self {
        log_blowup: 2,
        pow_bits: 0,
        num_queries: 16,
    };
}

// =====================================================================
//  Type aliases for the concrete Plonky3 STARK stack.
// =====================================================================

/// Tip5 sponge for hashing matrix rows into Merkle leaves.
///   WIDTH = 16, RATE = 10, OUT = 5.
pub type Tip5Sponge = PaddingFreeSponge<Tip5Perm, 16, 10, 5>;

/// Tip5 2-to-1 truncated permutation for internal Merkle node compression.
///   ARITY = 2, OUT = 5, WIDTH = 16.
pub type Tip5Compress = TruncatedPermutation<Tip5Perm, 2, 5, 16>;

/// MMCS over Goldilocks values. `P = PW = <Goldilocks as Field>::Packing`
/// pulls in the SIMD-packed lane type so the Merkle commit step can
/// hash multiple field elements per call. Tip5 is run lane-by-lane via
/// the unpacking adapter `impl Permutation<[PackedGl; 16]>` below.
pub type ValMmcs = MerkleTreeMmcs<
    /* P */ <Goldilocks as Field>::Packing,
    /* PW */ <Goldilocks as Field>::Packing,
    /* H */ Tip5Sponge,
    /* C */ Tip5Compress,
    /* N (arity) */ 2,
    /* DIGEST_ELEMS */ 5,
>;

/// FRI challenge field: degree-2 binomial extension of Goldilocks.
pub type Challenge = BinomialExtensionField<Goldilocks, 2>;

/// MMCS for committing extension-field polynomials (FRI codewords).
pub type ChallengeMmcs = ExtensionMmcs<Goldilocks, Challenge, ValMmcs>;

/// Fiat–Shamir challenger using the same Tip5 permutation as the MMCS.
///   WIDTH = 16, RATE = 10.
pub type Challenger = DuplexChallenger<Goldilocks, Tip5Perm, 16, 10>;

/// DFT used by the FRI low-degree test on Goldilocks.
pub type Dft = Radix2DitParallel<Goldilocks>;

/// Univariate FRI PCS over Goldilocks.
pub type Pcs = TwoAdicFriPcs<Goldilocks, Dft, ValMmcs, ChallengeMmcs>;

/// The concrete `StarkConfig` `ai-pow-zk` uses everywhere.
pub type AiPowStarkConfig = StarkConfig<Pcs, Challenge, Challenger>;

// =====================================================================
//  Builder.
// =====================================================================

/// Assemble the Plonky3 `StarkConfig` for a given `(ZkParams, CircuitConfig)`.
///
/// The `ZkParams` is currently unused — proof shape depends only on the
/// AIR's trace width and height, both of which are computed by
/// `ai_pow_zk::prove` from the witness. The argument is kept for
/// forward-compatibility (e.g. choosing `log_final_poly_len` per matmul
/// shape later).
pub fn build_stark_config(_params: &ZkParams, config: &CircuitConfig) -> AiPowStarkConfig {
    let perm = Tip5Perm;
    let hash = Tip5Sponge::new(perm);
    let compress = Tip5Compress::new(perm);
    // `cap_height = 0` uses only the Merkle root; no cap. The cap is an
    // optimization for parallel verification, irrelevant at our trace
    // sizes.
    let val_mmcs = ValMmcs::new(hash, compress, 0);
    let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
    let dft = Dft::default();
    let challenger = Challenger::new(perm);
    let fri_params = FriParameters {
        log_blowup: config.log_blowup as usize,
        // log_final_poly_len controls the size of the constant FRI
        // tail. 0 = single-element tail (no early stop). Bumping later
        // shrinks proofs at the cost of slightly weaker soundness per
        // query; we keep it at 0 for the strongest provable bound.
        log_final_poly_len: 0,
        max_log_arity: 1, // binary folding
        num_queries: config.num_queries as usize,
        // Both FRI PoW tiers come from the same knob; soundness accounting
        // is `log_blowup * num_queries + 2 * pow_bits`.
        commit_proof_of_work_bits: config.pow_bits as usize,
        query_proof_of_work_bits: config.pow_bits as usize,
        mmcs: challenge_mmcs,
    };
    let pcs = Pcs::new(dft, val_mmcs, fri_params);
    StarkConfig::new(pcs, challenger)
}

/// Recursive-certificate newtype around
/// `nockchain_math::tip5::permute_5round`.
///
/// This adapter wraps the 16-element Goldilocks state so it can plug
/// into Plonky3's `CryptographicPermutation<[Val; 16]>` trait for
/// the AI-PoW proof transcript and recursive verifier circuit. It is
/// deliberately not the canonical Nockchain Tip5 hash; non-recursive
/// Nockchain hashing remains `nockchain_math::tip5::permute`
/// (7 rounds).
#[derive(Debug, Clone, Copy, Default)]
pub struct Tip5Perm;

impl Tip5Perm {
    /// Width of the Tip5 sponge state, in field elements. Mirrors
    /// `nockchain_math::tip5::STATE_SIZE`.
    pub const WIDTH: usize = nockchain_math::tip5::STATE_SIZE;

    /// Rate (input absorption per permutation call). Mirrors
    /// `nockchain_math::tip5::RATE`.
    pub const RATE: usize = nockchain_math::tip5::RATE;

    /// Capacity (state retained across calls). Mirrors
    /// `nockchain_math::tip5::CAPACITY`.
    pub const CAPACITY: usize = nockchain_math::tip5::CAPACITY;

    /// Number of permutation rounds — the **5-round** Tip5 variant.
    ///
    /// §recursion integration (2026-05-22): aligned with the
    /// `Plonky3-recursion` Tip5 circuit AIR (`tip5-circuit-air/src/
    /// tip5_spec.rs`, `NUM_ROUNDS = 5`) so the composite proof's
    /// FRI/MMCS/challenger transcript is byte-identical to what the
    /// in-circuit recursion verifier recomputes. Was the 7-round
    /// `nockchain_math::tip5::NUM_ROUNDS`; the recursion verifier
    /// rejected honest 7-round proofs (transcript divergence).
    pub const NUM_ROUNDS: usize = nockchain_math::tip5::NUM_ROUNDS_5ROUND;

    /// Apply the in-place 5-round Tip5 permutation to a 16-element
    /// state. One-line wrapper so the call site reads
    /// `Tip5Perm::permute(&mut s)`.
    pub fn permute(state: &mut [u64; Self::WIDTH]) {
        nockchain_math::tip5::permute_5round(state);
    }
}

// Plonky3 wires sponges and challengers through the `Permutation<T>`
// trait, where `T` is the state type. Our state type is
// `[Goldilocks; 16]`. We convert `Goldilocks → u64` via
// `PrimeField64::as_canonical_u64` and back via the `QuotientMap` impl.
// The recursive-certificate `permute_5round` variant then operates on
// the raw u64 buffer, reducing mod the Goldilocks prime per round
// constant.
impl Permutation<[Goldilocks; Tip5Perm::WIDTH]> for Tip5Perm {
    fn permute_mut(&self, input: &mut [Goldilocks; Tip5Perm::WIDTH]) {
        let mut raw: [u64; Tip5Perm::WIDTH] = [0u64; Tip5Perm::WIDTH];
        for i in 0..Tip5Perm::WIDTH {
            raw[i] = input[i].as_canonical_u64();
        }
        nockchain_math::tip5::permute_5round(&mut raw);
        // After the permutation, each lane is < ORDER_U64. The Plonky3
        // Goldilocks impl accepts arbitrary u64s; `from_int` is the
        // canonical "reduce a u64 into the field" constructor.
        for i in 0..Tip5Perm::WIDTH {
            input[i] = <Goldilocks as QuotientMap<u64>>::from_int(raw[i]);
        }
    }
}

// Marker: we treat Tip5 as cryptographically secure for our purposes.
impl CryptographicPermutation<[Goldilocks; Tip5Perm::WIDTH]> for Tip5Perm {}

// Packed-Goldilocks variant. Plonky3's `DuplexChallenger: GrindingChallenger`
// (used inside FRI's PoW phase, even when `pow_bits = 0`) bounds the
// permutation over both scalar and packed lanes:
//
//     P: CryptographicPermutation<[F; WIDTH]>
//      + CryptographicPermutation<[<F as Field>::Packing; WIDTH]>
//
// On platforms where Goldilocks has a real SIMD-packed type (aarch64
// Neon, x86_64 AVX2/AVX-512), we add a second `Permutation` impl that
// unpacks lane-by-lane, runs scalar `nockchain_math::tip5::permute_5round`
// on each lane, and repacks. This MUST use the same 5-round permutation
// as the scalar `Permutation` impl above (`§recursion` alignment) — the
// MMCS commit step batches Merkle-tree hashing over the packed lanes, so
// a packed/scalar round-count mismatch desynchronises the prover's
// committed cap from the verifier's scalar-path recompute (`CapMismatch`).
// This is functionally correct (each SIMD lane is an independent
// Goldilocks element); a real SIMD-native Tip5 would be faster but is
// out of scope.
//
// We name the concrete packed types directly (rather than going
// through `<Goldilocks as Field>::Packing`) because rustc's coherence
// checker can't disambiguate the projection from the scalar type
// across cfg variants — see the conflicting-impl error you hit if you
// try the projection route.

#[cfg(target_arch = "aarch64")]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksNeon;

    use super::*;

    impl Permutation<[PackedGoldilocksNeon; Tip5Perm::WIDTH]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksNeon; Tip5Perm::WIDTH]) {
            let lanes = <PackedGoldilocksNeon as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; Tip5Perm::WIDTH];
                for i in 0..Tip5Perm::WIDTH {
                    state[i] = input[i].as_slice()[lane].as_canonical_u64();
                }
                nockchain_math::tip5::permute_5round(&mut state);
                for i in 0..Tip5Perm::WIDTH {
                    input[i].as_slice_mut()[lane] =
                        <Goldilocks as QuotientMap<u64>>::from_int(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksNeon; Tip5Perm::WIDTH]> for Tip5Perm {}
}

#[cfg(all(target_arch = "x86_64", target_feature = "avx512f"))]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksAVX512;

    use super::*;

    impl Permutation<[PackedGoldilocksAVX512; Tip5Perm::WIDTH]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksAVX512; Tip5Perm::WIDTH]) {
            let lanes = <PackedGoldilocksAVX512 as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; Tip5Perm::WIDTH];
                for i in 0..Tip5Perm::WIDTH {
                    state[i] = input[i].as_slice()[lane].as_canonical_u64();
                }
                nockchain_math::tip5::permute_5round(&mut state);
                for i in 0..Tip5Perm::WIDTH {
                    input[i].as_slice_mut()[lane] =
                        <Goldilocks as QuotientMap<u64>>::from_int(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksAVX512; Tip5Perm::WIDTH]> for Tip5Perm {}
}

#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    not(target_feature = "avx512f")
))]
mod packed_perm {
    use p3_field::PackedValue;
    use p3_goldilocks::PackedGoldilocksAVX2;

    use super::*;

    impl Permutation<[PackedGoldilocksAVX2; Tip5Perm::WIDTH]> for Tip5Perm {
        fn permute_mut(&self, input: &mut [PackedGoldilocksAVX2; Tip5Perm::WIDTH]) {
            let lanes = <PackedGoldilocksAVX2 as PackedValue>::WIDTH;
            for lane in 0..lanes {
                let mut state = [0u64; Tip5Perm::WIDTH];
                for i in 0..Tip5Perm::WIDTH {
                    state[i] = input[i].as_slice()[lane].as_canonical_u64();
                }
                nockchain_math::tip5::permute_5round(&mut state);
                for i in 0..Tip5Perm::WIDTH {
                    input[i].as_slice_mut()[lane] =
                        <Goldilocks as QuotientMap<u64>>::from_int(state[i]);
                }
            }
        }
    }

    impl CryptographicPermutation<[PackedGoldilocksAVX2; Tip5Perm::WIDTH]> for Tip5Perm {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convert a `[Goldilocks; 16]` state to `[u64; 16]` via the public
    /// canonical-u64 view, exactly the way `Tip5Perm` does internally.
    fn to_u64s(state: &[Goldilocks; Tip5Perm::WIDTH]) -> [u64; Tip5Perm::WIDTH] {
        let mut out = [0u64; Tip5Perm::WIDTH];
        for i in 0..Tip5Perm::WIDTH {
            out[i] = state[i].as_canonical_u64();
        }
        out
    }

    /// Convert a `[u64; 16]` back to `[Goldilocks; 16]` via `from_int`.
    fn from_u64s(raw: &[u64; Tip5Perm::WIDTH]) -> [Goldilocks; Tip5Perm::WIDTH] {
        let mut out = [Goldilocks::default(); Tip5Perm::WIDTH];
        for i in 0..Tip5Perm::WIDTH {
            out[i] = <Goldilocks as QuotientMap<u64>>::from_int(raw[i]);
        }
        out
    }

    #[test]
    fn tip5_perm_width_constants_match_nockchain_math() {
        assert_eq!(Tip5Perm::WIDTH, nockchain_math::tip5::STATE_SIZE);
        assert_eq!(Tip5Perm::WIDTH, 16);
        assert_eq!(Tip5Perm::RATE, nockchain_math::tip5::RATE);
        assert_eq!(Tip5Perm::RATE, 10);
        assert_eq!(Tip5Perm::CAPACITY, nockchain_math::tip5::CAPACITY);
        assert_eq!(Tip5Perm::CAPACITY, 6);
        // §recursion (2026-05-22): Tip5Perm uses the 5-round variant
        // so the composite proof's transcript matches the recursion
        // verifier (see `Tip5Perm::NUM_ROUNDS`).
        assert_eq!(
            Tip5Perm::NUM_ROUNDS,
            nockchain_math::tip5::NUM_ROUNDS_5ROUND
        );
        assert_eq!(Tip5Perm::NUM_ROUNDS, 5);
        assert_eq!(
            Tip5Perm::WIDTH,
            Tip5Perm::RATE + Tip5Perm::CAPACITY,
            "WIDTH must equal RATE + CAPACITY"
        );
    }

    #[test]
    fn tip5_perm_static_wrapper_matches_nockchain_math() {
        // `Tip5Perm::permute(&mut s)` is just a wrapper; assert the
        // produced state byte-equals direct nockchain_math invocation.
        let mut raw_a: [u64; 16] =
            std::array::from_fn(|i| (0x1234_5678_9abc_def0u64).wrapping_mul((i as u64) + 1));
        let mut raw_b = raw_a;
        Tip5Perm::permute(&mut raw_a);
        nockchain_math::tip5::permute_5round(&mut raw_b);
        assert_eq!(raw_a, raw_b);
    }

    #[test]
    fn recursive_tip5_adapter_does_not_replace_canonical_nockchain_tip5() {
        assert_eq!(nockchain_math::tip5::NUM_ROUNDS, 7);
        assert_eq!(nockchain_math::tip5::NUM_ROUNDS_5ROUND, 5);
        assert_eq!(
            Tip5Perm::NUM_ROUNDS,
            nockchain_math::tip5::NUM_ROUNDS_5ROUND
        );

        let initial: [u64; 16] =
            std::array::from_fn(|i| 0xfeed_face_cafe_beefu64.wrapping_add((i as u64) * 19));
        let mut via_recursive_adapter = initial;
        let mut via_5round = initial;
        let mut via_canonical_7round = initial;

        Tip5Perm::permute(&mut via_recursive_adapter);
        nockchain_math::tip5::permute_5round(&mut via_5round);
        nockchain_math::tip5::permute(&mut via_canonical_7round);

        assert_eq!(via_recursive_adapter, via_5round);
        assert_ne!(
            via_recursive_adapter, via_canonical_7round,
            "recursive proving must use the 5-round adapter without changing canonical 7-round Tip5"
        );
    }

    #[test]
    fn tip5_perm_plonky3_permute_matches_static_wrapper() {
        // The trait-method path (used by Plonky3's sponge/challenger)
        // must produce the same final state as the static wrapper
        // applied to the corresponding u64 buffer.
        let perm = Tip5Perm;
        let initial_u64: [u64; 16] = std::array::from_fn(|i| (i as u64) * 0xdeadbeef_0badf00d);
        let initial_gl = from_u64s(&initial_u64);

        let mut via_trait = initial_gl;
        perm.permute_mut(&mut via_trait);
        let via_trait_u64 = to_u64s(&via_trait);

        let mut via_static = initial_u64;
        nockchain_math::tip5::permute_5round(&mut via_static);
        // `from_int`'s canonicalization may not change the value modulo
        // the prime, so compare canonical forms.
        let via_static_canon: [u64; 16] = {
            let gl = from_u64s(&via_static);
            to_u64s(&gl)
        };
        assert_eq!(via_trait_u64, via_static_canon);
    }

    #[test]
    fn tip5_perm_permute_is_deterministic() {
        let perm = Tip5Perm;
        let state: [Goldilocks; 16] = from_u64s(&[7u64; 16]);
        let a = perm.permute(state);
        let b = perm.permute(state);
        assert_eq!(to_u64s(&a), to_u64s(&b));
    }

    #[test]
    fn tip5_perm_permute_is_input_sensitive() {
        // Flipping one lane changes the output non-trivially.
        let perm = Tip5Perm;
        let base: [Goldilocks; 16] = from_u64s(&[0u64; 16]);
        let mut tweaked = base;
        tweaked[3] = <Goldilocks as QuotientMap<u64>>::from_int(1);
        let out_base = to_u64s(&perm.permute(base));
        let out_tweaked = to_u64s(&perm.permute(tweaked));
        assert_ne!(out_base, out_tweaked);
        // Most lanes should change too (diffusion sanity check; not a
        // tight statistical assertion).
        let diffs = (0..16).filter(|i| out_base[*i] != out_tweaked[*i]).count();
        assert!(
            diffs >= 8,
            "expected at least 8 lanes to differ after recursive 5-round Tip5; got {diffs}"
        );
    }

    #[test]
    fn tip5_perm_round_trip_via_clone() {
        // Plonky3's `Permutation<T>` blanket-implements `permute` from
        // `permute_mut` via `Clone`. Confirm both paths agree.
        let perm = Tip5Perm;
        let state: [Goldilocks; 16] = from_u64s(&std::array::from_fn(|i| (i as u64) * 17 + 5));
        let via_owned = perm.permute(state);
        let mut via_mut = state;
        perm.permute_mut(&mut via_mut);
        assert_eq!(to_u64s(&via_owned), to_u64s(&via_mut));
    }

    #[test]
    fn padding_free_sponge_compiles_and_hashes() {
        // Smoke test that the sponge type accepts our adapter and
        // produces a non-zero digest for a small input.
        use p3_symmetric::{CryptographicHasher, PaddingFreeSponge};
        let perm = Tip5Perm;
        let sponge: PaddingFreeSponge<Tip5Perm, 16, 10, 5> = PaddingFreeSponge::new(perm);
        let input: [Goldilocks; 7] = from_u64s(&[1, 2, 3, 4, 5, 6, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            [..7]
            .try_into()
            .unwrap();
        let digest: [Goldilocks; 5] = sponge.hash_iter(input.iter().copied());
        let digest_u64 = [
            digest[0].as_canonical_u64(),
            digest[1].as_canonical_u64(),
            digest[2].as_canonical_u64(),
            digest[3].as_canonical_u64(),
            digest[4].as_canonical_u64(),
        ];
        // Determinism.
        let digest2: [Goldilocks; 5] = sponge.hash_iter(input.iter().copied());
        let digest2_u64 = [
            digest2[0].as_canonical_u64(),
            digest2[1].as_canonical_u64(),
            digest2[2].as_canonical_u64(),
            digest2[3].as_canonical_u64(),
            digest2[4].as_canonical_u64(),
        ];
        assert_eq!(digest_u64, digest2_u64);
        // Non-trivial output (at least one lane non-zero).
        assert!(digest_u64.iter().any(|&v| v != 0));
    }

    #[test]
    fn padding_free_sponge_input_sensitive() {
        use p3_symmetric::{CryptographicHasher, PaddingFreeSponge};
        let perm = Tip5Perm;
        let sponge: PaddingFreeSponge<Tip5Perm, 16, 10, 5> = PaddingFreeSponge::new(perm);
        let a = from_u64s(&[1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])[..5].to_vec();
        let b = from_u64s(&[1, 2, 3, 4, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])[..5].to_vec();
        let da: [Goldilocks; 5] = sponge.hash_iter(a);
        let db: [Goldilocks; 5] = sponge.hash_iter(b);
        let to = |d: [Goldilocks; 5]| {
            [
                d[0].as_canonical_u64(),
                d[1].as_canonical_u64(),
                d[2].as_canonical_u64(),
                d[3].as_canonical_u64(),
                d[4].as_canonical_u64(),
            ]
        };
        assert_ne!(to(da), to(db));
    }

    fn sample_zk_params() -> ZkParams {
        ZkParams {
            m: 64,
            k: 64,
            n: 64,
            noise_rank: 32,
            tile: 8,
            difficulty_bits: 0,
        }
    }

    #[test]
    fn circuit_config_constants_are_well_formed() {
        // PROD is the deployed 60-bit Johnson profile:
        // lb=4 nq=15 pow=0+0, so 4 * 15 = 60 under Theorem 1.5.
        let prod = CircuitConfig::PROD;
        assert_eq!(prod.log_blowup, 4);
        assert_eq!(prod.num_queries, 15);
        assert_eq!(prod.pow_bits, 0);
        let johnson_bits = prod.johnson_fri_bits();
        assert_eq!(johnson_bits, PROD_JOHNSON_FLOOR_BITS);
        assert!(
            johnson_bits >= PROD_JOHNSON_FLOOR_BITS,
            "PROD must meet the 60-bit anchored Johnson floor"
        );
        // TEST is just for speed; sanity checks only.
        let test = CircuitConfig::TEST;
        assert!(test.log_blowup >= 1);
        assert!(test.num_queries >= 1);
        assert_eq!(test.pow_bits, 0);
    }

    /// Each `PROD_LBn` profile must meet the deployed ≥60-bit Johnson-radius
    /// floor (paper Theorem 1.5, maintainer policy).
    #[test]
    fn prod_sweep_profiles_meet_anchored_johnson_floor() {
        // Sweep variants are measurement comparators for proof-size studies.
        for (name, cfg) in [
            ("PROD", CircuitConfig::PROD),
            ("PROD_LB2", CircuitConfig::PROD_LB2),
            ("PROD_LB4", CircuitConfig::PROD_LB4),
            ("PROD_LB5", CircuitConfig::PROD_LB5),
            ("PROD_LB6", CircuitConfig::PROD_LB6),
        ] {
            // Unconditional Johnson bound per IACR ePrint 2025/2055
            // Theorem 1.5: lb*nq + both FRI PoW tiers.
            let bits = cfg.johnson_fri_bits();
            assert!(
                bits >= PROD_JOHNSON_FLOOR_BITS,
                "{name}: johnson_bits = lb*nq + 2*pow = {}*{} + 2*{} = {} < {}",
                cfg.log_blowup,
                cfg.num_queries,
                cfg.pow_bits,
                bits,
                PROD_JOHNSON_FLOOR_BITS
            );
        }
    }

    #[test]
    fn fri_soundness_calculator_accepts_prod_and_rejects_underbounds() {
        assert_eq!(
            PLONKY3_SOUNDNESS_REV,
            "11cc5849a1b57a2f520d6edc608b9e516517d841"
        );
        assert_eq!(
            FRI_SOUNDNESS_MODEL,
            "IACR ePrint 2025/2055 Theorem 1.5 Johnson-radius bound"
        );

        let prod = FriSoundnessProfile::from_circuit_config(
            CircuitConfig::PROD,
            3,
            crate::composite_layout::MIN_STARK_LEN,
        );
        assert_eq!(prod.johnson_bits(), PROD_JOHNSON_FLOOR_BITS);
        assert_eq!(prod.validate(PROD_JOHNSON_FLOOR_BITS), Ok(()));

        let bad_degree = FriSoundnessProfile {
            log_blowup: 1,
            ..prod
        };
        assert_eq!(
            bad_degree.validate(PROD_JOHNSON_FLOOR_BITS),
            Err(FriSoundnessError::DegreeExceedsBlowup {
                constraint_degree: 3,
                log_blowup: 1
            })
        );

        let short_trace = FriSoundnessProfile {
            log_final_poly_len: 2,
            log_trace_height: 5,
            ..prod
        };
        assert_eq!(
            short_trace.validate(PROD_JOHNSON_FLOOR_BITS),
            Err(FriSoundnessError::TraceTooShort {
                log_trace_height: 5,
                log_final_poly_len: 2,
                log_blowup: 4
            })
        );

        let low_queries = FriSoundnessProfile {
            num_queries: 14,
            ..prod
        };
        assert_eq!(
            low_queries.validate(PROD_JOHNSON_FLOOR_BITS),
            Err(FriSoundnessError::JohnsonFloor {
                bits: 56,
                floor: PROD_JOHNSON_FLOOR_BITS
            })
        );
    }

    #[test]
    fn fri_soundness_search_generates_only_admissible_candidates() {
        let candidates = admissible_fri_profiles(FriProfileSearch {
            log_blowup: 1..=4,
            log_final_poly_len: 0..=2,
            max_log_arity: 1..=3,
            num_queries: 1..=30,
            cap_height: 0..=4,
            constraint_degree: 3,
            log_trace_height: 13,
            floor_bits: PROD_JOHNSON_FLOOR_BITS,
        });
        assert!(candidates
            .iter()
            .all(|profile| profile.validate(PROD_JOHNSON_FLOOR_BITS).is_ok()));
        assert!(candidates.iter().any(|profile| {
            profile.log_blowup == 2
                && profile.num_queries == 30
                && profile.log_final_poly_len == 0
                && profile.max_log_arity == 1
        }));
        assert!(!candidates.iter().any(|profile| profile.log_blowup == 1));
    }

    #[test]
    fn build_stark_config_prod_assembles() {
        // Construction must not panic on PROD knobs.
        let cfg = build_stark_config(&sample_zk_params(), &CircuitConfig::PROD);
        // Clone confirms the whole tree implements Clone (required by
        // `p3_uni_stark` for the prove/verify entry points).
        let _ = cfg.clone();
    }

    #[test]
    fn build_stark_config_test_assembles() {
        let cfg = build_stark_config(&sample_zk_params(), &CircuitConfig::TEST);
        let _ = cfg.clone();
    }

    /// M10.1c smoke test: TEST_PEARL profile assembles and admits a
    /// log_blowup ≥ 2 quotient budget (needed for Pearl's degree-3
    /// constraints when chip evals are gated by a degree-1 selector).
    #[test]
    fn build_stark_config_test_pearl_assembles() {
        let pearl = CircuitConfig::TEST_PEARL;
        assert_eq!(pearl.log_blowup, 2);
        assert_eq!(pearl.pow_bits, 0);
        assert!(pearl.num_queries >= 8);
        // quotient_degree ≤ 2^log_blowup, so the budget admits
        // constraint_degree − 1 = 2 → constraint_degree = 3 (matches
        // Pearl's `constraint_degree() -> 3`).
        assert!(1u32 << pearl.log_blowup >= 3 /* degree-3 quotient bound */);
        let cfg = build_stark_config(&sample_zk_params(), &pearl);
        let _ = cfg.clone();
    }

    #[test]
    fn build_stark_config_accepts_custom_knobs() {
        // The FRI params field on `TwoAdicFriPcs` is `pub(crate)` so
        // we can't read them back directly. Instead, smoke-test that
        // build_stark_config accepts a non-default CircuitConfig
        // without panicking and the resulting StarkConfig is Cloneable
        // (a requirement of p3-uni-stark's prove/verify signatures).
        let custom = CircuitConfig {
            log_blowup: 2,
            num_queries: 30,
            pow_bits: 0,
        };
        let cfg = build_stark_config(&sample_zk_params(), &custom);
        let _ = cfg.clone();
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn packed_tip5_matches_scalar_lane_by_lane_aarch64() {
        // For each SIMD lane, running Tip5 on the packed state must
        // produce the same field element as running scalar Tip5 on the
        // corresponding lane's scalar inputs.
        use p3_field::PackedValue;
        use p3_goldilocks::PackedGoldilocksNeon;
        type P = PackedGoldilocksNeon;
        let perm = Tip5Perm;
        let mut scalar_states: Vec<[Goldilocks; 16]> = (0..<P as PackedValue>::WIDTH)
            .map(|lane| {
                from_u64s(&std::array::from_fn(|i| {
                    (lane as u64 + 1) * 0xdeadbeef + (i as u64 * 7)
                }))
            })
            .collect();
        let mut packed_state: [P; 16] =
            std::array::from_fn(|i| P::from_fn(|lane| scalar_states[lane][i]));
        for s in scalar_states.iter_mut() {
            perm.permute_mut(s);
        }
        perm.permute_mut(&mut packed_state);
        for lane in 0..<P as PackedValue>::WIDTH {
            for i in 0..16 {
                assert_eq!(
                    packed_state[i].as_slice()[lane].as_canonical_u64(),
                    scalar_states[lane][i].as_canonical_u64(),
                    "lane {lane}, state[{i}]"
                );
            }
        }
    }

    #[test]
    fn build_stark_config_provable_soundness_at_prod() {
        // Sanity assertion of the security claim: log_blowup ·
        // num_queries + both FRI PoW tiers = unconditional Johnson bits per
        // paper IACR ePrint 2025/2055 Theorem 1.5. **2026-05-21
        // anchored-between policy:** PROD `lb=4 nq=15 pow=0` ⇒
        // 4·15 = 60 pure-query bits. Meets the
        // ≥60-bit anchored-Johnson floor (maintainer 2026-05-21,
        // anchored between known-insecure ~22-bit CYCLE-SUM ceiling
        // at γ≥LDR and the prior conservative 80-bit floor).
        let prod = CircuitConfig::PROD;
        let _ = build_stark_config(&sample_zk_params(), &prod);
        let johnson_bits = prod.johnson_fri_bits();
        assert_eq!(johnson_bits, PROD_JOHNSON_FLOOR_BITS);
        assert!(
            johnson_bits >= PROD_JOHNSON_FLOOR_BITS,
            "PROD must meet the 60-bit anchored Johnson floor"
        );
    }

    #[test]
    fn truncated_permutation_two_to_one_deterministic() {
        // The 2→1 compress used in MerkleTreeMmcs takes two digests
        // (each of size DIGEST), concatenates them into the first
        // 2*DIGEST lanes of the WIDTH state, permutes, and reads back
        // the first DIGEST lanes.
        use p3_symmetric::{PseudoCompressionFunction, TruncatedPermutation};
        let perm = Tip5Perm;
        let compress: TruncatedPermutation<Tip5Perm, 2, 5, 16> = TruncatedPermutation::new(perm);
        let left: [Goldilocks; 5] =
            from_u64s(&[10, 20, 30, 40, 50, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])[..5]
                .try_into()
                .unwrap();
        let right: [Goldilocks; 5] =
            from_u64s(&[60, 70, 80, 90, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])[..5]
                .try_into()
                .unwrap();
        let c1 = compress.compress([left, right]);
        let c2 = compress.compress([left, right]);
        let c1_u64: [u64; 5] = std::array::from_fn(|i| c1[i].as_canonical_u64());
        let c2_u64: [u64; 5] = std::array::from_fn(|i| c2[i].as_canonical_u64());
        assert_eq!(c1_u64, c2_u64, "compress must be deterministic");
        // Order-sensitive: swapping (left, right) must change the
        // output. The state shape is `[left | right | capacity]`, so
        // a non-trivial permutation will diffuse the swap.
        let c_swapped = compress.compress([right, left]);
        let c_swapped_u64: [u64; 5] = std::array::from_fn(|i| c_swapped[i].as_canonical_u64());
        assert_ne!(c1_u64, c_swapped_u64);
    }
}
