use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hex::encode as hex_encode;
use tokio::time::{interval, MissedTickBehavior};
use tonic::{Request, Response, Status};

use crate::bridge_status::{BridgeStatus, ALERT_HISTORY_CAPACITY};
use crate::config::NonceEpochConfig;
use crate::deposit_log::DepositLog;
use crate::errors::BridgeError;
use crate::health::{NodeHealthSnapshot, NodeHealthStatus};
use crate::metrics;
use crate::status::BridgeStatusState;
use crate::tui::types::{
    format_nock_from_nicks, Alert, AlertSeverity, AlertState, BatchStatus as TuiBatchStatus,
    BridgeTx as TuiBridgeTx, ChainState as TuiChainState, DepositLogSnapshot, DepositLogView,
    MetricsState as TuiMetricsState, NockchainApiStatus as TuiNockchainApiStatus,
    Proposal as TuiProposal, ProposalState as TuiProposalState,
    ProposalStatus as TuiProposalStatus, TransactionState as TuiTransactionState,
    TxDirection as TuiTxDirection, TxStatus as TuiTxStatus, DEPOSIT_LOG_PAGE_SIZE,
};

pub mod proto {
    tonic::include_proto!("bridge.tui.v1");
}

use proto::batch_status::Status as ProtoBatchStatusKind;
use proto::bridge_tui_server::BridgeTui;
use proto::nockchain_api_status::State as ProtoNockchainApiState;
use proto::tx_status::Status as ProtoTxStatusKind;
use proto::{
    Alert as ProtoAlert, AlertSeverity as ProtoAlertSeverity, AlertView as ProtoAlertView,
    AlertsSnapshot as ProtoAlertsSnapshot, Base58Hash, BatchAwaitingSignatures, BatchIdle,
    BatchProcessing, BatchStatus, BatchSubmitting, BridgeTx as ProtoBridgeTx, ChainState,
    DepositLogRow, DepositLogSnapshot as ProtoDepositLogSnapshot,
    DepositLogView as ProtoDepositLogView, EthAddress as EthAddressProto, GetSnapshotRequest,
    GetSnapshotResponse, LastDeposit, MetricsState as ProtoMetricsState, NetworkState,
    PeerHealthStatus, PeerStatus, Proposal, ProposalState, ProposalStatus, RunningState,
    SuccessfulDeposit, TransactionState as ProtoTransactionState, TxDirection as ProtoTxDirection,
    TxStatus as ProtoTxStatus, TxStatusCompleted, TxStatusConfirming, TxStatusFailed,
    TxStatusPending, TxStatusProcessing,
};

#[derive(Clone)]
pub struct BridgeTuiService {
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
    snapshot_cache: Arc<RwLock<Option<CachedSnapshot>>>,
}

impl BridgeTuiService {
    pub async fn new(
        bridge_status: BridgeStatus,
        status_state: BridgeStatusState,
        deposit_log: Arc<DepositLog>,
        nonce_epoch: NonceEpochConfig,
    ) -> Result<Self, BridgeError> {
        let snapshot_cache = Arc::new(RwLock::new(None));

        match build_cached_snapshot(&bridge_status, &status_state, &deposit_log, &nonce_epoch).await
        {
            Ok(initial_snapshot) => {
                if let Ok(mut guard) = snapshot_cache.write() {
                    *guard = Some(initial_snapshot);
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "bridge.tui",
                    error=%err,
                    "failed to warm TUI snapshot cache, will retry"
                );
            }
        }

        spawn_snapshot_refresher(
            snapshot_cache.clone(),
            bridge_status.clone(),
            status_state.clone(),
            deposit_log.clone(),
            nonce_epoch.clone(),
        );

        Ok(Self {
            deposit_log,
            nonce_epoch,
            snapshot_cache,
        })
    }
}

