use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nockapp::noun::slab::NounSlab;
use nockapp_grpc_proto::pb::public::v2::{TransactionDetails, TransactionInput, TransactionOutput};
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::HoonMapIter;
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Name};
use nockchain_types::tx_engine::v0::{Lock, NoteV0, RawTx};
use nockvm::noun::{Noun, SIG};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

use crate::error::{NockAppGrpcError, Result as GrpcResult};
use crate::pb::common::v1 as pb_common;
use crate::public_nockchain::v2::metrics::NockchainGrpcApiMetrics;
use crate::public_nockchain::v2::server::BalanceHandle;

/// Minimal block metadata for block explorer
#[derive(Debug, Clone)]
pub struct BlockMetadata {
    pub height: u64,
    pub block_id: Hash,
    pub parent_id: Hash,
    pub timestamp: u64,
    pub tx_ids: Vec<Hash>,
}

/// Block explorer cache that maintains indexes over the heaviest chain
pub struct BlockExplorerCache {
    /// Blocks indexed by height (for pagination)
    blocks_by_height: Arc<RwLock<BTreeMap<u64, BlockMetadata>>>,

    /// Blocks indexed by block ID (for lookups)
    blocks_by_id: Arc<RwLock<HashMap<Hash, BlockMetadata>>>,

    /// Transaction lookup: tx_id → (block_id, height)
    tx_to_block: Arc<RwLock<HashMap<Hash, (Hash, u64)>>>,

    /// Base58 string index for prefix lookups
    tx_b58_index: Arc<RwLock<BTreeMap<String, Hash>>>,

    /// Current maximum height
    max_height: Arc<AtomicU64>,

    /// Last update timestamp
    last_updated: Arc<RwLock<Instant>>,

    metrics: Arc<NockchainGrpcApiMetrics>,
    seed_ready: Arc<AtomicBool>,
    backfill_resume: Arc<RwLock<Option<u64>>>,
    chunk_semaphore: Arc<Semaphore>,
}

