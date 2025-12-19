use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nockapp::noun::slab::NounSlab;
use nockapp_grpc_proto::pb::public::v2::{
    transaction_details, transaction_output, BigNum as ProtoBigNum, BlockDetails,
    CoinbaseSplit as ProtoCoinbaseSplit, CoinbaseSplitV0 as ProtoCoinbaseSplitV0,
    CoinbaseSplitV1 as ProtoCoinbaseSplitV1, CoinbaseSplitV1Entry, PageMsg as ProtoPageMsg,
    ProofOfWork, TransactionDetails, TransactionInput, TransactionOutput,
};
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::HoonMapIter;
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Name, Page};
use nockchain_types::tx_engine::v0::{Lock, NoteV0, RawTx};
use nockvm::noun::{Noun, SIG};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

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

#[derive(Debug, Clone)]
pub struct ExplorerMetricsSnapshot {
    pub cache_height: u64,
    pub heaviest_height: u64,
    pub cache_lowest_height: u64,
    pub cache_span: u64,
    pub cache_coverage_ratio: f64,
    pub seed_ready: bool,
    pub backfill_resume_height: Option<u64>,
    pub cache_age_seconds: f64,
    pub refresh_age_seconds: f64,
    pub backfill_age_seconds: Option<f64>,
    pub seed_time_seconds: f64,
    pub refresh_success_count: u64,
    pub refresh_error_count: u64,
    pub backfill_success_count: u64,
    pub backfill_error_count: u64,
    pub get_blocks_p50_ms: f64,
    pub get_blocks_p90_ms: f64,
    pub get_blocks_p99_ms: f64,
    pub get_block_details_p50_ms: f64,
    pub get_block_details_p90_ms: f64,
    pub get_block_details_p99_ms: f64,
}

#[derive(Debug)]
pub struct LatencyTracker {
    samples: Mutex<VecDeque<Duration>>,
    capacity: usize,
}

impl LatencyTracker {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn record(&self, duration: Duration) {
        let mut guard = self.samples.lock().expect("latency tracker poisoned");
        if guard.len() == self.capacity {
            guard.pop_front();
        }
        guard.push_back(duration);
    }

    fn quantile(sorted: &[f64], quantile: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len().saturating_sub(1)) as f64 * quantile).round() as usize;
        sorted[idx.min(sorted.len().saturating_sub(1))]
    }

    pub fn quantiles_ms(&self) -> (f64, f64, f64) {
        let guard = self.samples.lock().expect("latency tracker poisoned");
        if guard.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let mut values: Vec<f64> = guard.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        (
            Self::quantile(&values, 0.50),
            Self::quantile(&values, 0.90),
            Self::quantile(&values, 0.99),
        )
    }
}

/// Full page details with all blockchain consensus fields
#[derive(Debug, Clone)]
pub struct FullPageDetails {
    /// Block identity
    pub height: u64,
    pub block_id: Hash,
    pub parent_id: Hash,

    /// Block version (0 for v0, 1 for v1)
    pub version: u32,

    /// Proof of work
    pub pow_present: bool,
    pub pow_raw: Option<Vec<u8>>,

    /// Consensus parameters
    pub timestamp: u64,
    pub epoch_counter: u64,
    pub target: BigNumValue,
    pub accumulated_work: BigNumValue,

    /// Block content
    pub tx_ids: Vec<Hash>,
    pub coinbase: CoinbaseSplitValue,
    pub msg: PageMsgValue,
}

/// BigNum representation for display
#[derive(Debug, Clone)]
pub struct BigNumValue {
    pub raw_bytes: Vec<u8>,
}

impl BigNumValue {
    pub fn to_display(&self) -> String {
        if self.raw_bytes.is_empty() {
            return "0".to_string();
        }
        let ubig = ibig::UBig::from_le_bytes(&self.raw_bytes);
        ubig.to_string()
    }

    pub fn to_hex(&self) -> String {
        if self.raw_bytes.is_empty() {
            return "0x0".to_string();
        }
        format!("0x{}", hex::encode(&self.raw_bytes))
    }
}

/// Coinbase split value (v0 or v1)
#[derive(Debug, Clone)]
pub enum CoinbaseSplitValue {
    /// V0 with raw bytes (legacy format)
    V0(Vec<u8>),
    /// V0 with count of entries (when we can't decode the sig keys)
    V0Raw(usize),
    /// V1 with hash-to-coins map
    V1(Vec<(Hash, u64)>),
}

/// Page message value
#[derive(Debug, Clone)]
pub struct PageMsgValue {
    pub raw: Vec<u8>,
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

    /// Current maximum height in cache
    max_height: Arc<AtomicU64>,

    /// Current heaviest chain height (updated by refresh, read by metrics)
    heaviest_height: Arc<AtomicU64>,

    /// Last update timestamp
    last_updated: Arc<RwLock<Instant>>,
    refresh_success: AtomicU64,
    refresh_error: AtomicU64,
    backfill_success: AtomicU64,
    backfill_error: AtomicU64,

    metrics: Arc<NockchainGrpcApiMetrics>,
    seed_ready: Arc<AtomicBool>,
    backfill_resume: Arc<RwLock<Option<u64>>>,
    chunk_semaphore: Arc<Semaphore>,
    seed_start: Instant,
    last_refresh: Arc<RwLock<Instant>>,
    last_backfill: Arc<RwLock<Option<Instant>>>,
    get_blocks_latency: Arc<LatencyTracker>,
    get_block_details_latency: Arc<LatencyTracker>,
}

