//! Speed-of-light archive format
//!
//! Stores extracted blocks as jammed noun blobs with bincode metadata
//! for fast loading during benchmark runs.
//!
//! File format:
//! ```text
//! ┌─────────────────────────────────────┐
//! │ Metadata Header (bincode)           │
//! │  - magic, version, counts           │
//! │  - Vec<BlockEntry> index            │
//! ├─────────────────────────────────────┤
//! │ Jammed Noun Blobs                   │
//! │  - block 0 jam bytes                │
//! │  - block 1 jam bytes                │
//! │  - ...                              │
//! └─────────────────────────────────────┘
//! ```

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use nockchain_types::tx_engine::common::Hash;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::types::{ProofVersion, SolHeight};

/// Magic bytes to identify speed-of-light archive files
pub const ARCHIVE_MAGIC: &[u8; 8] = b"SOLARCH\0";

/// Current archive format version.
pub const ARCHIVE_VERSION: u32 = 1;

/// Byte offset wrapper for archive sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteOffset(pub u64);

impl ByteOffset {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for ByteOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for ByteOffset {
    fn from(value: u64) -> Self {
        ByteOffset(value)
    }
}

/// Byte size wrapper for archive sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for ByteSize {
    fn from(value: u64) -> Self {
        ByteSize(value)
    }
}

/// Errors that can occur when working with archives
#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),

    #[error("Invalid magic bytes")]
    InvalidMagic,

    #[error("Unsupported archive version: {0} (expected {1})")]
    UnsupportedVersion(u32, u32),

    #[error("Block not found at height {0:?}")]
    BlockNotFound(SolHeight),

    #[error("Archive is empty")]
    EmptyArchive,

    #[error("Metadata block count mismatch: declared {declared}, entries {actual}")]
    BlockCountMismatch { declared: u64, actual: usize },

    #[error("Invalid block height range: min {min:?} > max {max:?}")]
    InvalidHeightRange { min: SolHeight, max: SolHeight },

    #[error("Invalid slice range: start {start:?} > end {end:?}")]
    InvalidSliceRange { start: SolHeight, end: SolHeight },

    #[error("Slice range contains no blocks: {start:?}..={end:?}")]
    SliceRangeEmpty { start: SolHeight, end: SolHeight },

    #[error(
        "Block entry out of bounds: height {height:?}, offset {offset}, size {size}, section_len {section_len}"
    )]
    BlockEntryOutOfBounds {
        height: SolHeight,
        offset: ByteOffset,
        size: ByteSize,
        section_len: usize,
    },

    #[error(
        "Block entry overlaps or is out of order at height {height:?}: offset {offset}, prev_end {prev_end}"
    )]
    BlockEntryOutOfOrder {
        height: SolHeight,
        offset: ByteOffset,
        prev_end: ByteOffset,
    },

    #[error("Mempool snapshot count mismatch: declared {declared}, entries {actual}")]
    MempoolCountMismatch { declared: u64, actual: usize },

    #[error("Invalid mempool height range: min {min:?} > max {max:?}")]
    InvalidMempoolHeightRange { min: SolHeight, max: SolHeight },

    #[error(
        "Mempool entry out of bounds: height {height:?}, offset {offset}, size {size}, section_len {section_len}"
    )]
    MempoolEntryOutOfBounds {
        height: SolHeight,
        offset: ByteOffset,
        size: ByteSize,
        section_len: usize,
    },

    #[error(
        "Mempool entry overlaps or is out of order at height {height:?}: offset {offset}, prev_end {prev_end}"
    )]
    MempoolEntryOutOfOrder {
        height: SolHeight,
        offset: ByteOffset,
        prev_end: ByteOffset,
    },

    #[error("Offset too large for this platform: {offset}")]
    OffsetTooLarge { offset: u64 },

    #[error("Size too large for this platform: {size}")]
    SizeTooLarge { size: u64 },

    #[error("Range overflow: offset {offset}, size {size}")]
    RangeOverflow { offset: u64, size: u64 },

    #[error("Section size overflow for {section}")]
    SectionSizeOverflow { section: &'static str },

    #[error("Mempool metadata inconsistent: {0}")]
    MempoolMetadataInconsistent(String),

    #[error("Duplicate block height in archive metadata: {0:?}")]
    DuplicateBlockHeight(SolHeight),

    #[error(
        "Raw transaction count mismatch at height {height:?}: tx_count {tx_count}, raw_tx_count {raw_tx_count}"
    )]
    RawTxCountMismatch {
        height: SolHeight,
        tx_count: u64,
        raw_tx_count: u64,
    },

    #[error("Raw transaction count mismatch: declared {declared}, entries {actual}")]
    RawTxEntryCountMismatch { declared: u64, actual: usize },

    #[error(
        "Raw tx entry out of bounds: tx_id {tx_id:?}, offset {offset}, size {size}, section_len {section_len}"
    )]
    RawTxEntryOutOfBounds {
        tx_id: Hash,
        offset: ByteOffset,
        size: ByteSize,
        section_len: usize,
    },

    #[error("Raw tx entry overlaps or is out of order: offset {offset}, prev_end {prev_end}")]
    RawTxEntryOutOfOrder {
        offset: ByteOffset,
        prev_end: ByteOffset,
    },

    #[error(
        "Raw tx range out of bounds at height {height:?}: start {start}, count {count}, raw_tx_count {raw_tx_count}"
    )]
    RawTxRangeOutOfBounds {
        height: SolHeight,
        start: u64,
        count: u64,
        raw_tx_count: usize,
    },

    #[error("Unsupported archive operation: {0}")]
    UnsupportedOperation(&'static str),
}

impl ByteOffset {
    pub fn try_as_usize(self) -> Result<usize, ArchiveError> {
        usize::try_from(self.0).map_err(|_| ArchiveError::OffsetTooLarge { offset: self.0 })
    }
}

impl ByteSize {
    pub fn try_as_usize(self) -> Result<usize, ArchiveError> {
        usize::try_from(self.0).map_err(|_| ArchiveError::SizeTooLarge { size: self.0 })
    }
}

/// Metadata for a single block in the archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockEntry {
    /// Block height
    pub height: SolHeight,
    /// Block ID hash
    pub block_id: Hash,
    /// Number of transactions in this block
    pub tx_count: u64,
    /// Proof version for this block
    pub proof_version: ProofVersion,
    /// Offset into the jam blob section (bytes from start of blob section)
    pub jam_offset: ByteOffset,
    /// Size of the jammed noun blob in bytes
    pub jam_size: ByteSize,
    /// First raw transaction entry for this block.
    pub raw_tx_start: u64,
    /// Number of raw transaction entries for this block.
    pub raw_tx_count: u64,
}

/// Metadata for a raw transaction payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawTxEntry {
    /// Transaction ID
    pub tx_id: Hash,
    /// Offset into the raw transaction payload section.
    pub payload_offset: ByteOffset,
    /// Size of the jammed raw transaction payload.
    pub payload_size: ByteSize,
}

/// Borrowed raw transaction payload supplied to the archive writer.
#[derive(Debug, Clone)]
pub struct RawTxPayload<'a> {
    pub tx_id: Hash,
    pub jam_bytes: &'a [u8],
}

/// Mempool transaction entry for a snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MempoolTxEntry {
    /// Transaction ID
    pub tx_id: Hash,
    /// Block height when the tx was first heard
    pub heard_at: SolHeight,
}

/// Mempool snapshot metadata entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MempoolSnapshotEntry {
    /// Block height for this snapshot
    pub height: SolHeight,
    /// Number of transactions in the snapshot
    pub tx_count: u64,
    /// Offset into the mempool blob section (bytes from start of mempool section)
    pub blob_offset: ByteOffset,
    /// Size of the snapshot blob in bytes
    pub blob_size: ByteSize,
}

/// Archive header with raw transaction metadata and section sizes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveMetadata {
    /// Magic bytes for file identification
    pub magic: [u8; 8],
    /// Archive format version
    pub version: u32,
    /// Total number of blocks in archive
    pub block_count: u64,
    /// Total number of transactions across all blocks
    pub total_tx_count: u64,
    /// Minimum block height in archive
    pub min_height: SolHeight,
    /// Maximum block height in archive
    pub max_height: SolHeight,
    /// Hash of source checkpoint (for provenance tracking)
    pub source_checkpoint_hash: Option<Hash>,
    /// Whether mempool snapshots are included
    pub has_mempool: bool,
    /// Total number of mempool snapshots
    pub mempool_snapshot_count: u64,
    /// Minimum block height for mempool snapshots
    pub mempool_min_height: Option<SolHeight>,
    /// Maximum block height for mempool snapshots
    pub mempool_max_height: Option<SolHeight>,
    /// Raw transaction payload section is present for the current archive format.
    pub has_raw_txs: bool,
    /// Total number of raw transaction payloads.
    pub raw_tx_count: u64,
    /// Byte length of the block jam section.
    pub jam_section_size: ByteSize,
    /// Byte length of the mempool snapshot section.
    pub mempool_section_size: ByteSize,
    /// Byte length of the raw transaction payload section.
    pub raw_tx_payload_section_size: ByteSize,
    /// Block entries (index)
    pub blocks: Vec<BlockEntry>,
    /// Raw transaction entries (index)
    pub raw_txs: Vec<RawTxEntry>,
    /// Mempool snapshot entries (index)
    pub mempool_snapshots: Vec<MempoolSnapshotEntry>,
}

impl ArchiveMetadata {
    pub fn new() -> Self {
        Self {
            magic: *ARCHIVE_MAGIC,
            version: ARCHIVE_VERSION,
            block_count: 0,
            total_tx_count: 0,
            min_height: SolHeight::MAX,
            max_height: SolHeight::ZERO,
            source_checkpoint_hash: None,
            has_mempool: false,
            mempool_snapshot_count: 0,
            mempool_min_height: None,
            mempool_max_height: None,
            has_raw_txs: true,
            raw_tx_count: 0,
            jam_section_size: ByteSize(0),
            mempool_section_size: ByteSize(0),
            raw_tx_payload_section_size: ByteSize(0),
            blocks: Vec::new(),
            raw_txs: Vec::new(),
            mempool_snapshots: Vec::new(),
        }
    }

    pub fn with_source(source_hash: Hash) -> Self {
        let mut meta = Self::new();
        meta.source_checkpoint_hash = Some(source_hash);
        meta
    }

