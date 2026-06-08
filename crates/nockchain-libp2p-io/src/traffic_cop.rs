use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libp2p::PeerId;
use nockapp::driver::{NockAppHandle, PokeResult};
use nockapp::noun::slab::NounSlab;
use nockapp::wire::WireRepr;
use nockapp::NockAppError;
use tokio::select;
use tokio::sync::oneshot;
use tracing::{error, trace, warn};

use crate::key_fair_queue;
use crate::tracked_join_set::TrackedJoinSet;

const PEER_HIGH_BURST_BEFORE_LOW: usize = 8;
const HIGH_PRIORITY_QUEUE_MAX_TOTAL: usize = 16_384;
const HIGH_PRIORITY_QUEUE_MAX_PER_KEY: usize = 512;
const LOW_PRIORITY_QUEUE_MAX_TOTAL: usize = 8_192;
const LOW_PRIORITY_QUEUE_MAX_PER_KEY: usize = 256;

/// Timestamp (seconds since UNIX epoch) of the last successful TrafficCop
/// kernel operation. Bumped after high/low-priority pokes return from
/// `handle.poke` and after low-priority peeks return from `handle.peek`. The
/// libp2p-watchdog thread reads this in parallel with the driver heartbeat
/// counter and dumps thread stacks if it stops advancing, which is the precise
/// signal that was missing on the 2026-04-18 LAX1 stall (heartbeat kept ticking
/// because it's an independent task; only the kernel side wedged). Consensus
/// pokes are no longer bounded by a wall-clock timeout (so that a slow host
/// cannot turn "still validating" into a Nack/timeout), which means a genuine
/// kernel livelock now freezes this counter rather than masking itself as a
/// stream of timeout returns — exactly what the watchdog is there to catch.
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
        result: oneshot::Sender<Result<Option<NounSlab>, NockAppError>>,
    },
}

struct TrafficCopPoke {
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
        let action = TrafficCopPoke {
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
        let action = TrafficCopAction::Poke(TrafficCopPoke {
            wire,
            cause,
            timing,
            enable,
            result: result_tx,
        });
        self.low_priority
            .send(peer_id, action)
            .map_err(key_fair_queue_error_to_nockapp)?;
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
            result: result_tx,
        };
        self.low_priority
            .send(peer_id, action)
            .map_err(key_fair_queue_error_to_nockapp)?;
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
    let mut consecutive_peer_high = 0usize;
    loop {
        if !system_high_open && !peer_high_open && !low_open {
            error!("Traffic cop channels closed");
            break Err(NockAppError::ChannelClosedError);
        }

        if consecutive_peer_high >= PEER_HIGH_BURST_BEFORE_LOW {
            select! { biased;
                system_high_priority_poke = system_high.recv(), if system_high_open => match system_high_priority_poke {
                    Some((_, poke)) => {
                        consecutive_peer_high = 0;
                        process_poke(&handle, poke, "system high priority", &last_poke_completed_at).await;
                    }
                    None => {
                        system_high_open = false;
                    }
                },
                low_priority_action = low.recv(), if low_open => match low_priority_action {
                    Some((_peer_id, action)) => {
                        consecutive_peer_high = 0;
                        process_low_priority_action(
                            &handle,
                            peek_timeout,
                            action,
                            &last_poke_completed_at,
                        )
                        .await;
                    }
                    None => {
                        low_open = false;
                    }
                },
                peer_high_priority_poke = peer_high.recv(), if peer_high_open => match peer_high_priority_poke {
                    Some((_peer_id, poke)) => {
                        consecutive_peer_high = consecutive_peer_high.saturating_add(1);
                        process_poke(&handle, poke, "peer high priority", &last_poke_completed_at).await;
                    }
                    None => {
                        peer_high_open = false;
                    }
                },
                _ = handle.next_effect() => {
                    // We have to do this to prevent the broadcast channel from lagging
                }
            }
        } else {
            select! { biased;
                system_high_priority_poke = system_high.recv(), if system_high_open => match system_high_priority_poke {
                    Some((_, poke)) => {
                        consecutive_peer_high = 0;
                        process_poke(&handle, poke, "system high priority", &last_poke_completed_at).await;
                    }
                    None => {
                        system_high_open = false;
                    }
                },
                peer_high_priority_poke = peer_high.recv(), if peer_high_open => match peer_high_priority_poke {
                    Some((_peer_id, poke)) => {
                        consecutive_peer_high = consecutive_peer_high.saturating_add(1);
                        process_poke(&handle, poke, "peer high priority", &last_poke_completed_at).await;
                    }
                    None => {
                        peer_high_open = false;
                    }
                },
                low_priority_action = low.recv(), if low_open => match low_priority_action {
                    Some((_peer_id, action)) => {
                        consecutive_peer_high = 0;
                        process_low_priority_action(
                            &handle,
                            peek_timeout,
                            action,
                            &last_poke_completed_at,
                        )
                        .await;
                    }
                    None => {
                        low_open = false;
                    }
                },
                _ = handle.next_effect() => {
                    // We have to do this to prevent the broadcast channel from lagging
                }
            }
        }
    }
}

