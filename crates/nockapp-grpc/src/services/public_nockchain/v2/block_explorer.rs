use std::collections::{BTreeMap, HashMap, VecDeque};
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
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonMapIter;
#[cfg(test)]
use nockchain_types::tx_engine::common::Page;
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Name};
use nockchain_types::tx_engine::v0::{Lock, NoteV0, RawTx};
use nockvm::noun::{DirectAtom, Noun, NounAllocator, NounHandle, NounSpace, SIG};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::error::{NockAppGrpcError, Result as GrpcResult};
use crate::pb::common::v1 as pb_common;
use crate::public_nockchain::v2::metrics::NockchainGrpcApiMetrics;
use crate::public_nockchain::v2::server::BalanceHandle;

fn direct_height_noun(height: u64) -> GrpcResult<Noun> {
    DirectAtom::new(height)
        .map(DirectAtom::as_noun)
        .map_err(|_| {
            NockAppGrpcError::InvalidRequest("height exceeds maximum supported value".into())
        })
}

struct PageHeader {
    digest: Hash,
    parent: Hash,
    timestamp: u64,
    height: u64,
}

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
    pub raw_page_bytes: u64,
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

#[derive(Debug, PartialEq, Eq)]
enum RangeInsert {
    Inserted,
    Stale,
    Disconnected,
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

    /// Highest retained height; authoritative only while seed_ready is true.
    max_height: Arc<AtomicU64>,

    /// Last observed kernel tip height, including while retained indexes are stale.
    heaviest_height: Arc<AtomicU64>,

    /// Fences branch-dependent work at authority invalidation and replacement publication.
    chain_generation: AtomicU64,

    /// Last update timestamp
    last_updated: Arc<RwLock<Instant>>,
    refresh_success: AtomicU64,
    refresh_error: AtomicU64,
    backfill_success: AtomicU64,
    backfill_error: AtomicU64,

    metrics: Arc<NockchainGrpcApiMetrics>,
    /// Canonical-read authority. False may describe retained, quarantined indexes.
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
    // Keep each kernel peek small enough for block sync traffic to interleave
    // between cache slices without waiting on ~27s 1024-block decodes.
    const RANGE_CHUNK: u64 = 128;
    const INITIAL_SEED_RETRY_DELAY: Duration = Duration::from_secs(2);
    const INITIAL_SEED_MAX_WAIT: Duration = Duration::from_secs(120);
    const MIN_TX_PREFIX_LEN: usize = 8;
    const MAX_PREFIX_MATCHES: usize = 16;

    fn canonical_unavailable() -> NockAppGrpcError {
        NockAppGrpcError::Status(tonic::Status::unavailable(
            "Canonical explorer index is synchronizing; retry",
        ))
    }

    fn range_chunk_start(upper: u64) -> u64 {
        upper.saturating_sub(Self::RANGE_CHUNK - 1)
    }

    fn range_chunk_end(lower: u64, upper: u64) -> u64 {
        lower.saturating_add(Self::RANGE_CHUNK - 1).min(upper)
    }

