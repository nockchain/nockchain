use std::sync::Arc;

use either::{Left, Right};
use libp2p::PeerId;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::scry::ScryResult;
use nockapp::NockAppError;
use nockvm::noun::{Noun, NounAllocator, NounHandle, NounSpace, D, T};
use serde_bytes::ByteBuf;
use tokio::sync::Mutex;
use tracing::{debug, trace, warn};

use crate::driver::gen2::*;
use crate::driver::{
    create_response_result_from_payload, create_scry_response, record_crown_error_metric,
    request_to_scry_slab,
};
use crate::messages::{
    block_id_from_page, BatchResultItem, BatchResultStatus, BundledBlockWithTxs, BundledTxEnvelope,
    NockchainDataRequest, NockchainResponse, ResponseEnvelope,
};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::{CacheResponse, P2PState};
use crate::tip5_util::tip5_hash_to_base58;
use crate::traffic_cop;

struct HeavyTxsPeek<'a> {
    height: u64,
    block_id_base58: String,
    page: NounHandle<'a>,
    raw_txs: Vec<(String, NounHandle<'a>)>,
}

enum PreparedBundledTx {
    Candidate {
        tx_id: String,
        message: ByteBuf,
        cache_slab: NounSlab,
    },
    Unincluded {
        tx_id: String,
    },
}

struct PreparedBundleResponse {
    block_id_base58: String,
    block_message: ByteBuf,
    block_cache_slab: NounSlab,
    raw_txs: Vec<PreparedBundledTx>,
}

struct PreparedRangeEntry {
    height: u64,
    block_id_base58: String,
    block_message: ByteBuf,
    block_cache_slab: NounSlab,
    raw_txs: Vec<PreparedBundledTx>,
}

struct RangeBundleCandidate {
    height: u64,
    block_cache_slab: NounSlab,
    block: BundledBlockWithTxs,
    tx_cache_entries: Vec<(String, NounSlab)>,
    message_bytes_hint: usize,
}