    pub fn add_block(&mut self, entry: BlockEntry) {
        if entry.height < self.min_height {
            self.min_height = entry.height;
        }
        if entry.height > self.max_height {
            self.max_height = entry.height;
        }
        self.total_tx_count = self.total_tx_count.saturating_add(entry.tx_count);
        self.block_count = self.block_count.saturating_add(1);
        self.blocks.push(entry);
    }

    pub fn add_raw_tx(&mut self, entry: RawTxEntry) {
        self.raw_tx_count = self.raw_tx_count.saturating_add(1);
        self.raw_txs.push(entry);
    }

    pub fn add_mempool_snapshot(&mut self, entry: MempoolSnapshotEntry) {
        self.has_mempool = true;
        self.mempool_snapshot_count += 1;

        match self.mempool_min_height {
            Some(min) if entry.height < min => self.mempool_min_height = Some(entry.height),
            None => self.mempool_min_height = Some(entry.height),
            _ => {}
        }

        match self.mempool_max_height {
            Some(max) if entry.height > max => self.mempool_max_height = Some(entry.height),
            None => self.mempool_max_height = Some(entry.height),
            _ => {}
        }

        self.mempool_snapshots.push(entry);
    }

    pub fn validate(&self) -> Result<(), ArchiveError> {
        if self.magic != *ARCHIVE_MAGIC {
            return Err(ArchiveError::InvalidMagic);
        }
        if self.version != ARCHIVE_VERSION {
            return Err(ArchiveError::UnsupportedVersion(
                self.version, ARCHIVE_VERSION,
            ));
        }
        Ok(())
    }

    pub fn validate_consistency(&self) -> Result<(), ArchiveError> {
        validate_block_metadata_common(
            self.block_count,
            self.min_height,
            self.max_height,
            self.blocks.iter().map(|entry| entry.height),
        )?;

        let mut heights = BTreeSet::new();
        for entry in &self.blocks {
            if !heights.insert(entry.height) {
                return Err(ArchiveError::DuplicateBlockHeight(entry.height));
            }
            if entry.tx_count != entry.raw_tx_count {
                return Err(ArchiveError::RawTxCountMismatch {
                    height: entry.height,
                    tx_count: entry.tx_count,
                    raw_tx_count: entry.raw_tx_count,
                });
            }
            let raw_tx_end = entry.raw_tx_start.checked_add(entry.raw_tx_count).ok_or(
                ArchiveError::RangeOverflow {
                    offset: entry.raw_tx_start,
                    size: entry.raw_tx_count,
                },
            )?;
            if raw_tx_end > self.raw_txs.len() as u64 {
                return Err(ArchiveError::RawTxRangeOutOfBounds {
                    height: entry.height,
                    start: entry.raw_tx_start,
                    count: entry.raw_tx_count,
                    raw_tx_count: self.raw_txs.len(),
                });
            }
        }

        if self.raw_tx_count != self.raw_txs.len() as u64 {
            return Err(ArchiveError::RawTxEntryCountMismatch {
                declared: self.raw_tx_count,
                actual: self.raw_txs.len(),
            });
        }
        if self.total_tx_count != self.raw_tx_count {
            return Err(ArchiveError::RawTxEntryCountMismatch {
                declared: self.total_tx_count,
                actual: self.raw_txs.len(),
            });
        }

        validate_mempool_metadata_consistency(
            self.has_mempool, self.mempool_snapshot_count, self.mempool_min_height,
            self.mempool_max_height, &self.mempool_snapshots,
        )
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ArchiveError> {
        Ok(bincode::serialize(self)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ArchiveError> {
        let meta: Self = bincode::deserialize(bytes)?;
        meta.validate()?;
        Ok(meta)
    }

    pub fn get_block(&self, height: SolHeight) -> Option<&BlockEntry> {
        self.blocks.iter().find(|entry| entry.height == height)
    }

    pub fn get_block_by_index(&self, index: usize) -> Option<&BlockEntry> {
        self.blocks.get(index)
    }

    pub fn get_mempool_snapshot(&self, height: SolHeight) -> Option<&MempoolSnapshotEntry> {
        self.mempool_snapshots
            .iter()
            .find(|snapshot| snapshot.height == height)
    }
}

impl Default for ArchiveMetadata {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_block_metadata_common(
    block_count: u64,
    min_height: SolHeight,
    max_height: SolHeight,
    heights: impl IntoIterator<Item = SolHeight>,
) -> Result<(), ArchiveError> {
    let heights: Vec<SolHeight> = heights.into_iter().collect();
    if block_count != heights.len() as u64 {
        return Err(ArchiveError::BlockCountMismatch {
            declared: block_count,
            actual: heights.len(),
        });
    }

    if heights.is_empty() {
        if min_height != SolHeight::MAX || max_height != SolHeight::ZERO {
            return Err(ArchiveError::InvalidHeightRange {
                min: min_height,
                max: max_height,
            });
        }
        return Ok(());
    }

    if min_height > max_height {
        return Err(ArchiveError::InvalidHeightRange {
            min: min_height,
            max: max_height,
        });
    }

    let actual_min = heights.iter().copied().min().unwrap_or(SolHeight::MAX);
    let actual_max = heights.iter().copied().max().unwrap_or(SolHeight::ZERO);
    if min_height != actual_min || max_height != actual_max {
        return Err(ArchiveError::InvalidHeightRange {
            min: actual_min,
            max: actual_max,
        });
    }

    Ok(())
}

fn validate_mempool_metadata_consistency(
    has_mempool: bool,
    mempool_snapshot_count: u64,
    mempool_min_height: Option<SolHeight>,
    mempool_max_height: Option<SolHeight>,
    snapshots: &[MempoolSnapshotEntry],
) -> Result<(), ArchiveError> {
    if mempool_snapshot_count != snapshots.len() as u64 {
        return Err(ArchiveError::MempoolCountMismatch {
            declared: mempool_snapshot_count,
            actual: snapshots.len(),
        });
    }

    if snapshots.is_empty() {
        if has_mempool || mempool_min_height.is_some() || mempool_max_height.is_some() {
            return Err(ArchiveError::MempoolMetadataInconsistent(
                "mempool flag/range set with no snapshots".to_string(),
            ));
        }
        return Ok(());
    }

    if !has_mempool {
        return Err(ArchiveError::MempoolMetadataInconsistent(
            "mempool snapshots present but has_mempool is false".to_string(),
        ));
    }

    let actual_min = snapshots
        .iter()
        .map(|entry| entry.height)
        .min()
        .unwrap_or(SolHeight::MAX);
    let actual_max = snapshots
        .iter()
        .map(|entry| entry.height)
        .max()
        .unwrap_or(SolHeight::ZERO);

    match (mempool_min_height, mempool_max_height) {
        (Some(min), Some(max)) if min <= max => {
            if min != actual_min || max != actual_max {
                return Err(ArchiveError::InvalidMempoolHeightRange {
                    min: actual_min,
                    max: actual_max,
                });
            }
        }
        _ => {
            return Err(ArchiveError::InvalidMempoolHeightRange {
                min: actual_min,
                max: actual_max,
            });
        }
    }

    Ok(())
}

/// Shared inspection view for archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInspect {
    pub block_count: u64,
    pub total_tx_count: u64,
    pub min_height: SolHeight,
    pub max_height: SolHeight,
    pub source_checkpoint_hash: Option<Hash>,
    pub has_mempool: bool,
    pub mempool_snapshot_count: u64,
}

/// Writer for creating speed-of-light archive files.
pub struct SolArchiveWriter {
    metadata: ArchiveMetadata,
    jam_blobs: Vec<u8>,
    mempool_blobs: Vec<u8>,
    raw_tx_payload_blobs: Vec<u8>,
}

impl SolArchiveWriter {
    pub fn new() -> Self {
        Self {
            metadata: ArchiveMetadata::new(),
            jam_blobs: Vec::new(),
            mempool_blobs: Vec::new(),
            raw_tx_payload_blobs: Vec::new(),
        }
    }

    pub fn with_source(source_hash: Hash) -> Self {
        Self {
            metadata: ArchiveMetadata::with_source(source_hash),
            jam_blobs: Vec::new(),
            mempool_blobs: Vec::new(),
            raw_tx_payload_blobs: Vec::new(),
        }
    }

    pub fn add_block_with_raw_txs<'a>(
        &mut self,
        height: SolHeight,
        block_id: Hash,
        proof_version: ProofVersion,
        jam_bytes: &[u8],
        raw_txs: impl IntoIterator<Item = RawTxPayload<'a>>,
    ) -> Result<(), ArchiveError> {
        let raw_txs: Vec<RawTxPayload<'a>> = raw_txs.into_iter().collect();
        let jam_offset = ByteOffset(self.jam_blobs.len() as u64);
        let jam_size = ByteSize(jam_bytes.len() as u64);
        let raw_tx_start = self.metadata.raw_txs.len() as u64;

        self.jam_blobs.extend_from_slice(jam_bytes);
        self.metadata.jam_section_size = ByteSize(self.jam_blobs.len() as u64);

        for raw_tx in &raw_txs {
            let payload_offset = ByteOffset(self.raw_tx_payload_blobs.len() as u64);
            let payload_size = ByteSize(raw_tx.jam_bytes.len() as u64);
            self.raw_tx_payload_blobs
                .extend_from_slice(raw_tx.jam_bytes);
            self.metadata.add_raw_tx(RawTxEntry {
                tx_id: raw_tx.tx_id.clone(),
                payload_offset,
                payload_size,
            });
        }
        self.metadata.raw_tx_payload_section_size =
            ByteSize(self.raw_tx_payload_blobs.len() as u64);

        self.metadata.add_block(BlockEntry {
            height,
            block_id,
            tx_count: raw_txs.len() as u64,
            proof_version,
            jam_offset,
            jam_size,
            raw_tx_start,
            raw_tx_count: raw_txs.len() as u64,
        });

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn add_block_with_tx_count_for_test(
        &mut self,
        height: SolHeight,
        block_id: Hash,
        tx_count: usize,
        proof_version: ProofVersion,
        jam_bytes: &[u8],
    ) -> Result<(), ArchiveError> {
        let raw_payloads: Vec<(Hash, Vec<u8>)> = (0..tx_count)
            .map(|idx| (block_id.clone(), vec![idx as u8]))
            .collect();
        let raw_txs = raw_payloads.iter().map(|(tx_id, jam_bytes)| RawTxPayload {
            tx_id: tx_id.clone(),
            jam_bytes,
        });
        self.add_block_with_raw_txs(height, block_id, proof_version, jam_bytes, raw_txs)
    }

    #[cfg(test)]
    pub(crate) fn jam_blob_size_for_test(&self) -> usize {
        self.jam_blobs.len()
    }

    pub fn add_mempool_snapshot(
        &mut self,
        height: SolHeight,
        txs: &[MempoolTxEntry],
    ) -> Result<(), ArchiveError> {
        let blob_offset = ByteOffset(self.mempool_blobs.len() as u64);
        let blob_bytes = bincode::serialize(txs)?;
        let blob_size = ByteSize(blob_bytes.len() as u64);

        self.mempool_blobs.extend_from_slice(&blob_bytes);
        self.metadata.mempool_section_size = ByteSize(self.mempool_blobs.len() as u64);

        self.metadata.add_mempool_snapshot(MempoolSnapshotEntry {
            height,
            tx_count: txs.len() as u64,
            blob_offset,
            blob_size,
        });

        Ok(())
    }

    pub fn block_count(&self) -> u64 {
        self.metadata.block_count
    }

    pub fn metadata(&self) -> &ArchiveMetadata {
        &self.metadata
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), ArchiveError> {
        let meta_bytes = self.metadata.to_bytes()?;
        let meta_len = meta_bytes.len() as u64;

        writer.write_all(&meta_len.to_le_bytes())?;
        writer.write_all(&meta_bytes)?;
        writer.write_all(&self.jam_blobs)?;
        writer.write_all(&self.mempool_blobs)?;
        writer.write_all(&self.raw_tx_payload_blobs)?;

        Ok(())
    }

    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), ArchiveError> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        self.write_to(&mut writer)?;
        writer.flush()?;

        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, ArchiveError> {
        let mut buffer = Vec::new();
        self.write_to(&mut buffer)?;
        Ok(buffer)
    }
}

impl Default for SolArchiveWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Reader for speed-of-light archive files
///
/// Provides access to archived blocks and their jammed noun bytes.
/// Supports both in-memory and memory-mapped access patterns.
///
/// # Example
/// ```ignore
/// let reader = SolArchiveReader::from_file("blocks.solar")?;
/// println!("Archive has {} blocks", reader.block_count());
///
/// // Get jam bytes for a specific block
/// let jam = reader.get_jam_by_height(SolHeight(5629))?;
///
/// // Iterate through all blocks
/// for (entry, jam_bytes) in reader.iter()? {
///     println!("Block {}: {} bytes", entry.height, jam_bytes.len());
/// }
/// ```
pub struct SolArchiveReader {
    body: ArchiveBody,
}

/// Parsed archive body.
pub struct ArchiveBody {
    metadata: ArchiveMetadata,
    jam_section: Vec<u8>,
    mempool_section: Vec<u8>,
    raw_tx_payload_section: Vec<u8>,
}

struct ArchiveLayout {
    jam_start: usize,
    jam_end: usize,
    mempool_start: usize,
    mempool_end: usize,
    raw_tx_payload_start: usize,
    raw_tx_payload_end: usize,
}

/// Filters for iterating archive entries
#[derive(Debug, Clone, Default)]
pub struct ArchiveFilter {
    pub proof_version: Option<ProofVersion>,
    pub start_height: Option<SolHeight>,
    pub end_height: Option<SolHeight>,
}

/// Summary of an archive slice operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSliceResult {
    /// First copied block height.
    pub start_height: SolHeight,
    /// Last copied block height.
    pub end_height: SolHeight,
    /// Number of copied blocks.
    pub block_count: u64,
    /// Number of copied mempool snapshots.
    pub mempool_snapshot_count: u64,
}

impl ArchiveFilter {
    pub fn matches(&self, entry: &BlockEntry) -> bool {
        if let Some(start) = self.start_height {
            if entry.height < start {
                return false;
            }
        }
        if let Some(end) = self.end_height {
            if entry.height > end {
                return false;
            }
        }
        if let Some(version) = self.proof_version {
            if entry.proof_version != version {
                return false;
            }
        }
        true
    }
}

impl SolArchiveReader {
    /// Open an archive from a file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ArchiveError> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(bytes)
    }

