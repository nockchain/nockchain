//! Block extraction from a running kernel via peek

use std::path::{Path, PathBuf};

use bytes::Bytes;
use nockapp::nockapp::wire::WireRepr;
use nockapp::nockapp::NockApp;
use nockapp::noun::slab::NounSlab;
use nockchain_types::tx_engine::common::Hash;
use nockvm::noun::{Noun, SIG};
use thiserror::Error;
use tracing::{debug, info};

use super::archive::{MempoolTxEntry, RawTxPayload, SolArchiveReader, SolArchiveWriter};
use super::boot_source::{BootSourceError, BootSourceInput};
use super::checkpoint::CheckpointLoadError;
use super::kernel_utils::{
    init_boot_source_backed_nockapp, peek_heaviest_chain, sol_replay_wire,
    BootSourceBackedInitError, KernelInitError, PeekChainError,
};
use super::poke::build_poke_slab_from_jam;
use super::types::{summarize_archive_entry, ArchiveBlockSummary, SolHeight};
use super::{noun_compat, pma_replay};

#[derive(Debug, Clone)]
struct ArchiveBlockWithJam {
    summary: ArchiveBlockSummary,
    jam_bytes: Bytes,
    raw_txs: Vec<ExtractedRawTxPayload>,
}

#[derive(Debug, Clone)]
struct ExtractedRawTxPayload {
    tx_id: Hash,
    jam_bytes: Bytes,
}

/// Phase of archive extraction progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtractionPhase {
    /// Extracting block jam blobs and writing archive entries.
    Blocks,
    /// Replaying archived blocks to capture mempool snapshots.
    MempoolReplay,
    /// Finished writing the archive file.
    Complete,
}

/// Progress update emitted during archive extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveExtractionProgress {
    /// Current extraction phase.
    pub phase: ArchiveExtractionPhase,
    /// Number of blocks archived so far.
    pub blocks_archived: usize,
    /// Requested block target for extraction.
    pub target_blocks: u64,
    /// Number of transactions archived so far.
    pub txs_archived: usize,
    /// Inclusive start height for the current chunk (blocks phase).
    pub chunk_start: Option<u64>,
    /// Inclusive end height for the current chunk (blocks phase).
    pub chunk_end: Option<u64>,
    /// Number of blocks in the current chunk (blocks phase).
    pub chunk_blocks: usize,
    /// Number of mempool snapshots captured so far (mempool phase).
    pub mempool_snapshots_done: usize,
    /// Total mempool snapshots expected (mempool phase).
    pub mempool_snapshots_total: usize,
}

