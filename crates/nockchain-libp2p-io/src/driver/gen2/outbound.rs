use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use libp2p::{request_response, PeerId};
use nockapp::NockAppError;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, trace, warn};

use crate::driver::gen2::*;
use crate::driver::{
    record_local_peer_abuse, LocalPeerAbuseKind, LocalPeerAbuseSeverity, SwarmAction,
    SwarmActionDispatcher,
};
use crate::ip_block::PeerExclusions;
use crate::messages::{
    block_by_height_message, decode_request_item_message, BatchResultItem, BatchResultStatus,
    EnvelopeKind, NockchainDataRequest, NockchainFact, NockchainResponse, ResponseEnvelope,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::P2PState;
use crate::traffic_cop;

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_outbound_response(
    peer: PeerId,
    request_id: request_response::OutboundRequestId,
    response: NockchainResponse,
    swarm_tx: mpsc::Sender<SwarmAction>,
    equix_builder: &mut equix::EquiXBuilder,
    local_peer_id: PeerId,
    traffic: traffic_cop::TrafficCop,
    metrics: Arc<NockchainP2PMetrics>,
    driver_state: Arc<Mutex<P2PState>>,
    peer_exclusions: PeerExclusions,
) -> Result<(), NockAppError> {
    let request_context = {
        let state_guard = driver_state.lock().await;
        state_guard.outbound_request_context(request_id).cloned()
    };
    let response_bytes = match req_res_message_encoded_bytes(&response) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(
                peer = %peer,
                request_id = %request_id,
                error = %err,
                "Failed to encode req-res response for peer stats"
            );
            0
        }
    };
    let response_item_count = match &response {
        NockchainResponse::BatchResult { results } => results.len(),
        NockchainResponse::Result { .. } | NockchainResponse::Ack { .. } => 1,
    };
    let response_failure_count = match &response {
        NockchainResponse::BatchResult { results } => count_batch_result_item_failures(results),
        NockchainResponse::Result { .. } | NockchainResponse::Ack { .. } => 0,
    };
    let response_composition = ResponseComposition::from_response(&response);
    if let Some(request_context) = request_context.as_ref() {
        info!(
            peer = %peer,
            request_id = %request_id,
            generation = ?request_context.generation,
            request_shape = outbound_request_shape(&request_context.request),
            batch_items = batch_request_item_count(&request_context.request),
            retry_count = request_context.retry_count,
            fallback_attempted = request_context.fallback_attempted,
            response_bytes,
            response_items = response_item_count,
            response_failures = response_failure_count,
            batch_result_items = response_composition.batch_result_items,
            batch_result_success_items = response_composition.batch_result_success_items,
            batch_result_ack_items = response_composition.batch_result_ack_items,
            batch_result_not_found_items = response_composition.batch_result_not_found_items,
            batch_result_error_items = response_composition.batch_result_error_items,
            bundle_items = response_composition.bundle_items,
            bundle_block_bytes = response_composition.bundle_block_bytes,
            bundle_payload_bytes = response_composition.bundle_payload_bytes,
            bundle_envelope_bytes = response_composition.bundle_envelope_bytes,
            bundled_tx_count = response_composition.bundled_tx_count,
            bundled_tx_bytes = response_composition.bundled_tx_bytes,
            unincluded_tx_count = response_composition.unincluded_tx_count,
            "Nous req-res exchange completed"
        );
    }

    let response_result: Result<u64, NockAppError> = async {
                let mut semantic_failure_count = 0u64;
                if let Err(err) = response.validate() {
                    record_response_validation_abuse(
                        &swarm_tx,
                        &driver_state,
                        &metrics,
                        &peer_exclusions,
                        peer,
                    )
                    .await?;
                    if let Some(request_context) = request_context.as_ref() {
                        if let Err(retry_err) = schedule_request_context_retry(
                            &swarm_tx,
                            &metrics,
                            request_context,
                            &local_peer_id,
                            equix_builder,
                            None,
                        )
                        .await
                        {
                            warn!(
                                peer = %peer,
                                request_id = %request_id,
                                error = %retry_err,
                                "Failed to schedule retry after invalid response"
                            );
                        }
                    }
                    return Err(err);
                }

                match response {
                    NockchainResponse::Result { message } => {
                        trace!("handle_request_response: Response result received");
                        let decode_started = Instant::now();
                        let response = match response_fact_from_result_message(&message) {
                            Ok(response) => response,
                            Err(err) => {
                                record_response_validation_abuse(
                                    &swarm_tx,
                                    &driver_state,
                                    &metrics,
                                    &peer_exclusions,
                                    peer,
                                )
                                .await?;
                                if let Some(request_context) = request_context.as_ref() {
                                    if let Err(retry_err) = schedule_request_context_retry(
                                        &swarm_tx,
                                        &metrics,
                                        request_context,
                                        &local_peer_id,
                                        equix_builder,
                                        None,
                                    )
                                    .await
                                    {
                                        warn!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            error = %retry_err,
                                            "Failed to schedule retry after malformed single response"
                                        );
                                    }
                                }
                                return Err(err);
                            }
                        };
                        if let Some(data_request) =
                            data_request_single_request(request_context.as_ref())
                        {
                            if let Err(err) =
                                validate_response_fact_for_request(&data_request, &response)
                            {
                                record_response_validation_abuse(
                                    &swarm_tx,
                                    &driver_state,
                                    &metrics,
                                    &peer_exclusions,
                                    peer,
                                )
                                .await?;
                                if let Some(request_context) = request_context.as_ref() {
                                    if let Err(retry_err) = schedule_request_context_retry(
                                        &swarm_tx,
                                        &metrics,
                                        request_context,
                                        &local_peer_id,
                                        equix_builder,
                                        None,
                                    )
                                    .await
                                    {
                                        warn!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            error = %retry_err,
                                            "Failed to schedule retry after mismatched single response"
                                        );
                                    }
                                }
                                return Err(err);
                            }
                        }
                        let decode_elapsed = decode_started.elapsed();
                        if let NockchainFact::HeardBlock(block_id, fact_poke) = &response {
                            info!(
                                target: "nockchain::kernel_timing",
                                peer = %peer,
                                request_id = %request_id,
                                item_id = "",
                                block_id = %block_id,
                                block_height = ?heard_block_height_from_fact_poke(fact_poke).ok(),
                                decode_ms = decode_elapsed.as_secs_f64() * 1_000.0,
                                "Decoded req-res response fact"
                            );
                        }
                        trace!(
                            peer = %peer,
                            request_id = %request_id,
                            requested_block_height = ?request_context
                                .as_ref()
                                .and_then(|context| block_height_single_request(&context.request)),
                            response = %response_fact_trace_summary(&response),
                            "Decoded single req-res response fact"
                        );
                        match route_response_fact(
                            peer,
                            response,
                            &traffic,
                            &metrics,
                            &driver_state,
                            &swarm_tx,
                        )
                            .await
                        {
                            Ok(()) => {
                                if let Some(request_context) = request_context.as_ref() {
                                    if let Some((height, _)) =
                                        block_height_single_request_message(&request_context.request)
                                    {
                                        trace!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            height,
                                            "Clearing block-height attempted peers after successful single response"
                                        );
                                        driver_state
                                            .lock()
                                            .await
                                            .clear_block_height_attempted_peers(height);
                                    }
                                }
                            }
                            Err(NockAppError::MPSCFullError(_)) => {
                                if let Some(request_context) = request_context.as_ref() {
                                    if let Err(retry_err) = schedule_request_context_retry(
                                        &swarm_tx,
                                        &metrics,
                                        request_context,
                                        &local_peer_id,
                                        equix_builder,
                                        None,
                                    )
                                    .await
                                    {
                                        warn!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            error = %retry_err,
                                            "Failed to schedule retry after single response backpressure"
                                        );
                                    }
                                }
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    NockchainResponse::Ack { acked } => {
                        trace!("Received acknowledgement from peer {}", peer);
                        if !acked {
                            warn!("Peer {} did not acknowledge the response", peer);
                        }
                        if let Some(request_context) = request_context.as_ref() {
                            if let Some((height, request_message)) =
                                block_height_single_request_message(&request_context.request)
                            {
                                let queued_retry = queue_block_height_retry_to_alternate_peer(
                                    &swarm_tx,
                                    &driver_state,
                                    height,
                                    request_message,
                                )
                                .await?;
                                trace!(
                                    peer = %peer,
                                    request_id = %request_id,
                                    height,
                                    queued_retry,
                                    "Handled block-by-height ack as not-found"
                                );
                            }
                        }
                    }
                    NockchainResponse::BatchResult { results } => {
                        record_batch_result_item_errors(&metrics, &results);
                        let Some(expected_item_ids) =
                            batch_request_item_ids(request_context.as_ref())
                        else {
                            warn!(
                                peer = %peer,
                                request_id = %request_id,
                                item_count = results.len(),
                                "Dropping batch result without retained batch request context"
                            );
                            return Ok(response_failure_count.saturating_add(semantic_failure_count));
                        };

                        let mut retry_item_ids = BTreeSet::new();
                        let mut observed_item_ids = BTreeSet::new();
                        let mut result_iter = results.into_iter().peekable();

                        while let Some(result) = result_iter.next() {
                            if !expected_item_ids.contains(&result.item_id) {
                                semantic_failure_count =
                                    semantic_failure_count.saturating_add(1);
                                metrics.gen2_batch_result_unexpected_item_id.increment();
                                warn!(
                                    peer = %peer,
                                    request_id = %request_id,
                                    item_id = result.item_id,
                                    "Skipping batch result item with no matching outbound request item_id"
                                );
                                continue;
                            }
                            observed_item_ids.insert(result.item_id);

                            match result.status {
                                BatchResultStatus::Result => {
                                    let Some(envelope) = result.envelope.as_ref() else {
                                        semantic_failure_count =
                                            semantic_failure_count.saturating_add(1);
                                        warn!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            item_id = result.item_id,
                                            "Retrying batch result item without response envelope"
                                        );
                                        retry_item_ids.insert(result.item_id);
                                        continue;
                                    };
                                    if let Some(data_request) =
                                        data_request_batch_item(request_context.as_ref(), result.item_id)
                                    {
                                        if let Err(err) =
                                            validate_response_envelope_for_request(&data_request, envelope)
                                        {
                                            semantic_failure_count =
                                                semantic_failure_count.saturating_add(1);
                                            record_response_validation_abuse(
                                                &swarm_tx,
                                                &driver_state,
                                                &metrics,
                                                &peer_exclusions,
                                                peer,
                                            )
                                            .await?;
                                            warn!(
                                                peer = %peer,
                                                request_id = %request_id,
                                                item_id = result.item_id,
                                                error = %err,
                                                "Retrying batch result item with mismatched response envelope"
                                            );
                                            if let NockchainDataRequest::BlockRangeWithTxs {
                                                start_height,
                                                ..
                                            } = data_request
                                            {
                                                driver_state
                                                    .lock()
                                                    .await
                                                    .mark_peer_non_range_capable(peer);
                                                queue_classic_block_by_height_fallback(
                                                    &swarm_tx,
                                                    peer,
                                                    request_id,
                                                    result.item_id,
                                                    start_height,
                                                    "invalid_range_response",
                                                )
                                                .await;
                                            }
                                            retry_item_ids.insert(result.item_id);
                                            continue;
                                        }
                                    }
                                    record_batch_result_response_hint(
                                        &driver_state,
                                        request_context.as_ref(),
                                        result.item_id,
                                        envelope,
                                    )
                                    .await;

                                    let route_outcome: Result<(), NockAppError> = if envelope.kind
                                        == EnvelopeKind::HeardBlockRangeWithTxs
                                    {
                                        trace!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            item_id = result.item_id,
                                            range_block_count = envelope
                                                .range_blocks
                                                .as_ref()
                                                .map(Vec::len)
                                                .unwrap_or(0),
                                            "Unpacking block-range batch result item"
                                        );
                                        let mut range_swarm_actions =
                                            SwarmActionDispatcher::Channel(&swarm_tx);
                                        route_block_range_envelope_with_dispatcher(
                                            peer,
                                            envelope,
                                            &traffic,
                                            &metrics,
                                            &driver_state,
                                            &mut range_swarm_actions,
                                        )
                                        .await
                                    } else if envelope.kind == EnvelopeKind::HeardBlockWithTxs {
                                            trace!(
                                                peer = %peer,
                                                request_id = %request_id,
                                                item_id = result.item_id,
                                                requested_block_height = ?block_height_batch_item(
                                                    request_context.as_ref(),
                                                    result.item_id
                                                ),
                                                block_id = ?envelope.block_id,
                                                bundled_tx_count = envelope
                                                    .tx_envelopes
                                                    .as_ref()
                                                    .map(Vec::len)
                                                    .unwrap_or(0),
                                                unincluded_tx_count = envelope
                                                    .unincluded_tx_ids
                                                    .as_ref()
                                                    .map(Vec::len)
                                                    .unwrap_or(0),
                                                "Unpacking bundle batch result item"
                                            );
                                            let decode_started = Instant::now();
                                            let decoded_block = response_fact_from_result_message(
                                                &envelope.message,
                                            );
                                            let decode_elapsed = decode_started.elapsed();
                                            if let Ok(NockchainFact::HeardBlock(_, fact_poke)) =
                                                decoded_block.as_ref()
                                            {
                                                info!(
                                                    target: "nockchain::kernel_timing",
                                                    peer = %peer,
                                                    request_id = %request_id,
                                                    item_id = result.item_id,
                                                    block_id = %envelope.block_id.as_deref().unwrap_or(""),
                                                    block_height = ?heard_block_height_from_fact_poke(fact_poke).ok(),
                                                    decode_ms = decode_elapsed.as_secs_f64() * 1_000.0,
                                                    bundled_tx_count = envelope
                                                        .tx_envelopes
                                                        .as_ref()
                                                        .map(Vec::len)
                                                        .unwrap_or(0),
                                                    "Decoded bundle block response fact"
                                                );
                                            }
                                            route_bundle_envelope(
                                                peer,
                                                envelope,
                                                &traffic,
                                                &metrics,
                                                &driver_state,
                                                &swarm_tx,
                                            )
                                            .await
                                        } else {
                                            let decode_started = Instant::now();
                                            let response = match response_fact_from_envelope(envelope) {
                                                Ok(response) => response,
                                                Err(err) => {
                                                    semantic_failure_count =
                                                        semantic_failure_count.saturating_add(1);
                                                    record_response_validation_abuse(
                                                        &swarm_tx,
                                                        &driver_state,
                                                        &metrics,
                                                        &peer_exclusions,
                                                        peer,
                                                    )
                                                    .await?;
                                                    warn!(
                                                        peer = %peer,
                                                        request_id = %request_id,
                                                        item_id = result.item_id,
                                                        error = %err,
                                                        "Retrying malformed batch result item payload"
                                                    );
                                                    retry_item_ids.insert(result.item_id);
                                                    continue;
                                                }
                                            };
                                            let decode_elapsed = decode_started.elapsed();
                                            if let NockchainFact::HeardBlock(_, fact_poke) =
                                                &response
                                            {
                                                info!(
                                                    target: "nockchain::kernel_timing",
                                                    peer = %peer,
                                                    request_id = %request_id,
                                                    item_id = result.item_id,
                                                    block_id = %envelope.block_id.as_deref().unwrap_or(""),
                                                    block_height = ?heard_block_height_from_fact_poke(fact_poke).ok(),
                                                    decode_ms = decode_elapsed.as_secs_f64() * 1_000.0,
                                                    "Decoded req-res response fact"
                                                );
                                            }
                                            trace!(
                                                peer = %peer,
                                                request_id = %request_id,
                                                item_id = result.item_id,
                                                requested_block_height = ?block_height_batch_item(
                                                    request_context.as_ref(),
                                                    result.item_id
                                                ),
                                                response = %response_fact_trace_summary(&response),
                                                "Decoded batch req-res result item fact"
                                            );
                                            route_response_fact(
                                                peer,
                                                response,
                                                &traffic,
                                                &metrics,
                                                &driver_state,
                                                &swarm_tx,
                                            )
                                            .await
                                        };

                                    match route_outcome {
                                        Ok(()) => {
                                            if let Some((height, _)) = block_height_batch_item_message(
                                                request_context.as_ref(),
                                                result.item_id,
                                            ) {
                                                trace!(
                                                    peer = %peer,
                                                    request_id = %request_id,
                                                    item_id = result.item_id,
                                                    height,
                                                    "Clearing block-height attempted peers after successful batch item"
                                                );
                                                driver_state
                                                    .lock()
                                                    .await
                                                    .clear_block_height_attempted_peers(height);
                                            }
                                        }
                                        Err(NockAppError::MPSCFullError(_)) => {
                                            semantic_failure_count =
                                                semantic_failure_count.saturating_add(1);
                                            retry_item_ids.insert(result.item_id);
                                            for tail in result_iter {
                                                if expected_item_ids.contains(&tail.item_id) {
                                                    semantic_failure_count =
                                                        semantic_failure_count.saturating_add(1);
                                                    retry_item_ids.insert(tail.item_id);
                                                }
                                            }
                                            break;
                                        }
                                        Err(err) => {
                                            semantic_failure_count =
                                                semantic_failure_count.saturating_add(1);
                                            record_response_validation_abuse(
                                                &swarm_tx,
                                                &driver_state,
                                                &metrics,
                                                &peer_exclusions,
                                                peer,
                                            )
                                            .await?;
                                            warn!(
                                                peer = %peer,
                                                request_id = %request_id,
                                                item_id = result.item_id,
                                                error = %err,
                                                "Retrying malformed batch result item"
                                            );
                                            retry_item_ids.insert(result.item_id);
                                            continue;
                                        }
                                    }
                                }
                                BatchResultStatus::Ack => {
                                    semantic_failure_count =
                                        semantic_failure_count.saturating_add(1);
                                    trace!(
                                        peer = %peer,
                                        request_id = %request_id,
                                        item_id = result.item_id,
                                        "Received batch ack item"
                                    );
                                }
                                BatchResultStatus::NotFound => {
                                    semantic_failure_count =
                                        semantic_failure_count.saturating_add(1);
                                    trace!(
                                        peer = %peer,
                                        request_id = %request_id,
                                        item_id = result.item_id,
                                        "Received batch not-found item"
                                    );
                                    if let Some((height, request_message)) =
                                        block_height_batch_item_message(
                                            request_context.as_ref(),
                                            result.item_id,
                                        )
                                    {
                                        let queued_retry = queue_block_height_retry_to_alternate_peer(
                                            &swarm_tx,
                                            &driver_state,
                                            height,
                                            request_message,
                                        )
                                        .await?;
                                        trace!(
                                            peer = %peer,
                                            request_id = %request_id,
                                            item_id = result.item_id,
                                            height,
                                            queued_retry,
                                            "Handled batch block-by-height not-found item"
                                        );
                                    }
                                }
                                BatchResultStatus::Error => {
                                    if retryable_batch_error_class(result.error) {
                                        retry_item_ids.insert(result.item_id);
                                    }
                                    if bundle_error_triggers_classic_fallback(result.error) {
                                        if let Some(NockchainDataRequest::BlockRangeWithTxs {
                                            start_height,
                                            ..
                                        }) = data_request_batch_item(
                                            request_context.as_ref(),
                                            result.item_id,
                                        ) {
                                            driver_state
                                                .lock()
                                                .await
                                                .mark_peer_non_range_capable(peer);
                                            queue_classic_block_by_height_fallback(
                                                &swarm_tx,
                                                peer,
                                                request_id,
                                                result.item_id,
                                                start_height,
                                                "range_error",
                                            )
                                            .await;
                                        }
                                        if let Some(height) = bundle_request_batch_item_height(
                                            request_context.as_ref(),
                                            result.item_id,
                                        ) {
                                            driver_state
                                                .lock()
                                                .await
                                                .mark_peer_non_bundle_capable(peer);
                                            queue_classic_block_by_height_fallback(
                                                &swarm_tx,
                                                peer,
                                                request_id,
                                                result.item_id,
                                                height,
                                                "bundle_error",
                                            )
                                            .await;
                                        }
                                    }
                                    warn!(
                                        peer = %peer,
                                        request_id = %request_id,
                                        item_id = result.item_id,
                                        error_class = ?result.error,
                                        "Received batch error item"
                                    );
                                }
                            }
                        }

                        let missing_item_ids =
                            missing_batch_result_item_ids(&expected_item_ids, &observed_item_ids);
                        if !missing_item_ids.is_empty() {
                            semantic_failure_count = semantic_failure_count
                                .saturating_add(missing_item_ids.len() as u64);
                            warn!(
                                peer = %peer,
                                request_id = %request_id,
                                missing_item_ids = ?missing_item_ids,
                                "Retrying missing batch result items"
                            );
                            retry_item_ids.extend(missing_item_ids);
                        }

                        if !retry_item_ids.is_empty() {
                            if let Some(request_context) = request_context.as_ref() {
                                if let Err(err) = schedule_request_context_retry(
                                    &swarm_tx,
                                    &metrics,
                                    request_context,
                                    &local_peer_id,
                                    equix_builder,
                                    Some(&retry_item_ids),
                                )
                                .await
                                {
                                    warn!(
                                        peer = %peer,
                                        request_id = %request_id,
                                        error = %err,
                                        "Failed to schedule selective batch retry"
                                    );
                                }
                            }
                        }
                    }
                }

                Ok(response_failure_count.saturating_add(semantic_failure_count))
            }
            .await;
    let recorded_failure_count = match response_result.as_ref() {
        Ok(failure_count) => *failure_count,
        Err(_) => response_failure_count,
    };
    if response_result.is_ok() {
        peer_exclusions.record_peer_request_success(&peer);
        if let Some(request_context) = request_context.as_ref() {
            let mut state_guard = driver_state.lock().await;
            state_guard.record_request_success(peer);
            state_guard.record_outbound_response(
                request_context,
                response_bytes,
                request_context.started_at.elapsed(),
                recorded_failure_count,
            );
        } else {
            driver_state.lock().await.record_request_success(peer);
        }
    }
    driver_state
        .lock()
        .await
        .remove_outbound_request(request_id);
    response_result?;

    Ok(())
}

