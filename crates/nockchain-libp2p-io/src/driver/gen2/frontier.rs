use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use libp2p::PeerId;
use nockapp::noun::slab::NounSlab;
use nockapp::NockAppError;
use nockvm::noun::{NounAllocator, NounHandle};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::Instant;
use tracing::{info, warn};

use crate::driver::gen2::*;
use crate::driver::SwarmActionDispatcher;
use crate::messages::NockchainFact;
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{DeferredFlushEntry, P2PState};
use crate::tip5_util::tip5_hash_to_base58;
use crate::traffic_cop;

const TIP5_ZSET_MAX_ITEMS: usize = 65_536;
const TIP5_ZSET_MAX_STACK: usize = 65_536;

pub(crate) async fn track_future_heard_block_tx_hints(
    peer: PeerId,
    tx_ids: &[String],
    driver_state: &Arc<Mutex<P2PState>>,
) -> Result<usize, NockAppError> {
    let mut state_guard = driver_state.lock().await;
    state_guard.track_tx_ids_and_peer(tx_ids.iter().cloned(), peer);
    Ok(tx_ids.len())
}
pub(crate) fn heard_block_tx_ids_from_fact_poke(
    fact_poke: &NounSlab,
) -> Result<Vec<String>, NockAppError> {
    let space = fact_poke.noun_space();
    let fact = unsafe { *fact_poke.root() }.in_space(&space);
    let fact_cell = fact.as_cell()?;
    if !fact_cell.head().eq_bytes(b"fact") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %fact tag",
        )));
    }

    let response = fact_cell.tail().as_cell()?.tail();
    let response_cell = response.as_cell()?;
    if !response_cell.head().eq_bytes(b"heard-block") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %heard-block tag",
        )));
    }

    let tx_ids = page_tx_ids_noun(response_cell.tail())?;
    let mut tx_id_strings = Vec::new();
    collect_tip5_zset_strings(tx_ids, &mut tx_id_strings)?;
    Ok(tx_id_strings)
}

pub(crate) fn heard_block_height_from_fact_poke(fact_poke: &NounSlab) -> Result<u64, NockAppError> {
    let space = fact_poke.noun_space();
    let fact = unsafe { *fact_poke.root() }.in_space(&space);
    let fact_cell = fact.as_cell()?;
    if !fact_cell.head().eq_bytes(b"fact") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %fact tag",
        )));
    }

    let response = fact_cell.tail().as_cell()?.tail();
    let response_cell = response.as_cell()?;
    if !response_cell.head().eq_bytes(b"heard-block") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %heard-block tag",
        )));
    }

    block_height_from_page_noun(response_cell.tail())
}
pub(crate) fn block_height_from_page_noun(page: NounHandle<'_>) -> Result<u64, NockAppError> {
    let (height_index, version_label) = match page
        .as_cell()
        .ok()
        .and_then(|cell| cell.head().as_atom().ok())
        .and_then(|atom| atom.as_u64().ok())
    {
        Some(1) => (10usize, "v1"),
        Some(version) => {
            return Err(NockAppError::OtherError(format!(
                "unsupported page version {version}",
            )));
        }
        None => (9usize, "v0"),
    };

    let mut list = page;
    for index in 0..=height_index {
        let cell = list.as_cell().map_err(|_| {
            NockAppError::OtherError(format!(
                "page {version_label} missing height field at index {height_index}",
            ))
        })?;
        if index == height_index {
            return cell.head().as_atom()?.as_u64().map_err(Into::into);
        }
        list = cell.tail();
    }

    Err(NockAppError::OtherError(format!(
        "page {version_label} missing height field at index {height_index}",
    )))
}

