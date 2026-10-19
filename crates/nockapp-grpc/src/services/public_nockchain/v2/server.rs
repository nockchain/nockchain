use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Instant;

use async_trait::async_trait;
use gnort::instrument::TimingCount;
use nockapp::driver::{NativeOwnPokeResult, NativePokeObserver, NockAppHandle, PokeResult};
use nockapp::nockapp::NockAppExit;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::WireRepr;
use nockchain_libp2p_io::metrics::NockchainP2PMetrics;
use nockchain_libp2p_io::peer_stats::{
    global_peer_stats_registry, PeerReqResGeneration as TransportPeerReqResGeneration,
    PeerStatsEntry as TransportPeerStatsEntry, PeerStatsRegistry as TransportPeerStatsRegistry,
};
use nockchain_types::tx_engine::{v0, v1};
use nockvm::noun::{NounAllocator, SIG};
use noun_serde::{NounDecode, NounEncode};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tokio::time::{self, Duration};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tonic_reflection::server::Builder as ReflectionBuilder;
use tonic_web::GrpcWebLayer;
use tracing::{debug, error, info, warn};

use super::block_explorer::BlockExplorerCache;
use super::cache::{
    AddressBalanceCache, DEFAULT_PAGE_BYTES, DEFAULT_PAGE_SIZE, MAX_PAGE_BYTES, MAX_PAGE_SIZE,
};
use super::cors::cors_layer_from_env;
use super::ip_blocklist::{blocklist_layer, IpBlocklist};
use super::metrics::{init_metrics, NockchainGrpcApiMetrics};
use crate::error::{NockAppGrpcError, Result};
use crate::pb::common::v1::{Acknowledged, ErrorCode, ErrorStatus};
use crate::pb::public::v2::nockchain_block_service_server::{
    NockchainBlockService, NockchainBlockServiceServer,
};
use crate::pb::public::v2::nockchain_metrics_service_server::{
    NockchainMetricsService, NockchainMetricsServiceServer,
};
use crate::pb::public::v2::nockchain_service_server::{NockchainService, NockchainServiceServer};
use crate::pb::public::v2::*;
use crate::public_nockchain::v2::cache::{
    CachedBalanceEntryAddress, CachedBalanceEntryFirstName, FirstNameBalanceCache,
};
use crate::public_nockchain::v2::server::wallet_get_balance_request::Selector;
use crate::v2::pagination::{
    decode_cursor_address, decode_cursor_first_name, PageCursorAddress, PageCursorFirstName,
    PageKeyAddress, PageKeyFirstName,
};
use crate::wire_conversion::{create_grpc_wire, grpc_wire_to_nockapp};

const DEFAULT_HEAVIEST_CHAIN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL: Duration = Duration::from_secs(120);

/// Debug-only observation of a validated transaction's own native poke result.
pub(crate) fn transaction_poke_observer(tx_id: v0::Hash) -> Option<NativePokeObserver> {
    if !tracing::enabled!(target: "nockchain::kernel_timing", tracing::Level::DEBUG) {
        return None;
    }
    Some(NativePokeObserver::new(
        nockchain_libp2p_io::observation_time_us,
        move |started_us, completed_us, result| {
            observe_transaction_service(&tx_id, started_us, completed_us, result);
        },
    ))
}

fn own_seen_transaction(effects: &NounSlab, tx_id: &v0::Hash) -> bool {
    let space = effects.noun_space();
    let mut remaining = unsafe { effects.root() }.in_space(&space);
    let mut matching = false;
    loop {
        let list_cell = match remaining.as_cell() {
            Ok(cell) => cell,
            Err(_) => {
                return matching
                    && remaining.as_atom().and_then(|atom| atom.as_u64()).ok() == Some(0);
            }
        };
        remaining = list_cell.tail();
        let effect_cell = match list_cell.head().as_cell() {
            Ok(cell) => cell,
            Err(_) => return false,
        };
        if !effect_cell.head().eq_bytes(b"seen") {
            continue;
        }
        let seen_cell = match effect_cell.tail().as_cell() {
            Ok(cell) => cell,
            Err(_) => return false,
        };
        if seen_cell.head().eq_bytes(b"tx") {
            match v0::Hash::from_noun(&seen_cell.tail().noun(), &space) {
                Ok(seen_id) => matching |= seen_id == *tx_id,
                Err(_) => return false,
            }
        }
    }
}

fn observe_transaction_service(
    tx_id: &v0::Hash,
    service_started_us: u64,
    service_completed_us: u64,
    result: &NativeOwnPokeResult,
) {
    if !tracing::enabled!(target: "nockchain::kernel_timing", tracing::Level::DEBUG)
        || !result
            .as_ref()
            .is_ok_and(|effects| own_seen_transaction(effects, tx_id))
    {
        return;
    }
    let monotonic_us = nockchain_libp2p_io::observation_time_us();
    if service_started_us > service_completed_us || service_completed_us > monotonic_us {
        return;
    }
    // %seen proves accepted or already retained service, not first acceptance.
    // This native round trip includes the Serf queue, not just execution or commit.
    debug!(
        target: "nockchain::kernel_timing",
        content_id = %tx_id.to_base58(),
        service_started_us,
        service_completed_us,
        monotonic_us,
        outcome = "ack",
        accepted = true,
        acceptance_basis = "own_seen_effect",
        boundary = "kernel_round_trip",
        "Transaction kernel service completed"
    );
}

#[derive(Clone, Default)]
struct BalanceInflight {
    gates: Arc<StdMutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
}

impl BalanceInflight {
    fn gate(&self, key: String) -> Arc<AsyncMutex<()>> {
        let mut gates = self.gates.lock().expect("balance gate map poisoned");
        if gates.len() >= 4096 {
            gates.retain(|_, gate| gate.strong_count() > 0);
        }
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }
}

#[async_trait]
pub trait BalanceHandle: Send + Sync {
    async fn peek(
        &self,
        path: NounSlab,
    ) -> std::result::Result<Option<NounSlab>, nockapp::nockapp::error::NockAppError>;

    async fn poke(
        &self,
        wire: WireRepr,
        payload: NounSlab,
        observer: Option<NativePokeObserver>,
    ) -> std::result::Result<PokeResult, nockapp::nockapp::error::NockAppError>;
}

struct NockAppBalanceHandle(NockAppHandle);

#[async_trait]
impl BalanceHandle for NockAppBalanceHandle {
    async fn peek(
        &self,
        path: NounSlab,
    ) -> std::result::Result<Option<NounSlab>, nockapp::nockapp::error::NockAppError> {
        self.0.peek(path).await
    }

    async fn poke(
        &self,
        wire: WireRepr,
        payload: NounSlab,
        observer: Option<NativePokeObserver>,
    ) -> std::result::Result<PokeResult, nockapp::nockapp::error::NockAppError> {
        match observer {
            Some(observer) => self.0.poke_observed(wire, payload, observer).await,
            None => self.0.poke(wire, payload).await,
        }
    }
}

#[derive(Clone)]
pub struct PublicNockchainGrpcServer {
    handle: Arc<dyn BalanceHandle>,
    exit: Option<NockAppExit>,
    cache_by_address: AddressBalanceCache,
    cache_by_first_name: FirstNameBalanceCache,
    block_explorer_cache: Arc<BlockExplorerCache>,
    metrics: Arc<NockchainGrpcApiMetrics>,
    heaviest_chain: Arc<RwLock<Option<HeaviestChainSnapshot>>>,
    balance_inflight: BalanceInflight,
}

#[derive(Clone)]
struct HeaviestChainSnapshot {
    height: v1::BlockHeight,
    block_id: v1::Hash,
    fetched_at: Instant,
}

impl PublicNockchainGrpcServer {
    pub fn new(handle: NockAppHandle) -> Self {
        let metrics = init_metrics();
        let block_explorer_cache = Arc::new(BlockExplorerCache::new(metrics.clone()));
        let exit = handle.exit.clone();
        Self {
            handle: Arc::new(NockAppBalanceHandle(handle)),
            exit: Some(exit),
            cache_by_address: AddressBalanceCache::new(),
            cache_by_first_name: FirstNameBalanceCache::new(),
            block_explorer_cache,
            metrics,
            heaviest_chain: Arc::new(RwLock::new(None)),
            balance_inflight: BalanceInflight::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_handle(handle: Arc<dyn BalanceHandle>) -> Self {
        let metrics = init_metrics();
        let block_explorer_cache = Arc::new(BlockExplorerCache::new(metrics.clone()));
        Self {
            handle,
            exit: None,
            cache_by_address: AddressBalanceCache::new(),
            cache_by_first_name: FirstNameBalanceCache::new(),
            block_explorer_cache,
            metrics,
            heaviest_chain: Arc::new(RwLock::new(None)),
            balance_inflight: BalanceInflight::default(),
        }
    }

    #[tracing::instrument(
        name = "grpc.public_nockchain.serve",
        skip(self),
        fields(addr = tracing::field::Empty)
    )]
    pub async fn serve(self, addr: SocketAddr) -> Result<()> {
        tracing::Span::current().record("addr", &tracing::field::display(addr));
        info!("Starting PublicNockchain gRPC server on {}", addr);
        let (health_reporter, health_service) = tonic_health::server::health_reporter();
        health_reporter
            .set_serving::<NockchainServiceServer<PublicNockchainGrpcServer>>()
            .await;
        health_reporter
            .set_not_serving::<NockchainBlockServiceServer<NockchainBlockServer>>()
            .await;
        let reflection_service_v1 = ReflectionBuilder::configure()
            .register_encoded_file_descriptor_set(nockapp_grpc_proto::pb::FILE_DESCRIPTOR_SET)
            .build_v1()
            .map_err(|e| {
                NockAppGrpcError::Internal(format!("Failed to build v1 reflection service: {}", e))
            })?;
        if let Err(err) = self.refresh_heaviest_chain().await {
            self.metrics.heaviest_chain_refresh_failure.increment();
            warn!("Failed to seed heaviest chain cache: {}", err);
        }
        self.start_heaviest_chain_refresh();

        // Initialize block explorer cache
        // We need to get the raw handle for initialization
        // Since self.handle is Arc<dyn BalanceHandle>, we need to work around this
        // For now, we'll initialize in the background task
        self.start_block_explorer_refresh(health_reporter.clone());

        let nockchain_api = NockchainServiceServer::new(self.clone());

        // Create block explorer service
        let block_explorer_api = NockchainBlockServiceServer::new(NockchainBlockServer::new(
            self.handle.clone(),
            self.block_explorer_cache.clone(),
            self.metrics.clone(),
        ));
        let metrics_api = NockchainMetricsServiceServer::new(NockchainMetricsServer::new(
            self.handle.clone(),
            self.block_explorer_cache.clone(),
            self.metrics.clone(),
        ));
        // Restrict browser CORS to an explicit allowlist from
        // NOCKCHAIN_API_CORS_ALLOWED_ORIGINS (comma/whitespace separated).
        // Empty/unset => no browser origins allowed; native gRPC clients send
        // no Origin header and are unaffected. Methods/headers stay permissive
        // so gRPC-Web still works for an allowed origin.
        let cors = cors_layer_from_env();

        // Reject blocked client IPs (from the front proxy's x-forwarded-for)
        // before requests reach any service. Configured via the
        // NOCKCHAIN_API_IP_BLOCKLIST env var on top of compiled-in defaults.
        let blocklist = IpBlocklist::from_env_and_defaults();

        Server::builder()
            .accept_http1(true)
            .layer(cors)
            .layer(GrpcWebLayer::new())
            .layer(blocklist_layer(blocklist, self.metrics.clone()))
            .add_service(health_service)
            .add_service(reflection_service_v1)
            .add_service(nockchain_api)
            .add_service(block_explorer_api)
            .add_service(metrics_api)
            .serve(addr)
            .await
            .map_err(NockAppGrpcError::Transport)?;
        Ok(())
    }

    fn build_error_response<T>(&self, error: NockAppGrpcError) -> T
    where
        T: From<ErrorStatus>,
    {
        let error_status = ErrorStatus {
            code: match &error {
                NockAppGrpcError::PeekFailed => ErrorCode::PeekFailed as i32,
                NockAppGrpcError::PokeFailed => ErrorCode::PokeFailed as i32,
                NockAppGrpcError::Timeout => ErrorCode::Timeout as i32,
                NockAppGrpcError::InvalidRequest(_) => ErrorCode::InvalidRequest as i32,
                _ => ErrorCode::InternalError as i32,
            },
            message: error.to_string(),
            details: None,
        };
        T::from(error_status)
    }