async fn queue_classic_block_by_height_fallback(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    peer: PeerId,
    request_id: request_response::OutboundRequestId,
    item_id: u32,
    height: u64,
    reason: &'static str,
) {
    let classic_message = block_by_height_message(height);
    if let Err(send_err) = swarm_tx
        .send(SwarmAction::QueueKernelRequest {
            peer_id: peer,
            request_message: classic_message,
        })
        .await
    {
        warn!(
            peer = %peer,
            request_id = %request_id,
            item_id,
            height,
            reason,
            error = %send_err,
            "Failed to queue classic block-by-height fallback"
        );
    } else {
        trace!(
            peer = %peer,
            request_id = %request_id,
            item_id,
            height,
            reason,
            "Queued classic block-by-height fallback"
        );
    }
}

#[derive(Debug, Default)]
struct ResponseComposition {
    batch_result_items: usize,
    batch_result_success_items: usize,
    batch_result_ack_items: usize,
    batch_result_not_found_items: usize,
    batch_result_error_items: usize,
    bundle_items: usize,
    bundle_block_bytes: usize,
    bundle_payload_bytes: usize,
    bundle_envelope_bytes: usize,
    bundled_tx_count: usize,
    bundled_tx_bytes: usize,
    unincluded_tx_count: usize,
}

