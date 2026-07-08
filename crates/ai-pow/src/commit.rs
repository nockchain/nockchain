//! Merkle commitment over fixed-size 32-byte leaves with sentinel padding.
//!
//! Domain-separated BLAKE3 with three contexts: leaves, internal nodes, and
//! a fixed sentinel for padding non-power-of-two leaf counts up to the next
//! power of two. Real LLM matmul shapes (e.g. Gemma 4 31B FFN at tile=64
//! gives 64 × 336 = 21504 tiles) are not power-of-two; padding lets us match
//! them without changing the tile geometry.

use std::sync::OnceLock;

use blake3::Hasher;
use thiserror::Error;

const CTX_LEAF: &str = "ai-pow v3 merkle-leaf";
const CTX_NODE: &str = "ai-pow v3 merkle-node";
const CTX_SENTINEL: &str = "ai-pow v3 merkle-sentinel";
const CTX_A_ROW_LEAF: &str = "ai-pow v3 a-row-leaf";
const CTX_B_COL_LEAF: &str = "ai-pow v3 b-col-leaf";

fn leaf_hash(m: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(CTX_LEAF);
    hasher.update(m);
    *hasher.finalize().as_bytes()
}

fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(CTX_NODE);
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

/// Pearl `BLAKE3 CHUNK_LEN` — 1024 bytes per leaf in the BLAKE3 chunk-Merkle.
pub const CHUNK_LEN: usize = 1024;

/// Round `raw_len` up to the next multiple of [`CHUNK_LEN`].
/// (`Pearl pearl-blake3 merkle.rs:15-17`).
pub fn padded_chunk_len(raw_len: usize) -> usize {
    raw_len.div_ceil(CHUNK_LEN) * CHUNK_LEN
}

/// Zero-pad `data` so its length is a multiple of [`CHUNK_LEN`]. Pearl's
/// matrix commitments hash the chunk-padded byte stream
/// (`Pearl pearl-blake3 merkle.rs:23-27`).
pub fn pad_to_chunk_boundary(data: &[u8]) -> Vec<u8> {
    let mut padded = data.to_vec();
    padded.resize(padded_chunk_len(data.len()), 0);
    padded
}

/// Pearl matrix commitment (`Pearl zk-pow ffi/mine.rs:163-165`,
/// `Pearl pearl-blake3 merkle.rs:43-82`).
///
/// `H_A = BLAKE3(pad_to_chunk_boundary(A_row_major), key=kappa)` and
/// `H_B = BLAKE3(pad_to_chunk_boundary(B_col_major), key=kappa)`. The
/// output is the root of BLAKE3's internal chunk-Merkle tree over the
/// padded byte stream, byte-equivalent to
/// `pearl_blake3::MerkleTree::new(padded, kappa).root()`.
pub fn matrix_commitment(matrix_bytes: &[u8], kappa: &[u8; 32]) -> [u8; 32] {
    let padded = pad_to_chunk_boundary(matrix_bytes);
    *Hasher::new_keyed(kappa)
        .update(&padded)
        .finalize()
        .as_bytes()
}

