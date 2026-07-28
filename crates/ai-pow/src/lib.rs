//! AI-PoW v3: Pearl proof-of-useful-work with miner-supplied `A` and `B`.
//!
//! Implements the Pearl Whitepaper PoUW puzzle for caller-chosen INT8
//! matrices `A` and `B`:
//!
//! 1. **Commitments** (Pearl §4.3, Alg. 2): `κ` derived from the
//!    nonce-bound attempt state `(block_commitment, nonce, params_tag)`;
//!    `h_a` and `h_b` are legacy row/column Merkle roots for plain
//!    spot-opening diagnostics; `h_a_chunk` and `h_b_chunk` are nonce-keyed
//!    matrix commitments bound by the recursive ZK proof as `HASH_A` /
//!    `HASH_B`. Seeds `s_B = derive_key("s_b", κ ‖ h_b_chunk)` and
//!    `s_A = derive_key("s_a", s_B ‖ h_a_chunk)` bind the noise to the
//!    proof-bound matrices.
//! 2. **Low-rank noise** (Pearl §4.4, Alg. 3): `E = E_L · E_R` and
//!    `F = F_L · F_R` of rank `r`; `E_L, F_R` are int6, `E_R, F_L` are
//!    choice matrices (one `+1` and one `-1` per col/row).
//! 3. **Iterative tile state** (Pearl §4.5, Alg. 4): 512-bit `M_{i,j}`
//!    accumulator updated every `r`-stripe along the `k`-axis via
//!    `M[ℓ mod 16] ← (M[ℓ mod 16] ≪ 13) ⊕ X_ℓ`.
//! 4. **Shape-aware hardness**: `BLAKE3(M, key = pow_key) ≤ 2^(256-b) · r · t^2`,
//!    with `pow_key = derive_key("pow-key", s_A ‖ nonce)`. Since `s_A` is
//!    already nonce-bound through `κ`, changing the nonce requires fresh
//!    commitments, noise, and matmul-derived tile states before the hash check.
//!
//! The proof contains `H_A`, `H_B`, and per-opening row/column strips of
//! `A`/`B` with BLAKE3 Merkle authentication paths up to those roots, so
//! the verifier can replicate one tile without seeing the full matrices.
//! No SNARK / STARK is used (Pearl §4.7 — separate work).

// `unwrap()` is acceptable in unit tests (repo convention; production code uses
// explicit error handling / `expect` with a stated invariant).
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod commit;
/// Canonical AI-PoW difficulty representation: consensus target, shape work
/// factor, effective jackpot threshold, and the one accept predicate every
/// miner and the verifier share.
pub mod difficulty;
pub mod fiat_shamir;
pub mod matmul;
pub mod params;
pub mod pearl_compat;
pub mod pearl_moe_routing;
pub mod prng;
pub mod proof;
pub mod prover;
pub mod quant;
pub mod synth;
pub mod tile_hash;
// The plain-proof verifier is a local diagnostic, not an acceptance
// verifier: its noise seeds bind to proof-supplied fields the plain proof
// does not authenticate. It is compiled only for tests and the
// `diagnostics` feature so no production path can wire it into acceptance.
#[cfg(any(test, feature = "diagnostics"))]
pub mod verifier;

/// ZK integration: internal `MatmulProof` → recursive AI-PoW certificate.
/// Only compiled with the `zk` feature (pulls in `ai-pow-zk`).
#[cfg(feature = "zk")]
pub mod zk_bridge;

pub use crate::params::MatmulParams;
pub use crate::synth::synth_matrices;

// `BlockContext` remains available as `ai_pow::prover::BlockContext` for
// explicit diagnostics, tests, and miner internals. It is intentionally not
// re-exported at the crate root because it contains cached, nonce-bound matmul
// state and must not look like the normal production mining API.
//
// Plain `MatmulProof` and plain mining helpers remain available under
// `ai_pow::proof` and `ai_pow::prover` for diagnostics and for the miner's
// pre-ZKP target-hit check; the plain verifier is gated behind the
// `diagnostics` feature. They are intentionally not re-exported at the
// crate root: Nockchain's canonical production block artifact is the
// structured recursive certificate noun, not a plain opening proof.