pub(crate) fn future_heard_block_details(
    response: &NockchainFact,
) -> Result<Option<(String, u64)>, NockAppError> {
    let NockchainFact::HeardBlock(block_id, fact_poke) = response else {
        return Ok(None);
    };

    let height = heard_block_height_from_fact_poke(fact_poke)?;
    Ok(Some((block_id.clone(), height)))
}
/// Owns the drained entries until each route completes. On cancellation,
/// restoration precedes releasing the single-worker flag and rearming.
pub(crate) struct DeferredFlushRecovery {
    state: Arc<Mutex<P2PState>>,
    swarm_tx: mpsc::Sender<SwarmAction>,
    running: Option<Arc<AtomicBool>>,
    pending: std::vec::IntoIter<DeferredFlushEntry>,
    active: Option<DeferredFlushEntry>,
    retirement: Option<oneshot::Receiver<std::time::Duration>>,
    disarmed: bool,
}

impl DeferredFlushRecovery {
    pub(crate) fn new(
        state: Arc<Mutex<P2PState>>,
        swarm_tx: mpsc::Sender<SwarmAction>,
        running: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            state,
            swarm_tx,
            running,
            pending: Vec::new().into_iter(),
            active: None,
            retirement: None,
            disarmed: false,
        }
    }

    async fn restore_pending(&mut self) {
        if self.active.is_none() && self.pending.len() == 0 {
            return;
        }
        let mut state = self.state.lock().await;
        let active = self.active.take();
        let pending = std::mem::replace(&mut self.pending, Vec::new().into_iter());
        state.requeue_deferred_heard_block_entries(active.into_iter().chain(pending));
    }

    pub(crate) async fn finish(&mut self) -> Result<bool, NockAppError> {
        self.restore_pending().await;
        if let Some(running) = self.running.take() {
            running.store(false, Ordering::Release);
        }
        // Recover wakeups coalesced between the final empty take and flag
        // release, including ready entries restored after a route error.
        let ready_remaining = self.state.lock().await.has_ready_deferred_heard_blocks();
        if ready_remaining {
            self.swarm_tx
                .send(SwarmAction::FlushDeferredHeardBlocks)
                .await
                .map_err(|_| {
                    NockAppError::OtherError(String::from("deferred flush queue closed"))
                })?;
        }
        self.disarmed = true;
        Ok(ready_remaining)
    }
}

impl Drop for DeferredFlushRecovery {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        let state = Arc::clone(&self.state);
        let swarm_tx = self.swarm_tx.clone();
        let running = self.running.take();
        let active = self.active.take();
        let pending = std::mem::replace(&mut self.pending, Vec::new().into_iter());
        let retirement = self.retirement.take();
        tokio::spawn(async move {
            // A dispatched consensus poke must retire before its claim is
            // cleared or its retained entry can be restored and rearmed.
            if let Some(retirement) = retirement {
                let _ = retirement.await;
            }
            let ready_remaining = {
                let mut state = state.lock().await;
                if let Some(entry) = active.as_ref() {
                    state.cancel_processing_block(&entry.block_id);
                }
                state.requeue_deferred_heard_block_entries(active.into_iter().chain(pending));
                if let Some(running) = running {
                    running.store(false, Ordering::Release);
                }
                state.has_ready_deferred_heard_blocks()
            };
            if ready_remaining {
                let _ = swarm_tx.send(SwarmAction::FlushDeferredHeardBlocks).await;
            }
        });
    }
}