impl ResponseComposition {
    fn from_response(response: &NockchainResponse) -> Self {
        let mut composition = Self::default();
        let NockchainResponse::BatchResult { results } = response else {
            return composition;
        };

        for result in results {
            composition.record_batch_result_item(result);
        }

        composition
    }

    fn record_batch_result_item(&mut self, result: &BatchResultItem) {
        self.batch_result_items += 1;
        match result.status {
            BatchResultStatus::Result => {
                self.batch_result_success_items += 1;
                if let Some(envelope) = result.envelope.as_ref() {
                    self.record_envelope(envelope);
                }
            }
            BatchResultStatus::Ack => {
                self.batch_result_ack_items += 1;
            }
            BatchResultStatus::NotFound => {
                self.batch_result_not_found_items += 1;
            }
            BatchResultStatus::Error => {
                self.batch_result_error_items += 1;
            }
        }
    }

    fn record_envelope(&mut self, envelope: &ResponseEnvelope) {
        match envelope.kind {
            EnvelopeKind::HeardBlockWithTxs => {
                let tx_envelopes = envelope.tx_envelopes.as_deref().unwrap_or(&[]);
                let tx_bytes = tx_envelopes
                    .iter()
                    .map(|tx| tx.message.len())
                    .sum::<usize>();
                self.record_bundle_payload(
                    envelope.message.len(),
                    tx_envelopes.len(),
                    tx_bytes,
                    envelope.unincluded_tx_ids.as_deref().unwrap_or(&[]).len(),
                );
            }
            EnvelopeKind::HeardBlockRangeWithTxs => {
                for block in envelope.range_blocks.as_deref().unwrap_or(&[]) {
                    let tx_bytes = block
                        .tx_envelopes
                        .iter()
                        .map(|tx| tx.message.len())
                        .sum::<usize>();
                    self.record_bundle_payload(
                        block.block_message.len(),
                        block.tx_envelopes.len(),
                        tx_bytes,
                        block.unincluded_tx_ids.len(),
                    );
                }
            }
            EnvelopeKind::HeardBlock | EnvelopeKind::HeardTx | EnvelopeKind::HeardElders => return,
        }

        self.bundle_envelope_bytes = self
            .bundle_envelope_bytes
            .saturating_add(response_envelope_encoded_bytes(envelope));
    }

