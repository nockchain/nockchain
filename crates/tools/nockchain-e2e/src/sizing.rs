use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use nockapp_grpc_proto::pb::public::v2::BlockDetails;
use nockchain_types::tx_engine::common::Hash;
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::warn;

use crate::grpc::{
    fetch_block_details_by_height, fetch_transaction_details, transaction_location, TxLocation,
};

fn observed_block_raw_bytes(block_details: &BlockDetails) -> u64 {
    block_details.raw_page_bytes.unwrap_or_else(|| {
        block_details
            .msg
            .as_ref()
            .map(|msg| msg.raw.len() as u64)
            .unwrap_or(0)
    })
}

fn is_transient_tx_location_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("get_transaction_block") && message.contains("timed out")
}

#[derive(Debug, Serialize)]
pub struct FanInBlockSizeSummary {
    pub schema_version: &'static str,
    pub tx_id: String,
    pub block_height: u64,
    pub selected_note_count: u64,
    pub total_input_nicks: u64,
    pub fee_nicks: u64,
    pub send_amount_nicks: u64,
    pub tx_size_bytes: u64,
    pub tx_input_count: usize,
    pub tx_output_count: usize,
    pub block_tx_count: u32,
    pub block_raw_bytes: u64,
    pub gen2_batch_max_bytes: u64,
    pub gen2_item_max_bytes: u64,
    pub tx_size_vs_item_cap_ratio: f64,
    pub block_raw_vs_item_cap_ratio: f64,
    pub block_raw_vs_batch_cap_ratio: f64,
}

#[allow(clippy::too_many_arguments)]
pub async fn build_fan_in_block_size_summary(
    server: &str,
    tx_id: &Hash,
    selected_note_count: u64,
    total_input_nicks: u64,
    fee_nicks: u64,
    send_amount_nicks: u64,
    wait_timeout: Duration,
    gen2_batch_max_bytes: u64,
    gen2_item_max_bytes: u64,
) -> Result<FanInBlockSizeSummary> {
    let deadline = Instant::now() + wait_timeout;
    let block_height = loop {
        match transaction_location(server, tx_id).await {
            Ok(TxLocation::InBlock { height }) => break height,
            Ok(TxLocation::Pending | TxLocation::NotFound) => {
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for transaction {} to land in a block on {}",
                        tx_id.to_base58(),
                        server
                    ));
                }
            }
            Err(err) if Instant::now() < deadline && is_transient_tx_location_error(&err) => {}
            Err(err) => return Err(err),
        }
        sleep(Duration::from_secs(1)).await;
    };

    let tx_details = fetch_transaction_details(server, tx_id).await?;
    let block_details = fetch_block_details_by_height(server, block_height).await?;
    let block_raw_bytes = observed_block_raw_bytes(&block_details);
    let tx_size_bytes = tx_details.size_bytes;

    Ok(FanInBlockSizeSummary {
        schema_version: "nous_gen2_block_size_stress_v1",
        tx_id: tx_id.to_base58(),
        block_height,
        selected_note_count,
        total_input_nicks,
        fee_nicks,
        send_amount_nicks,
        tx_size_bytes,
        tx_input_count: tx_details.inputs.len(),
        tx_output_count: tx_details.outputs.len(),
        block_tx_count: block_details.tx_count,
        block_raw_bytes,
        gen2_batch_max_bytes,
        gen2_item_max_bytes,
        tx_size_vs_item_cap_ratio: tx_size_bytes as f64 / gen2_item_max_bytes as f64,
        block_raw_vs_item_cap_ratio: block_raw_bytes as f64 / gen2_item_max_bytes as f64,
        block_raw_vs_batch_cap_ratio: block_raw_bytes as f64 / gen2_batch_max_bytes as f64,
    })
}

/// One block's contribution to the block-plus-txs bundle-size distribution.
#[derive(Debug, Serialize, Clone)]
pub struct BlockTxSample {
    pub height: u64,
    pub block_id: String,
    pub block_raw_bytes: u64,
    pub tx_count: u64,
    pub tx_bytes: Vec<u64>,
    pub tx_bytes_sum: u64,
    pub combined_bytes: u64,
    pub tx_bytes_missing_count: u64,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct DistributionStats {
    pub count: u64,
    pub min: u64,
    pub p50: u64,
    pub p90: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
    pub mean: u64,
    pub total: u64,
}

fn quantile(sorted: &[u64], q: f64) -> u64 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    if n == 1 {
        return sorted[0];
    }
    let k = (n as f64 - 1.0) * q;
    let lo = k.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = k - lo as f64;
    let v = sorted[lo] as f64 + (sorted[hi] as f64 - sorted[lo] as f64) * frac;
    v.round() as u64
}