pub(crate) async fn flush_ready_deferred_heard_blocks_with_dispatcher(
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    _swarm_actions: &mut SwarmActionDispatcher<'_>,
    recovery: &mut DeferredFlushRecovery,
) -> Result<usize, NockAppError> {
    let (ready, frontier_start, backlog_start) = {
        let mut state = driver_state.lock().await;
        let frontier = state.first_negative;
        let backlog = state.deferred_heard_block_total();
        (
            state.take_ready_deferred_heard_block_entries(),
            frontier,
            backlog,
        )
    };
    if ready.is_empty() {
        return Ok(0);
    }
    let ready_len = ready.len();
    recovery.pending = ready.into_iter();
    static FLUSH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let flush_id = FLUSH_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    let flush_start = Instant::now();
    info!(
        target: "nockchain::deferred_flush",
        monotonic_us = traffic_cop::observation_time_us(),
        flush_id, count = ready_len, frontier = frontier_start, backlog = backlog_start,
        "Flushing deferred heard-blocks at current frontier"
    );
    let mut completed = 0usize;
    while let Some(entry) = recovery.pending.next() {
        recovery.active = Some(entry);
        let entry = recovery.active.as_mut().expect("active deferred entry");
        let valid = {
            let state = driver_state.lock().await;
            state.has_deferred_heard_block_lease(entry.height, &entry.block_id)
        };
        if !valid {
            let mut state = driver_state.lock().await;
            state.complete_deferred_heard_block_entry(entry);
            recovery.active.take();
            continue;
        }
        let route_start = Instant::now();
        // A bad page is terminal; retrying the same decode error forever
        // would prevent every later ready entry from reaching its consumer.
        let validation = heard_block_height_from_fact_poke(entry.fact.fact_poke());
        let permanent_error = validation.is_err();
        let result = match validation {
            Ok(_) => {
                super::routing::route_ready_response_fact_with_dispatcher(
                    entry.peer_id,
                    &mut entry.fact,
                    traffic,
                    metrics,
                    driver_state,
                    Some(entry.height),
                    &mut recovery.retirement,
                )
                .await
            }
            Err(error) => Err(error),
        };
        info!(
            target: "nockchain::deferred_flush",
            monotonic_us = traffic_cop::observation_time_us(),
            flush_id, index = completed, peer = %entry.peer_id,
            block_id = %entry.block_id, height = entry.height,
            route_total_ms = route_start.elapsed().as_secs_f64() * 1_000.0,
            outcome = if result.is_ok() { "completed" } else { "error" },
            "Completed deferred heard-block route"
        );
        if result.is_ok() || permanent_error {
            let mut state = driver_state.lock().await;
            state.complete_deferred_heard_block_entry(entry);
            recovery.active.take();
        }
        if let Err(error) = result {
            let restored = recovery.pending.len() + usize::from(recovery.active.is_some());
            recovery.restore_pending().await;
            let state = driver_state.lock().await;
            warn!(
                target: "nockchain::deferred_flush",
                monotonic_us = traffic_cop::observation_time_us(),
                flush_id, count = ready_len, completed, restored,
                failed_retryable = !permanent_error, error = %error,
                frontier = state.first_negative,
                frontier_delta = state.first_negative.saturating_sub(frontier_start),
                backlog = state.deferred_heard_block_total(),
                elapsed_ms = flush_start.elapsed().as_secs_f64() * 1_000.0,
                "Restored deferred heard-blocks after route error"
            );
            return Err(error);
        }
        completed += 1;
    }
    let state = driver_state.lock().await;
    info!(
        target: "nockchain::deferred_flush",
        monotonic_us = traffic_cop::observation_time_us(),
        flush_id, count = ready_len, completed,
        frontier = state.first_negative,
        frontier_delta = state.first_negative.saturating_sub(frontier_start),
        backlog = state.deferred_heard_block_total(),
        elapsed_ms = flush_start.elapsed().as_secs_f64() * 1_000.0,
        "Completed deferred heard-block flush"
    );
    Ok(completed)
}

#[cfg(test)]
pub(crate) async fn flush_ready_deferred_heard_blocks(
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
) -> Result<usize, NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    let mut recovery = DeferredFlushRecovery::new(Arc::clone(driver_state), swarm_tx.clone(), None);
    let result = flush_ready_deferred_heard_blocks_with_dispatcher(
        traffic, metrics, driver_state, &mut swarm_actions, &mut recovery,
    )
    .await;
    recovery.finish().await?;
    result
}

