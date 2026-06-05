use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use libp2p::PeerId;
use nockapp::driver::PokeResult;
use nockapp::wire::Wire;
use nockapp::NockAppError;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, trace, warn};

use crate::driver::gen2::*;
use crate::driver::{Libp2pWire, SwarmAction, SwarmActionDispatcher};
use crate::messages::NockchainFact;
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{BlockSource, P2PState};
use crate::traffic_cop;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResponseProcessingGate {
    HeardBlock(String),
    HeardTx(String),
    HeardElders,
}
pub(crate) fn response_processing_gate_name(
    response_gate: &ResponseProcessingGate,
) -> &'static str {
    match response_gate {
        ResponseProcessingGate::HeardBlock(_) => "heard-block",
        ResponseProcessingGate::HeardTx(_) => "heard-tx",
        ResponseProcessingGate::HeardElders => "heard-elders",
    }
}

impl From<&NockchainFact> for ResponseProcessingGate {
    fn from(response: &NockchainFact) -> Self {
        match response {
            NockchainFact::HeardBlock(id, _) => Self::HeardBlock(id.clone()),
            NockchainFact::HeardTx(id, _) => Self::HeardTx(id.clone()),
            NockchainFact::HeardElders(_, _, _) => Self::HeardElders,
        }
    }
}

#[cfg(test)]
pub(crate) fn checkpoint_route_trace_enabled() -> bool {
    std::env::var_os("REQ_RES_GEN2_ROUTE_TRACE").is_some()
}

#[cfg(test)]
pub(crate) fn checkpoint_route_trace(event: &str, block_id: &str, detail: String) {
    if checkpoint_route_trace_enabled() {
        eprintln!("req-res-route-trace: {event} block_id={block_id} {detail}");
    }
}
pub(crate) fn should_process_response(
    response_gate: &ResponseProcessingGate,
    state: &mut P2PState,
    metrics: &NockchainP2PMetrics,
    allow_seen_block_replay: bool,
) -> bool {
    match response_gate {
        ResponseProcessingGate::HeardBlock(block_id) => {
            let should_process = if allow_seen_block_replay {
                state.try_start_processing_block_with_seen_replay(block_id, true)
            } else {
                state.try_start_processing_block(block_id)
            };
            if !should_process {
                trace!("Block already seen, not processing: {:?}", block_id);
                metrics.block_seen_cache_hits.increment();
                #[cfg(test)]
                checkpoint_route_trace(
                    "gate",
                    block_id,
                    format!("allowed=false seen_blocks={}", state.seen_blocks.len()),
                );
                false
            } else {
                trace!("block not seen, processing: {:?}", block_id);
                metrics.block_seen_cache_misses.increment();
                #[cfg(test)]
                checkpoint_route_trace(
                    "gate",
                    block_id,
                    format!("allowed=true seen_blocks={}", state.seen_blocks.len()),
                );
                true
            }
        }
        ResponseProcessingGate::HeardTx(tx_id) => {
            if !state.try_start_processing_tx(tx_id) {
                trace!("Tx already seen, not processing: {:?}", tx_id);
                metrics.tx_seen_cache_hits.increment();
                false
            } else {
                trace!("tx not seen, processing: {:?}", tx_id);
                metrics.tx_seen_cache_misses.increment();
                true
            }
        }
        // heard-elders is a recovery trigger, not a content cache. The kernel
        // can legitimately emit the same follow-up request again until it
        // makes progress, so dropping identical elders responses here changes
        // recovery semantics.
        ResponseProcessingGate::HeardElders => true,
    }
}

pub(crate) async fn cancel_response_processing_gate(
    driver_state: &Arc<Mutex<P2PState>>,
    response_gate: &ResponseProcessingGate,
) {
    let mut state_guard = driver_state.lock().await;
    match response_gate {
        ResponseProcessingGate::HeardBlock(block_id) => {
            state_guard.cancel_processing_block(block_id);
        }
        ResponseProcessingGate::HeardTx(tx_id) => {
            state_guard.cancel_processing_tx(tx_id);
        }
        ResponseProcessingGate::HeardElders => {}
    }
}

