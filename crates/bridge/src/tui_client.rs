use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::{interval, MissedTickBehavior};
use tonic::Request;
use tracing::{debug, info, warn};

use crate::bridge_status::ALERT_HISTORY_CAPACITY;
use crate::health::{NodeHealthSnapshot, NodeHealthStatus};
use crate::tui::state::TuiStatus;
use crate::tui::types::{
    Alert, AlertSeverity, AlertState, BatchStatus, BridgeTx, ChainState, DepositLogRow,
    DepositLogSnapshot, DepositLogView, NetworkState, NockchainApiStatus, Proposal, ProposalState,
    ProposalStatus, TransactionState, TxDirection, TxStatus, NOCK_BASE_PER_NICK, NOCK_BASE_UNIT,
};
use crate::tui_api::proto::bridge_tui_client::BridgeTuiClient as GrpcBridgeTuiClient;

pub mod proto {
    pub use crate::tui_api::proto::*;
}

const SNAPSHOT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
static PROPOSAL_AMOUNT_PARSE_WARNED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    NeverConnected,
    Connected,
    Disconnected,
    Reconnecting,
}

#[derive(Clone, Debug)]
pub struct BridgeTuiSnapshot {
    pub running_state: proto::RunningState,
    pub nock_hold: bool,
    pub base_hold: bool,
    pub nock_hold_height: Option<u64>,
    pub base_hold_height: Option<u64>,
    pub network_state: NetworkState,
    pub deposit_log: DepositLogSnapshot,
    pub proposals: ProposalState,
    pub transactions: TransactionState,
    pub alerts: AlertState,
    pub peer_statuses: Vec<NodeHealthSnapshot>,
    pub last_submitted_deposit: Option<proto::LastDeposit>,
    pub last_successful_deposit: Option<proto::SuccessfulDeposit>,
}

impl BridgeTuiSnapshot {
    pub fn from_proto(response: proto::GetSnapshotResponse) -> Self {
        let mut network_state = response
            .network_state
            .map(network_state_from_proto)
            .unwrap_or_default();

        network_state.base_hold = response.base_hold;
        network_state.nock_hold = response.nock_hold;
        network_state.base_hold_height = response.base_hold_height;
        network_state.nock_hold_height = response.nock_hold_height;
        network_state.kernel_stopped =
            response.running_state == proto::RunningState::Stopped as i32;

        BridgeTuiSnapshot {
            running_state: proto::RunningState::try_from(response.running_state)
                .unwrap_or(proto::RunningState::Unspecified),
            nock_hold: response.nock_hold,
            base_hold: response.base_hold,
            nock_hold_height: response.nock_hold_height,
            base_hold_height: response.base_hold_height,
            network_state,
            deposit_log: response
                .deposit_log
                .map(deposit_log_snapshot_from_proto)
                .unwrap_or_default(),
            proposals: response
                .proposals
                .map(proposal_state_from_proto)
                .unwrap_or_default(),
            transactions: response
                .transactions
                .map(transaction_state_from_proto)
                .unwrap_or_else(default_transaction_state),
            alerts: response
                .alerts
                .map(alert_state_from_proto)
                .unwrap_or_default(),
            peer_statuses: response
                .peer_statuses
                .into_iter()
                .map(peer_status_from_proto)
                .collect(),
            last_submitted_deposit: response.last_submitted_deposit,
            last_successful_deposit: response.last_successful_deposit,
        }
    }

    pub fn apply_to(self, status: &TuiStatus) {
        status.update_network(self.network_state);
        if let Ok(mut guard) = status.proposals.write() {
            *guard = self.proposals;
        }
        if let Ok(mut guard) = status.transactions.write() {
            *guard = self.transactions;
        }
        if let Ok(mut guard) = status.alerts.write() {
            *guard = self.alerts;
        }
        if let Ok(mut guard) = status.health.write() {
            *guard = self.peer_statuses;
        }
        status.update_deposit_log_snapshot(self.deposit_log);
        status.set_last_deposit_nonce(self.last_successful_deposit.map(|deposit| deposit.nonce));
    }
}