impl BlockExplorerCache {
    const RANGE_CHUNK: u64 = 8;
    const INITIAL_SEED_RETRY_DELAY: Duration = Duration::from_secs(2);
    const INITIAL_SEED_MAX_WAIT: Duration = Duration::from_secs(120);
    const MIN_TX_PREFIX_LEN: usize = 8;
    const MAX_PREFIX_MATCHES: usize = 16;
    pub fn new(metrics: Arc<NockchainGrpcApiMetrics>) -> Self {
        Self {
            blocks_by_height: Arc::new(RwLock::new(BTreeMap::new())),
            blocks_by_id: Arc::new(RwLock::new(HashMap::new())),
            tx_to_block: Arc::new(RwLock::new(HashMap::new())),
            tx_b58_index: Arc::new(RwLock::new(BTreeMap::new())),
            max_height: Arc::new(AtomicU64::new(0)),
            last_updated: Arc::new(RwLock::new(Instant::now())),
            metrics,
            seed_ready: Arc::new(AtomicBool::new(false)),
            backfill_resume: Arc::new(RwLock::new(None)),
            chunk_semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    /// Initialize cache by scraping the entire blockchain
    #[tracing::instrument(name = "block_explorer_cache.initialize", skip(self, handle))]
    pub async fn initialize(self: Arc<Self>, handle: Arc<dyn BalanceHandle>) -> GrpcResult<()> {
        debug!("Initializing block explorer cache");
        let deadline = Instant::now() + Self::INITIAL_SEED_MAX_WAIT;
        loop {
            let started = Instant::now();
            let result = self.clone().initialize_inner(handle.clone()).await;
            self.metrics
                .block_explorer_cache_initialize_time
                .add_timing(&started.elapsed());
            match result {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    if Instant::now() >= deadline {
                        warn!("Block explorer cache still empty after waiting; continuing startup so incremental refresh can populate it later");
                        return Ok(());
                    }
                    tracing::info!(
                        "Block explorer cache waiting for heaviest chain data before serving"
                    );
                    tokio::time::sleep(Self::INITIAL_SEED_RETRY_DELAY).await;
                }
                Err(err) => {
                    self.metrics
                        .block_explorer_cache_initialize_error
                        .increment();
                    return Err(err);
                }
            }
        }
    }

    #[tracing::instrument(
        name = "block_explorer_cache.initialize_inner",
        skip(self, handle),
        fields(max_height = tracing::field::Empty)
    )]
    async fn initialize_inner(self: Arc<Self>, handle: Arc<dyn BalanceHandle>) -> GrpcResult<bool> {
        // Get current height
        let (max_height, _tip_block_id) = match self.peek_heaviest_chain(&handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty; skipping cache initialization");
                // Ensure we expose an empty cache instead of bubbling an error
                {
                    *self.blocks_by_height.write().await = BTreeMap::new();
                    *self.blocks_by_id.write().await = HashMap::new();
                    *self.tx_to_block.write().await = HashMap::new();
                    *self.tx_b58_index.write().await = BTreeMap::new();
                    self.max_height.store(0, Ordering::Release);
                }
                return Ok(false);
            }
            Err(err) => return Err(err),
        };
        tracing::Span::current().record("max_height", &tracing::field::display(max_height));
        info!(
            max_height,
            "Detected heaviest chain height for block explorer cache seed"
        );

        if max_height == 0 {
            debug!("Empty blockchain, no blocks to cache");
            return Ok(false);
        }

        {
            *self.blocks_by_height.write().await = BTreeMap::new();
            *self.blocks_by_id.write().await = HashMap::new();
            *self.tx_to_block.write().await = HashMap::new();
            *self.tx_b58_index.write().await = BTreeMap::new();
            self.max_height.store(0, Ordering::Release);
        }

        let first_start = max_height.saturating_sub(Self::RANGE_CHUNK - 1);
        let first_chunk = match self
            .peek_blocks_range(&handle, first_start, max_height)
            .await
        {
            Ok(chunk) => chunk,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!(
                    "Heaviest chain range {}..={} returned no blocks; cache seed will retry",
                    first_start, max_height
                );
                return Ok(false);
            }
            Err(err) => return Err(err),
        };
        let inserted = first_chunk.len();
        self.insert_blocks(first_chunk).await;

        debug!(
            "Inserted initial {} blocks ({}..={})",
            inserted, first_start, max_height
        );
        info!(
            inserted_blocks = inserted,
            range_start = first_start,
            range_end = max_height,
            "Seeded block explorer cache with initial range"
        );

        if first_start > 0 {
            let resume_height = first_start - 1;
            info!(resume_height, "Queueing block explorer cache backfill task");
            *self.backfill_resume.write().await = Some(resume_height);
        } else {
            *self.backfill_resume.write().await = None;
        }

        info!(
            next_backfill_start = first_start.saturating_sub(1),
            current_height = self.max_height.load(Ordering::Acquire),
            "Block explorer cache initialization complete"
        );
        self.seed_ready.store(true, Ordering::Release);
        Ok(true)
    }

    /// Refresh cache with new blocks
    #[tracing::instrument(name = "block_explorer_cache.refresh", skip(self, handle))]
    pub async fn refresh(&self, handle: &Arc<dyn BalanceHandle>) -> GrpcResult<()> {
        let started = Instant::now();
        let result = self.refresh_inner(handle).await;
        self.metrics
            .block_explorer_cache_refresh_time
            .add_timing(&started.elapsed());
        if result.is_err() {
            self.metrics.block_explorer_cache_refresh_error.increment();
        }
        result
    }

    #[tracing::instrument(
        name = "block_explorer_cache.refresh_inner",
        skip(self, handle),
        fields(last_height = tracing::field::Empty, current_height = tracing::field::Empty)
    )]
    async fn refresh_inner(&self, handle: &Arc<dyn BalanceHandle>) -> GrpcResult<()> {
        let (current_height, _) = match self.peek_heaviest_chain(handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty; skipping cache refresh");
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        tracing::Span::current().record("current_height", &tracing::field::display(current_height));
        let last_height = self.max_height.load(Ordering::Acquire);
        tracing::Span::current().record("last_height", &tracing::field::display(last_height));

        if current_height <= last_height {
            debug!(
                "No new blocks to fetch (current: {}, cached: {})",
                current_height, last_height
            );
            return Ok(());
        }

        debug!(
            "Fetching new blocks {} to {}",
            last_height + 1,
            current_height
        );
        info!(
            start_height = last_height + 1,
            end_height = current_height,
            "Refreshing block explorer cache with new blocks"
        );
        let new_blocks = match self
            .peek_blocks_range_chunked(handle, last_height + 1, current_height)
            .await
        {
            Ok(blocks) => blocks,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!(
                    "No block data available yet for range {}-{}; deferring refresh",
                    last_height + 1,
                    current_height
                );
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        if new_blocks.is_empty() {
            debug!(
                "Block explorer refresh range {}-{} returned no entries; waiting for data",
                last_height + 1,
                current_height
            );
            return Ok(());
        }
        let count = new_blocks.len();
        self.insert_blocks(new_blocks).await;

        debug!(
            "Block explorer cache refreshed to height {}, inserted {} blocks",
            current_height, count
        );
        Ok(())
    }
    /// Get paginated blocks (descending by height)
    #[tracing::instrument(
        name = "block_explorer_cache.get_blocks_page",
        skip(self),
        fields(start_height = tracing::field::Empty)
    )]
    pub async fn get_blocks_page(
        &self,
        cursor: Option<u64>,
        limit: usize,
    ) -> (Vec<BlockMetadata>, Option<u64>) {
        let blocks = self.blocks_by_height.read().await;
        let start_height = cursor.unwrap_or(self.max_height.load(Ordering::Acquire));
        tracing::Span::current().record("start_height", &tracing::field::display(start_height));

        let page: Vec<_> = blocks
            .range(..=start_height)
            .rev()
            .take(limit)
            .map(|(_, block)| block.clone())
            .collect();

        let next_cursor = page.last().map(|b| b.height.saturating_sub(1));
        (page, next_cursor.filter(|h| *h > 0))
    }

    /// Lookup block for transaction
    #[tracing::instrument(name = "block_explorer_cache.get_block_for_tx", skip(self))]
    pub async fn get_block_for_tx(&self, tx_id: &Hash) -> Option<BlockMetadata> {
        let tx_index = self.tx_to_block.read().await;
        let (block_id, _height) = tx_index.get(tx_id)?;

        let blocks = self.blocks_by_id.read().await;
        blocks.get(block_id).cloned()
    }

    /// Get current max height
    pub fn get_max_height(&self) -> u64 {
        self.max_height.load(Ordering::Acquire)
    }

    #[tracing::instrument(
        name = "block_explorer_cache.resolve_tx_id",
        skip(self),
        fields(input_len = tracing::field::Empty)
    )]
    pub async fn resolve_tx_id(&self, input: &str) -> GrpcResult<Hash> {
        let trimmed = input.trim();
        tracing::Span::current().record("input_len", &tracing::field::display(trimmed.len()));
        if trimmed.is_empty() {
            return Err(NockAppGrpcError::InvalidRequest(
                "tx_id prefix cannot be empty".into(),
            ));
        }

        if let Ok(hash) = Hash::from_base58(trimmed) {
            if hash.to_base58() == trimmed {
                return Ok(hash);
            }
        }

        self.lookup_tx_by_prefix(trimmed).await
    }

    #[tracing::instrument(name = "block_explorer_cache.lookup_tx_by_prefixes", skip(self))]
    pub async fn lookup_tx_by_prefixes(&self, prefix: &str) -> GrpcResult<Vec<(String, Hash)>> {
        if prefix.len() < Self::MIN_TX_PREFIX_LEN {
            return Err(NockAppGrpcError::TxPrefixTooShort(Self::MIN_TX_PREFIX_LEN));
        }

        let index = self.tx_b58_index.read().await;
        let mut iter = index.range(prefix.to_string()..);
        let mut first_match: Option<(String, Hash)> = None;
        let mut conflicts: Vec<String> = Vec::new();

        while let Some((key, hash)) = iter.next() {
            if !key.starts_with(prefix) {
                break;
            }

            if first_match.is_some() {
                if conflicts.len() >= Self::MAX_PREFIX_MATCHES - 1 {
                    break;
                }
                conflicts.push(key.clone());
            } else {
                first_match = Some((key.clone(), hash.clone()));
            }
        }

        if conflicts.is_empty() {
            first_match
                .map(|(key, hash)| vec![(key, hash)])
                .ok_or(NockAppGrpcError::NotFound)
        } else if let Some((key, hash)) = first_match {
            let mut results = Vec::new();
            results.push((key, hash));
            for conflict in conflicts {
                if let Some(tx_hash) = index.get(&conflict) {
                    results.push((conflict, tx_hash.clone()));
                }
            }
            Ok(results)
        } else {
            Err(NockAppGrpcError::NotFound)
        }
    }

    #[tracing::instrument(name = "block_explorer_cache.lookup_tx_by_prefix", skip(self))]
    async fn lookup_tx_by_prefix(&self, prefix: &str) -> GrpcResult<Hash> {
        let matches = self.lookup_tx_by_prefixes(prefix).await?;
        match matches.len() {
            1 => Ok(matches[0].1.clone()),
            n if n > 1 => Err(NockAppGrpcError::TxPrefixAmbiguous(
                matches
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
            _ => Err(NockAppGrpcError::NotFound),
        }
    }

    #[tracing::instrument(
        name = "block_explorer_cache.load_transaction_details",
        skip(self, handle),
        fields(tx_id = tracing::field::Empty)
    )]
    pub async fn load_transaction_details(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        tx_id: &Hash,
    ) -> GrpcResult<TransactionDetails> {
        tracing::Span::current().record("tx_id", &tracing::field::display(tx_id.to_base58()));
        // First check cache for confirmed block metadata
        if let Some(block_meta) = self.get_block_for_tx(tx_id).await {
            let tx_details = self
                .fetch_transaction_from_block(handle, &block_meta, tx_id)
                .await?;
            return Ok(tx_details);
        }

        // If not found, see if it's pending
        if self.transaction_pending(handle, tx_id).await? {
            return Err(NockAppGrpcError::TxPending);
        }

        Err(NockAppGrpcError::NotFound)
    }

    #[tracing::instrument(
        name = "block_explorer_cache.fetch_transaction_from_block",
        skip(self, handle, meta),
        fields(height = tracing::field::Empty, tx_id = tracing::field::Empty)
    )]
    async fn fetch_transaction_from_block(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        meta: &BlockMetadata,
        tx_id: &Hash,
    ) -> GrpcResult<TransactionDetails> {
        tracing::Span::current().record("height", &tracing::field::display(meta.height));
        tracing::Span::current().record("tx_id", &tracing::field::display(tx_id.to_base58()));
        let block = self
            .load_block_with_transactions(handle, meta.height)
            .await?;

        let metadata = block.metadata.clone();
        let (hash, tx) = block
            .txs
            .into_iter()
            .find(|(hash, _)| hash == tx_id)
            .ok_or(NockAppGrpcError::NotFound)?;

        Ok(build_transaction_details(&metadata, &hash, tx))
    }

    /// Peek /heaviest-chain ~ to get current tip
    #[tracing::instrument(name = "block_explorer_cache.peek_heaviest_chain", skip(self, handle))]
    async fn peek_heaviest_chain(
        &self,
        handle: &Arc<dyn BalanceHandle>,
    ) -> GrpcResult<(u64, Hash)> {
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain").as_noun();
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };

        // Decode Option<Option<(BlockHeight, Hash)>>
        let opt: Option<Option<(BlockHeight, Hash)>> =
            NounDecode::from_noun(&result_noun).map_err(NockAppGrpcError::NounDecode)?;

        let (height, hash) = opt.flatten().ok_or(NockAppGrpcError::PeekReturnedNoData)?;

        Ok((height.0 .0, hash)) // Extract u64 from BlockHeight(Belt)
    }

    #[tracing::instrument(
        name = "block_explorer_cache.transaction_pending",
        skip(self, handle),
        fields(tx_id = tracing::field::Empty)
    )]
    async fn transaction_pending(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        tx_id: &Hash,
    ) -> GrpcResult<bool> {
        tracing::Span::current().record("tx_id", &tracing::field::display(tx_id.to_base58()));
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "raw-transaction").as_noun();
        let tx_id_b58 = tx_id.to_base58();
        let tx_id_noun = tx_id_b58.to_noun(&mut path_slab);
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, tx_id_noun, SIG]);
        path_slab.set_root(path_noun);

        match handle.peek(path_slab).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(NockAppGrpcError::from(e)),
        }
    }

    #[tracing::instrument(
        name = "block_explorer_cache.load_block_with_transactions",
        skip(self, handle),
        fields(height = tracing::field::Empty)
    )]
    async fn load_block_with_transactions(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        height: u64,
    ) -> GrpcResult<BlockEntryWithTxs> {
        tracing::Span::current().record("height", &tracing::field::display(height));
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain-blocks-range").as_noun();
        let start_noun = nockvm::noun::D(height);
        let end_noun = nockvm::noun::D(height);
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, start_noun, end_noun, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };
        let opt: Option<Option<Vec<BlockRangeEntryNoun>>> =
            NounDecode::from_noun(&result_noun).map_err(NockAppGrpcError::NounDecode)?;
        let entries = opt.flatten().ok_or(NockAppGrpcError::PeekReturnedNoData)?;

        let mut parsed = Vec::new();
        for entry in entries {
            parsed.push(BlockEntryWithTxs::try_from(entry).map_err(NockAppGrpcError::NounDecode)?);
        }

        parsed
            .into_iter()
            .find(|entry| entry.metadata.height == height)
            .ok_or(NockAppGrpcError::PeekReturnedNoData)
    }

    /// Peek /heaviest-chain-blocks-range/[start]/[end] ~
    /// Returns: Vec<(height, block_id, parent_id, timestamp, tx_ids)>
    #[tracing::instrument(
        name = "block_explorer_cache.peek_blocks_range_chunked",
        skip(self, handle)
    )]
    async fn peek_blocks_range_chunked(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        start: u64,
        end: u64,
    ) -> GrpcResult<Vec<(u64, Hash, Hash, u64, Vec<Hash>)>> {
        if start > end {
            return Ok(Vec::new());
        }

        let mut acc = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = (chunk_start + Self::RANGE_CHUNK - 1).min(end);
            match self.peek_blocks_range(handle, chunk_start, chunk_end).await {
                Ok(mut chunk) => acc.append(&mut chunk),
                Err(NockAppGrpcError::PeekReturnedNoData) => {
                    // Log at debug level and continue to next chunk.
                    debug!(
                        "No data for heaviest-chain range {}-{}, skipping",
                        chunk_start, chunk_end
                    );
                    return Err(NockAppGrpcError::PeekReturnedNoData);
                }
                Err(err) => return Err(err),
            }

            if chunk_end == u64::MAX {
                break;
            }
            tokio::time::sleep(Self::INITIAL_SEED_RETRY_DELAY).await;
            chunk_start = chunk_end.saturating_add(1);
        }

        Ok(acc)
    }

    #[tracing::instrument(name = "block_explorer_cache.backfill_older", skip(self, handle))]
    pub async fn backfill_older(
        self: Arc<Self>,
        handle: Arc<dyn BalanceHandle>,
        mut upper: u64,
    ) -> GrpcResult<()> {
        while !self.seed_ready.load(Ordering::Acquire) {
            debug!(
                "Backfill waiting for initial seed before starting; sleeping {:?}",
                Self::INITIAL_SEED_RETRY_DELAY
            );
            tokio::time::sleep(Self::INITIAL_SEED_RETRY_DELAY).await;
        }
        loop {
            if upper == u64::MAX {
                break;
            }
            let start = upper.saturating_sub(Self::RANGE_CHUNK - 1);
            let chunk = match self.peek_blocks_range(&handle, start, upper).await {
                Ok(blocks) => blocks,
                Err(NockAppGrpcError::PeekReturnedNoData) => {
                    debug!(
                        "No data for heaviest-chain range {}-{} during backfill; backing off for {:?}",
                        start,
                        upper,
                        Self::INITIAL_SEED_RETRY_DELAY
                    );
                    tokio::time::sleep(Self::INITIAL_SEED_RETRY_DELAY).await;
                    continue;
                }
                Err(err) => {
                    self.metrics.block_explorer_cache_backfill_error.increment();
                    return Err(err);
                }
            };

            if chunk.is_empty() {
                if start == 0 {
                    break;
                } else {
                    upper = start.saturating_sub(1);
                    continue;
                }
            }

            let inserted = chunk.len();
            self.insert_blocks(chunk).await;
            info!(
                range_start = start,
                range_end = upper,
                inserted_blocks = inserted,
                "Backfilled block explorer cache range"
            );
            tokio::time::sleep(Self::INITIAL_SEED_RETRY_DELAY).await;

            if start == 0 {
                break;
            }
            upper = start.saturating_sub(1);
        }

        Ok(())
    }

    pub async fn take_backfill_resume(&self) -> Option<u64> {
        self.backfill_resume.write().await.take()
    }

    #[tracing::instrument(
        name = "block_explorer_cache.insert_blocks",
        skip(self, blocks),
        fields(batch_size = tracing::field::Empty)
    )]
    async fn insert_blocks(&self, blocks: Vec<(u64, Hash, Hash, u64, Vec<Hash>)>) {
        if blocks.is_empty() {
            return;
        }
        let batch_size = blocks.len();
        tracing::Span::current().record("batch_size", &tracing::field::display(batch_size));

        let mut by_height_guard = self.blocks_by_height.write().await;
        let mut by_id_guard = self.blocks_by_id.write().await;
        let mut tx_index_guard = self.tx_to_block.write().await;
        let mut tx_b58_guard = self.tx_b58_index.write().await;

        let mut max_in_batch = 0;
        for (height, block_id, parent_id, timestamp, tx_ids) in blocks {
            if height > max_in_batch {
                max_in_batch = height;
            }

            let metadata = BlockMetadata {
                height,
                block_id: block_id.clone(),
                parent_id,
                timestamp,
                tx_ids: tx_ids.clone(),
            };

            by_height_guard.insert(height, metadata.clone());
            by_id_guard.insert(block_id.clone(), metadata);

            for tx_id in tx_ids {
                tx_index_guard.insert(tx_id.clone(), (block_id.clone(), height));
                tx_b58_guard.insert(tx_id.to_base58(), tx_id);
            }
        }

        drop(tx_index_guard);
        drop(tx_b58_guard);
        drop(by_id_guard);
        drop(by_height_guard);

        self.max_height.fetch_max(max_in_batch, Ordering::Release);
        *self.last_updated.write().await = Instant::now();
        debug!(
            batch_size,
            highest_seen = max_in_batch,
            "Inserted blocks into block explorer cache"
        );
    }

    #[tracing::instrument(name = "block_explorer_cache.peek_blocks_range", skip(self, handle))]
    async fn peek_blocks_range(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        start: u64,
        end: u64,
    ) -> GrpcResult<Vec<(u64, Hash, Hash, u64, Vec<Hash>)>> {
        let _permit = self
            .chunk_semaphore
            .acquire()
            .await
            .expect("chunk semaphore closed");
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain-blocks-range").as_noun();
        let start_noun = nockvm::noun::D(start);
        let end_noun = nockvm::noun::D(end);
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, start_noun, end_noun, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };

        // Decode Option<Option<Vec<(height, block-id, page, txs)>>>
        // We need to extract fields from the page and txs
        let opt: Option<Option<Vec<BlockRangeEntryNoun>>> =
            NounDecode::from_noun(&result_noun).map_err(NockAppGrpcError::NounDecode)?;

        let entries = opt.flatten().ok_or(NockAppGrpcError::PeekReturnedNoData)?;
        if entries.is_empty() {
            return Err(NockAppGrpcError::PeekReturnedNoData);
        }
        let entries: Vec<BlockRangeEntry> = entries
            .into_iter()
            .map(BlockRangeEntry::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(NockAppGrpcError::NounDecode)?;

        // Extract the data we need from each entry
        let result = entries
            .into_iter()
            .map(|entry| {
                (
                    entry.height, entry.block_id, entry.parent_id, entry.timestamp, entry.tx_ids,
                )
            })
            .collect();

        Ok(result)
    }
}