    #[tracing::instrument(name = "public_nockchain.peek_heaviest_chain_path", skip(self))]
    async fn peek_heaviest_chain(&self) -> Result<Option<(v1::BlockHeight, v1::Hash)>> {
        let metrics = &self.metrics;

        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "heaviest-chain").as_noun();
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, SIG]);
        path_slab.set_root(path_noun);

        let started = Instant::now();
        let peek_result = self.handle.peek(path_slab).await;
        metrics.heaviest_chain_peek.add_timing(&started.elapsed());

        let result = match peek_result {
            Ok(Some(result_slab)) => {
                let result_noun = unsafe { result_slab.root() };
                let space = result_slab.noun_space();
                match <Option<Option<(v1::BlockHeight, v1::Hash)>>>::from_noun(&result_noun, &space)
                {
                    Ok(opt) => Ok(opt.flatten()),
                    // Peek either returned [~ ~] or ~
                    Err(_) => Err(NockAppGrpcError::PeekReturnedNoData),
                }
            }
            Ok(None) => Err(NockAppGrpcError::PeekFailed),
            Err(e) => Err(NockAppGrpcError::from(e)),
        };

        result
    }

    fn start_heaviest_chain_refresh(&self) {
        let server = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(DEFAULT_HEAVIEST_CHAIN_REFRESH_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(err) = server.refresh_heaviest_chain().await {
                    server.metrics.heaviest_chain_refresh_failure.increment();
                    warn!("Failed to refresh heaviest chain cache: {}", err);
                }
            }
        });
    }

    fn start_block_explorer_refresh(
        &self,
        health_reporter: tonic_health::server::HealthReporter,
    ) -> tokio::task::JoinHandle<()> {
        let server = self.clone();
        tokio::spawn(async move {
            health_reporter
                .set_not_serving::<NockchainBlockServiceServer<NockchainBlockServer>>()
                .await;

            let cache = server.block_explorer_cache.clone();
            let handle = server.handle.clone();
            let exit = server.exit.clone();

            // Helper to handle fatal decode errors
            let handle_fatal_error = |err: &NockAppGrpcError, _exit: &Option<_>, context: &str| {
                if matches!(err, NockAppGrpcError::NounDecode(_)) {
                    error!(
                        "Fatal decode error during {}: {}. Signaling server shutdown.",
                        context, err
                    );
                    true
                } else {
                    false
                }
            };

            info!("Block explorer refresh worker starting");
            let mut interval = time::interval(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL);
            let mut initialized = false;
            let mut backfill_worker: Option<(u64, tokio::task::JoinHandle<()>)> = None;

            loop {
                interval.tick().await;

                // Attempt initialization if not yet successful
                if !initialized {
                    info!("Block explorer attempting initialization");
                    match cache.clone().initialize(handle.clone()).await {
                        Ok(()) => {
                            info!("Block explorer cache initialized successfully");
                            initialized = true;
                            health_reporter
                                .set_serving::<NockchainBlockServiceServer<NockchainBlockServer>>()
                                .await;
                        }
                        Err(err) => {
                            if handle_fatal_error(&err, &exit, "block explorer initialization") {
                                if let Some(ref exit_handle) = exit {
                                    if let Err(exit_err) = exit_handle.exit(1).await {
                                        error!("Failed to signal exit: {:?}", exit_err);
                                    }
                                }
                                return;
                            }
                            warn!(
                                "Failed to initialize block explorer cache: {}, will retry",
                                err
                            );
                        }
                    }
                    continue; // Skip refresh on init attempts
                }

                // Normal refresh cycle (only after successful init)
                if let Err(err) = cache.refresh(&handle).await {
                    if handle_fatal_error(&err, &exit, "block explorer refresh") {
                        if let Some(ref exit_handle) = exit {
                            if let Err(exit_err) = exit_handle.exit(1).await {
                                error!("Failed to signal exit: {:?}", exit_err);
                            }
                        }
                        return;
                    }
                    warn!("Failed to refresh block explorer cache: {}", err);
                }

                // A pending cursor belongs to a chain generation, not to the
                // lifetime of this refresh loop. Completed history has no cursor.
                if let Some((generation, resume_height)) = cache.pending_backfill().await {
                    let already_running = backfill_worker
                        .as_ref()
                        .is_some_and(|(active, task)| *active == generation && !task.is_finished());
                    if !already_running {
                        if let Some((_, old_worker)) = backfill_worker.take() {
                            old_worker.abort();
                        }
                        let backfill_cache = cache.clone();
                        let backfill_handle = handle.clone();
                        let backfill_exit = exit.clone();
                        let worker = tokio::spawn(async move {
                            info!(
                                generation,
                                resume_height, "Block explorer backfill worker starting"
                            );
                            match backfill_cache
                                .backfill_older(backfill_handle, generation, resume_height)
                                .await
                            {
                                Ok(_) => info!("Block explorer backfill worker finished"),
                                Err(err) => {
                                    if matches!(&err, NockAppGrpcError::NounDecode(_)) {
                                        error!(
                                            "Fatal decode error during block explorer backfill: {}. \
                                             Signaling server shutdown.",
                                            err
                                        );
                                        if let Some(ref exit) = backfill_exit {
                                            if let Err(exit_err) = exit.exit(1).await {
                                                error!("Failed to signal exit: {:?}", exit_err);
                                            }
                                        }
                                        return;
                                    }
                                    warn!("Block explorer backfill failed: {}", err);
                                }
                            }
                        });
                        backfill_worker = Some((generation, worker));
                    }
                }
            }
        })
    }

    #[tracing::instrument(
        name = "grpc.heaviest_chain.refresh",
        skip(self),
        fields(new_height = tracing::field::Empty)
    )]
    async fn refresh_heaviest_chain(&self) -> Result<()> {
        match self.peek_heaviest_chain().await? {
            Some((height, block_id)) => {
                tracing::debug!("refreshed heaviest chain");
                let mut guard = self.heaviest_chain.write().await;
                let new_height_value = height.0 .0;
                tracing::Span::current()
                    .record("new_height", &tracing::field::display(new_height_value));
                // The peek reports the tip, and a reorg onto a chain with more
                // accumulated work in fewer blocks lowers it. The snapshot must
                // follow it down: it keys the balance cache, so pinning it to a
                // height the chain no longer holds misses on every lookup.
                if let Some(current) = guard.as_ref() {
                    if new_height_value < current.height.0 .0 {
                        warn!(
                            new_height = new_height_value,
                            cached_height = current.height.0 .0,
                            "Heaviest chain tip moved down: reorg onto a shorter heavier chain"
                        );
                    }
                }
                let snapshot = HeaviestChainSnapshot {
                    height,
                    block_id,
                    fetched_at: Instant::now(),
                };
                *guard = Some(snapshot);
                self.metrics.heaviest_chain_age_seconds.swap(0.0);
            }
            None => {}
        }
        Ok(())
    }

    async fn cached_heaviest_chain(&self) -> Option<(v1::BlockHeight, v1::Hash)> {
        let guard = self.heaviest_chain.read().await;
        if let Some(snapshot) = guard.as_ref() {
            let age = snapshot.fetched_at.elapsed().as_secs_f64();
            self.metrics.heaviest_chain_age_seconds.swap(age);
            Some((snapshot.height.clone(), snapshot.block_id.clone()))
        } else {
            self.metrics.heaviest_chain_age_seconds.swap(-1.0);
            None
        }
    }
}

/// Separate service for block explorer APIs
#[derive(Clone)]
pub struct NockchainBlockServer {
    handle: Arc<dyn BalanceHandle>,
    block_explorer_cache: Arc<BlockExplorerCache>,
    metrics: Arc<NockchainGrpcApiMetrics>,
}

#[derive(Clone)]
pub struct NockchainMetricsServer {
    #[allow(dead_code)]
    handle: Arc<dyn BalanceHandle>,
    block_explorer_cache: Arc<BlockExplorerCache>,
    metrics: Arc<NockchainGrpcApiMetrics>,
    peer_stats_registry: Arc<TransportPeerStatsRegistry>,
    transport_metrics: Arc<NockchainP2PMetrics>,
}

impl NockchainBlockServer {
    pub fn new(
        handle: Arc<dyn BalanceHandle>,
        cache: Arc<BlockExplorerCache>,
        metrics: Arc<NockchainGrpcApiMetrics>,
    ) -> Self {
        Self {
            handle,
            block_explorer_cache: cache,
            metrics,
        }
    }
}

impl NockchainMetricsServer {
    pub fn new(
        handle: Arc<dyn BalanceHandle>,
        cache: Arc<BlockExplorerCache>,
        metrics: Arc<NockchainGrpcApiMetrics>,
    ) -> Self {
        Self {
            handle,
            block_explorer_cache: cache,
            metrics,
            peer_stats_registry: global_peer_stats_registry(),
            transport_metrics: Arc::new(
                NockchainP2PMetrics::register(gnort::global_metrics_registry())
                    .expect("Failed to register transport metrics!"),
            ),
        }
    }

    #[cfg(test)]
    fn with_peer_stats_registry(
        handle: Arc<dyn BalanceHandle>,
        cache: Arc<BlockExplorerCache>,
        metrics: Arc<NockchainGrpcApiMetrics>,
        peer_stats_registry: Arc<TransportPeerStatsRegistry>,
    ) -> Self {
        Self {
            handle,
            block_explorer_cache: cache,
            metrics,
            peer_stats_registry,
            transport_metrics: Arc::new(
                NockchainP2PMetrics::register(gnort::global_metrics_registry())
                    .expect("Failed to register transport metrics!"),
            ),
        }
    }

    fn map_peer_generation(generation: TransportPeerReqResGeneration) -> PeerReqResGeneration {
        match generation {
            TransportPeerReqResGeneration::Unknown => PeerReqResGeneration::Unspecified,
            TransportPeerReqResGeneration::Gen1 => PeerReqResGeneration::Gen1,
            TransportPeerReqResGeneration::Gen2 => PeerReqResGeneration::Gen2,
        }
    }

    fn map_peer_stat(entry: TransportPeerStatsEntry) -> PeerStat {
        PeerStat {
            peer_id: entry.peer_id,
            protocol_generation: Self::map_peer_generation(entry.protocol_generation).into(),
            request_count: entry.request_count,
            bytes_sent: entry.bytes_sent,
            bytes_received: entry.bytes_received,
            average_round_trip_ms: entry.average_round_trip_ms,
            average_batch_size: entry.average_batch_size,
            failure_count: entry.failure_count,
            timeout_count: entry.timeout_count,
            blocks_received: entry.blocks_received,
            average_block_propagation_ms: entry.average_block_propagation_ms,
            connection_duration_seconds: entry.connection_duration_seconds,
        }
    }

    fn req_res_metrics_snapshot(&self) -> ReqResMetricsData {
        ReqResMetricsData {
            gen1_outbound_timeouts: self
                .transport_metrics
                .gen1_outbound_timeouts
                .fetch_add(0)
                .try_into()
                .expect("timeout counter should fit in u64"),
            gen2_outbound_timeouts: self
                .transport_metrics
                .gen2_outbound_timeouts
                .fetch_add(0)
                .try_into()
                .expect("timeout counter should fit in u64"),
            gen2_batch_requests_sent: self
                .transport_metrics
                .gen2_batch_requests_sent
                .fetch_add(0)
                .try_into()
                .expect("batch request counter should fit in u64"),
            gen2_batch_requests_received: self
                .transport_metrics
                .gen2_batch_requests_received
                .fetch_add(0)
                .try_into()
                .expect("batch request counter should fit in u64"),
            req_res_fallback_total: self
                .transport_metrics
                .req_res_fallback_total
                .fetch_add(0)
                .try_into()
                .expect("fallback counter should fit in u64"),
            req_res_block_by_height_gen1_routed: self
                .transport_metrics
                .req_res_block_by_height_gen1_routed
                .fetch_add(0)
                .try_into()
                .expect("fallback route counter should fit in u64"),
        }
    }
}

#[tonic::async_trait]
impl NockchainMetricsService for NockchainMetricsServer {
    async fn get_explorer_metrics(
        &self,
        _request: Request<GetExplorerMetricsRequest>,
    ) -> std::result::Result<Response<GetExplorerMetricsResponse>, Status> {
        // Fast path: metrics_snapshot no longer does kernel peeks, just reads atomics/cached values
        let snapshot = self.block_explorer_cache.metrics_snapshot().await;
        {
            self.metrics
                .block_explorer_cache_height
                .swap(snapshot.cache_height as f64);
            self.metrics
                .block_explorer_heaviest_height
                .swap(snapshot.heaviest_height as f64);
            self.metrics
                .block_explorer_seed_ready
                .swap(if snapshot.seed_ready { 1.0 } else { 0.0 });
            self.metrics
                .block_explorer_cache_lowest_height
                .swap(snapshot.cache_lowest_height as f64);
            self.metrics
                .block_explorer_cache_span
                .swap(snapshot.cache_span as f64);
            self.metrics
                .block_explorer_cache_coverage_ratio
                .swap(snapshot.cache_coverage_ratio);
            self.metrics
                .block_explorer_backfill_resume_height
                .swap(snapshot.backfill_resume_height.unwrap_or(u64::MAX) as f64);
            self.metrics
                .block_explorer_cache_age_seconds
                .swap(snapshot.cache_age_seconds);
            self.metrics
                .block_explorer_refresh_age_seconds
                .swap(snapshot.refresh_age_seconds);
            self.metrics
                .block_explorer_backfill_age_seconds
                .swap(snapshot.backfill_age_seconds.unwrap_or(-1.0));
            self.metrics
                .block_explorer_seed_time_seconds
                .swap(snapshot.seed_time_seconds);
            self.metrics
                .block_explorer_get_blocks_p50_ms
                .swap(snapshot.get_blocks_p50_ms);
            self.metrics
                .block_explorer_get_blocks_p90_ms
                .swap(snapshot.get_blocks_p90_ms);
            self.metrics
                .block_explorer_get_blocks_p99_ms
                .swap(snapshot.get_blocks_p99_ms);
            self.metrics
                .block_explorer_get_block_details_p50_ms
                .swap(snapshot.get_block_details_p50_ms);
            self.metrics
                .block_explorer_get_block_details_p90_ms
                .swap(snapshot.get_block_details_p90_ms);
            self.metrics
                .block_explorer_get_block_details_p99_ms
                .swap(snapshot.get_block_details_p99_ms);

            let backfill_height = snapshot
                .backfill_resume_height
                .map(|h| h as i64)
                .unwrap_or(-1);
            let resp = GetExplorerMetricsResponse {
                result: Some(get_explorer_metrics_response::Result::Metrics(
                    ExplorerMetrics {
                        cache_height: snapshot.cache_height,
                        heaviest_height: snapshot.heaviest_height,
                        cache_lowest_height: snapshot.cache_lowest_height,
                        cache_span: snapshot.cache_span,
                        cache_coverage_ratio: snapshot.cache_coverage_ratio,
                        refresh_age_seconds: snapshot.refresh_age_seconds,
                        backfill_age_seconds: snapshot.backfill_age_seconds.unwrap_or(-1.0),
                        seed_ready: snapshot.seed_ready,
                        seed_time_seconds: snapshot.seed_time_seconds,
                        backfill_resume_height: backfill_height,
                        cache_age_seconds: snapshot.cache_age_seconds,
                        refresh_success_count: snapshot.refresh_success_count,
                        refresh_error_count: snapshot.refresh_error_count,
                        backfill_success_count: snapshot.backfill_success_count,
                        backfill_error_count: snapshot.backfill_error_count,
                        get_blocks_p50_ms: snapshot.get_blocks_p50_ms,
                        get_blocks_p90_ms: snapshot.get_blocks_p90_ms,
                        get_blocks_p99_ms: snapshot.get_blocks_p99_ms,
                        get_block_details_p50_ms: snapshot.get_block_details_p50_ms,
                        get_block_details_p90_ms: snapshot.get_block_details_p90_ms,
                        get_block_details_p99_ms: snapshot.get_block_details_p99_ms,
                    },
                )),
            };
            Ok(Response::new(resp))
        }
    }

