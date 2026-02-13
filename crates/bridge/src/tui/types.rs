//! Shared types for the Bridge TUI.
//!
//! This module defines the data structures used across all TUI panels
//! for displaying bridge state, proposals, transactions, and alerts.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

/// Chain synchronization state for a single blockchain.
#[derive(Clone, Debug, Default)]
pub struct ChainState {
    /// Current block height.
    pub height: u64,
    /// Block hash of the current tip (hex-encoded).
    pub tip_hash: String,
    /// Number of confirmations for the latest relevant transaction.
    pub confirmations: u64,
    /// Whether the chain is currently syncing.
    pub is_syncing: bool,
    /// Last time this state was updated.
    pub last_updated: Option<SystemTime>,
}

/// Connection status for the nockchain API endpoint.
///
/// Tracks whether the bridge is connected to the nockchain gRPC API,
/// providing visibility into connection health and reconnection attempts.
#[derive(Clone, Debug)]
pub enum NockchainApiStatus {
    /// Successfully connected to the nockchain API.
    Connected {
        /// When the connection was established.
        since: SystemTime,
    },
    /// Currently attempting to connect/reconnect.
    Connecting {
        /// Current reconnection attempt number (1-based).
        attempt: u32,
        /// Error from the last failed connection attempt, if any.
        last_error: Option<String>,
        /// When reconnection started.
        since: SystemTime,
    },
    /// Disconnected from the nockchain API.
    Disconnected {
        /// When the disconnection occurred.
        since: SystemTime,
        /// The error that caused the disconnection.
        error: String,
    },
}

impl Default for NockchainApiStatus {
    fn default() -> Self {
        // Start in Connecting state since we haven't established a connection yet
        NockchainApiStatus::Connecting {
            attempt: 0,
            last_error: None,
            since: SystemTime::now(),
        }
    }
}

impl NockchainApiStatus {
    /// Create a new Connected status.
    pub fn connected() -> Self {
        NockchainApiStatus::Connected {
            since: SystemTime::now(),
        }
    }

    /// Create a new Connecting status.
    pub fn connecting(attempt: u32, last_error: Option<String>) -> Self {
        NockchainApiStatus::Connecting {
            attempt,
            last_error,
            since: SystemTime::now(),
        }
    }

    /// Create a new Disconnected status.
    pub fn disconnected(error: String) -> Self {
        NockchainApiStatus::Disconnected {
            since: SystemTime::now(),
            error,
        }
    }

    /// Get the duration since this status was set.
    pub fn duration(&self) -> Duration {
        let since = match self {
            NockchainApiStatus::Connected { since } => since,
            NockchainApiStatus::Connecting { since, .. } => since,
            NockchainApiStatus::Disconnected { since, .. } => since,
        };
        since.elapsed().unwrap_or(Duration::ZERO)
    }

    /// Get the last error message if available.
    pub fn last_error(&self) -> Option<&str> {
        match self {
            NockchainApiStatus::Connecting { last_error, .. } => last_error.as_deref(),
            NockchainApiStatus::Disconnected { error, .. } => Some(error.as_str()),
            _ => None,
        }
    }
}

/// Overall network state combining both chains.
#[derive(Clone, Debug, Default)]
pub struct NetworkState {
    /// Base chain (Ethereum L2) state.
    pub base: ChainState,
    /// Nockchain state.
    pub nockchain: ChainState,
    /// Next base hashchain height expected by the kernel.
    pub base_next_height: Option<u64>,
    /// Next nock hashchain height expected by the kernel.
    pub nock_next_height: Option<u64>,
    /// Nockchain API connection status.
    pub nockchain_api_status: NockchainApiStatus,
    /// Number of pending deposit operations.
    pub pending_deposits: u64,
    /// Number of pending withdrawal operations.
    pub pending_withdrawals: u64,
    /// Current batch processing status.
    pub batch_status: BatchStatus,
    /// Degradation warning (when < 4 nodes healthy).
    pub degradation_warning: Option<String>,

    // --- Kernel state counts (from peeks) ---
    /// Unsettled deposits waiting to be processed.
    pub unsettled_deposit_count: u64,
    /// Unsettled withdrawals waiting to be processed.
    pub unsettled_withdrawal_count: u64,