#[tonic::async_trait]
impl BridgeTui for BridgeTuiService {
    async fn get_snapshot(
        &self,
        request: Request<GetSnapshotRequest>,
    ) -> Result<Response<GetSnapshotResponse>, Status> {
        let metrics = metrics::init_metrics();
        let started = Instant::now();
        metrics.tui_snapshot_requests.increment();

        let request = request.into_inner();
        let view = request
            .deposit_log_view
            .map(deposit_log_view_from_proto)
            .unwrap_or_default();
        let alert_limit = request
            .alert_view
            .map(alert_view_from_proto)
            .unwrap_or(ALERT_HISTORY_CAPACITY);
        metrics
            .tui_snapshot_alert_limit_requested
            .swap(alert_limit as f64);
        metrics.tui_snapshot_limit_requested.swap(view.limit as f64);
        metrics
            .tui_snapshot_offset_requested
            .swap(view.offset as f64);
        if view.limit > SNAPSHOT_CACHE_LIMIT {
            metrics.tui_snapshot_limit_over_cache.increment();
        }
        if view.limit > 10_000 {
            metrics.tui_snapshot_limit_over_10000.increment();
        }

        let cached = self
            .snapshot_cache
            .read()
            .ok()
            .and_then(|guard| guard.clone());

        let Some(snapshot) = cached else {
            metrics
                .tui_snapshot_response_time
                .add_timing(&started.elapsed());
            return Err(Status::unavailable("snapshot cache is not ready"));
        };

        let to_response_started = Instant::now();
        let mut response = snapshot.to_response(view, alert_limit);
        metrics
            .tui_snapshot_to_response_time
            .add_timing(&to_response_started.elapsed());
        if !snapshot.deposit_log.covers(view) {
            metrics.tui_snapshot_uncached_requests.increment();
            let uncached_started = Instant::now();
            match self.deposit_log.snapshot(&self.nonce_epoch, view).await {
                Ok(snapshot) => {
                    response.deposit_log = Some(deposit_log_snapshot_to_proto(&snapshot));
                }
                Err(err) => {
                    tracing::warn!(
                        target: "bridge.tui",
                        error=%err,
                        "failed to load deposit log page"
                    );
                }
            }
            metrics
                .tui_snapshot_uncached_load_time
                .add_timing(&uncached_started.elapsed());
        }

        metrics
            .tui_snapshot_response_time
            .add_timing(&started.elapsed());
        Ok(Response::new(response))
    }
}

const SNAPSHOT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SNAPSHOT_CACHE_LIMIT: usize = DEPOSIT_LOG_PAGE_SIZE;

#[derive(Clone, Debug)]
struct CachedDepositLog {
    total_count: u64,
    first_epoch_nonce: u64,
    rows: Vec<DepositLogRow>,
}

impl CachedDepositLog {
    fn from_snapshot(snapshot: &DepositLogSnapshot) -> Self {
        Self {
            total_count: snapshot.total_count,
            first_epoch_nonce: snapshot.first_epoch_nonce,
            rows: snapshot
                .rows
                .iter()
                .map(|row| DepositLogRow {
                    nonce: row.nonce,
                    block_height: row.block_height,
                    tx_id_base58: row.tx_id_base58.clone(),
                    recipient_hex: row.recipient_hex.clone(),
                    amount: row.amount,
                })
                .collect(),
        }
    }

    fn covers(&self, view: DepositLogView) -> bool {
        if self.total_count == 0 {
            return true;
        }

        let end = view.offset.saturating_add(view.limit);
        end <= self.rows.len()
    }

    fn slice(&self, view: DepositLogView) -> ProtoDepositLogSnapshot {
        if self.rows.is_empty() {
            return ProtoDepositLogSnapshot {
                total_count: self.total_count,
                first_epoch_nonce: self.first_epoch_nonce,
                rows: Vec::new(),
            };
        }

        let start = view.offset.min(self.rows.len());
        let end = start.saturating_add(view.limit).min(self.rows.len());
        let rows = if start >= end {
            Vec::new()
        } else {
            self.rows[start..end].to_vec()
        };

        ProtoDepositLogSnapshot {
            total_count: self.total_count,
            first_epoch_nonce: self.first_epoch_nonce,
            rows,
        }
    }
}