pub(crate) async fn execute_request_item(
    peer: PeerId,
    data_request: NockchainDataRequest,
    limits: ReqResRuntimeLimits,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
) -> Result<RequestExecutionOutcome, NockAppError> {
    let data_request = clamp_block_range_request(data_request, limits.block_range_max_len);
    let cached = {
        let cache_result = {
            let mut state_guard = driver_state.lock().await;
            state_guard.check_cache(data_request.clone(), metrics).await
        };
        match cache_result {
            Ok(CacheResponse::Cached(slab)) => {
                trace!("Found cached response for request");
                Some(slab)
            }
            Ok(CacheResponse::NegativeCached) => {
                trace!("Negative-cached response for request");
                return Ok(RequestExecutionOutcome::NotFound);
            }
            Ok(CacheResponse::NotCached) => None,
            Err(err) => {
                warn!("Error checking block cache: {err:?}");
                None
            }
        }
    };

    let request_for_hint = data_request.clone();
    let (scry_res_slab, cache_hit) = if let Some(cache_result) = cached {
        trace!("found cached response for request");
        (cache_result, true)
    } else {
        let scry_slab = request_to_scry_slab(data_request.clone())?;
        let Some(scry_res_slab) = (match traffic.peek(Some(peer), scry_slab).await {
            Ok(Some(res_slab)) => {
                metrics.requests_peeked_some.increment();
                Some(res_slab)
            }
            Ok(None) => {
                metrics.requests_peeked_none.increment();
                trace!("No data found for incoming request from: {}", peer);
                None
            }
            Err(NockAppError::MPSCFullError(act)) => {
                metrics.requests_dropped.increment();
                trace!(
                    peer = %peer,
                    decision = queue_saturation_decision(QueueSaturationPath::RequestExecution),
                    saturation_path = queue_saturation_path(QueueSaturationPath::RequestExecution),
                    "handle_request_response: Request dropped due to backpressure"
                );
                Err(NockAppError::MPSCFullError(act))?
            }
            Err(err) => {
                if let NockAppError::CrownError(ref crown_err) = err {
                    record_crown_error_metric(crown_err, metrics.as_ref());
                }
                match data_request {
                    NockchainDataRequest::BlockByHeight(height) => {
                        debug!("Peek error getting block at height: {:?}", height);
                        metrics.requests_erred_block_by_height.increment();
                    }
                    NockchainDataRequest::BlockWithTxsByHeight(height) => {
                        debug!("Peek error getting block-with-txs at height: {:?}", height);
                        metrics.requests_erred_block_by_height.increment();
                    }
                    NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                        debug!(
                            "Peek error getting block range start={} len={}",
                            start_height, len
                        );
                        metrics.requests_erred_block_by_height.increment();
                    }
                    NockchainDataRequest::EldersById(ref id, _, _) => {
                        debug!("Peek error getting elders of id: {:?}", id);
                        metrics.requests_erred_elders_by_id.increment();
                    }
                    NockchainDataRequest::RawTransactionById(ref id, _) => {
                        debug!("Peek error getting raw tx with id: {:?}", &id);
                        metrics.requests_erred_raw_tx_by_id.increment();
                    }
                }
                trace!("handle_request_response: Error getting response");
                Err(err)?
            }
        }) else {
            if let NockchainDataRequest::BlockWithTxsByHeight(height) = data_request {
                let mut res_slab = NounSlab::new();
                let Some(prepared) = prepare_bundle_response_from_range_fallback(
                    peer, height, traffic, metrics, &mut res_slab,
                )
                .await?
                else {
                    return Ok(RequestExecutionOutcome::NotFound);
                };
                return complete_bundle_response(height, prepared, false, limits, driver_state)
                    .await;
            }
            return Ok(RequestExecutionOutcome::NotFound);
        };
        (scry_res_slab, false)
    };

    let mut res_slab = NounSlab::new();
    let response = match data_request {
        NockchainDataRequest::BlockByHeight(height) => {
            let response_result = {
                let space = scry_res_slab.noun_space();
                let scry_res = unsafe { scry_res_slab.root() };
                match create_scry_response(scry_res, &space, "heard-block", &mut res_slab) {
                    Left(()) => None,
                    Right(result) => Some(result),
                }
            };
            let Some(response_result) = response_result else {
                trace!("No data found for incoming block by-height request");
                return Ok(RequestExecutionOutcome::NotFound);
            };
            if !cache_hit {
                let mut state_guard = driver_state.lock().await;
                state_guard
                    .block_cache
                    .insert(height, scry_res_slab.clone());
            }
            response_result?
        }
        NockchainDataRequest::EldersById(id, _, _) => {
            let response_result = {
                let space = scry_res_slab.noun_space();
                let scry_res = unsafe { scry_res_slab.root() };
                match create_scry_response(scry_res, &space, "heard-elders", &mut res_slab) {
                    Left(()) => None,
                    Right(result) => Some(result),
                }
            };
            let Some(response_result) = response_result else {
                trace!("No data found for incoming elders request");
                let mut state_guard = driver_state.lock().await;
                state_guard.record_elders_negative_cache(id);
                return Ok(RequestExecutionOutcome::NotFound);
            };
            if !cache_hit {
                let mut state_guard = driver_state.lock().await;
                state_guard.elders_cache.insert(id, scry_res_slab.clone());
            }
            response_result?
        }
        NockchainDataRequest::RawTransactionById(ref id, _) => {
            let response_result = {
                let space = scry_res_slab.noun_space();
                let scry_res = unsafe { scry_res_slab.root() };
                match create_scry_response(scry_res, &space, "heard-tx", &mut res_slab) {
                    Left(()) => None,
                    Right(result) => Some(result),
                }
            };
            let Some(response_result) = response_result else {
                trace!("No data found for incoming raw-tx request");
                return Ok(RequestExecutionOutcome::NotFound);
            };
            if !cache_hit {
                let mut state_guard = driver_state.lock().await;
                trace!("cacheing tx request by id={:?}", id);
                state_guard
                    .tx_cache
                    .insert(id.clone(), scry_res_slab.clone());
            }
            response_result?
        }
        NockchainDataRequest::BlockWithTxsByHeight(height) => {
            let prepared = {
                let prepared_from_heavy_txs = {
                    let heavy_txs_space = scry_res_slab.noun_space();
                    let scry_res = unsafe { scry_res_slab.root() };
                    match parse_heavy_txs_scry_result(scry_res, &heavy_txs_space)? {
                        Some(peek) => Some(prepare_bundle_response_from_heavy_txs(
                            height, peek, &mut res_slab,
                        )?),
                        None => {
                            trace!(
                                height,
                                "No data found for incoming block-with-txs request; trying range fallback"
                            );
                            None
                        }
                    }
                };

                if let Some(prepared) = prepared_from_heavy_txs {
                    prepared
                } else {
                    let Some(prepared) = prepare_bundle_response_from_range_fallback(
                        peer, height, traffic, metrics, &mut res_slab,
                    )
                    .await?
                    else {
                        return Ok(RequestExecutionOutcome::NotFound);
                    };
                    prepared
                }
            };
            return complete_bundle_response(height, prepared, cache_hit, limits, driver_state)
                .await;
        }
        NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
            let prepared_entries = {
                let range_space = scry_res_slab.noun_space();
                let scry_res = unsafe { scry_res_slab.root() };
                let entries =
                    match parse_heaviest_chain_blocks_range_scry_result(scry_res, &range_space)? {
                        Some(entries) => entries,
                        None => {
                            trace!(
                            start_height, len,
                            "No data found for incoming block-range request; returning NotFound"
                        );
                            return Ok(RequestExecutionOutcome::NotFound);
                        }
                    };

                if entries.is_empty() {
                    trace!(
                        start_height, len,
                        "Block-range peek returned empty list; returning NotFound"
                    );
                    return Ok(RequestExecutionOutcome::NotFound);
                }

                // Enforce contiguous heights starting at `start_height`. The peek
                // arm at `inner.hoon:389` skips heights that are not on the
                // heaviest chain, so a non-contiguous response means the chain
                // ran out before `len` heights or there is a gap. Truncate at
                // the first hole; the requester reissues for the missing tail.
                let mut contiguous: Vec<HeaviestChainBlocksRangeEntry<'_>> = Vec::new();
                let mut expected = start_height;
                for entry in entries {
                    if entry.height != expected {
                        break;
                    }
                    expected = expected.saturating_add(1);
                    contiguous.push(entry);
                    if contiguous.len() == usize::from(len) {
                        break;
                    }
                }
                if contiguous.is_empty() {
                    trace!(
                        start_height, len,
                        "Block-range peek had no entry at start_height; returning NotFound"
                    );
                    return Ok(RequestExecutionOutcome::NotFound);
                }

                let mut prepared_entries = Vec::with_capacity(contiguous.len());
                for entry in contiguous {
                    let mut entry_block_slab = NounSlab::new();
                    let block_response = create_response_result_from_payload(
                        entry.page, "heard-block", &mut entry_block_slab,
                    )?;
                    let block_message = response_result_message(
                        block_response, "block-range entry produced unexpected response shape",
                    )?;
                    let block_cache_slab = scry_some_slab(entry.page);
                    let raw_txs = entry
                        .raw_txs
                        .into_iter()
                        .map(|(tx_id, raw_tx)| prepare_bundled_tx(tx_id, raw_tx, "block-range"))
                        .collect();
                    prepared_entries.push(PreparedRangeEntry {
                        height: entry.height,
                        block_id_base58: entry.block_id_base58,
                        block_message,
                        block_cache_slab,
                        raw_txs,
                    });
                }
                prepared_entries
            };

            // Per-block bundle assembly. Mirrors the single-bundle path,
            // packing each block under the per-item byte cap and packing
            // the range itself under the block-batch response byte budget.
            let item_cap = limits.gen2_item_max_bytes;
            let mut bundles: Vec<BundledBlockWithTxs> = Vec::with_capacity(prepared_entries.len());
            let mut block_cache_entries: Vec<(u64, NounSlab)> = Vec::new();
            let mut tx_cache_entries: Vec<(String, NounSlab)> = Vec::new();
            let mut total_response_bytes: usize = 0;

            for entry in prepared_entries {
                let candidate = prepare_range_bundle_candidate(entry, item_cap);
                let projected_bytes = projected_range_batch_result_bytes(&bundles, &candidate)?;
                let budget_bytes = if bundles.is_empty() {
                    limits.gen2_batch_max_bytes
                } else {
                    block_batch_response_budget_bytes(limits)
                };
                if projected_bytes > budget_bytes && !bundles.is_empty() {
                    trace!(
                        accepted_blocks = bundles.len(),
                        candidate_height = candidate.height,
                        projected_bytes,
                        budget_bytes,
                        "Block-range response budget filled; returning contiguous prefix"
                    );
                    break;
                }
                if projected_bytes > budget_bytes {
                    warn!(
                        candidate_height = candidate.height,
                        projected_bytes,
                        budget_bytes,
                        "First block-range entry exceeds response budget; returning it alone"
                    );
                }

                if !cache_hit {
                    block_cache_entries.push((candidate.height, candidate.block_cache_slab));
                    tx_cache_entries.extend(candidate.tx_cache_entries);
                }
                total_response_bytes =
                    total_response_bytes.saturating_add(candidate.message_bytes_hint);
                bundles.push(candidate.block);
            }

            let envelope = ResponseEnvelope::heard_block_range_with_txs(bundles);
            envelope.validate()?;
            let accepted_len = envelope
                .range_blocks
                .as_ref()
                .map(Vec::len)
                .unwrap_or_default();
            let accepted_len = u8::try_from(accepted_len).map_err(|_| {
                NockAppError::OtherError(String::from(
                    "accepted block-range response length did not fit in u8",
                ))
            })?;

            if !cache_hit {
                let mut state_guard = driver_state.lock().await;
                for (height, block_cache_slab) in block_cache_entries {
                    state_guard.block_cache.insert(height, block_cache_slab);
                }
                for (tx_id, tx_cache_slab) in tx_cache_entries {
                    state_guard.tx_cache.insert(tx_id, tx_cache_slab);
                }
            }

            driver_state.lock().await.record_response_message_hint(
                &NockchainDataRequest::BlockRangeWithTxs {
                    start_height,
                    len: accepted_len,
                },
                total_response_bytes,
            );

            return Ok(RequestExecutionOutcome::Result {
                response: NockchainResponse::Result {
                    message: ByteBuf::new(),
                },
                envelope,
            });
        }
    };

    let envelope = match &response {
        NockchainResponse::Result { message } => response_envelope_from_result_message(message)?,
        NockchainResponse::Ack { .. } | NockchainResponse::BatchResult { .. } => {
            return Err(NockAppError::OtherError(String::from(
                "request item execution produced unexpected response shape",
            )))
        }
    };
    driver_state
        .lock()
        .await
        .record_response_message_hint(&request_for_hint, envelope.message.len());

    Ok(RequestExecutionOutcome::Result { response, envelope })
}