    // --- Hold status (circuit breakers) ---
    /// Whether Base chain processing is on hold.
    pub base_hold: bool,
    /// Whether Nockchain processing is on hold.
    pub nock_hold: bool,
    /// Whether the kernel has latched a stop state.
    pub kernel_stopped: bool,
    /// Counterparty nock height that releases the base hold.
    pub base_hold_height: Option<u64>,
    /// Counterparty base height that releases the nock hold.
    pub nock_hold_height: Option<u64>,

    // --- Network mode ---
    /// Whether the bridge is running in mainnet mode (true) or fakenet mode (false).
    /// None indicates the status hasn't been fetched yet.
    pub is_mainnet: Option<bool>,
}

/// Batch processing status.
#[derive(Clone, Debug, Default)]
pub enum BatchStatus {
    #[default]
    Idle,
    /// Processing a batch with the given ID.
    Processing { batch_id: u64, progress_pct: u8 },
    /// Waiting for signatures.
    AwaitingSignatures {
        batch_id: u64,
        collected: u8,
        required: u8,
    },
    /// Submitting to chain.
    Submitting { batch_id: u64 },
}

impl BatchStatus {
    pub fn display(&self) -> String {
        match self {
            BatchStatus::Idle => "idle".into(),
            BatchStatus::Processing {
                batch_id,
                progress_pct,
            } => {
                format!("processing #{} ({}%)", batch_id, progress_pct)
            }
            BatchStatus::AwaitingSignatures {
                batch_id,
                collected,
                required,
            } => {
                format!("batch #{}: {}/{} sigs", batch_id, collected, required)
            }
            BatchStatus::Submitting { batch_id } => {
                format!("submitting #{}", batch_id)
            }
        }
    }
}

/// Direction of a bridge transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxDirection {
    Deposit,
    Withdrawal,
}

/// Status of a bridge transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxStatus {
    Pending,
    Confirming { confirmations: u64, required: u64 },
    Processing,
    Completed,
    Failed { reason: String },
}

impl TxStatus {
    pub fn display(&self) -> String {
        match self {
            TxStatus::Pending => "pending".into(),
            TxStatus::Confirming {
                confirmations,
                required,
            } => {
                format!("{}/{} conf", confirmations, required)
            }
            TxStatus::Processing => "processing".into(),
            TxStatus::Completed => "completed".into(),
            TxStatus::Failed { reason } => format!("failed: {}", reason),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, TxStatus::Completed | TxStatus::Failed { .. })
    }
}

/// A bridge transaction (deposit or withdrawal).
#[derive(Clone, Debug)]
pub struct BridgeTx {
    /// Transaction hash on the source chain.
    pub tx_hash: String,
    /// Direction of the transfer.
    pub direction: TxDirection,
    /// Sender address.
    pub from: String,
    /// Recipient address.
    pub to: String,
    /// Amount in nicks (1 NOCK = 65,536 nicks).
    pub amount: u128,
    /// Current status.
    pub status: TxStatus,
    /// Timestamp when first seen.
    pub timestamp: SystemTime,
    /// Base chain block number (for deposits).
    pub base_block: Option<u64>,
    /// Nockchain block height (for withdrawals).
    pub nock_height: Option<u64>,
}

impl BridgeTx {
    /// Generate basescan URL for this transaction.
    pub fn basescan_url(&self) -> Option<String> {
        if self.direction == TxDirection::Deposit {
            Some(format!("https://basescan.org/tx/{}", self.tx_hash))
        } else {
            None
        }
    }

    /// Format amount for display (amount is in nicks, 1 NOCK = 65,536 nicks).
    pub fn format_amount(&self) -> String {
        format_nock_from_nicks(self.amount)
    }
}

/// Base unit for NOCK (10^16) to match on-chain decimals.
pub const NOCK_BASE_UNIT: u128 = 10_000_000_000_000_000;

/// Nicks per NOCK on Nockchain (2^16).
pub const NICKS_PER_NOCK: u128 = 65_536;

/// NOCK base units per nick.
pub const NOCK_BASE_PER_NICK: u128 = NOCK_BASE_UNIT / NICKS_PER_NOCK;