    /// Parse an archive from a byte buffer
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, ArchiveError> {
        Ok(Self {
            body: read_archive_body(&bytes)?,
        })
    }

    /// Get shared archive inspection metadata.
    pub fn inspect(&self) -> ArchiveInspect {
        ArchiveInspect {
            block_count: self.body.metadata.block_count,
            total_tx_count: self.body.metadata.total_tx_count,
            min_height: self.body.metadata.min_height,
            max_height: self.body.metadata.max_height,
            source_checkpoint_hash: self.body.metadata.source_checkpoint_hash.clone(),
            has_mempool: self.body.metadata.has_mempool,
            mempool_snapshot_count: self.body.metadata.mempool_snapshot_count,
        }
    }

    pub fn body(&self) -> &ArchiveBody {
        &self.body
    }

    pub fn metadata(&self) -> &ArchiveMetadata {
        &self.body.metadata
    }

    /// Get the number of blocks in the archive
    pub fn block_count(&self) -> u64 {
        self.inspect().block_count
    }

    /// Get the total number of transactions across all blocks
    pub fn total_tx_count(&self) -> u64 {
        self.inspect().total_tx_count
    }

    /// Get the minimum block height in the archive
    pub fn min_height(&self) -> SolHeight {
        self.inspect().min_height
    }

    /// Get the maximum block height in the archive
    pub fn max_height(&self) -> SolHeight {
        self.inspect().max_height
    }

    /// Whether the archive includes mempool snapshots
    pub fn has_mempool(&self) -> bool {
        self.inspect().has_mempool
    }

    /// Get total number of mempool snapshots
    pub fn mempool_snapshot_count(&self) -> u64 {
        self.inspect().mempool_snapshot_count
    }

    /// Get mempool snapshot metadata entries.
    pub fn mempool_snapshot_entries(&self) -> &[MempoolSnapshotEntry] {
        &self.body.metadata.mempool_snapshots
    }

    /// Get mempool snapshot by height
    pub fn get_mempool_snapshot(
        &self,
        height: SolHeight,
    ) -> Result<Option<Vec<MempoolTxEntry>>, ArchiveError> {
        let Some(entry) = self
            .body
            .metadata
            .mempool_snapshots
            .iter()
            .find(|s| s.height == height)
        else {
            return Ok(None);
        };
        let mempool_section = self.body.mempool_section.as_slice();

        let (start, end) = section_range(entry.blob_offset, entry.blob_size)?;

        if end > mempool_section.len() {
            return Err(ArchiveError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "mempool blob out of bounds: offset={}, size={}, section_len={}",
                    entry.blob_offset.as_u64(),
                    entry.blob_size.as_u64(),
                    mempool_section.len()
                ),
            )));
        }

        let bytes = &mempool_section[start..end];
        let snapshot: Vec<MempoolTxEntry> = bincode::deserialize(bytes)?;
        Ok(Some(snapshot))
    }

    /// Get jam bytes for a block by height
    pub fn get_jam_by_height(&self, height: SolHeight) -> Result<&[u8], ArchiveError> {
        let entry = self
            .body
            .get_entry_by_height(height)
            .ok_or(ArchiveError::BlockNotFound(height))?;
        self.body.get_jam_for_entry(entry)
    }

    /// Get jam bytes for a block by index
    pub fn get_jam_by_index(&self, index: usize) -> Result<&[u8], ArchiveError> {
        let entry = self
            .body
            .metadata
            .blocks
            .get(index)
            .ok_or(ArchiveError::BlockNotFound(SolHeight(index as u64)))?;
        self.body.get_jam_for_entry(entry)
    }

    pub fn get_entry_by_height(&self, height: SolHeight) -> Option<&BlockEntry> {
        self.body.get_entry_by_height(height)
    }

    pub fn get_entry_by_index(&self, index: usize) -> Option<&BlockEntry> {
        self.body.get_entry_by_index(index)
    }

    /// Iterate over all blocks in index order (which is insertion order)
    pub fn iter(&self) -> ArchiveIterator<'_> {
        ArchiveIterator {
            body: &self.body,
            index: 0,
        }
    }

    /// Iterate over blocks matching a filter
    pub fn iter_filtered(&self, filter: ArchiveFilter) -> impl Iterator<Item = &BlockEntry> {
        self.body.iter_filtered(filter)
    }
}

impl ArchiveBody {
    pub fn metadata(&self) -> &ArchiveMetadata {
        &self.metadata
    }

    pub fn get_entry_by_height(&self, height: SolHeight) -> Option<&BlockEntry> {
        self.metadata
            .blocks
            .iter()
            .find(|entry| entry.height == height)
    }

    pub fn get_entry_by_index(&self, index: usize) -> Option<&BlockEntry> {
        self.metadata.blocks.get(index)
    }

    pub fn get_jam_for_entry(&self, entry: &BlockEntry) -> Result<&[u8], ArchiveError> {
        get_jam_for_entry(self, entry)
    }

    pub fn raw_tx_entries_for_block(
        &self,
        entry: &BlockEntry,
    ) -> Result<&[RawTxEntry], ArchiveError> {
        let start =
            usize::try_from(entry.raw_tx_start).map_err(|_| ArchiveError::OffsetTooLarge {
                offset: entry.raw_tx_start,
            })?;
        let count =
            usize::try_from(entry.raw_tx_count).map_err(|_| ArchiveError::SizeTooLarge {
                size: entry.raw_tx_count,
            })?;
        let end = start
            .checked_add(count)
            .ok_or(ArchiveError::RangeOverflow {
                offset: entry.raw_tx_start,
                size: entry.raw_tx_count,
            })?;
        if end > self.metadata.raw_txs.len() {
            return Err(ArchiveError::RawTxRangeOutOfBounds {
                height: entry.height,
                start: entry.raw_tx_start,
                count: entry.raw_tx_count,
                raw_tx_count: self.metadata.raw_txs.len(),
            });
        }
        Ok(&self.metadata.raw_txs[start..end])
    }

