//! Structured noun encoder for recursive AI-PoW certificates.
//!
//! This module intentionally accepts the recursive certificate object, not
//! `MatmulProof` and not a standalone raw Layer-0 `AiPowBatchProof`. The
//! recursive certificate embeds Layer-0 proof/program context so verification
//! can rebuild the L1 circuit binding. Its verifier boundary also runs the
//! full-matmul statement precheck before recursive proof reconstruction or
//! verification. The selected compact final-layer batch-STARK path is encoded
//! as canonical postcard bytes inside a bounded proof-node atom so the noun
//! boundary does not re-inflate the compact certificate. The compact verify
//! boundary here IS the consensus verify path: `check-pow`'s `%ai-pow` branch
//! calls it through the `%ai-pow-verify` jet.
//!
//! ## Production API guide
//!
//! - Build a Pearl-compatible `%ai-pow` artifact with
//!   [`crate::certificate_noun::build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run`].
//! - Verify a compact artifact with verifier-owned setup using
//!   [`crate::certificate_noun::verify_ai_pow_pearl_merge_compact_artifact_jam_with_digest_bytes_and_context`].
//! - Use metadata precheck helpers such as
//!   [`crate::certificate_noun::precheck_ai_pow_pearl_merge_artifact_jam_with_context`] when caller code
//!   only needs cheap nonce/statement/target rejection before proof traversal.
//!
//! The non-compact checkpoint constructors and verifiers are retained for
//! regression only and are hidden from normal rustdoc. They are too large for
//! the selected production wire path.

use std::panic::{catch_unwind, AssertUnwindSafe};

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    verify_pearl_aux_inclusion, verify_pearl_compatible_work_committed,
    verify_pearl_merge_public_statement_bytes,
    verify_pearl_merge_public_statement_bytes_with_aux_inclusion, verify_pearl_moe_compatible_work,
    PearlAuxInclusionProof, PearlCompatError, PearlIncompleteBlockHeader, PearlMergeMiningPrecheck,
    PearlMergePublicStatement, PearlMergeTicketAttempt, PearlMiningConfig, PearlMoeParams,
    PearlMoeWorkPrecheck, PearlNockchainAux, PearlPatternTicket, PearlPublicProofParams,
    PearlWorkCommitments, PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES,
    PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH, PEARL_INCOMPLETE_BLOCK_HEADER_SIZE,
    PEARL_MINING_CONFIG_SIZE, PEARL_MOE_MAX_NUM_EXPERTS, PEARL_MOE_MAX_OUTER_INDICES,
    PEARL_MOE_MAX_ROUTING_ENTRIES, PEARL_MOE_NONCE_MAX_ATOM_BYTES, PEARL_PUBLIC_PROOF_PARAMS_SIZE,
};
#[cfg(test)]
use ai_pow::pearl_compat::{PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX, PEARL_NOCKCHAIN_AUX_EXTRA_MAX};
use ai_pow::zk_bridge::{
    expected_layer0_rows_for_strip_schedule, verify_pearl_moe_compact_recursive_certificate,
    zk_params_from_matmul, AiPowCompactRecursiveCertificateRun, AiPowRecursiveCertificateRun,
    BridgeError, ZkPublicCommitments,
};
use ai_pow_zk::canonical::StripIndexSchedule;
use ai_pow_zk::{CompositePublicInputs, ZkParams};
use nockapp::noun::slab::{CueError, NounSlab};
use nockapp::Bytes;
use nockvm::jets::bits::util::met;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D, T};
use nockvm_macros::tas;
use serde::de::{
    self, DeserializeOwned, EnumAccess, IntoDeserializer, MapAccess, SeqAccess, VariantAccess,
    Visitor,
};
use serde::ser::{self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeTuple};

const AI_POW_CERT_VERSION: u64 = 1;
const GOLDILOCKS_MODULUS: u64 = 0xffff_ffff_0000_0001;
pub const AI_POW_NONCE_MAGIC: [u8; 4] = *b"AIP1";
pub const AI_POW_NONCE_MAX_SIZE: usize = 4
    + 2
    + ai_pow::pearl_compat::PEARL_MERGE_PUBLIC_STATEMENT_MAX_SIZE
    + 4
    + PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES
    + 1
    + 32 * PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH;

/// Magic tag for a MoE (GROUPED_GEMM) `ai-pow-nonce`. Distinct from the dense
/// [`AI_POW_NONCE_MAGIC`] so the dense wire stays byte-identical and a decoder
/// dispatches on the tag; a dense decoder rejects a MoE nonce (and vice versa).
pub const AI_POW_NONCE_MAGIC_MOE: [u8; 4] = *b"AIM1";

/// Upper bound on a MoE `ai-pow-nonce` (dense envelope + MoE tail + routing block).
pub const AI_POW_NONCE_MOE_MAX_SIZE: usize = AI_POW_NONCE_MAX_SIZE
    + 2
    + PEARL_MOE_MAX_NUM_EXPERTS * 4
    + 32
    + 1
    + PEARL_MOE_MAX_OUTER_INDICES * 4
    + 4
    + PEARL_MOE_MAX_ROUTING_ENTRIES * 4;

#[derive(Debug, thiserror::Error)]
pub enum CertificateNounError {
    #[error("recursive certificate serialization: {0}")]
    Serialize(String),
    #[error("recursive certificate deserialization: {0}")]
    Deserialize(String),
    #[error("recursive certificate noun has invalid shape: {0}")]
    Shape(&'static str),
    #[error("recursive certificate proof-node is not canonical")]
    NonCanonicalProofNode,
    #[error("unsupported AI-PoW certificate version {0}")]
    UnsupportedVersion(u64),
    #[error("recursive certificate noun exceeds {0} limit")]
    LimitExceeded(&'static str),
    #[error("{tag} declares {declared} bytes but atom contains {actual} bytes")]
    PackedLengthMismatch {
        tag: &'static str,
        declared: usize,
        actual: usize,
    },
    #[error("recursive certificate noun has invalid proof-node tag {0}")]
    InvalidTag(u64),
    #[error("integer field {field} is out of range")]
    IntegerOutOfRange { field: &'static str },
    #[error("field element {field} is not canonical")]
    NonCanonicalField { field: &'static str },
    #[error("certificate ZK params do not match trusted AI-PoW params: expected {expected:?}, got {actual:?}")]
    ZkParamsMismatch {
        expected: ZkParams,
        actual: ZkParams,
    },
    #[error("certificate statement metadata is not bound to trusted block state: {0}")]
    Statement(#[from] BridgeError),
    #[error("Pearl merge statement is invalid: {0}")]
    PearlMergeStatement(#[from] PearlCompatError),
    #[error("Pearl merge recursive certificate public input mismatch: {0}")]
    PearlMergePublicInputMismatch(&'static str),
    #[error("Pearl merge recursive certificate params are not supported by the current recursive parameter envelope")]
    PearlMergeUnsupportedTileShape,
    #[error("recursive certificate verification failed: {0}")]
    RecursiveCertificate(String),
    #[error("compact recursive certificate verifier-key digest mismatch: {0}")]
    CompactVerifierKeyDigestMismatch(&'static str),
    #[error("compact recursive certificate verifier-key digest encoding: {0}")]
    CompactVerifierKeyDigestEncoding(String),
    #[error("jammed AI-PoW artifact is {actual} bytes, exceeding {limit} byte limit")]
    JammedLengthExceeded { limit: usize, actual: usize },
    #[error("jammed AI-PoW artifact cue failed: {0}")]
    Cue(#[from] CueError),
    #[error("jammed AI-PoW artifact cue panicked")]
    CuePanic,
    #[error("jammed AI-PoW artifact is not canonical jam")]
    NonCanonicalJam,
}

/// Resource limits for decoding the structured certificate noun.
///
/// These limits are deliberately independent of ZK verification: they bound
/// parser work before Hoon/Rust code walks a miner-controlled proof artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertificateNounLimits {
    pub max_depth: usize,
    pub max_total_nodes: usize,
    pub max_list_items: usize,
    pub max_atom_bytes: usize,
    pub max_packed_items: usize,
    /// Maximum jammed artifact bytes accepted before cueing attacker input.
    pub max_jam_bytes: usize,
    /// Maximum CUMULATIVE decoded atom bytes across the whole proof tree (audit
    /// N9). `max_atom_bytes` caps a *single* node; without this, `max_total_nodes`
    /// (1M) × `max_atom_bytes` (1 MiB) permits ~1 TiB of transient allocation from
    /// a ≤4 MiB jam (each `[%bytes [len data]]` costs only a few jam bytes once the
    /// tag is back-referenced, yet allocates `len`). This bounds the total.
    pub max_total_atom_bytes: usize,
}

impl Default for CertificateNounLimits {
    fn default() -> Self {
        Self {
            max_depth: 256,
            max_total_nodes: 1_000_000,
            max_list_items: 1_000_000,
            max_atom_bytes: PEARL_MOE_NONCE_MAX_ATOM_BYTES,
            max_packed_items: 1_000_000,
            max_jam_bytes: 4 << 20,
            // 64 MiB: ~16× the jam cap — generous for any legitimate proof tree
            // (real compact certs decode to < a few MiB) yet bounds the DoS from
            // ~1 TiB to 64 MiB.
            max_total_atom_bytes: 64 << 20,
        }
    }
}

/// Decoded top-level `ai-pow-certificate` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPowCertificateShape {
    pub version: u64,
    pub zk_params: ZkParams,
    pub found_idx: u32,
    pub trace_height: usize,
    pub commitments: ZkPublicCommitments,
    pub public_inputs: CompositePublicInputs,
    pub certificate: AiProofNode,
}

/// Metadata-only view of a decoded `%ai-pow` artifact.
///
/// This intentionally stops before the recursive proof-node tail. Verifier and
/// submission boundaries can use it to reject malformed nonces, replayed aux
/// bindings, target misses, and certificate metadata drift before traversing a
/// miner-controlled proof tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiPowArtifactMetadataShape {
    pub nonce: Vec<u8>,
    pub certificate: AiPowCertificateMetadata,
}

/// Rust-side structured view of the Pearl-compatible public statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergePublicStatementShape {
    pub block_header: [u8; PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
    pub public_data: [u8; PEARL_PUBLIC_PROOF_PARAMS_SIZE],
    pub expected_aux_commitment: [u8; 32],
    pub aux: PearlNockchainAux,
}

impl PearlMergePublicStatementShape {
    pub fn from_wire_statement(
        statement: &PearlMergePublicStatement,
    ) -> Result<Self, CertificateNounError> {
        Ok(Self {
            block_header: statement.block_header,
            public_data: statement.public_data,
            expected_aux_commitment: statement.expected_aux_commitment,
            aux: PearlNockchainAux::from_bytes(&statement.aux_bytes)?,
        })
    }

    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, CertificateNounError> {
        let statement = PearlMergePublicStatement::from_bytes(bytes)?;
        Self::from_wire_statement(&statement)
    }

    pub fn to_wire_statement(&self) -> Result<PearlMergePublicStatement, CertificateNounError> {
        Ok(PearlMergePublicStatement {
            block_header: self.block_header,
            public_data: self.public_data,
            expected_aux_commitment: self.expected_aux_commitment,
            aux_bytes: self.aux.to_bytes()?,
        })
    }

    pub fn to_wire_bytes(&self) -> Result<Vec<u8>, CertificateNounError> {
        Ok(self.to_wire_statement()?.to_bytes()?)
    }
}

pub(crate) fn encode_pearl_merge_ai_pow_nonce(
    statement: &PearlMergePublicStatementShape,
    aux_inclusion: &PearlAuxInclusionProof,
) -> Result<Vec<u8>, CertificateNounError> {
    let statement_bytes = statement.to_wire_bytes()?;
    let statement_len = u16::try_from(statement_bytes.len())
        .map_err(|_| CertificateNounError::LimitExceeded("ai-pow nonce statement bytes"))?;
    if aux_inclusion.coinbase_tx.len() > PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow nonce coinbase bytes",
        ));
    }
    if aux_inclusion.merkle_branch.len() > PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow nonce merkle branch",
        ));
    }

    let mut out = Vec::with_capacity(
        4 + 2
            + statement_bytes.len()
            + 4
            + aux_inclusion.coinbase_tx.len()
            + 1
            + 32 * aux_inclusion.merkle_branch.len(),
    );
    out.extend_from_slice(&AI_POW_NONCE_MAGIC);
    out.extend_from_slice(&statement_len.to_le_bytes());
    out.extend_from_slice(&statement_bytes);
    out.extend_from_slice(&(aux_inclusion.coinbase_tx.len() as u32).to_le_bytes());
    out.extend_from_slice(&aux_inclusion.coinbase_tx);
    out.push(aux_inclusion.merkle_branch.len() as u8);
    for digest in &aux_inclusion.merkle_branch {
        out.extend_from_slice(digest);
    }
    Ok(out)
}

pub(crate) fn decode_pearl_merge_ai_pow_nonce(
    nonce: &[u8],
) -> Result<PearlMergeAiPowNonceShape, CertificateNounError> {
    if nonce.len() > AI_POW_NONCE_MAX_SIZE {
        return Err(CertificateNounError::LimitExceeded("ai-pow nonce bytes"));
    }
    if nonce.len() < 4 + 2 + 4 + 1 {
        return Err(CertificateNounError::Shape("ai-pow nonce is too short"));
    }
    if nonce[0..4] != AI_POW_NONCE_MAGIC {
        return Err(CertificateNounError::Shape("ai-pow nonce magic"));
    }

    let mut offset = 4usize;
    let statement_len = u16::from_le_bytes(
        nonce[offset..offset + 2]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    ) as usize;
    offset += 2;
    let statement_end =
        offset
            .checked_add(statement_len)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow nonce statement bytes",
            ))?;
    if statement_end > nonce.len() {
        return Err(CertificateNounError::Shape("ai-pow nonce statement length"));
    }
    let statement = PearlMergePublicStatementShape::from_wire_bytes(&nonce[offset..statement_end])?;
    offset = statement_end;

    if nonce.len().saturating_sub(offset) < 4 + 1 {
        return Err(CertificateNounError::Shape("ai-pow nonce coinbase length"));
    }
    let coinbase_len = u32::from_le_bytes(
        nonce[offset..offset + 4]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    ) as usize;
    offset += 4;
    if coinbase_len > PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow nonce coinbase bytes",
        ));
    }
    let coinbase_end =
        offset
            .checked_add(coinbase_len)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow nonce coinbase bytes",
            ))?;
    if coinbase_end > nonce.len() {
        return Err(CertificateNounError::Shape("ai-pow nonce coinbase length"));
    }
    let coinbase_tx = nonce[offset..coinbase_end].to_vec();
    offset = coinbase_end;

    let Some(&branch_len_byte) = nonce.get(offset) else {
        return Err(CertificateNounError::Shape("ai-pow nonce branch length"));
    };
    let branch_len = branch_len_byte as usize;
    offset += 1;
    if branch_len > PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow nonce merkle branch",
        ));
    }
    let branch_bytes = branch_len
        .checked_mul(32)
        .ok_or(CertificateNounError::LimitExceeded(
            "ai-pow nonce merkle branch",
        ))?;
    let expected_end =
        offset
            .checked_add(branch_bytes)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow nonce merkle branch",
            ))?;
    if expected_end != nonce.len() {
        return Err(CertificateNounError::Shape("ai-pow nonce trailing bytes"));
    }
    let mut merkle_branch = Vec::with_capacity(branch_len);
    for chunk in nonce[offset..expected_end].chunks_exact(32) {
        merkle_branch.push(chunk.try_into().expect("chunk length"));
    }

    Ok(PearlMergeAiPowNonceShape {
        statement,
        aux_inclusion: PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch,
        },
    })
}

/// Rust-owned MoE (GROUPED_GEMM) contents carried alongside a Pearl-compatible
/// statement in a MoE `ai-pow-nonce`.
///
/// `moe` is the Pearl `MoEParams` tail (`expert_idx`, `routing_offsets`,
/// `hash_routing`, `outer_indices`); `routing_data` is the flat `m·top_k`
/// per-expert-sorted token array the node needs to recompute the routing
/// commitment (`verify_pearl_moe_routing_binding`). This codec only carries and
/// bounds these values; it performs **no** semantic validation (routing length
/// vs `m·top_k`, token ranges, root binding, gather). Production admission
/// performs those checks in the compact MoE verifier and requires its recursive
/// certificate; this codec alone is never an acceptance gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeMoeArtifact {
    pub moe: PearlMoeParams,
    pub routing_data: Vec<u32>,
}

/// Decoded MoE `ai-pow-nonce`: the dense-framed Pearl statement + aux inclusion,
/// plus the MoE tail and `routing_data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeAiPowNonceMoeShape {
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub moe: PearlMergeMoeArtifact,
}

/// Expert count `e` from a 164-byte Pearl core `public_data`, via a canonical
/// `MiningConfiguration` parse of the leading 52 bytes (which enforces the
/// trailer rules: reserved-zero, `top_k == 0` when `e == 0`). Returns `0` for a
/// dense (non-MoE) core.
fn moe_expert_count_from_core(
    core: &[u8; PEARL_PUBLIC_PROOF_PARAMS_SIZE],
) -> Result<usize, CertificateNounError> {
    let cfg = PearlMiningConfig::from_bytes(&core[0..PEARL_MINING_CONFIG_SIZE])?;
    Ok(cfg.moe().map_or(0, |m| m.e as usize))
}

/// Encode a MoE (GROUPED_GEMM) `ai-pow-nonce`.
///
/// Additive to [`encode_pearl_merge_ai_pow_nonce`]: the dense statement + aux
/// framing is produced verbatim (so the 164-byte core, aux commitment, and aux
/// inclusion are byte-identical to the dense path), the envelope is retagged with
/// [`AI_POW_NONCE_MAGIC_MOE`], and the Pearl MoE tail
/// (`expert_idx ‖ routing_offsets ‖ hash_routing ‖ outer_count ‖ outer_indices`,
/// matching `PearlPublicProofParams::to_wire_bytes_moe`) plus the natively-bound
/// `routing_data` block (`len(u32 LE) ‖ data[u32 LE]`) are appended.
///
/// `statement.public_data` must be a GROUPED_GEMM core (`e > 0`) with
/// `e == moe.moe.routing_offsets.len()`. All Pearl caps (`e ≤ 1024`,
/// `outer ≤ 128`) and the [`PEARL_MOE_MAX_ROUTING_ENTRIES`] DoS cap are enforced.
pub(crate) fn encode_pearl_merge_ai_pow_nonce_moe(
    statement: &PearlMergePublicStatementShape,
    aux_inclusion: &PearlAuxInclusionProof,
    moe: &PearlMergeMoeArtifact,
) -> Result<Vec<u8>, CertificateNounError> {
    let e = moe_expert_count_from_core(&statement.public_data)?;
    if e == 0 {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce core is not GROUPED_GEMM (e == 0)",
        ));
    }
    if e > PEARL_MOE_MAX_NUM_EXPERTS {
        return Err(CertificateNounError::LimitExceeded("ai-pow MoE experts"));
    }
    if moe.moe.routing_offsets.len() != e {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE routing_offsets length must equal expert count e",
        ));
    }
    if (moe.moe.expert_idx as usize) >= e {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE expert_idx out of range",
        ));
    }
    if moe.moe.outer_indices.len() > PEARL_MOE_MAX_OUTER_INDICES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE outer indices",
        ));
    }
    if moe.routing_data.len() > PEARL_MOE_MAX_ROUTING_ENTRIES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE routing_data",
        ));
    }

    // Reuse the exact dense framing, then retag the magic — this guarantees the
    // statement/aux bytes are identical to the dense encoding.
    let mut out = encode_pearl_merge_ai_pow_nonce(statement, aux_inclusion)?;
    out[0..4].copy_from_slice(&AI_POW_NONCE_MAGIC_MOE);

    // Pearl MoE tail (mirror of `PublicProofParams::to_wire_bytes_moe`).
    out.extend_from_slice(&moe.moe.expert_idx.to_le_bytes());
    for off in &moe.moe.routing_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    out.extend_from_slice(&moe.moe.hash_routing);
    out.push(moe.moe.outer_indices.len() as u8);
    for idx in &moe.moe.outer_indices {
        out.extend_from_slice(&idx.to_le_bytes());
    }

    // Natively-bound routing_data block.
    out.extend_from_slice(&(moe.routing_data.len() as u32).to_le_bytes());
    for token in &moe.routing_data {
        out.extend_from_slice(&token.to_le_bytes());
    }
    Ok(out)
}

/// Decode a MoE `ai-pow-nonce` produced by [`encode_pearl_merge_ai_pow_nonce_moe`].
///
/// Every read is length-checked before indexing and every count is capped before
/// allocation, so a crafted nonce cannot over-allocate or index out of bounds.
/// The final length must match exactly (no trailing bytes).
pub(crate) fn decode_pearl_merge_ai_pow_nonce_moe(
    nonce: &[u8],
) -> Result<PearlMergeAiPowNonceMoeShape, CertificateNounError> {
    if nonce.len() > AI_POW_NONCE_MOE_MAX_SIZE {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE nonce bytes",
        ));
    }
    if nonce.len() < 4 + 2 + 4 + 1 {
        return Err(CertificateNounError::Shape("ai-pow MoE nonce is too short"));
    }
    if nonce[0..4] != AI_POW_NONCE_MAGIC_MOE {
        return Err(CertificateNounError::Shape("ai-pow MoE nonce magic"));
    }

    // --- dense framing: statement ---
    let mut offset = 4usize;
    let statement_len = u16::from_le_bytes(
        nonce[offset..offset + 2]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    ) as usize;
    offset += 2;
    let statement_end =
        offset
            .checked_add(statement_len)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow MoE nonce statement bytes",
            ))?;
    if statement_end > nonce.len() {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce statement length",
        ));
    }
    let statement = PearlMergePublicStatementShape::from_wire_bytes(&nonce[offset..statement_end])?;
    offset = statement_end;

    // --- dense framing: coinbase ---
    if nonce.len().saturating_sub(offset) < 4 + 1 {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce coinbase length",
        ));
    }
    let coinbase_len = u32::from_le_bytes(
        nonce[offset..offset + 4]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    ) as usize;
    offset += 4;
    if coinbase_len > PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE nonce coinbase bytes",
        ));
    }
    let coinbase_end =
        offset
            .checked_add(coinbase_len)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow MoE nonce coinbase bytes",
            ))?;
    if coinbase_end > nonce.len() {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce coinbase length",
        ));
    }
    let coinbase_tx = nonce[offset..coinbase_end].to_vec();
    offset = coinbase_end;

    // --- dense framing: merkle branch ---
    let Some(&branch_len_byte) = nonce.get(offset) else {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce branch length",
        ));
    };
    let branch_len = branch_len_byte as usize;
    offset += 1;
    if branch_len > PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE nonce merkle branch",
        ));
    }
    let branch_bytes = branch_len
        .checked_mul(32)
        .ok_or(CertificateNounError::LimitExceeded(
            "ai-pow MoE nonce merkle branch",
        ))?;
    let branch_end =
        offset
            .checked_add(branch_bytes)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow MoE nonce merkle branch",
            ))?;
    if branch_end > nonce.len() {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce branch length",
        ));
    }
    let mut merkle_branch = Vec::with_capacity(branch_len);
    for chunk in nonce[offset..branch_end].chunks_exact(32) {
        merkle_branch.push(chunk.try_into().expect("chunk length"));
    }
    offset = branch_end;

    // --- MoE tail (expert count comes from the canonical core trailer) ---
    let e = moe_expert_count_from_core(&statement.public_data)?;
    if e == 0 {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce core is not GROUPED_GEMM (e == 0)",
        ));
    }
    if e > PEARL_MOE_MAX_NUM_EXPERTS {
        return Err(CertificateNounError::LimitExceeded("ai-pow MoE experts"));
    }
    let tail_fixed = 2usize
        + e.checked_mul(4)
            .ok_or(CertificateNounError::LimitExceeded("ai-pow MoE experts"))?
        + 32
        + 1;
    if nonce.len().saturating_sub(offset) < tail_fixed {
        return Err(CertificateNounError::Shape("ai-pow MoE nonce tail"));
    }
    let expert_idx = u16::from_le_bytes(
        nonce[offset..offset + 2]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    );
    offset += 2;
    if (expert_idx as usize) >= e {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE expert_idx out of range",
        ));
    }
    let mut routing_offsets = Vec::with_capacity(e);
    for _ in 0..e {
        routing_offsets.push(u32::from_le_bytes(
            nonce[offset..offset + 4]
                .try_into()
                .expect("fixed-width field; buffer length checked above"),
        ));
        offset += 4;
    }
    let hash_routing: [u8; 32] = nonce[offset..offset + 32]
        .try_into()
        .expect("fixed-width field; buffer length checked above");
    offset += 32;
    let outer_count = nonce[offset] as usize;
    offset += 1;
    if outer_count > PEARL_MOE_MAX_OUTER_INDICES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE outer indices",
        ));
    }
    let outer_bytes = outer_count
        .checked_mul(4)
        .ok_or(CertificateNounError::LimitExceeded(
            "ai-pow MoE outer indices",
        ))?;

    // --- outer_indices + routing_data block ---
    // Remaining must hold outer_indices(4·oc) + routing_len(4) at minimum.
    if nonce.len().saturating_sub(offset) < outer_bytes + 4 {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce outer/routing length",
        ));
    }
    let mut outer_indices = Vec::with_capacity(outer_count);
    for _ in 0..outer_count {
        outer_indices.push(u32::from_le_bytes(
            nonce[offset..offset + 4]
                .try_into()
                .expect("fixed-width field; buffer length checked above"),
        ));
        offset += 4;
    }
    let routing_len = u32::from_le_bytes(
        nonce[offset..offset + 4]
            .try_into()
            .expect("fixed-width field; buffer length checked above"),
    ) as usize;
    offset += 4;
    if routing_len > PEARL_MOE_MAX_ROUTING_ENTRIES {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow MoE routing_data",
        ));
    }
    let routing_bytes = routing_len
        .checked_mul(4)
        .ok_or(CertificateNounError::LimitExceeded(
            "ai-pow MoE routing_data",
        ))?;
    let routing_end =
        offset
            .checked_add(routing_bytes)
            .ok_or(CertificateNounError::LimitExceeded(
                "ai-pow MoE routing_data",
            ))?;
    if routing_end != nonce.len() {
        return Err(CertificateNounError::Shape(
            "ai-pow MoE nonce trailing bytes",
        ));
    }
    let mut routing_data = Vec::with_capacity(routing_len);
    for chunk in nonce[offset..routing_end].chunks_exact(4) {
        routing_data.push(u32::from_le_bytes(chunk.try_into().expect("chunk length")));
    }

    Ok(PearlMergeAiPowNonceMoeShape {
        statement,
        aux_inclusion: PearlAuxInclusionProof {
            coinbase_tx,
            merkle_branch,
        },
        moe: PearlMergeMoeArtifact {
            moe: PearlMoeParams {
                expert_idx,
                routing_offsets,
                hash_routing,
                outer_indices,
            },
            routing_data,
        },
    })
}

/// Decoded canonical `%ai-pow` block artifact after Rust parses the opaque
/// nonce.
///
/// This carries a structured Pearl-compatible public statement and the
/// Nockchain-native recursive certificate. It intentionally does not carry a
/// Pearl ZKP or a raw Layer-0 proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeAiPowArtifactShape {
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub certificate: AiPowCertificateShape,
    /// GROUPED_GEMM (MoE) tail from an `AIM1` nonce: `PearlMoeParams` +
    /// `routing_data`. `None` for a dense (`AIP1`) artifact. The live MoE
    /// verification branch uses this data to bind routing consistency, the
    /// expert-local opened schedule, and the canonical program commitment.
    pub moe: Option<PearlMergeMoeArtifact>,
}

/// Metadata-only view of a Pearl-format-compatible `%ai-pow` artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeAiPowArtifactMetadataShape {
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
    pub certificate: AiPowCertificateMetadata,
}

/// Trusted verifier context for a Nockchain-side Pearl-format `%ai-pow`
/// artifact.
///
/// These values must come from consensus state or verifier configuration, not
/// from the miner-controlled proof artifact. Keeping them in one struct makes
/// the future verifier integration contract explicit and avoids argument-order
/// mistakes at the trust boundary. Hoon does not call the verifier helpers in
/// the current milestone.
#[derive(Debug, Clone, Copy)]
pub struct PearlMergeAiPowVerifierContext<'a> {
    pub candidate_nock_block_commitment: &'a [u8; 32],
    pub a_row_major: &'a [i8],
    pub b_col_major: &'a [i8],
    pub nockchain_target: &'a [u8; 32],
    pub max_pattern_len: usize,
}

/// Rust-owned contents of the opaque Hoon `ai-pow-nonce`.
///
/// Hoon deliberately sees only `[len data]`; Pearl-format names and parsing
/// rules stay in Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeAiPowNonceShape {
    pub statement: PearlMergePublicStatementShape,
    pub aux_inclusion: PearlAuxInclusionProof,
}

/// Canonical recursive certificate metadata derived from one successful
/// Pearl-compatible ticket attempt.
///
/// The producer-side `%ai-pow` builders use this shape to avoid accepting
/// caller-supplied recursive metadata that can drift from the Pearl statement.
/// This is metadata only; public production artifact construction still
/// requires an [`AiPowRecursiveCertificateRun`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeRecursiveCertificateParts {
    pub statement: PearlMergePublicStatementShape,
    pub zk_params: ZkParams,
    pub found_idx: u32,
    pub strip_schedule: StripIndexSchedule,
    pub trace_height: usize,
    pub commitments: ZkPublicCommitments,
    pub public_inputs: CompositePublicInputs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiPowCertificateMetadata {
    pub version: u64,
    pub zk_params: ZkParams,
    pub found_idx: u32,
    pub trace_height: usize,
    pub commitments: ZkPublicCommitments,
    pub public_inputs: CompositePublicInputs,
}

/// Generic Hoon-compatible tree used for the recursive certificate internals.
///
/// Homogeneous integer vectors are packed into atoms so the real recursive
/// proof remains much closer to the compact certificate size than a scalar
/// list encoding. This is still a structured noun: field boundaries and
/// recursive proof containers are preserved by the surrounding node tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiProofNode {
    Unit,
    Bool(bool),
    U64(u64),
    I64(i64),
    Ext2([u64; 2]),
    Ext2s(Vec<[u64; 2]>),
    Bytes(Vec<u8>),
    U64s(Vec<u64>),
    I64s(Vec<i64>),
    Seq(Vec<AiProofNode>),
    Map(Vec<(AiProofNode, AiProofNode)>),
    None,
    Some(Box<AiProofNode>),
}

/// Convert a recursive certificate into the generic proof-node tree.
pub(crate) fn recursive_certificate_to_node<C: Serialize>(
    certificate: &C,
) -> Result<AiProofNode, CertificateNounError> {
    let node = certificate
        .serialize(NodeSerializer)
        .map_err(|e| CertificateNounError::Serialize(e.to_string()))?;
    Ok(node.normalized())
}

/// Convert the compact final-layer batch-STARK certificate into the generic
/// proof-node tree without expanding its internal proof structure.
///
/// The compact route's size target applies to the canonical postcard body.
/// Encoding it as `Bytes` preserves that size at the noun boundary while still
/// letting the Rust verifier reconstruct and canonicalize the typed
/// certificate.
pub(crate) fn compact_recursive_certificate_to_node(
    certificate: &ai_pow_zk::recursion::AiPowCompactBatchRecursiveCertificate,
) -> Result<AiProofNode, CertificateNounError> {
    let bytes = ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(certificate)
        .map_err(|e| CertificateNounError::Serialize(e.to_string()))?;
    if !ai_pow_zk::recursion::compact_batch_recursive_certificate_len_within_limit(bytes.len()) {
        return Err(CertificateNounError::LimitExceeded(
            "compact recursive certificate bytes",
        ));
    }
    Ok(AiProofNode::Bytes(bytes))
}

/// Reconstruct a serde-backed recursive certificate from a decoded proof-node
/// tree.
///
/// This is the inverse of [`recursive_certificate_to_node`]. It exists so the
/// production Rust/Hoon boundary can verify the structured noun artifact
/// directly instead of requiring an adjacent compact byte blob.
#[allow(dead_code)] // used under some feature/target configs only
fn recursive_certificate_from_node<C: DeserializeOwned>(
    node: &AiProofNode,
) -> Result<C, CertificateNounError> {
    C::deserialize(NodeDeserializer { node: node.clone() })
        .map_err(|e| CertificateNounError::Deserialize(e.to_string()))
}