fn response_result_message(
    response: NockchainResponse,
    error_message: &str,
) -> Result<ByteBuf, NockAppError> {
    match response {
        NockchainResponse::Result { message } => Ok(message),
        NockchainResponse::Ack { .. } | NockchainResponse::BatchResult { .. } => {
            Err(NockAppError::OtherError(String::from(error_message)))
        }
    }
}

fn prepare_range_bundle_candidate(
    entry: PreparedRangeEntry,
    item_cap: usize,
) -> RangeBundleCandidate {
    const BUNDLED_TX_ENVELOPE_OVERHEAD: usize = 64;

    let mut entry_total = entry.block_message.len();
    let mut tx_envelopes: Vec<BundledTxEnvelope> = Vec::new();
    let mut unincluded_tx_ids: Vec<String> = Vec::new();
    let mut tx_cache_entries: Vec<(String, NounSlab)> = Vec::new();

    for raw_tx in entry.raw_txs {
        let (tx_id, tx_message, cache_slab) = match raw_tx {
            PreparedBundledTx::Candidate {
                tx_id,
                message,
                cache_slab,
            } => (tx_id, message, cache_slab),
            PreparedBundledTx::Unincluded { tx_id } => {
                unincluded_tx_ids.push(tx_id);
                continue;
            }
        };
        let projected = entry_total
            .saturating_add(tx_message.len())
            .saturating_add(tx_id.len())
            .saturating_add(BUNDLED_TX_ENVELOPE_OVERHEAD);
        if projected > item_cap {
            trace!(
                tx_id = %tx_id,
                projected,
                item_cap,
                "block-range entry cap would be exceeded; deferring tx to unincluded list"
            );
            unincluded_tx_ids.push(tx_id);
            continue;
        }

        entry_total = projected;
        tx_cache_entries.push((tx_id.clone(), cache_slab));
        tx_envelopes.push(BundledTxEnvelope {
            tx_id,
            message: tx_message,
        });
    }

    RangeBundleCandidate {
        height: entry.height,
        block_cache_slab: entry.block_cache_slab,
        block: BundledBlockWithTxs {
            block_id: entry.block_id_base58,
            block_message: entry.block_message,
            tx_envelopes,
            unincluded_tx_ids,
        },
        tx_cache_entries,
        message_bytes_hint: entry_total,
    }
}