/// Intermediate type for decoding the block range peek result
/// This matches the Hoon type: [page-number block-id page (z-map tx-id tx)]
#[derive(Debug, Clone)]
struct BlockRangeEntry {
    height: u64,
    block_id: Hash,
    parent_id: Hash,
    timestamp: u64,
    tx_ids: Vec<Hash>,
}

#[derive(Debug, Clone, NounDecode)]
struct BlockRangeEntryNoun {
    height: BlockHeight,
    tail: BlockRangeEntryTail,
}

#[derive(Debug, Clone, NounDecode)]
struct BlockRangeEntryTail {
    block_id: Hash,
    tail: PageAndTxs,
}

#[derive(Debug, Clone, NounDecode)]
struct PageAndTxs {
    page: PageNoun,
    txs: Noun,
}

#[derive(Debug, Clone, NounDecode)]
struct PageNoun {
    _digest: Hash,
    _pow: Noun,
    parent: Hash,
    _tx_ids: Noun,
    _coinbase: Noun,
    timestamp: Noun,
    _epoch_counter: Noun,
    _target: Noun,
    _accumulated_work: Noun,
    _height: BlockHeight,
    _msg: Noun,
}

impl TryFrom<BlockRangeEntryNoun> for BlockRangeEntry {
    type Error = NounDecodeError;