fn validate_proof_node_limits(
    node: &AiProofNode,
    limits: CertificateNounLimits,
) -> Result<(), CertificateNounError> {
    let mut total_nodes = 0usize;
    let mut stack = vec![(node, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > limits.max_depth {
            return Err(CertificateNounError::LimitExceeded("proof-node depth"));
        }
        total_nodes = total_nodes
            .checked_add(1)
            .ok_or(CertificateNounError::LimitExceeded("proof-node count"))?;
        if total_nodes > limits.max_total_nodes {
            return Err(CertificateNounError::LimitExceeded("proof-node count"));
        }

        match node {
            AiProofNode::Unit
            | AiProofNode::Bool(_)
            | AiProofNode::U64(_)
            | AiProofNode::I64(_)
            | AiProofNode::None => {}
            AiProofNode::Ext2(value) => {
                expect_goldilocks(value[0], "ext2.c0")?;
                expect_goldilocks(value[1], "ext2.c1")?;
            }
            AiProofNode::Ext2s(values) => {
                if values.len() > limits.max_packed_items {
                    return Err(CertificateNounError::LimitExceeded("ext2s length"));
                }
                values
                    .len()
                    .checked_mul(16)
                    .filter(|bytes| *bytes <= limits.max_atom_bytes)
                    .ok_or(CertificateNounError::LimitExceeded("atom bytes"))?;
                for value in values {
                    expect_goldilocks(value[0], "ext2s.c0")?;
                    expect_goldilocks(value[1], "ext2s.c1")?;
                }
            }
            AiProofNode::Bytes(bytes) => {
                if bytes.len() > limits.max_packed_items {
                    return Err(CertificateNounError::LimitExceeded("bytes length"));
                }
                if bytes.len() > limits.max_atom_bytes {
                    return Err(CertificateNounError::LimitExceeded("atom bytes"));
                }
            }
            AiProofNode::U64s(values) => {
                if values.len() > limits.max_packed_items {
                    return Err(CertificateNounError::LimitExceeded("u64s length"));
                }
                values
                    .len()
                    .checked_mul(8)
                    .filter(|bytes| *bytes <= limits.max_atom_bytes)
                    .ok_or(CertificateNounError::LimitExceeded("atom bytes"))?;
            }
            AiProofNode::I64s(values) => {
                if values.len() > limits.max_packed_items {
                    return Err(CertificateNounError::LimitExceeded("i64s length"));
                }
                values
                    .len()
                    .checked_mul(8)
                    .filter(|bytes| *bytes <= limits.max_atom_bytes)
                    .ok_or(CertificateNounError::LimitExceeded("atom bytes"))?;
            }
            AiProofNode::Seq(items) => {
                if items.len() > limits.max_list_items {
                    return Err(CertificateNounError::LimitExceeded(
                        "proof-node list length",
                    ));
                }
                let child_depth = depth
                    .checked_add(1)
                    .ok_or(CertificateNounError::LimitExceeded("proof-node depth"))?;
                for item in items {
                    stack.push((item, child_depth));
                }
            }
            AiProofNode::Map(items) => {
                if items.len() > limits.max_list_items {
                    return Err(CertificateNounError::LimitExceeded("proof-node map length"));
                }
                let child_depth = depth
                    .checked_add(1)
                    .ok_or(CertificateNounError::LimitExceeded("proof-node depth"))?;
                for (key, value) in items {
                    stack.push((key, child_depth));
                    stack.push((value, child_depth));
                }
            }
            AiProofNode::Some(inner) => {
                let child_depth = depth
                    .checked_add(1)
                    .ok_or(CertificateNounError::LimitExceeded("proof-node depth"))?;
                stack.push((inner, child_depth));
            }
        }
    }
    Ok(())
}

/// Reconstruct the compact final-layer batch-STARK certificate from a decoded
/// Hoon-compatible proof-node tree.
pub fn ai_pow_compact_recursive_certificate_from_node(
    node: &AiProofNode,
) -> Result<ai_pow_zk::recursion::AiPowCompactBatchRecursiveCertificate, CertificateNounError> {
    ai_pow_compact_recursive_certificate_from_node_with_limits(
        node,
        CertificateNounLimits::default(),
    )
}

/// Reconstruct a compact recursive certificate after enforcing explicit
/// proof-node resource limits and canonical postcard encoding.
pub(crate) fn ai_pow_compact_recursive_certificate_from_node_with_limits(
    node: &AiProofNode,
    limits: CertificateNounLimits,
) -> Result<ai_pow_zk::recursion::AiPowCompactBatchRecursiveCertificate, CertificateNounError> {
    validate_proof_node_limits(node, limits)?;
    let AiProofNode::Bytes(bytes) = node else {
        return Err(CertificateNounError::Shape(
            "compact recursive certificate must be bytes",
        ));
    };
    if !ai_pow_zk::recursion::compact_batch_recursive_certificate_len_within_limit(bytes.len()) {
        return Err(CertificateNounError::LimitExceeded(
            "compact recursive certificate bytes",
        ));
    }
    let certificate = ai_pow_zk::recursion::decode_compact_batch_recursive_certificate(bytes)
        .map_err(|e| CertificateNounError::Deserialize(e.to_string()))?;
    let canonical = ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&certificate)
        .map_err(|e| CertificateNounError::Serialize(e.to_string()))?;
    if canonical != *bytes {
        return Err(CertificateNounError::NonCanonicalProofNode);
    }
    Ok(certificate)
}

/// Build the Hoon `ai-pow-certificate` noun:
///
/// ```hoon
/// [version params found-idx trace-height commitments public-inputs certificate]
///
/// `commitments` serializes only the production verifier inputs
/// `[h-a-chunk h-b-chunk]`. Row/column opening roots are not part of the
/// canonical noun.
/// ```
fn build_ai_pow_certificate_noun<C: Serialize>(
    zk_params: &ZkParams,
    found_idx: u32,
    trace_height: usize,
    commitments: &ZkPublicCommitments,
    pis: &CompositePublicInputs,
    recursive_certificate: &C,
) -> Result<NounSlab, CertificateNounError> {
    let certificate = recursive_certificate_to_node(recursive_certificate)?;
    Ok(build_ai_pow_certificate_noun_from_node(
        zk_params, found_idx, trace_height, commitments, pis, &certificate,
    ))
}

/// Build the Hoon `ai-pow-certificate` noun from a real recursive prover run.
///
/// This is the public production constructor for Nockchain's recursive
/// AI-PoW certificate noun. It deliberately takes the typed prover-run result
/// instead of an arbitrary `Serialize` value. `AiPowRecursiveCertificateRun`
/// has private fields, so downstream callers cannot synthesize a fake run or
/// accidentally package a diagnostic or non-canonical proof object as the block
/// certificate.
#[doc(hidden)]
pub fn build_ai_pow_certificate_noun_from_recursive_run(
    run: &AiPowRecursiveCertificateRun,
) -> Result<NounSlab, CertificateNounError> {
    build_ai_pow_certificate_noun(
        &run.zk_params(),
        run.found_idx(),
        run.trace_height(),
        &run.commitments(),
        run.public_inputs(),
        run.certificate(),
    )
}

/// Build the Hoon `ai-pow-certificate` noun from a compact recursive prover
/// run.
///
/// This is the selected production-proof constructor. It keeps the same
/// verifier-derived metadata envelope as the checkpoint constructor, but stores
/// the compact certificate as canonical bytes so the noun wire artifact remains
/// close to the compact proof size.
pub fn build_ai_pow_certificate_noun_from_compact_recursive_run(
    run: &AiPowCompactRecursiveCertificateRun,
) -> Result<NounSlab, CertificateNounError> {
    let certificate = compact_recursive_certificate_to_node(run.certificate())?;
    Ok(build_ai_pow_certificate_noun_from_node(
        &run.zk_params(),
        run.found_idx(),
        run.trace_height(),
        &run.commitments(),
        run.public_inputs(),
        &certificate,
    ))
}

/// Build the same top-level certificate noun from an already-serialized proof
/// node. This is mainly useful for focused shape tests.
pub fn build_ai_pow_certificate_noun_from_node(
    zk_params: &ZkParams,
    found_idx: u32,
    trace_height: usize,
    commitments: &ZkPublicCommitments,
    pis: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> NounSlab {
    let mut slab = NounSlab::new();
    let root = encode_ai_pow_certificate_noun(
        &mut slab, zk_params, found_idx, trace_height, commitments, pis, certificate,
    );
    slab.set_root(root);
    slab
}

/// Build a Rust/test-only structured statement noun.
///
/// Variable-length aux fields are encoded as `[len data]` pairs so trailing
/// zero bytes are consensus-visible and round-trip into the exact `NPA1` aux
/// byte envelope.
#[cfg(test)]
pub(crate) fn build_pearl_merge_public_statement_noun<A: NounAllocator>(
    allocator: &mut A,
    statement: &PearlMergePublicStatementShape,
) -> Noun {
    let block_header = bytes_to_atom(allocator, &statement.block_header);
    let public_data = bytes_to_atom(allocator, &statement.public_data);
    let expected_aux_commitment = bytes_to_atom(allocator, &statement.expected_aux_commitment);
    let chain_id_data = bytes_to_atom(allocator, &statement.aux.nockchain_chain_id);
    let chain_id = T(
        allocator,
        &[D(statement.aux.nockchain_chain_id.len() as u64), chain_id_data],
    );
    let nock_block = bytes_to_atom(allocator, &statement.aux.nock_block_commitment);
    let extra_data = bytes_to_atom(allocator, &statement.aux.extra_domain_data);
    let extra = T(
        allocator,
        &[D(statement.aux.extra_domain_data.len() as u64), extra_data],
    );
    let aux = T(
        allocator,
        &[chain_id, nock_block, D(statement.aux.nockchain_target_epoch_or_height), extra],
    );
    T(
        allocator,
        &[block_header, public_data, expected_aux_commitment, aux],
    )
}

/// Build a Rust/test-only slab rooted at the structured statement noun.
#[cfg(test)]
pub(crate) fn build_pearl_merge_public_statement_slab(
    statement: &PearlMergePublicStatementShape,
) -> NounSlab {
    let mut slab = NounSlab::new();
    let root = build_pearl_merge_public_statement_noun(&mut slab, statement);
    slab.set_root(root);
    slab
}

fn build_ai_pow_nonce_noun<A: NounAllocator>(allocator: &mut A, nonce: &[u8]) -> Noun {
    let data = bytes_to_atom(allocator, nonce);
    T(allocator, &[D(nonce.len() as u64), data])
}

fn validate_pearl_merge_statement_aux_inclusion(
    statement: &PearlMergePublicStatementShape,
    aux_inclusion: &PearlAuxInclusionProof,
) -> Result<(), CertificateNounError> {
    let header = PearlIncompleteBlockHeader::from_bytes(&statement.block_header)?;
    verify_pearl_aux_inclusion(&header, &statement.expected_aux_commitment, aux_inclusion)?;
    Ok(())
}

/// Build the Hoon `%ai-pow` artifact noun from an already-serialized proof
/// node and pre-derived statement.
///
/// This is intentionally crate-internal: production callers should use the
/// ticket-derived builders so recursive metadata is recomputed from the mined
/// Pearl-compatible attempt instead of supplied independently.
pub(crate) fn build_ai_pow_pearl_merge_artifact_noun_from_node(
    statement: &PearlMergePublicStatementShape,
    aux_inclusion: &PearlAuxInclusionProof,
    zk_params: &ZkParams,
    found_idx: u32,
    trace_height: usize,
    commitments: &ZkPublicCommitments,
    pis: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> Result<NounSlab, CertificateNounError> {
    let nonce = encode_pearl_merge_ai_pow_nonce(statement, aux_inclusion)?;
    validate_pearl_merge_statement_aux_inclusion(statement, aux_inclusion)?;

    let mut slab = NounSlab::new();
    let nonce = build_ai_pow_nonce_noun(&mut slab, &nonce);
    let certificate = encode_ai_pow_certificate_noun(
        &mut slab, zk_params, found_idx, trace_height, commitments, pis, certificate,
    );
    let root = T(&mut slab, &[D(tas!(b"ai-pow")), nonce, certificate]);
    slab.set_root(root);
    Ok(slab)
}

/// Build a **MoE** (`AIM1`) Pearl-merge `%ai-pow` artifact noun (M1 encoder) — the
/// GROUPED_GEMM counterpart of [`build_ai_pow_pearl_merge_artifact_noun_from_node`].
///
/// Identical framing (`[%ai-pow nonce cert]`) except the opaque nonce is the MoE
/// nonce ([`encode_pearl_merge_ai_pow_nonce_moe`]): the dense statement + aux
/// framing verbatim, retagged `AIM1`, with the Pearl MoE tail
/// (`expert_idx ‖ routing_offsets ‖ hash_routing ‖ outer_indices`) + the DoS-capped
/// `routing_data` appended. Decodes back through
/// [`decode_ai_pow_pearl_merge_artifact_noun`] (which dispatches on the tag) into a
/// `PearlMergeAiPowArtifactShape` carrying `moe: Some(..)`, ready for the node MoE
/// verify branch.
pub fn build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
    statement: &PearlMergePublicStatementShape,
    aux_inclusion: &PearlAuxInclusionProof,
    moe: &PearlMergeMoeArtifact,
    zk_params: &ZkParams,
    found_idx: u32,
    trace_height: usize,
    commitments: &ZkPublicCommitments,
    pis: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> Result<NounSlab, CertificateNounError> {
    let nonce = encode_pearl_merge_ai_pow_nonce_moe(statement, aux_inclusion, moe)?;
    validate_pearl_merge_statement_aux_inclusion(statement, aux_inclusion)?;

    let mut slab = NounSlab::new();
    let nonce = build_ai_pow_nonce_noun(&mut slab, &nonce);
    let certificate = encode_ai_pow_certificate_noun(
        &mut slab, zk_params, found_idx, trace_height, commitments, pis, certificate,
    );
    let root = T(&mut slab, &[D(tas!(b"ai-pow")), nonce, certificate]);
    slab.set_root(root);
    Ok(slab)
}

/// Derive the exact `%ai-pow` recursive metadata for one successful
/// Pearl-compatible ticket attempt.
///
/// This is the producer-side canonicalization boundary. It rejects non-winning
/// tickets, forged statement/public-param drift, forged public ticket work, and
/// Pearl geometries outside the current recursive parameter envelope. The
/// recursive statement binds the explicit Pearl ticket schedule, not a native
/// verifier-selected full-matmul attempt. This helper does not build a block
/// artifact and does not accept proof material; the public production artifact
/// builder requires a real [`AiPowCompactRecursiveCertificateRun`].
pub(crate) fn pearl_merge_recursive_certificate_parts_from_ticket(
    attempt: &PearlMergeTicketAttempt,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
) -> Result<PearlMergeRecursiveCertificateParts, CertificateNounError> {
    attempt
        .public_params
        .check_nockchain_jackpot_target(&attempt.nockchain_target)?;

    let statement = PearlMergePublicStatementShape::from_wire_statement(&attempt.statement)?;
    if statement.block_header != attempt.public_params.block_header.to_bytes() {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.statement.block-header",
        ));
    }
    if statement.public_data != attempt.public_params.to_public_data()? {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.statement.public-data",
        ));
    }
    if statement.expected_aux_commitment != attempt.aux_commitment {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.statement.expected-aux-commitment",
        ));
    }
    if statement.aux != attempt.aux {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.statement.aux",
        ));
    }

    let precheck = verify_pearl_merge_public_statement_bytes(
        &attempt.aux.nock_block_commitment,
        &attempt.statement.to_bytes()?,
        a_row_major,
        b_col_major,
        &attempt.nockchain_target,
        max_pattern_len,
    )?;
    if precheck.work.commitments != attempt.commitments {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.commitments",
        ));
    }
    if precheck.work.ticket != attempt.ticket {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.work",
        ));
    }
    if precheck.work.pearl_target != attempt.pearl_target {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.pearl-target",
        ));
    }
    if precheck.work.nockchain_target != attempt.nockchain_target {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.nockchain-target",
        ));
    }
    if precheck.aux_commitment != attempt.aux_commitment {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.aux-commitment",
        ));
    }

    if attempt.public_params.hash_a != attempt.commitments.h_a {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.public.hash-a",
        ));
    }
    if attempt.public_params.hash_b != attempt.commitments.h_b {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.public.hash-b",
        ));
    }
    if attempt.public_params.hash_jackpot != attempt.ticket.jackpot_hash {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.public.hash-jackpot",
        ));
    }

    let expected_rows = attempt
        .public_params
        .a_rows_indices_bounded(max_pattern_len)?;
    let expected_cols = attempt
        .public_params
        .b_cols_indices_bounded(max_pattern_len)?;
    if attempt.ticket.a_rows != expected_rows || attempt.ticket.b_cols != expected_cols {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "ticket.pattern-indices",
        ));
    }
    let h = attempt.public_params.h()?;
    let params = MatmulParams {
        m: attempt.public_params.m,
        k: attempt.public_params.mining_config.common_dim,
        n: attempt.public_params.n,
        noise_rank: u32::from(attempt.public_params.mining_config.rank),
        tile: h,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    validate_pearl_merge_recursive_params(&params)?;
    let zk_params = zk_params_from_matmul(&params);
    let strip_schedule = StripIndexSchedule::from_indices(&zk_params, expected_rows, expected_cols)
        .map_err(|_| CertificateNounError::PearlMergeUnsupportedTileShape)?;
    let found_idx = pearl_merge_legacy_found_idx(&attempt.public_params, &params).unwrap_or(0);
    let trace_height = expected_layer0_rows_for_strip_schedule(&params, &strip_schedule)
        .map_err(|_| CertificateNounError::PearlMergeUnsupportedTileShape)?
        .required_trace_len();

    Ok(PearlMergeRecursiveCertificateParts {
        statement,
        zk_params,
        found_idx,
        strip_schedule,
        trace_height,
        commitments: ZkPublicCommitments {
            h_a_chunk: attempt.commitments.h_a,
            h_b_chunk: attempt.commitments.h_b,
        },
        public_inputs: pearl_merge_recursive_public_inputs_from_work(
            &attempt.commitments, &attempt.ticket,
        ),
    })
}

/// Derive the exact `%ai-pow` recursive metadata for one successful
/// Pearl-compatible ticket attempt, preserving the public inputs produced by
/// the actual recursive prover run.
///
/// The Pearl-bound slots are still fully re-derived from the ticket and
/// trusted matrices. The only field this API does not derive is `cumsum`,
/// because that is a Layer-0 trace detail rather than part of Pearl's public
/// work statement. This is still a metadata helper, not a public artifact
/// constructor; block submission uses the compact recursive-run builder.
pub(crate) fn pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
    attempt: &PearlMergeTicketAttempt,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    public_inputs: &CompositePublicInputs,
) -> Result<PearlMergeRecursiveCertificateParts, CertificateNounError> {
    let mut parts = pearl_merge_recursive_certificate_parts_from_ticket(
        attempt, a_row_major, b_col_major, max_pattern_len,
    )?;
    // Miner producer path: it holds the matrices and built the tile, so bind the
    // full pis including the raw `jackpot` tile PI.
    precheck_pearl_merge_bound_public_inputs(public_inputs, &parts.public_inputs, true)?;
    parts.public_inputs = public_inputs.clone();
    Ok(parts)
}

fn validate_pearl_merge_ticket_aux_inclusion(
    parts: &PearlMergeRecursiveCertificateParts,
    aux_inclusion: &PearlAuxInclusionProof,
) -> Result<(), CertificateNounError> {
    validate_pearl_merge_statement_aux_inclusion(&parts.statement, aux_inclusion)
}

/// Test-only canonical `%ai-pow` artifact builder from a successful
/// Pearl-compatible ticket and an already-serialized recursive proof node.
///
/// Production callers use
/// [`build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run`].
#[cfg(test)]
pub(crate) fn build_ai_pow_pearl_merge_artifact_noun_from_ticket_node(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    certificate: &AiProofNode,
) -> Result<NounSlab, CertificateNounError> {
    let parts = pearl_merge_recursive_certificate_parts_from_ticket(
        attempt, a_row_major, b_col_major, max_pattern_len,
    )?;
    validate_pearl_merge_ticket_aux_inclusion(&parts, aux_inclusion)?;
    build_ai_pow_pearl_merge_artifact_noun_from_node(
        &parts.statement, aux_inclusion, &parts.zk_params, parts.found_idx, parts.trace_height,
        &parts.commitments, &parts.public_inputs, certificate,
    )
}

/// Crate-internal canonical `%ai-pow` artifact builder used after a
/// private-field certificate builder has supplied recursive public inputs and a
/// serialized proof node. External callers should use the recursive-run API.
pub(crate) fn build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    public_inputs: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> Result<NounSlab, CertificateNounError> {
    let parts = pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
        attempt, a_row_major, b_col_major, max_pattern_len, public_inputs,
    )?;
    validate_pearl_merge_ticket_aux_inclusion(&parts, aux_inclusion)?;
    build_ai_pow_pearl_merge_artifact_noun_from_node(
        &parts.statement, aux_inclusion, &parts.zk_params, parts.found_idx, parts.trace_height,
        &parts.commitments, &parts.public_inputs, certificate,
    )
}

/// Build the canonical `%ai-pow` artifact from a successful Pearl-compatible
/// ticket and a real recursive prover run.
#[doc(hidden)]
pub(crate) fn build_ai_pow_pearl_merge_artifact_noun_from_ticket_recursive_run(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    run: &AiPowRecursiveCertificateRun,
) -> Result<NounSlab, CertificateNounError> {
    let parts = pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
        attempt,
        a_row_major,
        b_col_major,
        max_pattern_len,
        run.public_inputs(),
    )?;
    validate_pearl_merge_ticket_aux_inclusion(&parts, aux_inclusion)?;
    if run.zk_params() != parts.zk_params {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.zk-params",
        ));
    }
    if run.found_idx() != parts.found_idx {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.found-idx",
        ));
    }
    if run.strip_schedule() != &parts.strip_schedule {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.strip-schedule",
        ));
    }
    if run.trace_height() != parts.trace_height {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.trace-height",
        ));
    }
    if run.commitments() != parts.commitments {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.commitments",
        ));
    }
    let certificate = recursive_certificate_to_node(run.certificate())?;
    build_ai_pow_pearl_merge_artifact_noun_from_node(
        &parts.statement, aux_inclusion, &parts.zk_params, parts.found_idx, parts.trace_height,
        &parts.commitments, &parts.public_inputs, &certificate,
    )
}

/// Build the canonical `%ai-pow` artifact from a successful Pearl-compatible
/// ticket and a real compact recursive prover run.
pub(crate) fn build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
    attempt: &PearlMergeTicketAttempt,
    aux_inclusion: &PearlAuxInclusionProof,
    a_row_major: &[i8],
    b_col_major: &[i8],
    max_pattern_len: usize,
    run: &AiPowCompactRecursiveCertificateRun,
) -> Result<NounSlab, CertificateNounError> {
    let parts = pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
        attempt,
        a_row_major,
        b_col_major,
        max_pattern_len,
        run.public_inputs(),
    )?;
    validate_pearl_merge_ticket_aux_inclusion(&parts, aux_inclusion)?;
    if run.zk_params() != parts.zk_params {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.zk-params",
        ));
    }
    if run.found_idx() != parts.found_idx {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.found-idx",
        ));
    }
    if run.strip_schedule() != &parts.strip_schedule {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.strip-schedule",
        ));
    }
    if run.trace_height() != parts.trace_height {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.trace-height",
        ));
    }
    if run.commitments() != parts.commitments {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "recursive-run.commitments",
        ));
    }
    let certificate = compact_recursive_certificate_to_node(run.certificate())?;
    build_ai_pow_pearl_merge_artifact_noun_from_node(
        &parts.statement, aux_inclusion, &parts.zk_params, parts.found_idx, parts.trace_height,
        &parts.commitments, &parts.public_inputs, &certificate,
    )
}

/// Decode and validate the Hoon `ai-pow-certificate` root in a slab.
pub fn decode_ai_pow_certificate_slab<J>(
    slab: &NounSlab<J>,
    limits: CertificateNounLimits,
) -> Result<AiPowCertificateShape, CertificateNounError> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    decode_ai_pow_certificate_noun(root, &space, limits)
}

/// Decode and validate a Hoon `ai-pow-certificate` noun.
///
/// This parser enforces the production shape and bounded proof-node recursion.
/// It does not verify the recursive ZKP itself.
pub(crate) fn decode_ai_pow_certificate_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<AiPowCertificateShape, CertificateNounError> {
    let fields = tuple7(root, space, "ai-pow-certificate")?;
    let metadata = decode_ai_pow_certificate_metadata_fields(&fields, space, limits)?;
    let mut state = DecodeState::new(limits);
    Ok(AiPowCertificateShape {
        version: metadata.version,
        zk_params: metadata.zk_params,
        found_idx: metadata.found_idx,
        trace_height: metadata.trace_height,
        commitments: metadata.commitments,
        public_inputs: metadata.public_inputs,
        certificate: decode_proof_node(fields[6], space, &mut state, 0)?,
    })
}

fn decode_ai_pow_certificate_metadata_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<AiPowCertificateMetadata, CertificateNounError> {
    let fields = tuple7(root, space, "ai-pow-certificate")?;
    decode_ai_pow_certificate_metadata_fields(&fields, space, limits)
}

fn decode_ai_pow_certificate_metadata_fields(
    fields: &[Noun; 7],
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<AiPowCertificateMetadata, CertificateNounError> {
    let version = expect_u64(fields[0], space, "version")?;
    if version != AI_POW_CERT_VERSION {
        return Err(CertificateNounError::UnsupportedVersion(version));
    }
    Ok(AiPowCertificateMetadata {
        version,
        zk_params: decode_params(fields[1], space)?,
        found_idx: u32::try_from(expect_u64(fields[2], space, "found-idx")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "found-idx" })?,
        trace_height: usize::try_from(expect_u64(fields[3], space, "trace-height")?).map_err(
            |_| CertificateNounError::IntegerOutOfRange {
                field: "trace-height",
            },
        )?,
        commitments: decode_commitments(fields[4], space, limits)?,
        public_inputs: decode_public_inputs(fields[5], space, limits)?,
    })
}

fn cue_canonical_artifact_jam(
    jammed: &[u8],
    limits: CertificateNounLimits,
) -> Result<NounSlab, CertificateNounError> {
    if jammed.is_empty() {
        return Err(CertificateNounError::Cue(CueError::TruncatedBuffer));
    }
    if jammed.len() > limits.max_jam_bytes {
        return Err(CertificateNounError::JammedLengthExceeded {
            limit: limits.max_jam_bytes,
            actual: jammed.len(),
        });
    }
    preflight_ai_pow_artifact_jam(jammed, limits)?;

    let mut slab: NounSlab = NounSlab::new();
    let root = catch_unwind(AssertUnwindSafe(|| {
        slab.cue_into(Bytes::copy_from_slice(jammed))
    }))
    .map_err(|_| CertificateNounError::CuePanic)??;
    slab.set_root(root);
    if slab.jam().as_ref() != jammed {
        return Err(CertificateNounError::NonCanonicalJam);
    }
    Ok(slab)
}

fn preflight_ai_pow_artifact_jam(
    jammed: &[u8],
    limits: CertificateNounLimits,
) -> Result<(), CertificateNounError> {
    let total_bits = jammed
        .len()
        .checked_mul(8)
        .ok_or(CertificateNounError::LimitExceeded("jam bytes"))?;
    let mut cursor = 0usize;
    let mut total_nodes = 0usize;
    let mut stack = vec![1usize];

    while let Some(depth) = stack.pop() {
        if depth > limits.max_depth {
            return Err(CertificateNounError::LimitExceeded("jam noun depth"));
        }
        total_nodes = total_nodes
            .checked_add(1)
            .ok_or(CertificateNounError::LimitExceeded("jam noun count"))?;
        if total_nodes > limits.max_total_nodes {
            return Err(CertificateNounError::LimitExceeded("jam noun count"));
        }

        let first = bit_at(jammed, cursor).ok_or(CueError::TruncatedBuffer)?;
        cursor += 1;
        if first {
            let second = bit_at(jammed, cursor).ok_or(CueError::TruncatedBuffer)?;
            cursor += 1;
            if second {
                let backref = rub_usize(&mut cursor, jammed, total_bits, "jam backref")?;
                if backref >= cursor {
                    return Err(CertificateNounError::Cue(CueError::BadBackref));
                }
            } else {
                let child_depth = depth
                    .checked_add(1)
                    .ok_or(CertificateNounError::LimitExceeded("jam noun depth"))?;
                stack.push(child_depth);
                stack.push(child_depth);
            }
        } else {
            let atom_bits = rub_usize(&mut cursor, jammed, total_bits, "jam atom size")?;
            let atom_bytes = atom_bits.saturating_add(7) / 8;
            if atom_bytes > limits.max_atom_bytes {
                return Err(CertificateNounError::LimitExceeded("jam atom bytes"));
            }
            cursor = cursor
                .checked_add(atom_bits)
                .ok_or(CertificateNounError::LimitExceeded("jam atom size"))?;
            if cursor > total_bits {
                return Err(CertificateNounError::Cue(CueError::TruncatedBuffer));
            }
        }
    }

    Ok(())
}

fn rub_usize(
    cursor: &mut usize,
    jammed: &[u8],
    total_bits: usize,
    field: &'static str,
) -> Result<usize, CertificateNounError> {
    let Some(idx) = first_one(jammed, *cursor, total_bits) else {
        return Err(CertificateNounError::Cue(CueError::TruncatedBuffer));
    };
    if idx == 0 {
        *cursor = (*cursor)
            .checked_add(1)
            .ok_or(CertificateNounError::LimitExceeded(field))?;
        return Ok(0);
    }

    let size_bits = idx - 1;
    if size_bits >= usize::BITS as usize {
        return Err(CertificateNounError::LimitExceeded(field));
    }
    *cursor = (*cursor)
        .checked_add(idx + 1)
        .ok_or(CertificateNounError::LimitExceeded(field))?;
    if total_bits < (*cursor).saturating_add(size_bits) {
        return Err(CertificateNounError::Cue(CueError::TruncatedBuffer));
    }

    let mut value = 1usize << size_bits;
    for bit in 0..size_bits {
        if bit_at(jammed, *cursor + bit).ok_or(CueError::TruncatedBuffer)? {
            value |= 1usize << bit;
        }
    }
    *cursor = (*cursor)
        .checked_add(size_bits)
        .ok_or(CertificateNounError::LimitExceeded(field))?;
    Ok(value)
}

fn first_one(jammed: &[u8], start: usize, total_bits: usize) -> Option<usize> {
    let mut offset = 0usize;
    let mut cursor = start;
    while cursor < total_bits {
        if bit_at(jammed, cursor)? {
            return Some(offset);
        }
        offset += 1;
        cursor += 1;
    }
    None
}

fn bit_at(jammed: &[u8], bit: usize) -> Option<bool> {
    let byte = *jammed.get(bit / 8)?;
    Some(((byte >> (bit % 8)) & 1) == 1)
}

/// Decode only the top-level generic `%ai-pow` nonce and certificate metadata.
///
/// This does not traverse the recursive proof-node tail.
fn decode_ai_pow_artifact_metadata_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<AiPowArtifactMetadataShape, CertificateNounError> {
    let fields = tuple3(root, space, "ai-pow artifact")?;
    let tag = expect_u64(fields[0], space, "ai-pow artifact tag")?;
    if tag != tas!(b"ai-pow") {
        return Err(CertificateNounError::Shape("expected %ai-pow artifact"));
    }
    let nonce = expect_declared_bounded_bytes(
        fields[1], space, 1, AI_POW_NONCE_MOE_MAX_SIZE, "ai-pow nonce", limits,
    )?;
    Ok(AiPowArtifactMetadataShape {
        nonce,
        certificate: decode_ai_pow_certificate_metadata_noun(fields[2], space, limits)?,
    })
}

/// Decode and validate a Rust/test-only structured statement noun.
///
/// Expected noun shape:
///
/// ```hoon
/// [block-header public-data expected-aux-commitment
///  [[chain-id-len chain-id-data] nock-block-commitment
///   target-epoch-or-height [extra-len extra-data]]]
/// ```
#[cfg(test)]
pub(crate) fn decode_pearl_merge_public_statement_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<PearlMergePublicStatementShape, CertificateNounError> {
    let fields = tuple4(root, space, "pearl-merge-public-statement")?;
    let aux_fields = tuple4(fields[3], space, "pearl-nockchain-aux")?;
    Ok(PearlMergePublicStatementShape {
        block_header: expect_fixed_bytes(fields[0], space, "pearl-merge.block-header", limits)?,
        public_data: expect_fixed_bytes(fields[1], space, "pearl-merge.public-data", limits)?,
        expected_aux_commitment: expect_fixed_bytes(
            fields[2], space, "pearl-merge.expected-aux-commitment", limits,
        )?,
        aux: PearlNockchainAux {
            nockchain_chain_id: expect_declared_bounded_bytes(
                aux_fields[0], space, 1, PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX,
                "pearl-merge.aux.chain-id", limits,
            )?,
            nock_block_commitment: expect_fixed_bytes(
                aux_fields[1], space, "pearl-merge.aux.nock-block-commitment", limits,
            )?,
            nockchain_target_epoch_or_height: expect_u64(
                aux_fields[2], space, "pearl-merge.aux.target-epoch-or-height",
            )?,
            extra_domain_data: expect_declared_bounded_bytes(
                aux_fields[3], space, 0, PEARL_NOCKCHAIN_AUX_EXTRA_MAX,
                "pearl-merge.aux.extra-domain-data", limits,
            )?,
        },
    })
}

/// Decode and validate a full Hoon `%ai-pow` block artifact as the
/// Pearl-format-compatible Nockchain submission artifact.
///
/// Expected noun shape:
///
/// ```hoon
/// [%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]
/// ```
pub fn decode_ai_pow_pearl_merge_artifact_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactShape, CertificateNounError> {
    let fields = tuple3(root, space, "ai-pow artifact")?;
    let tag = expect_u64(fields[0], space, "ai-pow artifact tag")?;
    if tag != tas!(b"ai-pow") {
        return Err(CertificateNounError::Shape("expected %ai-pow artifact"));
    }
    let nonce = expect_declared_bounded_bytes(
        fields[1], space, 1, AI_POW_NONCE_MOE_MAX_SIZE, "ai-pow nonce", limits,
    )?;
    // Dispatch on the nonce tag: `AIM1` (MoE) carries the routing tail, `AIP1`
    // (dense) does not. The dense decoder rejects an `AIM1` nonce on the magic, so
    // this branch is the only way MoE data reaches the node.
    let (statement, aux_inclusion, moe) =
        if nonce.len() >= 4 && nonce[0..4] == AI_POW_NONCE_MAGIC_MOE {
            let parsed = decode_pearl_merge_ai_pow_nonce_moe(&nonce)?;
            (parsed.statement, parsed.aux_inclusion, Some(parsed.moe))
        } else {
            let parsed = decode_pearl_merge_ai_pow_nonce(&nonce)?;
            (parsed.statement, parsed.aux_inclusion, None)
        };
    Ok(PearlMergeAiPowArtifactShape {
        statement,
        aux_inclusion,
        certificate: decode_ai_pow_certificate_noun(fields[2], space, limits)?,
        moe,
    })
}