/// Format a nicks amount as a NOCK string with fractional precision.
/// 1 NOCK = 10^16 base units, 1 NOCK = 65,536 nicks.
pub fn format_nock_from_nicks(nicks: u128) -> String {
    if nicks == 0 {
        return "0".to_string();
    }

    let whole = nicks / NICKS_PER_NOCK;
    let frac = nicks % NICKS_PER_NOCK;

    if frac == 0 {
        return whole.to_string();
    }

    let frac_scaled = frac * NOCK_BASE_PER_NICK;
    let mut frac_str = format!("{:016}", frac_scaled);
    while frac_str.ends_with('0') {
        frac_str.pop();
    }

    format!("{}.{}", whole, frac_str)
}

/// Transaction activity state.
#[derive(Clone, Debug, Default)]
pub struct TransactionState {
    /// Recent transactions (newest first).
    pub transactions: VecDeque<BridgeTx>,
    /// Maximum transactions to keep in memory.
    pub max_transactions: usize,
}

impl TransactionState {
    pub fn new(capacity: usize) -> Self {
        Self {
            transactions: VecDeque::with_capacity(capacity),
            max_transactions: capacity,
        }
    }

    pub fn push(&mut self, tx: BridgeTx) {
        if self.transactions.len() >= self.max_transactions {
            self.transactions.pop_back();
        }
        self.transactions.push_front(tx);
    }

    pub fn deposits(&self) -> impl Iterator<Item = &BridgeTx> {
        self.transactions
            .iter()
            .filter(|tx| tx.direction == TxDirection::Deposit)
    }

    pub fn withdrawals(&self) -> impl Iterator<Item = &BridgeTx> {
        self.transactions
            .iter()
            .filter(|tx| tx.direction == TxDirection::Withdrawal)
    }
}

/// Deposit log view entry for the TUI.
#[derive(Clone, Debug)]
pub struct DepositLogRow {
    pub nonce: u64,
    pub block_height: u64,
    pub tx_id_base58: String,
    pub recipient_hex: String,
    pub amount: u64,
}

/// Deposit log snapshot for the TUI.
#[derive(Clone, Debug, Default)]
pub struct DepositLogSnapshot {
    pub total_count: u64,
    pub first_epoch_nonce: u64,
    pub rows: Vec<DepositLogRow>,
}

/// Deposit log query view (newest-first offset).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepositLogView {
    pub offset: usize,
    pub limit: usize,
}

pub const DEPOSIT_LOG_PAGE_SIZE: usize = 200;

impl Default for DepositLogView {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: DEPOSIT_LOG_PAGE_SIZE,
        }
    }
}

/// A multi-sig proposal.
#[derive(Clone, Debug)]
pub struct Proposal {
    /// Unique proposal identifier.
    pub id: String,
    /// Type of proposal (e.g., "deposit_batch", "withdrawal").
    pub proposal_type: String,
    /// Human-readable description.
    pub description: String,
    /// Number of signatures collected.
    pub signatures_collected: u8,
    /// Number of signatures required.
    pub signatures_required: u8,
    /// Which node IDs have signed.
    pub signers: Vec<u64>,
    /// When the proposal was created.
    pub created_at: SystemTime,
    /// Current status.
    pub status: ProposalStatus,
    /// Associated data hash.
    pub data_hash: String,
    /// Base chain block height when submitted (if submitted).
    pub submitted_at_block: Option<u64>,
    /// Timestamp when submitted to chain.
    pub submitted_at: Option<SystemTime>,
    /// Transaction hash on Base (if submitted).
    pub tx_hash: Option<String>,
    /// Time from creation to submission (for metrics).
    pub time_to_submit_ms: Option<u64>,
    /// Base chain block height when executed/confirmed.
    pub executed_at_block: Option<u64>,

    // --- Deposit details (source transaction info) ---
    /// Block height where the deposit was detected.
    pub source_block: Option<u64>,
    /// Amount in nicks (1 NOCK = 65,536 nicks).
    pub amount: Option<u128>,
    /// Recipient address (hex).
    pub recipient: Option<String>,
    /// Deposit nonce.
    pub nonce: Option<u64>,
    /// Source transaction ID (Tip5 hash, base58).
    pub source_tx_id: Option<String>,

