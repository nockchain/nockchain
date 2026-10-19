use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::PeerId;
use nockapp::driver::{NockAppHandle, PokeResult};
use nockapp::noun::slab::NounSlab;
use nockapp::wire::WireRepr;
use nockapp::NockAppError;
use tokio::select;
use tokio::sync::oneshot;
use tracing::{debug, error, trace, warn};

use crate::key_fair_queue;
use crate::tracked_join_set::TrackedJoinSet;

const PEER_HIGH_BURST_BEFORE_LOW: usize = 8;
const HIGH_PRIORITY_QUEUE_MAX_TOTAL: usize = 16_384;
const HIGH_PRIORITY_QUEUE_MAX_PER_KEY: usize = 512;
const LOW_PRIORITY_QUEUE_MAX_TOTAL: usize = 8_192;
const LOW_PRIORITY_QUEUE_MAX_PER_KEY: usize = 256;

/// A shared process-local monotonic clock for native service, queue and flush events.
///
/// Values share one epoch within this process only, not across processes or restarts.
pub fn observation_time_us() -> u64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_micros() as u64
}

#[derive(Clone, Copy)]
struct DispatchContext {
    sequence: u64,
    priority: &'static str,
    peer: Option<PeerId>,
    peer_high_since_low: usize,
}

fn log_kernel_service(
    dispatch: DispatchContext,
    operation: &'static str,
    queued_at: Instant,
    started_at: Instant,
    outcome: &'static str,
) {
    debug!(
        target: "nockchain::kernel_queue",
        monotonic_us = observation_time_us(),
        dispatch_seq = dispatch.sequence,
        priority = dispatch.priority,
        peer = ?dispatch.peer,
        peer_high_since_low = dispatch.peer_high_since_low,
        operation,
        queue_wait_ms = started_at.duration_since(queued_at).as_secs_f64() * 1_000.0,
        kernel_ms = started_at.elapsed().as_secs_f64() * 1_000.0,
        outcome,
        "Completed queued kernel operation"
    );
}

/// Timestamp (seconds since UNIX epoch) of the last successful TrafficCop
/// kernel operation. Bumped after high/low-priority pokes return from
/// `handle.poke` and after low-priority peeks return from `handle.peek`. The
/// watchdog compares kernel-completion progress with the independent driver
/// heartbeat and dumps thread stacks if progress stops. Consensus pokes have
/// no wall-clock timeout, so slow validation is not converted into a Nack.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

enum TrafficCopAction {
    Poke(TrafficCopPoke),
    Peek {
        path: NounSlab,
        queued_at: Instant,
        result: oneshot::Sender<Result<Option<NounSlab>, NockAppError>>,
    },
}

struct TrafficCopPoke {
    queued_at: Instant,
    wire: WireRepr,
    cause: NounSlab,
    timing: Option<oneshot::Sender<Duration>>,
    enable: Pin<Box<dyn Future<Output = bool> + Send>>,
    result: oneshot::Sender<Result<PokeResult, NockAppError>>,
}

#[derive(Clone)]
pub(crate) struct TrafficCop {
    system_high_priority_pokes: key_fair_queue::Sender<(), TrafficCopPoke>,
    peer_high_priority_pokes: key_fair_queue::Sender<PeerId, TrafficCopPoke>,
    low_priority: key_fair_queue::Sender<Option<PeerId>, TrafficCopAction>,
    /// Unix timestamp of the last successful TrafficCop kernel operation.
    /// See `unix_now` docstring above. Exposed via `last_poke_completed_at()`
    /// for libp2p-watchdog stall detection independent of the driver
    /// heartbeat. The reader (`spawn_deadlock_watchdog`) is Linux-only; on
    /// other platforms the field is still populated but goes unread, suppress
    /// the dead-code warning rather than cfg-gate the whole struct.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    last_poke_completed_at: Arc<AtomicU64>,
}

impl TrafficCop {
    #[cfg(test)]
    pub(crate) fn new(
        handle: NockAppHandle,
        join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
        peek_timeout: Duration,
    ) -> Self {
        Self::new_with_peek_timeout(handle, join_set, peek_timeout)
    }