    fn try_from(raw: BlockRangeEntryNoun) -> std::result::Result<Self, Self::Error> {
        let BlockRangeEntryNoun { height, tail } = raw;
        let BlockRangeEntryTail { block_id, tail } = tail;
        let PageAndTxs { page, txs } = tail;

        let parent_id = page.parent;
        let timestamp = u64::from_noun(&page.timestamp)?;
        let tx_ids = extract_tx_ids_from_map(&txs)?;

        Ok(Self {
            height: height.0 .0,
            block_id,
            parent_id,
            timestamp,
            tx_ids,
        })
    }
}

/// Extract transaction IDs from transactions map (z-map tx-id tx)
fn extract_tx_ids_from_map(
    txs_noun: &Noun,
) -> std::result::Result<Vec<Hash>, noun_serde::NounDecodeError> {
    // Check if it's an empty map (atom 0)
    if let Ok(atom) = txs_noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    // Iterate over the z-map and collect keys (tx-ids)
    let tx_ids: Vec<Hash> = HoonMapIter::from(*txs_noun)
        .filter(|entry| entry.is_cell())
        .filter_map(|entry| {
            let [key, _value] = entry.uncell().ok()?;
            Hash::from_noun(&key).ok()
        })
        .collect();

    Ok(tx_ids)
}

