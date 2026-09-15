use std::collections::VecDeque;
use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use backon::Retryable;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, ToBytes};
use nockapp_grpc::services::private_nockapp::client::PrivateNockAppGrpcClient;
use nockchain_types::tx_engine::common::{BlockId, Page, TxId};
use nockchain_types::BlockchainConstants;
use nockvm::noun::{NounAllocator, NounSpace};
use noun_serde::prelude::*;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::core::loop_policy::NockObserverLoopPolicy;
use crate::core::observation::nock::{plan_nock_tick, NockPlanAction, NockPlanInput};
use crate::core::ports::{NockSourcePort, NockTipInfo};
use crate::observability::metrics;
use crate::observability::status::BridgeStatus;
use crate::observability::tui::types::{
    AlertSeverity, ChainState, NetworkState, NockchainApiStatus,
};
use crate::shared::errors::BridgeError;
use crate::shared::runtime::{BridgeEvent, BridgeRuntimeHandle, ChainEvent, NockBlockEvent};
use crate::shared::stop::StopHandle;
use crate::shared::types::{NockchainTxsMap, Tx};
use crate::withdrawal::snapshot::BridgeNoteSnapshotService;

const CLIENT_PID: i32 = 1;
const NOCK_GRPC_TIMEOUT_MARKER: &str = " timed out after ";
const NOCK_GRPC_RESPONSE_TOO_LARGE_MARKER: &str = "message length too large";
pub const DEFAULT_NOCK_GRPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const NOCK_BLOCK_BATCH_SIZE: u64 = 64;
const NOCK_TIP_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
pub const BLOCKCHAIN_CONSTANTS_PATH: &str = "blockchain-constants";

/// Default nockchain confirmation depth used by the driver if not specified in config.
///
/// The bridge kernel assumes blocks it receives are final; this is enforced by the Rust driver.
pub const DEFAULT_NOCKCHAIN_CONFIRMATION_DEPTH: u64 = 400;

fn tip_covers_next_confirmed_height(
    tip_info: Option<&NockTipInfo>,
    next_needed_height: Option<u64>,
    confirmation_depth: u64,
) -> bool {
    match (tip_info, next_needed_height) {
        (Some(tip), Some(next_height)) => {
            tip.height.saturating_sub(confirmation_depth) >= next_height
        }
        _ => false,
    }
}

fn tip_change_invalidates_prefetch(
    previous: Option<&NockTipInfo>,
    refreshed: Option<&NockTipInfo>,
) -> bool {
    match (previous, refreshed) {
        (Some(previous), Some(refreshed)) => {
            previous.tip_hash != refreshed.tip_hash || refreshed.height < previous.height
        }
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(test)]
fn confirmed_height(chain_tip: u64, confirmation_depth: u64) -> Option<u64> {
    let target = if confirmation_depth == 0 {
        chain_tip
    } else {
        chain_tip.saturating_sub(confirmation_depth)
    };
    if target == 0 {
        None
    } else {
        Some(target)
    }
}

pub struct NockGrpcSource {
    client: PrivateNockAppGrpcClient,
    request_timeout: Duration,
}

impl NockGrpcSource {
    pub async fn connect(endpoint: String) -> Result<Self, BridgeError> {
        Self::connect_with_timeout(endpoint, DEFAULT_NOCK_GRPC_REQUEST_TIMEOUT).await
    }

    pub async fn connect_with_timeout(
        endpoint: String,
        request_timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let client = connect_private_nockapp(endpoint, request_timeout).await?;
        Ok(Self::from_client_with_timeout(client, request_timeout))
    }

    pub fn from_client(client: PrivateNockAppGrpcClient) -> Self {
        Self::from_client_with_timeout(client, DEFAULT_NOCK_GRPC_REQUEST_TIMEOUT)
    }

    pub fn from_client_with_timeout(
        client: PrivateNockAppGrpcClient,
        request_timeout: Duration,
    ) -> Self {
        Self {
            client,
            request_timeout,
        }
    }
}

pub async fn fetch_private_blockchain_constants(
    endpoint: &str,
) -> Result<BlockchainConstants, BridgeError> {
    let mut client =
        connect_private_nockapp(endpoint.to_string(), DEFAULT_NOCK_GRPC_REQUEST_TIMEOUT).await?;
    fetch_private_blockchain_constants_from_client(&mut client).await
}

pub async fn fetch_private_blockchain_constants_from_client(
    client: &mut PrivateNockAppGrpcClient,
) -> Result<BlockchainConstants, BridgeError> {
    fetch_private_blockchain_constants_from_client_with_timeout(
        client, DEFAULT_NOCK_GRPC_REQUEST_TIMEOUT,
    )
    .await
}

async fn fetch_private_blockchain_constants_from_client_with_timeout(
    client: &mut PrivateNockAppGrpcClient,
    request_timeout: Duration,
) -> Result<BlockchainConstants, BridgeError> {
    let mut path_slab = NounSlab::<NockJammer>::new();
    let path_noun = vec![BLOCKCHAIN_CONSTANTS_PATH.to_string()].to_noun(&mut path_slab);
    path_slab.set_root(path_noun);
    let response = peek_private_nockapp(
        client,
        "blockchain-constants peek",
        path_slab.jam().to_vec(),
        request_timeout,
    )
    .await?;
    decode_blockchain_constants_response(response)
}

async fn connect_private_nockapp(
    endpoint: String,
    request_timeout: Duration,
) -> Result<PrivateNockAppGrpcClient, BridgeError> {
    with_nock_grpc_timeout(
        "private nockapp gRPC connect",
        request_timeout,
        PrivateNockAppGrpcClient::connect(endpoint),
    )
    .await
}

async fn peek_private_nockapp(
    client: &mut PrivateNockAppGrpcClient,
    operation: &str,
    path: Vec<u8>,
    request_timeout: Duration,
) -> Result<Vec<u8>, BridgeError> {
    with_nock_grpc_timeout(operation, request_timeout, client.peek(CLIENT_PID, path)).await
}

async fn with_nock_grpc_timeout<F, T, E>(
    operation: &str,
    request_timeout: Duration,
    future: F,
) -> Result<T, BridgeError>
where
    F: Future<Output = Result<T, E>>,
    E: Display,
{
    match timeout(request_timeout, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(BridgeError::EventMonitoring(err.to_string())),
        Err(_) => Err(BridgeError::EventMonitoring(format!(
            "{operation} timed out after {request_timeout:?}"
        ))),
    }
}

fn is_nock_grpc_timeout_error(err: &BridgeError) -> bool {
    matches!(
        err,
        BridgeError::EventMonitoring(message) if message.contains(NOCK_GRPC_TIMEOUT_MARKER)
    )
}

fn is_nock_grpc_response_too_large_error(err: &BridgeError) -> bool {
    matches!(
        err,
        BridgeError::EventMonitoring(message)
            if message.contains(NOCK_GRPC_RESPONSE_TOO_LARGE_MARKER)
    )
}

fn reduced_batch_end(start: u64, end: u64) -> u64 {
    start.saturating_add(end.saturating_sub(start) / 2)
}

pub async fn bootstrap_blockchain_constants(
    endpoint: &str,
) -> Result<BlockchainConstants, BridgeError> {
    fetch_private_blockchain_constants(endpoint).await
}

pub fn validate_blockchain_constants_match(
    expected: &BlockchainConstants,
    actual: &BlockchainConstants,
) -> Result<(), BridgeError> {
    if actual == expected {
        return Ok(());
    }

    Err(BridgeError::Runtime(format!(
        "connected nockchain node reported blockchain constants that differ from bridge kernel state: {}",
        format_blockchain_constants_difference(expected, actual),
    )))
}

fn format_blockchain_constants_difference(
    expected: &BlockchainConstants,
    actual: &BlockchainConstants,
) -> String {
    let mut differences = Vec::new();
    macro_rules! differing_field {
        ($($field:ident).+) => {
            if expected.$($field).+ != actual.$($field).+ {
                differences.push(format!(
                    "{} expected={:?} actual={:?}",
                    stringify!($($field).+),
                    expected.$($field).+,
                    actual.$($field).+,
                ));
            }
        };
    }

    differing_field!(max_block_size);
    differing_field!(blocks_per_epoch);
    differing_field!(target_epoch_duration);
    differing_field!(update_candidate_timestamp_interval);
    differing_field!(max_future_timestamp);
    differing_field!(min_past_blocks);
    differing_field!(genesis_target_atom);
    differing_field!(max_target_atom);
    differing_field!(coinbase_timelock_min);
    differing_field!(pow_len);
    differing_field!(max_coinbase_split);
    differing_field!(first_month_coinbase_min);
    differing_field!(v1_phase);
    differing_field!(bythos_phase);
    differing_field!(note_data);
    differing_field!(base_fee);
    differing_field!(input_fee_divisor);
    differing_field!(zk_asert.phase);
    differing_field!(zk_asert.anchor_height);
    differing_field!(zk_asert.anchor_target_atom);
    differing_field!(zk_asert.ideal_block_time);
    differing_field!(zk_asert.half_life);
    differing_field!(zk_asert.anchor_min_timestamp);
    differing_field!(zk_asert_post_ai.phase);
    differing_field!(zk_asert_post_ai.anchor_height);
    differing_field!(zk_asert_post_ai.anchor_target_atom);
    differing_field!(zk_asert_post_ai.ideal_block_time);
    differing_field!(zk_asert_post_ai.half_life);
    differing_field!(zk_asert_post_ai.anchor_min_timestamp);
    differing_field!(ai_pow_activation_height);
    differing_field!(ai_asert.phase);
    differing_field!(ai_asert.anchor_height);
    differing_field!(ai_asert.anchor_target_atom);
    differing_field!(ai_asert.ideal_block_time);
    differing_field!(ai_asert.half_life);
    differing_field!(ai_asert.anchor_min_timestamp);

    differences.join(", ")
}

fn decode_blockchain_constants_response(
    bytes: Vec<u8>,
) -> Result<BlockchainConstants, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(bytes)).map_err(|err| {
        BridgeError::Runtime(format!(
            "failed to cue blockchain-constants response: {err}"
        ))
    })?;
    let space = slab.noun_space();
    let payload =
        Option::<Option<BlockchainConstants>>::from_noun(&noun, &space).map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to decode blockchain-constants response: {err}"
            ))
        })?;
    payload.flatten().ok_or_else(|| {
        BridgeError::Runtime("nockchain node returned no blockchain-constants payload".into())
    })
}

