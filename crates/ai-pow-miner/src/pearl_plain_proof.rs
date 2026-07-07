//! Pearl Gateway `submitPlainProof` payload construction.
//!
//! Gateway expects `plain_proof` to be base64 of Pearl's
//! `bincode 1.3.3` serialization of:
//!
//! ```text
//! PlainProof {
//!   m, n, k, noise_rank,
//!   a:  MatrixMerkleProof { proof: MerkleProof, row_indices },
//!   bt: MatrixMerkleProof { proof: MerkleProof, row_indices },
//! }
//! ```
//!
//! This module intentionally stays in the Rust miner crate. Hoon receives only
//! the opaque `%ai-pow` nonce and recursive Nockchain certificate.

use std::collections::BTreeSet;

use ai_pow::commit::matrix_commitment;
use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::PearlMergeTicketAttempt;
use ai_pow_zk::blake3_tree::{chunk_cv, parent_cv, CHUNK_LEN};
use base64::Engine as _;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PearlPlainProofError {
    #[error("matrix dimensions do not match the mined Pearl attempt")]
    MatrixShape,
    #[error("Pearl ticket has no opened rows or columns")]
    EmptyOpening,
    #[error("Pearl matrix commitment mismatch for {matrix}")]
    CommitmentMismatch { matrix: &'static str },
    #[error("Pearl proof leaf index is out of bounds")]
    LeafIndexOutOfBounds,
    #[error("Pearl proof serialization length overflow")]
    LengthOverflow,
    #[error("Pearl MoE (GROUPED_GEMM) plain-proof share generation is not supported yet")]
    MoeNotSupported,
}

/// bincode tag byte for `Option::None`. Pearl's V2 `PlainProof` ends in a
/// `moe: Option<MoEProofParams>` field, so a native V2 dense proof appends this
/// single tag byte after `bt` (Pearl `zk-pow/src/ffi/plain_proof.rs`).
const BINCODE_OPTION_NONE_TAG: u8 = 0x00;

/// Which Pearl `PlainProof` wire encoding to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainProofWireFormat {
    /// Native post-MoE-fork V2: the trailing `moe: Option<_>` bincode tag is
    /// present. A dense share carries `moe == None`, i.e. a single `0x00` byte.
    /// This is the exact byte format Pearl's V2 prover serializes.
    V2,
    /// Legacy pre-fork V1: no trailing `moe` field. A V2 gateway still accepts
    /// this via `PlainProof::deserialize_compat` (which retries with a `0x00`
    /// appended); retained only for talking to a pre-fork gateway.
    LegacyV1,
}

/// Placeholder for the MoE (GROUPED_GEMM) `PlainProof` tail (Pearl
/// `MoEProofParams`). Track B: constructing and encoding a MoE share is not yet
/// supported and fails closed. Kept as a distinct `Option` payload type so the
/// dense wire path (`moe == None`) stays byte-exact while the shape is reserved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PearlMoeProof {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlPlainProof {
    pub m: usize,
    pub n: usize,
    pub k: usize,
    pub noise_rank: usize,
    pub a: PearlMatrixMerkleProof,
    pub bt: PearlMatrixMerkleProof,
    /// MoE tail. `None` for a dense (standard) share — the only mode supported
    /// today. `Some(..)` is Track B and fails closed on encode.
    pub moe: Option<PearlMoeProof>,
}