    async fn get_peer_stats(
        &self,
        _request: Request<GetPeerStatsRequest>,
    ) -> std::result::Result<Response<GetPeerStatsResponse>, Status> {
        let snapshot = self.peer_stats_registry.snapshot();
        Ok(Response::new(GetPeerStatsResponse {
            result: Some(get_peer_stats_response::Result::Stats(PeerStatsData {
                collected_at_unix_ms: snapshot.collected_at_unix_ms,
                peers: snapshot
                    .peers
                    .into_iter()
                    .map(Self::map_peer_stat)
                    .collect(),
            })),
        }))
    }

    async fn get_req_res_metrics(
        &self,
        _request: Request<GetReqResMetricsRequest>,
    ) -> std::result::Result<Response<GetReqResMetricsResponse>, Status> {
        Ok(Response::new(GetReqResMetricsResponse {
            result: Some(get_req_res_metrics_response::Result::Metrics(
                self.req_res_metrics_snapshot(),
            )),
        }))
    }
}

fn timed_return<T>(metric: &TimingCount, started: Instant, value: T) -> T {
    metric.add_timing(&started.elapsed());
    value
}

#[tonic::async_trait]
impl NockchainService for PublicNockchainGrpcServer {
    async fn wallet_get_balance(
        &self,
        request: Request<WalletGetBalanceRequest>,
    ) -> std::result::Result<Response<WalletGetBalanceResponse>, Status> {
        let remote_addr = request.remote_addr();
        let x_forwarded_for = request
            .metadata()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let req = request.into_inner();
        let request_start = Instant::now();
        let metrics = &self.metrics;
        info!(
            "WalletGetBalance client_ip={:?} x_forwarded_for={:?}",
            remote_addr, x_forwarded_for
        );

        let WalletGetBalanceRequest { selector, page, .. } = req;
        if selector.is_none() {
            self.metrics
                .balance_request_error_invalid_request_missing_selector
                .increment();
            let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::InvalidRequest(
                "selector is required".into(),
            ));
            return timed_return(
                &metrics.balance_update_error,
                request_start,
                Ok(Response::new(WalletGetBalanceResponse {
                    result: Some(wallet_get_balance_response::Result::Error(err)),
                })),
            );
        }

        let (client_page_items_limit, token, max_bytes) = if let Some(request) = page {
            (
                if request.client_page_items_limit == 0 {
                    DEFAULT_PAGE_SIZE
                } else {
                    std::cmp::min(request.client_page_items_limit as usize, MAX_PAGE_SIZE)
                },
                request.page_token,
                if request.max_bytes == 0 {
                    DEFAULT_PAGE_BYTES
                } else {
                    std::cmp::min(request.max_bytes, MAX_PAGE_BYTES)
                },
            )
        } else {
            self.metrics
                .balance_request_error_invalid_request_page_missing
                .increment();
            let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::InvalidRequest(
                "Page request is missing".into(),
            ));
            return timed_return(
                &metrics.balance_update_error,
                request_start,
                Ok(Response::new(WalletGetBalanceResponse {
                    result: Some(wallet_get_balance_response::Result::Error(err)),
                })),
            );
        };

        match selector.expect("selector should be set") {
            Selector::Address(address) => {
                let cursor: Option<PageCursorAddress> = if !token.is_empty() {
                    match decode_cursor_address(&token) {
                        Some(cur) => Some(cur),
                        None => {
                            self.metrics
                                .balance_request_error_invalid_request_token_invalid
                                .increment();
                            let err = ErrorStatus {
                                code: ErrorCode::InvalidRequest as i32,
                                message: "Invalid page token".into(),
                                details: None,
                            };
                            return timed_return(
                                &metrics.balance_update_error,
                                request_start,
                                Ok(Response::new(WalletGetBalanceResponse {
                                    result: Some(wallet_get_balance_response::Result::Error(err)),
                                })),
                            );
                        }
                    }
                } else {
                    None
                };

                if v1::SchnorrPubkey::from_base58(&address.key).is_err() {
                    self.metrics
                        .balance_request_error_invalid_request_address_format
                        .increment();
                    let err = self.build_error_response::<ErrorStatus>(
                        NockAppGrpcError::InvalidRequest("Address is improperly formatted".into()),
                    );
                    return timed_return(
                        &metrics.balance_update_error,
                        request_start,
                        Ok(Response::new(WalletGetBalanceResponse {
                            result: Some(wallet_get_balance_response::Result::Error(err)),
                        })),
                    );
                };

                if let Some(ref cur) = cursor {
                    if cur.key.address != address.key {
                        self.metrics
                            .balance_request_error_invalid_request_token_mismatch
                            .increment();
                        let err = self.build_error_response::<ErrorStatus>(
                            NockAppGrpcError::InvalidRequest(
                                "Page token does not match requested address".into(),
                            ),
                        );
                        return timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        );
                    }
                }

                let mut cached: Option<Arc<CachedBalanceEntryAddress>> = None;

                if let Some(ref cursor) = cursor {
                    cached = self.cache_by_address.get(cursor.key())
                } else {
                    match self.cached_heaviest_chain().await {
                        Some((height, block_id)) => {
                            let cache_key = PageKeyAddress::new(
                                address.key.clone(),
                                height.0 .0,
                                block_id.clone(),
                            );
                            cached = self.cache_by_address.get(&cache_key);
                        }
                        None => {
                            warn!("Cache missed for heaviest chain, this should never happen except with a fresh nockchain node.");
                            self.metrics.heaviest_chain_cache_miss.increment();
                        }
                    }
                }

                // Only first-page cold misses need singleflight. Hold the
                // per-address gate through the peek and cache insertion below.
                let _inflight = if cached.is_none() && cursor.is_none() {
                    let gate = self
                        .balance_inflight
                        .gate(format!("address:{}", address.key));
                    let guard = gate.lock_owned().await;
                    if self.cached_heaviest_chain().await.is_none() {
                        if let Err(err) = self.refresh_heaviest_chain().await {
                            warn!("Could not refresh heaviest tip for address cache: {err}");
                        }
                    }
                    if let Some((height, block_id)) = self.cached_heaviest_chain().await {
                        cached = self.cache_by_address.get(&PageKeyAddress::new(
                            address.key.clone(),
                            height.0 .0,
                            block_id,
                        ));
                    }
                    Some(guard)
                } else {
                    None
                };

                if let Some(cached) = cached {
                    tracing::debug!("Cache hit for address: {}", address.key);
                    self.metrics.balance_cache_address_hit.increment();
                    match cached.build_paginated_response_address(
                        cursor.clone(),
                        client_page_items_limit,
                        max_bytes,
                        &self.metrics,
                    ) {
                        Ok(response) => {
                            return timed_return(
                                &metrics.balance_update_success_hit,
                                request_start,
                                Ok(Response::new(response)),
                            )
                        }
                        Err(err) => {
                            return timed_return(
                                &metrics.balance_update_error,
                                request_start,
                                Ok(Response::new(WalletGetBalanceResponse {
                                    result: Some(wallet_get_balance_response::Result::Error(err)),
                                })),
                            );
                        }
                    }
                }

                self.metrics.balance_cache_address_miss.increment();
                let path = vec!["balance-by-pubkey".to_string(), address.key.clone()];
                let mut path_slab = NounSlab::new();
                let path_noun = path.to_noun(&mut path_slab);
                path_slab.set_root(path_noun);

                info!(
                    "peek path=balance-by-pubkey address={} client_ip={:?} x_forwarded_for={:?}",
                    address.key, remote_addr, x_forwarded_for
                );
                let peek_start = Instant::now();
                let peek_result = self.handle.peek(path_slab).await;
                self.metrics
                    .balance_update_peek_time
                    .add_timing(&peek_start.elapsed());
                match peek_result {
                    Ok(Some(result_slab)) => {
                        let result_noun = unsafe { result_slab.root() };
                        let space = result_slab.noun_space();
                        let result =
                            <Option<Option<v0::BalanceUpdate>>>::from_noun(&result_noun, &space);

                        match result {
                            Ok(update) => {
                                let update = match update {
                                    // Peek result is double wrapped unit over the balance update
                                    Some(Some(update)) => update,
                                    Some(None) | None => {
                                        self.metrics.balance_request_error_peek_failed.increment();
                                        let err = self.build_error_response::<ErrorStatus>(
                                            NockAppGrpcError::PeekFailed,
                                        );
                                        return timed_return(
                                            &metrics.balance_update_error,
                                            request_start,
                                            Ok(Response::new(WalletGetBalanceResponse {
                                                result: Some(
                                                    wallet_get_balance_response::Result::Error(err),
                                                ),
                                            })),
                                        );
                                    }
                                };
                                let entry = self.cache_by_address.insert(&address.key, update);

                                match entry.build_paginated_response_address(
                                    cursor.clone(),
                                    client_page_items_limit,
                                    max_bytes,
                                    &self.metrics,
                                ) {
                                    Ok(response) => {
                                        return timed_return(
                                            &metrics.balance_update_success_miss,
                                            request_start,
                                            Ok(Response::new(response)),
                                        );
                                    }
                                    Err(err) => {
                                        return timed_return(
                                            &metrics.balance_update_error,
                                            request_start,
                                            Ok(Response::new(WalletGetBalanceResponse {
                                                result: Some(
                                                    wallet_get_balance_response::Result::Error(err),
                                                ),
                                            })),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                self.metrics.balance_request_error_decode.increment();
                                let err = self.build_error_response::<ErrorStatus>(
                                    NockAppGrpcError::NounDecode(e),
                                );
                                return timed_return(
                                    &metrics.balance_update_error,
                                    request_start,
                                    Ok(Response::new(WalletGetBalanceResponse {
                                        result: Some(wallet_get_balance_response::Result::Error(
                                            err,
                                        )),
                                    })),
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        self.metrics.balance_request_error_peek_failed.increment();
                        let err =
                            self.build_error_response::<ErrorStatus>(NockAppGrpcError::PeekFailed);
                        timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        )
                    }
                    Err(e) => {
                        self.metrics.balance_request_error_nockapp.increment();
                        let err =
                            self.build_error_response::<ErrorStatus>(NockAppGrpcError::NockApp(e));
                        timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        )
                    }
                }
            }
            Selector::FirstName(first_name_str) => {
                let cursor: Option<PageCursorFirstName> = if !token.is_empty() {
                    match decode_cursor_first_name(&token) {
                        Some(cur) => Some(cur),
                        None => {
                            self.metrics
                                .balance_request_error_invalid_request_token_invalid
                                .increment();
                            let err = ErrorStatus {
                                code: ErrorCode::InvalidRequest as i32,
                                message: "Invalid page token".into(),
                                details: None,
                            };
                            return timed_return(
                                &metrics.balance_update_error,
                                request_start,
                                Ok(Response::new(WalletGetBalanceResponse {
                                    result: Some(wallet_get_balance_response::Result::Error(err)),
                                })),
                            );
                        }
                    }
                } else {
                    None
                };

                let first_name: v1::Hash = match v1::Hash::from_base58(first_name_str.hash.as_str())
                {
                    Ok(hash) => hash,
                    Err(_) => {
                        self.metrics
                            .balance_request_error_invalid_request_invalid_first_name
                            .increment();
                        let err = self.build_error_response::<ErrorStatus>(
                            NockAppGrpcError::InvalidRequest("invalid first name hash".into()),
                        );
                        return timed_return(
                            &metrics.send_tx_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        );
                    }
                };

                if let Some(ref cur) = cursor {
                    if cur.key.first_name != first_name {
                        self.metrics
                            .balance_request_error_invalid_request_token_mismatch
                            .increment();
                        let err = self.build_error_response::<ErrorStatus>(
                            NockAppGrpcError::InvalidRequest(
                                "Page token does not match requested first name".into(),
                            ),
                        );
                        return timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        );
                    }
                }

                let mut cached: Option<Arc<CachedBalanceEntryFirstName>> = None;

                if let Some(ref cursor) = cursor {
                    cached = self.cache_by_first_name.get(cursor.key())
                } else {
                    match self.cached_heaviest_chain().await {
                        Some((height, block_id)) => {
                            let cache_key = PageKeyFirstName::new(
                                first_name.clone(),
                                height.0 .0,
                                block_id.clone(),
                            );
                            cached = self.cache_by_first_name.get(&cache_key);
                        }
                        None => {
                            warn!("Cache missed for heaviest chain, this should never happen except with a fresh nockchain node.");
                            self.metrics.heaviest_chain_cache_miss.increment();
                        }
                    }
                }

                let _inflight = if cached.is_none() && cursor.is_none() {
                    let gate = self
                        .balance_inflight
                        .gate(format!("first-name:{}", first_name_str.hash));
                    let guard = gate.lock_owned().await;
                    if self.cached_heaviest_chain().await.is_none() {
                        if let Err(err) = self.refresh_heaviest_chain().await {
                            warn!("Could not refresh heaviest tip for first-name cache: {err}");
                        }
                    }
                    if let Some((height, block_id)) = self.cached_heaviest_chain().await {
                        cached = self.cache_by_first_name.get(&PageKeyFirstName::new(
                            first_name.clone(),
                            height.0 .0,
                            block_id,
                        ));
                    }
                    Some(guard)
                } else {
                    None
                };

                if let Some(cached) = cached {
                    tracing::debug!("Cache hit for first name: {:?}", first_name);
                    self.metrics.balance_cache_first_name_hit.increment();
                    match cached.build_paginated_response_first_name(
                        cursor.clone(),
                        client_page_items_limit,
                        max_bytes,
                        &self.metrics,
                    ) {
                        Ok(response) => {
                            return timed_return(
                                &metrics.balance_update_success_hit,
                                request_start,
                                Ok(Response::new(response)),
                            )
                        }
                        Err(err) => {
                            return timed_return(
                                &metrics.balance_update_error,
                                request_start,
                                Ok(Response::new(WalletGetBalanceResponse {
                                    result: Some(wallet_get_balance_response::Result::Error(err)),
                                })),
                            );
                        }
                    }
                }

                self.metrics.balance_cache_first_name_miss.increment();
                info!(
                    "peek path=balance-by-first-name first_name={} client_ip={:?} x_forwarded_for={:?}",
                    first_name_str.hash, remote_addr, x_forwarded_for
                );
                let path = vec!["balance-by-first-name".to_string(), first_name_str.hash];
                let mut path_slab = NounSlab::new();
                let path_noun = path.to_noun(&mut path_slab);
                path_slab.set_root(path_noun);
                let peek_start = Instant::now();
                let peek_result = self.handle.peek(path_slab).await;
                self.metrics
                    .balance_update_peek_time
                    .add_timing(&peek_start.elapsed());

                match peek_result {
                    Ok(Some(result_slab)) => {
                        let result_noun = unsafe { result_slab.root() };
                        let space = result_slab.noun_space();
                        let result =
                            <Option<Option<v1::BalanceUpdate>>>::from_noun(&result_noun, &space);

                        match result {
                            Ok(update) => {
                                let update = match update {
                                    // Peek result is double wrapped unit over the balance update
                                    Some(Some(update)) => update,
                                    Some(None) | None => {
                                        self.metrics.balance_request_error_peek_failed.increment();
                                        let err = self.build_error_response::<ErrorStatus>(
                                            NockAppGrpcError::PeekFailed,
                                        );
                                        return timed_return(
                                            &metrics.balance_update_error,
                                            request_start,
                                            Ok(Response::new(WalletGetBalanceResponse {
                                                result: Some(
                                                    wallet_get_balance_response::Result::Error(err),
                                                ),
                                            })),
                                        );
                                    }
                                };
                                let entry = self.cache_by_first_name.insert(&first_name, update);

                                match entry.build_paginated_response_first_name(
                                    cursor.clone(),
                                    client_page_items_limit,
                                    max_bytes,
                                    &self.metrics,
                                ) {
                                    Ok(response) => {
                                        return timed_return(
                                            &metrics.balance_update_success_miss,
                                            request_start,
                                            Ok(Response::new(response)),
                                        )
                                    }
                                    Err(err) => {
                                        return timed_return(
                                            &metrics.balance_update_error,
                                            request_start,
                                            Ok(Response::new(WalletGetBalanceResponse {
                                                result: Some(
                                                    wallet_get_balance_response::Result::Error(err),
                                                ),
                                            })),
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                self.metrics.balance_request_error_decode.increment();
                                let err = self.build_error_response::<ErrorStatus>(
                                    NockAppGrpcError::NounDecode(e),
                                );
                                return timed_return(
                                    &metrics.balance_update_error,
                                    request_start,
                                    Ok(Response::new(WalletGetBalanceResponse {
                                        result: Some(wallet_get_balance_response::Result::Error(
                                            err,
                                        )),
                                    })),
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        self.metrics.balance_request_error_peek_failed.increment();
                        let err =
                            self.build_error_response::<ErrorStatus>(NockAppGrpcError::PeekFailed);
                        timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        )
                    }
                    Err(e) => {
                        self.metrics.balance_request_error_nockapp.increment();
                        let err =
                            self.build_error_response::<ErrorStatus>(NockAppGrpcError::NockApp(e));
                        timed_return(
                            &metrics.balance_update_error,
                            request_start,
                            Ok(Response::new(WalletGetBalanceResponse {
                                result: Some(wallet_get_balance_response::Result::Error(err)),
                            })),
                        )
                    }
                }
            }
        }
    }

    async fn wallet_send_transaction(
        &self,
        request: Request<WalletSendTransactionRequest>,
    ) -> std::result::Result<Response<WalletSendTransactionResponse>, Status> {
        let remote_addr = request.remote_addr();
        let x_forwarded_for = request
            .metadata()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let req = request.into_inner();
        let request_start = Instant::now();
        let metrics = &self.metrics;
        debug!(
            "WalletSendTransaction tx_id={:?} client_ip={:?} x_forwarded_for={:?}",
            req.tx_id, remote_addr, x_forwarded_for
        );
        let tx_id_pb = match req.tx_id.clone() {
            Some(id) => id,
            None => {
                self.metrics
                    .send_tx_error_invalid_request_tx_id_missing
                    .increment();
                let err = self.build_error_response::<ErrorStatus>(
                    NockAppGrpcError::InvalidRequest("tx_id is required".into()),
                );
                return timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                );
            }
        };

        let raw_tx_pb = match req.raw_tx.clone() {
            Some(raw) => raw,
            None => {
                self.metrics
                    .send_tx_error_invalid_request_raw_tx_missing
                    .increment();
                let err = self.build_error_response::<ErrorStatus>(
                    NockAppGrpcError::InvalidRequest("raw_tx is required".into()),
                );
                return timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                );
            }
        };

        let tx_id_domain: v0::Hash = match tx_id_pb.clone().try_into() {
            Ok(id) => id,
            Err(_) => {
                self.metrics
                    .send_tx_error_invalid_request_tx_id_invalid
                    .increment();
                let err = self.build_error_response::<ErrorStatus>(
                    NockAppGrpcError::InvalidRequest("invalid tx_id".into()),
                );
                return timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                );
            }
        };

        let raw_tx: v1::RawTx = match raw_tx_pb.clone().try_into() {
            Ok(tx) => tx,
            Err(e) => {
                self.metrics
                    .send_tx_error_invalid_request_raw_tx_invalid
                    .increment();
                let err = self.build_error_response::<ErrorStatus>(
                    NockAppGrpcError::InvalidRequest(format!("invalid raw_tx: {}", e)),
                );
                return timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                );
            }
        };

        if raw_tx.id != tx_id_domain {
            self.metrics
                .send_tx_error_invalid_request_tx_id_mismatch
                .increment();
            let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::InvalidRequest(
                "tx_id does not match raw_tx.id".to_string(),
            ));
            return timed_return(
                &metrics.send_tx_error,
                request_start,
                Ok(Response::new(WalletSendTransactionResponse {
                    result: Some(wallet_send_transaction_response::Result::Error(err)),
                })),
            );
        }

        let mut payload_slab = NounSlab::new();
        let fact = nockapp::utils::make_tas(&mut payload_slab, "fact").as_noun();
        let heard_tx = nockapp::utils::make_tas(&mut payload_slab, "heard-tx").as_noun();
        let zero = nockvm::noun::D(0);
        let raw_noun = raw_tx.to_noun(&mut payload_slab);
        let heard_cell = nockvm::noun::T(&mut payload_slab, &[heard_tx, raw_noun]);
        let cause = nockvm::noun::T(&mut payload_slab, &[fact, zero, heard_cell]);
        payload_slab.set_root(cause);

        let wire = match grpc_wire_to_nockapp(&create_grpc_wire()) {
            Ok(w) => w,
            Err(e) => {
                let err = self.build_error_response::<ErrorStatus>(e);
                self.metrics.send_tx_error_internal.increment();
                return timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                );
            }
        };

        let observer = transaction_poke_observer(tx_id_domain);
        let started_poke = Instant::now();
        let poke_result = self.handle.poke(wire, payload_slab, observer).await;
        metrics
            .send_tx_poke_time
            .add_timing(&started_poke.elapsed());
        match poke_result {
            Ok(nockapp::driver::PokeResult::Ack) => timed_return(
                &metrics.send_tx_success,
                request_start,
                Ok(Response::new(WalletSendTransactionResponse {
                    result: Some(wallet_send_transaction_response::Result::Ack(
                        Acknowledged {},
                    )),
                })),
            ),
            Ok(nockapp::driver::PokeResult::Nack) => {
                self.metrics.send_tx_error_poke_failed.increment();
                let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::PokeFailed);
                timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                )
            }
            Err(e) => {
                self.metrics.send_tx_error_nockapp.increment();
                let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::NockApp(e));
                timed_return(
                    &metrics.send_tx_error,
                    request_start,
                    Ok(Response::new(WalletSendTransactionResponse {
                        result: Some(wallet_send_transaction_response::Result::Error(err)),
                    })),
                )
            }
        }
    }

    async fn transaction_accepted(
        &self,
        request: Request<TransactionAcceptedRequest>,
    ) -> std::result::Result<Response<TransactionAcceptedResponse>, Status> {
        let remote_addr = request.remote_addr();
        let x_forwarded_for = request
            .metadata()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let req = request.into_inner();
        let request_start = Instant::now();
        let metrics = &self.metrics;
        debug!(
            "TransactionAccepted tx_id={:?} client_ip={:?} x_forwarded_for={:?}",
            req.tx_id, remote_addr, x_forwarded_for
        );

        let Some(pb_hash) = req.tx_id else {
            self.metrics
                .tx_accepted_error_invalid_request_missing_tx_id
                .increment();
            let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::InvalidRequest(
                "tx_id is required".into(),
            ));
            return timed_return(
                &metrics.tx_accepted_error,
                request_start,
                Ok(Response::new(TransactionAcceptedResponse {
                    result: Some(transaction_accepted_response::Result::Error(err)),
                })),
            );
        };

        let tx_id: String = pb_hash.hash.into();
        if tx_id.is_empty() {
            self.metrics
                .tx_accepted_error_invalid_request_empty_tx_id
                .increment();
            let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::InvalidRequest(
                "tx_id is required".into(),
            ));
            return timed_return(
                &metrics.tx_accepted_error,
                request_start,
                Ok(Response::new(TransactionAcceptedResponse {
                    result: Some(transaction_accepted_response::Result::Error(err)),
                })),
            );
        }

        let mut path_slab = NounSlab::new();
        let tag = nockapp::utils::make_tas(&mut path_slab, "tx-accepted").as_noun();
        let tx_id_noun: nockvm::noun::Noun = tx_id.to_noun(&mut path_slab);
        let path_noun = nockvm::noun::T(&mut path_slab, &[tag, tx_id_noun, SIG]);
        path_slab.set_root(path_noun);

        let start_peek = Instant::now();
        let peek_result = self.handle.peek(path_slab).await;
        metrics
            .tx_accepted_peek_time
            .add_timing(&start_peek.elapsed());
        match peek_result {
            Ok(Some(result_slab)) => {
                let result_noun = unsafe { result_slab.root() };
                let space = result_slab.noun_space();
                match <Option<Option<bool>>>::from_noun(&result_noun, &space) {
                    Ok(opt) => {
                        let accepted = opt.flatten().unwrap_or(false);
                        timed_return(
                            &metrics.tx_accepted_success,
                            request_start,
                            Ok(Response::new(TransactionAcceptedResponse {
                                result: Some(transaction_accepted_response::Result::Accepted(
                                    accepted,
                                )),
                            })),
                        )
                    }
                    Err(e) => {
                        self.metrics.tx_accepted_error_decode.increment();
                        let err = self
                            .build_error_response::<ErrorStatus>(NockAppGrpcError::NounDecode(e));
                        timed_return(
                            &metrics.tx_accepted_error,
                            request_start,
                            Ok(Response::new(TransactionAcceptedResponse {
                                result: Some(transaction_accepted_response::Result::Error(err)),
                            })),
                        )
                    }
                }
            }
            Ok(None) => {
                self.metrics.tx_accepted_error_peek_failed.increment();
                let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::PeekFailed);
                timed_return(
                    &metrics.tx_accepted_error,
                    request_start,
                    Ok(Response::new(TransactionAcceptedResponse {
                        result: Some(transaction_accepted_response::Result::Error(err)),
                    })),
                )
            }
            Err(e) => {
                self.metrics.tx_accepted_error_nockapp.increment();
                let err = self.build_error_response::<ErrorStatus>(NockAppGrpcError::NockApp(e));
                timed_return(
                    &metrics.tx_accepted_error,
                    request_start,
                    Ok(Response::new(TransactionAcceptedResponse {
                        result: Some(transaction_accepted_response::Result::Error(err)),
                    })),
                )
            }
        }
    }
}

