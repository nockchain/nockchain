use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use nockapp::nockapp::wire::WireRepr;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp_grpc::error::NockAppGrpcError;
use nockapp_grpc::private_nockapp::client::{PokeResponseResult, PrivateNockAppGrpcClient};
use nockapp_grpc_proto::pb::common::v1::{
    Base58Hash, ErrorCode, Hash as PbHash, PageRequest, Wire,
};
use nockapp_grpc_proto::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;
use nockapp_grpc_proto::pb::public::v2::nockchain_metrics_service_client::NockchainMetricsServiceClient;
use nockapp_grpc_proto::pb::public::v2::nockchain_service_client::NockchainServiceClient;
use nockapp_grpc_proto::pb::public::v2::{
    get_block_details_request, get_block_details_response, get_blocks_response,
    get_explorer_metrics_response, get_peer_stats_response, get_transaction_block_response,
    get_transaction_details_response, transaction_accepted_response,
    wallet_send_transaction_response, BlockDetails, GetBlockDetailsRequest, GetBlocksRequest,
    GetExplorerMetricsRequest, GetPeerStatsRequest, GetTransactionBlockRequest,
    GetTransactionDetailsRequest, TransactionAcceptedRequest, TransactionDetails,
    WalletSendTransactionRequest,
};
use nockchain_mining_common::{MiningKeyConfig, MiningPkhConfig, NodeClient, NodeClientError};
use nockchain_types::tx_engine::common::{BlockHeight, Hash, Page};
use nockvm::noun::{NounAllocator, SIG, T};
use noun_serde::NounDecode;
use tokio::time::{sleep, timeout as tokio_timeout};
use tonic::Request;

/// Thread-safe private-RPC failures exposed by the E2E controller. SDK errors
/// can own local noun allocations; convert them here, never change their
/// Send/Sync contracts or confuse malformed requests with a kernel NACK.
#[derive(Debug)]
pub enum PrivateRpcError {
    Transport(tonic::transport::Error),
    Status(tonic::Status),
    Client(tonic::Status),
    KernelRejected { code: i32, message: String },
    PokeResponse { code: i32, message: String },
    PokeDeadlineBeforeSend,
    PokeAckTimedOut,
    PokeAckCancelled,
    CandidateDecode(String),
    TimedOut { label: String, duration: Duration },
}

impl PrivateRpcError {
    fn poke_rejected(code: i32, message: String) -> Self {
        if code == ErrorCode::PokeFailed as i32 {
            Self::KernelRejected { code, message }
        } else {
            Self::PokeResponse { code, message }
        }
    }
}

impl From<NockAppGrpcError> for PrivateRpcError {
    fn from(error: NockAppGrpcError) -> Self {
        match error {
            NockAppGrpcError::Transport(error) => Self::Transport(error),
            NockAppGrpcError::Status(status) => Self::Status(status),
            // Preserve the SDK's defined status conversion for local failures.
            error => Self::Client(error.into()),
        }
    }
}

impl From<NodeClientError> for PrivateRpcError {
    fn from(error: NodeClientError) -> Self {
        match error {
            NodeClientError::Grpc(error) => error.into(),
            NodeClientError::PokeRejected { code, message } => Self::poke_rejected(code, message),
            NodeClientError::PokeDeadlineBeforeSend => Self::PokeDeadlineBeforeSend,
            NodeClientError::PokeAckTimedOut => Self::PokeAckTimedOut,
            NodeClientError::PokeAckCancelled => Self::PokeAckCancelled,
            NodeClientError::Decode(error) => Self::CandidateDecode(error.to_string()),
        }
    }
}