fn nock_block_still_waiting_for_kernel(
    in_flight_height: Option<u64>,
    next_needed_height: Option<u64>,
) -> bool {
    in_flight_height.is_some() && in_flight_height == next_needed_height
}

fn retain_hydratable_prefix(blocks: &mut Vec<NockBlockEvent>) -> Option<&mut NockBlockEvent> {
    let index = blocks
        .iter()
        .position(|block| !block.block.tx_ids.is_empty())?;
    if index == 0 {
        blocks.truncate(1);
        blocks.first_mut()
    } else {
        blocks.truncate(index);
        None
    }
}

fn buffered_transaction_requires_tip_refresh(
    blocks: &VecDeque<NockBlockEvent>,
    next_needed_height: Option<u64>,
) -> bool {
    blocks.front().is_some_and(|block| {
        Some(block.block.height) == next_needed_height && !block.block.tx_ids.is_empty()
    })
}

#[async_trait]
impl NockSourcePort for NockGrpcSource {
    async fn tip_info(&mut self) -> Result<Option<NockTipInfo>, BridgeError> {
        let info = Self::fetch_tip_info_from_client(&mut self.client, self.request_timeout).await?;
        Ok(info.map(|(height, tip_hash)| NockTipInfo { height, tip_hash }))
    }

    async fn fetch_block_at_height(
        &mut self,
        height: u64,
    ) -> Result<Option<NockBlockEvent>, BridgeError> {
        Ok(Self::fetch_blocks_in_range_from_client(
            &mut self.client, height, height, self.request_timeout,
        )
        .await?
        .into_iter()
        .next())
    }

    async fn fetch_blocks_in_range(
        &mut self,
        start: u64,
        end: u64,
    ) -> Result<Vec<NockBlockEvent>, BridgeError> {
        Self::fetch_blocks_in_range_from_client(&mut self.client, start, end, self.request_timeout)
            .await
    }
}

impl NockGrpcSource {
    async fn fetch_tip_info_from_client(
        client: &mut PrivateNockAppGrpcClient,
        request_timeout: Duration,
    ) -> Result<Option<(u64, String)>, BridgeError> {
        let path = jam_path(&[Bytes::from("heaviest-chain")])?;
        let response =
            peek_private_nockapp(client, "heaviest chain peek", path, request_timeout).await?;
        let (response_slab, response_noun) = cue_response(response)?;
        let response_space = response_slab.noun_space();
        let Some(inner) =
            decode_unit_payload(response_noun, &response_space, "heaviest chain outer")?
        else {
            return Ok(None);
        };
        let Some(tip) = decode_unit_payload(inner, &response_space, "heaviest chain inner")? else {
            return Ok(None);
        };
        let tip_cell = tip.in_space(&response_space).as_cell().map_err(|_| {
            BridgeError::EventMonitoring("heaviest chain tip expected a cell".into())
        })?;
        let height = tip_cell
            .head()
            .as_atom()
            .map_err(|_| {
                BridgeError::EventMonitoring("heaviest chain height expected an atom".into())
            })?
            .as_u64()
            .map_err(|_| {
                BridgeError::EventMonitoring("heaviest chain height is too large".into())
            })?;
        let block_id =
            BlockId::from_noun(&tip_cell.tail().noun(), &response_space).map_err(|err| {
                BridgeError::EventMonitoring(format!(
                    "failed to decode heaviest chain block id: {err}"
                ))
            })?;
        Ok(Some((height, block_id.to_base58())))
    }

