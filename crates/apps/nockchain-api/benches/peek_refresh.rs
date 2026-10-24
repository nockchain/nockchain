#![allow(clippy::print_stdout)]

use std::error::Error;
use std::time::Instant;

use nockapp_grpc_proto::pb::common::v1::{Base58Hash, PageRequest};
use nockapp_grpc_proto::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;
use nockapp_grpc_proto::pb::public::v2::{
    get_block_details_request, get_block_details_response, get_blocks_response,
    get_transaction_details_response, GetBlockDetailsRequest, GetBlocksRequest,
    GetTransactionDetailsRequest,
};
use tonic::transport::Channel;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let explicit_addr = std::env::var("NOCKCHAIN_BENCH_SERVER").ok();
    let addr = explicit_addr
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:50051".to_string());

    // Check if we should test a specific block
    if let Ok(height_str) = std::env::var("TEST_BLOCK_HEIGHT") {
        let height: u64 = height_str.parse()?;
        return test_specific_block(&addr, height).await;
    }

    run_grpc_mode(&addr, explicit_addr.is_some()).await
}

async fn run_grpc_mode(addr: &str, require_server: bool) -> Result<(), Box<dyn Error>> {
    println!("Using existing gRPC server at {}", addr);
    let channel = match Channel::from_shared(addr.to_string())?.connect().await {
        Ok(channel) => channel,
        Err(err) if !require_server => {
            println!(
                "Skipping peek_refresh benchmark: no gRPC server reachable at default address ({err})"
            );
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };
    let mut client = NockchainBlockServiceClient::new(channel);

    let tip_height = fetch_tip_height(&mut client).await?;
    println!("tip height: {}\n", tip_height);

    let sizes = [2_u64, 8, 64, 256, 1024];

    // Benchmark 1: GetBlocks only (metadata from cache)
    println!("=== Benchmark: GetBlocks only (cached metadata) ===");
    run_get_blocks_benchmark(&mut client, tip_height, &sizes).await?;

    // Benchmark 2: GetBlocks + GetBlockDetails for each block
    println!("\n=== Benchmark: GetBlocks + GetBlockDetails (kernel peek per block) ===");
    run_get_blocks_with_details_benchmark(&mut client, tip_height, &sizes).await?;

    Ok(())
}

async fn run_get_blocks_benchmark(
    client: &mut NockchainBlockServiceClient<Channel>,
    tip_height: u64,
    sizes: &[u64],
) -> Result<(), Box<dyn Error>> {
    let mut next_end = tip_height;
    for &size in sizes {
        let start = next_end.saturating_sub(size.saturating_sub(1));
        if start == 0 && next_end == 0 {
            println!("  size={:>4} skipped (no heights available)", size);
            continue;
        }
        next_end = start.saturating_sub(1);

        let page_token = format!("{:x}", start);
        let req = GetBlocksRequest {
            page: Some(PageRequest {
                page_token,
                client_page_items_limit: size as u32,
                max_bytes: 0,
            }),
        };

        let started = Instant::now();
        let resp = client.get_blocks(req).await?;
        let elapsed = started.elapsed();

        if let Some(get_blocks_response::Result::Blocks(data)) = resp.into_inner().result {
            let returned = data.blocks.len();
            let per_block_us = if returned > 0 {
                elapsed.as_micros() as f64 / returned as f64
            } else {
                0.0
            };
            println!(
                "  size={:>4} returned={:>4} total={:>10.2?} per_block={:>8.1}us",
                size, returned, elapsed, per_block_us
            );
        } else {
            println!("  size={:>4} returned no blocks (took {:?})", size, elapsed);
        }
    }
    Ok(())
}

async fn run_get_blocks_with_details_benchmark(
    client: &mut NockchainBlockServiceClient<Channel>,
    tip_height: u64,
    sizes: &[u64],
) -> Result<(), Box<dyn Error>> {
    let mut next_end = tip_height;
    for &size in sizes {
        let start = next_end.saturating_sub(size.saturating_sub(1));
        if start == 0 && next_end == 0 {
            println!("  size={:>4} skipped (no heights available)", size);
            continue;
        }
        next_end = start.saturating_sub(1);

        let page_token = format!("{:x}", start);
        let req = GetBlocksRequest {
            page: Some(PageRequest {
                page_token,
                client_page_items_limit: size as u32,
                max_bytes: 0,
            }),
        };

        let total_started = Instant::now();

        // Phase 1: GetBlocks
        let get_blocks_started = Instant::now();
        let resp = client.get_blocks(req).await?;
        let get_blocks_elapsed = get_blocks_started.elapsed();

        let blocks = match resp.into_inner().result {
            Some(get_blocks_response::Result::Blocks(data)) => data.blocks,
            _ => {
                println!("  size={:>4} GetBlocks returned no blocks", size);
                continue;
            }
        };

        // Phase 2: GetBlockDetails for each block
        let details_started = Instant::now();
        let mut details_count = 0;
        for block in &blocks {
            if let Err(e) = fetch_block_details(client, block.height).await {
                println!(
                    "  Warning: GetBlockDetails failed for height {}: {}",
                    block.height, e
                );
            } else {
                details_count += 1;
            }
        }
        let details_elapsed = details_started.elapsed();

        let total_elapsed = total_started.elapsed();
        let per_detail_us = if details_count > 0 {
            details_elapsed.as_micros() as f64 / details_count as f64
        } else {
            0.0
        };

        println!(
            "  size={:>4} returned={:>4} get_blocks={:>10.2?} details={:>10.2?} total={:>10.2?} per_detail={:>10.1}us",
            size, blocks.len(), get_blocks_elapsed, details_elapsed, total_elapsed, per_detail_us
        );
    }
    Ok(())
}

async fn fetch_block_details(
    client: &mut NockchainBlockServiceClient<Channel>,
    height: u64,
) -> Result<(), Box<dyn Error>> {
    let request = GetBlockDetailsRequest {
        selector: Some(get_block_details_request::Selector::Height(height)),
    };

    let resp = client.get_block_details(request).await?;
    match resp.into_inner().result {
        Some(get_block_details_response::Result::Details(_)) => Ok(()),
        Some(get_block_details_response::Result::Error(e)) => {
            Err(format!("GetBlockDetails error: {}", e.message).into())
        }
        None => Err("GetBlockDetails returned no result".into()),
    }
}

async fn fetch_tip_height(
    client: &mut NockchainBlockServiceClient<Channel>,
) -> Result<u64, Box<dyn Error>> {
    let req = GetBlocksRequest {
        page: Some(PageRequest {
            page_token: "".into(),
            client_page_items_limit: 1,
            max_bytes: 0,
        }),
    };
    let resp = client.get_blocks(req).await?;
    if let Some(get_blocks_response::Result::Blocks(data)) = resp.into_inner().result {
        Ok(data.current_height)
    } else {
        Err("GetBlocks returned no data".into())
    }
}

/// Test a specific block and all its transactions for decoding errors.
/// Run with: TEST_BLOCK_HEIGHT=43099 cargo run --bin peek_refresh
async fn test_specific_block(addr: &str, height: u64) -> Result<(), Box<dyn Error>> {
    println!("=== Testing specific block at height {} ===\n", height);

    let channel = Channel::from_shared(addr.to_string())?.connect().await?;
    let mut client = NockchainBlockServiceClient::new(channel);

    // Step 1: Get block details
    println!("Step 1: Fetching block details for height {}...", height);
    let request = GetBlockDetailsRequest {
        selector: Some(get_block_details_request::Selector::Height(height)),
    };

    let resp = client.get_block_details(request).await;
    match resp {
        Ok(response) => {
            let inner = response.into_inner();
            match inner.result {
                Some(get_block_details_response::Result::Details(details)) => {
                    println!("  Block ID: {:?}", details.block_id);
                    println!("  Version: {}", details.version);
                    println!("  Tx count: {}", details.tx_ids.len());
                    println!("  Block details fetched OK!\n");

                    // Step 2: Try to fetch each transaction's details
                    println!("Step 2: Fetching transaction details...");
                    for (i, tx_id) in details.tx_ids.iter().enumerate() {
                        println!(
                            "  [{}/{}] Fetching tx: {}",
                            i + 1,
                            details.tx_ids.len(),
                            tx_id.hash
                        );

                        let tx_request = GetTransactionDetailsRequest {
                            tx_id: Some(Base58Hash {
                                hash: tx_id.hash.clone(),
                            }),
                        };

                        match client.get_transaction_details(tx_request).await {
                            Ok(tx_resp) => {
                                let tx_inner = tx_resp.into_inner();
                                match tx_inner.result {
                                    Some(get_transaction_details_response::Result::Details(
                                        tx_details,
                                    )) => {
                                        println!("    Version: {}", tx_details.version);
                                        println!("    Inputs: {}", tx_details.inputs.len());
                                        println!("    Outputs: {}", tx_details.outputs.len());
                                        println!("    OK!");
                                    }
                                    Some(get_transaction_details_response::Result::Pending(_)) => {
                                        println!("    Status: Pending");
                                    }
                                    Some(get_transaction_details_response::Result::Error(e)) => {
                                        println!("    ERROR in response: {}", e.message);
                                    }
                                    None => {
                                        println!("    ERROR: Empty response");
                                    }
                                }
                            }
                            Err(e) => {
                                println!("    gRPC ERROR: {:#?}", e);
                            }
                        }
                    }
                }
                Some(get_block_details_response::Result::Error(e)) => {
                    println!("  ERROR in response: {}", e.message);
                }
                None => {
                    println!("  ERROR: Empty response");
                }
            }
        }
        Err(e) => {
            println!("  gRPC ERROR: {:#?}", e);
        }
    }

    println!("\n=== Test complete ===");
    Ok(())
}