    pub fn get_raw_tx_payload(&self, entry: &RawTxEntry) -> Result<&[u8], ArchiveError> {
        let (start, end) = section_range(entry.payload_offset, entry.payload_size)?;
        if end > self.raw_tx_payload_section.len() {
            return Err(ArchiveError::RawTxEntryOutOfBounds {
                tx_id: entry.tx_id.clone(),
                offset: entry.payload_offset,
                size: entry.payload_size,
                section_len: self.raw_tx_payload_section.len(),
            });
        }
        Ok(&self.raw_tx_payload_section[start..end])
    }

    pub fn iter_filtered(&self, filter: ArchiveFilter) -> impl Iterator<Item = &BlockEntry> {
        self.metadata
            .blocks
            .iter()
            .filter(move |entry| filter.matches(entry))
    }
}

/// Copy a block-height range from one archive into a new archive file.
///
/// The output archive includes all blocks in `start_height..=end_height` that
/// exist in the input archive. When `include_mempool` is true, mempool
/// snapshots for copied heights are included as well.
pub fn slice_archive_file<PIn, POut>(
    input_path: PIn,
    output_path: POut,
    start_height: SolHeight,
    end_height: SolHeight,
    include_mempool: bool,
) -> Result<ArchiveSliceResult, ArchiveError>
where
    PIn: AsRef<Path>,
    POut: AsRef<Path>,
{
    if start_height > end_height {
        return Err(ArchiveError::InvalidSliceRange {
            start: start_height,
            end: end_height,
        });
    }

    let reader = SolArchiveReader::from_file(input_path)?;
    let source_hash = reader.inspect().source_checkpoint_hash;
    let mut writer = match source_hash {
        Some(hash) => SolArchiveWriter::with_source(hash),
        None => SolArchiveWriter::new(),
    };
    let body = reader.body();

    let mut copied_block_count = 0u64;
    let mut copied_mempool_snapshot_count = 0u64;
    let mut copied_start = SolHeight::MAX;
    let mut copied_end = SolHeight::ZERO;

    for entry in body.iter_filtered(ArchiveFilter {
        proof_version: None,
        start_height: Some(start_height),
        end_height: Some(end_height),
    }) {
        let jam_bytes = body.get_jam_for_entry(entry)?;
        let raw_tx_entries = body.raw_tx_entries_for_block(entry)?;
        let mut raw_tx_payloads = Vec::with_capacity(raw_tx_entries.len());
        for raw_entry in raw_tx_entries {
            raw_tx_payloads.push(RawTxPayload {
                tx_id: raw_entry.tx_id.clone(),
                jam_bytes: body.get_raw_tx_payload(raw_entry)?,
            });
        }

        writer.add_block_with_raw_txs(
            entry.height,
            entry.block_id.clone(),
            entry.proof_version,
            jam_bytes,
            raw_tx_payloads,
        )?;
        copied_block_count = copied_block_count.saturating_add(1);
        copied_start = copied_start.min(entry.height);
        copied_end = copied_end.max(entry.height);
    }

    if copied_block_count == 0 {
        return Err(ArchiveError::SliceRangeEmpty {
            start: start_height,
            end: end_height,
        });
    }

    if include_mempool && reader.has_mempool() {
        for snapshot in &body.metadata.mempool_snapshots {
            if snapshot.height < start_height || snapshot.height > end_height {
                continue;
            }
            if let Some(txs) = reader.get_mempool_snapshot(snapshot.height)? {
                writer.add_mempool_snapshot(snapshot.height, &txs)?;
                copied_mempool_snapshot_count = copied_mempool_snapshot_count.saturating_add(1);
            }
        }
    }

    writer.write_to_file(output_path)?;

    Ok(ArchiveSliceResult {
        start_height: copied_start,
        end_height: copied_end,
        block_count: copied_block_count,
        mempool_snapshot_count: copied_mempool_snapshot_count,
    })
}

fn read_archive_body(bytes: &[u8]) -> Result<ArchiveBody, ArchiveError> {
    let (_, required_len) = parse_metadata_prefix(bytes)?;
    if bytes.len() < required_len {
        return Err(ArchiveError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "file too small for metadata",
        )));
    }

    let meta_bytes = &bytes[8..required_len];
    let version = metadata_archive_version(meta_bytes)?;
    if version != ARCHIVE_VERSION {
        return Err(ArchiveError::UnsupportedVersion(version, ARCHIVE_VERSION));
    }
    let metadata = ArchiveMetadata::from_bytes(meta_bytes)?;
    metadata.validate_consistency()?;
    let layout = compute_layout(bytes, &metadata)?;
    validate_block_entries(&metadata, layout.jam_end - layout.jam_start)?;
    validate_mempool_entries_for_section(
        &metadata.mempool_snapshots,
        layout.mempool_end - layout.mempool_start,
    )?;
    validate_raw_tx_entries(
        &metadata,
        layout.raw_tx_payload_end - layout.raw_tx_payload_start,
    )?;
    Ok(ArchiveBody {
        metadata,
        jam_section: bytes[layout.jam_start..layout.jam_end].to_vec(),
        mempool_section: bytes[layout.mempool_start..layout.mempool_end].to_vec(),
        raw_tx_payload_section: bytes[layout.raw_tx_payload_start..layout.raw_tx_payload_end]
            .to_vec(),
    })
}

fn metadata_archive_version(meta_bytes: &[u8]) -> Result<u32, ArchiveError> {
    if meta_bytes.len() < 12 {
        return Err(ArchiveError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "metadata too small for magic/version prefix",
        )));
    }
    if &meta_bytes[0..8] != ARCHIVE_MAGIC {
        return Err(ArchiveError::InvalidMagic);
    }
    Ok(u32::from_le_bytes(
        meta_bytes[8..12].try_into().expect("4-byte slice"),
    ))
}

fn compute_layout(bytes: &[u8], metadata: &ArchiveMetadata) -> Result<ArchiveLayout, ArchiveError> {
    let (meta_len, _) = parse_metadata_prefix(bytes)?;
    let jam_start = 8usize
        .checked_add(meta_len)
        .ok_or(ArchiveError::SectionSizeOverflow {
            section: "metadata",
        })?;
    let jam_len = metadata.jam_section_size.try_as_usize()?;
    let mempool_len = metadata.mempool_section_size.try_as_usize()?;
    let raw_tx_payload_len = metadata.raw_tx_payload_section_size.try_as_usize()?;

    let jam_end = jam_start
        .checked_add(jam_len)
        .ok_or(ArchiveError::SectionSizeOverflow { section: "jam" })?;
    let mempool_start = jam_end;
    let mempool_end = mempool_start
        .checked_add(mempool_len)
        .ok_or(ArchiveError::SectionSizeOverflow { section: "mempool" })?;
    let raw_tx_payload_start = mempool_end;
    let raw_tx_payload_end = raw_tx_payload_start.checked_add(raw_tx_payload_len).ok_or(
        ArchiveError::SectionSizeOverflow {
            section: "raw_tx_payload",
        },
    )?;

    if bytes.len() < raw_tx_payload_end {
        return Err(ArchiveError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "file too small for archive payload sections",
        )));
    }

    Ok(ArchiveLayout {
        jam_start,
        jam_end,
        mempool_start,
        mempool_end,
        raw_tx_payload_start,
        raw_tx_payload_end,
    })
}

fn parse_metadata_prefix(bytes: &[u8]) -> Result<(usize, usize), ArchiveError> {
    if bytes.len() < 8 {
        return Err(ArchiveError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "file too small for metadata length",
        )));
    }

    let meta_len_u64 = u64::from_le_bytes(bytes[0..8].try_into().expect("8-byte slice"));
    let meta_len = usize::try_from(meta_len_u64)
        .map_err(|_| ArchiveError::SizeTooLarge { size: meta_len_u64 })?;
    let required_len = 8usize
        .checked_add(meta_len)
        .ok_or(ArchiveError::SectionSizeOverflow {
            section: "metadata",
        })?;
    Ok((meta_len, required_len))
}

fn checked_entry_end(offset: ByteOffset, size: ByteSize) -> Result<ByteOffset, ArchiveError> {
    let end = offset
        .0
        .checked_add(size.0)
        .ok_or(ArchiveError::RangeOverflow {
            offset: offset.as_u64(),
            size: size.as_u64(),
        })?;
    Ok(ByteOffset(end))
}

fn section_range(offset: ByteOffset, size: ByteSize) -> Result<(usize, usize), ArchiveError> {
    let start = offset.try_as_usize()?;
    let section_size = size.try_as_usize()?;
    let end = start
        .checked_add(section_size)
        .ok_or(ArchiveError::RangeOverflow {
            offset: offset.as_u64(),
            size: size.as_u64(),
        })?;
    Ok((start, end))
}

fn validate_ordered_entries(
    entries: impl IntoIterator<Item = (SolHeight, ByteOffset, ByteSize)>,
    section_len: usize,
    out_of_order: impl Fn(SolHeight, ByteOffset, ByteOffset) -> ArchiveError,
    out_of_bounds: impl Fn(SolHeight, ByteOffset, ByteSize, usize) -> ArchiveError,
) -> Result<(), ArchiveError> {
    let mut prev_end = ByteOffset(0);
    for (height, offset, size) in entries {
        let end = checked_entry_end(offset, size)?;
        if offset < prev_end {
            return Err(out_of_order(height, offset, prev_end));
        }
        if end.try_as_usize()? > section_len {
            return Err(out_of_bounds(height, offset, size, section_len));
        }
        prev_end = end;
    }
    Ok(())
}

fn validate_block_entries(
    metadata: &ArchiveMetadata,
    jam_section_len: usize,
) -> Result<(), ArchiveError> {
    validate_ordered_entries(
        metadata
            .blocks
            .iter()
            .map(|entry| (entry.height, entry.jam_offset, entry.jam_size)),
        jam_section_len,
        |height, offset, prev_end| ArchiveError::BlockEntryOutOfOrder {
            height,
            offset,
            prev_end,
        },
        |height, offset, size, section_len| ArchiveError::BlockEntryOutOfBounds {
            height,
            offset,
            size,
            section_len,
        },
    )
}

fn validate_mempool_entries_for_section(
    snapshots: &[MempoolSnapshotEntry],
    mempool_section_len: usize,
) -> Result<(), ArchiveError> {
    if snapshots.is_empty() {
        return Ok(());
    }

    validate_ordered_entries(
        snapshots
            .iter()
            .map(|entry| (entry.height, entry.blob_offset, entry.blob_size)),
        mempool_section_len,
        |height, offset, prev_end| ArchiveError::MempoolEntryOutOfOrder {
            height,
            offset,
            prev_end,
        },
        |height, offset, size, section_len| ArchiveError::MempoolEntryOutOfBounds {
            height,
            offset,
            size,
            section_len,
        },
    )
}