    // --- Turn-based proposal state ---
    /// Node ID of the current proposer (who should post).
    pub current_proposer: Option<u64>,
    /// Whether it's this node's turn to post.
    pub is_my_turn: bool,
    /// Time remaining until proposer takeover (if applicable).
    pub time_until_takeover: Option<std::time::Duration>,
}

impl Proposal {
    pub fn signature_progress(&self) -> f64 {
        if self.signatures_required == 0 {
            return 1.0;
        }
        self.signatures_collected as f64 / self.signatures_required as f64
    }

    pub fn is_ready(&self) -> bool {
        self.signatures_collected >= self.signatures_required
    }
}

/// Status of a proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalStatus {
    /// Collecting signatures.
    Pending,
    /// Has enough signatures, ready to execute.
    Ready,
    /// Submitted to chain.
    Submitted,
    /// Successfully executed.
    Executed,
    /// Expired or cancelled.
    Expired,
    /// Failed execution.
    Failed { reason: String },
}

impl ProposalStatus {
    pub fn display(&self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Ready => "ready",
            ProposalStatus::Submitted => "submitted",
            ProposalStatus::Executed => "executed",
            ProposalStatus::Expired => "expired",
            ProposalStatus::Failed { .. } => "failed",
        }
    }
}

/// Proposal management state.
#[derive(Clone, Debug, Default)]
pub struct ProposalState {
    /// Last proposal we submitted.
    pub last_submitted: Option<Proposal>,
    /// Pending proposals awaiting our signature.
    pub pending_inbound: Vec<Proposal>,
    /// Proposal history (most recent first).
    pub history: VecDeque<Proposal>,
    /// Maximum history entries to keep.
    pub max_history: usize,
}

impl ProposalState {
    pub fn new(max_history: usize) -> Self {
        Self {
            last_submitted: None,
            pending_inbound: Vec::new(),
            history: VecDeque::with_capacity(max_history),
            max_history,
        }
    }

    /// Find a proposal by ID in all collections.
    pub fn find_by_id(&self, id: &str) -> Option<&Proposal> {
        // Check last submitted
        if let Some(ref p) = self.last_submitted {
            if p.id == id {
                return Some(p);
            }
        }
        // Check pending inbound
        if let Some(p) = self.pending_inbound.iter().find(|p| p.id == id) {
            return Some(p);
        }
        // Check history
        self.history.iter().find(|p| p.id == id)
    }

    /// Update or insert a proposal. Handles placement based on status.
    pub fn update_or_insert(&mut self, proposal: Proposal) {
        let id = proposal.id.clone();

        // Remove from all collections first
        if let Some(ref p) = self.last_submitted {
            if p.id == id {
                self.last_submitted = None;
            }
        }
        self.pending_inbound.retain(|p| p.id != id);
        self.history.retain(|p| p.id != id);

        // Place in appropriate collection based on status
        match proposal.status {
            ProposalStatus::Pending => {
                self.pending_inbound.push(proposal);
            }
            ProposalStatus::Ready
            | ProposalStatus::Submitted
            | ProposalStatus::Executed
            | ProposalStatus::Expired
            | ProposalStatus::Failed { .. } => {
                if self.history.len() >= self.max_history {
                    self.history.pop_back();
                }
                self.history.push_front(proposal);
            }
        }
    }

    /// Add a signature to a proposal, returning true if signature was added.
    ///
    /// When a proposal reaches the signature threshold, its status transitions
    /// from `Pending` to `Ready` and it moves from `pending_inbound` to `history`.
    pub fn add_signature(&mut self, id: &str, node_id: u64) -> bool {
        // Check last submitted
        if let Some(ref mut p) = self.last_submitted {
            if p.id == id && !p.signers.contains(&node_id) {
                p.signers.push(node_id);
                p.signatures_collected = p.signers.len() as u8;
                if p.is_ready() && p.status == ProposalStatus::Pending {
                    p.status = ProposalStatus::Ready;
                }
                return true;
            }
        }

        // Check pending inbound - need to handle status transition specially
        if let Some(idx) = self.pending_inbound.iter().position(|p| p.id == id) {
            let p = &mut self.pending_inbound[idx];
            if p.signers.contains(&node_id) {
                return false; // Duplicate signature
            }
            p.signers.push(node_id);
            p.signatures_collected = p.signers.len() as u8;

            // If threshold reached, transition to Ready and move to history
            if p.is_ready() && p.status == ProposalStatus::Pending {
                let mut proposal = self.pending_inbound.remove(idx);
                proposal.status = ProposalStatus::Ready;
                if self.history.len() >= self.max_history {
                    self.history.pop_back();
                }
                self.history.push_front(proposal);
            }
            return true;
        }

        // Check history - proposals here are already past Pending state
        if let Some(p) = self.history.iter_mut().find(|p| p.id == id) {
            if !p.signers.contains(&node_id) {
                p.signers.push(node_id);
                p.signatures_collected = p.signers.len() as u8;
                return true;
            }
        }

        false
    }
}