#[derive(Clone, Debug)]
struct CachedSnapshot {
    running_state: i32,
    nock_hold: bool,
    base_hold: bool,
    nock_hold_height: Option<u64>,
    base_hold_height: Option<u64>,
    network_state: NetworkState,
    deposit_log: CachedDepositLog,
    proposals: ProposalState,
    peer_statuses: Vec<PeerStatus>,
    last_submitted_deposit: Option<LastDeposit>,
    last_successful_deposit: Option<SuccessfulDeposit>,
    alerts: Vec<ProtoAlert>,
    metrics: ProtoMetricsState,
    transactions: ProtoTransactionState,
}

impl CachedSnapshot {
    fn to_response(&self, view: DepositLogView, alert_limit: usize) -> GetSnapshotResponse {
        let alerts = if alert_limit == 0 {
            Vec::new()
        } else {
            self.alerts
                .iter()
                .take(alert_limit.min(self.alerts.len()))
                .cloned()
                .collect()
        };

        GetSnapshotResponse {
            running_state: self.running_state,
            nock_hold: self.nock_hold,
            base_hold: self.base_hold,
            nock_hold_height: self.nock_hold_height,
            base_hold_height: self.base_hold_height,
            network_state: Some(self.network_state.clone()),
            deposit_log: Some(self.deposit_log.slice(view)),
            proposals: Some(self.proposals.clone()),
            peer_statuses: self.peer_statuses.clone(),
            last_submitted_deposit: self.last_submitted_deposit.clone(),
            last_successful_deposit: self.last_successful_deposit.clone(),
            alerts: Some(ProtoAlertsSnapshot { alerts }),
            metrics: Some(self.metrics.clone()),
            transactions: Some(self.transactions.clone()),
        }
    }
}

fn spawn_snapshot_refresher(
    snapshot_cache: Arc<RwLock<Option<CachedSnapshot>>>,
    bridge_status: BridgeStatus,
    status_state: BridgeStatusState,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
) {
    tokio::spawn(async move {
        let mut ticker = interval(SNAPSHOT_REFRESH_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match build_cached_snapshot(&bridge_status, &status_state, &deposit_log, &nonce_epoch)
                .await
            {
                Ok(snapshot) => {
                    if let Ok(mut guard) = snapshot_cache.write() {
                        *guard = Some(snapshot);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        target: "bridge.tui",
                        error=%err,
                        "failed to refresh cached TUI snapshot"
                    );
                }
            }
        }
    });
}