impl BlockExplorerCache {
    const RANGE_CHUNK: u64 = 1024;
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
            heaviest_height: Arc::new(AtomicU64::new(0)),
            last_updated: Arc::new(RwLock::new(Instant::now())),
            refresh_success: AtomicU64::new(0),
            refresh_error: AtomicU64::new(0),
            backfill_success: AtomicU64::new(0),
            backfill_error: AtomicU64::new(0),
            metrics,
            seed_ready: Arc::new(AtomicBool::new(false)),
            backfill_resume: Arc::new(RwLock::new(None)),
            chunk_semaphore: Arc::new(Semaphore::new(1)),
            seed_start: Instant::now(),
            last_refresh: Arc::new(RwLock::new(Instant::now())),
            last_backfill: Arc::new(RwLock::new(None)),
            get_blocks_latency: Arc::new(LatencyTracker::new(512)),
            get_block_details_latency: Arc::new(LatencyTracker::new(512)),
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
        // Get current height (try heaviest-chain first, fall back to heaviest-block)
        let (max_height, _tip_block_id) = match self.get_heaviest_tip(&handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty (both derived and consensus state); skipping cache initialization");
                // Ensure we expose an empty cache instead of bubbling an error
                {
                    *self.blocks_by_height.write().await = BTreeMap::new();
                    *self.blocks_by_id.write().await = HashMap::new();
                    *self.tx_to_block.write().await = HashMap::new();
                    *self.tx_b58_index.write().await = BTreeMap::new();
                    self.max_height.store(0, Ordering::Release);
                }
                self.metrics.block_explorer_seed_ready.swap(0.0);
                return Ok(false);
            }
            Err(err) => return Err(err),
        };
        // Update cached heaviest height for metrics (avoids peek on every metrics call)
        self.heaviest_height.store(max_height, Ordering::Release);
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
        info!(
            "Attempting to fetch blocks range {}..={}",
            first_start, max_height
        );
        let first_chunk = match self
            .peek_blocks_range(&handle, first_start, max_height)
            .await
        {
            Ok(chunk) => chunk,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                warn!(
                    "Heaviest chain range {}..={} returned no blocks; cache seed will retry",
                    first_start, max_height
                );
                return Ok(false);
            }
            Err(NockAppGrpcError::NounDecode(decode_err)) => {
                error!(
                    "FATAL: Block explorer cache failed to decode blocks in range {}..={}\n\
                     \n\
                     Decode error: {:?}\n\
                     \n\
                     This indicates the Page decoder cannot handle the data format returned by the kernel.\n\
                     The Page decoder (nockchain-types/src/tx_engine/common/page.rs) must correctly decode:\n\
                     - v0 pages: [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]\n\
                     - v1 pages: [%1 digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]\n\
                     \n\
                     Known format requirements:\n\
                     - tx-ids: (z-set tx-id) - balanced tree, NOT a list\n\
                     - target/accumulated-work: [%bn (list u32)] bignum format\n\
                     - coinbase: v0=(z-map sig coins), v1=(z-map hash coins)\n\
                     \n\
                     Server must shut down to prevent silent data issues.",
                    first_start, max_height, decode_err
                );
                return Err(NockAppGrpcError::NounDecode(decode_err));
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
            self.metrics
                .block_explorer_backfill_resume_height
                .swap(resume_height as f64);
        } else {
            *self.backfill_resume.write().await = None;
            self.metrics
                .block_explorer_backfill_resume_height
                .swap(-1.0);
        }

        info!(
            next_backfill_start = first_start.saturating_sub(1),
            current_height = self.max_height.load(Ordering::Acquire),
            "Block explorer cache initialization complete"
        );
        self.seed_ready.store(true, Ordering::Release);
        self.metrics.block_explorer_seed_ready.swap(1.0);
        self.metrics
            .block_explorer_seed_time_seconds
            .swap(self.seed_start.elapsed().as_secs_f64());
        self.metrics
            .block_explorer_cache_height
            .swap(self.max_height.load(Ordering::Acquire) as f64);
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
        // Get current height (try heaviest-chain first, fall back to heaviest-block)
        let (current_height, _) = match self.get_heaviest_tip(handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty (both derived and consensus state); skipping cache refresh");
                self.metrics.block_explorer_refresh_error.increment();
                self.refresh_error.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        // Update cached heaviest height for metrics (avoids peek on every metrics call)
        self.heaviest_height
            .store(current_height, Ordering::Release);
        tracing::Span::current().record("current_height", &tracing::field::display(current_height));
        let last_height = self.max_height.load(Ordering::Acquire);
        tracing::Span::current().record("last_height", &tracing::field::display(last_height));

        if current_height <= last_height {
            debug!(
                "No new blocks to fetch (current: {}, cached: {})",
                current_height, last_height
            );
            self.metrics.block_explorer_refresh_success.increment();
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
        let mut inserted = 0usize;
        let mut chunk_start = last_height + 1;
        while chunk_start <= current_height {
            let chunk_end = (chunk_start + Self::RANGE_CHUNK - 1).min(current_height);
            match self.peek_blocks_range(handle, chunk_start, chunk_end).await {
                Ok(blocks) => {
                    if !blocks.is_empty() {
                        inserted += blocks.len();
                        self.insert_blocks(blocks).await;
                    }
                }
                Err(NockAppGrpcError::PeekReturnedNoData) => {
                    debug!(
                        "No block data available yet for range {}-{}; deferring refresh",
                        chunk_start, chunk_end
                    );
                    self.metrics.block_explorer_refresh_error.increment();
                    self.refresh_error.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                Err(err) => return Err(err),
            }

            if chunk_end == u64::MAX {
                break;
            }
            chunk_start = chunk_end.saturating_add(1);
        }

        debug!(
            "Block explorer cache refreshed to height {}, inserted {} blocks",
            current_height, inserted
        );
        self.metrics.block_explorer_refresh_success.increment();
        self.refresh_success.fetch_add(1, Ordering::Relaxed);
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

    pub fn record_get_blocks_latency(&self, duration: Duration) {
        self.get_blocks_latency.record(duration);
    }

    pub fn record_get_block_details_latency(&self, duration: Duration) {
        self.get_block_details_latency.record(duration);
    }

    /// Get metrics snapshot. This is a fast path that only reads atomics and cached values.
    /// The heaviest_height is updated by the refresh task, not fetched fresh here.
    pub async fn metrics_snapshot(&self) -> ExplorerMetricsSnapshot {
        let cache_height = self.max_height.load(Ordering::Acquire);
        let cache_lowest_height = self
            .blocks_by_height
            .read()
            .await
            .first_key_value()
            .map(|(h, _)| *h)
            .unwrap_or(0);
        let cache_span = cache_height.saturating_sub(cache_lowest_height);
        let backfill_resume_height = *self.backfill_resume.read().await;
        let cache_age_seconds = self.last_updated.read().await.elapsed().as_secs_f64();
        // Read cached heaviest height instead of doing a kernel peek
        let heaviest_height = self.heaviest_height.load(Ordering::Acquire);
        let refresh_age = self.last_refresh.read().await.elapsed().as_secs_f64();
        let backfill_age = self
            .last_backfill
            .read()
            .await
            .map(|t| t.elapsed().as_secs_f64());
        let coverage_ratio = if heaviest_height > 0 {
            (cache_span as f64) / (heaviest_height as f64).max(1.0)
        } else {
            0.0
        };
        let (get_blocks_p50_ms, get_blocks_p90_ms, get_blocks_p99_ms) =
            self.get_blocks_latency.quantiles_ms();
        let (get_block_details_p50_ms, get_block_details_p90_ms, get_block_details_p99_ms) =
            self.get_block_details_latency.quantiles_ms();
        ExplorerMetricsSnapshot {
            cache_height,
            heaviest_height,
            cache_lowest_height,
            cache_span,
            cache_coverage_ratio: coverage_ratio,
            seed_ready: self.seed_ready.load(Ordering::Acquire),
            backfill_resume_height,
            cache_age_seconds,
            refresh_age_seconds: refresh_age,
            backfill_age_seconds: backfill_age,
            seed_time_seconds: self.seed_start.elapsed().as_secs_f64(),
            refresh_success_count: self.refresh_success.load(Ordering::Relaxed),
            refresh_error_count: self.refresh_error.load(Ordering::Relaxed),
            backfill_success_count: self.backfill_success.load(Ordering::Relaxed),
            backfill_error_count: self.backfill_error.load(Ordering::Relaxed),
            get_blocks_p50_ms,
            get_blocks_p90_ms,
            get_blocks_p99_ms,
            get_block_details_p50_ms,
            get_block_details_p90_ms,
            get_block_details_p99_ms,
        }
    }

    /// Load full page details by height
    #[tracing::instrument(
        name = "block_explorer_cache.load_full_page_by_height",
        skip(self, handle),
        fields(height = tracing::field::Empty)
    )]
    pub async fn load_full_page_by_height(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        height: u64,
    ) -> GrpcResult<FullPageDetails> {
        tracing::Span::current().record("height", &tracing::field::display(height));
        self.peek_full_page(handle, height).await
    }

    /// Load full page details by block ID
    #[tracing::instrument(
        name = "block_explorer_cache.load_full_page_by_id",
        skip(self, handle),
        fields(block_id = tracing::field::Empty)
    )]
    pub async fn load_full_page_by_id(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        block_id: &str,
    ) -> GrpcResult<FullPageDetails> {
        tracing::Span::current().record("block_id", &tracing::field::display(block_id));

        let hash = Hash::from_base58(block_id).map_err(|_| {
            NockAppGrpcError::InvalidRequest(format!("Invalid block_id: {}", block_id))
        })?;

        let blocks = self.blocks_by_id.read().await;
        let meta = blocks.get(&hash).ok_or(NockAppGrpcError::NotFound)?.clone();
        drop(blocks);

        self.peek_full_page(handle, meta.height).await
    }

    /// Peek full page from kernel
    #[tracing::instrument(
        name = "block_explorer_cache.peek_full_page",
        skip(self, handle),
        fields(height = tracing::field::Empty)
    )]
    async fn peek_full_page(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        height: u64,
    ) -> GrpcResult<FullPageDetails> {
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

        tracing::debug!(
            noun_is_atom = result_noun.is_atom(),
            "peek_full_page raw result"
        );

        let opt: Option<Option<Vec<FullPageEntryNoun>>> = NounDecode::from_noun(&result_noun)
            .map_err(|e| {
                tracing::error!("Failed to decode FullPageEntryNoun list: {:?}", e);
                NockAppGrpcError::NounDecode(e)
            })?;
        let entries = opt.flatten().ok_or(NockAppGrpcError::PeekReturnedNoData)?;

        let parsed: Vec<FullPageDetails> = entries
            .into_iter()
            .map(|entry| {
                let entry_height = entry.height.0 .0;
                FullPageDetails::try_from(entry).map_err(|e| {
                    tracing::error!(
                        height = entry_height,
                        error = %e,
                        "Failed to decode FullPageDetails"
                    );
                    NockAppGrpcError::NounDecode(e)
                })
            })
            .collect::<GrpcResult<Vec<_>>>()?;

        parsed
            .into_iter()
            .find(|p| p.height == height)
            .ok_or(NockAppGrpcError::PeekReturnedNoData)
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
        let iter = index.range(prefix.to_string()..);
        let mut first_match: Option<(String, Hash)> = None;
        let mut conflicts: Vec<String> = Vec::new();

        for (key, hash) in iter {
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
        let opt: Option<Option<(BlockHeight, Hash)>> = NounDecode::from_noun(&result_noun)
            .map_err(|e| {
                error!(
                    "Failed to decode heaviest-chain peek result.\n\
                     Decode error: {:?}\n\
                     Expected: Option<Option<(BlockHeight, Hash)>>\n\
                     Got noun is_atom={}, is_cell={}",
                    e,
                    result_noun.is_atom(),
                    result_noun.is_cell()
                );
                NockAppGrpcError::NounDecode(e)
            })?;

        debug!("peek_heaviest_chain decoded: outer={:?}", opt.is_some());
        if let Some(ref inner) = opt {
            debug!("peek_heaviest_chain inner: {:?}", inner.is_some());
        }

        let (height, hash) = opt.flatten().ok_or_else(|| {
            debug!("peek_heaviest_chain returned None/empty");
            NockAppGrpcError::PeekReturnedNoData
        })?;

        self.metrics
            .block_explorer_heaviest_height
            .swap(height.0 .0 as f64);

        debug!("peek_heaviest_chain success: height={}", height.0 .0);
        Ok((height.0 .0, hash)) // Extract u64 from BlockHeight(Belt)
    }

    /// Peek /heaviest-block ~ to get heaviest block's full page data.
    /// This is a fallback for when %heaviest-chain returns no data because
    /// derived state (highest-block-height) isn't populated, even though
    /// consensus state (heaviest-block, blocks) has data.
    #[tracing::instrument(name = "block_explorer_cache.peek_heaviest_block", skip(self, handle))]
    async fn peek_heaviest_block(
        &self,
        handle: &Arc<dyn BalanceHandle>,
    ) -> GrpcResult<(u64, Hash)> {
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-block").as_noun();
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };

        // Decode Option<Option<Page>>
        let opt: Option<Option<Page>> = NounDecode::from_noun(&result_noun).map_err(|e| {
            error!(
                "Failed to decode heaviest-block peek result.\n\
                 Decode error: {:?}\n\
                 Expected: Option<Option<Page>>",
                e
            );
            NockAppGrpcError::NounDecode(e)
        })?;

        debug!(
            "peek_heaviest_block decoded: outer={:?}",
            opt.as_ref().map(|o| o.is_some())
        );

        let page = opt.flatten().ok_or_else(|| {
            debug!("peek_heaviest_block returned None/empty");
            NockAppGrpcError::PeekReturnedNoData
        })?;

        let height = page.height;
        let hash = page.digest;

        debug!("peek_heaviest_block success: height={}", height);
        Ok((height, hash))
    }

    /// Get the current heaviest chain tip, trying %heaviest-chain first,
    /// then falling back to %heaviest-block if derived state isn't populated.
    #[tracing::instrument(name = "block_explorer_cache.get_heaviest_tip", skip(self, handle))]
    async fn get_heaviest_tip(&self, handle: &Arc<dyn BalanceHandle>) -> GrpcResult<(u64, Hash)> {
        match self.peek_heaviest_chain(handle).await {
            Ok(result) => Ok(result),
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("peek_heaviest_chain returned no data, trying peek_heaviest_block fallback");
                self.peek_heaviest_block(handle).await
            }
            Err(e) => Err(e),
        }
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
    #[allow(dead_code)]
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
                    self.metrics.block_explorer_backfill_error.increment();
                    self.backfill_error.fetch_add(1, Ordering::Relaxed);
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

        self.metrics.block_explorer_backfill_success.increment();
        self.backfill_success.fetch_add(1, Ordering::Relaxed);
        *self.last_backfill.write().await = Some(Instant::now());
        Ok(())
    }

    pub async fn take_backfill_resume(&self) -> Option<u64> {
        let mut guard = self.backfill_resume.write().await;
        let val = guard.take();
        if val.is_none() {
            self.metrics
                .block_explorer_backfill_resume_height
                .swap(-1.0);
        }
        val
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
        let cache_height = self.max_height.load(Ordering::Acquire) as f64;
        self.metrics.block_explorer_cache_height.swap(cache_height);
        let lowest = self
            .blocks_by_height
            .read()
            .await
            .first_key_value()
            .map(|(h, _)| *h)
            .unwrap_or(0);
        self.metrics
            .block_explorer_cache_lowest_height
            .swap(lowest as f64);
        let span = cache_height - lowest as f64;
        self.metrics.block_explorer_cache_span.swap(span);
        let heaviest = self.metrics.block_explorer_heaviest_height.load();
        if heaviest > 0.0 {
            self.metrics
                .block_explorer_cache_coverage_ratio
                .swap(span / heaviest.max(1.0));
        }
        *self.last_refresh.write().await = Instant::now();
        let age_secs = self.last_updated.read().await.elapsed().as_secs_f64();
        self.metrics.block_explorer_cache_age_seconds.swap(age_secs);
        self.metrics.block_explorer_refresh_age_seconds.swap(0.0);
        if let Some(last) = *self.last_backfill.read().await {
            self.metrics
                .block_explorer_backfill_age_seconds
                .swap(last.elapsed().as_secs_f64());
        }
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

        // Debug: log raw noun structure
        let is_atom = result_noun.is_atom();
        let is_cell = result_noun.is_cell();
        if is_atom {
            if let Ok(atom) = result_noun.as_atom() {
                let val = atom.as_u64().unwrap_or(u64::MAX);
                info!(
                    is_atom,
                    atom_val = val,
                    "peek_blocks_range raw result is atom"
                );
            }
        } else if is_cell {
            if let Ok(cell) = result_noun.as_cell() {
                let head_is_atom = cell.head().is_atom();
                let tail_is_atom = cell.tail().is_atom();
                info!(
                    head_is_atom,
                    tail_is_atom, "peek_blocks_range raw result is cell"
                );
            }
        }

        // Decode Option<Option<Vec<(height, block-id, page, txs)>>>
        // We need to extract fields from the page and txs
        let opt: Option<Option<Vec<BlockRangeEntryNoun>>> =
            NounDecode::from_noun(&result_noun).map_err(|e| {
                // Log detailed noun structure to help diagnose format issues
                let noun_debug = if result_noun.is_atom() {
                    format!(
                        "atom (value: {:?})",
                        result_noun.as_atom().ok().and_then(|a| a.as_u64().ok())
                    )
                } else if let Ok(cell) = result_noun.as_cell() {
                    let head_type = if cell.head().is_atom() {
                        "atom"
                    } else {
                        "cell"
                    };
                    let tail_type = if cell.tail().is_atom() {
                        "atom"
                    } else {
                        "cell"
                    };
                    format!("cell [head={}, tail={}]", head_type, tail_type)
                } else {
                    "unknown".to_string()
                };

                error!(
                    "Failed to decode BlockRangeEntryNoun list.\n\
                     Decode error: {:?}\n\
                     Result noun structure: {}\n\
                     This is likely a Page decoder issue - check tx_ids (z-set vs list), bignum, or coinbase format.",
                    e, noun_debug
                );
                NockAppGrpcError::NounDecode(e)
            })?;

        let outer_some = opt.is_some();
        let inner_some = opt.as_ref().map(|v| v.is_some()).unwrap_or(false);
        info!(
            outer_some,
            inner_some, "peek_blocks_range decoded outer options"
        );

        let entries = opt.flatten().ok_or_else(|| {
            warn!("peek_blocks_range: opt.flatten() returned None");
            NockAppGrpcError::PeekReturnedNoData
        })?;
        info!("Decoded {} raw block entries", entries.len());
        if entries.is_empty() {
            warn!("peek_blocks_range: entries list is empty");
            return Err(NockAppGrpcError::PeekReturnedNoData);
        }
        let entries: Vec<BlockRangeEntry> = entries
            .into_iter()
            .map(|entry| {
                BlockRangeEntry::try_from(entry).map_err(|e| {
                    error!("Failed to convert BlockRangeEntryNoun: {:?}", e);
                    e
                })
            })
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
    page: Page,
    txs: Noun,
}