/// Decode and validate the Pearl-format-compatible `%ai-pow` nonce and
/// certificate metadata only.
///
/// This is the cheap-rejection boundary for Nockchain submission and
/// consensus prechecks: it parses the Rust-owned `AIP1` nonce and certificate
/// statement metadata without reconstructing the recursive proof-node tail.
pub(crate) fn decode_ai_pow_pearl_merge_artifact_metadata_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactMetadataShape, CertificateNounError> {
    let metadata = decode_ai_pow_artifact_metadata_noun(root, space, limits)?;
    let parsed_nonce = decode_pearl_merge_ai_pow_nonce(&metadata.nonce)?;
    Ok(PearlMergeAiPowArtifactMetadataShape {
        statement: parsed_nonce.statement,
        aux_inclusion: parsed_nonce.aux_inclusion,
        certificate: metadata.certificate,
    })
}

/// Decode and validate the Pearl-format-compatible `%ai-pow` nonce and
/// certificate metadata only in a slab.
pub(crate) fn decode_ai_pow_pearl_merge_artifact_metadata_slab<J>(
    slab: &NounSlab<J>,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactMetadataShape, CertificateNounError> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    decode_ai_pow_pearl_merge_artifact_metadata_noun(root, &space, limits)
}

/// Decode and validate a full Hoon `%ai-pow` block artifact slab.
pub fn decode_ai_pow_pearl_merge_artifact_slab<J>(
    slab: &NounSlab<J>,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactShape, CertificateNounError> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    decode_ai_pow_pearl_merge_artifact_noun(root, &space, limits)
}

fn expect_ai_pow_command_artifact_noun(
    root: Noun,
    space: &NounSpace,
) -> Result<Noun, CertificateNounError> {
    let fields = tuple3(root, space, "ai-pow command")?;
    let command_tag = expect_u64(fields[0], space, "ai-pow command tag")?;
    if command_tag != tas!(b"command") {
        return Err(CertificateNounError::Shape("expected %command"));
    }
    let pow_tag = expect_u64(fields[1], space, "ai-pow command pow tag")?;
    if pow_tag != tas!(b"pow") {
        return Err(CertificateNounError::Shape("expected %pow command"));
    }
    Ok(fields[2])
}

/// Decode the exact Nockchain submission command and return its canonical
/// `%ai-pow` artifact metadata.
///
/// Expected command shape:
///
/// ```hoon
/// [%command %pow [%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]]
/// ```
///
/// This is the Rust boundary a future Hoon/jet integration can call before
/// walking the recursive proof tail.
pub fn decode_ai_pow_pearl_merge_command_metadata_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactMetadataShape, CertificateNounError> {
    let artifact = expect_ai_pow_command_artifact_noun(root, space)?;
    decode_ai_pow_pearl_merge_artifact_metadata_noun(artifact, space, limits)
}

/// Decode the exact Nockchain submission command and return the full `%ai-pow`
/// artifact.
pub fn decode_ai_pow_pearl_merge_command_noun(
    root: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactShape, CertificateNounError> {
    let artifact = expect_ai_pow_command_artifact_noun(root, space)?;
    decode_ai_pow_pearl_merge_artifact_noun(artifact, space, limits)
}

/// Decode the exact Nockchain submission command slab and return metadata only.
pub fn decode_ai_pow_pearl_merge_command_metadata_slab<J>(
    slab: &NounSlab<J>,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactMetadataShape, CertificateNounError> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    decode_ai_pow_pearl_merge_command_metadata_noun(root, &space, limits)
}

/// Decode the exact Nockchain submission command slab and return the full
/// artifact.
pub fn decode_ai_pow_pearl_merge_command_slab<J>(
    slab: &NounSlab<J>,
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactShape, CertificateNounError> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    decode_ai_pow_pearl_merge_command_noun(root, &space, limits)
}

/// Decode and validate a jammed Hoon `%ai-pow` block artifact.
pub fn decode_ai_pow_pearl_merge_artifact_jam(
    jammed: &[u8],
    limits: CertificateNounLimits,
) -> Result<PearlMergeAiPowArtifactShape, CertificateNounError> {
    let slab = cue_canonical_artifact_jam(jammed, limits)?;
    decode_ai_pow_pearl_merge_artifact_slab(&slab, limits)
}

/// Production statement precheck for a decoded Pearl merge-mined AI-PoW
/// artifact.
///
/// This checks the shared Pearl-compatible attempt, verifies the Pearl
/// coinbase/transaction merkle inclusion proof for the Nockchain aux digest,
/// and confirms the recursive certificate's public statement fields are the
/// Pearl fields, not a native explicit-nonce statement.
pub fn precheck_ai_pow_pearl_merge_artifact_statement(
    artifact: &PearlMergeAiPowArtifactShape,
    candidate_nock_block_commitment: &[u8; 32],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    precheck_ai_pow_pearl_merge_artifact_statement_with_context(
        artifact,
        PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment,
            a_row_major,
            b_col_major,
            nockchain_target,
            max_pattern_len,
        },
    )
}

/// Context-based form of [`precheck_ai_pow_pearl_merge_artifact_statement`].
pub fn precheck_ai_pow_pearl_merge_artifact_statement_with_context(
    artifact: &PearlMergeAiPowArtifactShape,
    context: PearlMergeAiPowVerifierContext<'_>,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let statement_bytes = artifact.statement.to_wire_bytes()?;
    let precheck = verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
        context.candidate_nock_block_commitment, &statement_bytes, context.a_row_major,
        context.b_col_major, context.nockchain_target, context.max_pattern_len,
        &artifact.aux_inclusion,
    )?;
    // Tile-based precheck (context carries the matrices): bind the full pis
    // including the raw `jackpot` tile PI. The matrix-free compact node path uses
    // `precheck_ai_pow_pearl_merge_artifact_statement_committed` instead.
    precheck_pearl_merge_certificate_public_inputs(
        &artifact.certificate, &artifact.statement, &precheck, true,
    )?;
    Ok(precheck)
}

/// Matrix-free dense statement precheck for the option-(a) **COMPACT** node verify
/// (arbitrary miner-chosen matrices, Pearl parity — no synthetic-matrix pin).
///
/// Mirrors the MoE compact node path: aux binding + `verify_pearl_compatible_work_
/// committed` (the miner's COMMITTED `H_A`/`H_B`, opened rows/cols from the public
/// pattern, difficulty on the authenticated `hash_jackpot`, NO tile recompute) +
/// the pis binding WITHOUT the raw `jackpot` tile PI. The compact recursive
/// certificate proves `pis.jackpot`/`pis.hash_jackpot` in-circuit over the committed
/// matrices; the `hash_jackpot` PI comparison here (from the authenticated
/// statement) ties the difficulty gate to the proven tile. The commitment-keyed
/// noise is the anti-grind, so arbitrary/degenerate matrices are safe.
pub(crate) fn precheck_ai_pow_pearl_merge_artifact_statement_committed(
    artifact: &PearlMergeAiPowArtifactShape,
    context: &PearlMergeAiPowVerifierContext<'_>,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let statement = &artifact.statement;

    // (1) Aux binding (matrix-free) — identical to the MoE compact path.
    let header = PearlIncompleteBlockHeader::from_bytes(&statement.block_header)?;
    verify_pearl_aux_inclusion(
        &header, &statement.expected_aux_commitment, &artifact.aux_inclusion,
    )?;
    if statement.aux.nock_block_commitment != *context.candidate_nock_block_commitment {
        return Err(CertificateNounError::PearlMergeStatement(
            PearlCompatError::NockchainAuxBlockCommitmentMismatch,
        ));
    }
    let aux_commitment = statement.aux.commitment()?;
    if aux_commitment != statement.expected_aux_commitment {
        return Err(CertificateNounError::PearlMergeStatement(
            PearlCompatError::NockchainAuxCommitmentMismatch,
        ));
    }

    // (2) Matrix-free work: committed H_A/H_B + public-pattern rows/cols + difficulty.
    // `from_public_data` (dense) fail-closes on a MoE trailer, matching this path's
    // dense-only contract (the caller also rejects `artifact.moe.is_some()`).
    let public_params = PearlPublicProofParams::from_public_data(header, &statement.public_data)?;
    let work = verify_pearl_compatible_work_committed(
        &public_params, context.nockchain_target, context.max_pattern_len,
    )?;
    let precheck = PearlMergeMiningPrecheck {
        work,
        aux: statement.aux.clone(),
        aux_commitment,
    };

    // (3) Bind the pis WITHOUT the raw `jackpot` tile PI (proof-bound); `hash_jackpot`
    // (from the authenticated statement) is still bound, tying difficulty to the proof.
    precheck_pearl_merge_certificate_public_inputs(
        &artifact.certificate, &artifact.statement, &precheck, false,
    )?;
    Ok(precheck)
}

/// Metadata-only statement precheck for a Pearl-format-compatible `%ai-pow`
/// artifact.
///
/// This has the same statement semantics as
/// [`precheck_ai_pow_pearl_merge_artifact_statement`], but it uses only the
/// decoded opaque nonce and certificate metadata. It is the preferred cheap
/// gate before any recursive proof-node traversal.
pub fn precheck_ai_pow_pearl_merge_artifact_metadata(
    artifact: &PearlMergeAiPowArtifactMetadataShape,
    candidate_nock_block_commitment: &[u8; 32],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    precheck_ai_pow_pearl_merge_artifact_metadata_with_context(
        artifact,
        PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment,
            a_row_major,
            b_col_major,
            nockchain_target,
            max_pattern_len,
        },
    )
}

/// Context-based form of [`precheck_ai_pow_pearl_merge_artifact_metadata`].
pub fn precheck_ai_pow_pearl_merge_artifact_metadata_with_context(
    artifact: &PearlMergeAiPowArtifactMetadataShape,
    context: PearlMergeAiPowVerifierContext<'_>,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let statement_bytes = artifact.statement.to_wire_bytes()?;
    let precheck = verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
        context.candidate_nock_block_commitment, &statement_bytes, context.a_row_major,
        context.b_col_major, context.nockchain_target, context.max_pattern_len,
        &artifact.aux_inclusion,
    )?;
    precheck_pearl_merge_certificate_metadata(
        &artifact.certificate, &artifact.statement, &precheck, true,
    )?;
    Ok(precheck)
}

/// Decode a jammed `%ai-pow` artifact and run only the cheap Rust-owned nonce
/// statement precheck.
///
/// This is the DoS-resistant boundary for consensus code before recursive
/// verifier work is wired. It caps jam bytes, preflights the noun, cues and
/// canonicalizes the jam, decodes only the opaque nonce plus certificate
/// metadata, then rejects replay/tamper before walking the recursive
/// proof-node tail.
pub fn precheck_ai_pow_pearl_merge_artifact_jam(
    jammed: &[u8],
    limits: CertificateNounLimits,
    candidate_nock_block_commitment: &[u8; 32],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    precheck_ai_pow_pearl_merge_artifact_jam_with_context(
        jammed,
        limits,
        PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment,
            a_row_major,
            b_col_major,
            nockchain_target,
            max_pattern_len,
        },
    )
}

/// Context-based form of [`precheck_ai_pow_pearl_merge_artifact_jam`].
pub fn precheck_ai_pow_pearl_merge_artifact_jam_with_context(
    jammed: &[u8],
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let slab = cue_canonical_artifact_jam(jammed, limits)?;
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    let fields = tuple3(root, &space, "ai-pow artifact")?;
    let tag = expect_u64(fields[0], &space, "ai-pow artifact tag")?;
    if tag != tas!(b"ai-pow") {
        return Err(CertificateNounError::Shape("expected %ai-pow artifact"));
    }
    let nonce = expect_declared_bounded_bytes(
        fields[1], &space, 1, AI_POW_NONCE_MAX_SIZE, "ai-pow nonce", limits,
    )?;
    let parsed_nonce = decode_pearl_merge_ai_pow_nonce(&nonce)?;
    let metadata = decode_ai_pow_certificate_metadata_noun(fields[2], &space, limits)?;

    precheck_ai_pow_pearl_merge_artifact_metadata_with_context(
        &PearlMergeAiPowArtifactMetadataShape {
            statement: parsed_nonce.statement,
            aux_inclusion: parsed_nonce.aux_inclusion,
            certificate: metadata,
        },
        context,
    )
}

/// Metadata-only precheck for the exact Nockchain submission command.
///
/// This deliberately does not reconstruct or verify the recursive proof. It
/// exists so Nockchain-side glue can cheaply reject malformed command shapes,
/// malformed `AIP1` nonces, candidate-block replay, aux tamper, target misses,
/// and recursive metadata drift before any verifier path is called.
pub fn precheck_ai_pow_pearl_merge_command_metadata_with_context<J>(
    command: &NounSlab<J>,
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let metadata = decode_ai_pow_pearl_merge_command_metadata_slab(command, limits)?;
    precheck_ai_pow_pearl_merge_artifact_metadata_with_context(&metadata, context)
}

fn verify_compact_certificate_shape_with_context_and_limits(
    certificate_shape: &AiPowCertificateShape,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest: &ai_pow_zk::recursion::AiPowCompactBatchVerifierKeyDigest,
    limits: CertificateNounLimits,
    // P0/D6: the CANONICAL L0 program commitment, derived by the node from the
    // opened schedule (never from the prover). Binds the opened schedule on the
    // compact path (the recursion folds it into the statement digest).
    l0_program_commitment: &[ai_pow_zk::Val],
) -> Result<(), CertificateNounError> {
    if compact_context.verifier_key_digest() != expected_verifier_key_digest {
        return Err(CertificateNounError::CompactVerifierKeyDigestMismatch(
            "verifier-context",
        ));
    }
    let certificate = ai_pow_compact_recursive_certificate_from_node_with_limits(
        &certificate_shape.certificate, limits,
    )?;
    if certificate.verifier_key_digest() != expected_verifier_key_digest {
        return Err(CertificateNounError::CompactVerifierKeyDigestMismatch(
            "certificate",
        ));
    }
    ai_pow_zk::recursion::verify_compact_batch_recursive_certificate_with_context(
        compact_context, certificate, &certificate_shape.public_inputs, l0_program_commitment,
    )
    .map_err(|e| CertificateNounError::RecursiveCertificate(e.to_string()))
}

/// P0/D6: derive the canonical L0 program's preprocessed commitment the node must
/// fold into the compact statement digest, from the opened schedule the precheck
/// rebuilt (`work.ticket`) and the work commitments — never from the prover.
fn canonical_l0_commitment_for_compact(
    certificate: &AiPowCertificateShape,
    precheck: &PearlMergeMiningPrecheck,
) -> Result<Vec<ai_pow_zk::Val>, CertificateNounError> {
    let zk_params = certificate.zk_params;
    let ticket = &precheck.work.ticket;
    let strip_schedule =
        StripIndexSchedule::from_indices(&zk_params, ticket.a_rows.clone(), ticket.b_cols.clone())
            .map_err(|_| CertificateNounError::PearlMergeUnsupportedTileShape)?;
    let col_tiles = zk_params.n / zk_params.tile;
    if col_tiles == 0 {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    let block_public = ai_pow_zk::canonical::BlockPublic {
        tile_i: certificate.found_idx / col_tiles,
        tile_j: certificate.found_idx % col_tiles,
        kappa: precheck.work.commitments.kappa,
        s_a: precheck.work.commitments.s_a,
        s_b: precheck.work.commitments.s_b,
    };
    let program = ai_pow_zk::canonical::canonical_program_for_strip_schedule(
        &zk_params, &strip_schedule, &block_public, certificate.trace_height,
    )
    .map_err(|e| {
        CertificateNounError::RecursiveCertificate(format!("canonical L0 program: {e:?}"))
    })?;
    let profile = ai_pow_zk::circuit::CircuitConfig::for_layer0_trace(certificate.trace_height);
    Ok(ai_pow_zk::recursion::canonical_l0_program_commitment_vals(
        &zk_params, &profile, &program,
    ))
}

/// Verify a decoded Pearl-compatible `%ai-pow` artifact carrying the selected
/// compact recursive certificate.
///
/// This is the compact counterpart to
/// [`verify_decoded_ai_pow_pearl_merge_artifact_with_context_and_limits`]. It
/// keeps the same cheap Pearl statement precheck before proof-node traversal,
/// then reconstructs the compact certificate from canonical bytes and verifies
/// it with verifier-owned compact context. The caller must pass the expected
/// verifier-key/setup digest from trusted verifier configuration; this function
/// rejects before proof verification if either the context or certificate uses
/// a different digest.
///
/// # Soundness — verifier-derived canonical program binding
///
/// The compact certificate carries no raw Layer-0 program. The node reconstructs
/// the canonical program from the public opened schedule in `precheck`, derives
/// its preprocessed commitment through
/// [`ai_pow_zk::recursion::canonical_l0_program_commitment_vals`], and supplies
/// that commitment to compact verification. The P0/D6 statement-digest fold
/// therefore rejects a certificate proven over any other rows, columns, or
/// selector schedule. `compact_context` and `expected_verifier_key_digest` must
/// still come from verifier-owned setup; proof-carried setup is never authority.
pub(crate) fn verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits(
    artifact: &PearlMergeAiPowArtifactShape,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest: &ai_pow_zk::recursion::AiPowCompactBatchVerifierKeyDigest,
    limits: CertificateNounLimits,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    // Dense compact verify: a MoE (AIM1) artifact MUST NOT be routed here. The
    // statement precheck already fail-closes on a MoE `public_data` (via
    // `sanity_check`), but reject explicitly for a precise error — MoE uses
    // `verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits`.
    if artifact.moe.is_some() {
        return Err(CertificateNounError::Shape(
            "dense compact verify called on a MoE (AIM1) artifact; use the MoE verify path",
        ));
    }
    // Option (a): matrix-free precheck (arbitrary miner-chosen matrices). The tile
    // is proof-bound, not recomputed from a fixed matrix set. See
    // `precheck_ai_pow_pearl_merge_artifact_statement_committed`.
    let mut precheck =
        precheck_ai_pow_pearl_merge_artifact_statement_committed(artifact, &context)?;
    // P0/D6: derive the canonical L0 program commitment from the opened schedule
    // the precheck rebuilt (never from the prover) and bind it into the compact
    // verify — a certificate proven over a different program fails the statement
    // digest. Closes the compact-path opened-schedule binding gap.
    let l0_program_commitment =
        canonical_l0_commitment_for_compact(&artifact.certificate, &precheck)?;
    verify_compact_certificate_shape_with_context_and_limits(
        &artifact.certificate, compact_context, expected_verifier_key_digest, limits,
        &l0_program_commitment,
    )?;
    precheck.work.ticket.tile_state =
        tile_state_from_words(&artifact.certificate.public_inputs.jackpot);
    Ok(precheck)
}

/// Node-side MoE (GROUPED_GEMM) precheck result: the MoE work verification plus
/// the aux binding, mirroring [`PearlMergeMiningPrecheck`] for the MoE path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeMoeMiningPrecheck {
    pub work: PearlMoeWorkPrecheck,
    pub aux: PearlNockchainAux,
    pub aux_commitment: [u8; 32],
}

fn moe_matmul_params_from_certificate_zk(
    public_params: &PearlPublicProofParams,
    zk: &ZkParams,
) -> Result<MatmulParams, CertificateNounError> {
    let total_b_cols = public_params.total_b_cols()?;
    if zk.m != public_params.m
        || zk.n != total_b_cols
        || zk.k != public_params.mining_config.common_dim
        || zk.noise_rank != u32::from(public_params.mining_config.rank)
    {
        return Err(CertificateNounError::Shape(
            "compact certificate zk_params disagree with the authenticated MoE statement dims",
        ));
    }
    if zk.difficulty_bits != 0 {
        return Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "difficulty_bits must be 0; Nockchain target is verifier-supplied",
        )
        .into());
    }
    Ok(MatmulParams {
        m: public_params.m,
        k: public_params.mining_config.common_dim,
        n: total_b_cols,
        noise_rank: u32::from(public_params.mining_config.rank),
        tile: zk.tile,
        spot_checks: 1,
        difficulty_bits: zk.difficulty_bits,
    })
}

/// Verify a decoded **MoE** (`AIM1`) Pearl-merge `%ai-pow` compact artifact
/// (`e > 0`) — the MoE counterpart of
/// [`verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits`].
///
/// The dense function cannot handle MoE (its statement precheck fail-closes on a
/// MoE `public_data`). This runs the MoE-aware verification:
///   1. reconstruct + **aux-bind** the statement (identical to the dense prefix:
///      aux inclusion, `nock_block_commitment`, `aux_commitment`);
///   2. [`verify_pearl_moe_compatible_work`] — MoE-aware envelope + routing
///      binding + difficulty gate on the authenticated `hash_jackpot` over the
///      miner's COMMITTED matrices (no tile recompute; option (a), arbitrary
///      models), then the caller binds `pis.hash_jackpot == public_params.hash_jackpot`;
///   3. reconstruct `MatmulParams` from the **authenticated** statement dims
///      (requiring the certificate's `zk_params` to agree), then
///      [`verify_pearl_moe_compact_recursive_certificate`] — routing/PI/schedule
///      binding + the compact proof verify (with the P0/D6 program-commitment
///      fold).
///
/// The caller must pass the trusted expected compact verifier-key digest, exactly
/// as for the dense path. MoE block admission additionally remains gated behind
/// the recursive-prover acceptance guard + the Hoon↔Rust consensus wiring (M4
/// admission half + D4/S1); this function is the soundness-complete verify the
/// eventual dispatcher calls for `artifact.moe.is_some()`.
pub(crate) fn verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
    artifact: &PearlMergeAiPowArtifactShape,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest: &ai_pow_zk::recursion::AiPowCompactBatchVerifierKeyDigest,
    limits: CertificateNounLimits,
) -> Result<PearlMergeMoeMiningPrecheck, CertificateNounError> {
    let moe_art = artifact.moe.as_ref().ok_or(CertificateNounError::Shape(
        "MoE compact verify requires a MoE (AIM1) artifact",
    ))?;
    let statement = &artifact.statement;

    // (1) Aux binding — identical to the dense `verify_pearl_merge_mining_public_data`
    // prefix: the Nockchain candidate block is committed into the Pearl header via
    // the aux inclusion + commitment. (`?` auto-converts PearlCompatError.)
    let header = PearlIncompleteBlockHeader::from_bytes(&statement.block_header)?;
    verify_pearl_aux_inclusion(
        &header, &statement.expected_aux_commitment, &artifact.aux_inclusion,
    )?;
    if statement.aux.nock_block_commitment != *context.candidate_nock_block_commitment {
        return Err(CertificateNounError::PearlMergeStatement(
            PearlCompatError::NockchainAuxBlockCommitmentMismatch,
        ));
    }
    let aux_commitment = statement.aux.commitment()?;
    if aux_commitment != statement.expected_aux_commitment {
        return Err(CertificateNounError::PearlMergeStatement(
            PearlCompatError::NockchainAuxCommitmentMismatch,
        ));
    }

    // (2) Public params from the authenticated statement (MoE-tolerant parse — the
    // dense `from_public_data` fail-closes on the MoE trailer), then the MoE work
    // precheck (envelope + routing-consistency + jackpot/difficulty binding).
    let public_params =
        PearlPublicProofParams::from_public_data_allowing_moe(header, &statement.public_data)?;
    let work = verify_pearl_moe_compatible_work(
        &public_params, &moe_art.moe, &moe_art.routing_data, context.nockchain_target,
        context.max_pattern_len,
    )?;

    // (a) Bind the difficulty-gated statement jackpot to the PROVEN jackpot. Step (5)'s
    // recursive certificate proves `pis.hash_jackpot` is the opened tile's real output
    // over the miner's committed `H_A`/`H_B`; requiring the authenticated statement's
    // `hash_jackpot` (which +verify_pearl_moe_compatible_work gates on the target) to
    // equal it makes the difficulty gate hold for the proven tile. Matrices are
    // miner-chosen, Pearl-parity, with commitment-keyed noise as the anti-grind.
    if digest_words_to_bytes(&artifact.certificate.public_inputs.hash_jackpot)
        != public_params.hash_jackpot
    {
        return Err(PearlCompatError::JackpotHashMismatch.into());
    }

    // (3) Reconstruct MatmulParams from the authenticated statement. Pearl's
    // GROUPED_GEMM wire stores per-expert `n_e`; the recursive circuit stores
    // the total committed B width `n_e * e`.
    let params =
        moe_matmul_params_from_certificate_zk(&public_params, &artifact.certificate.zk_params)?;

    // (4) Verifier-key digest checks + decode the compact certificate.
    if compact_context.verifier_key_digest() != expected_verifier_key_digest {
        return Err(CertificateNounError::CompactVerifierKeyDigestMismatch(
            "verifier-context",
        ));
    }
    let cert = ai_pow_compact_recursive_certificate_from_node_with_limits(
        &artifact.certificate.certificate, limits,
    )?;
    if cert.verifier_key_digest() != expected_verifier_key_digest {
        return Err(CertificateNounError::CompactVerifierKeyDigestMismatch(
            "certificate",
        ));
    }

    // (5) Proof half: routing-consistency + expert-column recompute + routing-spliced
    // s_A/PI binding + the P0/D6 opened-schedule commitment fold + compact verify.
    verify_pearl_moe_compact_recursive_certificate(
        compact_context, cert, &artifact.certificate.public_inputs, &params,
        &work.commitments.kappa, &work.commitments.h_a, &work.commitments.h_b,
        &public_params.mining_config, &moe_art.moe, public_params.m, public_params.n,
        public_params.t_rows, public_params.t_cols, &moe_art.routing_data, context.max_pattern_len,
    )
    .map_err(|e| CertificateNounError::RecursiveCertificate(e.to_string()))?;

    Ok(PearlMergeMoeMiningPrecheck {
        work,
        aux: statement.aux.clone(),
        aux_commitment,
    })
}

/// Byte-digest form of
/// [`verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits`].
///
/// This is the production-configuration friendly entry point: the expected
/// compact verifier-key/setup digest is a canonical 40-byte value instead of
/// caller-constructed field elements.
pub fn verify_decoded_ai_pow_pearl_merge_compact_artifact_with_digest_bytes_and_limits(
    artifact: &PearlMergeAiPowArtifactShape,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest_bytes: &[u8],
    limits: CertificateNounLimits,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let expected_verifier_key_digest =
        ai_pow_zk::recursion::compact_batch_verifier_key_digest_from_bytes(
            expected_verifier_key_digest_bytes,
        )
        .map_err(|e| CertificateNounError::CompactVerifierKeyDigestEncoding(e.to_string()))?;
    verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits(
        artifact, context, compact_context, &expected_verifier_key_digest, limits,
    )
}

fn precheck_pearl_merge_certificate_public_inputs(
    certificate: &AiPowCertificateShape,
    statement: &PearlMergePublicStatementShape,
    precheck: &PearlMergeMiningPrecheck,
    check_jackpot: bool,
) -> Result<(), CertificateNounError> {
    let metadata = AiPowCertificateMetadata {
        version: certificate.version,
        zk_params: certificate.zk_params,
        found_idx: certificate.found_idx,
        trace_height: certificate.trace_height,
        commitments: certificate.commitments,
        public_inputs: certificate.public_inputs.clone(),
    };
    precheck_pearl_merge_certificate_metadata(&metadata, statement, precheck, check_jackpot)
}

fn precheck_pearl_merge_certificate_metadata(
    metadata: &AiPowCertificateMetadata,
    statement: &PearlMergePublicStatementShape,
    precheck: &PearlMergeMiningPrecheck,
    check_jackpot: bool,
) -> Result<(), CertificateNounError> {
    let block_header = PearlIncompleteBlockHeader::from_bytes(&statement.block_header)?;
    let public_params =
        PearlPublicProofParams::from_public_data(block_header, &statement.public_data)?;
    if metadata.zk_params.m != public_params.m {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params.m",
        ));
    }
    if metadata.zk_params.k != public_params.mining_config.common_dim {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params.k",
        ));
    }
    if metadata.zk_params.n != public_params.n {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params.n",
        ));
    }
    if metadata.zk_params.noise_rank != u32::from(public_params.mining_config.rank) {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params.noise-rank",
        ));
    }
    if metadata.zk_params.difficulty_bits != 0 {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params.difficulty-bits",
        ));
    }
    let h = public_params.h()?;
    if metadata.zk_params.tile != h {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    let params = MatmulParams {
        m: public_params.m,
        k: public_params.mining_config.common_dim,
        n: public_params.n,
        noise_rank: u32::from(public_params.mining_config.rank),
        tile: h,
        spot_checks: 1,
        difficulty_bits: metadata.zk_params.difficulty_bits,
    };
    validate_pearl_merge_recursive_params(&params)?;
    let expected_zk_params = zk_params_from_matmul(&params);
    if metadata.zk_params != expected_zk_params {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "params",
        ));
    }
    let strip_schedule = StripIndexSchedule::from_indices(
        &metadata.zk_params,
        precheck.work.ticket.a_rows.clone(),
        precheck.work.ticket.b_cols.clone(),
    )
    .map_err(|_| CertificateNounError::PearlMergeUnsupportedTileShape)?;
    let expected_found_idx = pearl_merge_legacy_found_idx(&public_params, &params).unwrap_or(0);
    if metadata.found_idx != expected_found_idx {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "found-idx",
        ));
    }
    let expected_trace_height = expected_layer0_rows_for_strip_schedule(&params, &strip_schedule)
        .map_err(|_| CertificateNounError::PearlMergeUnsupportedTileShape)?
        .required_trace_len();
    // Defense-in-depth backstop for the one-tile-one-STARK invariant: the
    // Pearl `h·w ≤ 256` cap (enforced upstream in `sanity_check`) already
    // bounds the Layer-0 trace, but assert it directly at the consensus
    // boundary so a future change to the trace formula or an upstream gap
    // cannot admit a certificate whose tile does not fit a single STARK.
    if expected_trace_height as u64 > ai_pow::params::PEARL_TRACE_BOUND {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    if metadata.trace_height != expected_trace_height {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "trace-height",
        ));
    }
    if metadata.commitments.h_a_chunk != precheck.work.commitments.h_a {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "commitments.h-a-chunk",
        ));
    }
    if metadata.commitments.h_b_chunk != precheck.work.commitments.h_b {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "commitments.h-b-chunk",
        ));
    }
    let expected_public_inputs = pearl_merge_recursive_public_inputs_from_work(
        &precheck.work.commitments, &precheck.work.ticket,
    );
    precheck_pearl_merge_bound_public_inputs(
        &metadata.public_inputs, &expected_public_inputs, check_jackpot,
    )?;
    Ok(())
}