fn block_range_batch_result_encoded_bytes(
    blocks: Vec<BundledBlockWithTxs>,
) -> Result<usize, NockAppError> {
    let envelope = ResponseEnvelope::heard_block_range_with_txs(blocks);
    batch_result_encoded_bytes(&[BatchResultItem {
        item_id: 0,
        status: BatchResultStatus::Result,
        error: None,
        envelope: Some(envelope),
    }])
}

fn projected_range_batch_result_bytes(
    accepted: &[BundledBlockWithTxs],
    candidate: &RangeBundleCandidate,
) -> Result<usize, NockAppError> {
    let mut projected = accepted.to_vec();
    projected.push(candidate.block.clone());
    block_range_batch_result_encoded_bytes(projected)
}

fn prepare_bundle_response_from_heavy_txs(
    height: u64,
    heavy_txs: HeavyTxsPeek<'_>,
    res_slab: &mut NounSlab,
) -> Result<PreparedBundleResponse, NockAppError> {
    if heavy_txs.height != height {
        return Err(NockAppError::OtherError(format!(
            "heavy-txs peek returned height {} for requested height {}",
            heavy_txs.height, height
        )));
    }

    let block_response =
        create_response_result_from_payload(heavy_txs.page, "heard-block", res_slab)?;
    let block_message = response_result_message(
        block_response, "bundle block scry produced unexpected response shape",
    )?;
    let block_cache_slab = scry_some_slab(heavy_txs.page);
    let raw_txs = heavy_txs
        .raw_txs
        .into_iter()
        .map(|(tx_id, raw_tx)| prepare_bundled_tx(tx_id, raw_tx, "single-bundle"))
        .collect();

    Ok(PreparedBundleResponse {
        block_id_base58: heavy_txs.block_id_base58,
        block_message,
        block_cache_slab,
        raw_txs,
    })
}