    pub fn new(metrics: Arc<NockchainGrpcApiMetrics>) -> Self {
        Self {
            blocks_by_height: Arc::new(RwLock::new(BTreeMap::new())),
            blocks_by_id: Arc::new(RwLock::new(HashMap::new())),
            tx_to_block: Arc::new(RwLock::new(HashMap::new())),
            tx_b58_index: Arc::new(RwLock::new(BTreeMap::new())),
            max_height: Arc::new(AtomicU64::new(0)),
            heaviest_height: Arc::new(AtomicU64::new(0)),
            chain_generation: AtomicU64::new(0),
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
        let previous_generation = self.chain_generation.load(Ordering::Acquire);
        if !self
            .clear_cached_chain_for_generation(previous_generation)
            .await
        {
            return Ok(false);
        }
        self.seed_chain(handle, previous_generation.wrapping_add(1))
            .await
    }

    async fn seed_chain(
        self: Arc<Self>,
        handle: Arc<dyn BalanceHandle>,
        generation: u64,
    ) -> GrpcResult<bool> {
        if self.chain_generation.load(Ordering::Acquire) != generation {
            return Ok(false);
        }
        // Get current height (try heaviest-chain first, fall back to heaviest-block)
        let (max_height, tip_block_id) = match self.get_heaviest_tip(&handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty (both derived and consensus state); skipping cache initialization");
                self.metrics.block_explorer_seed_ready.swap(0.0);
                return Ok(false);
            }
            Err(err) => return Err(err),
        };
        if self.chain_generation.load(Ordering::Acquire) != generation {
            return Ok(false);
        }
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

        let first_start = Self::range_chunk_start(max_height);
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
        match self
            .insert_blocks_for_generation(
                first_chunk,
                generation,
                first_start,
                max_height,
                Some(&tip_block_id),
            )
            .await
        {
            RangeInsert::Inserted => {}
            RangeInsert::Stale => return Ok(false),
            RangeInsert::Disconnected => {
                self.clear_cached_chain_for_generation(generation).await;
                return Ok(false);
            }
        }

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

        let _chain = self.blocks_by_height.write().await;
        if self.chain_generation.load(Ordering::Acquire) != generation {
            return Ok(false);
        }
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
    pub async fn refresh(self: &Arc<Self>, handle: &Arc<dyn BalanceHandle>) -> GrpcResult<()> {
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
    async fn refresh_inner(self: &Arc<Self>, handle: &Arc<dyn BalanceHandle>) -> GrpcResult<()> {
        let (generation, empty, ready) = {
            let chain = self.blocks_by_height.read().await;
            (
                self.chain_generation.load(Ordering::Acquire),
                chain.is_empty(),
                self.seed_ready.load(Ordering::Acquire),
            )
        };
        if !ready && empty {
            Arc::clone(self)
                .seed_chain(Arc::clone(handle), generation)
                .await?;
            return Ok(());
        }
        // Get current height (try heaviest-chain first, fall back to heaviest-block)
        let (current_height, current_tip_id) = match self.get_heaviest_tip(handle).await {
            Ok(val) => val,
            Err(NockAppGrpcError::PeekReturnedNoData) => {
                debug!("Heaviest chain is empty (both derived and consensus state); skipping cache refresh");
                self.metrics.block_explorer_refresh_error.increment();
                self.refresh_error.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        let (last_height, cached_tip_id, ready) = {
            let chain = self.blocks_by_height.read().await;
            if self.chain_generation.load(Ordering::Acquire) != generation {
                return Ok(());
            }
            // Observation is diagnostic until a validated index snapshot is published.
            self.heaviest_height
                .store(current_height, Ordering::Release);
            let last_height = self.max_height.load(Ordering::Acquire);
            (
                last_height,
                chain.get(&last_height).map(|block| block.block_id.clone()),
                self.seed_ready.load(Ordering::Acquire),
            )
        };
        tracing::Span::current().record("current_height", &tracing::field::display(current_height));
        tracing::Span::current().record("last_height", &tracing::field::display(last_height));

        // Retained storage cannot become authoritative through an unchanged-tip
        // shortcut, forward append, or a reversion to its original tip identity.
        if !ready {
            self.reconcile_chain(handle, generation, current_height, &current_tip_id)
                .await?;
            return Ok(());
        }

        if last_height > 0 {
            let canonical_at_cached_tip = if current_height < last_height {
                None
            } else if current_height == last_height {
                Some(current_tip_id.clone())
            } else {
                self.peek_blocks_range(handle, last_height, last_height)
                    .await?
                    .into_iter()
                    .find(|(height, ..)| *height == last_height)
                    .map(|(_, block_id, ..)| block_id)
            };
            if current_height < last_height || cached_tip_id != canonical_at_cached_tip {
                warn!(
                    cached_height = last_height,
                    canonical_height = current_height,
                    "Block explorer cache crossed a chain reorganization; reconciling canonical indexes"
                );
                self.reconcile_chain(handle, generation, current_height, &current_tip_id)
                    .await?;
                return Ok(());
            }
        }

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
            let chunk_end = Self::range_chunk_end(chunk_start, current_height);
            match self.peek_blocks_range(handle, chunk_start, chunk_end).await {
                Ok(blocks) => {
                    inserted += blocks.len();
                    let expected_tip = (chunk_end == current_height).then_some(&current_tip_id);
                    match self
                        .insert_blocks_for_generation(
                            blocks, generation, chunk_start, chunk_end, expected_tip,
                        )
                        .await
                    {
                        RangeInsert::Inserted => {}
                        RangeInsert::Stale => return Ok(()),
                        RangeInsert::Disconnected => {
                            if self.clear_cached_chain_for_generation(generation).await {
                                Arc::clone(self)
                                    .seed_chain(Arc::clone(handle), generation.wrapping_add(1))
                                    .await?;
                            }
                            return Ok(());
                        }
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
    /// Get descending blocks, their next cursor and current height from one authoritative snapshot.
    #[tracing::instrument(
        name = "block_explorer_cache.get_blocks_page",
        skip(self),
        fields(start_height = tracing::field::Empty)
    )]
    pub async fn get_blocks_page(
        &self,
        cursor: Option<u64>,
        limit: usize,
    ) -> GrpcResult<(Vec<BlockMetadata>, Option<u64>, u64)> {
        let blocks = self.blocks_by_height.read().await;
        if !self.seed_ready.load(Ordering::Acquire) {
            return Err(Self::canonical_unavailable());
        }
        // Retained storage is not canonical-read authority during reconciliation.
        // In an authoritative snapshot, retain the existing observed-tip clamp.
        let current_height = self
            .max_height
            .load(Ordering::Acquire)
            .min(self.heaviest_height.load(Ordering::Acquire));
        let start_height = cursor.unwrap_or(current_height).min(current_height);
        tracing::Span::current().record("start_height", &tracing::field::display(start_height));

        let page: Vec<_> = blocks
            .range(..=start_height)
            .rev()
            .take(limit)
            .map(|(_, block)| block.clone())
            .collect();

        let next_cursor = page.last().map(|b| b.height.saturating_sub(1));
        Ok((page, next_cursor.filter(|h| *h > 0), current_height))
    }

    /// A raw transaction is retained after confirmation, so only a complete
    /// canonical index can distinguish a pending transaction from a cache miss.
    pub(super) async fn lookup_transaction_block(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        tx_id: &Hash,
    ) -> GrpcResult<BlockMetadata> {
        let generation = {
            let chain = self.blocks_by_height.read().await;
            if !self.seed_ready.load(Ordering::Acquire) {
                return Err(Self::canonical_unavailable());
            }
            let generation = self.chain_generation.load(Ordering::Acquire);
            if let Some((block_id, height)) = self.tx_to_block.read().await.get(tx_id) {
                if *height <= self.heaviest_height.load(Ordering::Acquire) {
                    if let Some(block) = chain
                        .get(height)
                        .filter(|block| block.block_id == *block_id)
                    {
                        return Ok(block.clone());
                    }
                }
            }
            generation
        };
        // Stage errors too: a chain change during the peek must not be exposed
        // as a definitive absence or pending classification for a stale index.
        let pending = self.transaction_pending(handle, tx_id).await;
        let chain = self.blocks_by_height.read().await;
        if self.chain_generation.load(Ordering::Acquire) != generation
            || !self.seed_ready.load(Ordering::Acquire)
            || chain.first_key_value().map(|(height, _)| *height) != Some(0)
            || chain.last_key_value().map(|(height, _)| *height)
                != Some(self.heaviest_height.load(Ordering::Acquire))
        {
            return Err(NockAppGrpcError::Status(tonic::Status::unavailable(
                "Transaction confirmation index is incomplete; retry after explorer synchronization",
            )));
        }
        // A forward refresh may have confirmed the transaction during the peek.
        if let Some((block_id, height)) = self.tx_to_block.read().await.get(tx_id) {
            if let Some(block) = chain
                .get(height)
                .filter(|block| block.block_id == *block_id)
            {
                return Ok(block.clone());
            }
        }
        Err(if pending? {
            NockAppGrpcError::TxPending
        } else {
            NockAppGrpcError::NotFound
        })
    }

    async fn reconcile_chain(
        &self,
        handle: &Arc<dyn BalanceHandle>,
        generation: u64,
        height: u64,
        tip: &Hash,
    ) -> GrpcResult<()> {
        let generation = {
            let _chain = self.blocks_by_height.write().await;
            if self.chain_generation.load(Ordering::Acquire) != generation {
                return Ok(());
            }
            if self.seed_ready.load(Ordering::Acquire) {
                // Retain all indexes, cached coverage and pending history, but
                // revoke canonical authority before waiting on replacement IO.
                self.seed_ready.store(false, Ordering::Release);
                self.metrics.block_explorer_seed_ready.swap(0.0);
                self.chain_generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1)
            } else {
                generation
            }
        };
        let start = Self::range_chunk_start(height);
        let blocks = self.peek_blocks_range(handle, start, height).await?;
        if !Self::valid_range(&blocks, start, height, Some(tip)) {
            self.clear_cached_chain_for_generation(generation).await;
            return Ok(());
        }
        let mut by_height = self.blocks_by_height.write().await;
        let mut by_id = self.blocks_by_id.write().await;
        let mut tx_index = self.tx_to_block.write().await;
        let mut tx_b58 = self.tx_b58_index.write().await;
        if self.chain_generation.load(Ordering::Acquire) != generation {
            return Ok(());
        }
        // Matching block identities anchor the entire cached ancestry. With no
        // match in this bounded window, none of the old history is trusted.
        let common = blocks
            .iter()
            .rev()
            .find(|(height, id, ..)| {
                by_height
                    .get(height)
                    .is_some_and(|block| block.block_id == *id)
            })
            .map(|(height, ..)| *height)
            .or_else(|| {
                let first = blocks.first()?;
                let parent_height = first.0.checked_sub(1)?;
                by_height
                    .get(&parent_height)
                    .filter(|block| block.block_id == first.2)
                    .map(|_| parent_height)
            });
        self.chain_generation.fetch_add(1, Ordering::AcqRel);
        let orphaned = match common {
            Some(common) => by_height.split_off(&common.saturating_add(1)),
            None => std::mem::take(&mut *by_height),
        };
        for block in orphaned.into_values() {
            by_id.remove(&block.block_id);
            for tx in block.tx_ids {
                tx_index.remove(&tx);
                tx_b58.remove(&tx.to_base58());
            }
        }
        for (height, block_id, parent_id, timestamp, tx_ids) in blocks {
            if by_height.contains_key(&height) {
                continue;
            }
            for tx in &tx_ids {
                tx_index.insert(tx.clone(), (block_id.clone(), height));
                tx_b58.insert(tx.to_base58(), tx.clone());
            }
            let block = BlockMetadata {
                height,
                block_id,
                parent_id,
                timestamp,
                tx_ids,
            };
            by_id.insert(block.block_id.clone(), block.clone());
            by_height.insert(height, block);
        }
        // Actual coverage includes chunks committed before their cursor update.
        let lowest = *by_height.first_key_value().expect("validated seed").0;
        let resume = lowest.checked_sub(1);
        *self.backfill_resume.write().await = resume;
        self.max_height.store(height, Ordering::Release);
        self.heaviest_height.store(height, Ordering::Release);
        self.metrics.block_explorer_cache_height.swap(height as f64);
        self.metrics
            .block_explorer_cache_lowest_height
            .swap(lowest as f64);
        self.metrics
            .block_explorer_cache_span
            .swap(height.saturating_sub(lowest) as f64);
        self.metrics
            .block_explorer_cache_coverage_ratio
            .swap(height.saturating_sub(lowest) as f64 / height.max(1) as f64);
        self.metrics
            .block_explorer_backfill_resume_height
            .swap(resume.map_or(-1.0, |height| height as f64));
        *self.last_updated.write().await = Instant::now();
        self.metrics.block_explorer_seed_ready.swap(1.0);
        // Height-locked readers see authority only after the full state is ready.
        self.seed_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// Lookup block for transaction
    #[tracing::instrument(name = "block_explorer_cache.get_block_for_tx", skip(self))]
    pub async fn get_block_for_tx(&self, tx_id: &Hash) -> Option<BlockMetadata> {
        let chain = self.blocks_by_height.read().await;
        if !self.seed_ready.load(Ordering::Acquire) {
            return None;
        }
        let (block_id, height) = self.tx_to_block.read().await.get(tx_id).cloned()?;
        if height > self.heaviest_height.load(Ordering::Acquire) {
            return None;
        }

        chain
            .get(&height)
            .filter(|block| block.block_id == block_id)
            .cloned()
    }

    #[cfg(test)]
    async fn clear_cached_chain(&self) {
        let generation = self.chain_generation.load(Ordering::Acquire);
        self.clear_cached_chain_for_generation(generation).await;
    }

    async fn clear_cached_chain_for_generation(&self, generation: u64) -> bool {
        let mut by_height = self.blocks_by_height.write().await;
        let mut by_id = self.blocks_by_id.write().await;
        let mut tx_index = self.tx_to_block.write().await;
        let mut tx_b58 = self.tx_b58_index.write().await;
        if self.chain_generation.load(Ordering::Acquire) != generation {
            return false;
        }
        self.chain_generation.fetch_add(1, Ordering::AcqRel);
        by_height.clear();
        by_id.clear();
        tx_index.clear();
        tx_b58.clear();
        self.max_height.store(0, Ordering::Release);
        self.seed_ready.store(false, Ordering::Release);
        *self.backfill_resume.write().await = None;
        self.metrics.block_explorer_cache_height.swap(0.0);
        self.metrics.block_explorer_cache_lowest_height.swap(0.0);
        self.metrics.block_explorer_cache_span.swap(0.0);
        self.metrics.block_explorer_cache_coverage_ratio.swap(0.0);
        self.metrics.block_explorer_seed_ready.swap(0.0);
        self.metrics
            .block_explorer_backfill_resume_height
            .swap(-1.0);
        true
    }

    /// Get the highest retained height (not a canonical RPC snapshot).
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
        let generation = {
            let chain = self.blocks_by_height.read().await;
            if !self.seed_ready.load(Ordering::Acquire) && !chain.is_empty() {
                return Err(Self::canonical_unavailable());
            }
            self.chain_generation.load(Ordering::Acquire)
        };
        let result = self.peek_full_page(handle, height).await;
        let chain = self.blocks_by_height.read().await;
        if self.chain_generation.load(Ordering::Acquire) != generation
            || (!self.seed_ready.load(Ordering::Acquire) && !chain.is_empty())
        {
            return Err(Self::canonical_unavailable());
        }
        result
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

        let (generation, height) = {
            let _chain = self.blocks_by_height.read().await;
            if !self.seed_ready.load(Ordering::Acquire) {
                return Err(Self::canonical_unavailable());
            }
            let blocks = self.blocks_by_id.read().await;
            let meta = blocks.get(&hash).ok_or(NockAppGrpcError::NotFound)?;
            if meta.height > self.heaviest_height.load(Ordering::Acquire) {
                return Err(NockAppGrpcError::NotFound);
            }
            (self.chain_generation.load(Ordering::Acquire), meta.height)
        };
        let result = self.peek_full_page(handle, height).await;
        let _chain = self.blocks_by_height.read().await;
        if self.chain_generation.load(Ordering::Acquire) != generation
            || !self.seed_ready.load(Ordering::Acquire)
        {
            return Err(Self::canonical_unavailable());
        }
        let page = result?;
        // A kernel height read must still match the requested cached identity.
        if page.block_id != hash {
            return Err(NockAppGrpcError::NotFound);
        }
        Ok(page)
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
        let height_noun = direct_height_noun(height)?;
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, height_noun, height_noun, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };
        let space = result.noun_space();
        let result_noun_handle = result_noun.in_space(&space);
        tracing::debug!(
            noun_is_atom = result_noun.is_atom(),
            "peek_full_page raw result"
        );

        let entries = decode_optional_optional_vec_handles(result_noun_handle).map_err(|e| {
            tracing::error!("Failed to decode full page entry list: {:?}", e);
            NockAppGrpcError::NounDecode(e)
        })?;
        let entries = entries.ok_or(NockAppGrpcError::PeekReturnedNoData)?;

        let parsed: Vec<FullPageDetails> = entries
            .into_iter()
            .map(|entry| {
                let entry_height = entry
                    .slot(2)
                    .ok()
                    .and_then(|noun| noun.as_atom().ok())
                    .and_then(|atom| atom.as_u64().ok())
                    .unwrap_or(u64::MAX);
                FullPageDetails::from_noun_handle(&entry).map_err(|e| {
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

        let _chain = self.blocks_by_height.read().await;
        if !self.seed_ready.load(Ordering::Acquire) {
            return Err(Self::canonical_unavailable());
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
        let generation = {
            let _chain = self.blocks_by_height.read().await;
            if !self.seed_ready.load(Ordering::Acquire) {
                return Err(Self::canonical_unavailable());
            }
            self.chain_generation.load(Ordering::Acquire)
        };
        let result = async {
            let block_meta = self.lookup_transaction_block(handle, tx_id).await?;
            self.fetch_transaction_from_block(handle, &block_meta, tx_id)
                .await
        }
        .await;
        let _chain = self.blocks_by_height.read().await;
        if self.chain_generation.load(Ordering::Acquire) != generation
            || !self.seed_ready.load(Ordering::Acquire)
        {
            return Err(Self::canonical_unavailable());
        }
        result
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
        // The height-range scry returns raw transactions. Their noun shape is
        // not the validated tx shape used by DecodedTx, so query the exact
        // block/tx pair from the kernel's validated transaction map instead.
        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "block-transaction").as_noun();
        let block_id = meta.block_id.to_base58().to_noun(&mut path_slab);
        let tx_id_noun = tx_id.to_base58().to_noun(&mut path_slab);
        let path = nockvm::noun::T(&mut path_slab, &[tag, block_id, tx_id_noun, SIG]);
        path_slab.set_root(path);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::NotFound)?;
        let space = result.noun_space();
        let result = unsafe { *result.root() }.in_space(&space);
        let Some(inner) = decode_option_handle(result).map_err(NockAppGrpcError::NounDecode)?
        else {
            return Err(NockAppGrpcError::NotFound);
        };
        let Some(tx) = decode_option_handle(inner).map_err(NockAppGrpcError::NounDecode)? else {
            return Err(NockAppGrpcError::NotFound);
        };
        let decoded = DecodedTx::from_noun_handle(&tx).map_err(NockAppGrpcError::NounDecode)?;
        Ok(build_transaction_details(meta, tx_id, decoded))
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
        let space = result.noun_space();

        // Decode Option<Option<(BlockHeight, Hash)>>
        let opt: Option<Option<(BlockHeight, Hash)>> = NounDecode::from_noun(&result_noun, &space)
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
        let space = result.noun_space();
        let result_noun_handle = result_noun.in_space(&space);
        let page_noun = decode_option_handle(result_noun_handle)
            .and_then(|outer| outer.map(decode_option_handle).transpose())
            .map_err(|e| {
                error!(
                    "Failed to decode heaviest-block peek result.\n\
                 Decode error: {:?}\n\
                 Expected: Option<Option<Page>>",
                    e
                );
                NockAppGrpcError::NounDecode(e)
            })?
            .flatten()
            .ok_or_else(|| {
                debug!("peek_heaviest_block returned None/empty");
                NockAppGrpcError::PeekReturnedNoData
            })?;
        let page = decode_page_header(page_noun).map_err(NockAppGrpcError::NounDecode)?;
        debug!("peek_heaviest_block success: height={}", page.height);
        Ok((page.height, page.digest))
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

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;
        let space = result.noun_space();
        let result = unsafe { *result.root() }.in_space(&space);
        // The outer unit reports a valid scry path; the inner unit reports
        // whether the raw transaction exists. A successful peek alone does not.
        let inner = decode_option_handle(result)
            .map_err(NockAppGrpcError::NounDecode)?
            .ok_or(NockAppGrpcError::PeekReturnedNoData)?;
        Ok(decode_option_handle(inner)
            .map_err(NockAppGrpcError::NounDecode)?
            .is_some())
    }

    #[tracing::instrument(name = "block_explorer_cache.backfill_older", skip(self, handle))]
    pub async fn backfill_older(
        self: Arc<Self>,
        handle: Arc<dyn BalanceHandle>,
        generation: u64,
        mut upper: u64,
    ) -> GrpcResult<()> {
        loop {
            if self.chain_generation.load(Ordering::Acquire) != generation
                || !self.seed_ready.load(Ordering::Acquire)
            {
                return Ok(());
            }
            let start = Self::range_chunk_start(upper);
            let result = self.peek_blocks_range(&handle, start, upper).await;
            if self.chain_generation.load(Ordering::Acquire) != generation
                || !self.seed_ready.load(Ordering::Acquire)
            {
                return Ok(());
            }
            let chunk = match result {
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

            let inserted = chunk.len();
            match self
                .insert_blocks_for_generation(chunk, generation, start, upper, None)
                .await
            {
                RangeInsert::Inserted => {}
                RangeInsert::Stale => return Ok(()),
                RangeInsert::Disconnected => {
                    self.clear_cached_chain_for_generation(generation).await;
                    return Ok(());
                }
            }
            // Keep the cursor pending until this generation has committed it.
            // A failed worker can resume, and a stale worker cannot erase new work.
            let chain = self.blocks_by_height.write().await;
            if self.chain_generation.load(Ordering::Acquire) != generation
                || !self.seed_ready.load(Ordering::Acquire)
            {
                return Ok(());
            }
            *self.backfill_resume.write().await = start.checked_sub(1);
            self.metrics
                .block_explorer_backfill_resume_height
                .swap(start.checked_sub(1).map_or(-1.0, |height| height as f64));
            drop(chain);
            info!(
                range_start = start,
                range_end = upper,
                inserted_blocks = inserted,
                "Backfilled block explorer cache range"
            );
            if start == 0 {
                break;
            }
            upper = start.saturating_sub(1);
            tokio::task::yield_now().await;
        }

        let _chain = self.blocks_by_height.write().await;
        if self.chain_generation.load(Ordering::Acquire) != generation
            || !self.seed_ready.load(Ordering::Acquire)
        {
            return Ok(());
        }
        self.metrics.block_explorer_backfill_success.increment();
        self.backfill_success.fetch_add(1, Ordering::Relaxed);
        *self.last_backfill.write().await = Some(Instant::now());
        Ok(())
    }

    pub(super) async fn pending_backfill(&self) -> Option<(u64, u64)> {
        let _chain = self.blocks_by_height.read().await;
        if !self.seed_ready.load(Ordering::Acquire) {
            return None;
        }
        let upper = (*self.backfill_resume.read().await)?;
        Some((self.chain_generation.load(Ordering::Acquire), upper))
    }

    #[cfg(test)]
    async fn insert_blocks(&self, blocks: Vec<(u64, Hash, Hash, u64, Vec<Hash>)>) {
        // Fixtures may deliberately model the inconsistent state under test.
        for (height, block_id, parent_id, timestamp, tx_ids) in blocks {
            let metadata = BlockMetadata {
                height,
                block_id: block_id.clone(),
                parent_id,
                timestamp,
                tx_ids: tx_ids.clone(),
            };
            self.blocks_by_height
                .write()
                .await
                .insert(height, metadata.clone());
            self.blocks_by_id
                .write()
                .await
                .insert(block_id.clone(), metadata);
            for tx_id in tx_ids {
                self.tx_to_block
                    .write()
                    .await
                    .insert(tx_id.clone(), (block_id.clone(), height));
                self.tx_b58_index
                    .write()
                    .await
                    .insert(tx_id.to_base58(), tx_id);
            }
            self.max_height.fetch_max(height, Ordering::Release);
        }
    }

    fn valid_range(
        blocks: &[(u64, Hash, Hash, u64, Vec<Hash>)],
        start: u64,
        end: u64,
        expected_tip: Option<&Hash>,
    ) -> bool {
        let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else {
            return false;
        };
        first.0 == start
            && last.0 == end
            && !blocks
                .windows(2)
                .any(|pair| pair[0].0.checked_add(1) != Some(pair[1].0) || pair[1].2 != pair[0].1)
            && expected_tip.is_none_or(|tip| last.1 == *tip)
    }

    #[tracing::instrument(
        name = "block_explorer_cache.insert_blocks",
        skip(self, blocks, expected_tip),
        fields(batch_size = tracing::field::Empty)
    )]
    async fn insert_blocks_for_generation(
        &self,
        blocks: Vec<(u64, Hash, Hash, u64, Vec<Hash>)>,
        expected_generation: u64,
        start: u64,
        end: u64,
        expected_tip: Option<&Hash>,
    ) -> RangeInsert {
        let batch_size = blocks.len();
        tracing::Span::current().record("batch_size", &tracing::field::display(batch_size));

        let mut by_height_guard = self.blocks_by_height.write().await;
        let mut by_id_guard = self.blocks_by_id.write().await;
        let mut tx_index_guard = self.tx_to_block.write().await;
        let mut tx_b58_guard = self.tx_b58_index.write().await;

        // Initial seeding publishes into an empty index before authority exists.
        // All subsequent range commits require this generation's authority.
        if self.chain_generation.load(Ordering::Acquire) != expected_generation
            || (!self.seed_ready.load(Ordering::Acquire) && !by_height_guard.is_empty())
        {
            return RangeInsert::Stale;
        }
        // Each range is one kernel read, but distinct reads need not share a
        // chain snapshot. Validate both its contents and its cached-chain join
        // before publishing any of the block or transaction indexes.
        if !Self::valid_range(&blocks, start, end, expected_tip) {
            return RangeInsert::Disconnected;
        }
        let first = blocks.first().expect("validated range");
        let last = blocks.last().expect("validated range");
        if let (Some((_, lowest)), Some((_, highest))) = (
            by_height_guard.first_key_value(),
            by_height_guard.last_key_value(),
        ) {
            let extends_tip =
                highest.height.checked_add(1) == Some(first.0) && first.2 == highest.block_id;
            let extends_history =
                last.0.checked_add(1) == Some(lowest.height) && lowest.parent_id == last.1;
            if !extends_tip && !extends_history {
                return RangeInsert::Disconnected;
            }
        }

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

        // Publish the retained high-water mark under the height-first writer
        // fence shared with authority invalidation and branch replacement.
        self.max_height.fetch_max(max_in_batch, Ordering::Release);
        drop(tx_index_guard);
        drop(tx_b58_guard);
        drop(by_id_guard);
        drop(by_height_guard);

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
        RangeInsert::Inserted
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
        let start_noun = direct_height_noun(start)?;
        let end_noun = direct_height_noun(end)?;
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, start_noun, end_noun, SIG]);
        path_slab.set_root(path_noun);

        let result = handle
            .peek(path_slab)
            .await
            .map_err(NockAppGrpcError::from)?
            .ok_or(NockAppGrpcError::PeekFailed)?;

        let result_noun = unsafe { result.root() };
        let space = result.noun_space(); // Debug: log raw noun structure
        let is_atom = result_noun.is_atom();
        let is_cell = result_noun.is_cell();
        if is_atom {
            if let Ok(atom) = result_noun.as_atom() {
                let val = atom.in_space(&space).as_u64().unwrap_or(u64::MAX);
                info!(
                    is_atom,
                    atom_val = val,
                    "peek_blocks_range raw result is atom"
                );
            }
        } else if is_cell {
            if let Ok(cell) = result_noun.as_cell() {
                let head_is_atom = cell.in_space(&space).head().is_atom();
                let tail_is_atom = cell.in_space(&space).tail().is_atom();
                info!(
                    head_is_atom,
                    tail_is_atom, "peek_blocks_range raw result is cell"
                );
            }
        }

        // Decode Option<Option<Vec<(height, block-id, page, txs)>>>
        // We need to extract fields from the page and txs
        let decoded_entries =
            decode_optional_optional_vec_handles(result_noun.in_space(&space)).map_err(|e| {
                // Log detailed noun structure to help diagnose format issues
                let noun_debug = if result_noun.is_atom() {
                    format!(
                        "atom (value: {:?})",
                        result_noun.as_atom().ok().and_then(|a| a.in_space(&space).as_u64().ok())
                    )
                } else if let Ok(cell) = result_noun.as_cell() {
                    let head_type = if cell.in_space(&space).head().is_atom() {
                        "atom"
                    } else {
                        "cell"
                    };
                    let tail_type = if cell.in_space(&space).tail().is_atom() {
                        "atom"
                    } else {
                        "cell"
                    };
                    format!("cell [head={}, tail={}]", head_type, tail_type)
                } else {
                    "unknown".to_string()
                };
                error!(
                    "Failed to decode block range entry list.\n\
                     Decode error: {:?}\n\
                     Result noun structure: {}\n\
                     This is likely a Page decoder issue - check tx_ids (z-set vs list), bignum, or coinbase format.",
                    e, noun_debug
                );
                NockAppGrpcError::NounDecode(e)})?;

        let outer_some = result_noun.is_cell();
        let inner_some = decoded_entries.is_some();
        info!(
            outer_some,
            inner_some, "peek_blocks_range decoded outer options"
        );

        let entries = decoded_entries.ok_or_else(|| {
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
                BlockRangeEntry::from_noun_handle(&entry).map_err(|e| {
                    error!("Failed to convert block range entry: {:?}", e);
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

fn decode_option_handle<'a>(
    noun: NounHandle<'a>,
) -> Result<Option<NounHandle<'a>>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(None);
        }
        return Err(NounDecodeError::Custom("Invalid Option encoding".into()));
    }

    let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
    let head = cell
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::ExpectedAtom)?;
    if head.as_u64()? != 0 {
        return Err(NounDecodeError::Custom(
            "Invalid Option encoding - expected ~".into(),
        ));
    }
    Ok(Some(cell.tail()))
}

fn decode_vec_handles<'a>(noun: NounHandle<'a>) -> Result<Vec<NounHandle<'a>>, NounDecodeError> {
    let mut result = Vec::new();
    let mut current = noun;

    while let Ok(cell) = current.as_cell() {
        result.push(cell.head());
        current = cell.tail();
    }

    let atom = current
        .as_atom()
        .map_err(|_| NounDecodeError::ExpectedAtom)?;
    if atom.as_u64()? != 0 {
        return Err(NounDecodeError::Custom("Invalid list termination".into()));
    }

    Ok(result)
}

fn decode_optional_optional_vec_handles<'a>(
    noun: NounHandle<'a>,
) -> Result<Option<Vec<NounHandle<'a>>>, NounDecodeError> {
    let Some(inner) = decode_option_handle(noun)? else {
        return Ok(None);
    };
    let Some(list) = decode_option_handle(inner)? else {
        return Ok(None);
    };
    Ok(Some(decode_vec_handles(list)?))
}

#[cfg(test)]
#[derive(Debug, Clone, NounEncode, NounDecode)]
struct BlockRangeEntryNoun {
    height: BlockHeight,
    tail: BlockRangeEntryTail,
}

#[cfg(test)]
#[derive(Debug, Clone, NounEncode, NounDecode)]
struct BlockRangeEntryTail {
    block_id: Hash,
    tail: PageAndTxs,
}

#[cfg(test)]
#[derive(Debug, Clone, NounEncode, NounDecode)]
struct PageAndTxs {
    page: Page,
    txs: Vec<RawTx>,
}

fn decode_page_header(page: NounHandle) -> Result<PageHeader, NounDecodeError> {
    let cell = page.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
    let (digest, rest_after_digest) =
        if let Ok(version_atom) = cell.head().as_atom() {
            let version = version_atom.as_u64()?;
            if version != 1 {
                return Err(NounDecodeError::Custom(format!(
                    "unknown page version: {version}"
                )));
            }
            let rest = cell.tail().as_cell().map_err(|_| {
                NounDecodeError::Custom("v1 page: expected cell after version".into())
            })?;
            let digest = Hash::from_noun_handle(&rest.head())
                .map_err(|err| NounDecodeError::Custom(format!("v1 page digest: {err}")))?;
            let rest_after_digest = rest.tail().as_cell().map_err(|_| {
                NounDecodeError::Custom("v1 page: expected cell after digest".into())
            })?;
            (digest, rest_after_digest)
        } else {
            let digest = Hash::from_noun_handle(&cell.head())
                .map_err(|err| NounDecodeError::Custom(format!("v0 page digest: {err}")))?;
            let rest_after_digest = cell.tail().as_cell().map_err(|_| {
                NounDecodeError::Custom("v0 page: expected cell after digest".into())
            })?;
            (digest, rest_after_digest)
        };

    let after_pow = rest_after_digest
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after pow".into()))?;
    let parent = Hash::from_noun_handle(&after_pow.head())
        .map_err(|err| NounDecodeError::Custom(format!("page parent: {err}")))?;

    let after_tx_ids = after_pow
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after parent".into()))?
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after tx ids".into()))?;
    let after_coinbase = after_tx_ids
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after coinbase".into()))?;
    let timestamp = after_coinbase
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("page timestamp: expected atom".into()))?
        .as_u64()?;
    let after_timestamp = after_coinbase
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after timestamp".into()))?;
    let after_epoch_counter = after_timestamp
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after epoch counter".into()))?;
    let after_target = after_epoch_counter
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after target".into()))?;
    let height = after_target
        .tail()
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("page: expected cell after accumulated work".into()))?
        .head()
        .as_atom()
        .map_err(|_| NounDecodeError::Custom("page height: expected atom".into()))?
        .as_u64()?;

    Ok(PageHeader {
        digest,
        parent,
        timestamp,
        height,
    })
}

