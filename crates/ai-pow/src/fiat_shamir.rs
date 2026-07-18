//! Fiat-Shamir transcript over BLAKE3 keyed-derive.
//!
//! Implements the Pearl §4.3 Algorithm 2-shaped commitment-hash
//! derivation chain. Nockchain production uses the same hash shape, but the
//! `H_A` / `H_B` inputs are the nonce-keyed matrix commitments bound by the
//! recursive ZK proof, not the legacy row/column opening roots:
//!
//!   κ   = derive_key("kappa",        block_state(block, nonce) ‖ params_tag)
//!   H_A = matrix_commitment(A, κ)    // ZK HASH_A / h_a_chunk
//!   H_B = matrix_commitment(B, κ)    // ZK HASH_B / h_b_chunk
//!   s_B = derive_key("s_b",          κ ‖ H_B)
//!   s_A = derive_key("s_a",          s_B ‖ H_A)
//!
//! Noise generation reads s_A (for `E = E_L · E_R`) and s_B (for `F = F_L ·
//! F_R`). Pearl's asymmetry only permits reuse while `σ` is fixed. For
//! Nockchain production, each nonce attempt changes `σ`, so the keyed
//! commitments, seeds, noise, and matmul-derived values must not be reused
//! across nonces.
//!
//! The final `pow_key = derive_key("pow-key", s_A ‖ nonce)` is the
//! keyed-BLAKE3 key for the tile-state hashes. It is domain separation on top
//! of an already nonce-bound `s_A`; it is not the sole attempt binding.

use std::collections::HashSet;

use blake3::Hasher;

const CTX_TRANSCRIPT: &str = "ai-pow v3 transcript";
const CTX_INDICES: &str = "ai-pow v3 challenge-indices";
const CTX_POW_KEY: &str = "ai-pow v3 pow-key";
const CTX_CHALLENGE: &str = "ai-pow v3 challenge-seed";
const CTX_ATTEMPT_TILE: &str = "ai-pow v3 attempt-tile";

/// Build the per-block `state` byte string fed to the prover and verifier.
pub fn block_state(block_commitment: &[u8], nonce: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + block_commitment.len() + 8 + nonce.len());
    buf.extend_from_slice(&(block_commitment.len() as u64).to_le_bytes());
    buf.extend_from_slice(block_commitment);
    buf.extend_from_slice(&(nonce.len() as u64).to_le_bytes());
    buf.extend_from_slice(nonce);
    buf
}

/// Current `κ` helper (Pearl `compute_job_key`,
/// `Pearl zk-pow ffi/mine.rs:156-161`): unkeyed BLAKE3 over the
/// concatenation of the attempt state and `params_tag`. Pearl uses
/// `header.to_bytes() || config.to_bytes()`; we accept the two parts as
/// separate slices but feed them into BLAKE3 in flat order (no length
/// prefix) to match Pearl exactly.
///
/// The caller must pass the full per-attempt state. Omitting the
/// nonce/extranonce would make downstream noise reusable and is not
/// production-sound.
pub fn commitment_key(attempt_state: &[u8], params_tag: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(attempt_state);
    hasher.update(params_tag);
    *hasher.finalize().as_bytes()
}

/// `s_B` (Pearl `compute_commitment_hash` line 4,
/// `Pearl zk-pow ffi/mine.rs:167-170`): unkeyed BLAKE3 of the 64-byte
/// concatenation `κ ‖ H_B`.
pub fn noise_seed_b(kappa: &[u8; 32], h_b: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(kappa);
    input[32..].copy_from_slice(h_b);
    *Hasher::new().update(&input).finalize().as_bytes()
}

/// `s_A` (Pearl `compute_commitment_hash` line 5,
/// `Pearl zk-pow ffi/mine.rs:172-175`): unkeyed BLAKE3 of the 64-byte
/// concatenation `s_B ‖ H_A`.
pub fn noise_seed_a(s_b: &[u8; 32], h_a: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(s_b);
    input[32..].copy_from_slice(h_a);
    *Hasher::new().update(&input).finalize().as_bytes()
}