fn summarize(values: &[u64]) -> DistributionStats {
    if values.is_empty() {
        return DistributionStats::default();
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let total: u128 = sorted.iter().map(|v| *v as u128).sum();
    DistributionStats {
        count: sorted.len() as u64,
        min: *sorted
            .first()
            .expect("nonempty distribution should have min"),
        p50: quantile(&sorted, 0.50),
        p90: quantile(&sorted, 0.90),
        p95: quantile(&sorted, 0.95),
        p99: quantile(&sorted, 0.99),
        max: *sorted
            .last()
            .expect("nonempty distribution should have max"),
        mean: (total / sorted.len() as u128) as u64,
        total: total as u64,
    }
}

/// Aggregate bundle-size report for a swept height range.
///
/// The point of this shape is to answer the question "can a block + all its
/// transactions fit inside a single Gen2 batch response envelope?" — i.e.
/// whether an atomic block-plus-txs bundle design will blow past
/// `gen2_block_batch_max_response_bytes` under realistic traffic, and how
/// much headroom we actually have.
#[derive(Debug, Serialize)]
pub struct BlockTxDistributionReport {
    pub schema_version: &'static str,
    pub server: String,
    pub range_start: u64,
    pub range_end: u64,
    pub requested_heights: u64,
    pub blocks_observed: u64,
    pub heights_not_found: u64,
    pub blocks_with_missing_tx_data: u64,
    pub gen2_batch_max_bytes: u64,
    pub gen2_block_batch_max_response_bytes: u64,
    pub gen2_item_max_bytes: u64,
    pub block_raw_bytes: DistributionStats,
    pub tx_bytes_sum_per_block: DistributionStats,
    pub combined_per_block: DistributionStats,
    pub individual_tx_bytes: DistributionStats,
    pub tx_count_per_block: DistributionStats,
    pub blocks_over_block_batch_cap: u64,
    pub blocks_over_gen2_batch_cap: u64,
    pub samples: Vec<BlockTxSample>,
}

async fn collect_sample_for_height(server: &str, height: u64) -> Option<BlockTxSample> {
    let block = match fetch_block_details_by_height(server, height).await {
        Ok(b) => b,
        Err(err) => {
            warn!(height, error = %err, "block fetch failed; skipping");
            return None;
        }
    };
    let block_raw_bytes = observed_block_raw_bytes(&block);
    // block_id is a common.v1.Hash (fixed 5-belt struct). Converting to base58
    // needs a dedicated helper; we don't need it for distribution analysis,
    // so we leave it blank and let height act as the primary key.
    let block_id = String::new();
    let tx_ids: Vec<Hash> = block
        .tx_ids
        .iter()
        .filter_map(|b58| Hash::from_base58(&b58.hash).ok())
        .collect();
    let tx_count = tx_ids.len() as u64;

    let mut tx_bytes: Vec<u64> = Vec::with_capacity(tx_ids.len());
    let mut missing: u64 = 0;
    for tid in &tx_ids {
        match fetch_transaction_details(server, tid).await {
            Ok(td) => tx_bytes.push(td.size_bytes),
            Err(err) => {
                warn!(height, tx_id = %tid.to_base58(), error = %err, "tx fetch failed; counting as missing");
                missing += 1;
            }
        }
    }
    let tx_bytes_sum: u64 = tx_bytes.iter().sum();

    Some(BlockTxSample {
        height,
        block_id,
        block_raw_bytes,
        tx_count,
        tx_bytes,
        tx_bytes_sum,
        combined_bytes: block_raw_bytes.saturating_add(tx_bytes_sum),
        tx_bytes_missing_count: missing,
    })
}

/// Sweep every height in `[start_height, end_height]` inclusive, fetching
/// block + transaction details and tallying sizes.
///
/// Missing heights (`NotFound` from the running node) are skipped silently;
/// the report records how many we expected versus actually observed. Tx
/// fetch errors within an observed block are recorded per-block.
pub async fn collect_block_tx_distribution(
    server: &str,
    start_height: u64,
    end_height: u64,
    concurrency: usize,
    gen2_batch_max_bytes: u64,
    gen2_block_batch_max_response_bytes: u64,
    gen2_item_max_bytes: u64,
) -> Result<BlockTxDistributionReport> {
    if end_height < start_height {
        return Err(anyhow!(
            "end-height ({end_height}) must be >= start-height ({start_height})"
        ));
    }
    let concurrency = concurrency.max(1);
    let requested = end_height - start_height + 1;

    let semaphore = Arc::new(Semaphore::new(concurrency));
    let server = server.to_string();
    let mut join_set: JoinSet<(u64, Option<BlockTxSample>)> = JoinSet::new();

    for height in start_height..=end_height {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .context("semaphore closed")?;
        let server_copy = server.clone();
        join_set.spawn(async move {
            let _permit = permit;
            let sample = collect_sample_for_height(&server_copy, height).await;
            (height, sample)
        });
    }

    let mut samples: Vec<BlockTxSample> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok((_height, Some(sample))) => samples.push(sample),
            Ok((_height, None)) => {}
            Err(err) => return Err(anyhow!("join error: {err}")),
        }
    }
    samples.sort_by_key(|s| s.height);

    let blocks_observed = samples.len() as u64;
    let heights_not_found = requested.saturating_sub(blocks_observed);
    let blocks_with_missing_tx_data = samples
        .iter()
        .filter(|s| s.tx_bytes_missing_count > 0)
        .count() as u64;

    let block_bytes_vec: Vec<u64> = samples.iter().map(|s| s.block_raw_bytes).collect();
    let tx_sum_vec: Vec<u64> = samples.iter().map(|s| s.tx_bytes_sum).collect();
    let combined_vec: Vec<u64> = samples.iter().map(|s| s.combined_bytes).collect();
    let tx_counts_vec: Vec<u64> = samples.iter().map(|s| s.tx_count).collect();
    let individual_tx_vec: Vec<u64> = samples
        .iter()
        .flat_map(|s| s.tx_bytes.iter().copied())
        .collect();

    let blocks_over_block_batch_cap = combined_vec
        .iter()
        .filter(|v| **v > gen2_block_batch_max_response_bytes)
        .count() as u64;
    let blocks_over_gen2_batch_cap = combined_vec
        .iter()
        .filter(|v| **v > gen2_batch_max_bytes)
        .count() as u64;

    Ok(BlockTxDistributionReport {
        schema_version: "block_tx_distribution_v1",
        server,
        range_start: start_height,
        range_end: end_height,
        requested_heights: requested,
        blocks_observed,
        heights_not_found,
        blocks_with_missing_tx_data,
        gen2_batch_max_bytes,
        gen2_block_batch_max_response_bytes,
        gen2_item_max_bytes,
        block_raw_bytes: summarize(&block_bytes_vec),
        tx_bytes_sum_per_block: summarize(&tx_sum_vec),
        combined_per_block: summarize(&combined_vec),
        individual_tx_bytes: summarize(&individual_tx_vec),
        tx_count_per_block: summarize(&tx_counts_vec),
        blocks_over_block_batch_cap,
        blocks_over_gen2_batch_cap,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use nockapp_grpc_proto::pb::public::v2::{BlockDetails, PageMsg};

    use super::observed_block_raw_bytes;

    #[test]
    fn prefers_raw_page_bytes_when_present() {
        let block_details = BlockDetails {
            raw_page_bytes: Some(4096),
            msg: Some(PageMsg {
                raw: vec![1, 2, 3],
                decoded: String::new(),
            }),
            ..Default::default()
        };

        assert_eq!(observed_block_raw_bytes(&block_details), 4096);
    }

    #[test]
    fn falls_back_to_message_bytes_for_older_servers() {
        let block_details = BlockDetails {
            raw_page_bytes: None,
            msg: Some(PageMsg {
                raw: vec![1, 2, 3, 4],
                decoded: String::new(),
            }),
            ..Default::default()
        };

        assert_eq!(observed_block_raw_bytes(&block_details), 4);
    }
}
