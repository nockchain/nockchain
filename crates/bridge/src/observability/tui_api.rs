use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hex::encode as hex_encode;
use tokio::time::{interval, MissedTickBehavior};
use tonic::{Request, Response, Status};

use crate::deposit::log::{DepositLog, DepositLogEntry};
use crate::observability::health::{NodeHealthSnapshot, NodeHealthStatus};
use crate::observability::metrics;
use crate::observability::status::{BridgeStatus, BridgeStatusState, ALERT_HISTORY_CAPACITY};
use crate::observability::tui::types::{
    format_nock_from_nicks, Alert, AlertSeverity, AlertState, BatchStatus as TuiBatchStatus,
    BridgeTx as TuiBridgeTx, ChainState as TuiChainState, DepositLogSnapshot, DepositLogView,
    MetricsState as TuiMetricsState, NockchainApiStatus as TuiNockchainApiStatus,
    Proposal as TuiProposal, ProposalState as TuiProposalState,
    ProposalStatus as TuiProposalStatus, TransactionState as TuiTransactionState,
    TxDirection as TuiTxDirection, TxStatus as TuiTxStatus, WithdrawalCacheSummary,
    WithdrawalFrontierStatus as TuiWithdrawalFrontierStatus, WithdrawalLifecycleCounts,
    WithdrawalLocalSnapshot as TuiWithdrawalLocalSnapshot, WithdrawalQueueRow,
    WithdrawalSequencerSnapshot as TuiWithdrawalSequencerSnapshot,
    WithdrawalStateSnapshot as TuiWithdrawalStateSnapshot, DEPOSIT_LOG_PAGE_SIZE,
};
use crate::shared::config::{NonceEpochConfig, WithdrawalActivationCutoff};
use crate::shared::errors::BridgeError;
use crate::shared::proposer::withdrawal_turn_proposer;
use crate::withdrawal::proposals::{WithdrawalProposalRegistry, WithdrawalTuiRow};
use crate::withdrawal::submission::WithdrawalSequencerPort;

pub mod proto {
    #[cfg(feature = "bazel_build")]
    include!(env!("BRIDGE_TUI_PROTO_RS"));

    #[cfg(not(feature = "bazel_build"))]
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
    TxStatusPending, TxStatusProcessing, WithdrawalCacheSummary as ProtoWithdrawalCacheSummary,
    WithdrawalFrontierStatus as ProtoWithdrawalFrontierStatus,
    WithdrawalLifecycleCounts as ProtoWithdrawalLifecycleCounts,
    WithdrawalLocalSnapshot as ProtoWithdrawalLocalSnapshot,
    WithdrawalQueueRow as ProtoWithdrawalQueueRow,
    WithdrawalSequencerSnapshot as ProtoWithdrawalSequencerSnapshot,
    WithdrawalStateSnapshot as ProtoWithdrawalStateSnapshot,
};

const WITHDRAWAL_TUI_QUEUE_LIMIT: usize = 21;

#[derive(Clone)]
pub struct WithdrawalTuiSource {
    pub registry: Arc<WithdrawalProposalRegistry>,
    pub sequencer: Option<Arc<dyn WithdrawalSequencerPort>>,
    pub activation_cutoff: WithdrawalActivationCutoff,
    pub local_node_id: u64,
    pub node_pkhs: Vec<nockchain_types::tx_engine::common::Hash>,
}

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
        withdrawal_source: Option<WithdrawalTuiSource>,
    ) -> Result<Self, BridgeError> {
        let snapshot_cache = Arc::new(RwLock::new(None));

        match build_cached_snapshot(
            &bridge_status,
            &status_state,
            &deposit_log,
            &nonce_epoch,
            withdrawal_source.as_ref(),
        )
        .await
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
            withdrawal_source.clone(),
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
    withdrawals: ProtoWithdrawalStateSnapshot,
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
            withdrawals: Some(self.withdrawals.clone()),
        }
    }
}