pub struct BridgeTuiClient {
    server_uri: String,
    client: Option<GrpcBridgeTuiClient<tonic::transport::Channel>>,
    connection_status: ConnectionStatus,
    last_successful_connection: Option<Instant>,
    last_connection_attempt: Instant,
    last_connection_error: Option<String>,
    poll_interval: Duration,
}

impl BridgeTuiClient {
    pub async fn new(server_uri: String) -> Self {
        let mut client = Self {
            server_uri,
            client: None,
            connection_status: ConnectionStatus::NeverConnected,
            last_successful_connection: None,
            last_connection_attempt: Instant::now(),
            last_connection_error: None,
            poll_interval: SNAPSHOT_POLL_INTERVAL,
        };
        client.connect_initial().await;
        client
    }

    pub async fn run(mut self, shared: TuiStatus, shutdown: Arc<AtomicBool>) {
        let mut ticker = interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            ticker.tick().await;

            if self.should_retry_connection() {
                let _ = self.attempt_reconnect().await;
            }

            if self.connection_status != ConnectionStatus::Connected {
                continue;
            }

            self.poll_snapshot(&shared).await;
        }
    }

    async fn connect_initial(&mut self) {
        match GrpcBridgeTuiClient::connect(self.server_uri.clone()).await {
            Ok(client) => {
                info!("TUI connected to {}", self.server_uri);
                self.client = Some(client);
                self.connection_status = ConnectionStatus::Connected;
                self.last_successful_connection = Some(Instant::now());
                self.last_connection_error = None;
            }
            Err(err) => {
                warn!("Initial TUI gRPC connection failed (will retry): {}", err);
                self.connection_status = ConnectionStatus::NeverConnected;
                self.last_connection_error = Some(err.to_string());
            }
        }
    }

    async fn poll_snapshot(&mut self, shared: &TuiStatus) {
        let Some(client) = self.client.as_mut() else {
            return;
        };

        let view = shared.deposit_log_view();
        let request = snapshot_request_with_alert_limit(view, ALERT_HISTORY_CAPACITY);

        match client.get_snapshot(Request::new(request)).await {
            Ok(response) => {
                if self.connection_status != ConnectionStatus::Connected {
                    self.connection_status = ConnectionStatus::Connected;
                    self.last_successful_connection = Some(Instant::now());
                    self.last_connection_error = None;
                    info!("TUI reconnected to {}", self.server_uri);
                }

                let snapshot = BridgeTuiSnapshot::from_proto(response.into_inner());
                snapshot.apply_to(shared);
            }
            Err(err) => {
                warn!("TUI snapshot poll failed: {}", err);
                self.connection_status = ConnectionStatus::Disconnected;
                self.last_connection_error = Some(err.to_string());
                self.client = None;
            }
        }
    }

    async fn attempt_reconnect(&mut self) -> Result<(), tonic::transport::Error> {
        self.last_connection_attempt = Instant::now();
        self.connection_status = ConnectionStatus::Reconnecting;

        match GrpcBridgeTuiClient::connect(self.server_uri.clone()).await {
            Ok(client) => {
                self.client = Some(client);
                self.connection_status = ConnectionStatus::Connected;
                self.last_successful_connection = Some(Instant::now());
                self.last_connection_error = None;
                info!("TUI reconnected to {}", self.server_uri);
                Ok(())
            }
            Err(err) => {
                self.connection_status = if self.last_successful_connection.is_some() {
                    ConnectionStatus::Disconnected
                } else {
                    ConnectionStatus::NeverConnected
                };
                self.last_connection_error = Some(err.to_string());
                debug!("TUI reconnection failed: {}", err);
                Err(err)
            }
        }
    }

    fn should_retry_connection(&self) -> bool {
        matches!(
            self.connection_status,
            ConnectionStatus::NeverConnected | ConnectionStatus::Disconnected
        ) && self.last_connection_attempt.elapsed() >= RECONNECT_INTERVAL
    }
}

