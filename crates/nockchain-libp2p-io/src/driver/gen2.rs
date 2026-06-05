use rand::Rng;

use super::gen1::build_unsupported_protocol_fallback_contexts;
use super::*;
use crate::messages::{block_by_height_message, BatchErrorClass};

mod batch;
mod frontier;
mod inbound;
mod outbound;
mod request_exec;
mod responses;
mod routing;

pub(crate) use batch::*;
#[cfg(test)]
pub(crate) use frontier::flush_ready_deferred_heard_blocks;
pub(crate) use frontier::{
    collect_tip5_zset_strings, flush_ready_deferred_heard_blocks_with_dispatcher,
    future_heard_block_details, heard_block_height_from_fact_poke,
    heard_block_tx_ids_from_fact_poke, queue_speculative_raw_tx_prefetches_with_dispatcher,
    track_future_heard_block_tx_hints_and_prefetch,
};
use inbound::handle_inbound_request;
use outbound::handle_outbound_response;
pub(super) use request_exec::execute_request_item;
pub(crate) use responses::{
    batch_request_item_ids, missing_batch_result_item_ids, response_envelope_from_result_message,
    response_fact_from_envelope, response_fact_from_result_message, response_fact_trace_summary,
    route_block_range_envelope_with_dispatcher, route_bundle_envelope,
    validate_response_envelope_for_request, validate_response_fact_for_request,
};
#[cfg(test)]
pub(crate) use routing::checkpoint_route_trace;
pub(crate) use routing::{
    cancel_response_processing_gate, route_response_fact, route_response_fact_with_dispatcher,
    route_response_fact_with_source_with_dispatcher, should_process_response,
    ResponseProcessingGate,
};

pub(super) async fn enqueue_unsupported_protocol_fallback(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    request_context: &OutboundRequestContext,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
) -> Result<usize, NockAppError> {
    let fallback_contexts = build_unsupported_protocol_fallback_contexts(
        request_context, local_peer_id, equix_builder,
    )?;
    let fallback_count = fallback_contexts.len();

    for fallback_context in fallback_contexts {
        let request = fallback_context.request.clone();
        swarm_actions
            .dispatch(SwarmAction::SendRequest {
                peer_id: fallback_context.peer_id,
                request,
                request_context: Some(fallback_context),
            })
            .await
            .map_err(|_| {
                NockAppError::OtherError(String::from("Failed to queue fallback request"))
            })?;
    }

    Ok(fallback_count)
}

pub(super) async fn outbound_request_estimated_response_bytes(
    request: &NockchainRequest,
    limits: ReqResRuntimeLimits,
    driver_state: &Arc<Mutex<P2PState>>,
) -> usize {
    let NockchainRequest::Request { message, .. } = request else {
        return limits.gen2_batch_max_bytes;
    };

    outbound_request_message_estimated_response_bytes(message, limits, driver_state).await
}
pub(super) async fn outbound_request_message_estimated_response_bytes(
    message: &[u8],
    limits: ReqResRuntimeLimits,
    driver_state: &Arc<Mutex<P2PState>>,
) -> usize {
    let Ok(data_request) = decode_request_item_message(message) else {
        return limits.gen2_item_max_bytes;
    };
    let fallback_message_bytes = response_estimate_fallback_message_bytes(limits);

    let state_guard = driver_state.lock().await;
    state_guard
        .estimated_response_message_bytes(&data_request, fallback_message_bytes)
        .0
}

pub(super) fn take_pending_batch_request(
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    peer_id: PeerId,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
) -> Result<Option<OutboundRequestContext>, NockAppError> {
    let Some(pending_batch) = pending_gen2_batches.get_mut(&peer_id) else {
        return Ok(None);
    };
    if pending_batch.is_empty() {
        pending_gen2_batches.remove(&peer_id);
        return Ok(None);
    }

    let items = pending_batch.take_items();
    pending_gen2_batches.remove(&peer_id);
    // Keep actual multi-item block batches on gen2, but avoid regressing
    // singleton block sync by wrapping a lone BlockByHeight into a one-item
    // gen2 batch.
    if let [item] = items.as_slice() {
        if matches!(
            decode_request_item_message(&item.message),
            Ok(NockchainDataRequest::BlockByHeight(_))
        ) {
            let request_slab = request_slab_from_message(&item.message)?;
            let request = NockchainRequest::new_request(
                equix_builder, local_peer_id, &peer_id, &request_slab,
            );
            return Ok(Some(OutboundRequestContext::new(
                peer_id,
                ReqResGeneration::Gen1,
                request,
            )));
        }
    }
    let request =
        NockchainRequest::new_batch_request(equix_builder, local_peer_id, &peer_id, items)?;
    Ok(Some(OutboundRequestContext::new(
        peer_id,
        ReqResGeneration::Gen2,
        request,
    )))
}
pub(super) async fn send_outbound_request_now(
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    request_context: OutboundRequestContext,
) {
    let peer_id = request_context.peer_id;
    let request = request_context.request.clone();
    if let Some(item_count) = batch_request_item_count(&request_context.request) {
        metrics.gen2_batch_requests_sent.increment();
        metrics.gen2_batch_items_sent.fetch_add(item_count);
    }
    if matches!(
        &request_context.request,
        NockchainRequest::AuthenticatedGossip { .. }
    ) {
        metrics.authenticated_gossip_sent.increment();
    }
    let batch_payload_bytes = match &request_context.request {
        NockchainRequest::BatchRequest { items, .. } => batch_request_payload_bytes(items).ok(),
        _ => None,
    };
    let request_id = swarm
        .behaviour_mut()
        .request_response
        .send_request(&peer_id, request);
    info!(
        peer = %peer_id,
        request_id = %request_id,
        generation = ?request_context.generation,
        request_shape = outbound_request_shape(&request_context.request),
        request_keys = %outbound_request_keys_csv(&request_context.request),
        request_block_heights = %outbound_request_block_heights_csv(&request_context.request),
        batch_items = batch_request_item_count(&request_context.request),
        batch_payload_bytes,
        retry_count = request_context.retry_count,
        fallback_attempted = request_context.fallback_attempted,
        "Nous req-res outbound request sent"
    );
    driver_state
        .lock()
        .await
        .record_outbound_request(request_id, request_context);
}