impl PearlPlainProof {
    pub fn from_attempt(
        params: &MatmulParams,
        attempt: &PearlMergeTicketAttempt,
        a_row_major: &[i8],
        b_col_major: &[i8],
    ) -> Result<Self, PearlPlainProofError> {
        let m = usize::try_from(params.m).map_err(|_| PearlPlainProofError::MatrixShape)?;
        let n = usize::try_from(params.n).map_err(|_| PearlPlainProofError::MatrixShape)?;
        let k = usize::try_from(params.k).map_err(|_| PearlPlainProofError::MatrixShape)?;
        let noise_rank = usize::from(attempt.public_params.mining_config.rank);
        if attempt.public_params.m != params.m
            || attempt.public_params.n != params.n
            || attempt.public_params.mining_config.common_dim != params.k
            || a_row_major.len() != m.saturating_mul(k)
            || b_col_major.len() != n.saturating_mul(k)
        {
            return Err(PearlPlainProofError::MatrixShape);
        }

        let a_bytes = i8_slice_as_u8_vec(a_row_major);
        let b_bytes = i8_slice_as_u8_vec(b_col_major);
        let a = build_matrix_merkle_proof(
            &a_bytes, m, k, &attempt.ticket.a_rows, &attempt.commitments.kappa,
            &attempt.public_params.hash_a, "A",
        )?;
        let bt = build_matrix_merkle_proof(
            &b_bytes, n, k, &attempt.ticket.b_cols, &attempt.commitments.kappa,
            &attempt.public_params.hash_b, "B^T",
        )?;

        Ok(Self {
            m,
            n,
            k,
            noise_rank,
            a,
            bt,
            // Dense share: no MoE tail.
            moe: None,
        })
    }