impl TryFrom<BlockRangeEntryNoun> for BlockRangeEntry {
    type Error = NounDecodeError;

    fn try_from(raw: BlockRangeEntryNoun) -> std::result::Result<Self, Self::Error> {
        let BlockRangeEntryNoun { height, tail } = raw;
        let BlockRangeEntryTail { block_id, tail } = tail;
        let PageAndTxs { page, txs } = tail;

        let parent_id = page.parent;
        let timestamp = page.timestamp;
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
    let mut tx_ids = Vec::new();
    for (idx, entry) in HoonMapIter::from(*txs_noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "extract_tx_ids_from_map: entry {} is not a cell (expected z-map [key value] pair)",
                idx
            )));
        }
        let [key, _value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "extract_tx_ids_from_map: failed to decode tx_id at entry {}: {}",
                idx, e
            ))
        })?;
        tx_ids.push(hash);
    }

    Ok(tx_ids)
}

struct BlockEntryWithTxs {
    metadata: BlockMetadata,
    txs: Vec<(Hash, DecodedTx)>,
}

impl TryFrom<BlockRangeEntryNoun> for BlockEntryWithTxs {
    type Error = NounDecodeError;

    fn try_from(raw: BlockRangeEntryNoun) -> std::result::Result<Self, Self::Error> {
        let BlockRangeEntryNoun { height, tail } = raw;
        let BlockRangeEntryTail { block_id, tail } = tail;
        let PageAndTxs { page, txs } = tail;

        let parent_id = page.parent;
        let timestamp = page.timestamp;
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
) -> std::result::Result<Vec<(Hash, DecodedTx)>, noun_serde::NounDecodeError> {
    if let Ok(atom) = txs_noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut txs = Vec::new();
    for (idx, entry) in HoonMapIter::from(*txs_noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "extract_transactions_from_map: entry {} is not a cell (expected z-map [key value] pair)",
                idx
            )));
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "extract_transactions_from_map: failed to decode tx hash at entry {}: {}",
                idx, e
            ))
        })?;
        let tx = DecodedTx::from_noun(&value).map_err(|e| {
            NounDecodeError::Custom(format!(
                "extract_transactions_from_map: failed to decode tx at entry {} (hash={}): {}",
                idx,
                hash.to_base58(),
                e
            ))
        })?;
        txs.push((hash, tx));
    }

    Ok(txs)
}

