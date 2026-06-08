use std::sync::Arc;

use libp2p::PeerId;
use nockapp::noun::slab::NounSlab;
use nockapp::NockAppError;
use nockvm::noun::{NounAllocator, NounHandle};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::info;

use crate::driver::gen2::*;
use crate::driver::{SwarmAction, SwarmActionDispatcher};
use crate::messages::{raw_tx_request_message, NockchainFact};
use crate::metrics::NockchainP2PMetrics;
use crate::p2p_state::P2PState;
use crate::tip5_util::tip5_hash_to_base58;
use crate::traffic_cop;

const TIP5_ZSET_MAX_ITEMS: usize = 65_536;
const TIP5_ZSET_MAX_STACK: usize = 65_536;

pub(crate) async fn queue_speculative_raw_tx_prefetches_with_dispatcher(
    peer: PeerId,
    tx_ids: Vec<String>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<(), NockAppError> {
    for tx_id in tx_ids {
        let request_message = raw_tx_request_message(&tx_id)?;
        swarm_actions
            .dispatch(SwarmAction::QueueKernelRequest {
                peer_id: peer,
                request_message,
            })
            .await
            .map_err(|_| {
                NockAppError::OtherError(String::from(
                    "Failed to queue speculative raw-tx prefetch",
                ))
            })?;
    }
    Ok(())
}

pub(crate) async fn track_future_heard_block_tx_hints_and_prefetch(
    peer: PeerId,
    tx_ids: &[String],
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<usize, NockAppError> {
    let speculative_tx_ids = {
        let mut state_guard = driver_state.lock().await;
        state_guard.track_tx_ids_and_peer(tx_ids.iter().cloned(), peer);
        state_guard.claim_speculative_tx_prefetch_ids(
            tx_ids.iter().cloned(),
            SPECULATIVE_TX_PREFETCH_TTL,
            SPECULATIVE_TX_PREFETCH_MAX_IDS_PER_BLOCK,
        )
    };
    let claimed_count = speculative_tx_ids.len();
    if claimed_count > 0 {
        queue_speculative_raw_tx_prefetches_with_dispatcher(
            peer, speculative_tx_ids, swarm_actions,
        )
        .await?;
    }
    Ok(claimed_count)
}
pub(crate) fn heard_block_tx_ids_from_fact_poke(
    fact_poke: &NounSlab,
) -> Result<Vec<String>, NockAppError> {
    let space = fact_poke.noun_space();
    let fact = unsafe { *fact_poke.root() }.in_space(&space);
    let fact_cell = fact.as_cell()?;
    if !fact_cell.head().eq_bytes(b"fact") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %fact tag",
        )));
    }

    let response = fact_cell.tail().as_cell()?.tail();
    let response_cell = response.as_cell()?;
    if !response_cell.head().eq_bytes(b"heard-block") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %heard-block tag",
        )));
    }

    let tx_ids = page_tx_ids_noun(response_cell.tail())?;
    let mut tx_id_strings = Vec::new();
    collect_tip5_zset_strings(tx_ids, &mut tx_id_strings)?;
    Ok(tx_id_strings)
}

pub(crate) fn heard_block_height_from_fact_poke(fact_poke: &NounSlab) -> Result<u64, NockAppError> {
    let space = fact_poke.noun_space();
    let fact = unsafe { *fact_poke.root() }.in_space(&space);
    let fact_cell = fact.as_cell()?;
    if !fact_cell.head().eq_bytes(b"fact") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %fact tag",
        )));
    }

    let response = fact_cell.tail().as_cell()?.tail();
    let response_cell = response.as_cell()?;
    if !response_cell.head().eq_bytes(b"heard-block") {
        return Err(NockAppError::OtherError(String::from(
            "fact poke missing %heard-block tag",
        )));
    }

    block_height_from_page_noun(response_cell.tail())
}
pub(crate) fn block_height_from_page_noun(page: NounHandle<'_>) -> Result<u64, NockAppError> {
    let (height_index, version_label) = match page
        .as_cell()
        .ok()
        .and_then(|cell| cell.head().as_atom().ok())
        .and_then(|atom| atom.as_u64().ok())
    {
        Some(1) => (10usize, "v1"),
        Some(version) => {
            return Err(NockAppError::OtherError(format!(
                "unsupported page version {version}",
            )));
        }
        None => (9usize, "v0"),
    };

    let mut list = page;
    for index in 0..=height_index {
        let cell = list.as_cell().map_err(|_| {
            NockAppError::OtherError(format!(
                "page {version_label} missing height field at index {height_index}",
            ))
        })?;
        if index == height_index {
            return cell.head().as_atom()?.as_u64().map_err(Into::into);
        }
        list = cell.tail();
    }

    Err(NockAppError::OtherError(format!(
        "page {version_label} missing height field at index {height_index}",
    )))
}