fn spawn_snapshot_refresher(
    snapshot_cache: Arc<RwLock<Option<CachedSnapshot>>>,
    bridge_status: BridgeStatus,
    status_state: BridgeStatusState,
    deposit_log: Arc<DepositLog>,
    nonce_epoch: NonceEpochConfig,
    withdrawal_source: Option<WithdrawalTuiSource>,
) {
    tokio::spawn(async move {
        let mut ticker = interval(SNAPSHOT_REFRESH_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match build_cached_snapshot(
                &bridge_status,
                &status_state,
                &deposit_log,
                &nonce_epoch,
                withdrawal_source.as_ref(),
            )
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
    withdrawal_source: Option<&WithdrawalTuiSource>,
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

    let view = DepositLogView {
        offset: 0,
        limit: SNAPSHOT_CACHE_LIMIT,
    };
    let deposit_log_snapshot = deposit_log.snapshot(nonce_epoch, view).await?;
    let cached_deposit_log = CachedDepositLog::from_snapshot(&deposit_log_snapshot);
    let last_successful_deposit = load_last_successful_deposit(
        bridge_status.last_deposit_nonce(),
        Some(&deposit_log_snapshot),
        deposit_log,
        nonce_epoch,
    )
    .await;

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

    let withdrawals_state = build_withdrawal_state_snapshot(&network, withdrawal_source).await;
    bridge_status.update_withdrawals(withdrawals_state.clone());
    let withdrawals_proto = withdrawal_state_to_proto(&withdrawals_state);

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
        withdrawals: withdrawals_proto,
    };
    metrics
        .tui_snapshot_build_cache_time
        .add_timing(&build_started.elapsed());
    Ok(snapshot)
}

async fn load_last_successful_deposit(
    status_nonce: Option<u64>,
    log_snapshot: Option<&DepositLogSnapshot>,
    deposit_log: &DepositLog,
    nonce_epoch: &NonceEpochConfig,
) -> Option<SuccessfulDeposit> {
    for nonce in last_successful_deposit_nonce_candidates(status_nonce, log_snapshot) {
        match deposit_log.get_by_nonce(nonce, nonce_epoch).await {
            Ok(Some(entry)) => return Some(successful_deposit_from_log_entry(nonce, entry)),
            Ok(None) => {
                tracing::warn!(
                    target: "bridge.tui",
                    nonce,
                    "last successful deposit nonce was not present in local log"
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "bridge.tui",
                    error=%err,
                    nonce,
                    "failed to load last successful deposit from log"
                );
            }
        }
    }
    None
}

fn last_successful_deposit_nonce_candidates(
    status_nonce: Option<u64>,
    log_snapshot: Option<&DepositLogSnapshot>,
) -> Vec<u64> {
    let log_nonce = log_snapshot.and_then(|snapshot| {
        if snapshot.total_count == 0 {
            None
        } else {
            Some(
                snapshot
                    .first_epoch_nonce
                    .saturating_add(snapshot.total_count.saturating_sub(1)),
            )
        }
    });

    let mut candidates = Vec::with_capacity(2);
    if let Some(nonce) = status_nonce {
        candidates.push(nonce);
    }
    if let Some(nonce) = log_nonce {
        candidates.push(nonce);
    }
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.dedup();
    candidates
}

fn successful_deposit_from_log_entry(nonce: u64, entry: DepositLogEntry) -> SuccessfulDeposit {
    SuccessfulDeposit {
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
    }
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

fn network_state_to_proto(state: &crate::observability::tui::types::NetworkState) -> NetworkState {
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

async fn build_withdrawal_state_snapshot(
    network: &crate::observability::tui::types::NetworkState,
    source: Option<&WithdrawalTuiSource>,
) -> TuiWithdrawalStateSnapshot {
    let current_base_height = network.base.last_updated.map(|_| network.base.height);
    let current_base_next_height = network.base_next_height;
    let current_nock_next_height = network.nock_next_height;
    let Some(source) = source else {
        return TuiWithdrawalStateSnapshot {
            local: TuiWithdrawalLocalSnapshot {
                current_base_height,
                current_base_next_height,
                current_nock_next_height,
                ..TuiWithdrawalLocalSnapshot::default()
            },
            ..TuiWithdrawalStateSnapshot::default()
        };
    };

    let activation_ready = current_nock_next_height
        .map(|nock_next| nock_next >= source.activation_cutoff.nock_next_height);

    let (cache, cache_error) = match source.registry.cache_summary() {
        Ok(summary) => (
            WithdrawalCacheSummary {
                proposal_count: summary.proposal_count,
                signature_count: summary.signature_count,
            },
            None,
        ),
        Err(err) => (WithdrawalCacheSummary::default(), Some(err.to_string())),
    };

    let mut snapshot = TuiWithdrawalStateSnapshot {
        local: TuiWithdrawalLocalSnapshot {
            activation_ready,
            activation_base_next_height: None,
            activation_nock_next_height: Some(source.activation_cutoff.nock_next_height),
            current_base_next_height,
            current_nock_next_height,
            current_base_height,
            cache,
            last_error: cache_error,
            ..TuiWithdrawalLocalSnapshot::default()
        },
        ..TuiWithdrawalStateSnapshot::default()
    };

    let frontier_nonce = match source.sequencer.as_ref() {
        Some(sequencer) => match sequencer.current_live_withdrawal_nonce().await {
            Ok(Some(nonce)) => {
                snapshot.sequencer.frontier_status = TuiWithdrawalFrontierStatus::Present;
                snapshot.sequencer.frontier_nonce = Some(nonce);
                Some(nonce)
            }
            Ok(None) => {
                snapshot.sequencer.frontier_status = TuiWithdrawalFrontierStatus::None;
                None
            }
            Err(err) => {
                snapshot.sequencer.frontier_status = TuiWithdrawalFrontierStatus::Unknown;
                snapshot.sequencer.last_error = Some(err.to_string());
                None
            }
        },
        None => None,
    };

    match source.registry.load_tui_counts(frontier_nonce).await {
        Ok(counts) => {
            snapshot.local.lifecycle = WithdrawalLifecycleCounts {
                total_count: counts.total_count,
                live_count: counts.live_count,
                ordering_blocking_count: counts.ordering_blocking_count,
                pending_count: counts.pending_count,
                assembling_count: counts.assembling_count,
                prepared_count: counts.prepared_count,
                peer_canonical_count: counts.peer_canonical_count,
                authorized_count: counts.authorized_count,
                mempool_accepted_count: counts.mempool_accepted_count,
                confirmed_count: counts.confirmed_count,
                below_frontier_count: counts.below_frontier_count,
                above_frontier_count: counts.above_frontier_count,
            };
        }
        Err(err) => {
            snapshot.local.last_error = Some(err.to_string());
        }
    }

    match source
        .registry
        .load_tui_rows_around_nonce(frontier_nonce, WITHDRAWAL_TUI_QUEUE_LIMIT)
        .await
    {
        Ok(rows) => {
            snapshot.local.queue = rows
                .into_iter()
                .map(|row| {
                    withdrawal_tui_row_to_snapshot_row(
                        &row, source.local_node_id, &source.node_pkhs, None,
                    )
                })
                .collect();
        }
        Err(err) => {
            snapshot.local.last_error = Some(err.to_string());
        }
    }

    if let Some(frontier_nonce) = frontier_nonce {
        match source.registry.fetch_tui_row_by_nonce(frontier_nonce).await {
            Ok(Some(row)) => {
                let handoff = match source.sequencer.as_ref() {
                    Some(sequencer) => {
                        match sequencer.get_sequenced_withdrawal_status(&row.id).await {
                            Ok(status) if status.found => {
                                snapshot.sequencer.frontier_state = Some(status.state.clone());
                                snapshot.sequencer.frontier_epoch = Some(status.current_epoch);
                                snapshot.sequencer.current_confirmed_base_height =
                                    status.current_confirmed_base_height;
                                snapshot.sequencer.handoff_window_blocks =
                                    Some(status.handoff_window_blocks);
                                snapshot.sequencer.turn_started_base_height =
                                    status.turn_started_base_height;
                                snapshot.sequencer.handoff_index = status.handoff_index;
                                snapshot.sequencer.blocks_until_handoff =
                                    status.blocks_until_handoff;
                                snapshot.sequencer.current_responsible_node =
                                    (!source.node_pkhs.is_empty()).then(|| {
                                        withdrawal_turn_proposer(
                                            &row.id, status.current_epoch, status.handoff_index,
                                            &source.node_pkhs,
                                        ) as u64
                                    });
                                snapshot.sequencer.is_my_turn =
                                    snapshot.sequencer.current_responsible_node
                                        == Some(source.local_node_id);
                                Some(WithdrawalSnapshotHandoff {
                                    handoff_index: status.handoff_index,
                                    turn_started_base_height: status.turn_started_base_height,
                                    blocks_until_handoff: status.blocks_until_handoff,
                                })
                            }
                            Ok(status) => {
                                snapshot.sequencer.current_confirmed_base_height =
                                    status.current_confirmed_base_height;
                                snapshot.sequencer.handoff_window_blocks =
                                    Some(status.handoff_window_blocks);
                                None
                            }
                            Err(err) => {
                                snapshot.sequencer.last_error = Some(err.to_string());
                                None
                            }
                        }
                    }
                    None => None,
                };
                snapshot.local.frontier_row = Some(withdrawal_tui_row_to_snapshot_row(
                    &row,
                    source.local_node_id,
                    &source.node_pkhs,
                    handoff.as_ref(),
                ));
            }
            Ok(None) => {}
            Err(err) => {
                snapshot.local.last_error = Some(err.to_string());
            }
        }
    }

    update_withdrawal_snapshot_metrics(&snapshot);
    snapshot
}

fn update_withdrawal_snapshot_metrics(snapshot: &TuiWithdrawalStateSnapshot) {
    let metrics = metrics::init_metrics();
    let local = &snapshot.local;
    let sequencer = &snapshot.sequencer;
    metrics.withdrawal_frontier_present.swap(matches!(
        sequencer.frontier_status,
        TuiWithdrawalFrontierStatus::Present
    ) as u8 as f64);
    metrics
        .withdrawal_frontier_nonce
        .swap(sequencer.frontier_nonce.unwrap_or_default() as f64);
    metrics
        .withdrawal_frontier_local_row_present
        .swap(if local.frontier_row.is_some() {
            1.0
        } else {
            0.0
        });
    metrics.withdrawal_frontier_local_state.swap(
        local
            .frontier_row
            .as_ref()
            .map(|row| withdrawal_state_metric(&row.state))
            .unwrap_or_default() as f64,
    );
    metrics
        .withdrawal_lifecycle_total
        .swap(local.lifecycle.total_count as f64);
    metrics
        .withdrawal_lifecycle_live
        .swap(local.lifecycle.live_count as f64);
    metrics
        .withdrawal_lifecycle_ordering_blocking
        .swap(local.lifecycle.ordering_blocking_count as f64);
    metrics
        .withdrawal_lifecycle_pending
        .swap(local.lifecycle.pending_count as f64);
    metrics
        .withdrawal_lifecycle_assembling
        .swap(local.lifecycle.assembling_count as f64);
    metrics
        .withdrawal_lifecycle_prepared
        .swap(local.lifecycle.prepared_count as f64);
    metrics
        .withdrawal_lifecycle_peer_canonical
        .swap(local.lifecycle.peer_canonical_count as f64);
    metrics
        .withdrawal_lifecycle_authorized
        .swap(local.lifecycle.authorized_count as f64);
    metrics
        .withdrawal_lifecycle_mempool_accepted
        .swap(local.lifecycle.mempool_accepted_count as f64);
    metrics
        .withdrawal_lifecycle_confirmed
        .swap(local.lifecycle.confirmed_count as f64);
    metrics
        .withdrawal_lifecycle_below_frontier
        .swap(local.lifecycle.below_frontier_count as f64);
    metrics
        .withdrawal_lifecycle_above_frontier
        .swap(local.lifecycle.above_frontier_count as f64);
    metrics
        .withdrawal_proposal_cache_proposals
        .swap(local.cache.proposal_count as f64);
    metrics
        .withdrawal_proposal_cache_signatures
        .swap(local.cache.signature_count as f64);
    if let Some(ready) = local.activation_ready {
        metrics
            .withdrawal_activation_ready
            .swap(if ready { 1.0 } else { 0.0 });
        metrics
            .withdrawal_activation_waiting
            .swap(if ready { 0.0 } else { 1.0 });
    }
    metrics
        .withdrawal_activation_nock_next_height
        .swap(local.activation_nock_next_height.unwrap_or_default() as f64);
}

fn withdrawal_state_metric(state: &str) -> u64 {
    match state {
        "pending" => 1,
        "assembling" => 2,
        "prepared" => 3,
        "peer_canonical" => 4,
        "authorized" => 5,
        "mempool_accepted" => 6,
        "confirmed" => 7,
        _ => 0,
    }
}

struct WithdrawalSnapshotHandoff {
    handoff_index: u64,
    turn_started_base_height: Option<u64>,
    blocks_until_handoff: Option<u64>,
}

fn withdrawal_tui_row_to_snapshot_row(
    row: &WithdrawalTuiRow,
    local_node_id: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
    handoff: Option<&WithdrawalSnapshotHandoff>,
) -> WithdrawalQueueRow {
    let handoff_index = handoff
        .map(|handoff| handoff.handoff_index)
        .unwrap_or_default();
    let current_responsible_node = handoff.and_then(|handoff| {
        (!node_pkhs.is_empty()).then(|| {
            withdrawal_turn_proposer(&row.id, row.current_epoch, handoff.handoff_index, node_pkhs)
                as u64
        })
    });
    WithdrawalQueueRow {
        nonce: row.withdrawal_nonce,
        id: format!(
            "{}:{}",
            row.id.as_of.to_base58_string(),
            hex_encode(&row.id.base_event_id.0)
        ),
        state: row.state.as_str().to_string(),
        epoch: row.current_epoch,
        amount: row.amount,
        recipient: row.recipient.as_ref().map(Tip5HashExt::to_base58_string),
        base_batch_end: row.base_batch_end,
        proposal_hash: row.proposal_hash.clone(),
        has_commit_certificate: row.has_commit_certificate,
        has_authorized_transaction: row.has_authorized_transaction,
        has_submitted_transaction: row.has_submitted_transaction,
        turn_started_base_height: handoff
            .and_then(|handoff| handoff.turn_started_base_height)
            .or(row.turn_started_base_height),
        handoff_index,
        current_responsible_node,
        is_my_turn: current_responsible_node == Some(local_node_id),
        blocks_until_handoff: handoff.and_then(|handoff| handoff.blocks_until_handoff),
        blocks_until_retry: None,
        updated_at_secs: u64::try_from(row.updated_at).ok(),
    }
}

trait Tip5HashExt {
    fn to_base58_string(&self) -> String;
}

impl Tip5HashExt for crate::shared::types::Tip5Hash {
    fn to_base58_string(&self) -> String {
        self.to_base58()
    }
}

fn withdrawal_state_to_proto(
    snapshot: &TuiWithdrawalStateSnapshot,
) -> ProtoWithdrawalStateSnapshot {
    ProtoWithdrawalStateSnapshot {
        local: Some(withdrawal_local_snapshot_to_proto(&snapshot.local)),
        sequencer: Some(withdrawal_sequencer_snapshot_to_proto(&snapshot.sequencer)),
    }
}

fn withdrawal_local_snapshot_to_proto(
    snapshot: &TuiWithdrawalLocalSnapshot,
) -> ProtoWithdrawalLocalSnapshot {
    ProtoWithdrawalLocalSnapshot {
        activation_ready: snapshot.activation_ready,
        activation_base_next_height: snapshot.activation_base_next_height,
        activation_nock_next_height: snapshot.activation_nock_next_height,
        current_base_next_height: snapshot.current_base_next_height,
        current_nock_next_height: snapshot.current_nock_next_height,
        current_base_height: snapshot.current_base_height,
        frontier_row: snapshot
            .frontier_row
            .as_ref()
            .map(withdrawal_queue_row_to_proto),
        queue: snapshot
            .queue
            .iter()
            .map(withdrawal_queue_row_to_proto)
            .collect(),
        cache: Some(ProtoWithdrawalCacheSummary {
            proposal_count: snapshot.cache.proposal_count,
            signature_count: snapshot.cache.signature_count,
        }),
        lifecycle: Some(withdrawal_lifecycle_counts_to_proto(&snapshot.lifecycle)),
        last_error: snapshot.last_error.clone(),
    }
}

fn withdrawal_sequencer_snapshot_to_proto(
    snapshot: &TuiWithdrawalSequencerSnapshot,
) -> ProtoWithdrawalSequencerSnapshot {
    ProtoWithdrawalSequencerSnapshot {
        frontier_status: withdrawal_frontier_status_to_proto(snapshot.frontier_status.clone())
            as i32,
        frontier_nonce: snapshot.frontier_nonce,
        frontier_state: snapshot.frontier_state.clone(),
        frontier_epoch: snapshot.frontier_epoch,
        current_confirmed_base_height: snapshot.current_confirmed_base_height,
        handoff_window_blocks: snapshot.handoff_window_blocks,
        turn_started_base_height: snapshot.turn_started_base_height,
        handoff_index: snapshot.handoff_index,
        current_responsible_node: snapshot.current_responsible_node,
        is_my_turn: snapshot.is_my_turn,
        blocks_until_handoff: snapshot.blocks_until_handoff,
        last_error: snapshot.last_error.clone(),
    }
}

fn withdrawal_frontier_status_to_proto(
    status: TuiWithdrawalFrontierStatus,
) -> ProtoWithdrawalFrontierStatus {
    match status {
        TuiWithdrawalFrontierStatus::Unknown => ProtoWithdrawalFrontierStatus::Unknown,
        TuiWithdrawalFrontierStatus::None => ProtoWithdrawalFrontierStatus::None,
        TuiWithdrawalFrontierStatus::Present => ProtoWithdrawalFrontierStatus::Present,
    }
}

fn withdrawal_queue_row_to_proto(row: &WithdrawalQueueRow) -> ProtoWithdrawalQueueRow {
    ProtoWithdrawalQueueRow {
        nonce: row.nonce,
        id: row.id.clone(),
        state: row.state.clone(),
        epoch: row.epoch,
        amount: row.amount,
        recipient: row.recipient.clone(),
        base_batch_end: row.base_batch_end,
        proposal_hash: row.proposal_hash.clone(),
        has_commit_certificate: row.has_commit_certificate,
        has_authorized_transaction: row.has_authorized_transaction,
        has_submitted_transaction: row.has_submitted_transaction,
        turn_started_base_height: row.turn_started_base_height,
        handoff_index: row.handoff_index,
        current_responsible_node: row.current_responsible_node,
        is_my_turn: row.is_my_turn,
        blocks_until_handoff: row.blocks_until_handoff,
        blocks_until_retry: row.blocks_until_retry,
        updated_at_secs: row.updated_at_secs,
    }
}

fn withdrawal_lifecycle_counts_to_proto(
    counts: &WithdrawalLifecycleCounts,
) -> ProtoWithdrawalLifecycleCounts {
    ProtoWithdrawalLifecycleCounts {
        total_count: counts.total_count,
        live_count: counts.live_count,
        ordering_blocking_count: counts.ordering_blocking_count,
        pending_count: counts.pending_count,
        assembling_count: counts.assembling_count,
        prepared_count: counts.prepared_count,
        peer_canonical_count: counts.peer_canonical_count,
        authorized_count: counts.authorized_count,
        mempool_accepted_count: counts.mempool_accepted_count,
        confirmed_count: counts.confirmed_count,
        below_frontier_count: counts.below_frontier_count,
        above_frontier_count: counts.above_frontier_count,
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

fn last_submitted_deposit_to_proto(
    entry: crate::observability::status::LastSubmittedDeposit,
) -> LastDeposit {
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

    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash as Tip5Hash;

    use super::*;
    use crate::shared::types::AtomBytes;
    use crate::withdrawal::state::WithdrawalState;
    use crate::withdrawal::types::WithdrawalId;

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
        let nicks_per_nock = crate::observability::tui::types::NICKS_PER_NOCK;
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
    fn last_successful_deposit_candidates_include_durable_log_nonce() {
        let snapshot = DepositLogSnapshot {
            total_count: 3,
            first_epoch_nonce: 10,
            rows: Vec::new(),
        };

        assert_eq!(
            last_successful_deposit_nonce_candidates(None, Some(&snapshot)),
            vec![12]
        );
        assert_eq!(
            last_successful_deposit_nonce_candidates(Some(11), Some(&snapshot)),
            vec![12, 11]
        );
        assert_eq!(
            last_successful_deposit_nonce_candidates(Some(12), Some(&snapshot)),
            vec![12]
        );
    }

    fn sample_withdrawal_tui_row(withdrawal_nonce: u64) -> WithdrawalTuiRow {
        WithdrawalTuiRow {
            id: WithdrawalId {
                as_of: crate::shared::types::Tip5Hash([
                    Belt(11),
                    Belt(12),
                    Belt(13),
                    Belt(14),
                    Belt(15),
                ]),
                base_event_id: AtomBytes(vec![0xaa; 32]),
            },
            recipient: Some(crate::shared::types::Tip5Hash([
                Belt(21),
                Belt(22),
                Belt(23),
                Belt(24),
                Belt(25),
            ])),
            amount: Some(42),
            base_batch_end: Some(100),
            withdrawal_nonce,
            current_epoch: 2,
            proposal_hash: None,
            has_commit_certificate: false,
            has_authorized_transaction: false,
            has_submitted_transaction: false,
            turn_started_base_height: Some(50),
            state: WithdrawalState::Pending,
            updated_at: 5,
        }
    }

    #[test]
    fn withdrawal_queue_snapshot_rows_are_passive_without_sequencer_status() {
        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let row = sample_withdrawal_tui_row(3);
        let snapshot_row = withdrawal_tui_row_to_snapshot_row(&row, 0, &node_pkhs, None);

        assert_eq!(snapshot_row.handoff_index, 0);
        assert_eq!(snapshot_row.current_responsible_node, None);
        assert!(!snapshot_row.is_my_turn);
        assert_eq!(snapshot_row.blocks_until_handoff, None);
    }

    #[test]
    fn withdrawal_frontier_snapshot_row_uses_sequencer_handoff_status() {
        let node_pkhs = vec![
            Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Tip5Hash([Belt(6), Belt(7), Belt(8), Belt(9), Belt(10)]),
        ];
        let row = sample_withdrawal_tui_row(3);
        let handoff = WithdrawalSnapshotHandoff {
            handoff_index: 1,
            turn_started_base_height: Some(60),
            blocks_until_handoff: Some(7),
        };
        let expected_node = withdrawal_turn_proposer(
            &row.id, row.current_epoch, handoff.handoff_index, &node_pkhs,
        ) as u64;
        let snapshot_row =
            withdrawal_tui_row_to_snapshot_row(&row, expected_node, &node_pkhs, Some(&handoff));

        assert_eq!(snapshot_row.handoff_index, 1);
        assert_eq!(snapshot_row.current_responsible_node, Some(expected_node));
        assert!(snapshot_row.is_my_turn);
        assert_eq!(snapshot_row.turn_started_base_height, Some(60));
        assert_eq!(snapshot_row.blocks_until_handoff, Some(7));
    }

    #[test]
    fn withdrawal_snapshot_proto_preserves_fields() {
        let row = WithdrawalQueueRow {
            nonce: 7,
            id: "withdrawal".to_string(),
            state: "authorized".to_string(),
            epoch: 2,
            amount: Some(123),
            recipient: Some("recipient".to_string()),
            base_batch_end: Some(55),
            proposal_hash: Some("proposal".to_string()),
            has_commit_certificate: true,
            has_authorized_transaction: true,
            has_submitted_transaction: false,
            turn_started_base_height: Some(50),
            handoff_index: 1,
            current_responsible_node: Some(3),
            is_my_turn: true,
            blocks_until_handoff: Some(4),
            blocks_until_retry: Some(5),
            updated_at_secs: Some(6),
        };
        let snapshot = TuiWithdrawalStateSnapshot {
            local: TuiWithdrawalLocalSnapshot {
                activation_ready: Some(true),
                activation_base_next_height: None,
                activation_nock_next_height: Some(20),
                current_base_next_height: Some(11),
                current_nock_next_height: Some(21),
                current_base_height: Some(9),
                frontier_row: Some(row.clone()),
                queue: vec![row],
                cache: WithdrawalCacheSummary {
                    proposal_count: 2,
                    signature_count: 3,
                },
                lifecycle: WithdrawalLifecycleCounts {
                    total_count: 8,
                    live_count: 6,
                    ordering_blocking_count: 4,
                    pending_count: 1,
                    assembling_count: 1,
                    prepared_count: 1,
                    peer_canonical_count: 1,
                    authorized_count: 0,
                    mempool_accepted_count: 2,
                    confirmed_count: 2,
                    below_frontier_count: 1,
                    above_frontier_count: 3,
                },
                last_error: Some("local err".to_string()),
            },
            sequencer: TuiWithdrawalSequencerSnapshot {
                frontier_status: TuiWithdrawalFrontierStatus::Present,
                frontier_nonce: Some(7),
                frontier_state: Some("authorized".to_string()),
                frontier_epoch: Some(2),
                current_confirmed_base_height: Some(99),
                handoff_window_blocks: Some(100),
                turn_started_base_height: Some(50),
                handoff_index: 1,
                current_responsible_node: Some(3),
                is_my_turn: true,
                blocks_until_handoff: Some(4),
                last_error: Some("sequencer err".to_string()),
            },
        };

        let proto = withdrawal_state_to_proto(&snapshot);
        let local = proto.local.as_ref().expect("local snapshot");
        let sequencer = proto.sequencer.as_ref().expect("sequencer snapshot");
        assert_eq!(local.activation_ready, Some(true));
        assert_eq!(sequencer.frontier_nonce, Some(7));
        assert_eq!(
            sequencer.frontier_status,
            ProtoWithdrawalFrontierStatus::Present as i32
        );
        assert_eq!(sequencer.frontier_state, Some("authorized".to_string()));
        assert_eq!(sequencer.current_confirmed_base_height, Some(99));
        assert_eq!(sequencer.handoff_window_blocks, Some(100));
        assert_eq!(sequencer.blocks_until_handoff, Some(4));
        assert_eq!(local.frontier_row.as_ref().map(|row| row.nonce), Some(7));
        assert_eq!(local.queue.len(), 1);
        assert_eq!(
            local.cache.as_ref().map(|cache| cache.signature_count),
            Some(3)
        );
        assert_eq!(
            local
                .lifecycle
                .as_ref()
                .map(|counts| counts.mempool_accepted_count),
            Some(2)
        );
        assert_eq!(
            local
                .lifecycle
                .as_ref()
                .map(|counts| counts.confirmed_count),
            Some(2)
        );
        assert_eq!(
            local
                .lifecycle
                .as_ref()
                .map(|counts| counts.ordering_blocking_count),
            Some(4)
        );
        assert_eq!(local.last_error, Some("local err".to_string()));
        assert_eq!(sequencer.last_error, Some("sequencer err".to_string()));
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
