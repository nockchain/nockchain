use std::collections::BTreeSet;
use std::sync::Arc;

use libp2p::PeerId;
use nockapp::NockAppError;
use tokio::sync::{mpsc, Mutex};
use tracing::trace;

use crate::driver::gen2::*;
use crate::driver::{SwarmAction, SwarmActionDispatcher};
use crate::messages::{
    BundledBlockWithTxs, EnvelopeKind, NockchainDataRequest, NockchainFact, NockchainRequest,
    ResponseEnvelope,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{BlockSource, OutboundRequestContext, P2PState};
use crate::traffic_cop;

/// Decompose a bundle envelope (`kind == HeardBlockWithTxs`) into its
/// constituent block + tx facts, route each through `route_response_fact` in
/// page-declared order, and queue `RawTransactionById` requests for any tx-id
/// the responder listed as unincluded.
///
/// Kind/id consistency is checked against the decoded inner facts; a block-id
/// or tx-id that doesn't match the envelope metadata is surfaced as an error
/// so the caller can mark the item for retry rather than silently accepting
/// a crafted mismatch.
pub(crate) async fn route_bundle_envelope_with_dispatcher(
    peer: PeerId,
    envelope: &ResponseEnvelope,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<(), NockAppError> {
    route_bundle_envelope_with_source_with_dispatcher(
        peer,
        envelope,
        traffic,
        metrics,
        driver_state,
        swarm_actions,
        BlockSource::Gossip,
    )
    .await
}

pub(crate) async fn route_bundle_envelope_with_source_with_dispatcher(
    peer: PeerId,
    envelope: &ResponseEnvelope,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
    block_source: BlockSource,
) -> Result<(), NockAppError> {
    envelope.validate()?;
    if envelope.kind != EnvelopeKind::HeardBlockWithTxs {
        return Err(NockAppError::OtherError(String::from(
            "route_bundle_envelope called with non-bundle envelope",
        )));
    }

    // Decode the block fact up front for id/kind cross-checks, but *route the
    // txs first* below. If we poke the block first, the kernel processes it
    // before the bundled txs reach mempool (dumbnet/inner.hoon:706-717),
    // notices the referenced txs are missing, and emits
    // `%request %raw-tx %by-id` effects, which the driver turns into fresh
    // Gen2 tx requests that immediately duplicate every bundled tx on the
    // wire. Routing txs ahead of the block keeps the kernel's mempool
    // populated before it evaluates the block, so no redundant tx requests
    // get generated and the bundle's whole point (one round-trip per block)
    // holds. Verified via `missing_txs` recovery-event count dropping from
    // ~47 to ~2 per sync with the reorder.
    let block_fact = response_fact_from_result_message(&envelope.message)?;
    match &block_fact {
        NockchainFact::HeardBlock(decoded_id, _) => {
            if envelope.block_id.as_deref() != Some(decoded_id.as_str()) {
                return Err(NockAppError::OtherError(format!(
                    "bundle envelope block_id {:?} did not match decoded fact {}",
                    envelope.block_id, decoded_id
                )));
            }
        }
        _ => {
            return Err(NockAppError::OtherError(String::from(
                "bundle envelope message did not decode as heard-block",
            )));
        }
    }

    let tx_envelopes = envelope.tx_envelopes.as_ref().ok_or_else(|| {
        NockAppError::OtherError(String::from(
            "bundle envelope missing tx_envelopes during routing",
        ))
    })?;
    for bundled in tx_envelopes {
        let tx_fact = response_fact_from_result_message(&bundled.message)?;
        match &tx_fact {
            NockchainFact::HeardTx(decoded_id, _) => {
                if decoded_id != &bundled.tx_id {
                    return Err(NockAppError::OtherError(format!(
                        "bundled tx envelope tx-id {} did not match decoded heard-tx {}",
                        bundled.tx_id, decoded_id
                    )));
                }
            }
            _ => {
                return Err(NockAppError::OtherError(format!(
                    "bundled tx envelope {} did not decode as heard-tx",
                    bundled.tx_id
                )));
            }
        }
        route_response_fact_with_dispatcher(
            peer, tx_fact, traffic, metrics, driver_state, swarm_actions,
        )
        .await?;
    }

    route_response_fact_with_source_with_dispatcher(
        peer, block_fact, traffic, metrics, driver_state, swarm_actions, block_source,
    )
    .await?;

    if let Some(unincluded) = envelope.unincluded_tx_ids.as_ref() {
        if !unincluded.is_empty() {
            let tx_ids = {
                let mut state_guard = driver_state.lock().await;
                state_guard.track_tx_ids_and_peer(unincluded.iter().cloned(), peer);
                state_guard.claim_speculative_tx_prefetch_ids(
                    unincluded.iter().cloned(),
                    SPECULATIVE_TX_PREFETCH_TTL,
                    SPECULATIVE_TX_PREFETCH_MAX_IDS_PER_BLOCK,
                )
            };
            if tx_ids.is_empty() {
                return Ok(());
            }
            trace!(
                peer = %peer,
                unincluded_count = unincluded.len(),
                claimed_count = tx_ids.len(),
                "Queueing classic raw-tx requests for bundle remainder"
            );
            queue_speculative_raw_tx_prefetches_with_dispatcher(peer, tx_ids, swarm_actions)
                .await?;
        }
    }

    Ok(())
}
pub(crate) async fn route_bundle_envelope(
    peer: PeerId,
    envelope: &ResponseEnvelope,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
) -> Result<(), NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    route_bundle_envelope_with_dispatcher(
        peer, envelope, traffic, metrics, driver_state, &mut swarm_actions,
    )
    .await
}

/// Decompose a `HeardBlockRangeWithTxs` envelope into its constituent
/// per-block bundles and route each through `route_bundle_envelope`. Each
/// bundle becomes a synthetic `HeardBlockWithTxs` envelope so the existing
/// txs-before-block ordering and unincluded-tx queueing apply unchanged.
/// Phase 3 of catch-up prefetch.
pub(crate) async fn route_block_range_envelope_with_dispatcher(
    peer: PeerId,
    envelope: &ResponseEnvelope,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<(), NockAppError> {
    envelope.validate()?;
    if envelope.kind != EnvelopeKind::HeardBlockRangeWithTxs {
        return Err(NockAppError::OtherError(String::from(
            "route_block_range_envelope called with non-range envelope",
        )));
    }
    let blocks = envelope.range_blocks.as_ref().ok_or_else(|| {
        NockAppError::OtherError(String::from(
            "range envelope missing range_blocks during routing",
        ))
    })?;
    for block in blocks {
        let inner = ResponseEnvelope::heard_block_with_txs(
            block.block_id.clone(),
            block.block_message.as_ref(),
            block.tx_envelopes.clone(),
            block.unincluded_tx_ids.clone(),
        );
        route_bundle_envelope_with_source_with_dispatcher(
            peer,
            &inner,
            traffic,
            metrics,
            driver_state,
            swarm_actions,
            BlockSource::Prefetch,
        )
        .await?;
    }
    Ok(())
}

pub(crate) fn response_envelope_from_result_message(
    message: &[u8],
) -> Result<ResponseEnvelope, NockAppError> {
    let fact = response_fact_from_result_message(message)?;
    let envelope = match fact {
        NockchainFact::HeardBlock(block_id, _) => ResponseEnvelope::heard_block(block_id, message),
        NockchainFact::HeardTx(tx_id, _) => ResponseEnvelope::heard_tx(tx_id, message),
        NockchainFact::HeardElders(_, _, _) => ResponseEnvelope::heard_elders(message),
    };
    envelope.validate()?;
    Ok(envelope)
}
pub(crate) fn response_fact_from_result_message(
    message: &[u8],
) -> Result<NockchainFact, NockAppError> {
    NockchainFact::from_message_bytes(message)
}

pub(crate) fn response_envelope_matches_fact(
    envelope: &ResponseEnvelope,
    fact: &NockchainFact,
) -> bool {
    match fact {
        NockchainFact::HeardBlock(block_id, _) => {
            envelope.kind == EnvelopeKind::HeardBlock
                && envelope.block_id.as_deref() == Some(block_id.as_str())
                && envelope.tx_id.is_none()
        }
        NockchainFact::HeardTx(tx_id, _) => {
            envelope.kind == EnvelopeKind::HeardTx
                && envelope.tx_id.as_deref() == Some(tx_id.as_str())
                && envelope.block_id.is_none()
        }
        NockchainFact::HeardElders(_, _, _) => {
            envelope.kind == EnvelopeKind::HeardElders
                && envelope.block_id.is_none()
                && envelope.tx_id.is_none()
        }
    }
}
pub(crate) fn response_fact_from_envelope(
    envelope: &ResponseEnvelope,
) -> Result<NockchainFact, NockAppError> {
    envelope.validate()?;
    let fact = response_fact_from_result_message(&envelope.message)?;
    if !response_envelope_matches_fact(envelope, &fact) {
        return Err(NockAppError::OtherError(String::from(
            "response envelope metadata did not match payload",
        )));
    }
    Ok(fact)
}

pub(crate) fn validate_response_fact_for_request(
    request: &NockchainDataRequest,
    fact: &NockchainFact,
) -> Result<(), NockAppError> {
    match request {
        NockchainDataRequest::BlockByHeight(expected_height)
        | NockchainDataRequest::BlockWithTxsByHeight(expected_height) => {
            let NockchainFact::HeardBlock(_, slab) = fact else {
                return Err(response_kind_mismatch(request));
            };
            let actual_height = heard_block_height_from_fact_poke(slab)?;
            if actual_height != *expected_height {
                return Err(NockAppError::OtherError(format!(
                    "heard-block response height {actual_height} did not match requested height {expected_height}",
                )));
            }
            Ok(())
        }
        NockchainDataRequest::RawTransactionById(expected_tx_id, _) => {
            let NockchainFact::HeardTx(tx_id, _) = fact else {
                return Err(response_kind_mismatch(request));
            };
            if tx_id != expected_tx_id {
                return Err(NockAppError::OtherError(format!(
                    "heard-tx response id {tx_id} did not match requested tx {expected_tx_id}",
                )));
            }
            Ok(())
        }
        NockchainDataRequest::EldersById(_, _, _) => match fact {
            NockchainFact::HeardElders(_, _, _) => Ok(()),
            _ => Err(response_kind_mismatch(request)),
        },
        NockchainDataRequest::BlockRangeWithTxs { .. } => Err(response_kind_mismatch(request)),
    }
}

fn response_kind_mismatch(request: &NockchainDataRequest) -> NockAppError {
    NockAppError::OtherError(format!(
        "response fact kind did not match request kind {:?}",
        request
    ))
}

pub(crate) fn validate_response_envelope_for_request(
    request: &NockchainDataRequest,
    envelope: &ResponseEnvelope,
) -> Result<(), NockAppError> {
    envelope.validate()?;
    let expected_kind = expected_envelope_kind(request);
    if envelope.kind != expected_kind {
        return Err(NockAppError::OtherError(format!(
            "response envelope kind {:?} did not match requested kind {:?}",
            envelope.kind, expected_kind
        )));
    }

    match request {
        NockchainDataRequest::BlockByHeight(height) => {
            validate_block_envelope_height(envelope, *height)
        }
        NockchainDataRequest::BlockWithTxsByHeight(height) => {
            validate_block_envelope_height(envelope, *height)
        }
        NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            validate_range_envelope_for_request(envelope, *start_height, *len)
        }
        NockchainDataRequest::RawTransactionById(expected_tx_id, _) => {
            if envelope.tx_id.as_deref() != Some(expected_tx_id.as_str()) {
                return Err(NockAppError::OtherError(format!(
                    "heard-tx response id {:?} did not match requested tx {}",
                    envelope.tx_id, expected_tx_id
                )));
            }
            let fact = response_fact_from_envelope(envelope)?;
            validate_response_fact_for_request(request, &fact)
        }
        NockchainDataRequest::EldersById(_, _, _) => {
            let fact = response_fact_from_envelope(envelope)?;
            validate_response_fact_for_request(request, &fact)
        }
    }
}

fn expected_envelope_kind(request: &NockchainDataRequest) -> EnvelopeKind {
    match request {
        NockchainDataRequest::BlockByHeight(_) => EnvelopeKind::HeardBlock,
        NockchainDataRequest::RawTransactionById(_, _) => EnvelopeKind::HeardTx,
        NockchainDataRequest::EldersById(_, _, _) => EnvelopeKind::HeardElders,
        NockchainDataRequest::BlockWithTxsByHeight(_) => EnvelopeKind::HeardBlockWithTxs,
        NockchainDataRequest::BlockRangeWithTxs { .. } => EnvelopeKind::HeardBlockRangeWithTxs,
    }
}

fn validate_block_envelope_height(
    envelope: &ResponseEnvelope,
    expected_height: u64,
) -> Result<(), NockAppError> {
    let fact = response_fact_from_result_message(&envelope.message)?;
    let NockchainFact::HeardBlock(decoded_block_id, fact_poke) = &fact else {
        return Err(NockAppError::OtherError(String::from(
            "block response envelope did not decode as heard-block",
        )));
    };
    if envelope.block_id.as_deref() != Some(decoded_block_id.as_str()) {
        return Err(NockAppError::OtherError(format!(
            "block response id {:?} did not match decoded block id {}",
            envelope.block_id, decoded_block_id
        )));
    }
    let actual_height = heard_block_height_from_fact_poke(fact_poke)?;
    if actual_height != expected_height {
        return Err(NockAppError::OtherError(format!(
            "block response height {actual_height} did not match requested height {expected_height}",
        )));
    }
    Ok(())
}

fn validate_range_envelope_for_request(
    envelope: &ResponseEnvelope,
    start_height: u64,
    requested_len: u8,
) -> Result<(), NockAppError> {
    let blocks = envelope.range_blocks.as_ref().ok_or_else(|| {
        NockAppError::OtherError(String::from("range envelope missing range_blocks"))
    })?;
    if blocks.len() > usize::from(requested_len) {
        return Err(NockAppError::OtherError(format!(
            "range response length {} exceeded requested length {}",
            blocks.len(),
            requested_len
        )));
    }
    for (offset, block) in blocks.iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| {
            NockAppError::OtherError(String::from("range response offset exceeded u64"))
        })?;
        let expected_height = start_height.checked_add(offset).ok_or_else(|| {
            NockAppError::OtherError(String::from("range response height overflowed"))
        })?;
        validate_bundled_block_for_height(block, expected_height)?;
    }
    Ok(())
}