    async fn fetch_blocks_in_range_from_client(
        client: &mut PrivateNockAppGrpcClient,
        start: u64,
        end: u64,
        request_timeout: Duration,
    ) -> Result<Vec<NockBlockEvent>, BridgeError> {
        if start > end {
            return Ok(Vec::new());
        }

        let mut range_end = end;
        let mut blocks = loop {
            let range_path = vec![
                Bytes::from("heaviest-chain-blocks-range"),
                Bytes::from(start.to_bytes()?),
                Bytes::from(range_end.to_bytes()?),
            ];
            let range_bytes = jam_path(&range_path)?;
            match peek_private_nockapp(client, "block range peek", range_bytes, request_timeout)
                .await
            {
                Ok(response) => break decode_block_range_response(response)?,
                Err(err) if is_nock_grpc_response_too_large_error(&err) && range_end > start => {
                    let requested_end = range_end;
                    range_end = reduced_batch_end(start, range_end);
                    warn!(
                        target: "bridge.nock-watcher",
                        start,
                        requested_end,
                        reduced_end = range_end,
                        "nock block range response exceeded gRPC limit; retrying a smaller range"
                    );
                }
                Err(err) if is_nock_grpc_response_too_large_error(&err) => {
                    warn!(
                        target: "bridge.nock-watcher",
                        height = start,
                        "single-block range response exceeded gRPC limit; falling back to individual peeks"
                    );
                    return Ok(Self::fetch_single_block_from_client(
                        client, start, request_timeout,
                    )
                    .await?
                    .into_iter()
                    .collect());
                }
                Err(err) => return Err(err),
            }
        };

        if let Some(block) = retain_hydratable_prefix(&mut blocks) {
            Self::hydrate_block_transactions_from_client(client, block, request_timeout).await?;
        }
        Ok(blocks)
    }

    async fn fetch_single_block_from_client(
        client: &mut PrivateNockAppGrpcClient,
        height: u64,
        request_timeout: Duration,
    ) -> Result<Option<NockBlockEvent>, BridgeError> {
        let path = vec![Bytes::from("heavy-n"), Bytes::from(height.to_bytes()?)];
        let response = peek_private_nockapp(
            client,
            "height block peek",
            jam_path(&path)?,
            request_timeout,
        )
        .await?;
        let Some(mut block) = decode_block_page_response(response)? else {
            return Ok(None);
        };
        if block.block.height != height {
            return Err(BridgeError::EventMonitoring(format!(
                "height block peek returned height {}, expected {height}",
                block.block.height
            )));
        }
        Self::hydrate_block_transactions_from_client(client, &mut block, request_timeout).await?;
        Ok(Some(block))
    }

    async fn hydrate_block_transactions_from_client(
        client: &mut PrivateNockAppGrpcClient,
        block: &mut NockBlockEvent,
        request_timeout: Duration,
    ) -> Result<(), BridgeError> {
        if block.block.tx_ids.is_empty() {
            return Ok(());
        }
        let txs = Self::fetch_block_transactions_from_client(
            client, &block.block.digest, &block.block.tx_ids, request_timeout,
        )
        .await?;
        let complete = txs.len() == block.block.tx_ids.len()
            && block
                .block
                .tx_ids
                .iter()
                .all(|expected| txs.iter().any(|(actual, _)| actual == expected));
        if !complete {
            return Err(BridgeError::EventMonitoring(format!(
                "block transaction map does not match page at height {}: expected {} transactions, got {}",
                block.block.height,
                block.block.tx_ids.len(),
                txs.len()
            )));
        }
        block.txs = txs;
        Ok(())
    }

    async fn fetch_block_transactions_from_client(
        client: &mut PrivateNockAppGrpcClient,
        block_id: &BlockId,
        tx_ids: &[TxId],
        request_timeout: Duration,
    ) -> Result<Vec<(TxId, Tx)>, BridgeError> {
        let path = vec![Bytes::from("block-transactions"), Bytes::from(block_id.to_base58())];
        match peek_private_nockapp(
            client,
            "block transactions peek",
            jam_path(&path)?,
            request_timeout,
        )
        .await
        {
            Ok(response) => decode_block_transactions_response(response),
            Err(err) if is_nock_grpc_response_too_large_error(&err) => {
                warn!(
                    target: "bridge.nock-watcher",
                    block_id = %block_id.to_base58(),
                    txs_count = tx_ids.len(),
                    "block transaction map exceeded gRPC limit; fetching transactions individually"
                );
                Self::fetch_transactions_individually_from_client(
                    client, block_id, tx_ids, request_timeout,
                )
                .await
            }
            Err(err) => Err(err),
        }
    }

    async fn fetch_transactions_individually_from_client(
        client: &mut PrivateNockAppGrpcClient,
        block_id: &BlockId,
        tx_ids: &[TxId],
        request_timeout: Duration,
    ) -> Result<Vec<(TxId, Tx)>, BridgeError> {
        let block_id = block_id.to_base58();
        let mut txs = Vec::with_capacity(tx_ids.len());
        for tx_id in tx_ids {
            let path = vec![
                Bytes::from("block-transaction"),
                Bytes::from(block_id.clone()),
                Bytes::from(tx_id.to_base58()),
            ];
            let response = peek_private_nockapp(
                client,
                "block transaction peek",
                jam_path(&path)?,
                request_timeout,
            )
            .await?;
            txs.push((tx_id.clone(), decode_block_transaction_response(response)?));
        }
        Ok(txs)
    }
}

struct NockWatcherConnection {
    endpoint: String,
    policy: NockObserverLoopPolicy,
}

struct NockWatcherDeps {
    runtime: Arc<BridgeRuntimeHandle>,
    stop: StopHandle,
    confirmed_snapshot: Option<Arc<BridgeNoteSnapshotService>>,
}

struct NockWatcherConfig {
    confirmation_depth: u64,
}

struct NockWatcherUi {
    /// Optional bridge_status for connection status + alert updates.
    bridge_status: Option<BridgeStatus>,
}

pub struct NockchainWatcher {
    connection: NockWatcherConnection,
    deps: NockWatcherDeps,
    config: NockWatcherConfig,
    ui: NockWatcherUi,
}

impl NockchainWatcher {
    pub fn new(
        endpoint: String,
        runtime: Arc<BridgeRuntimeHandle>,
        confirmation_depth: u64,
        stop: StopHandle,
    ) -> Self {
        Self::with_policy(
            endpoint,
            runtime,
            confirmation_depth,
            stop,
            NockObserverLoopPolicy::default(),
        )
    }

    pub fn with_poll_interval(
        endpoint: String,
        runtime: Arc<BridgeRuntimeHandle>,
        poll_interval: Duration,
        confirmation_depth: u64,
        stop: StopHandle,
    ) -> Self {
        let policy = NockObserverLoopPolicy {
            poll_interval,
            ..NockObserverLoopPolicy::default()
        };
        Self::with_policy(endpoint, runtime, confirmation_depth, stop, policy)
    }

    pub fn with_policy(
        endpoint: String,
        runtime: Arc<BridgeRuntimeHandle>,
        confirmation_depth: u64,
        stop: StopHandle,
        policy: NockObserverLoopPolicy,
    ) -> Self {
        Self {
            connection: NockWatcherConnection { endpoint, policy },
            deps: NockWatcherDeps {
                runtime,
                stop,
                confirmed_snapshot: None,
            },
            config: NockWatcherConfig { confirmation_depth },
            ui: NockWatcherUi {
                bridge_status: None,
            },
        }
    }

    /// Set the TUI state for connection status updates.
    pub fn with_bridge_status(mut self, bridge_status: BridgeStatus) -> Self {
        self.ui.bridge_status = Some(bridge_status);
        self
    }

    pub fn with_confirmed_snapshot_service(
        mut self,
        snapshot_service: Arc<BridgeNoteSnapshotService>,
    ) -> Self {
        self.deps.confirmed_snapshot = Some(snapshot_service);
        self
    }

    /// Update the nockchain API connection status in the TUI.
    fn update_status(&self, status: NockchainApiStatus) {
        if let Some(ref bridge_status) = self.ui.bridge_status {
            bridge_status.update_nockchain_api_status(status);
        }
    }