#[derive(Debug, Error)]
pub enum ExtractorError {
    #[error("Archive error: {0}")]
    Archive(#[from] super::archive::ArchiveError),

    #[error("Checkpoint load error: {0}")]
    CheckpointLoad(#[from] CheckpointLoadError),

    #[error("Boot source error: {0}")]
    BootSource(#[from] BootSourceError),

    #[error("Kernel load error: {0}")]
    KernelLoad(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Peek failed")]
    PeekFailed,

    #[error("Peek returned no data")]
    PeekReturnedNoData,

    #[error("Entry decode error: {0}")]
    EntryDecode(String),

    #[error("Noun decode error: {0}")]
    NounDecode(#[from] noun_serde::NounDecodeError),

    #[error("NockApp error: {0}")]
    NockApp(#[from] nockapp::nockapp::NockAppError),

    #[error("Kernel init error: {0}")]
    KernelInit(#[from] KernelInitError),

    #[error("Boot source init error: {0}")]
    BootSourceInit(#[from] BootSourceBackedInitError),

    #[error("Chain height peek error: {0}")]
    ChainPeek(#[from] PeekChainError),

    #[error("Invalid extraction range: start={start} end={end}")]
    InvalidRange { start: u64, end: u64 },

    #[error("Requested start height {start} exceeds chain tip {tip}")]
    StartAboveChainTip { start: u64, tip: u64 },
}

/// Configuration for block extraction
#[derive(Debug, Clone)]
pub struct ExtractorConfig {
    /// Boot source used to initialize replay state.
    pub boot_source: BootSourceInput,
    /// Path to the kernel jam file
    pub kernel_path: String,
    /// Number of blocks to extract (starting from genesis)
    pub block_count: u64,
    /// Chunk size for range queries
    pub chunk_size: u64,
    /// Working directory for NockApp (for any temp files)
    pub work_dir: PathBuf,
    /// Whether to include mempool snapshots in the archive
    pub include_mempool: bool,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        Self {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: PathBuf::from("checkpoint_1000.chkjam"),
            },
            kernel_path: "assets/dumb.jam".to_string(),
            block_count: 1000,
            chunk_size: 8,
            work_dir: PathBuf::from("."),
            include_mempool: false,
        }
    }
}

/// Extracts blocks from a checkpoint using kernel peek operations
pub struct BlockExtractor {
    config: ExtractorConfig,
    nockapp: Option<NockApp>,
}

impl BlockExtractor {
    /// Create a new extractor with the given configuration
    pub fn new(config: ExtractorConfig) -> Self {
        Self {
            config,
            nockapp: None,
        }
    }

    /// Initialize the NockApp from checkpoint and kernel files
    pub async fn initialize(&mut self) -> Result<(), ExtractorError> {
        info!(
            boot_source = ?self.config.boot_source,
            kernel = %self.config.kernel_path,
            "Initializing block extractor"
        );

        let boot_source = self.config.boot_source.clone().resolve()?;
        let work_dir = self.config.work_dir.clone();
        let nockapp = init_boot_source_backed_nockapp(
            &boot_source,
            std::path::Path::new(&self.config.kernel_path),
            &work_dir,
            true,
        )
        .await?;

        info!("NockApp initialized successfully");
        self.nockapp = Some(nockapp);
        Ok(())
    }

    /// Get the current chain tip height
    pub async fn get_chain_height(&mut self) -> Result<(u64, Hash), ExtractorError> {
        let nockapp = self.nockapp_mut()?;

        let (height, hash) = peek_heaviest_chain(nockapp)
            .await?
            .ok_or(ExtractorError::PeekReturnedNoData)?;

        Ok((height.0 .0, hash))
    }

    async fn poke_block_jam_bytes(
        &mut self,
        jam_bytes: &[u8],
        wire: &WireRepr,
    ) -> Result<(), ExtractorError> {
        let nockapp = self.nockapp_mut()?;

        let poke_slab = build_poke_slab_from_jam(jam_bytes).map_err(ExtractorError::EntryDecode)?;

        nockapp.poke(wire.clone(), poke_slab).await?;
        Ok(())
    }

    async fn peek_raw_transactions(&mut self) -> Result<Vec<MempoolTxEntry>, ExtractorError> {
        let nockapp = self.nockapp_mut()?;

        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "raw-transactions").as_noun();
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
        path_slab.set_root(path_noun);

        let result = nockapp.peek(path_slab).await?;
        let result_noun = unsafe { result.root() };
        let result_space = noun_compat::space_for_slab(&result);
        decode_raw_transactions_result(*result_noun, &result_space)
    }

    async fn populate_mempool_snapshots_with_progress<F>(
        &mut self,
        writer: &mut SolArchiveWriter,
        mut on_progress: F,
    ) -> Result<(), ExtractorError>
    where
        F: FnMut(usize, usize, SolHeight),
    {
        let reader = SolArchiveReader::from_bytes(writer.to_bytes()?)?;
        let body = reader.body();
        let total = body.metadata().block_count as usize;

        let wire = sol_replay_wire();

        for (idx, entry) in body
            .iter_filtered(super::archive::ArchiveFilter::default())
            .enumerate()
        {
            let jam_bytes = body.get_jam_for_entry(entry)?;
            self.poke_block_jam_bytes(jam_bytes, &wire).await?;
            let snapshot = self.peek_raw_transactions().await?;
            writer.add_mempool_snapshot(entry.height, &snapshot)?;
            on_progress(idx + 1, total, entry.height);
        }

        Ok(())
    }

    /// Extract blocks for archive writing without decoding historical page/tx shapes.
    async fn extract_archive_blocks_range_with_jam(
        &mut self,
        start: u64,
        end: u64,
    ) -> Result<Vec<ArchiveBlockWithJam>, ExtractorError> {
        let nockapp = self.nockapp_mut()?;

        debug!(start, end, "Extracting archive block range with jam");

        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain-blocks-range").as_noun();
        let start_noun = nockvm::noun::D(start);
        let end_noun = nockvm::noun::D(end);
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, start_noun, end_noun, SIG]);
        path_slab.set_root(path_noun);

        let result = nockapp.peek(path_slab).await?;
        let result_noun = unsafe { result.root() };
        let result_space = noun_compat::space_for_slab(&result);
        let blocks_with_jam =
            decode_block_range_result(*result_noun, &result_space, &result, start, end)?;

        debug!(
            start,
            end,
            block_count = blocks_with_jam.len(),
            "Extracted archive block range with jam"
        );

        Ok(blocks_with_jam)
    }

    /// Extract blocks and write directly to an archive file
    ///
    /// This is the main entry point for creating speed-of-light archives.
    /// It extracts blocks with their jammed noun bytes and writes them
    /// to a binary archive format that can be loaded quickly for benchmarks.
    pub async fn extract_to_archive<P: AsRef<Path>>(
        &mut self,
        count: u64,
        output_path: P,
    ) -> Result<(), ExtractorError> {
        self.extract_to_archive_with_progress(count, output_path, |_| {})
            .await
    }

    /// Extract an inclusive block-height range directly to an archive file.
    pub async fn extract_range_to_archive<P: AsRef<Path>>(
        &mut self,
        start_height: u64,
        end_height: u64,
        output_path: P,
    ) -> Result<(), ExtractorError> {
        self.extract_range_to_archive_with_progress(start_height, end_height, output_path, |_| {})
            .await
    }

    /// Extract blocks and write directly to an archive file with progress callbacks.
    pub async fn extract_to_archive_with_progress<P, F>(
        &mut self,
        count: u64,
        output_path: P,
        on_progress: F,
    ) -> Result<(), ExtractorError>
    where
        P: AsRef<Path>,
        F: FnMut(ArchiveExtractionProgress),
    {
        if count == 0 {
            return Err(ExtractorError::InvalidRange { start: 0, end: 0 });
        }
        let end_height = count.saturating_sub(1);
        self.extract_range_to_archive_with_progress(0, end_height, output_path, on_progress)
            .await
    }

    /// Extract an inclusive block-height range directly to an archive file with progress callbacks.
    pub async fn extract_range_to_archive_with_progress<P, F>(
        &mut self,
        start_height: u64,
        end_height: u64,
        output_path: P,
        mut on_progress: F,
    ) -> Result<(), ExtractorError>
    where
        P: AsRef<Path>,
        F: FnMut(ArchiveExtractionProgress),
    {
        if start_height > end_height {
            return Err(ExtractorError::InvalidRange {
                start: start_height,
                end: end_height,
            });
        }

        info!(
            start_height,
            end_height,
            path = %output_path.as_ref().display(),
            "Extracting block range to archive"
        );

        let requested_target_blocks = end_height.saturating_sub(start_height).saturating_add(1);

        // Try to get chain height. If available, cap the end to the chain tip.
        let effective_end_height = match self.get_chain_height().await {
            Ok((chain_height, _)) => {
                info!(chain_height, "Chain height available");
                if start_height > chain_height {
                    return Err(ExtractorError::StartAboveChainTip {
                        start: start_height,
                        tip: chain_height,
                    });
                }
                end_height.min(chain_height)
            }
            Err(ExtractorError::PeekReturnedNoData) => {
                info!("Chain height unavailable, will extract until empty results");
                end_height
            }
            Err(e) => return Err(e),
        };

        let target_blocks = effective_end_height
            .saturating_sub(start_height)
            .saturating_add(1);
        let mut current = start_height;
        let mut total_blocks = 0usize;
        let mut total_txs = 0usize;

        let mut writer = SolArchiveWriter::new();
        while current <= effective_end_height {
            let chunk_end = (current + self.config.chunk_size - 1).min(effective_end_height);
            match self
                .extract_archive_blocks_range_with_jam(current, chunk_end)
                .await
            {
                Ok(blocks) => {
                    if blocks.is_empty() {
                        info!(current, "No more blocks available, stopping extraction");
                        break;
                    }

                    for block in &blocks {
                        let raw_txs = block.raw_txs.iter().map(|raw_tx| RawTxPayload {
                            tx_id: raw_tx.tx_id.clone(),
                            jam_bytes: raw_tx.jam_bytes.as_ref(),
                        });
                        writer.add_block_with_raw_txs(
                            block.summary.height,
                            block.summary.block_id.clone(),
                            block.summary.proof_version,
                            &block.jam_bytes,
                            raw_txs,
                        )?;
                        total_txs += block.summary.tx_count;
                    }
                    total_blocks += blocks.len();
                    on_progress(ArchiveExtractionProgress::blocks(
                        total_blocks,
                        target_blocks,
                        total_txs,
                        current,
                        chunk_end,
                        blocks.len(),
                    ));

                    info!(
                        start = current,
                        end = chunk_end,
                        blocks = blocks.len(),
                        total_blocks,
                        total_txs,
                        "Archived block chunk"
                    );
                }
                Err(ExtractorError::PeekReturnedNoData) => {
                    info!(current, "No more blocks available, stopping extraction");
                    break;
                }
                Err(e) => return Err(e),
            }

            current = chunk_end + 1;
        }

        if self.config.include_mempool {
            info!("Replaying blocks to capture diagnostic mempool snapshots");
            self.populate_mempool_snapshots_with_progress(&mut writer, |done, total, _height| {
                on_progress(ArchiveExtractionProgress::mempool(
                    total_blocks, target_blocks, total_txs, done, total,
                ));
            })
            .await?;
        }

        writer.write_to_file(output_path.as_ref())?;
        on_progress(ArchiveExtractionProgress::complete(
            total_blocks, target_blocks, total_txs,
        ));

        if total_blocks == 0 && requested_target_blocks > 0 {
            return Err(ExtractorError::StartAboveChainTip {
                start: start_height,
                tip: effective_end_height,
            });
        }

        info!(
            total_blocks,
            total_txs,
            path = %output_path.as_ref().display(),
            "Archive written successfully"
        );

        Ok(())
    }

    fn nockapp_mut(&mut self) -> Result<&mut NockApp, ExtractorError> {
        self.nockapp
            .as_mut()
            .ok_or_else(|| ExtractorError::KernelLoad("NockApp not initialized".to_string()))
    }
}

impl ArchiveExtractionProgress {
    fn blocks(
        blocks_archived: usize,
        target_blocks: u64,
        txs_archived: usize,
        chunk_start: u64,
        chunk_end: u64,
        chunk_blocks: usize,
    ) -> Self {
        Self {
            phase: ArchiveExtractionPhase::Blocks,
            blocks_archived,
            target_blocks,
            txs_archived,
            chunk_start: Some(chunk_start),
            chunk_end: Some(chunk_end),
            chunk_blocks,
            mempool_snapshots_done: 0,
            mempool_snapshots_total: 0,
        }
    }

    fn mempool(
        blocks_archived: usize,
        target_blocks: u64,
        txs_archived: usize,
        mempool_snapshots_done: usize,
        mempool_snapshots_total: usize,
    ) -> Self {
        Self {
            phase: ArchiveExtractionPhase::MempoolReplay,
            blocks_archived,
            target_blocks,
            txs_archived,
            chunk_start: None,
            chunk_end: None,
            chunk_blocks: 0,
            mempool_snapshots_done,
            mempool_snapshots_total,
        }
    }

    fn complete(blocks_archived: usize, target_blocks: u64, txs_archived: usize) -> Self {
        Self {
            phase: ArchiveExtractionPhase::Complete,
            blocks_archived,
            target_blocks,
            txs_archived,
            chunk_start: None,
            chunk_end: None,
            chunk_blocks: 0,
            mempool_snapshots_done: 0,
            mempool_snapshots_total: 0,
        }
    }
}

fn decode_raw_transactions_result(
    result_noun: Noun,
    space: &noun_compat::NounSpace,
) -> Result<Vec<MempoolTxEntry>, ExtractorError> {
    let map_noun = match decode_unit_unit(result_noun, space) {
        Some(noun) => noun,
        None => return Ok(Vec::new()),
    };

    if map_noun.is_atom() && noun_compat::atom_is_zero(&map_noun, space).unwrap_or(false) {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in noun_compat::hoon_map_entries(map_noun, space) {
        let key = match noun_compat::noun_head(entry, space) {
            Ok(key) => key,
            Err(_) => continue,
        };
        let value = match noun_compat::noun_tail(entry, space) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let tx_id = noun_compat::decode_with_space::<Hash>(&key, space)?;
        let heard_at_noun = noun_compat::noun_tail(value, space)
            .map_err(|_| ExtractorError::EntryDecode("raw-tx entry not a cell".to_string()))?;
        let heard_at = noun_compat::decode_with_space::<u64>(&heard_at_noun, space)?;

        entries.push(MempoolTxEntry {
            tx_id,
            heard_at: SolHeight(heard_at),
        });
    }

    Ok(entries)
}

fn decode_block_range_result<J>(
    result_noun: Noun,
    space: &noun_compat::NounSpace,
    result_slab: &NounSlab<J>,
    start: u64,
    end: u64,
) -> Result<Vec<ArchiveBlockWithJam>, ExtractorError> {
    let list_noun =
        decode_unit_unit(result_noun, space).ok_or(ExtractorError::PeekReturnedNoData)?;
    let mut blocks_with_jam = Vec::new();

    for entry_noun in noun_compat::hoon_list_items(list_noun, space)
        .map_err(|_| ExtractorError::PeekReturnedNoData)?
    {
        let summary = summarize_archive_entry(entry_noun, space).map_err(|e| {
            ExtractorError::EntryDecode(format!(
                "range {start}..={end}: failed to summarize archive block-range entry noun: {e}"
            ))
        })?;

        let mut entry_slab: NounSlab = NounSlab::new();
        let copied_noun =
            pma_replay::copy_from_source_slab(&mut entry_slab, entry_noun, result_slab);
        entry_slab.set_root(copied_noun);
        let jam_bytes = entry_slab.jam();

        let raw_txs = extract_raw_tx_payloads_from_block_entry(entry_noun, space, result_slab)
            .map_err(|e| {
                ExtractorError::EntryDecode(format!(
                    "range {start}..={end}: failed to extract raw transactions: {e}"
                ))
            })?;

        blocks_with_jam.push(ArchiveBlockWithJam {
            summary,
            jam_bytes,
            raw_txs,
        });
    }

    Ok(blocks_with_jam)
}

fn extract_raw_tx_payloads_from_block_entry<J>(
    entry_noun: Noun,
    space: &noun_compat::NounSpace,
    source_slab: &NounSlab<J>,
) -> Result<Vec<ExtractedRawTxPayload>, ExtractorError> {
    let tail = noun_compat::noun_tail(entry_noun, space)
        .map_err(|_| ExtractorError::EntryDecode("block entry not a cell".to_string()))?;
    let page_and_txs = noun_compat::noun_tail(tail, space).map_err(|_| {
        ExtractorError::EntryDecode("block entry tail missing page/txs".to_string())
    })?;
    let txs_noun = noun_compat::noun_tail(page_and_txs, space)
        .map_err(|_| ExtractorError::EntryDecode("page/txs missing tx map".to_string()))?;

    if txs_noun.is_atom() && noun_compat::atom_is_zero(&txs_noun, space).unwrap_or(false) {
        return Ok(Vec::new());
    }

    let mut raw_txs = Vec::new();
    for map_entry in noun_compat::hoon_map_entries(txs_noun, space) {
        let key = noun_compat::noun_head(map_entry, space)
            .map_err(|_| ExtractorError::EntryDecode("tx map entry missing key".to_string()))?;
        let tx = noun_compat::noun_tail(map_entry, space)
            .map_err(|_| ExtractorError::EntryDecode("tx map entry missing value".to_string()))?;
        let tx_id = noun_compat::decode_with_space::<Hash>(&key, space)?;
        let raw_tx = raw_tx_from_validated_tx(tx, space)?;
        let raw_tx_id = raw_tx_id(raw_tx, space)?;
        if raw_tx_id != tx_id {
            return Err(ExtractorError::EntryDecode(format!(
                "tx id mismatch: map key {} raw tx {}",
                tx_id.to_base58(),
                raw_tx_id.to_base58()
            )));
        }

        let mut raw_slab: NounSlab = NounSlab::new();
        let copied_raw = pma_replay::copy_from_source_slab(&mut raw_slab, raw_tx, source_slab);
        raw_slab.set_root(copied_raw);
        raw_txs.push(ExtractedRawTxPayload {
            tx_id,
            jam_bytes: raw_slab.jam(),
        });
    }

    Ok(raw_txs)
}

fn raw_tx_from_validated_tx(
    tx: Noun,
    space: &noun_compat::NounSpace,
) -> Result<Noun, ExtractorError> {
    let tag_noun = noun_compat::noun_head(tx, space).map_err(|_| {
        ExtractorError::EntryDecode("validated tx is not a tagged cell".to_string())
    })?;
    let tag = noun_compat::decode_with_space::<u64>(&tag_noun, space)?;
    if tag != 0 && tag != 1 {
        return Err(ExtractorError::EntryDecode(format!(
            "unsupported validated tx tag {tag}"
        )));
    }
    let tail = noun_compat::noun_tail(tx, space)
        .map_err(|_| ExtractorError::EntryDecode("validated tx missing payload".to_string()))?;
    noun_compat::noun_head(tail, space)
        .map_err(|_| ExtractorError::EntryDecode("validated tx missing raw-tx".to_string()))
}

fn raw_tx_id(raw_tx: Noun, space: &noun_compat::NounSpace) -> Result<Hash, ExtractorError> {
    let head = noun_compat::noun_head(raw_tx, space)
        .map_err(|_| ExtractorError::EntryDecode("raw-tx is not a cell".to_string()))?;
    if let Ok(version) = noun_compat::decode_with_space::<u64>(&head, space) {
        if version == 1 {
            let tail = noun_compat::noun_tail(raw_tx, space).map_err(|_| {
                ExtractorError::EntryDecode("v1 raw-tx missing id payload".to_string())
            })?;
            let id_noun = noun_compat::noun_head(tail, space)
                .map_err(|_| ExtractorError::EntryDecode("v1 raw-tx missing id".to_string()))?;
            return Ok(noun_compat::decode_with_space::<Hash>(&id_noun, space)?);
        }
    }

    Ok(noun_compat::decode_with_space::<Hash>(&head, space)?)
}

fn decode_unit(noun: Noun, space: &noun_compat::NounSpace) -> Option<Noun> {
    if noun.is_atom() {
        return None;
    }

    let head = noun_compat::noun_head(noun, space).ok()?;
    let head_atom = noun_compat::decode_with_space::<u64>(&head, space).ok()?;
    if head_atom != 0 {
        return None;
    }

    noun_compat::noun_tail(noun, space).ok()
}

fn decode_unit_unit(noun: Noun, space: &noun_compat::NounSpace) -> Option<Noun> {
    let inner = decode_unit(noun, space)?;
    decode_unit(inner, space)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nockapp::noun::slab::NockJammer;
    use nockchain_math::belt::Belt;
    use nockchain_math::zoon::common::DefaultTipHasher;
    use nockchain_math::zoon::zmap;
    use nockvm::noun::{D, T};
    use noun_serde::NounEncode;
    use tempfile::tempdir;
    use tokio::sync::{Mutex, OnceCell};

    use super::*;
    use crate::speed_of_light::archive::SolArchiveReader;
    use crate::speed_of_light::noun_compat;

    // Path helpers - tests run from crate root, so we need to go up to repo root
    fn checkpoint_path() -> PathBuf {
        std::env::var("SOL_CHECKPOINT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("../../checkpoint_1000.chkjam"))
    }

    fn kernel_path() -> String {
        std::env::var("SOL_KERNEL_PATH").unwrap_or_else(|_| "../../assets/dumb.jam".to_string())
    }

    // Shared extractor for integration tests - avoids reinitializing for each test
    static SHARED_EXTRACTOR: OnceCell<Arc<Mutex<BlockExtractor>>> = OnceCell::const_new();

    async fn initialized_extractor(include_mempool: bool) -> BlockExtractor {
        let config = ExtractorConfig {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: checkpoint_path(),
            },
            kernel_path: kernel_path(),
            block_count: 1000,
            chunk_size: 8,
            work_dir: PathBuf::from("."),
            include_mempool,
        };
        let mut extractor = BlockExtractor::new(config);
        extractor
            .initialize()
            .await
            .expect("should initialize NockApp");
        extractor
    }

    async fn get_shared_extractor() -> Arc<Mutex<BlockExtractor>> {
        SHARED_EXTRACTOR
            .get_or_init(|| async {
                println!("=== Initializing shared BlockExtractor ===");
                let extractor = initialized_extractor(false).await;
                println!("=== Shared BlockExtractor ready ===");
                Arc::new(Mutex::new(extractor))
            })
            .await
            .clone()
    }

    // ==================== QUICK TESTS ====================
    // These tests don't require kernel initialization and run fast

    /// Test ExtractorConfig defaults
    #[test]
    fn test_extractor_config_defaults() {
        let config = ExtractorConfig::default();
        assert_eq!(config.block_count, 1000);
        assert_eq!(config.chunk_size, 8);
        assert!(!config.include_mempool);
    }

    /// Test BlockExtractor can be created without initialization
    #[test]
    fn test_extractor_creation() {
        let config = ExtractorConfig {
            boot_source: BootSourceInput::Checkpoint {
                checkpoint: checkpoint_path(),
            },
            kernel_path: kernel_path(),
            block_count: 100,
            chunk_size: 8,
            work_dir: PathBuf::from("."),
            include_mempool: false,
        };
        let extractor = BlockExtractor::new(config);
        assert!(
            extractor.nockapp.is_none(),
            "nockapp should be None before initialize"
        );
    }

    fn slab_space<J>(slab: &NounSlab<J>) -> noun_compat::NounSpace {
        noun_compat::space_for_slab(slab)
    }

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    fn unit(slab: &mut NounSlab, noun: Noun) -> Noun {
        T(slab, &[D(0), noun])
    }

    fn tx_map_with_heard_at_entries(slab: &mut NounSlab, entries: &[(Hash, u64)]) -> Noun {
        entries.iter().fold(D(0), |map, (tx_id, heard_at)| {
            let mut key = tx_id.to_noun(slab);
            let mut value = T(slab, &[D(0), D(*heard_at)]);
            zmap::z_map_put(slab, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("tx z-map insert should succeed")
        })
    }

    fn raw_tx_v0(slab: &mut NounSlab, tx_id: Hash) -> Noun {
        let id = tx_id.to_noun(slab);
        T(slab, &[id, D(10), D(20), D(30)])
    }

    fn raw_tx_v1(slab: &mut NounSlab, tx_id: Hash) -> Noun {
        let id = tx_id.to_noun(slab);
        T(slab, &[D(1), id, D(99)])
    }

    fn validated_tx(slab: &mut NounSlab, tag: u64, raw_tx: Noun) -> Noun {
        T(slab, &[D(tag), raw_tx, D(0), D(0)])
    }

    fn tx_map_with_validated_txs(slab: &mut NounSlab, entries: &[(Hash, Noun)]) -> Noun {
        entries.iter().fold(D(0), |map, (tx_id, tx)| {
            let mut key = tx_id.to_noun(slab);
            let mut value = *tx;
            zmap::z_map_put(slab, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("tx z-map insert should succeed")
        })
    }

    fn block_range_entry_noun(
        slab: &mut NounSlab,
        height: u64,
        block_id: Hash,
        page: Noun,
        txs: Noun,
    ) -> Noun {
        let height = nockchain_types::tx_engine::common::BlockHeight(Belt(height)).to_noun(slab);
        let block_id = block_id.to_noun(slab);
        let page_and_txs = T(slab, &[page, txs]);
        let tail = T(slab, &[block_id, page_and_txs]);
        T(slab, &[height, tail])
    }

    #[test]
    fn test_decode_raw_transactions_result_handles_zero_atom() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let inner = unit(&mut slab, D(0));
        let result = unit(&mut slab, inner);
        let space = slab_space(&slab);

        let decoded =
            decode_raw_transactions_result(result, &space).expect("raw tx decode should succeed");

        assert!(decoded.is_empty());
    }

    #[test]
    fn test_decode_raw_transactions_result_reads_tx_id_and_height() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let tx_id = dummy_hash(1_000);
        let tx_map = tx_map_with_heard_at_entries(&mut slab, &[(tx_id.clone(), 77)]);
        let inner = unit(&mut slab, tx_map);
        let result = unit(&mut slab, inner);
        let space = slab_space(&slab);

        let decoded =
            decode_raw_transactions_result(result, &space).expect("raw tx decode should succeed");

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].tx_id.to_base58(), tx_id.to_base58());
        assert_eq!(decoded[0].heard_at, SolHeight(77));
    }

    #[test]
    fn test_decode_block_range_result_reads_archive_entries() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let block_id = dummy_hash(2_000);
        let tx_id = dummy_hash(2_100);
        let page = T(&mut slab, &[D(1), D(2), D(3)]);
        let raw_tx = raw_tx_v1(&mut slab, tx_id.clone());
        let tx = validated_tx(&mut slab, 1, raw_tx);
        let txs = tx_map_with_validated_txs(&mut slab, &[(tx_id.clone(), tx)]);
        let entry = block_range_entry_noun(&mut slab, 12, block_id.clone(), page, txs);
        let list = T(&mut slab, &[entry, D(0)]);
        let inner = unit(&mut slab, list);
        let result = unit(&mut slab, inner);
        let space = slab_space(&slab);

        let decoded = decode_block_range_result(result, &space, &slab, 12, 12)
            .expect("block range decode should succeed");

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].summary.height, SolHeight(12));
        assert_eq!(
            decoded[0].summary.block_id.to_base58(),
            block_id.to_base58()
        );
        assert_eq!(decoded[0].summary.tx_count, 1);
        assert!(!decoded[0].jam_bytes.is_empty());
        assert_eq!(decoded[0].raw_txs.len(), 1);
        assert_eq!(decoded[0].raw_txs[0].tx_id.to_base58(), tx_id.to_base58());
        assert!(!decoded[0].raw_txs[0].jam_bytes.is_empty());
    }