    /// Serialize exactly as Pearl's V2 `PlainProof.to_base64()` does: bincode 1
    /// fixed-int, little-endian, then standard base64. Emits the native V2
    /// encoding (trailing `moe` `Option` tag) by default.
    pub fn to_base64_bincode1(&self) -> Result<String, PearlPlainProofError> {
        let mut bytes = Vec::new();
        self.encode_bincode1(&mut bytes)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Native V2 encoding (default). Equivalent to
    /// `encode_bincode1_with_format(out, PlainProofWireFormat::V2)`.
    pub fn encode_bincode1(&self, out: &mut Vec<u8>) -> Result<(), PearlPlainProofError> {
        self.encode_bincode1_with_format(out, PlainProofWireFormat::V2)
    }

    pub fn encode_bincode1_with_format(
        &self,
        out: &mut Vec<u8>,
        format: PlainProofWireFormat,
    ) -> Result<(), PearlPlainProofError> {
        put_usize(out, self.m)?;
        put_usize(out, self.n)?;
        put_usize(out, self.k)?;
        put_usize(out, self.noise_rank)?;
        self.a.encode_bincode1(out)?;
        self.bt.encode_bincode1(out)?;
        match (&self.moe, format) {
            // MoE (GROUPED_GEMM) share generation is Track B: fail closed.
            (Some(_), _) => return Err(PearlPlainProofError::MoeNotSupported),
            // Native V2 dense: trailing bincode `Option::None` tag.
            (None, PlainProofWireFormat::V2) => out.push(BINCODE_OPTION_NONE_TAG),
            // Legacy V1: no trailing `moe` field.
            (None, PlainProofWireFormat::LegacyV1) => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMatrixMerkleProof {
    pub proof: PearlMerkleProof,
    pub row_indices: Vec<usize>,
}

impl PearlMatrixMerkleProof {
    fn encode_bincode1(&self, out: &mut Vec<u8>) -> Result<(), PearlPlainProofError> {
        self.proof.encode_bincode1(out)?;
        put_usize_vec(out, &self.row_indices)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMerkleProof {
    pub leaf_data: Vec<[u8; CHUNK_LEN]>,
    pub leaf_indices: Vec<usize>,
    pub total_leaves: usize,
    pub root: [u8; 32],
    pub siblings: Vec<[u8; 32]>,
}

impl PearlMerkleProof {
    fn encode_bincode1(&self, out: &mut Vec<u8>) -> Result<(), PearlPlainProofError> {
        // Pearl serializes leaf_data through a serde helper as Vec<&[u8]>.
        put_len(out, self.leaf_data.len())?;
        for leaf in &self.leaf_data {
            put_len(out, leaf.len())?;
            out.extend_from_slice(leaf);
        }
        put_usize_vec(out, &self.leaf_indices)?;
        put_usize(out, self.total_leaves)?;
        out.extend_from_slice(&self.root);
        put_len(out, self.siblings.len())?;
        for sibling in &self.siblings {
            out.extend_from_slice(sibling);
        }
        Ok(())
    }
}

fn build_matrix_merkle_proof(
    matrix_bytes: &[u8],
    rows: usize,
    cols: usize,
    row_indices_u32: &[u32],
    kappa: &[u8; 32],
    expected_root: &[u8; 32],
    matrix: &'static str,
) -> Result<PearlMatrixMerkleProof, PearlPlainProofError> {
    if row_indices_u32.is_empty() {
        return Err(PearlPlainProofError::EmptyOpening);
    }
    if matrix_bytes.len() != rows.saturating_mul(cols) {
        return Err(PearlPlainProofError::MatrixShape);
    }
    let actual_root = matrix_commitment(matrix_bytes, kappa);
    if &actual_root != expected_root {
        return Err(PearlPlainProofError::CommitmentMismatch { matrix });
    }

    let row_indices: Vec<usize> = row_indices_u32
        .iter()
        .map(|&row| usize::try_from(row).map_err(|_| PearlPlainProofError::MatrixShape))
        .collect::<Result<_, _>>()?;
    if row_indices.iter().any(|&row| row >= rows) {
        return Err(PearlPlainProofError::MatrixShape);
    }

    let padded = pad_to_chunk_boundary(matrix_bytes);
    let total_leaves = padded.len() / CHUNK_LEN;
    if total_leaves == 0 {
        return Err(PearlPlainProofError::MatrixShape);
    }
    let leaf_indices = compute_leaf_indices_from_rows(&row_indices, cols);
    if leaf_indices.is_empty() {
        return Err(PearlPlainProofError::EmptyOpening);
    }
    if leaf_indices.iter().any(|&idx| idx >= total_leaves) {
        return Err(PearlPlainProofError::LeafIndexOutOfBounds);
    }

    let layers = build_merkle_layers(&padded, kappa);
    let root = if total_leaves == 1 {
        actual_root
    } else {
        layers
            .last()
            .and_then(|layer| layer.first())
            .copied()
            .ok_or(PearlPlainProofError::MatrixShape)?
    };
    if root != *expected_root {
        return Err(PearlPlainProofError::CommitmentMismatch { matrix });
    }

    let leaf_data = leaf_indices
        .iter()
        .map(|&idx| {
            let mut leaf = [0u8; CHUNK_LEN];
            let start = idx * CHUNK_LEN;
            leaf.copy_from_slice(&padded[start..start + CHUNK_LEN]);
            leaf
        })
        .collect();
    let siblings = collect_multileaf_siblings(&layers, &leaf_indices, total_leaves);

    Ok(PearlMatrixMerkleProof {
        proof: PearlMerkleProof {
            leaf_data,
            leaf_indices,
            total_leaves,
            root,
            siblings,
        },
        row_indices,
    })
}

fn compute_leaf_indices_from_rows(row_indices: &[usize], cols: usize) -> Vec<usize> {
    let mut out = BTreeSet::new();
    for &row in row_indices {
        let first = (row * cols) / CHUNK_LEN;
        let last = ((row + 1) * cols - 1) / CHUNK_LEN;
        for idx in first..=last {
            out.insert(idx);
        }
    }
    out.into_iter().collect()
}

fn build_merkle_layers(padded: &[u8], kappa: &[u8; 32]) -> Vec<Vec<[u8; 32]>> {
    let mut layers = Vec::new();
    let mut current: Vec<[u8; 32]> = padded
        .chunks_exact(CHUNK_LEN)
        .enumerate()
        .map(|(idx, chunk)| {
            let mut leaf = [0u8; CHUNK_LEN];
            leaf.copy_from_slice(chunk);
            chunk_cv(&leaf, idx as u64, kappa, false)
        })
        .collect();
    layers.push(current.clone());

    while current.len() > 2 {
        let mut next = Vec::with_capacity(current.len().div_ceil(2));
        for pair in current.chunks(2) {
            if pair.len() == 2 {
                next.push(parent_cv(&pair[0], &pair[1], kappa, false));
            } else {
                next.push(pair[0]);
            }
        }
        current = next;
        layers.push(current.clone());
    }
    if current.len() == 2 {
        layers.push(vec![parent_cv(&current[0], &current[1], kappa, true)]);
    }
    layers
}

fn collect_multileaf_siblings(
    layers: &[Vec<[u8; 32]>],
    leaf_indices: &[usize],
    total_leaves: usize,
) -> Vec<[u8; 32]> {
    let mut siblings = Vec::new();
    let mut current_set: BTreeSet<usize> = leaf_indices.iter().copied().collect();
    let mut level_len = total_leaves;
    let mut level = 0usize;

    while level_len > 1 && !current_set.is_empty() {
        let level_nodes = &layers[level];
        for &i in &current_set {
            if i % 2 == 1 {
                if !current_set.contains(&(i - 1)) {
                    siblings.push(level_nodes[i - 1]);
                }
            } else if !current_set.contains(&(i + 1)) && (i + 1) < level_len {
                siblings.push(level_nodes[i + 1]);
            }
        }
        current_set = current_set.iter().map(|&i| i / 2).collect();
        level_len = level_len.div_ceil(2);
        level += 1;
    }

    siblings
}

fn pad_to_chunk_boundary(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    out.resize(data.len().div_ceil(CHUNK_LEN) * CHUNK_LEN, 0);
    out
}

fn i8_slice_as_u8_vec(values: &[i8]) -> Vec<u8> {
    values.iter().map(|&v| v as u8).collect()
}

fn put_usize(out: &mut Vec<u8>, value: usize) -> Result<(), PearlPlainProofError> {
    let value = u64::try_from(value).map_err(|_| PearlPlainProofError::LengthOverflow)?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_len(out: &mut Vec<u8>, value: usize) -> Result<(), PearlPlainProofError> {
    put_usize(out, value)
}

fn put_usize_vec(out: &mut Vec<u8>, values: &[usize]) -> Result<(), PearlPlainProofError> {
    put_len(out, values.len())?;
    for &value in values {
        put_usize(out, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ai_pow::params::MatmulParams;
    use ai_pow::pearl_compat::{
        evaluate_pearl_merge_ticket_attempt, PearlIncompleteBlockHeader, PearlMiningConfig,
        PearlNockchainAux, PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE,
        PEARL_MMA_INT7XINT7_TO_INT32,
    };
    use ai_pow::synth::synth_matrices;
    use base64::Engine as _;
    use serde::{Deserialize, Serialize};

    use super::*;

    /// Serde mirror of Pearl's V2 `PlainProof` (with the trailing
    /// `moe: Option<_>` field). We only exercise the dense case (`moe == None`),
    /// which bincode encodes as a single `0x00` tag regardless of the payload
    /// type, so `Option<()>` is byte-faithful here.
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SerdePlainProof {
        m: usize,
        n: usize,
        k: usize,
        noise_rank: usize,
        a: SerdeMatrixMerkleProof,
        bt: SerdeMatrixMerkleProof,
        moe: Option<()>,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SerdeMatrixMerkleProof {
        proof: SerdeMerkleProof,
        row_indices: Vec<usize>,
    }

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct SerdeMerkleProof {
        leaf_data: Vec<Vec<u8>>,
        leaf_indices: Vec<usize>,
        total_leaves: usize,
        root: [u8; 32],
        siblings: Vec<[u8; 32]>,
    }

    impl From<&PearlPlainProof> for SerdePlainProof {
        fn from(value: &PearlPlainProof) -> Self {
            Self {
                m: value.m,
                n: value.n,
                k: value.k,
                noise_rank: value.noise_rank,
                a: SerdeMatrixMerkleProof::from(&value.a),
                bt: SerdeMatrixMerkleProof::from(&value.bt),
                moe: value.moe.as_ref().map(|_| ()),
            }
        }
    }

    impl From<&PearlMatrixMerkleProof> for SerdeMatrixMerkleProof {
        fn from(value: &PearlMatrixMerkleProof) -> Self {
            Self {
                proof: SerdeMerkleProof::from(&value.proof),
                row_indices: value.row_indices.clone(),
            }
        }
    }

    impl From<&PearlMerkleProof> for SerdeMerkleProof {
        fn from(value: &PearlMerkleProof) -> Self {
            Self {
                leaf_data: value.leaf_data.iter().map(|leaf| leaf.to_vec()).collect(),
                leaf_indices: value.leaf_indices.clone(),
                total_leaves: value.total_leaves,
                root: value.root,
                siblings: value.siblings.clone(),
            }
        }
    }

    fn test_config() -> PearlMiningConfig {
        PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: PearlPeriodicPattern {
                shape: [(1, 16), (16, 1), (16, 1)],
            },
            cols_pattern: PearlPeriodicPattern {
                shape: [(1, 16), (16, 1), (16, 1)],
            },
            reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        }
    }

    fn test_header() -> PearlIncompleteBlockHeader {
        PearlIncompleteBlockHeader {
            version: 0x2000_0000,
            prev_block: [1u8; 32],
            merkle_root: [2u8; 32],
            timestamp: 1_700_000_000,
            nbits: 0x207f_ffff,
        }
    }

    fn test_aux() -> PearlNockchainAux {
        PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet".to_vec(),
            nock_block_commitment: [4u8; 32],
            nockchain_target_epoch_or_height: 42,
            extra_domain_data: Vec::new(),
        }
    }

    fn test_params() -> MatmulParams {
        MatmulParams {
            m: 32,
            k: 1024,
            n: 32,
            noise_rank: 64,
            tile: 16,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    #[test]
    fn pearl_plain_proof_base64_is_bincode1_plain_proof_payload() {
        let params = test_params();
        let config = test_config();
        let (a, b) = synth_matrices(b"pearl-plain-proof", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &test_header(),
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xffu8; 32],
            16,
            test_aux(),
        )
        .expect("evaluate Pearl ticket");

        let proof =
            PearlPlainProof::from_attempt(&params, &attempt, &a, &b).expect("build plain proof");
        assert_eq!(proof.m, 32);
        assert_eq!(proof.n, 32);
        assert_eq!(proof.k, 1024);
        assert_eq!(proof.noise_rank, 64);
        assert_eq!(proof.a.row_indices, (0usize..16).collect::<Vec<_>>());
        assert_eq!(proof.bt.row_indices, (0usize..16).collect::<Vec<_>>());
        assert_eq!(proof.a.proof.root, attempt.public_params.hash_a);
        assert_eq!(proof.bt.proof.root, attempt.public_params.hash_b);

        let mut raw = Vec::new();
        proof.encode_bincode1(&mut raw).expect("serialize raw");
        let encoded = proof.to_base64_bincode1().expect("serialize base64");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("decode base64"),
            raw
        );
        assert!(raw.len() > 2 * CHUNK_LEN);
    }

    #[test]
    fn pearl_plain_proof_manual_bytes_match_bincode1_serde_layout() {
        let params = test_params();
        let config = test_config();
        let (a, b) = synth_matrices(b"pearl-plain-proof-bincode1", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &test_header(),
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xffu8; 32],
            16,
            test_aux(),
        )
        .expect("evaluate Pearl ticket");
        let proof =
            PearlPlainProof::from_attempt(&params, &attempt, &a, &b).expect("build plain proof");

        let mut manual = Vec::new();
        proof.encode_bincode1(&mut manual).expect("manual encode");
        let serde_shape = SerdePlainProof::from(&proof);
        let bincode = bincode1::serialize(&serde_shape).expect("bincode1 serialize");

        assert_eq!(
            manual, bincode,
            "manual Pearl PlainProof encoder must match Pearl's bincode 1 serde layout"
        );
    }

    #[test]
    fn pearl_plain_proof_rejects_wrong_matrix() {
        let params = test_params();
        let config = test_config();
        let (mut a, b) = synth_matrices(b"pearl-plain-proof-wrong-matrix", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &test_header(),
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xffu8; 32],
            16,
            test_aux(),
        )
        .expect("evaluate Pearl ticket");
        a[0] ^= 1;

        assert!(matches!(
            PearlPlainProof::from_attempt(&params, &attempt, &a, &b),
            Err(PearlPlainProofError::CommitmentMismatch { matrix: "A" })
        ));
    }

    fn build_dense_proof() -> PearlPlainProof {
        let params = test_params();
        let config = test_config();
        let (a, b) = synth_matrices(b"pearl-plain-proof-v2", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &test_header(),
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xffu8; 32],
            16,
            test_aux(),
        )
        .expect("evaluate Pearl ticket");
        PearlPlainProof::from_attempt(&params, &attempt, &a, &b).expect("build plain proof")
    }

    /// A3 — native V2 dense encoding is exactly the legacy V1 bytes plus a single
    /// trailing bincode `Option::None` tag, and the default encoder emits V2.
    #[test]
    fn pearl_v2_native_dense_is_legacy_plus_option_none_tag() {
        let proof = build_dense_proof();
        assert!(proof.moe.is_none());

        let mut v2 = Vec::new();
        proof
            .encode_bincode1_with_format(&mut v2, PlainProofWireFormat::V2)
            .unwrap();
        let mut legacy = Vec::new();
        proof
            .encode_bincode1_with_format(&mut legacy, PlainProofWireFormat::LegacyV1)
            .unwrap();

        assert_eq!(
            v2.len(),
            legacy.len() + 1,
            "V2 adds exactly one option-tag byte over legacy"
        );
        assert_eq!(&v2[..legacy.len()], &legacy[..], "V2 core == legacy encoding");
        assert_eq!(*v2.last().unwrap(), 0x00, "trailing bincode Option::None tag");

        // Default encoder / base64 helper both emit native V2.
        let mut default = Vec::new();
        proof.encode_bincode1(&mut default).unwrap();
        assert_eq!(default, v2, "default encoding is native V2");
        let b64 = proof.to_base64_bincode1().unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap(),
            v2
        );
    }

    /// A3/A4 — our dense bytes deserialize as `moe == None` under the exact
    /// `bincode 1` fixint config Pearl uses, and the legacy encoding is recovered
    /// only after appending the `0x00` tag — i.e. our two encodings behave
    /// exactly as Pearl's `PlainProof::deserialize_compat` expects.
    #[test]
    fn pearl_v2_dense_bytes_deserialize_as_moe_none_like_pearl_compat() {
        let proof = build_dense_proof();
        let expected = SerdePlainProof::from(&proof);
        assert!(expected.moe.is_none());

        // Native V2: strict bincode1 (fixint LE) decodes directly, moe == None.
        let mut v2 = Vec::new();
        proof
            .encode_bincode1_with_format(&mut v2, PlainProofWireFormat::V2)
            .unwrap();
        let decoded: SerdePlainProof = bincode1::deserialize(&v2).expect("strict decode V2");
        assert_eq!(decoded, expected);
        assert!(decoded.moe.is_none());

        // Legacy V1: strict decode fails (missing moe tag); appending the
        // Option::None tag recovers it — exactly Pearl's `deserialize_compat`.
        let mut legacy = Vec::new();
        proof
            .encode_bincode1_with_format(&mut legacy, PlainProofWireFormat::LegacyV1)
            .unwrap();
        assert!(
            bincode1::deserialize::<SerdePlainProof>(&legacy).is_err(),
            "legacy bytes must not strict-decode as V2"
        );
        let mut padded = legacy.clone();
        padded.push(0x00);
        let recovered: SerdePlainProof = bincode1::deserialize(&padded).expect("compat decode");
        assert_eq!(recovered, expected);
    }

    /// A5 — generating a MoE (GROUPED_GEMM) share is Track B and fails closed on
    /// every encoding path, so we can never emit an unsupported MoE share.
    #[test]
    fn pearl_moe_share_encoding_fails_closed() {
        let mut proof = build_dense_proof();
        proof.moe = Some(PearlMoeProof {});

        let mut out = Vec::new();
        assert!(matches!(
            proof.encode_bincode1(&mut out),
            Err(PearlPlainProofError::MoeNotSupported)
        ));
        assert!(matches!(
            proof.to_base64_bincode1(),
            Err(PearlPlainProofError::MoeNotSupported)
        ));
        let mut out_legacy = Vec::new();
        assert!(matches!(
            proof.encode_bincode1_with_format(&mut out_legacy, PlainProofWireFormat::LegacyV1),
            Err(PearlPlainProofError::MoeNotSupported)
        ));
    }
}