async fn build_cached_snapshot(
    bridge_status: &BridgeStatus,
    status_state: &BridgeStatusState,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
) -> Result<CachedSnapshot, BridgeError> {
    let metrics = metrics::init_metrics();
    let build_started = Instant::now();
    let network = bridge_status.network();
    let running_state = if network.kernel_stopped {
        RunningState::Stopped
    } else {
        RunningState::Running
    };

    let last_submitted_deposit = status_state
        .last_submitted_deposit()
        .map(last_submitted_deposit_to_proto);

    let last_successful_deposit = match bridge_status.last_deposit_nonce() {
        Some(nonce) => match deposit_log.get_by_nonce(nonce, nonce_epoch).await {
            Ok(Some(entry)) => Some(SuccessfulDeposit {
                tx_id: Some(Base58Hash {
                    value: entry.tx_id.to_base58(),
                }),
                name_first: Some(Base58Hash {
                    value: entry.name.first.to_base58(),
                }),
                name_last: Some(Base58Hash {
                    value: entry.name.last.to_base58(),
                }),
                recipient: Some(EthAddressProto {
                    value: format!("0x{}", hex_encode(entry.recipient.0)),
                }),
                amount: entry.amount_to_mint,
                block_height: entry.block_height,
                as_of: Some(Base58Hash {
                    value: entry.as_of.to_base58(),
                }),
                nonce,
            }),
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(
                    target: "bridge.tui",
                    error=%err,
                    nonce,
                    "failed to load last successful deposit from log"
                );
                None
            }
        },
        None => None,
    };

    let view = DepositLogView {
        offset: 0,
        limit: SNAPSHOT_CACHE_LIMIT,
    };
    let deposit_log_snapshot = deposit_log.snapshot(nonce_epoch, view).await?;
    let cached_deposit_log = CachedDepositLog::from_snapshot(&deposit_log_snapshot);

    let alerts_snapshot = alerts_snapshot_to_proto(&bridge_status.alerts(), ALERT_HISTORY_CAPACITY);
    let proposals_state = bridge_status.proposals();

    let proposal_build_started = Instant::now();
    let proposals_proto = proposal_state_to_proto(&proposals_state);
    metrics
        .tui_snapshot_build_proposals_time
        .add_timing(&proposal_build_started.elapsed());

    let pending_inbound_signature_count: usize = proposals_state
        .pending_inbound
        .iter()
        .map(|proposal| proposal.signers.len())
        .sum();
    let history_signature_count: usize = proposals_state
        .history
        .iter()
        .map(|proposal| proposal.signers.len())
        .sum();
    let pending_inbound_bytes: usize = proposals_state
        .pending_inbound
        .iter()
        .map(approximate_tui_proposal_bytes)
        .sum();
    let history_bytes: usize = proposals_state
        .history
        .iter()
        .map(approximate_tui_proposal_bytes)
        .sum();
    let last_submitted_bytes = proposals_state
        .last_submitted
        .as_ref()
        .map(approximate_tui_proposal_bytes)
        .unwrap_or_default();

    metrics
        .tui_proposals_pending_inbound_count
        .swap(proposals_state.pending_inbound.len() as f64);
    metrics
        .tui_proposals_history_count
        .swap(proposals_state.history.len() as f64);
    metrics.tui_proposals_last_submitted_present.swap(
        if proposals_state.last_submitted.is_some() {
            1.0
        } else {
            0.0
        },
    );
    metrics
        .tui_proposals_pending_inbound_signature_count
        .swap(pending_inbound_signature_count as f64);
    metrics
        .tui_proposals_history_signature_count
        .swap(history_signature_count as f64);
    metrics
        .tui_proposals_pending_inbound_approx_bytes
        .swap(pending_inbound_bytes as f64);
    metrics
        .tui_proposals_history_approx_bytes
        .swap(history_bytes as f64);
    metrics
        .tui_proposals_last_submitted_approx_bytes
        .swap(last_submitted_bytes as f64);
    metrics
        .tui_proposals_approx_total_bytes
        .swap((pending_inbound_bytes + history_bytes + last_submitted_bytes) as f64);

    let snapshot = CachedSnapshot {
        running_state: running_state as i32,
        nock_hold: network.nock_hold,
        base_hold: network.base_hold,
        nock_hold_height: if network.nock_hold {
            network.nock_hold_height
        } else {
            None
        },
        base_hold_height: if network.base_hold {
            network.base_hold_height
        } else {
            None
        },
        network_state: network_state_to_proto(&network),
        deposit_log: cached_deposit_log,
        proposals: proposals_proto,
        peer_statuses: bridge_status
            .health_snapshots()
            .iter()
            .map(peer_status_to_proto)
            .collect(),
        last_submitted_deposit,
        last_successful_deposit,
        alerts: alerts_snapshot.alerts,
        metrics: metrics_state_to_proto(&bridge_status.metrics()),
        transactions: transaction_state_to_proto(&bridge_status.transactions()),
    };
    metrics
        .tui_snapshot_build_cache_time
        .add_timing(&build_started.elapsed());
    Ok(snapshot)
}

fn deposit_log_view_from_proto(view: ProtoDepositLogView) -> DepositLogView {
    DepositLogView {
        offset: usize::try_from(view.offset).unwrap_or(usize::MAX),
        limit: usize::try_from(view.limit).unwrap_or(usize::MAX),
    }
}

fn alert_view_from_proto(view: ProtoAlertView) -> usize {
    usize::try_from(view.limit).unwrap_or(ALERT_HISTORY_CAPACITY)
}

