use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::error::Error;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use futures::AsyncWriteExt;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use libp2p::core::ConnectedPoint;
use libp2p::request_response::cbor;
use libp2p::swarm::{ConnectionId, NetworkBehaviour};
use libp2p::{request_response, Multiaddr, PeerId, Swarm};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp::AtomExt;
use nockvm::noun::{Atom, NounAllocator, D, T};
use nockvm_macros::tas;
use serde_bytes::ByteBuf;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use crate::behaviour::request_response_protocols;
use crate::config::LibP2PConfig;
use crate::driver::{
    build_retry_request_contexts, build_unsupported_protocol_fallback_contexts,
    collect_tip5_zset_strings, handle_outbound_request_failure, heard_block_height_from_fact_poke,
    heard_block_tx_ids_from_fact_poke, SwarmAction,
};
use crate::ip_block::PeerExclusions;
use crate::key_fair_queue;
use crate::messages::{
    block_range_with_txs_request_message, decode_request_item_message, NockchainDataRequest,
    NockchainFact,
};
pub use crate::messages::{
    BatchErrorClass, BatchRequestItem, BatchResultItem, BatchResultStatus, BundledBlockWithTxs,
    NockchainRequest, NockchainResponse, ResponseEnvelope,
};
use crate::metrics::NockchainP2PMetrics;
pub use crate::p2p_state::ReqResGeneration;
use crate::p2p_state::{
    InboundReplayAdmission, OutboundRequestContext, P2PState, DEFERRED_HEARD_BLOCK_PER_PEER_CAP,
};
use crate::peer_stats::{PeerStatsRegistry, PeerStatsSnapshot};
use crate::tip5_util::tip5_hash_to_base58;

#[derive(Clone, Debug, Default)]
pub struct ProtocolTrace {
    entries: Arc<Mutex<Vec<ProtocolTraceEntry>>>,
}

#[derive(Clone, Debug, Default)]
pub struct RawResponseInjection {
    pending: Arc<Mutex<Option<Vec<u8>>>>,
}

impl RawResponseInjection {
    pub fn inject_once(&self, bytes: Vec<u8>) {
        *self
            .pending
            .lock()
            .expect("raw response injection mutex poisoned") = Some(bytes);
    }

