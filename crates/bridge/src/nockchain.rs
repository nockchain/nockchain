use std::sync::Arc;
use std::time::{Duration, SystemTime};

use backon::{ExponentialBuilder, Retryable};
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, ToBytes};
use nockapp_grpc::services::private_nockapp::client::PrivateNockAppGrpcClient;
use nockchain_types::tx_engine::common::{BlockId, Heavy, Page, TxId};
use noun_serde::prelude::*;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::bridge_status::BridgeStatus;
use crate::errors::BridgeError;
use crate::metrics;
use crate::runtime::{BridgeEvent, BridgeRuntimeHandle, ChainEvent, NockBlockEvent};
use crate::stop::StopHandle;
use crate::tui::types::{AlertSeverity, ChainState, NetworkState, NockchainApiStatus};
use crate::types::Tx;

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(10);
const CLIENT_PID: i32 = 1;

/// Default nockchain confirmation depth used by the driver if not specified in config.
///
/// The bridge kernel assumes blocks it receives are final; this is enforced by the Rust driver.
pub const DEFAULT_NOCKCHAIN_CONFIRMATION_DEPTH: u64 = 100;

fn confirmed_height(chain_tip: u64, confirmation_depth: u64) -> Option<u64> {
    if confirmation_depth == 0 {
        return None;
    }
    let target = chain_tip.saturating_sub(confirmation_depth);
    if target == 0 {
        None
    } else {
        Some(target)
    }
}

struct NockBlockObservation {
    block: Page,
    page_slab: NounSlab<NockJammer>,
    page_noun: nockapp::Noun,
    txs: Vec<(TxId, Tx)>,
}

pub struct NockchainWatcher {
    endpoint: String,
    poll_interval: Duration,
    runtime: Arc<BridgeRuntimeHandle>,
    confirmation_depth: u64,
    stop: StopHandle,
    /// Optional bridge_status for connection status + alert updates.
    bridge_status: Option<BridgeStatus>,
}

impl NockchainWatcher {
    pub fn new(
        endpoint: String,
        runtime: Arc<BridgeRuntimeHandle>,
        confirmation_depth: u64,
        stop: StopHandle,
    ) -> Self {
        Self {
            endpoint,
            poll_interval: DEFAULT_POLL_INTERVAL,
            runtime,
            confirmation_depth,
            stop,
            bridge_status: None,
        }
    }

    pub fn with_poll_interval(
        endpoint: String,
        runtime: Arc<BridgeRuntimeHandle>,
        poll_interval: Duration,
        confirmation_depth: u64,
        stop: StopHandle,
    ) -> Self {
        Self {
            endpoint,
            poll_interval,
            runtime,
            confirmation_depth,
            stop,
            bridge_status: None,
        }
    }

    /// Set the TUI state for connection status updates.
    pub fn with_bridge_status(mut self, bridge_status: BridgeStatus) -> Self {
        self.bridge_status = Some(bridge_status);
        self
    }

    /// Update the nockchain API connection status in the TUI.
    fn update_status(&self, status: NockchainApiStatus) {
        if let Some(ref bridge_status) = self.bridge_status {
            bridge_status.update_nockchain_api_status(status);
        }
    }

    /// Update the nockchain tip hash in the TUI.
    fn update_tip_hash(&self, tip_hash: String) {
        if let Some(ref bridge_status) = self.bridge_status {
            bridge_status.update_nockchain_tip_hash(tip_hash);
        }
    }

    /// Push an alert to the TUI.
    fn push_alert(&self, severity: AlertSeverity, title: String, message: String) {
        if let Some(ref bridge_status) = self.bridge_status {
            bridge_status.push_alert(severity, title, message, "nock-watcher".to_string());
        }
    }