fn alerts_snapshot_to_proto(alerts: &AlertState, limit: usize) -> ProtoAlertsSnapshot {
    if limit == 0 {
        return ProtoAlertsSnapshot { alerts: Vec::new() };
    }

    let mut all: Vec<Alert> = alerts.alerts.iter().cloned().collect();
    all.sort_by_key(|alert| std::cmp::Reverse(alert_timestamp_ms(alert)));
    all.truncate(limit);

    ProtoAlertsSnapshot {
        alerts: all.iter().map(alert_to_proto).collect(),
    }
}

fn alert_to_proto(alert: &Alert) -> ProtoAlert {
    ProtoAlert {
        id: alert.id,
        severity: alert_severity_to_proto(alert.severity) as i32,
        title: alert.title.clone(),
        message: alert.message.clone(),
        source: alert.source.clone(),
        created_at_ms: alert_timestamp_ms(alert),
    }
}

fn alert_severity_to_proto(severity: AlertSeverity) -> ProtoAlertSeverity {
    match severity {
        AlertSeverity::Info => ProtoAlertSeverity::Info,
        AlertSeverity::Warning => ProtoAlertSeverity::Warning,
        AlertSeverity::Error => ProtoAlertSeverity::Error,
        AlertSeverity::Critical => ProtoAlertSeverity::Critical,
    }
}

fn alert_timestamp_ms(alert: &Alert) -> u64 {
    system_time_to_millis(alert.timestamp).unwrap_or(0)
}

fn deposit_log_snapshot_to_proto(snapshot: &DepositLogSnapshot) -> ProtoDepositLogSnapshot {
    ProtoDepositLogSnapshot {
        total_count: snapshot.total_count,
        first_epoch_nonce: snapshot.first_epoch_nonce,
        rows: snapshot
            .rows
            .iter()
            .map(|row| DepositLogRow {
                nonce: row.nonce,
                block_height: row.block_height,
                tx_id_base58: row.tx_id_base58.clone(),
                recipient_hex: row.recipient_hex.clone(),
                amount: row.amount,
            })
            .collect(),
    }
}

fn network_state_to_proto(state: &crate::tui::types::NetworkState) -> NetworkState {
    NetworkState {
        base: Some(chain_state_to_proto(&state.base)),
        nockchain: Some(chain_state_to_proto(&state.nockchain)),
        pending_deposits: state.pending_deposits,
        pending_withdrawals: state.pending_withdrawals,
        unsettled_deposit_count: state.unsettled_deposit_count,
        unsettled_withdrawal_count: state.unsettled_withdrawal_count,
        batch_status: Some(batch_status_to_proto(&state.batch_status)),
        is_mainnet: state.is_mainnet,
        nockchain_api_status: Some(nockchain_api_status_to_proto(&state.nockchain_api_status)),
        base_next_height: state.base_next_height,
        nock_next_height: state.nock_next_height,
        degradation_warning: state.degradation_warning.clone(),
    }
}

fn chain_state_to_proto(state: &TuiChainState) -> ChainState {
    ChainState {
        height: state.height,
        tip_hash: state.tip_hash.clone(),
        confirmations: state.confirmations,
        is_syncing: state.is_syncing,
        last_updated_ms: state.last_updated.and_then(system_time_to_millis),
    }
}

fn nockchain_api_status_to_proto(status: &TuiNockchainApiStatus) -> proto::NockchainApiStatus {
    match status {
        TuiNockchainApiStatus::Connected { since } => proto::NockchainApiStatus {
            state: ProtoNockchainApiState::Connected as i32,
            since_ms: system_time_to_millis(*since),
            attempt: None,
            last_error: None,
        },
        TuiNockchainApiStatus::Connecting {
            attempt,
            last_error,
            since,
        } => proto::NockchainApiStatus {
            state: ProtoNockchainApiState::Connecting as i32,
            since_ms: system_time_to_millis(*since),
            attempt: Some(*attempt),
            last_error: last_error.clone(),
        },
        TuiNockchainApiStatus::Disconnected { since, error } => proto::NockchainApiStatus {
            state: ProtoNockchainApiState::Disconnected as i32,
            since_ms: system_time_to_millis(*since),
            attempt: None,
            last_error: Some(error.clone()),
        },
    }
}

