use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use libp2p::request_response::ResponseChannel;
use libp2p::swarm::ConnectionId;
use libp2p::PeerId;
use nockapp::driver::PokeResult;
use nockapp::noun::slab::NounSlab;
use nockapp::wire::Wire;
use nockapp::NockAppError;
use nockvm::noun::NounAllocator;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, trace, warn};

use crate::driver::gen2::*;
use crate::driver::{
    record_local_peer_abuse, Libp2pWire, LocalPeerAbuseKind, LocalPeerAbuseSeverity, SwarmAction,
    SwarmActionDispatcher,
};
use crate::ip_block::PeerExclusions;
use crate::messages::{
    BatchErrorClass, BatchResultStatus, NockchainFact, NockchainRequest, NockchainResponse,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{
    GossipBucketAdmission, InboundReplayAdmission, IpBucketAdmission, P2PState,
};
use crate::p2p_util::MultiaddrExt;
use crate::traffic_cop;

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_inbound_request(
    peer: PeerId,
    connection_id: ConnectionId,
    request: NockchainRequest,
    channel: ResponseChannel<NockchainResponse>,
    swarm_tx: mpsc::Sender<SwarmAction>,
    equix_builder: &mut equix::EquiXBuilder,
    local_peer_id: PeerId,
    traffic: traffic_cop::TrafficCop,
    metrics: Arc<NockchainP2PMetrics>,
    driver_state: Arc<Mutex<P2PState>>,
    req_res_limits: ReqResRuntimeLimits,
    peer_exclusions: PeerExclusions,
) -> Result<(), NockAppError> {
    if let NockchainRequest::BatchRequest { items, .. } = &request {
        metrics.gen2_batch_requests_received.increment();
        metrics.gen2_batch_items_received.fetch_add(items.len());
        debug!(
            peer = %peer,
            item_count = items.len(),
            "Nous req-res inbound gen2 batch received"
        );
        if let Err(err) = validate_batch_request_items(items) {
            record_batch_rejection(&metrics, BatchRejectReason::Malformed);
            warn!(
                peer = %peer,
                reject_reason = "malformed",
                error = %err,
                "Rejecting malformed batch request"
            );
            return Ok(());
        }
        if let Err(violation) = validate_batch_request_top_level_limits(items, req_res_limits) {
            match violation {
                BatchTopLevelLimitViolation::TooManyItems {
                    item_count,
                    max_items,
                } => {
                    record_batch_rejection(&metrics, BatchRejectReason::TooManyItems);
                    warn!(
                        peer = %peer,
                        generation = "gen2",
                        reject_reason = "too_many_items",
                        item_count,
                        configured_cap = max_items,
                        "Rejecting over-limit batch request before execution"
                    );
                }
                BatchTopLevelLimitViolation::TooManyBytes {
                    payload_bytes,
                    max_bytes,
                } => {
                    record_batch_rejection(&metrics, BatchRejectReason::TooManyBytes);
                    warn!(
                        peer = %peer,
                        generation = "gen2",
                        reject_reason = "too_many_bytes",
                        item_count = items.len(),
                        observed_bytes = payload_bytes,
                        configured_cap = max_bytes,
                        "Rejecting over-limit batch request before execution"
                    );
                }
            }
            return Ok(());
        }
    }
    if matches!(&request, NockchainRequest::Gossip { .. }) {
        metrics.legacy_gossip_received.increment();
    }
    if matches!(&request, NockchainRequest::Gossip { .. })
        && !req_res_limits.legacy_gossip_accept_enabled
    {
        metrics.legacy_gossip_compatibility_rejected.increment();
        metrics.gossip_dropped.increment();
        warn!(
            peer = %peer,
            "Rejecting legacy gossip because unauthenticated gossip compatibility is disabled"
        );
        return Ok(());
    }

    if !matches!(
        &request,
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. }
    ) {
        let admission = driver_state.lock().await.admit_request_from_connection(
            connection_id, req_res_limits.ip_bucket_request_admission_limit,
        );
        if let IpBucketAdmission::Rejected {
            bucket,
            count,
            limit,
        } = admission
        {
            metrics.ip_bucket_request_rejected.increment();
            metrics.requests_dropped.increment();
            warn!(
                peer = %peer,
                bucket = %bucket,
                count,
                limit,
                "Rejecting request because IP bucket request cap is exceeded"
            );
            record_local_peer_abuse(
                &swarm_tx,
                &driver_state,
                &metrics,
                &peer_exclusions,
                peer,
                Some(connection_id),
                None,
                LocalPeerAbuseKind::IpBucketRequestCap,
                LocalPeerAbuseSeverity::Strong,
                false,
            )
            .await?;
            return Ok(());
        }
    }

    let replay_key = request.replay_key()?;
    if let Some(replay_key) = replay_key {
        if driver_state
            .lock()
            .await
            .has_inbound_replay(peer, replay_key, req_res_limits.request_replay_cache_ttl)
        {
            metrics.request_replay_rejected.increment();
            metrics.requests_dropped.increment();
            warn!(
                peer = %peer,
                replay_kind = ?replay_key.kind,
                nonce = replay_key.nonce,
                "Rejecting inbound request replay before PoW verification"
            );
            return Ok(());
        }
    }

    let Ok(()) = request.verify_pow(equix_builder, &local_peer_id, &peer) else {
        warn!("bad libp2p powork from {peer}, blocking!");
        record_local_peer_abuse(
            &swarm_tx,
            &driver_state,
            &metrics,
            &peer_exclusions,
            peer,
            Some(connection_id),
            None,
            LocalPeerAbuseKind::BadRequestPow,
            LocalPeerAbuseSeverity::Strong,
            true,
        )
        .await?;
        return Ok(());
    };
    trace!("handle_request_response: powork verified");
    if let Some(replay_key) = replay_key {
        let replay_admission = driver_state.lock().await.admit_inbound_replay_key(
            peer, replay_key, req_res_limits.request_replay_cache_ttl,
            req_res_limits.request_replay_cache_max_per_peer,
        );
        if replay_admission == InboundReplayAdmission::Replayed {
            metrics.request_replay_rejected.increment();
            metrics.requests_dropped.increment();
            warn!(
                peer = %peer,
                replay_kind = ?replay_key.kind,
                nonce = replay_key.nonce,
                "Rejecting inbound request replay after PoW verification"
            );
            return Ok(());
        }
    }
    if matches!(&request, NockchainRequest::AuthenticatedGossip { .. }) {
        metrics.authenticated_gossip_verified.increment();
    }
    let addr = { driver_state.lock().await.connection_address(connection_id) };
    if let Some(addr) = addr {
        let addr_str = addr.to_string();
        debug!("Request received from peer at address {addr_str} with id {peer}");
        if let Some(ip) = addr.ip_addr() {
            let threshold_exceeded = driver_state
                .lock()
                .await
                .requested(ip, req_res_limits.request_high_threshold);
            if let Some(count) = threshold_exceeded {
                trace!("IP address {ip} exceeded the request-per-interval threshold with {count} requests");
            }
        }
    } else {
        trace!("Request received but connection not tracked. Please inform the developers.");
    }
    let admitted = {
        let mut state_guard = driver_state.lock().await;
        state_guard.try_admit_inbound_req_res(peer, req_res_limits.gen2_max_inflight_per_peer)
    };
    if !admitted {
        if matches!(&request, NockchainRequest::BatchRequest { .. }) {
            record_batch_rejection(&metrics, BatchRejectReason::Backpressure);
        }
        warn!(
            peer = %peer,
            decision = queue_saturation_decision(QueueSaturationPath::InflightAdmission),
            saturation_path = queue_saturation_path(QueueSaturationPath::InflightAdmission),
            reject_reason = "inflight_backpressure",
            max_inflight = req_res_limits.gen2_max_inflight_per_peer,
            "Rejecting request before execution because peer inflight cap is full"
        );
        metrics.requests_dropped.increment();
        record_local_peer_abuse(
            &swarm_tx,
            &driver_state,
            &metrics,
            &peer_exclusions,
            peer,
            Some(connection_id),
            None,
            LocalPeerAbuseKind::InboundAdmissionRejected,
            LocalPeerAbuseSeverity::Weak,
            false,
        )
        .await?;
        return Ok(());
    }

    let driver_state_for_release = Arc::clone(&driver_state);
    let request_result: Result<(), NockAppError> = async move {
        match request {
            NockchainRequest::Request {
                pow: _,
                nonce: _,
                message,
            } => {
                trace!("handle_request_response: Request received");
                let data_request = decode_request_item_message(&message)?;
                let response = execute_request_item(
                    peer, data_request, req_res_limits, &traffic, &metrics, &driver_state,
                )
                .await?
                .into_single_response();
                swarm_tx
                    .send(SwarmAction::SendResponse { channel, response })
                    .await
                    .map_err(|_| {
                        NockAppError::OtherError(String::from("Failed to send SwarmAction response"))
                    })
            }
            NockchainRequest::Gossip { message }
            | NockchainRequest::AuthenticatedGossip { message, .. } => {
                trace!("handle_request_response: Gossip received");
                let admission = driver_state.lock().await.admit_gossip_from_connection(
                    connection_id,
                    req_res_limits.gossip_bucket_capacity,
                    req_res_limits.gossip_bucket_refill_per_second,
                );
                if let GossipBucketAdmission::Rejected { bucket } = admission {
                    metrics.gossip_ip_bucket_rejected.increment();
                    metrics.gossip_dropped.increment();
                    warn!(
                        peer = %peer,
                        bucket = %bucket,
                        "Rejecting gossip before jam decode because IP bucket is over budget"
                    );
                    record_local_peer_abuse(
                        &swarm_tx,
                        &driver_state,
                        &metrics,
                        &peer_exclusions,
                        peer,
                        Some(connection_id),
                        None,
                        LocalPeerAbuseKind::GossipBucketCap,
                        LocalPeerAbuseSeverity::Strong,
                        false,
                    )
                    .await?;
                    return Ok(());
                }
                let mut request_slab: NounSlab = NounSlab::new();
                let message_bytes = Bytes::from(message.to_vec());
                let request_noun = request_slab.cue_into(message_bytes)?;
                request_slab.set_root(request_noun);
                trace!("handle_request_response: Gossip noun parsed");
                let driver_state_for_poke = driver_state.clone();
                let metrics_for_poke = metrics.clone();
                let swarm_tx_for_poke = swarm_tx.clone();

                let send_response: tokio::task::JoinHandle<Result<(), NockAppError>> =
                    tokio::spawn(async move {
                        let response = NockchainResponse::Ack { acked: true };
                        swarm_tx
                            .send(SwarmAction::SendResponse { channel, response })
                            .await
                            .map_err(|_| {
                                NockAppError::OtherError(String::from(
                                    "Failed to send SwarmAction response",
                                ))
                            })?;
                        Ok(())
                    });

                let poke_kernel = tokio::task::spawn(async move {
                    let gossip = NockchainFact::from_owned_noun_slab(request_slab)?;
                    let state_arc = driver_state_for_poke;
                    let track_state_arc = state_arc.clone();
                    let metrics_arc = metrics_for_poke.clone();
                    let response_gate = ResponseProcessingGate::from(&gossip);
                    let response_gate_for_enable = response_gate.clone();
                    let processing_started = Arc::new(AtomicBool::new(false));
                    let processing_started_for_enable = Arc::clone(&processing_started);
                    let heard_block_tx_ids = match &gossip {
                        NockchainFact::HeardBlock(_, fact_poke) => {
                            match heard_block_tx_ids_from_fact_poke(fact_poke) {
                                Ok(tx_ids) => Some(tx_ids),
                                Err(err) => {
                                    warn!(
                                        peer = %peer,
                                        error = ?err,
                                        "failed to extract tx source hints from heard-block gossip"
                                    );
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    if matches!(&gossip, NockchainFact::HeardElders(..)) {
                        warn!("Heard elders over gossip, should not happen!");
                    }
                    let enable_fut: Pin<Box<dyn Future<Output = bool> + Send>> =
                        Box::pin(async move {
                            let mut state_guard = state_arc.lock().await;
                            let should_process = should_process_response(
                                &response_gate_for_enable,
                                &mut state_guard,
                                &metrics_arc,
                                false,
                            );
                            if should_process {
                                processing_started_for_enable.store(true, Ordering::Relaxed);
                            }
                            should_process
                        });

                    let wire = Libp2pWire::Gossip(peer);

                    {
                        let poke_slab = gossip.fact_poke();
                        let space = poke_slab.noun_space();
                        trace!(
                            "Poking kernel with wire: {:?} noun: {:?}",
                            wire,
                            nockvm::noun::FullDebugCell {
                                cell: unsafe { &poke_slab.root().as_cell()? },
                                space: &space,
                            }
                        );
                    }

                    if let Some((block_id, height)) = future_heard_block_details(&gossip)? {
                        let mut state_guard = track_state_arc.lock().await;
                        let frontier = state_guard.first_negative;
                        if height > frontier {
                            let inserted =
                                state_guard.defer_heard_block(peer, height, block_id.clone(), gossip);
                            drop(state_guard);
                            let speculative_prefetch_count =
                                if let Some(tx_ids) = heard_block_tx_ids.as_ref() {
                                    let mut swarm_actions =
                                        SwarmActionDispatcher::Channel(&swarm_tx_for_poke);
                                    track_future_heard_block_tx_hints_and_prefetch(
                                        peer, tx_ids, &track_state_arc, &mut swarm_actions,
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
                                "Deferred future heard-block gossip before kernel poke"
                            );
                            return Ok(());
                        }
                    }

                    let poke = gossip.fact_poke();
                    let (timing, timing_rx) = tokio::sync::oneshot::channel();
                    let poke_result = traffic
                        .poke_high_priority(
                            Some(peer),
                            wire.to_wire(),
                            poke.clone(),
                            enable_fut,
                            Some(timing),
                        )
                        .await;
                    // Timing channel drop is not a task-level error;
                    // the traffic cop may have gated this gossip and
                    // returned Nack without recording timing.
                    let elapsed = match timing_rx.await {
                        Ok(elapsed) => elapsed,
                        Err(err) => {
                            trace!(
                                peer = %peer,
                                error = ?err,
                                "Gossip poke timing channel dropped before completion"
                            );
                            Duration::from_nanos(0)
                        }
                    };
                    match gossip {
                        NockchainFact::HeardBlock(_, _) => {
                            metrics.heard_block_poke_time.add_timing(&elapsed);
                        }
                        NockchainFact::HeardTx(_, _) => {
                            metrics.heard_tx_poke_time.add_timing(&elapsed);
                        }
                        _ => {}
                    }
                    match poke_result {
                        Ok(PokeResult::Ack) => {
                            // Release the processing claim on ack, matching
                            // the req-res routing path. The kernel acks
                            // gossip without emitting `%seen` whenever it
                            // discards the item (missing-parent blocks
                            // during a fork race, txs whose inputs are not
                            // in the heaviest balance). Holding the claim
                            // past the ack gates every future delivery of
                            // the same item and freezes the catch-up
                            // frontier (livenet wedge, 2026-06-11). Seen
                            // dedupe stays driven by `%seen` effects only.
                            if processing_started.load(Ordering::Relaxed) {
                                cancel_response_processing_gate(&track_state_arc, &response_gate)
                                    .await;
                            }
                            match gossip {
                                NockchainFact::HeardBlock(..) => {
                                    metrics.gossip_acked_heard_block.increment();
                                }
                                NockchainFact::HeardTx(..) => {
                                    metrics.gossip_acked_heard_tx.increment();
                                }
                                NockchainFact::HeardElders(..) => {
                                    metrics.gossip_acked_heard_elders.increment();
                                }
                            }
                            if let Some(tx_ids) = heard_block_tx_ids.as_ref() {
                                track_state_arc
                                    .lock()
                                    .await
                                    .track_tx_ids_and_peer(tx_ids.iter().cloned(), peer);
                            }
                            Ok(())
                        }
                        Ok(PokeResult::Nack) => {
                            if processing_started.load(Ordering::Relaxed) {
                                cancel_response_processing_gate(&track_state_arc, &response_gate)
                                    .await;
                            }
                            match gossip {
                                NockchainFact::HeardBlock(height, _) => {
                                    debug!(
                                        "Poke gossip nacked for heard-block at height: {:?}",
                                        height
                                    );
                                    metrics.gossip_nacked_heard_block.increment();
                                }
                                NockchainFact::HeardTx(id, _) => {
                                    debug!("Poke gossip nacked for heard-tx id: {:?}", id);
                                    metrics.gossip_nacked_heard_tx.increment();
                                }
                                NockchainFact::HeardElders(oldest, block_ids, _) => {
                                    debug!(
                                        "Poke heard-elders nacked for block height {:?} with ancestors {:?}",
                                        oldest, block_ids
                                    );
                                    metrics.gossip_nacked_heard_elders.increment();
                                }
                            };
                            trace!("handle_request_response: gossip poke nacked");
                            Ok(())
                        }
                        Err(NockAppError::MPSCFullError(act)) => {
                            if processing_started.load(Ordering::Relaxed) {
                                cancel_response_processing_gate(&track_state_arc, &response_gate)
                                    .await;
                            }
                            metrics.gossip_dropped.increment();
                            trace!(
                                decision =
                                    queue_saturation_decision(QueueSaturationPath::GossipRoute),
                                saturation_path =
                                    queue_saturation_path(QueueSaturationPath::GossipRoute),
                                "handle_request_response: gossip poke dropped due to backpressure"
                            );
                            Err(NockAppError::MPSCFullError(act))
                        }
                        Err(err) => {
                            if processing_started.load(Ordering::Relaxed) {
                                cancel_response_processing_gate(&track_state_arc, &response_gate)
                                    .await;
                            }
                            match gossip {
                                NockchainFact::HeardBlock(height, _) => {
                                    debug!(
                                        "Poke gossip erred for heard-block at height: {:?}",
                                        height
                                    );
                                    metrics.gossip_erred_heard_block.increment();
                                }
                                NockchainFact::HeardTx(id, _) => {
                                    debug!("Poke gossip erred for heard-tx id: {:?}", id);
                                    metrics.gossip_erred_heard_tx.increment();
                                }
                                NockchainFact::HeardElders(oldest, block_ids, _) => {
                                    debug!(
                                        "Poke heard-elders erred for block height {:?} with ancestors {:?}",
                                        oldest, block_ids
                                    );
                                    metrics.gossip_erred_heard_elders.increment();
                                }
                            };
                            trace!("handle_request_response: Poke errored");
                            Err(err)
                        }
                    }?;
                    trace!("handle_request_response: Poke successful");
                    Ok(())
                });
                send_response.await??;
                poke_kernel.await?
            }
            NockchainRequest::BatchRequest { nonce, items, .. } => {
                trace!(
                    peer = %peer,
                    item_count = items.len(),
                    nonce,
                    "handle_request_response: BatchRequest received"
                );
                let results = execute_batch_request_items(
                    &items,
                    req_res_limits.gen2_batch_max_bytes,
                    |item| {
                        let driver_state = driver_state.clone();
                        let item = item.clone();
                        async move {
                            estimate_batch_request_item_response(
                                &item,
                                req_res_limits,
                                &driver_state,
                            )
                            .await
                        }
                    },
                    |item| {
                        let traffic = traffic.clone();
                        let metrics = metrics.clone();
                        let driver_state = driver_state.clone();
                        let item = item.clone();
                        async move {
                            execute_batch_request_item(
                                peer, &item, req_res_limits, &traffic, &metrics, &driver_state,
                            )
                            .await
                        }
                    },
                )
                .await?;
                record_batch_result_item_errors(&metrics, &results);
                let response_bytes = batch_result_encoded_bytes(&results)?;
                let result_items = results
                    .iter()
                    .filter(|result| result.status == BatchResultStatus::Result)
                    .count();
                let not_found_items = results
                    .iter()
                    .filter(|result| result.status == BatchResultStatus::NotFound)
                    .count();
                let backpressure_items = results
                    .iter()
                    .filter(|result| matches!(result.error, Some(BatchErrorClass::Backpressure)))
                    .count();
                let too_large_items = results
                    .iter()
                    .filter(|result| matches!(result.error, Some(BatchErrorClass::TooLarge)))
                    .count();
                let response_cap_bytes = req_res_limits.gen2_batch_max_bytes;
                let cap_utilization_ratio = if response_cap_bytes == 0 {
                    0.0
                } else {
                    response_bytes as f64 / response_cap_bytes as f64
                };
                debug!(
                    peer = %peer,
                    request_items = items.len(),
                    result_items,
                    not_found_items,
                    backpressure_items,
                    too_large_items,
                    response_bytes,
                    response_cap_bytes,
                    cap_utilization_ratio,
                    "Nous req-res inbound gen2 batch response prepared"
                );
                let response = NockchainResponse::BatchResult { results };
                response.validate()?;
                swarm_tx
                    .send(SwarmAction::SendResponse { channel, response })
                    .await
                    .map_err(|_| {
                        NockAppError::OtherError(String::from("Failed to send SwarmAction response"))
                    })
            }
        }
    }
    .await;

    driver_state_for_release
        .lock()
        .await
        .release_inbound_req_res(peer);
    request_result?;

    Ok(())
}