    pub(crate) fn new_with_peek_timeout(
        handle: NockAppHandle,
        join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
        peek_timeout: Duration,
    ) -> Self {
        let (system_high_priority_pokes, system_high) = key_fair_queue::channel_with_limits(
            HIGH_PRIORITY_QUEUE_MAX_TOTAL, HIGH_PRIORITY_QUEUE_MAX_PER_KEY,
        );
        let (peer_high_priority_pokes, peer_high) = key_fair_queue::channel_with_limits(
            HIGH_PRIORITY_QUEUE_MAX_TOTAL, HIGH_PRIORITY_QUEUE_MAX_PER_KEY,
        );
        let (low_priority, low) = key_fair_queue::channel_with_limits(
            LOW_PRIORITY_QUEUE_MAX_TOTAL, LOW_PRIORITY_QUEUE_MAX_PER_KEY,
        );
        let last_poke_completed_at = Arc::new(AtomicU64::new(unix_now()));
        join_set.spawn(
            "traffic_cop".to_string(),
            traffic_cop_task(
                handle,
                system_high,
                peer_high,
                low,
                peek_timeout,
                last_poke_completed_at.clone(),
            ),
        );
        Self {
            system_high_priority_pokes,
            peer_high_priority_pokes,
            low_priority,
            last_poke_completed_at,
        }
    }

    /// Hand to the watchdog thread for kernel-side liveness tracking separate
    /// from the driver heartbeat. Returns the seconds-since-epoch of the last
    /// observed successful TrafficCop kernel operation. Only called on Linux
    /// (where the watchdog thread exists); marked because the dead-code lint
    /// would otherwise fire on other platforms.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn last_poke_completed_at(&self) -> Arc<AtomicU64> {
        self.last_poke_completed_at.clone()
    }

    /// enable: Future which is polled just prior to poking, intended to allow checking block/tx caches
    pub(crate) async fn poke_high_priority(
        &self,
        peer_id: Option<PeerId>,
        wire: WireRepr,
        cause: NounSlab,
        enable: Pin<Box<dyn Future<Output = bool> + Send>>,
        timing: Option<oneshot::Sender<std::time::Duration>>,
    ) -> Result<PokeResult, NockAppError> {
        let (result_tx, result_rx) = oneshot::channel();
        let queued_at = Instant::now();
        let action = TrafficCopPoke {
            queued_at,
            wire,
            cause,
            timing,
            enable,
            result: result_tx,
        };
        match peer_id {
            Some(peer_id) => self
                .peer_high_priority_pokes
                .send(peer_id, action)
                .map_err(key_fair_queue_error_to_nockapp)?,
            None => self
                .system_high_priority_pokes
                .send((), action)
                .map_err(key_fair_queue_error_to_nockapp)?,
        }
        debug!(
            target: "nockchain::kernel_queue",
            monotonic_us = observation_time_us(),
            priority = if peer_id.is_some() { "peer_high" } else { "system_high" },
            peer = ?peer_id,
            operation = "poke",
            "Queued kernel operation"
        );
        result_rx.await?
    }

    #[allow(dead_code)]
    pub(crate) async fn poke_low_priority(
        &self,
        peer_id: Option<PeerId>,
        wire: WireRepr,
        cause: NounSlab,
        enable: Pin<Box<dyn Future<Output = bool> + Send>>,
        timing: Option<oneshot::Sender<std::time::Duration>>,
    ) -> Result<PokeResult, NockAppError> {
        let (result_tx, result_rx) = oneshot::channel();
        let queued_at = Instant::now();
        let action = TrafficCopAction::Poke(TrafficCopPoke {
            queued_at,
            wire,
            cause,
            timing,
            enable,
            result: result_tx,
        });
        self.low_priority
            .send(peer_id, action)
            .map_err(key_fair_queue_error_to_nockapp)?;
        debug!(
            target: "nockchain::kernel_queue",
            monotonic_us = observation_time_us(),
            priority = "low",
            peer = ?peer_id,
            operation = "poke",
            "Queued kernel operation"
        );
        result_rx.await?
    }

    pub(crate) async fn peek(
        &self,
        peer_id: Option<PeerId>,
        path: NounSlab,
    ) -> Result<Option<NounSlab>, NockAppError> {
        let (result_tx, result_rx) = oneshot::channel();
        let action = TrafficCopAction::Peek {
            path,
            queued_at: Instant::now(),
            result: result_tx,
        };
        self.low_priority
            .send(peer_id, action)
            .map_err(key_fair_queue_error_to_nockapp)?;
        debug!(
            target: "nockchain::kernel_queue",
            monotonic_us = observation_time_us(),
            priority = "low",
            peer = ?peer_id,
            operation = "peek",
            "Queued kernel operation"
        );
        result_rx.await?
    }
}

