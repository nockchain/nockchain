//! Fiat-Shamir transcript over BLAKE3 keyed-derive.
//!
//! Implements the Pearl certificate-V3 noise-seed derivation over nonce-keyed
//! matrix commitments:
//!
//!   κ       = BLAKE3(σ ‖ μ)
//!   H_A/H_B = matrix_commitment(A/B, κ)       // ZK HASH_A / HASH_B
//!   A'      = BLAKE3(H_A ‖ LE(m)   ‖ 0^224, key=SEED_SALT_A)
//!   B'      = BLAKE3(H_B ‖ LE(n/n_e) ‖ 0^224, key=SEED_SALT_B)
//!   s_B     = BLAKE3(κ ‖ B')
//!   s_A     = BLAKE3(s_B ‖ A')               // dense
//!
//! MoE replaces the dense A-side input with
//! `BLAKE3(A' ‖ BLAKE3(routing_root ‖ hash_offsets))`. The routing root and
//! offsets are keyed matrix commitments under `κ`. Certificate-V3 binds matrix
//! dimensions before either seed, so every distinct dense and MoE statement has
//! a distinct seed domain.
//!
//! Noise generation reads s_A (for `E = E_L · E_R`) and s_B (for `F = F_L ·
//! F_R`). Each nonce attempt changes `σ`, so the keyed commitments, seeds,
//! noise, and matmul-derived values must not be reused across nonces.
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

const PEARL_V3_SEED_SALT_A: [u8; 32] = [
    0x82, 0x49, 0x40, 0x6c, 0xa0, 0xed, 0x15, 0x16, 0x96, 0x16, 0xf6, 0x92, 0xfc, 0xf0, 0x76, 0xf8,
    0x92, 0xdb, 0xdb, 0x2a, 0x70, 0x23, 0xb8, 0x52, 0xf0, 0xd4, 0x77, 0x19, 0xc3, 0x90, 0x01, 0x7b,
];
const PEARL_V3_SEED_SALT_B: [u8; 32] = [
    0x11, 0x30, 0x06, 0x32, 0xec, 0x63, 0x01, 0xca, 0x2b, 0xe2, 0xaf, 0x71, 0x8b, 0x3f, 0x4d, 0x4f,
    0x1a, 0xe9, 0xc6, 0x39, 0x88, 0xe8, 0xcc, 0x04, 0x48, 0x44, 0x30, 0x1d, 0x71, 0xb8, 0x9a, 0xa9,
];

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

fn bind_root(salt: &[u8; 32], root: &[u8; 32], dimension: u32) -> [u8; 32] {
    let mut message = [0u8; 64];
    message[..32].copy_from_slice(root);
    message[32..36].copy_from_slice(&dimension.to_le_bytes());
    *Hasher::new_keyed(salt)
        .update(&message)
        .finalize()
        .as_bytes()
}

/// Canonical V3 dense seeds for raw authenticated matrix commitments.
pub fn canonical_noise_seeds_from_matrix_commitments(
    kappa: &[u8; 32],
    h_a_chunk: &[u8; 32],
    h_b_chunk: &[u8; 32],
    m: u32,
    n: u32,
) -> ([u8; 32], [u8; 32]) {
    let a_bound = bind_root(&PEARL_V3_SEED_SALT_A, h_a_chunk, m);
    let b_bound = bind_root(&PEARL_V3_SEED_SALT_B, h_b_chunk, n);
    let s_b = hash_pair(kappa, &b_bound);
    (hash_pair(&s_b, &a_bound), s_b)
}

/// The V3 MoE routing commitment derived from canonical private routing bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeRoutingCommitment {
    /// `MerkleTree(pad_chunk(routing_data_le), key=κ).root`.
    pub routing_root: [u8; 32],
    /// `BLAKE3(pad_chunk(routing_offsets_le), key=κ)`.
    pub hash_offsets: [u8; 32],
    /// `BLAKE3(routing_root ‖ hash_offsets)`.
    pub hash_routing: [u8; 32],
    /// `BLAKE3(A' ‖ hash_routing)`.
    pub hash_activations: [u8; 32],
}