pub(crate) fn page_tx_ids_noun<'a>(page: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let page_cell = page.as_cell()?;
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version != 1 {
                return Err(NockAppError::OtherError(format!(
                    "unsupported page version {version}",
                )));
            }
            Ok(page_cell
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .head())
        }
        Err(_) => Ok(page_cell
            .tail()
            .as_cell()?
            .tail()
            .as_cell()?
            .tail()
            .as_cell()?
            .head()),
    }
}
pub(crate) fn collect_tip5_zset_strings(
    noun: NounHandle<'_>,
    out: &mut Vec<String>,
) -> Result<(), NockAppError> {
    let mut stack = vec![(noun, false)];
    while let Some((node, emit_value)) = stack.pop() {
        if let Ok(atom) = node.as_atom() {
            if atom.as_u64() == Ok(0) {
                continue;
            }
            return Err(NockAppError::OtherError(String::from(
                "unexpected non-zero atom in tx-id set",
            )));
        }

        let cell = node.as_cell()?;
        if emit_value {
            if out.len() >= TIP5_ZSET_MAX_ITEMS {
                return Err(NockAppError::OtherError(format!(
                    "tx-id set exceeded item budget {TIP5_ZSET_MAX_ITEMS}",
                )));
            }
            out.push(tip5_hash_to_base58(
                cell.head().noun(),
                cell.head().space(),
            )?);
            continue;
        }

        if stack.len().saturating_add(3) > TIP5_ZSET_MAX_STACK {
            return Err(NockAppError::OtherError(format!(
                "tx-id set exceeded traversal stack budget {TIP5_ZSET_MAX_STACK}",
            )));
        }
        let branches = cell.tail().as_cell()?;
        stack.push((branches.tail(), false));
        stack.push((node, true));
        stack.push((branches.head(), false));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use nockapp::driver::{IOAction, NockAppHandle, PokeResult};
    use nockapp::nockapp::test::setup_nockapp;
    use nockvm::noun::{D, T};
    use nockvm_macros::tas;

    use super::*;
    use crate::config::LibP2PConfig;
    use crate::p2p_state::BlockSource;
    use crate::tracked_join_set::TrackedJoinSet;

    fn metrics() -> Arc<NockchainP2PMetrics> {
        let registry = gnort::MetricsRegistry::new(gnort::RegistryConfig::default());
        Arc::new(NockchainP2PMetrics::register(&registry).expect("metrics should register"))
    }

    fn fact(id: &str, height: u64) -> NockchainFact {
        let mut slab = NounSlab::new();
        let page = T(
            &mut slab,
            &[D(1), D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(height), D(0)],
        );
        let heard_block = nockapp::utils::make_tas(&mut slab, "heard-block").as_noun();
        let response = T(&mut slab, &[heard_block, page]);
        let poke = T(&mut slab, &[D(tas!(b"fact")), D(0), response]);
        slab.set_root(poke);
        NockchainFact::HeardBlock(id.to_owned(), slab)
    }

    fn malformed(id: &str) -> NockchainFact {
        let mut slab = NounSlab::new();
        slab.set_root(D(0));
        NockchainFact::HeardBlock(id.to_owned(), slab)
    }

    async fn traffic() -> (
        traffic_cop::TrafficCop,
        TrackedJoinSet<Result<(), NockAppError>>,
        mpsc::Receiver<IOAction>,
    ) {
        let (_home, app) = setup_nockapp("test-ker.jam").await;
        let base = app.get_handle();
        let (io_sender, io_receiver) = mpsc::channel(16);
        let handle = NockAppHandle {
            io_sender,
            effect_sender: base.effect_sender.clone(),
            effect_receiver: Mutex::new(base.effect_sender.subscribe()),
            metrics: base.metrics.clone(),
            exit: base.exit.clone(),
        };
        let mut tasks = TrackedJoinSet::new();
        let traffic =
            traffic_cop::TrafficCop::new(handle, &mut tasks, std::time::Duration::from_secs(2));
        (traffic, tasks, io_receiver)
    }

    #[tokio::test]
    async fn malformed_ready_page_keeps_requested_tail_deliverable() {
        let metrics = metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            Arc::clone(&metrics),
            LibP2PConfig::default().seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        {
            let mut state = state.lock().await;
            state.first_negative = 3;
            assert!(state.defer_heard_block(peer, 1, "bad".to_owned(), malformed("bad")));
            assert!(state.defer_heard_block_with_source(
                peer,
                2,
                "requested".to_owned(),
                fact("requested", 2),
                BlockSource::Prefetch,
            ));
            state.note_kernel_block_height_requested(2);
            assert!(state.defer_heard_block(peer, 3, "later".to_owned(), fact("later", 3)));
        }
        let (traffic, _tasks, mut kernel) = traffic().await;
        let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
        assert!(
            flush_ready_deferred_heard_blocks(&traffic, &metrics, &state, &swarm_tx)
                .await
                .is_err()
        );
        {
            let state = state.lock().await;
            assert_eq!(state.deferred_heard_block_total(), 2);
            assert!(!state.has_deferred_block_at_height(1));
            assert!(state.has_ready_deferred_block_at_height(2));
            assert!(state.has_kernel_block_height_request(2));
            assert!(state.has_ready_deferred_block_at_height(3));
        }
        assert!(matches!(
            swarm_rx.recv().await,
            Some(SwarmAction::FlushDeferredHeardBlocks)
        ));
        let consumer = tokio::spawn(async move {
            let mut delivered = Vec::new();
            for _ in 0..2 {
                match kernel
                    .recv()
                    .await
                    .expect("tail should reach kernel consumer")
                {
                    IOAction::Poke {
                        poke, ack_channel, ..
                    } => {
                        delivered
                            .push(heard_block_height_from_fact_poke(&poke).expect("page height"));
                        ack_channel
                            .send(PokeResult::Ack)
                            .expect("route should await result");
                    }
                    IOAction::Peek { .. } => panic!("tail delivery should poke"),
                }
            }
            delivered
        });
        assert_eq!(
            flush_ready_deferred_heard_blocks(&traffic, &metrics, &state, &swarm_tx)
                .await
                .expect("restored tail should drain"),
            2,
        );
        assert_eq!(consumer.await.expect("consumer should finish"), vec![2, 3]);
        assert_eq!(state.lock().await.deferred_heard_block_total(), 0);
    }

    async fn cancelled_dispatched_route_waits_for_retirement(kernel_error: bool) {
        let metrics = metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            Arc::clone(&metrics),
            LibP2PConfig::default().seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        {
            let mut state = state.lock().await;
            state.first_negative = 2;
            assert!(state.defer_heard_block(peer, 1, "active".to_owned(), fact("active", 1)));
            assert!(state.defer_heard_block(peer, 2, "tail".to_owned(), fact("tail", 2)));
        }
        let (traffic, _tasks, mut kernel) = traffic().await;
        let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
        let running = Arc::new(AtomicBool::new(true));
        let mut recovery = DeferredFlushRecovery::new(
            Arc::clone(&state),
            swarm_tx.clone(),
            Some(Arc::clone(&running)),
        );
        let worker_state = Arc::clone(&state);
        let worker = tokio::spawn(async move {
            let mut actions = SwarmActionDispatcher::Channel(&swarm_tx);
            flush_ready_deferred_heard_blocks_with_dispatcher(
                &traffic, &metrics, &worker_state, &mut actions, &mut recovery,
            )
            .await
        });
        let ack_channel = match kernel
            .recv()
            .await
            .expect("active route should reach consumer")
        {
            IOAction::Poke { ack_channel, .. } => ack_channel,
            IOAction::Peek { .. } => panic!("active route should poke"),
        };
        let state_guard = state.lock().await;
        assert!(state_guard.is_processing_block("active"));
        worker.abort();
        assert!(worker
            .await
            .expect_err("worker should be cancelled")
            .is_cancelled());
        assert!(
            running.load(Ordering::Acquire),
            "a dispatched route retains the worker flag"
        );
        drop(state_guard);
        assert!(
            running.load(Ordering::Acquire),
            "kernel retirement precedes restoration"
        );
        if kernel_error {
            drop(ack_channel);
        } else {
            ack_channel
                .send(PokeResult::Nack)
                .expect("dispatched poke should still await kernel result");
        }
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), swarm_rx.recv())
                .await
                .expect("cancelled worker should rearm"),
            Some(SwarmAction::FlushDeferredHeardBlocks),
        ));
        let state_guard = state.lock().await;
        assert!(!running.load(Ordering::Acquire));
        assert_eq!(state_guard.deferred_heard_block_total(), 2);
        assert!(state_guard.has_ready_deferred_block_at_height(1));
        assert!(state_guard.has_ready_deferred_block_at_height(2));
        assert!(!state_guard.is_processing_block("active"));
        drop(state_guard);
    }

    #[tokio::test]
    async fn cancelled_route_restores_active_and_tail_after_dispatched_nack() {
        cancelled_dispatched_route_waits_for_retirement(false).await;
    }

    #[tokio::test]
    async fn cancelled_route_restores_active_and_tail_after_dispatched_error() {
        cancelled_dispatched_route_waits_for_retirement(true).await;
    }

    async fn cancelled_queued_route_keeps_active_and_tail_deliverable(blocking_error: bool) {
        use std::future::{poll_fn, Future};
        use std::task::Poll;

        use nockapp::wire::{SystemWire, Wire};

        let metrics = metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            Arc::clone(&metrics),
            LibP2PConfig::default().seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        {
            let mut state = state.lock().await;
            state.first_negative = 2;
            assert!(state.defer_heard_block(peer, 1, "active".to_owned(), fact("active", 1)));
            assert!(state.defer_heard_block(peer, 2, "tail".to_owned(), fact("tail", 2)));
        }
        let (traffic, _tasks, mut kernel) = traffic().await;
        let blocking_traffic = traffic.clone();
        let blocking = tokio::spawn(async move {
            let mut cause = NounSlab::new();
            cause.set_root(D(0));
            blocking_traffic
                .poke_high_priority(
                    None,
                    SystemWire.to_wire(),
                    cause,
                    Box::pin(async { true }),
                    None,
                )
                .await
        });
        let ack_channel = match kernel
            .recv()
            .await
            .expect("blocking poke should reach kernel")
        {
            IOAction::Poke { ack_channel, .. } => ack_channel,
            IOAction::Peek { .. } => panic!("blocking operation should poke"),
        };
        let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
        let running = Arc::new(AtomicBool::new(true));
        let mut recovery = DeferredFlushRecovery::new(
            Arc::clone(&state),
            swarm_tx.clone(),
            Some(Arc::clone(&running)),
        );
        let worker_state = Arc::clone(&state);
        let worker_traffic = traffic.clone();
        let worker_metrics = Arc::clone(&metrics);
        let worker_tx = swarm_tx.clone();
        let mut worker = Box::pin(async move {
            let mut actions = SwarmActionDispatcher::Channel(&worker_tx);
            flush_ready_deferred_heard_blocks_with_dispatcher(
                &worker_traffic, &worker_metrics, &worker_state, &mut actions, &mut recovery,
            )
            .await
        });
        // All state locks are free. Polling until Pending queues the actual
        // deferred route behind the kernel operation held by this consumer.
        poll_fn(|cx| {
            assert!(worker.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        {
            let state = state.lock().await;
            assert!(!state.has_ready_deferred_heard_blocks());
            assert!(state.has_deferred_heard_block_lease(1, "active"));
            assert!(!state.is_processing_block("active"));
        }
        drop(worker);
        assert!(running.load(Ordering::Acquire));
        assert!(
            swarm_rx.try_recv().is_err(),
            "cancelled queued work has not retired yet"
        );
        if blocking_error {
            drop(ack_channel);
            assert!(blocking
                .await
                .expect("blocking task should finish")
                .is_err());
        } else {
            ack_channel
                .send(PokeResult::Nack)
                .expect("blocking consumer should still await result");
            assert!(matches!(
                blocking.await.expect("blocking task should finish"),
                Ok(PokeResult::Nack)
            ));
        }
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), swarm_rx.recv())
                .await
                .expect("retired queued route should rearm"),
            Some(SwarmAction::FlushDeferredHeardBlocks),
        ));
        assert!(
            swarm_rx.try_recv().is_err(),
            "cancellation should rearm one worker"
        );
        {
            let state = state.lock().await;
            assert!(!running.load(Ordering::Acquire));
            assert!(!state.is_processing_block("active"));
            assert!(state.has_ready_deferred_block_at_height(1));
            assert!(state.has_ready_deferred_block_at_height(2));
        }
        let consumer = tokio::spawn(async move {
            let mut delivered = Vec::new();
            for _ in 0..2 {
                match kernel
                    .recv()
                    .await
                    .expect("restored page should reach consumer")
                {
                    IOAction::Poke {
                        poke, ack_channel, ..
                    } => {
                        delivered
                            .push(heard_block_height_from_fact_poke(&poke).expect("page height"));
                        ack_channel
                            .send(PokeResult::Ack)
                            .expect("restored route should await result");
                    }
                    IOAction::Peek { .. } => panic!("restored page should poke"),
                }
            }
            delivered
        });
        assert_eq!(
            flush_ready_deferred_heard_blocks(&traffic, &metrics, &state, &swarm_tx)
                .await
                .expect("restored routes should complete"),
            2,
        );
        assert_eq!(consumer.await.expect("consumer should finish"), vec![1, 2]);
        let state = state.lock().await;
        assert_eq!(state.deferred_heard_block_total(), 0);
        assert!(!state.is_processing_block("active"));
        assert!(!state.is_processing_block("tail"));
    }

    #[tokio::test]
    async fn cancelled_queued_route_is_deliverable_after_blocking_nack_without_seen() {
        cancelled_queued_route_keeps_active_and_tail_deliverable(false).await;
    }

    #[tokio::test]
    async fn cancelled_queued_route_is_deliverable_after_blocking_error_without_seen() {
        cancelled_queued_route_keeps_active_and_tail_deliverable(true).await;
    }

    async fn invalidated_leased_material_is_not_replayed(peer_disconnect: bool) {
        use libp2p::core::ConnectedPoint;
        use libp2p::swarm::ConnectionId;
        use libp2p::Multiaddr;

        let metrics = metrics();
        let state = Arc::new(Mutex::new(P2PState::new(
            Arc::clone(&metrics),
            LibP2PConfig::default().seen_tx_clear_interval,
        )));
        let peer = PeerId::random();
        let address: Multiaddr = "/ip4/127.0.0.1/udp/4611/quic-v1"
            .parse()
            .expect("fixture endpoint");
        {
            let mut state = state.lock().await;
            state.track_connection(
                ConnectionId::new_unchecked(71),
                peer,
                &address,
                ConnectedPoint::Listener {
                    local_addr: address.clone(),
                    send_back_addr: address.clone(),
                },
            );
            state.first_negative = 2;
            assert!(state.defer_heard_block(peer, 1, "active".to_owned(), fact("active", 1)));
            assert!(state.defer_heard_block(peer, 2, "discarded".to_owned(), fact("discarded", 2)));
        }
        let (traffic, _tasks, mut kernel) = traffic().await;
        let (swarm_tx, mut swarm_rx) = mpsc::channel(8);
        let running = Arc::new(AtomicBool::new(true));
        let mut recovery = DeferredFlushRecovery::new(
            Arc::clone(&state),
            swarm_tx.clone(),
            Some(Arc::clone(&running)),
        );
        let worker_state = Arc::clone(&state);
        let worker_traffic = traffic.clone();
        let worker_metrics = Arc::clone(&metrics);
        let worker_tx = swarm_tx.clone();
        let worker = tokio::spawn(async move {
            let mut actions = SwarmActionDispatcher::Channel(&worker_tx);
            flush_ready_deferred_heard_blocks_with_dispatcher(
                &worker_traffic, &worker_metrics, &worker_state, &mut actions, &mut recovery,
            )
            .await
        });
        let ack_channel = match kernel
            .recv()
            .await
            .expect("active page should reach consumer")
        {
            IOAction::Poke { ack_channel, .. } => ack_channel,
            IOAction::Peek { .. } => panic!("page delivery should poke"),
        };
        {
            let mut state = state.lock().await;
            if peer_disconnect {
                state.lost_connection(ConnectionId::new_unchecked(71));
                state.track_connection(
                    ConnectionId::new_unchecked(72),
                    peer,
                    &address,
                    ConnectedPoint::Listener {
                        local_addr: address.clone(),
                        send_back_addr: address.clone(),
                    },
                );
                assert!(!state.has_deferred_heard_block_lease(1, "active"));
            } else {
                assert!(state.invalidate_deferred_block_id("discarded"));
                assert!(state.has_deferred_heard_block_lease(1, "active"));
            }
            assert!(!state.has_deferred_heard_block_lease(2, "discarded"));
            assert_eq!(
                state.deferred_heard_block_total(),
                2,
                "worker still owns both slabs"
            );
        }
        worker.abort();
        assert!(worker
            .await
            .expect_err("worker should be cancelled")
            .is_cancelled());
        assert!(running.load(Ordering::Acquire));
        assert!(swarm_rx.try_recv().is_err());
        // Exercise an actual failed kernel reply without any seen effect.
        drop(ack_channel);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while running.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("retired action should release cancellation guard");
        {
            let mut state = state.lock().await;
            assert!(!state.has_deferred_block_at_height(2));
            assert!(!state.is_processing_block("active"));
            if peer_disconnect {
                assert_eq!(state.deferred_heard_block_total(), 0);
                assert!(!state.has_ready_deferred_heard_blocks());
                assert!(
                    swarm_rx.try_recv().is_err(),
                    "discarded session must not rearm"
                );
                assert!(state.defer_heard_block(peer, 1, "fresh".to_owned(), fact("fresh", 1)));
            } else {
                assert_eq!(state.deferred_heard_block_total(), 1);
                assert!(state.has_ready_deferred_block_at_height(1));
            }
        }
        if !peer_disconnect {
            assert!(matches!(
                swarm_rx.recv().await,
                Some(SwarmAction::FlushDeferredHeardBlocks)
            ));
            assert!(swarm_rx.try_recv().is_err());
        }
        let consumer = tokio::spawn(async move {
            match kernel
                .recv()
                .await
                .expect("remaining valid page should reach consumer")
            {
                IOAction::Poke {
                    poke, ack_channel, ..
                } => {
                    let height = heard_block_height_from_fact_poke(&poke).expect("page height");
                    ack_channel
                        .send(PokeResult::Ack)
                        .expect("replacement route should await result");
                    assert!(
                        kernel.try_recv().is_err(),
                        "discarded tail must not be delivered"
                    );
                    height
                }
                IOAction::Peek { .. } => panic!("page delivery should poke"),
            }
        });
        assert_eq!(
            flush_ready_deferred_heard_blocks(&traffic, &metrics, &state, &swarm_tx)
                .await
                .expect("remaining valid material should drain"),
            1,
        );
        assert_eq!(consumer.await.expect("consumer should finish"), 1);
        assert_eq!(state.lock().await.deferred_heard_block_total(), 0);
    }

    #[tokio::test]
    async fn disconnected_and_reconnected_supplier_does_not_resurrect_cancelled_lease() {
        invalidated_leased_material_is_not_replayed(true).await;
    }

    #[tokio::test]
    async fn invalidated_tail_is_not_restored_after_cancelled_kernel_error() {
        invalidated_leased_material_is_not_replayed(false).await;
    }
}