struct BlockEntryWithTxs {
    metadata: BlockMetadata,
    txs: Vec<(Hash, TxV0)>,
}

impl TryFrom<BlockRangeEntryNoun> for BlockEntryWithTxs {
    type Error = NounDecodeError;

    fn try_from(raw: BlockRangeEntryNoun) -> std::result::Result<Self, Self::Error> {
        let BlockRangeEntryNoun { height, tail } = raw;
        let BlockRangeEntryTail { block_id, tail } = tail;
        let PageAndTxs { page, txs } = tail;

        let parent_id = page.parent;
        let timestamp = u64::from_noun(&page.timestamp)?;
        let txs_full = extract_transactions_from_map(&txs)?;
        let tx_ids = txs_full.iter().map(|(hash, _)| hash.clone()).collect();

        Ok(Self {
            metadata: BlockMetadata {
                height: height.0 .0,
                block_id,
                parent_id,
                timestamp,
                tx_ids,
            },
            txs: txs_full,
        })
    }
}

fn extract_transactions_from_map(
    txs_noun: &Noun,
) -> std::result::Result<Vec<(Hash, TxV0)>, noun_serde::NounDecodeError> {
    if let Ok(atom) = txs_noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut txs = Vec::new();
    for entry in HoonMapIter::from(*txs_noun) {
        if !entry.is_cell() {
            continue;
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun(&key)?;
        let tx = TxV0::from_noun(&value)?;
        txs.push((hash, tx));
    }

    Ok(txs)
}

#[derive(Debug, Clone)]
struct TxV0 {
    version: u64,
    raw_tx: RawTx,
    total_size: u64,
    outputs: Vec<TxOutput>,
}

#[derive(Debug, Clone)]
struct TxOutput {
    lock: Lock,
    note: NoteV0,
}

impl NounDecode for TxV0 {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let version = u64::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let raw_tx = RawTx::from_noun(&cell.head())?;

        let tail = cell.tail();
        let cell = tail.as_cell()?;
        let total_size = u64::from_noun(&cell.head())?;
        let outputs = decode_outputs(&cell.tail())?;

        Ok(Self {
            version,
            raw_tx,
            total_size,
            outputs,
        })
    }
}