fn prepare_bundle_response_from_range_entry(
    height: u64,
    entry: HeaviestChainBlocksRangeEntry<'_>,
    res_slab: &mut NounSlab,
) -> Result<PreparedBundleResponse, NockAppError> {
    if entry.height != height {
        return Err(NockAppError::OtherError(format!(
            "range fallback returned height {} for requested height {}",
            entry.height, height
        )));
    }

    let block_response = create_response_result_from_payload(entry.page, "heard-block", res_slab)?;
    let block_message = response_result_message(
        block_response, "range fallback bundle block produced unexpected response shape",
    )?;
    let block_cache_slab = scry_some_slab(entry.page);
    let raw_txs = entry
        .raw_txs
        .into_iter()
        .map(|(tx_id, raw_tx)| prepare_bundled_tx(tx_id, raw_tx, "single-bundle-range-fallback"))
        .collect();

    Ok(PreparedBundleResponse {
        block_id_base58: entry.block_id_base58,
        block_message,
        block_cache_slab,
        raw_txs,
    })
}

async fn prepare_bundle_response_from_range_fallback(
    peer: PeerId,
    height: u64,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    res_slab: &mut NounSlab,
) -> Result<Option<PreparedBundleResponse>, NockAppError> {
    let Some(range_res_slab) =
        peek_single_block_range_fallback(peer, height, traffic, metrics).await?
    else {
        trace!(
            height, "No data found for incoming block-with-txs range fallback; returning NotFound"
        );
        return Ok(None);
    };
    let range_space = range_res_slab.noun_space();
    let range_res = unsafe { range_res_slab.root() };
    let entries = match parse_heaviest_chain_blocks_range_scry_result(range_res, &range_space)? {
        Some(entries) => entries,
        None => {
            trace!(
                height, "Range fallback for block-with-txs returned no data; returning NotFound"
            );
            return Ok(None);
        }
    };
    let Some(entry) = entries.into_iter().find(|entry| entry.height == height) else {
        trace!(
            height,
            "Range fallback for block-with-txs did not include requested height; returning NotFound"
        );
        return Ok(None);
    };
    prepare_bundle_response_from_range_entry(height, entry, res_slab).map(Some)
}

async fn complete_bundle_response(
    height: u64,
    prepared: PreparedBundleResponse,
    cache_hit: bool,
    limits: ReqResRuntimeLimits,
    driver_state: &Arc<Mutex<P2PState>>,
) -> Result<RequestExecutionOutcome, NockAppError> {
    let PreparedBundleResponse {
        block_id_base58,
        block_message,
        block_cache_slab,
        raw_txs,
    } = prepared;

    if !cache_hit {
        let mut state_guard = driver_state.lock().await;
        state_guard.block_cache.insert(height, block_cache_slab);
    }

    let bundle_cap = limits.gen2_item_max_bytes;
    const BUNDLED_TX_ENVELOPE_OVERHEAD: usize = 64;
    let mut total_bytes = block_message.len();
    let mut bundled: Vec<BundledTxEnvelope> = Vec::new();
    let mut unincluded = Vec::new();

    for raw_tx in raw_txs {
        let (tx_id, tx_message, cache_slab) = match raw_tx {
            PreparedBundledTx::Candidate {
                tx_id,
                message,
                cache_slab,
            } => (tx_id, message, cache_slab),
            PreparedBundledTx::Unincluded { tx_id } => {
                unincluded.push(tx_id);
                continue;
            }
        };

        let projected = total_bytes
            .saturating_add(tx_message.len())
            .saturating_add(tx_id.len())
            .saturating_add(BUNDLED_TX_ENVELOPE_OVERHEAD);
        if projected > bundle_cap {
            trace!(
                tx_id = %tx_id,
                projected,
                bundle_cap,
                "bundle cap would be exceeded; deferring tx to unincluded list"
            );
            unincluded.push(tx_id);
            continue;
        }

        let mut state_guard = driver_state.lock().await;
        state_guard.tx_cache.insert(tx_id.clone(), cache_slab);
        drop(state_guard);

        total_bytes = total_bytes
            .saturating_add(tx_message.len())
            .saturating_add(tx_id.len())
            .saturating_add(BUNDLED_TX_ENVELOPE_OVERHEAD);
        bundled.push(BundledTxEnvelope {
            tx_id,
            message: tx_message,
        });
    }

    let envelope = ResponseEnvelope::heard_block_with_txs(
        block_id_base58, &block_message, bundled, unincluded,
    );
    envelope.validate()?;

    let mut state_guard = driver_state.lock().await;
    let request_for_hint = NockchainDataRequest::BlockWithTxsByHeight(height);
    state_guard.record_response_message_hint(&request_for_hint, total_bytes);

    Ok(RequestExecutionOutcome::Result {
        response: NockchainResponse::Result {
            message: block_message,
        },
        envelope,
    })
}

