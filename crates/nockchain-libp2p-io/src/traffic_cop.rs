use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use libp2p::PeerId;
use nockapp::driver::{NockAppHandle, PokeResult};
use nockapp::noun::slab::NounSlab;
use nockapp::wire::WireRepr;
use nockapp::NockAppError;
use tokio::select;
use tokio::sync::oneshot;
use tracing::error;

use crate::key_fair_queue;
use crate::tracked_join_set::TrackedJoinSet;

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
    high_priority_pokes: key_fair_queue::Sender<Option<PeerId>, TrafficCopPoke>,
    low_priority: key_fair_queue::Sender<Option<PeerId>, TrafficCopAction>,
}

impl TrafficCop {
    pub(crate) fn new(
        handle: NockAppHandle,
        join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
        poke_timeout: Duration,
    ) -> Self {
        let (high_priority_pokes, high) = key_fair_queue::channel();
        let (low_priority, low) = key_fair_queue::channel();
        join_set.spawn(
            "traffic_cop".to_string(),
            traffic_cop_task(handle, high, low, poke_timeout),
        );
        Self {
            high_priority_pokes,
            low_priority,
        }
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
        self.high_priority_pokes
            .send(peer_id, action)
            .map_err(|e| match e {
                key_fair_queue::Error::SendError(_) => NockAppError::ChannelClosedError,
            })?;
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
            .map_err(|e| match e {
                key_fair_queue::Error::SendError(_) => NockAppError::ChannelClosedError,
            })?;
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
            .map_err(|e| match e {
                key_fair_queue::Error::SendError(_) => NockAppError::ChannelClosedError,
            })?;
        result_rx.await?
    }
}

async fn traffic_cop_task(
    handle: NockAppHandle,
    mut high: key_fair_queue::Receiver<Option<PeerId>, TrafficCopPoke>,
    mut low: key_fair_queue::Receiver<Option<PeerId>, TrafficCopAction>,
    poke_timeout: Duration,
) -> Result<(), NockAppError> {
    loop {
        select! { biased;
            high_priority_poke = high.recv() => match high_priority_poke {
                Some((_peer_id,TrafficCopPoke { wire, cause, result, enable, timing })) => {
                    let enabled = enable.await;
                    if !(enabled) {
                        let _ = result.send(Ok(PokeResult::Nack)).inspect_err(|_e| {
                            error!("Failed to send high priority poke result");
                        });
                        continue;
                    }
                    let now = Instant::now();
                    let res = handle.poke_timeout(wire, cause, poke_timeout).await;
                    timing.map(|c| c.send(now.elapsed()));
                    let _ = result.send(res).inspect_err(|_e| {
                        error!("Failed to send high priority poke result");
                    });
                }
                None => {
                    error!("High priority channel closed");
                    break Err(NockAppError::ChannelClosedError);
                }
            },
            low_priority_action = low.recv() => match low_priority_action {
                Some((_peer_id, TrafficCopAction::Poke(TrafficCopPoke { wire, cause, result, enable, timing }))) => {
                    let enabled = enable.await;
                    if !enabled {
                        let _ = result.send(Ok(PokeResult::Nack)).inspect_err(|_e| {
                            error!("Failed to send low priority peek result");
                        });
                        continue;
                    }
                    let now = Instant::now();
                    let res = handle.poke_timeout(wire, cause, poke_timeout).await;
                    let elapsed = now.elapsed();
                    timing.map(|c| c.send(elapsed));
                    let _ = result.send(res).inspect_err(|_e| {
                        error!("Failed to send low priority poke result");
                    });
                }
                Some((_peer_id, TrafficCopAction::Peek { path, result })) => {
                    let res = handle.peek(path).await;
                    let _ = result.send(res).inspect_err(|_e| {
                        error!("Failed to send low priority peek result");
                    });
                }
                None => {
                    error!("Low priority channel closed");
                    break Err(NockAppError::ChannelClosedError);
                }
            },
            _ = handle.next_effect() => {
                // We have to do this to prevent the broadcast channel from lagging
            }
        }
    }
}