pub(super) async fn queue_retry_requests_with_dispatcher(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    metrics: &NockchainP2PMetrics,
    requests: Vec<OutboundRequestContext>,
    delay: Duration,
) -> Result<(), NockAppError> {
    if requests.is_empty() {
        return Ok(());
    }
    metrics
        .req_res_retry_scheduled_total
        .fetch_add(requests.len());
    swarm_actions
        .dispatch(SwarmAction::RetryRequests { requests, delay })
        .await
        .map_err(|_| NockAppError::OtherError(String::from("Failed to schedule retry requests")))
}

#[cfg(test)]
pub(super) async fn queue_retry_requests(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    metrics: &NockchainP2PMetrics,
    requests: Vec<OutboundRequestContext>,
    delay: Duration,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    queue_retry_requests_with_dispatcher(&mut swarm_actions, metrics, requests, delay).await
}
pub(super) async fn handle_outbound_request_failure_with_dispatcher(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    driver_state: Arc<Mutex<P2PState>>,
    metrics: Arc<NockchainP2PMetrics>,
    local_peer_id: PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    peer_exclusions: PeerExclusions,
    peer: PeerId,
    request_id: request_response::OutboundRequestId,
    error: request_response::OutboundFailure,
) {
    let request_context = driver_state
        .lock()
        .await
        .remove_outbound_request(request_id);
    driver_state.lock().await.record_request_failure(peer);
    if peer_exclusions.record_peer_request_failure(peer) {
        metrics.request_peer_cooldowns_created.increment();
    }
    let timed_out = matches!(&error, request_response::OutboundFailure::Timeout);
    let unsupported_protocols = matches!(
        &error,
        request_response::OutboundFailure::UnsupportedProtocols
    );
    let transient_failure = transient_outbound_failure(&error);
    let mut skip_transient_retry = false;
    let mut retry_item_filter = None;

    if timed_out {
        if let Some(request_context) = request_context.as_ref() {
            match &request_context.request {
                NockchainRequest::Request { .. } => {
                    if let Some((height, request_message)) =
                        range_singleton_fallback_message(&request_context.request).or_else(|| {
                            block_height_single_request_message(&request_context.request)
                        })
                    {
                        match queue_block_height_retry_to_alternate_peer_with_dispatcher(
                            swarm_actions, &driver_state, height, request_message,
                        )
                        .await
                        {
                            Ok(true) => {
                                skip_transient_retry = true;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                warn!(
                                    peer = %peer,
                                    request_id = %request_id,
                                    height,
                                    error = %err,
                                    "Failed to queue alternate peer retry after block-by-height timeout"
                                );
                            }
                        }
                    }
                }
                NockchainRequest::BatchRequest { items, .. } => {
                    let mut retry_item_ids = BTreeSet::new();
                    for item in items {
                        let Some((height, request_message)) =
                            block_range_singleton_fallback_message(&item.message).or_else(|| {
                                block_height_from_request_message(&item.message)
                                    .map(|height| (height, item.message.clone()))
                            })
                        else {
                            if request_message_block_range_start(&item.message).is_none() {
                                retry_item_ids.insert(item.item_id);
                            }
                            continue;
                        };
                        match queue_block_height_retry_to_alternate_peer_with_dispatcher(
                            swarm_actions, &driver_state, height, request_message,
                        )
                        .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                if request_message_block_range_start(&item.message).is_none() {
                                    retry_item_ids.insert(item.item_id);
                                }
                            }
                            Err(err) => {
                                warn!(
                                    peer = %peer,
                                    request_id = %request_id,
                                    item_id = item.item_id,
                                    height,
                                    error = %err,
                                    "Failed to queue alternate peer retry after batch block-by-height timeout"
                                );
                                retry_item_ids.insert(item.item_id);
                            }
                        }
                    }
                    if items.is_empty() || retry_item_ids.len() == items.len() {
                        retry_item_filter = None;
                    } else {
                        retry_item_filter = Some(retry_item_ids);
                        if retry_item_filter.as_ref().is_some_and(BTreeSet::is_empty) {
                            skip_transient_retry = true;
                        }
                    }
                }
                NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {}
            }
        }
    }

    if unsupported_protocols {
        if let Some(request_context) = request_context.as_ref() {
            let range_fallbacks = range_singleton_fallback_messages(&request_context.request);
            if !range_fallbacks.is_empty() {
                let mut fallback_count = 0usize;
                for (height, request_message) in range_fallbacks {
                    let request_slab = match request_slab_from_message(&request_message) {
                        Ok(request_slab) => request_slab,
                        Err(err) => {
                            warn!(
                                peer = %peer,
                                request_id = %request_id,
                                height,
                                error = %err,
                                "Failed to decode singleton fallback after range unsupported-protocol failure"
                            );
                            continue;
                        }
                    };
                    let request = NockchainRequest::new_request(
                        equix_builder, &local_peer_id, &request_context.peer_id, &request_slab,
                    );
                    let fallback_context = OutboundRequestContext::with_attempt(
                        request_context.peer_id,
                        ReqResGeneration::Gen1,
                        request.clone(),
                        request_context.retry_count.saturating_add(1),
                        true,
                    );
                    match swarm_actions
                        .dispatch(SwarmAction::SendRequest {
                            peer_id: request_context.peer_id,
                            request,
                            request_context: Some(fallback_context),
                        })
                        .await
                    {
                        Ok(()) => {
                            fallback_count = fallback_count.saturating_add(1);
                        }
                        Err(err) => {
                            warn!(
                                peer = %peer,
                                request_id = %request_id,
                                height,
                                error = %err,
                                "Failed to queue singleton fallback after range unsupported-protocol failure"
                            );
                        }
                    }
                }
                record_req_res_fallback(metrics.as_ref(), fallback_count);
                if fallback_count > 0 {
                    info!(
                        peer = %peer,
                        request_id = %request_id,
                        fallback_count,
                        retry_count = request_context.retry_count.saturating_add(1),
                        "Queued gen1 singleton fallback after range unsupported-protocol failure"
                    );
                }
                skip_transient_retry = true;
            } else {
                match enqueue_unsupported_protocol_fallback(
                    swarm_actions, request_context, &local_peer_id, equix_builder,
                )
                .await
                {
                    Ok(fallback_count) if fallback_count > 0 => {
                        record_req_res_fallback(metrics.as_ref(), fallback_count);
                        info!(
                            peer = %peer,
                            request_id = %request_id,
                            fallback_count,
                            retry_count = request_context.retry_count.saturating_add(1),
                            "Queued gen1 fallback after gen2 unsupported-protocol failure"
                        );
                    }
                    Ok(_) => {
                        debug!(
                            peer = %peer,
                            request_id = %request_id,
                            "No explicit fallback queued for unsupported-protocol failure"
                        );
                    }
                    Err(err) => {
                        warn!(
                            peer = %peer,
                            request_id = %request_id,
                            error = %err,
                            "Failed to queue unsupported-protocol fallback"
                        );
                    }
                }
            }
        }
    }

    if transient_failure && !skip_transient_retry {
        if let Some(request_context) = request_context.as_ref() {
            match build_retry_request_contexts(
                request_context,
                &local_peer_id,
                equix_builder,
                retry_item_filter.as_ref(),
            ) {
                Ok(retry_requests) if !retry_requests.is_empty() => {
                    let delay = retry_delay_for_attempt(retry_requests[0].retry_count);
                    if let Err(err) = queue_retry_requests_with_dispatcher(
                        swarm_actions,
                        metrics.as_ref(),
                        retry_requests,
                        delay,
                    )
                    .await
                    {
                        warn!(
                            peer = %peer,
                            request_id = %request_id,
                            error = %err,
                            "Failed to schedule transient req-res retry"
                        );
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        peer = %peer,
                        request_id = %request_id,
                        error = %err,
                        "Failed to build transient req-res retry request"
                    );
                }
            }
        }
    }

    if let Some(request_context) = request_context.as_ref() {
        let mut state_guard = driver_state.lock().await;
        if unsupported_protocols && request_context.generation == ReqResGeneration::Gen2 {
            state_guard.observe_peer_generation(request_context.peer_id, ReqResGeneration::Gen1);
            if request_is_block_range_with_txs(&request_context.request) {
                state_guard.mark_peer_non_range_capable(request_context.peer_id);
            }
        }
        state_guard.record_outbound_failure(
            request_context,
            timed_out,
            request_context.logical_request_count().unwrap_or(0),
        );
    }

    log_outbound_failure(peer, request_id, error, request_context.as_ref(), metrics);
}