    /// Update the nockchain tip hash in the TUI.
    fn update_tip_hash(&self, tip_hash: String) {
        if let Some(ref bridge_status) = self.ui.bridge_status {
            bridge_status.update_nockchain_tip_hash(tip_hash);
        }
    }

    /// Push an alert to the TUI.
    fn push_alert(&self, severity: AlertSeverity, title: String, message: String) {
        if let Some(ref bridge_status) = self.ui.bridge_status {
            bridge_status.push_alert(severity, title, message, "nock-watcher".to_string());
        }
    }

    pub async fn run(self) -> Result<(), BridgeError> {
        let mut was_connected = false;

        // Unlimited retries with exponential backoff by default.
        let connect_backoff = self.connection.policy.connect_retry;

        self.update_status(NockchainApiStatus::connecting(0, None));

        loop {
            if self.deps.stop.is_stopped() {
                sleep(self.connection.policy.poll_interval).await;
                continue;
            }

            let attempt_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let attempt_count_notify = attempt_count.clone();
            let endpoint = self.connection.endpoint.clone();
            let request_timeout = self.connection.policy.request_timeout;
            let connect = || {
                let endpoint = endpoint.clone();
                async move { connect_private_nockapp(endpoint, request_timeout).await }
            };

            self.update_status(NockchainApiStatus::connecting(1, None));

            let connect_result = connect
                .retry(connect_backoff.exponential_builder())
                .notify(|err, dur| {
                    let attempt =
                        attempt_count_notify.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let error_msg = err.to_string();

                    self.update_status(NockchainApiStatus::connecting(
                        attempt + 1,
                        Some(error_msg.clone()),
                    ));

                    warn!(
                        target: "bridge.nock-watcher",
                        endpoint=%self.connection.endpoint,
                        error=%error_msg,
                        attempt=attempt,
                        backoff_secs=dur.as_secs(),
                        "failed to connect, will retry"
                    );

                    if attempt == 1 || attempt.is_multiple_of(10) {
                        self.push_alert(
                            AlertSeverity::Warning,
                            "Nockchain API Connection Failed".to_string(),
                            format!(
                                "Attempt {}: {}",
                                attempt,
                                truncate_error_msg(&error_msg, 50)
                            ),
                        );
                    }
                })
                .await;

            match connect_result {
                Ok(client) => {
                    self.update_status(NockchainApiStatus::connected());

                    if was_connected {
                        info!(
                            target: "bridge.nock-watcher",
                            endpoint=%self.connection.endpoint,
                            "reconnected to nockchain gRPC endpoint"
                        );
                        self.push_alert(
                            AlertSeverity::Info,
                            "Nockchain API Reconnected".to_string(),
                            format!("Reconnected to {}", self.connection.endpoint),
                        );
                    } else {
                        info!(
                            target: "bridge.nock-watcher",
                            endpoint=%self.connection.endpoint,
                            "connected to nockchain gRPC endpoint"
                        );
                    }
                    was_connected = true;

                    let mut source = NockGrpcSource::from_client_with_timeout(
                        client, self.connection.policy.request_timeout,
                    );
                    if let Err(err) = self.stream_events_with_source(&mut source).await {
                        let error_msg = err.to_string();
                        warn!(
                            target: "bridge.nock-watcher",
                            error=%error_msg,
                            "nockchain watcher stream failed, reconnecting"
                        );
                        self.update_status(NockchainApiStatus::disconnected(error_msg.clone()));
                        self.push_alert(
                            AlertSeverity::Warning,
                            "Nockchain API Disconnected".to_string(),
                            format!("Connection lost: {}", truncate_error_msg(&error_msg, 60)),
                        );
                    }
                }
                Err(err) => {
                    // Should not happen with unlimited retries
                    let error_msg = err.to_string();
                    warn!(
                        target: "bridge.nock-watcher",
                        endpoint=%self.connection.endpoint,
                        error=%error_msg,
                        "connect failed unexpectedly"
                    );
                    self.update_status(NockchainApiStatus::disconnected(error_msg));
                    sleep(self.connection.policy.connect_failure_sleep).await;
                }
            }
        }
    }