    fn take(&self) -> Option<Vec<u8>> {
        self.pending
            .lock()
            .expect("raw response injection mutex poisoned")
            .take()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RawRequestInjection {
    pending: Arc<Mutex<Option<Vec<u8>>>>,
}

impl RawRequestInjection {
    pub fn inject_once(&self, bytes: Vec<u8>) {
        *self
            .pending
            .lock()
            .expect("raw request injection mutex poisoned") = Some(bytes);
    }

    fn take(&self) -> Option<Vec<u8>> {
        self.pending
            .lock()
            .expect("raw request injection mutex poisoned")
            .take()
    }
}

impl ProtocolTrace {
    pub fn snapshot(&self) -> Vec<ProtocolTraceEntry> {
        self.entries
            .lock()
            .expect("protocol trace mutex poisoned")
            .clone()
    }

    fn record(
        &self,
        actor: &'static str,
        operation: &'static str,
        protocol: &libp2p::StreamProtocol,
    ) {
        self.entries
            .lock()
            .expect("protocol trace mutex poisoned")
            .push(ProtocolTraceEntry {
                actor,
                operation,
                protocol: protocol.as_ref().to_string(),
            });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolTraceEntry {
    pub actor: &'static str,
    pub operation: &'static str,
    pub protocol: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReqResProtocolSupportSummary {
    pub protocol: String,
    pub inbound: bool,
    pub outbound: bool,
}

#[derive(Clone)]
pub struct ProtocolRecordingCodec {
    actor: &'static str,
    trace: Option<ProtocolTrace>,
    raw_request_injection: RawRequestInjection,
    raw_response_injection: RawResponseInjection,
    inner: cbor::codec::Codec<NockchainRequest, NockchainResponse>,
}

impl ProtocolRecordingCodec {
    fn new(
        actor: &'static str,
        trace: Option<ProtocolTrace>,
        raw_request_injection: RawRequestInjection,
        raw_response_injection: RawResponseInjection,
        libp2p_config: &LibP2PConfig,
    ) -> Self {
        let inner = cbor::codec::Codec::default()
            .set_request_size_maximum(libp2p_config.gen2_batch_max_bytes() as u64)
            .set_response_size_maximum(libp2p_config.gen2_batch_max_bytes() as u64);
        Self {
            actor,
            trace,
            raw_request_injection,
            raw_response_injection,
            inner,
        }
    }

    fn record(&self, operation: &'static str, protocol: &libp2p::StreamProtocol) {
        if let Some(trace) = &self.trace {
            trace.record(self.actor, operation, protocol);
        }
    }
}

#[async_trait]
impl request_response::Codec for ProtocolRecordingCodec {
    type Protocol = libp2p::StreamProtocol;
    type Request = NockchainRequest;
    type Response = NockchainResponse;

    async fn read_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let request = request_response::Codec::read_request(&mut self.inner, protocol, io).await?;
        self.record("read_request", protocol);
        Ok(request)
    }

    async fn read_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let response =
            request_response::Codec::read_response(&mut self.inner, protocol, io).await?;
        self.record("read_response", protocol);
        Ok(response)
    }

    async fn write_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        if let Some(raw_request) = self.raw_request_injection.take() {
            io.write_all(raw_request.as_ref()).await?;
            self.record("write_request", protocol);
            return Ok(());
        }

        request_response::Codec::write_request(&mut self.inner, protocol, io, req).await?;
        self.record("write_request", protocol);
        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        resp: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        if let Some(raw_response) = self.raw_response_injection.take() {
            io.write_all(raw_response.as_ref()).await?;
            self.record("write_response", protocol);
            return Ok(());
        }

        request_response::Codec::write_response(&mut self.inner, protocol, io, resp).await?;
        self.record("write_response", protocol);
        Ok(())
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "ReqResTestEvent")]
pub struct ReqResTestBehaviour {
    pub request_response: request_response::Behaviour<ProtocolRecordingCodec>,
}

#[derive(Debug)]
pub enum ReqResTestEvent {
    RequestResponse(request_response::Event<NockchainRequest, NockchainResponse>),
}

impl From<request_response::Event<NockchainRequest, NockchainResponse>> for ReqResTestEvent {
    fn from(event: request_response::Event<NockchainRequest, NockchainResponse>) -> Self {
        Self::RequestResponse(event)
    }
}

impl From<Infallible> for ReqResTestEvent {
    fn from(value: Infallible) -> Self {
        match value {}
    }
}

pub type ReqResTestSwarm = Swarm<ReqResTestBehaviour>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReqResObservabilitySnapshot {
    pub request_failed: u64,
    pub gen1_outbound_failures: u64,
    pub gen1_outbound_timeouts: u64,
    pub gen2_outbound_failures: u64,
    pub gen2_outbound_timeouts: u64,
    pub req_res_fallback_total: u64,
}

pub struct ReqResFailureObservabilityProbe {
    local_peer_id: PeerId,
    metrics: Arc<NockchainP2PMetrics>,
    peer_exclusions: PeerExclusions,
    peer_stats_registry: Arc<PeerStatsRegistry>,
    state: Arc<AsyncMutex<P2PState>>,
    swarm_tx: mpsc::Sender<SwarmAction>,
    swarm_rx: AsyncMutex<mpsc::Receiver<SwarmAction>>,
    next_connection_id: AtomicUsize,
}

impl ReqResFailureObservabilityProbe {
    pub fn new(local_peer_id: PeerId, libp2p_config: &LibP2PConfig) -> Self {
        let metrics_registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        let metrics = Arc::new(
            NockchainP2PMetrics::register(&metrics_registry).expect("Could not register metrics"),
        );
        let peer_stats_registry = Arc::new(PeerStatsRegistry::default());
        let state = Arc::new(AsyncMutex::new(P2PState::with_peer_stats_registry(
            metrics.clone(),
            libp2p_config.seen_tx_clear_interval,
            peer_stats_registry.clone(),
        )));
        let (swarm_tx, swarm_rx) = mpsc::channel(32);

        Self {
            local_peer_id,
            metrics,
            peer_exclusions: PeerExclusions::default(),
            peer_stats_registry,
            state,
            swarm_tx,
            swarm_rx: AsyncMutex::new(swarm_rx),
            next_connection_id: AtomicUsize::new(1),
        }
    }

    pub async fn observe_connected_peer(
        &self,
        peer_id: PeerId,
        remote_addr: &Multiaddr,
        local_addr: &Multiaddr,
        generation: ReqResGeneration,
    ) {
        let connection_id =
            ConnectionId::new_unchecked(self.next_connection_id.fetch_add(1, Ordering::Relaxed));
        let mut state = self.state.lock().await;
        state.track_connection(
            connection_id,
            peer_id,
            remote_addr,
            ConnectedPoint::Listener {
                local_addr: local_addr.clone(),
                send_back_addr: remote_addr.clone(),
            },
        );
        state.observe_peer_generation(peer_id, generation);
    }

    pub async fn observe_outbound_failure(
        &self,
        peer_id: PeerId,
        generation: ReqResGeneration,
        request: NockchainRequest,
        error: request_response::OutboundFailure,
    ) {
        let request_id = fresh_outbound_request_id();
        {
            let mut state = self.state.lock().await;
            state.record_outbound_request(
                request_id,
                OutboundRequestContext::new(peer_id, generation, request),
            );
        }

        let mut equix_builder = equix::EquiXBuilder::new();
        handle_outbound_request_failure(
            &self.swarm_tx,
            Arc::clone(&self.state),
            Arc::clone(&self.metrics),
            self.local_peer_id,
            &mut equix_builder,
            self.peer_exclusions.clone(),
            peer_id,
            request_id,
            error,
        )
        .await;

        self.drain_swarm_actions().await;
    }

    pub fn snapshot(&self) -> ReqResObservabilitySnapshot {
        ReqResObservabilitySnapshot {
            request_failed: self.metrics.request_failed.fetch_add(0) as u64,
            gen1_outbound_failures: self.metrics.gen1_outbound_failures.fetch_add(0) as u64,
            gen1_outbound_timeouts: self.metrics.gen1_outbound_timeouts.fetch_add(0) as u64,
            gen2_outbound_failures: self.metrics.gen2_outbound_failures.fetch_add(0) as u64,
            gen2_outbound_timeouts: self.metrics.gen2_outbound_timeouts.fetch_add(0) as u64,
            req_res_fallback_total: self.metrics.req_res_fallback_total.fetch_add(0) as u64,
        }
    }

    pub fn peer_stats_snapshot(&self) -> PeerStatsSnapshot {
        self.peer_stats_registry.snapshot()
    }

    async fn drain_swarm_actions(&self) {
        let mut swarm_rx = self.swarm_rx.lock().await;
        while swarm_rx.try_recv().is_ok() {}
    }
}

fn fresh_outbound_request_id() -> request_response::OutboundRequestId {
    let mut behaviour: request_response::cbor::Behaviour<NockchainRequest, NockchainResponse> =
        request_response::cbor::Behaviour::new(
            [(
                libp2p::StreamProtocol::new(LibP2PConfig::req_res_gen1_protocol_version()),
                request_response::ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        );
    behaviour.send_request(
        &PeerId::random(),
        NockchainRequest::Gossip {
            message: ByteBuf::from(vec![0xAB]),
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayProbeAdmission {
    Accepted,
    Replayed,
    Disabled,
}

impl From<InboundReplayAdmission> for ReplayProbeAdmission {
    fn from(value: InboundReplayAdmission) -> Self {
        match value {
            InboundReplayAdmission::Accepted => Self::Accepted,
            InboundReplayAdmission::Replayed => Self::Replayed,
            InboundReplayAdmission::Disabled => Self::Disabled,
        }
    }
}

pub struct ReqResStateBoundsProbe {
    state: P2PState,
    peer_id: PeerId,
    ttl: Duration,
    max_replay_keys_per_peer: usize,
    next_connection_id: usize,
}

impl ReqResStateBoundsProbe {
    pub fn new(max_replay_keys_per_peer: usize) -> Self {
        let metrics_registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        let metrics = Arc::new(
            NockchainP2PMetrics::register(&metrics_registry).expect("Could not register metrics"),
        );
        Self {
            state: P2PState::new(metrics, LibP2PConfig::default().seen_tx_clear_interval),
            peer_id: PeerId::random(),
            ttl: Duration::from_secs(60),
            max_replay_keys_per_peer,
            next_connection_id: 1,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn connect_peer(&mut self, remote_addr: Multiaddr) -> ConnectionId {
        let connection_id = ConnectionId::new_unchecked(self.next_connection_id);
        self.next_connection_id = self.next_connection_id.saturating_add(1);
        self.state.track_connection(
            connection_id,
            self.peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr: "/ip4/0.0.0.0/tcp/0".parse().expect("valid local addr"),
                send_back_addr: remote_addr.clone(),
            },
        );
        connection_id
    }

    pub fn disconnect(&mut self, connection_id: ConnectionId) -> usize {
        self.state.lost_connection(connection_id)
    }

    pub fn remove_peer(&mut self) {
        self.state.remove_peer(&self.peer_id);
    }

    pub fn admit_replay_key(
        &mut self,
        request: &NockchainRequest,
    ) -> Result<ReplayProbeAdmission, nockapp::NockAppError> {
        let Some(key) = request.replay_key()? else {
            return Ok(ReplayProbeAdmission::Disabled);
        };
        Ok(self
            .state
            .admit_inbound_replay_key(self.peer_id, key, self.ttl, self.max_replay_keys_per_peer)
            .into())
    }

    pub fn has_replay_key(
        &mut self,
        request: &NockchainRequest,
    ) -> Result<bool, nockapp::NockAppError> {
        let Some(key) = request.replay_key()? else {
            return Ok(false);
        };
        Ok(self.state.has_inbound_replay(self.peer_id, key, self.ttl))
    }

    pub fn defer_dummy_heard_block(&mut self, height: u64, block_id: String) -> bool {
        self.state
            .defer_heard_block(self.peer_id, height, block_id, dummy_heard_block_fact())
    }

    pub fn defer_realistic_heard_block(
        &mut self,
        height: u64,
        tx_seeds: &[u64],
    ) -> Result<Option<usize>, nockapp::NockAppError> {
        let (block_id, fact, jam_bytes) = realistic_heard_block_fact_for_height(height, tx_seeds)?;
        Ok(self
            .state
            .defer_heard_block(self.peer_id, height, block_id, fact)
            .then_some(jam_bytes))
    }

    pub fn deferred_heard_block_total(&self) -> usize {
        self.state.deferred_heard_block_total()
    }

    pub fn has_deferred_block_at_height(&self, height: u64) -> bool {
        self.state.has_deferred_block_at_height(height)
    }
}

pub fn deferred_heard_block_per_peer_cap() -> usize {
    DEFERRED_HEARD_BLOCK_PER_PEER_CAP
}

fn dummy_heard_block_fact() -> NockchainFact {
    let mut slab: NounSlab = NounSlab::new();
    let inner = T(&mut slab, &[D(1), D(2)]);
    slab.set_root(inner);
    NockchainFact::HeardBlock(String::from("dummy"), slab)
}

pub fn solve_block_by_height_request(
    sender_peer_id: &PeerId,
    receiver_peer_id: &PeerId,
    height: u64,
) -> NockchainRequest {
    let mut slab = NounSlab::new();
    let by_height_tas = make_tas(&mut slab, "by-height");
    let by_height = T(&mut slab, &[by_height_tas.as_noun(), D(height)]);
    let block_cell = T(&mut slab, &[D(tas!(b"block")), by_height]);
    let request = T(&mut slab, &[D(tas!(b"request")), block_cell]);
    slab.set_root(request);

    let mut builder = equix::EquiXBuilder::new();
    NockchainRequest::new_request(&mut builder, sender_peer_id, receiver_peer_id, &slab)
}

pub fn solve_batch_request(
    sender_peer_id: &PeerId,
    receiver_peer_id: &PeerId,
    items: Vec<BatchRequestItem>,
) -> Result<NockchainRequest, nockapp::NockAppError> {
    let mut builder = equix::EquiXBuilder::new();
    NockchainRequest::new_batch_request(&mut builder, sender_peer_id, receiver_peer_id, items)
}

pub fn solve_authenticated_gossip(
    sender_peer_id: &PeerId,
    receiver_peer_id: &PeerId,
    message: impl AsRef<[u8]>,
) -> NockchainRequest {
    let mut builder = equix::EquiXBuilder::new();
    NockchainRequest::authenticated_gossip_from_message(
        &mut builder,
        sender_peer_id,
        receiver_peer_id,
        ByteBuf::from(message.as_ref().to_vec()),
    )
    .expect("authenticated gossip PoW should be solved")
}

pub fn request_pow_verifies_at(
    request: &NockchainRequest,
    receiver_peer_id: &PeerId,
    sender_peer_id: &PeerId,
) -> bool {
    let mut builder = equix::EquiXBuilder::new();
    request
        .verify_pow(&mut builder, receiver_peer_id, sender_peer_id)
        .is_ok()
}

pub struct ReqResLiarBanProbe {
    state: P2PState,
}

impl ReqResLiarBanProbe {
    pub fn new() -> Self {
        let metrics_registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        let metrics = Arc::new(
            NockchainP2PMetrics::register(&metrics_registry).expect("Could not register metrics"),
        );
        Self {
            state: P2PState::new(metrics, LibP2PConfig::default().seen_tx_clear_interval),
        }
    }

    pub fn track_block_id_from_peer(&mut self, block_id: impl Into<String>, peer_id: PeerId) {
        self.state
            .track_block_id_str_and_peer(block_id.into(), peer_id);
    }

    pub fn mark_liar_block_id(&mut self, block_id: &str) -> Vec<PeerId> {
        self.state.invalidate_deferred_block_id(block_id);
        self.state.process_bad_block_id_str(block_id)
    }

    pub fn block_ids_for_peer(&self, peer_id: PeerId) -> Vec<String> {
        self.state.get_block_ids_for_peer(peer_id)
    }

    pub fn is_tracking_peer(&self, peer_id: PeerId) -> bool {
        self.state.is_tracking_peer(peer_id)
    }

    pub fn defer_dummy_heard_block(&mut self, peer_id: PeerId, height: u64, block_id: String) {
        self.state
            .defer_heard_block(peer_id, height, block_id, dummy_heard_block_fact());
    }

    pub fn deferred_heard_block_total(&self) -> usize {
        self.state.deferred_heard_block_total()
    }

    pub fn has_deferred_block_at_height(&self, height: u64) -> bool {
        self.state.has_deferred_block_at_height(height)
    }
}

impl Default for ReqResLiarBanProbe {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReqResKernelPressureProbe {
    state: P2PState,
}

impl ReqResKernelPressureProbe {
    pub fn new() -> Self {
        let metrics_registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        let metrics = Arc::new(
            NockchainP2PMetrics::register(&metrics_registry).expect("Could not register metrics"),
        );
        Self {
            state: P2PState::new(metrics, LibP2PConfig::default().seen_tx_clear_interval),
        }
    }

    pub fn claim_speculative_tx_prefetches(
        &mut self,
        tx_ids: Vec<String>,
        max_claims: usize,
    ) -> Vec<String> {
        self.state
            .claim_speculative_tx_prefetch_ids(tx_ids, Duration::from_secs(15), max_claims)
    }

    pub fn clear_speculative_tx_prefetch(&mut self, tx_id: &str) {
        self.state.clear_speculative_tx_prefetch(tx_id);
    }
}

impl Default for ReqResKernelPressureProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFairQueuePressureObservation {
    pub per_key_rejected: bool,
    pub total_rejected: bool,
    pub recovered_after_recv: bool,
    pub received_before_recovery: Option<(u8, u8)>,
    pub received_after_recovery: Vec<(u8, u8)>,
}

pub async fn observe_bounded_key_fair_queue_pressure() -> KeyFairQueuePressureObservation {
    let (sender, mut receiver) = key_fair_queue::channel_with_limits::<u8, u8>(2, 1);
    sender.send(1, 10).expect("first value should fit");
    let per_key_rejected = matches!(sender.send(1, 11), Err(key_fair_queue::Error::Full));
    sender.send(2, 20).expect("second key should fit");
    let total_rejected = matches!(sender.send(3, 30), Err(key_fair_queue::Error::Full));

    let received_before_recovery = receiver.recv().await;
    let recovered_after_recv = sender.send(3, 30).is_ok();
    let mut received_after_recovery = Vec::new();
    while let Ok(Some(item)) =
        tokio::time::timeout(Duration::from_millis(10), receiver.recv()).await
    {
        received_after_recovery.push(item);
        if received_after_recovery.len() == 2 {
            break;
        }
    }

    KeyFairQueuePressureObservation {
        per_key_rejected,
        total_rejected,
        recovered_after_recv,
        received_before_recovery,
        received_after_recovery,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueuePressurePeer {
    Hostile,
    Honest(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixedPeerQueuePressureObservation {
    pub hostile_per_key_rejected: bool,
    pub total_rejected_after_mixed_fill: bool,
    pub accepted_honest_items: usize,
    pub received: Vec<(QueuePressurePeer, u8)>,
}

pub async fn observe_mixed_peer_key_fair_queue_pressure() -> MixedPeerQueuePressureObservation {
    let (sender, mut receiver) = key_fair_queue::channel_with_limits::<QueuePressurePeer, u8>(7, 4);

    for value in 0..4 {
        sender
            .send(QueuePressurePeer::Hostile, value)
            .expect("hostile peer should fill its per-key budget");
    }
    let hostile_per_key_rejected = matches!(
        sender.send(QueuePressurePeer::Hostile, 4),
        Err(key_fair_queue::Error::Full)
    );

    let mut accepted_honest_items = 0;
    for (peer, value) in [
        (QueuePressurePeer::Honest(1), 10),
        (QueuePressurePeer::Honest(2), 20),
        (QueuePressurePeer::Honest(3), 30),
    ] {
        if sender.send(peer, value).is_ok() {
            accepted_honest_items += 1;
        }
    }

    let total_rejected_after_mixed_fill = matches!(
        sender.send(QueuePressurePeer::Honest(4), 40),
        Err(key_fair_queue::Error::Full)
    );

    let mut received = Vec::new();
    while received.len() < 7 {
        match tokio::time::timeout(Duration::from_millis(10), receiver.recv()).await {
            Ok(Some(item)) => received.push(item),
            Ok(None) | Err(_) => break,
        }
    }

    MixedPeerQueuePressureObservation {
        hostile_per_key_rejected,
        total_rejected_after_mixed_fill,
        accepted_honest_items,
        received,
    }
}

pub fn collect_tip5_zset_strings_for_seeds(seeds: &[u64]) -> Result<Vec<String>, String> {
    let (out, _) = collect_tip5_zset_strings_for_seeds_with_elapsed(seeds)?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tip5ZsetTraversalMeasurement {
    pub item_count: usize,
    pub output_count: usize,
    pub elapsed: Duration,
}

pub fn measure_tip5_zset_traversal_for_seeds(
    seeds: &[u64],
) -> Result<Tip5ZsetTraversalMeasurement, String> {
    let (_, measurement) = collect_tip5_zset_strings_for_seeds_with_elapsed(seeds)?;
    Ok(measurement)
}

fn collect_tip5_zset_strings_for_seeds_with_elapsed(
    seeds: &[u64],
) -> Result<(Vec<String>, Tip5ZsetTraversalMeasurement), String> {
    let mut slab = NounSlab::new();
    let zset = tip5_zset(&mut slab, seeds);
    slab.set_root(zset);
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    let mut out = Vec::new();
    let started_at = Instant::now();
    collect_tip5_zset_strings(root.in_space(&space), &mut out).map_err(|err| err.to_string())?;
    let elapsed = started_at.elapsed();
    let measurement = Tip5ZsetTraversalMeasurement {
        item_count: seeds.len(),
        output_count: out.len(),
        elapsed,
    };
    Ok((out, measurement))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeardBlockDecodeMeasurement {
    pub tx_id_count: usize,
    pub decoded_tx_id_count: usize,
    pub jam_bytes: usize,
    pub height: u64,
    pub height_elapsed: Duration,
    pub tx_ids_elapsed: Duration,
}

pub fn measure_realistic_heard_block_decode(
    height: u64,
    tx_seeds: &[u64],
) -> Result<HeardBlockDecodeMeasurement, nockapp::NockAppError> {
    let (_, fact, jam_bytes) = realistic_heard_block_fact_for_height(height, tx_seeds)?;
    let fact_poke = fact.fact_poke();

    let height_started_at = Instant::now();
    let decoded_height = heard_block_height_from_fact_poke(fact_poke)?;
    let height_elapsed = height_started_at.elapsed();

    let tx_ids_started_at = Instant::now();
    let tx_ids = heard_block_tx_ids_from_fact_poke(fact_poke)?;
    let tx_ids_elapsed = tx_ids_started_at.elapsed();

    Ok(HeardBlockDecodeMeasurement {
        tx_id_count: tx_seeds.len(),
        decoded_tx_id_count: tx_ids.len(),
        jam_bytes,
        height: decoded_height,
        height_elapsed,
        tx_ids_elapsed,
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerExclusionProbeOutcome {
    pub address_cooldown_created: bool,
    pub ip_exclusion_created: bool,
    pub fail2ban: bool,
    pub active_ip_exclusions: usize,
    pub active_address_cooldowns: usize,
}

pub struct ReqResIpPeerBanningProbe {
    state: P2PState,
    peer_exclusions: PeerExclusions,
    next_connection_id: usize,
}

impl ReqResIpPeerBanningProbe {
    pub fn new(libp2p_config: LibP2PConfig) -> Self {
        let metrics_registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        let metrics = Arc::new(
            NockchainP2PMetrics::register(&metrics_registry).expect("Could not register metrics"),
        );
        let peer_exclusion_config = libp2p_config
            .peer_exclusion_config()
            .expect("peer exclusion config should be valid in test probe");
        Self {
            state: P2PState::new(metrics, libp2p_config.seen_tx_clear_interval),
            peer_exclusions: PeerExclusions::new(peer_exclusion_config),
            next_connection_id: 1,
        }
    }

    pub fn track_peer_connection(
        &mut self,
        peer_id: PeerId,
        remote_addr: Multiaddr,
    ) -> ConnectionId {
        let connection_id = ConnectionId::new_unchecked(self.next_connection_id);
        self.next_connection_id = self.next_connection_id.saturating_add(1);
        self.state.track_connection(
            connection_id,
            peer_id,
            &remote_addr,
            ConnectedPoint::Listener {
                local_addr: "/ip4/0.0.0.0/tcp/0".parse().expect("valid local addr"),
                send_back_addr: remote_addr.clone(),
            },
        );
        connection_id
    }

    pub fn record_peer_misbehavior(
        &self,
        address: &Multiaddr,
        peer_id: PeerId,
    ) -> PeerExclusionProbeOutcome {
        self.observe_outcome(
            self.peer_exclusions
                .record_peer_misbehavior(address, peer_id),
        )
    }

    pub fn record_peer_request_failure(&self, peer_id: PeerId) -> bool {
        self.peer_exclusions.record_peer_request_failure(peer_id)
    }

    pub fn record_peer_request_success(&self, peer_id: &PeerId) {
        self.peer_exclusions.record_peer_request_success(peer_id);
    }

    pub fn is_peer_request_cooled_down(&self, peer_id: &PeerId) -> bool {
        self.peer_exclusions.is_peer_request_cooled_down(peer_id)
    }

    pub fn is_address_excluded(&self, address: &Multiaddr, expected_peer: Option<PeerId>) -> bool {
        self.peer_exclusions
            .is_address_excluded(address, expected_peer)
    }

    pub fn is_ip_excluded(&self, ip: &IpAddr) -> bool {
        self.peer_exclusions.is_ip_excluded(ip)
    }

    pub fn select_request_peers(&self, target_peers: Vec<PeerId>, limit: usize) -> Vec<PeerId> {
        self.state.select_request_peers_with_preferences(
            target_peers,
            limit,
            &self.peer_exclusions,
            true,
            &[],
        )
    }

    fn observe_outcome(
        &self,
        outcome: crate::ip_block::ExclusionOutcome,
    ) -> PeerExclusionProbeOutcome {
        PeerExclusionProbeOutcome {
            address_cooldown_created: outcome.address_cooldown.is_some(),
            ip_exclusion_created: outcome.ip_exclusion.is_some(),
            fail2ban: outcome
                .ip_exclusion
                .as_ref()
                .is_some_and(|ip_exclusion| ip_exclusion.fail2ban),
            active_ip_exclusions: self.peer_exclusions.active_ip_exclusion_count(),
            active_address_cooldowns: self.peer_exclusions.active_address_cooldown_count(),
        }
    }
}

pub fn first_common_outbound_protocol(
    local: &LibP2PConfig,
    remote: &LibP2PConfig,
) -> Option<String> {
    let remote_inbound = request_response_protocols(remote)
        .into_iter()
        .filter(|(_, support)| support.inbound())
        .map(|(protocol, _)| protocol.as_ref().to_string())
        .collect::<std::collections::BTreeSet<_>>();

    request_response_protocols(local)
        .into_iter()
        .filter(|(_, support)| support.outbound())
        .map(|(protocol, _)| protocol.as_ref().to_string())
        .find(|protocol| remote_inbound.contains(protocol))
}

pub fn request_response_protocol_summary(
    config: &LibP2PConfig,
) -> Vec<ReqResProtocolSupportSummary> {
    request_response_protocols(config)
        .into_iter()
        .map(|(protocol, support)| ReqResProtocolSupportSummary {
            protocol: protocol.as_ref().to_string(),
            inbound: support.inbound(),
            outbound: support.outbound(),
        })
        .collect()
}

pub fn build_req_res_test_swarm(
    libp2p_config: LibP2PConfig,
    keypair: libp2p::identity::Keypair,
    bind: Vec<Multiaddr>,
) -> Result<ReqResTestSwarm, Box<dyn Error>> {
    let (swarm, _raw_request_injection, _raw_response_injection) =
        build_req_res_test_swarm_internal(libp2p_config, keypair, bind, "req-res-test-peer", None)?;
    Ok(swarm)
}

pub fn build_req_res_test_swarm_with_protocol_trace(
    actor: &'static str,
    libp2p_config: LibP2PConfig,
    keypair: libp2p::identity::Keypair,
    bind: Vec<Multiaddr>,
) -> Result<
    (
        ReqResTestSwarm,
        ProtocolTrace,
        RawRequestInjection,
        RawResponseInjection,
    ),
    Box<dyn Error>,
> {
    let trace = ProtocolTrace::default();
    let (swarm, raw_request_injection, raw_response_injection) = build_req_res_test_swarm_internal(
        libp2p_config,
        keypair,
        bind,
        actor,
        Some(trace.clone()),
    )?;
    Ok((swarm, trace, raw_request_injection, raw_response_injection))
}

fn build_req_res_test_swarm_internal(
    libp2p_config: LibP2PConfig,
    keypair: libp2p::identity::Keypair,
    bind: Vec<Multiaddr>,
    actor: &'static str,
    trace: Option<ProtocolTrace>,
) -> Result<(ReqResTestSwarm, RawRequestInjection, RawResponseInjection), Box<dyn Error>> {
    let (resolver_config, resolver_opts) =
        if let Ok(sys) = hickory_resolver::system_conf::read_system_conf() {
            sys
        } else {
            (ResolverConfig::cloudflare(), ResolverOpts::default())
        };

    let max_idle_timeout_millisecs = libp2p_config.max_idle_timeout_millisecs();
    let keep_alive_interval = libp2p_config.keep_alive_interval();
    let handshake_timeout = libp2p_config.handshake_timeout();
    let connection_timeout = libp2p_config.connection_timeout();
    let swarm_idle_timeout = libp2p_config.swarm_idle_timeout();
    let behaviour_config = libp2p_config.clone();
    let request_response_config = request_response::Config::default()
        .with_max_concurrent_streams(libp2p_config.request_response_max_concurrent_streams())
        .with_request_timeout(libp2p_config.request_response_timeout());
    let raw_request_injection = RawRequestInjection::default();
    let raw_request_injection_for_codec = raw_request_injection.clone();
    let raw_response_injection = RawResponseInjection::default();
    let raw_response_injection_for_codec = raw_response_injection.clone();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic_config(|mut cfg| {
            cfg.max_idle_timeout = max_idle_timeout_millisecs;
            cfg.keep_alive_interval = keep_alive_interval;
            cfg.handshake_timeout = handshake_timeout;
            cfg
        })
        .with_dns_config(resolver_config, resolver_opts)
        .with_behaviour(move |_| ReqResTestBehaviour {
            request_response: request_response::Behaviour::with_codec(
                ProtocolRecordingCodec::new(
                    actor,
                    trace.clone(),
                    raw_request_injection_for_codec.clone(),
                    raw_response_injection_for_codec.clone(),
                    &behaviour_config,
                ),
                request_response_protocols(&behaviour_config),
                request_response_config.clone(),
            ),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(swarm_idle_timeout))
        .with_connection_timeout(connection_timeout)
        .build();

    for bind_addr in bind {
        swarm.listen_on(bind_addr)?;
    }

    Ok((swarm, raw_request_injection, raw_response_injection))
}

/// Jam-encode a `[%request [%block [%by-height height]]]` noun into the byte
/// format used by `NockchainRequest::Request { message }`.  This is the same
/// encoding the driver produces for outbound `BlockByHeight` requests.
pub fn jam_block_by_height_request(height: u64) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let by_height_tas = make_tas(&mut slab, "by-height");
    let by_height = T(&mut slab, &[by_height_tas.as_noun(), D(height)]);
    let block_cell = T(&mut slab, &[D(tas!(b"block")), by_height]);
    let request = T(&mut slab, &[D(tas!(b"request")), block_cell]);
    slab.set_root(request);
    slab.jam().as_ref().to_vec()
}

/// Jam-encode a `[%request [%raw-tx [%by-id tx-id]]]` noun.  The `seed` value
/// is expanded into a 5-element tx-id tuple so the message is structurally
/// valid for decode.
pub fn jam_raw_tx_request(seed: u64) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let tx_id = T(
        &mut slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    );
    let by_id = T(&mut slab, &[D(tas!(b"by-id")), tx_id]);
    let raw_tx = T(&mut slab, &[D(tas!(b"raw-tx")), by_id]);
    let request = T(&mut slab, &[D(tas!(b"request")), raw_tx]);
    slab.set_root(request);
    slab.jam().as_ref().to_vec()
}

pub fn jam_block_range_with_txs_request(
    start_height: u64,
    len: u8,
) -> Result<Vec<u8>, nockapp::NockAppError> {
    Ok(block_range_with_txs_request_message(start_height, len)?.into_vec())
}

/// Jam-encode a `[%heard-tx raw-tx]` noun so tests can return a valid
/// singleton `Result` payload for raw-tx request/response flows.
pub fn jam_heard_tx_response(seed: u64, payload_len: usize) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let tx_id = T(
        &mut slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    );
    let payload = Atom::from_value(&mut slab, vec![0xCDu8; payload_len])
        .expect("payload atom should build")
        .as_noun();
    let raw_tx = T(&mut slab, &[tx_id, payload]);
    let response = T(&mut slab, &[D(tas!(b"heard-tx")), raw_tx]);
    slab.set_root(response);
    slab.jam().as_ref().to_vec()
}

fn tip5_tuple(slab: &mut NounSlab, seed: u64) -> nockvm::noun::Noun {
    T(
        slab,
        &[
            D(seed),
            D(seed.saturating_add(1)),
            D(seed.saturating_add(2)),
            D(seed.saturating_add(3)),
            D(seed.saturating_add(4)),
        ],
    )
}

fn tip5_zset(slab: &mut NounSlab, seeds: &[u64]) -> nockvm::noun::Noun {
    seeds.iter().rev().fold(D(0), |tree, seed| {
        let item = tip5_tuple(slab, *seed);
        T(slab, &[item, D(0), tree])
    })
}

pub fn base58_for_tip5_seed(seed: u64) -> String {
    let mut slab = NounSlab::new();
    let noun = tip5_tuple(&mut slab, seed);
    let space = slab.noun_space();
    tip5_hash_to_base58(noun, &space).expect("tip5 tuple should convert to base58")
}

/// Build a synthetic bundled block with the same page shape used by the driver
/// tests. The resulting `block_message` is the jammed page payload, not the
/// outer `%heard-block` fact wrapper.
pub fn bundled_block_for_height(height: u64, tx_seeds: &[u64]) -> BundledBlockWithTxs {
    let (block_id, unincluded_tx_ids, slab) = synthetic_block_page_slab(height, tx_seeds);

    BundledBlockWithTxs {
        block_id,
        block_message: ByteBuf::from(slab.jam().as_ref().to_vec()),
        tx_envelopes: Vec::new(),
        unincluded_tx_ids,
    }
}

pub fn realistic_heard_block_fact_for_height(
    height: u64,
    tx_seeds: &[u64],
) -> Result<(String, NockchainFact, usize), nockapp::NockAppError> {
    let (block_id, _, slab) = synthetic_heard_block_response_slab(height, tx_seeds);
    let fact = NockchainFact::from_owned_noun_slab(slab)?;
    let jam_bytes = fact.fact_poke().jam().as_ref().len();
    Ok((block_id, fact, jam_bytes))
}

fn synthetic_block_page_slab(height: u64, tx_seeds: &[u64]) -> (String, Vec<String>, NounSlab) {
    let mut slab = NounSlab::new();
    let page = synthetic_block_page_noun(&mut slab, height, tx_seeds);
    slab.set_root(page);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();
    (block_id_base58, tx_ids_base58, slab)
}

fn synthetic_heard_block_response_slab(
    height: u64,
    tx_seeds: &[u64],
) -> (String, Vec<String>, NounSlab) {
    let mut slab = NounSlab::new();
    let page = synthetic_block_page_noun(&mut slab, height, tx_seeds);
    let heard_block = make_tas(&mut slab, "heard-block");
    let response = T(&mut slab, &[heard_block.as_noun(), page]);
    slab.set_root(response);

    let block_id_base58 = base58_for_tip5_seed(10_000 + height);
    let tx_ids_base58 = tx_seeds
        .iter()
        .map(|seed| base58_for_tip5_seed(*seed))
        .collect();
    (block_id_base58, tx_ids_base58, slab)
}

fn synthetic_block_page_noun(
    slab: &mut NounSlab,
    height: u64,
    tx_seeds: &[u64],
) -> nockvm::noun::Noun {
    let block_id = tip5_tuple(slab, 10_000 + height);
    let parent_id = tip5_tuple(slab, 20_000 + height);
    let tx_ids = tip5_zset(slab, tx_seeds);
    T(
        slab,
        &[
            D(1),
            block_id,
            D(0),
            parent_id,
            tx_ids,
            D(0),
            D(0),
            D(0),
            D(0),
            D(0),
            D(height),
            D(0),
        ],
    )
}

pub fn validated_batch_response_retry_item_ids(
    request: &NockchainRequest,
    response: &NockchainResponse,
) -> Result<Vec<u32>, nockapp::NockAppError> {
    let NockchainRequest::BatchRequest { items, .. } = request else {
        return Err(nockapp::NockAppError::OtherError(String::from(
            "retry validation helper requires a BatchRequest",
        )));
    };
    let NockchainResponse::BatchResult { results } = response else {
        return Err(nockapp::NockAppError::OtherError(String::from(
            "retry validation helper requires a BatchResult",
        )));
    };

    let mut requests = BTreeMap::new();
    for item in items {
        let data_request = decode_request_item_message(&item.message)?;
        requests.insert(item.item_id, data_request);
    }

    let expected_item_ids = requests.keys().copied().collect::<BTreeSet<_>>();
    let mut observed_item_ids = BTreeSet::new();
    let mut retry_item_ids = BTreeSet::new();

    for result in results {
        let Some(data_request) = requests.get(&result.item_id) else {
            continue;
        };
        observed_item_ids.insert(result.item_id);

        match result.status {
            BatchResultStatus::Result => match result.envelope.as_ref() {
                Some(envelope) => {
                    if crate::driver::validate_response_envelope_for_request(data_request, envelope)
                        .is_err()
                    {
                        retry_item_ids.insert(result.item_id);
                    }
                }
                None => {
                    retry_item_ids.insert(result.item_id);
                }
            },
            BatchResultStatus::Error => match result.error {
                Some(BatchErrorClass::Backpressure | BatchErrorClass::Internal) => {
                    retry_item_ids.insert(result.item_id);
                }
                Some(
                    BatchErrorClass::Decode
                    | BatchErrorClass::TooLarge
                    | BatchErrorClass::InvalidPow,
                )
                | None => {}
            },
            BatchResultStatus::Ack | BatchResultStatus::NotFound => {}
        }
    }

    retry_item_ids.extend(expected_item_ids.difference(&observed_item_ids).copied());
    Ok(retry_item_ids.into_iter().collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnsupportedProtocolFallbackRequest {
    pub item_id: u32,
    pub request: NockchainRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectiveBatchRetryRequest {
    pub item_ids: Vec<u32>,
    pub retry_count: u8,
    pub request: NockchainRequest,
}

/// Build the ordered gen1 singleton requests the driver would queue after a
/// gen2 batch hits `UnsupportedProtocols`.
pub fn build_unsupported_protocol_fallback_requests(
    local_peer_id: &libp2p::PeerId,
    remote_peer_id: &libp2p::PeerId,
    batch_request: NockchainRequest,
) -> Result<Vec<UnsupportedProtocolFallbackRequest>, nockapp::NockAppError> {
    let item_ids = match &batch_request {
        NockchainRequest::BatchRequest { items, .. } => {
            items.iter().map(|item| item.item_id).collect::<Vec<_>>()
        }
        _ => {
            return Err(nockapp::NockAppError::OtherError(String::from(
                "fallback helper requires a BatchRequest",
            )));
        }
    };

    let request_context = OutboundRequestContext::with_attempt(
        *remote_peer_id,
        ReqResGeneration::Gen2,
        batch_request,
        0,
        false,
    );
    let mut equix_builder = equix::EquiXBuilder::new();
    let fallback_contexts = build_unsupported_protocol_fallback_contexts(
        &request_context, local_peer_id, &mut equix_builder,
    )?;

    Ok(item_ids
        .into_iter()
        .zip(fallback_contexts)
        .map(
            |(item_id, fallback_context)| UnsupportedProtocolFallbackRequest {
                item_id,
                request: fallback_context.request,
            },
        )
        .collect())
}

/// Build the selective gen2 retry batch requests the driver would queue after
/// a partial batch response leaves some item_ids retryable or missing.
pub fn build_selective_batch_retry_requests(
    local_peer_id: &libp2p::PeerId,
    remote_peer_id: &libp2p::PeerId,
    batch_request: NockchainRequest,
    retry_count: u8,
    retry_item_ids: &[u32],
) -> Result<Vec<SelectiveBatchRetryRequest>, nockapp::NockAppError> {
    if !matches!(&batch_request, NockchainRequest::BatchRequest { .. }) {
        return Err(nockapp::NockAppError::OtherError(String::from(
            "selective retry helper requires a BatchRequest",
        )));
    }

    let request_context = OutboundRequestContext::with_attempt(
        *remote_peer_id,
        ReqResGeneration::Gen2,
        batch_request,
        retry_count,
        false,
    );
    let retry_item_ids = retry_item_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut equix_builder = equix::EquiXBuilder::new();
    let retry_contexts = build_retry_request_contexts(
        &request_context,
        local_peer_id,
        &mut equix_builder,
        Some(&retry_item_ids),
    )?;

    Ok(retry_contexts
        .into_iter()
        .map(|retry_context| {
            let item_ids = match &retry_context.request {
                NockchainRequest::BatchRequest { items, .. } => {
                    items.iter().map(|item| item.item_id).collect()
                }
                _ => unreachable!("batch retries must stay batched"),
            };
            SelectiveBatchRetryRequest {
                item_ids,
                retry_count: retry_context.retry_count,
                request: retry_context.request,
            }
        })
        .collect())
}

/// Returns `true` when `message` bytes decode to a `BlockByHeight` data
/// request, matching the check the driver uses in `request_is_block_by_height`
/// when applying block-aware gen2 batching rules.
pub fn is_block_by_height_message(message: &[u8]) -> bool {
    let mut slab: NounSlab = NounSlab::new();
    let Ok(noun) = slab.cue_into(Bytes::copy_from_slice(message)) else {
        return false;
    };
    let space = slab.noun_space();
    matches!(
        NockchainDataRequest::from_noun(noun, &space),
        Ok(NockchainDataRequest::BlockByHeight(_))
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnsupportedProtocolFallbackReplay {
    pub fallback_requests: Vec<NockchainRequest>,
    pub fallback_metric_total: u64,
}

/// Build the same ordered gen1 singleton replay that the driver queues after a
/// gen2 `UnsupportedProtocols` failure on a batch request.
pub fn build_unsupported_protocol_fallback_replay(
    local_peer_id: PeerId,
    remote_peer_id: PeerId,
    items: Vec<BatchRequestItem>,
) -> Result<UnsupportedProtocolFallbackReplay, Box<dyn Error>> {
    let request_context = OutboundRequestContext::with_attempt(
        remote_peer_id,
        ReqResGeneration::Gen2,
        NockchainRequest::BatchRequest {
            pow: [0; 16],
            nonce: 0,
            items,
        },
        0,
        false,
    );
    let mut equix_builder = equix::EquiXBuilder::new();
    let fallback_contexts = build_unsupported_protocol_fallback_contexts(
        &request_context, &local_peer_id, &mut equix_builder,
    )?;

    let fallback_metric_total = fallback_contexts.len() as u64;
    let mut fallback_requests = Vec::with_capacity(fallback_contexts.len());
    for fallback_context in fallback_contexts {
        fallback_context
            .request
            .verify_pow(&mut equix_builder, &remote_peer_id, &local_peer_id)?;
        fallback_requests.push(fallback_context.request);
    }

    Ok(UnsupportedProtocolFallbackReplay {
        fallback_requests,
        fallback_metric_total,
    })
}