fn key_fair_queue_error_to_nockapp<K>(error: key_fair_queue::Error<K>) -> NockAppError {
    match error {
        key_fair_queue::Error::SendError(_) => NockAppError::ChannelClosedError,
        key_fair_queue::Error::Full => {
            NockAppError::OtherError(String::from("traffic cop queue is full"))
        }
    }
}

async fn traffic_cop_task(
    handle: NockAppHandle,
    mut system_high: key_fair_queue::Receiver<(), TrafficCopPoke>,
    mut peer_high: key_fair_queue::Receiver<PeerId, TrafficCopPoke>,
    mut low: key_fair_queue::Receiver<Option<PeerId>, TrafficCopAction>,
    peek_timeout: Duration,
    last_poke_completed_at: Arc<AtomicU64>,
) -> Result<(), NockAppError> {
    let mut system_high_open = true;
    let mut peer_high_open = true;
    let mut low_open = true;
    let mut peer_high_since_low = 0usize;
    let mut dispatch_sequence = 0u64;
    loop {
        if !system_high_open && !peer_high_open && !low_open {
            error!("Traffic cop channels closed");
            break Err(NockAppError::ChannelClosedError);
        }

        // System work does not replenish the peer burst allowance. Once it
        // is spent, queued low-priority work precedes both high queues.
        if peer_high_since_low >= PEER_HIGH_BURST_BEFORE_LOW {
            select! { biased;
                low_priority_action = low.recv(), if low_open => match low_priority_action {
                    Some((peer, action)) => {
                        dispatch_sequence += 1;
                        let dispatch = DispatchContext {
                            sequence: dispatch_sequence,
                            priority: "low",
                            peer,
                            peer_high_since_low,
                        };
                        peer_high_since_low = 0;
                        process_low_priority_action(
                            &handle, peek_timeout, action, dispatch, &last_poke_completed_at,
                        ).await;
                    }
                    None => low_open = false,
                },
                system_high_priority_poke = system_high.recv(), if system_high_open => match system_high_priority_poke {
                    Some((_, poke)) => {
                        dispatch_sequence += 1;
                        process_poke(
                            &handle, poke,
                            DispatchContext {
                                sequence: dispatch_sequence, priority: "system_high",
                                peer: None, peer_high_since_low,
                            },
                            &last_poke_completed_at,
                        ).await;
                    }
                    None => system_high_open = false,
                },
                peer_high_priority_poke = peer_high.recv(), if peer_high_open => match peer_high_priority_poke {
                    Some((peer, poke)) => {
                        dispatch_sequence += 1;
                        peer_high_since_low = peer_high_since_low.saturating_add(1);
                        process_poke(
                            &handle, poke,
                            DispatchContext {
                                sequence: dispatch_sequence, priority: "peer_high",
                                peer: Some(peer), peer_high_since_low,
                            },
                            &last_poke_completed_at,
                        ).await;
                    }
                    None => peer_high_open = false,
                },
                _ = handle.next_effect() => {
                    // Drain the broadcast channel so it does not lag.
                }
            }
        } else {
            select! { biased;
                system_high_priority_poke = system_high.recv(), if system_high_open => match system_high_priority_poke {
                    Some((_, poke)) => {
                        dispatch_sequence += 1;
                        process_poke(
                            &handle, poke,
                            DispatchContext {
                                sequence: dispatch_sequence, priority: "system_high",
                                peer: None, peer_high_since_low,
                            },
                            &last_poke_completed_at,
                        ).await;
                    }
                    None => system_high_open = false,
                },
                peer_high_priority_poke = peer_high.recv(), if peer_high_open => match peer_high_priority_poke {
                    Some((peer, poke)) => {
                        dispatch_sequence += 1;
                        peer_high_since_low = peer_high_since_low.saturating_add(1);
                        process_poke(
                            &handle, poke,
                            DispatchContext {
                                sequence: dispatch_sequence, priority: "peer_high",
                                peer: Some(peer), peer_high_since_low,
                            },
                            &last_poke_completed_at,
                        ).await;
                    }
                    None => peer_high_open = false,
                },
                low_priority_action = low.recv(), if low_open => match low_priority_action {
                    Some((peer, action)) => {
                        dispatch_sequence += 1;
                        let dispatch = DispatchContext {
                            sequence: dispatch_sequence, priority: "low", peer,
                            peer_high_since_low,
                        };
                        peer_high_since_low = 0;
                        process_low_priority_action(
                            &handle, peek_timeout, action, dispatch, &last_poke_completed_at,
                        ).await;
                    }
                    None => low_open = false,
                },
                _ = handle.next_effect() => {
                    // Drain the broadcast channel so it does not lag.
                }
            }
        }
    }
}