    async fn stream_events_with_source<S>(&self, source: &mut S) -> Result<(), BridgeError>
    where
        S: NockSourcePort,
    {
        let poll_interval = self.connection.policy.poll_interval;
        info!(
            target: "bridge.nock-watcher",
            confirmation_depth = self.config.confirmation_depth,
            "starting nock observer with confirmation depth"
        );
        let mut nock_block_in_flight: Option<u64> = None;
        let mut cached_tip_info: Option<NockTipInfo> = None;
        let mut tip_refreshed_at: Option<Instant> = None;
        let mut prefetched_blocks: VecDeque<NockBlockEvent> = VecDeque::new();
        loop {
            if self.deps.stop.is_stopped() {
                sleep(poll_interval).await;
                continue;
            }

            let next_needed_height = match self.deps.runtime.peek_nock_next_height().await {
                Ok(height) => height,
                Err(err) => {
                    warn!(
                        target: "bridge.nock-watcher",
                        error=%err,
                        "failed to peek nock next height"
                    );
                    sleep(poll_interval).await;
                    continue;
                }
            };

            if nock_block_still_waiting_for_kernel(nock_block_in_flight, next_needed_height) {
                let height = nock_block_in_flight.unwrap_or_default();
                debug!(
                    target: "bridge.nock-watcher",
                    height,
                    "nock block already enqueued, waiting for kernel height advance"
                );
                sleep(poll_interval).await;
                continue;
            }
            nock_block_in_flight = None;

            let cached_tip_covers_next = tip_covers_next_confirmed_height(
                cached_tip_info.as_ref(),
                next_needed_height,
                self.config.confirmation_depth,
            );
            let tip_refresh_due = prefetched_blocks.is_empty()
                || buffered_transaction_requires_tip_refresh(
                    &prefetched_blocks, next_needed_height,
                )
                || tip_refreshed_at
                    .map(|refreshed_at| refreshed_at.elapsed() >= NOCK_TIP_REFRESH_INTERVAL)
                    .unwrap_or(true);
            if !cached_tip_covers_next || tip_refresh_due {
                let refreshed_tip = match source.tip_info().await {
                    Ok(info) => info,
                    Err(err) => {
                        if is_nock_grpc_timeout_error(&err) {
                            warn!(
                                target: "bridge.nock-watcher",
                                error=%err,
                                "failed to fetch tip height after nock gRPC timeout; reconnecting nock gRPC source"
                            );
                            return Err(err);
                        }
                        warn!(
                            target: "bridge.nock-watcher",
                            error=%err,
                            "failed to fetch tip height"
                        );
                        sleep(poll_interval).await;
                        continue;
                    }
                };
                if tip_change_invalidates_prefetch(cached_tip_info.as_ref(), refreshed_tip.as_ref())
                {
                    debug!(
                        target: "bridge.nock-watcher",
                        previous_height = cached_tip_info.as_ref().map(|tip| tip.height),
                        previous_hash = cached_tip_info.as_ref().map(|tip| tip.tip_hash.as_str()),
                        refreshed_height = refreshed_tip.as_ref().map(|tip| tip.height),
                        refreshed_hash = refreshed_tip.as_ref().map(|tip| tip.tip_hash.as_str()),
                        "nock tip changed; discarding prefetched blocks"
                    );
                    prefetched_blocks.clear();
                }
                cached_tip_info = refreshed_tip;
                tip_refreshed_at = Some(Instant::now());
                if let Some(info) = &cached_tip_info {
                    self.update_tip_hash(info.tip_hash.clone());
                }
            }

            let action = plan_nock_tick(NockPlanInput {
                tip_height: cached_tip_info.as_ref().map(|info| info.height),
                next_needed_height,
                confirmation_depth: self.config.confirmation_depth,
            });

            let (tip_height, confirmed_target, target_height) = match action {
                NockPlanAction::NoTipAvailable => {
                    debug!(
                        target: "bridge.nock-watcher",
                        "no heaviest block available from private nockapp"
                    );
                    sleep(poll_interval).await;
                    continue;
                }
                NockPlanAction::NoPendingHeight {
                    tip_height,
                    confirmed_target,
                } => {
                    nock_block_in_flight = None;
                    prefetched_blocks.clear();
                    debug!(
                        target: "bridge.nock-watcher",
                        tip_height,
                        confirmed_target,
                        "kernel has no pending nock block"
                    );
                    sleep(poll_interval).await;
                    continue;
                }
                NockPlanAction::BootstrapUnconfirmed {
                    tip_height,
                    confirmation_depth,
                } => {
                    debug!(
                        target: "bridge.nock-watcher",
                        tip_height,
                        confirmation_depth,
                        "no confirmed block yet (bootstrap)"
                    );
                    sleep(poll_interval).await;
                    continue;
                }
                NockPlanAction::NotYetConfirmed {
                    tip_height,
                    confirmed_target,
                    next_needed_height,
                } => {
                    prefetched_blocks.clear();
                    debug!(
                        target: "bridge.nock-watcher",
                        tip_height,
                        confirmed_target,
                        next_needed_height,
                        "target height not yet confirmed for kernel need"
                    );
                    sleep(poll_interval).await;
                    continue;
                }
                NockPlanAction::FetchHeight {
                    tip_height,
                    confirmed_target,
                    height,
                } => (tip_height, confirmed_target, height),
            };

            let block_result = if prefetched_blocks
                .front()
                .is_some_and(|event| event.block.height == target_height)
            {
                Ok(prefetched_blocks.pop_front())
            } else {
                prefetched_blocks.clear();
                let batch_end = target_height
                    .saturating_add(NOCK_BLOCK_BATCH_SIZE - 1)
                    .min(confirmed_target);
                match source.fetch_blocks_in_range(target_height, batch_end).await {
                    Ok(blocks) => {
                        let first_unexpected = blocks.iter().enumerate().find(|(index, event)| {
                            event.block.height != target_height.saturating_add(*index as u64)
                        });
                        if let Some((index, event)) = first_unexpected {
                            Err(BridgeError::EventMonitoring(format!(
                                "block range is not contiguous at index {index}: expected height {}, got {}",
                                target_height.saturating_add(index as u64),
                                event.block.height
                            )))
                        } else {
                            prefetched_blocks.extend(blocks);
                            Ok(prefetched_blocks.pop_front())
                        }
                    }
                    Err(err) => Err(err),
                }
            };

            match block_result {
                Ok(Some(event)) => {
                    let height = event.block.height;
                    let block_hash = event.block.digest.to_base58();
                    let confirmed_block_id = event.block.digest.clone();
                    let txs_count = event.txs.len();
                    self.deps
                        .runtime
                        .send_event(BridgeEvent::Chain(Box::new(ChainEvent::Nock(event))))
                        .await?;
                    nock_block_in_flight = Some(height);
                    if let Some(snapshot_service) = &self.deps.confirmed_snapshot {
                        if txs_count == 0 {
                            snapshot_service.refresh_in_background();
                        } else if let Err(err) = snapshot_service
                            .refresh_on_confirmed_block(height, &confirmed_block_id)
                            .await
                        {
                            warn!(
                                target: "bridge.nock-watcher",
                                height,
                                block_id = %block_hash,
                                error = %err,
                                "failed to refresh confirmed bridge note snapshot"
                            );
                        }
                    }
                    info!(
                        target: "bridge.nock-watcher",
                        height,
                        tip_height,
                        confirmations = tip_height.saturating_sub(height),
                        hash=%block_hash,
                        txs_count=%txs_count,
                        "emitted confirmed nock block"
                    );
                    continue;
                }
                Ok(None) => {
                    debug!(
                        target: "bridge.nock-watcher",
                        target = target_height,
                        "block at target height not found"
                    );
                }
                Err(err) => {
                    if is_nock_grpc_timeout_error(&err) {
                        warn!(
                            target: "bridge.nock-watcher",
                            target = target_height,
                            error=%err,
                            "failed to fetch block range after nock gRPC timeout; reconnecting nock gRPC source"
                        );
                        return Err(err);
                    }
                    warn!(
                        target: "bridge.nock-watcher",
                        target = target_height,
                        error=%err,
                        "failed to fetch block range"
                    );
                }
            }
            sleep(poll_interval).await;
        }
    }
}

/// Poll chain heights and kernel state, update TUI NetworkState.
pub async fn run_network_monitor(
    runtime: Arc<BridgeRuntimeHandle>,
    bridge_status: BridgeStatus,
    poll_interval: Duration,
) -> Result<(), BridgeError> {
    let mut interval = tokio::time::interval(poll_interval);

    // Fetch fakenet status once; retry until available.
    // The peek returns true for fakenet, false for mainnet.
    let mut is_fakenet: Option<bool> = None;

    loop {
        interval.tick().await;

        // Peek kernel state counts (includes hold status)
        // This method never fails - returns defaults on error
        let current_bridge_state = runtime.update_bridge_state().await;
        let base_height = current_bridge_state
            .base_next_height
            .map(|height| height.saturating_sub(1));
        let nock_height = current_bridge_state
            .nock_next_height
            .map(|height| height.saturating_sub(1));

        if is_fakenet.is_none() {
            match current_bridge_state.is_fakenet {
                Some(status) => {
                    info!(
                        target: "bridge.network-monitor",
                        is_fakenet = status,
                        "detected network mode: {}",
                        if status { "fakenet" } else { "mainnet" }
                    );
                    is_fakenet = Some(status);
                }
                None => {
                    warn!(
                        target: "bridge.network-monitor",
                        "failed to peek network mode, will retry"
                    );
                }
            }
        }

        let now = SystemTime::now();
        let state = bridge_status.network();
        let base_tip_hash = current_bridge_state
            .base_tip_hash
            .clone()
            .unwrap_or_else(|| state.base.tip_hash.clone());
        let mut network_state = NetworkState {
            nockchain_api_status: state.nockchain_api_status.clone(),
            ..Default::default()
        };
        network_state.base.tip_hash = base_tip_hash.clone();
        network_state.nockchain.tip_hash = state.nockchain.tip_hash.clone();
        network_state.base_next_height = current_bridge_state.base_next_height;
        network_state.nock_next_height = current_bridge_state.nock_next_height;

        if let Some(height) = base_height {
            network_state.base = ChainState {
                height,
                tip_hash: base_tip_hash.clone(),
                confirmations: 0,
                is_syncing: false,
                last_updated: Some(now),
            };
            debug!(
                target: "bridge.network-monitor",
                base_height = height,
                "updated base chain height"
            );
        }

        if let Some(height) = nock_height {
            let tip_hash = state.nockchain.tip_hash.clone();

            network_state.nockchain = ChainState {
                height,
                tip_hash,
                confirmations: 0,
                is_syncing: false,
                last_updated: Some(now),
            };
            debug!(
                target: "bridge.network-monitor",
                nock_height = height,
                "updated nockchain height"
            );
        }

        // Populate kernel state counts
        network_state.unsettled_deposit_count = current_bridge_state.unsettled_deposits;
        network_state.unsettled_withdrawal_count = current_bridge_state.unsettled_withdrawals;

        // Pending deposits come from kernel state counts (independent of TUI focus).
        network_state.pending_deposits = current_bridge_state.unsettled_deposits;
        network_state.pending_withdrawals = current_bridge_state.unsettled_withdrawals;

        // Populate hold status
        network_state.base_hold = current_bridge_state.base_hold;
        network_state.nock_hold = current_bridge_state.nock_hold;
        network_state.kernel_stopped = current_bridge_state.kernel_stopped;
        network_state.base_hold_height = if current_bridge_state.base_hold {
            current_bridge_state.base_hold_height
        } else {
            None
        };
        network_state.nock_hold_height = if current_bridge_state.nock_hold {
            current_bridge_state.nock_hold_height
        } else {
            None
        };

        // Populate network mode (mainnet vs fakenet)
        // is_fakenet is true for fakenet, so invert for is_mainnet
        network_state.is_mainnet = is_fakenet.map(|f| !f);

        debug!(
            target: "bridge.network-monitor",
            unsettled_deposits = current_bridge_state.unsettled_deposits,
            unsettled_withdrawals = current_bridge_state.unsettled_withdrawals,
            base_hold = current_bridge_state.base_hold,
            nock_hold = current_bridge_state.nock_hold,
            kernel_stopped = current_bridge_state.kernel_stopped,
            "updated kernel state counts"
        );

        metrics::update_bridge_metrics(&network_state, bridge_status.last_deposit_nonce());
        bridge_status.update_network(network_state);
    }
}