pub(crate) async fn handle_outbound_request_failure(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    driver_state: Arc<Mutex<P2PState>>,
    metrics: Arc<NockchainP2PMetrics>,
    local_peer_id: PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    peer_exclusions: PeerExclusions,
    peer: PeerId,
    request_id: request_response::OutboundRequestId,
    error: request_response::OutboundFailure,
) {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    handle_outbound_request_failure_with_dispatcher(
        &mut swarm_actions, driver_state, metrics, local_peer_id, equix_builder, peer_exclusions,
        peer, request_id, error,
    )
    .await
}
pub(super) fn should_batch_request(
    request_context: &OutboundRequestContext,
    req_res_gen2_send_enabled: bool,
    peer_supports_gen2: bool,
    gen2_item_max_bytes: usize,
    gen2_batch_max_bytes: usize,
) -> bool {
    req_res_gen2_send_enabled
        && peer_supports_gen2
        && request_context.generation == ReqResGeneration::Gen2
        && matches!(
            request_context.request,
            NockchainRequest::Request { ref message, .. }
                if message.len() <= gen2_item_max_bytes
                    && (std::mem::size_of::<u32>()
                        + std::mem::size_of::<u32>()
                        + std::mem::size_of::<u32>()
                        + message.len())
                        <= gen2_batch_max_bytes
        )
}
pub(super) fn request_message_can_join_batch(
    generation: ReqResGeneration,
    request_message: &[u8],
    gen2_item_max_bytes: usize,
    gen2_batch_max_bytes: usize,
) -> bool {
    generation == ReqResGeneration::Gen2
        && request_message.len() <= gen2_item_max_bytes
        && (std::mem::size_of::<u32>()
            + std::mem::size_of::<u32>()
            + std::mem::size_of::<u32>()
            + request_message.len())
            <= gen2_batch_max_bytes
}
pub(super) fn request_is_block_by_height(request: &NockchainRequest) -> bool {
    match request {
        NockchainRequest::Request { message, .. } => request_message_is_block_by_height(message),
        NockchainRequest::BatchRequest { .. }
        | NockchainRequest::Gossip { .. }
        | NockchainRequest::AuthenticatedGossip { .. } => false,
    }
}