async fn peek_single_block_range_fallback(
    peer: PeerId,
    height: u64,
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
) -> Result<Option<NounSlab>, NockAppError> {
    let scry_slab = request_to_scry_slab(NockchainDataRequest::BlockRangeWithTxs {
        start_height: height,
        len: 1,
    })?;
    match traffic.peek(Some(peer), scry_slab).await {
        Ok(Some(res_slab)) => {
            metrics.requests_peeked_some.increment();
            Ok(Some(res_slab))
        }
        Ok(None) => {
            metrics.requests_peeked_none.increment();
            Ok(None)
        }
        Err(NockAppError::MPSCFullError(act)) => {
            metrics.requests_dropped.increment();
            trace!(
                peer = %peer,
                decision = queue_saturation_decision(QueueSaturationPath::RequestExecution),
                saturation_path = queue_saturation_path(QueueSaturationPath::RequestExecution),
                "block-with-txs range fallback dropped due to backpressure"
            );
            Err(NockAppError::MPSCFullError(act))
        }
        Err(err) => {
            if let NockAppError::CrownError(ref crown_err) = err {
                record_crown_error_metric(crown_err, metrics.as_ref());
            }
            metrics.requests_erred_block_by_height.increment();
            Err(err)
        }
    }
}

fn prepare_bundled_tx(
    tx_id: String,
    raw_tx: NounHandle<'_>,
    bundle_context: &'static str,
) -> PreparedBundledTx {
    let mut tx_res_slab = NounSlab::new();
    let tx_response =
        match create_response_result_from_payload(raw_tx, "heard-tx", &mut tx_res_slab) {
            Ok(response) => response,
            Err(err) => {
                warn!(
                    tx_id = %tx_id,
                    error = %err,
                    bundle_context,
                    "bundled raw-tx response build failed"
                );
                return PreparedBundledTx::Unincluded { tx_id };
            }
        };

    match tx_response {
        NockchainResponse::Result { message } => PreparedBundledTx::Candidate {
            tx_id,
            message,
            cache_slab: scry_some_slab(raw_tx),
        },
        NockchainResponse::Ack { .. } | NockchainResponse::BatchResult { .. } => {
            PreparedBundledTx::Unincluded { tx_id }
        }
    }
}

fn parse_heavy_txs_scry_result<'a>(
    scry_res: &Noun,
    space: &'a NounSpace,
) -> Result<Option<HeavyTxsPeek<'a>>, NockAppError> {
    match ScryResult::from_noun(scry_res, space) {
        ScryResult::BadPath | ScryResult::Nothing => Ok(None),
        ScryResult::Some(payload) => parse_heavy_txs_payload(payload).map(Some),
        ScryResult::Invalid => Err(NockAppError::OtherError(String::from(
            "Invalid heavy-txs scry result",
        ))),
    }
}

fn parse_heavy_txs_payload<'a>(payload: NounHandle<'a>) -> Result<HeavyTxsPeek<'a>, NockAppError> {
    let payload_cell = payload.as_cell()?;
    let height = payload_cell.head().as_atom()?.as_u64()?;
    let tail = payload_cell.tail().as_cell()?;
    let block_id = tail.head();
    let tail = tail.tail().as_cell()?;
    let page = tail.head();
    let raw_txs_noun = tail.tail();

    let block_id_base58 = tip5_hash_to_base58(block_id.noun(), block_id.space())?;
    let page_block_id = block_id_from_page(page)?;
    let page_block_id_base58 = tip5_hash_to_base58(page_block_id.noun(), page_block_id.space())?;
    if block_id_base58 != page_block_id_base58 {
        return Err(NockAppError::OtherError(format!(
            "heavy-txs block id {} did not match page block id {}",
            block_id_base58, page_block_id_base58
        )));
    }

    Ok(HeavyTxsPeek {
        height,
        block_id_base58,
        page,
        raw_txs: parse_heavy_txs_raw_txs(raw_txs_noun)?,
    })
}

fn parse_heavy_txs_raw_txs<'a>(
    raw_txs: NounHandle<'a>,
) -> Result<Vec<(String, NounHandle<'a>)>, NockAppError> {
    let mut parsed = Vec::new();
    for_list(raw_txs, |entry| {
        let entry_cell = entry.as_cell()?;
        let tx_id = tip5_hash_to_base58(entry_cell.head().noun(), entry_cell.head().space())?;
        let raw_tx = entry_cell.tail();
        let raw_tx_id_noun = tx_id_from_raw_tx(raw_tx)?;
        let raw_tx_id = tip5_hash_to_base58(raw_tx_id_noun.noun(), raw_tx_id_noun.space())?;
        if tx_id != raw_tx_id {
            return Err(NockAppError::OtherError(format!(
                "heavy-txs entry id {} did not match raw tx id {}",
                tx_id, raw_tx_id
            )));
        }
        parsed.push((tx_id, raw_tx));
        Ok(())
    })?;
    Ok(parsed)
}

struct HeaviestChainBlocksRangeEntry<'a> {
    height: u64,
    block_id_base58: String,
    page: NounHandle<'a>,
    raw_txs: Vec<(String, NounHandle<'a>)>,
}