/// Truncate an error message for display in alerts.
fn truncate_error_msg(error: &str, max_len: usize) -> String {
    if error.len() <= max_len {
        error.to_string()
    } else {
        format!("{}...", &error[..max_len])
    }
}

fn jam_path(path: &[Bytes]) -> Result<Vec<u8>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let mut list = nockvm::noun::D(0);
    for segment in path.iter().rev() {
        let atom = unsafe {
            let mut ia = nockvm::noun::IndirectAtom::new_raw_bytes(
                &mut slab,
                segment.len(),
                segment.as_ptr(),
            );
            let space = slab.noun_space();
            ia.normalize_as_atom(&space).as_noun()
        };
        list = nockvm::noun::T(&mut slab, &[atom, list]);
    }
    slab.set_root(list);
    Ok(slab.jam().to_vec())
}

fn cue_response(bytes: Vec<u8>) -> Result<(NounSlab<NockJammer>, nockapp::Noun), BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(bytes))
        .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
    Ok((slab, noun))
}

fn decode_unit_payload(
    noun: nockapp::Noun,
    space: &NounSpace,
    context: &str,
) -> Result<Option<nockapp::Noun>, BridgeError> {
    if let Ok(atom) = noun.in_space(space).as_atom() {
        let value = atom.as_u64().map_err(|_| {
            BridgeError::EventMonitoring(format!("{context} unit tag is too large"))
        })?;
        return if value == 0 {
            Ok(None)
        } else {
            Err(BridgeError::EventMonitoring(format!(
                "{context} has invalid unit atom {value}"
            )))
        };
    }

    let cell = noun.in_space(space).as_cell().map_err(|_| {
        BridgeError::EventMonitoring(format!("{context} unit expected to be a cell"))
    })?;
    let tag = cell
        .head()
        .as_atom()
        .map_err(|_| {
            BridgeError::EventMonitoring(format!("{context} unit tag expected to be an atom"))
        })?
        .as_u64()
        .map_err(|_| BridgeError::EventMonitoring(format!("{context} unit tag is too large")))?;
    if tag != 0 {
        return Err(BridgeError::EventMonitoring(format!(
            "{context} has invalid unit tag {tag}"
        )));
    }
    Ok(Some(cell.tail().noun()))
}

fn decode_block_page_response(bytes: Vec<u8>) -> Result<Option<NockBlockEvent>, BridgeError> {
    let (response_slab, response_noun) = cue_response(bytes)?;
    let response_space = response_slab.noun_space();
    let Some(inner) = decode_unit_payload(response_noun, &response_space, "block page outer")?
    else {
        return Ok(None);
    };
    let Some(source_page_noun) = decode_unit_payload(inner, &response_space, "block page inner")?
    else {
        return Ok(None);
    };
    let page = Page::from_noun(&source_page_noun, &response_space).map_err(|err| {
        BridgeError::EventMonitoring(format!("failed to decode block page: {err}"))
    })?;
    let mut page_slab = NounSlab::<NockJammer>::new();
    let page_noun = page_slab.copy_into(source_page_noun, &response_space);
    page_slab.set_root(page_noun);
    Ok(Some(NockBlockEvent {
        block: page,
        page_slab,
        page_noun,
        txs: Vec::new(),
    }))
}

fn decode_block_transaction_response(bytes: Vec<u8>) -> Result<Tx, BridgeError> {
    let (response_slab, response_noun) = cue_response(bytes)?;
    let response_space = response_slab.noun_space();
    let Some(inner) =
        decode_unit_payload(response_noun, &response_space, "block transaction outer")?
    else {
        return Err(BridgeError::EventMonitoring(
            "block transaction response has no outer payload".into(),
        ));
    };
    let Some(tx_noun) = decode_unit_payload(inner, &response_space, "block transaction inner")?
    else {
        return Err(BridgeError::EventMonitoring(
            "block transaction response has no transaction".into(),
        ));
    };
    Tx::from_noun(&tx_noun, &response_space).map_err(|err| {
        BridgeError::EventMonitoring(format!("failed to decode block transaction: {err}"))
    })
}

fn decode_block_transactions_response(bytes: Vec<u8>) -> Result<Vec<(TxId, Tx)>, BridgeError> {
    let (response_slab, response_noun) = cue_response(bytes)?;
    let response_space = response_slab.noun_space();
    let Some(inner) =
        decode_unit_payload(response_noun, &response_space, "block transactions outer")?
    else {
        return Err(BridgeError::EventMonitoring(
            "block transactions response has no outer payload".into(),
        ));
    };
    let Some(txs_noun) = decode_unit_payload(inner, &response_space, "block transactions inner")?
    else {
        return Err(BridgeError::EventMonitoring(
            "block transactions response has no transaction map".into(),
        ));
    };
    NockchainTxsMap::from_noun(&txs_noun, &response_space)
        .map(|txs| txs.0)
        .map_err(|err| {
            BridgeError::EventMonitoring(format!(
                "failed to decode block transactions response: {err}"
            ))
        })
}