/// Leaf hash for a row of `A` under `H_A` (Pearl §4.3 line 2). Keyed by
/// `kappa` and domain-separated from `B`-column leaves via a fixed prefix.
pub fn a_row_leaf_hash(row_bytes: &[i8], kappa: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new_keyed(kappa);
    hasher.update(CTX_A_ROW_LEAF.as_bytes());
    hasher.update(&[0u8]); // null terminator separating context from content
                           // SAFETY: i8 and u8 share layout; we only need the raw bits.
    let bytes: &[u8] =
        unsafe { core::slice::from_raw_parts(row_bytes.as_ptr() as *const u8, row_bytes.len()) };
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

/// Leaf hash for a column of `B` under `H_B` (Pearl §4.3 line 3).
pub fn b_col_leaf_hash(col_bytes: &[i8], kappa: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Hasher::new_keyed(kappa);
    hasher.update(CTX_B_COL_LEAF.as_bytes());
    hasher.update(&[0u8]);
    let bytes: &[u8] =
        unsafe { core::slice::from_raw_parts(col_bytes.as_ptr() as *const u8, col_bytes.len()) };
    hasher.update(bytes);
    *hasher.finalize().as_bytes()
}

/// Sentinel placed at the leaf level for indices past the real leaf count.
/// Computed once via `derive_key` over an empty input under a dedicated
/// context, so it can never collide with a real `leaf_hash` output.
fn sentinel_leaf() -> [u8; 32] {
    static SENTINEL: OnceLock<[u8; 32]> = OnceLock::new();
    *SENTINEL.get_or_init(|| {
        let hasher = Hasher::new_derive_key(CTX_SENTINEL);
        *hasher.finalize().as_bytes()
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MerkleError {
    #[error("leaves slice is empty")]
    Empty,
    #[error("leaf index out of range")]
    IndexOutOfRange,
    #[error("path length does not match the padded tree depth")]
    PathLengthMismatch,
}

/// Compute the Merkle root of `leaves`, padding the leaf set to the next
/// power of two with a fixed sentinel hash. Returns an error if the slice
/// is empty.
pub fn merkle_root(leaves: &[[u8; 32]]) -> Result<[u8; 32], MerkleError> {
    let n = leaves.len();
    if n == 0 {
        return Err(MerkleError::Empty);
    }
    let padded = n.next_power_of_two();
    let mut layer: Vec<[u8; 32]> = Vec::with_capacity(padded);
    for m in leaves {
        layer.push(leaf_hash(m));
    }
    let sent = sentinel_leaf();
    while layer.len() < padded {
        layer.push(sent);
    }
    while layer.len() > 1 {
        layer = layer
            .chunks_exact(2)
            .map(|pair| node_hash(&pair[0], &pair[1]))
            .collect();
    }
    Ok(layer[0])
}

/// Sibling list for the leaf at `idx`, ordered from leaf-level upward. Each
/// sibling is the node not on the path from the leaf to the root, possibly
/// a sentinel if the sibling lies in the padded region.  `idx` must point to
/// a real leaf (`< leaves.len()`); sentinels are never opened.
pub fn merkle_path(leaves: &[[u8; 32]], idx: usize) -> Result<Vec<[u8; 32]>, MerkleError> {
    let n = leaves.len();
    if n == 0 {
        return Err(MerkleError::Empty);
    }
    if idx >= n {
        return Err(MerkleError::IndexOutOfRange);
    }
    let padded = n.next_power_of_two();
    let mut layer: Vec<[u8; 32]> = leaves.iter().map(leaf_hash).collect();
    let sent = sentinel_leaf();
    while layer.len() < padded {
        layer.push(sent);
    }
    let mut path = Vec::new();
    let mut pos = idx;
    while layer.len() > 1 {
        let sibling = if pos % 2 == 0 {
            layer[pos + 1]
        } else {
            layer[pos - 1]
        };
        path.push(sibling);
        layer = layer
            .chunks_exact(2)
            .map(|pair| node_hash(&pair[0], &pair[1]))
            .collect();
        pos /= 2;
    }
    Ok(path)
}

/// Recompute the root from a real leaf, its position, and the sibling path.
/// `num_leaves` is the *unpadded* count; the padded depth is derived
/// internally. Returns the recomputed root for caller-side comparison.
pub fn merkle_recover_root(
    leaf: &[u8; 32],
    idx: usize,
    path: &[[u8; 32]],
    num_leaves: usize,
) -> Result<[u8; 32], MerkleError> {
    if num_leaves == 0 {
        return Err(MerkleError::Empty);
    }
    if idx >= num_leaves {
        return Err(MerkleError::IndexOutOfRange);
    }
    let padded = num_leaves.next_power_of_two();
    let depth = padded.trailing_zeros() as usize;
    if path.len() != depth {
        return Err(MerkleError::PathLengthMismatch);
    }
    let mut node = leaf_hash(leaf);
    let mut pos = idx;
    for sibling in path {
        node = if pos % 2 == 0 {
            node_hash(&node, sibling)
        } else {
            node_hash(sibling, &node)
        };
        pos /= 2;
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_leaves(n: usize) -> Vec<[u8; 32]> {
        (0..n)
            .map(|i| {
                let mut v = [0u8; 32];
                v[..8].copy_from_slice(&(i as u64).to_le_bytes());
                v
            })
            .collect()
    }

    #[test]
    fn root_determinism() {
        let leaves = sample_leaves(8);
        assert_eq!(merkle_root(&leaves).unwrap(), merkle_root(&leaves).unwrap());
    }

    #[test]
    fn round_trip_each_leaf_pow2() {
        for n_log in 0..7 {
            let n = 1usize << n_log;
            let leaves = sample_leaves(n);
            let root = merkle_root(&leaves).unwrap();
            for idx in 0..n {
                let path = merkle_path(&leaves, idx).unwrap();
                let recovered = merkle_recover_root(&leaves[idx], idx, &path, n).unwrap();
                assert_eq!(root, recovered, "n={n} idx={idx}");
            }
        }
    }

    #[test]
    fn round_trip_each_leaf_non_pow2() {
        // Cover several non-power-of-2 leaf counts and every real-leaf index.
        for &n in &[3usize, 5, 7, 9, 13, 21, 96, 100] {
            let leaves = sample_leaves(n);
            let root = merkle_root(&leaves).unwrap();
            for idx in 0..n {
                let path = merkle_path(&leaves, idx).unwrap();
                let recovered = merkle_recover_root(&leaves[idx], idx, &path, n).unwrap();
                assert_eq!(root, recovered, "n={n} idx={idx}");
                assert_eq!(path.len(), n.next_power_of_two().trailing_zeros() as usize);
            }
        }
    }

    #[test]
    fn tampered_leaf_rejects() {
        let leaves = sample_leaves(8);
        let root = merkle_root(&leaves).unwrap();
        let path = merkle_path(&leaves, 3).unwrap();
        let mut tampered = leaves[3];
        tampered[0] ^= 1;
        let recovered = merkle_recover_root(&tampered, 3, &path, 8).unwrap();
        assert_ne!(recovered, root);
    }

    #[test]
    fn tampered_path_rejects() {
        let leaves = sample_leaves(8);
        let root = merkle_root(&leaves).unwrap();
        let mut path = merkle_path(&leaves, 3).unwrap();
        path[0][0] ^= 1;
        let recovered = merkle_recover_root(&leaves[3], 3, &path, 8).unwrap();
        assert_ne!(recovered, root);
    }

    #[test]
    fn rejects_empty() {
        let leaves: Vec<[u8; 32]> = Vec::new();
        assert_eq!(merkle_root(&leaves).err(), Some(MerkleError::Empty));
    }

    #[test]
    fn cannot_open_sentinel_index() {
        let leaves = sample_leaves(5);
        assert_eq!(
            merkle_path(&leaves, 5).err(),
            Some(MerkleError::IndexOutOfRange),
        );
        assert_eq!(
            merkle_path(&leaves, 7).err(),
            Some(MerkleError::IndexOutOfRange),
        );
    }

    #[test]
    fn matrix_leaf_hashes_are_domain_separated() {
        // A row and a B column with the same byte contents must yield
        // different leaf hashes (different domain-separation contexts).
        let row: [i8; 8] = [1, -2, 3, -4, 5, -6, 7, -8];
        let kappa = [9u8; 32];
        let r = a_row_leaf_hash(&row, &kappa);
        let c = b_col_leaf_hash(&row, &kappa);
        assert_ne!(r, c, "A-row and B-col leaves must be domain-separated");
        // Determinism.
        assert_eq!(r, a_row_leaf_hash(&row, &kappa));
        // Sensitivity to kappa.
        assert_ne!(a_row_leaf_hash(&row, &[10u8; 32]), r);
        // Sensitivity to content.
        let mut other = row;
        other[0] ^= 1;
        assert_ne!(a_row_leaf_hash(&other, &kappa), r);
    }

    /// M52 step 5: `matrix_commitment` is byte-equivalent to
    /// `blake3::Hasher::new_keyed(&κ).update(pad(matrix)).finalize()`,
    /// the same digest `ai-pow-zk::CompositeTrace::place_matrix_hash_a`
    /// computes inside the SNARK. This anchors the plain-side
    /// `BlockContext::h_a_chunk` to what the SNARK will bind as
    /// public input `HASH_A`.
    #[test]
    fn matrix_commitment_byte_equivalent_to_blake3_keyed() {
        // Multi-chunk case (4 KiB = 4 chunks).
        let matrix: Vec<u8> = (0..4096).map(|i| ((i * 31 + 7) & 0xFF) as u8).collect();
        let kappa = [0xA5u8; 32];
        let got = matrix_commitment(&matrix, &kappa);
        let expected = *Hasher::new_keyed(&kappa)
            .update(&matrix)
            .finalize()
            .as_bytes();
        assert_eq!(got, expected, "multi-chunk byte-equivalence");

        // Sub-chunk padding case (500 bytes → padded to 1024).
        let matrix2: Vec<u8> = (0..500).map(|i| (i as u8).wrapping_mul(13)).collect();
        let mut padded = matrix2.clone();
        padded.resize(1024, 0);
        let got2 = matrix_commitment(&matrix2, &kappa);
        let expected2 = *Hasher::new_keyed(&kappa)
            .update(&padded)
            .finalize()
            .as_bytes();
        assert_eq!(got2, expected2, "sub-chunk pad byte-equivalence");

        // Domain separation: changing κ changes the digest.
        let got_diff_kappa = matrix_commitment(&matrix, &[0xA6u8; 32]);
        assert_ne!(got, got_diff_kappa);
    }

    #[test]
    fn changing_leaf_count_changes_root() {
        // Padding should make the tree height match `next_power_of_two`, so
        // adding a real leaf into a previously-sentinel slot changes the root.
        let leaves5 = sample_leaves(5);
        let leaves6 = sample_leaves(6);
        let r5 = merkle_root(&leaves5).unwrap();
        let r6 = merkle_root(&leaves6).unwrap();
        assert_ne!(r5, r6);
    }
}