impl std::fmt::Display for PrivateRpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "gRPC transport error: {error}"),
            Self::Status(status) => write!(formatter, "gRPC status error: {status}"),
            Self::Client(status) => write!(formatter, "private RPC client error: {status}"),
            Self::KernelRejected { code, message } => {
                write!(
                    formatter,
                    "kernel rejected poke: code={code} message={message}"
                )
            }
            Self::PokeResponse { code, message } => {
                write!(
                    formatter,
                    "private RPC rejected request: code={code} message={message}"
                )
            }
            Self::PokeDeadlineBeforeSend => {
                formatter.write_str("poke deadline expired before send")
            }
            Self::PokeAckTimedOut => {
                formatter.write_str("poke acknowledgement not observed before deadline")
            }
            Self::PokeAckCancelled => {
                formatter.write_str("poke acknowledgement not observed before shutdown")
            }
            Self::CandidateDecode(message) => {
                write!(formatter, "decoding candidate effect: {message}")
            }
            Self::TimedOut { label, duration } => {
                write!(
                    formatter,
                    "{label} timed out after {}ms",
                    duration.as_millis()
                )
            }
        }
    }
}

impl std::error::Error for PrivateRpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Status(status) | Self::Client(status) => Some(status),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeadInfo {
    pub height: u64,
    pub block_id: Option<nockapp_grpc_proto::pb::common::v1::Hash>,
}

#[derive(Debug, Clone)]
pub struct PrivateHeadInfo {
    pub height: u64,
    pub block_id: Option<Hash>,
}