    #[test]
    fn test_extract_raw_tx_payloads_reads_v0_and_v1_in_map_order() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let block_id = dummy_hash(3_000);
        let tx_id_v0 = dummy_hash(3_100);
        let tx_id_v1 = dummy_hash(3_200);
        let raw_v0 = raw_tx_v0(&mut slab, tx_id_v0.clone());
        let raw_v1 = raw_tx_v1(&mut slab, tx_id_v1.clone());
        let tx_v0 = validated_tx(&mut slab, 0, raw_v0);
        let tx_v1 = validated_tx(&mut slab, 1, raw_v1);
        let txs = tx_map_with_validated_txs(
            &mut slab,
            &[(tx_id_v0.clone(), tx_v0), (tx_id_v1.clone(), tx_v1)],
        );
        let page = T(&mut slab, &[D(1), D(2), D(3)]);
        let entry = block_range_entry_noun(&mut slab, 13, block_id, page, txs);
        let space = slab_space(&slab);

        let decoded = extract_raw_tx_payloads_from_block_entry(entry, &space, &slab)
            .expect("raw tx extraction should succeed");

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].tx_id, tx_id_v0);
        assert_eq!(decoded[1].tx_id, tx_id_v1);
        assert!(!decoded[0].jam_bytes.is_empty());
        assert!(!decoded[1].jam_bytes.is_empty());
    }

    #[test]
    fn test_extract_raw_tx_payloads_rejects_mismatched_tx_id() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let block_id = dummy_hash(4_000);
        let map_tx_id = dummy_hash(4_100);
        let raw_tx = raw_tx_v1(&mut slab, dummy_hash(4_200));
        let tx = validated_tx(&mut slab, 1, raw_tx);
        let txs = tx_map_with_validated_txs(&mut slab, &[(map_tx_id, tx)]);
        let page = T(&mut slab, &[D(1), D(2), D(3)]);
        let entry = block_range_entry_noun(&mut slab, 14, block_id, page, txs);
        let space = slab_space(&slab);

        let err = extract_raw_tx_payloads_from_block_entry(entry, &space, &slab)
            .expect_err("raw tx id mismatch should fail");

        assert!(err.to_string().contains("tx id mismatch"));
    }

    #[tokio::test]
    async fn test_extract_to_archive_rejects_zero_count() {
        let mut extractor = BlockExtractor::new(ExtractorConfig::default());
        let temp_dir = tempdir().expect("should create temp dir");
        let err = extractor
            .extract_to_archive(0, temp_dir.path().join("empty.solarch"))
            .await
            .expect_err("zero-count archive extraction should fail");
        assert!(matches!(
            err,
            ExtractorError::InvalidRange { start: 0, end: 0 }
        ));
    }

    #[tokio::test]
    async fn test_extract_range_to_archive_rejects_invalid_range() {
        let mut extractor = BlockExtractor::new(ExtractorConfig::default());
        let temp_dir = tempdir().expect("should create temp dir");
        let err = extractor
            .extract_range_to_archive(8, 7, temp_dir.path().join("invalid.solarch"))
            .await
            .expect_err("descending archive extraction range should fail");
        assert!(matches!(
            err,
            ExtractorError::InvalidRange { start: 8, end: 7 }
        ));
    }

    // ==================== INTEGRATION TESTS ====================
    // These tests require full kernel initialization.
    // Run with: cargo test -p nockchain-bench integration_test_ -- --ignored --test-threads=1

    /// Full integration test: Initialize extractor with kernel and checkpoint
    #[tokio::test]
    #[ignore = "Requires checkpoint - run with --ignored --test-threads=1"]
    async fn integration_test_01_extractor_initializes() {
        let extractor = get_shared_extractor().await;
        let guard = extractor.lock().await;
        assert!(
            guard.nockapp.is_some(),
            "nockapp should be Some after initialize"
        );
        println!("Extractor initialized successfully");
    }

    /// Full integration test: Get chain height via peek.
    #[tokio::test]
    #[ignore = "Requires checkpoint - run with --ignored --test-threads=1"]
    async fn integration_test_02_peek_chain_height() {
        let extractor = get_shared_extractor().await;
        let mut guard = extractor.lock().await;

        println!("[TEST 02] About to call get_chain_height()");
        match guard.get_chain_height().await {
            Ok((height, hash)) => {
                println!("[TEST 02] Chain height: {}", height);
                println!("[TEST 02] Tip hash: {}", hash.to_base58());
                assert!(
                    height > 0,
                    "chain height should be > 0 for a real checkpoint"
                );
            }
            Err(ExtractorError::PeekReturnedNoData) => {
                println!(
                    "[TEST 02] Chain height not available in checkpoint (expected for some states)"
                );
                println!("[TEST 02] Archive extraction can still proceed via range peek");
            }
            Err(e) => {
                println!(
                    "[TEST 02] get_chain_height failed with unexpected error: {:?}",
                    e
                );
                panic!("unexpected error: {:?}", e);
            }
        }
    }

    /// Full integration test: Extract blocks to archive file.
    #[tokio::test]
    #[ignore = "Requires checkpoint - run with --ignored --test-threads=1"]
    async fn integration_test_03_extract_to_archive() {
        let extractor = get_shared_extractor().await;
        let mut guard = extractor.lock().await;

        // Create a temp directory for the archive
        let temp_dir = tempdir().expect("should create temp dir");
        let archive_path = temp_dir.path().join("test.solarch");

        println!("[TEST 03] Extracting 100 blocks to archive...");
        guard
            .extract_to_archive(100, &archive_path)
            .await
            .expect("should extract to archive");

        // Verify the archive exists
        assert!(archive_path.exists(), "archive file should exist");

        // Read the archive back
        let archive_bytes = std::fs::read(&archive_path).expect("should read archive");
        println!("[TEST 03] Archive size: {} bytes", archive_bytes.len());

        let reader = SolArchiveReader::from_bytes(archive_bytes).expect("should parse archive");
        let metadata = reader.metadata();

        println!("[TEST 03] Archive metadata:");
        println!("  block_count: {}", metadata.block_count);
        println!("  total_tx_count: {}", metadata.total_tx_count);
        println!(
            "  height range: {}..={}",
            metadata.min_height.as_u64(),
            metadata.max_height.as_u64()
        );

        assert_eq!(metadata.block_count, 100, "should have 100 blocks");
        assert_eq!(metadata.min_height, SolHeight(0), "should start at block 0");
        assert_eq!(metadata.max_height, SolHeight(99), "should end at block 99");

        println!("[TEST 03] ✓ Archive created and validated successfully");
    }

    /// Archive regression test for the first two historical chunks.
    #[tokio::test]
    #[ignore = "Requires checkpoint - run with --ignored --test-threads=1"]
    async fn integration_test_04_full_pipeline_archive_roundtrip() {
        let extractor = get_shared_extractor().await;
        let mut guard = extractor.lock().await;

        // Create temp directory
        let temp_dir = tempdir().expect("should create temp dir");
        let archive_path = temp_dir.path().join("pipeline_test.solarch");
        let mut progress = Vec::new();

        println!("[TEST 04] Extracting blocks 0-15 to archive...");
        guard
            .extract_range_to_archive_with_progress(0, 15, &archive_path, |update| {
                progress.push(update);
            })
            .await
            .expect("should extract to archive");

        let block_progress: Vec<_> = progress
            .iter()
            .copied()
            .filter(|update| update.phase == ArchiveExtractionPhase::Blocks)
            .collect();
        assert_eq!(
            block_progress.len(),
            2,
            "0..15 should archive in two chunks"
        );
        assert_eq!(block_progress[0].chunk_start, Some(0));
        assert_eq!(block_progress[0].chunk_end, Some(7));
        assert_eq!(block_progress[1].chunk_start, Some(8));
        assert_eq!(block_progress[1].chunk_end, Some(15));

        println!("[TEST 04] Loading archive...");
        let archive_bytes = std::fs::read(&archive_path).expect("should read archive");
        let reader = SolArchiveReader::from_bytes(archive_bytes).expect("should parse archive");

        assert_eq!(
            reader.block_count(),
            16,
            "archive should include blocks 0..15"
        );
        assert_eq!(reader.min_height(), SolHeight(0));
        assert_eq!(reader.max_height(), SolHeight(15));

        for expected_height in 0..=15 {
            let entry = reader
                .get_entry_by_height(SolHeight(expected_height))
                .expect("archive entry should exist");
            let jam_bytes = reader
                .get_jam_by_height(SolHeight(expected_height))
                .expect("jam bytes should exist");
            assert_eq!(entry.height, SolHeight(expected_height));
            assert!(!jam_bytes.is_empty(), "jam bytes should not be empty");
        }

        assert!(progress
            .iter()
            .any(|update| update.phase == ArchiveExtractionPhase::Complete));

        println!("[TEST 04] ✓ Archive roundtrip verified for blocks 0-15");
    }

    /// Archive regression test for optional mempool snapshot capture.
    #[tokio::test]
    #[ignore = "Requires checkpoint - run with --ignored --test-threads=1"]
    async fn integration_test_05_archive_extract_with_mempool_snapshots() {
        let mut extractor = initialized_extractor(true).await;
        let temp_dir = tempdir().expect("should create temp dir");
        let archive_path = temp_dir.path().join("mempool.solarch");
        let mut progress = Vec::new();

        println!("[TEST 05] Extracting blocks 0-15 to archive with mempool snapshots...");
        extractor
            .extract_range_to_archive_with_progress(0, 15, &archive_path, |update| {
                progress.push(update);
            })
            .await
            .expect("should extract archive with mempool snapshots");

        let archive_bytes = std::fs::read(&archive_path).expect("should read archive");
        let reader = SolArchiveReader::from_bytes(archive_bytes).expect("should parse archive");
        let metadata = reader.metadata();

        assert!(
            metadata.has_mempool,
            "archive should record mempool snapshots"
        );
        assert_eq!(metadata.mempool_snapshot_count, 16);
        assert_eq!(metadata.mempool_min_height, Some(SolHeight(0)));
        assert_eq!(metadata.mempool_max_height, Some(SolHeight(15)));
        assert_eq!(
            reader.mempool_snapshot_count(),
            16,
            "reader should expose one snapshot per archived block"
        );
        assert!(progress
            .iter()
            .any(|update| update.phase == ArchiveExtractionPhase::MempoolReplay));
        assert!(progress
            .iter()
            .any(|update| update.phase == ArchiveExtractionPhase::Complete));

        println!("[TEST 05] ✓ Archive mempool replay verified for blocks 0-15");
    }
}