/// Transaction data decoded from kernel, supporting both v0 and v1 formats
#[derive(Debug, Clone)]
enum DecodedTx {
    V0(TxV0Data),
    V1(TxV1Data),
}

#[derive(Debug, Clone)]
struct TxV0Data {
    raw_tx: RawTx,
    total_size: u64,
    outputs: Vec<TxOutputV0>,
}

#[derive(Debug, Clone)]
struct TxV1Data {
    total_size: u64,
    total_fee: u64,
    /// Simplified input info: (name_hash, assets)
    inputs: Vec<TxV1Input>,
    /// Simplified output info: (lock_hash, assets)
    outputs: Vec<TxV1Output>,
}

#[derive(Debug, Clone)]
struct TxV1Input {
    name_first: Hash,
    // We don't have access to input amounts in v1 spends directly
    // The amount comes from the note being spent, which we don't have here
}

#[derive(Debug, Clone)]
struct TxV1Output {
    lock_root: Hash,
    assets: u64,
}

#[derive(Debug, Clone)]
struct TxOutputV0 {
    lock: Lock,
    note: NoteV0,
}

impl NounDecode for DecodedTx {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        let version = u64::from_noun(&cell.head())?;

        match version {
            0 => {
                // v0 tx: [%0 raw-tx:v0 total-size outputs:v0]
                let tail = cell.tail();
                let cell = tail.as_cell()?;
                let raw_tx = RawTx::from_noun(&cell.head())?;

                let tail = cell.tail();
                let cell = tail.as_cell()?;
                let total_size = u64::from_noun(&cell.head())?;
                let outputs = decode_outputs_v0(&cell.tail())?;

                Ok(DecodedTx::V0(TxV0Data {
                    raw_tx,
                    total_size,
                    outputs,
                }))
            }
            1 => {
                // v1 tx: [%1 raw-tx:v1 total-size outputs:v1]
                // raw-tx:v1 = [%1 id spends]
                // outputs:v1 = (z-set output:v1) where output:v1 = [note seeds]
                let tail = cell.tail();
                let cell = tail.as_cell().map_err(|e| {
                    NounDecodeError::Custom(format!("v1 tx outer tail not cell: {e:?}"))
                })?;
                let raw_tx_noun = cell.head();

                // Parse raw-tx:v1 = [%1 id spends]
                let raw_cell = raw_tx_noun
                    .as_cell()
                    .map_err(|e| NounDecodeError::Custom(format!("v1 raw-tx not cell: {e:?}")))?;
                let raw_version = u64::from_noun(&raw_cell.head()).map_err(|e| {
                    NounDecodeError::Custom(format!("v1 raw-tx version not atom: {e:?}"))
                })?;
                if raw_version != 1 {
                    return Err(NounDecodeError::Custom(format!(
                        "Expected v1 raw-tx version 1, got {}",
                        raw_version
                    )));
                }
                let raw_tail = raw_cell.tail().as_cell().map_err(|e| {
                    NounDecodeError::Custom(format!("v1 raw-tx tail not cell: {e:?}"))
                })?;
                let _tx_id = Hash::from_noun(&raw_tail.head()).map_err(|e| {
                    NounDecodeError::Custom(format!("v1 raw-tx id parse failed: {e:?}"))
                })?;
                let spends_noun = raw_tail.tail();

                // Parse spends: (z-map nname spend) - returns (inputs, total_fee)
                let (inputs, total_fee) = decode_v1_spends(&spends_noun).map_err(|e| {
                    NounDecodeError::Custom(format!("v1 spends parse failed: {e:?}"))
                })?;

                let tail = cell.tail();
                let cell = tail.as_cell().map_err(|e| {
                    NounDecodeError::Custom(format!("v1 tx size/outputs tail not cell: {e:?}"))
                })?;
                let total_size = u64::from_noun(&cell.head()).map_err(|e| {
                    NounDecodeError::Custom(format!("v1 tx total_size not atom: {e:?}"))
                })?;
                let outputs = decode_outputs_v1(&cell.tail()).map_err(|e| {
                    NounDecodeError::Custom(format!("v1 outputs parse failed: {e:?}"))
                })?;

                Ok(DecodedTx::V1(TxV1Data {
                    total_size,
                    total_fee,
                    inputs,
                    outputs,
                }))
            }
            _ => Err(NounDecodeError::Custom(format!(
                "Unknown tx version: {}",
                version
            ))),
        }
    }
}