fn parse_heaviest_chain_blocks_range_scry_result<'a>(
    scry_res: &Noun,
    space: &'a NounSpace,
) -> Result<Option<Vec<HeaviestChainBlocksRangeEntry<'a>>>, NockAppError> {
    match ScryResult::from_noun(scry_res, space) {
        ScryResult::BadPath | ScryResult::Nothing => Ok(None),
        ScryResult::Some(payload) => parse_heaviest_chain_blocks_range_payload(payload).map(Some),
        ScryResult::Invalid => Err(NockAppError::OtherError(String::from(
            "Invalid heaviest-chain-blocks-range scry result",
        ))),
    }
}

fn parse_heaviest_chain_blocks_range_payload<'a>(
    payload: NounHandle<'a>,
) -> Result<Vec<HeaviestChainBlocksRangeEntry<'a>>, NockAppError> {
    // payload :: (list [page-number block-id page (z-map tx-id raw-tx-or-tx)])
    let mut entries = Vec::new();
    for_list(payload, |entry| {
        let entry_cell = entry.as_cell()?;
        let height = entry_cell.head().as_atom()?.as_u64()?;
        let tail = entry_cell.tail().as_cell()?;
        let block_id_noun = tail.head();
        let tail = tail.tail().as_cell()?;
        let page = tail.head();
        let txs_map = tail.tail();

        let block_id_base58 = tip5_hash_to_base58(block_id_noun.noun(), block_id_noun.space())?;
        let page_block_id = block_id_from_page(page)?;
        let page_block_id_base58 =
            tip5_hash_to_base58(page_block_id.noun(), page_block_id.space())?;
        if block_id_base58 != page_block_id_base58 {
            return Err(NockAppError::OtherError(format!(
                "heaviest-chain-blocks-range entry block id {} did not match page block id {}",
                block_id_base58, page_block_id_base58
            )));
        }

        let raw_txs = parse_z_map_tx_entries(txs_map)?;
        entries.push(HeaviestChainBlocksRangeEntry {
            height,
            block_id_base58,
            page,
            raw_txs,
        });
        Ok(())
    })?;
    Ok(entries)
}

/// Walk a Hoon `(z-map tx-id raw-tx-like)` and yield each `[tx-id raw-tx]`
/// pair. The map is a treap of `[node=[k v] left=tree right=tree]` where empty
/// subtrees are atom `0`. Checkpointed range peeks have existed with direct
/// raw tx values, validated `tx:t` values, and `[raw-tx heard-at]` raw index
/// values, so normalize all accepted shapes before bundling.
fn parse_z_map_tx_entries<'a>(
    map: NounHandle<'a>,
) -> Result<Vec<(String, NounHandle<'a>)>, NockAppError> {
    let mut out = Vec::new();
    let mut stack: Vec<NounHandle<'a>> = Vec::new();
    if map.is_cell() {
        stack.push(map);
    }
    while let Some(node) = stack.pop() {
        let cell = match node.as_cell() {
            Ok(c) => c,
            Err(_) => continue,
        };
        let kv = cell.head().as_cell()?;
        let tx_id = tip5_hash_to_base58(kv.head().noun(), kv.head().space())?;
        let raw_tx = tx_map_value_to_raw_tx(&tx_id, kv.tail())?;
        out.push((tx_id, raw_tx));

        let subtrees = cell.tail().as_cell()?;
        let left = subtrees.head();
        let right = subtrees.tail();
        if left.is_cell() {
            stack.push(left);
        }
        if right.is_cell() {
            stack.push(right);
        }
    }
    Ok(out)
}

fn tx_map_value_to_raw_tx<'a>(
    expected_tx_id: &str,
    tx_value: NounHandle<'a>,
) -> Result<NounHandle<'a>, NockAppError> {
    let mut errors = Vec::new();

    match raw_tx_with_expected_id(expected_tx_id, tx_value, "raw-tx") {
        Ok(raw_tx) => return Ok(raw_tx),
        Err(err) => errors.push(err.to_string()),
    }

    match raw_tx_from_heard_at_pair(tx_value)
        .and_then(|raw_tx| raw_tx_with_expected_id(expected_tx_id, raw_tx, "raw-tx/heard-at"))
    {
        Ok(raw_tx) => return Ok(raw_tx),
        Err(err) => errors.push(err.to_string()),
    }

    match raw_tx_from_validated_tx(tx_value)
        .and_then(|raw_tx| raw_tx_with_expected_id(expected_tx_id, raw_tx, "tx.raw-tx"))
    {
        Ok(raw_tx) => return Ok(raw_tx),
        Err(err) => errors.push(err.to_string()),
    }

    Err(NockAppError::OtherError(format!(
        "block-range z-map value did not contain raw-tx for id {}: {}",
        expected_tx_id,
        errors.join("; ")
    )))
}