#[derive(Debug, Clone)]
pub struct SubmitTxOutcome {
    pub acknowledged: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoLiveInfo {
    pub peer_count: usize,
    pub cache_height: u64,
    pub heaviest_height: u64,
    pub refresh_success_count: u64,
    pub backfill_success_count: u64,
}

pub type ReviewReadyInfo = DemoLiveInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedCatchUpInfo {
    pub local: DemoLiveInfo,
    pub reference_cache_height: u64,
}

const DEFAULT_V0_MINING_KEY: &str =
    "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ";
const DEFAULT_GRPC_CONNECT_TIMEOUT_MS: u64 = 1_000;
const DEFAULT_GRPC_REQUEST_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxLocation {
    InBlock { height: u64 },
    Pending,
    NotFound,
}

pub async fn wait_for_ready(addr: &str, timeout: Duration) -> Result<()> {
    wait_for_ready_with_timeouts(addr, timeout, grpc_timeouts()).await
}

async fn wait_for_ready_with_timeouts(
    addr: &str,
    timeout: Duration,
    timeouts: GrpcTimeouts,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match fetch_head_with_timeouts(addr, timeouts).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err.context(format!("timed out waiting for gRPC at {}", addr)));
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn wait_for_height(addr: &str, height: u64, timeout: Duration) -> Result<HeadInfo> {
    wait_for_height_with_timeouts(addr, height, timeout, grpc_timeouts()).await
}

pub async fn wait_for_demo_live(addr: &str, timeout: Duration) -> Result<DemoLiveInfo> {
    wait_for_demo_live_with_timeouts(addr, timeout, grpc_timeouts()).await
}

pub async fn wait_for_review_ready(addr: &str, timeout: Duration) -> Result<ReviewReadyInfo> {
    wait_for_demo_live(addr, timeout).await
}

pub async fn wait_for_seed_catch_up(
    addr: &str,
    reference_addr: &str,
    timeout: Duration,
) -> Result<SeedCatchUpInfo> {
    wait_for_seed_catch_up_with_timeouts(addr, reference_addr, timeout, grpc_timeouts()).await
}

async fn wait_for_height_with_timeouts(
    addr: &str,
    height: u64,
    timeout: Duration,
    timeouts: GrpcTimeouts,
) -> Result<HeadInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        match fetch_head_with_timeouts(addr, timeouts).await {
            Ok(head) => {
                if head.height >= height {
                    return Ok(head);
                }
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for height {} at {} (current {})", height, addr,
                        head.height
                    ));
                }
            }
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err.context(format!(
                        "timed out waiting for height {} at {}",
                        height, addr
                    )));
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_demo_live_with_timeouts(
    addr: &str,
    timeout: Duration,
    timeouts: GrpcTimeouts,
) -> Result<DemoLiveInfo> {
    let deadline = Instant::now() + timeout;
    let mut last_observation = None;
    let mut last_error: Option<anyhow::Error>;

    loop {
        last_error = None;
        match fetch_demo_live_info_with_timeouts(addr, timeouts).await {
            Ok(info) => {
                if demo_live(&info) {
                    return Ok(info);
                }
                last_observation = Some(info);
            }
            Err(err) => last_error = Some(err),
        }

        if Instant::now() >= deadline {
            if let Some(info) = last_observation {
                return Err(anyhow!(
                    "timed out waiting for demo-live telemetry at {} ({})",
                    addr,
                    format_demo_live_observation(&info)
                ));
            }
            if let Some(err) = last_error {
                return Err(err.context(format!(
                    "timed out waiting for demo-live telemetry at {}",
                    addr
                )));
            }
            return Err(anyhow!(
                "timed out waiting for demo-live telemetry at {}", addr
            ));
        }

        sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_seed_catch_up_with_timeouts(
    addr: &str,
    reference_addr: &str,
    timeout: Duration,
    timeouts: GrpcTimeouts,
) -> Result<SeedCatchUpInfo> {
    let deadline = Instant::now() + timeout;
    let remaining_timeout = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    let reference =
        wait_for_demo_live_with_timeouts(reference_addr, remaining_timeout, timeouts).await?;
    let reference_cache_height = reference.cache_height;
    let mut last_observation = None;
    let mut last_error: Option<anyhow::Error>;

    loop {
        last_error = None;
        match fetch_demo_live_info_with_timeouts(addr, timeouts).await {
            Ok(info) => {
                if caught_up_to_reference_cache(&info, reference_cache_height) {
                    return Ok(SeedCatchUpInfo {
                        local: info,
                        reference_cache_height,
                    });
                }
                last_observation = Some(info);
            }
            Err(err) => last_error = Some(err),
        }

        if Instant::now() >= deadline {
            if let Some(info) = last_observation {
                return Err(anyhow!(
                    "timed out waiting for {} to reach {} cache height {} ({})",
                    addr,
                    reference_addr,
                    reference_cache_height,
                    format_seed_catch_up_observation(&info, reference_cache_height)
                ));
            }
            if let Some(err) = last_error {
                return Err(err.context(format!(
                    "timed out waiting for {} to reach {} cache height {}",
                    addr, reference_addr, reference_cache_height
                )));
            }
            return Err(anyhow!(
                "timed out waiting for {} to reach {} cache height {}", addr, reference_addr,
                reference_cache_height
            ));
        }

        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn wait_for_height_private(
    addr: &str,
    auth_token: Option<&str>,
    height: u64,
    timeout: Duration,
) -> Result<PrivateHeadInfo> {
    wait_for_height_private_with_timeouts(addr, auth_token, height, timeout, grpc_timeouts()).await
}

async fn wait_for_height_private_with_timeouts(
    addr: &str,
    auth_token: Option<&str>,
    height: u64,
    timeout: Duration,
    timeouts: GrpcTimeouts,
) -> Result<PrivateHeadInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        match fetch_heaviest_private_with_timeouts(addr, auth_token, timeouts).await {
            Ok(head) => {
                if head.height >= height {
                    return Ok(head);
                }
                if Instant::now() >= deadline {
                    return Err(anyhow!(
                        "timed out waiting for height {} at {} (current {})", height, addr,
                        head.height
                    ));
                }
            }
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(err.context(format!(
                        "timed out waiting for height {} at {}",
                        height, addr
                    )));
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn fetch_head(addr: &str) -> Result<HeadInfo> {
    fetch_head_with_timeouts(addr, grpc_timeouts()).await
}

async fn fetch_demo_live_info_with_timeouts(
    addr: &str,
    timeouts: GrpcTimeouts,
) -> Result<DemoLiveInfo> {
    let mut client = connect_metrics_client(addr, timeouts).await?;

    let peer_response = run_grpc_op(
        format!("get_peer_stats at {}", addr),
        timeouts.request,
        client.get_peer_stats(Request::new(GetPeerStatsRequest {})),
    )
    .await?
    .into_inner();
    let peer_count = match peer_response.result {
        Some(get_peer_stats_response::Result::Stats(stats)) => stats.peers.len(),
        Some(get_peer_stats_response::Result::Error(err)) => {
            return Err(anyhow!(
                "get_peer_stats error: code={:?} message={}", err.code, err.message
            ));
        }
        None => return Err(anyhow!("get_peer_stats returned no result")),
    };

    let metrics_response = run_grpc_op(
        format!("get_explorer_metrics at {}", addr),
        timeouts.request,
        client.get_explorer_metrics(Request::new(GetExplorerMetricsRequest {})),
    )
    .await?
    .into_inner();
    match metrics_response.result {
        Some(get_explorer_metrics_response::Result::Metrics(metrics)) => Ok(DemoLiveInfo {
            peer_count,
            cache_height: metrics.cache_height,
            heaviest_height: metrics.heaviest_height,
            refresh_success_count: metrics.refresh_success_count,
            backfill_success_count: metrics.backfill_success_count,
        }),
        Some(get_explorer_metrics_response::Result::Error(err)) => Err(anyhow!(
            "get_explorer_metrics error: code={:?} message={}", err.code, err.message
        )),
        None => Err(anyhow!("get_explorer_metrics returned no result")),
    }
}

async fn fetch_head_with_timeouts(addr: &str, timeouts: GrpcTimeouts) -> Result<HeadInfo> {
    let mut client = connect_block_client(addr, timeouts).await?;

    let request = GetBlocksRequest {
        page: Some(PageRequest {
            client_page_items_limit: 1,
            page_token: String::new(),
            max_bytes: 0,
        }),
    };

    let response = run_grpc_op(
        format!("get_blocks at {}", addr),
        timeouts.request,
        client.get_blocks(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(get_blocks_response::Result::Blocks(data)) => {
            let block_id = data.blocks.first().and_then(|entry| entry.block_id);
            Ok(HeadInfo {
                height: data.current_height,
                block_id,
            })
        }
        Some(get_blocks_response::Result::Error(err)) => Err(anyhow!(
            "get_blocks error: code={:?} message={}", err.code, err.message
        )),
        None => Err(anyhow!("get_blocks returned no result")),
    }
}

pub async fn fetch_heaviest_height_private(addr: &str, auth_token: Option<&str>) -> Result<u64> {
    let head = fetch_heaviest_private(addr, auth_token).await?;
    Ok(head.height)
}

pub async fn fetch_heaviest_private(
    addr: &str,
    auth_token: Option<&str>,
) -> Result<PrivateHeadInfo> {
    fetch_heaviest_private_with_timeouts(addr, auth_token, grpc_timeouts()).await
}

async fn fetch_heaviest_private_with_timeouts(
    addr: &str,
    auth_token: Option<&str>,
    timeouts: GrpcTimeouts,
) -> Result<PrivateHeadInfo> {
    let mut client = connect_private_client(addr, auth_token, timeouts).await?;

    let chain_path = encode_heaviest_chain_path()?;
    let chain_data = peek_path(&mut client, chain_path, "heaviest-chain", addr, timeouts).await?;
    if let Some((height, hash)) = decode_heaviest_chain(chain_data)? {
        return Ok(PrivateHeadInfo {
            height: height.0 .0,
            block_id: Some(hash),
        });
    }

    let block_path = encode_heaviest_block_path()?;
    let block_data = peek_path(&mut client, block_path, "heaviest-block", addr, timeouts).await?;
    if let Some(page) = decode_heaviest_block(block_data)? {
        return Ok(PrivateHeadInfo {
            height: page.height,
            block_id: Some(page.digest),
        });
    }

    Ok(PrivateHeadInfo {
        height: 0,
        block_id: None,
    })
}

pub async fn fetch_constants_private(addr: &str, auth_token: Option<&str>) -> Result<Vec<u8>> {
    let timeouts = grpc_timeouts();
    let mut client = connect_private_client(addr, auth_token, timeouts).await?;
    peek_path(
        &mut client,
        encode_constants_path()?,
        "constants",
        addr,
        timeouts,
    )
    .await
}

pub async fn submit_raw_tx(
    addr: &str,
    raw_tx: nockchain_types::tx_engine::v1::RawTx,
    tx_id_override: Option<Hash>,
) -> Result<SubmitTxOutcome> {
    let timeouts = grpc_timeouts();
    let mut client = connect_service_client(addr, timeouts).await?;

    let tx_id = tx_id_override.unwrap_or_else(|| raw_tx.id.clone());
    let request = WalletSendTransactionRequest {
        tx_id: Some(PbHash::from(tx_id)),
        raw_tx: Some(raw_tx.into()),
    };

    let response = run_grpc_op(
        format!("wallet_send_transaction at {}", addr),
        timeouts.request,
        client.wallet_send_transaction(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(wallet_send_transaction_response::Result::Ack(_)) => Ok(SubmitTxOutcome {
            acknowledged: true,
            error: None,
        }),
        Some(wallet_send_transaction_response::Result::Error(err)) => Ok(SubmitTxOutcome {
            acknowledged: false,
            error: Some(format!("code={:?} message={}", err.code, err.message)),
        }),
        None => Err(anyhow!("wallet_send_transaction returned no result")),
    }
}

pub async fn transaction_accepted(addr: &str, tx_id: &Hash) -> Result<bool> {
    let timeouts = grpc_timeouts();
    let mut client = connect_service_client(addr, timeouts).await?;

    let request = TransactionAcceptedRequest {
        tx_id: Some(nockapp_grpc_proto::pb::common::v1::Base58Hash {
            hash: tx_id.to_base58(),
        }),
    };

    let response = run_grpc_op(
        format!("transaction_accepted at {}", addr),
        timeouts.request,
        client.transaction_accepted(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(transaction_accepted_response::Result::Accepted(accepted)) => Ok(accepted),
        Some(transaction_accepted_response::Result::Error(err)) => Err(anyhow!(
            "transaction_accepted error: code={:?} message={}", err.code, err.message
        )),
        None => Err(anyhow!("transaction_accepted returned no result")),
    }
}

pub async fn transaction_location(addr: &str, tx_id: &Hash) -> Result<TxLocation> {
    let timeouts = grpc_timeouts();
    let mut client = connect_block_client(addr, timeouts).await?;

    let request = GetTransactionBlockRequest {
        tx_id: Some(Base58Hash {
            hash: tx_id.to_base58(),
        }),
    };

    let response = run_grpc_op(
        format!("get_transaction_block at {}", addr),
        timeouts.request,
        client.get_transaction_block(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(get_transaction_block_response::Result::Block(block)) => Ok(TxLocation::InBlock {
            height: block.height,
        }),
        Some(get_transaction_block_response::Result::Pending(_)) => Ok(TxLocation::Pending),
        Some(get_transaction_block_response::Result::Error(err)) => {
            if err.message.contains("not found") {
                Ok(TxLocation::NotFound)
            } else {
                Err(anyhow!(
                    "get_transaction_block error: code={:?} message={}", err.code, err.message
                ))
            }
        }
        None => Err(anyhow!("get_transaction_block returned no result")),
    }
}

pub async fn fetch_transaction_details(addr: &str, tx_id: &Hash) -> Result<TransactionDetails> {
    let timeouts = grpc_timeouts();
    let mut client = connect_block_client(addr, timeouts).await?;

    let request = GetTransactionDetailsRequest {
        tx_id: Some(Base58Hash {
            hash: tx_id.to_base58(),
        }),
    };

    let response = run_grpc_op(
        format!("get_transaction_details at {}", addr),
        timeouts.request,
        client.get_transaction_details(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(get_transaction_details_response::Result::Details(details)) => Ok(details),
        Some(get_transaction_details_response::Result::Pending(_)) => Err(anyhow!(
            "transaction {} is still pending on {}",
            tx_id.to_base58(),
            addr
        )),
        Some(get_transaction_details_response::Result::Error(err)) => Err(anyhow!(
            "get_transaction_details error: code={:?} message={}", err.code, err.message
        )),
        None => Err(anyhow!("get_transaction_details returned no result")),
    }
}

pub async fn fetch_block_details_by_height(addr: &str, height: u64) -> Result<BlockDetails> {
    let timeouts = grpc_timeouts();
    let mut client = connect_block_client(addr, timeouts).await?;

    let request = GetBlockDetailsRequest {
        selector: Some(get_block_details_request::Selector::Height(height)),
    };

    let response = run_grpc_op(
        format!("get_block_details at {}", addr),
        timeouts.request,
        client.get_block_details(request),
    )
    .await?
    .into_inner();

    match response.result {
        Some(get_block_details_response::Result::Details(details)) => Ok(details),
        Some(get_block_details_response::Result::Error(err)) => Err(anyhow!(
            "get_block_details error: code={:?} message={}", err.code, err.message
        )),
        None => Err(anyhow!("get_block_details returned no result")),
    }
}

pub async fn wait_for_tx_in_block(
    addr: &str,
    tx_id: &Hash,
    timeout: Duration,
) -> Result<Option<u64>> {
    let deadline = Instant::now() + timeout;
    loop {
        match transaction_location(addr, tx_id).await? {
            TxLocation::InBlock { height } => return Ok(Some(height)),
            TxLocation::Pending | TxLocation::NotFound => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn poke_private(addr: &str, auth_token: Option<&str>, payload: Vec<u8>) -> Result<()> {
    let timeouts = grpc_timeouts();
    let mut client = connect_private_client(addr, auth_token, timeouts).await?;
    let wire = Wire {
        source: "grpc".to_string(),
        version: 1,
        tags: vec![],
    };
    let response = run_grpc_op(format!("poke at {}", addr), timeouts.request, async {
        client
            .poke_response(0, wire, payload)
            .await
            .map_err(PrivateRpcError::from)
    })
    .await?;
    match response {
        PokeResponseResult::Acknowledged(true) => Ok(()),
        PokeResponseResult::Acknowledged(false) => Err(PrivateRpcError::poke_rejected(
            ErrorCode::PokeFailed as i32,
            "kernel did not acknowledge poke".to_string(),
        )
        .into()),
        PokeResponseResult::Error(error) => {
            Err(PrivateRpcError::poke_rejected(error.code, error.message).into())
        }
    }
}

async fn connect_mining_operator(addr: &str, auth_token: Option<&str>) -> Result<NodeClient> {
    run_grpc_op(
        format!("connect to private gRPC at {}", addr),
        grpc_timeouts().connect,
        async {
            NodeClient::connect_operator_with_auth_token(format!("http://{}", addr), auth_token)
                .await
                .map_err(PrivateRpcError::from)
        },
    )
    .await
}

pub(crate) async fn configure_mining_payout(
    addr: &str,
    auth_token: Option<&str>,
    pkh: &str,
) -> Result<()> {
    let mut client = connect_mining_operator(addr, auth_token).await?;
    // Preserve both reward eras; the standalone miner's v1-only update leaves
    // the existing v0 share map intact.
    run_grpc_op(
        format!("configure mining payout at {}", addr),
        grpc_timeouts().request,
        async {
            client
                .set_mining_key(
                    WireRepr::new("zk-pow-miner", 1, vec!["setpubkey".into()]),
                    vec![MiningKeyConfig {
                        share: 1,
                        m: 1,
                        keys: vec![DEFAULT_V0_MINING_KEY.to_string()],
                    }],
                    vec![MiningPkhConfig {
                        share: 1,
                        pkh: pkh.to_string(),
                    }],
                )
                .await
                .map_err(PrivateRpcError::from)
        },
    )
    .await
}

pub(crate) async fn set_mining_enabled(
    addr: &str,
    auth_token: Option<&str>,
    enabled: bool,
) -> Result<()> {
    let mut client = connect_mining_operator(addr, auth_token).await?;
    run_grpc_op(
        format!("set mining enabled at {}", addr),
        grpc_timeouts().request,
        async {
            client
                .enable_mining(
                    WireRepr::new("zk-pow-miner", 1, vec!["enable".into()]),
                    enabled,
                )
                .await
                .map_err(PrivateRpcError::from)
        },
    )
    .await
}

fn encode_heaviest_chain_path() -> Result<Vec<u8>> {
    let mut slab: NounSlab = NounSlab::new();
    let tag = make_tas(&mut slab, "heaviest-chain").as_noun();
    let path_noun = T(&mut slab, &[tag, SIG]);
    slab.set_root(path_noun);
    Ok(slab.jam().to_vec())
}

fn encode_heaviest_block_path() -> Result<Vec<u8>> {
    let mut slab: NounSlab = NounSlab::new();
    let tag = make_tas(&mut slab, "heaviest-block").as_noun();
    let path_noun = T(&mut slab, &[tag, SIG]);
    slab.set_root(path_noun);
    Ok(slab.jam().to_vec())
}

fn decode_heaviest_chain(data: Vec<u8>) -> Result<Option<(BlockHeight, Hash)>> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(data))?;
    let space = slab.noun_space();
    let opt: Option<Option<(BlockHeight, Hash)>> =
        Option::<Option<(BlockHeight, Hash)>>::from_noun(&noun, &space)
            .context("decode heaviest-chain")?;
    Ok(opt.flatten())
}

fn decode_heaviest_block(data: Vec<u8>) -> Result<Option<Page>> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(data))?;
    let space = slab.noun_space();
    let opt: Option<Option<Page>> =
        Option::<Option<Page>>::from_noun(&noun, &space).context("decode heaviest-block")?;
    Ok(opt.flatten())
}

fn encode_constants_path() -> Result<Vec<u8>> {
    let mut slab: NounSlab = NounSlab::new();
    let tag = make_tas(&mut slab, "constants").as_noun();
    let path_noun = T(&mut slab, &[tag, SIG]);
    slab.set_root(path_noun);
    Ok(slab.jam().to_vec())
}

async fn peek_path(
    client: &mut PrivateNockAppGrpcClient,
    path: Vec<u8>,
    label: &str,
    addr: &str,
    timeouts: GrpcTimeouts,
) -> Result<Vec<u8>> {
    run_grpc_op(
        format!("peek {} at {}", label, addr),
        timeouts.request,
        async { client.peek(0, path).await.map_err(PrivateRpcError::from) },
    )
    .await
}

#[derive(Clone, Copy)]
struct GrpcTimeouts {
    connect: Duration,
    request: Duration,
}

fn grpc_timeouts() -> GrpcTimeouts {
    GrpcTimeouts {
        connect: duration_from_env_ms(
            "NOCKCHAIN_E2E_GRPC_CONNECT_TIMEOUT_MS", DEFAULT_GRPC_CONNECT_TIMEOUT_MS,
        ),
        request: duration_from_env_ms(
            "NOCKCHAIN_E2E_GRPC_REQUEST_TIMEOUT_MS", DEFAULT_GRPC_REQUEST_TIMEOUT_MS,
        ),
    }
}

fn duration_from_env_ms(key: &str, default_ms: u64) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(default_ms))
}

async fn connect_block_client(
    addr: &str,
    timeouts: GrpcTimeouts,
) -> Result<NockchainBlockServiceClient<tonic::transport::Channel>> {
    run_grpc_op(
        format!("connect to gRPC at {}", addr),
        timeouts.connect,
        NockchainBlockServiceClient::connect(format!("http://{}", addr)),
    )
    .await
}

async fn connect_private_client(
    addr: &str,
    auth_token: Option<&str>,
    timeouts: GrpcTimeouts,
) -> Result<PrivateNockAppGrpcClient> {
    run_grpc_op(
        format!("connect to private gRPC at {}", addr),
        timeouts.connect,
        async {
            PrivateNockAppGrpcClient::connect_with_auth_token(
                format!("http://{}", addr),
                auth_token,
            )
            .await
            .map_err(PrivateRpcError::from)
        },
    )
    .await
}

async fn connect_service_client(
    addr: &str,
    timeouts: GrpcTimeouts,
) -> Result<NockchainServiceClient<tonic::transport::Channel>> {
    run_grpc_op(
        format!("connect to gRPC at {}", addr),
        timeouts.connect,
        NockchainServiceClient::connect(format!("http://{}", addr)),
    )
    .await
}

async fn connect_metrics_client(
    addr: &str,
    timeouts: GrpcTimeouts,
) -> Result<NockchainMetricsServiceClient<tonic::transport::Channel>> {
    run_grpc_op(
        format!("connect to metrics gRPC at {}", addr),
        timeouts.connect,
        NockchainMetricsServiceClient::connect(format!("http://{}", addr)),
    )
    .await
}

fn demo_live(info: &DemoLiveInfo) -> bool {
    info.peer_count > 0 && has_live_explorer_telemetry(info)
}

fn caught_up_to_reference_cache(info: &DemoLiveInfo, reference_cache_height: u64) -> bool {
    demo_live(info) && info.cache_height >= reference_cache_height
}

fn local_cache_gap(info: &DemoLiveInfo) -> u64 {
    info.heaviest_height.saturating_sub(info.cache_height)
}

fn has_live_explorer_telemetry(info: &DemoLiveInfo) -> bool {
    info.refresh_success_count > 0
        || info.backfill_success_count > 0
        || info.cache_height > 0
        || info.heaviest_height > 0
}

fn format_demo_live_observation(info: &DemoLiveInfo) -> String {
    format!(
        "{} live peer(s), cache {} / {}, local gap {}, refresh successes {}, backfill successes {}",
        info.peer_count,
        info.cache_height,
        info.heaviest_height,
        local_cache_gap(info),
        info.refresh_success_count,
        info.backfill_success_count
    )
}

fn format_seed_catch_up_observation(info: &DemoLiveInfo, reference_cache_height: u64) -> String {
    format!(
        "{} live peer(s), cache {} / seed target {}, local head {}, local gap {}",
        info.peer_count,
        info.cache_height,
        reference_cache_height,
        info.heaviest_height,
        local_cache_gap(info)
    )
}

async fn run_grpc_op<T, E, F>(label: String, timeout_duration: Duration, future: F) -> Result<T>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    match tokio_timeout(timeout_duration, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(anyhow::Error::new(err)).context(label),
        Err(_) => Err(PrivateRpcError::TimedOut {
            label,
            duration: timeout_duration,
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::{Duration, Instant};

    use tokio::net::TcpListener;

    use super::{
        caught_up_to_reference_cache, demo_live, has_live_explorer_telemetry,
        wait_for_ready_with_timeouts, DemoLiveInfo, GrpcTimeouts,
    };

    #[test]
    fn demo_live_requires_a_peer_and_live_metrics() {
        let mut info = DemoLiveInfo {
            peer_count: 0,
            cache_height: 0,
            heaviest_height: 0,
            refresh_success_count: 0,
            backfill_success_count: 0,
        };
        assert!(!demo_live(&info));

        info.peer_count = 1;
        assert!(!demo_live(&info));

        info.cache_height = 12;
        info.heaviest_height = 14;
        assert!(has_live_explorer_telemetry(&info));
        assert!(demo_live(&info));
    }

    #[test]
    fn local_cache_caught_up_requires_zero_gap_after_demo_is_live() {
        let mut info = DemoLiveInfo {
            peer_count: 1,
            cache_height: 12,
            heaviest_height: 14,
            refresh_success_count: 1,
            backfill_success_count: 0,
        };

        assert!(demo_live(&info));
        assert!(info.cache_height < info.heaviest_height);

        info.cache_height = 14;
        assert!(info.cache_height >= info.heaviest_height);
    }

    #[test]
    fn seed_catch_up_requires_cache_height_to_reach_reference_target() {
        let mut info = DemoLiveInfo {
            peer_count: 1,
            cache_height: 12,
            heaviest_height: 40,
            refresh_success_count: 1,
            backfill_success_count: 0,
        };

        assert!(!caught_up_to_reference_cache(&info, 20));

        info.cache_height = 20;
        assert!(caught_up_to_reference_cache(&info, 20));
    }

    #[tokio::test]
    async fn wait_for_ready_times_out_when_grpc_endpoint_stalls() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let addr = listener
            .local_addr()
            .expect("test listener should expose local addr");
        let accept_task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("test listener should accept connection");
                tokio::spawn(async move {
                    let _stream = stream;
                    pending::<()>().await;
                });
            }
        });

        let started = Instant::now();
        let err = wait_for_ready_with_timeouts(
            &addr.to_string(),
            Duration::from_millis(350),
            GrpcTimeouts {
                connect: Duration::from_millis(100),
                request: Duration::from_millis(100),
            },
        )
        .await
        .expect_err("stalled endpoint should time out");

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "wait_for_ready took too long: {:?}",
            started.elapsed()
        );
        assert!(err.to_string().contains("timed out waiting for gRPC"));

        accept_task.abort();
    }
}