async fn process_poke(
    handle: &NockAppHandle,
    TrafficCopPoke {
        wire,
        cause,
        timing,
        enable,
        result,
    }: TrafficCopPoke,
    label: &str,
    last_poke_completed_at: &Arc<AtomicU64>,
) {
    let enabled = enable.await;
    if !enabled {
        trace!(
            label,
            had_timing_channel = timing.is_some(),
            "Traffic cop gated high priority poke before dispatch"
        );
        // Close the timing channel explicitly so callers awaiting `timing_rx`
        // wake with Ok(zero) rather than Err(RecvError). Dropping the Sender
        // silently here is what produced the ~16 k/hr "Background req-res
        // task lost a oneshot response" warn storm observed on LAX1 prior
        // to the 2026-04-17 freeze.
        if let Some(timing) = timing {
            let _ = timing.send(Duration::from_nanos(0));
        }
        let _ = result.send(Ok(PokeResult::Nack)).inspect_err(|_e| {
            error!("Failed to send {label} poke result");
        });
        // Intentionally NOT bumping `last_poke_completed_at` here; a gated
        // Nack does not constitute a kernel-side round trip. If every
        // poke is gated we want the watchdog to eventually trip.
        return;
    }

    let now = Instant::now();
    // No wall-clock timeout: a consensus poke runs to completion so that a slow
    // host cannot turn "still validating" into a Nack/timeout.
    // Recording progress here means the await actually returned; a genuine
    // kernel livelock now leaves this un-bumped, which is the signal the
    // libp2p-watchdog watches for (see the `unix_now` docstring).
    let res = handle.poke(wire, cause).await;
    last_poke_completed_at.store(unix_now(), Ordering::Relaxed);
    if let Some(timing) = timing {
        let _ = timing.send(now.elapsed());
    }
    let _ = result.send(res).inspect_err(|_e| {
        error!("Failed to send {label} poke result");
    });
}

async fn process_low_priority_action(
    handle: &NockAppHandle,
    peek_timeout: Duration,
    action: TrafficCopAction,
    last_poke_completed_at: &Arc<AtomicU64>,
) {
    match action {
        TrafficCopAction::Poke(TrafficCopPoke {
            wire,
            cause,
            result,
            enable,
            timing,
        }) => {
            let enabled = enable.await;
            if !enabled {
                trace!(
                    had_timing_channel = timing.is_some(),
                    "Traffic cop gated low priority poke before dispatch"
                );
                // Mirror of the fix in `process_poke`: close the timing
                // oneshot on the gated path, letting callers awaiting
                // `timing_rx` wake with Ok(zero) rather than Err(RecvError).
                if let Some(timing) = timing {
                    let _ = timing.send(Duration::from_nanos(0));
                }
                let _ = result.send(Ok(PokeResult::Nack)).inspect_err(|_e| {
                    error!("Failed to send low priority poke result");
                });
                // See `process_poke`: gated Nack is not kernel progress.
                return;
            }
            let now = Instant::now();
            // No wall-clock timeout on consensus pokes; see
            // `process_poke`.
            let res = handle.poke(wire, cause).await;
            last_poke_completed_at.store(unix_now(), Ordering::Relaxed);
            if let Some(timing) = timing {
                let _ = timing.send(now.elapsed());
            }
            let _ = result.send(res).inspect_err(|_e| {
                error!("Failed to send low priority poke result");
            });
        }
        TrafficCopAction::Peek { path, result } => {
            let res = match tokio::time::timeout(peek_timeout, handle.peek(path)).await {
                Ok(res) => {
                    // Peeks are read-only kernel round trips; they still prove
                    // the serf thread is alive, so bump the liveness counter.
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
            let _ = result.send(res).inspect_err(|_e| {
                error!("Failed to send low priority peek result");
            });
        }
    }
}