async fn process_poke(
    handle: &NockAppHandle,
    TrafficCopPoke {
        queued_at,
        wire,
        cause,
        timing,
        enable,
        result,
    }: TrafficCopPoke,
    dispatch: DispatchContext,
    last_poke_completed_at: &Arc<AtomicU64>,
) {
    if result.is_closed() {
        if let Some(timing) = timing {
            let _ = timing.send(Duration::ZERO);
        }
        log_kernel_service(dispatch, "poke", queued_at, Instant::now(), "cancelled");
        return;
    }
    let enabled = enable.await;
    let started_at = Instant::now();
    debug!(
        target: "nockchain::kernel_queue",
        monotonic_us = observation_time_us(),
        dispatch_seq = dispatch.sequence,
        priority = dispatch.priority,
        peer = ?dispatch.peer,
        peer_high_since_low = dispatch.peer_high_since_low,
        operation = "poke",
        queue_wait_ms = started_at.duration_since(queued_at).as_secs_f64() * 1_000.0,
        enabled,
        "Dispatching queued kernel operation"
    );
    if !enabled {
        trace!(
            priority = dispatch.priority,
            had_timing_channel = timing.is_some(),
            "Traffic cop gated poke before dispatch"
        );
        if let Some(timing) = timing {
            let _ = timing.send(Duration::ZERO);
        }
        let _ = result.send(Ok(PokeResult::Nack)).inspect_err(|_e| {
            error!(priority = dispatch.priority, "Failed to send poke result");
        });
        // A gated Nack is not a kernel round trip.
        log_kernel_service(dispatch, "poke", queued_at, started_at, "gated");
        return;
    }
    if result.is_closed() {
        if let Some(timing) = timing {
            let _ = timing.send(Duration::ZERO);
        }
        log_kernel_service(dispatch, "poke", queued_at, started_at, "cancelled");
        return;
    }

    // No wall-clock timeout: slow consensus validation runs to completion.
    let res = handle.poke(wire, cause).await;
    last_poke_completed_at.store(unix_now(), Ordering::Relaxed);
    let outcome = match &res {
        Ok(PokeResult::Ack) => "ack",
        Ok(PokeResult::Nack) => "nack",
        Err(_) => "error",
    };
    log_kernel_service(dispatch, "poke", queued_at, started_at, outcome);
    if let Some(timing) = timing {
        let _ = timing.send(started_at.elapsed());
    }
    let _ = result.send(res).inspect_err(|_e| {
        error!(priority = dispatch.priority, "Failed to send poke result");
    });
}