fn decode_outputs(noun: &Noun) -> Result<Vec<TxOutput>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut outputs = Vec::new();
    for entry in HoonMapIter::from(*noun) {
        if !entry.is_cell() {
            continue;
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let lock = Lock::from_noun(&key)?;
        let value_cell = value.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let note = NoteV0::from_noun(&value_cell.head())?;
        outputs.push(TxOutput { lock, note });
    }

    Ok(outputs)
}

fn build_transaction_details(
    metadata: &BlockMetadata,
    tx_hash: &Hash,
    tx: TxV0,
) -> TransactionDetails {
    let TxV0 {
        version,
        raw_tx,
        total_size,
        outputs,
    } = tx;

    let mut total_input = 0u64;
    let mut inputs = Vec::new();
    for (name, input) in &raw_tx.inputs.0 {
        let amount = input.note.tail.assets.0 as u64;
        total_input += amount;
        inputs.push(TransactionInput {
            note_name_b58: note_name_to_b58(&name),
            amount: Some(pb_common::Nicks { value: amount }),
            source_tx_id: input.note.tail.source.hash.to_base58(),
            coinbase: input.note.tail.source.is_coinbase,
        });
    }

    let mut total_output = 0u64;
    let mut outputs_proto = Vec::new();
    for output in outputs {
        let amount = output.note.tail.assets.0 as u64;
        total_output += amount;
        outputs_proto.push(TransactionOutput {
            note_name_b58: note_name_to_b58(&output.note.tail.name),
            amount: Some(pb_common::Nicks { value: amount }),
            lock_summary: lock_summary(&output.lock),
        });
    }

    TransactionDetails {
        tx_id: tx_hash.to_base58(),
        block_id: Some(hash_to_proto(&metadata.block_id)),
        parent: Some(hash_to_proto(&metadata.parent_id)),
        height: metadata.height,
        timestamp: metadata.timestamp,
        version,
        size_bytes: total_size,
        total_input: Some(pb_common::Nicks { value: total_input }),
        total_output: Some(pb_common::Nicks {
            value: total_output,
        }),
        fee: Some(pb_common::Nicks {
            value: raw_tx.total_fees.0 as u64,
        }),
        inputs,
        outputs: outputs_proto,
    }
}