    fn record_bundle_payload(
        &mut self,
        block_bytes: usize,
        tx_count: usize,
        tx_bytes: usize,
        unincluded_tx_count: usize,
    ) {
        self.bundle_items = self.bundle_items.saturating_add(1);
        self.bundle_block_bytes = self.bundle_block_bytes.saturating_add(block_bytes);
        self.bundled_tx_count = self.bundled_tx_count.saturating_add(tx_count);
        self.bundled_tx_bytes = self.bundled_tx_bytes.saturating_add(tx_bytes);
        self.unincluded_tx_count = self.unincluded_tx_count.saturating_add(unincluded_tx_count);
        self.bundle_payload_bytes = self
            .bundle_payload_bytes
            .saturating_add(block_bytes)
            .saturating_add(tx_bytes);
    }
}

fn response_envelope_encoded_bytes(envelope: &ResponseEnvelope) -> usize {
    match req_res_message_encoded_bytes(envelope) {
        Ok(bytes) => bytes,
        Err(err) => {
            trace!(
                error = %err,
                "Failed to encode response envelope for bundle composition stats"
            );
            0
        }
    }
}

async fn record_batch_result_response_hint(
    driver_state: &Arc<Mutex<P2PState>>,
    request_context: Option<&OutboundRequestContext>,
    item_id: u32,
    envelope: &ResponseEnvelope,
) {
    let Some(mut data_request) = data_request_batch_item(request_context, item_id) else {
        return;
    };

    if let NockchainDataRequest::BlockRangeWithTxs { start_height, .. } = data_request {
        let actual_len = envelope.range_blocks.as_ref().map(Vec::len).unwrap_or(0);
        if actual_len == 0 {
            return;
        }
        let Ok(len) = u8::try_from(actual_len) else {
            warn!(
                item_id,
                actual_len, "Skipping response-size hint for overlong block range"
            );
            return;
        };
        data_request = NockchainDataRequest::BlockRangeWithTxs { start_height, len };
    }

    driver_state.lock().await.record_response_message_hint(
        &data_request,
        batch_result_item_response_bytes(item_id, envelope),
    );
}