fn validate_raw_tx_entries(
    metadata: &ArchiveMetadata,
    raw_tx_payload_section_len: usize,
) -> Result<(), ArchiveError> {
    let mut prev_end = ByteOffset(0);
    for entry in &metadata.raw_txs {
        let end = checked_entry_end(entry.payload_offset, entry.payload_size)?;
        if entry.payload_offset < prev_end {
            return Err(ArchiveError::RawTxEntryOutOfOrder {
                offset: entry.payload_offset,
                prev_end,
            });
        }
        if end.try_as_usize()? > raw_tx_payload_section_len {
            return Err(ArchiveError::RawTxEntryOutOfBounds {
                tx_id: entry.tx_id.clone(),
                offset: entry.payload_offset,
                size: entry.payload_size,
                section_len: raw_tx_payload_section_len,
            });
        }
        prev_end = end;
    }
    Ok(())
}

fn get_jam_for_entry<'a>(
    body: &'a ArchiveBody,
    entry: &BlockEntry,
) -> Result<&'a [u8], ArchiveError> {
    let (start, end) = section_range(entry.jam_offset, entry.jam_size)?;

    if end > body.jam_section.len() {
        return Err(ArchiveError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "jam blob out of bounds: offset={}, size={}, section_len={}",
                entry.jam_offset.as_u64(),
                entry.jam_size.as_u64(),
                body.jam_section.len()
            ),
        )));
    }

    Ok(&body.jam_section[start..end])
}

/// Iterator over all blocks in an archive (by index order)
pub struct ArchiveIterator<'a> {
    body: &'a ArchiveBody,
    index: usize,
}

impl<'a> Iterator for ArchiveIterator<'a> {
    type Item = (&'a BlockEntry, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.body.metadata.get_block_by_index(self.index)?;
        let jam = get_jam_for_entry(self.body, entry).ok()?;
        self.index += 1;
        Some((entry, jam))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.body.metadata.blocks.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl<'a> ExactSizeIterator for ArchiveIterator<'a> {}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;

    use super::*;
    use crate::speed_of_light::{PROOF_VERSION_1_START, PROOF_VERSION_2_START};

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    fn proof_for_height(height: SolHeight) -> ProofVersion {
        ProofVersion::for_height(height)
    }

    /// Test that ArchiveMetadata serializes and deserializes correctly
    #[test]
    fn test_archive_metadata_roundtrip() {
        let mut meta = ArchiveMetadata::new();
        meta.source_checkpoint_hash = Some(dummy_hash(12345));

        // Add some block entries
        meta.add_block(BlockEntry {
            height: SolHeight(0),
            block_id: dummy_hash(100),
            tx_count: 0,
            proof_version: proof_for_height(SolHeight(0)),
            jam_offset: ByteOffset(0),
            jam_size: ByteSize(1024),
            raw_tx_start: 0,
            raw_tx_count: 0,
        });
        meta.add_block(BlockEntry {
            height: SolHeight(1),
            block_id: dummy_hash(101),
            tx_count: 2,
            proof_version: proof_for_height(SolHeight(1)),
            jam_offset: ByteOffset(1024),
            jam_size: ByteSize(2048),
            raw_tx_start: 0,
            raw_tx_count: 2,
        });
        meta.add_block(BlockEntry {
            height: SolHeight(2),
            block_id: dummy_hash(102),
            tx_count: 1,
            proof_version: proof_for_height(SolHeight(2)),
            jam_offset: ByteOffset(3072),
            jam_size: ByteSize(512),
            raw_tx_start: 2,
            raw_tx_count: 1,
        });

        // Serialize
        let bytes = meta.to_bytes().expect("serialization should succeed");

        // Deserialize
        let restored = ArchiveMetadata::from_bytes(&bytes).expect("deserialization should succeed");

        // Verify
        assert_eq!(meta, restored);
        assert_eq!(restored.block_count, 3);
        assert_eq!(restored.total_tx_count, 3);
        assert_eq!(restored.min_height, SolHeight(0));
        assert_eq!(restored.max_height, SolHeight(2));
        assert_eq!(restored.blocks.len(), 3);
    }

    /// Test that BlockEntry serializes and deserializes correctly
    #[test]
    fn test_block_entry_roundtrip() {
        let entry = BlockEntry {
            height: SolHeight(5629),
            block_id: dummy_hash(999),
            tx_count: 5,
            proof_version: proof_for_height(SolHeight(5629)),
            jam_offset: ByteOffset(123456),
            jam_size: ByteSize(7890),
            raw_tx_start: 0,
            raw_tx_count: 5,
        };

        let bytes = bincode::serialize(&entry).expect("serialization should succeed");
        let restored: BlockEntry =
            bincode::deserialize(&bytes).expect("deserialization should succeed");

        assert_eq!(entry, restored);
        assert_eq!(restored.height, SolHeight(5629));
        assert_eq!(restored.tx_count, 5);
        assert_eq!(restored.jam_offset, ByteOffset(123456));
        assert_eq!(restored.jam_size, ByteSize(7890));
    }

    /// Test that version validation works correctly
    #[test]
    fn test_archive_version_check() {
        // Valid metadata should pass validation
        let valid_meta = ArchiveMetadata::new();
        assert!(valid_meta.validate().is_ok());

        // Invalid magic should fail
        let mut bad_magic = ArchiveMetadata::new();
        bad_magic.magic = *b"BADMAGIC";
        assert!(matches!(
            bad_magic.validate(),
            Err(ArchiveError::InvalidMagic)
        ));

        // Invalid version should fail
        let mut bad_version = ArchiveMetadata::new();
        bad_version.version = 999;
        assert!(matches!(
            bad_version.validate(),
            Err(ArchiveError::UnsupportedVersion(999, ARCHIVE_VERSION))
        ));
    }

    /// Test metadata stats are updated correctly when adding blocks
    #[test]
    fn test_archive_metadata_stats() {
        let mut meta = ArchiveMetadata::new();

        // Initially empty
        assert_eq!(meta.block_count, 0);
        assert_eq!(meta.total_tx_count, 0);

        // Add blocks out of order to test min/max tracking
        meta.add_block(BlockEntry {
            height: SolHeight(100),
            block_id: dummy_hash(1),
            tx_count: 3,
            proof_version: proof_for_height(SolHeight(100)),
            jam_offset: ByteOffset(0),
            jam_size: ByteSize(100),
            raw_tx_start: 0,
            raw_tx_count: 3,
        });
        assert_eq!(meta.min_height, SolHeight(100));
        assert_eq!(meta.max_height, SolHeight(100));

        meta.add_block(BlockEntry {
            height: SolHeight(50),
            block_id: dummy_hash(2),
            tx_count: 2,
            proof_version: proof_for_height(SolHeight(50)),
            jam_offset: ByteOffset(100),
            jam_size: ByteSize(100),
            raw_tx_start: 3,
            raw_tx_count: 2,
        });
        assert_eq!(meta.min_height, SolHeight(50));
        assert_eq!(meta.max_height, SolHeight(100));

        meta.add_block(BlockEntry {
            height: SolHeight(200),
            block_id: dummy_hash(3),
            tx_count: 5,
            proof_version: proof_for_height(SolHeight(200)),
            jam_offset: ByteOffset(200),
            jam_size: ByteSize(100),
            raw_tx_start: 5,
            raw_tx_count: 5,
        });
        assert_eq!(meta.min_height, SolHeight(50));
        assert_eq!(meta.max_height, SolHeight(200));

        assert_eq!(meta.block_count, 3);
        assert_eq!(meta.total_tx_count, 10); // 3 + 2 + 5
    }

    /// Test block lookup methods
    #[test]
    fn test_archive_block_lookup() {
        let mut meta = ArchiveMetadata::new();

        meta.add_block(BlockEntry {
            height: SolHeight(0),
            block_id: dummy_hash(100),
            tx_count: 0,
            proof_version: proof_for_height(SolHeight(0)),
            jam_offset: ByteOffset(0),
            jam_size: ByteSize(100),
            raw_tx_start: 0,
            raw_tx_count: 0,
        });
        meta.add_block(BlockEntry {
            height: SolHeight(5),
            block_id: dummy_hash(105),
            tx_count: 1,
            proof_version: proof_for_height(SolHeight(5)),
            jam_offset: ByteOffset(100),
            jam_size: ByteSize(200),
            raw_tx_start: 0,
            raw_tx_count: 1,
        });
        meta.add_block(BlockEntry {
            height: SolHeight(10),
            block_id: dummy_hash(110),
            tx_count: 2,
            proof_version: proof_for_height(SolHeight(10)),
            jam_offset: ByteOffset(300),
            jam_size: ByteSize(150),
            raw_tx_start: 1,
            raw_tx_count: 2,
        });

        // Lookup by height
        let block_5 = meta.get_block(SolHeight(5)).expect("block 5 should exist");
        assert_eq!(block_5.height, SolHeight(5));
        assert_eq!(block_5.tx_count, 1);

        // Missing height returns None
        assert!(meta.get_block(SolHeight(3)).is_none());
        assert!(meta.get_block(SolHeight(100)).is_none());

        // Lookup by index
        let block_idx_1 = meta.get_block_by_index(1).expect("index 1 should exist");
        assert_eq!(block_idx_1.height, SolHeight(5));

        // Out of bounds returns None
        assert!(meta.get_block_by_index(100).is_none());
    }

    // ==================== SolArchiveWriter Tests ====================

    /// Test writing an empty archive
    #[test]
    fn test_archive_writer_empty() {
        let writer = SolArchiveWriter::new();

        assert_eq!(writer.block_count(), 0);
        assert_eq!(writer.jam_blob_size_for_test(), 0);

        let bytes = writer.to_bytes().expect("should serialize empty archive");

        // Should have at least the metadata length prefix (8 bytes) + metadata
        assert!(bytes.len() >= 8);

        // Verify the metadata length prefix
        let meta_len = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        assert!(meta_len > 0, "metadata should have non-zero length");

        // Verify we can parse the metadata
        let meta_bytes = &bytes[8..8 + meta_len as usize];
        let meta = ArchiveMetadata::from_bytes(meta_bytes).expect("should parse metadata");
        assert_eq!(meta.block_count, 0);
    }

    #[test]
    fn test_archive_roundtrips_raw_tx_payloads_and_inspect_view() {
        let mut writer = SolArchiveWriter::with_source(dummy_hash(42));
        let block_jam = vec![0x10, 0x20, 0x30, 0x40];
        let raw_tx_a = vec![0xAA, 0xBB, 0xCC];
        let raw_tx_b = vec![0xDD, 0xEE];

        writer
            .add_block_with_raw_txs(
                SolHeight(7),
                dummy_hash(700),
                proof_for_height(SolHeight(7)),
                &block_jam,
                [
                    RawTxPayload {
                        tx_id: dummy_hash(701),
                        jam_bytes: &raw_tx_a,
                    },
                    RawTxPayload {
                        tx_id: dummy_hash(702),
                        jam_bytes: &raw_tx_b,
                    },
                ],
            )
            .expect("archive block should be accepted");
        writer
            .add_mempool_snapshot(
                SolHeight(7),
                &[MempoolTxEntry {
                    tx_id: dummy_hash(800),
                    heard_at: SolHeight(6),
                }],
            )
            .expect("mempool snapshot should be accepted");

        let reader =
            SolArchiveReader::from_bytes(writer.to_bytes().expect("archive should serialize"))
                .expect("archive should parse");

        let inspect = reader.inspect();
        assert_eq!(inspect.block_count, 1);
        assert_eq!(inspect.total_tx_count, 2);
        assert_eq!(inspect.source_checkpoint_hash, Some(dummy_hash(42)));
        assert!(inspect.has_mempool);
        assert_eq!(inspect.mempool_snapshot_count, 1);
        assert_eq!(reader.mempool_snapshot_entries().len(), 1);
        assert_eq!(reader.mempool_snapshot_entries()[0].height, SolHeight(7));

        let body = reader.body();
        let entry = body
            .get_entry_by_height(SolHeight(7))
            .expect("block entry should exist");
        assert_eq!(entry.tx_count, 2);
        assert_eq!(entry.raw_tx_count, 2);
        assert_eq!(
            body.get_jam_for_entry(entry).expect("jam should exist"),
            block_jam
        );

        let raw_entries = body
            .raw_tx_entries_for_block(entry)
            .expect("raw tx entries should exist");
        assert_eq!(raw_entries.len(), 2);
        assert_eq!(raw_entries[0].tx_id, dummy_hash(701));
        assert_eq!(raw_entries[1].tx_id, dummy_hash(702));
        assert_eq!(
            body.get_raw_tx_payload(&raw_entries[0])
                .expect("first payload should exist"),
            raw_tx_a
        );
        assert_eq!(
            body.get_raw_tx_payload(&raw_entries[1])
                .expect("second payload should exist"),
            raw_tx_b
        );

        let mempool = reader
            .get_mempool_snapshot(SolHeight(7))
            .expect("mempool read should succeed")
            .expect("mempool snapshot should exist");
        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool[0].tx_id, dummy_hash(800));
    }

