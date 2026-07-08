//! Pearl merge-mining compatibility primitives.
//!
//! This module is intentionally separate from the native Nockchain AI-PoW
//! transcript. Pearl-compatible merge mining means sharing the same Pearl work
//! attempt and jackpot digest, not sharing proof bytes. The canonical attempt
//! transcript here is:
//!
//! ```text
//! kappa = BLAKE3(sigma || mu)
//! H_A   = BLAKE3(pad(A_row_major), key=kappa)
//! H_B   = BLAKE3(pad(B_col_major), key=kappa)
//! s_B   = BLAKE3(kappa || H_B)
//! s_A   = BLAKE3(s_B || H_A)
//! hash  = BLAKE3(M_i_j, key=s_A)
//! ```
//!
//! Nockchain-native proof systems may prove this statement with their own
//! recursive certificate format, but they must not change these public work
//! bytes in Pearl-compatible mode.

use blake3::Hasher;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::commit::matrix_commitment;
use crate::fiat_shamir::{noise_seed_a, noise_seed_b};
use crate::matmul::{
    compute_pattern_tile_trace_from_slices, compute_tile, BlockNoise, Matrices, TileState,
};
use crate::params::{MatmulParams, ParamError, STRIPE_MAX};
use crate::prng;
use crate::tile_hash::hash_le_target;

const INPUT_RANGE_MAX: i8 = 64;

pub const PEARL_INCOMPLETE_BLOCK_HEADER_SIZE: usize = 76;
pub const PEARL_MINING_CONFIG_SIZE: usize = 52;
pub const PEARL_MINING_CONFIG_RESERVED_SIZE: usize = 32;
pub const PEARL_MMA_INT7XINT7_TO_INT32: u16 = 0;
pub const PEARL_PUBLIC_PROOF_PARAMS_SIZE: usize = 164;
pub const PEARL_TILE_D: u32 = 16;
pub const PEARL_TILE_H: u32 = 2;
pub const PEARL_DWORD_SIZE: u32 = 8;
pub const PEARL_WORKER_INPUT_MAX: u64 = 1 << 22;
pub const PEARL_NOCKCHAIN_AUX_DOMAIN: &[u8] = b"nockchain-ai-pow-aux-v1";
pub const PEARL_NOCKCHAIN_AUX_MAGIC: [u8; 4] = *b"NPA1";
pub const PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX: usize = 64;
pub const PEARL_NOCKCHAIN_AUX_EXTRA_MAX: usize = 1024;
pub const PEARL_NOCKCHAIN_AUX_MIN_SIZE: usize = 4 + 1 + 1 + 32 + 8 + 2;
pub const PEARL_NOCKCHAIN_AUX_MAX_SIZE: usize =
    4 + 1 + PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX + 32 + 8 + 2 + PEARL_NOCKCHAIN_AUX_EXTRA_MAX;
pub const PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG: &[u8] = b"NOCKCHAIN-AI-POW-AUX";
pub const PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES: usize = 100_000;
/// Current production merge-mining profile uses Pearl blocks with only the
/// coinbase transaction, so the aux inclusion proof must have no merkle branch.
pub const PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH: usize = 0;
pub const PEARL_MERGE_PUBLIC_STATEMENT_MAGIC: [u8; 4] = *b"PMP1";
pub const PEARL_MERGE_PUBLIC_STATEMENT_FIXED_SIZE: usize =
    4 + PEARL_INCOMPLETE_BLOCK_HEADER_SIZE + PEARL_PUBLIC_PROOF_PARAMS_SIZE + 32 + 2;
pub const PEARL_MERGE_PUBLIC_STATEMENT_MIN_SIZE: usize =
    PEARL_MERGE_PUBLIC_STATEMENT_FIXED_SIZE + PEARL_NOCKCHAIN_AUX_MIN_SIZE;
pub const PEARL_MERGE_PUBLIC_STATEMENT_MAX_SIZE: usize =
    PEARL_MERGE_PUBLIC_STATEMENT_FIXED_SIZE + PEARL_NOCKCHAIN_AUX_MAX_SIZE;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PearlCompatError {
    #[error("invalid params: {0}")]
    Params(#[from] ParamError),
    #[error("Pearl encoded header has wrong length: expected 76, got {0}")]
    BadHeaderLen(usize),
    #[error("Pearl encoded mining config has wrong length: expected 52, got {0}")]
    BadMiningConfigLen(usize),
    #[error("Pearl encoded periodic pattern has wrong length: expected 6, got {0}")]
    BadPatternLen(usize),
    #[error("Pearl encoded public proof params have wrong length: expected 164, got {0}")]
    BadPublicParamsLen(usize),
    #[error("Pearl MoE public data is too short: need at least {expected}, got {actual}")]
    MoeWireTooShort { expected: usize, actual: usize },
    #[error("Pearl MoE public data length mismatch: expected {expected}, got {actual}")]
    MoeWireLengthMismatch { expected: usize, actual: usize },
    #[error("Pearl MoE number of experts {0} exceeds the maximum (1024)")]
    MoeExpertsExceedMax(usize),
    #[error("Pearl MoE outer_indices length {0} exceeds the maximum (128)")]
    MoeOuterIndicesExceedMax(usize),
    #[error("Pearl MoE public data present but mining config is not GROUPED_GEMM")]
    MoePublicMissingConfig,
    #[error("Pearl MoE routing_offsets length {actual} must equal the expert count {expected}")]
    MoeExpertCountMismatch { expected: usize, actual: usize },
    #[error("Pearl MoE expert_idx {expert_idx} is out of range (e={e})")]
    MoeExpertIdxOutOfRange { expert_idx: u16, e: u16 },
    #[error(transparent)]
    MoeRouting(#[from] crate::pearl_moe_routing::RoutingError),
    #[error("Pearl MoE routing_data length {actual} must equal m*top_k = {expected}")]
    MoeRoutingDataLenMismatch { expected: u64, actual: usize },
    #[error("Pearl MoE routing commitment does not match the committed routing_root")]
    MoeRoutingRootMismatch,
    #[error("Pearl MoE routing token index {token} at slot {slot} is out of range (m={m})")]
    MoeRoutingTokenOutOfRange { slot: usize, token: u32, m: u32 },
    #[error("Pearl MoE routing_offsets are not a valid non-decreasing partition ending at m*top_k")]
    MoeOffsetsInconsistent,
    #[error("Pearl MoE top_k={top_k} must be < number of experts e={e}")]
    MoeTopKNotLessThanExperts { top_k: usize, e: usize },
    #[error("Pearl MoE expert {expert} span {span} exceeds the token count m={m} (a token routes to an expert at most once)")]
    MoeExpertSpanExceedsTokens { expert: usize, span: u32, m: u32 },
    #[error("Pearl MoE column dimension n={n} is not divisible by the number of experts e={e}")]
    MoeColumnDimIndivisible { n: u32, e: u16 },
    #[error("Pearl MoE local column {local} reaches outside expert {expert_idx}'s block (n_e={n_e}); would bleed into a neighbouring expert's weights")]
    MoeColumnOutsideExpert { local: u32, n_e: u32, expert_idx: u16 },
    #[error("Pearl MoE outer_indices length {actual} must equal the row-pattern size {expected}")]
    MoeOuterIndicesLenMismatch { expected: usize, actual: usize },
    #[error("Pearl MoE opened row position {pos} falls outside expert {expert_idx}'s routed tokens")]
    MoeOuterIndexOutsideExpert { expert_idx: u16, pos: u32 },
    #[error("Pearl MoE outer_indices do not match the expert's routed tokens (gather mismatch)")]
    MoeOuterIndicesMismatch,
    #[error("unsupported Pearl MMA type: {0}")]
    UnsupportedMmaType(u16),
    #[error("Pearl mining config common_dim does not match params.k")]
    CommonDimMismatch,
    #[error("Pearl mining config rank does not match params.noise_rank")]
    RankMismatch,
    #[error("Pearl mining config reserved trailer (bytes 4..32) must be all zero")]
    NonzeroReserved,
    #[error(
        "Pearl MoE (GROUPED_GEMM) mining config is not supported yet \
         (e={e}, top_k={top_k}); dense (e=0) only"
    )]
    UnsupportedMoeConfig { e: u16, top_k: u16 },
    #[error("Pearl mining config has top_k={0} with e==0 (a non-MoE job requires top_k==0)")]
    MoeTopKWithoutExperts(u16),
    #[error("Pearl periodic pattern has non-canonical trailing dimension")]
    NonCanonicalPattern,
    #[error("Pearl periodic pattern must not break a single stride across dimensions")]
    BrokenSingleStride,
    #[error("Pearl periodic pattern stride must be a positive multiple of prior period")]
    BadPatternStride,
    #[error("Pearl periodic pattern factor or length does not fit one byte")]
    PatternByteOverflow,
    #[error("Pearl periodic pattern period exceeds 2^24")]
    PatternPeriodTooLarge,
    #[error("Pearl periodic pattern period must divide the matrix dimension")]
    PatternPeriodDoesNotDivideDimension,
    #[error("Pearl periodic pattern is empty")]
    PatternEmpty,
    #[error("Pearl periodic pattern must be sorted, unique, and strictly increasing")]
    PatternNotStrictlyIncreasing,
    #[error("Pearl periodic pattern must start at zero")]
    PatternMustStartAtZero,
    #[error("Pearl periodic pattern is not representable as three Pearl dimensions")]
    PatternNotRepresentable,
    #[error("Pearl periodic pattern list would exceed caller limit")]
    PatternListTooLarge,
    #[error("Pearl public proof params have an invalid row or column pattern offset")]
    InvalidPatternOffset,
    #[error("Pearl public proof params place the row or column pattern outside the matrix")]
    PatternOutOfMatrix,
    #[error("Pearl public proof params violate the production parameter envelope")]
    PublicParamEnvelope,
    #[error("Pearl mining config is outside the recursive prover envelope")]
    UnsupportedRecursivePearlShape,
    #[error("Pearl recursive prover params are outside the recursive prover envelope: {0}")]
    UnsupportedRecursivePearlParams(&'static str),
    #[error("Pearl public proof commitments do not match the derived work commitments")]
    PublicCommitmentMismatch,
    #[error("Pearl public proof jackpot hash does not match the recomputed pattern ticket")]
    JackpotHashMismatch,
    #[error("Pearl jackpot hash does not satisfy Pearl nbits target")]
    PearlTargetNotMet,
    #[error("Pearl jackpot hash does not satisfy Nockchain target")]
    NockchainTargetNotMet,
    #[error("A has wrong length: expected m*k = {expected}, got {actual}")]
    InputAShape { expected: usize, actual: usize },
    #[error("B has wrong length: expected n*k = {expected}, got {actual}")]
    InputBShape { expected: usize, actual: usize },
    #[error("input entry out of range [-64, 64]: matrix={matrix}, index={index}, value={value}")]
    InputOutOfRange {
        matrix: &'static str,
        index: usize,
        value: i8,
    },
    #[error("Nockchain aux chain id must not be empty")]
    NockchainAuxChainIdEmpty,
    #[error("Nockchain aux chain id is too large: max 64 bytes, got {0}")]
    NockchainAuxChainIdTooLarge(usize),
    #[error("Nockchain aux extra domain data is too large: max 1024 bytes, got {0}")]
    NockchainAuxExtraTooLarge(usize),
    #[error("Nockchain aux commitment does not match the expected Pearl inclusion digest")]
    NockchainAuxCommitmentMismatch,
    #[error("Nockchain aux block commitment does not match the candidate block")]
    NockchainAuxBlockCommitmentMismatch,
    #[error("Nockchain aux bytes have wrong length: got {0}")]
    BadNockchainAuxLen(usize),
    #[error("Nockchain aux bytes have bad magic: {0:?}")]
    BadNockchainAuxMagic([u8; 4]),
    #[error("Nockchain aux bytes have trailing data: expected {expected}, got {actual}")]
    NockchainAuxTrailingData { expected: usize, actual: usize },
    #[error("Pearl merge public statement bytes have wrong length: got {0}")]
    BadMergePublicStatementLen(usize),
    #[error("Pearl merge public statement bytes have bad magic: {0:?}")]
    BadMergePublicStatementMagic([u8; 4]),
    #[error(
        "Pearl merge public statement bytes have trailing data: expected {expected}, got {actual}"
    )]
    MergePublicStatementTrailingData { expected: usize, actual: usize },
    #[error("Pearl aux inclusion coinbase transaction is empty")]
    PearlAuxCoinbaseTxEmpty,
    #[error("Pearl aux inclusion coinbase transaction is too large: max 100000 bytes, got {0}")]
    PearlAuxCoinbaseTxTooLarge(usize),
    #[error("Pearl aux inclusion merkle branch is too deep: max 0 siblings, got {0}")]
    PearlAuxMerkleBranchTooDeep(usize),
    #[error("Pearl aux inclusion coinbase transaction has malformed Bitcoin encoding")]
    PearlAuxMalformedCoinbaseTx,
    #[error("Pearl aux inclusion proof leaf is not a coinbase transaction")]
    PearlAuxNotCoinbase,
    #[error("Pearl aux commitment tag is not present in the txid-committed coinbase script")]
    PearlAuxCommitmentTagMissing,
    #[error("Pearl aux inclusion merkle branch does not match the Pearl header merkle root")]
    PearlAuxMerkleRootMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlIncompleteBlockHeader {
    pub version: u32,
    pub prev_block: [u8; 32],
    pub merkle_root: [u8; 32],
    pub timestamp: u32,
    pub nbits: u32,
}