    pub async fn run(self) -> Result<(), BridgeError> {
        let mut was_connected = false;

        // Unlimited retries with exponential backoff
        let backoff = || {
            ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_max_delay(Duration::from_secs(300))
                .with_jitter()
        };

        self.update_status(NockchainApiStatus::connecting(0, None));

        loop {
            if self.stop.is_stopped() {
                sleep(self.poll_interval).await;
                continue;
            }

            let attempt_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let attempt_count_notify = attempt_count.clone();
            let endpoint = self.endpoint.clone();
            let connect = || async { PrivateNockAppGrpcClient::connect(endpoint.clone()).await };

            self.update_status(NockchainApiStatus::connecting(1, None));

            let connect_result = connect
                .retry(backoff())
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
                        endpoint=%self.endpoint,
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
                Ok(mut client) => {
                    self.update_status(NockchainApiStatus::connected());

                    if was_connected {
                        info!(
                            target: "bridge.nock-watcher",
                            endpoint=%self.endpoint,
                            "reconnected to nockchain gRPC endpoint"
                        );
                        self.push_alert(
                            AlertSeverity::Info,
                            "Nockchain API Reconnected".to_string(),
                            format!("Reconnected to {}", self.endpoint),
                        );
                    } else {
                        info!(
                            target: "bridge.nock-watcher",
                            endpoint=%self.endpoint,
                            "connected to nockchain gRPC endpoint"
                        );
                    }
                    was_connected = true;

                    if let Err(err) = self.stream_events(&mut client).await {
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
                        endpoint=%self.endpoint,
                        error=%error_msg,
                        "connect failed unexpectedly"
                    );
                    self.update_status(NockchainApiStatus::disconnected(error_msg));
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn stream_events(
        &self,
        client: &mut PrivateNockAppGrpcClient,
    ) -> Result<(), BridgeError> {
        info!(
            target: "bridge.nock-watcher",
            confirmation_depth = self.confirmation_depth,
            "starting nock observer with confirmation depth"
        );
        loop {
            if self.stop.is_stopped() {
                sleep(self.poll_interval).await;
                continue;
            }
            let (tip_height, tip_hash) = match self.fetch_tip_info(client).await {
                Ok(Some(info)) => info,
                Ok(None) => {
                    debug!(
                        target: "bridge.nock-watcher",
                        "no heaviest block available from private nockapp"
                    );
                    sleep(self.poll_interval).await;
                    continue;
                }
                Err(err) => {
                    warn!(
                        target: "bridge.nock-watcher",
                        error=%err,
                        "failed to fetch tip height"
                    );
                    sleep(self.poll_interval).await;
                    continue;
                }
            };
            self.update_tip_hash(tip_hash);

            let Some(confirmed_target) = confirmed_height(tip_height, self.confirmation_depth)
            else {
                debug!(
                    target: "bridge.nock-watcher",
                    tip_height,
                    "no confirmed block yet (bootstrap)"
                );
                sleep(self.poll_interval).await;
                continue;
            };

            let next_needed_height = match self.runtime.peek_nock_next_height().await {
                Ok(Some(height)) => height,
                Ok(None) => {
                    debug!(
                        target: "bridge.nock-watcher",
                        tip_height,
                        "kernel has no pending nock block"
                    );
                    sleep(self.poll_interval).await;
                    continue;
                }
                Err(err) => {
                    warn!(
                        target: "bridge.nock-watcher",
                        error=%err,
                        "failed to peek nock next height"
                    );
                    sleep(self.poll_interval).await;
                    continue;
                }
            };

            if confirmed_target < next_needed_height {
                debug!(
                    target: "bridge.nock-watcher",
                    tip_height,
                    confirmed_target,
                    next_needed_height,
                    "target height not yet confirmed for kernel need"
                );
                sleep(self.poll_interval).await;
                continue;
            }

            match self.fetch_block_at_height(client, next_needed_height).await {
                Ok(Some(observation)) => {
                    let height = observation.block.height;
                    let block_hash = observation.block.digest.to_base58();
                    let txs_count = observation.txs.len();
                    let event = NockBlockEvent {
                        block: observation.block,
                        page_slab: observation.page_slab,
                        page_noun: observation.page_noun,
                        txs: observation.txs,
                    };
                    self.runtime
                        .send_event(BridgeEvent::Chain(Box::new(ChainEvent::Nock(event))))
                        .await?;
                    info!(
                        target: "bridge.nock-watcher",
                        height,
                        tip_height,
                        confirmations = tip_height - height,
                        hash=%block_hash,
                        txs_count=%txs_count,
                        "emitted confirmed nock block"
                    );
                }
                Ok(None) => {
                    debug!(
                        target: "bridge.nock-watcher",
                        target = next_needed_height,
                        "block at target height not found"
                    );
                }
                Err(err) => {
                    warn!(
                        target: "bridge.nock-watcher",
                        target = next_needed_height,
                        error=%err,
                        "failed to fetch block at height"
                    );
                }
            }
            sleep(self.poll_interval).await;
        }
    }

    async fn fetch_tip_info(
        &self,
        client: &mut PrivateNockAppGrpcClient,
    ) -> Result<Option<(u64, String)>, BridgeError> {
        let heavy_path = vec![Bytes::from("heavy")];
        let heavy_bytes = jam_path(&heavy_path)?;
        let response = client
            .peek(CLIENT_PID, heavy_bytes)
            .await
            .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
        let (_heavy_slab, heavy_noun) = cue_response(response)?;
        let heavy: Heavy = heavy_noun.decode().map_err(|err| {
            BridgeError::EventMonitoring(format!("failed to decode heavy response: {}", err))
        })?;
        let Some(block_id_base58) = heavy.to_base58() else {
            return Ok(None);
        };
        let tip_hash = block_id_base58.clone();

        let block_path = vec![Bytes::from("block"), Bytes::from(block_id_base58.clone())];
        let block_bytes = jam_path(&block_path)?;
        let response = client
            .peek(CLIENT_PID, block_bytes)
            .await
            .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
        let (_page_slab, block_noun) = cue_response(response)?;

        let (page, _page_noun) = decode_page_from_peek(&block_noun)?;
        Ok(Some((page.height, tip_hash)))
    }

    async fn fetch_block_at_height(
        &self,
        client: &mut PrivateNockAppGrpcClient,
        height: u64,
    ) -> Result<Option<NockBlockObservation>, BridgeError> {
        let heavy_n_path = vec![Bytes::from("heavy-n"), Bytes::from(height.to_bytes()?)];
        let heavy_n_bytes = jam_path(&heavy_n_path)?;
        let response = client
            .peek(CLIENT_PID, heavy_n_bytes)
            .await
            .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
        let (page_slab, block_noun) = cue_response(response)?;

        let (page, page_noun) = match decode_page_from_peek(&block_noun) {
            Ok(result) => result,
            Err(_) => return Ok(None),
        };

        let txs = self
            .fetch_transactions(client, &page.digest, &page.tx_ids)
            .await?;

        Ok(Some(NockBlockObservation {
            block: page,
            page_slab,
            page_noun,
            txs,
        }))
    }

    async fn fetch_transactions(
        &self,
        client: &mut PrivateNockAppGrpcClient,
        block_id: &BlockId,
        tx_ids: &[TxId],
    ) -> Result<Vec<(TxId, Tx)>, BridgeError> {
        let block_id_base58 = block_id.to_base58();
        let mut txs = Vec::with_capacity(tx_ids.len());
        for tx_id in tx_ids {
            let tx_id_base58 = tx_id.to_base58();
            let tx_path = vec![
                Bytes::from("block-transaction"),
                Bytes::from(block_id_base58.clone()),
                Bytes::from(tx_id_base58),
            ];
            let tx_bytes = jam_path(&tx_path)?;
            let response = client
                .peek(CLIENT_PID, tx_bytes)
                .await
                .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
            let (_tx_slab, tx_noun) = cue_response(response)?;
            let tx = decode_tx_from_peek(&tx_noun)?;
            txs.push((tx_id.clone(), tx));
        }
        Ok(txs)
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
            ia.normalize_as_atom().as_noun()
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

fn decode_page_from_peek(noun: &nockapp::Noun) -> Result<(Page, nockapp::Noun), BridgeError> {
    let outer_cell = noun
        .as_cell()
        .map_err(|_| BridgeError::EventMonitoring("peek response expected to be cell".into()))?;

    let outer_head = outer_cell.head();
    let outer_tail = outer_cell.tail();

    let outer_tag = outer_head.as_atom().map_err(|_| {
        BridgeError::EventMonitoring("peek response outer unit tag expected to be atom".into())
    })?;

    let outer_tag_val = outer_tag
        .as_u64()
        .map_err(|_| BridgeError::EventMonitoring("peek response outer tag too large".into()))?;

    if outer_tag_val != 0 {
        return Err(BridgeError::EventMonitoring(format!(
            "peek response indicates no data (outer unit tag={})",
            outer_tag_val
        )));
    }

    let inner = outer_tail;

    if inner.is_atom() {
        return Err(BridgeError::EventMonitoring(
            "peek response inner is atom (no data)".into(),
        ));
    }

    let inner_cell = inner.as_cell().map_err(|_| {
        BridgeError::EventMonitoring("peek response inner expected to be cell".into())
    })?;

    let inner_head = inner_cell.head();
    let inner_tail = inner_cell.tail();

    if let Ok(tag_atom) = inner_head.as_atom() {
        if let Ok(tag_val) = tag_atom.as_u64() {
            if tag_val == 0 {
                if inner_tail.is_atom() {
                    if let Ok(atom) = inner_tail.as_atom() {
                        if let Ok(val) = atom.as_u64() {
                            if val == 0 {
                                return Err(BridgeError::EventMonitoring(
                                    "block not found (inner unit is null)".into(),
                                ));
                            }
                        }
                    }
                    return Err(BridgeError::EventMonitoring(
                        "inner_tail is atom but not null - unexpected structure".into(),
                    ));
                }

                let page = Page::from_noun(&inner_tail).map_err(|err| {
                    BridgeError::EventMonitoring(format!(
                        "failed to decode Page (after inner unwrap): {}",
                        err
                    ))
                })?;
                return Ok((page, inner_tail));
            }
        }
    }

    let page = Page::from_noun(&inner)
        .map_err(|err| BridgeError::EventMonitoring(format!("failed to decode Page: {}", err)))?;
    Ok((page, inner))
}

fn decode_tx_from_peek(noun: &nockapp::Noun) -> Result<Tx, BridgeError> {
    // peek returns (unit (unit tx:t)), need to unwrap both layers
    let outer_cell = noun
        .as_cell()
        .map_err(|_| BridgeError::EventMonitoring("peek response expected to be cell".into()))?;

    let outer_tag = outer_cell.head().as_atom().map_err(|_| {
        BridgeError::EventMonitoring("peek response outer unit tag expected to be atom".into())
    })?;

    if outer_tag
        .as_u64()
        .map_err(|_| BridgeError::EventMonitoring("peek response outer tag too large".into()))?
        != 0
    {
        return Err(BridgeError::EventMonitoring(
            "peek response indicates no data for transaction (outer unit)".into(),
        ));
    }

    let inner_cell = outer_cell.tail().as_cell().map_err(|_| {
        BridgeError::EventMonitoring("peek response inner unit expected to be cell".into())
    })?;

    let inner_tag = inner_cell.head().as_atom().map_err(|_| {
        BridgeError::EventMonitoring("peek response inner unit tag expected to be atom".into())
    })?;

    if inner_tag
        .as_u64()
        .map_err(|_| BridgeError::EventMonitoring("peek response inner tag too large".into()))?
        != 0
    {
        return Err(BridgeError::EventMonitoring(
            "peek response indicates no data for transaction (inner unit)".into(),
        ));
    }

    Tx::from_noun(&inner_cell.tail())
        .map_err(|err| BridgeError::EventMonitoring(format!("failed to decode Tx: {}", err)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom_to_string(atom: nockvm::noun::Atom) -> String {
        let mut bytes = atom.to_be_bytes();
        while bytes.first() == Some(&0) {
            bytes.remove(0);
        }
        bytes.reverse();
        String::from_utf8(bytes).expect("utf8")
    }

    #[test]
    fn jam_path_roundtrips_through_cue() {
        let path = vec![Bytes::from("block"), Bytes::from("42")];
        let jammed = jam_path(&path).expect("jam path");
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let mut current = slab
            .cue_into(Bytes::from(jammed.clone()))
            .expect("cue jammed path");
        for segment in path {
            let cell = current.as_cell().expect("cell");
            let atom = cell.head().as_atom().expect("atom");
            let decoded = atom_to_string(atom);
            assert_eq!(decoded, segment);
            current = cell.tail();
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
    fn tx_id_to_base58_does_not_error() {
        use nockchain_math::belt::Belt;
        use nockchain_types::tx_engine::common::Hash;

        let tx_id = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let result = tx_id.to_base58();
        assert!(!result.is_empty());
    }

    #[test]
    fn decode_page_from_peek_decodes_tagged_bn_numbers() {
        use nockchain_math::belt::Belt;
        use nockchain_types::tx_engine::common::{CoinbaseSplit, Hash};
        use noun_serde::NounEncode;
        use num_bigint::BigUint;

        fn tagged_bn_noun(allocator: &mut NounSlab<NockJammer>, chunks: &[u32]) -> nockapp::Noun {
            let chunks_noun = chunks.to_vec().to_noun(allocator);
            nockvm::noun::T(allocator, &[nockvm::noun::D(28258), chunks_noun])
        }

        fn biguint_from_u32_chunks(chunks: &[u32]) -> BigUint {
            let mut bytes = Vec::with_capacity(chunks.len() * 4);
            for &chunk in chunks {
                bytes.extend_from_slice(&chunk.to_le_bytes());
            }
            while bytes.last() == Some(&0) {
                bytes.pop();
            }
            BigUint::from_bytes_le(&bytes)
        }

        let digest = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let parent = Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]);
        let expected_coinbase = CoinbaseSplit::V0(vec![0xaa, 0xbb]);
        let expected_msg = vec![7u32, 8u32, 9u32];

        let target_chunks = [0x89abcdef, 0x01234567, 0xfedcba98, 0x76543210];
        let accumulated_work_chunks = [0xffffffff, 0x00000000, 0x22222222, 0x33333333, 0x44444444];

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let digest_noun = digest.to_noun(&mut slab);
        let parent_noun = parent.to_noun(&mut slab);
        let coinbase_noun = expected_coinbase.to_noun(&mut slab);
        let msg_noun = expected_msg.to_noun(&mut slab);
        let target_noun = tagged_bn_noun(&mut slab, &target_chunks);
        let accumulated_work_noun = tagged_bn_noun(&mut slab, &accumulated_work_chunks);

        let page_noun = nockvm::noun::T(
            &mut slab,
            &[
                nockvm::noun::D(1), // page version
                digest_noun,
                nockvm::noun::D(0), // no pow
                parent_noun,
                nockvm::noun::D(0), // empty tx_ids z-set
                coinbase_noun,
                nockvm::noun::D(1_717_171),
                nockvm::noun::D(42),
                target_noun,
                accumulated_work_noun,
                nockvm::noun::D(77),
                msg_noun,
            ],
        );

        // peek response shape: [~ [~ page]]
        let inner_unit = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), page_noun]);
        let peek_noun = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), inner_unit]);

        let (page, _) = decode_page_from_peek(&peek_noun).expect("decode tagged-bn page");

        assert_eq!(page.digest, digest, "digest should decode correctly");
        assert_eq!(page.parent, parent, "parent should decode correctly");
        assert_eq!(page.coinbase, expected_coinbase, "coinbase should decode");
        assert_eq!(page.timestamp, 1_717_171, "timestamp should decode");
        assert_eq!(page.epoch_counter, 42, "epoch counter should decode");
        assert_eq!(page.height, 77, "height should decode");
        assert_eq!(page.msg, expected_msg, "msg should decode");
        assert_eq!(
            page.target.0,
            biguint_from_u32_chunks(&target_chunks),
            "target should decode via [%bn (list u32)] path"
        );
        assert_eq!(
            page.accumulated_work.0,
            biguint_from_u32_chunks(&accumulated_work_chunks),
            "accumulated_work should decode via [%bn (list u32)] path"
        );
    }

    #[test]
    fn confirmed_height_returns_none_during_bootstrap() {
        let depth = DEFAULT_NOCKCHAIN_CONFIRMATION_DEPTH;
        assert!(confirmed_height(0, depth).is_none());
        assert!(confirmed_height(depth, depth).is_none());
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
}