pub fn snapshot_request_from_view(view: DepositLogView) -> proto::GetSnapshotRequest {
    proto::GetSnapshotRequest {
        deposit_log_view: Some(deposit_log_view_to_proto(view)),
        alert_view: None,
    }
}

pub fn snapshot_request_with_alert_limit(
    view: DepositLogView,
    alert_limit: usize,
) -> proto::GetSnapshotRequest {
    proto::GetSnapshotRequest {
        deposit_log_view: Some(deposit_log_view_to_proto(view)),
        alert_view: Some(proto::AlertView {
            limit: u32::try_from(alert_limit).unwrap_or(u32::MAX),
        }),
    }
}

pub fn deposit_log_view_to_proto(view: DepositLogView) -> proto::DepositLogView {
    proto::DepositLogView {
        offset: u64::try_from(view.offset).unwrap_or(u64::MAX),
        limit: u64::try_from(view.limit).unwrap_or(u64::MAX),
    }
}

fn deposit_log_snapshot_from_proto(snapshot: proto::DepositLogSnapshot) -> DepositLogSnapshot {
    DepositLogSnapshot {
        total_count: snapshot.total_count,
        first_epoch_nonce: snapshot.first_epoch_nonce,
        rows: snapshot
            .rows
            .into_iter()
            .map(|row| DepositLogRow {
                nonce: row.nonce,
                block_height: row.block_height,
                tx_id_base58: row.tx_id_base58,
                recipient_hex: row.recipient_hex,
                amount: row.amount,
            })
            .collect(),
    }
}

fn network_state_from_proto(state: proto::NetworkState) -> NetworkState {
    NetworkState {
        base: state.base.map(chain_state_from_proto).unwrap_or_default(),
        nockchain: state
            .nockchain
            .map(chain_state_from_proto)
            .unwrap_or_default(),
        base_next_height: state.base_next_height,
        nock_next_height: state.nock_next_height,
        nockchain_api_status: state
            .nockchain_api_status
            .map(nockchain_api_status_from_proto)
            .unwrap_or_default(),
        pending_deposits: state.pending_deposits,
        pending_withdrawals: state.pending_withdrawals,
        batch_status: state
            .batch_status
            .map(batch_status_from_proto)
            .unwrap_or(BatchStatus::Idle),
        degradation_warning: state.degradation_warning,
        unsettled_deposit_count: state.unsettled_deposit_count,
        unsettled_withdrawal_count: state.unsettled_withdrawal_count,
        base_hold: false,
        nock_hold: false,
        kernel_stopped: false,
        base_hold_height: None,
        nock_hold_height: None,
        is_mainnet: state.is_mainnet,
    }
}

fn chain_state_from_proto(state: proto::ChainState) -> ChainState {
    ChainState {
        height: state.height,
        tip_hash: state.tip_hash,
        confirmations: state.confirmations,
        is_syncing: state.is_syncing,
        last_updated: state.last_updated_ms.map(system_time_from_millis),
    }
}

fn nockchain_api_status_from_proto(status: proto::NockchainApiStatus) -> NockchainApiStatus {
    let since = status
        .since_ms
        .map(system_time_from_millis)
        .unwrap_or_else(SystemTime::now);
    let attempt = status.attempt.unwrap_or(0);

    match proto::nockchain_api_status::State::try_from(status.state)
        .unwrap_or(proto::nockchain_api_status::State::Unspecified)
    {
        proto::nockchain_api_status::State::Connected => NockchainApiStatus::Connected { since },
        proto::nockchain_api_status::State::Connecting => NockchainApiStatus::Connecting {
            attempt,
            last_error: status.last_error,
            since,
        },
        proto::nockchain_api_status::State::Disconnected => NockchainApiStatus::Disconnected {
            since,
            error: status
                .last_error
                .unwrap_or_else(|| "disconnected".to_string()),
        },
        proto::nockchain_api_status::State::Unspecified => NockchainApiStatus::default(),
    }
}