fn precheck_pearl_merge_bound_public_inputs(
    got: &CompositePublicInputs,
    expected: &CompositePublicInputs,
    // `check_jackpot`: whether to bind the raw tile PI `jackpot`. TRUE for the
    // matrix-holding paths (the miner producer + the intermediate checkpoint) that
    // recompute the tile off-circuit. FALSE for the option-(a) COMPACT node verify,
    // whose `expected` has no tile (matrices are miner-chosen); there the compact
    // recursive certificate binds `pis.jackpot` in-circuit, and `hash_jackpot`
    // (checked below) ties the difficulty gate to the proven tile.
    check_jackpot: bool,
) -> Result<(), CertificateNounError> {
    if got.hash_a != expected.hash_a {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.hash-a",
        ));
    }
    if got.hash_b != expected.hash_b {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.hash-b",
        ));
    }
    if got.job_key != expected.job_key {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.job-key",
        ));
    }
    if got.commitment_hash != expected.commitment_hash {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.commitment-hash",
        ));
    }
    if check_jackpot && got.jackpot != expected.jackpot {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.jackpot",
        ));
    }
    if got.hash_jackpot != expected.hash_jackpot {
        return Err(CertificateNounError::PearlMergePublicInputMismatch(
            "public-inputs.hash-jackpot",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn contiguous_indices(start: u32, len: u32) -> Vec<u32> {
    (0..len).map(|offset| start + offset).collect()
}

fn pearl_merge_legacy_found_idx(
    public_params: &PearlPublicProofParams,
    params: &MatmulParams,
) -> Option<u32> {
    let h = public_params.h().ok()?;
    let w = public_params.w().ok()?;
    if h != params.tile || w != params.tile {
        return None;
    }
    if !public_params.t_rows.is_multiple_of(params.tile)
        || !public_params.t_cols.is_multiple_of(params.tile)
    {
        return None;
    }
    let col_tiles = public_params.n / params.tile;
    if col_tiles == 0 {
        return None;
    }
    (public_params.t_rows / params.tile)
        .checked_mul(col_tiles)
        .and_then(|base| base.checked_add(public_params.t_cols / params.tile))
}

fn validate_pearl_merge_recursive_params(
    params: &MatmulParams,
) -> Result<(), CertificateNounError> {
    if params.m == 0 || params.n == 0 {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    if params.k == 0 || params.k > ai_pow::params::PEARL_K_MAX {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    if params.noise_rank < 2
        || params.noise_rank > params.k
        || !params.noise_rank.is_power_of_two()
        || !params.k.is_multiple_of(params.noise_rank)
    {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    if (params.k / params.noise_rank) as usize > ai_pow::params::PEARL_STRIPE_MAX {
        return Err(CertificateNounError::PearlMergeUnsupportedTileShape);
    }
    Ok(())
}

/// Derive the recursive certificate public-input slots that identify a
/// Pearl-compatible ticket statement.
///
/// Pearl merge-mined certificates use the existing Layer-0 public-input slots,
/// but with Pearl semantics: `JOB_KEY = kappa`, `COMMITMENT_HASH = s_A`,
/// `JACKPOT_MSG = TileState`, and `HASH_JACKPOT = BLAKE3(TileState, key=s_A)`.
/// Keeping this derivation centralized prevents miner/prover code from mixing
/// incompatible public-input semantics into the `%ai-pow` artifact. The
/// `cumsum` slots are left at zero here because the current Pearl precheck does
/// not derive them; the recursive proof verifier still checks the full public
/// input vector supplied by the certificate. This helper is not a production
/// artifact constructor.
pub(crate) fn pearl_merge_recursive_public_inputs_from_work(
    commitments: &PearlWorkCommitments,
    ticket: &PearlPatternTicket,
) -> CompositePublicInputs {
    let mut pis = CompositePublicInputs::zero();
    pis.hash_a = digest_words(&commitments.h_a);
    pis.hash_b = digest_words(&commitments.h_b);
    pis.job_key = digest_words(&commitments.kappa);
    pis.commitment_hash = digest_words(&commitments.s_a);
    pis.jackpot = tile_state_words(&ticket.tile_state);
    pis.hash_jackpot = digest_words(&ticket.jackpot_hash);
    pis
}

/// Derive the recursive certificate public inputs from a completed Pearl
/// merge-mining precheck. This helper derives metadata only and does not build
/// a block artifact.
pub fn pearl_merge_recursive_public_inputs_from_precheck(
    precheck: &PearlMergeMiningPrecheck,
) -> CompositePublicInputs {
    pearl_merge_recursive_public_inputs_from_work(&precheck.work.commitments, &precheck.work.ticket)
}

fn tile_state_words(tile_state: &ai_pow::matmul::TileState) -> [u32; 16] {
    core::array::from_fn(|i| tile_state.0[i] as u32)
}

fn tile_state_from_words(words: &[u32; 16]) -> ai_pow::matmul::TileState {
    ai_pow::matmul::TileState(core::array::from_fn(|i| words[i] as i32))
}

/// Decode and verify a jammed Pearl-compatible `%ai-pow` artifact carrying the
/// selected compact recursive certificate.
///
/// The ordering mirrors [`verify_ai_pow_pearl_merge_artifact_jam_with_context`]:
/// byte cap, canonical cue, opaque nonce and certificate metadata decode,
/// cheap Pearl statement precheck, then proof-node traversal and compact proof
/// verification. The compact verifier context is verifier-owned, and
/// `expected_verifier_key_digest` must come from pinned verifier configuration.
pub fn verify_ai_pow_pearl_merge_compact_artifact_jam_with_context(
    jammed: &[u8],
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest: &ai_pow_zk::recursion::AiPowCompactBatchVerifierKeyDigest,
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let slab = cue_canonical_artifact_jam(jammed, limits)?;
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    let fields = tuple3(root, &space, "ai-pow artifact")?;
    let tag = expect_u64(fields[0], &space, "ai-pow artifact tag")?;
    if tag != tas!(b"ai-pow") {
        return Err(CertificateNounError::Shape("expected %ai-pow artifact"));
    }
    let nonce = expect_declared_bounded_bytes(
        fields[1], &space, 1, AI_POW_NONCE_MAX_SIZE, "ai-pow nonce", limits,
    )?;
    let parsed_nonce = decode_pearl_merge_ai_pow_nonce(&nonce)?;
    let metadata = decode_ai_pow_certificate_metadata_noun(fields[2], &space, limits)?;

    let precheck = precheck_ai_pow_pearl_merge_artifact_metadata_with_context(
        &PearlMergeAiPowArtifactMetadataShape {
            statement: parsed_nonce.statement,
            aux_inclusion: parsed_nonce.aux_inclusion,
            certificate: metadata,
        },
        context,
    )?;

    let certificate_shape = decode_ai_pow_certificate_noun(fields[2], &space, limits)?;
    // P0/D6: bind the canonical L0 program commitment (derived from the opened
    // schedule, not the prover) into the compact verify.
    let l0_program_commitment = canonical_l0_commitment_for_compact(&certificate_shape, &precheck)?;
    verify_compact_certificate_shape_with_context_and_limits(
        &certificate_shape, compact_context, expected_verifier_key_digest, limits,
        &l0_program_commitment,
    )?;
    Ok(precheck)
}

/// Byte-digest form of
/// [`verify_ai_pow_pearl_merge_compact_artifact_jam_with_context`].
pub fn verify_ai_pow_pearl_merge_compact_artifact_jam_with_digest_bytes_and_context(
    jammed: &[u8],
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest_bytes: &[u8],
) -> Result<PearlMergeMiningPrecheck, CertificateNounError> {
    let expected_verifier_key_digest =
        ai_pow_zk::recursion::compact_batch_verifier_key_digest_from_bytes(
            expected_verifier_key_digest_bytes,
        )
        .map_err(|e| CertificateNounError::CompactVerifierKeyDigestEncoding(e.to_string()))?;
    verify_ai_pow_pearl_merge_compact_artifact_jam_with_context(
        jammed, limits, context, compact_context, &expected_verifier_key_digest,
    )
}

/// Verify a jammed **MoE** (`AIM1`) Pearl-merge `%ai-pow` compact artifact — the
/// GROUPED_GEMM counterpart of
/// [`verify_ai_pow_pearl_merge_compact_artifact_jam_with_context`], and the jam
/// boundary the consensus dispatcher calls for a MoE artifact.
///
/// The dense jam entry hardcodes the dense nonce decoder + dense statement
/// precheck (which fail-closes on a MoE trailer), so MoE needs its own boundary.
/// This cues + bounded-decodes the full artifact (the decoder dispatches on the
/// `AIM1` tag into `moe: Some(..)`) and runs the node MoE verify branch.
///
/// The eventual consensus dispatcher peeks the 4-byte nonce tag (`AIM1` vs `AIP1`)
/// to choose this entry vs the dense one; the two return distinct precheck types.
pub fn verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with_context(
    jammed: &[u8],
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest: &ai_pow_zk::recursion::AiPowCompactBatchVerifierKeyDigest,
) -> Result<PearlMergeMoeMiningPrecheck, CertificateNounError> {
    let artifact = decode_ai_pow_pearl_merge_artifact_jam(jammed, limits)?;
    if artifact.moe.is_none() {
        return Err(CertificateNounError::Shape(
            "expected a MoE (AIM1) artifact on the MoE jam verify path",
        ));
    }
    verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
        &artifact, context, compact_context, expected_verifier_key_digest, limits,
    )
}

/// Byte-digest form of
/// [`verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with_context`].
pub fn verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with_digest_bytes_and_context(
    jammed: &[u8],
    limits: CertificateNounLimits,
    context: PearlMergeAiPowVerifierContext<'_>,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest_bytes: &[u8],
) -> Result<PearlMergeMoeMiningPrecheck, CertificateNounError> {
    let expected_verifier_key_digest =
        ai_pow_zk::recursion::compact_batch_verifier_key_digest_from_bytes(
            expected_verifier_key_digest_bytes,
        )
        .map_err(|e| CertificateNounError::CompactVerifierKeyDigestEncoding(e.to_string()))?;
    verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with_context(
        jammed, limits, context, compact_context, &expected_verifier_key_digest,
    )
}

/// Outcome of the self-contained consensus verifier
/// [`verify_ai_pow_block_artifact_jam`]: which puzzle variant verified, plus its
/// precheck (for the caller to persist work commitments / aux binding).
#[derive(Debug, Clone)]
pub enum AiPowBlockVerifyOutcome {
    Dense(PearlMergeMiningPrecheck),
    Moe(PearlMergeMoeMiningPrecheck),
}

/// **Self-contained consensus verifier** for a jammed `%ai-pow` compact block
/// artifact — the single entrypoint the Hoon `check-pow`/`do-pow` jet calls (D4).
///
/// Unlike the lower-level `verify_..._jam_with_context` entries, the matrices are
/// never supplied or reconstructed: the model is miner-chosen (Pearl parity — no
/// synthetic-matrix pin). The verifier holds only the block, the compact verifier
/// setup, and the consensus-derived commitment/target. It dispatches on the nonce
/// tag (`AIM1` → MoE, `AIP1` → dense) to the corresponding compact node verify
/// branch.
///
/// The caller supplies the trusted compact verifier setup (`compact_context` +
/// `expected_verifier_key_digest_bytes`) — deterministic from the production
/// params and built/cached once at node startup (shared with dense; see the
/// parity doc §4 D4/S1) — and the consensus-derived
/// `candidate_nock_block_commitment` + `nockchain_target`.
///
/// # Soundness
///
/// The miner's `(A, B)` are bound, not re-derived: the compact certificate proves
/// the opened tile over the block-committed `HASH_A`/`HASH_B` (dense) or the
/// routing/jackpot commitment (MoE), and the anti-grind is the commitment-keyed
/// noise — a proof over any other matrices fails that public-input binding. So no
/// external model distribution is needed and a miner cannot grind a favorable
/// `(A, B)` without redoing the bound work.
pub fn verify_ai_pow_block_artifact_jam(
    jammed: &[u8],
    limits: CertificateNounLimits,
    candidate_nock_block_commitment: &[u8; 32],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest_bytes: &[u8],
) -> Result<AiPowBlockVerifyOutcome, CertificateNounError> {
    let artifact = decode_ai_pow_pearl_merge_artifact_jam(jammed, limits)?;
    verify_ai_pow_block_artifact(
        &artifact, limits, candidate_nock_block_commitment, nockchain_target, max_pattern_len,
        compact_context, expected_verifier_key_digest_bytes,
    )
}

/// Shape-based core of [`verify_ai_pow_block_artifact_jam`] — verifies an
/// already-decoded artifact. This is the entrypoint the **jet** uses after
/// decoding the (transparent) `%ai-pow` artifact noun via
/// [`decode_ai_pow_pearl_merge_artifact_noun`], avoiding a re-jam. Option (a): the
/// matrices are miner-chosen (arbitrary model, Pearl parity — no synthetic pin); it
/// verifies against the block's COMMITTED `H_A`/`H_B` with the tile proven
/// in-circuit, needing no model, then dispatches on the nonce tag (dense/MoE).
pub fn verify_ai_pow_block_artifact(
    artifact: &PearlMergeAiPowArtifactShape,
    limits: CertificateNounLimits,
    candidate_nock_block_commitment: &[u8; 32],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    compact_context: &ai_pow_zk::recursion::AiPowCompactBatchVerifierContext,
    expected_verifier_key_digest_bytes: &[u8],
) -> Result<AiPowBlockVerifyOutcome, CertificateNounError> {
    // Consensus cap (defense-in-depth; the jet also rejects before its setup
    // lookup): reject any block whose Layer-0 trace height exceeds
    // AI_POW_MAX_TRACE_HEIGHT (2^19). The top-of-envelope 2^20 setup is not built
    // (it needs a large-RAM node), so consensus caps the accept-band here.
    if artifact.certificate.trace_height > ai_pow::params::AI_POW_MAX_TRACE_HEIGHT {
        return Err(CertificateNounError::LimitExceeded(
            "ai-pow trace height above consensus cap",
        ));
    }
    // Option (a): the matrices are miner-chosen (arbitrary model, Pearl parity — no
    // synthetic-matrix pin). The compact node verifies (dense + MoE) bind to the
    // miner's block-COMMITTED H_A/H_B and prove the opened tile in-circuit, so NO
    // matrices are re-derived here; the verifier never needs the model. The context's
    // matrix slices are empty (the compact paths ignore them; only the intermediate
    // checkpoint verify, which this production path does not call, reads them).
    let expected_verifier_key_digest =
        ai_pow_zk::recursion::compact_batch_verifier_key_digest_from_bytes(
            expected_verifier_key_digest_bytes,
        )
        .map_err(|e| CertificateNounError::CompactVerifierKeyDigestEncoding(e.to_string()))?;
    let context = PearlMergeAiPowVerifierContext {
        candidate_nock_block_commitment,
        a_row_major: &[],
        b_col_major: &[],
        nockchain_target,
        max_pattern_len,
    };

    if artifact.moe.is_some() {
        let pre = verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
            artifact, context, compact_context, &expected_verifier_key_digest, limits,
        )?;
        Ok(AiPowBlockVerifyOutcome::Moe(pre))
    } else {
        let pre = verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits(
            artifact, context, compact_context, &expected_verifier_key_digest, limits,
        )?;
        Ok(AiPowBlockVerifyOutcome::Dense(pre))
    }
}

#[derive(Debug)]
struct DecodeState {
    limits: CertificateNounLimits,
    total_nodes: usize,
    total_atom_bytes: usize,
}

impl DecodeState {
    fn new(limits: CertificateNounLimits) -> Self {
        Self {
            limits,
            total_nodes: 0,
            total_atom_bytes: 0,
        }
    }

    fn enter(&mut self, depth: usize) -> Result<(), CertificateNounError> {
        if depth > self.limits.max_depth {
            return Err(CertificateNounError::LimitExceeded("proof-node depth"));
        }
        self.total_nodes = self
            .total_nodes
            .checked_add(1)
            .ok_or(CertificateNounError::LimitExceeded("proof-node count"))?;
        if self.total_nodes > self.limits.max_total_nodes {
            return Err(CertificateNounError::LimitExceeded("proof-node count"));
        }
        Ok(())
    }

    /// Charge `n` decoded atom bytes against the cumulative budget (audit N9).
    /// A single node is already capped by `max_atom_bytes`; this bounds the SUM
    /// across all nodes so a many-node jam cannot drive unbounded allocation.
    fn charge_atom_bytes(&mut self, n: usize) -> Result<(), CertificateNounError> {
        self.total_atom_bytes = self
            .total_atom_bytes
            .checked_add(n)
            .ok_or(CertificateNounError::LimitExceeded("cumulative atom bytes"))?;
        if self.total_atom_bytes > self.limits.max_total_atom_bytes {
            return Err(CertificateNounError::LimitExceeded("cumulative atom bytes"));
        }
        Ok(())
    }
}

fn decode_params(noun: Noun, space: &NounSpace) -> Result<ZkParams, CertificateNounError> {
    let fields = tuple6(noun, space, "zk-params")?;
    Ok(ZkParams {
        m: u32::try_from(expect_u64(fields[0], space, "params.m")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "params.m" })?,
        k: u32::try_from(expect_u64(fields[1], space, "params.k")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "params.k" })?,
        n: u32::try_from(expect_u64(fields[2], space, "params.n")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "params.n" })?,
        noise_rank: u32::try_from(expect_u64(fields[3], space, "params.noise-rank")?).map_err(
            |_| CertificateNounError::IntegerOutOfRange {
                field: "params.noise-rank",
            },
        )?,
        tile: u32::try_from(expect_u64(fields[4], space, "params.tile")?).map_err(|_| {
            CertificateNounError::IntegerOutOfRange {
                field: "params.tile",
            }
        })?,
        difficulty_bits: u32::try_from(expect_u64(fields[5], space, "params.difficulty-bits")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange {
                field: "params.difficulty-bits",
            })?,
    })
}

fn decode_commitments(
    noun: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<ZkPublicCommitments, CertificateNounError> {
    let fields = tuple2(noun, space, "ai-pow-commitments")?;
    Ok(ZkPublicCommitments {
        h_a_chunk: expect_fixed_bytes(fields[0], space, "commitments.h-a-chunk", limits)?,
        h_b_chunk: expect_fixed_bytes(fields[1], space, "commitments.h-b-chunk", limits)?,
    })
}

fn decode_public_inputs(
    noun: Noun,
    space: &NounSpace,
    limits: CertificateNounLimits,
) -> Result<CompositePublicInputs, CertificateNounError> {
    let fields = tuple7(noun, space, "ai-pow-public-inputs")?;
    let cumsum_fields = tuple4(fields[0], space, "public-inputs.cumsum")?;
    let jackpot_fields = tuple16(fields[1], space, "public-inputs.jackpot")?;
    let mut jackpot = [0u32; 16];
    for (i, field) in jackpot_fields.iter().enumerate() {
        jackpot[i] = u32::try_from(expect_u64(*field, space, "jackpot")?)
            .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "jackpot" })?;
    }
    Ok(CompositePublicInputs {
        cumsum: [
            i32::try_from(expect_i64(cumsum_fields[0], space, "cumsum.0", limits)?)
                .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "cumsum.0" })?,
            i32::try_from(expect_i64(cumsum_fields[1], space, "cumsum.1", limits)?)
                .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "cumsum.1" })?,
            i32::try_from(expect_i64(cumsum_fields[2], space, "cumsum.2", limits)?)
                .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "cumsum.2" })?,
            i32::try_from(expect_i64(cumsum_fields[3], space, "cumsum.3", limits)?)
                .map_err(|_| CertificateNounError::IntegerOutOfRange { field: "cumsum.3" })?,
        ],
        jackpot,
        hash_a: decode_digest_words(fields[2], space, "hash-a", limits)?,
        hash_b: decode_digest_words(fields[3], space, "hash-b", limits)?,
        job_key: decode_digest_words(fields[4], space, "job-key", limits)?,
        commitment_hash: decode_digest_words(fields[5], space, "commitment-hash", limits)?,
        hash_jackpot: decode_digest_words(fields[6], space, "hash-jackpot", limits)?,
    })
}

fn decode_digest_words(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
    limits: CertificateNounLimits,
) -> Result<[u32; 8], CertificateNounError> {
    let bytes = expect_fixed_bytes::<32>(noun, space, field, limits)?;
    Ok(core::array::from_fn(|i| {
        u32::from_le_bytes(bytes[i * 4..(i + 1) * 4].try_into().expect("slice len"))
    }))
}

fn decode_proof_node(
    noun: Noun,
    space: &NounSpace,
    state: &mut DecodeState,
    depth: usize,
) -> Result<AiProofNode, CertificateNounError> {
    state.enter(depth)?;
    let cell = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape("proof node must be a tagged cell"))?;
    let tag = expect_u64(cell.head().noun(), space, "proof-node tag")?;
    let tail = cell.tail().noun();
    match tag {
        x if x == tas!(b"n") => {
            expect_nil(tail, space, "unit proof-node tail")?;
            Ok(AiProofNode::Unit)
        }
        x if x == tas!(b"b") => match expect_u64(tail, space, "bool proof-node value")? {
            0 => Ok(AiProofNode::Bool(false)),
            1 => Ok(AiProofNode::Bool(true)),
            _ => Err(CertificateNounError::Shape(
                "bool proof-node value must be 0 or 1",
            )),
        },
        x if x == tas!(b"u") => Ok(AiProofNode::U64(expect_u64(
            tail, space, "u64 proof-node value",
        )?)),
        x if x == tas!(b"i") => Ok(AiProofNode::I64(expect_i64(
            tail, space, "i64 proof-node value", state.limits,
        )?)),
        x if x == tas!(b"ext2") => Ok(AiProofNode::Ext2(expect_ext2(
            tail, space, "ext2", state.limits,
        )?)),
        x if x == tas!(b"ext2s") => {
            let fields = tuple2(tail, space, "ext2s proof-node")?;
            let len = expect_len(
                fields[0], space, "ext2s length", state.limits.max_packed_items,
            )?;
            let bytes = expect_declared_bytes(
                fields[1],
                space,
                len.checked_mul(16)
                    .ok_or(CertificateNounError::LimitExceeded("ext2s bytes"))?,
                "ext2s",
                state.limits,
            )?;
            state.charge_atom_bytes(bytes.len())?;
            let mut values = Vec::with_capacity(len);
            for chunk in bytes.chunks_exact(16) {
                let c0 = u64::from_le_bytes(chunk[0..8].try_into().expect("chunk len"));
                let c1 = u64::from_le_bytes(chunk[8..16].try_into().expect("chunk len"));
                expect_goldilocks(c0, "ext2s.c0")?;
                expect_goldilocks(c1, "ext2s.c1")?;
                values.push([c0, c1]);
            }
            Ok(AiProofNode::Ext2s(values))
        }
        x if x == tas!(b"bytes") => {
            let fields = tuple2(tail, space, "bytes proof-node")?;
            let len = expect_len(
                fields[0], space, "bytes length", state.limits.max_packed_items,
            )?;
            let bytes = expect_declared_bytes(fields[1], space, len, "bytes", state.limits)?;
            state.charge_atom_bytes(bytes.len())?;
            Ok(AiProofNode::Bytes(bytes))
        }
        x if x == tas!(b"u64s") => {
            let fields = tuple2(tail, space, "u64s proof-node")?;
            let len = expect_len(
                fields[0], space, "u64s length", state.limits.max_packed_items,
            )?;
            let bytes = expect_declared_bytes(
                fields[1],
                space,
                len.checked_mul(8)
                    .ok_or(CertificateNounError::LimitExceeded("u64s bytes"))?,
                "u64s",
                state.limits,
            )?;
            state.charge_atom_bytes(bytes.len())?;
            Ok(AiProofNode::U64s(
                bytes
                    .chunks_exact(8)
                    .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("chunk len")))
                    .collect(),
            ))
        }
        x if x == tas!(b"i64s") => {
            let fields = tuple2(tail, space, "i64s proof-node")?;
            let len = expect_len(
                fields[0], space, "i64s length", state.limits.max_packed_items,
            )?;
            let bytes = expect_declared_bytes(
                fields[1],
                space,
                len.checked_mul(8)
                    .ok_or(CertificateNounError::LimitExceeded("i64s bytes"))?,
                "i64s",
                state.limits,
            )?;
            state.charge_atom_bytes(bytes.len())?;
            Ok(AiProofNode::I64s(
                bytes
                    .chunks_exact(8)
                    .map(|chunk| i64::from_le_bytes(chunk.try_into().expect("chunk len")))
                    .collect(),
            ))
        }
        x if x == tas!(b"seq") => Ok(AiProofNode::Seq(decode_list(tail, space, state, depth)?)),
        x if x == tas!(b"map") => Ok(AiProofNode::Map(decode_map_entries(
            tail, space, state, depth,
        )?)),
        x if x == tas!(b"none") => {
            expect_nil(tail, space, "none proof-node tail")?;
            Ok(AiProofNode::None)
        }
        x if x == tas!(b"some") => Ok(AiProofNode::Some(Box::new(decode_proof_node(
            tail,
            space,
            state,
            depth + 1,
        )?))),
        other => Err(CertificateNounError::InvalidTag(other)),
    }
}

fn decode_list(
    mut list: Noun,
    space: &NounSpace,
    state: &mut DecodeState,
    depth: usize,
) -> Result<Vec<AiProofNode>, CertificateNounError> {
    let mut out = Vec::new();
    while !is_nil(list, space)? {
        if out.len() >= state.limits.max_list_items {
            return Err(CertificateNounError::LimitExceeded(
                "proof-node list length",
            ));
        }
        let cell = list
            .in_space(space)
            .as_cell()
            .map_err(|_| CertificateNounError::Shape("improper proof-node list"))?;
        out.push(decode_proof_node(
            cell.head().noun(),
            space,
            state,
            depth + 1,
        )?);
        list = cell.tail().noun();
    }
    Ok(out)
}

fn decode_map_entries(
    mut list: Noun,
    space: &NounSpace,
    state: &mut DecodeState,
    depth: usize,
) -> Result<Vec<(AiProofNode, AiProofNode)>, CertificateNounError> {
    let mut out = Vec::new();
    while !is_nil(list, space)? {
        if out.len() >= state.limits.max_list_items {
            return Err(CertificateNounError::LimitExceeded("proof-node map length"));
        }
        let cell = list
            .in_space(space)
            .as_cell()
            .map_err(|_| CertificateNounError::Shape("improper proof-node map"))?;
        let pair = tuple2(cell.head().noun(), space, "proof-node map entry")?;
        let key = decode_proof_node(pair[0], space, state, depth + 1)?;
        let value = decode_proof_node(pair[1], space, state, depth + 1)?;
        out.push((key, value));
        list = cell.tail().noun();
    }
    Ok(out)
}

fn tuple2(
    noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 2], CertificateNounError> {
    let c = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    Ok([c.head().noun(), c.tail().noun()])
}

fn tuple3(
    noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 3], CertificateNounError> {
    let c1 = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c2 = c1
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    Ok([c1.head().noun(), c2.head().noun(), c2.tail().noun()])
}

fn tuple4(
    noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 4], CertificateNounError> {
    let c1 = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c2 = c1
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c3 = c2
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    Ok([c1.head().noun(), c2.head().noun(), c3.head().noun(), c3.tail().noun()])
}

fn tuple6(
    noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 6], CertificateNounError> {
    let c1 = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c2 = c1
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c3 = c2
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c4 = c3
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c5 = c4
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    Ok([
        c1.head().noun(),
        c2.head().noun(),
        c3.head().noun(),
        c4.head().noun(),
        c5.head().noun(),
        c5.tail().noun(),
    ])
}

fn tuple7(
    noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 7], CertificateNounError> {
    let c1 = noun
        .in_space(space)
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c2 = c1
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c3 = c2
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c4 = c3
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c5 = c4
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    let c6 = c5
        .tail()
        .as_cell()
        .map_err(|_| CertificateNounError::Shape(name))?;
    Ok([
        c1.head().noun(),
        c2.head().noun(),
        c3.head().noun(),
        c4.head().noun(),
        c5.head().noun(),
        c6.head().noun(),
        c6.tail().noun(),
    ])
}

fn tuple16(
    mut noun: Noun,
    space: &NounSpace,
    name: &'static str,
) -> Result<[Noun; 16], CertificateNounError> {
    let mut fields = [D(0); 16];
    for field in fields.iter_mut().take(15) {
        let cell = noun
            .in_space(space)
            .as_cell()
            .map_err(|_| CertificateNounError::Shape(name))?;
        *field = cell.head().noun();
        noun = cell.tail().noun();
    }
    fields[15] = noun;
    Ok(fields)
}

fn expect_u64(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
) -> Result<u64, CertificateNounError> {
    noun.in_space(space)
        .as_atom()
        .and_then(|atom| atom.as_u64())
        .map_err(|_| CertificateNounError::IntegerOutOfRange { field })
}

fn expect_len(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
    max: usize,
) -> Result<usize, CertificateNounError> {
    let len = usize::try_from(expect_u64(noun, space, field)?)
        .map_err(|_| CertificateNounError::IntegerOutOfRange { field })?;
    if len > max {
        return Err(CertificateNounError::LimitExceeded(field));
    }
    Ok(len)
}

fn expect_i64(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
    limits: CertificateNounLimits,
) -> Result<i64, CertificateNounError> {
    let bytes = expect_declared_bytes(noun, space, 8, field, limits)?;
    Ok(i64::from_le_bytes(bytes.try_into().expect("declared len")))
}

fn expect_ext2(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
    limits: CertificateNounLimits,
) -> Result<[u64; 2], CertificateNounError> {
    let bytes = expect_declared_bytes(noun, space, 16, field, limits)?;
    let c0 = u64::from_le_bytes(bytes[0..8].try_into().expect("chunk len"));
    let c1 = u64::from_le_bytes(bytes[8..16].try_into().expect("chunk len"));
    expect_goldilocks(c0, "ext2.c0")?;
    expect_goldilocks(c1, "ext2.c1")?;
    Ok([c0, c1])
}

fn expect_goldilocks(value: u64, field: &'static str) -> Result<(), CertificateNounError> {
    if value < GOLDILOCKS_MODULUS {
        Ok(())
    } else {
        Err(CertificateNounError::NonCanonicalField { field })
    }
}

fn expect_fixed_bytes<const N: usize>(
    noun: Noun,
    space: &NounSpace,
    tag: &'static str,
    limits: CertificateNounLimits,
) -> Result<[u8; N], CertificateNounError> {
    let bytes = expect_declared_bytes(noun, space, N, tag, limits)?;
    Ok(bytes.try_into().expect("declared len"))
}

fn expect_declared_bounded_bytes(
    noun: Noun,
    space: &NounSpace,
    min: usize,
    max: usize,
    tag: &'static str,
    limits: CertificateNounLimits,
) -> Result<Vec<u8>, CertificateNounError> {
    let fields = tuple2(noun, space, tag)?;
    let declared = usize::try_from(expect_u64(fields[0], space, tag)?)
        .map_err(|_| CertificateNounError::IntegerOutOfRange { field: tag })?;
    if declared < min || declared > max {
        return Err(CertificateNounError::PackedLengthMismatch {
            tag,
            declared: max,
            actual: declared,
        });
    }
    if max > limits.max_atom_bytes {
        return Err(CertificateNounError::LimitExceeded("atom bytes"));
    }
    let atom = fields[1]
        .in_space(space)
        .as_atom()
        .map_err(|_| CertificateNounError::Shape("expected atom bytes"))?;
    let actual = met(3, atom.atom(), space);
    if actual > declared {
        return Err(CertificateNounError::PackedLengthMismatch {
            tag,
            declared,
            actual,
        });
    }
    if actual > limits.max_atom_bytes {
        return Err(CertificateNounError::LimitExceeded("atom bytes"));
    }
    let mut out = vec![0u8; declared];
    out[..actual].copy_from_slice(&atom.as_ne_bytes()[..actual]);
    Ok(out)
}

fn expect_declared_bytes(
    noun: Noun,
    space: &NounSpace,
    declared: usize,
    tag: &'static str,
    limits: CertificateNounLimits,
) -> Result<Vec<u8>, CertificateNounError> {
    if declared > limits.max_atom_bytes {
        return Err(CertificateNounError::LimitExceeded("atom bytes"));
    }
    let atom = noun
        .in_space(space)
        .as_atom()
        .map_err(|_| CertificateNounError::Shape("expected atom bytes"))?;
    let actual = met(3, atom.atom(), space);
    if actual > declared {
        return Err(CertificateNounError::PackedLengthMismatch {
            tag,
            declared,
            actual,
        });
    }
    if actual > limits.max_atom_bytes {
        return Err(CertificateNounError::LimitExceeded("atom bytes"));
    }
    let mut out = vec![0u8; declared];
    out[..actual].copy_from_slice(&atom.as_ne_bytes()[..actual]);
    Ok(out)
}

fn expect_nil(
    noun: Noun,
    space: &NounSpace,
    field: &'static str,
) -> Result<(), CertificateNounError> {
    if is_nil(noun, space)? {
        Ok(())
    } else {
        Err(CertificateNounError::Shape(field))
    }
}

fn is_nil(noun: Noun, space: &NounSpace) -> Result<bool, CertificateNounError> {
    match noun.in_space(space).as_atom() {
        Ok(atom) => Ok(atom.as_u64().map(|value| value == 0).unwrap_or(false)),
        Err(_) => Ok(false),
    }
}

impl AiProofNode {
    fn normalized(self) -> Self {
        match self {
            AiProofNode::Seq(items) => normalize_seq(items),
            AiProofNode::Map(items) => AiProofNode::Map(
                items
                    .into_iter()
                    .map(|(k, v)| (k.normalized(), v.normalized()))
                    .collect(),
            ),
            AiProofNode::Some(inner) => AiProofNode::Some(Box::new(inner.normalized())),
            other => other,
        }
    }