fn batch_status_to_proto(status: &TuiBatchStatus) -> BatchStatus {
    let status = match status {
        TuiBatchStatus::Idle => Some(ProtoBatchStatusKind::Idle(BatchIdle {})),
        TuiBatchStatus::Processing {
            batch_id,
            progress_pct,
        } => Some(ProtoBatchStatusKind::Processing(BatchProcessing {
            batch_id: *batch_id,
            progress_pct: u32::from(*progress_pct),
        })),
        TuiBatchStatus::AwaitingSignatures {
            batch_id,
            collected,
            required,
        } => Some(ProtoBatchStatusKind::AwaitingSignatures(
            BatchAwaitingSignatures {
                batch_id: *batch_id,
                collected: u32::from(*collected),
                required: u32::from(*required),
            },
        )),
        TuiBatchStatus::Submitting { batch_id } => {
            Some(ProtoBatchStatusKind::Submitting(BatchSubmitting {
                batch_id: *batch_id,
            }))
        }
    };

    BatchStatus { status }
}

fn proposal_state_to_proto(state: &TuiProposalState) -> ProposalState {
    ProposalState {
        last_submitted: state.last_submitted.as_ref().map(proposal_to_proto),
        pending_inbound: state
            .pending_inbound
            .iter()
            .map(proposal_to_proto)
            .collect(),
        history: state.history.iter().map(proposal_to_proto).collect(),
    }
}

fn proposal_to_proto(proposal: &TuiProposal) -> Proposal {
    let (status, failure_reason) = match &proposal.status {
        TuiProposalStatus::Pending => (ProposalStatus::Pending, None),
        TuiProposalStatus::Ready => (ProposalStatus::Ready, None),
        TuiProposalStatus::Submitted => (ProposalStatus::Submitted, None),
        TuiProposalStatus::Executed => (ProposalStatus::Executed, None),
        TuiProposalStatus::Expired => (ProposalStatus::Expired, None),
        TuiProposalStatus::Failed { reason } => (ProposalStatus::Failed, Some(reason.clone())),
    };

    Proposal {
        id: proposal.id.clone(),
        proposal_type: proposal.proposal_type.clone(),
        description: proposal.description.clone(),
        signatures_collected: u32::from(proposal.signatures_collected),
        signatures_required: u32::from(proposal.signatures_required),
        signers: proposal.signers.clone(),
        created_at_ms: system_time_to_millis(proposal.created_at),
        status: status as i32,
        data_hash: proposal.data_hash.clone(),
        submitted_at_block: proposal.submitted_at_block,
        submitted_at_ms: proposal.submitted_at.and_then(system_time_to_millis),
        tx_hash: proposal.tx_hash.clone(),
        time_to_submit_ms: proposal.time_to_submit_ms,
        executed_at_block: proposal.executed_at_block,
        source_block: proposal.source_block,
        amount: proposal.amount.map(format_nock_from_nicks),
        recipient: proposal.recipient.clone(),
        nonce: proposal.nonce,
        source_tx_id: proposal.source_tx_id.clone(),
        current_proposer: proposal.current_proposer,
        is_my_turn: proposal.is_my_turn,
        time_until_takeover_ms: proposal.time_until_takeover.map(duration_to_millis),
        failure_reason,
    }
}

fn approximate_tui_proposal_bytes(proposal: &TuiProposal) -> usize {
    let mut bytes = std::mem::size_of::<TuiProposal>();
    bytes = bytes
        .saturating_add(proposal.id.len())
        .saturating_add(proposal.proposal_type.len())
        .saturating_add(proposal.description.len())
        .saturating_add(proposal.data_hash.len())
        .saturating_add(
            proposal
                .signers
                .len()
                .saturating_mul(std::mem::size_of::<u64>()),
        );

    if let Some(tx_hash) = &proposal.tx_hash {
        bytes = bytes.saturating_add(tx_hash.len());
    }
    if let Some(recipient) = &proposal.recipient {
        bytes = bytes.saturating_add(recipient.len());
    }
    if let Some(source_tx_id) = &proposal.source_tx_id {
        bytes = bytes.saturating_add(source_tx_id.len());
    }
    if let TuiProposalStatus::Failed { reason } = &proposal.status {
        bytes = bytes.saturating_add(reason.len());
    }

    bytes
}