fn batch_status_from_proto(status: proto::BatchStatus) -> BatchStatus {
    match status.status {
        Some(proto::batch_status::Status::Idle(_)) => BatchStatus::Idle,
        Some(proto::batch_status::Status::Processing(processing)) => BatchStatus::Processing {
            batch_id: processing.batch_id,
            progress_pct: u8::try_from(processing.progress_pct).unwrap_or(u8::MAX),
        },
        Some(proto::batch_status::Status::AwaitingSignatures(awaiting)) => {
            BatchStatus::AwaitingSignatures {
                batch_id: awaiting.batch_id,
                collected: u8::try_from(awaiting.collected).unwrap_or(u8::MAX),
                required: u8::try_from(awaiting.required).unwrap_or(u8::MAX),
            }
        }
        Some(proto::batch_status::Status::Submitting(submitting)) => BatchStatus::Submitting {
            batch_id: submitting.batch_id,
        },
        None => BatchStatus::Idle,
    }
}

fn proposal_state_from_proto(state: proto::ProposalState) -> ProposalState {
    let max_history = crate::bridge_status::PROPOSAL_HISTORY_CAPACITY;
    ProposalState {
        last_submitted: state.last_submitted.map(proposal_from_proto),
        pending_inbound: state
            .pending_inbound
            .into_iter()
            .map(proposal_from_proto)
            .collect(),
        history: state
            .history
            .into_iter()
            .map(proposal_from_proto)
            .collect::<VecDeque<_>>(),
        max_history,
    }
}

fn proposal_from_proto(proposal: proto::Proposal) -> Proposal {
    let status = match proto::ProposalStatus::try_from(proposal.status)
        .unwrap_or(proto::ProposalStatus::Unspecified)
    {
        proto::ProposalStatus::Pending => ProposalStatus::Pending,
        proto::ProposalStatus::Ready => ProposalStatus::Ready,
        proto::ProposalStatus::Submitted => ProposalStatus::Submitted,
        proto::ProposalStatus::Executed => ProposalStatus::Executed,
        proto::ProposalStatus::Expired => ProposalStatus::Expired,
        proto::ProposalStatus::Failed => ProposalStatus::Failed {
            reason: proposal
                .failure_reason
                .clone()
                .unwrap_or_else(|| "failed".to_string()),
        },
        proto::ProposalStatus::Unspecified => ProposalStatus::Pending,
    };

    let amount = proposal.amount.as_deref().and_then(|value| {
        let parsed = parse_nock_amount_to_nicks(value);
        if parsed.is_none() && !PROPOSAL_AMOUNT_PARSE_WARNED.swap(true, Ordering::Relaxed) {
            warn!(
                amount = value,
                "failed to parse proposal amount as NOCK decimal"
            );
        }
        parsed
    });

    Proposal {
        id: proposal.id,
        proposal_type: proposal.proposal_type,
        description: proposal.description,
        signatures_collected: u8::try_from(proposal.signatures_collected).unwrap_or(u8::MAX),
        signatures_required: u8::try_from(proposal.signatures_required).unwrap_or(u8::MAX),
        signers: proposal.signers,
        created_at: proposal
            .created_at_ms
            .map(system_time_from_millis)
            .unwrap_or_else(SystemTime::now),
        status,
        data_hash: proposal.data_hash,
        submitted_at_block: proposal.submitted_at_block,
        submitted_at: proposal.submitted_at_ms.map(system_time_from_millis),
        tx_hash: proposal.tx_hash,
        time_to_submit_ms: proposal.time_to_submit_ms,
        executed_at_block: proposal.executed_at_block,
        source_block: proposal.source_block,
        amount,
        recipient: proposal.recipient,
        nonce: proposal.nonce,
        source_tx_id: proposal.source_tx_id,
        current_proposer: proposal.current_proposer,
        is_my_turn: proposal.is_my_turn,
        time_until_takeover: proposal.time_until_takeover_ms.map(duration_from_millis),
    }
}