/// Returns (inputs, total_fee)
fn decode_v1_spends(noun: &Noun) -> Result<(Vec<TxV1Input>, u64), NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok((Vec::new(), 0));
        }
    }

    let mut inputs = Vec::new();
    let mut total_fee = 0u64;
    for (idx, entry) in HoonMapIter::from(*noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "decode_v1_spends: entry {} is not a cell (expected z-map [nname spend] pair)",
                idx
            )));
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        // key is nname (Name type)
        let name = Name::from_noun(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_v1_spends: failed to decode name at entry {}: {}",
                idx, e
            ))
        })?;
        inputs.push(TxV1Input {
            name_first: name.first,
        });

        // Extract fee from spend: spend is [tag [sig/witness [seeds fee]]]
        // Navigate: value.tail().tail().tail() to get fee
        if let Ok(spend_cell) = value.as_cell() {
            // spend_cell is [tag spend-data]
            if let Ok(spend_data) = spend_cell.tail().as_cell() {
                // spend_data is [sig/witness [seeds fee]]
                if let Ok(seeds_fee) = spend_data.tail().as_cell() {
                    // seeds_fee is [seeds fee]
                    let fee_noun = seeds_fee.tail();
                    if let Ok(fee_atom) = fee_noun.as_atom() {
                        if let Ok(fee) = fee_atom.as_u64() {
                            total_fee += fee;
                        }
                    }
                }
            }
        }
    }

    Ok((inputs, total_fee))
}

fn decode_outputs_v1(noun: &Noun) -> Result<Vec<TxV1Output>, NounDecodeError> {
    // outputs:v1 can be either:
    // 1. Tagged form from polymorphic outputs: [%1 (z-set output)]
    // 2. Untagged form from tx:v1: (z-set output)
    // We need to handle both cases.

    let zset_noun = if let Ok(cell) = noun.as_cell() {
        // Check if head is the %1 tag (an atom)
        if let Ok(atom) = cell.head().as_atom() {
            if let Ok(tag) = atom.as_u64() {
                if tag == 1 {
                    // Tagged form: [%1 z-set] - strip the tag
                    cell.tail()
                } else {
                    // Head is an atom but not %1 - this is the z-set itself
                    // (z-set node structure: [n l r] where n could be atom for empty)
                    *noun
                }
            } else {
                // Large atom - treat as z-set
                *noun
            }
        } else {
            // Head is a cell - this is a z-set node directly: [n=[output] l r]
            *noun
        }
    } else {
        // It's an atom (empty z-set = 0)
        *noun
    };

    // Check for empty z-set
    if let Ok(atom) = zset_noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut outputs = Vec::new();
    // z-set structure: [n=value l=z-set r=z-set]
    // HoonMapIter returns n (the value) directly for each node
    for (idx, entry) in HoonMapIter::from(zset_noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "decode_outputs_v1: entry {} is not a cell (expected z-set output entry)",
                idx
            )));
        }
        // output:v1 = [note seeds] where note is nnote:v1
        // (from tx-engine-1.hoon line 969: +$  form  [note=nnote =seeds])
        // The entry IS the output directly (not a [value ~] pair)
        let output_cell = entry.as_cell()?;
        let note_noun = output_cell.head(); // note (nnote)
        let _seeds = output_cell.tail(); // seeds - we don't parse fully

        // nnote is polymorphic: $^(nnote:v0 nnote-1)
        // - nnote:v0: head is a cell (the old v0 note structure)
        // - nnote-1 (v1): head is atom %1, then [origin-page name note-data assets]
        let note_cell = note_noun.as_cell().map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} note not cell: {e:?}",
                idx
            ))
        })?;
        let note_head = note_cell.head();

        // If head is a cell, this is a v0 note - skip it (polymorphic handling)
        if note_head.is_cell() {
            continue;
        }

        let note_version = u64::from_noun(&note_head).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} note version not atom: {e:?}",
                idx
            ))
        })?;
        if note_version != 1 {
            // Skip non-v1 notes (polymorphic handling - shouldn't happen but possible)
            continue;
        }
        let note_tail = note_cell.tail().as_cell().map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} note tail (origin-page+) not cell: {e:?}",
                idx
            ))
        })?;
        let _origin_page = u64::from_noun(&note_tail.head()).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} origin-page not atom: {e:?}",
                idx
            ))
        })?;
        let note_tail = note_tail.tail().as_cell().map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} note tail (name+) not cell: {e:?}",
                idx
            ))
        })?;
        let name = Name::from_noun(&note_tail.head()).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} name parse failed: {e:?}",
                idx
            ))
        })?;
        let note_tail = note_tail.tail().as_cell().map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} note tail (note-data+) not cell: {e:?}",
                idx
            ))
        })?;
        let _note_data = note_tail.head(); // z-map @tas * - skip
        let assets = u64::from_noun(&note_tail.tail()).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v1: output {} assets not atom: {e:?}",
                idx
            ))
        })?;

        outputs.push(TxV1Output {
            lock_root: name.first, // For v1, name.first is the lock root hash
            assets,
        });
    }

    Ok(outputs)
}