fn metrics_state_to_proto(state: &TuiMetricsState) -> ProtoMetricsState {
    ProtoMetricsState {
        total_deposited: state.total_deposited.to_string(),
        total_withdrawn: state.total_withdrawn.to_string(),
        hourly_tx_counts: state.hourly_tx_counts.iter().copied().collect(),
        avg_latency_secs: state.avg_latency_secs,
        success_rate: state.success_rate,
        total_fees: state.total_fees.to_string(),
        tx_count: state.tx_count,
        latency_sum_ms: state.latency_sum_ms,
        latency_count: state.latency_count,
    }
}

fn transaction_state_to_proto(state: &TuiTransactionState) -> ProtoTransactionState {
    ProtoTransactionState {
        transactions: state.transactions.iter().map(bridge_tx_to_proto).collect(),
        max_transactions: u64::try_from(state.max_transactions).unwrap_or(u64::MAX),
    }
}

fn bridge_tx_to_proto(tx: &TuiBridgeTx) -> ProtoBridgeTx {
    ProtoBridgeTx {
        tx_hash: tx.tx_hash.clone(),
        direction: tx_direction_to_proto(tx.direction) as i32,
        from: tx.from.clone(),
        to: tx.to.clone(),
        amount: tx.amount.to_string(),
        status: Some(tx_status_to_proto(&tx.status)),
        timestamp_ms: system_time_to_millis(tx.timestamp).unwrap_or(0),
        base_block: tx.base_block,
        nock_height: tx.nock_height,
    }
}

fn tx_direction_to_proto(direction: TuiTxDirection) -> ProtoTxDirection {
    match direction {
        TuiTxDirection::Deposit => ProtoTxDirection::Deposit,
        TuiTxDirection::Withdrawal => ProtoTxDirection::Withdrawal,
    }
}

fn tx_status_to_proto(status: &TuiTxStatus) -> ProtoTxStatus {
    let status = match status {
        TuiTxStatus::Pending => ProtoTxStatusKind::Pending(TxStatusPending {}),
        TuiTxStatus::Confirming {
            confirmations,
            required,
        } => ProtoTxStatusKind::Confirming(TxStatusConfirming {
            confirmations: *confirmations,
            required: *required,
        }),
        TuiTxStatus::Processing => ProtoTxStatusKind::Processing(TxStatusProcessing {}),
        TuiTxStatus::Completed => ProtoTxStatusKind::Completed(TxStatusCompleted {}),
        TuiTxStatus::Failed { reason } => ProtoTxStatusKind::Failed(TxStatusFailed {
            reason: reason.clone(),
        }),
    };

    ProtoTxStatus {
        status: Some(status),
    }
}

fn peer_status_to_proto(snapshot: &NodeHealthSnapshot) -> PeerStatus {
    let (status, error) = match &snapshot.status {
        NodeHealthStatus::Healthy => (PeerHealthStatus::Healthy, None),
        NodeHealthStatus::Unreachable { error } => {
            (PeerHealthStatus::Unreachable, Some(error.clone()))
        }
    };

    PeerStatus {
        node_id: snapshot.node_id,
        address: snapshot.address.clone(),
        status: status as i32,
        error,
        latency_ms: snapshot.latency_ms.map(u128_to_u64),
        peer_uptime_ms: snapshot.peer_uptime_ms,
        last_updated_ms: system_time_to_millis(snapshot.last_updated),
    }
}