fn transaction_state_from_proto(state: proto::TransactionState) -> TransactionState {
    let max_transactions = usize::try_from(state.max_transactions)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(crate::bridge_status::TX_CAPACITY);
    let mut transactions = VecDeque::with_capacity(max_transactions.max(state.transactions.len()));
    for tx in state.transactions {
        transactions.push_back(bridge_tx_from_proto(tx));
    }
    TransactionState {
        transactions,
        max_transactions,
    }
}

/// Parse a NOCK decimal string into nicks, requiring exact nick granularity.
/// 1 NOCK = 10^16 base units, 1 NOCK = 65,536 nicks, so the base amount must
/// be divisible by NOCK_BASE_PER_NICK (no rounding).
fn parse_nock_amount_to_nicks(value: &str) -> Option<u128> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    let mut parts = trimmed.split('.');
    let whole_str = parts.next().unwrap_or("");
    let frac_str = parts.next();
    if parts.next().is_some() {
        return None;
    }

    let whole = if whole_str.is_empty() {
        0
    } else {
        whole_str.parse::<u128>().ok()?
    };

    let frac_scaled = match frac_str {
        None => 0,
        Some("") => 0,
        Some(frac) => {
            if frac.len() > 16 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let raw = frac.parse::<u128>().ok()?;
            let scale = 16usize.saturating_sub(frac.len());
            raw.checked_mul(10u128.pow(scale as u32))?
        }
    };

    let base_units = whole
        .checked_mul(NOCK_BASE_UNIT)?
        .checked_add(frac_scaled)?;

    if base_units % NOCK_BASE_PER_NICK != 0 {
        return None;
    }

    Some(base_units / NOCK_BASE_PER_NICK)
}

fn default_transaction_state() -> TransactionState {
    TransactionState::new(crate::bridge_status::TX_CAPACITY)
}

fn bridge_tx_from_proto(tx: proto::BridgeTx) -> BridgeTx {
    let direction = match proto::TxDirection::try_from(tx.direction)
        .unwrap_or(proto::TxDirection::Unspecified)
    {
        proto::TxDirection::Deposit => TxDirection::Deposit,
        proto::TxDirection::Withdrawal => TxDirection::Withdrawal,
        proto::TxDirection::Unspecified => TxDirection::Deposit,
    };

    let status = tx_status_from_proto(tx.status);

    BridgeTx {
        tx_hash: tx.tx_hash,
        direction,
        from: tx.from,
        to: tx.to,
        amount: tx.amount.parse::<u128>().unwrap_or(0),
        status,
        timestamp: system_time_from_millis(tx.timestamp_ms),
        base_block: tx.base_block,
        nock_height: tx.nock_height,
    }
}

fn tx_status_from_proto(status: Option<proto::TxStatus>) -> TxStatus {
    let Some(status) = status.and_then(|status| status.status) else {
        return TxStatus::Pending;
    };

    match status {
        proto::tx_status::Status::Pending(_) => TxStatus::Pending,
        proto::tx_status::Status::Confirming(confirming) => TxStatus::Confirming {
            confirmations: confirming.confirmations,
            required: confirming.required,
        },
        proto::tx_status::Status::Processing(_) => TxStatus::Processing,
        proto::tx_status::Status::Completed(_) => TxStatus::Completed,
        proto::tx_status::Status::Failed(failed) => TxStatus::Failed {
            reason: failed.reason,
        },
    }
}