/// Maximum hourly transaction counts to keep (24 hours).
pub const HOURLY_TX_CAPACITY: usize = 24;

/// Bridge metrics for analytics.
#[derive(Clone, Debug, Default)]
pub struct MetricsState {
    /// Total value deposited (lifetime).
    pub total_deposited: u128,
    /// Total value withdrawn (lifetime).
    pub total_withdrawn: u128,
    /// Transaction counts per hour (last 24 hours, bounded).
    pub hourly_tx_counts: VecDeque<u64>,
    /// Average transaction latency in seconds.
    pub avg_latency_secs: f64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
    /// Total fees collected.
    pub total_fees: u128,

    // --- Internal tracking fields for running averages ---
    /// Total number of transactions recorded (for success rate calculation).
    pub(crate) tx_count: u64,
    /// Sum of all latencies in milliseconds (for average calculation).
    pub(crate) latency_sum_ms: u64,
    /// Count of latency samples (for average calculation).
    pub(crate) latency_count: u64,
}

impl MetricsState {
    /// Push an hourly transaction count, enforcing capacity limit.
    pub fn push_hourly_count(&mut self, count: u64) {
        if self.hourly_tx_counts.len() >= HOURLY_TX_CAPACITY {
            self.hourly_tx_counts.pop_front();
        }
        self.hourly_tx_counts.push_back(count);
    }

    /// Get sparkline data for transaction volume.
    pub fn volume_sparkline(&self) -> Vec<u64> {
        self.hourly_tx_counts.iter().copied().collect()
    }

    /// Format total bridged value for display (Nock token has 16 decimals).
    pub fn format_total_bridged(&self) -> String {
        let total = self.total_deposited.saturating_add(self.total_withdrawn);
        let whole = total / 10_000_000_000_000_000;
        format!("{} NOCK", whole)
    }
}

/// Alert severity levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl AlertSeverity {
    pub fn symbol(&self) -> &'static str {
        match self {
            AlertSeverity::Info => "ℹ",
            AlertSeverity::Warning => "⚠",
            AlertSeverity::Error => "✗",
            AlertSeverity::Critical => "🔥",
        }
    }
}

/// An alert/notification.
#[derive(Clone, Debug)]
pub struct Alert {
    /// Unique alert ID.
    pub id: u64,
    /// Severity level.
    pub severity: AlertSeverity,
    /// Alert title.
    pub title: String,
    /// Detailed message.
    pub message: String,
    /// When the alert was triggered.
    pub timestamp: SystemTime,
    /// Source of the alert (e.g., "health", "tx", "reorg").
    pub source: String,
}

/// Alert state management.
#[derive(Clone, Debug, Default)]
pub struct AlertState {
    /// Alerts (most recent first).
    pub alerts: VecDeque<Alert>,
    /// Next alert ID.
    next_id: u64,
    /// Maximum history size.
    pub max_history: usize,
}

impl AlertState {
    pub fn new(max_history: usize) -> Self {
        Self {
            alerts: VecDeque::with_capacity(max_history),
            next_id: 1,
            max_history,
        }
    }

    pub fn refresh_next_id_from_alerts(&mut self) {
        let max_id = self.alerts.iter().map(|alert| alert.id).max().unwrap_or(0);
        self.next_id = max_id.saturating_add(1);
    }