fn decode_outputs_v0(noun: &Noun) -> Result<Vec<TxOutputV0>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut outputs = Vec::new();
    for (idx, entry) in HoonMapIter::from(*noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "decode_outputs_v0: entry {} is not a cell (expected z-map [lock note] pair)",
                idx
            )));
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let lock = Lock::from_noun(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v0: output {} lock parse failed: {}",
                idx, e
            ))
        })?;
        let value_cell = value.as_cell().map_err(|_| {
            NounDecodeError::Custom(format!("decode_outputs_v0: output {} value not cell", idx))
        })?;
        let note = NoteV0::from_noun(&value_cell.head()).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v0: output {} note parse failed: {}",
                idx, e
            ))
        })?;
        outputs.push(TxOutputV0 { lock, note });
    }

    Ok(outputs)
}

fn build_transaction_details(
    metadata: &BlockMetadata,
    tx_hash: &Hash,
    tx: DecodedTx,
) -> TransactionDetails {
    match tx {
        DecodedTx::V0(tx_data) => build_transaction_details_v0(metadata, tx_hash, tx_data),
        DecodedTx::V1(tx_data) => build_transaction_details_v1(metadata, tx_hash, tx_data),
    }
}

fn build_transaction_details_v0(
    metadata: &BlockMetadata,
    tx_hash: &Hash,
    tx: TxV0Data,
) -> TransactionDetails {
    let TxV0Data {
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
            note_name_b58: note_name_to_b58(name),
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
            amount_required: Some(transaction_output::AmountRequired::Amount(
                pb_common::Nicks { value: amount },
            )),
            lock_summary: lock_summary(&output.lock),
        });
    }

    TransactionDetails {
        tx_id: tx_hash.to_base58(),
        block_id: Some(hash_to_proto(&metadata.block_id)),
        parent: Some(hash_to_proto(&metadata.parent_id)),
        height: metadata.height,
        timestamp: metadata.timestamp,
        version: 0,
        size_bytes: total_size,
        total_input: Some(pb_common::Nicks { value: total_input }),
        total_output_required: Some(transaction_details::TotalOutputRequired::TotalOutput(
            pb_common::Nicks {
                value: total_output,
            },
        )),
        fee_required: Some(transaction_details::FeeRequired::Fee(pb_common::Nicks {
            value: raw_tx.total_fees.0 as u64,
        })),
        inputs,
        outputs: outputs_proto,
    }
}