pub(crate) fn future_heard_block_details(
    response: &NockchainFact,
) -> Result<Option<(String, u64)>, NockAppError> {
    let NockchainFact::HeardBlock(block_id, fact_poke) = response else {
        return Ok(None);
    };

    let height = heard_block_height_from_fact_poke(fact_poke)?;
    Ok(Some((block_id.clone(), height)))
}
pub(crate) async fn flush_ready_deferred_heard_blocks_with_dispatcher(
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_actions: &mut SwarmActionDispatcher<'_>,
) -> Result<usize, NockAppError> {
    let ready = { driver_state.lock().await.take_ready_deferred_heard_blocks() };
    if ready.is_empty() {
        return Ok(0);
    }
    let ready_len = ready.len();

    // Flush observability. The main select loop is parked on this for-loop
    // until every deferred block completes its kernel poke, so a slow or
    // livelocked poke inside the flush starves every other driver activity
    // while heartbeat keeps ticking. Per-flush count + elapsed makes the
    // previously-invisible shape of this burst directly observable in logs
    // and greppable by the e2e harness. See
    // docs/incidents/lax1-stall-20260420/README.md.
    info!(
        target: "nockchain::deferred_flush",
        count = ready_len,
        "Flushing deferred heard-blocks at current frontier"
    );
    let flush_start = Instant::now();
    for (peer, response) in ready {
        route_response_fact_with_dispatcher(
            peer, response, traffic, metrics, driver_state, swarm_actions,
        )
        .await?;
    }
    let elapsed_ms = flush_start.elapsed().as_secs_f64() * 1_000.0;
    info!(
        target: "nockchain::deferred_flush",
        count = ready_len,
        elapsed_ms,
        "Completed deferred heard-block flush"
    );
    Ok(ready_len)
}

#[cfg(test)]
pub(crate) async fn flush_ready_deferred_heard_blocks(
    traffic: &traffic_cop::TrafficCop,
    metrics: &Arc<NockchainP2PMetrics>,
    driver_state: &Arc<Mutex<P2PState>>,
    swarm_tx: &mpsc::Sender<SwarmAction>,
) -> Result<usize, NockAppError> {
    let mut swarm_actions = SwarmActionDispatcher::Channel(swarm_tx);
    flush_ready_deferred_heard_blocks_with_dispatcher(
        traffic, metrics, driver_state, &mut swarm_actions,
    )
    .await
}

pub(crate) fn page_tx_ids_noun<'a>(page: NounHandle<'a>) -> Result<NounHandle<'a>, NockAppError> {
    let page_cell = page.as_cell()?;
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version != 1 {
                return Err(NockAppError::OtherError(format!(
                    "unsupported page version {version}",
                )));
            }
            Ok(page_cell
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .tail()
                .as_cell()?
                .head())
        }
        Err(_) => Ok(page_cell
            .tail()
            .as_cell()?
            .tail()
            .as_cell()?
            .tail()
            .as_cell()?
            .head()),
    }
}
pub(crate) fn collect_tip5_zset_strings(
    noun: NounHandle<'_>,
    out: &mut Vec<String>,
) -> Result<(), NockAppError> {
    let mut stack = vec![(noun, false)];
    while let Some((node, emit_value)) = stack.pop() {
        if let Ok(atom) = node.as_atom() {
            if atom.as_u64() == Ok(0) {
                continue;
            }
            return Err(NockAppError::OtherError(String::from(
                "unexpected non-zero atom in tx-id set",
            )));
        }

        let cell = node.as_cell()?;
        if emit_value {
            if out.len() >= TIP5_ZSET_MAX_ITEMS {
                return Err(NockAppError::OtherError(format!(
                    "tx-id set exceeded item budget {TIP5_ZSET_MAX_ITEMS}",
                )));
            }
            out.push(tip5_hash_to_base58(
                cell.head().noun(),
                cell.head().space(),
            )?);
            continue;
        }

        if stack.len().saturating_add(3) > TIP5_ZSET_MAX_STACK {
            return Err(NockAppError::OtherError(format!(
                "tx-id set exceeded traversal stack budget {TIP5_ZSET_MAX_STACK}",
            )));
        }
        let branches = cell.tail().as_cell()?;
        stack.push((branches.tail(), false));
        stack.push((node, true));
        stack.push((branches.head(), false));
    }
    Ok(())
}