fn routing_commitment_from_public_root(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    m: u32,
    routing_root: &[u8; 32],
    routing_offsets_le: &[u8],
) -> MoeRoutingCommitment {
    let hash_offsets = crate::commit::matrix_commitment(routing_offsets_le, kappa);
    let hash_routing = hash_pair(routing_root, &hash_offsets);
    let a_bound = bind_root(&PEARL_V3_SEED_SALT_A, h_a, m);
    let hash_activations = hash_pair(&a_bound, &hash_routing);
    MoeRoutingCommitment {
        routing_root: *routing_root,
        hash_offsets,
        hash_routing,
        hash_activations,
    }
}

/// Compute the V3 MoE routing commitment from canonical private routing bytes.
pub fn moe_routing_commitment(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    m: u32,
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> MoeRoutingCommitment {
    let routing_root = crate::commit::matrix_commitment(routing_data_le, kappa);
    routing_commitment_from_public_root(kappa, h_a, m, &routing_root, routing_offsets_le)
}

fn moe_seeds_from_commitment(
    kappa: &[u8; 32],
    h_b: &[u8; 32],
    n_e: u32,
    commitment: &MoeRoutingCommitment,
) -> ([u8; 32], [u8; 32]) {
    let b_bound = bind_root(&PEARL_V3_SEED_SALT_B, h_b, n_e);
    let s_b = hash_pair(kappa, &b_bound);
    (hash_pair(&s_b, &commitment.hash_activations), s_b)
}

/// Canonical V3 MoE seeds for canonical private routing bytes.
#[allow(clippy::too_many_arguments)]
pub fn canonical_noise_seeds_moe(
    kappa: &[u8; 32],
    h_a_chunk: &[u8; 32],
    h_b_chunk: &[u8; 32],
    m: u32,
    n_e: u32,
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> ([u8; 32], [u8; 32], MoeRoutingCommitment) {
    let commitment =
        moe_routing_commitment(kappa, h_a_chunk, m, routing_data_le, routing_offsets_le);
    let (s_a, s_b) = moe_seeds_from_commitment(kappa, h_b_chunk, n_e, &commitment);
    (s_a, s_b, commitment)
}

/// Canonical V3 MoE seeds for a verified public routing root.
#[allow(clippy::too_many_arguments)]
pub fn canonical_noise_seeds_moe_from_public_routing(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    h_b: &[u8; 32],
    m: u32,
    n_e: u32,
    routing_root: &[u8; 32],
    routing_offsets_le: &[u8],
) -> ([u8; 32], [u8; 32]) {
    let commitment =
        routing_commitment_from_public_root(kappa, h_a, m, routing_root, routing_offsets_le);
    moe_seeds_from_commitment(kappa, h_b, n_e, &commitment)
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

    // Tracks taken indices in a `HashSet`: `O(count)` memory
    // regardless of `range` (= `num_tiles`, bounded only by
    // `u32::MAX`) — a `vec![false; range]` taken-flags array would
    // burn up to ~4 GiB on a crafted call.
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

    #[test]
    fn dense_v3_seeds_bind_roots_and_dimensions() {
        let kappa = [0x11u8; 32];
        let h_a = [0xaau8; 32];
        let h_b = [0xbbu8; 32];
        let base = canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b, 192, 320);

        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b, 193, 320)
        );
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b, 192, 321)
        );
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &[0xabu8; 32], &h_b, 192, 320)
        );
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &[0xbcu8; 32], 192, 320)
        );
    }

    #[test]
    fn moe_v3_seeds_bind_dimensions_and_routing() {
        let kappa = [0x11u8; 32];
        let h_a = [0xaau8; 32];
        let h_b = [0xbbu8; 32];
        let routing_data: Vec<u8> = (0u32..40).flat_map(u32::to_le_bytes).collect();
        let routing_offsets: Vec<u8> = [10u32, 20, 30, 40]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let (s_a, s_b, commitment) =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 192, 64, &routing_data, &routing_offsets);
        assert_eq!(
            (s_a, s_b),
            canonical_noise_seeds_moe_from_public_routing(
                &kappa, &h_a, &h_b, 192, 64, &commitment.routing_root, &routing_offsets,
            )
        );
        let changed_m =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 193, 64, &routing_data, &routing_offsets);
        assert_ne!((s_a, s_b), (changed_m.0, changed_m.1));
        let changed_n_e =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 192, 65, &routing_data, &routing_offsets);
        assert_ne!((s_a, s_b), (changed_n_e.0, changed_n_e.1));
        let stacked_width = canonical_noise_seeds_moe(
            &kappa, &h_a, &h_b, 192, 512, &routing_data, &routing_offsets,
        );
        assert_ne!((s_a, s_b), (stacked_width.0, stacked_width.1));
        let mut changed_data = routing_data.clone();
        changed_data[..8].reverse();
        let reordered =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 192, 64, &changed_data, &routing_offsets);
        assert_ne!((s_a, s_b), (reordered.0, reordered.1));
        let mut changed_offsets = routing_offsets.clone();
        changed_offsets[0] ^= 1;
        let altered_offsets =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 192, 64, &routing_data, &changed_offsets);
        assert_ne!((s_a, s_b), (altered_offsets.0, altered_offsets.1));
        let mut changed_root = commitment.routing_root;
        changed_root[0] ^= 1;
        assert_ne!(
            (s_a, s_b),
            canonical_noise_seeds_moe_from_public_routing(
                &kappa, &h_a, &h_b, 192, 64, &changed_root, &routing_offsets,
            )
        );
    }

    #[cfg(feature = "zk")]
    #[test]
    fn moe_ref_matches_complete_v3_transcript() {
        let kappa = [0x11u8; 32];
        let h_a = [0x22u8; 32];
        let h_b = [0x33u8; 32];
        let routing_data: Vec<u8> = (0u32..40).flat_map(u32::to_le_bytes).collect();
        let routing_offsets: Vec<u8> = [10u32, 20, 30, 40]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let (s_a, s_b, commitment) =
            canonical_noise_seeds_moe(&kappa, &h_a, &h_b, 192, 64, &routing_data, &routing_offsets);
        let reference = ai_pow_zk::moe_ref::moe_v3_reference(
            &kappa, &h_a, &h_b, 192, 64, &routing_data, &routing_offsets,
        );
        assert_eq!(commitment.routing_root, reference.commitment.routing_root);
        assert_eq!(commitment.hash_offsets, reference.commitment.hash_offsets);
        assert_eq!(commitment.hash_routing, reference.commitment.hash_routing);
        assert_eq!(
            commitment.hash_activations,
            reference.commitment.hash_activations
        );
        assert_eq!(s_a, reference.s_a);
        assert_eq!(s_b, reference.s_b);
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
    fn pearl_v3_derivation_chain_binds_all_inputs() {
        let kappa = commitment_key(b"hdr", &[1u8; 32]);
        let h_a = [2u8; 32];
        let h_b = [3u8; 32];
        let base = canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &h_b, 192, 320);

        let kappa2 = commitment_key(b"hdr-other", &[1u8; 32]);
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa2, &h_a, &h_b, 192, 320)
        );
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &h_a, &[7u8; 32], 192, 320)
        );
        assert_ne!(
            base,
            canonical_noise_seeds_from_matrix_commitments(&kappa, &[8u8; 32], &h_b, 192, 320)
        );
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
        let seed = canonical_noise_seeds_from_matrix_commitments(
            &[1u8; 32], &[2u8; 32], &[3u8; 32], 192, 320,
        )
        .0;
        assert_ne!(pow_key_for_nonce(&s_a, b"nce"), seed);
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
