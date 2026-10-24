//! Independent Pearl V3 MoE transcript reference.
//!
//! The reference receives raw authenticated roots and dimensions. It binds A
//! before the routing fold and binds B with the per-expert width before the
//! seed chain. `ai-pow-zk` has no dependency on `ai-pow`, so cross-crate tests
//! compare the complete reference result with the production implementation.

use blake3::Hasher;

use crate::blake3_tree::pad_to_chunk_boundary;

const SEED_SALT_A: [u8; 32] = [
    0x82, 0x49, 0x40, 0x6c, 0xa0, 0xed, 0x15, 0x16, 0x96, 0x16, 0xf6, 0x92, 0xfc, 0xf0, 0x76, 0xf8,
    0x92, 0xdb, 0xdb, 0x2a, 0x70, 0x23, 0xb8, 0x52, 0xf0, 0xd4, 0x77, 0x19, 0xc3, 0x90, 0x01, 0x7b,
];
const SEED_SALT_B: [u8; 32] = [
    0x11, 0x30, 0x06, 0x32, 0xec, 0x63, 0x01, 0xca, 0x2b, 0xe2, 0xaf, 0x71, 0x8b, 0x3f, 0x4d, 0x4f,
    0x1a, 0xe9, 0xc6, 0x39, 0x88, 0xe8, 0xcc, 0x04, 0x48, 0x44, 0x30, 0x1d, 0x71, 0xb8, 0x9a, 0xa9,
];

fn keyed_matrix_commitment(bytes: &[u8], key: &[u8; 32]) -> [u8; 32] {
    let padded = pad_to_chunk_boundary(bytes);
    *Hasher::new_keyed(key).update(&padded).finalize().as_bytes()
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeCommitment {
    pub routing_root: [u8; 32],
    pub hash_offsets: [u8; 32],
    pub hash_routing: [u8; 32],
    pub hash_activations: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeV3Reference {
    pub commitment: MoeCommitment,
    pub s_a: [u8; 32],
    pub s_b: [u8; 32],
}

#[allow(clippy::too_many_arguments)]
pub fn moe_v3_reference(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    h_b: &[u8; 32],
    m: u32,
    n_e: u32,
    routing_data_le: &[u8],
    routing_offsets_le: &[u8],
) -> MoeV3Reference {
    let routing_root = keyed_matrix_commitment(routing_data_le, kappa);
    let hash_offsets = keyed_matrix_commitment(routing_offsets_le, kappa);
    let hash_routing = hash_pair(&routing_root, &hash_offsets);
    let a_bound = bind_root(&SEED_SALT_A, h_a, m);
    let hash_activations = hash_pair(&a_bound, &hash_routing);
    let b_bound = bind_root(&SEED_SALT_B, h_b, n_e);
    let s_b = hash_pair(kappa, &b_bound);
    let s_a = hash_pair(&s_b, &hash_activations);
    MoeV3Reference {
        commitment: MoeCommitment {
            routing_root,
            hash_offsets,
            hash_routing,
            hash_activations,
        },
        s_a,
        s_b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moe_v3_reference_binds_roots_dimensions_and_routing() {
        let kappa = [0x11u8; 32];
        let h_a = [0x22u8; 32];
        let h_b = [0x33u8; 32];
        let routing_data: Vec<u8> = (0u32..40).flat_map(u32::to_le_bytes).collect();
        let routing_offsets: Vec<u8> = [10u32, 20, 30, 40]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let base = moe_v3_reference(
            &kappa, &h_a, &h_b, 192, 320, &routing_data, &routing_offsets,
        );

        assert_ne!(
            base,
            moe_v3_reference(&kappa, &h_a, &h_b, 193, 320, &routing_data, &routing_offsets)
        );
        assert_ne!(
            base,
            moe_v3_reference(&kappa, &h_a, &h_b, 192, 321, &routing_data, &routing_offsets)
        );
        let mut reordered = routing_data.clone();
        reordered[..8].reverse();
        assert_ne!(
            base,
            moe_v3_reference(&kappa, &h_a, &h_b, 192, 320, &reordered, &routing_offsets)
        );
        let mut changed_offsets = routing_offsets.clone();
        changed_offsets[0] ^= 1;
        assert_ne!(
            base,
            moe_v3_reference(&kappa, &h_a, &h_b, 192, 320, &routing_data, &changed_offsets)
        );
    }
}