    pub fn push(
        &mut self,
        severity: AlertSeverity,
        title: String,
        message: String,
        source: String,
    ) {
        let alert = Alert {
            id: self.next_id,
            severity,
            title,
            message,
            timestamp: SystemTime::now(),
            source,
        };
        self.next_id += 1;
        self.alerts.push_front(alert);
        while self.alerts.len() > self.max_history {
            self.alerts.pop_back();
        }
    }

    pub fn highest_severity(&self) -> Option<AlertSeverity> {
        self.alerts.iter().map(|a| a.severity).max()
    }
}

/// UI mode for the TUI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiMode {
    #[default]
    Normal,
    /// Help overlay is shown.
    Help,
}

/// Which panel is currently focused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusedPanel {
    #[default]
    Health,
    DepositLog,
    Proposals,
    Transactions,
    Alerts,
}

impl FocusedPanel {
    pub fn display(&self) -> &'static str {
        match self {
            FocusedPanel::Health => "health",
            FocusedPanel::DepositLog => "deposit log",
            FocusedPanel::Proposals => "proposals",
            FocusedPanel::Transactions => "transactions",
            FocusedPanel::Alerts => "alerts",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            FocusedPanel::Health => FocusedPanel::DepositLog,
            FocusedPanel::DepositLog => FocusedPanel::Proposals,
            FocusedPanel::Proposals => FocusedPanel::Transactions,
            FocusedPanel::Transactions => FocusedPanel::Alerts,
            FocusedPanel::Alerts => FocusedPanel::Health,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            FocusedPanel::Health => FocusedPanel::Alerts,
            FocusedPanel::DepositLog => FocusedPanel::Health,
            FocusedPanel::Proposals => FocusedPanel::DepositLog,
            FocusedPanel::Transactions => FocusedPanel::Proposals,
            FocusedPanel::Alerts => FocusedPanel::Transactions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nockchain_api_status_connected() {
        let status = NockchainApiStatus::connected();
        assert!(matches!(status, NockchainApiStatus::Connected { .. }));
        assert!(status.last_error().is_none());
    }

    #[test]
    fn test_nockchain_api_status_connecting() {
        let status = NockchainApiStatus::connecting(3, Some("connection refused".to_string()));
        match &status {
            NockchainApiStatus::Connecting {
                attempt,
                last_error,
                ..
            } => {
                assert_eq!(*attempt, 3);
                assert_eq!(last_error.as_deref(), Some("connection refused"));
            }
            _ => panic!("expected Connecting"),
        }
        assert_eq!(status.last_error(), Some("connection refused"));
    }

    #[test]
    fn test_nockchain_api_status_connecting_no_error() {
        let status = NockchainApiStatus::connecting(1, None);
        assert!(status.last_error().is_none());
    }

    #[test]
    fn test_nockchain_api_status_disconnected() {
        let status = NockchainApiStatus::disconnected("timeout".to_string());
        match &status {
            NockchainApiStatus::Disconnected { error, .. } => {
                assert_eq!(error, "timeout");
            }
            _ => panic!("expected Disconnected"),
        }
        assert_eq!(status.last_error(), Some("timeout"));
    }

    #[test]
    fn test_nockchain_api_status_default() {
        let status = NockchainApiStatus::default();
        assert!(matches!(
            status,
            NockchainApiStatus::Connecting {
                attempt: 0,
                last_error: None,
                ..
            }
        ));
    }

    #[test]
    fn test_nockchain_api_status_duration() {
        let status = NockchainApiStatus::connected();
        // Duration should be very small (just created)
        assert!(status.duration() < Duration::from_secs(1));
    }

    #[test]
    fn test_format_nock_from_nicks() {
        assert_eq!(format_nock_from_nicks(0), "0");
        assert_eq!(format_nock_from_nicks(NICKS_PER_NOCK), "1");
        assert_eq!(format_nock_from_nicks(NICKS_PER_NOCK * 2), "2");
        assert_eq!(
            format_nock_from_nicks(NICKS_PER_NOCK + (NICKS_PER_NOCK / 2)),
            "1.5"
        );
        assert_eq!(format_nock_from_nicks(1), "0.0000152587890625");
        assert_eq!(format_nock_from_nicks(653_410_000_000), "9970245.361328125");
    }
}