/// Canonical production seed derivation for one nonce-bound AI-PoW attempt.
///
/// The inputs named `h_a_chunk` / `h_b_chunk` are the keyed full-matrix
/// commitments that the recursive proof exposes as `HASH_A` / `HASH_B`.
/// Deriving noise from these values prevents a prover from choosing separate
/// row/column roots as an unproved seed surface while proving different matrix
/// commitments in ZK.
pub fn canonical_noise_seeds_from_matrix_commitments(
    kappa: &[u8; 32],
    h_a_chunk: &[u8; 32],
    h_b_chunk: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    let s_b = noise_seed_b(kappa, h_b_chunk);
    let s_a = noise_seed_a(&s_b, h_a_chunk);
    (s_a, s_b)
}

/// The four MoE routing sub-hashes (Pearl
/// `zk-pow/src/api/proof_utils.rs::compute_hash_activations`). `routing_root`
/// and `hash_offsets` are keyed matrix commitments over the little-endian
/// routing / offsets bytes; `hash_routing` and `hash_activations` are unkeyed
/// 64-byte BLAKE3 concatenations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeRoutingCommitment {
    /// `MerkleTree(pad_chunk(routing_data_le), key=κ).root` — public `moe.hash_routing`.
    pub routing_root: [u8; 32],
    /// `BLAKE3(pad_chunk(routing_offsets_le), key=κ)`.
    pub hash_offsets: [u8; 32],
    /// `BLAKE3(routing_root ‖ hash_offsets)`.
    pub hash_routing: [u8; 32],
    /// `BLAKE3(H_A ‖ hash_routing)` — replaces `H_A` in the `s_A` derivation.
    pub hash_activations: [u8; 32],
}

/// `hash_routing = BLAKE3(routing_root ‖ hash_offsets)` (unkeyed, 64-byte concat).
pub fn moe_hash_routing(routing_root: &[u8; 32], hash_offsets: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(routing_root);
    input[32..].copy_from_slice(hash_offsets);
    *Hasher::new().update(&input).finalize().as_bytes()
}

/// `hash_activations = BLAKE3(H_A ‖ hash_routing)` (unkeyed, 64-byte concat).
///
/// In the dense (non-MoE) case Pearl uses `hash_activations = H_A` directly — do
/// **not** call this on a dense attempt; the dense `s_A` derivation must remain
/// byte-identical to V1 (see [`canonical_noise_seeds_from_matrix_commitments`]).
pub fn moe_hash_activations(h_a: &[u8; 32], hash_routing: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(h_a);
    input[32..].copy_from_slice(hash_routing);
    *Hasher::new().update(&input).finalize().as_bytes()
}