impl PearlIncompleteBlockHeader {
    pub fn to_bytes(&self) -> [u8; PEARL_INCOMPLETE_BLOCK_HEADER_SIZE] {
        let mut out = [0u8; PEARL_INCOMPLETE_BLOCK_HEADER_SIZE];
        out[0..4].copy_from_slice(&self.version.to_le_bytes());
        for (dst, src) in out[4..36].iter_mut().zip(self.prev_block.iter().rev()) {
            *dst = *src;
        }
        for (dst, src) in out[36..68].iter_mut().zip(self.merkle_root.iter().rev()) {
            *dst = *src;
        }
        out[68..72].copy_from_slice(&self.timestamp.to_le_bytes());
        out[72..76].copy_from_slice(&self.nbits.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PearlCompatError> {
        if bytes.len() != PEARL_INCOMPLETE_BLOCK_HEADER_SIZE {
            return Err(PearlCompatError::BadHeaderLen(bytes.len()));
        }
        let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let mut prev_block: [u8; 32] = bytes[4..36].try_into().unwrap();
        prev_block.reverse();
        let mut merkle_root: [u8; 32] = bytes[36..68].try_into().unwrap();
        merkle_root.reverse();
        let timestamp = u32::from_le_bytes(bytes[68..72].try_into().unwrap());
        let nbits = u32::from_le_bytes(bytes[72..76].try_into().unwrap());
        Ok(Self {
            version,
            prev_block,
            merkle_root,
            timestamp,
            nbits,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlPeriodicPattern {
    pub shape: [(u32, u32); 3],
}

impl PearlPeriodicPattern {
    pub const NUM_DIMS: usize = 3;
    pub const MAX_PERIOD: u32 = 1 << 24;

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PearlCompatError> {
        if bytes.len() != 2 * Self::NUM_DIMS {
            return Err(PearlCompatError::BadPatternLen(bytes.len()));
        }
        let mut shape = [(1u32, 1u32); Self::NUM_DIMS];
        let mut min_stride = 1u32;
        let mut done = false;
        for (idx, chunk) in bytes.chunks_exact(2).enumerate() {
            let factor = 1 + u32::from(chunk[0]);
            let length = 1 + u32::from(chunk[1]);
            if length == 1 || done {
                if factor != 1 || length != 1 {
                    return Err(PearlCompatError::NonCanonicalPattern);
                }
                done = true;
            } else if factor <= 1 && min_stride != 1 {
                return Err(PearlCompatError::BrokenSingleStride);
            }
            let Some(period) = min_stride
                .checked_mul(factor)
                .and_then(|s| s.checked_mul(length))
            else {
                return Err(PearlCompatError::PatternPeriodTooLarge);
            };
            if period > Self::MAX_PERIOD {
                return Err(PearlCompatError::PatternPeriodTooLarge);
            }
            let stride = factor * min_stride;
            shape[idx] = (stride, length);
            min_stride = period;
        }
        Ok(Self { shape })
    }

    pub fn to_bytes(&self) -> Result<[u8; 2 * Self::NUM_DIMS], PearlCompatError> {
        let mut out = [0u8; 2 * Self::NUM_DIMS];
        let mut min_stride = 1u32;
        let mut done = false;
        for (idx, &(stride, length)) in self.shape.iter().enumerate() {
            if stride == 0 || length == 0 || stride % min_stride != 0 {
                return Err(PearlCompatError::BadPatternStride);
            }
            let factor = stride / min_stride;
            if length == 1 || done {
                if factor != 1 || length != 1 {
                    return Err(PearlCompatError::NonCanonicalPattern);
                }
                done = true;
            } else if factor <= 1 && min_stride != 1 {
                return Err(PearlCompatError::BrokenSingleStride);
            }
            if factor > 256 || length > 256 {
                return Err(PearlCompatError::PatternByteOverflow);
            }
            let Some(period) = stride.checked_mul(length) else {
                return Err(PearlCompatError::PatternPeriodTooLarge);
            };
            if period > Self::MAX_PERIOD {
                return Err(PearlCompatError::PatternPeriodTooLarge);
            }
            out[2 * idx] = (factor - 1) as u8;
            out[2 * idx + 1] = (length - 1) as u8;
            min_stride = period;
        }
        Ok(out)
    }

    pub fn from_list(indices: &[u32]) -> Result<Self, PearlCompatError> {
        if indices.is_empty() {
            return Err(PearlCompatError::PatternEmpty);
        }
        if !indices.windows(2).all(|w| w[0] < w[1]) {
            return Err(PearlCompatError::PatternNotStrictlyIncreasing);
        }
        if indices[0] != 0 {
            return Err(PearlCompatError::PatternMustStartAtZero);
        }

        let mut pattern = indices.to_vec();
        let mut shape = Vec::new();

        while pattern.len() > 1 {
            let mut found = false;
            for period in 1..pattern.len() {
                if pattern.len() % period != 0 {
                    continue;
                }
                let stride = pattern[period];
                let is_periodic =
                    (0..pattern.len() - period).all(|i| pattern[i] + stride == pattern[i + period]);
                if is_periodic {
                    shape.push((stride, (pattern.len() / period) as u32));
                    pattern.truncate(period);
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(PearlCompatError::PatternNotRepresentable);
            }
            if shape.len() > Self::NUM_DIMS {
                return Err(PearlCompatError::PatternNotRepresentable);
            }
        }

        shape.reverse();
        let period = match shape.last() {
            Some(&(stride, length)) => stride
                .checked_mul(length)
                .ok_or(PearlCompatError::PatternPeriodTooLarge)?,
            None => 1,
        };
        while shape.len() < Self::NUM_DIMS {
            shape.push((period, 1));
        }
        let shape: [(u32, u32); Self::NUM_DIMS] = shape
            .try_into()
            .map_err(|_| PearlCompatError::PatternNotRepresentable)?;
        let pattern = Self { shape };
        if !pattern.is_valid() {
            return Err(PearlCompatError::PatternNotRepresentable);
        }
        Ok(pattern)
    }

    pub fn to_list_bounded(&self, max_len: usize) -> Result<Vec<u32>, PearlCompatError> {
        let size = self.checked_size()?;
        if size > max_len {
            return Err(PearlCompatError::PatternListTooLarge);
        }
        let mut result = vec![0u32];
        for &(stride, length) in &self.shape {
            let next_len = result
                .len()
                .checked_mul(length as usize)
                .ok_or(PearlCompatError::PatternListTooLarge)?;
            if next_len > max_len {
                return Err(PearlCompatError::PatternListTooLarge);
            }
            let mut next = Vec::with_capacity(next_len);
            for i in 0..length {
                for &base in &result {
                    next.push(base + i * stride);
                }
            }
            result = next;
        }
        Ok(result)
    }

    pub fn to_list(&self) -> Result<Vec<u32>, PearlCompatError> {
        self.to_list_bounded(self.checked_size()?)
    }

    pub fn max(&self) -> Result<u32, PearlCompatError> {
        Ok(self.to_list()?.into_iter().max().unwrap_or(0))
    }

    pub fn offset_is_valid(&self, mut offset: u32) -> bool {
        for &(stride, length) in self.shape.iter().rev() {
            let Some(period) = stride.checked_mul(length) else {
                return false;
            };
            if period == 0 {
                return false;
            }
            offset %= period;
            if offset >= stride {
                return false;
            }
        }
        true
    }

    pub fn is_valid(&self) -> bool {
        self.to_bytes()
            .and_then(|bytes| Self::from_bytes(&bytes))
            .is_ok_and(|restored| restored == *self)
    }

    pub fn period(&self) -> Result<u32, PearlCompatError> {
        let (stride, length) = self.shape[Self::NUM_DIMS - 1];
        stride
            .checked_mul(length)
            .ok_or(PearlCompatError::PatternPeriodTooLarge)
    }

    pub fn size(&self) -> Result<u32, PearlCompatError> {
        let size = self.checked_size()?;
        u32::try_from(size).map_err(|_| PearlCompatError::PatternListTooLarge)
    }

    fn checked_size(&self) -> Result<usize, PearlCompatError> {
        self.shape.iter().try_fold(1usize, |acc, &(_, length)| {
            acc.checked_mul(length as usize)
                .ok_or(PearlCompatError::PatternListTooLarge)
        })
    }

    pub fn indices_with_offset_bounded(
        &self,
        offset: u32,
        max_len: usize,
    ) -> Result<Vec<u32>, PearlCompatError> {
        let mut indices = self.to_list_bounded(max_len)?;
        for index in &mut indices {
            *index = index
                .checked_add(offset)
                .ok_or(PearlCompatError::PatternPeriodTooLarge)?;
        }
        Ok(indices)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlMiningConfig {
    pub common_dim: u32,
    pub rank: u16,
    pub mma_type: u16,
    pub rows_pattern: PearlPeriodicPattern,
    pub cols_pattern: PearlPeriodicPattern,
    pub reserved: [u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
}

/// A parsed Pearl MoE (GROUPED_GEMM) mining config: `e` experts, each token
/// routed to `top_k` of them (Pearl `MoEConfig`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlMoeConfig {
    pub e: u16,
    pub top_k: u16,
}

/// Parse + structurally validate the 32-byte `MiningConfiguration` trailer per
/// Pearl's MoE-aware layout `e(2 LE) | top_k(2 LE) | zero-padding(28)` (Pearl
/// `zk-pow/src/api/proof_utils.rs::MiningConfiguration::from_bytes`).
///
/// Returns `None` for a dense job (`e == 0`, byte-identical to the pre-MoE
/// all-zero trailer) or `Some(cfg)` for GROUPED_GEMM. Mirrors Pearl's structural
/// checks (`trailer[4..] == 0`, `top_k == 0` when `e == 0`) so a dense trailer
/// round-trips unchanged. MoE-specific envelope validation (`top_k < e`,
/// `e ≤ MAX`, `n·e ≤ 2²⁴`, …) is layered on at use sites; and MoE *proving /
/// acceptance* remains fail-closed until the recursive circuit (Track B5) lands.
fn parse_mining_config_trailer(
    reserved: &[u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
) -> Result<Option<PearlMoeConfig>, PearlCompatError> {
    let e = u16::from_le_bytes([reserved[0], reserved[1]]);
    let top_k = u16::from_le_bytes([reserved[2], reserved[3]]);
    if reserved[4..].iter().any(|&b| b != 0) {
        return Err(PearlCompatError::NonzeroReserved);
    }
    if e == 0 {
        if top_k != 0 {
            return Err(PearlCompatError::MoeTopKWithoutExperts(top_k));
        }
        Ok(None)
    } else {
        Ok(Some(PearlMoeConfig { e, top_k }))
    }
}

impl PearlMiningConfig {
    /// The parsed MoE config, or `None` for a dense job. Assumes a
    /// validly-encoded trailer (enforced by [`from_bytes`](Self::from_bytes) /
    /// [`to_bytes`](Self::to_bytes)).
    pub fn moe(&self) -> Option<PearlMoeConfig> {
        let e = u16::from_le_bytes([self.reserved[0], self.reserved[1]]);
        if e == 0 {
            None
        } else {
            Some(PearlMoeConfig {
                e,
                top_k: u16::from_le_bytes([self.reserved[2], self.reserved[3]]),
            })
        }
    }

    /// Encode an MoE config into the 32-byte trailer (`e | top_k | zero(28)`).
    pub fn moe_trailer(e: u16, top_k: u16) -> [u8; PEARL_MINING_CONFIG_RESERVED_SIZE] {
        let mut t = [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE];
        t[0..2].copy_from_slice(&e.to_le_bytes());
        t[2..4].copy_from_slice(&top_k.to_le_bytes());
        t
    }

    pub fn to_bytes(&self) -> Result<[u8; PEARL_MINING_CONFIG_SIZE], PearlCompatError> {
        if self.mma_type != PEARL_MMA_INT7XINT7_TO_INT32 {
            return Err(PearlCompatError::UnsupportedMmaType(self.mma_type));
        }
        parse_mining_config_trailer(&self.reserved)?;
        let mut out = [0u8; PEARL_MINING_CONFIG_SIZE];
        out[0..4].copy_from_slice(&self.common_dim.to_le_bytes());
        out[4..6].copy_from_slice(&self.rank.to_le_bytes());
        out[6..8].copy_from_slice(&self.mma_type.to_le_bytes());
        out[8..14].copy_from_slice(&self.rows_pattern.to_bytes()?);
        out[14..20].copy_from_slice(&self.cols_pattern.to_bytes()?);
        out[20..52].copy_from_slice(&self.reserved);
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PearlCompatError> {
        if bytes.len() != PEARL_MINING_CONFIG_SIZE {
            return Err(PearlCompatError::BadMiningConfigLen(bytes.len()));
        }
        let common_dim = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let rank = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        let mma_type = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
        if mma_type != PEARL_MMA_INT7XINT7_TO_INT32 {
            return Err(PearlCompatError::UnsupportedMmaType(mma_type));
        }
        let rows_pattern = PearlPeriodicPattern::from_bytes(&bytes[8..14])?;
        let cols_pattern = PearlPeriodicPattern::from_bytes(&bytes[14..20])?;
        let reserved: [u8; PEARL_MINING_CONFIG_RESERVED_SIZE] = bytes[20..52].try_into().unwrap();
        parse_mining_config_trailer(&reserved)?;
        Ok(Self {
            common_dim,
            rank,
            mma_type,
            rows_pattern,
            cols_pattern,
            reserved,
        })
    }

    pub fn dot_product_length(&self) -> Result<usize, PearlCompatError> {
        if self.rank == 0 {
            return Err(PearlCompatError::PublicParamEnvelope);
        }
        let common_dim = self.common_dim as usize;
        let rank = self.rank as usize;
        Ok(common_dim - common_dim % rank)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlPublicProofParams {
    pub block_header: PearlIncompleteBlockHeader,
    pub mining_config: PearlMiningConfig,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub hash_jackpot: [u8; 32],
    pub m: u32,
    pub n: u32,
    pub t_rows: u32,
    pub t_cols: u32,
}

/// Pearl `PublicProofParams::MAX_NUM_EXPERTS`.
pub const PEARL_MOE_MAX_NUM_EXPERTS: usize = 1024;
/// Pearl `PublicProofParams::MAX_OUTER_INDICES`.
pub const PEARL_MOE_MAX_OUTER_INDICES: usize = 128;
/// Bytes per routing offset (`u32`).
pub const PEARL_MOE_ROUTING_OFFSET_BYTES: usize = 4;
/// Fixed part of an MoE `public_data`: 164-byte core + `expert_idx(2)` +
/// `hash_routing(32)` + `outer_count(1)`. Variable: `routing_offsets(4·e)` and
/// `outer_indices(4·oc)`. (Pearl `MIN_MOE_WIRE_SIZE = 199`.)
pub const PEARL_MOE_MIN_WIRE_SIZE: usize = PEARL_PUBLIC_PROOF_PARAMS_SIZE + 2 + 32 + 1;
/// Largest MoE `public_data` (Pearl `MAX_WIRE_SIZE = 4807`).
pub const PEARL_MOE_MAX_WIRE_SIZE: usize = PEARL_MOE_MIN_WIRE_SIZE
    + PEARL_MOE_MAX_OUTER_INDICES * 4
    + PEARL_MOE_MAX_NUM_EXPERTS * PEARL_MOE_ROUTING_OFFSET_BYTES;

/// The MoE-specific public parameters carried in the `public_data` tail (Pearl
/// `MoEParams`). `e` and `top_k` live in the mining-config trailer, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMoeParams {
    /// The opened expert.
    pub expert_idx: u16,
    /// Per-expert exclusive-end cumulative token counts; `len == e`, last entry
    /// `== m·top_k`.
    pub routing_offsets: Vec<u32>,
    /// `moe.hash_routing` (the routing Merkle root, `= routing_root`).
    pub hash_routing: [u8; 32],
    /// Opened tile rows decoded to global token positions in `A`.
    pub outer_indices: Vec<u32>,
}

impl PearlPublicProofParams {
    pub fn to_public_data(&self) -> Result<[u8; PEARL_PUBLIC_PROOF_PARAMS_SIZE], PearlCompatError> {
        let mut out = [0u8; PEARL_PUBLIC_PROOF_PARAMS_SIZE];
        out[0..52].copy_from_slice(&self.mining_config.to_bytes()?);
        out[52..84].copy_from_slice(&self.hash_a);
        out[84..116].copy_from_slice(&self.hash_b);
        out[116..148].copy_from_slice(&self.hash_jackpot);
        out[148..152].copy_from_slice(&self.m.to_le_bytes());
        out[152..156].copy_from_slice(&self.n.to_le_bytes());
        out[156..160].copy_from_slice(&self.t_rows.to_le_bytes());
        out[160..164].copy_from_slice(&self.t_cols.to_le_bytes());
        Ok(out)
    }

    /// Serialize this (MoE) statement to Pearl V2 wire `public_data` including the
    /// MoE tail (Pearl `PublicProofParams::to_wire_bytes`, MoE arm):
    ///
    /// ```text
    /// core(164) ‖ expert_idx(2) ‖ routing_offsets[e]·(4) ‖ hash_routing(32)
    ///           ‖ outer_count(1) ‖ outer_indices[oc]·(4)
    /// ```
    ///
    /// `self.mining_config` must be GROUPED_GEMM with `e == routing_offsets.len()`.
    pub fn to_wire_bytes_moe(&self, moe: &PearlMoeParams) -> Result<Vec<u8>, PearlCompatError> {
        let cfg = self
            .mining_config
            .moe()
            .ok_or(PearlCompatError::MoePublicMissingConfig)?;
        let e = cfg.e as usize;
        if moe.routing_offsets.len() != e {
            return Err(PearlCompatError::MoeExpertCountMismatch {
                expected: e,
                actual: moe.routing_offsets.len(),
            });
        }
        if e > PEARL_MOE_MAX_NUM_EXPERTS {
            return Err(PearlCompatError::MoeExpertsExceedMax(e));
        }
        if moe.outer_indices.len() > PEARL_MOE_MAX_OUTER_INDICES {
            return Err(PearlCompatError::MoeOuterIndicesExceedMax(
                moe.outer_indices.len(),
            ));
        }
        if moe.expert_idx >= cfg.e {
            return Err(PearlCompatError::MoeExpertIdxOutOfRange {
                expert_idx: moe.expert_idx,
                e: cfg.e,
            });
        }
        let mut out = Vec::with_capacity(
            PEARL_MOE_MIN_WIRE_SIZE + moe.routing_offsets.len() * 4 + moe.outer_indices.len() * 4,
        );
        out.extend_from_slice(&self.to_public_data()?);
        out.extend_from_slice(&moe.expert_idx.to_le_bytes());
        for off in &moe.routing_offsets {
            out.extend_from_slice(&off.to_le_bytes());
        }
        out.extend_from_slice(&moe.hash_routing);
        out.push(moe.outer_indices.len() as u8);
        for idx in &moe.outer_indices {
            out.extend_from_slice(&idx.to_le_bytes());
        }
        Ok(out)
    }

    /// Decode a Pearl V2 MoE `public_data` (core + MoE tail) into the core
    /// statement and its [`PearlMoeParams`]. Mirrors Pearl
    /// `PublicProofParams::from_wire_bytes` (MoE arm): the expert count `e` comes
    /// from the mining-config trailer and fixes the routing-offsets length; the
    /// total length is checked exactly.
    ///
    /// This is the byte-level MoE decoder; it does **not** by itself accept an MoE
    /// proof — the recursive certificate path stays fail-closed until Track B5.
    pub fn from_wire_bytes_moe(
        block_header: PearlIncompleteBlockHeader,
        bytes: &[u8],
    ) -> Result<(Self, PearlMoeParams), PearlCompatError> {
        if bytes.len() < PEARL_MOE_MIN_WIRE_SIZE {
            return Err(PearlCompatError::MoeWireTooShort {
                expected: PEARL_MOE_MIN_WIRE_SIZE,
                actual: bytes.len(),
            });
        }
        // Parse the 164-byte core directly (from_public_data is fail-closed on MoE).
        let mining_config = PearlMiningConfig::from_bytes(&bytes[0..PEARL_MINING_CONFIG_SIZE])?;
        let cfg = mining_config
            .moe()
            .ok_or(PearlCompatError::MoePublicMissingConfig)?;
        let e = cfg.e as usize;
        if e > PEARL_MOE_MAX_NUM_EXPERTS {
            return Err(PearlCompatError::MoeExpertsExceedMax(e));
        }
        let hash_a = bytes[52..84].try_into().unwrap();
        let hash_b = bytes[84..116].try_into().unwrap();
        let hash_jackpot = bytes[116..148].try_into().unwrap();
        let m = u32::from_le_bytes(bytes[148..152].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[152..156].try_into().unwrap());
        let t_rows = u32::from_le_bytes(bytes[156..160].try_into().unwrap());
        let t_cols = u32::from_le_bytes(bytes[160..164].try_into().unwrap());

        // MoE tail. Need at least the routing offsets before the fixed remainder.
        let min_with_offsets = PEARL_MOE_MIN_WIRE_SIZE + e * PEARL_MOE_ROUTING_OFFSET_BYTES;
        if bytes.len() < min_with_offsets {
            return Err(PearlCompatError::MoeWireTooShort {
                expected: min_with_offsets,
                actual: bytes.len(),
            });
        }
        let tail = &bytes[PEARL_PUBLIC_PROOF_PARAMS_SIZE..];
        let expert_idx = u16::from_le_bytes(tail[0..2].try_into().unwrap());
        if expert_idx >= cfg.e {
            return Err(PearlCompatError::MoeExpertIdxOutOfRange {
                expert_idx,
                e: cfg.e,
            });
        }
        let mut cursor = 2usize;
        let mut routing_offsets = Vec::with_capacity(e);
        for _ in 0..e {
            routing_offsets.push(u32::from_le_bytes(
                tail[cursor..cursor + 4].try_into().unwrap(),
            ));
            cursor += 4;
        }
        let hash_routing: [u8; 32] = tail[cursor..cursor + 32].try_into().unwrap();
        cursor += 32;
        let num_outer = tail[cursor] as usize;
        cursor += 1;
        if num_outer > PEARL_MOE_MAX_OUTER_INDICES {
            return Err(PearlCompatError::MoeOuterIndicesExceedMax(num_outer));
        }
        let expected_len = PEARL_MOE_MIN_WIRE_SIZE
            + num_outer * 4
            + e * PEARL_MOE_ROUTING_OFFSET_BYTES;
        if bytes.len() != expected_len {
            return Err(PearlCompatError::MoeWireLengthMismatch {
                expected: expected_len,
                actual: bytes.len(),
            });
        }
        let mut outer_indices = Vec::with_capacity(num_outer);
        for _ in 0..num_outer {
            outer_indices.push(u32::from_le_bytes(
                tail[cursor..cursor + 4].try_into().unwrap(),
            ));
            cursor += 4;
        }

        // Core offset validity (same as the dense decoder) + t_rows/t_cols bounds
        // (Pearl `from_wire_bytes`).
        if !mining_config.rows_pattern.offset_is_valid(t_rows)
            || !mining_config.cols_pattern.offset_is_valid(t_cols)
        {
            return Err(PearlCompatError::InvalidPatternOffset);
        }
        if t_rows >= m || t_cols >= n {
            return Err(PearlCompatError::PatternOutOfMatrix);
        }

        let core = Self {
            block_header,
            mining_config,
            hash_a,
            hash_b,
            hash_jackpot,
            m,
            n,
            t_rows,
            t_cols,
        };
        Ok((
            core,
            PearlMoeParams {
                expert_idx,
                routing_offsets,
                hash_routing,
                outer_indices,
            },
        ))
    }

    pub fn from_public_data(
        block_header: PearlIncompleteBlockHeader,
        bytes: &[u8],
    ) -> Result<Self, PearlCompatError> {
        // Fail closed on MoE (GROUPED_GEMM) public data regardless of length: the
        // MoE variant carries a variable-length tail after the 164-byte dense core
        // (Pearl `PublicProofParams::to_wire_bytes`), so a length check alone would
        // report `BadPublicParamsLen` for what is really an unsupported MoE proof.
        // The mode discriminant `e` lives at bytes[20..22] (mining-config trailer).
        // MoE (GROUPED_GEMM) public data carries a variable-length tail after the
        // 164-byte dense core (Track B3b). Until that parser lands, fail closed on
        // the MoE discriminant `e` (trailer bytes 20..22) with a precise error
        // rather than a misleading length error.
        if bytes.len() >= PEARL_MINING_CONFIG_SIZE {
            let e = u16::from_le_bytes([bytes[20], bytes[21]]);
            if e != 0 {
                let top_k = u16::from_le_bytes([bytes[22], bytes[23]]);
                // Surface a malformed-trailer error first if the trailer is bad.
                PearlMiningConfig::from_bytes(&bytes[0..PEARL_MINING_CONFIG_SIZE])?;
                return Err(PearlCompatError::UnsupportedMoeConfig { e, top_k });
            }
        }
        if bytes.len() != PEARL_PUBLIC_PROOF_PARAMS_SIZE {
            return Err(PearlCompatError::BadPublicParamsLen(bytes.len()));
        }
        let mining_config = PearlMiningConfig::from_bytes(&bytes[0..52])?;
        let hash_a = bytes[52..84].try_into().unwrap();
        let hash_b = bytes[84..116].try_into().unwrap();
        let hash_jackpot = bytes[116..148].try_into().unwrap();
        let m = u32::from_le_bytes(bytes[148..152].try_into().unwrap());
        let n = u32::from_le_bytes(bytes[152..156].try_into().unwrap());
        let t_rows = u32::from_le_bytes(bytes[156..160].try_into().unwrap());
        let t_cols = u32::from_le_bytes(bytes[160..164].try_into().unwrap());

        if !mining_config.rows_pattern.offset_is_valid(t_rows)
            || !mining_config.cols_pattern.offset_is_valid(t_cols)
        {
            return Err(PearlCompatError::InvalidPatternOffset);
        }

        Ok(Self {
            block_header,
            mining_config,
            hash_a,
            hash_b,
            hash_jackpot,
            m,
            n,
            t_rows,
            t_cols,
        })
    }

    pub fn h(&self) -> Result<u32, PearlCompatError> {
        self.mining_config.rows_pattern.size()
    }

    pub fn w(&self) -> Result<u32, PearlCompatError> {
        self.mining_config.cols_pattern.size()
    }

    pub fn a_rows_indices_bounded(&self, max_len: usize) -> Result<Vec<u32>, PearlCompatError> {
        self.mining_config
            .rows_pattern
            .indices_with_offset_bounded(self.t_rows, max_len)
    }

    pub fn b_cols_indices_bounded(&self, max_len: usize) -> Result<Vec<u32>, PearlCompatError> {
        self.mining_config
            .cols_pattern
            .indices_with_offset_bounded(self.t_cols, max_len)
    }

    pub fn row_thread_partitions_bounded(
        &self,
        max_indices_per_partition: usize,
        max_partitions: usize,
    ) -> Result<Vec<Vec<u32>>, PearlCompatError> {
        pattern_partitions_bounded(
            &self.mining_config.rows_pattern, self.m, max_indices_per_partition, max_partitions,
        )
    }

    pub fn col_thread_partitions_bounded(
        &self,
        max_indices_per_partition: usize,
        max_partitions: usize,
    ) -> Result<Vec<Vec<u32>>, PearlCompatError> {
        pattern_partitions_bounded(
            &self.mining_config.cols_pattern, self.n, max_indices_per_partition, max_partitions,
        )
    }

    pub fn sanity_check(&self) -> Result<(), PearlCompatError> {
        // Fail-closed on MoE (GROUPED_GEMM): the shape-aware envelope below is the
        // dense envelope. MoE envelope validation + the grouped-tile compute land
        // in Track B3d; until then no MoE statement is computed or accepted.
        if let Some(m) = self.mining_config.moe() {
            return Err(PearlCompatError::UnsupportedMoeConfig {
                e: m.e,
                top_k: m.top_k,
            });
        }
        let k = self.mining_config.common_dim;
        let r = u32::from(self.mining_config.rank);
        let h = self.h()?;
        let w = self.w()?;
        let dot_product_len = self.mining_config.dot_product_length()? as u64;
        let worker_input_size = u64::from(h.saturating_add(w)).saturating_mul(dot_product_len);

        if !(r.is_power_of_two() && (32..=1024).contains(&r))
            || !r.is_multiple_of(PEARL_TILE_D)
            || k > (1 << 16)
            || !k.is_multiple_of(64)
            || k > 4 * r * r
            || k < 16 * r
            || k < 1024
            || !h.is_multiple_of(PEARL_TILE_H)
            || !w.is_multiple_of(PEARL_TILE_H)
            || u64::from(h) * u64::from(w) < 32
            || dot_product_len % u64::from(PEARL_DWORD_SIZE) != 0
            || self.m > PearlPeriodicPattern::MAX_PERIOD
            || self.n > PearlPeriodicPattern::MAX_PERIOD
            || worker_input_size > PEARL_WORKER_INPUT_MAX
        {
            return Err(PearlCompatError::PublicParamEnvelope);
        }

        let rmax = self.mining_config.rows_pattern.max()?;
        let cmax = self.mining_config.cols_pattern.max()?;
        let Some(row_max) = self.t_rows.checked_add(rmax) else {
            return Err(PearlCompatError::PatternOutOfMatrix);
        };
        let Some(col_max) = self.t_cols.checked_add(cmax) else {
            return Err(PearlCompatError::PatternOutOfMatrix);
        };
        if row_max >= self.m || col_max >= self.n {
            return Err(PearlCompatError::PatternOutOfMatrix);
        }
        Ok(())
    }

    pub fn difficulty_adjustment_factor(&self) -> Result<u128, PearlCompatError> {
        let h = u128::from(self.h()?);
        let w = u128::from(self.w()?);
        let dot = self.mining_config.dot_product_length()? as u128;
        h.checked_mul(w)
            .and_then(|tile| tile.checked_mul(dot))
            .ok_or(PearlCompatError::PublicParamEnvelope)
    }

    pub fn pearl_adjusted_target(&self) -> Result<[u8; 32], PearlCompatError> {
        let base = pearl_nbits_to_target_le(self.block_header.nbits);
        Ok(u256_le_mul_u128_saturating(
            &base,
            self.difficulty_adjustment_factor()?,
        ))
    }

    pub fn nockchain_adjusted_target(
        &self,
        nockchain_target: &[u8; 32],
    ) -> Result<[u8; 32], PearlCompatError> {
        Ok(u256_le_mul_u128_saturating(
            nockchain_target,
            self.difficulty_adjustment_factor()?,
        ))
    }

    pub fn check_pearl_jackpot_difficulty(&self) -> Result<(), PearlCompatError> {
        let target = self.pearl_adjusted_target()?;
        if hash_le_target(&self.hash_jackpot, &target) {
            Ok(())
        } else {
            Err(PearlCompatError::PearlTargetNotMet)
        }
    }

    pub fn check_nockchain_jackpot_target(
        &self,
        nockchain_target: &[u8; 32],
    ) -> Result<(), PearlCompatError> {
        let target = self.nockchain_adjusted_target(nockchain_target)?;
        if hash_le_target(&self.hash_jackpot, &target) {
            Ok(())
        } else {
            Err(PearlCompatError::NockchainTargetNotMet)
        }
    }
}

pub fn pearl_nbits_to_target_le(nbits: u32) -> [u8; 32] {
    let exponent = (nbits >> 24) as usize;
    let mantissa = nbits & 0x00ff_ffff;
    if exponent == 0 || mantissa == 0 || (mantissa & 0x0080_0000) != 0 {
        return [0u8; 32];
    }

    let mut out = [0u8; 32];
    if exponent <= 3 {
        let shifted = mantissa >> (8 * (3 - exponent));
        out[0..4].copy_from_slice(&shifted.to_le_bytes());
    } else {
        let offset = exponent - 3;
        let bytes = mantissa.to_le_bytes();
        for i in 0..3 {
            if offset + i < out.len() {
                out[offset + i] = bytes[i];
            }
        }
    }
    out
}

pub fn pearl_adjust_target_for_config(
    nbits: u32,
    config: &PearlMiningConfig,
) -> Result<[u8; 32], PearlCompatError> {
    let h = u128::from(config.rows_pattern.size()?);
    let w = u128::from(config.cols_pattern.size()?);
    let dot = config.dot_product_length()? as u128;
    let factor = h
        .checked_mul(w)
        .and_then(|tile| tile.checked_mul(dot))
        .ok_or(PearlCompatError::PublicParamEnvelope)?;
    Ok(u256_le_mul_u128_saturating(
        &pearl_nbits_to_target_le(nbits),
        factor,
    ))
}

/// Validate the Pearl mining config that Nockchain's Pearl merge recursive
/// certificate producer can prove.
///
/// Production Pearl-compatible Nockchain mining proves one explicit Pearl
/// ticket selected by the Pearl public row/column patterns and `t_rows` /
/// `t_cols`. The recursive bridge binds those concrete shifted indices as a
/// verifier-derived strip schedule; it must not rewrite them to a cheaper
/// square-contiguous native tile.
pub fn validate_pearl_merge_config_for_recursive_prover(
    config: &PearlMiningConfig,
    params: &MatmulParams,
    max_pattern_len: usize,
) -> Result<(), PearlCompatError> {
    // Block-acceptance fail-closed: the recursive certificate does not yet bind
    // the MoE routing commitment / grouped matmul (Track B5). Refuse MoE mining
    // configs so no MoE proof can be accepted as a Nockchain block until the
    // circuit lands, even though the config now parses (B3a).
    if config.moe().is_some() {
        return Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "MoE (GROUPED_GEMM) recursive proving is not implemented",
        ));
    }
    if params.difficulty_bits != 0 {
        return Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "difficulty_bits must be 0; Nockchain target is verifier-supplied",
        ));
    }
    if params.spot_checks != 1 {
        return Err(PearlCompatError::UnsupportedRecursivePearlParams(
            "spot_checks must be 1; Pearl-compatible mode proves one explicit ticket",
        ));
    }
    validate_recursive_params_for_pearl_schedule(params)?;
    if (params.k / params.noise_rank) as usize > STRIPE_MAX {
        return Err(PearlCompatError::PublicParamEnvelope);
    }
    config.to_bytes()?;
    validate_config_matches_params(config, params)?;

    config.rows_pattern.to_list_bounded(max_pattern_len)?;
    config.cols_pattern.to_list_bounded(max_pattern_len)?;

    PearlPublicProofParams {
        block_header: PearlIncompleteBlockHeader {
            version: 0,
            prev_block: [0u8; 32],
            merkle_root: [0u8; 32],
            timestamp: 0,
            nbits: 0,
        },
        mining_config: *config,
        hash_a: [0u8; 32],
        hash_b: [0u8; 32],
        hash_jackpot: [0u8; 32],
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    }
    .sanity_check()?;
    Ok(())
}

fn validate_recursive_params_for_pearl_schedule(
    params: &MatmulParams,
) -> Result<(), PearlCompatError> {
    if params.m == 0 || params.n == 0 {
        return Err(PearlCompatError::PublicParamEnvelope);
    }
    if params.k == 0 || params.k > crate::params::PEARL_K_MAX {
        return Err(PearlCompatError::PublicParamEnvelope);
    }
    if params.noise_rank < 2
        || params.noise_rank > params.k
        || !params.noise_rank.is_power_of_two()
        || params.k % params.noise_rank != 0
    {
        return Err(PearlCompatError::PublicParamEnvelope);
    }
    Ok(())
}

fn u256_le_mul_u128_saturating(value: &[u8; 32], factor: u128) -> [u8; 32] {
    if factor == 0 || value.iter().all(|&b| b == 0) {
        return [0u8; 32];
    }

    let mut value_limbs = [0u32; 8];
    for i in 0..8 {
        value_limbs[i] = u32::from_le_bytes([
            value[i * 4],
            value[i * 4 + 1],
            value[i * 4 + 2],
            value[i * 4 + 3],
        ]);
    }
    let mut factor_limbs = [0u32; 4];
    let factor_bytes = factor.to_le_bytes();
    for i in 0..4 {
        factor_limbs[i] = u32::from_le_bytes([
            factor_bytes[i * 4],
            factor_bytes[i * 4 + 1],
            factor_bytes[i * 4 + 2],
            factor_bytes[i * 4 + 3],
        ]);
    }

    let mut acc = [0u128; 12];
    for i in 0..8 {
        for j in 0..4 {
            acc[i + j] += u128::from(value_limbs[i]) * u128::from(factor_limbs[j]);
        }
    }

    let mut out_limbs = [0u32; 8];
    let mut carry = 0u128;
    for i in 0..12 {
        let total = acc[i] + carry;
        if i < 8 {
            out_limbs[i] = (total & 0xffff_ffff) as u32;
            carry = total >> 32;
        } else if total != 0 {
            return [0xffu8; 32];
        }
    }
    if carry != 0 {
        return [0xffu8; 32];
    }

    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&out_limbs[i].to_le_bytes());
    }
    out
}

pub fn pattern_partitions_bounded(
    pattern: &PearlPeriodicPattern,
    total_dimension: u32,
    max_indices_per_partition: usize,
    max_partitions: usize,
) -> Result<Vec<Vec<u32>>, PearlCompatError> {
    let period = pattern.period()?;
    if period == 0 || total_dimension % period != 0 {
        return Err(PearlCompatError::PatternPeriodDoesNotDivideDimension);
    }
    let base_indices = pattern.to_list_bounded(max_indices_per_partition)?;
    let mut partitions = Vec::new();
    for offset in 0..total_dimension {
        if pattern.offset_is_valid(offset) {
            if partitions.len() == max_partitions {
                return Err(PearlCompatError::PatternListTooLarge);
            }
            let mut partition = Vec::with_capacity(base_indices.len());
            for &base in &base_indices {
                partition.push(
                    offset
                        .checked_add(base)
                        .ok_or(PearlCompatError::PatternPeriodTooLarge)?,
                );
            }
            partitions.push(partition);
        }
    }
    Ok(partitions)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlPatternTicket {
    pub a_rows: Vec<u32>,
    pub b_cols: Vec<u32>,
    pub tile_state: TileState,
    pub jackpot_hash: [u8; 32],
}

pub fn compute_pearl_pattern_ticket(
    public_params: &PearlPublicProofParams,
    a_row_major: &[i8],
    b_col_major: &[i8],
    commitments: &PearlWorkCommitments,
    max_pattern_len: usize,
) -> Result<PearlPatternTicket, PearlCompatError> {
    public_params.sanity_check()?;
    if public_params.hash_a != commitments.h_a || public_params.hash_b != commitments.h_b {
        return Err(PearlCompatError::PublicCommitmentMismatch);
    }
    validate_public_matrix_inputs(a_row_major, b_col_major, public_params)?;

    let a_rows = public_params.a_rows_indices_bounded(max_pattern_len)?;
    let b_cols = public_params.b_cols_indices_bounded(max_pattern_len)?;
    let k = public_params.mining_config.common_dim as usize;
    let r = public_params.mining_config.rank as usize;
    let dot_product_len = public_params.mining_config.dot_product_length()?;

    let mut a_prime_rows = Vec::with_capacity(a_rows.len() * k);
    let mut e_row = vec![0i8; k];
    for &row in &a_rows {
        pearl_e_row_into(
            &commitments.s_a, row, public_params.mining_config.common_dim, r, &mut e_row,
        );
        let off = row as usize * k;
        for l in 0..k {
            a_prime_rows.push((a_row_major[off + l] as i16 + e_row[l] as i16) as i8);
        }
    }

    let mut b_prime_cols = Vec::with_capacity(b_cols.len() * k);
    let mut f_col = vec![0i8; k];
    for &col in &b_cols {
        pearl_f_col_into(
            &commitments.s_b, col, public_params.mining_config.common_dim, r, &mut f_col,
        );
        let off = col as usize * k;
        for l in 0..k {
            b_prime_cols.push((b_col_major[off + l] as i16 + f_col[l] as i16) as i8);
        }
    }

    let h = a_rows.len();
    let w = b_cols.len();
    let tile_state = compute_pattern_tile_trace_from_slices(
        &a_prime_rows, &b_prime_cols, h, w, k, r, dot_product_len,
    )
    .state;

    let jackpot_hash = pearl_jackpot_hash(&tile_state, &commitments.s_a);
    Ok(PearlPatternTicket {
        a_rows,
        b_cols,
        tile_state,
        jackpot_hash,
    })
}

/// Off-circuit MoE grouped-tile reference (Track B3d).
///
/// Builds `A' = A + E` over the opened global token rows (`outer_indices`, from
/// [`crate::pearl_moe_routing::RoutingData::outer_indices`]) and `B' = B + F` over
/// the opened global expert columns (`b_cols_global = expert_idx·n + local`),
/// applies the noise seeds (`s_a` from the MoE routing-commitment splice —
/// [`crate::fiat_shamir::canonical_noise_seeds_moe`]; `s_b` unchanged), then
/// computes the tile state and jackpot. This composition is byte-identical to the
/// dense per-tile compute given the same opened indices and seeds (that
/// equivalence is the reference test); the only MoE-specific inputs are *which*
/// global rows/columns are opened and *which* `s_a` is used.
///
/// This is the reference the recursive circuit (Track B5) must reproduce. It does
/// **not** by itself accept an MoE proof — acceptance stays fail-closed until B5.
///
/// `a_row_major` is the `m × k` token matrix (row-major); `b_col_major` is the
/// full `k × (n·e)` weight matrix (column-major, column `c` at `c·k`).
#[allow(clippy::too_many_arguments)]
pub fn compute_moe_tile(
    a_row_major: &[i8],
    b_col_major: &[i8],
    outer_indices: &[u32],
    b_cols_global: &[u32],
    s_a: &[u8; 32],
    s_b: &[u8; 32],
    k: usize,
    r: usize,
    dot_product_len: usize,
) -> (TileState, [u8; 32]) {
    let mut a_prime = Vec::with_capacity(outer_indices.len() * k);
    let mut e_row = vec![0i8; k];
    for &row in outer_indices {
        pearl_e_row_into(s_a, row, k as u32, r, &mut e_row);
        let off = row as usize * k;
        for l in 0..k {
            a_prime.push((a_row_major[off + l] as i16 + e_row[l] as i16) as i8);
        }
    }
    let mut b_prime = Vec::with_capacity(b_cols_global.len() * k);
    let mut f_col = vec![0i8; k];
    for &col in b_cols_global {
        pearl_f_col_into(s_b, col, k as u32, r, &mut f_col);
        let off = col as usize * k;
        for l in 0..k {
            b_prime.push((b_col_major[off + l] as i16 + f_col[l] as i16) as i8);
        }
    }
    let tile_state = compute_pattern_tile_trace_from_slices(
        &a_prime,
        &b_prime,
        outer_indices.len(),
        b_cols_global.len(),
        k,
        r,
        dot_product_len,
    )
    .state;
    let jackpot = pearl_jackpot_hash(&tile_state, s_a);
    (tile_state, jackpot)
}

/// A fully-assembled off-circuit MoE work ticket (Track B end-to-end, Rust side):
/// routing (B1) → routing-commitment splice + `s_A` (B2) → `outer_indices` gather
/// (B3d) → grouped tile + jackpot (B3d). This is the prover-side reference the
/// recursive certificate binds; MoE acceptance stays fail-closed until the
/// in-circuit `outer_indices`↔routing CTL (the sole remaining soundness change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMoeTicket {
    pub s_a: [u8; 32],
    pub s_b: [u8; 32],
    pub commitment: crate::fiat_shamir::MoeRoutingCommitment,
    pub outer_indices: Vec<u32>,
    pub b_cols_global: Vec<u32>,
    pub tile_state: TileState,
    pub jackpot_hash: [u8; 32],
}

/// Assemble a MoE work ticket end-to-end (Rust). `kappa` is the job key,
/// `h_a`/`h_b` the keyed matrix commitments of the token matrix `A` and weight
/// matrix `B`; `routing` is the canonical routing; `inner_a_rows` selects the
/// expert's opened token rows (local positions), `local_b_cols` the expert-local
/// columns (offset by `expert_idx·n_e` into the stacked weight matrix).
#[allow(clippy::too_many_arguments)]
pub fn compute_pearl_moe_ticket(
    kappa: &[u8; 32],
    h_a: &[u8; 32],
    h_b: &[u8; 32],
    a_row_major: &[i8],
    b_col_major: &[i8],
    routing: &crate::pearl_moe_routing::RoutingData,
    expert_idx: usize,
    inner_a_rows: &[u32],
    local_b_cols: &[u32],
    n_e: usize,
    k: usize,
    r: usize,
    dot_product_len: usize,
) -> Result<PearlMoeTicket, PearlCompatError> {
    let (s_a, s_b, commitment) = crate::fiat_shamir::canonical_noise_seeds_moe(
        kappa,
        h_a,
        h_b,
        &routing.routing_data_le_bytes(),
        &routing.routing_offsets_le_bytes(),
    );
    let outer_indices = routing.outer_indices(expert_idx, inner_a_rows)?;
    let b_cols_global: Vec<u32> = local_b_cols
        .iter()
        .map(|&c| c + (expert_idx * n_e) as u32)
        .collect();
    let (tile_state, jackpot_hash) = compute_moe_tile(
        a_row_major,
        b_col_major,
        &outer_indices,
        &b_cols_global,
        &s_a,
        &s_b,
        k,
        r,
        dot_product_len,
    );
    Ok(PearlMoeTicket {
        s_a,
        s_b,
        commitment,
        outer_indices,
        b_cols_global,
        tile_state,
        jackpot_hash,
    })
}

/// **MoE routing-consistency binding (soundness gate).**
///
/// Verifies that the opened tile rows (`moe.outer_indices` — the rows the
/// recursive Layer-0 STARK proves the tile over) are **exactly** the expert's
/// routed tokens selected by the *public* row pattern, and that this follows from
/// the *committed* routing. Without this a prover could open arbitrary A-rows and
/// claim they are the expert's routed tokens, breaking the correspondence to a
/// valid Pearl MoE proof.
///
/// The Nockchain recursive certificate proves the Pearl-compatible statement its
/// own way, so it may carry `routing_data` (the flat per-expert-sorted token
/// indices) publicly and check the gather in the verifier, rather than via an
/// in-circuit CTL. Everything the check relies on is committed:
/// `routing_data` → `routing_root` (= `moe.hash_routing`) → `hash_activations` →
/// `s_A` → jackpot; `routing_offsets` → `hash_offsets` → `s_A`; `rows_pattern` /
/// `expert_idx` → the mining config → `job_key`. So a forged `outer_indices`,
/// `routing_data`, or `routing_offsets` either fails a check here or changes the
/// jackpot the STARK is bound to.
///
/// Checks: `routing_data` is a well-formed `m·top_k` array of in-range token
/// indices; `routing_root == matrix_commitment(routing_data)`; `routing_offsets`
/// is a non-decreasing partition ending at `m·top_k`; and the gather
/// `outer_indices[u] == routing_data[expert_start + pattern[u]]` with each opened
/// position inside the expert's real (non-padding) token span.
pub fn verify_pearl_moe_routing_binding(
    kappa: &[u8; 32],
    mining_config: &PearlMiningConfig,
    moe: &PearlMoeParams,
    m: u32,
    t_rows: u32,
    routing_data: &[u32],
    max_pattern_len: usize,
) -> Result<(), PearlCompatError> {
    let cfg = mining_config
        .moe()
        .ok_or(PearlCompatError::MoePublicMissingConfig)?;
    let e = cfg.e as usize;
    let top_k = cfg.top_k as usize;

    // Expert bookkeeping is well-formed.
    if moe.routing_offsets.len() != e {
        return Err(PearlCompatError::MoeExpertCountMismatch {
            expected: e,
            actual: moe.routing_offsets.len(),
        });
    }
    if (moe.expert_idx as usize) >= e {
        return Err(PearlCompatError::MoeExpertIdxOutOfRange {
            expert_idx: moe.expert_idx,
            e: cfg.e,
        });
    }

    // routing_data is a well-formed m*top_k array of in-range token indices.
    let numel = (m as u64)
        .checked_mul(top_k as u64)
        .ok_or(PearlCompatError::PublicParamEnvelope)?;
    if routing_data.len() as u64 != numel {
        return Err(PearlCompatError::MoeRoutingDataLenMismatch {
            expected: numel,
            actual: routing_data.len(),
        });
    }
    for (slot, &token) in routing_data.iter().enumerate() {
        if token >= m {
            return Err(PearlCompatError::MoeRoutingTokenOutOfRange { slot, token, m });
        }
    }

    // routing_offsets is a non-decreasing partition ending at numel (each expert
    // span is non-negative; the last entry accounts for every routed slot).
    if !moe.routing_offsets.windows(2).all(|w| w[0] <= w[1]) {
        return Err(PearlCompatError::MoeOffsetsInconsistent);
    }
    if u64::from(*moe.routing_offsets.last().unwrap()) != numel {
        return Err(PearlCompatError::MoeOffsetsInconsistent);
    }

    // Pearl acceptance-set parity (`zk-pow/src/api/sanity_checks.rs`): `top_k < e`
    // and each expert's span `<= m`. A token routes to a given expert at most once,
    // so an expert holds at most `m` of the `m*top_k` slots; without these bounds
    // we would accept degenerate over-routings (a token repeated within an expert,
    // or `top_k >= e`) that Pearl rejects — a merge-mining divergence, and a
    // routing the difficulty model never priced. `offsets[0] <= m` is the span of
    // expert 0 (start 0), covered by the loop below.
    if top_k >= e {
        return Err(PearlCompatError::MoeTopKNotLessThanExperts { top_k, e });
    }
    let mut prev = 0u32;
    for (expert, &end) in moe.routing_offsets.iter().enumerate() {
        // `end >= prev` (non-decreasing, checked above) so the span never underflows.
        let span = end - prev;
        if span > m {
            return Err(PearlCompatError::MoeExpertSpanExceedsTokens { expert, span, m });
        }
        prev = end;
    }

    // routing_root binding: the carried routing_data commits to moe.hash_routing.
    let routing_data_le: Vec<u8> = routing_data.iter().flat_map(|v| v.to_le_bytes()).collect();
    if crate::commit::matrix_commitment(&routing_data_le, kappa) != moe.hash_routing {
        return Err(PearlCompatError::MoeRoutingRootMismatch);
    }

    // The public row pattern selects positions within the expert's token subset.
    let inner = mining_config
        .rows_pattern
        .indices_with_offset_bounded(t_rows, max_pattern_len)?;
    if moe.outer_indices.len() != inner.len() {
        return Err(PearlCompatError::MoeOuterIndicesLenMismatch {
            expected: inner.len(),
            actual: moe.outer_indices.len(),
        });
    }
    let expert_start = if moe.expert_idx == 0 {
        0u32
    } else {
        moe.routing_offsets[moe.expert_idx as usize - 1]
    };
    let expert_end = moe.routing_offsets[moe.expert_idx as usize];

    // The gather: each opened row is the expert's routed token at the pattern
    // position, and that position is inside the expert's real (non-padding) span.
    for (u, &inner_u) in inner.iter().enumerate() {
        let pos = expert_start
            .checked_add(inner_u)
            .ok_or(PearlCompatError::PatternPeriodTooLarge)?;
        if pos >= expert_end {
            return Err(PearlCompatError::MoeOuterIndexOutsideExpert {
                expert_idx: moe.expert_idx,
                pos,
            });
        }
        if moe.outer_indices[u] != routing_data[pos as usize] {
            return Err(PearlCompatError::MoeOuterIndicesMismatch);
        }
    }
    Ok(())
}

/// Recompute the opened **global** B-columns for a MoE expert tile from the PUBLIC
/// column pattern, binding them to the expert's own weight block.
///
/// Pearl stacks the per-expert weight matrices, so expert `expert_idx` owns
/// **exactly** columns `[expert_idx·n_e, (expert_idx+1)·n_e)` with `n_e = n / e`
/// (Pearl `proof_utils.rs`: `first_expert_col = n_e · expert_idx`; the intra-expert
/// pattern indices are drawn from `n_e`). The intra-expert local columns therefore
/// MUST stay within `n_e`.
///
/// **Soundness (audit N1):** the downstream `validate_strip_indices` only bounds
/// the *global* column `< n` (total), which does NOT catch a local column
/// `≥ n_e`. Without the `local < n_e` clamp below, a `cols_pattern`/`t_cols` that
/// reaches past `n_e` makes `b_cols_global = local + expert_idx·n_e` bleed into a
/// **neighbouring expert's weights** while still passing `< n` — a divergence from
/// Pearl (which validates the pattern against the per-expert `n_e`) and a
/// column-grinding lever (the prover could open a favourable expert's columns
/// under a different `expert_idx`). This helper enforces the missing clamp.
pub fn moe_expert_b_cols_global(
    mining_config: &PearlMiningConfig,
    e: u16,
    n: u32,
    expert_idx: u16,
    t_cols: u32,
    max_pattern_len: usize,
) -> Result<Vec<u32>, PearlCompatError> {
    if e == 0 {
        return Err(PearlCompatError::MoePublicMissingConfig);
    }
    if expert_idx >= e {
        return Err(PearlCompatError::MoeExpertIdxOutOfRange { expert_idx, e });
    }
    if n % u32::from(e) != 0 {
        return Err(PearlCompatError::MoeColumnDimIndivisible { n, e });
    }
    let n_e = n / u32::from(e);
    let inner_cols = mining_config
        .cols_pattern
        .indices_with_offset_bounded(t_cols, max_pattern_len)?;
    // The load-bearing clamp: every intra-expert column stays inside this expert's
    // n_e-wide block (matches Pearl, which draws pattern indices from n_e).
    for &local in &inner_cols {
        if local >= n_e {
            return Err(PearlCompatError::MoeColumnOutsideExpert {
                local,
                n_e,
                expert_idx,
            });
        }
    }
    Ok(inner_cols
        .iter()
        .map(|&local| local + u32::from(expert_idx) * n_e)
        .collect())
}

pub fn verify_pearl_pattern_ticket(
    public_params: &PearlPublicProofParams,
    a_row_major: &[i8],
    b_col_major: &[i8],
    commitments: &PearlWorkCommitments,
    max_pattern_len: usize,
) -> Result<PearlPatternTicket, PearlCompatError> {
    let ticket = compute_pearl_pattern_ticket(
        public_params, a_row_major, b_col_major, commitments, max_pattern_len,
    )?;
    if ticket.jackpot_hash != public_params.hash_jackpot {
        return Err(PearlCompatError::JackpotHashMismatch);
    }
    Ok(ticket)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlCompatibleWorkPrecheck {
    pub commitments: PearlWorkCommitments,
    pub ticket: PearlPatternTicket,
    pub pearl_target: [u8; 32],
    pub nockchain_target: [u8; 32],
    pub nockchain_adjusted_target: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlNockchainAux {
    pub nockchain_chain_id: Vec<u8>,
    /// Canonical 32-byte digest of Nockchain's kernel-emitted
    /// `block-commitment:page:t` mining surface. The Hoon commitment itself
    /// binds the parent block id, tx-id set, coinbase split, timestamp, epoch
    /// counter, target, accumulated work, height, and page message.
    pub nock_block_commitment: [u8; 32],
    pub nockchain_target_epoch_or_height: u64,
    pub extra_domain_data: Vec<u8>,
}

impl PearlNockchainAux {
    pub fn commitment(&self) -> Result<[u8; 32], PearlCompatError> {
        pearl_nockchain_aux_commitment(
            &self.nockchain_chain_id, &self.nock_block_commitment,
            self.nockchain_target_epoch_or_height, &self.extra_domain_data,
        )
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, PearlCompatError> {
        validate_nockchain_aux_fields(&self.nockchain_chain_id, &self.extra_domain_data)?;
        let mut out = Vec::with_capacity(
            4 + 1 + self.nockchain_chain_id.len() + 32 + 8 + 2 + self.extra_domain_data.len(),
        );
        out.extend_from_slice(&PEARL_NOCKCHAIN_AUX_MAGIC);
        out.push(self.nockchain_chain_id.len() as u8);
        out.extend_from_slice(&self.nockchain_chain_id);
        out.extend_from_slice(&self.nock_block_commitment);
        out.extend_from_slice(&self.nockchain_target_epoch_or_height.to_le_bytes());
        out.extend_from_slice(&(self.extra_domain_data.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.extra_domain_data);
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PearlCompatError> {
        if !(PEARL_NOCKCHAIN_AUX_MIN_SIZE..=PEARL_NOCKCHAIN_AUX_MAX_SIZE).contains(&bytes.len()) {
            return Err(PearlCompatError::BadNockchainAuxLen(bytes.len()));
        }
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if magic != PEARL_NOCKCHAIN_AUX_MAGIC {
            return Err(PearlCompatError::BadNockchainAuxMagic(magic));
        }

        let chain_len = bytes[4] as usize;
        validate_nockchain_aux_chain_id_len(chain_len)?;

        let mut offset = 5usize;
        let after_chain = offset
            .checked_add(chain_len)
            .ok_or(PearlCompatError::BadNockchainAuxLen(bytes.len()))?;
        let fixed_after_chain = after_chain
            .checked_add(32 + 8 + 2)
            .ok_or(PearlCompatError::BadNockchainAuxLen(bytes.len()))?;
        if fixed_after_chain > bytes.len() {
            return Err(PearlCompatError::BadNockchainAuxLen(bytes.len()));
        }

        let nockchain_chain_id = bytes[offset..after_chain].to_vec();
        offset = after_chain;
        let nock_block_commitment: [u8; 32] = bytes[offset..offset + 32].try_into().unwrap();
        offset += 32;
        let nockchain_target_epoch_or_height =
            u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        offset += 8;
        let extra_len = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        validate_nockchain_aux_extra_len(extra_len)?;
        let expected = offset
            .checked_add(extra_len)
            .ok_or(PearlCompatError::BadNockchainAuxLen(bytes.len()))?;
        if expected != bytes.len() {
            return Err(PearlCompatError::NockchainAuxTrailingData {
                expected,
                actual: bytes.len(),
            });
        }
        let extra_domain_data = bytes[offset..expected].to_vec();

        Ok(Self {
            nockchain_chain_id,
            nock_block_commitment,
            nockchain_target_epoch_or_height,
            extra_domain_data,
        })
    }
}

/// Pearl-side evidence that the Nockchain aux digest was committed before the
/// shared work attempt was mined.
///
/// The proof is intentionally coinbase-rooted. The current Nockchain production
/// profile uses coinbase-only Pearl block templates, so `merkle_branch` must be
/// empty and any nonempty branch is rejected. The field remains in the
/// Rust-owned nonce format so a future milestone can deliberately add ordinary
/// Pearl transaction merkle tree support without changing the outer `%ai-pow`
/// noun shape. The header stores the resulting root in display byte order,
/// matching `IncompleteBlockHeader::merkle_root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlAuxInclusionProof {
    pub coinbase_tx: Vec<u8>,
    pub merkle_branch: Vec<[u8; 32]>,
}

/// Verify that `aux_commitment` is present in the txid-committed coinbase script
/// and that the coinbase txid is the Pearl header merkle root.
///
/// This checks the Pearl block commitment side of merge mining without
/// requiring Nockchain to parse or verify Pearl's ZKP, or to construct Pearl
/// transaction trees itself. The current production profile deliberately
/// supports only coinbase-only Pearl block templates, so the merkle branch must
/// be empty and the header root is just the coinbase txid in header byte order.
/// The tagged payload is:
///
/// ```text
/// "NOCKCHAIN-AI-POW-AUX" || aux_commitment
/// ```
///
/// The tag must appear in the coinbase input script bytes committed by the
/// transaction id; a SegWit witness-only occurrence is rejected because witness
/// bytes are not part of the txid or regular transaction merkle root.
pub fn verify_pearl_aux_inclusion(
    header: &PearlIncompleteBlockHeader,
    aux_commitment: &[u8; 32],
    proof: &PearlAuxInclusionProof,
) -> Result<(), PearlCompatError> {
    if proof.coinbase_tx.is_empty() {
        return Err(PearlCompatError::PearlAuxCoinbaseTxEmpty);
    }
    if proof.coinbase_tx.len() > PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES {
        return Err(PearlCompatError::PearlAuxCoinbaseTxTooLarge(
            proof.coinbase_tx.len(),
        ));
    }
    if proof.merkle_branch.len() > PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH {
        return Err(PearlCompatError::PearlAuxMerkleBranchTooDeep(
            proof.merkle_branch.len(),
        ));
    }

    let parsed_tx = pearl_txid_committed_bytes(&proof.coinbase_tx)?;
    let mut tagged = Vec::with_capacity(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG.len() + 32);
    tagged.extend_from_slice(PEARL_NOCKCHAIN_AUX_COMMITMENT_TAG);
    tagged.extend_from_slice(aux_commitment);
    if !contains_subslice(&parsed_tx.coinbase_script, &tagged) {
        return Err(PearlCompatError::PearlAuxCommitmentTagMissing);
    }

    let mut root = pearl_bitcoin_double_sha256_raw(&parsed_tx.txid_committed_bytes);
    for sibling in &proof.merkle_branch {
        let mut pair = [0u8; 64];
        pair[..32].copy_from_slice(&root);
        pair[32..].copy_from_slice(sibling);
        root = pearl_bitcoin_double_sha256_raw(&pair);
    }
    root.reverse();
    if root != header.merkle_root {
        return Err(PearlCompatError::PearlAuxMerkleRootMismatch);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeMiningPrecheck {
    pub work: PearlCompatibleWorkPrecheck,
    pub aux: PearlNockchainAux,
    pub aux_commitment: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergePublicStatement {
    pub block_header: [u8; PEARL_INCOMPLETE_BLOCK_HEADER_SIZE],
    pub public_data: [u8; PEARL_PUBLIC_PROOF_PARAMS_SIZE],
    pub expected_aux_commitment: [u8; 32],
    pub aux_bytes: Vec<u8>,
}

impl PearlMergePublicStatement {
    pub fn to_bytes(&self) -> Result<Vec<u8>, PearlCompatError> {
        PearlNockchainAux::from_bytes(&self.aux_bytes)?;
        let mut out =
            Vec::with_capacity(PEARL_MERGE_PUBLIC_STATEMENT_FIXED_SIZE + self.aux_bytes.len());
        out.extend_from_slice(&PEARL_MERGE_PUBLIC_STATEMENT_MAGIC);
        out.extend_from_slice(&self.block_header);
        out.extend_from_slice(&self.public_data);
        out.extend_from_slice(&self.expected_aux_commitment);
        out.extend_from_slice(&(self.aux_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.aux_bytes);
        Ok(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PearlCompatError> {
        if !(PEARL_MERGE_PUBLIC_STATEMENT_MIN_SIZE..=PEARL_MERGE_PUBLIC_STATEMENT_MAX_SIZE)
            .contains(&bytes.len())
        {
            return Err(PearlCompatError::BadMergePublicStatementLen(bytes.len()));
        }
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        if magic != PEARL_MERGE_PUBLIC_STATEMENT_MAGIC {
            return Err(PearlCompatError::BadMergePublicStatementMagic(magic));
        }

        let mut offset = 4usize;
        let block_header = bytes[offset..offset + PEARL_INCOMPLETE_BLOCK_HEADER_SIZE]
            .try_into()
            .unwrap();
        offset += PEARL_INCOMPLETE_BLOCK_HEADER_SIZE;
        let public_data = bytes[offset..offset + PEARL_PUBLIC_PROOF_PARAMS_SIZE]
            .try_into()
            .unwrap();
        offset += PEARL_PUBLIC_PROOF_PARAMS_SIZE;
        let expected_aux_commitment = bytes[offset..offset + 32].try_into().unwrap();
        offset += 32;
        let aux_len = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let expected = offset
            .checked_add(aux_len)
            .ok_or(PearlCompatError::BadMergePublicStatementLen(bytes.len()))?;
        if expected != bytes.len() {
            return Err(PearlCompatError::MergePublicStatementTrailingData {
                expected,
                actual: bytes.len(),
            });
        }
        let aux_bytes = bytes[offset..expected].to_vec();
        PearlNockchainAux::from_bytes(&aux_bytes)?;

        Ok(Self {
            block_header,
            public_data,
            expected_aux_commitment,
            aux_bytes,
        })
    }
}

/// Verify the complete Pearl-compatible work precheck shared by Pearl and
/// Nockchain.
///
/// This is the canonical Rust entrypoint for checking that a public
/// Pearl-style work statement is tied to the supplied matrices and clears the
/// independent Nockchain target. It deliberately
/// uses Pearl's serialized `sigma` and `mu` transcript, with no Nockchain nonce
/// or selected-tile derivation mixed in.
pub fn verify_pearl_compatible_work(
    public_params: &PearlPublicProofParams,
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlCompatibleWorkPrecheck, PearlCompatError> {
    public_params.sanity_check()?;

    let pearl_target = public_params.pearl_adjusted_target()?;
    let nockchain_adjusted_target = public_params.nockchain_adjusted_target(nockchain_target)?;
    if !hash_le_target(&public_params.hash_jackpot, &nockchain_adjusted_target) {
        return Err(PearlCompatError::NockchainTargetNotMet);
    }
    validate_public_matrix_inputs(a_row_major, b_col_major, public_params)?;

    let sigma = public_params.block_header.to_bytes();
    let mu = public_params.mining_config.to_bytes()?;
    let commitments = derive_pearl_work_commitments(&sigma, &mu, a_row_major, b_col_major);
    let ticket = verify_pearl_pattern_ticket(
        public_params, a_row_major, b_col_major, &commitments, max_pattern_len,
    )?;

    Ok(PearlCompatibleWorkPrecheck {
        commitments,
        ticket,
        pearl_target,
        nockchain_target: *nockchain_target,
        nockchain_adjusted_target,
    })
}

/// Decode Pearl's persisted/wire public statement bytes and run the complete
/// shared-work precheck.
///
/// `block_header_bytes` is Pearl's 76-byte serialized `IncompleteBlockHeader`
/// (`sigma`). `public_data` is Pearl's 164-byte public proof parameter blob
/// (`mu || H_A || H_B || hash_jackpot || m || n || t_rows || t_cols`). This
/// entrypoint is intentionally strict about lengths and uses the decoded bytes
/// to rederive the exact same transcript checked by
/// [`verify_pearl_compatible_work`].
pub fn verify_pearl_compatible_public_data(
    block_header_bytes: &[u8],
    public_data: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlCompatibleWorkPrecheck, PearlCompatError> {
    let block_header = PearlIncompleteBlockHeader::from_bytes(block_header_bytes)?;
    let public_params = PearlPublicProofParams::from_public_data(block_header, public_data)?;
    verify_pearl_compatible_work(
        &public_params, a_row_major, b_col_major, nockchain_target, max_pattern_len,
    )
}

/// Verify a Pearl-compatible public work statement and bind it to the expected
/// Nockchain AuxPoW digest.
///
/// `expected_aux_commitment` must be the digest the caller has independently
/// verified as included in the Pearl block/work state represented by
/// `block_header_bytes`. This function does not prove that inclusion; it
/// closes the replay gap between the verified Pearl work attempt and the
/// candidate Nockchain block once the inclusion verifier has supplied that
/// digest.
pub fn verify_pearl_merge_mining_public_data(
    candidate_nock_block_commitment: &[u8; 32],
    block_header_bytes: &[u8],
    public_data: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    aux: PearlNockchainAux,
    expected_aux_commitment: &[u8; 32],
) -> Result<PearlMergeMiningPrecheck, PearlCompatError> {
    if aux.nock_block_commitment != *candidate_nock_block_commitment {
        return Err(PearlCompatError::NockchainAuxBlockCommitmentMismatch);
    }
    let aux_commitment = aux.commitment()?;
    if &aux_commitment != expected_aux_commitment {
        return Err(PearlCompatError::NockchainAuxCommitmentMismatch);
    }
    let work = verify_pearl_compatible_public_data(
        block_header_bytes, public_data, a_row_major, b_col_major, nockchain_target,
        max_pattern_len,
    )?;
    Ok(PearlMergeMiningPrecheck {
        work,
        aux,
        aux_commitment,
    })
}

/// Decode canonical Nockchain aux bytes and verify the complete
/// Pearl-compatible merge-mining statement.
///
/// This is the wire-facing variant of
/// [`verify_pearl_merge_mining_public_data`]. It rejects malformed aux bytes
/// before checking the trusted candidate Nockchain block commitment, the
/// expected Pearl-included aux digest, and the shared Pearl work statement.
pub fn verify_pearl_merge_mining_public_data_with_aux_bytes(
    candidate_nock_block_commitment: &[u8; 32],
    block_header_bytes: &[u8],
    public_data: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    aux_bytes: &[u8],
    expected_aux_commitment: &[u8; 32],
) -> Result<PearlMergeMiningPrecheck, PearlCompatError> {
    let aux = PearlNockchainAux::from_bytes(aux_bytes)?;
    verify_pearl_merge_mining_public_data(
        candidate_nock_block_commitment, block_header_bytes, public_data, a_row_major, b_col_major,
        nockchain_target, max_pattern_len, aux, expected_aux_commitment,
    )
}

/// Verify a Pearl-compatible public work statement and the Pearl merkle
/// inclusion proof for the Nockchain aux digest.
///
/// This is the Rust-side verifier API that closes the aux replay gap: it first
/// proves that `expected_aux_commitment` is present in txid-committed coinbase
/// bytes under the Pearl header merkle root, then runs the normal
/// Nockchain/Pearl shared-work precheck.
pub fn verify_pearl_merge_mining_public_data_with_aux_inclusion(
    candidate_nock_block_commitment: &[u8; 32],
    block_header_bytes: &[u8],
    public_data: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    aux_bytes: &[u8],
    expected_aux_commitment: &[u8; 32],
    inclusion_proof: &PearlAuxInclusionProof,
) -> Result<PearlMergeMiningPrecheck, PearlCompatError> {
    let header = PearlIncompleteBlockHeader::from_bytes(block_header_bytes)?;
    verify_pearl_aux_inclusion(&header, expected_aux_commitment, inclusion_proof)?;
    verify_pearl_merge_mining_public_data_with_aux_bytes(
        candidate_nock_block_commitment, block_header_bytes, public_data, a_row_major, b_col_major,
        nockchain_target, max_pattern_len, aux_bytes, expected_aux_commitment,
    )
}

/// Decode the complete canonical Pearl merge-mining public statement envelope
/// and verify it against verifier-derived block/target data.
pub fn verify_pearl_merge_public_statement_bytes(
    candidate_nock_block_commitment: &[u8; 32],
    statement_bytes: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
) -> Result<PearlMergeMiningPrecheck, PearlCompatError> {
    let statement = PearlMergePublicStatement::from_bytes(statement_bytes)?;
    verify_pearl_merge_mining_public_data_with_aux_bytes(
        candidate_nock_block_commitment, &statement.block_header, &statement.public_data,
        a_row_major, b_col_major, nockchain_target, max_pattern_len, &statement.aux_bytes,
        &statement.expected_aux_commitment,
    )
}

/// Decode the complete canonical Pearl merge-mining public statement envelope,
/// verify the aux digest is included in the Pearl header's transaction merkle
/// root, and then verify the shared Pearl/Nockchain work statement.
pub fn verify_pearl_merge_public_statement_bytes_with_aux_inclusion(
    candidate_nock_block_commitment: &[u8; 32],
    statement_bytes: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    inclusion_proof: &PearlAuxInclusionProof,
) -> Result<PearlMergeMiningPrecheck, PearlCompatError> {
    let statement = PearlMergePublicStatement::from_bytes(statement_bytes)?;
    verify_pearl_merge_mining_public_data_with_aux_inclusion(
        candidate_nock_block_commitment, &statement.block_header, &statement.public_data,
        a_row_major, b_col_major, nockchain_target, max_pattern_len, &statement.aux_bytes,
        &statement.expected_aux_commitment, inclusion_proof,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlMergeTicketAttempt {
    pub public_params: PearlPublicProofParams,
    pub ticket: PearlPatternTicket,
    pub commitments: PearlWorkCommitments,
    pub pearl_target: [u8; 32],
    pub nockchain_target: [u8; 32],
    pub aux: PearlNockchainAux,
    pub aux_commitment: [u8; 32],
    pub statement: PearlMergePublicStatement,
}

/// Build the exact Pearl-compatible ticket statement for one explicit
/// `t_rows`/`t_cols` attempt.
///
/// This does not search alternate offsets or nonce-like values over cached
/// work. Callers that want to try another ticket must call this again with the
/// next Pearl-valid offset pair and then generate the Nockchain recursive proof
/// only if [`mine_pearl_merge_ticket_attempt`] returns `Some`.
pub fn evaluate_pearl_merge_ticket_attempt(
    header: &PearlIncompleteBlockHeader,
    config: &PearlMiningConfig,
    params: &MatmulParams,
    t_rows: u32,
    t_cols: u32,
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    aux: PearlNockchainAux,
) -> Result<PearlMergeTicketAttempt, PearlCompatError> {
    if !config.rows_pattern.offset_is_valid(t_rows) || !config.cols_pattern.offset_is_valid(t_cols)
    {
        return Err(PearlCompatError::InvalidPatternOffset);
    }
    let aux_commitment = aux.commitment()?;
    let aux_bytes = aux.to_bytes()?;
    validate_config_matches_params(config, params)?;
    validate_attempt_inputs(a_row_major, b_col_major, params)?;

    let sigma = header.to_bytes();
    let mu = config.to_bytes()?;
    let commitments = derive_pearl_work_commitments(&sigma, &mu, a_row_major, b_col_major);
    let mut public_params = PearlPublicProofParams {
        block_header: *header,
        mining_config: *config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: [0u8; 32],
        m: params.m,
        n: params.n,
        t_rows,
        t_cols,
    };
    public_params.sanity_check()?;

    let ticket = compute_pearl_pattern_ticket(
        &public_params, a_row_major, b_col_major, &commitments, max_pattern_len,
    )?;
    public_params.hash_jackpot = ticket.jackpot_hash;
    let pearl_target = public_params.pearl_adjusted_target()?;
    let public_data = public_params.to_public_data()?;
    let statement = PearlMergePublicStatement {
        block_header: sigma,
        public_data,
        expected_aux_commitment: aux_commitment,
        aux_bytes,
    };

    Ok(PearlMergeTicketAttempt {
        public_params,
        ticket,
        commitments,
        pearl_target,
        nockchain_target: *nockchain_target,
        aux,
        aux_commitment,
        statement,
    })
}

/// Return the canonical Pearl-format-compatible Nockchain public statement for
/// one explicit ticket only when that ticket satisfies the caller-supplied
/// Nockchain target.
pub fn mine_pearl_merge_ticket_attempt(
    header: &PearlIncompleteBlockHeader,
    config: &PearlMiningConfig,
    params: &MatmulParams,
    t_rows: u32,
    t_cols: u32,
    a_row_major: &[i8],
    b_col_major: &[i8],
    nockchain_target: &[u8; 32],
    max_pattern_len: usize,
    aux: PearlNockchainAux,
) -> Result<Option<PearlMergeTicketAttempt>, PearlCompatError> {
    let attempt = evaluate_pearl_merge_ticket_attempt(
        header, config, params, t_rows, t_cols, a_row_major, b_col_major, nockchain_target,
        max_pattern_len, aux,
    )?;
    if attempt
        .public_params
        .check_nockchain_jackpot_target(nockchain_target)
        .is_err()
    {
        return Ok(None);
    }
    Ok(Some(attempt))
}

fn validate_public_matrix_inputs(
    a_row_major: &[i8],
    b_col_major: &[i8],
    public_params: &PearlPublicProofParams,
) -> Result<(), PearlCompatError> {
    let m = public_params.m as usize;
    let k = public_params.mining_config.common_dim as usize;
    let n = public_params.n as usize;
    if a_row_major.len() != m * k {
        return Err(PearlCompatError::InputAShape {
            expected: m * k,
            actual: a_row_major.len(),
        });
    }
    if b_col_major.len() != n * k {
        return Err(PearlCompatError::InputBShape {
            expected: n * k,
            actual: b_col_major.len(),
        });
    }
    for (index, &value) in a_row_major.iter().enumerate() {
        if !(-INPUT_RANGE_MAX..=INPUT_RANGE_MAX).contains(&value) {
            return Err(PearlCompatError::InputOutOfRange {
                matrix: "A",
                index,
                value,
            });
        }
    }
    for (index, &value) in b_col_major.iter().enumerate() {
        if !(-INPUT_RANGE_MAX..=INPUT_RANGE_MAX).contains(&value) {
            return Err(PearlCompatError::InputOutOfRange {
                matrix: "B",
                index,
                value,
            });
        }
    }
    Ok(())
}

fn pearl_e_row_into(seed: &[u8; 32], row: u32, k: u32, r: usize, out: &mut [i8]) {
    debug_assert_eq!(out.len(), k as usize);
    let mut e_l_row = vec![0i8; r];
    prng::expand_e_l_row(seed, row, r as u32, &mut e_l_row);
    for l in 0..k {
        let (pp, pm) = prng::e_r_col_positions(seed, l, r as u32);
        out[l as usize] = e_l_row[pp as usize] - e_l_row[pm as usize];
    }
}

fn pearl_f_col_into(seed: &[u8; 32], col: u32, k: u32, r: usize, out: &mut [i8]) {
    debug_assert_eq!(out.len(), k as usize);
    let mut f_r_col = vec![0i8; r];
    prng::expand_f_r_col(seed, col, r as u32, &mut f_r_col);
    for l in 0..k {
        let (pp, pm) = prng::f_l_row_positions(seed, l, r as u32);
        out[l as usize] = f_r_col[pp as usize] - f_r_col[pm as usize];
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PearlWorkCommitments {
    pub kappa: [u8; 32],
    pub h_a: [u8; 32],
    pub h_b: [u8; 32],
    pub s_a: [u8; 32],
    pub s_b: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlTileDigest {
    pub tile_i: u32,
    pub tile_j: u32,
    pub tile_state: TileState,
    pub jackpot_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PearlAttempt {
    pub sigma: Vec<u8>,
    pub mu: Vec<u8>,
    pub params: MatmulParams,
    pub commitments: PearlWorkCommitments,
    pub tile_digests: Vec<PearlTileDigest>,
}

pub fn pearl_kappa(sigma: &[u8], mu: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(sigma);
    hasher.update(mu);
    *hasher.finalize().as_bytes()
}

pub fn pearl_matrix_commitments(
    a_row_major: &[i8],
    b_col_major: &[i8],
    kappa: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    let a_bytes = i8_slice_as_u8(a_row_major);
    let b_bytes = i8_slice_as_u8(b_col_major);
    (
        matrix_commitment(a_bytes, kappa),
        matrix_commitment(b_bytes, kappa),
    )
}

pub fn pearl_noise_seeds(kappa: &[u8; 32], h_a: &[u8; 32], h_b: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let s_b = noise_seed_b(kappa, h_b);
    let s_a = noise_seed_a(&s_b, h_a);
    (s_a, s_b)
}

pub fn pearl_jackpot_hash(tile_state: &TileState, s_a: &[u8; 32]) -> [u8; 32] {
    tile_state.keyed_hash(s_a)
}

/// Domain-separated Nockchain AuxPoW commitment to embed into Pearl's work
/// state before mining.
///
/// The variable-length fields are length-prefixed so distinct tuples cannot
/// collide by concatenation. `nock_block_commitment` is the canonical digest of
/// Nockchain's kernel-emitted `block-commitment:page:t`, so it transitively
/// binds the previous block id/header chain and the candidate tx-id set. The
/// returned digest must be included in Pearl's block commitment path;
/// Nockchain validation must then verify that inclusion against the exact Pearl
/// `sigma` used for the shared work attempt.
pub fn pearl_nockchain_aux_commitment(
    nockchain_chain_id: &[u8],
    nock_block_commitment: &[u8; 32],
    nockchain_target_epoch_or_height: u64,
    extra_domain_data: &[u8],
) -> Result<[u8; 32], PearlCompatError> {
    validate_nockchain_aux_fields(nockchain_chain_id, extra_domain_data)?;

    let mut hasher = Hasher::new();
    hasher.update(PEARL_NOCKCHAIN_AUX_DOMAIN);
    hash_len_prefixed(&mut hasher, nockchain_chain_id);
    hasher.update(nock_block_commitment);
    hasher.update(&nockchain_target_epoch_or_height.to_le_bytes());
    hash_len_prefixed(&mut hasher, extra_domain_data);
    Ok(*hasher.finalize().as_bytes())
}

pub fn derive_pearl_work_commitments(
    sigma: &[u8],
    mu: &[u8],
    a_row_major: &[i8],
    b_col_major: &[i8],
) -> PearlWorkCommitments {
    let kappa = pearl_kappa(sigma, mu);
    let (h_a, h_b) = pearl_matrix_commitments(a_row_major, b_col_major, &kappa);
    let (s_a, s_b) = pearl_noise_seeds(&kappa, &h_a, &h_b);
    PearlWorkCommitments {
        kappa,
        h_a,
        h_b,
        s_a,
        s_b,
    }
}

impl PearlAttempt {
    /// Build a diagnostic native-square tile digest set from typed Pearl
    /// header/config values.
    ///
    /// Production Pearl merge mining should use
    /// [`evaluate_pearl_merge_ticket_attempt`], which binds the exact Pearl
    /// periodic-pattern ticket. This helper remains for tests and native
    /// square-tile diagnostics that want all legacy tile digests.
    pub fn build_with_config(
        header: &PearlIncompleteBlockHeader,
        config: &PearlMiningConfig,
        a_row_major: &[i8],
        b_col_major: &[i8],
        params: &MatmulParams,
    ) -> Result<Self, PearlCompatError> {
        let sigma = header.to_bytes();
        let mu = config.to_bytes()?;
        Self::build_from_serialized(&sigma, &mu, a_row_major, b_col_major, params)
    }

    /// Build a diagnostic native-square tile digest set from Pearl's serialized
    /// `IncompleteBlockHeader` (`sigma`) and `MiningConfiguration` (`mu`).
    ///
    /// Both byte strings are parsed before use. This helper enumerates native
    /// square tiles through `MatmulParams`; it is not the production
    /// Pearl-compatible ticket path for arbitrary `PeriodicPattern` values.
    pub fn build_from_serialized(
        sigma: &[u8],
        mu: &[u8],
        a_row_major: &[i8],
        b_col_major: &[i8],
        params: &MatmulParams,
    ) -> Result<Self, PearlCompatError> {
        let _header = PearlIncompleteBlockHeader::from_bytes(sigma)?;
        let config = PearlMiningConfig::from_bytes(mu)?;
        validate_config_matches_params(&config, params)?;
        validate_attempt_inputs(a_row_major, b_col_major, params)?;
        let commitments = derive_pearl_work_commitments(sigma, mu, a_row_major, b_col_major);
        let noise = BlockNoise::expand(&commitments.s_a, &commitments.s_b, params);
        let matrices = Matrices::build(a_row_major, b_col_major, &noise, params);
        let mut tile_digests = Vec::with_capacity(params.num_tiles() as usize);
        for tile_i in 0..params.row_tiles() {
            for tile_j in 0..params.col_tiles() {
                let tile_state = compute_tile(&matrices, params, tile_i, tile_j);
                let jackpot_hash = pearl_jackpot_hash(&tile_state, &commitments.s_a);
                tile_digests.push(PearlTileDigest {
                    tile_i,
                    tile_j,
                    tile_state,
                    jackpot_hash,
                });
            }
        }
        Ok(Self {
            sigma: sigma.to_vec(),
            mu: mu.to_vec(),
            params: *params,
            commitments,
            tile_digests,
        })
    }
}

fn hash_len_prefixed(hasher: &mut Hasher, bytes: &[u8]) {
    debug_assert!(u32::try_from(bytes.len()).is_ok());
    hasher.update(&(bytes.len() as u32).to_le_bytes());
    hasher.update(bytes);
}

fn validate_nockchain_aux_fields(
    nockchain_chain_id: &[u8],
    extra_domain_data: &[u8],
) -> Result<(), PearlCompatError> {
    validate_nockchain_aux_chain_id_len(nockchain_chain_id.len())?;
    validate_nockchain_aux_extra_len(extra_domain_data.len())?;
    Ok(())
}

fn validate_nockchain_aux_chain_id_len(len: usize) -> Result<(), PearlCompatError> {
    if len == 0 {
        return Err(PearlCompatError::NockchainAuxChainIdEmpty);
    }
    if len > PEARL_NOCKCHAIN_AUX_CHAIN_ID_MAX {
        return Err(PearlCompatError::NockchainAuxChainIdTooLarge(len));
    }
    Ok(())
}

fn validate_nockchain_aux_extra_len(len: usize) -> Result<(), PearlCompatError> {
    if len > PEARL_NOCKCHAIN_AUX_EXTRA_MAX {
        return Err(PearlCompatError::NockchainAuxExtraTooLarge(len));
    }
    Ok(())
}

fn validate_config_matches_params(
    config: &PearlMiningConfig,
    params: &MatmulParams,
) -> Result<(), PearlCompatError> {
    if config.common_dim != params.k {
        return Err(PearlCompatError::CommonDimMismatch);
    }
    if u32::from(config.rank) != params.noise_rank {
        return Err(PearlCompatError::RankMismatch);
    }
    Ok(())
}

fn validate_attempt_inputs(
    a_row_major: &[i8],
    b_col_major: &[i8],
    params: &MatmulParams,
) -> Result<(), PearlCompatError> {
    validate_recursive_params_for_pearl_schedule(params)?;
    let m = params.m as usize;
    let k = params.k as usize;
    let n = params.n as usize;
    if a_row_major.len() != m * k {
        return Err(PearlCompatError::InputAShape {
            expected: m * k,
            actual: a_row_major.len(),
        });
    }
    if b_col_major.len() != n * k {
        return Err(PearlCompatError::InputBShape {
            expected: n * k,
            actual: b_col_major.len(),
        });
    }
    for (index, &value) in a_row_major.iter().enumerate() {
        if !(-INPUT_RANGE_MAX..=INPUT_RANGE_MAX).contains(&value) {
            return Err(PearlCompatError::InputOutOfRange {
                matrix: "A",
                index,
                value,
            });
        }
    }
    for (index, &value) in b_col_major.iter().enumerate() {
        if !(-INPUT_RANGE_MAX..=INPUT_RANGE_MAX).contains(&value) {
            return Err(PearlCompatError::InputOutOfRange {
                matrix: "B",
                index,
                value,
            });
        }
    }
    Ok(())
}

pub fn pearl_bitcoin_double_sha256_raw(bytes: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(bytes);
    Sha256::digest(first).into()
}

struct PearlTxidCommittedBytes {
    txid_committed_bytes: Vec<u8>,
    coinbase_script: Vec<u8>,
}

fn pearl_txid_committed_bytes(tx: &[u8]) -> Result<PearlTxidCommittedBytes, PearlCompatError> {
    let mut offset = 0usize;
    take(tx, &mut offset, 4)?;
    let mut txid = Vec::with_capacity(tx.len());
    txid.extend_from_slice(&tx[..4]);

    let segwit = if tx.get(offset) == Some(&0) {
        if tx.get(offset + 1) != Some(&1) {
            return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
        }
        offset += 2;
        true
    } else {
        false
    };

    let committed_start = offset;
    let input_count = read_canonical_varint(tx, &mut offset)?;
    if input_count != 1 {
        return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
    }
    let first_input_start = offset;
    let mut coinbase_script = Vec::new();
    for _ in 0..input_count {
        take(tx, &mut offset, 36)?;
        let script_len = read_canonical_varint_usize(tx, &mut offset)?;
        coinbase_script = take(tx, &mut offset, script_len)?.to_vec();
        take(tx, &mut offset, 4)?;
    }
    validate_first_input_is_coinbase(tx, first_input_start)?;

    let output_count = read_canonical_varint(tx, &mut offset)?;
    if output_count == 0 {
        return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
    }
    for _ in 0..output_count {
        take(tx, &mut offset, 8)?;
        let script_len = read_canonical_varint_usize(tx, &mut offset)?;
        take(tx, &mut offset, script_len)?;
    }
    txid.extend_from_slice(&tx[committed_start..offset]);

    if segwit {
        for _ in 0..input_count {
            let item_count = read_canonical_varint(tx, &mut offset)?;
            for _ in 0..item_count {
                let item_len = read_canonical_varint_usize(tx, &mut offset)?;
                take(tx, &mut offset, item_len)?;
            }
        }
    }

    let locktime = take(tx, &mut offset, 4)?;
    txid.extend_from_slice(locktime);
    if offset != tx.len() {
        return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
    }
    Ok(PearlTxidCommittedBytes {
        txid_committed_bytes: txid,
        coinbase_script,
    })
}

fn validate_first_input_is_coinbase(tx: &[u8], input_start: usize) -> Result<(), PearlCompatError> {
    let prevout = tx
        .get(input_start..input_start + 36)
        .ok_or(PearlCompatError::PearlAuxMalformedCoinbaseTx)?;
    if prevout[..32] != [0u8; 32] || prevout[32..36] != u32::MAX.to_le_bytes() {
        return Err(PearlCompatError::PearlAuxNotCoinbase);
    }
    Ok(())
}

fn read_canonical_varint(tx: &[u8], offset: &mut usize) -> Result<u64, PearlCompatError> {
    let tag = *tx
        .get(*offset)
        .ok_or(PearlCompatError::PearlAuxMalformedCoinbaseTx)?;
    *offset += 1;
    match tag {
        0x00..=0xfc => Ok(u64::from(tag)),
        0xfd => {
            let bytes = take(tx, offset, 2)?;
            let value = u16::from_le_bytes(bytes.try_into().unwrap()) as u64;
            if value < 0xfd {
                return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
            }
            Ok(value)
        }
        0xfe => {
            let bytes = take(tx, offset, 4)?;
            let value = u32::from_le_bytes(bytes.try_into().unwrap()) as u64;
            if value <= u64::from(u16::MAX) {
                return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
            }
            Ok(value)
        }
        0xff => {
            let bytes = take(tx, offset, 8)?;
            let value = u64::from_le_bytes(bytes.try_into().unwrap());
            if value <= u64::from(u32::MAX) {
                return Err(PearlCompatError::PearlAuxMalformedCoinbaseTx);
            }
            Ok(value)
        }
    }
}

fn read_canonical_varint_usize(tx: &[u8], offset: &mut usize) -> Result<usize, PearlCompatError> {
    usize::try_from(read_canonical_varint(tx, offset)?)
        .map_err(|_| PearlCompatError::PearlAuxMalformedCoinbaseTx)
}

fn take<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], PearlCompatError> {
    let end = offset
        .checked_add(len)
        .ok_or(PearlCompatError::PearlAuxMalformedCoinbaseTx)?;
    let out = bytes
        .get(*offset..end)
        .ok_or(PearlCompatError::PearlAuxMalformedCoinbaseTx)?;
    *offset = end;
    Ok(out)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn i8_slice_as_u8(input: &[i8]) -> &[u8] {
    // SAFETY: i8 and u8 have identical layout and alignment. The commitment
    // hashes raw two's-complement bytes, which is exactly what Pearl hashes.
    unsafe { core::slice::from_raw_parts(input.as_ptr() as *const u8, input.len()) }
}