impl BlockRangeEntry {
    fn from_noun_handle(noun: &NounHandle) -> std::result::Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let height = BlockHeight::from_noun_handle(&cell.head())?;

        let tail = cell
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let block_id = Hash::from_noun_handle(&tail.head())?;

        let page_and_txs = tail
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let page_header = decode_page_header(page_and_txs.head())?;
        let tx_ids = extract_tx_ids_from_map(&page_and_txs.tail())?;

        Ok(Self {
            height: height.0 .0,
            block_id,
            parent_id: page_header.parent,
            timestamp: page_header.timestamp,
            tx_ids,
        })
    }
}

/// Extract transaction IDs from transactions map (z-map tx-id tx)
fn extract_tx_ids_from_map(
    txs_noun: &NounHandle,
) -> std::result::Result<Vec<Hash>, noun_serde::NounDecodeError> {
    // Check if it's an empty map (atom 0)
    if let Ok(atom) = txs_noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    // Iterate over the z-map and collect keys (tx-ids)
    let mut tx_ids = Vec::new();
    for (idx, entry) in HoonMapIter::new(txs_noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "extract_tx_ids_from_map: entry {} is not a cell (expected z-map [key value] pair)",
                idx
            )));
        }
        let [key, _value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun_handle(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "extract_tx_ids_from_map: failed to decode tx_id at entry {}: {}",
                idx, e
            ))
        })?;
        tx_ids.push(hash);
    }

    Ok(tx_ids)
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
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun.in_space(space).as_cell()?;
        let version = u64::from_noun_handle(&cell.head())?;

        match version {
            0 => {
                // v0 tx: [%0 raw-tx:v0 total-size outputs:v0]
                let tail = cell.tail();
                let cell = tail.as_cell()?;
                let raw_tx = RawTx::from_noun_handle(&cell.head())?;

                let tail = cell.tail();
                let cell = tail.as_cell()?;
                let total_size = u64::from_noun_handle(&cell.head())?;
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
                let raw_version = u64::from_noun_handle(&raw_cell.head()).map_err(|e| {
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
                let _tx_id = Hash::from_noun_handle(&raw_tail.head()).map_err(|e| {
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
                let total_size = u64::from_noun_handle(&cell.head()).map_err(|e| {
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
fn decode_v1_spends(noun: &NounHandle) -> Result<(Vec<TxV1Input>, u64), NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok((Vec::new(), 0));
        }
    }

    let mut inputs = Vec::new();
    let mut total_fee = 0u64;
    for (idx, entry) in HoonMapIter::new(noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "decode_v1_spends: entry {} is not a cell (expected z-map [nname spend] pair)",
                idx
            )));
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        // key is nname (Name type)
        let name = Name::from_noun_handle(&key).map_err(|e| {
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

fn decode_outputs_v1(noun: &NounHandle) -> Result<Vec<TxV1Output>, NounDecodeError> {
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
    for (idx, entry) in HoonMapIter::new(&zset_noun).enumerate() {
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

        let note_version = u64::from_noun_handle(&note_head).map_err(|e| {
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
        let _origin_page = u64::from_noun_handle(&note_tail.head()).map_err(|e| {
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
        let name = Name::from_noun_handle(&note_tail.head()).map_err(|e| {
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
        let assets = u64::from_noun_handle(&note_tail.tail()).map_err(|e| {
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

fn decode_outputs_v0(noun: &NounHandle) -> Result<Vec<TxOutputV0>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut outputs = Vec::new();
    for (idx, entry) in HoonMapIter::new(noun).enumerate() {
        if !entry.is_cell() {
            return Err(NounDecodeError::Custom(format!(
                "decode_outputs_v0: entry {} is not a cell (expected z-map [lock note] pair)",
                idx
            )));
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let lock = Lock::from_noun_handle(&key).map_err(|e| {
            NounDecodeError::Custom(format!(
                "decode_outputs_v0: output {} lock parse failed: {}",
                idx, e
            ))
        })?;
        let value_cell = value.as_cell().map_err(|_| {
            NounDecodeError::Custom(format!("decode_outputs_v0: output {} value not cell", idx))
        })?;
        let note = NoteV0::from_noun_handle(&value_cell.head()).map_err(|e| {
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

impl FullPageDetails {
    fn from_noun_handle(noun: &NounHandle) -> std::result::Result<Self, NounDecodeError> {
        let cell = noun.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let height = BlockHeight::from_noun_handle(&cell.head())?.0 .0;

        let tail = cell
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let block_id = Hash::from_noun_handle(&tail.head())?;

        let page_and_txs = tail
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::ExpectedCell)?;
        let page = page_and_txs.head();
        let txs_noun = page_and_txs.tail();

        let page_cell = page.as_cell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let version_or_digest = page_cell.head();
        let rest = page_cell.tail();
        let raw_page_bytes = jammed_noun_len(page.noun(), page.space()) as u64;

        // Determine if v0 or v1 page:
        // - v0 page: [digest pow parent ...] where digest is a Hash (cell of 5 Belts)
        // - v1 page: [%1 digest pow parent ...] where %1 is an atom
        // So if head is an atom, it's v1; if head is a cell (Hash), it's v0
        let head_is_atom = version_or_digest.is_atom();
        let head_is_cell = version_or_digest.is_cell();
        let head_as_u64 = version_or_digest
            .as_atom()
            .ok()
            .and_then(|a| a.as_u64().ok());
        let head_as_bytes = version_or_digest.as_atom().ok().map(|atom_handle| {
            let bytes = atom_handle.as_ne_bytes();
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
            let version = version_or_digest
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
            decode_v1_page(height, block_id, raw_page_bytes, rest, txs_noun)
        } else {
            // v0 page - head is the digest
            decode_v0_page(
                height, block_id, raw_page_bytes, version_or_digest, rest, txs_noun,
            )
        }
    }
}

fn decode_v0_page(
    height: u64,
    block_id: Hash,
    raw_page_bytes: u64,
    digest_noun: NounHandle,
    rest: NounHandle,
    txs_noun: NounHandle,
) -> Result<FullPageDetails, NounDecodeError> {
    // v0 page: [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]
    let _digest = Hash::from_noun_handle(&digest_noun)
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
    let parent = Hash::from_noun_handle(&cell.head())
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
        raw_page_bytes,
        msg,
    })
}

fn decode_v1_page(
    height: u64,
    block_id: Hash,
    raw_page_bytes: u64,
    rest: NounHandle,
    txs_noun: NounHandle,
) -> Result<FullPageDetails, NounDecodeError> {
    let _space = rest.space();
    // v1 page (after version tag): [digest pow parent tx-ids coinbase timestamp epoch-counter target accumulated-work height msg]
    let cell = rest
        .as_cell()
        .map_err(|_| NounDecodeError::Custom("v1 page: rest should be cell".into()))?;
    let _digest = Hash::from_noun_handle(&cell.head())
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
    let parent = Hash::from_noun_handle(&cell.head())
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
        raw_page_bytes,
        msg,
    })
}

fn decode_pow(noun: &NounHandle) -> Result<(bool, Option<Vec<u8>>), NounDecodeError> {
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

fn decode_bignum(noun: &NounHandle) -> Result<BigNumValue, NounDecodeError> {
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

fn decode_coinbase(noun: &NounHandle) -> Result<CoinbaseSplitValue, NounDecodeError> {
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

fn decode_coinbase_v1_map(noun: &NounHandle) -> Result<Vec<(Hash, u64)>, NounDecodeError> {
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64()? == 0 {
            return Ok(Vec::new());
        }
    }

    let mut entries = Vec::new();
    for entry in HoonMapIter::new(noun) {
        if !entry.is_cell() {
            continue;
        }
        let [key, value] = entry.uncell().map_err(|_| NounDecodeError::ExpectedCell)?;
        let hash = Hash::from_noun_handle(&key)?;
        let coins = value.as_atom()?.as_u64()?;
        entries.push((hash, coins));
    }
    Ok(entries)
}

fn count_map_entries(noun: &NounHandle) -> usize {
    let mut count = 0;
    for entry in HoonMapIter::new(noun) {
        if entry.is_cell() {
            count += 1;
        }
    }
    count
}

fn decode_page_msg(noun: &NounHandle) -> Result<PageMsgValue, NounDecodeError> {
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

fn jammed_noun_len(noun: Noun, space: &NounSpace) -> usize {
    let mut slab: NounSlab = NounSlab::new();
    slab.copy_into(noun, space);
    slab.jam().len()
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
            raw_page_bytes: Some(self.raw_page_bytes),
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
pub(super) mod tests {
    use nockapp::driver::PokeResult;
    use nockapp::nockapp::error::NockAppError;
    use nockapp::wire::WireRepr;
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::{
        BigNum, BlockHeight, CoinbaseSplit, Hash, Nicks, Page, TimelockRangeAbsolute, Version,
    };
    use nockchain_types::tx_engine::v0::Inputs;
    use nockchain_types::tx_engine::v1::{RawTx as RawTxV1, Spends};
    use nockvm::noun::NounAllocator;
    use noun_serde::{NounDecode, NounEncode};
    use parking_lot::Mutex as ChainMutex;

    use super::*;
    use crate::pb::common::v1::Base58Hash;
    use crate::pb::public::v2::nockchain_block_service_server::NockchainBlockService;
    use crate::pb::public::v2::{
        get_transaction_block_response, get_transaction_details_response,
        GetTransactionBlockRequest, GetTransactionDetailsRequest,
    };
    use crate::public_nockchain::v2::server::NockchainBlockServer;

    #[derive(Clone)]
    enum RawPresence {
        Missing,
        Present(Version),
        BadPath,
        MalformedOuter,
        MalformedInner,
        NoPeek,
        FailedPeek,
    }

    struct RawPresenceHandle {
        response: RawPresence,
        pause: bool,
        entered: Semaphore,
        release: Semaphore,
    }

    impl RawPresenceHandle {
        fn new(response: RawPresence, pause: bool) -> Self {
            Self {
                response,
                pause,
                entered: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl BalanceHandle for RawPresenceHandle {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, NockAppError> {
            let tag = {
                let space = path.noun_space();
                let items = path_items(unsafe { *path.root() }, &space);
                String::from_noun(&items[0], &space).unwrap()
            };
            if tag == "block-transaction" {
                return Ok(Some(
                    ExactTxHandle {
                        block_id: test_hash(10),
                        tx_id: test_hash(900),
                        version: match self.response.clone() {
                            RawPresence::Present(version) => version,
                            _ => Version::V0,
                        },
                        total_size: 48,
                    }
                    .encode_block_transaction(),
                ));
            }
            assert_eq!(tag, "raw-transaction");
            if self.pause {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            let mut slab = NounSlab::new();
            // `%raw-transaction` returns (unit (unit raw-tx:t)):
            // absence is [~ ~], presence is [~ ~ raw], bad path is ~.
            let root = match self.response.clone() {
                RawPresence::Missing => nockvm::noun::T(&mut slab, &[SIG, SIG]),
                RawPresence::Present(version) => {
                    let raw = match version {
                        Version::V0 => RawTx {
                            id: test_hash(900),
                            inputs: Inputs(Vec::new()),
                            timelock_range: TimelockRangeAbsolute::new(None, None),
                            total_fees: Nicks(7),
                        }
                        .to_noun(&mut slab),
                        Version::V1 => RawTxV1 {
                            version,
                            id: test_hash(900),
                            spends: Spends(Vec::new()),
                        }
                        .to_noun(&mut slab),
                        Version::V2 => panic!("unsupported raw transaction version"),
                    };
                    nockvm::noun::T(&mut slab, &[SIG, SIG, raw])
                }
                RawPresence::BadPath => SIG,
                RawPresence::MalformedOuter => {
                    nockvm::noun::T(&mut slab, &[nockvm::noun::D(1), SIG])
                }
                RawPresence::MalformedInner => {
                    nockvm::noun::T(&mut slab, &[SIG, nockvm::noun::D(1)])
                }
                RawPresence::NoPeek => return Ok(None),
                RawPresence::FailedPeek => {
                    return Err(NockAppError::OtherError("raw peek failed".into()));
                }
            };
            slab.set_root(root);
            Ok(Some(slab))
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<nockapp::driver::NativePokeObserver>,
        ) -> std::result::Result<PokeResult, NockAppError> {
            Err(NockAppError::OtherError("poke not supported".into()))
        }
    }

    async fn raw_presence_endpoint(
        response: RawPresence,
        complete: bool,
        confirmed: bool,
        pause: bool,
    ) -> (
        Arc<BlockExplorerCache>,
        Arc<RawPresenceHandle>,
        Arc<NockchainBlockServer>,
    ) {
        let cache = new_cache();
        cache
            .insert_blocks(vec![(
                0,
                test_hash(10),
                test_hash(9),
                12345,
                if confirmed {
                    vec![test_hash(900)]
                } else {
                    Vec::new()
                },
            )])
            .await;
        cache.seed_ready.store(complete, Ordering::Release);
        let handle = Arc::new(RawPresenceHandle::new(response, pause));
        let server = Arc::new(NockchainBlockServer::new(
            handle.clone(),
            cache.clone(),
            crate::public_nockchain::v2::metrics::init_metrics(),
        ));
        (cache, handle, server)
    }

    async fn presence_block(
        server: &NockchainBlockServer,
    ) -> Result<crate::pb::public::v2::GetTransactionBlockResponse, tonic::Status> {
        server
            .get_transaction_block(tonic::Request::new(GetTransactionBlockRequest {
                tx_id: Some(Base58Hash {
                    hash: test_hash(900).to_base58(),
                }),
            }))
            .await
            .map(tonic::Response::into_inner)
    }

    async fn presence_details(
        server: &NockchainBlockServer,
    ) -> Result<crate::pb::public::v2::GetTransactionDetailsResponse, tonic::Status> {
        server
            .get_transaction_details(tonic::Request::new(GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: test_hash(900).to_base58(),
                }),
            }))
            .await
            .map(tonic::Response::into_inner)
    }

    #[tokio::test]
    async fn raw_presence_distinguishes_unknown_pending_and_confirmed_for_both_endpoints() {
        let (_, _, server) = raw_presence_endpoint(RawPresence::Missing, true, false, false).await;
        for error in [
            presence_block(&server).await.unwrap_err(),
            presence_details(&server).await.unwrap_err(),
        ] {
            assert_eq!(error.code(), tonic::Code::NotFound);
        }
        for version in [Version::V0, Version::V1] {
            let (_, _, server) =
                raw_presence_endpoint(RawPresence::Present(version.clone()), true, false, false)
                    .await;
            assert!(matches!(
                presence_block(&server).await.unwrap().result,
                Some(get_transaction_block_response::Result::Pending(_))
            ));
            assert!(matches!(
                presence_details(&server).await.unwrap().result,
                Some(get_transaction_details_response::Result::Pending(_))
            ));
            let (_, _, server) =
                raw_presence_endpoint(RawPresence::Present(version.clone()), true, true, false)
                    .await;
            assert!(matches!(
                presence_block(&server).await.unwrap().result,
                Some(get_transaction_block_response::Result::Block(block)) if block.height == 0
            ));
            let details = presence_details(&server).await.unwrap();
            let Some(get_transaction_details_response::Result::Details(details)) = details.result
            else {
                panic!("confirmed transaction must have details")
            };
            assert_eq!(details.height, 0);
            assert_eq!(details.version, u64::from(u32::from(version)));
            assert_eq!(details.tx_id, test_hash(900).to_base58());
            assert_eq!(details.size_bytes, 48);
        }
    }

    #[tokio::test]
    async fn raw_presence_scry_errors_are_not_pending_or_transaction_absence() {
        for (response, code) in [
            (RawPresence::BadPath, tonic::Code::NotFound),
            (RawPresence::MalformedOuter, tonic::Code::Internal),
            (RawPresence::MalformedInner, tonic::Code::Internal),
            (RawPresence::NoPeek, tonic::Code::NotFound),
            (RawPresence::FailedPeek, tonic::Code::Internal),
        ] {
            let (_, _, server) = raw_presence_endpoint(response, true, false, false).await;
            for error in [
                presence_block(&server).await.unwrap_err(),
                presence_details(&server).await.unwrap_err(),
            ] {
                assert_eq!(error.code(), code);
            }
        }
    }

    #[tokio::test]
    async fn raw_presence_incomplete_index_and_concurrent_reorg_remain_unavailable() {
        for response in [RawPresence::Missing, RawPresence::Present(Version::V1)] {
            let (_, _, server) = raw_presence_endpoint(response.clone(), false, false, false).await;
            assert_eq!(
                presence_block(&server).await.unwrap_err().code(),
                tonic::Code::Unavailable
            );
            assert_eq!(
                presence_details(&server).await.unwrap_err().code(),
                tonic::Code::Unavailable
            );

            let (cache, handle, server) = raw_presence_endpoint(response, true, false, true).await;
            let block_server = server.clone();
            let block = tokio::spawn(async move { presence_block(&block_server).await });
            let details = tokio::spawn(async move { presence_details(&server).await });
            handle.entered.acquire_many(2).await.unwrap().forget();
            cache.clear_cached_chain().await;
            handle.release.add_permits(2);
            assert_eq!(
                block.await.unwrap().unwrap_err().code(),
                tonic::Code::Unavailable
            );
            assert_eq!(
                details.await.unwrap().unwrap_err().code(),
                tonic::Code::Unavailable
            );
        }
    }

    #[test]
    fn test_decode_blockheight_as_atom() {
        let mut slab: NounSlab = NounSlab::new();

        // Test that BlockHeight encodes as a simple atom
        let height = BlockHeight(Belt(105));
        let height_noun = height.to_noun(&mut slab);
        let space = slab.noun_space();

        // Verify it's an atom
        assert!(height_noun.is_atom(), "BlockHeight should encode as atom");

        // Try to decode as u64 directly
        let result_u64 = u64::from_noun(&height_noun, &space);
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
        let result_height = BlockHeight::from_noun(&height_noun, &space);
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
    fn test_decode_block_range_entry_with_structured_pow() {
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
        let proof_node = nockvm::noun::T(&mut slab, &[nockvm::noun::D(1), nockvm::noun::D(2)]);
        let proof_list = nockvm::noun::T(&mut slab, &[proof_node, nockvm::noun::D(0)]);
        let pow = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), proof_list]);
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
        let space = slab.noun_space();
        let entry = BlockRangeEntry::from_noun_handle(&entry_noun.in_space(&space))
            .expect("decode block range entry");

        assert_eq!(entry.height, 42);
        assert_eq!(entry.block_id, block_id);
        assert_eq!(entry.parent_id, parent);
        assert_eq!(entry.timestamp, 1234567890);
        assert_eq!(entry.tx_ids.len(), 0);
    }

    #[test]
    fn test_full_page_details_reports_jammed_page_bytes() {
        let mut slab: NounSlab = NounSlab::new();

        let height = BlockHeight(Belt(42));
        let height_noun = height.to_noun(&mut slab);

        let block_id = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let block_id_noun = block_id.to_noun(&mut slab);

        let digest = Hash([Belt(10), Belt(11), Belt(12), Belt(13), Belt(14)]);
        let pow = nockvm::noun::D(0);
        let tx_ids_set = nockvm::noun::D(0);
        let coinbase = nockvm::noun::D(0);
        let timestamp = Belt(1234567890);
        let epoch_counter = Belt(0);
        let target = Belt(100);
        let accumulated_work = Belt(500);
        let page_height = Belt(42);
        let parent = Hash([Belt(20), Belt(21), Belt(22), Belt(23), Belt(24)]);
        let msg = nockvm::noun::D(0);

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
        let txs_map_noun = nockvm::noun::D(0);
        let page_txs_cell = nockvm::noun::T(&mut slab, &[page_noun, txs_map_noun]);
        let block_page_cell = nockvm::noun::T(&mut slab, &[block_id_noun, page_txs_cell]);
        let entry_noun = nockvm::noun::T(&mut slab, &[height_noun, block_page_cell]);

        let space = slab.noun_space();
        let expected_raw_page_bytes = jammed_noun_len(page_noun, &space) as u64;
        let details = FullPageDetails::from_noun_handle(&entry_noun.in_space(&space))
            .expect("convert raw entry");

        assert_eq!(details.raw_page_bytes, expected_raw_page_bytes);
        assert!(details.msg.raw.is_empty());
    }

    #[test]
    fn test_range_chunk_helpers_bound_kernel_slice() {
        assert_eq!(BlockExplorerCache::RANGE_CHUNK, 128);
        assert_eq!(BlockExplorerCache::range_chunk_start(255), 128);
        assert_eq!(BlockExplorerCache::range_chunk_end(1, 255), 128);
        assert_eq!(BlockExplorerCache::range_chunk_end(129, 255), 255);
    }

    #[derive(Default)]
    struct NoDataHandle {
        peek_calls: AtomicU64,
    }

    #[async_trait::async_trait]
    impl BalanceHandle for NoDataHandle {
        async fn peek(
            &self,
            _path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, NockAppError> {
            self.peek_calls.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<nockapp::driver::NativePokeObserver>,
        ) -> std::result::Result<PokeResult, NockAppError> {
            Err(NockAppError::OtherError(
                "poke not supported in test handle".into(),
            ))
        }
    }

    struct ExactTxHandle {
        block_id: Hash,
        tx_id: Hash,
        version: Version,
        total_size: u64,
    }

    impl ExactTxHandle {
        /// Encode the `%block-transaction` scry result the way
        /// `hoon/apps/dumbnet/inner.hoon` declares it:
        ///
        /// ```hoon
        /// [%block-transaction bid=@ tid=@ ~]
        /// ^-  (unit (unit tx:t))
        /// ```
        ///
        /// where `tx:t` (`hoon/common/tx-engine.hoon`, `++  tx`) is
        ///
        /// ```hoon
        /// $%  [%0 =raw-tx:v0 total-size=size =outputs:v0]
        ///     [%1 =raw-tx:v1 total-size=size =outputs:v1]   :: form:tx:v1
        /// ==
        /// ```
        ///
        /// The `raw-tx` halves come from the real `nockchain-types` encoders
        /// (`v0::RawTx`, `v1::RawTx`), so a drift in the raw transaction
        /// layout fails this test. The validated wrapper (`%0`/`%1` tag,
        /// `total-size`, `outputs`) has no Rust type and is laid out by hand
        /// from the Hoon above; `outputs` is an empty `z-map`/`z-set`, which
        /// encodes as atom `0`. The `unit`s nest as `[~ ~ tx]`.
        ///
        /// A live-node check still has to confirm that the kernel's
        /// `txs.c.k` map really holds `tx:t` values for both v0 and v1 blocks
        /// (not `raw-tx`), that `total-size` matches what the explorer reports
        /// as `size_bytes`, and that non-empty `outputs` decode through
        /// `decode_outputs_v0`/`decode_outputs_v1`. Mock data cannot establish
        /// any of that.
        fn encode_block_transaction(&self) -> NounSlab {
            let mut slab = NounSlab::new();
            let (tag, raw) = match self.version {
                Version::V0 => {
                    let raw = RawTx {
                        id: self.tx_id.clone(),
                        inputs: Inputs(Vec::new()),
                        timelock_range: TimelockRangeAbsolute::new(None, None),
                        total_fees: Nicks(7),
                    };
                    (nockvm::noun::D(0), raw.to_noun(&mut slab))
                }
                Version::V1 => {
                    let raw = RawTxV1 {
                        version: Version::V1,
                        id: self.tx_id.clone(),
                        spends: Spends(Vec::new()),
                    };
                    (nockvm::noun::D(1), raw.to_noun(&mut slab))
                }
                Version::V2 => panic!("no validated tx shape for v2"),
            };
            let total_size = nockvm::noun::D(self.total_size);
            let outputs = nockvm::noun::D(0);
            let validated = nockvm::noun::T(&mut slab, &[tag, raw, total_size, outputs]);
            let response = nockvm::noun::T(
                &mut slab,
                &[nockvm::noun::D(0), nockvm::noun::D(0), validated],
            );
            slab.set_root(response);
            slab
        }
    }

    #[async_trait::async_trait]
    impl BalanceHandle for ExactTxHandle {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, NockAppError> {
            let space = path.noun_space();
            let items = path_items(unsafe { *path.root() }, &space);
            assert_eq!(items.len(), 3);
            assert_eq!(
                String::from_noun(&items[0], &space).unwrap(),
                "block-transaction"
            );
            assert_eq!(
                String::from_noun(&items[1], &space).unwrap(),
                self.block_id.to_base58()
            );
            assert_eq!(
                String::from_noun(&items[2], &space).unwrap(),
                self.tx_id.to_base58()
            );

            Ok(Some(self.encode_block_transaction()))
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<nockapp::driver::NativePokeObserver>,
        ) -> std::result::Result<PokeResult, NockAppError> {
            Err(NockAppError::OtherError("poke not supported".into()))
        }
    }

    async fn transaction_details_for_version(version: Version, total_size: u64) {
        let block_id = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let tx_id = Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]);
        let meta = BlockMetadata {
            height: 42,
            block_id: block_id.clone(),
            parent_id: Hash([Belt(11), Belt(12), Belt(13), Belt(14), Belt(15)]),
            timestamp: 12345,
            tx_ids: vec![tx_id.clone()],
        };
        let expected_version = u64::from(u32::from(version.clone()));
        let handle: Arc<dyn BalanceHandle> = Arc::new(ExactTxHandle {
            block_id,
            tx_id: tx_id.clone(),
            version,
            total_size,
        });
        let cache = BlockExplorerCache::new(crate::public_nockchain::v2::metrics::init_metrics());
        let details = cache
            .fetch_transaction_from_block(&handle, &meta, &tx_id)
            .await
            .unwrap();
        assert_eq!(details.tx_id, tx_id.to_base58());
        assert_eq!(details.version, expected_version);
        assert_eq!(details.height, 42);
        assert_eq!(details.size_bytes, total_size);
    }

    // The explorer must read the kernel's validated `tx:t` by exact
    // block/tx id. The mock encodes the `(unit (unit tx:t))` result with the
    // real `raw-tx` encoders; see `ExactTxHandle::encode_block_transaction`
    // for the shape derivation and the live-node checks that remain.
    #[tokio::test]
    async fn transaction_details_uses_validated_block_transaction_scry() {
        transaction_details_for_version(Version::V1, 64).await;
    }

    #[tokio::test]
    async fn transaction_details_decodes_validated_v0_block_transaction() {
        transaction_details_for_version(Version::V0, 48).await;
    }

    #[tokio::test]
    async fn load_full_page_by_height_rejects_non_direct_heights() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = BlockExplorerCache::new(metrics);
        let handle = Arc::new(NoDataHandle::default());
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();

        for height in [0, nockvm::noun::DIRECT_MAX] {
            assert!(matches!(
                cache
                    .load_full_page_by_height(&balance_handle, height)
                    .await,
                Err(NockAppGrpcError::PeekFailed)
            ));
        }

        for height in [nockvm::noun::DIRECT_MAX + 1, u64::MAX] {
            assert!(
                matches!(
                    cache
                        .load_full_page_by_height(&balance_handle, height)
                        .await,
                    Err(NockAppGrpcError::InvalidRequest(_))
                ),
                "height {height} should be rejected"
            );
        }

        assert_eq!(handle.peek_calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn explorer_rejects_non_direct_heights_on_all_paths() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = BlockExplorerCache::new(metrics);
        let handle = Arc::new(NoDataHandle::default());
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();

        assert!(
            matches!(
                cache.peek_full_page(&balance_handle, u64::MAX).await,
                Err(NockAppGrpcError::InvalidRequest(_))
            ),
            "peek_full_page should reject u64::MAX"
        );
        assert!(
            matches!(
                cache
                    .peek_blocks_range(&balance_handle, nockvm::noun::DIRECT_MAX + 1, 0)
                    .await,
                Err(NockAppGrpcError::InvalidRequest(_))
            ),
            "peek_blocks_range should reject a non-direct start"
        );
        assert!(
            matches!(
                cache.peek_blocks_range(&balance_handle, 0, u64::MAX).await,
                Err(NockAppGrpcError::InvalidRequest(_))
            ),
            "peek_blocks_range should reject a non-direct end"
        );

        assert_eq!(
            handle.peek_calls.load(Ordering::Relaxed),
            0,
            "non-direct heights must be rejected before any kernel peek"
        );
    }

    struct RecordingRangeHandle {
        heaviest_height: u64,
        requested_ranges: Mutex<Vec<(u64, u64)>>,
    }

    impl RecordingRangeHandle {
        fn new(heaviest_height: u64) -> Self {
            Self {
                heaviest_height,
                requested_ranges: Mutex::new(Vec::new()),
            }
        }

        fn requested_ranges(&self) -> Vec<(u64, u64)> {
            self.requested_ranges
                .lock()
                .expect("requested ranges lock poisoned")
                .clone()
        }

        fn encode_heaviest_chain(&self) -> NounSlab {
            let mut slab = NounSlab::new();
            let noun = Some(Some((
                BlockHeight(Belt(self.heaviest_height)),
                test_hash(self.heaviest_height + 10),
            )))
            .to_noun(&mut slab);
            slab.set_root(noun);
            slab
        }

        fn encode_block_range(&self, start: u64, end: u64) -> NounSlab {
            let mut slab = NounSlab::new();
            let entries: Vec<_> = (start..=end).map(test_block_range_entry).collect();
            let noun = Some(Some(entries)).to_noun(&mut slab);
            slab.set_root(noun);
            slab
        }
    }

    #[async_trait::async_trait]
    impl BalanceHandle for RecordingRangeHandle {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, NockAppError> {
            let space = path.noun_space();
            let root = unsafe { *path.root() };
            let items = path_items(root, &space);
            let tag = String::from_noun(&items[0], &space).expect("path tag should decode");

            match tag.as_str() {
                "heaviest-chain" => Ok(Some(self.encode_heaviest_chain())),
                "heaviest-chain-blocks-range" => {
                    assert_eq!(items.len(), 3, "range path should include start and end");
                    let start = atom_u64(items[1], &space);
                    let end = atom_u64(items[2], &space);
                    self.requested_ranges
                        .lock()
                        .expect("requested ranges lock poisoned")
                        .push((start, end));
                    Ok(Some(self.encode_block_range(start, end)))
                }
                other => panic!("unexpected peek path tag: {other}"),
            }
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<nockapp::driver::NativePokeObserver>,
        ) -> std::result::Result<PokeResult, NockAppError> {
            Err(NockAppError::OtherError(
                "poke not supported in test handle".into(),
            ))
        }
    }

    #[tokio::test]
    async fn test_initialize_inner_requests_latest_seed_window_and_queues_backfill() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics));
        let handle = Arc::new(RecordingRangeHandle::new(255));
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();

        let initialized = cache
            .clone()
            .initialize_inner(balance_handle)
            .await
            .expect("cache initialization should succeed");

        assert!(initialized, "cache should report a successful seed");
        assert_eq!(handle.requested_ranges(), vec![(128, 255)]);
        assert_eq!(cache.get_max_height(), 255);
        assert_eq!(cache.pending_backfill().await, Some((1, 127)));
    }

    #[tokio::test]
    async fn shorter_heavier_chain_removes_orphaned_transaction_confirmation() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics));
        let orphan_tx = test_hash(700);
        cache
            .insert_blocks(vec![(
                200,
                test_hash(210),
                test_hash(209),
                2_000,
                vec![orphan_tx.clone()],
            )])
            .await;
        cache.heaviest_height.store(200, Ordering::Release);
        cache.seed_ready.store(true, Ordering::Release);
        assert!(cache.get_block_for_tx(&orphan_tx).await.is_some());

        let handle: Arc<dyn BalanceHandle> = Arc::new(RecordingRangeHandle::new(150));
        cache.refresh(&handle).await.expect("reorg refresh");

        assert_eq!(cache.get_max_height(), 150);
        assert!(cache.get_block_for_tx(&orphan_tx).await.is_none());
    }

    #[tokio::test]
    async fn forward_progress_after_reorg_does_not_resurrect_orphaned_confirmation() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics));
        let orphan_tx = test_hash(701);
        cache
            .insert_blocks(vec![(
                200,
                test_hash(900),
                test_hash(899),
                2_000,
                vec![orphan_tx.clone()],
            )])
            .await;
        cache.heaviest_height.store(200, Ordering::Release);
        cache.seed_ready.store(true, Ordering::Release);

        let handle = Arc::new(RecordingRangeHandle::new(202));
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        cache.refresh(&balance_handle).await.expect("reorg refresh");

        assert_eq!(cache.get_max_height(), 202);
        assert!(cache.get_block_for_tx(&orphan_tx).await.is_none());
        assert_eq!(handle.requested_ranges(), vec![(200, 200), (75, 202)]);
    }

    #[tokio::test]
    async fn cached_blocks_above_heaviest_height_are_not_served_by_height_or_id() {
        // Between refreshes a reorg can leave cached entries above the
        // canonical tip. Neither the paginated height listing nor the by-id
        // lookup may serve them, and the by-id path must not even ask the
        // kernel for the page.
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics));
        let canonical_id = test_hash(160);
        let orphan_id = test_hash(210);
        cache
            .insert_blocks(vec![
                (150, canonical_id.clone(), test_hash(170), 1_500, Vec::new()),
                (200, orphan_id.clone(), test_hash(209), 2_000, Vec::new()),
            ])
            .await;
        cache.heaviest_height.store(150, Ordering::Release);
        cache.seed_ready.store(true, Ordering::Release);

        let (page, next, current_height) = cache.get_blocks_page(None, 10).await.unwrap();
        assert_eq!(current_height, 150);
        assert_eq!(
            page.iter().map(|block| block.height).collect::<Vec<_>>(),
            vec![150]
        );
        assert_eq!(next, Some(149));
        let (page, _, current_height) = cache.get_blocks_page(Some(200), 10).await.unwrap();
        assert_eq!(current_height, 150);
        assert_eq!(
            page.iter().map(|block| block.height).collect::<Vec<_>>(),
            vec![150]
        );

        let no_data = Arc::new(NoDataHandle::default());
        let no_data_handle: Arc<dyn BalanceHandle> = no_data.clone();
        assert!(matches!(
            cache
                .load_full_page_by_id(&no_data_handle, &orphan_id.to_base58())
                .await,
            Err(NockAppGrpcError::NotFound)
        ));
        assert_eq!(
            no_data.peek_calls.load(Ordering::Relaxed),
            0,
            "orphaned id above the tip must be rejected before any kernel peek"
        );

        // The canonical block at or below the tip is still served by id; the
        // range mock answers height 150 with block id test_hash(160).
        let range_handle: Arc<dyn BalanceHandle> = Arc::new(RecordingRangeHandle::new(150));
        let page = cache
            .load_full_page_by_id(&range_handle, &canonical_id.to_base58())
            .await
            .expect("canonical block is served by id");
        assert_eq!(page.height, 150);
        assert_eq!(page.block_id, canonical_id);
    }

    #[tokio::test]
    async fn orphaned_block_id_at_canonical_height_is_not_served() {
        // A cached id at a height the canonical chain still has, but
        // whose block was replaced by a reorg, must not be answered with the
        // canonical block that now occupies that height.
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics));
        let orphan_id = test_hash(999);
        cache
            .insert_blocks(vec![(
                150,
                orphan_id.clone(),
                test_hash(170),
                1_500,
                Vec::new(),
            )])
            .await;
        cache.heaviest_height.store(200, Ordering::Release);
        cache.seed_ready.store(true, Ordering::Release);

        // The kernel's canonical page at height 150 carries test_hash(160).
        let range_handle: Arc<dyn BalanceHandle> = Arc::new(RecordingRangeHandle::new(200));
        assert!(matches!(
            cache
                .load_full_page_by_id(&range_handle, &orphan_id.to_base58())
                .await,
            Err(NockAppGrpcError::NotFound)
        ));
    }

    #[tokio::test]
    async fn old_backfill_cannot_insert_after_reorg_clear() {
        let metrics = crate::public_nockchain::v2::metrics::init_metrics();
        let cache = BlockExplorerCache::new(metrics);
        let old_generation = cache.chain_generation.load(Ordering::Acquire);
        cache.clear_cached_chain().await;
        let orphan_tx = test_hash(702);

        assert_eq!(
            cache
                .insert_blocks_for_generation(
                    vec![(
                        200,
                        test_hash(210),
                        test_hash(209),
                        2_000,
                        vec![orphan_tx.clone()],
                    )],
                    old_generation,
                    200,
                    200,
                    None,
                )
                .await,
            RangeInsert::Stale
        );
        assert_eq!(cache.get_max_height(), 0);
        assert!(cache.get_block_for_tx(&orphan_tx).await.is_none());
    }

    struct ChainState {
        height: u64,
        branch: u64,
        fork_height: u64,
        retain_raw: bool,
        ranges: Vec<(u64, u64)>,
        switch_before_range: Option<(usize, u64)>,
        pause_range: Option<(u64, u64)>,
        fail_range: Option<(u64, u64)>,
        pause_block_transaction: bool,
        fail_block_transaction: bool,
    }

    pub(crate) struct ReorgHandle {
        state: ChainMutex<ChainState>,
        range_entered: Semaphore,
        range_release: Semaphore,
        block_transaction_entered: Semaphore,
        block_transaction_release: Semaphore,
    }

    impl ReorgHandle {
        pub(crate) fn new(height: u64) -> Self {
            Self {
                state: ChainMutex::new(ChainState {
                    height,
                    branch: 0,
                    fork_height: 0,
                    retain_raw: false,
                    ranges: Vec::new(),
                    switch_before_range: None,
                    pause_range: None,
                    fail_range: None,
                    pause_block_transaction: false,
                    fail_block_transaction: false,
                }),
                range_entered: Semaphore::new(0),
                range_release: Semaphore::new(0),
                block_transaction_entered: Semaphore::new(0),
                block_transaction_release: Semaphore::new(0),
            }
        }

        pub(crate) fn set_tip(&self, height: u64, branch: u64) {
            let mut state = self.state.lock();
            state.height = height;
            state.branch = branch;
        }

        pub(crate) fn fork_at(&self, height: u64) {
            let mut state = self.state.lock();
            state.fork_height = height;
            state.branch = 1;
        }

        pub(crate) fn retain_raw_transactions(&self) {
            self.state.lock().retain_raw = true;
        }

        pub(crate) fn switch_after_ranges(&self, successful_ranges: usize, branch: u64) {
            let mut state = self.state.lock();
            state.switch_before_range = Some((state.ranges.len() + successful_ranges + 1, branch));
        }

        pub(crate) fn ranges(&self) -> Vec<(u64, u64)> {
            self.state.lock().ranges.clone()
        }

        pub(crate) fn pause_range(&self, start: u64, end: u64) {
            self.state.lock().pause_range = Some((start, end));
        }

        pub(crate) fn fail_range_once(&self, start: u64, end: u64) {
            self.state.lock().fail_range = Some((start, end));
        }

        pub(crate) async fn wait_for_range(&self) {
            self.range_entered.acquire().await.unwrap().forget();
        }

        pub(crate) fn release_range(&self) {
            self.range_release.add_permits(1);
        }

        pub(crate) fn pause_block_transaction(&self) {
            self.state.lock().pause_block_transaction = true;
        }

        pub(crate) fn fail_block_transaction_once(&self) {
            self.state.lock().fail_block_transaction = true;
        }

        pub(crate) async fn wait_for_block_transaction(&self) {
            self.block_transaction_entered
                .acquire()
                .await
                .unwrap()
                .forget();
        }

        pub(crate) fn release_block_transaction(&self) {
            self.block_transaction_release.add_permits(1);
        }

        pub(crate) fn block_id(branch: u64, height: u64) -> Hash {
            test_hash(branch * 10_000 + height + 10)
        }

        pub(crate) fn orphan_tx() -> Hash {
            test_hash(90_000)
        }

        pub(crate) fn suffix_tx() -> Hash {
            test_hash(90_001)
        }

        pub(crate) fn pending_tx() -> Hash {
            test_hash(90_002)
        }

        fn encode_range(start: u64, end: u64, branch: u64, fork_height: u64) -> NounSlab {
            let mut slab = NounSlab::new();
            let mut list = SIG;
            for height in (start..=end).rev() {
                let block_branch = if height < fork_height { 0 } else { branch };
                let parent_branch = if height.saturating_sub(1) < fork_height {
                    0
                } else {
                    branch
                };
                let mut entry = test_block_range_entry(height);
                entry.tail.tail.page.digest = Self::block_id(block_branch, height);
                entry.tail.tail.page.parent =
                    Self::block_id(parent_branch, height.saturating_sub(1));
                if block_branch == 0 && (height == 100 || height == 250) {
                    entry.tail.tail.page.tx_ids = vec![if height == 100 {
                        Self::orphan_tx()
                    } else {
                        Self::suffix_tx()
                    }];
                }
                let page = entry.tail.tail.page.to_noun(&mut slab);
                let block_id = Self::block_id(block_branch, height).to_noun(&mut slab);
                // The kernel returns a z-map, not a Rust list of raw transactions.
                let txs = if let Some(id) = entry.tail.tail.page.tx_ids.first() {
                    let id = id.clone();
                    let raw = RawTx {
                        id: id.clone(),
                        inputs: Inputs(Vec::new()),
                        timelock_range: TimelockRangeAbsolute::new(None, None),
                        total_fees: Nicks(7),
                    };
                    let tx_id = id.to_noun(&mut slab);
                    let raw = raw.to_noun(&mut slab);
                    let pair = nockvm::noun::T(&mut slab, &[tx_id, raw]);
                    nockvm::noun::T(&mut slab, &[pair, SIG, SIG])
                } else {
                    SIG
                };
                let item =
                    nockvm::noun::T(&mut slab, &[nockvm::noun::D(height), block_id, page, txs]);
                list = nockvm::noun::T(&mut slab, &[item, list]);
            }
            let root = nockvm::noun::T(&mut slab, &[SIG, SIG, list]);
            slab.set_root(root);
            slab
        }
    }

    #[async_trait::async_trait]
    impl BalanceHandle for ReorgHandle {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, NockAppError> {
            let (tag, range, ids) = {
                let space = path.noun_space();
                let items = path_items(unsafe { *path.root() }, &space);
                let tag = String::from_noun(&items[0], &space).unwrap();
                let range = (tag == "heaviest-chain-blocks-range")
                    .then(|| (atom_u64(items[1], &space), atom_u64(items[2], &space)));
                let ids = if tag == "raw-transaction" || tag == "block-transaction" {
                    items[1..]
                        .iter()
                        .map(|item| String::from_noun(item, &space).unwrap())
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                (tag, range, ids)
            };
            if tag == "raw-transaction" {
                if !self.state.lock().retain_raw {
                    let mut slab = NounSlab::new();
                    let root = nockvm::noun::T(&mut slab, &[SIG, SIG]);
                    slab.set_root(root);
                    return Ok(Some(slab));
                }
                let id = Hash::from_base58(&ids[0]).unwrap();
                let mut slab = NounSlab::new();
                let raw = RawTx {
                    id,
                    inputs: Inputs(Vec::new()),
                    timelock_range: TimelockRangeAbsolute::new(None, None),
                    total_fees: Nicks(7),
                }
                .to_noun(&mut slab);
                let root = nockvm::noun::T(&mut slab, &[SIG, SIG, raw]);
                slab.set_root(root);
                return Ok(Some(slab));
            }
            if tag == "block-transaction" {
                let response = ExactTxHandle {
                    block_id: Hash::from_base58(&ids[0]).unwrap(),
                    tx_id: Hash::from_base58(&ids[1]).unwrap(),
                    version: Version::V0,
                    total_size: 48,
                }
                .encode_block_transaction();
                let (pause, fail) = {
                    let mut state = self.state.lock();
                    (
                        std::mem::take(&mut state.pause_block_transaction),
                        std::mem::take(&mut state.fail_block_transaction),
                    )
                };
                if pause {
                    self.block_transaction_entered.add_permits(1);
                    self.block_transaction_release
                        .acquire()
                        .await
                        .unwrap()
                        .forget();
                }
                if fail {
                    return Err(NockAppError::OtherError(
                        "injected one-shot block-transaction failure".into(),
                    ));
                }
                return Ok(Some(response));
            }
            let (response, pause, fail) = {
                let mut state = self.state.lock();
                if let Some((start, end)) = range {
                    let call = state.ranges.len() + 1;
                    if let Some((at, branch)) = state.switch_before_range {
                        if call == at {
                            state.branch = branch;
                            state.switch_before_range = None;
                        }
                    }
                    state.ranges.push((start, end));
                    let fail = state.fail_range == Some((start, end));
                    if fail {
                        state.fail_range = None;
                    }
                    let pause = state.pause_range == Some((start, end));
                    if pause {
                        state.pause_range = None;
                    }
                    (
                        Self::encode_range(start, end, state.branch, state.fork_height),
                        pause,
                        fail,
                    )
                } else {
                    assert_eq!(tag, "heaviest-chain");
                    let mut slab = NounSlab::new();
                    let tip = Some(Some((
                        BlockHeight(Belt(state.height)),
                        Self::block_id(
                            if state.height < state.fork_height {
                                0
                            } else {
                                state.branch
                            },
                            state.height,
                        ),
                    )))
                    .to_noun(&mut slab);
                    slab.set_root(tip);
                    (slab, false, false)
                }
            };
            if pause {
                self.range_entered.add_permits(1);
                self.range_release.acquire().await.unwrap().forget();
            }
            if fail {
                return Err(NockAppError::OtherError(
                    "injected one-shot range failure".into(),
                ));
            }
            Ok(Some(response))
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<nockapp::driver::NativePokeObserver>,
        ) -> std::result::Result<PokeResult, NockAppError> {
            Err(NockAppError::OtherError("poke not supported".into()))
        }
    }

    fn new_cache() -> Arc<BlockExplorerCache> {
        Arc::new(BlockExplorerCache::new(
            crate::public_nockchain::v2::metrics::init_metrics(),
        ))
    }

    async fn assert_connected(cache: &BlockExplorerCache, branch: u64) {
        let blocks = cache.blocks_by_height.read().await;
        assert!(!blocks.is_empty());
        let mut previous: Option<&BlockMetadata> = None;
        for block in blocks.values() {
            assert_eq!(block.block_id, ReorgHandle::block_id(branch, block.height));
            if let Some(parent) = previous {
                assert_eq!(block.height, parent.height + 1);
                assert_eq!(block.parent_id, parent.block_id);
            }
            previous = Some(block);
        }
    }

    type RetainedBlock = (u64, Hash, Hash, u64, Vec<Hash>);

    #[derive(Debug, PartialEq, Eq)]
    struct RetainedExplorerState {
        by_height: BTreeMap<u64, RetainedBlock>,
        by_id: HashMap<Hash, RetainedBlock>,
        tx: HashMap<Hash, (Hash, u64)>,
        prefixes: BTreeMap<String, Hash>,
        max_height: u64,
        cursor: Option<u64>,
    }

    fn retained_block(block: &BlockMetadata) -> RetainedBlock {
        (
            block.height,
            block.block_id.clone(),
            block.parent_id.clone(),
            block.timestamp,
            block.tx_ids.clone(),
        )
    }

    async fn retained_explorer_state(cache: &BlockExplorerCache) -> RetainedExplorerState {
        let by_height = cache.blocks_by_height.read().await;
        let by_id = cache.blocks_by_id.read().await;
        let tx = cache.tx_to_block.read().await;
        let prefixes = cache.tx_b58_index.read().await;
        RetainedExplorerState {
            by_height: by_height
                .iter()
                .map(|(height, block)| (*height, retained_block(block)))
                .collect(),
            by_id: by_id
                .iter()
                .map(|(id, block)| (id.clone(), retained_block(block)))
                .collect(),
            tx: tx.clone(),
            prefixes: prefixes.clone(),
            max_height: cache.get_max_height(),
            cursor: *cache.backfill_resume.read().await,
        }
    }

    fn assert_canonical_unavailable<T>(result: GrpcResult<T>) {
        match result {
            Err(NockAppGrpcError::Status(status)) => {
                assert_eq!(status.code(), tonic::Code::Unavailable);
            }
            Err(err) => panic!("expected canonical Unavailable, got {err}"),
            Ok(_) => panic!("retained nonauthoritative storage was served"),
        }
    }

    async fn assert_retained_reads_quarantined(
        cache: &BlockExplorerCache,
        handle: &Arc<dyn BalanceHandle>,
    ) {
        let calls = cache.chain_generation.load(Ordering::Acquire);
        assert!(!cache.seed_ready.load(Ordering::Acquire));
        assert!(cache.pending_backfill().await.is_none());
        assert!(cache
            .get_block_for_tx(&ReorgHandle::suffix_tx())
            .await
            .is_none());
        for cursor in [None, Some(250), Some(255), Some(0)] {
            assert_canonical_unavailable(cache.get_blocks_page(cursor, 10).await);
        }
        for tx in [ReorgHandle::suffix_tx(), ReorgHandle::orphan_tx(), ReorgHandle::pending_tx()] {
            assert_canonical_unavailable(cache.lookup_transaction_block(handle, &tx).await);
            assert_canonical_unavailable(cache.load_transaction_details(handle, &tx).await);
        }
        for id in [
            ReorgHandle::block_id(0, 250),
            ReorgHandle::block_id(0, 255),
            ReorgHandle::block_id(1, 250),
            test_hash(999_000),
        ] {
            assert_canonical_unavailable(cache.load_full_page_by_id(handle, &id.to_base58()).await);
        }
        for height in [0, 100, 250, 255] {
            assert_canonical_unavailable(cache.load_full_page_by_height(handle, height).await);
        }
        let suffix = ReorgHandle::suffix_tx().to_base58();
        for prefix in [&suffix[..8], "11111111"] {
            assert_canonical_unavailable(cache.lookup_tx_by_prefixes(prefix).await);
        }
        // This noncanonical full-hash spelling actually exercises prefix fallback.
        assert_canonical_unavailable(cache.resolve_tx_id("11111111").await);
        // Pure exact-hash parsing remains independent of canonical authority.
        assert_eq!(
            cache.resolve_tx_id(&suffix).await.unwrap(),
            ReorgHandle::suffix_tx()
        );
        assert_eq!(cache.chain_generation.load(Ordering::Acquire), calls);
    }
    #[tokio::test]
    async fn transient_reorg_range_failure_retains_seed_and_resumes_exact_history() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        assert!(cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let (old_generation, upper) = cache.pending_backfill().await.unwrap();
        assert_eq!(upper, 127);
        assert_eq!(
            cache
                .lookup_transaction_block(&balance, &ReorgHandle::suffix_tx())
                .await
                .unwrap()
                .height,
            250,
        );
        let before = retained_explorer_state(&cache).await;
        assert_eq!(before.by_height.first_key_value().unwrap().0, &128);
        assert_eq!(before.max_height, 255);
        assert_eq!(before.cursor, Some(127));
        let coverage = cache.metrics_snapshot().await;

        handle.fork_at(240);
        handle.fail_range_once(128, 255);
        assert!(cache.refresh(&balance).await.is_err());
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_ne!(
            cache.chain_generation.load(Ordering::Acquire),
            old_generation
        );
        let stale = cache.metrics_snapshot().await;
        assert!(!stale.seed_ready);
        assert_eq!(stale.cache_height, coverage.cache_height);
        assert_eq!(stale.cache_lowest_height, coverage.cache_lowest_height);
        assert_eq!(stale.cache_span, coverage.cache_span);
        assert_eq!(stale.backfill_resume_height, Some(127));
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 0);
        let requests = handle.ranges().len();
        assert_retained_reads_quarantined(&cache, &balance).await;
        assert_eq!(
            handle.ranges().len(),
            requests,
            "stale APIs must not issue range peeks"
        );

        cache.refresh(&balance).await.unwrap();
        assert!(cache.seed_ready.load(Ordering::Acquire));
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        assert_ne!(generation, old_generation);
        assert_eq!(upper, 127);
        {
            let blocks = cache.blocks_by_height.read().await;
            assert_eq!(blocks.first_key_value().unwrap().0, &128);
            assert_eq!(blocks[&239].block_id, ReorgHandle::block_id(0, 239));
            assert_eq!(blocks[&240].block_id, ReorgHandle::block_id(1, 240));
            assert_eq!(blocks[&255].block_id, ReorgHandle::block_id(1, 255));
            let mut previous: Option<&BlockMetadata> = None;
            for block in blocks.values() {
                if let Some(parent) = previous {
                    assert_eq!(block.height, parent.height + 1);
                    assert_eq!(block.parent_id, parent.block_id);
                }
                previous = Some(block);
            }
        }
        assert!(!cache
            .blocks_by_id
            .read()
            .await
            .contains_key(&ReorgHandle::block_id(0, 250)));
        assert!(!cache
            .tx_to_block
            .read()
            .await
            .contains_key(&ReorgHandle::suffix_tx()));
        assert!(!cache
            .tx_b58_index
            .read()
            .await
            .contains_key(&ReorgHandle::suffix_tx().to_base58()));
        let (page, _, current_height) = cache.get_blocks_page(None, 10).await.unwrap();
        assert_eq!(current_height, 255);
        assert_eq!(
            page.first().unwrap().block_id,
            ReorgHandle::block_id(1, 255)
        );

        let requests = handle.ranges().len();
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(&handle.ranges()[requests..], &[(0, 127)]);
        assert_eq!(*cache.backfill_resume.read().await, None);
        assert!(cache.pending_backfill().await.is_none());
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache
                .lookup_transaction_block(&balance, &ReorgHandle::orphan_tx())
                .await
                .unwrap()
                .height,
            100,
        );
        assert!(matches!(
            cache
                .lookup_transaction_block(&balance, &ReorgHandle::suffix_tx())
                .await,
            Err(NockAppGrpcError::NotFound),
        ));
    }

    #[tokio::test]
    async fn transient_shorter_fork_retains_high_water_then_repairs_cursor_from_coverage() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        assert!(cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let before = retained_explorer_state(&cache).await;
        handle.fork_at(240);
        handle.set_tip(254, 1);
        handle.fail_range_once(127, 254);
        assert!(cache.refresh(&balance).await.is_err());
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_eq!(cache.get_max_height(), 255);
        assert_eq!(cache.heaviest_height.load(Ordering::Acquire), 254);
        assert_eq!(*cache.backfill_resume.read().await, Some(127));
        assert_retained_reads_quarantined(&cache, &balance).await;

        cache.refresh(&balance).await.unwrap();
        assert_eq!(cache.get_max_height(), 254);
        let (_, cursor) = cache.pending_backfill().await.unwrap();
        assert_eq!(cursor, 126);
        let blocks = cache.blocks_by_height.read().await;
        assert_eq!(blocks.first_key_value().unwrap().0, &127);
        assert_eq!(blocks.last_key_value().unwrap().0, &254);
        assert_eq!(blocks[&239].block_id, ReorgHandle::block_id(0, 239));
        assert_eq!(blocks[&250].block_id, ReorgHandle::block_id(1, 250));
        assert!(!blocks.contains_key(&255));
        drop(blocks);
        assert!(!cache
            .blocks_by_id
            .read()
            .await
            .contains_key(&ReorgHandle::block_id(0, 255)));
        assert!(!cache
            .blocks_by_id
            .read()
            .await
            .contains_key(&ReorgHandle::block_id(0, 250)));
        let (page, _, current_height) = cache.get_blocks_page(Some(255), 10).await.unwrap();
        assert_eq!(current_height, 254);
        assert_eq!(page.first().unwrap().height, 254);
    }

    #[tokio::test]
    async fn repeated_transient_reorg_errors_and_tip_reversion_still_require_reconciliation() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        assert!(cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let before = retained_explorer_state(&cache).await;
        let old_generation = cache.chain_generation.load(Ordering::Acquire);
        handle.fork_at(240);
        handle.fail_range_once(128, 255);
        assert!(cache.refresh(&balance).await.is_err());
        let stale_generation = cache.chain_generation.load(Ordering::Acquire);
        assert_eq!(stale_generation, old_generation.wrapping_add(1));
        for _ in 0..2 {
            handle.fail_range_once(128, 255);
            assert!(cache.refresh(&balance).await.is_err());
            assert_eq!(
                cache.chain_generation.load(Ordering::Acquire),
                stale_generation
            );
            assert_eq!(retained_explorer_state(&cache).await, before);
            assert!(!cache.seed_ready.load(Ordering::Acquire));
            assert!(cache.pending_backfill().await.is_none());
        }
        // Returning to the exact original tip is not evidence of authority.
        handle.set_tip(255, 0);
        let requests = handle.ranges().len();
        cache.refresh(&balance).await.unwrap();
        assert_eq!(&handle.ranges()[requests..], &[(128, 255)]);
        assert_eq!(
            cache.chain_generation.load(Ordering::Acquire),
            stale_generation.wrapping_add(1)
        );
        assert!(cache.seed_ready.load(Ordering::Acquire));
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_eq!(cache.pending_backfill().await.unwrap().1, 127);
        assert_eq!(
            cache
                .lookup_transaction_block(&balance, &ReorgHandle::suffix_tx())
                .await
                .unwrap()
                .height,
            250,
        );
    }

    #[tokio::test]
    async fn transient_reorg_retains_committed_chunk_and_repairs_lagging_cursor() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(511));
        assert!(cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        assert_eq!(upper, 383);
        let committed = cache.peek_blocks_range(&balance, 256, upper).await.unwrap();
        assert_eq!(
            cache
                .insert_blocks_for_generation(committed, generation, 256, upper, None)
                .await,
            RangeInsert::Inserted,
        );
        assert_eq!(*cache.backfill_resume.read().await, Some(383));
        let before = retained_explorer_state(&cache).await;
        handle.fork_at(480);
        handle.fail_range_once(384, 511);
        assert!(cache.refresh(&balance).await.is_err());
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert!(!cache.seed_ready.load(Ordering::Acquire));
        assert!(cache.pending_backfill().await.is_none());

        cache.refresh(&balance).await.unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        assert_eq!(upper, 255);
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .first_key_value()
                .unwrap()
                .0,
            &256
        );
        assert_eq!(
            cache.blocks_by_height.read().await[&300].block_id,
            ReorgHandle::block_id(0, 300)
        );
        assert_eq!(
            cache.blocks_by_height.read().await[&500].block_id,
            ReorgHandle::block_id(1, 500)
        );
        let requests = handle.ranges().len();
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(&handle.ranges()[requests..], &[(128, 255), (0, 127)]);
        assert_eq!(*cache.backfill_resume.read().await, None);
    }

    #[tokio::test]
    async fn initial_empty_range_failure_keeps_direct_height_reads_and_seed_recovery() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let empty = retained_explorer_state(&cache).await;
        handle.fail_range_once(128, 255);
        assert!(cache.refresh(&balance).await.is_err());
        assert_eq!(retained_explorer_state(&cache).await, empty);
        assert!(!cache.seed_ready.load(Ordering::Acquire));
        assert!(cache.pending_backfill().await.is_none());
        assert_eq!(cache.chain_generation.load(Ordering::Acquire), 0);
        let page = cache.load_full_page_by_height(&balance, 7).await.unwrap();
        assert_eq!(page.height, 7);
        assert_eq!(page.block_id, ReorgHandle::block_id(0, 7));
        handle.fail_range_once(7, 7);
        let error = cache.load_full_page_by_height(&balance, 7).await;
        assert!(error.is_err());
        assert!(!matches!(
            error,
            Err(NockAppGrpcError::Status(status)) if status.code() == tonic::Code::Unavailable
        ));
        assert_eq!(retained_explorer_state(&cache).await, empty);
        cache.refresh(&balance).await.unwrap();
        assert!(cache.seed_ready.load(Ordering::Acquire));
        assert_eq!(cache.get_max_height(), 255);
        assert_eq!(cache.pending_backfill().await, Some((0, 127)));
        assert_connected(&cache, 0).await;
    }
    #[tokio::test(start_paused = true)]
    async fn failed_reconciliation_fences_old_backfill_before_waiting_for_its_permit() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        assert!(cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        let balance: Arc<dyn BalanceHandle> = handle.clone();
        let before = retained_explorer_state(&cache).await;
        let (old_generation, upper) = cache.pending_backfill().await.unwrap();
        handle.pause_range(0, 127);
        let worker = tokio::spawn(cache.clone().backfill_older(
            handle.clone(),
            old_generation,
            upper,
        ));
        tokio::time::timeout(Duration::from_secs(1), handle.wait_for_range())
            .await
            .expect("old history request should hold the range permit");
        handle.fork_at(240);
        handle.fail_range_once(128, 255);
        let refresh = tokio::spawn({
            let cache = cache.clone();
            let balance = balance.clone();
            async move { cache.refresh(&balance).await }
        });
        for _ in 0..100 {
            if cache.chain_generation.load(Ordering::Acquire) != old_generation {
                break;
            }
            tokio::task::yield_now().await;
        }
        let stale_generation = cache.chain_generation.load(Ordering::Acquire);
        assert_eq!(stale_generation, old_generation.wrapping_add(1));
        assert!(!cache.seed_ready.load(Ordering::Acquire));
        assert!(
            !refresh.is_finished(),
            "replacement is still waiting for the old permit"
        );
        assert_eq!(handle.ranges(), vec![(128, 255), (0, 127)]);
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert!(cache.pending_backfill().await.is_none());
        assert_canonical_unavailable(cache.get_blocks_page(None, 1).await);
        // Even a caller using the current stale generation cannot request work.
        cache
            .clone()
            .backfill_older(handle.clone(), stale_generation, upper)
            .await
            .unwrap();
        assert_eq!(handle.ranges(), vec![(128, 255), (0, 127)]);

        handle.release_range();
        worker.await.unwrap().unwrap();
        assert!(refresh.await.unwrap().is_err());
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 0);
        assert!(cache.last_backfill.read().await.is_none());
        assert_eq!(
            cache.chain_generation.load(Ordering::Acquire),
            stale_generation
        );
        assert!(
            !cache
                .clear_cached_chain_for_generation(old_generation)
                .await
        );
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_eq!(*cache.backfill_resume.read().await, Some(127));

        cache.refresh(&balance).await.unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        assert_eq!(upper, 127);
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 1);
        assert_eq!(
            cache.blocks_by_height.read().await[&100].block_id,
            ReorgHandle::block_id(0, 100)
        );
    }
    #[tokio::test(start_paused = true)]
    async fn block_detail_reads_in_flight_across_failed_replacement_are_unavailable() {
        for by_id in [false, true] {
            for fail_old_read in [false, true] {
                let cache = new_cache();
                let handle = Arc::new(ReorgHandle::new(255));
                assert!(cache
                    .clone()
                    .initialize_inner(handle.clone())
                    .await
                    .unwrap());
                let balance: Arc<dyn BalanceHandle> = handle.clone();
                handle.pause_range(250, 250);
                if fail_old_read {
                    handle.fail_range_once(250, 250);
                }
                let lookup = tokio::spawn({
                    let cache = cache.clone();
                    let balance = balance.clone();
                    async move {
                        if by_id {
                            cache
                                .load_full_page_by_id(
                                    &balance,
                                    &ReorgHandle::block_id(0, 250).to_base58(),
                                )
                                .await
                        } else {
                            cache.load_full_page_by_height(&balance, 250).await
                        }
                    }
                });
                tokio::time::timeout(Duration::from_secs(1), handle.wait_for_range())
                    .await
                    .expect("old detail read should reach the barrier");
                handle.fork_at(240);
                handle.fail_range_once(128, 255);
                assert!(cache.refresh(&balance).await.is_err());
                assert!(!cache.seed_ready.load(Ordering::Acquire));
                handle.release_range();
                assert_canonical_unavailable(lookup.await.unwrap());
                cache.refresh(&balance).await.unwrap();
                assert_eq!(
                    cache
                        .load_full_page_by_height(&balance, 250)
                        .await
                        .unwrap()
                        .block_id,
                    ReorgHandle::block_id(1, 250),
                );
                assert!(matches!(
                    cache
                        .load_full_page_by_id(&balance, &ReorgHandle::block_id(0, 250).to_base58())
                        .await,
                    Err(NockAppGrpcError::NotFound),
                ));
            }
        }
    }

    #[tokio::test]
    async fn stale_prefix_hit_miss_and_ambiguity_are_all_unavailable() {
        let cache = new_cache();
        let first = ReorgHandle::suffix_tx();
        let mut second = first.clone();
        second.0[0].0 += 1;
        let first_b58 = first.to_base58();
        let second_b58 = second.to_base58();
        let prefix = &first_b58[..8];
        assert!(second_b58.starts_with(prefix));
        // A ready height-zero fixture with two real canonical hash spellings
        // sharing a prefix; no alternate production index convention is added.
        cache
            .insert_blocks(vec![(
                0,
                ReorgHandle::block_id(0, 0),
                test_hash(1),
                0,
                vec![first.clone(), second],
            )])
            .await;
        cache.seed_ready.store(true, Ordering::Release);
        assert_eq!(cache.lookup_tx_by_prefixes(prefix).await.unwrap().len(), 2);
        let before = retained_explorer_state(&cache).await;
        let handle = Arc::new(ReorgHandle::new(255));
        handle.set_tip(255, 1);
        handle.fail_range_once(128, 255);
        let balance: Arc<dyn BalanceHandle> = handle;
        assert!(cache
            .reconcile_chain(&balance, 0, 255, &ReorgHandle::block_id(1, 255),)
            .await
            .is_err());
        assert_eq!(retained_explorer_state(&cache).await, before);
        assert_canonical_unavailable(cache.lookup_tx_by_prefixes(prefix).await);
        assert_canonical_unavailable(cache.lookup_tx_by_prefixes("11111111").await);
        assert_canonical_unavailable(cache.resolve_tx_id("11111111").await);
        assert_eq!(cache.resolve_tx_id(&first_b58).await.unwrap(), first);
        assert!(matches!(
            cache.lookup_tx_by_prefixes("short").await,
            Err(NockAppGrpcError::TxPrefixTooShort(_)),
        ));
    }

    #[tokio::test]
    async fn seed_reorg_between_tip_and_range_does_not_publish_unanchored_blocks() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(200));
        handle.switch_after_ranges(0, 1);
        assert!(!cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap());
        assert_eq!(cache.get_max_height(), 0);
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        assert!(cache.pending_backfill().await.is_none());

        assert!(cache.clone().initialize_inner(handle).await.unwrap());
        assert_connected(&cache, 1).await;
    }

    #[tokio::test]
    async fn reorg_between_cached_tip_check_and_suffix_fetch_removes_orphan_indexes() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(200));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_some());
        handle.set_tip(202, 0);
        // The singleton cached-tip check succeeds on branch zero; only the
        // separately awaited suffix observes branch one.
        handle.switch_after_ranges(1, 1);
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        for _ in 0..3 {
            cache.refresh(&balance_handle).await.unwrap();
            assert!(cache
                .get_block_for_tx(&ReorgHandle::orphan_tx())
                .await
                .is_none());
            assert!(!cache
                .tx_b58_index
                .read()
                .await
                .contains_key(&ReorgHandle::orphan_tx().to_base58(),));
            assert_connected(&cache, 1).await;
        }
        assert_eq!(&handle.ranges()[..3], &[(73, 200), (200, 200), (201, 202)]);
    }

    #[tokio::test]
    async fn reorg_between_forward_chunks_discards_the_old_prefix() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(128));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        handle.set_tip(400, 0);
        handle.switch_after_ranges(2, 1);
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        cache.refresh(&balance_handle).await.unwrap();
        assert_eq!(
            &handle.ranges()[..4],
            &[(1, 128), (128, 128), (129, 256), (257, 384)],
        );
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        assert_connected(&cache, 1).await;
    }

    #[tokio::test]
    async fn same_height_reorg_replaces_transaction_and_block_indexes() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(200));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        handle.set_tip(200, 1);
        let balance_handle: Arc<dyn BalanceHandle> = handle;
        cache.refresh(&balance_handle).await.unwrap();
        assert_eq!(cache.get_max_height(), 200);
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        assert_connected(&cache, 1).await;
    }

    #[tokio::test]
    async fn backfill_reorg_at_cached_join_invalidates_and_recovers_history() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        handle.set_tip(255, 1);
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(cache.get_max_height(), 0);
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        cache.refresh(&balance_handle).await.unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        cache
            .clone()
            .backfill_older(handle, generation, upper)
            .await
            .unwrap();
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .get(&100)
                .unwrap()
                .block_id,
            ReorgHandle::block_id(1, 100)
        );
        assert!(cache.pending_backfill().await.is_none());
        assert_connected(&cache, 1).await;
    }

    #[tokio::test(start_paused = true)]
    async fn reorg_between_backfill_chunks_discards_previously_fetched_history() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(511));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        handle.switch_after_ranges(1, 1);
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(&handle.ranges()[..3], &[(384, 511), (256, 383), (128, 255)],);
        assert_eq!(cache.get_max_height(), 0);
        assert!(cache.blocks_by_height.read().await.is_empty());
        assert!(cache.blocks_by_id.read().await.is_empty());
        assert!(cache.tx_to_block.read().await.is_empty());
        let balance_handle: Arc<dyn BalanceHandle> = handle;
        cache.refresh(&balance_handle).await.unwrap();
        assert_connected(&cache, 1).await;
    }

    #[tokio::test]
    async fn queued_old_backfill_cannot_claim_a_new_generation() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (old_generation, old_upper) = cache.pending_backfill().await.unwrap();
        handle.set_tip(255, 1);
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        cache.refresh(&balance_handle).await.unwrap();
        let pending = cache.pending_backfill().await;
        let calls = handle.ranges().len();
        cache
            .clone()
            .backfill_older(handle.clone(), old_generation, old_upper)
            .await
            .unwrap();
        assert_eq!(handle.ranges().len(), calls);
        assert_eq!(cache.pending_backfill().await, pending);
        assert_ne!(pending.unwrap().0, old_generation);
        assert_connected(&cache, 1).await;
    }

    #[tokio::test(start_paused = true)]
    async fn in_flight_old_backfill_result_cannot_publish_or_complete_new_history() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (old_generation, upper) = cache.pending_backfill().await.unwrap();
        handle.pause_range(0, 127);
        let worker = tokio::spawn(cache.clone().backfill_older(
            handle.clone(),
            old_generation,
            upper,
        ));
        tokio::time::timeout(Duration::from_secs(1), handle.wait_for_range())
            .await
            .expect("old backfill should start");
        handle.set_tip(255, 1);
        let reseed = tokio::spawn(cache.clone().initialize_inner(handle.clone()));
        for _ in 0..100 {
            if cache.chain_generation.load(Ordering::Acquire) != old_generation {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_ne!(
            cache.chain_generation.load(Ordering::Acquire),
            old_generation
        );
        handle.release_range();
        worker.await.unwrap().unwrap();
        assert!(reseed.await.unwrap().unwrap());
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 0);
        assert!(cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        assert_ne!(generation, old_generation);
        assert_eq!(upper, 127);
        cache
            .clone()
            .backfill_older(handle, generation, upper)
            .await
            .unwrap();
        assert_eq!(cache.backfill_success.load(Ordering::Relaxed), 1);
        assert_connected(&cache, 1).await;
    }

    #[tokio::test]
    async fn unknown_ancestor_discards_history_and_rejects_stale_in_flight_fill() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(511));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (old_generation, upper) = cache.pending_backfill().await.unwrap();
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        // Model a worker that committed its range but has not advanced its cursor.
        let committed = cache
            .peek_blocks_range(&balance_handle, 256, upper)
            .await
            .unwrap();
        assert_eq!(
            cache
                .insert_blocks_for_generation(committed, old_generation, 256, upper, None,)
                .await,
            RangeInsert::Inserted
        );
        // This completed read belongs to the old branch and is still in flight
        // from the index writer's point of view when reconciliation commits.
        let stale = cache
            .peek_blocks_range(&balance_handle, 128, 255)
            .await
            .unwrap();
        handle.fork_at(240);
        cache.refresh(&balance_handle).await.unwrap();
        // The fork predates cached history, so that history cannot be preserved.
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .first_key_value()
                .unwrap()
                .0,
            &384
        );
        assert_eq!(cache.pending_backfill().await.unwrap().1, 383);
        assert_eq!(
            cache
                .insert_blocks_for_generation(stale, old_generation, 128, 255, None)
                .await,
            RangeInsert::Stale
        );
        assert!(cache
            .get_block_for_tx(&ReorgHandle::suffix_tx())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn shallow_fork_preserves_committed_history_and_repairs_backfill_cursor() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(511));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        let committed = cache
            .peek_blocks_range(&balance_handle, 256, upper)
            .await
            .unwrap();
        assert_eq!(
            cache
                .insert_blocks_for_generation(committed, generation, 256, upper, None,)
                .await,
            RangeInsert::Inserted
        );
        handle.fork_at(480);
        cache.refresh(&balance_handle).await.unwrap();
        let (new_generation, resume) = cache.pending_backfill().await.unwrap();
        assert_ne!(new_generation, generation);
        assert_eq!(resume, 255);
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .first_key_value()
                .unwrap()
                .0,
            &256
        );
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .get(&300)
                .unwrap()
                .block_id,
            ReorgHandle::block_id(0, 300)
        );
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .get(&500)
                .unwrap()
                .block_id,
            ReorgHandle::block_id(1, 500)
        );
    }

    #[tokio::test]
    async fn rollback_seed_adopts_newly_available_history_without_losing_common_prefix() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        handle.set_tip(254, 0);
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        cache.refresh(&balance_handle).await.unwrap();
        assert_eq!(cache.pending_backfill().await.unwrap().1, 126);
        assert_eq!(
            cache
                .blocks_by_height
                .read()
                .await
                .first_key_value()
                .unwrap()
                .0,
            &127
        );
        assert_connected(&cache, 0).await;
    }

    #[tokio::test]
    async fn stale_in_flight_fill_cannot_restore_shallow_fork_orphan() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        let balance_handle: Arc<dyn BalanceHandle> = handle.clone();
        let stale = cache
            .peek_blocks_range(&balance_handle, 240, 255)
            .await
            .unwrap();
        handle.fork_at(240);
        cache.refresh(&balance_handle).await.unwrap();
        assert_eq!(
            cache
                .insert_blocks_for_generation(stale, generation, 240, 255, None)
                .await,
            RangeInsert::Stale
        );
        assert!(cache
            .get_block_for_tx(&ReorgHandle::suffix_tx())
            .await
            .is_none());
        assert_eq!(
            cache
                .get_block_for_tx(&ReorgHandle::orphan_tx())
                .await
                .unwrap()
                .height,
            100
        );
        assert!(cache.pending_backfill().await.is_none());
    }

    #[tokio::test]
    async fn reorg_seed_that_disagrees_with_fetched_tip_preserves_no_unverified_history() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(255));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        handle.set_tip(255, 1);
        handle.switch_after_ranges(0, 2);
        let balance_handle: Arc<dyn BalanceHandle> = handle;
        cache.refresh(&balance_handle).await.unwrap();
        assert_eq!(cache.get_max_height(), 0);
        assert!(cache.blocks_by_height.read().await.is_empty());
        assert!(cache.pending_backfill().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn successful_backfill_chunks_do_not_impose_retry_backoff() {
        let cache = new_cache();
        let handle = Arc::new(ReorgHandle::new(511));
        cache
            .clone()
            .initialize_inner(handle.clone())
            .await
            .unwrap();
        let (generation, upper) = cache.pending_backfill().await.unwrap();
        let started = tokio::time::Instant::now();
        cache
            .clone()
            .backfill_older(handle.clone(), generation, upper)
            .await
            .unwrap();
        assert_eq!(tokio::time::Instant::now(), started);
        assert_eq!(
            handle.ranges(),
            vec![(384, 511), (256, 383), (128, 255), (0, 127)]
        );
        assert!(cache.pending_backfill().await.is_none());
    }

    #[tokio::test]
    async fn disconnected_or_incomplete_ranges_publish_no_indexes() {
        let cases = [
            vec![
                (1, test_hash(11), test_hash(10), 10, vec![test_hash(900)]),
                (2, test_hash(12), test_hash(999), 20, Vec::new()),
                (3, test_hash(13), test_hash(12), 30, Vec::new()),
            ],
            vec![
                (1, test_hash(11), test_hash(10), 10, vec![test_hash(900)]),
                (3, test_hash(13), test_hash(11), 30, Vec::new()),
            ],
        ];
        for blocks in cases {
            let cache = new_cache();
            assert_eq!(
                cache
                    .insert_blocks_for_generation(blocks, 0, 1, 3, None)
                    .await,
                RangeInsert::Disconnected,
            );
            assert_eq!(cache.get_max_height(), 0);
            assert!(cache.blocks_by_height.read().await.is_empty());
            assert!(cache.blocks_by_id.read().await.is_empty());
            assert!(cache.tx_to_block.read().await.is_empty());
            assert!(cache.tx_b58_index.read().await.is_empty());
        }
    }

    fn test_hash(seed: u64) -> Hash {
        Hash([Belt(seed + 1), Belt(seed + 2), Belt(seed + 3), Belt(seed + 4), Belt(seed + 5)])
    }

    fn test_block_range_entry(height: u64) -> BlockRangeEntryNoun {
        let digest = test_hash(height + 10);
        BlockRangeEntryNoun {
            height: BlockHeight(Belt(height)),
            tail: BlockRangeEntryTail {
                block_id: digest.clone(),
                tail: PageAndTxs {
                    page: Page {
                        digest,
                        pow: None,
                        parent: test_hash(height + 9),
                        tx_ids: Vec::new(),
                        coinbase: CoinbaseSplit::V1,
                        timestamp: height * 10,
                        epoch_counter: 0,
                        target: BigNum::from_u64(1),
                        accumulated_work: BigNum::from_u64(height + 1),
                        height,
                        msg: Vec::new(),
                    },
                    txs: Vec::new(),
                },
            },
        }
    }

    fn path_items(noun: Noun, space: &NounSpace) -> Vec<Noun> {
        let mut items = Vec::new();
        let mut noun = noun.in_space(space);
        loop {
            if let Ok(atom) = noun.as_atom() {
                assert_eq!(atom.as_u64().expect("path terminator should be u64"), 0);
                return items;
            }
            let cell = noun.as_cell().expect("path should decode as noun list");
            items.push(cell.head().noun());
            noun = cell.tail();
        }
    }

    fn atom_u64(noun: Noun, space: &NounSpace) -> u64 {
        noun.in_space(space)
            .as_atom()
            .expect("expected atom")
            .as_u64()
            .expect("expected u64 atom")
    }
}