fn decode_block_range_response(bytes: Vec<u8>) -> Result<Vec<NockBlockEvent>, BridgeError> {
    let (response_slab, response_noun) = cue_response(bytes)?;
    let response_space = response_slab.noun_space();
    let Some(inner) = decode_unit_payload(response_noun, &response_space, "block range outer")?
    else {
        return Ok(Vec::new());
    };
    let Some(mut list) = decode_unit_payload(inner, &response_space, "block range inner")? else {
        return Ok(Vec::new());
    };

    let mut blocks = Vec::new();
    loop {
        if let Ok(atom) = list.in_space(&response_space).as_atom() {
            let value = atom.as_u64().map_err(|_| {
                BridgeError::EventMonitoring("block range list terminator is too large".into())
            })?;
            if value == 0 {
                break;
            }
            return Err(BridgeError::EventMonitoring(format!(
                "block range list has invalid terminator {value}"
            )));
        }

        let list_cell = list
            .in_space(&response_space)
            .as_cell()
            .map_err(|_| BridgeError::EventMonitoring("block range expected a list".into()))?;
        let entry = list_cell.head().as_cell().map_err(|_| {
            BridgeError::EventMonitoring("block range entry expected a cell".into())
        })?;
        let height = entry
            .head()
            .as_atom()
            .map_err(|_| {
                BridgeError::EventMonitoring("block range height expected an atom".into())
            })?
            .as_u64()
            .map_err(|_| BridgeError::EventMonitoring("block range height is too large".into()))?;
        let after_height = entry.tail().as_cell().map_err(|_| {
            BridgeError::EventMonitoring("block range entry missing block id".into())
        })?;
        let block_id =
            BlockId::from_noun(&after_height.head().noun(), &response_space).map_err(|err| {
                BridgeError::EventMonitoring(format!(
                    "failed to decode block range block id: {err}"
                ))
            })?;
        let page_and_txs = after_height.tail().as_cell().map_err(|_| {
            BridgeError::EventMonitoring("block range entry missing page or transactions".into())
        })?;
        let source_page_noun = page_and_txs.head().noun();
        let page = Page::from_noun(&source_page_noun, &response_space).map_err(|err| {
            BridgeError::EventMonitoring(format!("failed to decode block range page: {err}"))
        })?;
        if page.height != height || page.digest != block_id {
            return Err(BridgeError::EventMonitoring(format!(
                "block range entry metadata mismatch: range_height={height} page_height={} block_id={} page_digest={}",
                page.height,
                block_id.to_base58(),
                page.digest.to_base58()
            )));
        }

        let mut page_slab = NounSlab::<NockJammer>::new();
        let page_noun = page_slab.copy_into(source_page_noun, &response_space);
        page_slab.set_root(page_noun);
        blocks.push(NockBlockEvent {
            block: page,
            page_slab,
            page_noun,
            txs: Vec::new(),
        });
        list = list_cell.tail().noun();
    }

    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use nockchain_types::default_fakenet_blockchain_constants;

    use super::*;

    fn atom_to_string(atom: nockvm::noun::AtomHandle<'_>) -> String {
        let mut bytes = atom.to_be_bytes();
        while bytes.first() == Some(&0) {
            bytes.remove(0);
        }
        bytes.reverse();
        String::from_utf8(bytes).expect("utf8")
    }

    fn sample_page(height: u64) -> Page {
        use nockchain_math::belt::Belt;
        use nockchain_types::tx_engine::common::{BigNum, CoinbaseSplit, Hash};

        Page {
            digest: Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            pow: None,
            parent: Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
            tx_ids: Vec::new(),
            coinbase: CoinbaseSplit::V0(Vec::new()),
            timestamp: 1_717_171,
            epoch_counter: 42,
            target: BigNum::from_u64(1_000),
            accumulated_work: BigNum::from_u64(2_000),
            height,
            msg: Vec::new(),
        }
    }

    fn sample_block_event(height: u64, has_transactions: bool) -> NockBlockEvent {
        let mut page = sample_page(height);
        if has_transactions {
            page.tx_ids.push(page.digest.clone());
        }
        let mut page_slab = NounSlab::<NockJammer>::new();
        let page_noun = page.to_noun(&mut page_slab);
        page_slab.set_root(page_noun);
        NockBlockEvent {
            block: page,
            page_slab,
            page_noun,
            txs: Vec::new(),
        }
    }
    #[test]
    fn jam_path_roundtrips_through_cue() {
        let path = vec![Bytes::from("block"), Bytes::from("42")];
        let jammed = jam_path(&path).expect("jam path");
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let mut current = slab
            .cue_into(Bytes::from(jammed.clone()))
            .expect("cue jammed path");

        let space = slab.noun_space();
        for segment in path {
            let cell = current.in_space(&space).as_cell().expect("cell");
            let atom = cell.head().as_atom().expect("atom");
            let decoded = atom_to_string(atom);
            assert_eq!(decoded, segment);
            current = cell.tail().noun();
        }
    }

    #[test]
    fn hash_to_base58_produces_valid_output() {
        use nockchain_math::belt::Belt;
        use nockchain_types::tx_engine::common::Hash;

        let hash = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let base58 = hash.to_base58();
        assert!(!base58.is_empty());
    }

    #[test]
    fn nock_in_flight_guard_waits_until_kernel_height_advances() {
        assert!(nock_block_still_waiting_for_kernel(Some(7), Some(7)));
        assert!(!nock_block_still_waiting_for_kernel(Some(7), Some(8)));
        assert!(!nock_block_still_waiting_for_kernel(Some(7), None));
        assert!(!nock_block_still_waiting_for_kernel(None, Some(7)));
    }

    #[test]
    fn buffered_transaction_boundary_requires_fresh_tip() {
        let mut blocks =
            VecDeque::from([sample_block_event(10, false), sample_block_event(11, true)]);

        assert!(!buffered_transaction_requires_tip_refresh(
            &blocks,
            Some(10)
        ));
        blocks.pop_front();
        assert!(buffered_transaction_requires_tip_refresh(&blocks, Some(11)));
        assert!(!buffered_transaction_requires_tip_refresh(
            &blocks,
            Some(12)
        ));
        assert!(!buffered_transaction_requires_tip_refresh(&blocks, None));
    }

    #[test]
    fn cached_tip_only_covers_confirmed_heights() {
        let tip = NockTipInfo {
            height: 1_000,
            tip_hash: "tip".to_string(),
        };

        assert!(tip_covers_next_confirmed_height(Some(&tip), Some(600), 400));
        assert!(!tip_covers_next_confirmed_height(
            Some(&tip),
            Some(601),
            400
        ));
        assert!(!tip_covers_next_confirmed_height(Some(&tip), None, 400));
    }

    #[test]
    fn tip_change_invalidates_prefetched_blocks() {
        let previous = NockTipInfo {
            height: 1_000,
            tip_hash: "old-tip".to_string(),
        };
        let same = NockTipInfo {
            height: 1_000,
            tip_hash: "old-tip".to_string(),
        };
        let extension = NockTipInfo {
            height: 1_001,
            tip_hash: "new-tip".to_string(),
        };
        let lower_same_hash = NockTipInfo {
            height: 999,
            tip_hash: "old-tip".to_string(),
        };

        assert!(!tip_change_invalidates_prefetch(
            Some(&previous),
            Some(&same)
        ));
        assert!(tip_change_invalidates_prefetch(
            Some(&previous),
            Some(&extension)
        ));
        assert!(tip_change_invalidates_prefetch(
            Some(&previous),
            Some(&lower_same_hash)
        ));
        assert!(tip_change_invalidates_prefetch(Some(&previous), None));
        assert!(!tip_change_invalidates_prefetch(None, Some(&extension)));
    }

    #[test]
    fn range_returns_blocks_before_future_transaction_hydration() {
        let mut blocks = vec![
            sample_block_event(10, false),
            sample_block_event(11, false),
            sample_block_event(12, true),
            sample_block_event(13, true),
        ];

        let hydration_height =
            retain_hydratable_prefix(&mut blocks).map(|block| block.block.height);

        assert_eq!(hydration_height, None);
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.block.height)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    fn range_hydrates_only_current_transaction_block() {
        let mut blocks = vec![sample_block_event(12, true), sample_block_event(13, true)];

        let hydration_height =
            retain_hydratable_prefix(&mut blocks).map(|block| block.block.height);

        assert_eq!(hydration_height, Some(12));
        assert_eq!(blocks.len(), 1);
    }
    #[test]
    fn block_range_decoder_owns_each_page_noun() {
        let page = sample_page(77);
        let mut slab = NounSlab::<NockJammer>::new();
        let block_id = page.digest.to_noun(&mut slab);
        let page_noun = page.to_noun(&mut slab);
        let entry = nockvm::noun::T(
            &mut slab,
            &[nockvm::noun::D(page.height), block_id, page_noun, nockvm::noun::D(0)],
        );
        let list = nockvm::noun::T(&mut slab, &[entry, nockvm::noun::D(0)]);
        let inner = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), list]);
        let outer = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), inner]);
        slab.set_root(outer);

        let mut decoded =
            decode_block_range_response(slab.jam().to_vec()).expect("decode block range");
        assert_eq!(decoded.len(), 1);
        let event = decoded.pop().expect("one block");
        assert_eq!(event.block, page);
        assert!(event.txs.is_empty());

        let owned_space = event.page_slab.noun_space();
        let owned_page =
            Page::from_noun(&event.page_noun, &owned_space).expect("owned page noun decodes");
        assert_eq!(owned_page, page);
    }

    #[test]
    fn single_block_fallback_decoder_owns_page_noun() {
        let page = sample_page(88);
        let mut slab = NounSlab::<NockJammer>::new();
        let page_noun = page.to_noun(&mut slab);
        let inner = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), page_noun]);
        let outer = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), inner]);
        slab.set_root(outer);

        let event = decode_block_page_response(slab.jam().to_vec())
            .expect("decode single block")
            .expect("block payload");
        assert_eq!(event.block, page);
        let owned_space = event.page_slab.noun_space();
        let owned_page =
            Page::from_noun(&event.page_noun, &owned_space).expect("owned page noun decodes");
        assert_eq!(owned_page, page);
    }

    #[tokio::test]
    async fn nock_grpc_timeout_returns_event_monitoring_error() {
        let err = with_nock_grpc_timeout(
            "test nock request",
            Duration::from_millis(1),
            std::future::pending::<Result<(), BridgeError>>(),
        )
        .await
        .expect_err("pending request should time out");

        assert!(
            matches!(err, BridgeError::EventMonitoring(_)),
            "unexpected error variant: {err:?}"
        );
        assert!(
            err.to_string().contains("test nock request timed out"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn nock_grpc_timeout_classifier_matches_private_timeout_format() {
        let timeout_err =
            BridgeError::EventMonitoring("height block peek timed out after 30s".to_string());
        let event_monitoring_err =
            BridgeError::EventMonitoring("height block peek failed to decode response".to_string());
        let runtime_err = BridgeError::Runtime("height block peek timed out after 30s".to_string());

        assert!(is_nock_grpc_timeout_error(&timeout_err));
        assert!(!is_nock_grpc_timeout_error(&event_monitoring_err));
        assert!(!is_nock_grpc_timeout_error(&runtime_err));
    }

    #[test]
    fn nock_grpc_response_size_classifier_and_reduction_are_bounded() {
        let oversized = BridgeError::EventMonitoring(
            "gRPC status error: Error, decoded message length too large: found 20 bytes, the limit is: 10 bytes"
                .to_string(),
        );
        let unrelated = BridgeError::EventMonitoring("connection unavailable".to_string());

        assert!(is_nock_grpc_response_too_large_error(&oversized));
        assert!(!is_nock_grpc_response_too_large_error(&unrelated));
        assert_eq!(reduced_batch_end(100, 163), 131);
        assert_eq!(reduced_batch_end(100, 101), 100);
        assert_eq!(reduced_batch_end(100, 100), 100);
    }

    #[test]
    fn tx_id_to_base58_does_not_error() {
        use nockchain_math::belt::Belt;
        use nockchain_types::tx_engine::common::Hash;

        let tx_id = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let result = tx_id.to_base58();
        assert!(!result.is_empty());
    }

    #[test]
    fn confirmed_height_returns_none_during_bootstrap() {
        let depth = DEFAULT_NOCKCHAIN_CONFIRMATION_DEPTH;
        assert!(confirmed_height(0, depth).is_none());
        assert!(confirmed_height(depth, depth).is_none());
    }

    #[test]
    fn confirmed_height_zero_depth_uses_current_tip() {
        assert_eq!(confirmed_height(0, 0), None);
        assert_eq!(confirmed_height(50, 0), Some(50));
    }

    #[test]
    fn confirmed_height_returns_target_when_ready() {
        let depth = DEFAULT_NOCKCHAIN_CONFIRMATION_DEPTH;
        let tip = depth + 50;
        let target = confirmed_height(tip, depth);
        assert!(target.is_some());
        assert_eq!(target.expect("target should be Some for valid input"), 50);
    }

    #[test]
    fn truncate_error_msg_short_string() {
        let msg = "short error";
        assert_eq!(truncate_error_msg(msg, 50), "short error");
    }

    #[test]
    fn truncate_error_msg_exact_length() {
        let msg = "12345";
        assert_eq!(truncate_error_msg(msg, 5), "12345");
    }

    #[test]
    fn truncate_error_msg_long_string() {
        let msg = "this is a very long error message that should be truncated";
        let result = truncate_error_msg(msg, 20);
        assert_eq!(result, "this is a very long ...");
        assert_eq!(result.len(), 23); // 20 + "..."
    }

    #[test]
    fn decode_blockchain_constants_response_roundtrips_nested_unit_payload() {
        let constants = default_fakenet_blockchain_constants();
        let mut slab = NounSlab::<NockJammer>::new();
        let noun = Some(Some(constants.clone())).to_noun(&mut slab);
        slab.set_root(noun);

        let decoded =
            decode_blockchain_constants_response(slab.jam().to_vec()).expect("decode response");
        assert_eq!(decoded, constants);
    }

    #[test]
    fn validate_blockchain_constants_match_accepts_equal_values() {
        let constants = default_fakenet_blockchain_constants();
        validate_blockchain_constants_match(&constants, &constants)
            .expect("matching constants should validate");
    }

    #[test]
    fn validate_blockchain_constants_match_rejects_mismatch() {
        let expected = default_fakenet_blockchain_constants();
        let mut actual = expected.clone();
        actual.ai_asert.anchor_min_timestamp += 1;

        let err = validate_blockchain_constants_match(&expected, &actual)
            .expect_err("mismatched constants should fail");
        let message = err.to_string();
        assert!(message.contains("bridge kernel state"));
        assert!(message.contains("ai_asert.anchor_min_timestamp"));
        assert!(message.contains("ai_asert.anchor_min_timestamp expected="));
    }
}