fn alert_state_from_proto(snapshot: proto::AlertsSnapshot) -> AlertState {
    let mut state = AlertState::new(ALERT_HISTORY_CAPACITY);
    state.alerts = snapshot
        .alerts
        .into_iter()
        .map(alert_from_proto)
        .collect::<VecDeque<_>>();
    state.refresh_next_id_from_alerts();
    state
}

fn alert_from_proto(alert: proto::Alert) -> Alert {
    Alert {
        id: alert.id,
        severity: alert_severity_from_proto(alert.severity),
        title: alert.title,
        message: alert.message,
        timestamp: system_time_from_millis(alert.created_at_ms),
        source: alert.source,
    }
}

fn alert_severity_from_proto(severity: i32) -> AlertSeverity {
    match proto::AlertSeverity::try_from(severity).unwrap_or(proto::AlertSeverity::Unspecified) {
        proto::AlertSeverity::Info => AlertSeverity::Info,
        proto::AlertSeverity::Warning => AlertSeverity::Warning,
        proto::AlertSeverity::Error => AlertSeverity::Error,
        proto::AlertSeverity::Critical => AlertSeverity::Critical,
        proto::AlertSeverity::Unspecified => AlertSeverity::Info,
    }
}

fn peer_status_from_proto(status: proto::PeerStatus) -> NodeHealthSnapshot {
    let health_status = match proto::PeerHealthStatus::try_from(status.status)
        .unwrap_or(proto::PeerHealthStatus::Unspecified)
    {
        proto::PeerHealthStatus::Healthy => NodeHealthStatus::Healthy,
        proto::PeerHealthStatus::Unreachable => NodeHealthStatus::Unreachable {
            error: status
                .error
                .clone()
                .unwrap_or_else(|| "unreachable".to_string()),
        },
        proto::PeerHealthStatus::Unspecified => NodeHealthStatus::Unreachable {
            error: status
                .error
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        },
    };

    let last_updated = status
        .last_updated_ms
        .map(system_time_from_millis)
        .unwrap_or_else(SystemTime::now);

    NodeHealthSnapshot {
        node_id: status.node_id,
        address: status.address,
        status: health_status,
        latency_ms: status.latency_ms.map(u64_to_u128),
        peer_uptime_ms: status.peer_uptime_ms,
        last_updated,
    }
}