fn validate_bundled_block_for_height(
    block: &BundledBlockWithTxs,
    expected_height: u64,
) -> Result<(), NockAppError> {
    let fact = response_fact_from_result_message(&block.block_message)?;
    let NockchainFact::HeardBlock(decoded_block_id, fact_poke) = fact else {
        return Err(NockAppError::OtherError(format!(
            "range block {} did not decode as heard-block",
            block.block_id
        )));
    };
    if decoded_block_id != block.block_id {
        return Err(NockAppError::OtherError(format!(
            "range block id {} did not match decoded block id {}",
            block.block_id, decoded_block_id
        )));
    }
    let actual_height = heard_block_height_from_fact_poke(&fact_poke)?;
    if actual_height != expected_height {
        return Err(NockAppError::OtherError(format!(
            "range block height {actual_height} did not match expected height {expected_height}",
        )));
    }
    Ok(())
}

pub(crate) fn response_fact_trace_summary(response: &NockchainFact) -> String {
    match response {
        NockchainFact::HeardBlock(block_id, _) => {
            format!("heard-block block_id={block_id}")
        }
        NockchainFact::HeardTx(tx_id, _) => {
            format!("heard-tx tx_id={tx_id}")
        }
        NockchainFact::HeardElders(oldest_height, block_ids, _) => {
            let head_block_id = block_ids.first().map(String::as_str).unwrap_or("-");
            format!(
                "heard-elders oldest_height={oldest_height} ancestor_count={} head_block_id={head_block_id}",
                block_ids.len()
            )
        }
    }
}
pub(crate) fn batch_request_item_ids(
    request_context: Option<&OutboundRequestContext>,
) -> Option<BTreeSet<u32>> {
    match request_context.map(|context| &context.request) {
        Some(NockchainRequest::BatchRequest { items, .. }) => {
            Some(items.iter().map(|item| item.item_id).collect())
        }
        Some(
            NockchainRequest::Request { .. }
            | NockchainRequest::Gossip { .. }
            | NockchainRequest::AuthenticatedGossip { .. },
        )
        | None => None,
    }
}

pub(crate) fn missing_batch_result_item_ids(
    expected_item_ids: &BTreeSet<u32>,
    observed_item_ids: &BTreeSet<u32>,
) -> BTreeSet<u32> {
    expected_item_ids
        .difference(observed_item_ids)
        .copied()
        .collect()
}