    #[test]
    fn test_metadata_rejects_duplicate_heights_and_tx_count_mismatch() {
        let mut duplicate = ArchiveMetadata::new();
        duplicate.add_block(BlockEntry {
            height: SolHeight(3),
            block_id: dummy_hash(300),
            tx_count: 0,
            proof_version: proof_for_height(SolHeight(3)),
            jam_offset: ByteOffset(0),
            jam_size: ByteSize(4),
            raw_tx_start: 0,
            raw_tx_count: 0,
        });
        duplicate.add_block(BlockEntry {
            height: SolHeight(3),
            block_id: dummy_hash(301),
            tx_count: 0,
            proof_version: proof_for_height(SolHeight(3)),
            jam_offset: ByteOffset(4),
            jam_size: ByteSize(4),
            raw_tx_start: 0,
            raw_tx_count: 0,
        });
        assert!(matches!(
            duplicate.validate_consistency(),
            Err(ArchiveError::DuplicateBlockHeight(SolHeight(3)))
        ));

        let mut mismatch = ArchiveMetadata::new();
        mismatch.add_block(BlockEntry {
            height: SolHeight(4),
            block_id: dummy_hash(400),
            tx_count: 2,
            proof_version: proof_for_height(SolHeight(4)),
            jam_offset: ByteOffset(0),
            jam_size: ByteSize(4),
            raw_tx_start: 0,
            raw_tx_count: 1,
        });
        assert!(matches!(
            mismatch.validate_consistency(),
            Err(ArchiveError::RawTxCountMismatch {
                height: SolHeight(4),
                tx_count: 2,
                raw_tx_count: 1,
            })
        ));
    }