#[tonic::async_trait]
impl NockchainBlockService for NockchainBlockServer {
    #[tracing::instrument(
        name = "grpc.block_explorer.get_blocks",
        skip(self, request),
        fields(
            page_token_len = tracing::field::Empty,
            limit = tracing::field::Empty,
            cursor = tracing::field::Empty
        )
    )]
    async fn get_blocks(
        &self,
        request: Request<GetBlocksRequest>,
    ) -> std::result::Result<Response<GetBlocksResponse>, Status> {
        let span = tracing::Span::current();
        let req = request.into_inner();
        let metrics = &self.metrics;
        let request_start = Instant::now();

        // Parse pagination parameters
        let page_req = match req.page {
            Some(page) => page,
            None => {
                metrics
                    .block_explorer_get_blocks_error_invalid_request
                    .increment();
                let elapsed = request_start.elapsed();
                self.block_explorer_cache.record_get_blocks_latency(elapsed);
                metrics.block_explorer_get_blocks_error.add_timing(&elapsed);
                return Err(Status::invalid_argument("page is required"));
            }
        };
        span.record(
            "page_token_len",
            &tracing::field::display(page_req.page_token.len()),
        );

        let limit = if page_req.client_page_items_limit == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            std::cmp::min(page_req.client_page_items_limit as usize, MAX_PAGE_SIZE)
        };
        span.record("limit", &tracing::field::display(limit));

        // Decode cursor (height as u64) from page token
        let cursor = if page_req.page_token.is_empty() {
            None
        } else {
            // Parse hex-encoded u64
            match u64::from_str_radix(&page_req.page_token, 16) {
                Ok(height) => Some(height),
                Err(_) => {
                    metrics
                        .block_explorer_get_blocks_error_invalid_request
                        .increment();
                    let elapsed = request_start.elapsed();
                    self.block_explorer_cache.record_get_blocks_latency(elapsed);
                    metrics.block_explorer_get_blocks_error.add_timing(&elapsed);
                    return Err(Status::invalid_argument("invalid page token"));
                }
            }
        };
        let cursor_repr = cursor
            .map(|height| height.to_string())
            .unwrap_or_else(|| "tip".into());
        span.record("cursor", &tracing::field::display(cursor_repr));

        info!(
            limit,
            cursor = cursor.unwrap_or_default(),
            "Serving GetBlocks request"
        );

        // Page entries and current_height share one authoritative cache snapshot.
        let (blocks, next_cursor, current_height) = match self
            .block_explorer_cache
            .get_blocks_page(cursor, limit)
            .await
        {
            Ok(page) => page,
            Err(err) => {
                let elapsed = request_start.elapsed();
                self.block_explorer_cache.record_get_blocks_latency(elapsed);
                metrics.block_explorer_get_blocks_error.add_timing(&elapsed);
                return Err(Status::from(err));
            }
        };

        // Convert to proto
        use crate::pb::common::v1 as pb_common;
        let block_entries: Vec<BlockEntry> = blocks
            .into_iter()
            .map(|b| BlockEntry {
                block_id: Some(pb_common::Hash {
                    belt_1: Some(pb_common::Belt {
                        value: b.block_id.0[0].0,
                    }),
                    belt_2: Some(pb_common::Belt {
                        value: b.block_id.0[1].0,
                    }),
                    belt_3: Some(pb_common::Belt {
                        value: b.block_id.0[2].0,
                    }),
                    belt_4: Some(pb_common::Belt {
                        value: b.block_id.0[3].0,
                    }),
                    belt_5: Some(pb_common::Belt {
                        value: b.block_id.0[4].0,
                    }),
                }),
                height: b.height,
                parent: Some(pb_common::Hash {
                    belt_1: Some(pb_common::Belt {
                        value: b.parent_id.0[0].0,
                    }),
                    belt_2: Some(pb_common::Belt {
                        value: b.parent_id.0[1].0,
                    }),
                    belt_3: Some(pb_common::Belt {
                        value: b.parent_id.0[2].0,
                    }),
                    belt_4: Some(pb_common::Belt {
                        value: b.parent_id.0[3].0,
                    }),
                    belt_5: Some(pb_common::Belt {
                        value: b.parent_id.0[4].0,
                    }),
                }),
                timestamp: b.timestamp,
                tx_ids: b
                    .tx_ids
                    .iter()
                    .map(|tx_id| pb_common::Base58Hash {
                        hash: tx_id.to_base58(),
                    })
                    .collect(),
            })
            .collect();

        // Encode next cursor as hex
        let next_page_token = next_cursor.map(|h| format!("{:x}", h)).unwrap_or_default();

        let response = BlocksData {
            blocks: block_entries,
            current_height,
            page: Some(pb_common::PageResponse { next_page_token }),
        };

        info!(
            returned = response.blocks.len(),
            height = response.current_height,
            "Responding to GetBlocks request"
        );

        let elapsed = request_start.elapsed();
        self.block_explorer_cache.record_get_blocks_latency(elapsed);
        metrics
            .block_explorer_get_blocks_success
            .add_timing(&elapsed);
        Ok(Response::new(GetBlocksResponse {
            result: Some(get_blocks_response::Result::Blocks(response)),
        }))
    }

    #[tracing::instrument(
        name = "grpc.block_explorer.get_block_details",
        skip(self, request),
        fields(
            height = tracing::field::Empty,
            block_id = tracing::field::Empty
        )
    )]
    async fn get_block_details(
        &self,
        request: Request<GetBlockDetailsRequest>,
    ) -> std::result::Result<Response<GetBlockDetailsResponse>, Status> {
        let span = tracing::Span::current();
        let req = request.into_inner();
        let metrics = &self.metrics;
        let request_start = Instant::now();

        let result = match req.selector {
            Some(get_block_details_request::Selector::Height(height)) => {
                span.record("height", &tracing::field::display(height));
                info!(height, "Serving GetBlockDetails by height");
                self.block_explorer_cache
                    .load_full_page_by_height(&self.handle, height)
                    .await
            }
            Some(get_block_details_request::Selector::BlockId(ref block_id_b58)) => {
                span.record("block_id", &tracing::field::display(&block_id_b58.hash));
                info!(
                    block_id = block_id_b58.hash.as_str(),
                    "Serving GetBlockDetails by block_id"
                );
                self.block_explorer_cache
                    .load_full_page_by_id(&self.handle, &block_id_b58.hash)
                    .await
            }
            None => {
                metrics
                    .block_explorer_get_block_details_invalid_request
                    .increment();
                metrics
                    .block_explorer_get_block_details_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::invalid_argument(
                    "selector is required (height or block_id)",
                ));
            }
        };

        match result {
            Ok(details) => {
                info!(
                    height = details.height,
                    block_id = details.block_id.to_base58().as_str(),
                    tx_count = details.tx_ids.len(),
                    "Responding to GetBlockDetails request"
                );
                let elapsed = request_start.elapsed();
                self.block_explorer_cache
                    .record_get_block_details_latency(elapsed);
                metrics
                    .block_explorer_get_block_details_success
                    .add_timing(&elapsed);
                Ok(Response::new(GetBlockDetailsResponse {
                    result: Some(get_block_details_response::Result::Details(
                        details.to_proto(),
                    )),
                }))
            }
            Err(NockAppGrpcError::NotFound) => {
                metrics
                    .block_explorer_get_block_details_not_found
                    .increment();
                let elapsed = request_start.elapsed();
                self.block_explorer_cache
                    .record_get_block_details_latency(elapsed);
                metrics
                    .block_explorer_get_block_details_error
                    .add_timing(&elapsed);
                Err(Status::not_found("Block not found"))
            }
            Err(NockAppGrpcError::InvalidRequest(msg)) => {
                metrics
                    .block_explorer_get_block_details_invalid_request
                    .increment();
                let elapsed = request_start.elapsed();
                self.block_explorer_cache
                    .record_get_block_details_latency(elapsed);
                metrics
                    .block_explorer_get_block_details_error
                    .add_timing(&elapsed);
                Err(Status::invalid_argument(msg))
            }
            Err(err) => {
                let elapsed = request_start.elapsed();
                self.block_explorer_cache
                    .record_get_block_details_latency(elapsed);
                metrics
                    .block_explorer_get_block_details_error
                    .add_timing(&elapsed);
                Err(match err {
                    NockAppGrpcError::Status(status) => status,
                    err => Status::internal(err.to_string()),
                })
            }
        }
    }

    #[tracing::instrument(
        name = "grpc.block_explorer.get_transaction_block",
        skip(self, request),
        fields(tx_id = tracing::field::Empty)
    )]
    async fn get_transaction_block(
        &self,
        request: Request<GetTransactionBlockRequest>,
    ) -> std::result::Result<Response<GetTransactionBlockResponse>, Status> {
        let span = tracing::Span::current();
        let req = request.into_inner();
        let metrics = &self.metrics;
        let request_start = Instant::now();

        let tx_id_b58 = match req.tx_id {
            Some(id) => id.hash,
            None => {
                metrics
                    .block_explorer_get_transaction_block_invalid_request
                    .increment();
                metrics
                    .block_explorer_get_transaction_block_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::invalid_argument("tx_id is required"));
            }
        };
        span.record("tx_id", &tracing::field::display(tx_id_b58.as_str()));

        info!(
            tx_id = tx_id_b58.as_str(),
            "Serving GetTransactionBlock request"
        );

        let tx_id = match self.block_explorer_cache.resolve_tx_id(&tx_id_b58).await {
            Ok(hash) => hash,
            Err(NockAppGrpcError::Status(status)) => {
                metrics
                    .block_explorer_get_transaction_block_error
                    .add_timing(&request_start.elapsed());
                return Err(status);
            }
            Err(err) => {
                metrics
                    .block_explorer_get_transaction_block_invalid_request
                    .increment();
                metrics
                    .block_explorer_get_transaction_block_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::invalid_argument(err.to_string()));
            }
        };

        let block = match self
            .block_explorer_cache
            .lookup_transaction_block(&self.handle, &tx_id)
            .await
        {
            Ok(block) => Some(block),
            Err(NockAppGrpcError::TxPending) => None,
            Err(err) => {
                if matches!(&err, NockAppGrpcError::NotFound) {
                    metrics
                        .block_explorer_get_transaction_block_not_found
                        .increment();
                }
                metrics
                    .block_explorer_get_transaction_block_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::from(err));
            }
        };
        if let Some(block) = block {
            use crate::pb::common::v1 as pb_common;
            return timed_return(
                &metrics.block_explorer_get_transaction_block_success,
                request_start,
                Ok(Response::new(GetTransactionBlockResponse {
                    result: Some(get_transaction_block_response::Result::Block(
                        TransactionBlockData {
                            block_id: Some(pb_common::Hash {
                                belt_1: Some(pb_common::Belt {
                                    value: block.block_id.0[0].0,
                                }),
                                belt_2: Some(pb_common::Belt {
                                    value: block.block_id.0[1].0,
                                }),
                                belt_3: Some(pb_common::Belt {
                                    value: block.block_id.0[2].0,
                                }),
                                belt_4: Some(pb_common::Belt {
                                    value: block.block_id.0[3].0,
                                }),
                                belt_5: Some(pb_common::Belt {
                                    value: block.block_id.0[4].0,
                                }),
                            }),
                            height: block.height,
                            parent: Some(pb_common::Hash {
                                belt_1: Some(pb_common::Belt {
                                    value: block.parent_id.0[0].0,
                                }),
                                belt_2: Some(pb_common::Belt {
                                    value: block.parent_id.0[1].0,
                                }),
                                belt_3: Some(pb_common::Belt {
                                    value: block.parent_id.0[2].0,
                                }),
                                belt_4: Some(pb_common::Belt {
                                    value: block.parent_id.0[3].0,
                                }),
                                belt_5: Some(pb_common::Belt {
                                    value: block.parent_id.0[4].0,
                                }),
                            }),
                            timestamp: block.timestamp,
                        },
                    )),
                })),
            );
        }

        metrics
            .block_explorer_get_transaction_block_pending
            .increment();
        timed_return(
            &metrics.block_explorer_get_transaction_block_success,
            request_start,
            Ok(Response::new(GetTransactionBlockResponse {
                result: Some(get_transaction_block_response::Result::Pending(
                    TransactionPending {},
                )),
            })),
        )
    }

    #[tracing::instrument(
        name = "grpc.block_explorer.get_transaction_details",
        skip(self, request),
        fields(tx_id = tracing::field::Empty)
    )]
    async fn get_transaction_details(
        &self,
        request: Request<GetTransactionDetailsRequest>,
    ) -> std::result::Result<Response<GetTransactionDetailsResponse>, Status> {
        let span = tracing::Span::current();
        let req = request.into_inner();
        let metrics = &self.metrics;
        let request_start = Instant::now();

        let tx_id_b58 = match req.tx_id {
            Some(id) => id.hash,
            None => {
                metrics
                    .block_explorer_get_transaction_details_invalid_request
                    .increment();
                metrics
                    .block_explorer_get_transaction_details_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::invalid_argument("tx_id is required"));
            }
        };
        span.record("tx_id", &tracing::field::display(tx_id_b58.as_str()));

        info!(
            tx_id = tx_id_b58.as_str(),
            "Serving GetTransactionDetails request"
        );

        let tx_hash = match self.block_explorer_cache.resolve_tx_id(&tx_id_b58).await {
            Ok(hash) => hash,
            Err(NockAppGrpcError::Status(status)) => {
                metrics
                    .block_explorer_get_transaction_details_error
                    .add_timing(&request_start.elapsed());
                return Err(status);
            }
            Err(err) => {
                metrics
                    .block_explorer_get_transaction_details_invalid_request
                    .increment();
                metrics
                    .block_explorer_get_transaction_details_error
                    .add_timing(&request_start.elapsed());
                return Err(Status::invalid_argument(err.to_string()));
            }
        };

        match self
            .block_explorer_cache
            .load_transaction_details(&self.handle, &tx_hash)
            .await
        {
            Ok(details) => timed_return(
                &metrics.block_explorer_get_transaction_details_success,
                request_start,
                Ok(Response::new(GetTransactionDetailsResponse {
                    result: Some(get_transaction_details_response::Result::Details(details)),
                })),
            ),
            Err(NockAppGrpcError::TxPending) => {
                metrics
                    .block_explorer_get_transaction_details_pending
                    .increment();
                timed_return(
                    &metrics.block_explorer_get_transaction_details_success,
                    request_start,
                    Ok(Response::new(GetTransactionDetailsResponse {
                        result: Some(get_transaction_details_response::Result::Pending(
                            TransactionPending {},
                        )),
                    })),
                )
            }
            Err(NockAppGrpcError::NotFound) => {
                metrics
                    .block_explorer_get_transaction_details_not_found
                    .increment();
                metrics
                    .block_explorer_get_transaction_details_error
                    .add_timing(&request_start.elapsed());
                Err(Status::not_found("Transaction not found"))
            }
            Err(err) => {
                metrics
                    .block_explorer_get_transaction_details_error
                    .add_timing(&request_start.elapsed());
                Err(Status::from(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use nockapp_grpc_proto::pb::common::v1::Base58Hash;
    use nockchain_libp2p_io::peer_stats::{
        PeerReqResGeneration as TransportPeerReqResGeneration,
        PeerStatsEntry as TransportPeerStatsEntry, PeerStatsRegistry as TransportPeerStatsRegistry,
        PeerStatsSnapshot as TransportPeerStatsSnapshot,
    };
    use nockchain_math::crypto::cheetah::A_GEN;
    use nockchain_types::v1::Hash;
    use tracing_subscriber::prelude::*;

    use super::*;
    use crate::pb::common::v1 as pb_common_v1;
    use crate::public_nockchain::v1::fixtures as fixtures_v1;
    use crate::public_nockchain::v2::block_explorer::tests::ReorgHandle;
    use crate::public_nockchain::v2::fixtures;
    use crate::v2::pagination::cmp_name;

    async fn wait_for_explorer_block(
        cache: &BlockExplorerCache,
        height: u64,
        branch: u64,
        completed_backfills: u64,
    ) {
        for _ in 0..100 {
            let page = cache.get_blocks_page(Some(height), 1).await;
            let snapshot = cache.metrics_snapshot().await;
            if snapshot.seed_ready
                && snapshot.backfill_success_count >= completed_backfills
                && page.as_ref().is_ok_and(|(blocks, _, _)| {
                    blocks.first().is_some_and(|block| {
                        block.height == height
                            && block.block_id == ReorgHandle::block_id(branch, height)
                    })
                })
            {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("refresh worker did not restore branch {branch} block {height}");
    }

    #[tokio::test(start_paused = true)]
    async fn explorer_refresh_tick_retries_retained_reconciliation_before_resuming_backfill() {
        let handle = Arc::new(ReorgHandle::new(255));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        let (reporter, _health) = tonic_health::server::health_reporter();
        let worker = server.start_block_explorer_refresh(reporter);
        wait_for_explorer_block(&server.block_explorer_cache, 255, 0, 0).await;
        assert_eq!(
            server
                .block_explorer_cache
                .pending_backfill()
                .await
                .unwrap()
                .1,
            127
        );
        handle.fork_at(240);
        handle.fail_range_once(128, 255);
        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        for _ in 0..100 {
            if handle.ranges().len() >= 2
                && !server
                    .block_explorer_cache
                    .metrics_snapshot()
                    .await
                    .seed_ready
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(handle.ranges(), vec![(128, 255), (128, 255)]);
        let stale = server.block_explorer_cache.metrics_snapshot().await;
        assert!(!stale.seed_ready);
        assert_eq!(stale.cache_height, 255);
        assert_eq!(stale.cache_lowest_height, 128);
        assert_eq!(stale.backfill_resume_height, Some(127));
        assert_eq!(stale.backfill_success_count, 0);
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(handle.ranges(), vec![(128, 255), (128, 255)]);
        assert!(matches!(
            server.block_explorer_cache.get_blocks_page(None, 1).await,
            Err(NockAppGrpcError::Status(status)) if status.code() == tonic::Code::Unavailable
        ));

        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        wait_for_explorer_block(&server.block_explorer_cache, 100, 0, 1).await;
        assert_eq!(
            handle.ranges(),
            vec![(128, 255), (128, 255), (128, 255), (0, 127)],
        );
        let (page, _, current_height) = server
            .block_explorer_cache
            .get_blocks_page(None, 1)
            .await
            .unwrap();
        assert_eq!(current_height, 255);
        assert_eq!(
            page.first().unwrap().block_id,
            ReorgHandle::block_id(1, 255)
        );
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());
        assert_eq!(
            server
                .block_explorer_cache
                .metrics_snapshot()
                .await
                .backfill_success_count,
            1
        );
        worker.abort();
    }
    #[tokio::test(start_paused = true)]
    async fn explorer_refresh_worker_restarts_completed_backfill_after_reorg_at_255() {
        let handle = Arc::new(ReorgHandle::new(255));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        let (reporter, _health) = tonic_health::server::health_reporter();
        let worker = server.start_block_explorer_refresh(reporter);
        wait_for_explorer_block(&server.block_explorer_cache, 255, 0, 0).await;
        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        wait_for_explorer_block(&server.block_explorer_cache, 100, 0, 1).await;
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());

        handle.set_tip(255, 1);
        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        wait_for_explorer_block(&server.block_explorer_cache, 100, 1, 2).await;
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());
        assert!(server
            .block_explorer_cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        assert_eq!(
            server
                .block_explorer_cache
                .metrics_snapshot()
                .await
                .backfill_success_count,
            2
        );
        for _ in 0..3 {
            time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(
            handle
                .ranges()
                .iter()
                .filter(|range| **range == (0, 127))
                .count(),
            2
        );
        worker.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn explorer_refresh_worker_restarts_in_flight_backfill_after_generation_change() {
        let handle = Arc::new(ReorgHandle::new(255));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        let (reporter, _health) = tonic_health::server::health_reporter();
        let worker = server.start_block_explorer_refresh(reporter);
        wait_for_explorer_block(&server.block_explorer_cache, 255, 0, 0).await;
        handle.pause_range(0, 127);
        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        time::timeout(Duration::from_secs(1), handle.wait_for_range())
            .await
            .expect("old backfill should start");

        handle.set_tip(255, 1);
        time::advance(DEFAULT_BLOCK_EXPLORER_REFRESH_INTERVAL).await;
        // Old storage survives the replacement wait, but known-divergent
        // canonical reads are unavailable before that range permit is released.
        for _ in 0..100 {
            if !server
                .block_explorer_cache
                .metrics_snapshot()
                .await
                .seed_ready
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let stale = server.block_explorer_cache.get_blocks_page(None, 1).await;
        assert!(matches!(
            stale,
            Err(NockAppGrpcError::Status(status)) if status.code() == tonic::Code::Unavailable
        ));
        handle.release_range();
        wait_for_explorer_block(&server.block_explorer_cache, 100, 1, 1).await;
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());
        assert!(server
            .block_explorer_cache
            .get_block_for_tx(&ReorgHandle::orphan_tx())
            .await
            .is_none());
        assert!(
            server
                .block_explorer_cache
                .metrics_snapshot()
                .await
                .backfill_success_count
                >= 1
        );
        assert_eq!(
            handle
                .ranges()
                .iter()
                .filter(|range| **range == (0, 127))
                .count(),
            2
        );
        worker.abort();
    }

    #[tokio::test]
    async fn get_transaction_block_drops_orphan_after_tip_check_suffix_reorg_and_repeated_refreshes(
    ) {
        let handle = Arc::new(ReorgHandle::new(200));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        server
            .block_explorer_cache
            .clone()
            .initialize(handle.clone())
            .await
            .unwrap();
        let block_server = NockchainBlockServer::new(
            handle.clone(),
            server.block_explorer_cache.clone(),
            server.metrics.clone(),
        );
        let request = GetTransactionBlockRequest {
            tx_id: Some(Base58Hash {
                hash: ReorgHandle::orphan_tx().to_base58(),
            }),
        };
        let confirmed = block_server
            .get_transaction_block(Request::new(request.clone()))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            confirmed.result,
            Some(get_transaction_block_response::Result::Block(
                TransactionBlockData { height: 100, .. }
            )),
        ));

        handle.set_tip(202, 0);
        handle.switch_after_ranges(1, 1);
        for _ in 0..3 {
            server
                .block_explorer_cache
                .refresh(&server.handle)
                .await
                .unwrap();
            let err = block_server
                .get_transaction_block(Request::new(request.clone()))
                .await
                .unwrap_err();
            assert_eq!(err.code(), tonic::Code::Unavailable);
        }
    }

    async fn explorer_endpoint_fixture(
        handle: Arc<ReorgHandle>,
        complete: bool,
    ) -> (PublicNockchainGrpcServer, NockchainBlockServer) {
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        server
            .block_explorer_cache
            .clone()
            .initialize(handle.clone())
            .await
            .unwrap();
        if complete {
            if let Some((generation, upper)) = server.block_explorer_cache.pending_backfill().await
            {
                server
                    .block_explorer_cache
                    .clone()
                    .backfill_older(handle.clone(), generation, upper)
                    .await
                    .unwrap();
            }
        }
        let blocks = NockchainBlockServer::new(
            handle,
            server.block_explorer_cache.clone(),
            server.metrics.clone(),
        );
        (server, blocks)
    }

    async fn assert_transaction_confirmed_by_both(
        server: &NockchainBlockServer,
        tx: &Hash,
        height: u64,
    ) {
        let block = server
            .get_transaction_block(Request::new(GetTransactionBlockRequest {
                tx_id: Some(Base58Hash {
                    hash: tx.to_base58(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            block.result,
            Some(get_transaction_block_response::Result::Block(TransactionBlockData {
                height: actual, ..
            })) if actual == height
        ));
        let details = server
            .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: tx.to_base58(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            details.result,
            Some(get_transaction_details_response::Result::Details(details)) if details.height == height
        ));
    }
    async fn assert_retained_rpc_reads_unavailable(blocks: &NockchainBlockServer) {
        for token in ["", "fa", "ff", "0"] {
            let status = blocks
                .get_blocks(Request::new(GetBlocksRequest {
                    page: Some(pb_common_v1::PageRequest {
                        page_token: token.into(),
                        client_page_items_limit: 10,
                        ..Default::default()
                    }),
                }))
                .await
                .unwrap_err();
            assert_eq!(status.code(), tonic::Code::Unavailable);
        }
        for selector in [
            get_block_details_request::Selector::Height(250),
            get_block_details_request::Selector::Height(255),
            get_block_details_request::Selector::Height(0),
            get_block_details_request::Selector::BlockId(Base58Hash {
                hash: ReorgHandle::block_id(0, 250).to_base58(),
            }),
            get_block_details_request::Selector::BlockId(Base58Hash {
                hash: ReorgHandle::block_id(0, 255).to_base58(),
            }),
            get_block_details_request::Selector::BlockId(Base58Hash {
                hash: ReorgHandle::block_id(1, 250).to_base58(),
            }),
        ] {
            let status = blocks
                .get_block_details(Request::new(GetBlockDetailsRequest {
                    selector: Some(selector),
                }))
                .await
                .unwrap_err();
            assert_eq!(status.code(), tonic::Code::Unavailable);
        }
        let suffix = ReorgHandle::suffix_tx().to_base58();
        for spelling in [
            suffix.clone(),
            suffix[..8].to_string(),
            ReorgHandle::orphan_tx().to_base58(),
            ReorgHandle::pending_tx().to_base58(),
            "11111111".to_string(),
        ] {
            let block = blocks
                .get_transaction_block(Request::new(GetTransactionBlockRequest {
                    tx_id: Some(Base58Hash {
                        hash: spelling.clone(),
                    }),
                }))
                .await
                .unwrap_err();
            let detail = blocks
                .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                    tx_id: Some(Base58Hash { hash: spelling }),
                }))
                .await
                .unwrap_err();
            assert_eq!(block.code(), tonic::Code::Unavailable);
            assert_eq!(detail.code(), tonic::Code::Unavailable);
        }
    }

    #[tokio::test]
    async fn transient_reorg_quarantines_every_rpc_then_recovers_canonical_suffix_and_history() {
        for tip in [255, 254] {
            let handle = Arc::new(ReorgHandle::new(255));
            let (server, blocks) = explorer_endpoint_fixture(handle.clone(), false).await;
            assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::suffix_tx(), 250).await;
            assert_eq!(
                server
                    .block_explorer_cache
                    .pending_backfill()
                    .await
                    .unwrap()
                    .1,
                127
            );
            handle.fork_at(240);
            handle.set_tip(tip, 1);
            handle.fail_range_once(tip - 127, tip);
            assert!(server
                .block_explorer_cache
                .refresh(&server.handle)
                .await
                .is_err());
            assert_eq!(server.block_explorer_cache.get_max_height(), 255);
            let snapshot = server.block_explorer_cache.metrics_snapshot().await;
            assert!(!snapshot.seed_ready);
            assert_eq!(snapshot.cache_lowest_height, 128);
            assert_eq!(snapshot.backfill_resume_height, Some(127));
            assert!(server
                .block_explorer_cache
                .pending_backfill()
                .await
                .is_none());
            let calls = handle.ranges().len();
            assert_retained_rpc_reads_unavailable(&blocks).await;
            assert_eq!(handle.ranges().len(), calls);

            server
                .block_explorer_cache
                .refresh(&server.handle)
                .await
                .unwrap();
            let page = blocks
                .get_blocks(Request::new(GetBlocksRequest {
                    page: Some(pb_common_v1::PageRequest {
                        page_token: "ff".into(),
                        client_page_items_limit: 10,
                        ..Default::default()
                    }),
                }))
                .await
                .unwrap()
                .into_inner();
            let Some(get_blocks_response::Result::Blocks(page)) = page.result else {
                panic!("canonical block page must be returned")
            };
            assert_eq!(page.current_height, tip);
            assert_eq!(page.blocks.first().unwrap().height, tip);
            assert!(page
                .blocks
                .iter()
                .all(|block| block.height <= page.current_height));
            let (generation, upper) = server
                .block_explorer_cache
                .pending_backfill()
                .await
                .unwrap();
            assert_eq!(upper, if tip == 255 { 127 } else { 126 });
            let calls = handle.ranges().len();
            server
                .block_explorer_cache
                .clone()
                .backfill_older(handle.clone(), generation, upper)
                .await
                .unwrap();
            assert_eq!(&handle.ranges()[calls..], &[(0, upper)]);
            assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::orphan_tx(), 100).await;
            for status in [
                blocks
                    .get_transaction_block(Request::new(GetTransactionBlockRequest {
                        tx_id: Some(Base58Hash {
                            hash: ReorgHandle::suffix_tx().to_base58(),
                        }),
                    }))
                    .await
                    .unwrap_err(),
                blocks
                    .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                        tx_id: Some(Base58Hash {
                            hash: ReorgHandle::suffix_tx().to_base58(),
                        }),
                    }))
                    .await
                    .unwrap_err(),
            ] {
                assert_eq!(status.code(), tonic::Code::NotFound);
            }
            assert!(server
                .block_explorer_cache
                .pending_backfill()
                .await
                .is_none());
        }
    }

    #[tokio::test]
    async fn transaction_detail_success_and_error_in_flight_across_failed_replacement_are_unavailable(
    ) {
        for fail_old_read in [false, true] {
            let handle = Arc::new(ReorgHandle::new(255));
            let (server, blocks) = explorer_endpoint_fixture(handle.clone(), true).await;
            assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::suffix_tx(), 250).await;
            let blocks = Arc::new(blocks);
            let request = GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::suffix_tx().to_base58(),
                }),
            };
            handle.pause_block_transaction();
            if fail_old_read {
                handle.fail_block_transaction_once();
            }
            let lookup = tokio::spawn({
                let blocks = blocks.clone();
                let request = request.clone();
                async move { blocks.get_transaction_details(Request::new(request)).await }
            });
            time::timeout(Duration::from_secs(1), handle.wait_for_block_transaction())
                .await
                .expect("old transaction read should reach the barrier");
            handle.fork_at(240);
            handle.fail_range_once(128, 255);
            assert!(server
                .block_explorer_cache
                .refresh(&server.handle)
                .await
                .is_err());
            assert!(
                !server
                    .block_explorer_cache
                    .metrics_snapshot()
                    .await
                    .seed_ready
            );
            handle.release_block_transaction();
            assert_eq!(
                lookup.await.unwrap().unwrap_err().code(),
                tonic::Code::Unavailable
            );
            assert_eq!(
                blocks
                    .get_transaction_details(Request::new(request.clone()))
                    .await
                    .unwrap_err()
                    .code(),
                tonic::Code::Unavailable,
            );
            server
                .block_explorer_cache
                .refresh(&server.handle)
                .await
                .unwrap();
            assert_eq!(
                blocks
                    .get_transaction_details(Request::new(request))
                    .await
                    .unwrap_err()
                    .code(),
                tonic::Code::NotFound,
            );
            assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::orphan_tx(), 100).await;
        }
    }

    #[tokio::test]
    async fn rollback_255_to_254_keeps_height_100_confirmed_by_both_endpoints() {
        use futures::FutureExt;

        use crate::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;

        let handle = Arc::new(ReorgHandle::new(255));
        handle.retain_raw_transactions();
        let (server, blocks) = explorer_endpoint_fixture(handle.clone(), false).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let incoming = futures::stream::unfold(listener, |listener| async move {
            let connection = listener.accept().await.map(|(stream, _)| stream);
            Some((connection, listener))
        });
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        let transport = tokio::spawn(async move {
            Server::builder()
                .add_service(NockchainBlockServiceServer::new(blocks))
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = stopped.await;
                })
                .await
        });

        // Shut down and join the loopback server even if a consumer assertion
        // panics, so this regression never leaves a listener or task behind.
        let outcome = std::panic::AssertUnwindSafe(async {
            let mut client = NockchainBlockServiceClient::connect(format!("http://{address}"))
                .await
                .unwrap();
            let tx_id = Base58Hash {
                hash: ReorgHandle::orphan_tx().to_base58(),
            };
            let block = client
                .get_transaction_block(GetTransactionBlockRequest {
                    tx_id: Some(tx_id.clone()),
                })
                .await
                .unwrap_err();
            let details = client
                .get_transaction_details(GetTransactionDetailsRequest {
                    tx_id: Some(tx_id.clone()),
                })
                .await
                .unwrap_err();
            assert_eq!(block.code(), tonic::Code::Unavailable);
            assert_eq!(details.code(), tonic::Code::Unavailable);

            let (generation, upper) = server
                .block_explorer_cache
                .pending_backfill()
                .await
                .unwrap();
            server
                .block_explorer_cache
                .clone()
                .backfill_older(handle.clone(), generation, upper)
                .await
                .unwrap();
            let calls = handle.ranges().len();
            for rollback in [false, true] {
                if rollback {
                    handle.set_tip(254, 0);
                    server
                        .block_explorer_cache
                        .refresh(&server.handle)
                        .await
                        .unwrap();
                }
                let block = client
                    .get_transaction_block(GetTransactionBlockRequest {
                        tx_id: Some(tx_id.clone()),
                    })
                    .await
                    .unwrap()
                    .into_inner();
                assert!(matches!(
                    block.result,
                    Some(get_transaction_block_response::Result::Block(
                        TransactionBlockData { height: 100, .. }
                    ))
                ));
                let details = client
                    .get_transaction_details(GetTransactionDetailsRequest {
                        tx_id: Some(tx_id.clone()),
                    })
                    .await
                    .unwrap()
                    .into_inner();
                assert!(matches!(
                    details.result,
                    Some(get_transaction_details_response::Result::Details(details))
                        if details.height == 100
                ));
            }
            assert!(server
                .block_explorer_cache
                .pending_backfill()
                .await
                .is_none());
            assert_eq!(&handle.ranges()[calls..], &[(127, 254)]);
            assert!(server
                .block_explorer_cache
                .load_full_page_by_id(&server.handle, &ReorgHandle::block_id(0, 255).to_base58(),)
                .await
                .is_err());
        })
        .catch_unwind()
        .await;
        let _ = shutdown.send(());
        transport.await.unwrap().unwrap();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }

    #[tokio::test]
    async fn same_height_fork_retains_common_confirmations_and_removes_orphan_by_both_endpoints() {
        let handle = Arc::new(ReorgHandle::new(255));
        let (server, blocks) = explorer_endpoint_fixture(handle.clone(), true).await;
        assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::suffix_tx(), 250).await;
        handle.fork_at(240);
        server
            .block_explorer_cache
            .refresh(&server.handle)
            .await
            .unwrap();
        assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::orphan_tx(), 100).await;
        assert!(server
            .block_explorer_cache
            .pending_backfill()
            .await
            .is_none());
        let block = blocks
            .get_transaction_block(Request::new(GetTransactionBlockRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::suffix_tx().to_base58(),
                }),
            }))
            .await
            .unwrap_err();
        let details = blocks
            .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::suffix_tx().to_base58(),
                }),
            }))
            .await
            .unwrap_err();
        assert_eq!(block.code(), tonic::Code::NotFound);
        assert_eq!(details.code(), tonic::Code::NotFound);
        assert!(server
            .block_explorer_cache
            .load_full_page_by_id(&server.handle, &ReorgHandle::block_id(0, 250).to_base58(),)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn transaction_details_in_flight_across_reconciliation_returns_unavailable() {
        let handle = Arc::new(ReorgHandle::new(255));
        let (server, blocks) = explorer_endpoint_fixture(handle.clone(), true).await;
        assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::suffix_tx(), 250).await;
        let blocks = Arc::new(blocks);
        let request = GetTransactionDetailsRequest {
            tx_id: Some(Base58Hash {
                hash: ReorgHandle::suffix_tx().to_base58(),
            }),
        };
        handle.pause_block_transaction();
        let lookup = tokio::spawn({
            let blocks = blocks.clone();
            let request = request.clone();
            async move { blocks.get_transaction_details(Request::new(request)).await }
        });
        time::timeout(Duration::from_secs(1), handle.wait_for_block_transaction())
            .await
            .expect("confirmed transaction peek should reach its release barrier");

        // The authoritative peek has captured the old validated transaction.
        // Publish the new branch before allowing that response to reach the API.
        handle.fork_at(240);
        server
            .block_explorer_cache
            .refresh(&server.handle)
            .await
            .unwrap();
        assert!(server
            .block_explorer_cache
            .get_block_for_tx(&ReorgHandle::suffix_tx())
            .await
            .is_none());
        handle.release_block_transaction();
        let stale = lookup.await.unwrap().unwrap_err();
        assert_eq!(stale.code(), tonic::Code::Unavailable);

        let retry = blocks
            .get_transaction_details(Request::new(request))
            .await
            .unwrap_err();
        assert_eq!(retry.code(), tonic::Code::NotFound);
        assert_transaction_confirmed_by_both(&blocks, &ReorgHandle::orphan_tx(), 100).await;
    }

    #[tokio::test]
    async fn incomplete_history_raw_transaction_miss_is_unavailable_by_both_endpoints() {
        let handle = Arc::new(ReorgHandle::new(255));
        handle.retain_raw_transactions();
        let (_server, blocks) = explorer_endpoint_fixture(handle, false).await;
        let block = blocks
            .get_transaction_block(Request::new(GetTransactionBlockRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::orphan_tx().to_base58(),
                }),
            }))
            .await
            .unwrap_err();
        let details = blocks
            .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::orphan_tx().to_base58(),
                }),
            }))
            .await
            .unwrap_err();
        assert_eq!(block.code(), tonic::Code::Unavailable);
        assert_eq!(details.code(), tonic::Code::Unavailable);
    }

    #[tokio::test]
    async fn complete_history_genuine_pending_is_pending_by_both_endpoints() {
        let handle = Arc::new(ReorgHandle::new(255));
        handle.retain_raw_transactions();
        let (_server, blocks) = explorer_endpoint_fixture(handle, true).await;
        let block = blocks
            .get_transaction_block(Request::new(GetTransactionBlockRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::pending_tx().to_base58(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        let details = blocks
            .get_transaction_details(Request::new(GetTransactionDetailsRequest {
                tx_id: Some(Base58Hash {
                    hash: ReorgHandle::pending_tx().to_base58(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            block.result,
            Some(get_transaction_block_response::Result::Pending(_))
        ));
        assert!(matches!(
            details.result,
            Some(get_transaction_details_response::Result::Pending(_))
        ));
    }

    #[derive(Debug, Default)]
    struct CapturedTransactionService {
        content_id: String,
        started_us: u64,
        completed_us: u64,
        recorded_us: u64,
        accepted: bool,
        outcome: String,
        basis: String,
        boundary: String,
    }

    impl tracing::field::Visit for CapturedTransactionService {
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            match field.name() {
                "service_started_us" => self.started_us = value,
                "service_completed_us" => self.completed_us = value,
                "monotonic_us" => self.recorded_us = value,
                _ => {}
            }
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "accepted" {
                self.accepted = value;
            }
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            match field.name() {
                "content_id" => self.content_id = value.to_owned(),
                "outcome" => self.outcome = value.to_owned(),
                "acceptance_basis" => self.basis = value.to_owned(),
                "boundary" => self.boundary = value.to_owned(),
                _ => {}
            }
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "content_id" {
                self.content_id = format!("{value:?}");
            }
        }
    }

    #[derive(Clone, Default)]
    struct CaptureTransactionServices(Arc<StdMutex<Vec<CapturedTransactionService>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureTransactionServices {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _context: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() == "nockchain::kernel_timing" {
                let mut service = CapturedTransactionService::default();
                event.record(&mut service);
                self.0.lock().expect("captured services").push(service);
            }
        }
    }

    fn seen_transaction_effects(ids: &[Hash], terminator: u64) -> NounSlab {
        let mut slab = NounSlab::new();
        let seen = nockapp::utils::make_tas(&mut slab, "seen").as_noun();
        let tx = nockapp::utils::make_tas(&mut slab, "tx").as_noun();
        let mut list = nockvm::noun::D(terminator);
        for id in ids.iter().rev() {
            let id = id.to_noun(&mut slab);
            let effect = nockvm::noun::T(&mut slab, &[seen, tx, id]);
            list = nockvm::noun::T(&mut slab, &[effect, list]);
        }
        slab.set_root(list);
        slab
    }

    #[test]
    fn own_seen_effect_requires_matching_typed_id_and_well_formed_effects() {
        let id = fixtures::make_hash(10);
        let other = fixtures::make_hash(20);
        assert!(own_seen_transaction(
            &seen_transaction_effects(&[other.clone(), id.clone()], 0),
            &id
        ));
        assert!(!own_seen_transaction(
            &seen_transaction_effects(&[other], 0),
            &id
        ));
        assert!(!own_seen_transaction(
            &seen_transaction_effects(&[], 0),
            &id
        ));
        assert!(!own_seen_transaction(
            &seen_transaction_effects(std::slice::from_ref(&id), 1),
            &id
        ));

        let mut malformed = NounSlab::new();
        let seen = nockapp::utils::make_tas(&mut malformed, "seen").as_noun();
        let tx = nockapp::utils::make_tas(&mut malformed, "tx").as_noun();
        let effect = nockvm::noun::T(&mut malformed, &[seen, tx, nockvm::noun::D(1)]);
        let list = nockvm::noun::T(&mut malformed, &[effect, nockvm::noun::D(0)]);
        malformed.set_root(list);
        assert!(!own_seen_transaction(&malformed, &id));

        let mut seen_block = NounSlab::new();
        let seen = nockapp::utils::make_tas(&mut seen_block, "seen").as_noun();
        let block = nockapp::utils::make_tas(&mut seen_block, "block").as_noun();
        let noun_id = id.to_noun(&mut seen_block);
        let effect = nockvm::noun::T(&mut seen_block, &[seen, block, noun_id]);
        let list = nockvm::noun::T(&mut seen_block, &[effect, nockvm::noun::D(0)]);
        seen_block.set_root(list);
        assert!(!own_seen_transaction(&seen_block, &id));
    }

    #[test]
    fn transaction_service_event_excludes_ack_discard_wrong_id_error_and_invalid_clocks() {
        let id = fixtures::make_hash(10);
        let other = fixtures::make_hash(20);
        let capture = CaptureTransactionServices::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(capture.clone());
        let started = nockchain_libp2p_io::observation_time_us();
        let completed = nockchain_libp2p_io::observation_time_us();
        let accepted = Ok(seen_transaction_effects(std::slice::from_ref(&id), 0));
        tracing::subscriber::with_default(subscriber, || {
            observe_transaction_service(&id, started, completed, &accepted);
            // A concurrent/resubmitted poke for the same ID cannot borrow this
            // affirmative effect from the successful submission above.
            observe_transaction_service(
                &id,
                started,
                completed,
                &Ok(seen_transaction_effects(&[], 0)),
            );
            observe_transaction_service(
                &id,
                started,
                completed,
                &Ok(seen_transaction_effects(&[other], 0)),
            );
            observe_transaction_service(
                &id,
                started,
                completed,
                &Err(nockapp::CrownError::KernelError(Some(
                    seen_transaction_effects(std::slice::from_ref(&id), 0),
                ))),
            );
            observe_transaction_service(&id, u64::MAX, completed, &accepted);
            observe_transaction_service(&id, started, u64::MAX, &accepted);
        });
        let services = capture.0.lock().expect("captured services");
        assert_eq!(
            services.len(),
            1,
            "only the own matching successful result is affirmative"
        );
        let service = &services[0];
        assert_eq!(service.content_id, id.to_base58());
        assert_eq!(service.started_us, started);
        assert_eq!(service.completed_us, completed);
        assert!(service.started_us <= service.completed_us);
        assert!(service.completed_us <= service.recorded_us);
        assert!(service.accepted);
        assert_eq!(service.outcome, "ack");
        assert_eq!(service.basis, "own_seen_effect");
        assert_eq!(service.boundary, "kernel_round_trip");
    }

    struct MockHandleV0 {
        update: v0::BalanceUpdate,
        peek_calls: AtomicUsize,
        delay: Duration,
    }

    impl MockHandleV0 {
        fn new(update: v0::BalanceUpdate) -> Self {
            Self {
                update,
                peek_calls: AtomicUsize::new(0),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(update: v0::BalanceUpdate, delay: Duration) -> Self {
            Self {
                delay,
                ..Self::new(update)
            }
        }

        fn peek_calls(&self) -> usize {
            self.peek_calls.load(Ordering::SeqCst)
        }
    }

    struct MockHandle {
        update: v1::BalanceUpdate,
        peek_calls: AtomicUsize,
        delay: Duration,
    }

    impl MockHandle {
        fn new(update: v1::BalanceUpdate) -> Self {
            Self {
                update,
                peek_calls: AtomicUsize::new(0),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(update: v1::BalanceUpdate, delay: Duration) -> Self {
            Self {
                delay,
                ..Self::new(update)
            }
        }

        fn peek_calls(&self) -> usize {
            self.peek_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl BalanceHandle for MockHandle {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, nockapp::nockapp::error::NockAppError> {
            let is_heaviest_chain = {
                let space = path.noun_space();
                let root = unsafe { path.root() };
                <Vec<String>>::from_noun(&root, &space)
                    .ok()
                    .and_then(|segments| segments.first().cloned())
                    .as_deref()
                    == Some("heaviest-chain")
            };
            if is_heaviest_chain {
                let mut slab = NounSlab::new();
                let noun = Some(Some((
                    self.update.height.clone(),
                    self.update.block_id.clone(),
                )))
                .to_noun(&mut slab);
                slab.set_root(noun);
                return Ok(Some(slab));
            }
            drop(path);

            let call = self.peek_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(call, 0, "unexpected additional peek");
            tokio::time::sleep(self.delay).await;
            Ok(Some(encode_balance_update(&self.update)))
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<NativePokeObserver>,
        ) -> std::result::Result<PokeResult, nockapp::nockapp::error::NockAppError> {
            Err(nockapp::nockapp::error::NockAppError::OtherError(
                "poke not supported in mock".into(),
            ))
        }
    }

    #[async_trait]
    impl BalanceHandle for MockHandleV0 {
        async fn peek(
            &self,
            path: NounSlab,
        ) -> std::result::Result<Option<NounSlab>, nockapp::nockapp::error::NockAppError> {
            let is_heaviest_chain = {
                let space = path.noun_space();
                let root = unsafe { path.root() };
                <Vec<String>>::from_noun(&root, &space)
                    .ok()
                    .and_then(|segments| segments.first().cloned())
                    .as_deref()
                    == Some("heaviest-chain")
            };
            if is_heaviest_chain {
                let mut slab = NounSlab::new();
                let noun = Some(Some((
                    self.update.height.clone(),
                    self.update.block_id.clone(),
                )))
                .to_noun(&mut slab);
                slab.set_root(noun);
                return Ok(Some(slab));
            }
            drop(path);

            let call = self.peek_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(call, 0, "unexpected additional peek");
            tokio::time::sleep(self.delay).await;
            Ok(Some(encode_balance_update_v0(&self.update)))
        }

        async fn poke(
            &self,
            _wire: WireRepr,
            _payload: NounSlab,
            _observer: Option<NativePokeObserver>,
        ) -> std::result::Result<PokeResult, nockapp::nockapp::error::NockAppError> {
            Err(nockapp::nockapp::error::NockAppError::OtherError(
                "poke not supported in mock".into(),
            ))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_cold_address_requests_share_one_kernel_peek() {
        let (update, _) = fixtures_v1::make_balance_update(4);
        let handle = Arc::new(MockHandleV0::with_delay(update, Duration::from_millis(25)));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        let request = WalletGetBalanceRequest {
            selector: Some(Selector::Address(pb_common_v1::Base58Pubkey {
                key: A_GEN.into_base58().expect("address"),
            })),
            page: Some(pb_common_v1::PageRequest {
                client_page_items_limit: 2,
                page_token: String::new(),
                max_bytes: 0,
            }),
        };
        let (a, b, c, d, e) = tokio::join!(
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request)),
        );
        for response in [a, b, c, d, e] {
            assert!(response.is_ok());
        }
        assert_eq!(handle.peek_calls(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_cold_first_name_requests_share_one_kernel_peek() {
        let (update, _) = fixtures::make_balance_update(4);
        let handle = Arc::new(MockHandle::with_delay(update, Duration::from_millis(25)));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());
        let (name, _) = fixtures::make_named_note(15);
        let request = WalletGetBalanceRequest {
            selector: Some(Selector::FirstName(Base58Hash {
                hash: Hash::to_base58(&name.first),
            })),
            page: Some(pb_common_v1::PageRequest {
                client_page_items_limit: 2,
                page_token: String::new(),
                max_bytes: 0,
            }),
        };
        let (a, b, c, d, e) = tokio::join!(
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request.clone())),
            server.wallet_get_balance(Request::new(request)),
        );
        for response in [a, b, c, d, e] {
            assert!(response.is_ok());
        }
        assert_eq!(handle.peek_calls(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wallet_get_balance_uses_cache_for_subsequent_pages() {
        let (update, expected_names) = fixtures_v1::make_balance_update(4);
        let handle = Arc::new(MockHandleV0::new(update));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());

        let mut request = WalletGetBalanceRequest {
            selector: Some(wallet_get_balance_request::Selector::Address(
                pb_common_v1::Base58Pubkey {
                    key: A_GEN.into_base58().expect("address generation failed"),
                },
            )),
            page: Some(pb_common_v1::PageRequest {
                client_page_items_limit: 2,
                page_token: String::new(),
                max_bytes: 0,
            }),
        };

        let first_resp = server
            .wallet_get_balance(Request::new(request.clone()))
            .await
            .expect("first call ok")
            .into_inner();

        let first_balance = match first_resp.result {
            Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
            other => panic!("unexpected response: {:?}", other),
        };

        assert_eq!(first_balance.notes.len(), 2);
        let mut collected_names: Vec<pb_common_v1::Name> = first_balance
            .notes
            .iter()
            .map(|entry| entry.name.clone().expect("balance entry missing name"))
            .collect();

        let next_page_token = first_balance.page.expect("page info").next_page_token;
        assert!(!next_page_token.is_empty(), "expected non-empty page token");

        request.page = Some(pb_common_v1::PageRequest {
            client_page_items_limit: 2,
            page_token: next_page_token,
            max_bytes: 0,
        });

        let second_resp = server
            .wallet_get_balance(Request::new(request))
            .await
            .expect("second call ok")
            .into_inner();

        let second_balance = match second_resp.result {
            Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
            other => panic!("unexpected response: {:?}", other),
        };

        collected_names.extend(second_balance.notes.into_iter().map(|entry| {
            entry
                .name
                .expect("balance entry missing name on second page")
        }));

        let mut collected_sorted: Vec<v1::Name> = collected_names
            .into_iter()
            .map(|name| name.try_into().expect("convert name"))
            .collect();
        collected_sorted.sort_by(cmp_name);

        let mut expected_sorted = expected_names.clone();
        expected_sorted.sort_by(cmp_name);

        assert_eq!(collected_sorted, expected_sorted);
        assert_eq!(handle.peek_calls(), 1, "cache should prevent second peek");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wallet_get_balance_by_first_name_uses_cache_for_subsequent_pages() {
        // TODO: finish test
        let (update, expected_names) = fixtures::make_balance_update(4);
        let handle = Arc::new(MockHandle::new(update));
        let server = PublicNockchainGrpcServer::with_handle(handle.clone());

        let (name, _) = fixtures::make_named_note(15 as u64);

        let mut request = WalletGetBalanceRequest {
            selector: Some(wallet_get_balance_request::Selector::FirstName(
                Base58Hash {
                    hash: Hash::to_base58(&name.first),
                },
            )),
            page: Some(pb_common_v1::PageRequest {
                client_page_items_limit: 2,
                page_token: String::new(),
                max_bytes: 0,
            }),
        };

        let first_resp = server
            .wallet_get_balance(Request::new(request.clone()))
            .await
            .expect("first call ok")
            .into_inner();

        let first_balance = match first_resp.result {
            Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
            other => panic!("unexpected response: {:?}", other),
        };

        assert_eq!(first_balance.notes.len(), 2);
        let mut collected_names: Vec<pb_common_v1::Name> = first_balance
            .notes
            .iter()
            .map(|entry| entry.name.clone().expect("balance entry missing name"))
            .collect();

        let next_page_token = first_balance.page.expect("page info").next_page_token;
        assert!(!next_page_token.is_empty(), "expected non-empty page token");

        request.page = Some(pb_common_v1::PageRequest {
            client_page_items_limit: 2,
            page_token: next_page_token,
            max_bytes: 0,
        });

        let second_resp = server
            .wallet_get_balance(Request::new(request))
            .await
            .expect("second call ok")
            .into_inner();

        let second_balance = match second_resp.result {
            Some(wallet_get_balance_response::Result::Balance(balance)) => balance,
            other => panic!("unexpected response: {:?}", other),
        };

        collected_names.extend(second_balance.notes.into_iter().map(|entry| {
            entry
                .name
                .expect("balance entry missing name on second page")
        }));

        let mut collected_sorted: Vec<v1::Name> = collected_names
            .into_iter()
            .map(|name| name.try_into().expect("convert name"))
            .collect();
        collected_sorted.sort_by(cmp_name);

        let mut expected_sorted = expected_names.clone();
        expected_sorted.sort_by(cmp_name);

        assert_eq!(collected_sorted, expected_sorted);
        assert_eq!(handle.peek_calls(), 1, "cache should prevent second peek");
    }

    #[tokio::test]
    async fn get_peer_stats_returns_snapshot_from_registry() {
        let (update, _) = fixtures::make_balance_update(1);
        let handle = Arc::new(MockHandle::new(update));
        let metrics = init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics.clone()));
        let peer_stats_registry = Arc::new(TransportPeerStatsRegistry::default());
        peer_stats_registry.replace_snapshot(TransportPeerStatsSnapshot {
            collected_at_unix_ms: 42,
            peers: vec![TransportPeerStatsEntry {
                peer_id: "peer-1".to_string(),
                protocol_generation: TransportPeerReqResGeneration::Gen2,
                request_count: 7,
                bytes_sent: 128,
                bytes_received: 512,
                average_round_trip_ms: 4.5,
                average_batch_size: 3.5,
                failure_count: 1,
                timeout_count: 0,
                blocks_received: 2,
                average_block_propagation_ms: 1.25,
                connection_duration_seconds: 33,
            }],
        });
        let server = NockchainMetricsServer::with_peer_stats_registry(
            handle, cache, metrics, peer_stats_registry,
        );

        let response = server
            .get_peer_stats(Request::new(GetPeerStatsRequest {}))
            .await
            .expect("peer stats response")
            .into_inner();

        let stats = match response.result {
            Some(get_peer_stats_response::Result::Stats(stats)) => stats,
            other => panic!("unexpected response: {:?}", other),
        };
        assert_eq!(stats.collected_at_unix_ms, 42);
        assert_eq!(stats.peers.len(), 1);
        assert_eq!(stats.peers[0].peer_id, "peer-1");
        assert_eq!(
            stats.peers[0].protocol_generation,
            PeerReqResGeneration::Gen2 as i32
        );
        assert_eq!(stats.peers[0].request_count, 7);
        assert_eq!(stats.peers[0].bytes_received, 512);
        assert_eq!(stats.peers[0].connection_duration_seconds, 33);
    }

    #[tokio::test]
    async fn get_req_res_metrics_returns_transport_counters() {
        let (update, _) = fixtures::make_balance_update(1);
        let handle = Arc::new(MockHandle::new(update));
        let metrics = init_metrics();
        let cache = Arc::new(BlockExplorerCache::new(metrics.clone()));
        let server = NockchainMetricsServer::new(handle, cache, metrics);

        let baseline_gen1: u64 = server
            .transport_metrics
            .gen1_outbound_timeouts
            .fetch_add(0)
            .try_into()
            .expect("timeout counter should fit in u64");
        let baseline_gen2: u64 = server
            .transport_metrics
            .gen2_outbound_timeouts
            .fetch_add(0)
            .try_into()
            .expect("timeout counter should fit in u64");
        let baseline_sent: u64 = server
            .transport_metrics
            .gen2_batch_requests_sent
            .fetch_add(0)
            .try_into()
            .expect("batch request counter should fit in u64");
        let baseline_received: u64 = server
            .transport_metrics
            .gen2_batch_requests_received
            .fetch_add(0)
            .try_into()
            .expect("batch request counter should fit in u64");
        let baseline_fallback: u64 = server
            .transport_metrics
            .req_res_fallback_total
            .fetch_add(0)
            .try_into()
            .expect("fallback counter should fit in u64");
        let baseline_routed: u64 = server
            .transport_metrics
            .req_res_block_by_height_gen1_routed
            .fetch_add(0)
            .try_into()
            .expect("fallback route counter should fit in u64");

        server.transport_metrics.gen1_outbound_timeouts.fetch_add(2);
        server.transport_metrics.gen2_outbound_timeouts.fetch_add(3);
        server
            .transport_metrics
            .gen2_batch_requests_sent
            .fetch_add(5);
        server
            .transport_metrics
            .gen2_batch_requests_received
            .fetch_add(7);
        server
            .transport_metrics
            .req_res_fallback_total
            .fetch_add(11);
        server
            .transport_metrics
            .req_res_block_by_height_gen1_routed
            .fetch_add(13);

        let response = server
            .get_req_res_metrics(Request::new(GetReqResMetricsRequest {}))
            .await
            .expect("req-res metrics response")
            .into_inner();

        let metrics = match response.result {
            Some(get_req_res_metrics_response::Result::Metrics(metrics)) => metrics,
            other => panic!("unexpected response: {:?}", other),
        };
        assert_eq!(metrics.gen1_outbound_timeouts, baseline_gen1 + 2);
        assert_eq!(metrics.gen2_outbound_timeouts, baseline_gen2 + 3);
        assert_eq!(metrics.gen2_batch_requests_sent, baseline_sent + 5);
        assert_eq!(metrics.gen2_batch_requests_received, baseline_received + 7);
        assert_eq!(metrics.req_res_fallback_total, baseline_fallback + 11);
        assert_eq!(
            metrics.req_res_block_by_height_gen1_routed,
            baseline_routed + 13
        );
    }

    fn encode_balance_update(update: &v1::BalanceUpdate) -> NounSlab {
        let mut slab = NounSlab::new();
        let noun = Some(Some(update.clone())).to_noun(&mut slab);
        slab.set_root(noun);
        slab
    }

    fn encode_balance_update_v0(update: &v0::BalanceUpdate) -> NounSlab {
        let mut slab = NounSlab::new();
        let noun = Some(Some(update.clone())).to_noun(&mut slab);
        slab.set_root(noun);
        slab
    }
}