pub(crate) async fn route_response_fact_with_dispatcher(
    peer: PeerId,
    response: NockchainFact,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<(), NockAppError> {
    route_response_fact_with_source_with_dispatcher(
        peer,
        response,
        traffic,
        metrics,
        driver_state,
        swarm_actions,
        BlockSource::Gossip,
    )
    .await
}

pub(crate) async fn route_response_fact_with_source_with_dispatcher(
    peer: PeerId,
    response: NockchainFact,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    block_source: BlockSource,
) -> Result<(), NockAppError> {
    if let Some((block_id, height)) = future_heard_block_details(&response)? {
        let heard_block_tx_ids = match &response {
            NockchainFact::HeardBlock(_, fact_poke) => {
                match heard_block_tx_ids_from_fact_poke(fact_poke) {
                    Ok(tx_ids) => Some(tx_ids),
                    Err(err) => {
                        warn!(
                            peer = %peer,
                            error = ?err,
                            "failed to extract tx source hints from deferred heard-block response"
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        let mut state_guard = driver_state.lock().await;
        state_guard.record_observed_block_height_candidate(block_id.clone(), height);
        let frontier = state_guard.first_negative;
        if height > frontier {
            let inserted = state_guard.defer_heard_block_with_source(
                peer,
                height,
                block_id.clone(),
                response,
                block_source,
            );
            drop(state_guard);
            let speculative_prefetch_count = if let Some(tx_ids) = heard_block_tx_ids.as_ref() {
                track_future_heard_block_tx_hints_and_prefetch(
                    peer, tx_ids, driver_state, swarm_actions,
                )
                .await?
            } else {
                0
            };
            trace!(
                peer = %peer,
                block_id = %block_id,
                height,
                frontier,
                inserted,
                speculative_prefetch_count,
                "Deferred future heard-block until seen frontier advances"
            );
            return Ok(());
        }
    }
    #[cfg(test)]
    let trace_block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => Some(block_id.clone()),
        _ => None,
    };
    let received_block_id = match &response {
        NockchainFact::HeardBlock(block_id, _) => Some(block_id.clone()),
        _ => None,
    };
    let received_block_height = match &response {
        NockchainFact::HeardBlock(_, fact_poke) => {
            heard_block_height_from_fact_poke(fact_poke).ok()
        }
        _ => None,
    };
    if let (Some(block_id), Some(height)) = (received_block_id.as_deref(), received_block_height) {
        let mut state_guard = driver_state.lock().await;
        state_guard.record_observed_block_height_candidate(block_id.to_owned(), height);
    }
    let response_summary = response_fact_trace_summary(&response);
    let heard_block_tx_ids = match &response {
        NockchainFact::HeardBlock(_, fact_poke) => {
            match heard_block_tx_ids_from_fact_poke(fact_poke) {
                Ok(tx_ids) => Some(tx_ids),
                Err(err) => {
                    warn!(
                        peer = %peer,
                        error = ?err,
                        "failed to extract tx source hints from heard-block response"
                    );
                    None
                }
            }
        }
        _ => None,
    };
    let state_arc = driver_state.clone();
    let metrics_arc = metrics.clone();
    let response_gate = ResponseProcessingGate::from(&response);
    let response_gate_name = response_processing_gate_name(&response_gate);
    let response_gate_for_enable = response_gate.clone();
    let allow_seen_block_replay = if let Some(height) = received_block_height {
        let state_guard = driver_state.lock().await;
        state_guard.has_kernel_block_height_request(height)
    } else {
        false
    };
    let processing_started = Arc::new(AtomicBool::new(false));
    let processing_started_for_enable = Arc::clone(&processing_started);
    trace!(
        peer = %peer,
        response = %response_summary,
        response_gate = response_gate_name,
        "Routing req-res response fact to kernel"
    );
    let enable: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async move {
        let mut state_guard = state_arc.lock().await;
        let should_process = should_process_response(
            &response_gate_for_enable, &mut state_guard, &metrics_arc, allow_seen_block_replay,
        );
        if should_process {
            processing_started_for_enable.store(true, Ordering::Relaxed);
        }
        should_process
    });

    let wire = Libp2pWire::Response(peer);
    let poke_slab = response.fact_poke();

    let (timing, timing_rx) = tokio::sync::oneshot::channel();
    let route_started = Instant::now();
    let poke_result = traffic
        .poke_high_priority(
            Some(peer),
            wire.to_wire(),
            poke_slab.clone(),
            enable,
            Some(timing),
        )
        .await;
    // Timing channel drop is not a task-level error: the traffic cop may
    // have gated the poke before dispatch, in which case `poke_result` is
    // `Ok(Nack)` and the downstream gate-hit / nack accounting still needs
    // to run. Treat a missing timing value as "unknown" and keep going.
    let elapsed = match timing_rx.await {
        Ok(elapsed) => elapsed,
        Err(err) => {
            trace!(
                peer = %peer,
                response = %response_summary,
                response_gate = response_gate_name,
                error = ?err,
                "Req-res response fact timing channel dropped before completion"
            );
            Duration::from_nanos(0)
        }
    };
    let route_elapsed = route_started.elapsed();
    let traffic_cop_wait = route_elapsed
        .checked_sub(elapsed)
        .unwrap_or_else(|| Duration::from_nanos(0));
    let poke_result_label = match &poke_result {
        Ok(PokeResult::Ack) => "ack",
        Ok(PokeResult::Nack) => "nack",
        Err(NockAppError::MPSCFullError(_)) => "backpressure",
        Err(_) => "error",
    };
    info!(
        target: "nockchain::kernel_timing",
        peer = %peer,
        response = %response_summary,
        response_gate = response_gate_name,
        block_id = %received_block_id.as_deref().unwrap_or(""),
        block_height = ?received_block_height,
        route_total_ms = route_elapsed.as_secs_f64() * 1_000.0,
        traffic_cop_wait_ms = traffic_cop_wait.as_secs_f64() * 1_000.0,
        kernel_poke_ms = elapsed.as_secs_f64() * 1_000.0,
        poke_result = poke_result_label,
        "Req-res response fact kernel timing"
    );
    trace!(
        peer = %peer,
        response = %response_summary,
        response_gate = response_gate_name,
        elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
        "Completed req-res response fact kernel poke"
    );
    #[cfg(test)]
    if let Some(block_id) = trace_block_id.as_deref() {
        checkpoint_route_trace(
            "poke-complete",
            block_id,
            format!("elapsed_ms={:.3}", elapsed.as_secs_f64() * 1_000.0),
        );
    }

    match &response {
        NockchainFact::HeardBlock(..) => {
            metrics.heard_block_poke_time.add_timing(&elapsed);
        }
        NockchainFact::HeardTx(..) => {
            metrics.heard_tx_poke_time.add_timing(&elapsed);
        }
        NockchainFact::HeardElders(..) => {
            metrics.heard_elders_poke_time.add_timing(&elapsed);
        }
    }

    match poke_result {
        Ok(PokeResult::Ack) => {
            if allow_seen_block_replay && processing_started.load(Ordering::Relaxed) {
                cancel_response_processing_gate(driver_state, &response_gate).await;
            }
            trace!(
                peer = %peer,
                response = %response_summary,
                "Req-res response fact accepted by kernel"
            );
            #[cfg(test)]
            if let Some(block_id) = trace_block_id.as_deref() {
                checkpoint_route_trace("poke-result", block_id, String::from("result=ack"));
            }
            match response {
                NockchainFact::HeardBlock(..) => {
                    metrics.responses_acked_heard_block.increment();
                }
                NockchainFact::HeardTx(..) => {
                    metrics.responses_acked_heard_tx.increment();
                }
                NockchainFact::HeardElders(..) => {
                    metrics.responses_acked_heard_elders.increment();
                }
            }
            if let Some(block_id) = received_block_id.as_deref() {
                let mut state_guard = driver_state.lock().await;
                state_guard.record_block_received(peer, block_id);
                if let Some(tx_ids) = heard_block_tx_ids.as_ref() {
                    state_guard.track_tx_ids_and_peer(tx_ids.iter().cloned(), peer);
                }
                // Phase 5: a successful kernel-acked block at this height
                // resets the per-height retry budget; future failures
                // start fresh.
                if let Some(height) = received_block_height {
                    state_guard.clear_block_height_failure(height);
                }
            }
        }
        Ok(PokeResult::Nack) => {
            if processing_started.load(Ordering::Relaxed) {
                cancel_response_processing_gate(driver_state, &response_gate).await;
            }
            if let Some(block_id) = received_block_id.as_deref() {
                let mut state_guard = driver_state.lock().await;
                state_guard.remove_observed_block_height_candidate(block_id);
            }
            trace!(
                peer = %peer,
                response = %response_summary,
                "Req-res response fact nacked before kernel processing"
            );
            #[cfg(test)]
            if let Some(block_id) = trace_block_id.as_deref() {
                checkpoint_route_trace("poke-result", block_id, String::from("result=nack"));
            }
            match response {
                NockchainFact::HeardBlock(..) => {
                    metrics.responses_nacked_heard_block.increment();
                }
                NockchainFact::HeardTx(id, _) => {
                    debug!("Poke response nacked for heard-tx id: {:?}", id);
                    metrics.responses_nacked_heard_tx.increment();
                }
                NockchainFact::HeardElders(oldest, block_ids, _) => {
                    debug!(
                        "Poke response heard-elders nacked for block height {:?} with ancestors {:?}",
                        oldest, block_ids
                    );
                    metrics.responses_nacked_heard_elders.increment();
                }
            }
            trace!("handle_request_response: Poke failed");
            return Ok(());
        }
        Err(NockAppError::MPSCFullError(act)) => {
            if processing_started.load(Ordering::Relaxed) {
                cancel_response_processing_gate(driver_state, &response_gate).await;
            }
            if let Some(block_id) = received_block_id.as_deref() {
                let mut state_guard = driver_state.lock().await;
                state_guard.remove_observed_block_height_candidate(block_id);
            }
            #[cfg(test)]
            if let Some(block_id) = trace_block_id.as_deref() {
                checkpoint_route_trace(
                    "poke-result",
                    block_id,
                    String::from("result=backpressure"),
                );
            }
            trace!(
                peer = %peer,
                response = %response_summary,
                decision = queue_saturation_decision(QueueSaturationPath::ResponseRoute),
                saturation_path = queue_saturation_path(QueueSaturationPath::ResponseRoute),
                "handle_request_response: Response dropped due to backpressure."
            );
            metrics.responses_dropped.increment();
            return Err(NockAppError::MPSCFullError(act));
        }
        Err(err) => {
            if processing_started.load(Ordering::Relaxed) {
                cancel_response_processing_gate(driver_state, &response_gate).await;
            }
            if let Some(block_id) = received_block_id.as_deref() {
                let mut state_guard = driver_state.lock().await;
                state_guard.remove_observed_block_height_candidate(block_id);
            }
            #[cfg(test)]
            if let Some(block_id) = trace_block_id.as_deref() {
                checkpoint_route_trace("poke-result", block_id, String::from("result=error"));
            }
            trace!(
                peer = %peer,
                response = %response_summary,
                error = ?err,
                "Error routing req-res response fact to kernel"
            );
            match response {
                NockchainFact::HeardBlock(height, _) => {
                    debug!(
                        "Poke response error for heard-block at height: {:?}",
                        height
                    );
                    metrics.responses_erred_heard_block.increment();
                }
                NockchainFact::HeardTx(id, _) => {
                    debug!("Poke response error for heard-tx id: {:?}", id);
                    metrics.responses_erred_heard_tx.increment();
                }
                NockchainFact::HeardElders(oldest, block_ids, _) => {
                    debug!(
                        "Poke response error for heard-elders for block height {:?} with ancestors {:?}",
                        oldest, block_ids
                    );
                    metrics.responses_erred_heard_elders.increment();
                }
            }
            trace!("Error sending poke");
        }
    }
    trace!("handle_request_response: Poke successful");
    Ok(())
}

pub(crate) async fn route_response_fact(
    peer: PeerId,
    response: NockchainFact,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    route_response_fact_with_dispatcher(
        peer, response, traffic, metrics, driver_state, &mut swarm_actions,
    )
    .await
}