async fn process_low_priority_action(
    handle: &NockAppHandle,
    peek_timeout: Duration,
    action: TrafficCopAction,
    dispatch: DispatchContext,
    last_poke_completed_at: &Arc<AtomicU64>,
) {
    match action {
        TrafficCopAction::Poke(poke) => {
            process_poke(handle, poke, dispatch, last_poke_completed_at).await;
        }
        TrafficCopAction::Peek {
            path,
            queued_at,
            result,
        } => {
            let started_at = Instant::now();
            debug!(
                target: "nockchain::kernel_queue",
                monotonic_us = observation_time_us(),
                dispatch_seq = dispatch.sequence,
                priority = dispatch.priority,
                peer = ?dispatch.peer,
                peer_high_since_low = dispatch.peer_high_since_low,
                operation = "peek",
                queue_wait_ms = started_at.duration_since(queued_at).as_secs_f64() * 1_000.0,
                "Dispatching queued kernel operation"
            );
            let res = match tokio::time::timeout(peek_timeout, handle.peek(path)).await {
                Ok(res) => {
                    last_poke_completed_at.store(unix_now(), Ordering::Relaxed);
                    res
                }
                Err(_) => {
                    warn!(
                        timeout_ms = peek_timeout.as_secs_f64() * 1_000.0,
                        "Low priority peek timed out"
                    );
                    Err(NockAppError::Timeout)
                }
            };
            let outcome = match &res {
                Ok(Some(_)) => "some",
                Ok(None) => "none",
                Err(NockAppError::Timeout) => "timeout",
                Err(_) => "error",
            };
            log_kernel_service(dispatch, "peek", queued_at, started_at, outcome);
            let _ = result.send(res).inspect_err(|_e| {
                error!("Failed to send low priority peek result");
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use nockapp::driver::IOAction;
    use nockapp::nockapp::test::setup_nockapp;
    use nockapp::wire::{SystemWire, Wire};
    use nockvm::noun::{NounAllocator, D};

    use super::*;

    fn slab(value: u64) -> NounSlab {
        let mut slab = NounSlab::new();
        slab.set_root(D(value));
        slab
    }

    fn poke(
        value: u64,
    ) -> (
        TrafficCopPoke,
        oneshot::Receiver<Result<PokeResult, NockAppError>>,
    ) {
        let (result, receiver) = oneshot::channel();
        (
            TrafficCopPoke {
                queued_at: Instant::now(),
                wire: SystemWire.to_wire(),
                cause: slab(value),
                timing: None,
                enable: Box::pin(async { true }),
                result,
            },
            receiver,
        )
    }

    async fn queued_low_work_survives_interleaved_system_work(low_is_peek: bool) {
        let (_home, app) = setup_nockapp("test-ker.jam").await;
        let base = app.get_handle();
        let (io_sender, mut io_receiver) = tokio::sync::mpsc::channel(16);
        let handle = NockAppHandle {
            io_sender,
            effect_sender: base.effect_sender.clone(),
            effect_receiver: tokio::sync::Mutex::new(base.effect_sender.subscribe()),
            metrics: base.metrics.clone(),
            exit: base.exit.clone(),
        };
        let (system_tx, system_rx) = key_fair_queue::channel();
        let (peer_tx, peer_rx) = key_fair_queue::channel();
        let (low_tx, low_rx) = key_fair_queue::channel();
        let mut receipts = Vec::new();
        let peer = PeerId::random();
        for _ in 0..12 {
            let (action, receipt) = poke(2);
            receipts.push(receipt);
            peer_tx.send(peer, action).expect("peer work should queue");
        }
        let (peek_result, peek_receipt) = oneshot::channel();
        let (low_poke, low_poke_receipt) = poke(3);
        if low_is_peek {
            low_tx
                .send(
                    Some(peer),
                    TrafficCopAction::Peek {
                        path: slab(0),
                        queued_at: Instant::now(),
                        result: peek_result,
                    },
                )
                .expect("peek should queue");
        } else {
            low_tx
                .send(Some(peer), TrafficCopAction::Poke(low_poke))
                .expect("low poke should queue");
        }
        let worker = tokio::spawn(traffic_cop_task(
            handle,
            system_rx,
            peer_rx,
            low_rx,
            Duration::from_secs(2),
            Arc::new(AtomicU64::new(0)),
        ));
        let mut peer_dispatches = 0;
        loop {
            let action = tokio::time::timeout(Duration::from_secs(2), io_receiver.recv())
                .await
                .expect("queued work should reach its consumer")
                .expect("kernel channel should stay open");
            match action {
                IOAction::Poke {
                    poke: cause,
                    ack_channel,
                    ..
                } => {
                    let space = cause.noun_space();
                    let value = unsafe { *cause.root() }
                        .in_space(&space)
                        .as_atom()
                        .expect("fixture cause should be an atom")
                        .as_u64()
                        .expect("fixture cause should fit u64");
                    if value == 3 {
                        assert!(!low_is_peek);
                        assert_eq!(peer_dispatches, 8);
                        ack_channel
                            .send(PokeResult::Ack)
                            .expect("low consumer should be waiting");
                        assert!(matches!(low_poke_receipt.await, Ok(Ok(PokeResult::Ack))));
                        break;
                    }
                    if value == 2 {
                        peer_dispatches += 1;
                        // Timer/system work is available after every peer
                        // completion. It must not reset the peer allowance.
                        let (system, receipt) = poke(1);
                        receipts.push(receipt);
                        system_tx
                            .send((), system)
                            .expect("system work should queue");
                    }
                    ack_channel
                        .send(PokeResult::Ack)
                        .expect("poke consumer should be waiting");
                }
                IOAction::Peek { result_channel, .. } => {
                    assert!(low_is_peek);
                    assert_eq!(peer_dispatches, 8);
                    result_channel
                        .send(Some(slab(99)))
                        .expect("peek consumer should be waiting");
                    assert!(matches!(peek_receipt.await, Ok(Ok(Some(_)))));
                    break;
                }
            }
        }
        worker.abort();
        let _ = worker.await;
    }

    #[tokio::test]
    async fn queued_peek_is_served_after_eight_peer_pokes_despite_system_pokes() {
        queued_low_work_survives_interleaved_system_work(true).await;
    }

    #[tokio::test]
    async fn queued_low_poke_is_served_after_eight_peer_pokes_despite_system_pokes() {
        queued_low_work_survives_interleaved_system_work(false).await;
    }
}