    pub fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            AiProofNode::Unit => T(allocator, &[D(tas!(b"n")), D(0)]),
            AiProofNode::Bool(value) => T(allocator, &[D(tas!(b"b")), D(u64::from(*value))]),
            AiProofNode::U64(value) => T(allocator, &[D(tas!(b"u")), D(*value)]),
            AiProofNode::I64(value) => {
                let data = i64_to_atom(allocator, *value);
                T(allocator, &[D(tas!(b"i")), data])
            }
            AiProofNode::Ext2(value) => {
                let data = ext2_to_atom(allocator, *value);
                T(allocator, &[D(tas!(b"ext2")), data])
            }
            AiProofNode::Ext2s(values) => {
                let data = packed_ext2s_to_atom(allocator, values);
                T(
                    allocator,
                    &[D(tas!(b"ext2s")), D(values.len() as u64), data],
                )
            }
            AiProofNode::Bytes(bytes) => {
                let data = bytes_to_atom(allocator, bytes);
                T(allocator, &[D(tas!(b"bytes")), D(bytes.len() as u64), data])
            }
            AiProofNode::U64s(values) => {
                let data = packed_u64s_to_atom(allocator, values);
                T(allocator, &[D(tas!(b"u64s")), D(values.len() as u64), data])
            }
            AiProofNode::I64s(values) => {
                let data = packed_i64s_to_atom(allocator, values);
                T(allocator, &[D(tas!(b"i64s")), D(values.len() as u64), data])
            }
            AiProofNode::Seq(items) => {
                let list = encode_list(allocator, items, |allocator, item| item.to_noun(allocator));
                T(allocator, &[D(tas!(b"seq")), list])
            }
            AiProofNode::Map(items) => {
                let list = encode_list(allocator, items, |allocator, (k, v)| {
                    let key = k.to_noun(allocator);
                    let value = v.to_noun(allocator);
                    T(allocator, &[key, value])
                });
                T(allocator, &[D(tas!(b"map")), list])
            }
            AiProofNode::None => T(allocator, &[D(tas!(b"none")), D(0)]),
            AiProofNode::Some(inner) => {
                let inner = inner.to_noun(allocator);
                T(allocator, &[D(tas!(b"some")), inner])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeqKind {
    Seq,
    Tuple,
    Struct,
}

fn normalize_seq(items: Vec<AiProofNode>) -> AiProofNode {
    normalize_seq_with_kind(items, SeqKind::Seq)
}

fn normalize_seq_with_kind(items: Vec<AiProofNode>, kind: SeqKind) -> AiProofNode {
    let items = items
        .into_iter()
        .filter_map(|item| match item.normalized() {
            AiProofNode::Unit => None,
            other => Some(other),
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return AiProofNode::Seq(items);
    }
    if matches!(kind, SeqKind::Tuple) && items.len() == 2 {
        if let [AiProofNode::U64(c0), AiProofNode::U64(c1)] = items.as_slice() {
            return AiProofNode::Ext2([*c0, *c1]);
        }
    }
    if items
        .iter()
        .all(|item| matches!(item, AiProofNode::Ext2(_)))
    {
        return AiProofNode::Ext2s(
            items
                .into_iter()
                .map(|item| match item {
                    AiProofNode::Ext2(value) => value,
                    _ => unreachable!(),
                })
                .collect(),
        );
    }
    if items.iter().all(|item| matches!(item, AiProofNode::U64(_))) {
        return AiProofNode::U64s(
            items
                .into_iter()
                .map(|item| match item {
                    AiProofNode::U64(value) => value,
                    _ => unreachable!(),
                })
                .collect(),
        );
    }
    if items.iter().all(|item| matches!(item, AiProofNode::I64(_))) {
        return AiProofNode::I64s(
            items
                .into_iter()
                .map(|item| match item {
                    AiProofNode::I64(value) => value,
                    _ => unreachable!(),
                })
                .collect(),
        );
    }
    AiProofNode::Seq(items)
}

fn encode_params<A: NounAllocator>(allocator: &mut A, params: &ZkParams) -> Noun {
    T(
        allocator,
        &[
            D(params.m as u64),
            D(params.k as u64),
            D(params.n as u64),
            D(params.noise_rank as u64),
            D(params.tile as u64),
            D(params.difficulty_bits as u64),
        ],
    )
}

fn encode_ai_pow_certificate_noun<A: NounAllocator>(
    allocator: &mut A,
    zk_params: &ZkParams,
    found_idx: u32,
    trace_height: usize,
    commitments: &ZkPublicCommitments,
    pis: &CompositePublicInputs,
    certificate: &AiProofNode,
) -> Noun {
    let params = encode_params(allocator, zk_params);
    let commitments = encode_commitments(allocator, commitments);
    let public_inputs = encode_public_inputs(allocator, pis);
    let certificate = certificate.to_noun(allocator);
    T(
        allocator,
        &[
            D(AI_POW_CERT_VERSION),
            params,
            D(found_idx as u64),
            D(trace_height as u64),
            commitments,
            public_inputs,
            certificate,
        ],
    )
}

fn encode_commitments<A: NounAllocator>(
    allocator: &mut A,
    commitments: &ZkPublicCommitments,
) -> Noun {
    let h_a_chunk = bytes_to_atom(allocator, &commitments.h_a_chunk);
    let h_b_chunk = bytes_to_atom(allocator, &commitments.h_b_chunk);
    T(allocator, &[h_a_chunk, h_b_chunk])
}

fn encode_public_inputs<A: NounAllocator>(allocator: &mut A, pis: &CompositePublicInputs) -> Noun {
    let c0 = i64_to_atom(allocator, pis.cumsum[0] as i64);
    let c1 = i64_to_atom(allocator, pis.cumsum[1] as i64);
    let c2 = i64_to_atom(allocator, pis.cumsum[2] as i64);
    let c3 = i64_to_atom(allocator, pis.cumsum[3] as i64);
    let cumsum = T(allocator, &[c0, c1, c2, c3]);
    let jackpot = T(
        allocator,
        &[
            D(pis.jackpot[0] as u64),
            D(pis.jackpot[1] as u64),
            D(pis.jackpot[2] as u64),
            D(pis.jackpot[3] as u64),
            D(pis.jackpot[4] as u64),
            D(pis.jackpot[5] as u64),
            D(pis.jackpot[6] as u64),
            D(pis.jackpot[7] as u64),
            D(pis.jackpot[8] as u64),
            D(pis.jackpot[9] as u64),
            D(pis.jackpot[10] as u64),
            D(pis.jackpot[11] as u64),
            D(pis.jackpot[12] as u64),
            D(pis.jackpot[13] as u64),
            D(pis.jackpot[14] as u64),
            D(pis.jackpot[15] as u64),
        ],
    );
    let hash_a = bytes_to_atom(allocator, &digest_words_to_bytes(&pis.hash_a));
    let hash_b = bytes_to_atom(allocator, &digest_words_to_bytes(&pis.hash_b));
    let job_key = bytes_to_atom(allocator, &digest_words_to_bytes(&pis.job_key));
    let commitment_hash = bytes_to_atom(allocator, &digest_words_to_bytes(&pis.commitment_hash));
    let hash_jackpot = bytes_to_atom(allocator, &digest_words_to_bytes(&pis.hash_jackpot));
    T(
        allocator,
        &[cumsum, jackpot, hash_a, hash_b, job_key, commitment_hash, hash_jackpot],
    )
}

fn digest_words_to_bytes(words: &[u32; 8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, word) in words.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

fn digest_words(bytes: &[u8; 32]) -> [u32; 8] {
    core::array::from_fn(|i| {
        u32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]])
    })
}

fn encode_list<A, T, F>(allocator: &mut A, items: &[T], mut encode: F) -> Noun
where
    A: NounAllocator,
    F: FnMut(&mut A, &T) -> Noun,
{
    let mut list = D(0);
    for item in items.iter().rev() {
        let head = encode(allocator, item);
        list = T(allocator, &[head, list]);
    }
    list
}

fn bytes_to_atom<A: NounAllocator>(allocator: &mut A, bytes: &[u8]) -> Noun {
    use nockvm::noun::IndirectAtom;
    let atom = <IndirectAtom as nockapp::IndirectAtomExt>::from_bytes(allocator, bytes);
    atom.as_noun()
}

fn packed_u64s_to_atom<A: NounAllocator>(allocator: &mut A, values: &[u64]) -> Noun {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes_to_atom(allocator, &bytes)
}

fn packed_i64s_to_atom<A: NounAllocator>(allocator: &mut A, values: &[i64]) -> Noun {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes_to_atom(allocator, &bytes)
}

fn ext2_to_atom<A: NounAllocator>(allocator: &mut A, value: [u64; 2]) -> Noun {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&value[0].to_le_bytes());
    bytes.extend_from_slice(&value[1].to_le_bytes());
    bytes_to_atom(allocator, &bytes)
}

fn packed_ext2s_to_atom<A: NounAllocator>(allocator: &mut A, values: &[[u64; 2]]) -> Noun {
    let mut bytes = Vec::with_capacity(values.len() * 16);
    for value in values {
        bytes.extend_from_slice(&value[0].to_le_bytes());
        bytes.extend_from_slice(&value[1].to_le_bytes());
    }
    bytes_to_atom(allocator, &bytes)
}

fn i64_to_atom<A: NounAllocator>(allocator: &mut A, value: i64) -> Noun {
    bytes_to_atom(allocator, &value.to_le_bytes())
}

#[derive(Debug)]
struct SerError(String);

impl std::fmt::Display for SerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SerError {}

impl ser::Error for SerError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

type SerResult<T> = Result<T, SerError>;

struct NodeSerializer;

impl ser::Serializer for NodeSerializer {
    type Ok = AiProofNode;
    type Error = SerError;
    type SerializeSeq = NodeSeq;
    type SerializeTuple = NodeSeq;
    type SerializeTupleStruct = NodeSeq;
    type SerializeTupleVariant = NodeSeq;
    type SerializeMap = NodeMap;
    type SerializeStruct = NodeSeq;
    type SerializeStructVariant = NodeSeq;

    fn serialize_bool(self, v: bool) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> SerResult<Self::Ok> {
        Ok(AiProofNode::I64(v as i64))
    }
    fn serialize_i16(self, v: i16) -> SerResult<Self::Ok> {
        Ok(AiProofNode::I64(v as i64))
    }
    fn serialize_i32(self, v: i32) -> SerResult<Self::Ok> {
        Ok(AiProofNode::I64(v as i64))
    }
    fn serialize_i64(self, v: i64) -> SerResult<Self::Ok> {
        Ok(AiProofNode::I64(v))
    }
    fn serialize_i128(self, v: i128) -> SerResult<Self::Ok> {
        let value = i64::try_from(v)
            .map_err(|_| SerError("i128 values are not valid in AI-PoW certificates".into()))?;
        Ok(AiProofNode::I64(value))
    }
    fn serialize_u8(self, v: u8) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(v as u64))
    }
    fn serialize_u16(self, v: u16) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(v as u64))
    }
    fn serialize_u32(self, v: u32) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(v as u64))
    }
    fn serialize_u64(self, v: u64) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(v))
    }
    fn serialize_u128(self, v: u128) -> SerResult<Self::Ok> {
        let value = u64::try_from(v)
            .map_err(|_| SerError("u128 values are not valid in AI-PoW certificates".into()))?;
        Ok(AiProofNode::U64(value))
    }
    fn serialize_f32(self, _v: f32) -> SerResult<Self::Ok> {
        Err(SerError(
            "floating-point values are not valid in AI-PoW certificates".into(),
        ))
    }
    fn serialize_f64(self, _v: f64) -> SerResult<Self::Ok> {
        Err(SerError(
            "floating-point values are not valid in AI-PoW certificates".into(),
        ))
    }
    fn serialize_char(self, v: char) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(v as u32 as u64))
    }
    fn serialize_str(self, v: &str) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Bytes(v.as_bytes().to_vec()))
    }
    fn serialize_bytes(self, v: &[u8]) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> SerResult<Self::Ok> {
        Ok(AiProofNode::None)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Some(Box::new(
            value.serialize(NodeSerializer)?,
        )))
    }
    fn serialize_unit(self) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Unit)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Unit)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
    ) -> SerResult<Self::Ok> {
        Ok(AiProofNode::U64(variant_index as u64))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> SerResult<Self::Ok> {
        value.serialize(NodeSerializer)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        value: &T,
    ) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Seq(vec![
            AiProofNode::U64(variant_index as u64),
            value.serialize(NodeSerializer)?,
        ]))
    }
    fn serialize_seq(self, _len: Option<usize>) -> SerResult<Self::SerializeSeq> {
        Ok(NodeSeq::default())
    }
    fn serialize_tuple(self, _len: usize) -> SerResult<Self::SerializeTuple> {
        Ok(NodeSeq::new(SeqKind::Tuple))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> SerResult<Self::SerializeTupleStruct> {
        Ok(NodeSeq::new(SeqKind::Tuple))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> SerResult<Self::SerializeTupleVariant> {
        Ok(NodeSeq {
            items: vec![AiProofNode::U64(variant_index as u64)],
            kind: SeqKind::Tuple,
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> SerResult<Self::SerializeMap> {
        Ok(NodeMap::default())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> SerResult<Self::SerializeStruct> {
        Ok(NodeSeq::new(SeqKind::Struct))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> SerResult<Self::SerializeStructVariant> {
        Ok(NodeSeq {
            items: vec![AiProofNode::U64(variant_index as u64)],
            kind: SeqKind::Struct,
        })
    }
}

struct NodeSeq {
    items: Vec<AiProofNode>,
    kind: SeqKind,
}

impl NodeSeq {
    fn new(kind: SeqKind) -> Self {
        Self {
            items: Vec::new(),
            kind,
        }
    }
}

impl Default for NodeSeq {
    fn default() -> Self {
        Self::new(SeqKind::Seq)
    }
}

impl SerializeSeq for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> SerResult<()> {
        self.items.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> SerResult<Self::Ok> {
        Ok(normalize_seq_with_kind(self.items, self.kind))
    }
}

impl SerializeTuple for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> SerResult<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> SerResult<Self::Ok> {
        SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> SerResult<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> SerResult<Self::Ok> {
        SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleVariant for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> SerResult<()> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> SerResult<Self::Ok> {
        SerializeSeq::end(self)
    }
}

impl SerializeStruct for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> SerResult<()> {
        self.items.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> SerResult<Self::Ok> {
        SerializeSeq::end(self)
    }
}

impl ser::SerializeStructVariant for NodeSeq {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> SerResult<()> {
        SerializeStruct::serialize_field(self, _key, value)
    }

    fn end(self) -> SerResult<Self::Ok> {
        SerializeSeq::end(self)
    }
}

#[derive(Default)]
struct NodeMap {
    entries: Vec<(AiProofNode, AiProofNode)>,
    next_key: Option<AiProofNode>,
}

impl SerializeMap for NodeMap {
    type Ok = AiProofNode;
    type Error = SerError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> SerResult<()> {
        self.next_key = Some(key.serialize(NodeSerializer)?);
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> SerResult<()> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| SerError("serialize_value called before serialize_key".into()))?;
        self.entries.push((key, value.serialize(NodeSerializer)?));
        Ok(())
    }

    fn end(self) -> SerResult<Self::Ok> {
        Ok(AiProofNode::Map(self.entries))
    }
}

#[derive(Debug)]
#[allow(dead_code)] // used under some feature/target configs only
struct DeError(String);

impl std::fmt::Display for DeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

#[allow(dead_code)] // used under some feature/target configs only
type DeResult<T> = Result<T, DeError>;

#[allow(dead_code)] // used under some feature/target configs only
struct NodeDeserializer {
    node: AiProofNode,
}

impl<'de> de::Deserializer<'de> for NodeDeserializer {
    type Error = DeError;

    fn deserialize_any<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Unit => visitor.visit_unit(),
            AiProofNode::Bool(value) => visitor.visit_bool(*value),
            AiProofNode::U64(value) => visitor.visit_u64(*value),
            AiProofNode::I64(value) => visitor.visit_i64(*value),
            AiProofNode::Ext2(value) => visitor.visit_seq(NodeSeqAccess::from_owned(vec![
                AiProofNode::U64(value[0]),
                AiProofNode::U64(value[1]),
            ])),
            AiProofNode::Ext2s(values) => visitor.visit_seq(NodeSeqAccess::from_owned(
                values.iter().copied().map(AiProofNode::Ext2).collect(),
            )),
            AiProofNode::Bytes(bytes) => visitor.visit_byte_buf(bytes.clone()),
            AiProofNode::U64s(values) => visitor.visit_seq(NodeSeqAccess::from_owned(
                values.iter().copied().map(AiProofNode::U64).collect(),
            )),
            AiProofNode::I64s(values) => visitor.visit_seq(NodeSeqAccess::from_owned(
                values.iter().copied().map(AiProofNode::I64).collect(),
            )),
            AiProofNode::Seq(items) => visitor.visit_seq(NodeSeqAccess::from_slice(items)),
            AiProofNode::Map(items) => visitor.visit_map(NodeMapAccess::new(items)),
            AiProofNode::None => visitor.visit_none(),
            AiProofNode::Some(inner) => visitor.visit_some(NodeDeserializer {
                node: inner.as_ref().clone(),
            }),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Bool(value) => visitor.visit_bool(*value),
            other => Err(DeError(format!("expected bool, got {other:?}"))),
        }
    }

    fn deserialize_i8<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i16<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i32<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_i64(visitor)
    }

    fn deserialize_i64<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::I64(value) => visitor.visit_i64(*value),
            AiProofNode::U64(value) => {
                let value = i64::try_from(*value)
                    .map_err(|_| DeError("unsigned integer does not fit i64".into()))?;
                visitor.visit_i64(value)
            }
            other => Err(DeError(format!("expected signed integer, got {other:?}"))),
        }
    }

    fn deserialize_i128<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::I64(value) => visitor.visit_i128(*value as i128),
            AiProofNode::U64(value) => visitor.visit_i128(*value as i128),
            other => Err(DeError(format!(
                "expected i128-compatible integer, got {other:?}"
            ))),
        }
    }

    fn deserialize_u8<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u16<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u32<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u64<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::U64(value) => visitor.visit_u64(*value),
            AiProofNode::I64(value) => {
                let value = u64::try_from(*value)
                    .map_err(|_| DeError("negative integer does not fit u64".into()))?;
                visitor.visit_u64(value)
            }
            other => Err(DeError(format!("expected unsigned integer, got {other:?}"))),
        }
    }

    fn deserialize_u128<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::U64(value) => visitor.visit_u128(*value as u128),
            AiProofNode::I64(value) => {
                let value = u128::try_from(*value)
                    .map_err(|_| DeError("negative integer does not fit u128".into()))?;
                visitor.visit_u128(value)
            }
            other => Err(DeError(format!(
                "expected u128-compatible integer, got {other:?}"
            ))),
        }
    }

    fn deserialize_f32<V>(self, _visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(DeError(
            "floating-point values are not valid in AI-PoW certificates".into(),
        ))
    }

    fn deserialize_f64<V>(self, _visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        Err(DeError(
            "floating-point values are not valid in AI-PoW certificates".into(),
        ))
    }

    fn deserialize_char<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        let value = match &self.node {
            AiProofNode::U64(value) => u32::try_from(*value)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| DeError("invalid char scalar value".into()))?,
            other => return Err(DeError(format!("expected char integer, got {other:?}"))),
        };
        visitor.visit_char(value)
    }

    fn deserialize_str<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Bytes(bytes) => {
                let s = std::str::from_utf8(bytes)
                    .map_err(|e| DeError(format!("invalid utf8 string: {e}")))?;
                visitor.visit_str(s)
            }
            other => Err(DeError(format!("expected string bytes, got {other:?}"))),
        }
    }

    fn deserialize_string<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Bytes(bytes) => {
                let s = String::from_utf8(bytes.clone())
                    .map_err(|e| DeError(format!("invalid utf8 string: {e}")))?;
                visitor.visit_string(s)
            }
            other => Err(DeError(format!("expected string bytes, got {other:?}"))),
        }
    }

    fn deserialize_bytes<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Bytes(bytes) => visitor.visit_bytes(bytes),
            AiProofNode::U64s(values) => {
                let mut bytes = Vec::with_capacity(values.len());
                for &value in values {
                    bytes.push(
                        u8::try_from(value)
                            .map_err(|_| DeError("packed byte value out of range".into()))?,
                    );
                }
                visitor.visit_byte_buf(bytes)
            }
            other => Err(DeError(format!("expected bytes, got {other:?}"))),
        }
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::None => visitor.visit_none(),
            AiProofNode::Some(inner) => visitor.visit_some(NodeDeserializer {
                node: inner.as_ref().clone(),
            }),
            other => visitor.visit_some(NodeDeserializer {
                node: other.clone(),
            }),
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Unit => visitor.visit_unit(),
            other => Err(DeError(format!("expected unit, got {other:?}"))),
        }
    }

    fn deserialize_unit_struct<V>(self, _name: &'static str, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(self, _name: &'static str, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(NodeSeqAccess::for_node(&self.node))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        match &self.node {
            AiProofNode::Map(items) => visitor.visit_map(NodeMapAccess::new(items)),
            other => Err(DeError(format!("expected map, got {other:?}"))),
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(NodeSeqAccess::for_struct_node(&self.node, fields.len()))
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_enum(NodeEnumAccess::new(&self.node)?)
    }

    fn deserialize_identifier<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        self.deserialize_u32(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

#[allow(dead_code)] // used under some feature/target configs only
struct NodeSeqAccess {
    items: Vec<AiProofNode>,
    index: usize,
}

impl NodeSeqAccess {
    fn from_slice(items: &[AiProofNode]) -> Self {
        Self {
            items: items.to_vec(),
            index: 0,
        }
    }

    fn from_owned(items: Vec<AiProofNode>) -> Self {
        Self { items, index: 0 }
    }

    fn for_node(node: &AiProofNode) -> Self {
        match node {
            AiProofNode::Seq(items) => Self::from_slice(items),
            AiProofNode::Ext2(value) => {
                Self::from_owned(vec![AiProofNode::U64(value[0]), AiProofNode::U64(value[1])])
            }
            AiProofNode::Ext2s(values) => {
                Self::from_owned(values.iter().copied().map(AiProofNode::Ext2).collect())
            }
            AiProofNode::U64s(values) => {
                Self::from_owned(values.iter().copied().map(AiProofNode::U64).collect())
            }
            AiProofNode::I64s(values) => {
                Self::from_owned(values.iter().copied().map(AiProofNode::I64).collect())
            }
            other => Self::from_owned(vec![other.clone()]),
        }
    }

    fn for_struct_node(node: &AiProofNode, fields: usize) -> Self {
        let mut access = Self::for_node(node);
        while access.items.len() < fields {
            access.items.push(AiProofNode::Unit);
        }
        access
    }
}

impl<'de> SeqAccess<'de> for NodeSeqAccess {
    type Error = DeError;

    fn next_element_seed<T>(&mut self, seed: T) -> DeResult<Option<T::Value>>
    where
        T: de::DeserializeSeed<'de>,
    {
        let Some(node) = self.items.get(self.index).cloned() else {
            return Ok(None);
        };
        self.index += 1;
        seed.deserialize(NodeDeserializer { node }).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len().saturating_sub(self.index))
    }
}

#[allow(dead_code)] // used under some feature/target configs only
struct NodeMapAccess {
    items: Vec<(AiProofNode, AiProofNode)>,
    index: usize,
    value_ready: bool,
}

impl NodeMapAccess {
    #[allow(dead_code)] // used under some feature/target configs only
    fn new(items: &[(AiProofNode, AiProofNode)]) -> Self {
        Self {
            items: items.to_vec(),
            index: 0,
            value_ready: false,
        }
    }
}

impl<'de> MapAccess<'de> for NodeMapAccess {
    type Error = DeError;

    fn next_key_seed<K>(&mut self, seed: K) -> DeResult<Option<K::Value>>
    where
        K: de::DeserializeSeed<'de>,
    {
        let Some((key, _)) = self.items.get(self.index).cloned() else {
            return Ok(None);
        };
        self.value_ready = true;
        seed.deserialize(NodeDeserializer { node: key }).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> DeResult<V::Value>
    where
        V: de::DeserializeSeed<'de>,
    {
        if !self.value_ready {
            return Err(DeError("deserialize map value before key".into()));
        }
        let (_, value) = self
            .items
            .get(self.index)
            .cloned()
            .ok_or_else(|| DeError("deserialize map value past end".into()))?;
        self.index += 1;
        self.value_ready = false;
        seed.deserialize(NodeDeserializer { node: value })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len().saturating_sub(self.index))
    }
}

#[allow(dead_code)] // used under some feature/target configs only
struct NodeEnumAccess {
    variant: u32,
    payload: Option<AiProofNode>,
}

impl NodeEnumAccess {
    #[allow(dead_code)] // used under some feature/target configs only
    fn new(node: &AiProofNode) -> DeResult<Self> {
        match node {
            AiProofNode::U64(variant) => Ok(Self {
                variant: u32::try_from(*variant)
                    .map_err(|_| DeError("enum variant index out of range".into()))?,
                payload: None,
            }),
            AiProofNode::Seq(items) => {
                let Some(AiProofNode::U64(variant)) = items.first() else {
                    return Err(DeError("enum sequence missing numeric variant tag".into()));
                };
                let payload = match &items[1..] {
                    [] => None,
                    [single] => Some(single.clone()),
                    rest => Some(AiProofNode::Seq(rest.to_vec())),
                };
                Ok(Self {
                    variant: u32::try_from(*variant)
                        .map_err(|_| DeError("enum variant index out of range".into()))?,
                    payload,
                })
            }
            AiProofNode::U64s(values) => {
                let Some(&variant) = values.first() else {
                    return Err(DeError("enum packed integer sequence is empty".into()));
                };
                let payload = match &values[1..] {
                    [] => None,
                    [single] => Some(AiProofNode::U64(*single)),
                    rest => Some(AiProofNode::U64s(rest.to_vec())),
                };
                Ok(Self {
                    variant: u32::try_from(variant)
                        .map_err(|_| DeError("enum variant index out of range".into()))?,
                    payload,
                })
            }
            other => Err(DeError(format!("expected enum node, got {other:?}"))),
        }
    }
}

impl<'de> EnumAccess<'de> for NodeEnumAccess {
    type Error = DeError;
    type Variant = Self;

    fn variant_seed<V>(self, seed: V) -> DeResult<(V::Value, Self::Variant)>
    where
        V: de::DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, self))
    }
}

impl<'de> VariantAccess<'de> for NodeEnumAccess {
    type Error = DeError;

    fn unit_variant(self) -> DeResult<()> {
        if self.payload.is_some() {
            return Err(DeError("unit enum variant has payload".into()));
        }
        Ok(())
    }

    fn newtype_variant_seed<T>(self, seed: T) -> DeResult<T::Value>
    where
        T: de::DeserializeSeed<'de>,
    {
        let node = self
            .payload
            .ok_or_else(|| DeError("newtype enum variant missing payload".into()))?;
        seed.deserialize(NodeDeserializer { node })
    }

    fn tuple_variant<V>(self, _len: usize, visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        let node = self
            .payload
            .ok_or_else(|| DeError("tuple enum variant missing payload".into()))?;
        de::Deserializer::deserialize_seq(NodeDeserializer { node }, visitor)
    }

    fn struct_variant<V>(self, _fields: &'static [&'static str], visitor: V) -> DeResult<V::Value>
    where
        V: Visitor<'de>,
    {
        let node = self
            .payload
            .ok_or_else(|| DeError("struct enum variant missing payload".into()))?;
        de::Deserializer::deserialize_seq(NodeDeserializer { node }, visitor)
    }
}

#[cfg(test)]
mod tests {
    use ai_pow::fiat_shamir::{
        attempt_tile_index, block_state, canonical_noise_seeds_from_matrix_commitments,
        commitment_key, pow_key_for_nonce,
    };
    use ai_pow::pearl_compat::{
        compute_pearl_pattern_ticket, derive_pearl_work_commitments,
        evaluate_pearl_merge_ticket_attempt, pearl_bitcoin_double_sha256_raw, PearlAttempt,
        PearlAuxInclusionProof, PearlIncompleteBlockHeader, PearlMergePublicStatement,
        PearlMergeTicketAttempt, PearlMiningConfig, PearlNockchainAux, PearlPeriodicPattern,
        PearlPublicProofParams, PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH,
        PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
        PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX, PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG,
        PEARL_NOCKCHAIN_AUX_EXTRA_MAX,
    };
    use ai_pow::prover::params_tag;
    use ai_pow::synth::synth_matrices;
    use ai_pow::zk_bridge::zk_params_from_matmul;

    use super::*;