fn system_time_from_millis(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

fn duration_from_millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn u64_to_u128(value: u64) -> u128 {
    value as u128
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;
    use crate::bridge_status::BridgeStatus;
    use crate::tui::state::{new_log_buffer, TuiStatus};

    #[test]
    fn proposal_from_proto_parses_amount_and_failure_reason() {
        let nicks_per_nock = NOCK_BASE_UNIT / NOCK_BASE_PER_NICK;
        let proposal = proto::Proposal {
            id: "id".to_string(),
            proposal_type: "deposit".to_string(),
            description: "desc".to_string(),
            signatures_collected: 1,
            signatures_required: 3,
            signers: vec![1],
            created_at_ms: Some(1_000),
            status: proto::ProposalStatus::Failed as i32,
            data_hash: "hash".to_string(),
            submitted_at_block: None,
            submitted_at_ms: None,
            tx_hash: None,
            time_to_submit_ms: None,
            executed_at_block: None,
            source_block: None,
            amount: Some("1.5".to_string()),
            recipient: None,
            nonce: None,
            source_tx_id: None,
            current_proposer: None,
            is_my_turn: false,
            time_until_takeover_ms: None,
            failure_reason: Some("boom".to_string()),
        };

        let parsed = proposal_from_proto(proposal);
        assert_eq!(parsed.amount, Some(nicks_per_nock + (nicks_per_nock / 2)));
        assert!(matches!(parsed.status, ProposalStatus::Failed { .. }));
        if let ProposalStatus::Failed { reason } = parsed.status {
            assert_eq!(reason, "boom".to_string());
        }
    }

    #[test]
    fn proposal_from_proto_parses_integer_nock_amounts() {
        let nicks_per_nock = NOCK_BASE_UNIT / NOCK_BASE_PER_NICK;
        let proposal = proto::Proposal {
            id: "id".to_string(),
            proposal_type: "deposit".to_string(),
            description: "desc".to_string(),
            signatures_collected: 1,
            signatures_required: 3,
            signers: vec![1],
            created_at_ms: Some(1_000),
            status: proto::ProposalStatus::Pending as i32,
            data_hash: "hash".to_string(),
            submitted_at_block: None,
            submitted_at_ms: None,
            tx_hash: None,
            time_to_submit_ms: None,
            executed_at_block: None,
            source_block: None,
            amount: Some("2".to_string()),
            recipient: None,
            nonce: None,
            source_tx_id: None,
            current_proposer: None,
            is_my_turn: false,
            time_until_takeover_ms: None,
            failure_reason: None,
        };

        let parsed = proposal_from_proto(proposal);
        assert_eq!(parsed.amount, Some(nicks_per_nock * 2));
    }

    #[test]
    fn parse_nock_amount_to_nicks_accepts_decimal_amounts() {
        let nicks_per_nock = NOCK_BASE_UNIT / NOCK_BASE_PER_NICK;
        assert_eq!(parse_nock_amount_to_nicks("1"), Some(nicks_per_nock));
        assert_eq!(
            parse_nock_amount_to_nicks("1.5"),
            Some(nicks_per_nock + (nicks_per_nock / 2))
        );
        assert_eq!(parse_nock_amount_to_nicks("0.0000152587890625"), Some(1));
        assert_eq!(
            parse_nock_amount_to_nicks("2.0000000000000000"),
            Some(nicks_per_nock * 2)
        );
    }

    #[test]
    fn parse_nock_amount_to_nicks_rejects_invalid_amounts() {
        assert_eq!(parse_nock_amount_to_nicks(""), None);
        assert_eq!(parse_nock_amount_to_nicks("-1"), None);
        assert_eq!(parse_nock_amount_to_nicks("1.0000000000000001"), None);
        assert_eq!(parse_nock_amount_to_nicks("0.00001525878906250"), None);
        assert_eq!(parse_nock_amount_to_nicks("abc"), None);
        assert_eq!(parse_nock_amount_to_nicks("1.2.3"), None);
    }

    #[test]
    fn network_state_applies_holds_from_snapshot() {
        let response = proto::GetSnapshotResponse {
            running_state: proto::RunningState::Stopped as i32,
            nock_hold: true,
            base_hold: false,
            nock_hold_height: Some(10),
            base_hold_height: None,
            network_state: Some(proto::NetworkState {
                base: None,
                nockchain: None,
                pending_deposits: 0,
                pending_withdrawals: 0,
                unsettled_deposit_count: 0,
                unsettled_withdrawal_count: 0,
                batch_status: None,
                is_mainnet: None,
                nockchain_api_status: None,
                base_next_height: None,
                nock_next_height: None,
                degradation_warning: None,
            }),
            deposit_log: None,
            proposals: None,
            metrics: None,
            transactions: None,
            alerts: None,
            peer_statuses: Vec::new(),
            last_submitted_deposit: None,
            last_successful_deposit: None,
        };

        let snapshot = BridgeTuiSnapshot::from_proto(response);
        assert!(snapshot.network_state.kernel_stopped);
        assert!(snapshot.network_state.nock_hold);
        assert_eq!(snapshot.network_state.nock_hold_height, Some(10));
    }

    #[test]
    fn alerts_snapshot_preserves_order() {
        let snapshot = proto::AlertsSnapshot {
            alerts: vec![
                proto::Alert {
                    id: 1,
                    severity: proto::AlertSeverity::Warning as i32,
                    title: "warn".to_string(),
                    message: "warn".to_string(),
                    source: "test".to_string(),
                    created_at_ms: 1_000,
                },
                proto::Alert {
                    id: 2,
                    severity: proto::AlertSeverity::Info as i32,
                    title: "info".to_string(),
                    message: "info".to_string(),
                    source: "test".to_string(),
                    created_at_ms: 2_000,
                },
            ],
        };

        let alert_state = alert_state_from_proto(snapshot);
        assert_eq!(alert_state.alerts.len(), 2);
        assert_eq!(alert_state.alerts[0].id, 1);
        assert_eq!(alert_state.alerts[1].id, 2);

        let mut alert_state = alert_state;
        alert_state.push(
            AlertSeverity::Info,
            "next".to_string(),
            "next".to_string(),
            "test".to_string(),
        );
        assert_eq!(alert_state.alerts.front().map(|alert| alert.id), Some(3));
    }

    #[test]
    fn apply_snapshot_updates_tui_state() {
        let health = Arc::new(RwLock::new(Vec::new()));
        let core = BridgeStatus::new(health);
        let tui_status = TuiStatus::new(core, new_log_buffer());

        let response = proto::GetSnapshotResponse {
            running_state: proto::RunningState::Running as i32,
            nock_hold: false,
            base_hold: false,
            nock_hold_height: None,
            base_hold_height: None,
            network_state: Some(proto::NetworkState {
                base: None,
                nockchain: None,
                pending_deposits: 3,
                pending_withdrawals: 2,
                unsettled_deposit_count: 1,
                unsettled_withdrawal_count: 0,
                batch_status: None,
                is_mainnet: Some(true),
                nockchain_api_status: None,
                base_next_height: Some(10),
                nock_next_height: None,
                degradation_warning: None,
            }),
            deposit_log: Some(proto::DepositLogSnapshot {
                total_count: 1,
                first_epoch_nonce: 100,
                rows: vec![proto::DepositLogRow {
                    nonce: 101,
                    block_height: 5,
                    tx_id_base58: "tx".to_string(),
                    recipient_hex: "0xabc".to_string(),
                    amount: 42,
                }],
            }),
            proposals: Some(proto::ProposalState {
                last_submitted: None,
                pending_inbound: Vec::new(),
                history: Vec::new(),
            }),
            metrics: None,
            transactions: Some(proto::TransactionState {
                transactions: vec![proto::BridgeTx {
                    tx_hash: "0xabc".to_string(),
                    direction: proto::TxDirection::Deposit as i32,
                    from: "0xfrom".to_string(),
                    to: "0xto".to_string(),
                    amount: "123".to_string(),
                    status: Some(proto::TxStatus {
                        status: Some(proto::tx_status::Status::Processing(
                            proto::TxStatusProcessing {},
                        )),
                    }),
                    timestamp_ms: 1_000,
                    base_block: Some(42),
                    nock_height: None,
                }],
                max_transactions: 100,
            }),
            peer_statuses: vec![proto::PeerStatus {
                node_id: 1,
                address: "http://localhost:1".to_string(),
                status: proto::PeerHealthStatus::Healthy as i32,
                error: None,
                latency_ms: Some(5),
                peer_uptime_ms: Some(10),
                last_updated_ms: Some(1_000),
            }],
            last_submitted_deposit: None,
            last_successful_deposit: Some(proto::SuccessfulDeposit {
                tx_id: None,
                name_first: None,
                name_last: None,
                recipient: None,
                amount: 0,
                block_height: 0,
                as_of: None,
                nonce: 7,
            }),
            alerts: Some(proto::AlertsSnapshot { alerts: Vec::new() }),
        };

        let snapshot = BridgeTuiSnapshot::from_proto(response);
        snapshot.apply_to(&tui_status);

        assert_eq!(tui_status.network().pending_deposits, 3);
        assert_eq!(tui_status.deposit_log_snapshot().total_count, 1);
        assert_eq!(tui_status.health_snapshots().len(), 1);
        assert_eq!(tui_status.last_deposit_nonce(), Some(7));
        assert_eq!(tui_status.transactions().transactions.len(), 1);
    }
}
