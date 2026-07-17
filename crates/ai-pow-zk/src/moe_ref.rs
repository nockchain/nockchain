//! Track B5 (de-risk) — off-circuit MoE routing-commitment reference.
//!
//! The **canonical spec** the in-circuit MoE sub-AIR (B5a) must reproduce:
//! Pearl `zk-pow/src/api/proof_utils.rs::compute_hash_activations`, which folds
//! the routing commitment into A's noise seed:
//!
//! ```text
//! routing_root     = BLAKE3(pad_chunk(routing_data_le),    key = job_key)   // = MatrixMerkleTree.root
//! hash_offsets     = BLAKE3(pad_chunk(routing_offsets_le), key = job_key)   // = tensor_hash
//! hash_routing     = BLAKE3(routing_root ‖ hash_offsets)
//! hash_activations = BLAKE3(H_A ‖ hash_routing)                              // replaces H_A in s_A
//! ```
//!
//! Bit-for-bit mirror of `ai-pow::fiat_shamir`'s MoE splice
//! (`moe_routing_commitment`). `ai-pow-zk` must not depend on `ai-pow` (dep
//! cycle), so this re-derives from the `blake3` primitive and
//! [`crate::blake3_tree`]; the decisive cross-crate byte-equivalence KAT
//! (`moe_ref` == `ai-pow` MoE splice) lives on the `ai-pow` side (it may depend
//! on `ai-pow-zk`, `--features zk`). Pure / off-circuit — de-risks the spec
//! before the sub-AIR (the codebase's KAT-first discipline, cf. `noise_ref`).

use blake3::Hasher;

use crate::blake3_tree::pad_to_chunk_boundary;

/// Keyed matrix commitment `BLAKE3(pad_to_chunk(bytes), key)` — Pearl
/// `MatrixMerkleTree.root` / `tensor_hash`, byte-equivalent to
/// `ai-pow::commit::matrix_commitment` (fixture S8).
pub fn keyed_matrix_commitment(bytes: &[u8], key: &[u8; 32]) -> [u8; 32] {
    let padded = pad_to_chunk_boundary(bytes);
    *Hasher::new_keyed(key).update(&padded).finalize().as_bytes()
}

/// Unkeyed BLAKE3 of two concatenated 32-byte digests.
fn blake3_concat(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(a);
    input[32..].copy_from_slice(b);
    *Hasher::new().update(&input).finalize().as_bytes()
}

/// `hash_routing = BLAKE3(routing_root ‖ hash_offsets)`.
pub fn combine_routing(routing_root: &[u8; 32], hash_offsets: &[u8; 32]) -> [u8; 32] {
    blake3_concat(routing_root, hash_offsets)
}

/// `hash_activations = BLAKE3(H_A ‖ hash_routing)`.
pub fn combine_activations(h_a: &[u8; 32], hash_routing: &[u8; 32]) -> [u8; 32] {
    blake3_concat(h_a, hash_routing)
}

/// The four MoE routing sub-hashes (mirrors
/// `ai-pow::fiat_shamir::MoeRoutingCommitment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeCommitment {
    pub routing_root: [u8; 32],
    pub hash_offsets: [u8; 32],
    pub hash_routing: [u8; 32],
    pub hash_activations: [u8; 32],
}

/// Full MoE routing commitment from the canonical routing byte strings (LE `u32`
/// `routing_data` / `routing_offsets`; see
/// `ai-pow::pearl_moe_routing::RoutingData`) and the job key.
pub fn moe_commitment(
    job_key: &[u8; 32],
    h_a: &[u8; 32],
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> MoeCommitment {
    let routing_root = keyed_matrix_commitment(routing_data_le, job_key);
    let hash_offsets = keyed_matrix_commitment(routing_offsets_le, job_key);
    let hash_routing = combine_routing(&routing_root, &hash_offsets);
    let hash_activations = combine_activations(h_a, &hash_routing);
    MoeCommitment {
        routing_root,
        hash_offsets,
        hash_routing,
        hash_activations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Self-consistency: the composed helper matches an inline recomputation, and
    /// each input binds. (The cross-crate byte-equivalence to `ai-pow` lives in
    /// `ai-pow`'s `fiat_shamir` tests under `--features zk`.)
    #[test]
    fn moe_commitment_composes_and_binds() {
        let job_key = [0x11u8; 32];
        let h_a = [0x22u8; 32];
        let rd: Vec<u8> = (0u32..40).flat_map(|v| v.to_le_bytes()).collect();
        let ro: Vec<u8> = [10u32, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let c = moe_commitment(&job_key, &h_a, &rd, &ro);

        let rr = keyed_matrix_commitment(&rd, &job_key);
        let ho = keyed_matrix_commitment(&ro, &job_key);
        assert_eq!(c.routing_root, rr);
        assert_eq!(c.hash_offsets, ho);
        assert_eq!(c.hash_routing, combine_routing(&rr, &ho));
        assert_eq!(
            c.hash_activations,
            combine_activations(&h_a, &c.hash_routing)
        );

        // Each input binds hash_activations.
        let base = c.hash_activations;
        let mut rd2 = rd.clone();
        rd2[0] ^= 1;
        assert_ne!(
            base,
            moe_commitment(&job_key, &h_a, &rd2, &ro).hash_activations
        );
        let mut ro2 = ro.clone();
        ro2[0] ^= 1;
        assert_ne!(
            base,
            moe_commitment(&job_key, &h_a, &rd, &ro2).hash_activations
        );
        let mut jk2 = job_key;
        jk2[0] ^= 1;
        assert_ne!(base, moe_commitment(&jk2, &h_a, &rd, &ro).hash_activations);
        let mut ha2 = h_a;
        ha2[0] ^= 1;
        assert_ne!(
            base,
            moe_commitment(&job_key, &ha2, &rd, &ro).hash_activations
        );
    }
}
