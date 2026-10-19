//! Serial, off-loop reconciliation of the monotonic seen frontier with accepted kernel state.

use std::sync::Arc;

use nockapp::noun::slab::NounSlab;
use nockapp::NockAppError;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tracing::{debug, warn};

use super::kernel_io::{heaviest_chain_height_from_scry, heaviest_chain_scry_slab};
use super::{SwarmAction, SwarmActionDispatcher};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::P2PState;
use crate::traffic_cop;

/// How long the frontier may sit still with deferred blocks parked above it
/// before the reconcile pass warns that sync is stuck.
pub(crate) const FRONTIER_STALL_WARN_AFTER: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrontierReconcileReason {
    EffectLag,
    Periodic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrontierReconcileOutcome {
    /// The kernel has no heaviest block yet, or the peek failed.
    Unavailable,
    /// The frontier already covered the kernel's heaviest block.
    InSync { frontier: u64, kernel_heaviest: u64 },
    /// The frontier lagged the kernel's heaviest block and was moved forward.
    Advanced {
        previous_frontier: u64,
        frontier: u64,
        kernel_heaviest: u64,
    },
}

/// What the driver loop should do with one `next_effect` result.
pub(crate) enum KernelEffectRecv {
    Effect(NounSlab),
    /// Effects were lost to broadcast lag. A frontier resync was requested.
    Lagged,
    /// The effect stream is closed. Stop polling it.
    Closed,
}

/// Sort a `next_effect` result. A lag must not be skipped silently: the lost
/// effects may include the heighted `%seen` that moves the frontier.
pub(crate) fn triage_kernel_effect_recv(
    result: Result<NounSlab, NockAppError>,
    metrics: &NockchainP2PMetrics,
    lag_signal: &Notify,
) -> KernelEffectRecv {
    match result {
        Ok(noun_slab) => KernelEffectRecv::Effect(noun_slab),
        Err(NockAppError::BroadcastRecvLaggedError(missed)) => {
            metrics
                .kernel_effects_lagged_total
                .fetch_add(usize::try_from(missed).unwrap_or(usize::MAX));
            warn!(
                missed,
                "libp2p driver fell behind the kernel effect stream and lost effects; \
                 resyncing the seen frontier from the kernel"
            );
            lag_signal.notify_one();
            KernelEffectRecv::Lagged
        }
        Err(err) => {
            warn!(error = ?err, "Kernel effect stream closed; libp2p driver stops handling effects");
            KernelEffectRecv::Closed
        }
    }
}

/// One reconcile pass: peek the kernel's heaviest height, move the frontier
/// past it if the frontier lags, and queue a deferred flush if any deferred
/// block is now ready. Also warns when the frontier has sat still for
/// `stall_warn_after` with deferred blocks parked above it.
pub(crate) async fn reconcile_seen_frontier_with_kernel(
    reason: FrontierReconcileReason,
    traffic: &traffic_cop::TrafficCop,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    stall_warn_after: Duration,
) -> Result<FrontierReconcileOutcome, NockAppError> {
    let kernel_heaviest = match traffic.peek(None, heaviest_chain_scry_slab()).await {
        Ok(Some(scry_res)) => match heaviest_chain_height_from_scry(&scry_res) {
            Ok(Some(height)) => height,
            Ok(None) => {
                debug!(
                    ?reason,
                    "Kernel has no heaviest block yet; skipping frontier reconcile"
                );
                return Ok(FrontierReconcileOutcome::Unavailable);
            }
            Err(err) => {
                warn!(?reason, error = ?err, "Could not decode kernel heaviest-chain peek");
                return Ok(FrontierReconcileOutcome::Unavailable);
            }
        },
        Ok(None) => {
            debug!(?reason, "Kernel heaviest-chain peek returned nothing");
            return Ok(FrontierReconcileOutcome::Unavailable);
        }
        Err(err) => {
            debug!(?reason, error = ?err, "Kernel heaviest-chain peek failed");
            return Ok(FrontierReconcileOutcome::Unavailable);
        }
    };

    // Release the state guard before dispatching the flush; see the %seen
    // arm in `handle_effect_with_dispatcher`.
    let (outcome, should_flush, deferred_above) = {
        let mut state_guard = driver_state.lock().await;
        let previous_frontier = state_guard.first_negative;
        let outcome = if state_guard.advance_seen_frontier(kernel_heaviest) {
            FrontierReconcileOutcome::Advanced {
                previous_frontier,
                frontier: state_guard.first_negative,
                kernel_heaviest,
            }
        } else {
            FrontierReconcileOutcome::InSync {
                frontier: previous_frontier,
                kernel_heaviest,
            }
        };
        (
            outcome,
            state_guard.has_ready_deferred_heard_blocks(),
            state_guard.deferred_above_frontier(std::time::Instant::now()),
        )
    };

    if let FrontierReconcileOutcome::Advanced {
        previous_frontier,
        frontier,
        kernel_heaviest,
    } = outcome
    {
        metrics.seen_frontier_resync_total.increment();
        warn!(
            ?reason,
            previous_frontier,
            frontier,
            kernel_heaviest,
            "Seen frontier lagged the kernel's heaviest block; resynced"
        );
    }

    if should_flush {
        swarm_actions
            .dispatch(SwarmAction::FlushDeferredHeardBlocks)
            .await
            .map_err(|_| {
                NockAppError::OtherError(String::from("Failed to queue deferred heard-block flush"))
            })?;
    }

    match deferred_above {
        Some((lowest_height, height_count, frontier_idle)) if frontier_idle >= stall_warn_after => {
            metrics
                .deferred_heights_above_stalled_frontier
                .swap(height_count as f64);
            warn!(
                kernel_heaviest,
                lowest_deferred_height = lowest_height,
                deferred_heights = height_count,
                frontier_idle_secs = frontier_idle.as_secs(),
                "Seen frontier has not advanced while heard-blocks wait above it; sync may be stuck"
            );
        }
        _ => {
            metrics.deferred_heights_above_stalled_frontier.swap(0.0);
        }
    }

    Ok(outcome)
}

/// Long-lived task: reconcile whenever the driver loop reports effect lag,
/// and every `interval` as a backstop. Runs off the main select loop so a
/// slow or frozen kernel never parks the driver. `Notify` stores one permit,
/// so lags during a pass coalesce into one more pass.
pub(crate) async fn run_frontier_reconcile_task(
    traffic: traffic_cop::TrafficCop,
    driver_state: Arc<Mutex<P2PState>>,
    metrics: Arc<NockchainP2PMetrics>,
    swarm_tx: mpsc::Sender<SwarmAction>,
    lag_signal: Arc<Notify>,
    interval: Option<Duration>,
) -> Result<(), NockAppError> {
    let mut ticker = interval.map(|period| {
        let mut ticker = tokio::time::interval_at(Instant::now() + period, period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticker
    });
    // A consumed lag permit remains owed until a usable kernel snapshot is
    // obtained. Delay retries so an unavailable kernel cannot cause a spin.
    let mut lag_pending = false;
    let mut retry_at = None;
    loop {
        let reason = tokio::select! {
            _ = lag_signal.notified() => {
                lag_pending = true;
                FrontierReconcileReason::EffectLag
            },
            _ = async {
                match retry_at {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => FrontierReconcileReason::EffectLag,
            _ = async {
                match ticker.as_mut() {
                    Some(ticker) => { ticker.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => FrontierReconcileReason::Periodic,
        };
        let mut swarm_actions = SwarmActionDispatcher::Channel(&swarm_tx);
        let outcome = reconcile_seen_frontier_with_kernel(
            reason, &traffic, &driver_state, &metrics, &mut swarm_actions,
            FRONTIER_STALL_WARN_AFTER,
        )
        .await?;
        if outcome != FrontierReconcileOutcome::Unavailable {
            lag_pending = false;
        }
        retry_at = lag_pending.then(|| Instant::now() + Duration::from_secs(1));
    }
}