fn hash_to_proto(hash: &Hash) -> pb_common::Hash {
    pb_common::Hash {
        belt_1: Some(pb_common::Belt { value: hash.0[0].0 }),
        belt_2: Some(pb_common::Belt { value: hash.0[1].0 }),
        belt_3: Some(pb_common::Belt { value: hash.0[2].0 }),
        belt_4: Some(pb_common::Belt { value: hash.0[3].0 }),
        belt_5: Some(pb_common::Belt { value: hash.0[4].0 }),
    }
}

fn note_name_to_b58(name: &Name) -> String {
    name.first.to_base58()
}

fn lock_summary(lock: &Lock) -> String {
    let keys: Vec<String> = lock
        .pubkeys
        .iter()
        .filter_map(|key| key.to_base58().ok())
        .collect();
    if keys.is_empty() {
        format!("{}-of-{}", lock.keys_required, lock.pubkeys.len())
    } else {
        format!(
            "{}-of-{} [{}]",
            lock.keys_required,
            lock.pubkeys.len(),
            keys.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::{BlockHeight, Hash};
    use noun_serde::{NounDecode, NounEncode};

    use super::*;

    #[test]
    fn test_decode_blockheight_as_atom() {
        let mut slab: NounSlab = NounSlab::new();

        // Test that BlockHeight encodes as a simple atom
        let height = BlockHeight(Belt(105));
        let height_noun = height.to_noun(&mut slab);

        // Verify it's an atom
        assert!(height_noun.is_atom(), "BlockHeight should encode as atom");

        // Try to decode as u64 directly
        let result_u64 = u64::from_noun(&height_noun);
        match result_u64 {
            Ok(val) => {
                println!("BlockHeight decoded as u64: {}", val);
                assert_eq!(val, 105);
            }
            Err(e) => {
                panic!("Failed to decode BlockHeight as u64: {:?}", e);
            }
        }

        // Try to decode as BlockHeight
        let result_height = BlockHeight::from_noun(&height_noun);
        match result_height {
            Ok(h) => {
                println!("BlockHeight decoded correctly: {:?}", h);
                assert_eq!(h.0 .0, 105);
            }
            Err(e) => {
                panic!("Failed to decode as BlockHeight: {:?}", e);
            }
        }
    }

    #[test]
    fn test_decode_block_range_entry_minimal() {
        let mut slab: NounSlab = NounSlab::new();

        // Create a minimal BlockRangeEntry structure
        // [page-number block-id page (z-map tx-id tx)]

        // page-number as Belt (atom)
        let height = BlockHeight(Belt(42));
        let height_noun = height.to_noun(&mut slab);

        // block-id as Hash [Belt; 5]
        let block_id = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let block_id_noun = block_id.to_noun(&mut slab);

        // page structure: [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]
        let digest = Hash([Belt(10), Belt(11), Belt(12), Belt(13), Belt(14)]);
        let pow = nockvm::noun::D(0); // empty unit
        let tx_ids_set = nockvm::noun::D(0); // empty z-set
        let coinbase = nockvm::noun::D(0);
        let timestamp = Belt(1234567890);
        let epoch_counter = Belt(0);
        let target = Belt(100);
        let accumulated_work = Belt(500);
        let page_height = Belt(42);
        let parent = Hash([Belt(20), Belt(21), Belt(22), Belt(23), Belt(24)]);
        let msg = nockvm::noun::D(0);

        // Create all nouns first to avoid borrow checker issues
        let digest_noun = digest.to_noun(&mut slab);
        let parent_noun = parent.to_noun(&mut slab);
        let timestamp_noun = timestamp.to_noun(&mut slab);
        let epoch_counter_noun = epoch_counter.to_noun(&mut slab);
        let target_noun = target.to_noun(&mut slab);
        let accumulated_work_noun = accumulated_work.to_noun(&mut slab);
        let page_height_noun = page_height.to_noun(&mut slab);

        let page_noun = nockvm::noun::T(
            &mut slab,
            &[
                digest_noun, pow, parent_noun, tx_ids_set, coinbase, timestamp_noun,
                epoch_counter_noun, target_noun, accumulated_work_noun, page_height_noun, msg,
            ],
        );

        // Empty z-map (atom 0)
        let txs_map_noun = nockvm::noun::D(0);

        // Build inner cells first
        let page_txs_cell = nockvm::noun::T(&mut slab, &[page_noun, txs_map_noun]);
        let block_page_cell = nockvm::noun::T(&mut slab, &[block_id_noun, page_txs_cell]);

        // Build the entry: [height [block-id [page txs-map]]]
        let entry_noun = nockvm::noun::T(&mut slab, &[height_noun, block_page_cell]);

        // Try to decode it
        let raw = BlockRangeEntryNoun::from_noun(&entry_noun).expect("decode raw entry");
        let entry = BlockRangeEntry::try_from(raw).expect("convert raw entry");

        assert_eq!(entry.height, 42);
        assert_eq!(entry.block_id, block_id);
        assert_eq!(entry.parent_id, parent);
        assert_eq!(entry.timestamp, 1234567890);
        assert_eq!(entry.tx_ids.len(), 0);
    }
}