    #[test]
    fn test_archive_reader_remains_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<SolArchiveReader>();
    }

    /// Test writing a single block
    #[test]
    fn test_archive_writer_single_block() {
        let mut writer = SolArchiveWriter::new();

        // Add a block with some dummy jam bytes
        let jam_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(100),
                2,
                proof_for_height(SolHeight(0)),
                &jam_bytes,
            )
            .expect("should add block");

        assert_eq!(writer.block_count(), 1);
        assert_eq!(writer.jam_blob_size_for_test(), 8);

        let bytes = writer.to_bytes().expect("should serialize archive");

        // Parse the archive
        let meta_len = u64::from_le_bytes(bytes[0..8].try_into().unwrap()) as usize;
        let meta_bytes = &bytes[8..8 + meta_len];
        let meta = ArchiveMetadata::from_bytes(meta_bytes).expect("should parse metadata");
        let jam_start = 8 + meta_len;
        let jam_end = jam_start + meta.jam_section_size.as_usize();
        let jam_section = &bytes[jam_start..jam_end];
        assert_eq!(meta.block_count, 1);
        assert_eq!(meta.blocks[0].height, SolHeight(0));
        assert_eq!(meta.blocks[0].tx_count, 2);
        assert_eq!(meta.blocks[0].jam_offset, ByteOffset(0));
        assert_eq!(meta.blocks[0].jam_size, ByteSize(8));

        // Verify jam bytes
        assert_eq!(jam_section, &jam_bytes);
    }

    /// Test writing multiple blocks and verify offsets
    #[test]
    fn test_archive_writer_multiple_blocks() {
        let mut writer = SolArchiveWriter::new();

        // Add three blocks with different sizes
        let jam_0 = vec![0x00; 100]; // 100 bytes
        let jam_1 = vec![0x11; 250]; // 250 bytes
        let jam_2 = vec![0x22; 50]; // 50 bytes

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(100),
                0,
                proof_for_height(SolHeight(0)),
                &jam_0,
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(1),
                dummy_hash(101),
                3,
                proof_for_height(SolHeight(1)),
                &jam_1,
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(2),
                dummy_hash(102),
                1,
                proof_for_height(SolHeight(2)),
                &jam_2,
            )
            .unwrap();

        assert_eq!(writer.block_count(), 3);
        assert_eq!(writer.jam_blob_size_for_test(), 400); // 100 + 250 + 50

        let bytes = writer.to_bytes().expect("should serialize archive");

        // Parse the archive
        let meta_len = u64::from_le_bytes(bytes[0..8].try_into().unwrap()) as usize;
        let meta_bytes = &bytes[8..8 + meta_len];
        let jam_section = &bytes[8 + meta_len..];

        let meta = ArchiveMetadata::from_bytes(meta_bytes).expect("should parse metadata");

        // Verify block entries have correct offsets
        assert_eq!(meta.blocks[0].jam_offset, ByteOffset(0));
        assert_eq!(meta.blocks[0].jam_size, ByteSize(100));

        assert_eq!(meta.blocks[1].jam_offset, ByteOffset(100));
        assert_eq!(meta.blocks[1].jam_size, ByteSize(250));

        assert_eq!(meta.blocks[2].jam_offset, ByteOffset(350));
        assert_eq!(meta.blocks[2].jam_size, ByteSize(50));

        // Verify we can extract each block's jam bytes using the offsets
        for entry in &meta.blocks {
            let start = entry.jam_offset.as_usize();
            let end = start + entry.jam_size.as_usize();
            let block_jam = &jam_section[start..end];

            // Verify content matches what we wrote
            let expected_byte = match entry.height {
                SolHeight(0) => 0x00,
                SolHeight(1) => 0x11,
                SolHeight(2) => 0x22,
                _ => panic!("unexpected height"),
            };
            assert!(block_jam.iter().all(|&b| b == expected_byte));
        }
    }

    /// Test that archive file can be written and structure is valid
    #[test]
    fn test_archive_writer_file_structure() {
        let mut writer = SolArchiveWriter::with_source(dummy_hash(99999));

        // Add some blocks
        for i in 0u64..5 {
            let jam = vec![i as u8; (i as usize + 1) * 100];
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(i),
                    dummy_hash(i),
                    i as usize,
                    proof_for_height(SolHeight(i)),
                    &jam,
                )
                .unwrap();
        }

        // Write to a temp file
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join("test_archive.solar");

        writer.write_to_file(&temp_path).expect("should write file");

        // Read the file back
        let file_bytes = std::fs::read(&temp_path).expect("should read file");

        // Clean up
        std::fs::remove_file(&temp_path).ok();

        // Parse and verify
        let meta_len = u64::from_le_bytes(file_bytes[0..8].try_into().unwrap()) as usize;
        let meta_bytes = &file_bytes[8..8 + meta_len];
        let meta = ArchiveMetadata::from_bytes(meta_bytes).expect("should parse metadata");
        let jam_start = 8 + meta_len;
        let jam_end = jam_start + meta.jam_section_size.as_usize();
        let jam_section = &file_bytes[jam_start..jam_end];

        assert_eq!(meta.block_count, 5);
        assert_eq!(meta.min_height, SolHeight(0));
        assert_eq!(meta.max_height, SolHeight(4));
        assert_eq!(meta.total_tx_count, 0 + 1 + 2 + 3 + 4);
        assert!(meta.source_checkpoint_hash.is_some());

        // Verify total jam size: 100 + 200 + 300 + 400 + 500 = 1500
        let expected_jam_size: u64 = (1..=5).map(|i| i * 100).sum();
        assert_eq!(jam_section.len() as u64, expected_jam_size);
    }

    // ==================== SolArchiveReader Tests ====================

    /// Test parsing an archive from bytes
    #[test]
    fn test_archive_reader_from_bytes() {
        // Create an archive with the writer
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(100),
                0,
                proof_for_height(SolHeight(0)),
                &[0x00; 50],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(1),
                dummy_hash(101),
                2,
                proof_for_height(SolHeight(1)),
                &[0x11; 100],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(2),
                dummy_hash(102),
                1,
                proof_for_height(SolHeight(2)),
                &[0x22; 75],
            )
            .unwrap();

        let bytes = writer.to_bytes().unwrap();

        // Parse with reader
        let reader = SolArchiveReader::from_bytes(bytes).expect("should parse archive");

        assert_eq!(reader.block_count(), 3);
        assert_eq!(reader.total_tx_count(), 3);
        assert_eq!(reader.min_height(), SolHeight(0));
        assert_eq!(reader.max_height(), SolHeight(2));
    }

    /// Test getting jam bytes by height
    #[test]
    fn test_archive_reader_get_jam_by_height() {
        let mut writer = SolArchiveWriter::new();

        // Add blocks with distinctive content
        let jam_0 = vec![0xAA; 100];
        let jam_5 = vec![0xBB; 200];
        let jam_10 = vec![0xCC; 150];

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(100),
                0,
                proof_for_height(SolHeight(0)),
                &jam_0,
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(5),
                dummy_hash(105),
                1,
                proof_for_height(SolHeight(5)),
                &jam_5,
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(10),
                dummy_hash(110),
                2,
                proof_for_height(SolHeight(10)),
                &jam_10,
            )
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        // Get jam by height
        let retrieved_0 = reader
            .get_jam_by_height(SolHeight(0))
            .expect("block 0 should exist");
        assert_eq!(retrieved_0.len(), 100);
        assert!(retrieved_0.iter().all(|&b| b == 0xAA));

        let retrieved_5 = reader
            .get_jam_by_height(SolHeight(5))
            .expect("block 5 should exist");
        assert_eq!(retrieved_5.len(), 200);
        assert!(retrieved_5.iter().all(|&b| b == 0xBB));

        let retrieved_10 = reader
            .get_jam_by_height(SolHeight(10))
            .expect("block 10 should exist");
        assert_eq!(retrieved_10.len(), 150);
        assert!(retrieved_10.iter().all(|&b| b == 0xCC));

        // Missing height should error
        assert!(matches!(
            reader.get_jam_by_height(SolHeight(3)),
            Err(ArchiveError::BlockNotFound(SolHeight(3)))
        ));
    }

    /// Test getting jam bytes by index
    #[test]
    fn test_archive_reader_get_jam_by_index() {
        let mut writer = SolArchiveWriter::new();

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(100),
                dummy_hash(100),
                0,
                proof_for_height(SolHeight(100)),
                &[0x11; 50],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(200),
                dummy_hash(200),
                0,
                proof_for_height(SolHeight(200)),
                &[0x22; 60],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(300),
                dummy_hash(300),
                0,
                proof_for_height(SolHeight(300)),
                &[0x33; 70],
            )
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        // Index 0 = first block added (height 100)
        let (entry_0, jam_0) = (
            reader.get_entry_by_index(0).unwrap(),
            reader.get_jam_by_index(0).unwrap(),
        );
        assert_eq!(entry_0.height, SolHeight(100));
        assert_eq!(jam_0.len(), 50);

        // Index 1 = second block added (height 200)
        let (entry_1, jam_1) = (
            reader.get_entry_by_index(1).unwrap(),
            reader.get_jam_by_index(1).unwrap(),
        );
        assert_eq!(entry_1.height, SolHeight(200));
        assert_eq!(jam_1.len(), 60);

        // Index 2 = third block added (height 300)
        let (entry_2, jam_2) = (
            reader.get_entry_by_index(2).unwrap(),
            reader.get_jam_by_index(2).unwrap(),
        );
        assert_eq!(entry_2.height, SolHeight(300));
        assert_eq!(jam_2.len(), 70);

        // Out of bounds
        assert!(reader.get_jam_by_index(99).is_err());
    }

    /// Test full roundtrip: write with Writer, read with Reader
    #[test]
    fn test_archive_reader_roundtrip() {
        let mut writer = SolArchiveWriter::with_source(dummy_hash(99999));

        // Add blocks with varied content
        let test_data: Vec<(SolHeight, Vec<u8>)> = vec![
            (SolHeight(0), (0..100).collect()),
            (SolHeight(1), (100..250).cycle().take(150).collect()),
            (SolHeight(5), vec![0xFF; 200]),
            (SolHeight(10), vec![0x42; 50]),
        ];

        for (height, jam) in &test_data {
            let h = height.as_u64();
            writer
                .add_block_with_tx_count_for_test(
                    *height,
                    dummy_hash(h),
                    h as usize,
                    proof_for_height(*height),
                    jam,
                )
                .unwrap();
        }

        // Write to bytes and read back
        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        // Verify metadata
        assert_eq!(reader.block_count(), 4);
        assert_eq!(reader.min_height(), SolHeight(0));
        assert_eq!(reader.max_height(), SolHeight(10));
        assert!(reader.metadata().source_checkpoint_hash.is_some());

        // Verify each block's content matches
        for (height, expected_jam) in &test_data {
            let actual_jam = reader.get_jam_by_height(*height).unwrap();
            assert_eq!(
                actual_jam,
                expected_jam.as_slice(),
                "jam mismatch at height {:?}",
                height
            );
        }
    }

    /// Test iterating through all blocks
    #[test]
    fn test_archive_reader_iterate() {
        let mut writer = SolArchiveWriter::new();

        // Add blocks (note: not in height order to test iteration order)
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(5),
                dummy_hash(5),
                1,
                proof_for_height(SolHeight(5)),
                &[0x55; 50],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(2),
                dummy_hash(2),
                0,
                proof_for_height(SolHeight(2)),
                &[0x22; 20],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(8),
                dummy_hash(8),
                3,
                proof_for_height(SolHeight(8)),
                &[0x88; 80],
            )
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        // Iterate - should be in insertion order (5, 2, 8)
        let entries: Vec<_> = reader.iter().collect();
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].0.height, SolHeight(5));
        assert_eq!(entries[0].1.len(), 50);

        assert_eq!(entries[1].0.height, SolHeight(2));
        assert_eq!(entries[1].1.len(), 20);

        assert_eq!(entries[2].0.height, SolHeight(8));
        assert_eq!(entries[2].1.len(), 80);

        // Test ExactSizeIterator
        assert_eq!(reader.iter().len(), 3);
    }

    /// Test mempool snapshot roundtrip
    #[test]
    fn test_archive_reader_mempool_snapshots_roundtrip() {
        let mut writer = SolArchiveWriter::new();

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(0),
                0,
                proof_for_height(SolHeight(0)),
                &[0xAA; 10],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(1),
                dummy_hash(1),
                0,
                proof_for_height(SolHeight(1)),
                &[0xBB; 12],
            )
            .unwrap();

        let snapshot_0 = vec![
            MempoolTxEntry {
                tx_id: dummy_hash(10),
                heard_at: SolHeight(0),
            },
            MempoolTxEntry {
                tx_id: dummy_hash(11),
                heard_at: SolHeight(0),
            },
        ];
        let snapshot_1: Vec<MempoolTxEntry> = Vec::new();

        writer
            .add_mempool_snapshot(SolHeight(0), &snapshot_0)
            .unwrap();
        writer
            .add_mempool_snapshot(SolHeight(1), &snapshot_1)
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        assert!(reader.has_mempool());
        assert_eq!(reader.mempool_snapshot_count(), 2);
        assert_eq!(reader.metadata().mempool_min_height, Some(SolHeight(0)));
        assert_eq!(reader.metadata().mempool_max_height, Some(SolHeight(1)));

        let restored_0 = reader
            .get_mempool_snapshot(SolHeight(0))
            .expect("mempool snapshot should decode")
            .expect("snapshot height 0 should exist");
        assert_eq!(restored_0, snapshot_0);

        let restored_1 = reader
            .get_mempool_snapshot(SolHeight(1))
            .expect("mempool snapshot should decode")
            .expect("snapshot height 1 should exist");
        assert!(restored_1.is_empty());

        assert!(reader
            .get_mempool_snapshot(SolHeight(2))
            .expect("lookup should succeed")
            .is_none());
    }

    /// Test jam access remains correct with mempool snapshots present
    #[test]
    fn test_archive_reader_jam_with_mempool_snapshots() {
        let mut writer = SolArchiveWriter::new();

        let jam_0 = vec![0x01; 5];
        let jam_1 = vec![0x02; 7];

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(0),
                0,
                proof_for_height(SolHeight(0)),
                &jam_0,
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(1),
                dummy_hash(1),
                0,
                proof_for_height(SolHeight(1)),
                &jam_1,
            )
            .unwrap();

        let snapshot = vec![MempoolTxEntry {
            tx_id: dummy_hash(42),
            heard_at: SolHeight(1),
        }];
        writer
            .add_mempool_snapshot(SolHeight(0), &snapshot)
            .unwrap();
        writer
            .add_mempool_snapshot(SolHeight(1), &snapshot)
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        let retrieved_0 = reader.get_jam_by_height(SolHeight(0)).unwrap();
        assert_eq!(retrieved_0, jam_0.as_slice());

        let retrieved_1 = reader.get_jam_by_height(SolHeight(1)).unwrap();
        assert_eq!(retrieved_1, jam_1.as_slice());
    }

    /// Test iterating over a height range
    #[test]
    fn test_archive_reader_iterate_range() {
        let mut writer = SolArchiveWriter::new();

        // Add blocks 0, 2, 4, 6, 8, 10 (even numbers only)
        for i in (0..=10).step_by(2) {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(i),
                    dummy_hash(i),
                    0,
                    proof_for_height(SolHeight(i)),
                    &[i as u8; 10],
                )
                .unwrap();
        }

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        // Range 3..=7 should only yield 4 and 6 (since odd numbers don't exist)
        let range_entries: Vec<_> = reader
            .iter()
            .filter(|(entry, _)| entry.height >= SolHeight(3) && entry.height <= SolHeight(7))
            .collect();
        assert_eq!(range_entries.len(), 2);
        assert_eq!(range_entries[0].0.height, SolHeight(4));
        assert_eq!(range_entries[1].0.height, SolHeight(6));

        // Range 0..=4 should yield 0, 2, 4
        let range_entries: Vec<_> = reader
            .iter()
            .filter(|(entry, _)| entry.height >= SolHeight(0) && entry.height <= SolHeight(4))
            .collect();
        assert_eq!(range_entries.len(), 3);
        assert_eq!(range_entries[0].0.height, SolHeight(0));
        assert_eq!(range_entries[1].0.height, SolHeight(2));
        assert_eq!(range_entries[2].0.height, SolHeight(4));
    }

    /// Test filtering by proof version
    #[test]
    fn test_archive_reader_filter_by_proof_version() {
        let mut writer = SolArchiveWriter::new();

        writer
            .add_block_with_tx_count_for_test(
                SolHeight(0),
                dummy_hash(0),
                0,
                ProofVersion::V0,
                &[0x00; 10],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(PROOF_VERSION_1_START),
                dummy_hash(1),
                0,
                ProofVersion::V1,
                &[0x11; 10],
            )
            .unwrap();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(PROOF_VERSION_2_START),
                dummy_hash(2),
                0,
                ProofVersion::V2,
                &[0x22; 10],
            )
            .unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        let v1_entries: Vec<_> = reader
            .iter_filtered(ArchiveFilter {
                proof_version: Some(ProofVersion::V1),
                start_height: None,
                end_height: None,
            })
            .collect();
        assert_eq!(v1_entries.len(), 1);
        assert_eq!(v1_entries[0].height, SolHeight(PROOF_VERSION_1_START));

        let all_entries: Vec<_> = reader
            .iter_filtered(ArchiveFilter {
                proof_version: None,
                start_height: None,
                end_height: None,
            })
            .collect();
        assert_eq!(all_entries.len(), 3);
    }

    /// Test filtering by height range
    #[test]
    fn test_archive_reader_filter_by_height_range() {
        let mut writer = SolArchiveWriter::new();

        for height in 0u64..10 {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(height),
                    dummy_hash(height),
                    0,
                    proof_for_height(SolHeight(height)),
                    &[height as u8; 4],
                )
                .unwrap();
        }

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        let range_entries: Vec<_> = reader
            .iter_filtered(ArchiveFilter {
                proof_version: None,
                start_height: Some(SolHeight(3)),
                end_height: Some(SolHeight(6)),
            })
            .collect();

        assert_eq!(range_entries.len(), 4);
        assert_eq!(range_entries[0].height, SolHeight(3));
        assert_eq!(range_entries[3].height, SolHeight(6));
    }

    #[test]
    fn test_slice_archive_file_blocks_only() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let input_path = temp_dir.path().join("input.solarch");
        let output_path = temp_dir.path().join("slice.solarch");

        let mut writer = SolArchiveWriter::with_source(dummy_hash(4040));
        for height in 0u64..6 {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(height),
                    dummy_hash(height),
                    height as usize,
                    proof_for_height(SolHeight(height)),
                    &[height as u8; 4],
                )
                .expect("add block");
            writer
                .add_mempool_snapshot(
                    SolHeight(height),
                    &[MempoolTxEntry {
                        tx_id: dummy_hash(10_000 + height),
                        heard_at: SolHeight(height),
                    }],
                )
                .expect("add mempool snapshot");
        }
        writer.write_to_file(&input_path).expect("write archive");

        let result =
            slice_archive_file(&input_path, &output_path, SolHeight(2), SolHeight(4), false)
                .expect("slice archive");

        assert_eq!(result.start_height, SolHeight(2));
        assert_eq!(result.end_height, SolHeight(4));
        assert_eq!(result.block_count, 3);
        assert_eq!(result.mempool_snapshot_count, 0);

        let sliced = SolArchiveReader::from_file(&output_path).expect("read sliced archive");
        assert_eq!(sliced.block_count(), 3);
        assert_eq!(sliced.min_height(), SolHeight(2));
        assert_eq!(sliced.max_height(), SolHeight(4));
        assert!(!sliced.has_mempool());
        assert_eq!(sliced.mempool_snapshot_count(), 0);
        assert_eq!(
            sliced.metadata().source_checkpoint_hash,
            Some(dummy_hash(4040))
        );

        for height in 2u64..=4 {
            let jam = sliced
                .get_jam_by_height(SolHeight(height))
                .expect("jam by height");
            assert_eq!(jam, vec![height as u8; 4].as_slice());
        }
    }

    #[test]
    fn test_slice_archive_file_includes_mempool() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let input_path = temp_dir.path().join("input_with_mempool.solarch");
        let output_path = temp_dir.path().join("slice_with_mempool.solarch");

        let mut writer = SolArchiveWriter::new();
        for height in 0u64..4 {
            writer
                .add_block_with_tx_count_for_test(
                    SolHeight(height),
                    dummy_hash(height + 200),
                    1,
                    proof_for_height(SolHeight(height)),
                    &[0xAA + height as u8; 3],
                )
                .expect("add block");
            writer
                .add_mempool_snapshot(
                    SolHeight(height),
                    &[
                        MempoolTxEntry {
                            tx_id: dummy_hash(20_000 + height),
                            heard_at: SolHeight(height),
                        },
                        MempoolTxEntry {
                            tx_id: dummy_hash(30_000 + height),
                            heard_at: SolHeight(height),
                        },
                    ],
                )
                .expect("add mempool snapshot");
        }
        writer.write_to_file(&input_path).expect("write archive");

        let result =
            slice_archive_file(&input_path, &output_path, SolHeight(1), SolHeight(2), true)
                .expect("slice archive");
        assert_eq!(result.block_count, 2);
        assert_eq!(result.mempool_snapshot_count, 2);

        let sliced = SolArchiveReader::from_file(&output_path).expect("read sliced archive");
        assert!(sliced.has_mempool());
        assert_eq!(sliced.mempool_snapshot_count(), 2);
        assert!(sliced
            .get_mempool_snapshot(SolHeight(1))
            .expect("snapshot lookup")
            .is_some());
        assert!(sliced
            .get_mempool_snapshot(SolHeight(2))
            .expect("snapshot lookup")
            .is_some());
        assert!(sliced
            .get_mempool_snapshot(SolHeight(0))
            .expect("snapshot lookup")
            .is_none());
        assert!(sliced
            .get_mempool_snapshot(SolHeight(3))
            .expect("snapshot lookup")
            .is_none());
    }

    #[test]
    fn test_slice_archive_file_compacts_raw_tx_section() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let input_path = temp_dir.path().join("input_raw_tx.solarch");
        let output_path = temp_dir.path().join("slice_raw_tx.solarch");

        let mut writer = SolArchiveWriter::with_source(dummy_hash(5050));
        for height in 0u64..5 {
            let block_jam = vec![height as u8; 4];
            let raw_a = vec![0xA0 + height as u8, 0x01];
            let raw_b = vec![0xB0 + height as u8, 0x02, 0x03];
            writer
                .add_block_with_raw_txs(
                    SolHeight(height),
                    dummy_hash(500 + height),
                    proof_for_height(SolHeight(height)),
                    &block_jam,
                    [
                        RawTxPayload {
                            tx_id: dummy_hash(10_000 + height * 2),
                            jam_bytes: &raw_a,
                        },
                        RawTxPayload {
                            tx_id: dummy_hash(10_001 + height * 2),
                            jam_bytes: &raw_b,
                        },
                    ],
                )
                .expect("add archive block");
            writer
                .add_mempool_snapshot(
                    SolHeight(height),
                    &[MempoolTxEntry {
                        tx_id: dummy_hash(20_000 + height),
                        heard_at: SolHeight(height),
                    }],
                )
                .expect("add mempool snapshot");
        }
        writer.write_to_file(&input_path).expect("write archive");

        let result =
            slice_archive_file(&input_path, &output_path, SolHeight(1), SolHeight(3), true)
                .expect("slice archive");
        assert_eq!(result.block_count, 3);
        assert_eq!(result.mempool_snapshot_count, 3);

        let sliced = SolArchiveReader::from_file(&output_path).expect("read sliced archive");
        assert_eq!(
            sliced.inspect().source_checkpoint_hash,
            Some(dummy_hash(5050))
        );

        let body = sliced.body();
        for (index, height) in (1u64..=3).enumerate() {
            let entry = body
                .get_entry_by_height(SolHeight(height))
                .expect("sliced block should exist");
            assert_eq!(entry.raw_tx_start, (index as u64) * 2);
            assert_eq!(entry.raw_tx_count, 2);
            assert_eq!(
                body.get_jam_for_entry(entry)
                    .expect("block jam should exist"),
                vec![height as u8; 4]
            );

            let raw_entries = body
                .raw_tx_entries_for_block(entry)
                .expect("raw tx entries should exist");
            assert_eq!(raw_entries[0].tx_id, dummy_hash(10_000 + height * 2));
            assert_eq!(raw_entries[1].tx_id, dummy_hash(10_001 + height * 2));
            assert_eq!(
                body.get_raw_tx_payload(&raw_entries[0])
                    .expect("raw tx payload should exist"),
                vec![0xA0 + height as u8, 0x01]
            );
            assert_eq!(
                body.get_raw_tx_payload(&raw_entries[1])
                    .expect("raw tx payload should exist"),
                vec![0xB0 + height as u8, 0x02, 0x03]
            );
            assert!(sliced
                .get_mempool_snapshot(SolHeight(height))
                .expect("mempool lookup should succeed")
                .is_some());
        }
        assert!(sliced
            .get_mempool_snapshot(SolHeight(0))
            .expect("mempool lookup should succeed")
            .is_none());
        assert!(sliced
            .get_mempool_snapshot(SolHeight(4))
            .expect("mempool lookup should succeed")
            .is_none());
    }

    #[test]
    fn test_slice_archive_file_rejects_invalid_or_empty_range() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let input_path = temp_dir.path().join("input_invalid_range.solarch");
        let output_path = temp_dir.path().join("slice_invalid_range.solarch");

        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(5),
                dummy_hash(500),
                0,
                proof_for_height(SolHeight(5)),
                &[0xEE; 2],
            )
            .expect("add block");
        writer.write_to_file(&input_path).expect("write archive");

        let invalid = slice_archive_file(
            &input_path,
            &output_path,
            SolHeight(10),
            SolHeight(9),
            false,
        )
        .expect_err("invalid range should fail");
        assert!(matches!(invalid, ArchiveError::InvalidSliceRange { .. }));

        let empty =
            slice_archive_file(&input_path, &output_path, SolHeight(0), SolHeight(1), false)
                .expect_err("empty range should fail");
        assert!(matches!(empty, ArchiveError::SliceRangeEmpty { .. }));
    }
}