fn raw_tx_with_expected_id<'a>(
    expected_tx_id: &str,
    raw_tx: NounHandle<'a>,
    source: &str,
) -> Result<NounHandle<'a>, NockAppError> {
    let raw_tx_id_noun = tx_id_from_raw_tx(raw_tx)?;
    let raw_tx_id = tip5_hash_to_base58(raw_tx_id_noun.noun(), raw_tx_id_noun.space())?;
    if raw_tx_id != expected_tx_id {
        return Err(NockAppError::OtherError(format!(
            "block-range z-map entry id {} did not match {} id {}",
            expected_tx_id, source, raw_tx_id
        )));
    }
    Ok(raw_tx)
}

fn raw_tx_from_heard_at_pair<'a>(value: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let pair = value.as_cell()?;
    pair.tail().as_atom()?;
    Ok(pair.head())
}

fn raw_tx_from_validated_tx<'a>(tx: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let tx_cell = tx.as_cell()?;
    let tag = tx_cell.head();
    if !(tag_is_tx_variant(tag) || tag.eq_bytes(b"0") || tag.eq_bytes(b"1")) {
        return Err(NockAppError::OtherError(String::from(
            "validated tx tag was not %0 or %1",
        )));
    }
    Ok(tx_cell.tail().as_cell()?.head())
}

fn tag_is_tx_variant(tag: NounHandle<'_>) -> bool {
    matches!(tag.as_atom().and_then(|atom| atom.as_u64()), Ok(0 | 1))
}

fn clamp_block_range_request(request: NockchainDataRequest, max_len: u8) -> NockchainDataRequest {
    let max_len = max_len.max(1);
    match request {
        NockchainDataRequest::BlockRangeWithTxs { start_height, len } if len > max_len => {
            NockchainDataRequest::BlockRangeWithTxs {
                start_height,
                len: max_len,
            }
        }
        request => request,
    }
}

fn for_list<'a>(
    mut list: NounHandle<'a>,
    mut visit: impl FnMut(NounHandle<'a>) -> Result<(), NockAppError>,
) -> Result<(), NockAppError> {
    while !unsafe { list.noun().raw_equals(&D(0)) } {
        let cell = list.as_cell()?;
        visit(cell.head())?;
        list = cell.tail();
    }
    Ok(())
}

fn tx_id_from_raw_tx<'a>(raw_tx: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let raw_tx_cell = raw_tx.as_cell()?;
    match raw_tx_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(raw_tx_cell.tail().as_cell()?.head())
            } else {
                Err(NockAppError::OtherError(format!(
                    "Unsupported raw-tx version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(raw_tx_cell.head()),
    }
}

fn scry_some_slab(payload: NounHandle<'_>) -> NounSlab {
    let mut slab = NounSlab::new();
    let copied = slab.copy_into(payload.noun(), payload.space());
    let scry_some = T(&mut slab, &[D(0), D(0), copied]);
    slab.set_root(scry_some);
    slab
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use serde_bytes::ByteBuf;

    use crate::driver::gen2::request_exec::{
        clamp_block_range_request, projected_range_batch_result_bytes, RangeBundleCandidate,
    };
    use crate::messages::{BundledBlockWithTxs, NockchainDataRequest};

    fn range_candidate(height: u64, block_bytes: usize) -> RangeBundleCandidate {
        RangeBundleCandidate {
            height,
            block_cache_slab: NounSlab::new(),
            block: BundledBlockWithTxs {
                block_id: format!("block-{height}"),
                block_message: ByteBuf::from(vec![0xAB; block_bytes]),
                tx_envelopes: Vec::new(),
                unincluded_tx_ids: Vec::new(),
            },
            tx_cache_entries: Vec::new(),
            message_bytes_hint: block_bytes,
        }
    }

    #[test]
    fn range_projection_tracks_encoded_prefix_growth() {
        let first = range_candidate(10, 512);
        let second = range_candidate(11, 512);
        let first_bytes =
            projected_range_batch_result_bytes(&[], &first).expect("first range should encode");
        let accepted = vec![first.block.clone()];
        let second_bytes = projected_range_batch_result_bytes(&accepted, &second)
            .expect("second range should encode");

        assert!(second_bytes > first_bytes);
        let midpoint_budget = first_bytes + ((second_bytes - first_bytes) / 2);
        assert!(first_bytes <= midpoint_budget);
        assert!(second_bytes > midpoint_budget);
    }

    #[test]
    fn clamp_block_range_request_caps_len_to_local_policy() {
        let request = NockchainDataRequest::BlockRangeWithTxs {
            start_height: 500,
            len: 32,
        };

        match clamp_block_range_request(request, 8) {
            NockchainDataRequest::BlockRangeWithTxs { start_height, len } => {
                assert_eq!(start_height, 500);
                assert_eq!(len, 8);
            }
            other => panic!("unexpected request after clamp: {other:?}"),
        }
    }
}