fn last_submitted_deposit_to_proto(entry: crate::status::LastSubmittedDeposit) -> LastDeposit {
    LastDeposit {
        tx_id: Some(Base58Hash {
            value: entry.deposit.tx_id.to_base58(),
        }),
        name_first: Some(Base58Hash {
            value: entry.deposit.name.first.to_base58(),
        }),
        name_last: Some(Base58Hash {
            value: entry.deposit.name.last.to_base58(),
        }),
        recipient: Some(EthAddressProto {
            value: format!("0x{}", hex_encode(entry.deposit.recipient.0)),
        }),
        amount: entry.deposit.amount,
        block_height: entry.deposit.block_height,
        as_of: Some(Base58Hash {
            value: entry.deposit.as_of.to_base58(),
        }),
        nonce: entry.deposit.nonce,
        base_tx_hash: entry.base_tx_hash,
        base_block_number: entry.base_block_number,
    }
}

fn system_time_to_millis(time: SystemTime) -> Option<u64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(duration.as_millis()).ok()
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn u128_to_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn base_proposal() -> TuiProposal {
        TuiProposal {
            id: "proposal-1".to_string(),
            proposal_type: "deposit".to_string(),
            description: "test".to_string(),
            signatures_collected: 1,
            signatures_required: 3,
            signers: vec![1],
            created_at: UNIX_EPOCH + Duration::from_secs(1),
            status: TuiProposalStatus::Pending,
            data_hash: "hash".to_string(),
            submitted_at_block: None,
            submitted_at: None,
            tx_hash: None,
            time_to_submit_ms: None,
            executed_at_block: None,
            source_block: None,
            amount: None,
            recipient: None,
            nonce: None,
            source_tx_id: None,
            current_proposer: None,
            is_my_turn: false,
            time_until_takeover: None,
        }
    }

    #[test]
    fn proposal_failure_reason_is_preserved() {
        let mut proposal = base_proposal();
        proposal.status = TuiProposalStatus::Failed {
            reason: "boom".to_string(),
        };
        proposal.amount = Some(1234);

        let proto = proposal_to_proto(&proposal);
        assert_eq!(proto.status, ProposalStatus::Failed as i32);
        assert_eq!(proto.failure_reason, Some("boom".to_string()));
        assert_eq!(proto.amount, Some(format_nock_from_nicks(1234)));
        assert_eq!(proto.created_at_ms, Some(1000));
    }

    #[test]
    fn proposal_amount_is_formatted_as_nock_decimal() {
        let mut proposal = base_proposal();
        let nicks_per_nock = crate::tui::types::NICKS_PER_NOCK;
        proposal.amount = Some(nicks_per_nock + (nicks_per_nock / 2));

        let proto = proposal_to_proto(&proposal);
        assert_eq!(proto.amount, Some("1.5".to_string()));
    }

    #[test]
    fn nockchain_api_status_includes_attempts_and_error() {
        let since = UNIX_EPOCH + Duration::from_secs(5);
        let status = TuiNockchainApiStatus::Connecting {
            attempt: 2,
            last_error: Some("no route".to_string()),
            since,
        };

        let proto = nockchain_api_status_to_proto(&status);
        assert_eq!(proto.state, ProtoNockchainApiState::Connecting as i32);
        assert_eq!(proto.attempt, Some(2));
        assert_eq!(proto.last_error, Some("no route".to_string()));
        assert_eq!(proto.since_ms, Some(5000));
    }

    #[test]
    fn alerts_snapshot_limits_and_orders_newest_first() {
        let alert_old = Alert {
            id: 1,
            severity: AlertSeverity::Info,
            title: "old".to_string(),
            message: "old".to_string(),
            timestamp: UNIX_EPOCH + Duration::from_secs(1),
            source: "test".to_string(),
        };
        let alert_new = Alert {
            id: 2,
            severity: AlertSeverity::Error,
            title: "new".to_string(),
            message: "new".to_string(),
            timestamp: UNIX_EPOCH + Duration::from_secs(5),
            source: "test".to_string(),
        };

        let mut state = AlertState::new(10);
        state.alerts = VecDeque::from(vec![alert_old.clone(), alert_new.clone()]);

        let snapshot = alerts_snapshot_to_proto(&state, 1);
        assert_eq!(snapshot.alerts.len(), 1);
        assert_eq!(snapshot.alerts[0].id, alert_new.id);
        assert_eq!(
            snapshot.alerts[0].severity,
            ProtoAlertSeverity::Error as i32
        );
    }
}