fn build_transaction_details_v1(
    metadata: &BlockMetadata,
    tx_hash: &Hash,
    tx: TxV1Data,
) -> TransactionDetails {
    let TxV1Data {
        total_size,
        total_fee,
        inputs: tx_inputs,
        outputs: tx_outputs,
    } = tx;

    // For v1 inputs, we don't have direct access to input amounts
    // (those come from the notes being spent, which aren't in the tx data)
    let mut inputs = Vec::new();
    for input in tx_inputs {
        inputs.push(TransactionInput {
            note_name_b58: input.name_first.to_base58(),
            amount: None,                // Amount not available in v1 spend data
            source_tx_id: String::new(), // Not directly available
            coinbase: false,             // Would need to check the note being spent
        });
    }

    let mut total_output = 0u64;
    let mut outputs_proto = Vec::new();
    for output in tx_outputs {
        total_output += output.assets;
        outputs_proto.push(TransactionOutput {
            note_name_b58: output.lock_root.to_base58(),
            amount_required: Some(transaction_output::AmountRequired::Amount(
                pb_common::Nicks {
                    value: output.assets,
                },
            )),
            lock_summary: format!("lock:{}", &output.lock_root.to_base58()[..8]),
        });
    }

    TransactionDetails {
        tx_id: tx_hash.to_base58(),
        block_id: Some(hash_to_proto(&metadata.block_id)),
        parent: Some(hash_to_proto(&metadata.parent_id)),
        height: metadata.height,
        timestamp: metadata.timestamp,
        version: 1,
        size_bytes: total_size,
        total_input: None, // Not directly available for v1
        total_output_required: Some(transaction_details::TotalOutputRequired::TotalOutput(
            pb_common::Nicks {
                value: total_output,
            },
        )),
        fee_required: Some(transaction_details::FeeRequired::Fee(pb_common::Nicks {
            value: total_fee,
        })),
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

// ============================================================================
// Full Page Details - Noun Decoding Types
// ============================================================================

/// Noun decoder for full page entry
#[derive(Debug, Clone, NounDecode)]
struct FullPageEntryNoun {
    height: BlockHeight,
    tail: FullPageEntryTail,
}

#[derive(Debug, Clone, NounDecode)]
struct FullPageEntryTail {
    block_id: Hash,
    tail: FullPageData,
}

#[derive(Debug, Clone, NounDecode)]
struct FullPageData {
    page: FullPageNoun,
    txs: Noun,
}

/// Full page structure matching Hoon's page type
#[derive(Debug, Clone)]
struct FullPageNoun {
    /// For v1: version tag (%1), for v0: this is actually digest
    version_or_digest: Noun,
    /// Remaining page fields
    rest: Noun,
}

impl NounDecode for FullPageNoun {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun.as_cell()?;
        Ok(Self {
            version_or_digest: cell.head(),
            rest: cell.tail(),
        })
    }
}

impl TryFrom<FullPageEntryNoun> for FullPageDetails {
    type Error = NounDecodeError;

    fn try_from(raw: FullPageEntryNoun) -> Result<Self, Self::Error> {
        let height = raw.height.0 .0;
        let block_id = raw.tail.block_id;
        let page = raw.tail.tail.page;
        let txs_noun = raw.tail.tail.txs;

        // Determine if v0 or v1 page:
        // - v0 page: [digest pow parent ...] where digest is a Hash (cell of 5 Belts)
        // - v1 page: [%1 digest pow parent ...] where %1 is an atom
        // So if head is an atom, it's v1; if head is a cell (Hash), it's v0
        let head_is_atom = page.version_or_digest.is_atom();
        let head_is_cell = page.version_or_digest.is_cell();
        let head_as_u64 = page
            .version_or_digest
            .as_atom()
            .ok()
            .and_then(|a| a.as_u64().ok());
        let head_as_bytes = page.version_or_digest.as_atom().ok().map(|a| {
            let bytes = a.as_ne_bytes();
            format!("{:?} (len={})", bytes, bytes.len())
        });
        tracing::info!(
            height,
            head_is_atom,
            head_is_cell,
            head_as_u64 = ?head_as_u64,
            head_as_bytes = ?head_as_bytes,
            "Detecting page version from noun head"
        );

        if head_is_atom {
            // v1 page - head is the version tag
            let version = page
                .version_or_digest
                .as_atom()
                .map_err(|_| NounDecodeError::Custom("Expected atom for version".into()))?
                .as_u64()
                .map_err(|_| NounDecodeError::Custom("Version too large".into()))?;

            if version != 1 {
                return Err(NounDecodeError::Custom(format!(
                    "Unknown page version: {}",
                    version
                )));
            }
            decode_v1_page(height, block_id, page.rest, txs_noun)
        } else {
            // v0 page - head is the digest
            decode_v0_page(
                height, block_id, page.version_or_digest, page.rest, txs_noun,
            )
        }
    }
}

fn decode_v0_page(
    height: u64,
    block_id: Hash,
    digest_noun: Noun,
    rest: Noun,
    txs_noun: Noun,
) -> Result<FullPageDetails, NounDecodeError> {
    // v0 page: [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]
    let _digest = Hash::from_noun(&digest_noun)
        .map_err(|e| NounDecodeError::Custom(format!("v0 page digest: {}", e)))?;

    let cell = rest
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: rest should be cell after digest".into()))?;
    let (pow_present, pow_raw) = decode_pow(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page pow: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after pow".into()))?;
    let parent = Hash::from_noun(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page parent: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after parent".into()))?;
    let _page_tx_ids = &cell.head();

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after tx-ids".into()))?;
    let coinbase = decode_coinbase(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page coinbase: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after coinbase".into()))?;
    let timestamp = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v0 page timestamp: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v0 page timestamp: too large".into()))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after timestamp".into()))?;
    let epoch_counter = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v0 page epoch_counter: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v0 page epoch_counter: too large".into()))?;

    let cell = cell.tail().as_cell().map_err(|_| {
        NounDecodeError::Custom("v0 page: expected cell after epoch_counter".into())
    })?;
    let target = decode_bignum(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page target: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v0 page: expected cell after target".into()))?;
    let accumulated_work = decode_bignum(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page accumulated_work: {}", e)))?;

    let cell = cell.tail().as_cell().map_err(|_| {
        NounDecodeError::Custom("v0 page: expected cell after accumulated_work".into())
    })?;
    let _page_height = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v0 page height: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v0 page height: too large".into()))?;

    let msg = decode_page_msg(&cell.tail())
        .map_err(|e| NounDecodeError::Custom(format!("v0 page msg: {}", e)))?;
    let tx_ids = extract_tx_ids_from_map(&txs_noun)
        .map_err(|e| NounDecodeError::Custom(format!("v0 page tx_ids: {}", e)))?;

    Ok(FullPageDetails {
        height,
        block_id,
        parent_id: parent,
        version: 0,
        pow_present,
        pow_raw,
        timestamp,
        epoch_counter,
        target,
        accumulated_work,
        tx_ids,
        coinbase,
        msg,
    })
}

fn decode_v1_page(
    height: u64,
    block_id: Hash,
    rest: Noun,
    txs_noun: Noun,
) -> Result<FullPageDetails, NounDecodeError> {
    // v1 page (after version tag): [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]
    let cell = rest
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: rest should be cell".into()))?;
    let _digest = Hash::from_noun(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page digest: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after digest".into()))?;
    let (pow_present, pow_raw) = decode_pow(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page pow: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after pow".into()))?;
    let parent = Hash::from_noun(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page parent: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after parent".into()))?;
    let _page_tx_ids = &cell.head();

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after tx-ids".into()))?;
    let coinbase = decode_coinbase(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page coinbase: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after coinbase".into()))?;
    let timestamp = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v1 page timestamp: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v1 page timestamp: too large".into()))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after timestamp".into()))?;
    let epoch_counter = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v1 page epoch_counter: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v1 page epoch_counter: too large".into()))?;

    let cell = cell.tail().as_cell().map_err(|_| {
        NounDecodeError::Custom("v1 page: expected cell after epoch_counter".into())
    })?;
    let target = decode_bignum(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page target: {}", e)))?;

    let cell = cell
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: expected cell after target".into()))?;
    let accumulated_work = decode_bignum(&cell.head())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page accumulated_work: {}", e)))?;

    let cell = cell.tail().as_cell().map_err(|_| {
        NounDecodeError::Custom("v1 page: expected cell after accumulated_work".into())
    })?;
    let _page_height = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("v1 page height: expected atom".into()))?
        .as_u64()
        .map_err(|_| NounDecodeError::Custom("v1 page height: too large".into()))?;

    let msg = decode_page_msg(&cell.tail())
        .map_err(|e| NounDecodeError::Custom(format!("v1 page msg: {}", e)))?;
    let tx_ids = extract_tx_ids_from_map(&txs_noun)
        .map_err(|e| NounDecodeError::Custom(format!("v1 page tx_ids: {}", e)))?;

    Ok(FullPageDetails {
        height,
        block_id,
        parent_id: parent,
        version: 1,
        pow_present,
        pow_raw,
        timestamp,
        epoch_counter,
        target,
        accumulated_work,
        tx_ids,
        coinbase,
        msg,
    })
}

fn decode_pow(noun: &Noun) -> Result<(bool, Option<Vec<u8>>), NounDecodeError> {
    if noun.is_atom() {
        let atom = noun.as_atom()?;
        if atom.as_u64()? == 0 {
            return Ok((false, None));
        }
        Ok((true, Some(atom.as_ne_bytes().to_vec())))
    } else {
        // [~ proof] - proof present
        Ok((true, Some(vec![])))
    }
}

fn decode_bignum(noun: &Noun) -> Result<BigNumValue, NounDecodeError> {
    // Bignum can be [%bn list-of-u32] or just a raw atom
    if let Ok(cell) = noun.as_cell() {
        if let Ok(tag) = cell.head().as_atom() {
            let tag_val = tag.as_u64().unwrap_or(u64::MAX);
            // %bn = 28258 ('bn' as cord)
            if tag_val == 28258 {
                let mut chunks: Vec<u32> = Vec::new();
                let mut current = cell.tail();
                while let Ok(list_cell) = current.as_cell() {
                    let chunk = list_cell.head().as_atom()?.as_u64()? as u32;
                    chunks.push(chunk);
                    current = list_cell.tail();
                }
                // Reconstruct bytes from u32 chunks (LSB first)
                let mut bytes = Vec::new();
                for chunk in chunks {
                    bytes.extend_from_slice(&chunk.to_le_bytes());
                }
                // Trim trailing zeros
                while bytes.last() == Some(&0) && bytes.len() > 1 {
                    bytes.pop();
                }
                return Ok(BigNumValue { raw_bytes: bytes });
            }
        }
    }
    // Fallback: raw atom
    let atom = noun.as_atom()?;
    let bytes = atom.as_ne_bytes().to_vec();
    Ok(BigNumValue { raw_bytes: bytes })
}

fn decode_coinbase(noun: &Noun) -> Result<CoinbaseSplitValue, NounDecodeError> {
    // Coinbase in pages can be:
    // - Atom 0: empty map (genesis or no coinbase)
    // - v0 raw format: (z-map sig coins) where sig is a complex structure
    // - v1 tagged format: [%0 ...] or [%1 ...]
    // - v1 raw format: (z-map hash coins) where hash is a Hash

    // Atom 0 = empty map
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(CoinbaseSplitValue::V0(vec![]));
        }
        // Non-zero atom - shouldn't happen but handle gracefully
        return Ok(CoinbaseSplitValue::V0(atom.as_ne_bytes().to_vec()));
    }

    // Cell - could be tagged or raw map
    let cell = noun.as_cell()?;

    // Check for tag
    if let Ok(tag) = cell.head().as_atom() {
        let tag_val = tag.as_u64().unwrap_or(u64::MAX);
        if tag_val == 0 {
            // V0 tagged: [%0 data]
            return Ok(CoinbaseSplitValue::V0(vec![]));
        }
        if tag_val == 1 {
            // V1 tagged: [%1 z-map]
            let entries = decode_coinbase_v1_map(&cell.tail())?;
            return Ok(CoinbaseSplitValue::V1(entries));
        }
    }

    // Untagged map - try v1 first (hash keys), fall back to v0 (sig keys)
    match decode_coinbase_v1_map(noun) {
        Ok(entries) => Ok(CoinbaseSplitValue::V1(entries)),
        Err(_) => {
            // v0 coinbase: (z-map sig coins) - just note it exists
            // We can't decode sigs as hashes, so just mark as v0 with raw bytes
            Ok(CoinbaseSplitValue::V0Raw(count_map_entries(noun)))
        }
    }
}

fn decode_coinbase_v1_map(noun: &Noun) -> Result<Vec<(Hash, u64)>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut entries = Vec::new();
    for entry in HoonMapIter::from(*noun) {
        if !entry.is_cell() {
            continue;
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun(&key)?;
        let coins = value.as_atom()?.as_u64()?;
        entries.push((hash, coins));
    }
    Ok(entries)
}

fn count_map_entries(noun: &Noun) -> usize {
    let mut count = 0;
    for entry in HoonMapIter::from(*noun) {
        if entry.is_cell() {
            count += 1;
        }
    }
    count
}

fn decode_page_msg(noun: &Noun) -> Result<PageMsgValue, NounDecodeError> {
    // PageMsg is a list of u32, we'll just get the raw bytes
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(PageMsgValue { raw: vec![] });
        }
        return Ok(PageMsgValue {
            raw: atom.as_ne_bytes().to_vec(),
        });
    }
    // List of u32
    let mut bytes = Vec::new();
    let mut current = *noun;
    while let Ok(cell) = current.as_cell() {
        if let Ok(val) = cell.head().as_atom() {
            let v = val.as_u64().unwrap_or(0) as u32;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        current = cell.tail();
    }
    Ok(PageMsgValue { raw: bytes })
}

// ============================================================================
// Proto Conversion
// ============================================================================

impl FullPageDetails {
    pub fn to_proto(&self) -> BlockDetails {
        BlockDetails {
            block_id: Some(hash_to_proto(&self.block_id)),
            height: self.height,
            parent: Some(hash_to_proto(&self.parent_id)),
            pow: Some(ProofOfWork {
                present: self.pow_present,
                raw_proof: self.pow_raw.clone(),
            }),
            timestamp: self.timestamp,
            epoch_counter: self.epoch_counter,
            target: Some(ProtoBigNum {
                display: self.target.to_display(),
                raw_bytes: self.target.raw_bytes.clone(),
                hex: self.target.to_hex(),
            }),
            accumulated_work: Some(ProtoBigNum {
                display: self.accumulated_work.to_display(),
                raw_bytes: self.accumulated_work.raw_bytes.clone(),
                hex: self.accumulated_work.to_hex(),
            }),
            tx_ids: self
                .tx_ids
                .iter()
                .map(|id| pb_common::Base58Hash {
                    hash: id.to_base58(),
                })
                .collect(),
            coinbase: Some(self.coinbase_to_proto()),
            msg: Some(self.msg_to_proto()),
            tx_count: self.tx_ids.len() as u32,
            has_pow: self.pow_present,
            version: self.version,
        }
    }

    fn coinbase_to_proto(&self) -> ProtoCoinbaseSplit {
        use crate::pb::public::v2::coinbase_split::Version as CbVersion;

        match &self.coinbase {
            CoinbaseSplitValue::V0(_bytes) => {
                // V0 coinbase - legacy format with bytes
                ProtoCoinbaseSplit {
                    version: Some(CbVersion::V0(ProtoCoinbaseSplitV0 {
                        entries: vec![],
                        entry_count: 0,
                        note: "V0 legacy coinbase".into(),
                    })),
                }
            }
            CoinbaseSplitValue::V0Raw(count) => {
                // V0 coinbase - we only know the entry count
                ProtoCoinbaseSplit {
                    version: Some(CbVersion::V0(ProtoCoinbaseSplitV0 {
                        entries: vec![],
                        entry_count: *count as u32,
                        note: "V0 coinbase (sig-based)".into(),
                    })),
                }
            }
            CoinbaseSplitValue::V1(entries) => ProtoCoinbaseSplit {
                version: Some(CbVersion::V1(ProtoCoinbaseSplitV1 {
                    entries: entries
                        .iter()
                        .map(|(hash, amount)| CoinbaseSplitV1Entry {
                            lock_hash: Some(pb_common::Base58Hash {
                                hash: hash.to_base58(),
                            }),
                            amount: Some(pb_common::Nicks { value: *amount }),
                        })
                        .collect(),
                })),
            },
        }
    }

    fn msg_to_proto(&self) -> ProtoPageMsg {
        let decoded = String::from_utf8(self.msg.raw.clone()).unwrap_or_default();
        ProtoPageMsg {
            raw: self.msg.raw.clone(),
            decoded,
        }
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