    #[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct FakeRecursiveCert {
        cap: Vec<[u64; 5]>,
        ext_values: Vec<[u64; 2]>,
        maybe: Option<Vec<u64>>,
    }

    fn sample_params() -> ZkParams {
        ZkParams {
            m: 64,
            k: 64,
            n: 64,
            noise_rank: 4,
            tile: 8,
            difficulty_bits: 1,
        }
    }

    fn sample_pis() -> CompositePublicInputs {
        let mut pis = CompositePublicInputs::zero();
        pis.cumsum = [-1, 2, -3, 4];
        pis.jackpot = core::array::from_fn(|i| i as u32);
        pis.hash_a = [0x1111_1111; 8];
        pis.hash_b = [0x2222_2222; 8];
        pis.job_key = [0x3333_3333; 8];
        pis.commitment_hash = [0x4444_4444; 8];
        pis.hash_jackpot = [0x5555_5555; 8];
        pis
    }

    fn sample_commitments() -> ZkPublicCommitments {
        ZkPublicCommitments {
            h_a_chunk: [0x30; 32],
            h_b_chunk: [0x40; 32],
        }
    }

    fn noun_commitments(commitments: ZkPublicCommitments) -> ZkPublicCommitments {
        ZkPublicCommitments {
            h_a_chunk: commitments.h_a_chunk,
            h_b_chunk: commitments.h_b_chunk,
        }
    }

    #[allow(dead_code)] // used under some feature/target configs only
    fn words_le(b: &[u8; 32]) -> [u32; 8] {
        core::array::from_fn(|i| {
            u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
        })
    }

    #[allow(dead_code)] // used under some feature/target configs only
    fn single_tile_prod_params() -> MatmulParams {
        MatmulParams {
            m: 8,
            k: 512,
            n: 8,
            noise_rank: 32,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    #[allow(dead_code)] // used under some feature/target configs only
    fn production_statement_fixture_for_params(
        params: MatmulParams,
        block_commitment: &[u8],
        nonce: &[u8],
    ) -> (
        MatmulParams,
        ZkPublicCommitments,
        CompositePublicInputs,
        usize,
        u32,
    ) {
        params.validate_prod_envelope().unwrap();
        let commitments = ZkPublicCommitments {
            h_a_chunk: [0x33; 32],
            h_b_chunk: [0x44; 32],
        };
        let tag = params_tag(&params);
        let state = block_state(block_commitment, nonce);
        let kappa = commitment_key(&state, &tag);
        let (s_a, _) = canonical_noise_seeds_from_matrix_commitments(
            &kappa, &commitments.h_a_chunk, &commitments.h_b_chunk,
        );
        let found_idx = attempt_tile_index(&state, &tag, &s_a, params.num_tiles()) as u32;
        let pow_key = pow_key_for_nonce(&s_a, nonce);
        let mut pis = CompositePublicInputs::zero();
        pis.job_key = words_le(&kappa);
        pis.commitment_hash = words_le(&pow_key);
        pis.hash_a = words_le(&commitments.h_a_chunk);
        pis.hash_b = words_le(&commitments.h_b_chunk);
        pis.hash_jackpot = [1, 0, 0, 0, 0, 0, 0, 0];
        let zk_params = zk_params_from_matmul(&params);
        let col_tiles = params.n / params.tile;
        let tile_i = found_idx / col_tiles;
        let tile_j = found_idx % col_tiles;
        let strip_schedule = StripIndexSchedule::from_tile(&zk_params, tile_i, tile_j)
            .expect("fixture tile schedule");
        let trace_height = expected_layer0_rows_for_strip_schedule(&params, &strip_schedule)
            .expect("fixture trace height")
            .required_trace_len();
        (params, commitments, pis, trace_height, found_idx)
    }

    #[allow(dead_code)] // used under some feature/target configs only
    fn production_statement_fixture(
        block_commitment: &[u8],
        nonce: &[u8],
    ) -> (
        MatmulParams,
        ZkPublicCommitments,
        CompositePublicInputs,
        usize,
        u32,
    ) {
        production_statement_fixture_for_params(single_tile_prod_params(), block_commitment, nonce)
    }

    fn build_certificate_slab_with_raw_node<F>(build_certificate: F) -> NounSlab
    where
        F: FnOnce(&mut NounSlab) -> Noun,
    {
        let mut slab: NounSlab = NounSlab::new();
        let params = encode_params(&mut slab, &sample_params());
        let commitments = encode_commitments(&mut slab, &sample_commitments());
        let public_inputs = encode_public_inputs(&mut slab, &sample_pis());
        let certificate = build_certificate(&mut slab);
        let root = T(
            &mut slab,
            &[
                D(AI_POW_CERT_VERSION),
                params,
                D(7),
                D(8_192),
                commitments,
                public_inputs,
                certificate,
            ],
        );
        slab.set_root(root);
        slab
    }

    #[allow(dead_code)] // used under some feature/target configs only
    fn build_ai_pow_artifact_slab(nonce: &[u8], certificate: &NounSlab) -> NounSlab {
        let cert_space = certificate.noun_space();
        let mut slab: NounSlab = NounSlab::new();
        let nonce = build_ai_pow_nonce_noun(&mut slab, nonce);
        let cert = slab.copy_into(unsafe { *certificate.root() }, &cert_space);
        let root = T(&mut slab, &[D(tas!(b"ai-pow")), nonce, cert]);
        slab.set_root(root);
        slab
    }

    fn build_ai_pow_command_slab(artifact: &NounSlab) -> NounSlab {
        let artifact_space = artifact.noun_space();
        let mut slab = NounSlab::new();
        let artifact = slab.copy_into(unsafe { *artifact.root() }, &artifact_space);
        let root = T(&mut slab, &[D(tas!(b"command")), D(tas!(b"pow")), artifact]);
        slab.set_root(root);
        slab
    }

    fn build_pearl_merge_nonce_bytes_for_test(
        statement: &PearlMergePublicStatementShape,
        aux_inclusion: &PearlAuxInclusionProof,
    ) -> Vec<u8> {
        let statement_bytes = statement.to_wire_bytes().expect("statement bytes");
        let mut out = Vec::new();
        out.extend_from_slice(&AI_POW_NONCE_MAGIC);
        out.extend_from_slice(&(statement_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&statement_bytes);
        out.extend_from_slice(&(aux_inclusion.coinbase_tx.len() as u32).to_le_bytes());
        out.extend_from_slice(&aux_inclusion.coinbase_tx);
        out.push(aux_inclusion.merkle_branch.len() as u8);
        for digest in &aux_inclusion.merkle_branch {
            out.extend_from_slice(digest);
        }
        out
    }

    fn build_pearl_merge_artifact_slab(
        statement: &PearlMergePublicStatementShape,
        aux_inclusion: &PearlAuxInclusionProof,
        certificate: &NounSlab,
    ) -> NounSlab {
        let cert_space = certificate.noun_space();
        let mut slab = NounSlab::new();
        let nonce = build_pearl_merge_nonce_bytes_for_test(statement, aux_inclusion);
        let nonce = build_ai_pow_nonce_noun(&mut slab, &nonce);
        let cert = slab.copy_into(unsafe { *certificate.root() }, &cert_space);
        let root = T(&mut slab, &[D(tas!(b"ai-pow")), nonce, cert]);
        slab.set_root(root);
        slab
    }

    fn pearl_test_pattern(length: u32) -> PearlPeriodicPattern {
        PearlPeriodicPattern {
            shape: [(1, length), (length, 1), (length, 1)],
        }
    }

    fn pearl_test_header() -> PearlIncompleteBlockHeader {
        PearlIncompleteBlockHeader {
            version: 0x0102_0304,
            prev_block: [0x11; 32],
            merkle_root: [0x22; 32],
            timestamp: 0x6677_8899,
            nbits: 0x207f_ffff,
        }
    }

    fn pearl_test_config() -> PearlMiningConfig {
        PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        }
    }

    fn pearl_noncontiguous_test_pattern() -> PearlPeriodicPattern {
        PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73])
            .expect("non-contiguous Pearl test pattern")
    }

    fn pearl_test_aux() -> PearlNockchainAux {
        PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet\0".to_vec(),
            nock_block_commitment: [0x42; 32],
            nockchain_target_epoch_or_height: 123_456,
            extra_domain_data: b"ai-pow-target-window\0\0".to_vec(),
        }
    }

    fn pearl_test_coinbase_tx(aux_commitment: &[u8; 32]) -> Vec<u8> {
        let mut script = Vec::from([0x01, 0x00]);
        script.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
        script.extend_from_slice(aux_commitment);
        let mut tx = Vec::new();
        tx.extend_from_slice(&1u32.to_le_bytes());
        tx.push(1);
        tx.extend_from_slice(&[0u8; 32]);
        tx.extend_from_slice(&u32::MAX.to_le_bytes());
        tx.push(script.len() as u8);
        tx.extend_from_slice(&script);
        tx.extend_from_slice(&u32::MAX.to_le_bytes());
        tx.push(1);
        tx.extend_from_slice(&0u64.to_le_bytes());
        tx.push(1);
        tx.push(0x51);
        tx.extend_from_slice(&0u32.to_le_bytes());
        tx
    }

    fn pearl_test_aux_inclusion(
        aux_commitment: &[u8; 32],
    ) -> (PearlIncompleteBlockHeader, PearlAuxInclusionProof) {
        let coinbase_tx = pearl_test_coinbase_tx(aux_commitment);
        let mut merkle_root = pearl_bitcoin_double_sha256_raw(&coinbase_tx);
        merkle_root.reverse();
        let mut header = pearl_test_header();
        header.merkle_root = merkle_root;
        (
            header,
            PearlAuxInclusionProof {
                coinbase_tx,
                merkle_branch: Vec::new(),
            },
        )
    }

    fn pearl_test_params() -> MatmulParams {
        MatmulParams {
            m: 8,
            k: 1024,
            n: 8,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        }
    }

    fn pearl_merge_statement_fixture() -> (
        PearlMergePublicStatementShape,
        PearlAuxInclusionProof,
        ZkPublicCommitments,
        CompositePublicInputs,
        Vec<i8>,
        Vec<i8>,
    ) {
        let params = pearl_test_params();
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-merge-artifact-noun", &params);
        let attempt = PearlAttempt::build_with_config(&header, &config, &a, &b, &params).unwrap();
        let public = PearlPublicProofParams {
            block_header: header,
            mining_config: config,
            hash_a: attempt.commitments.h_a,
            hash_b: attempt.commitments.h_b,
            hash_jackpot: attempt.tile_digests[0].jackpot_hash,
            m: params.m,
            n: params.n,
            t_rows: 0,
            t_cols: 0,
        };
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux.commitment().unwrap(),
            aux,
        };
        let commitments = ZkPublicCommitments {
            h_a_chunk: attempt.commitments.h_a,
            h_b_chunk: attempt.commitments.h_b,
        };
        let ticket = PearlPatternTicket {
            a_rows: (0..8).collect(),
            b_cols: (0..8).collect(),
            tile_state: attempt.tile_digests[0].tile_state,
            jackpot_hash: attempt.tile_digests[0].jackpot_hash,
        };
        let pis = pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &ticket);
        (statement, aux_inclusion, commitments, pis, a, b)
    }

    fn pearl_test_trace_height(params: &MatmulParams) -> usize {
        let config = pearl_test_config();
        let zk_params = zk_params_from_matmul(params);
        let strip_schedule = StripIndexSchedule::from_indices(
            &zk_params,
            config
                .rows_pattern
                .indices_with_offset_bounded(0, 16)
                .expect("default Pearl row schedule"),
            config
                .cols_pattern
                .indices_with_offset_bounded(0, 16)
                .expect("default Pearl column schedule"),
        )
        .expect("default Pearl strip schedule");
        expected_layer0_rows_for_strip_schedule(params, &strip_schedule)
            .expect("default Pearl trace height")
            .required_trace_len()
    }

    fn pearl_merge_ticket_attempt_fixture() -> (
        PearlMergeTicketAttempt,
        PearlAuxInclusionProof,
        Vec<i8>,
        Vec<i8>,
    ) {
        let params = pearl_test_params();
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-artifact-builder", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate Pearl merge ticket attempt");
        (attempt, aux_inclusion, a, b)
    }

    fn unsupported_pearl_merge_geometry_fixture() -> (
        PearlMergePublicStatementShape,
        PearlAuxInclusionProof,
        ZkPublicCommitments,
        CompositePublicInputs,
        Vec<i8>,
        Vec<i8>,
        MatmulParams,
    ) {
        let params = MatmulParams {
            m: 130,
            ..pearl_test_params()
        };
        let aux = PearlNockchainAux {
            nockchain_chain_id: b"nockchain-mainnet".to_vec(),
            nock_block_commitment: [0x42; 32],
            nockchain_target_epoch_or_height: 123_456,
            extra_domain_data: b"ai-pow-target-window".to_vec(),
        };
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-merge-unsupported-geometry", &params);
        let sigma = header.to_bytes();
        let mu = config.to_bytes().unwrap();
        let work_commitments = derive_pearl_work_commitments(&sigma, &mu, &a, &b);
        let mut public = PearlPublicProofParams {
            block_header: header,
            mining_config: config,
            hash_a: work_commitments.h_a,
            hash_b: work_commitments.h_b,
            hash_jackpot: [0u8; 32],
            m: params.m,
            n: params.n,
            t_rows: 0,
            t_cols: 0,
        };
        let ticket = compute_pearl_pattern_ticket(&public, &a, &b, &work_commitments, 16).unwrap();
        public.hash_jackpot = ticket.jackpot_hash;
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux.commitment().unwrap(),
            aux,
        };
        let commitments = ZkPublicCommitments {
            h_a_chunk: work_commitments.h_a,
            h_b_chunk: work_commitments.h_b,
        };
        let pis = pearl_merge_recursive_public_inputs_from_work(&work_commitments, &ticket);
        (statement, aux_inclusion, commitments, pis, a, b, params)
    }

    fn build_certificate_slab_with_statement_and_raw_node<F>(
        zk_params: &ZkParams,
        found_idx: u32,
        trace_height: usize,
        commitments: &ZkPublicCommitments,
        pis: &CompositePublicInputs,
        build_certificate: F,
    ) -> NounSlab
    where
        F: FnOnce(&mut NounSlab) -> Noun,
    {
        let mut slab = NounSlab::new();
        let params = encode_params(&mut slab, zk_params);
        let commitments = encode_commitments(&mut slab, commitments);
        let public_inputs = encode_public_inputs(&mut slab, pis);
        let certificate = build_certificate(&mut slab);
        let root = T(
            &mut slab,
            &[
                D(AI_POW_CERT_VERSION),
                params,
                D(found_idx as u64),
                D(trace_height as u64),
                commitments,
                public_inputs,
                certificate,
            ],
        );
        slab.set_root(root);
        slab
    }

    #[test]
    fn recursive_certificate_serializer_packs_homogeneous_integer_vectors() {
        let cert = FakeRecursiveCert {
            cap: vec![[1, 2, 3, 4, 5], [6, 7, 8, 9, 10]],
            ext_values: vec![[11, 12], [13, 14]],
            maybe: Some(vec![15, 16, 17]),
        };
        let node = recursive_certificate_to_node(&cert).expect("serialize fake cert");
        let AiProofNode::Seq(fields) = node else {
            panic!("fake certificate struct should encode as seq");
        };
        assert!(matches!(fields[0], AiProofNode::Seq(_)));
        assert!(matches!(fields[1], AiProofNode::Ext2s(_)));
        assert!(matches!(fields[2], AiProofNode::Some(_)));
    }

    #[test]
    fn recursive_certificate_node_roundtrips_through_deserializer() {
        let cert = FakeRecursiveCert {
            cap: vec![[1, 2, 3, 4, 5], [6, 7, 8, 9, 10]],
            ext_values: vec![[11, 12], [13, 14]],
            maybe: Some(vec![15, 16, 17]),
        };
        let node = recursive_certificate_to_node(&cert).expect("serialize fake cert");
        let decoded: FakeRecursiveCert =
            recursive_certificate_from_node(&node).expect("deserialize fake cert");
        assert_eq!(decoded, cert);
    }

    #[test]
    fn recursive_certificate_serializer_packs_two_felt_tuples_as_ext2_aura_nodes() {
        #[derive(serde::Serialize)]
        struct TwoFeltCarrier {
            scalar: [u64; 2],
            vector: Vec<[u64; 2]>,
            plain_u64s: Vec<u64>,
        }

        let cert = TwoFeltCarrier {
            scalar: [1, 2],
            vector: vec![[3, 4], [5, 6]],
            plain_u64s: vec![7, 8],
        };
        let node = recursive_certificate_to_node(&cert).expect("serialize two-felt carrier");
        let AiProofNode::Seq(fields) = node else {
            panic!("carrier struct should encode as seq");
        };
        assert_eq!(fields[0], AiProofNode::Ext2([1, 2]));
        assert_eq!(fields[1], AiProofNode::Ext2s(vec![[3, 4], [5, 6]]));
        assert_eq!(fields[2], AiProofNode::U64s(vec![7, 8]));
    }

    #[test]
    fn top_level_certificate_noun_has_hoon_shape_and_structured_certificate_tail() {
        let params = sample_params();
        let commitments = sample_commitments();
        let pis = sample_pis();
        let cert = AiProofNode::Seq(vec![AiProofNode::U64s(vec![1, 2, 3, 4])]);
        let slab =
            build_ai_pow_certificate_noun_from_node(&params, 7, 8_192, &commitments, &pis, &cert);
        let jammed = slab.jam();
        assert!(
            jammed.len() > 64,
            "certificate noun must persist structure, not a tiny placeholder"
        );
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let cell = root.in_space(&space).as_cell().expect("certificate cell");
        assert_eq!(
            cell.head().as_atom().unwrap().as_u64().unwrap(),
            AI_POW_CERT_VERSION
        );
        let fields = tuple7(root, &space, "ai-pow-certificate").expect("certificate tuple");
        let commitment_fields =
            tuple2(fields[4], &space, "ai-pow-commitments").expect("commitment pair");
        assert_eq!(
            expect_fixed_bytes::<32>(
                commitment_fields[0],
                &space,
                "h-a-chunk",
                CertificateNounLimits::default()
            )
            .expect("h-a-chunk atom"),
            commitments.h_a_chunk
        );
        assert_eq!(
            expect_fixed_bytes::<32>(
                commitment_fields[1],
                &space,
                "h-b-chunk",
                CertificateNounLimits::default()
            )
            .expect("h-b-chunk atom"),
            commitments.h_b_chunk
        );
    }

    #[test]
    fn certificate_noun_roundtrips_through_jam_cue_and_bounded_decoder() {
        let params = sample_params();
        let commitments = sample_commitments();
        let pis = sample_pis();
        let cert = AiProofNode::Seq(vec![
            AiProofNode::Bytes(vec![1, 2, 0, 0]),
            AiProofNode::Ext2([7, 8]),
            AiProofNode::Ext2s(vec![[9, 10], [11, 12]]),
            AiProofNode::U64s(vec![3, 4]),
            AiProofNode::I64s(vec![-5, 6]),
        ]);
        let slab =
            build_ai_pow_certificate_noun_from_node(&params, 9, 16_384, &commitments, &pis, &cert);

        let jammed = slab.jam();
        let mut cued: NounSlab = NounSlab::new();
        let root = cued.cue_into(jammed).expect("cue certificate noun");
        cued.set_root(root);

        let decoded = decode_ai_pow_certificate_slab(&cued, CertificateNounLimits::default())
            .expect("decode certificate noun");
        assert_eq!(decoded.version, AI_POW_CERT_VERSION);
        assert_eq!(decoded.zk_params, params);
        assert_eq!(decoded.found_idx, 9);
        assert_eq!(decoded.trace_height, 16_384);
        assert_eq!(decoded.commitments, noun_commitments(commitments));
        assert_eq!(decoded.public_inputs, pis);
        assert_eq!(decoded.certificate, cert);
    }

    /// **Audit N9 — cumulative-atom-bytes DoS guard.** Each `%bytes` node here is
    /// well under `max_atom_bytes` (1 MiB), but their SUM exceeds a tight
    /// cumulative budget. Without the `charge_atom_bytes` guard, a many-node jam
    /// could drive `max_total_nodes × max_atom_bytes` ≈ 1 TiB of allocation; the
    /// guard bounds the total.
    #[test]
    fn decode_rejects_cumulative_atom_bytes_over_budget() {
        let params = sample_params();
        let commitments = sample_commitments();
        let pis = sample_pis();
        // 3 × 2 KiB atoms = 6 KiB; each node is tiny, the sum is the attack.
        let cert = AiProofNode::Seq(vec![
            AiProofNode::Bytes(vec![7u8; 2048]),
            AiProofNode::Bytes(vec![8u8; 2048]),
            AiProofNode::Bytes(vec![9u8; 2048]),
        ]);
        let slab =
            build_ai_pow_certificate_noun_from_node(&params, 9, 16_384, &commitments, &pis, &cert);
        let jammed = slab.jam();
        let mut cued: NounSlab = NounSlab::new();
        let root = cued.cue_into(jammed).expect("cue");
        cued.set_root(root);

        // The 6 KiB tree decodes fine under the default 64 MiB cumulative budget.
        decode_ai_pow_certificate_slab(&cued, CertificateNounLimits::default())
            .expect("6 KiB tree under default budget");

        // A 5 KiB cumulative budget rejects on the SUM (no single node exceeds it).
        let tight = CertificateNounLimits {
            max_total_atom_bytes: 5 * 1024,
            ..Default::default()
        };
        let err = decode_ai_pow_certificate_slab(&cued, tight).unwrap_err();
        assert!(
            matches!(
                err,
                CertificateNounError::LimitExceeded("cumulative atom bytes")
            ),
            "expected cumulative-atom-bytes rejection, got {err:?}"
        );
    }

    #[test]
    fn pearl_merge_artifact_decoder_keeps_statement_structured_and_prechecks_public_inputs() {
        let (statement, aux_inclusion, commitments, pis, a, b) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);

        let decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert!(decoded.statement.aux.nockchain_chain_id.ends_with(&[0]));
        assert!(decoded.statement.aux.extra_domain_data.ends_with(&[0, 0]));
        assert_eq!(decoded.certificate.commitments, commitments);
        assert_eq!(decoded.certificate.public_inputs, pis);

        let precheck = precheck_ai_pow_pearl_merge_artifact_statement(
            &decoded, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32], 16,
        )
        .expect("Pearl merge statement should precheck");
        assert_eq!(
            pearl_merge_recursive_public_inputs_from_precheck(&precheck),
            pis
        );
        assert_eq!(precheck.aux, statement.aux);
        assert_eq!(precheck.work.commitments.h_a, commitments.h_a_chunk);
        assert_eq!(precheck.work.commitments.h_b, commitments.h_b_chunk);

        let wire_bytes = statement.to_wire_bytes().expect("wire statement bytes");
        assert_eq!(
            PearlMergePublicStatement::from_bytes(&wire_bytes)
                .expect("wire statement parses")
                .expected_aux_commitment,
            statement.expected_aux_commitment
        );
    }

    #[test]
    fn pearl_merge_artifact_metadata_decoder_does_not_walk_proof_node_tail() {
        let (statement, aux_inclusion, commitments, pis, a, b) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            |_| D(0),
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);

        let metadata = decode_ai_pow_pearl_merge_artifact_metadata_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("metadata-only decode should not traverse the bad proof node");
        assert_eq!(metadata.statement, statement);
        assert_eq!(metadata.aux_inclusion, aux_inclusion);
        assert_eq!(metadata.certificate.public_inputs, pis);

        precheck_ai_pow_pearl_merge_artifact_metadata(
            &metadata, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32], 16,
        )
        .expect("metadata precheck should not traverse the bad proof node");

        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_metadata(
                &metadata, &statement.aux.nock_block_commitment, &a, &b, &[0u8; 32], 16,
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainTargetNotMet
            ))
        ));

        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_slab(
                &artifact_slab,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::Shape(
                "proof node must be a tagged cell"
            ))
        ));
    }

    #[test]
    fn pearl_merge_public_statement_builder_round_trips_trailing_zero_aux_fields() {
        let (statement, _, _, _, _, _) = pearl_merge_statement_fixture();
        let slab = build_pearl_merge_public_statement_slab(&statement);
        let space = slab.noun_space();
        let decoded = decode_pearl_merge_public_statement_noun(
            unsafe { *slab.root() },
            &space,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge public statement");

        assert_eq!(decoded, statement);
        assert!(decoded.aux.nockchain_chain_id.ends_with(&[0]));
        assert!(decoded.aux.extra_domain_data.ends_with(&[0, 0]));
        assert_eq!(
            decoded.to_wire_bytes().expect("decoded wire bytes"),
            statement.to_wire_bytes().expect("statement wire bytes")
        );
    }

    #[test]
    fn pearl_merge_public_statement_shape_converts_from_wire_statement() {
        let (statement, _, _, _, _, _) = pearl_merge_statement_fixture();
        let wire = statement.to_wire_statement().expect("wire statement");
        let from_wire = PearlMergePublicStatementShape::from_wire_statement(&wire)
            .expect("shape from wire statement");
        let from_bytes =
            PearlMergePublicStatementShape::from_wire_bytes(&wire.to_bytes().expect("wire bytes"))
                .expect("shape from wire bytes");

        assert_eq!(from_wire, statement);
        assert_eq!(from_bytes, statement);
    }

    #[test]
    fn pearl_merge_certificate_noun_accepts_full_rb_stripe_band() {
        let params = MatmulParams {
            m: 8,
            k: 65_536,
            n: 8,
            noise_rank: 128,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        params
            .validate_prod_envelope()
            .expect("full 512-stripe Pearl shape is in the production envelope");

        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = PearlMiningConfig {
            common_dim: params.k,
            rank: params.noise_rank as u16,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        };
        let (a, b) = synth_matrices(b"pearl-full-rb-stripe-band-noun", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate full R-b stripe-band ticket");
        let public_inputs =
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket);

        build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node(
            &attempt,
            &aux_inclusion,
            &a,
            &b,
            16,
            &public_inputs,
            &AiProofNode::Unit,
        )
        .expect("certificate noun path must admit the full R-b stripe band");
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_derives_recursive_metadata() {
        let (attempt, aux_inclusion, a, b) = pearl_merge_ticket_attempt_fixture();
        let params = pearl_test_params();
        let parts = pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16)
            .expect("derive recursive certificate parts from ticket");
        let expected_statement =
            PearlMergePublicStatementShape::from_wire_statement(&attempt.statement)
                .expect("statement shape from ticket wire");
        let expected_pis =
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket);

        assert_eq!(parts.statement, expected_statement);
        assert_eq!(parts.zk_params, zk_params_from_matmul(&params));
        assert_eq!(parts.found_idx, 0);
        assert_eq!(
            parts.trace_height,
            expected_layer0_rows_for_strip_schedule(&params, &parts.strip_schedule)
                .expect("schedule-aware row budget")
                .required_trace_len()
        );
        assert_eq!(
            parts.commitments,
            ZkPublicCommitments {
                h_a_chunk: attempt.commitments.h_a,
                h_b_chunk: attempt.commitments.h_b,
            }
        );
        assert_eq!(parts.public_inputs, expected_pis);
        assert_eq!(
            parts.strip_schedule,
            ai_pow_zk::canonical::StripIndexSchedule::from_tile(&parts.zk_params, 0, 0)
                .expect("expected tile schedule")
        );

        let artifact_slab = build_ai_pow_pearl_merge_artifact_noun_from_ticket_node(
            &attempt,
            &aux_inclusion,
            &a,
            &b,
            16,
            &AiProofNode::Unit,
        )
        .expect("build ai-pow artifact from ticket");
        let decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode ai-pow artifact from ticket");
        assert_eq!(decoded.statement, parts.statement);
        assert_eq!(decoded.certificate.zk_params, parts.zk_params);
        assert_eq!(decoded.certificate.found_idx, parts.found_idx);
        assert_eq!(decoded.certificate.trace_height, parts.trace_height);
        assert_eq!(decoded.certificate.commitments, parts.commitments);
        assert_eq!(decoded.certificate.public_inputs, parts.public_inputs);

        let precheck = precheck_ai_pow_pearl_merge_artifact_statement(
            &decoded, &attempt.aux.nock_block_commitment, &a, &b, &attempt.nockchain_target, 16,
        )
        .expect("precheck ai-pow artifact from ticket");
        assert_eq!(
            pearl_merge_recursive_public_inputs_from_precheck(&precheck),
            parts.public_inputs
        );
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_accepts_actual_recursive_public_inputs() {
        let (attempt, aux_inclusion, a, b) = pearl_merge_ticket_attempt_fixture();
        let mut proof_pis =
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket);
        proof_pis.cumsum = [17, -23, 42, -99];

        let parts = pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
            &attempt, &a, &b, 16, &proof_pis,
        )
        .expect("derive recursive certificate parts with actual proof public inputs");
        assert_eq!(parts.public_inputs, proof_pis);

        let artifact_slab = build_ai_pow_pearl_merge_artifact_noun_from_ticket_public_inputs_node(
            &attempt,
            &aux_inclusion,
            &a,
            &b,
            16,
            &proof_pis,
            &AiProofNode::Unit,
        )
        .expect("build ai-pow artifact from ticket and proof public inputs");
        let decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode ai-pow artifact from ticket and proof public inputs");
        assert_eq!(decoded.certificate.public_inputs.cumsum, proof_pis.cumsum);

        precheck_ai_pow_pearl_merge_artifact_statement(
            &decoded, &attempt.aux.nock_block_commitment, &a, &b, &attempt.nockchain_target, 16,
        )
        .expect("precheck accepts actual proof cumsum when Pearl-bound slots match");

        let cases: [(&str, fn(&mut CompositePublicInputs)); 6] = [
            ("public-inputs.hash-a", |pis| pis.hash_a[0] ^= 1),
            ("public-inputs.hash-b", |pis| pis.hash_b[0] ^= 1),
            ("public-inputs.job-key", |pis| pis.job_key[0] ^= 1),
            ("public-inputs.commitment-hash", |pis| {
                pis.commitment_hash[0] ^= 1
            }),
            ("public-inputs.jackpot", |pis| pis.jackpot[0] ^= 1),
            ("public-inputs.hash-jackpot", |pis| pis.hash_jackpot[0] ^= 1),
        ];
        for (field, tamper) in cases {
            let mut bad_pis = proof_pis.clone();
            tamper(&mut bad_pis);
            assert!(matches!(
                pearl_merge_recursive_certificate_parts_from_ticket_public_inputs(
                    &attempt, &a, &b, 16, &bad_pis,
                ),
                Err(CertificateNounError::PearlMergePublicInputMismatch(got)) if got == field
            ));
        }
    }

    /// Heavy opt-in integration: proves the selected compact recursive
    /// certificate, packages it as the Pearl-compatible `%ai-pow` artifact,
    /// jams/cues it, and confirms the compact proof remains canonical postcard
    /// bytes at the noun boundary.
    ///
    /// Run with:
    ///
    /// ```text
    /// RUSTFLAGS="-C target-cpu=native" cargo test -p ai-pow-miner --release --features node \
    ///   real_compact_pearl_merge_artifact_jam_size_for_selected_route -- --ignored --nocapture
    /// ```
    /// **D1a — production-scale-`m` compact size/latency.** The fixture above uses
    /// `m=n=8` (tile==matrix ⇒ opens the whole matrix, no real strip-opening
    /// merkle proof). The production default tile is `tile=8, k=1024, rank=64`
    /// (miner default), but over a large model matrix. This proves the compact
    /// certificate for the default tile over a large `m=n=512` matrix — a real
    /// disjoint strip opening (8 of 512 chunks + auth siblings up a depth-9 tree)
    /// — to confirm the ≤150 KB / ~30 s bar holds with a real merkle proof, not
    /// just the degenerate whole-matrix fixture. Run fully parallel + native:
    ///
    /// ```text
    /// RUSTFLAGS="-C target-cpu=native" cargo test -p ai-pow-miner --release --features node \
    ///   real_compact_pearl_merge_prod_scale_m_size_and_latency -- --ignored --nocapture --test-threads=1
    /// ```
    #[ignore = "real compact recursive proof generation is intentionally opt-in"]
    #[test]
    fn real_compact_pearl_merge_prod_scale_m_size_and_latency() {
        let params = crate::DENSE_PRODUCTION_PARAMS;
        let aux = pearl_test_aux();
        let (header, _aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config(); // common_dim=1024, rank=64, pattern(8)
        let (a, b) = synth_matrices(b"pearl-prod-scale-m-512", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate prod-scale Pearl merge ticket attempt");

        eprintln!(
            "D1a prod-scale-m: available_parallelism = {}",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(0)
        );
        let start = std::time::Instant::now();
        let run = ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate(
            &attempt, &params, &a, &b, 16,
        )
        .expect("prove compact Pearl recursive certificate (prod-scale m)");
        let prove_wall_ms = start.elapsed().as_millis();
        let compact_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(run.certificate())
                .expect("encode compact recursive certificate");
        eprintln!(
            "D1a prod-scale-m (m=n=512, tile=8, k=1024, r=64): compact_cert={} bytes ({:.2} KiB), \
             prove_wall_ms={}, l1_build_ms={}, l1_outer_ms={}, l2_prep_ms={}, l2_prove_ms={}, \
             l2_compact_ms={}, l2_compact_verify_ms={}, trace_height={}",
            compact_bytes.len(),
            compact_bytes.len() as f64 / 1024.0,
            prove_wall_ms,
            run.l1_circuit_build_ms(),
            run.l1_outer_cert_ms(),
            run.l2_prep_ms(),
            run.l2_prove_ms(),
            run.l2_compact_ms(),
            run.l2_compact_verify_ms(),
            run.trace_height(),
        );
        assert!(
            compact_bytes.len() <= 150_000,
            "prod-scale compact cert exceeded 150,000 bytes: {}",
            compact_bytes.len()
        );
    }

    #[ignore = "real compact recursive proof generation is intentionally opt-in"]
    #[test]
    fn real_compact_pearl_merge_prod_scale_m_warm_cache_size_and_latency() {
        let params = crate::DENSE_PRODUCTION_PARAMS;
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-prod-scale-m-512", &params);

        let warmup_aux = pearl_test_aux();
        let (warmup_header, _warmup_aux_inclusion) =
            pearl_test_aux_inclusion(&warmup_aux.commitment().unwrap());
        let warmup_attempt = evaluate_pearl_merge_ticket_attempt(
            &warmup_header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, warmup_aux,
        )
        .expect("evaluate warmup prod-scale Pearl merge ticket attempt");
        let warmup_run = ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate(
            &warmup_attempt, &params, &a, &b, 16,
        )
        .expect("prove warmup compact Pearl recursive certificate (prod-scale m)");
        let cache = warmup_run
            .into_prover_cache()
            .expect("warmup run must return reusable compact prover cache");

        let mut timed_aux = pearl_test_aux();
        timed_aux.nock_block_commitment = [0x43; 32];
        let (timed_header, _timed_aux_inclusion) =
            pearl_test_aux_inclusion(&timed_aux.commitment().unwrap());
        let timed_attempt = evaluate_pearl_merge_ticket_attempt(
            &timed_header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, timed_aux,
        )
        .expect("evaluate timed prod-scale Pearl merge ticket attempt");

        let start = std::time::Instant::now();
        let run =
            ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate_with_prover_cache(
                &timed_attempt, &params, &a, &b, 16, &cache,
            )
            .expect("prove cached compact Pearl recursive certificate (prod-scale m)");
        let prove_wall_ms = start.elapsed().as_millis();
        let compact_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(run.certificate())
                .expect("encode compact recursive certificate");
        eprintln!(
            "D1a prod-scale-m warm-cache (m=n=512, tile=8, k=1024, r=64): compact_cert={} bytes ({:.2} KiB), \
             prove_wall_ms={}, l1_build_ms={}, l1_outer_ms={}, l2_prep_ms={}, l2_prove_ms={}, \
             l2_compact_ms={}, l2_compact_verify_ms={}, trace_height={}, cache_hit=true",
            compact_bytes.len(),
            compact_bytes.len() as f64 / 1024.0,
            prove_wall_ms,
            run.l1_circuit_build_ms(),
            run.l1_outer_cert_ms(),
            run.l2_prep_ms(),
            run.l2_prove_ms(),
            run.l2_compact_ms(),
            run.l2_compact_verify_ms(),
            run.trace_height(),
        );
        assert!(
            compact_bytes.len() <= 150_000,
            "prod-scale compact cert exceeded 150,000 bytes: {}",
            compact_bytes.len()
        );
    }

    /// **D1a (worst case) — max prod-envelope tile compact size/latency.**
    /// `tile=16` (h·w=256 = `PEARL_HW_MAX`), `k=4096`, `r=64`
    /// (num_stripes=64=`STRIPE_MAX`) — the largest in-circuit tile the prod
    /// envelope admits. This measures how far the ~30 s wall-time bar stretches at
    /// the top of the envelope (the compact byte size stays fixed). Run fully
    /// parallel + native (see the default test above).
    #[ignore = "real compact recursive proof generation is intentionally opt-in"]
    #[test]
    fn real_compact_pearl_merge_max_envelope_size_and_latency() {
        let params = MatmulParams {
            m: 512,
            k: 4096,
            n: 512,
            noise_rank: 64,
            tile: 16,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        params
            .validate_prod_envelope()
            .expect("tile=16,k=4096,r=64 must be in the prod envelope");
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = PearlMiningConfig {
            common_dim: 4096,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(16),
            cols_pattern: pearl_test_pattern(16),
            reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        };
        let (a, b) = synth_matrices(b"pearl-max-envelope-t16-k4096", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 0, 0, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate max-envelope Pearl merge ticket attempt");

        let start = std::time::Instant::now();
        let run = ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate(
            &attempt, &params, &a, &b, 16,
        )
        .expect("prove compact Pearl recursive certificate (max envelope)");
        let prove_wall_ms = start.elapsed().as_millis();
        let compact_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(run.certificate())
                .expect("encode compact recursive certificate");
        eprintln!(
            "D1a max-envelope (m=n=512, tile=16, k=4096, r=64): compact_cert={} bytes ({:.2} KiB), \
             prove_wall_ms={}, l1_outer_ms={}, l2_prove_ms={}, trace_height={}",
            compact_bytes.len(),
            compact_bytes.len() as f64 / 1024.0,
            prove_wall_ms,
            run.l1_outer_cert_ms(),
            run.l2_prove_ms(),
            run.trace_height(),
        );
        assert!(
            compact_bytes.len() <= 150_000,
            "max-envelope compact cert exceeded 150,000 bytes: {}",
            compact_bytes.len()
        );

        // FULL NODE ROUND-TRIP at 2^16 — the degree-adaptive lb=2 path. The
        // artifact must decode + node-verify, confirming the verifier re-derives
        // the SAME `for_layer0_trace(trace_height)` profile (lb=2/nq=30 at 2^16)
        // the prover used across L0/L1/L2. A profile mismatch would fail here.
        let artifact_slab =
            build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
                &attempt, &aux_inclusion, &a, &b, 16, &run,
            )
            .expect("build max-envelope compact artifact");
        let jammed = artifact_slab.jam();
        let decoded =
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, CertificateNounLimits::default())
                .expect("decode max-envelope compact artifact");
        assert_eq!(decoded.certificate.trace_height, run.trace_height());
        let verifier_context = PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment: &attempt.aux.nock_block_commitment,
            a_row_major: &a,
            b_col_major: &b,
            nockchain_target: &attempt.nockchain_target,
            max_pattern_len: 16,
        };
        let expected_digest_bytes =
            ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
                run.verifier_key_digest(),
            );
        let verified =
            verify_decoded_ai_pow_pearl_merge_compact_artifact_with_digest_bytes_and_limits(
                &decoded,
                verifier_context,
                run.verifier_context(),
                &expected_digest_bytes,
                CertificateNounLimits::default(),
            )
            .expect("max-envelope compact artifact node-verifies (degree-adaptive lb=2)");
        assert_eq!(verified.work.ticket, attempt.ticket);
        assert_eq!(verified.work.commitments, attempt.commitments);
    }

    #[ignore = "real compact recursive proof generation is intentionally opt-in"]
    #[test]
    fn real_compact_pearl_merge_artifact_jam_size_for_selected_route() {
        let params = pearl_test_params();
        let (attempt, aux_inclusion, a, b) = pearl_merge_ticket_attempt_fixture();

        let start = std::time::Instant::now();
        eprintln!("real compact Pearl artifact: proving compact recursive certificate");
        let run = ai_pow::zk_bridge::prove_pearl_merge_compact_recursive_certificate(
            &attempt, &params, &a, &b, 16,
        )
        .expect("prove compact Pearl recursive certificate");
        let prove_wall_ms = start.elapsed().as_millis();
        let compact_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(run.certificate())
                .expect("encode compact recursive certificate");

        eprintln!("real compact Pearl artifact: building compact noun artifact");
        let artifact_slab =
            build_ai_pow_pearl_merge_artifact_noun_from_ticket_compact_recursive_run(
                &attempt, &aux_inclusion, &a, &b, 16, &run,
            )
            .expect("build compact artifact from real recursive run");
        let jammed = artifact_slab.jam();

        eprintln!("real compact Pearl artifact: bounded decoding jammed noun");
        let decoded =
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, CertificateNounLimits::default())
                .expect("decode compact artifact jam");
        let precheck = precheck_ai_pow_pearl_merge_artifact_statement(
            &decoded, &attempt.aux.nock_block_commitment, &a, &b, &attempt.nockchain_target, 16,
        )
        .expect("compact artifact statement precheck");
        assert_eq!(precheck.work.ticket, attempt.ticket);
        assert_eq!(precheck.work.commitments, attempt.commitments);
        assert_eq!(decoded.certificate.version, AI_POW_CERT_VERSION);
        assert_eq!(decoded.certificate.zk_params, run.zk_params());
        assert_eq!(decoded.certificate.found_idx, run.found_idx());
        assert_eq!(decoded.certificate.trace_height, run.trace_height());
        assert_eq!(decoded.certificate.commitments, run.commitments());
        assert_eq!(decoded.certificate.public_inputs, *run.public_inputs());

        let AiProofNode::Bytes(decoded_compact_bytes) = &decoded.certificate.certificate else {
            panic!("compact recursive certificate must stay encoded as a byte node");
        };
        assert_eq!(decoded_compact_bytes, &compact_bytes);
        let decoded_compact =
            ai_pow_compact_recursive_certificate_from_node(&decoded.certificate.certificate)
                .expect("compact proof-node reconstructs typed certificate");
        let recoded =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&decoded_compact)
                .expect("re-encode compact recursive certificate");
        assert_eq!(recoded, compact_bytes);

        let verifier_context = PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment: &attempt.aux.nock_block_commitment,
            a_row_major: &a,
            b_col_major: &b,
            nockchain_target: &attempt.nockchain_target,
            max_pattern_len: 16,
        };
        let expected_digest_bytes =
            ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
                run.verifier_key_digest(),
            );
        let compact_verified =
            verify_decoded_ai_pow_pearl_merge_compact_artifact_with_digest_bytes_and_limits(
                &decoded,
                verifier_context,
                run.verifier_context(),
                &expected_digest_bytes,
                CertificateNounLimits::default(),
            )
            .expect("decoded compact artifact verifies with pinned digest bytes");
        assert_eq!(compact_verified.work.ticket, attempt.ticket);
        assert_eq!(compact_verified.work.commitments, attempt.commitments);

        let compact_jam_verified =
            verify_ai_pow_pearl_merge_compact_artifact_jam_with_digest_bytes_and_context(
                &jammed,
                CertificateNounLimits::default(),
                verifier_context,
                run.verifier_context(),
                &expected_digest_bytes,
            )
            .expect("jammed compact artifact verifies with pinned digest bytes");
        assert_eq!(compact_jam_verified.work.ticket, attempt.ticket);
        assert_eq!(compact_jam_verified.work.commitments, attempt.commitments);

        let wrong_digest =
            [0u8; ai_pow_zk::recursion::AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES];
        assert_ne!(
            &wrong_digest[..],
            &expected_digest_bytes[..],
            "test fixture verifier-key digest should not be the all-zero digest"
        );
        let err = verify_decoded_ai_pow_pearl_merge_compact_artifact_with_digest_bytes_and_limits(
            &decoded,
            verifier_context,
            run.verifier_context(),
            &wrong_digest,
            CertificateNounLimits::default(),
        )
        .expect_err("wrong pinned verifier-key digest bytes must reject");
        assert!(matches!(
            err,
            CertificateNounError::CompactVerifierKeyDigestMismatch("verifier-context")
        ));

        let mut noncanonical_digest = expected_digest_bytes;
        noncanonical_digest[0..8].copy_from_slice(&GOLDILOCKS_MODULUS.to_le_bytes());
        let err = verify_decoded_ai_pow_pearl_merge_compact_artifact_with_digest_bytes_and_limits(
            &decoded,
            verifier_context,
            run.verifier_context(),
            &noncanonical_digest,
            CertificateNounLimits::default(),
        )
        .expect_err("noncanonical verifier-key digest bytes must reject");
        assert!(matches!(
            err,
            CertificateNounError::CompactVerifierKeyDigestEncoding(_)
        ));

        assert!(
            compact_bytes.len() <= 150_000,
            "compact recursive certificate exceeded relaxed 150,000 byte gate: {} bytes",
            compact_bytes.len()
        );
        assert!(
            jammed.len() <= 150_000,
            "jammed compact `%ai-pow` artifact exceeded relaxed 150,000 byte gate: {} bytes",
            jammed.len()
        );

        eprintln!(
            "real compact Pearl artifact: jammed={} bytes ({:.2} KiB), compact_cert={} bytes ({:.2} KiB), verifier_digest_hex={}, prove_wall_ms={}, l1_build_ms={}, l1_outer_ms={}, l2_prep_ms={}, l2_prove_ms={}, l2_compact_ms={}, l2_compact_verify_ms={}",
            jammed.len(),
            jammed.len() as f64 / 1024.0,
            compact_bytes.len(),
            compact_bytes.len() as f64 / 1024.0,
            hex::encode(expected_digest_bytes),
            prove_wall_ms,
            run.l1_circuit_build_ms(),
            run.l1_outer_cert_ms(),
            run.l2_prep_ms(),
            run.l2_prove_ms(),
            run.l2_compact_ms(),
            run.l2_compact_verify_ms(),
        );
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_rejects_tampered_aux_inclusion() {
        let (attempt, mut aux_inclusion, a, b) = pearl_merge_ticket_attempt_fixture();
        aux_inclusion.merkle_branch.push([0x66; 32]);

        assert!(matches!(
            build_ai_pow_pearl_merge_artifact_noun_from_ticket_node(
                &attempt,
                &aux_inclusion,
                &a,
                &b,
                16,
                &AiProofNode::Unit,
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::PearlAuxMerkleBranchTooDeep(1)
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_rejects_non_winning_ticket() {
        let (mut attempt, _, a, b) = pearl_merge_ticket_attempt_fixture();
        attempt.nockchain_target = [0u8; 32];

        assert!(matches!(
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainTargetNotMet
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_rejects_statement_drift() {
        let (mut attempt, _, a, b) = pearl_merge_ticket_attempt_fixture();
        attempt.statement.public_data[52] ^= 1;

        assert!(matches!(
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "ticket.statement.public-data"
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_recomputes_public_work() {
        let (mut attempt, _, a, b) = pearl_merge_ticket_attempt_fixture();
        attempt.public_params.hash_jackpot = [0u8; 32];
        attempt.ticket.jackpot_hash = [0u8; 32];
        attempt.statement.public_data = attempt.public_params.to_public_data().unwrap();

        assert!(matches!(
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::JackpotHashMismatch
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_rejects_forged_ticket_rows() {
        let (mut attempt, _, a, b) = pearl_merge_ticket_attempt_fixture();
        attempt.ticket.a_rows[0] = attempt.ticket.a_rows[0].saturating_add(1);

        assert!(matches!(
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "ticket.work"
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_rejects_wrong_matrices() {
        let (attempt, _, mut a, b) = pearl_merge_ticket_attempt_fixture();
        a[0] ^= 1;

        assert!(matches!(
            pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::PublicCommitmentMismatch
            ))
        ));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_accepts_multi_tile_ticket_metadata() {
        let params = MatmulParams {
            m: 16,
            n: 16,
            ..pearl_test_params()
        };
        let aux = pearl_test_aux();
        let (header, _) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-ticket-multi-tile-artifact", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 8, 8, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate multi-tile Pearl merge ticket attempt");
        assert!(params.num_tiles() > 1);

        let parts = pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16)
            .expect("derive multi-tile Pearl recursive metadata");
        assert_eq!(parts.found_idx, 3);
        assert_eq!(parts.zk_params, zk_params_from_matmul(&params));
        assert_eq!(parts.commitments.h_a_chunk, attempt.commitments.h_a);
        assert_eq!(parts.commitments.h_b_chunk, attempt.commitments.h_b);
        assert_eq!(
            parts.public_inputs,
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket)
        );
        assert_eq!(
            parts.strip_schedule,
            ai_pow_zk::canonical::StripIndexSchedule::from_tile(&parts.zk_params, 1, 1)
                .expect("expected multi-tile schedule")
        );
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_derives_noncontiguous_schedule() {
        let params = MatmulParams {
            m: 128,
            n: 128,
            ..pearl_test_params()
        };
        let header = pearl_test_header();
        let config = PearlMiningConfig {
            rows_pattern: pearl_noncontiguous_test_pattern(),
            cols_pattern: pearl_noncontiguous_test_pattern(),
            ..pearl_test_config()
        };
        let (a, b) = synth_matrices(b"pearl-ticket-noncontiguous-artifact", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xff; 32],
            16,
            pearl_test_aux(),
        )
        .expect("evaluate non-contiguous Pearl merge ticket attempt");

        let parts = pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16)
            .expect("derive non-contiguous Pearl recursive metadata");
        assert_eq!(parts.strip_schedule.a_indices, attempt.ticket.a_rows);
        assert_eq!(parts.strip_schedule.b_indices, attempt.ticket.b_cols);
        assert_ne!(parts.strip_schedule.a_indices, contiguous_indices(0, 8));
    }

    #[test]
    fn pearl_merge_ticket_artifact_builder_derives_rectangular_non_native_schedule() {
        let params = MatmulParams {
            m: 128,
            n: 125,
            tile: 6,
            ..pearl_test_params()
        };
        assert!(
            params.validate().is_err(),
            "native square tile grid rejects this Pearl-valid schedule"
        );
        let header = pearl_test_header();
        let config = PearlMiningConfig {
            rows_pattern: PearlPeriodicPattern::from_list(&[0, 1, 2, 3, 4, 5]).unwrap(),
            cols_pattern: PearlPeriodicPattern::from_list(&[0, 1, 2, 3, 4, 5, 6, 7]).unwrap(),
            ..pearl_test_config()
        };
        let (a, b) = synth_matrices(b"pearl-ticket-rectangular-native-grid-artifact", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header,
            &config,
            &params,
            0,
            0,
            &a,
            &b,
            &[0xff; 32],
            16,
            pearl_test_aux(),
        )
        .expect("evaluate rectangular Pearl merge ticket attempt");

        let parts = pearl_merge_recursive_certificate_parts_from_ticket(&attempt, &a, &b, 16)
            .expect("derive rectangular Pearl recursive metadata");
        assert_eq!(parts.found_idx, 0);
        assert_eq!(parts.strip_schedule.a_indices, attempt.ticket.a_rows);
        assert_eq!(parts.strip_schedule.b_indices, attempt.ticket.b_cols);
        assert_eq!(parts.zk_params.tile, 6);
    }

    #[test]
    fn pearl_merge_public_statement_decoder_rejects_short_declared_aux_length() {
        let (mut statement, _, _, _, _, _) = pearl_merge_statement_fixture();
        statement.aux.nockchain_chain_id = b"nockchain".to_vec();

        let mut slab: NounSlab = NounSlab::new();
        let block_header = bytes_to_atom(&mut slab, &statement.block_header);
        let public_data = bytes_to_atom(&mut slab, &statement.public_data);
        let expected_aux_commitment = bytes_to_atom(&mut slab, &statement.expected_aux_commitment);
        let chain_id_data = bytes_to_atom(&mut slab, &statement.aux.nockchain_chain_id);
        let chain_id = T(
            &mut slab,
            &[D((statement.aux.nockchain_chain_id.len() - 1) as u64), chain_id_data],
        );
        let nock_block = bytes_to_atom(&mut slab, &statement.aux.nock_block_commitment);
        let extra_data = bytes_to_atom(&mut slab, &statement.aux.extra_domain_data);
        let extra = T(
            &mut slab,
            &[D(statement.aux.extra_domain_data.len() as u64), extra_data],
        );
        let aux = T(
            &mut slab,
            &[chain_id, nock_block, D(statement.aux.nockchain_target_epoch_or_height), extra],
        );
        let root = T(
            &mut slab,
            &[block_header, public_data, expected_aux_commitment, aux],
        );
        slab.set_root(root);
        let space = slab.noun_space();

        assert!(matches!(
            decode_pearl_merge_public_statement_noun(
                unsafe { *slab.root() },
                &space,
                CertificateNounLimits::default(),
            ),
            Err(CertificateNounError::PackedLengthMismatch {
                tag: "pearl-merge.aux.chain-id",
                declared: 8,
                actual: 9,
            })
        ));
    }

    #[test]
    fn pearl_merge_artifact_public_builder_round_trips_through_jam_decoder() {
        let (statement, aux_inclusion, commitments, pis, _, _) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let trace_height = pearl_test_trace_height(&params);
        let certificate = AiProofNode::Seq(vec![
            AiProofNode::U64(1),
            AiProofNode::Bytes(b"recursive-node-placeholder".to_vec()),
        ]);
        let artifact_slab = build_ai_pow_pearl_merge_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &zk_params_from_matmul(&params),
            0,
            trace_height,
            &commitments,
            &pis,
            &certificate,
        )
        .expect("build ai-pow artifact");

        let decoded = decode_ai_pow_pearl_merge_artifact_jam(
            &artifact_slab.jam(),
            CertificateNounLimits::default(),
        )
        .expect("decode jammed pearl merge artifact");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert_eq!(
            decoded.certificate.zk_params,
            zk_params_from_matmul(&params)
        );
        assert_eq!(decoded.certificate.found_idx, 0);
        assert_eq!(decoded.certificate.trace_height, trace_height);
        assert_eq!(decoded.certificate.commitments, commitments);
        assert_eq!(decoded.certificate.public_inputs, pis);
        assert_eq!(decoded.certificate.certificate, certificate);
    }

    #[test]
    fn pearl_merge_artifact_public_builder_rejects_oversized_nonce_evidence() {
        let (statement, mut aux_inclusion, commitments, pis, _, _) =
            pearl_merge_statement_fixture();
        aux_inclusion.merkle_branch = vec![[0x55; 32]; PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH + 1];
        let params = pearl_test_params();

        let err = build_ai_pow_pearl_merge_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        )
        .expect_err("oversized nonce evidence must not panic or build an artifact");

        assert!(matches!(
            err,
            CertificateNounError::LimitExceeded("ai-pow nonce merkle branch")
        ));
    }

    #[test]
    fn pearl_merge_artifact_public_builder_rejects_tampered_aux_inclusion() {
        let (statement, mut aux_inclusion, commitments, pis, _, _) =
            pearl_merge_statement_fixture();
        aux_inclusion.merkle_branch.push([0x66; 32]);
        let params = pearl_test_params();

        let err = build_ai_pow_pearl_merge_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        )
        .expect_err("tampered nonce inclusion must not build a canonical artifact");

        assert!(matches!(
            err,
            CertificateNounError::LimitExceeded("ai-pow nonce merkle branch")
        ));
    }

    #[test]
    fn pearl_merge_ai_pow_nonce_size_budget_is_pinned_and_bounded() {
        let (mut statement, _, commitments, pis, _, _) = pearl_merge_statement_fixture();
        statement.aux.nockchain_chain_id = vec![0x43; PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX];
        statement.aux.extra_domain_data = vec![0x45; PEARL_NOCKCHAIN_AUX_EXTRA_MAX];
        let aux_inclusion = PearlAuxInclusionProof {
            coinbase_tx: vec![0x51; PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES],
            merkle_branch: vec![[0x52; 32]; PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH],
        };

        let nonce = encode_pearl_merge_ai_pow_nonce(&statement, &aux_inclusion)
            .expect("max-size nonce should encode");
        assert_eq!(nonce.len(), AI_POW_NONCE_MAX_SIZE);
        assert_eq!(AI_POW_NONCE_MAX_SIZE, 101_424);

        let decoded =
            decode_pearl_merge_ai_pow_nonce(&nonce).expect("max-size nonce should decode");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);

        let mut too_large = nonce;
        too_large.push(0);
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&too_large),
            Err(CertificateNounError::LimitExceeded("ai-pow nonce bytes"))
        ));

        let params = pearl_test_params();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let jammed = artifact_slab.jam();
        assert!(
            jammed.len() <= 110 * 1024,
            "max nonce artifact jam grew past budget: {} bytes",
            jammed.len()
        );
        decode_ai_pow_pearl_merge_artifact_metadata_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("max-size nonce artifact metadata should decode");
    }

    #[test]
    fn pearl_merge_ai_pow_nonce_decoder_rejects_malformed_aip1_envelopes() {
        let (statement, aux_inclusion, _, _, _, _) = pearl_merge_statement_fixture();
        let valid = encode_pearl_merge_ai_pow_nonce(&statement, &aux_inclusion)
            .expect("valid nonce should encode");
        let statement_len = u16::from_le_bytes(valid[4..6].try_into().unwrap()) as usize;
        let statement_offset = 6usize;
        let statement_end = statement_offset + statement_len;
        let coinbase_len_offset = statement_end;
        let coinbase_len = u32::from_le_bytes(
            valid[coinbase_len_offset..coinbase_len_offset + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        let branch_len_offset = coinbase_len_offset + 4 + coinbase_len;

        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 0xff;
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&bad_magic),
            Err(CertificateNounError::Shape("ai-pow nonce magic"))
        ));

        let mut bad_statement_len = valid.clone();
        bad_statement_len[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&bad_statement_len),
            Err(CertificateNounError::Shape("ai-pow nonce statement length"))
        ));

        let mut bad_statement_magic = valid.clone();
        bad_statement_magic[statement_offset] ^= 0xff;
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&bad_statement_magic),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::BadMergePublicStatementMagic(_)
            ))
        ));

        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&valid[..10]),
            Err(CertificateNounError::Shape("ai-pow nonce is too short"))
        ));

        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&valid[..statement_end + 2]),
            Err(CertificateNounError::Shape("ai-pow nonce coinbase length"))
        ));

        let mut oversized_coinbase = valid.clone();
        oversized_coinbase[coinbase_len_offset..coinbase_len_offset + 4].copy_from_slice(
            &((PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES + 1) as u32).to_le_bytes(),
        );
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&oversized_coinbase),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow nonce coinbase bytes"
            ))
        ));

        let mut truncated_coinbase = valid.clone();
        truncated_coinbase[coinbase_len_offset..coinbase_len_offset + 4]
            .copy_from_slice(&((coinbase_len + 2) as u32).to_le_bytes());
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&truncated_coinbase),
            Err(CertificateNounError::Shape("ai-pow nonce coinbase length"))
        ));

        let mut oversized_branch = valid.clone();
        oversized_branch[branch_len_offset] = (PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH + 1) as u8;
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&oversized_branch),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow nonce merkle branch"
            ))
        ));

        let mut trailing_bytes = valid;
        trailing_bytes.push(0);
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce(&trailing_bytes),
            Err(CertificateNounError::Shape("ai-pow nonce trailing bytes"))
        ));
    }

    #[test]
    fn pearl_merge_artifact_jam_precheck_rejects_oversized_aux_branch_before_proof_node_decode() {
        let (statement, mut aux_inclusion, commitments, pis, a, b) =
            pearl_merge_statement_fixture();
        aux_inclusion.merkle_branch = vec![[0x55; 32]; PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH + 1];
        let params = pearl_test_params();
        let cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            |_| D(0),
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);

        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_jam(
                &artifact_slab.jam(),
                CertificateNounLimits::default(),
                &statement.aux.nock_block_commitment,
                &a,
                &b,
                &[0xffu8; 32],
                16,
            ),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow nonce merkle branch"
            ))
        ));
    }

    #[test]
    fn pearl_merge_artifact_precheck_rejects_replay_and_certificate_mismatch() {
        let (statement, aux_inclusion, commitments, pis, a, b) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact");

        let mut wrong_candidate = statement.aux.nock_block_commitment;
        wrong_candidate[0] ^= 1;
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &decoded, &wrong_candidate, &a, &b, &[0xffu8; 32], 16,
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainAuxBlockCommitmentMismatch
            ))
        ));

        let mut bad_pis = pis.clone();
        bad_pis.commitment_hash[0] ^= 1;
        let bad_cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &bad_pis,
            &AiProofNode::Unit,
        );
        let bad_artifact_slab =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &bad_cert_slab);
        let bad_decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &bad_artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with bad PIs");
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &bad_decoded, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32], 16,
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "public-inputs.commitment-hash"
            ))
        ));

        let mut bad_jackpot_pis = pis.clone();
        bad_jackpot_pis.jackpot[0] ^= 1;
        let bad_jackpot_cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &bad_jackpot_pis,
            &AiProofNode::Unit,
        );
        let bad_jackpot_artifact_slab =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &bad_jackpot_cert_slab);
        let bad_jackpot_decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &bad_jackpot_artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with bad jackpot message");
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &bad_jackpot_decoded, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32],
                16,
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "public-inputs.jackpot"
            ))
        ));

        let commitment_cases: [(&str, fn(&mut ZkPublicCommitments)); 2] = [
            ("commitments.h-a-chunk", |commitments| {
                commitments.h_a_chunk[0] ^= 1
            }),
            ("commitments.h-b-chunk", |commitments| {
                commitments.h_b_chunk[0] ^= 1
            }),
        ];
        for (field, tamper) in commitment_cases {
            let mut bad_commitments = commitments;
            tamper(&mut bad_commitments);
            let bad_commitments_slab = build_ai_pow_certificate_noun_from_node(
                &zk_params_from_matmul(&params),
                0,
                pearl_test_trace_height(&params),
                &bad_commitments,
                &pis,
                &AiProofNode::Unit,
            );
            let bad_commitments_artifact =
                build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &bad_commitments_slab);
            let bad_commitments_decoded = decode_ai_pow_pearl_merge_artifact_slab(
                &bad_commitments_artifact,
                CertificateNounLimits::default(),
            )
            .expect("decode pearl merge artifact with bad commitments");
            assert!(matches!(
                precheck_ai_pow_pearl_merge_artifact_statement(
                    &bad_commitments_decoded, &statement.aux.nock_block_commitment, &a, &b,
                    &[0xffu8; 32], 16,
                ),
                Err(CertificateNounError::PearlMergePublicInputMismatch(got)) if got == field
            ));
        }

        let wrong_found_idx_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            1,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let wrong_found_idx_artifact =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &wrong_found_idx_slab);
        let wrong_found_idx_decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &wrong_found_idx_artifact,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with bad found_idx");
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &wrong_found_idx_decoded, &statement.aux.nock_block_commitment, &a, &b,
                &[0xffu8; 32], 16,
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "found-idx"
            ))
        ));

        let wrong_trace_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params) + 1,
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let wrong_trace_artifact =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &wrong_trace_slab);
        let wrong_trace_decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &wrong_trace_artifact,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with bad trace height");
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &wrong_trace_decoded, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32],
                16,
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "trace-height"
            ))
        ));

        let mut bad_zk_params = zk_params_from_matmul(&params);
        bad_zk_params.difficulty_bits = 1;
        let wrong_difficulty_slab = build_ai_pow_certificate_noun_from_node(
            &bad_zk_params,
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let wrong_difficulty_artifact =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &wrong_difficulty_slab);
        let wrong_difficulty_decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &wrong_difficulty_artifact,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with bad difficulty bits");
        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_statement(
                &wrong_difficulty_decoded, &statement.aux.nock_block_commitment, &a, &b,
                &[0xffu8; 32], 16,
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "params.difficulty-bits"
            ))
        ));
    }

    #[test]
    fn pearl_merge_artifact_jam_precheck_rejects_tampered_aux_inclusion_before_proof_node_decode() {
        let (statement, mut aux_inclusion, commitments, pis, a, b) =
            pearl_merge_statement_fixture();
        aux_inclusion.merkle_branch.push([0x66; 32]);
        let params = pearl_test_params();
        let cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            |_| D(0),
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);

        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_jam(
                &artifact_slab.jam(),
                CertificateNounLimits::default(),
                &statement.aux.nock_block_commitment,
                &a,
                &b,
                &[0xffu8; 32],
                16,
            ),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow nonce merkle branch"
            ))
        ));
    }

    #[test]
    fn pearl_merge_artifact_accepts_non_native_pearl_geometry() {
        let (statement, aux_inclusion, commitments, pis, a, b, params) =
            unsupported_pearl_merge_geometry_fixture();
        assert_ne!(params.m % params.tile, 0);
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let decoded = decode_ai_pow_pearl_merge_artifact_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode pearl merge artifact with unsupported geometry");

        precheck_ai_pow_pearl_merge_artifact_statement(
            &decoded, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32], 16,
        )
        .expect("Pearl-valid non-native geometry should precheck");
    }

    #[test]
    fn pearl_merge_artifact_metadata_precheck_accepts_multi_tile_ticket_claim() {
        let params = MatmulParams {
            m: 16,
            n: 16,
            ..pearl_test_params()
        };
        assert!(params.num_tiles() > 1);
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = pearl_test_config();
        let (a, b) = synth_matrices(b"pearl-metadata-multi-tile", &params);
        let attempt = evaluate_pearl_merge_ticket_attempt(
            &header, &config, &params, 8, 0, &a, &b, &[0xff; 32], 16, aux,
        )
        .expect("evaluate multi-tile Pearl metadata attempt");
        let statement =
            PearlMergePublicStatementShape::from_wire_statement(&attempt.statement).unwrap();
        let pis =
            pearl_merge_recursive_public_inputs_from_work(&attempt.commitments, &attempt.ticket);
        let strip_schedule = StripIndexSchedule::from_indices(
            &zk_params_from_matmul(&params),
            attempt.ticket.a_rows.clone(),
            attempt.ticket.b_cols.clone(),
        )
        .expect("multi-tile Pearl schedule");
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            2,
            expected_layer0_rows_for_strip_schedule(&params, &strip_schedule)
                .expect("schedule-aware trace height")
                .required_trace_len(),
            &ZkPublicCommitments {
                h_a_chunk: attempt.commitments.h_a,
                h_b_chunk: attempt.commitments.h_b,
            },
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let metadata = decode_ai_pow_pearl_merge_artifact_metadata_slab(
            &artifact_slab,
            CertificateNounLimits::default(),
        )
        .expect("decode multi-tile Pearl metadata");

        let precheck = precheck_ai_pow_pearl_merge_artifact_metadata(
            &metadata, &statement.aux.nock_block_commitment, &a, &b, &[0xffu8; 32], 16,
        )
        .expect("metadata precheck accepts multi-tile Pearl ticket claim");
        assert_eq!(precheck.work.ticket, attempt.ticket);
    }

    #[test]
    fn pearl_merge_artifact_jam_precheck_rejects_replay_before_proof_node_decode() {
        let (statement, aux_inclusion, commitments, pis, a, b) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            |_| D(0),
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let jammed = artifact_slab.jam();
        let mut wrong_candidate = statement.aux.nock_block_commitment;
        wrong_candidate[0] ^= 1;

        assert!(matches!(
            precheck_ai_pow_pearl_merge_artifact_jam(
                &jammed,
                CertificateNounLimits::default(),
                &wrong_candidate,
                &a,
                &b,
                &[0xffu8; 32],
                16,
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainAuxBlockCommitmentMismatch
            ))
        ));

        let precheck = precheck_ai_pow_pearl_merge_artifact_jam(
            &jammed,
            CertificateNounLimits::default(),
            &statement.aux.nock_block_commitment,
            &a,
            &b,
            &[0xffu8; 32],
            16,
        )
        .expect("metadata-only jam precheck should not decode the bad proof node");
        assert_eq!(precheck.aux, statement.aux);
    }

    #[test]
    fn pearl_merge_command_metadata_precheck_rejects_replay_before_proof_node_decode() {
        let (statement, aux_inclusion, commitments, pis, a, b) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            |_| D(0),
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let command_slab = build_ai_pow_command_slab(&artifact_slab);
        let mut wrong_candidate = statement.aux.nock_block_commitment;
        wrong_candidate[0] ^= 1;

        assert!(matches!(
            precheck_ai_pow_pearl_merge_command_metadata_with_context(
                &command_slab,
                CertificateNounLimits::default(),
                PearlMergeAiPowVerifierContext {
                    candidate_nock_block_commitment: &wrong_candidate,
                    a_row_major: &a,
                    b_col_major: &b,
                    nockchain_target: &[0xffu8; 32],
                    max_pattern_len: 16,
                },
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainAuxBlockCommitmentMismatch
            ))
        ));

        assert!(matches!(
            precheck_ai_pow_pearl_merge_command_metadata_with_context(
                &command_slab,
                CertificateNounLimits::default(),
                PearlMergeAiPowVerifierContext {
                    candidate_nock_block_commitment: &statement.aux.nock_block_commitment,
                    a_row_major: &a,
                    b_col_major: &b,
                    nockchain_target: &[0u8; 32],
                    max_pattern_len: 16,
                },
            ),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::NockchainTargetNotMet
            ))
        ));

        let mut tampered_aux_inclusion = aux_inclusion.clone();
        tampered_aux_inclusion.merkle_branch.push([0x77; 32]);
        let tampered_aux_artifact_slab =
            build_pearl_merge_artifact_slab(&statement, &tampered_aux_inclusion, &cert_slab);
        let tampered_aux_command_slab = build_ai_pow_command_slab(&tampered_aux_artifact_slab);
        assert!(matches!(
            precheck_ai_pow_pearl_merge_command_metadata_with_context(
                &tampered_aux_command_slab,
                CertificateNounLimits::default(),
                PearlMergeAiPowVerifierContext {
                    candidate_nock_block_commitment: &statement.aux.nock_block_commitment,
                    a_row_major: &a,
                    b_col_major: &b,
                    nockchain_target: &[0xffu8; 32],
                    max_pattern_len: 16,
                },
            ),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow nonce merkle branch"
            ))
        ));

        let mut bad_commitments = commitments;
        bad_commitments.h_b_chunk[0] ^= 1;
        let bad_cert_slab = build_certificate_slab_with_statement_and_raw_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &bad_commitments,
            &pis,
            |_| D(0),
        );
        let bad_artifact_slab =
            build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &bad_cert_slab);
        let bad_command_slab = build_ai_pow_command_slab(&bad_artifact_slab);
        assert!(matches!(
            precheck_ai_pow_pearl_merge_command_metadata_with_context(
                &bad_command_slab,
                CertificateNounLimits::default(),
                PearlMergeAiPowVerifierContext {
                    candidate_nock_block_commitment: &statement.aux.nock_block_commitment,
                    a_row_major: &a,
                    b_col_major: &b,
                    nockchain_target: &[0xffu8; 32],
                    max_pattern_len: 16,
                },
            ),
            Err(CertificateNounError::PearlMergePublicInputMismatch(
                "commitments.h-b-chunk"
            ))
        ));

        let decoded = decode_ai_pow_pearl_merge_command_metadata_slab(
            &command_slab,
            CertificateNounLimits::default(),
        )
        .expect("command metadata should decode without walking proof tail");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
    }

    #[test]
    fn pearl_merge_command_metadata_decoder_rejects_wrong_command_shape() {
        let (statement, aux_inclusion, commitments, pis, _, _) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let artifact_space = artifact_slab.noun_space();

        let mut wrong_command: NounSlab = NounSlab::new();
        let artifact = wrong_command.copy_into(unsafe { *artifact_slab.root() }, &artifact_space);
        let root = T(
            &mut wrong_command,
            &[D(tas!(b"bad-cmd")), D(tas!(b"pow")), artifact],
        );
        wrong_command.set_root(root);
        assert!(matches!(
            decode_ai_pow_pearl_merge_command_metadata_slab(
                &wrong_command,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::Shape("expected %command"))
        ));

        let mut wrong_pow: NounSlab = NounSlab::new();
        let artifact = wrong_pow.copy_into(unsafe { *artifact_slab.root() }, &artifact_space);
        let root = T(
            &mut wrong_pow,
            &[D(tas!(b"command")), D(tas!(b"not-pow")), artifact],
        );
        wrong_pow.set_root(root);
        assert!(matches!(
            decode_ai_pow_pearl_merge_command_metadata_slab(
                &wrong_pow,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::Shape("expected %pow command"))
        ));

        let cert_space = cert_slab.noun_space();
        let mut wrong_artifact: NounSlab = NounSlab::new();
        let nonce_bytes = build_pearl_merge_nonce_bytes_for_test(&statement, &aux_inclusion);
        let nonce = build_ai_pow_nonce_noun(&mut wrong_artifact, &nonce_bytes);
        let cert = wrong_artifact.copy_into(unsafe { *cert_slab.root() }, &cert_space);
        let root = T(&mut wrong_artifact, &[D(tas!(b"badart")), nonce, cert]);
        wrong_artifact.set_root(root);
        let wrong_artifact_command = build_ai_pow_command_slab(&wrong_artifact);
        assert!(matches!(
            decode_ai_pow_pearl_merge_command_metadata_slab(
                &wrong_artifact_command,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::Shape("expected %ai-pow artifact"))
        ));
    }

    #[test]
    fn pearl_merge_artifact_jam_decoder_enforces_byte_limit_before_cue() {
        let (statement, aux_inclusion, commitments, pis, _, _) = pearl_merge_statement_fixture();
        let params = pearl_test_params();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &zk_params_from_matmul(&params),
            0,
            pearl_test_trace_height(&params),
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );
        let artifact_slab = build_pearl_merge_artifact_slab(&statement, &aux_inclusion, &cert_slab);
        let jammed = artifact_slab.jam();

        let decoded =
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, CertificateNounLimits::default())
                .expect("decode production Pearl merge artifact jam");
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert_eq!(decoded.certificate.public_inputs, pis);

        let mut non_canonical = jammed.to_vec();
        non_canonical.push(0xff);
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_jam(
                &non_canonical,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::NonCanonicalJam)
        ));

        let mut node_limits = CertificateNounLimits::default();
        node_limits.max_total_nodes = 1;
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, node_limits),
            Err(CertificateNounError::LimitExceeded("jam noun count"))
        ));

        let mut depth_limits = CertificateNounLimits::default();
        depth_limits.max_depth = 1;
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, depth_limits),
            Err(CertificateNounError::LimitExceeded("jam noun depth"))
        ));

        let mut atom_limits = CertificateNounLimits::default();
        atom_limits.max_atom_bytes = 0;
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, atom_limits),
            Err(CertificateNounError::LimitExceeded("jam atom bytes"))
        ));

        let mut limits = CertificateNounLimits::default();
        limits.max_jam_bytes = jammed.len() - 1;
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, limits),
            Err(CertificateNounError::JammedLengthExceeded { limit, actual })
                if limit == jammed.len() - 1 && actual == jammed.len()
        ));

        let err = decode_ai_pow_pearl_merge_artifact_jam(&[], CertificateNounLimits::default())
            .expect_err("malformed jam must reject");
        assert!(matches!(err, CertificateNounError::Cue(_)));
    }

    #[test]
    fn pearl_merge_artifact_decoder_rejects_malformed_nonce_shape_and_tag() {
        let params = sample_params();
        let commitments = sample_commitments();
        let pis = sample_pis();
        let cert_slab = build_ai_pow_certificate_noun_from_node(
            &params,
            0,
            16_384,
            &commitments,
            &pis,
            &AiProofNode::Unit,
        );

        let cert_space = cert_slab.noun_space();
        let mut bad_len_slab: NounSlab = NounSlab::new();
        let bad_nonce_atom = bytes_to_atom(&mut bad_len_slab, &[0xff; 11]);
        let bad_nonce = T(&mut bad_len_slab, &[D(10), bad_nonce_atom]);
        let cert = bad_len_slab.copy_into(unsafe { *cert_slab.root() }, &cert_space);
        let root = T(&mut bad_len_slab, &[D(tas!(b"ai-pow")), bad_nonce, cert]);
        bad_len_slab.set_root(root);
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_slab(
                &bad_len_slab,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::PackedLengthMismatch {
                tag: "ai-pow nonce",
                declared: 10,
                actual: 11,
            })
        ));

        let mut wrong_tag_slab: NounSlab = NounSlab::new();
        let nonce = b"wrong-tag-opaque-ai-pow-nonce".to_vec();
        let nonce = build_ai_pow_nonce_noun(&mut wrong_tag_slab, &nonce);
        let cert = wrong_tag_slab.copy_into(unsafe { *cert_slab.root() }, &cert_space);
        let root = T(&mut wrong_tag_slab, &[D(tas!(b"not-ai")), nonce, cert]);
        wrong_tag_slab.set_root(root);
        assert!(matches!(
            decode_ai_pow_pearl_merge_artifact_slab(
                &wrong_tag_slab,
                CertificateNounLimits::default()
            ),
            Err(CertificateNounError::Shape("expected %ai-pow artifact"))
        ));
    }

    #[test]
    fn certificate_noun_decoder_rejects_oversized_packed_atom() {
        let slab = build_certificate_slab_with_raw_node(|slab| {
            let data = bytes_to_atom(slab, &[0xffu8; 9]);
            T(slab, &[D(tas!(b"u64s")), D(1), data])
        });

        let err = decode_ai_pow_certificate_slab(&slab, CertificateNounLimits::default())
            .expect_err("oversized packed u64 atom should be rejected");
        assert!(matches!(
            err,
            CertificateNounError::PackedLengthMismatch {
                tag: "u64s",
                declared: 8,
                actual: 9,
            }
        ));
    }

    #[test]
    fn certificate_noun_decoder_rejects_noncanonical_ext2_limb() {
        let slab = build_certificate_slab_with_raw_node(|slab| {
            let data = ext2_to_atom(slab, [GOLDILOCKS_MODULUS, 1]);
            T(slab, &[D(tas!(b"ext2")), data])
        });

        let err = decode_ai_pow_certificate_slab(&slab, CertificateNounLimits::default())
            .expect_err("non-canonical Goldilocks limb should be rejected");
        assert!(matches!(
            err,
            CertificateNounError::NonCanonicalField { field: "ext2.c0" }
        ));
    }

    #[test]
    fn certificate_noun_decoder_enforces_list_limits() {
        let params = sample_params();
        let commitments = sample_commitments();
        let pis = sample_pis();
        let cert = AiProofNode::Seq(vec![AiProofNode::U64(1), AiProofNode::U64(2)]);
        let slab =
            build_ai_pow_certificate_noun_from_node(&params, 7, 8_192, &commitments, &pis, &cert);
        let limits = CertificateNounLimits {
            max_list_items: 1,
            ..CertificateNounLimits::default()
        };

        let err = decode_ai_pow_certificate_slab(&slab, limits)
            .expect_err("proof-node list limit should be enforced");
        assert!(matches!(
            err,
            CertificateNounError::LimitExceeded("proof-node list length")
        ));
    }

    #[test]
    fn compact_recursive_certificate_from_node_requires_canonical_bytes() {
        assert!(matches!(
            ai_pow_compact_recursive_certificate_from_node(&AiProofNode::Unit),
            Err(CertificateNounError::Shape(
                "compact recursive certificate must be bytes"
            ))
        ));

        let err = match ai_pow_compact_recursive_certificate_from_node(&AiProofNode::Bytes(vec![
            0xff, 0x00,
        ])) {
            Ok(_) => panic!("invalid compact postcard bytes must reject"),
            Err(err) => err,
        };
        assert!(matches!(err, CertificateNounError::Deserialize(_)));
    }

    #[test]
    fn rb02_compact_recursive_certificate_from_node_rejects_overlimit_bytes_as_limit() {
        let overlimit = AiProofNode::Bytes(vec![
            0u8;
            ai_pow_zk::recursion::MAX_COMPACT_CERTIFICATE_BYTES
        ]);
        assert!(matches!(
            ai_pow_compact_recursive_certificate_from_node(&overlimit),
            Err(CertificateNounError::LimitExceeded(
                "compact recursive certificate bytes"
            ))
        ));

        let under_limit_invalid = AiProofNode::Bytes(vec![
            0xff;
            ai_pow_zk::recursion::MAX_COMPACT_CERTIFICATE_BYTES
                - 1
        ]);
        assert!(matches!(
            ai_pow_compact_recursive_certificate_from_node(&under_limit_invalid),
            Err(CertificateNounError::Deserialize(_))
        ));
    }

    #[test]
    fn certificate_noun_decoder_rejects_wrong_version_and_bad_tag() {
        let slab = build_certificate_slab_with_raw_node(|slab| T(slab, &[D(tas!(b"wat")), D(0)]));
        let err = decode_ai_pow_certificate_slab(&slab, CertificateNounLimits::default())
            .expect_err("bad proof-node tag should be rejected");
        assert!(matches!(err, CertificateNounError::InvalidTag(_)));

        let mut slab: NounSlab = NounSlab::new();
        let params = encode_params(&mut slab, &sample_params());
        let commitments = encode_commitments(&mut slab, &sample_commitments());
        let public_inputs = encode_public_inputs(&mut slab, &sample_pis());
        let cert = AiProofNode::Unit.to_noun(&mut slab);
        let root = T(
            &mut slab,
            &[D(99), params, D(7), D(8_192), commitments, public_inputs, cert],
        );
        slab.set_root(root);

        let err = decode_ai_pow_certificate_slab(&slab, CertificateNounLimits::default())
            .expect_err("wrong certificate version should be rejected");
        assert!(matches!(err, CertificateNounError::UnsupportedVersion(99)));
    }

    /// M3 (end-to-end, real proving) — a full MoE (`AIM1`) compact artifact
    /// proves, assembles into a `PearlMergeAiPowArtifactShape`, and VERIFIES
    /// through the node MoE branch: aux binding + MoE-aware envelope + routing
    /// binding + **jackpot/difficulty binding** + routing/PI/schedule binding +
    /// compact proof (P0/D6 fold). Adversarial: forged routing, unmet difficulty,
    /// and routing a MoE artifact through the dense verify all reject.
    ///
    /// This is the node-boundary validation of the whole MoE compact path with a
    /// statement-derived `kappa` (the node re-derives it from the block header +
    /// config, so this exercises the full chain the M2 kappa-fixed test does not).
    #[test]
    #[ignore = "real MoE compact recursive proof generation is opt-in (M2/M3)"]
    fn real_moe_compact_pearl_merge_artifact_verifies_through_node_branch() {
        use ai_pow::pearl_compat::derive_pearl_work_commitments;
        use ai_pow::pearl_moe_routing::build_routing_data;
        use ai_pow::zk_bridge::prove_pearl_moe_compact_recursive_certificate;

        // Envelope-valid MoE dims: k=1024, r=64, h=w=8, m=n=64, e=2 (n_e=32).
        let (m, n, e, top_k, n_e) = (64usize, 64usize, 2usize, 1usize, 32usize);
        let params = MatmulParams {
            m: m as u32,
            k: 1024,
            n: n as u32,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };

        // CANONICAL matrices — the protocol seed + these params, exactly as a
        // consensus verifier re-derives them (validates verify_ai_pow_block_artifact_jam).
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);
        assert_eq!(a.len(), m * 1024);
        assert_eq!(b.len(), n * 1024);

        // Valid aux binding + the block header it fixes.
        let aux = pearl_test_aux();
        let aux_commitment = aux.commitment().unwrap();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux_commitment);

        let config = PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
        };

        // Node-matching kappa/h_a/h_b: derived from the statement (header + config),
        // exactly as the node re-derives them in verify_pearl_moe_compatible_work.
        let sigma = header.to_bytes();
        let mu = config.to_bytes().unwrap();
        let commitments = derive_pearl_work_commitments(&sigma, &mu, &a, &b);

        let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
        let routing = build_routing_data(&topk, m, top_k, e).unwrap();
        let inner: Vec<u32> = config
            .rows_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let local_b: Vec<u32> = config
            .cols_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let expert_idx = 0usize;

        eprintln!("MoE artifact e2e: proving compact certificate (real proving)");
        let run = prove_pearl_moe_compact_recursive_certificate(
            &params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing,
            expert_idx, &inner, &local_b, n_e,
        )
        .expect("prove MoE compact certificate");

        let public = PearlPublicProofParams {
            block_header: header,
            mining_config: config,
            hash_a: commitments.h_a,
            hash_b: commitments.h_b,
            hash_jackpot: run.ticket.jackpot_hash,
            m: m as u32,
            n: n_e as u32,
            t_rows: 0,
            t_cols: 0,
        };
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux_commitment,
            aux: aux.clone(),
        };
        let cert_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&run.compact_cert)
                .unwrap();
        let certificate = AiPowCertificateShape {
            version: AI_POW_CERT_VERSION,
            zk_params: run.zk_params,
            found_idx: 0,
            trace_height: run.trace_height,
            commitments: run.commitments,
            public_inputs: run.pis.clone(),
            certificate: AiProofNode::Bytes(cert_bytes),
        };
        let moe_art = PearlMergeMoeArtifact {
            moe: PearlMoeParams {
                expert_idx: expert_idx as u16,
                routing_offsets: routing.routing_offsets.clone(),
                hash_routing: run.ticket.commitment.routing_root,
                outer_indices: run.ticket.outer_indices.clone(),
            },
            routing_data: routing.routing_data.clone(),
        };
        let artifact = PearlMergeAiPowArtifactShape {
            statement,
            aux_inclusion,
            certificate,
            moe: Some(moe_art),
        };

        const LOOSE_TARGET: [u8; 32] = [0xffu8; 32];
        let ctx = |target: &'static [u8; 32]| PearlMergeAiPowVerifierContext {
            candidate_nock_block_commitment: &aux.nock_block_commitment,
            a_row_major: &a,
            b_col_major: &b,
            nockchain_target: target,
            max_pattern_len: 4096,
        };
        let expected_digest = run.verifier_key_digest();

        // Happy path: the node MoE branch verifies the full artifact end-to-end.
        let pre = verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
            &artifact,
            ctx(&LOOSE_TARGET),
            &run.verifier_context,
            &expected_digest,
            CertificateNounLimits::default(),
        )
        .expect("MoE artifact verifies through the node branch");
        assert_eq!(pre.work.jackpot_hash, run.ticket.jackpot_hash);
        assert_eq!(pre.aux_commitment, aux_commitment);

        // M1 encoder round-trip: build the MoE artifact NOUN, jam it, decode it back
        // through the bounded parser, and confirm it (a) equals the directly-built
        // shape and (b) verifies through the node MoE branch. Closes the full
        // encode → jam → decode → verify artifact path for MoE.
        let noun_slab = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &artifact.statement,
            &artifact.aux_inclusion,
            artifact.moe.as_ref().unwrap(),
            &artifact.certificate.zk_params,
            artifact.certificate.found_idx,
            artifact.certificate.trace_height,
            &artifact.certificate.commitments,
            &artifact.certificate.public_inputs,
            &artifact.certificate.certificate,
        )
        .expect("build MoE artifact noun");
        let jammed = noun_slab.jam();
        let decoded =
            decode_ai_pow_pearl_merge_artifact_jam(&jammed, CertificateNounLimits::default())
                .expect("decode MoE artifact jam");
        assert_eq!(
            decoded, artifact,
            "decoded MoE artifact noun must equal the directly-built shape",
        );
        assert!(
            decoded.moe.is_some(),
            "decoded artifact must carry the MoE tail"
        );
        let pre_noun =
            verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
                &decoded,
                ctx(&LOOSE_TARGET),
                &run.verifier_context,
                &expected_digest,
                CertificateNounLimits::default(),
            )
            .expect("MoE artifact decoded from its noun verifies through the node branch");
        assert_eq!(pre_noun.work.jackpot_hash, run.ticket.jackpot_hash);

        // The jam boundary the consensus dispatcher calls: verify straight from the
        // jammed bytes through the MoE jam entry.
        let expected_digest_bytes =
            ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&expected_digest);
        let pre_jam =
            verify_ai_pow_pearl_merge_compact_moe_artifact_jam_with_digest_bytes_and_context(
                &jammed,
                CertificateNounLimits::default(),
                ctx(&LOOSE_TARGET),
                &run.verifier_context,
                &expected_digest_bytes,
            )
            .expect("MoE artifact verifies through the jam boundary entry");
        assert_eq!(pre_jam.work.jackpot_hash, run.ticket.jackpot_hash);
        // The dense jam entry must reject a MoE artifact (tag dispatch).
        assert!(
            verify_ai_pow_pearl_merge_compact_artifact_jam_with_digest_bytes_and_context(
                &jammed,
                CertificateNounLimits::default(),
                ctx(&LOOSE_TARGET),
                &run.verifier_context,
                &expected_digest_bytes,
            )
            .is_err(),
            "dense jam entry must reject a MoE artifact",
        );

        // The SELF-CONTAINED consensus entrypoint (the jet target): it re-derives
        // the canonical matrices from the protocol seed (NOT supplied here) + the
        // block params, dispatches on the AIM1 tag, and verifies. This proves the
        // a/b sourcing needs no external distribution.
        let outcome = verify_ai_pow_block_artifact_jam(
            &jammed,
            CertificateNounLimits::default(),
            &aux.nock_block_commitment,
            &LOOSE_TARGET,
            4096,
            &run.verifier_context,
            &expected_digest_bytes,
        )
        .expect("MoE block verifies through the self-contained consensus entrypoint");
        match outcome {
            AiPowBlockVerifyOutcome::Moe(p) => {
                assert_eq!(p.work.jackpot_hash, run.ticket.jackpot_hash)
            }
            AiPowBlockVerifyOutcome::Dense(_) => panic!("MoE artifact routed to the dense outcome"),
        }
        // Consensus entrypoint also enforces difficulty (target 0 ⇒ reject).
        assert!(
            verify_ai_pow_block_artifact_jam(
                &jammed,
                CertificateNounLimits::default(),
                &aux.nock_block_commitment,
                &[0u8; 32],
                4096,
                &run.verifier_context,
                &expected_digest_bytes,
            )
            .is_err(),
            "consensus entrypoint must reject an unmet difficulty target",
        );

        // Adversarial 1 — forged routing_data breaks the routing-consistency binding.
        let mut forged = artifact.clone();
        forged.moe.as_mut().unwrap().routing_data[0] ^= 1;
        assert!(
            verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
                &forged,
                ctx(&LOOSE_TARGET),
                &run.verifier_context,
                &expected_digest,
                CertificateNounLimits::default(),
            )
            .is_err(),
            "forged routing must reject",
        );

        // Adversarial 2 — unmet difficulty (target 0 ⇒ adjusted target 0).
        const ZERO_TARGET: [u8; 32] = [0u8; 32];
        assert!(
            verify_decoded_ai_pow_pearl_merge_compact_moe_artifact_with_context_and_limits(
                &artifact,
                ctx(&ZERO_TARGET),
                &run.verifier_context,
                &expected_digest,
                CertificateNounLimits::default(),
            )
            .is_err(),
            "unmet difficulty must reject",
        );

        // Adversarial 3 — routing a MoE artifact through the DENSE compact verify
        // is rejected by the dispatch guard.
        assert!(
            verify_decoded_ai_pow_pearl_merge_compact_artifact_with_context_and_limits(
                &artifact,
                ctx(&LOOSE_TARGET),
                &run.verifier_context,
                &expected_digest,
                CertificateNounLimits::default(),
            )
            .is_err(),
            "dense verify must reject a MoE artifact",
        );

        eprintln!(
            "MoE artifact e2e: OK — cert {} bytes, trace_height {}",
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&run.compact_cert)
                .unwrap()
                .len(),
            run.trace_height,
        );
    }

    /// Build + prove a MoE block distinguished only by `nock_commit` (⇒ a distinct
    /// header ⇒ distinct `kappa` ⇒ distinct proof), holding the puzzle SHAPE
    /// (params, schedule, trace-height) fixed. Returns the prove run + the jammed
    /// artifact. Used to validate that the compact verifier setup is
    /// proof-INDEPENDENT (D4 setup builder).
    #[cfg(test)]
    fn prove_moe_block_for_setup_test(
        nock_commit: [u8; 32],
    ) -> (ai_pow::zk_bridge::PearlMoeCompactProveRun, Vec<u8>) {
        use ai_pow::pearl_compat::derive_pearl_work_commitments;
        use ai_pow::pearl_moe_routing::build_routing_data;
        use ai_pow::zk_bridge::prove_pearl_moe_compact_recursive_certificate;

        let (m, n, e, top_k, n_e) = (64usize, 64usize, 2usize, 1usize, 32usize);
        let params = MatmulParams {
            m: m as u32,
            k: 1024,
            n: n as u32,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let (a, b) = ai_pow::synth::synth_matrices(ai_pow::synth::AI_POW_PROD_SYNTH_SEED, &params);

        let mut aux = pearl_test_aux();
        aux.nock_block_commitment = nock_commit;
        let aux_commitment = aux.commitment().unwrap();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux_commitment);
        let config = PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
        };
        let commitments =
            derive_pearl_work_commitments(&header.to_bytes(), &config.to_bytes().unwrap(), &a, &b);
        let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
        let routing = build_routing_data(&topk, m, top_k, e).unwrap();
        let inner: Vec<u32> = config
            .rows_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let local_b: Vec<u32> = config
            .cols_pattern
            .indices_with_offset_bounded(0, 4096)
            .unwrap();
        let run = prove_pearl_moe_compact_recursive_certificate(
            &params, &a, &b, &commitments.kappa, &commitments.h_a, &commitments.h_b, &routing, 0,
            &inner, &local_b, n_e,
        )
        .expect("prove MoE compact certificate for setup test");

        let public = PearlPublicProofParams {
            block_header: header,
            mining_config: config,
            hash_a: commitments.h_a,
            hash_b: commitments.h_b,
            hash_jackpot: run.ticket.jackpot_hash,
            m: m as u32,
            n: n_e as u32,
            t_rows: 0,
            t_cols: 0,
        };
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux_commitment,
            aux,
        };
        let cert_bytes =
            ai_pow_zk::recursion::encode_compact_batch_recursive_certificate(&run.compact_cert)
                .unwrap();
        let certificate = AiPowCertificateShape {
            version: AI_POW_CERT_VERSION,
            zk_params: run.zk_params,
            found_idx: 0,
            trace_height: run.trace_height,
            commitments: run.commitments,
            public_inputs: run.pis.clone(),
            certificate: AiProofNode::Bytes(cert_bytes),
        };
        let moe_art = PearlMergeMoeArtifact {
            moe: PearlMoeParams {
                expert_idx: 0,
                routing_offsets: routing.routing_offsets.clone(),
                hash_routing: run.ticket.commitment.routing_root,
                outer_indices: run.ticket.outer_indices.clone(),
            },
            routing_data: routing.routing_data.clone(),
        };
        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &statement, &aux_inclusion, &moe_art, &certificate.zk_params, certificate.found_idx,
            certificate.trace_height, &certificate.commitments, &certificate.public_inputs,
            &certificate.certificate,
        )
        .expect("build MoE artifact noun for setup test")
        .jam()
        .to_vec();
        (run, jammed)
    }

    /// D4 setup builder — **proof-independence of the compact verifier setup.**
    ///
    /// A consensus node must build the compact verifier `context` + `verifier_key_digest`
    /// ONCE (from the fixed production params) and reuse it for every incoming block —
    /// it cannot re-run the prover per block. That is sound iff the setup is
    /// determined by the puzzle SHAPE (params/trace-height), NOT the specific proof.
    /// This proves it: two MoE blocks of identical shape but different `kappa`
    /// (different aux) produce the SAME `verifier_key_digest`, and block A's jammed
    /// artifact verifies through the consensus entrypoint using block B's
    /// independently-built `verifier_context`.
    #[test]
    #[ignore = "runs two real MoE compact proofs (~50s); opt-in"]
    fn moe_compact_verifier_setup_is_proof_independent() {
        let (run_a, jam_a) = prove_moe_block_for_setup_test([0x42u8; 32]);
        let (run_b, _jam_b) = prove_moe_block_for_setup_test([0x99u8; 32]);

        // The verifier-key digest depends only on the puzzle shape, not the proof.
        assert_eq!(
            run_a.verifier_key_digest(),
            run_b.verifier_key_digest(),
            "compact verifier-key digest must be proof-INDEPENDENT (same shape ⇒ same setup)",
        );

        // Block A verifies against block B's INDEPENDENTLY-built context + digest —
        // i.e. a node that built its setup from any canonical prove verifies all
        // same-shape blocks. This is what makes a build-once startup setup sound.
        let digest_b_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &run_b.verifier_key_digest(),
        );
        let outcome = verify_ai_pow_block_artifact_jam(
            &jam_a,
            CertificateNounLimits::default(),
            &[0x42u8; 32],
            &[0xffu8; 32],
            4096,
            &run_b.verifier_context,
            &digest_b_bytes,
        )
        .expect("block A verifies against block B's independently-built verifier setup");
        assert!(matches!(outcome, AiPowBlockVerifyOutcome::Moe(_)));
    }

    // ===================================================================
    // M1: MoE (GROUPED_GEMM) `ai-pow-nonce` codec.
    //
    // The codec is additive: the dense `AIP1` path is byte-identical and
    // untouched; MoE uses the `AIM1` tag and appends the Pearl MoE tail plus a
    // DoS-capped `routing_data` block. These tests exercise round-trip fidelity,
    // dense/MoE tag disjointness, every cap, and adversarial truncation/trailing
    // input (a crafted nonce must error, never panic or over-allocate). This
    // codec only carries and bounds bytes; live MoE acceptance additionally
    // requires routing, schedule, work, and compact-certificate verification.
    // ===================================================================

    fn moe_nonce_test_statement(
        e: u16,
        top_k: u16,
    ) -> (PearlMergePublicStatementShape, PearlAuxInclusionProof) {
        let aux = pearl_test_aux();
        let (header, aux_inclusion) = pearl_test_aux_inclusion(&aux.commitment().unwrap());
        let config = PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pearl_test_pattern(8),
            cols_pattern: pearl_test_pattern(8),
            reserved: PearlMiningConfig::moe_trailer(e, top_k),
        };
        let public = PearlPublicProofParams {
            block_header: header,
            mining_config: config,
            hash_a: [0x11; 32],
            hash_b: [0x22; 32],
            hash_jackpot: [0x33; 32],
            m: 64,
            n: 64,
            t_rows: 0,
            t_cols: 0,
        };
        let statement = PearlMergePublicStatementShape {
            block_header: header.to_bytes(),
            public_data: public.to_public_data().unwrap(),
            expected_aux_commitment: aux.commitment().unwrap(),
            aux,
        };
        (statement, aux_inclusion)
    }

    fn moe_nonce_test_artifact(
        e: usize,
        num_outer: usize,
        num_routing: usize,
    ) -> PearlMergeMoeArtifact {
        PearlMergeMoeArtifact {
            moe: PearlMoeParams {
                expert_idx: 0,
                routing_offsets: (1..=e as u32).map(|i| i * 2).collect(),
                hash_routing: [0x44; 32],
                outer_indices: (0..num_outer as u32).collect(),
            },
            routing_data: (0..num_routing as u32).collect(),
        }
    }

    #[test]
    fn rb03_moe_nonce_cap_fits_default_atom_limit_and_cap_plus_one_rejects() {
        assert!(AI_POW_NONCE_MOE_MAX_SIZE <= CertificateNounLimits::default().max_atom_bytes);
        let e = PEARL_MOE_MAX_NUM_EXPERTS;
        let (statement, aux_inclusion) = moe_nonce_test_statement(e as u16, 1);
        let art = moe_nonce_test_artifact(
            e, PEARL_MOE_MAX_OUTER_INDICES, PEARL_MOE_MAX_ROUTING_ENTRIES,
        );
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art)
            .expect("max-size MoE nonce encodes");
        assert!(nonce.len() <= AI_POW_NONCE_MOE_MAX_SIZE);
        decode_pearl_merge_ai_pow_nonce_moe(&nonce).expect("max-size MoE nonce decodes");

        let over_cap = moe_nonce_test_artifact(
            e,
            PEARL_MOE_MAX_OUTER_INDICES,
            PEARL_MOE_MAX_ROUTING_ENTRIES + 1,
        );
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &over_cap),
            Err(CertificateNounError::LimitExceeded(
                "ai-pow MoE routing_data"
            ))
        ));
    }

    #[test]
    fn rb03_full_artifact_decode_dispatches_aim1_nonce_over_dense_cap_to_moe() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let routing_entries_over_dense_cap = AI_POW_NONCE_MAX_SIZE / 4 + 1;
        let art = moe_nonce_test_artifact(4, 2, routing_entries_over_dense_cap);
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art)
            .expect("MoE nonce over dense cap encodes");
        assert!(nonce.len() > AI_POW_NONCE_MAX_SIZE);
        assert!(nonce.len() <= AI_POW_NONCE_MOE_MAX_SIZE);

        let slab = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &statement,
            &aux_inclusion,
            &art,
            &sample_params(),
            0,
            8_192,
            &sample_commitments(),
            &sample_pis(),
            &AiProofNode::Unit,
        )
        .expect("build MoE artifact over dense nonce cap");
        let decoded =
            decode_ai_pow_pearl_merge_artifact_slab(&slab, CertificateNounLimits::default())
                .expect("full artifact decoder dispatches AIM1 before dense cap");
        assert_eq!(decoded.moe, Some(art));
    }

    #[test]
    fn rb05_moe_certificate_zk_params_reject_nonzero_difficulty_bits() {
        let (statement, _) = moe_nonce_test_statement(4, 2);
        let header = PearlIncompleteBlockHeader::from_bytes(&statement.block_header).unwrap();
        let public_params =
            PearlPublicProofParams::from_public_data_allowing_moe(header, &statement.public_data)
                .unwrap();
        let mut zk = ZkParams {
            m: public_params.m,
            k: public_params.mining_config.common_dim,
            n: public_params.total_b_cols().unwrap(),
            noise_rank: u32::from(public_params.mining_config.rank),
            tile: 8,
            difficulty_bits: 1,
        };
        assert!(matches!(
            moe_matmul_params_from_certificate_zk(&public_params, &zk),
            Err(CertificateNounError::PearlMergeStatement(
                PearlCompatError::UnsupportedRecursivePearlParams(_)
            ))
        ));
        zk.difficulty_bits = 0;
        moe_matmul_params_from_certificate_zk(&public_params, &zk)
            .expect("zero difficulty_bits is accepted at the MoE boundary");
    }

    #[test]
    fn moe_nonce_round_trips_and_is_tagged() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 6, 40);
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        assert_eq!(&nonce[0..4], &AI_POW_NONCE_MAGIC_MOE);
        let decoded = decode_pearl_merge_ai_pow_nonce_moe(&nonce).unwrap();
        assert_eq!(decoded.statement, statement);
        assert_eq!(decoded.aux_inclusion, aux_inclusion);
        assert_eq!(decoded.moe, art);
    }

    #[test]
    fn moe_nonce_round_trips_empty_routing_and_outer() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(2, 1);
        let art = moe_nonce_test_artifact(2, 0, 0);
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        let decoded = decode_pearl_merge_ai_pow_nonce_moe(&nonce).unwrap();
        assert_eq!(decoded.moe, art);
        assert_eq!(decoded.statement, statement);
    }

    #[test]
    fn moe_nonce_round_trips_at_max_experts_and_outer() {
        let e = PEARL_MOE_MAX_NUM_EXPERTS;
        let (statement, aux_inclusion) = moe_nonce_test_statement(e as u16, 1);
        let art = moe_nonce_test_artifact(e, PEARL_MOE_MAX_OUTER_INDICES, 100);
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        let decoded = decode_pearl_merge_ai_pow_nonce_moe(&nonce).unwrap();
        assert_eq!(decoded.moe, art);
        assert_eq!(decoded.statement, statement);
    }

    #[test]
    fn moe_nonce_reuses_dense_framing_verbatim() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 3, 8);
        let dense = encode_pearl_merge_ai_pow_nonce(&statement, &aux_inclusion).unwrap();
        let moe = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        assert_eq!(&dense[0..4], &AI_POW_NONCE_MAGIC);
        // Everything after the 4-byte magic is the dense framing verbatim.
        assert_eq!(&moe[4..dense.len()], &dense[4..]);
        assert!(moe.len() > dense.len());
    }

    #[test]
    fn dense_and_moe_decoders_reject_each_others_tags() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 2, 8);
        let dense = encode_pearl_merge_ai_pow_nonce(&statement, &aux_inclusion).unwrap();
        let moe = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        assert!(decode_pearl_merge_ai_pow_nonce_moe(&dense).is_err());
        assert!(decode_pearl_merge_ai_pow_nonce(&moe).is_err());
    }

    #[test]
    fn moe_encode_rejects_dense_core() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(0, 0);
        let art = moe_nonce_test_artifact(1, 0, 0);
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::Shape(_))
        ));
    }

    #[test]
    fn moe_encode_rejects_expert_count_mismatch() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let mut art = moe_nonce_test_artifact(4, 2, 8);
        art.moe.routing_offsets.pop();
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::Shape(_))
        ));
    }

    #[test]
    fn moe_encode_rejects_expert_idx_out_of_range() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let mut art = moe_nonce_test_artifact(4, 2, 8);
        art.moe.expert_idx = 4;
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::Shape(_))
        ));
    }

    #[test]
    fn moe_encode_rejects_outer_over_cap() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, PEARL_MOE_MAX_OUTER_INDICES + 1, 8);
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::LimitExceeded(_))
        ));
    }

    #[test]
    fn moe_encode_rejects_routing_over_cap() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 2, PEARL_MOE_MAX_ROUTING_ENTRIES + 1);
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::LimitExceeded(_))
        ));
    }

    #[test]
    fn moe_encode_rejects_expert_count_over_cap() {
        let (statement, aux_inclusion) =
            moe_nonce_test_statement((PEARL_MOE_MAX_NUM_EXPERTS + 1) as u16, 1);
        let art = moe_nonce_test_artifact(PEARL_MOE_MAX_NUM_EXPERTS + 1, 2, 8);
        assert!(matches!(
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art),
            Err(CertificateNounError::LimitExceeded(_))
        ));
    }

    #[test]
    fn moe_decode_rejects_routing_len_over_cap_without_allocating() {
        // A crafted nonce whose `routing_len` field claims > cap must be rejected
        // by the cap check *before* any allocation, not by running out of memory.
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 2, 4);
        let mut nonce =
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        // routing_len u32 precedes the 4 carried entries (4 * 4 = 16 bytes).
        let routing_len_pos = nonce.len() - 4 - 4 * 4;
        nonce[routing_len_pos..routing_len_pos + 4]
            .copy_from_slice(&((PEARL_MOE_MAX_ROUTING_ENTRIES as u32) + 1).to_le_bytes());
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce_moe(&nonce),
            Err(CertificateNounError::LimitExceeded(_))
        ));
    }

    #[test]
    fn moe_decode_rejects_expert_count_over_cap_in_core_trailer() {
        // Build a valid e=4 nonce, then poison the mining-config trailer's `e` to
        // 1025. The decoder must cap on `e` before reading the (mismatched) tail.
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 2, 8);
        let mut nonce =
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        // Nonce: nonce_magic(4) | stmt_len(2) | [stmt_magic(4) | block_header(76)
        // | public_data(164) | ...]. Mining-config trailer `e` is at
        // public_data[20..22] (common_dim 4 + rank 2 + mma 2 + rows 6 + cols 6).
        let e_pos = 4 + 2 + 4 + PEARL_INCOMPLETE_BLOCK_HEADER_SIZE + 20;
        nonce[e_pos..e_pos + 2]
            .copy_from_slice(&((PEARL_MOE_MAX_NUM_EXPERTS + 1) as u16).to_le_bytes());
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce_moe(&nonce),
            Err(CertificateNounError::LimitExceeded(_))
        ));
    }

    #[test]
    fn moe_decode_rejects_truncation_at_every_length() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 5, 12);
        let nonce = encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        for cut in 0..nonce.len() {
            assert!(
                decode_pearl_merge_ai_pow_nonce_moe(&nonce[..cut]).is_err(),
                "truncation to {cut} bytes must error, not panic"
            );
        }
        assert!(decode_pearl_merge_ai_pow_nonce_moe(&nonce).is_ok());
    }

    #[test]
    fn moe_decode_rejects_trailing_bytes() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 5, 12);
        let mut nonce =
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        nonce.push(0);
        assert!(matches!(
            decode_pearl_merge_ai_pow_nonce_moe(&nonce),
            Err(CertificateNounError::Shape(_))
        ));
    }

    #[test]
    fn moe_decode_faithfully_carries_routing_data() {
        let (statement, aux_inclusion) = moe_nonce_test_statement(4, 2);
        let art = moe_nonce_test_artifact(4, 5, 12);
        let mut nonce =
            encode_pearl_merge_ai_pow_nonce_moe(&statement, &aux_inclusion, &art).unwrap();
        let last = nonce.len() - 1;
        nonce[last] ^= 0xff;
        let decoded = decode_pearl_merge_ai_pow_nonce_moe(&nonce).unwrap();
        assert_ne!(decoded.moe.routing_data, art.routing_data);
    }
}