pub(super) fn request_uses_response_budget(request: &NockchainRequest) -> bool {
    match request {
        NockchainRequest::Request { message, .. } => request_message_uses_response_budget(message),
        NockchainRequest::BatchRequest { .. }
        | NockchainRequest::Gossip { .. }
        | NockchainRequest::AuthenticatedGossip { .. } => false,
    }
}

pub(super) fn request_message_is_block_by_height(message: &[u8]) -> bool {
    matches!(
        decode_request_item_message(message),
        Ok(NockchainDataRequest::BlockByHeight(_))
    )
}

pub(super) fn request_message_uses_response_budget(message: &[u8]) -> bool {
    matches!(
        decode_request_item_message(message),
        Ok(NockchainDataRequest::BlockByHeight(_)
            | NockchainDataRequest::BlockWithTxsByHeight(_)
            | NockchainDataRequest::BlockRangeWithTxs { .. }
            | NockchainDataRequest::RawTransactionById(_, _))
    )
}

pub(super) fn request_is_block_range_with_txs(request: &NockchainRequest) -> bool {
    match request {
        NockchainRequest::Request { message, .. } => {
            request_message_block_range_start(message).is_some()
        }
        NockchainRequest::BatchRequest { items, .. } => items
            .iter()
            .any(|item| request_message_block_range_start(&item.message).is_some()),
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => false,
    }
}

pub(super) fn request_message_is_raw_tx_by_id(message: &[u8]) -> bool {
    matches!(
        decode_request_item_message(message),
        Ok(NockchainDataRequest::RawTransactionById(_, _))
    )
}

pub(super) fn request_message_block_range_start(message: &[u8]) -> Option<u64> {
    match decode_request_item_message(message) {
        Ok(NockchainDataRequest::BlockRangeWithTxs { start_height, .. }) => Some(start_height),
        Ok(NockchainDataRequest::BlockByHeight(_))
        | Ok(NockchainDataRequest::BlockWithTxsByHeight(_))
        | Ok(NockchainDataRequest::EldersById(_, _, _))
        | Ok(NockchainDataRequest::RawTransactionById(_, _))
        | Err(_) => None,
    }
}

pub(super) fn block_range_singleton_fallback_message(message: &[u8]) -> Option<(u64, ByteBuf)> {
    request_message_block_range_start(message)
        .map(|height| (height, block_by_height_message(height)))
}

pub(super) fn block_height_from_request_message(message: &[u8]) -> Option<u64> {
    match decode_request_item_message(message) {
        Ok(NockchainDataRequest::BlockByHeight(height))
        | Ok(NockchainDataRequest::BlockWithTxsByHeight(height)) => Some(height),
        Ok(NockchainDataRequest::BlockRangeWithTxs { .. }) => None,
        Ok(NockchainDataRequest::EldersById(_, _, _))
        | Ok(NockchainDataRequest::RawTransactionById(_, _))
        | Err(_) => None,
    }
}
pub(super) fn block_height_single_request_message(
    request: &NockchainRequest,
) -> Option<(u64, ByteBuf)> {
    let NockchainRequest::Request { message, .. } = request else {
        return None;
    };
    block_height_from_request_message(message).map(|height| (height, message.clone()))
}

pub(super) fn range_singleton_fallback_message(
    request: &NockchainRequest,
) -> Option<(u64, ByteBuf)> {
    let NockchainRequest::Request { message, .. } = request else {
        return None;
    };
    block_range_singleton_fallback_message(message)
}

pub(super) fn range_singleton_fallback_messages(request: &NockchainRequest) -> Vec<(u64, ByteBuf)> {
    match request {
        NockchainRequest::Request { .. } => range_singleton_fallback_message(request)
            .into_iter()
            .collect(),
        NockchainRequest::BatchRequest { items, .. } => items
            .iter()
            .filter_map(|item| block_range_singleton_fallback_message(&item.message))
            .collect(),
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            Vec::new()
        }
    }
}

pub(super) fn block_height_single_request(request: &NockchainRequest) -> Option<u64> {
    let NockchainRequest::Request { message, .. } = request else {
        return None;
    };
    block_height_from_request_message(message)
}
pub(super) fn block_height_batch_item_message(
    request_context: Option<&OutboundRequestContext>,
    item_id: u32,
) -> Option<(u64, ByteBuf)> {
    let NockchainRequest::BatchRequest { items, .. } = &request_context?.request else {
        return None;
    };
    let item = items.iter().find(|item| item.item_id == item_id)?;
    block_height_from_request_message(&item.message).map(|height| (height, item.message.clone()))
}

