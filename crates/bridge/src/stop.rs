use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

use crate::bridge_status::BridgeStatus;
use crate::tui::types::AlertSeverity;
use crate::types::StopLastBlocks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopSource {
    KernelEffect,
    PeerBroadcast,
    Local,
}

#[derive(Clone, Debug)]
pub struct StopInfo {
    pub reason: String,
    pub last: Option<StopLastBlocks>,
    pub source: StopSource,
    pub at: SystemTime,
}

#[derive(Clone)]
pub struct StopController {
    tx: watch::Sender<Option<StopInfo>>,
}

#[derive(Clone)]
pub struct StopHandle {
    rx: watch::Receiver<Option<StopInfo>>,
}

impl StopController {
    pub fn new() -> (Self, StopHandle) {
        let (tx, rx) = watch::channel(None);
        (Self { tx }, StopHandle { rx })
    }

    pub fn handle(&self) -> StopHandle {
        StopHandle {
            rx: self.tx.subscribe(),
        }
    }

    /// Returns `true` if this call transitioned the controller into the stopped state.
    pub fn trigger(&self, info: StopInfo) -> bool {
        if self.tx.borrow().is_some() {
            return false;
        }
        let _ = self.tx.send_replace(Some(info));
        true
    }

    pub fn clear(&self) {
        let _ = self.tx.send_replace(None);
    }
}

impl StopHandle {
    pub fn is_stopped(&self) -> bool {
        self.rx.borrow().is_some()
    }

    pub fn info(&self) -> Option<StopInfo> {
        self.rx.borrow().clone()
    }
}

pub(crate) async fn trigger_local_stop(
    runtime: Arc<crate::runtime::BridgeRuntimeHandle>,
    stop_controller: StopController,
    bridge_status: BridgeStatus,
    reason: String,
) {
    use tracing::{info, warn};

    let metrics = crate::metrics::init_metrics();
    metrics.stop_local_requests.increment();

    info!(
        target: "bridge.stop",
        reason=%reason,
        "local stop requested"
    );

    let last = match runtime.peek_stop_info().await {
        Ok(v) => v,
        Err(err) => {
            warn!(
                target: "bridge.stop",
                error=%err,
                "failed to peek stop-info while triggering local stop"
            );
            None
        }
    };

    let info = StopInfo {
        reason: reason.clone(),
        last: last.clone(),
        source: StopSource::Local,
        at: SystemTime::now(),
    };

    if !stop_controller.trigger(info) {
        metrics.stop_local_duplicate.increment();
        info!(
            target: "bridge.stop",
            reason=%reason,
            "local stop already active, ignoring duplicate request"
        );
        return;
    }

    metrics.stop_local_triggered.increment();
    info!(
        target: "bridge.stop",
        reason=%reason,
        has_last = last.is_some(),
        "local stop activated"
    );

    bridge_status.push_alert(
        AlertSeverity::Error,
        "Bridge Stopped".to_string(),
        reason.clone(),
        "local-stop".to_string(),
    );

    if let Some(last) = last {
        info!(
            target: "bridge.stop",
            "forwarding local stop cause to kernel"
        );
        if let Err(err) = runtime.send_stop(last).await {
            warn!(
                target: "bridge.stop",
                error=%err,
                "failed to poke kernel with stop cause after local stop trigger"
            );
        }
    } else {
        info!(
            target: "bridge.stop",
            "no stop-info snapshot available; skipping kernel stop poke"
        );
    }
}

/// Driver for stop effects, propagates stop state locally and to peers.
pub fn create_stop_driver(
    runtime: Arc<crate::runtime::BridgeRuntimeHandle>,
    stop_controller: StopController,
    bridge_status: BridgeStatus,
    peers: Vec<crate::health::PeerEndpoint>,
    self_node_id: u64,
) -> nockapp::driver::IODriverFn {
    use nockapp::driver::{make_driver, NockAppHandle};
    use noun_serde::NounDecode;
    use tracing::warn;

    use crate::ingress::proto::StopBroadcast;
    use crate::ingress::spawn_broadcast_stop_to_peers;
    use crate::types::{BridgeEffect, BridgeEffectVariant};

    make_driver(move |handle: NockAppHandle| {
        let runtime = runtime.clone();
        let stop_controller = stop_controller.clone();
        let bridge_status = bridge_status.clone();
        let peers = peers.clone();

        async move {
            loop {
                let effect = match handle.next_effect().await {
                    Ok(effect) => effect,
                    Err(_) => continue,
                };

                let root = unsafe { effect.root() };
                let bridge_effect = match BridgeEffect::from_noun(root) {
                    Ok(effect) => effect,
                    Err(_) => continue,
                };

                let BridgeEffectVariant::Stop(data) = bridge_effect.variant else {
                    continue;
                };

                let stop_info = StopInfo {
                    reason: data.reason.clone(),
                    last: Some(data.last.clone()),
                    source: StopSource::KernelEffect,
                    at: SystemTime::now(),
                };

                if !stop_controller.trigger(stop_info) {
                    continue;
                }

                bridge_status.push_alert(
                    AlertSeverity::Error,
                    "Bridge Stopped".to_string(),
                    data.reason.clone(),
                    "kernel-stop".to_string(),
                );
                //  Note: poking the kernel with a %stop cause does not cause a %stop effect to get emitted.
                //  a %stop effect CAUSES a corresponding stop poke, which sets the kernel state machine to STOP status.
                if let Err(err) = runtime.send_stop(data.last.clone()).await {
                    warn!(
                        target: "bridge.stop",
                        error=%err,
                        "failed to poke kernel with stop cause"
                    );
                }

                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let msg = StopBroadcast {
                    sender_node_id: self_node_id,
                    reason: data.reason.clone(),
                    last_base_hash: Some(data.last.base.base_hash.to_be_limb_bytes().to_vec()),
                    last_base_height: Some(data.last.base.height),
                    last_nock_hash: Some(data.last.nock.nock_hash.to_be_limb_bytes().to_vec()),
                    last_nock_height: Some(data.last.nock.height),
                    timestamp,
                };

                spawn_broadcast_stop_to_peers(&peers, msg, "bridge.stop");
            }
        }
    })
}
