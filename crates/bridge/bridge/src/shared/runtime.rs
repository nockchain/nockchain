use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::observability::status::BridgeStatus;
use crate::shared::base::BaseBridge;
use crate::shared::errors::BridgeError;

mod kernel;

pub use kernel::{
    BaseBlockBatch, BridgeEvent, BridgeEventKind, BridgePoke, BridgeRuntime, BridgeRuntimeHandle,
    CauseBuildOutcome, CauseBuilder, ChainEvent, EventEnvelope, EventId, KernelCauseBuilder,
    NockBlockEvent,
};

pub fn spawn_kernel_runtime(runtime: BridgeRuntime) -> JoinHandle<Result<(), BridgeError>> {
    tokio::spawn(async move { runtime.run().await })
}

pub fn spawn_base_observer(
    base_bridge: Arc<BaseBridge>,
    bridge_status: BridgeStatus,
) -> JoinHandle<Result<(), BridgeError>> {
    tokio::spawn(async move { base_bridge.stream_base_events(Some(bridge_status)).await })
}