pub(super) fn block_height_batch_item(
    request_context: Option<&OutboundRequestContext>,
    item_id: u32,
) -> Option<u64> {
    let NockchainRequest::BatchRequest { items, .. } = &request_context?.request else {
        return None;
    };
    let item = items.iter().find(|item| item.item_id == item_id)?;
    block_height_from_request_message(&item.message)
}

pub(super) fn data_request_batch_item(
    request_context: Option<&OutboundRequestContext>,
    item_id: u32,
) -> Option<NockchainDataRequest> {
    let NockchainRequest::BatchRequest { items, .. } = &request_context?.request else {
        return None;
    };
    let item = items.iter().find(|item| item.item_id == item_id)?;
    decode_request_item_message(&item.message).ok()
}

/// Return `Some(height)` only when the outbound batch item at `item_id` was a
/// `BlockWithTxsByHeight` request (the bundle variant). Used by the
/// Decode-fallback arm to distinguish "peer can't decode bundle" from "peer
/// can't decode an ordinary by-height request" before downgrading the item.
pub(super) fn bundle_request_batch_item_height(
    request_context: Option<&OutboundRequestContext>,
    item_id: u32,
) -> Option<u64> {
    let NockchainRequest::BatchRequest { items, .. } = &request_context?.request else {
        return None;
    };
    let item = items.iter().find(|item| item.item_id == item_id)?;
    match decode_request_item_message(&item.message) {
        Ok(NockchainDataRequest::BlockWithTxsByHeight(height)) => Some(height),
        _ => None,
    }
}
pub(super) async fn queue_block_height_retry_to_alternate_peer_with_dispatcher(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    driver_state: &Arc<Mutex<P2PState>>,
    height: u64,
    request_message: ByteBuf,
) -> Result<bool, NockAppError> {
    let retry_peer = {
        let mut state_guard = driver_state.lock().await;
        // Phase 5: every entry to this function is triggered by an upstream
        // failure (timeout / abort / decode error). Account it against the
        // height before deciding whether to retry. Once the rolling
        // failure count crosses the configured budget the height enters
        // backoff and we let the kernel timer arm reissue at its own
        // cadence rather than spraying alternates.
        let budget = state_guard.prefetch_height_failure_budget();
        let backoff = state_guard.prefetch_stuck_backoff();
        let failures = state_guard.record_block_height_failure(height, budget, backoff);
        if failures >= budget {
            trace!(
                height, failures, budget,
                "Block-height retries exhausted: marking stuck and backing off"
            );
            return Ok(false);
        }
        if state_guard.is_block_height_stuck(height) {
            trace!(height, "Skipping alternate-peer retry: height is stuck");
            return Ok(false);
        }
        let attempted_peers = state_guard
            .get_block_height_attempted_peers(height)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let attempted_peer_count = attempted_peers.len();
        let all_connected_peers = state_guard
            .peer_connections
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let connected_peer_count = all_connected_peers.len();
        let mut candidate_peers = all_connected_peers.clone();
        candidate_peers.retain(|peer_id| !attempted_peers.contains(peer_id));
        let mut recycled_attempts = false;

        if candidate_peers.is_empty() && !all_connected_peers.is_empty() {
            state_guard.clear_block_height_attempted_peers(height);
            recycled_attempts = true;
            trace!(
                height, attempted_peer_count, connected_peer_count, recycled_attempts,
                "Recycled block-by-height retry peer history after exhausting alternates"
            );
            return Ok(false);
        }

        let candidate_peer_count = candidate_peers.len();
        let selected_peers = select_request_peers_with_preferences(candidate_peers, 1, true, &[]);
        if !selected_peers.is_empty() {
            state_guard.track_block_height_attempted_peers(height, selected_peers.iter().copied());
        }
        let retry_peer = selected_peers.into_iter().next();
        trace!(
            height,
            attempted_peer_count,
            connected_peer_count,
            candidate_peer_count,
            recycled_attempts,
            selected_peer = ?retry_peer,
            "Evaluated alternate peer retry for block-by-height request"
        );
        retry_peer
    };

    let Some(peer_id) = retry_peer else {
        return Ok(false);
    };

    swarm_actions
        .dispatch(SwarmAction::QueueKernelRequest {
            peer_id,
            request_message,
        })
        .await
        .map_err(|_| {
            NockAppError::OtherError(String::from(
                "Failed to queue alternate peer block-by-height retry",
            ))
        })?;
    Ok(true)
}

pub(super) async fn queue_block_height_retry_to_alternate_peer(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    driver_state: &Arc<Mutex<P2PState>>,
    height: u64,
    request_message: ByteBuf,
) -> Result<bool, NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    queue_block_height_retry_to_alternate_peer_with_dispatcher(
        &mut swarm_actions, driver_state, height, request_message,
    )
    .await
}
pub(super) fn transient_outbound_failure(error: &request_response::OutboundFailure) -> bool {
    matches!(
        error,
        request_response::OutboundFailure::Timeout
            | request_response::OutboundFailure::ConnectionClosed
            | request_response::OutboundFailure::Io(_)
    )
}
pub(super) fn retryable_batch_error_class(error: Option<BatchErrorClass>) -> bool {
    matches!(
        error,
        Some(BatchErrorClass::Backpressure) | Some(BatchErrorClass::Internal)
    )
}