/// Compute the full MoE routing commitment from the canonical routing byte
/// strings (see [`crate::pearl_moe_routing::RoutingData`]) and the job key `κ`.
///
/// `routing_data_le` is the committed token-index array (LE `u32`) and
/// `routing_offsets_le` the per-expert exclusive-end offsets (LE `u32`). The
/// keyed roots reuse [`crate::commit::matrix_commitment`], which is
/// byte-equivalent to Pearl's `MatrixMerkleTree.root` / `tensor_hash` (fixture S8).
pub fn moe_routing_commitment(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> MoeRoutingCommitment {
    let routing_root = crate::commit::matrix_commitment(routing_data_le, kappa);
    let hash_offsets = crate::commit::matrix_commitment(routing_offsets_le, kappa);
    let hash_routing = moe_hash_routing(&routing_root, &hash_offsets);
    let hash_activations = moe_hash_activations(h_a, &hash_routing);
    MoeRoutingCommitment {
        routing_root,
        hash_offsets,
        hash_routing,
        hash_activations,
    }
}

/// MoE variant of [`canonical_noise_seeds_from_matrix_commitments`]: the A-side
/// seed keys off `hash_activations` instead of `H_A`, folding the routing
/// commitment into the noise seed (Pearl `commitment_hash`, MoE arm). `s_B` is
/// unchanged. Returns `(s_a, s_b, commitment)`.
pub fn canonical_noise_seeds_moe(
    kappa: &[u8; 32],
    h_a_chunk: &[u8; 32],
    h_b_chunk: &[u8; 32],
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> ([u8; 32], [u8; 32], MoeRoutingCommitment) {
    let commitment = moe_routing_commitment(kappa, h_a_chunk, routing_data_le, routing_offsets_le);
    let s_b = noise_seed_b(kappa, h_b_chunk);
    let s_a = noise_seed_a(&s_b, &commitment.hash_activations);
    (s_a, s_b, commitment)
}

/// Per-attempt `pow_key` used as the BLAKE3 key for
/// `BLAKE3(M_{i,j}, key=pow_key)`.
///
/// This function is not the only production attempt binding; callers must
/// derive `s_a` from the nonce-bound attempt state before computing `M`.
pub fn pow_key_for_nonce(s_a: &[u8; 32], nonce: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(CTX_POW_KEY);
    hasher.update(s_a);
    hasher.update(&(nonce.len() as u64).to_le_bytes());
    hasher.update(nonce);
    *hasher.finalize().as_bytes()
}

/// Challenge seed bound to the full per-attempt commitment `comm_M`. Used
/// to derive spot-check tile indices for replication verification.
pub fn challenge_seed(state: &[u8], comm_m: &[u8; 32], params_tag: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(CTX_CHALLENGE);
    hasher.update(&(state.len() as u64).to_le_bytes());
    hasher.update(state);
    hasher.update(comm_m);
    hasher.update(params_tag);
    *hasher.finalize().as_bytes()
}

/// Derive the single jackpot tile index for one nonce-bound attempt.
///
/// This removes `found_idx` as miner-selected search space and prevents
/// preselecting a tile from `(block, nonce, params)` alone: the eligible tile is
/// sampled only after the nonce-bound matrix commitments have fixed `s_a`.
/// Spot-check indices remain derived from `challenge_seed`, which additionally
/// binds the full `comm_m` tree.
pub fn attempt_tile_index(
    state: &[u8],
    params_tag: &[u8; 32],
    s_a: &[u8; 32],
    num_tiles: u64,
) -> u64 {
    assert!(num_tiles > 0, "num_tiles must be > 0");
    let mut hasher = Hasher::new_derive_key(CTX_ATTEMPT_TILE);
    hasher.update(&(state.len() as u64).to_le_bytes());
    hasher.update(state);
    hasher.update(params_tag);
    hasher.update(s_a);
    let seed = *hasher.finalize().as_bytes();
    challenge_indices(&seed, 1, num_tiles)[0]
}

/// Generic transcript hash: returns 32 bytes for an arbitrary list of byte
/// strings, length-prefixed individually.
pub fn transcript(label: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(CTX_TRANSCRIPT);
    hasher.update(&(label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

/// Derive `count` distinct indices in `0..range` from `seed`. Sampling is
/// without-replacement over a streamed XOF with rejection-and-set.
/// Determinism: same `(seed, count, range)` always yields the same vector.
pub fn challenge_indices(seed: &[u8; 32], count: u32, range: u64) -> Vec<u64> {
    assert!(range > 0, "range must be > 0");
    assert!(u64::from(count) <= range, "count must be <= range");
    let mut hasher = Hasher::new_derive_key(CTX_INDICES);
    hasher.update(seed);
    hasher.update(&count.to_le_bytes());
    hasher.update(&range.to_le_bytes());
    let mut xof = hasher.finalize_xof();

    // H2 (DoS audit): the prior implementation allocated
    // `vec![false; range as usize]` — `range = num_tiles`, which (post
    // H1) is bounded only by `u32::MAX`. A crafted call with a huge
    // range would burn up to ~4 GiB. A `HashSet` tracks "already-taken"
    // indices in `O(count)` memory regardless of `range`.
    let mut chosen: Vec<u64> = Vec::with_capacity(count as usize);
    let mut taken: HashSet<u64> = HashSet::with_capacity(count as usize);
    let mut buf = [0u8; 8];
    while chosen.len() < count as usize {
        xof.fill(&mut buf);
        let r = u64::from_le_bytes(buf);
        let idx = r % range;
        if taken.insert(idx) {
            chosen.push(idx);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    // MoE routing-commitment splice.

    /// The composed helpers reproduce the exact Pearl `compute_hash_activations`
    /// chain, verified against an independent inline recomputation (blake3
    /// directly), so a wiring bug in the helpers cannot hide.
    #[test]
    fn moe_routing_commitment_matches_inline_recomputation() {
        let kappa = [0x11u8; 32];
        let h_a = [0x22u8; 32];
        let routing_data_le: Vec<u8> = (0u32..40).flat_map(|v| v.to_le_bytes()).collect();
        let routing_offsets_le: Vec<u8> = [10u32, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        let c = moe_routing_commitment(&kappa, &h_a, &routing_data_le, &routing_offsets_le);

        // Independent recomputation of the Pearl chain.
        let routing_root = crate::commit::matrix_commitment(&routing_data_le, &kappa);
        let hash_offsets = crate::commit::matrix_commitment(&routing_offsets_le, &kappa);
        let hash_routing = {
            let mut i = Vec::new();
            i.extend_from_slice(&routing_root);
            i.extend_from_slice(&hash_offsets);
            *Hasher::new().update(&i).finalize().as_bytes()
        };
        let hash_activations = {
            let mut i = Vec::new();
            i.extend_from_slice(&h_a);
            i.extend_from_slice(&hash_routing);
            *Hasher::new().update(&i).finalize().as_bytes()
        };
        assert_eq!(c.routing_root, routing_root);
        assert_eq!(c.hash_offsets, hash_offsets);
        assert_eq!(c.hash_routing, hash_routing);
        assert_eq!(c.hash_activations, hash_activations);
    }

    /// `routing_root` and `hash_offsets` are exactly `matrix_commitment` (Pearl
    /// `MatrixMerkleTree.root` / `tensor_hash`, fixture-S8-equivalent).
    #[test]
    fn moe_roots_are_matrix_commitments() {
        let kappa = [7u8; 32];
        let h_a = [9u8; 32];
        let rd: Vec<u8> = (0u32..16).flat_map(|v| v.to_le_bytes()).collect();
        let ro: Vec<u8> = [16u32].iter().flat_map(|v| v.to_le_bytes()).collect();
        let c = moe_routing_commitment(&kappa, &h_a, &rd, &ro);
        assert_eq!(
            c.routing_root,
            crate::commit::matrix_commitment(&rd, &kappa)
        );
        assert_eq!(
            c.hash_offsets,
            crate::commit::matrix_commitment(&ro, &kappa)
        );
    }

    /// **Real Pearl KAT** — our `hash_offsets` (keyed matrix commitment of the
    /// LE routing offsets) + `hash_routing` + `hash_activations` composition must
    /// equal Pearl `zk_pow::api::proof_utils::compute_hash_activations` for a
    /// fixed input. Vector emitted from the Pearl `zk-pow` crate (2026-07-07):
    /// `hash_a=[0x22;32]`, `routing_root=[0x33;32]`, `routing_offsets=[10,20,30,40]`,
    /// `job_key=[0x11;32]`. This anchors B2 to Pearl's actual function, not just
    /// its documented formula.
    #[test]
    fn moe_hash_activations_matches_pearl_kat() {
        let hash_a = [0x22u8; 32];
        let routing_root = [0x33u8; 32];
        let job_key = [0x11u8; 32];
        let routing_offsets_le: Vec<u8> = [10u32, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let hash_offsets = crate::commit::matrix_commitment(&routing_offsets_le, &job_key);
        let hash_routing = moe_hash_routing(&routing_root, &hash_offsets);
        let hash_activations = moe_hash_activations(&hash_a, &hash_routing);
        let pearl: [u8; 32] = [
            124, 127, 230, 159, 165, 18, 134, 171, 128, 119, 180, 146, 8, 204, 18, 208, 114, 21,
            147, 95, 107, 11, 231, 120, 157, 77, 25, 124, 22, 30, 51, 102,
        ];
        assert_eq!(
            hash_activations, pearl,
            "hash_activations must match Pearl compute_hash_activations"
        );
    }

    /// **§E full-chain byte-compat (adversarial audit).** Independently
    /// re-implements Pearl's `compute_commitment_hash_with_offsets`
    /// (`zk-pow/src/ffi/mine.rs`) + `compute_hash_activations`
    /// (`proof_utils.rs`) byte-for-byte and asserts our `canonical_noise_seeds_moe`
    /// produces the identical `s_A` for a concrete routing built by
    /// `build_routing_data`. This binds the WHOLE splice chain (routing_data →
    /// routing_root → hash_offsets → hash_routing → hash_activations → s_b → s_a)
    /// to Pearl's exact formula. The real-Pearl KAT above anchors
    /// `hash_activations` to Pearl's actual output bytes; together they close the
    /// merge-mining fork risk for the MoE commitment. Pearl reference (verbatim):
    ///   hash_routing_data = blake3(pad_to_chunk_boundary(flatten_routing), key=job_key)
    ///   hash_offsets      = blake3(pad_to_chunk_boundary(offsets_le),      key=job_key)
    ///   hash_routing      = blake3(hash_routing_data ‖ hash_offsets)        (unkeyed)
    ///   hash_activations  = blake3(hash_a ‖ hash_routing)                   (unkeyed)
    ///   b_noise_seed      = blake3(job_key ‖ hash_b)                        (unkeyed)
    ///   a_noise_seed      = blake3(b_noise_seed ‖ hash_activations)         (unkeyed)
    #[test]
    fn full_moe_s_a_chain_matches_pearl_formula() {
        let kappa = [0x11u8; 32]; // job_key
        let hash_a = [0x22u8; 32];
        let hash_b = [0x44u8; 32];
        // m=5, top_k=2, e=4 — includes an empty expert and a token routed twice
        // to the same expert (slots 0,1 → expert 3), exercising the grouping.
        let topk = [3u32, 3, 0, 1, 3, 0, 2, 2, 1, 0];
        let rd = crate::pearl_moe_routing::build_routing_data(&topk, 5, 2, 4).unwrap();
        let routing_data_le: Vec<u8> = rd
            .routing_data
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let routing_offsets_le: Vec<u8> = rd
            .routing_offsets
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        // ---- Pearl reference, re-implemented from mine.rs + proof_utils.rs ----
        let unkeyed = |parts: &[&[u8]]| -> [u8; 32] {
            let mut h = Hasher::new();
            for p in parts {
                h.update(p);
            }
            *h.finalize().as_bytes()
        };
        let keyed_padded = |key: &[u8; 32], data: &[u8]| -> [u8; 32] {
            let padded = crate::commit::pad_to_chunk_boundary(data);
            *Hasher::new_keyed(key).update(&padded).finalize().as_bytes()
        };
        let routing_root = keyed_padded(&kappa, &routing_data_le);
        let hash_offsets = keyed_padded(&kappa, &routing_offsets_le);
        let hash_routing = unkeyed(&[&routing_root, &hash_offsets]);
        let hash_activations = unkeyed(&[&hash_a, &hash_routing]);
        let s_b_ref = unkeyed(&[&kappa, &hash_b]);
        let s_a_ref = unkeyed(&[&s_b_ref, &hash_activations]);

        // ---- Our implementation ----
        let (s_a, s_b, c) = canonical_noise_seeds_moe(
            &kappa, &hash_a, &hash_b, &routing_data_le, &routing_offsets_le,
        );
        assert_eq!(c.routing_root, routing_root, "routing_root");
        assert_eq!(c.hash_offsets, hash_offsets, "hash_offsets");
        assert_eq!(c.hash_activations, hash_activations, "hash_activations");
        assert_eq!(s_b, s_b_ref, "s_b (b_noise_seed)");
        assert_eq!(s_a, s_a_ref, "s_a (a_noise_seed) — full Pearl splice chain");
    }

    /// Cross-crate byte-equivalence: `ai-pow-zk`'s off-circuit MoE reference
    /// (`moe_ref`, the spec the B5 sub-AIR reproduces) equals this crate's MoE
    /// splice. Transitively Pearl-validated via the KAT above (this crate) and
    /// fixture S8 (`matrix_commitment`). `ai-pow-zk` cannot depend on `ai-pow`,
    /// so the equivalence is asserted here (`--features zk`).
    #[cfg(feature = "zk")]
    #[test]
    fn moe_ref_byte_equivalent_to_fiat_shamir_splice() {
        let job_key = [0x11u8; 32];
        let h_a = [0x22u8; 32];
        let rd: Vec<u8> = (0u32..40).flat_map(|v| v.to_le_bytes()).collect();
        let ro: Vec<u8> = [10u32, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ours = moe_routing_commitment(&job_key, &h_a, &rd, &ro);
        let zk = ai_pow_zk::moe_ref::moe_commitment(&job_key, &h_a, &rd, &ro);
        assert_eq!(ours.routing_root, zk.routing_root);
        assert_eq!(ours.hash_offsets, zk.hash_offsets);
        assert_eq!(ours.hash_routing, zk.hash_routing);
        assert_eq!(ours.hash_activations, zk.hash_activations);
    }

    /// The MoE seed derivation folds routing in: `s_A` differs from the dense
    /// `s_A` (which keys off `H_A`), while `s_B` is unchanged. This is the
    /// defining effect of the splice.
    #[test]
    fn moe_seed_folds_routing_and_leaves_s_b_unchanged() {
        let kappa = [1u8; 32];
        let h_a = [2u8; 32];
        let h_b = [3u8; 32];
        let rd: Vec<u8> = (0u32..32).flat_map(|v| v.to_le_bytes()).collect();
        let ro: Vec<u8> = [8u32, 16, 24, 32]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        let (dense_s_a, dense_s_b) =
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b);
        let (moe_s_a, moe_s_b, c) = canonical_noise_seeds_moe(&kappa, &h_a, &h_b, &rd, &ro);

        assert_eq!(moe_s_b, dense_s_b, "s_B must be unchanged by MoE");
        assert_ne!(
            moe_s_a, dense_s_a,
            "s_A must fold in the routing commitment"
        );
        // s_A is exactly noise_seed_a(s_B, hash_activations).
        assert_eq!(moe_s_a, noise_seed_a(&moe_s_b, &c.hash_activations));
    }

    /// Dense-path guardrail: the existing dense seed derivation is untouched by
    /// B2 (no routing enters), so dense byte-parity (S0–S9) is preserved.
    #[test]
    fn dense_seed_derivation_is_unaffected() {
        let kappa = [0x5au8; 32];
        let h_a = [0xa5u8; 32];
        let h_b = [0x3cu8; 32];
        // Recompute the dense chain by hand and compare.
        let s_b = noise_seed_b(&kappa, &h_b);
        let s_a = noise_seed_a(&s_b, &h_a);
        assert_eq!(
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b),
            (s_a, s_b)
        );
    }

    /// Every routing input perturbation changes `hash_activations` (and thus
    /// `s_A`): routing data, offsets, and the job key each bind.
    #[test]
    fn every_routing_input_binds() {
        let kappa = [1u8; 32];
        let h_a = [2u8; 32];
        let rd: Vec<u8> = (0u32..24).flat_map(|v| v.to_le_bytes()).collect();
        let ro: Vec<u8> = [12u32, 24].iter().flat_map(|v| v.to_le_bytes()).collect();
        let base = moe_routing_commitment(&kappa, &h_a, &rd, &ro).hash_activations;

        let mut rd2 = rd.clone();
        rd2[0] ^= 1;
        assert_ne!(
            base,
            moe_routing_commitment(&kappa, &h_a, &rd2, &ro).hash_activations,
            "routing data must bind"
        );
        let mut ro2 = ro.clone();
        ro2[0] ^= 1;
        assert_ne!(
            base,
            moe_routing_commitment(&kappa, &h_a, &rd, &ro2).hash_activations,
            "routing offsets must bind"
        );
        let mut kappa2 = kappa;
        kappa2[0] ^= 1;
        assert_ne!(
            base,
            moe_routing_commitment(&kappa2, &h_a, &rd, &ro).hash_activations,
            "job key must bind"
        );
        let mut h_a2 = h_a;
        h_a2[0] ^= 1;
        assert_ne!(
            base,
            moe_routing_commitment(&kappa, &h_a2, &rd, &ro).hash_activations,
            "H_A must bind"
        );
    }

    #[test]
    fn block_state_round_trip_is_unambiguous() {
        let s1 = block_state(b"abc", b"de");
        let s2 = block_state(b"ab", b"cde");
        assert_ne!(s1, s2, "length-prefixing must disambiguate concatenations");
    }

    #[test]
    fn commitment_key_binds_attempt_state() {
        // κ depends on the full attempt state + params_tag. Production callers
        // construct that attempt state with block_state(block_commitment, nonce).
        let tag = [9u8; 32];
        let k1 = commitment_key(b"hdr", &tag);
        let k2 = commitment_key(b"hdr", &tag);
        assert_eq!(k1, k2);
        assert_ne!(commitment_key(b"hdr2", &tag), k1);
        assert_ne!(commitment_key(b"hdr", &[10u8; 32]), k1);
    }

    #[test]
    fn pearl_derivation_chain_binds_all_inputs() {
        // s_A must differ when *any* of (attempt state, params, h_a, h_b) differs.
        let kappa = commitment_key(b"hdr", &[1u8; 32]);
        let h_a = [2u8; 32];
        let h_b = [3u8; 32];
        let s_b = noise_seed_b(&kappa, &h_b);
        let s_a = noise_seed_a(&s_b, &h_a);

        let kappa2 = commitment_key(b"hdr-other", &[1u8; 32]);
        let s_b2 = noise_seed_b(&kappa2, &h_b);
        let s_a2 = noise_seed_a(&s_b2, &h_a);
        assert_ne!(s_a, s_a2, "changing attempt state must change s_A");

        let s_a3 = noise_seed_a(&noise_seed_b(&kappa, &[7u8; 32]), &h_a);
        assert_ne!(s_a, s_a3, "changing h_b must change s_A");

        let s_a4 = noise_seed_a(&s_b, &[8u8; 32]);
        assert_ne!(s_a, s_a4, "changing h_a must change s_A");
    }

    #[test]
    fn pow_key_changes_with_nonce_but_not_with_unrelated_inputs() {
        let s_a = [4u8; 32];
        let k1 = pow_key_for_nonce(&s_a, b"nce-1");
        let k2 = pow_key_for_nonce(&s_a, b"nce-2");
        assert_ne!(k1, k2);
        assert_eq!(k1, pow_key_for_nonce(&s_a, b"nce-1"));
        assert_ne!(pow_key_for_nonce(&[5u8; 32], b"nce-1"), k1);
    }

    #[test]
    fn pow_key_separate_from_seeds() {
        // Domain contexts must keep pow_key distinct from the seed values
        // it's derived from.
        let s_a = [4u8; 32];
        assert_ne!(pow_key_for_nonce(&s_a, b""), s_a);
        assert_ne!(
            pow_key_for_nonce(&s_a, b"nce"),
            noise_seed_a(&[1u8; 32], &[2u8; 32])
        );
    }

    #[test]
    fn attempt_tile_index_is_deterministic_bounded_and_attempt_bound() {
        let tag = [9u8; 32];
        let s_a = [11u8; 32];
        let idx = attempt_tile_index(b"attempt-a", &tag, &s_a, 17);
        assert!(idx < 17);
        assert_eq!(idx, attempt_tile_index(b"attempt-a", &tag, &s_a, 17));
        assert_ne!(idx, attempt_tile_index(b"attempt-b", &tag, &s_a, 17));
        assert_ne!(idx, attempt_tile_index(b"attempt-a", &[10u8; 32], &s_a, 17));
        assert_ne!(idx, attempt_tile_index(b"attempt-a", &tag, &[12u8; 32], 17));
    }

    #[test]
    fn indices_unique_and_in_range() {
        let seed = [1u8; 32];
        let idx = challenge_indices(&seed, 16, 64);
        assert_eq!(idx.len(), 16);
        for &i in &idx {
            assert!(i < 64);
        }
        let mut sorted = idx.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 16);
    }

    #[test]
    fn indices_deterministic_and_seed_sensitive() {
        let s1 = [1u8; 32];
        let s2 = [2u8; 32];
        assert_eq!(
            challenge_indices(&s1, 16, 64),
            challenge_indices(&s1, 16, 64)
        );
        assert_ne!(
            challenge_indices(&s1, 16, 64),
            challenge_indices(&s2, 16, 64)
        );
    }

    #[test]
    fn transcript_determinism_and_label_separation() {
        let parts = [&b"hello"[..], &b"world"[..]];
        assert_eq!(transcript("a", &parts), transcript("a", &parts));
        assert_ne!(transcript("a", &parts), transcript("b", &parts));
    }
}