fn batch_result_item_response_bytes(item_id: u32, envelope: &ResponseEnvelope) -> usize {
    match batch_result_encoded_bytes(&[BatchResultItem {
        item_id,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope.clone()),
    }]) {
        Ok(bytes) => bytes,
        Err(err) => {
            trace!(
                error = %err,
                "Failed to encode batch result item for response-size hint"
            );
            response_envelope_encoded_bytes(envelope)
        }
    }
}

fn data_request_single_request(
    request_context: Option<&OutboundRequestContext>,
) -> Option<NockchainDataRequest> {
    let NockchainRequest::Request { message, .. } = &request_context?.request else {
        return None;
    };
    decode_request_item_message(message).ok()
}

async fn record_response_validation_abuse(
    swarm_tx: &mpsc::Sender<SwarmAction>,
    driver_state: &Arc<Mutex<P2PState>>,
    metrics: &NockchainP2PMetrics,
    peer_exclusions: &PeerExclusions,
    peer: PeerId,
) -> Result<(), NockAppError> {
    let address = driver_state.lock().await.peer_first_address(&peer);
    record_local_peer_abuse(
        swarm_tx,
        driver_state,
        metrics,
        peer_exclusions,
        peer,
        None,
        address,
        LocalPeerAbuseKind::ResponseValidationMismatch,
        LocalPeerAbuseSeverity::Strong,
        false,
    )
    .await
}