pub(super) fn bundle_error_triggers_classic_fallback(error: Option<BatchErrorClass>) -> bool {
    matches!(
        error,
        Some(BatchErrorClass::Decode) | Some(BatchErrorClass::TooLarge)
    )
}

pub(super) fn retry_delay_for_attempt(retry_count: u8) -> Duration {
    let exponent = retry_count.saturating_sub(1).min(4);
    let base_delay_ms = GEN2_RETRY_BASE_DELAY_MS.saturating_mul(1u64 << exponent);
    let jitter_ms = rng().random_range(0..=GEN2_RETRY_MAX_JITTER_MS);
    Duration::from_millis((base_delay_ms + jitter_ms).min(GEN2_RETRY_MAX_DELAY_MS))
}
pub(super) fn retry_chunk_size(item_count: usize, retry_count: u8) -> usize {
    let divisor = 1usize << retry_count.saturating_sub(1).min(4);
    item_count.div_ceil(divisor).max(1)
}

pub(crate) fn build_retry_request_contexts(
    request_context: &OutboundRequestContext,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    retry_item_ids: Option<&BTreeSet<u32>>,
) -> Result<Vec<OutboundRequestContext>, NockAppError> {
    let next_retry_count = request_context.retry_count.saturating_add(1);
    if next_retry_count > GEN2_RETRY_MAX_ATTEMPTS {
        return Ok(Vec::new());
    }

    match &request_context.request {
        NockchainRequest::BatchRequest { items, .. } => {
            let selected_items = items
                .iter()
                .filter(|item| {
                    if request_message_block_range_start(&item.message).is_some() {
                        return false;
                    }
                    retry_item_ids
                        .map(|item_ids| item_ids.contains(&item.item_id))
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>();
            if selected_items.is_empty() {
                return Ok(Vec::new());
            }

            let chunk_size = retry_chunk_size(selected_items.len(), next_retry_count);
            selected_items
                .chunks(chunk_size)
                .map(|chunk| {
                    let request = NockchainRequest::new_batch_request(
                        equix_builder,
                        local_peer_id,
                        &request_context.peer_id,
                        chunk.to_vec(),
                    )?;
                    Ok(OutboundRequestContext::with_attempt(
                        request_context.peer_id,
                        ReqResGeneration::Gen2,
                        request,
                        next_retry_count,
                        request_context.fallback_attempted,
                    ))
                })
                .collect()
        }
        NockchainRequest::Request { .. } => {
            if retry_item_ids.is_some() {
                return Ok(Vec::new());
            }
            if request_is_block_range_with_txs(&request_context.request) {
                return Ok(Vec::new());
            }
            Ok(vec![OutboundRequestContext::with_attempt(
                request_context.peer_id,
                request_context.generation,
                request_context.request.clone(),
                next_retry_count,
                request_context.fallback_attempted,
            )])
        }
        NockchainRequest::Gossip { .. } | NockchainRequest::AuthenticatedGossip { .. } => {
            Ok(Vec::new())
        }
    }
}

pub(super) async fn schedule_request_context_retry_with_dispatcher(
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    metrics: &NockchainP2PMetrics,
    request_context: &OutboundRequestContext,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    retry_item_ids: Option<&BTreeSet<u32>>,
) -> Result<bool, NockAppError> {
    let retry_requests = build_retry_request_contexts(
        request_context, local_peer_id, equix_builder, retry_item_ids,
    )?;
    if retry_requests.is_empty() {
        return Ok(false);
    }

    let delay = retry_delay_for_attempt(retry_requests[0].retry_count);
    queue_retry_requests_with_dispatcher(swarm_actions, metrics, retry_requests, delay).await?;
    Ok(true)
}

pub(super) async fn schedule_request_context_retry(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    metrics: &NockchainP2PMetrics,
    request_context: &OutboundRequestContext,
    local_peer_id: &PeerId,
    equix_builder: &mut equix::EquiXBuilder,
    retry_item_ids: Option<&BTreeSet<u32>>,
) -> Result<bool, NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    schedule_request_context_retry_with_dispatcher(
        &mut swarm_actions, metrics, request_context, local_peer_id, equix_builder, retry_item_ids,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn process_queue_kernel_request_action(
    peer_id: PeerId,
    request_message: ByteBuf,
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
    equix_builder: &mut equix::EquiXBuilder,
    peer_gen2_inbound: &mut BTreeMap<PeerId, bool>,
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    req_res_gen2_send_enabled: bool,
    req_res_limits: ReqResRuntimeLimits,
) -> Result<(), NockAppError> {
    if suppress_duplicate_active_outbound_request(
        driver_state, metrics, peer_id, "request", &request_message,
    )
    .await
    {
        return Ok(());
    }

    let peer_supports_gen2 = peer_gen2_inbound.get(&peer_id).copied().unwrap_or(false);
    let generation = if req_res_gen2_send_enabled && peer_supports_gen2 {
        ReqResGeneration::Gen2
    } else {
        ReqResGeneration::Gen1
    };
    let local_peer_id = *swarm.local_peer_id();
    if generation != ReqResGeneration::Gen2 {
        if let Some((height, classic_message)) =
            block_range_singleton_fallback_message(&request_message)
        {
            warn!(
                peer = %peer_id,
                height,
                peer_supports_gen2,
                req_res_gen2_send_enabled,
                "Suppressing range request to non-gen2 peer"
            );
            metrics.prefetch_peer_no_gen2_range_peer_total.increment();
            let request_slab = request_slab_from_message(&classic_message)?;
            let request = NockchainRequest::new_request(
                equix_builder, &local_peer_id, &peer_id, &request_slab,
            );
            record_block_by_height_gen1_routed(
                metrics, generation, req_res_gen2_send_enabled, peer_supports_gen2, &request,
            );
            send_outbound_request_now(
                swarm,
                driver_state,
                metrics,
                OutboundRequestContext::new(peer_id, generation, request),
            )
            .await;
            return Ok(());
        }
    }
    let batchable = request_message_can_join_batch(
        generation, &request_message, req_res_limits.gen2_item_max_bytes,
        req_res_limits.gen2_batch_max_bytes,
    );

    if !batchable {
        let request_slab = request_slab_from_message(&request_message)?;
        let request =
            NockchainRequest::new_request(equix_builder, &local_peer_id, &peer_id, &request_slab);
        record_block_by_height_gen1_routed(
            metrics, generation, req_res_gen2_send_enabled, peer_supports_gen2, &request,
        );
        send_outbound_request_now(
            swarm,
            driver_state,
            metrics,
            OutboundRequestContext::new(peer_id, generation, request),
        )
        .await;
        return Ok(());
    }

    let contains_response_budget_item = request_message_uses_response_budget(&request_message);
    let estimated_response_bytes = outbound_request_message_estimated_response_bytes(
        &request_message, req_res_limits, driver_state,
    )
    .await;

    let flush_reason = pending_gen2_batches
        .get(&peer_id)
        .map(|pending_batch| {
            pending_batch_pre_insert_flush_reason(
                pending_batch,
                request_message.len(),
                estimated_response_bytes,
                contains_response_budget_item,
                req_res_limits,
            )
        })
        .transpose()?
        .flatten();
    if let Some(flush_reason) = flush_reason {
        if let Some(pending_batch) = pending_gen2_batches.get(&peer_id) {
            log_pending_gen2_batch_flush(&peer_id, flush_reason, pending_batch, req_res_limits);
        }
        if let Some(flushed_batch) = take_pending_batch_request(
            pending_gen2_batches, peer_id, &local_peer_id, equix_builder,
        )? {
            send_outbound_request_now(swarm, driver_state, metrics, flushed_batch).await;
        }
        update_pending_batch_metrics(metrics, pending_gen2_batches);
    }

    let insert_outcome = queue_pending_gen2_batch_request(
        metrics, pending_gen2_batches, peer_id, &request_message, estimated_response_bytes,
        contains_response_budget_item,
    )?;
    match insert_outcome {
        PendingBatchInsertOutcome::Duplicate => {
            log_pending_gen2_batch_duplicate(&peer_id, "request", &request_message);
        }
        PendingBatchInsertOutcome::Inserted {
            item_count,
            payload_bytes,
            estimated_response_bytes,
            contains_response_budget_item,
        } => {
            if let Some(flush_reason) = inserted_batch_flush_reason(
                item_count, payload_bytes, estimated_response_bytes, contains_response_budget_item,
                req_res_limits,
            ) {
                if let Some(pending_batch) = pending_gen2_batches.get(&peer_id) {
                    log_pending_gen2_batch_flush(
                        &peer_id, flush_reason, pending_batch, req_res_limits,
                    );
                }
                if let Some(flushed_batch) = take_pending_batch_request(
                    pending_gen2_batches, peer_id, &local_peer_id, equix_builder,
                )? {
                    send_outbound_request_now(swarm, driver_state, metrics, flushed_batch).await;
                }
                update_pending_batch_metrics(metrics, pending_gen2_batches);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_send_request_action(
    peer_id: PeerId,
    request: NockchainRequest,
    request_context: Option<OutboundRequestContext>,
    swarm: &mut Swarm<NockchainBehaviour>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &Arc<NockchainP2PMetrics>,
    equix_builder: &mut equix::EquiXBuilder,
    peer_gen2_inbound: &mut BTreeMap<PeerId, bool>,
    pending_gen2_batches: &mut BTreeMap<PeerId, PendingGen2Batch>,
    req_res_gen2_send_enabled: bool,
    req_res_limits: ReqResRuntimeLimits,
) -> Result<(), NockAppError> {
    let peer_supports_gen2 = peer_gen2_inbound.get(&peer_id).copied().unwrap_or(false);
    let mut request_context = request_context.unwrap_or_else(|| {
        let generation =
            outbound_request_generation(&request, req_res_gen2_send_enabled, peer_supports_gen2);
        record_block_by_height_gen1_routed(
            metrics, generation, req_res_gen2_send_enabled, peer_supports_gen2, &request,
        );
        OutboundRequestContext::new(peer_id, generation, request.clone())
    });
    let local_peer_id = *swarm.local_peer_id();
    if should_authenticate_outbound_gossip(
        &request_context.request, req_res_limits, req_res_gen2_send_enabled, peer_supports_gen2,
        request_context.generation,
    ) {
        request_context.request = request_context
            .request
            .authenticate_gossip(equix_builder, &local_peer_id, &peer_id)?;
    }
    if let NockchainRequest::Request { message, .. } = &request_context.request {
        if suppress_duplicate_active_outbound_request(
            driver_state,
            metrics,
            peer_id,
            outbound_request_shape(&request_context.request),
            message,
        )
        .await
        {
            return Ok(());
        }
    }
    let batchable = should_batch_request(
        &request_context, req_res_gen2_send_enabled, peer_supports_gen2,
        req_res_limits.gen2_item_max_bytes, req_res_limits.gen2_batch_max_bytes,
    );

    if !batchable {
        send_outbound_request_now(swarm, driver_state, metrics, request_context).await;
        return Ok(());
    }

    let NockchainRequest::Request { message, .. } = &request_context.request else {
        unreachable!("batchable requests must be singleton requests");
    };
    let contains_response_budget_item = request_uses_response_budget(&request_context.request);
    let estimated_response_bytes = outbound_request_estimated_response_bytes(
        &request_context.request, req_res_limits, driver_state,
    )
    .await;

    let flush_reason = pending_gen2_batches
        .get(&peer_id)
        .map(|pending_batch| {
            pending_batch_pre_insert_flush_reason(
                pending_batch,
                message.len(),
                estimated_response_bytes,
                contains_response_budget_item,
                req_res_limits,
            )
        })
        .transpose()?
        .flatten();
    if let Some(flush_reason) = flush_reason {
        if let Some(pending_batch) = pending_gen2_batches.get(&peer_id) {
            log_pending_gen2_batch_flush(&peer_id, flush_reason, pending_batch, req_res_limits);
        }
        if let Some(flushed_batch) = take_pending_batch_request(
            pending_gen2_batches, peer_id, &local_peer_id, equix_builder,
        )? {
            send_outbound_request_now(swarm, driver_state, metrics, flushed_batch).await;
        }
        update_pending_batch_metrics(metrics, pending_gen2_batches);
    }

    let insert_outcome = queue_pending_gen2_batch_request(
        metrics, pending_gen2_batches, peer_id, message, estimated_response_bytes,
        contains_response_budget_item,
    )?;
    match insert_outcome {
        PendingBatchInsertOutcome::Duplicate => {
            log_pending_gen2_batch_duplicate(
                &peer_id,
                outbound_request_shape(&request_context.request),
                message,
            );
        }
        PendingBatchInsertOutcome::Inserted {
            item_count,
            payload_bytes,
            estimated_response_bytes,
            contains_response_budget_item,
        } => {
            if let Some(flush_reason) = inserted_batch_flush_reason(
                item_count, payload_bytes, estimated_response_bytes, contains_response_budget_item,
                req_res_limits,
            ) {
                if let Some(pending_batch) = pending_gen2_batches.get(&peer_id) {
                    log_pending_gen2_batch_flush(
                        &peer_id, flush_reason, pending_batch, req_res_limits,
                    );
                }
                if let Some(flushed_batch) = take_pending_batch_request(
                    pending_gen2_batches, peer_id, &local_peer_id, equix_builder,
                )? {
                    send_outbound_request_now(swarm, driver_state, metrics, flushed_batch).await;
                }
                update_pending_batch_metrics(metrics, pending_gen2_batches);
            }
        }
    }
    Ok(())
}

pub(super) fn spawn_retry_requests(
    join_set: &mut TrackedJoinSet<Result<(), NockAppError>>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
    requests: Vec<OutboundRequestContext>,
    delay: Duration,
) {
    let swarm_tx = swarm_tx.clone();
    join_set.spawn("req_res_retry".to_string(), async move {
        tokio::time::sleep(delay).await;
        for request_context in requests {
            let request = request_context.request.clone();
            swarm_tx
                .send(SwarmAction::SendRequest {
                    peer_id: request_context.peer_id,
                    request,
                    request_context: Some(request_context),
                })
                .await
                .map_err(|_| {
                    NockAppError::OtherError(String::from("Failed to queue retry request"))
                })?;
        }
        Ok(())
    });
}

pub(super) async fn process_flush_deferred_heard_blocks_action(
    buffered_swarm_actions: &mut VecDeque<SwarmAction>,
    traffic_cop: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Buffered(buffered_swarm_actions);
    let flushed = flush_ready_deferred_heard_blocks_with_dispatcher(
        traffic_cop, metrics, driver_state, &mut swarm_actions,
    )
    .await?;
    trace!(flushed, "Processed deferred heard-block flush action");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_request_response(
    peer: PeerId,
    connection_id: ConnectionId,
    message: request_response::Message<NockchainRequest, NockchainResponse>,
    swarm_tx: mpsc::Sender<SwarmAction>,
    equix_builder: &mut equix::EquiXBuilder,
    local_peer_id: PeerId,
    traffic: traffic_cop::TrafficCop,
    metrics: Arc<NockchainP2PMetrics>,
    driver_state: Arc<Mutex<P2PState>>,
    req_res_limits: ReqResRuntimeLimits,
    peer_exclusions: PeerExclusions,
) -> Result<(), NockAppError> {
    trace!("handle_request_response peer: {peer}");
    match message {
        Request {
            request, channel, ..
        } => {
            handle_inbound_request(
                peer,
                connection_id,
                request,
                channel,
                swarm_tx,
                equix_builder,
                local_peer_id,
                traffic,
                metrics,
                driver_state,
                req_res_limits,
                peer_exclusions.clone(),
            )
            .await?;
        }
        Response {
            request_id,
            response,
        } => {
            handle_outbound_response(
                peer, request_id, response, swarm_tx, equix_builder, local_peer_id, traffic,
                metrics, driver_state, peer_exclusions,
            )
            .await?;
        }
    }
    Ok(())
}
