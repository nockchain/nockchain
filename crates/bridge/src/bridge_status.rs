//! Shared bridge status state.
//!
//! This module provides the central state container that aggregates
//! all data sources for the TUI panels and status endpoint.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use alloy::primitives::Address;

use crate::health::{NodeHealthSnapshot, SharedHealthState};
use crate::proposal_cache::{ProposalState as CacheProposalState, SIGNATURE_THRESHOLD};
use crate::tui::types::{AlertState, MetricsState, NetworkState, ProposalState, TransactionState};

/// Capacity for transaction history.
pub const TX_CAPACITY: usize = 100;

/// Capacity for proposal history.
pub const PROPOSAL_HISTORY_CAPACITY: usize = 50;

/// Capacity for alert history.
pub const ALERT_HISTORY_CAPACITY: usize = 100;

/// Shared bridge status that can be updated from multiple sources.
///
/// This is the central data store for the TUI panels and status endpoint. Each field
/// is wrapped in Arc<RwLock<_>> to allow concurrent updates from
/// background tasks while the TUI renders.
#[derive(Clone, Debug)]
pub struct BridgeStatus {
    /// Peer health state (existing).
    pub health: SharedHealthState,
    /// Network/chain state.
    pub network: Arc<RwLock<NetworkState>>,
    /// Transaction activity.
    pub transactions: Arc<RwLock<TransactionState>>,
    /// Proposal management.
    pub proposals: Arc<RwLock<ProposalState>>,
    /// Metrics and analytics.
    pub metrics: Arc<RwLock<MetricsState>>,
    /// Alerts and notifications.
    pub alerts: Arc<RwLock<AlertState>>,
    /// Last confirmed deposit nonce from chain.
    last_deposit_nonce: Arc<RwLock<Option<u64>>>,
}

impl BridgeStatus {
    /// Create a new shared bridge status with the given health state.
    pub fn new(health: SharedHealthState) -> Self {
        Self {
            health,
            network: Arc::new(RwLock::new(NetworkState::default())),
            transactions: Arc::new(RwLock::new(TransactionState::new(TX_CAPACITY))),
            proposals: Arc::new(RwLock::new(ProposalState::new(PROPOSAL_HISTORY_CAPACITY))),
            metrics: Arc::new(RwLock::new(MetricsState::default())),
            alerts: Arc::new(RwLock::new(AlertState::new(ALERT_HISTORY_CAPACITY))),
            last_deposit_nonce: Arc::new(RwLock::new(None)),
        }
    }

    // NOTE: All read methods return defaults on lock poisoning for graceful TUI degradation.
    // A poisoned lock indicates a panic in a background thread - the TUI should continue
    // rendering (with empty data) rather than crashing. The underlying panic will be
    // logged elsewhere.

    /// Get health snapshots.
    pub fn health_snapshots(&self) -> Vec<NodeHealthSnapshot> {
        self.health
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Get network state.
    pub fn network(&self) -> NetworkState {
        self.network
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Get transaction state.
    pub fn transactions(&self) -> TransactionState {
        self.transactions
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Get proposal state.
    pub fn proposals(&self) -> ProposalState {
        self.proposals
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Get metrics state.
    pub fn metrics(&self) -> MetricsState {
        self.metrics
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Get alert state.
    pub fn alerts(&self) -> AlertState {
        self.alerts
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn update_proposal(&self, proposal: crate::tui::types::Proposal) {
        if let Ok(mut guard) = self.proposals.write() {
            guard.update_or_insert(proposal);
        }
    }

    pub fn update_proposal_signature(&self, proposal_id: &str, node_id: u64) -> bool {
        if let Ok(mut guard) = self.proposals.write() {
            guard.add_signature(proposal_id, node_id)
        } else {
            false
        }
    }

    /// Sync proposal signature counts and signers from the cache into the TUI.
    ///
    /// This keeps the UI accurate when signatures arrive before a proposal exists
    /// and are later applied from the pending signature queue.
    pub fn sync_proposal_signatures_from_cache(
        &self,
        proposal_id: &str,
        cache_state: &CacheProposalState,
        address_to_node_id: &HashMap<Address, u64>,
        self_node_id: u64,
    ) {
        let total_signatures = cache_state.peer_signatures.len()
            + if cache_state.my_signature.is_some() {
                1
            } else {
                0
            };

        if let Some(mut proposal) = self.find_proposal(proposal_id) {
            let mut signers = Vec::new();

            if cache_state.my_signature.is_some() {
                signers.push(self_node_id);
            }

            for addr in cache_state.peer_signatures.keys() {
                if let Some(&node_id) = address_to_node_id.get(addr) {
                    signers.push(node_id);
                }
            }

            signers.sort_unstable();
            signers.dedup();

            proposal.signers = signers;
            proposal.signatures_collected = total_signatures as u8;
            if proposal.signatures_required == 0 {
                proposal.signatures_required = SIGNATURE_THRESHOLD as u8;
            }

            if total_signatures >= proposal.signatures_required as usize
                && proposal.status == crate::tui::types::ProposalStatus::Pending
            {
                proposal.status = crate::tui::types::ProposalStatus::Ready;
            }

            self.update_proposal(proposal);
        }

        self.update_signature_count(
            cache_state.proposal.nonce, total_signatures as u8, SIGNATURE_THRESHOLD as u8,
        );
    }

    /// Find a proposal by ID and return a clone if found.
    pub fn find_proposal(&self, id: &str) -> Option<crate::tui::types::Proposal> {
        self.proposals
            .read()
            .ok()
            .and_then(|guard| guard.find_by_id(id).cloned())
    }

    pub fn push_transaction(&self, tx: crate::tui::types::BridgeTx) {
        if let Ok(mut guard) = self.transactions.write() {
            guard.push(tx);
        }
    }

    pub fn update_network(&self, network: NetworkState) {
        if let Ok(mut guard) = self.network.write() {
            *guard = network;
        }
    }

    pub fn update_base_tip_hash(&self, tip_hash: String) {
        if tip_hash.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.network.write() {
            guard.base.tip_hash = tip_hash;
        }
    }

    pub fn update_nockchain_tip_hash(&self, tip_hash: String) {
        if tip_hash.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.network.write() {
            guard.nockchain.tip_hash = tip_hash;
        }
    }

    /// Update the nockchain API connection status.
    ///
    /// This updates only the `nockchain_api_status` field, preserving other network state.
    pub fn update_nockchain_api_status(&self, status: crate::tui::types::NockchainApiStatus) {
        if let Ok(mut guard) = self.network.write() {
            guard.nockchain_api_status = status;
        }
    }

    pub fn push_alert(
        &self,
        severity: crate::tui::types::AlertSeverity,
        title: String,
        message: String,
        source: String,
    ) {
        if let Ok(mut guard) = self.alerts.write() {
            guard.push(severity, title, message, source);
        }
    }

    // --- Metrics update methods ---

    /// Record a transaction completion for metrics tracking.
    ///
    /// Updates running averages for latency and success rate.
    /// Uses saturating arithmetic to prevent overflow.
    pub fn record_tx_completion(
        &self,
        direction: crate::tui::types::TxDirection,
        amount: u128,
        latency_ms: u64,
        success: bool,
    ) {
        if let Ok(mut guard) = self.metrics.write() {
            // Update transaction count
            guard.tx_count = guard.tx_count.saturating_add(1);

            // Update latency tracking
            guard.latency_sum_ms = guard.latency_sum_ms.saturating_add(latency_ms);
            guard.latency_count = guard.latency_count.saturating_add(1);

            // Compute average latency in seconds (avoid division by zero)
            if guard.latency_count > 0 {
                let avg_ms = guard.latency_sum_ms / guard.latency_count;
                guard.avg_latency_secs = avg_ms as f64 / 1000.0;
            }

            // Update success rate (avoid division by zero)
            if guard.tx_count > 0 {
                // Track successes by maintaining running count
                // success_rate = successful_count / total_count
                // We can derive successful_count from: successful = success_rate * (tx_count - 1)
                let prev_successful = if guard.tx_count > 1 {
                    (guard.success_rate * (guard.tx_count - 1) as f64).round() as u64
                } else {
                    0
                };
                let new_successful = if success {
                    prev_successful + 1
                } else {
                    prev_successful
                };
                guard.success_rate = new_successful as f64 / guard.tx_count as f64;
            }

            // Update direction-specific totals with saturating add
            match direction {
                crate::tui::types::TxDirection::Deposit => {
                    guard.total_deposited = guard.total_deposited.saturating_add(amount);
                }
                crate::tui::types::TxDirection::Withdrawal => {
                    guard.total_withdrawn = guard.total_withdrawn.saturating_add(amount);
                }
            }
        }
    }

    /// Update metrics totals directly.
    ///
    /// This is useful for bulk updates from external sources.
    /// Uses saturating arithmetic to prevent overflow.
    pub fn update_metrics_totals(&self, deposited: u128, withdrawn: u128, fees: u128) {
        if let Ok(mut guard) = self.metrics.write() {
            guard.total_deposited = guard.total_deposited.saturating_add(deposited);
            guard.total_withdrawn = guard.total_withdrawn.saturating_add(withdrawn);
            guard.total_fees = guard.total_fees.saturating_add(fees);
        }
    }

    /// Push an hourly transaction count to the metrics sparkline.
    ///
    /// This delegates to MetricsState::push_hourly_count which enforces
    /// the 24-hour capacity limit.
    pub fn push_hourly_count(&self, count: u64) {
        if let Ok(mut guard) = self.metrics.write() {
            guard.push_hourly_count(count);
        }
    }

    /// Rotate hourly transaction counts (shift left, add 0).
    ///
    /// Called every hour by the background rotation task to maintain
    /// a rolling 24-hour window. Drops the oldest hour and adds a new
    /// empty count (0) for the current hour.
    pub fn rotate_hourly_counts(&self) {
        if let Ok(mut guard) = self.metrics.write() {
            guard.push_hourly_count(0);
        }
    }

    // --- Batch status update methods ---

    /// Update batch status with state transition validation.
    ///
    /// Validates that state transitions follow the expected flow:
    /// Idle → Processing → AwaitingSignatures → Submitting → Idle
    ///
    /// Invalid transitions log a warning but do not crash.
    pub fn update_batch_status(&self, status: crate::tui::types::BatchStatus) {
        if let Ok(mut guard) = self.network.write() {
            use crate::tui::types::BatchStatus;

            // Validate state transition
            let valid_transition = match (&guard.batch_status, &status) {
                // From Idle: can only go to Processing
                (BatchStatus::Idle, BatchStatus::Processing { .. }) => true,
                (BatchStatus::Idle, BatchStatus::Idle) => true, // Idempotent

                // From Processing: can go to AwaitingSignatures, Submitting (if no sigs needed), or back to Idle (on error)
                (BatchStatus::Processing { .. }, BatchStatus::AwaitingSignatures { .. }) => true,
                (BatchStatus::Processing { .. }, BatchStatus::Submitting { .. }) => true,
                (BatchStatus::Processing { .. }, BatchStatus::Idle) => true,
                (
                    BatchStatus::Processing { batch_id: id1, .. },
                    BatchStatus::Processing { batch_id: id2, .. },
                ) => {
                    id1 == id2 // Allow progress updates for same batch
                }

                // From AwaitingSignatures: can go to Submitting or back to Idle (on error/timeout)
                (BatchStatus::AwaitingSignatures { .. }, BatchStatus::Submitting { .. }) => true,
                (BatchStatus::AwaitingSignatures { .. }, BatchStatus::Idle) => true,
                (
                    BatchStatus::AwaitingSignatures { batch_id: id1, .. },
                    BatchStatus::AwaitingSignatures { batch_id: id2, .. },
                ) => {
                    id1 == id2 // Allow signature count updates for same batch
                }

                // From Submitting: can only go to Idle (completion or error)
                (BatchStatus::Submitting { .. }, BatchStatus::Idle) => true,

                // Invalid transitions
                _ => false,
            };

            if valid_transition {
                guard.batch_status = status;
            } else {
                // Log warning but don't crash - the TUI should continue rendering
                tracing::warn!(
                    "Invalid batch status transition: {:?} -> {:?}", guard.batch_status, status
                );
            }
        }
    }

    /// Update batch processing progress.
    ///
    /// This is a convenience method for updating progress percentage
    /// within the Processing state.
    pub fn update_batch_progress(&self, batch_id: u64, progress_pct: u8) {
        use crate::tui::types::BatchStatus;
        self.update_batch_status(BatchStatus::Processing {
            batch_id,
            progress_pct,
        });
    }

    /// Update signature collection count.
    ///
    /// This is a convenience method for updating signature collection
    /// within the AwaitingSignatures state.
    pub fn update_signature_count(&self, batch_id: u64, collected: u8, required: u8) {
        use crate::tui::types::BatchStatus;
        self.update_batch_status(BatchStatus::AwaitingSignatures {
            batch_id,
            collected,
            required,
        });
    }

    /// Get last confirmed deposit nonce from chain.
    pub fn last_deposit_nonce(&self) -> Option<u64> {
        self.last_deposit_nonce.read().ok().and_then(|guard| *guard)
    }

    /// Update last confirmed deposit nonce from chain.
    pub fn update_last_deposit_nonce(&self, nonce: u64) {
        if let Ok(mut guard) = self.last_deposit_nonce.write() {
            *guard = Some(nonce);
        }
    }

    /// Replace last confirmed deposit nonce (clears when None).
    pub fn set_last_deposit_nonce(&self, nonce: Option<u64>) {
        if let Ok(mut guard) = self.last_deposit_nonce.write() {
            *guard = nonce;
        }
    }
}

/// Rotates hourly transaction counts every hour for the sparkline display.
pub async fn run_hourly_rotation(bridge_status: BridgeStatus) {
    use tokio::time::{interval_at, Instant};

    let now = std::time::SystemTime::now();
    let secs_since_epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_until_next_hour = 3600 - (secs_since_epoch % 3600);

    let start = Instant::now() + Duration::from_secs(secs_until_next_hour);
    let mut timer = interval_at(start, Duration::from_secs(3600));

    loop {
        timer.tick().await;
        bridge_status.rotate_hourly_counts();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::*;
    use crate::tui::types::{BatchStatus, NockchainApiStatus, TxDirection, HOURLY_TX_CAPACITY};

    #[test]
    fn test_record_tx_completion_updates_totals() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Record a deposit
        state.record_tx_completion(TxDirection::Deposit, 1000, 500, true);

        let metrics = state.metrics();
        assert_eq!(metrics.total_deposited, 1000);
        assert_eq!(metrics.total_withdrawn, 0);

        // Record a withdrawal
        state.record_tx_completion(TxDirection::Withdrawal, 500, 300, true);

        let metrics = state.metrics();
        assert_eq!(metrics.total_deposited, 1000);
        assert_eq!(metrics.total_withdrawn, 500);
    }

    #[test]
    fn test_success_rate_calculation() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // All successful
        state.record_tx_completion(TxDirection::Deposit, 100, 100, true);
        state.record_tx_completion(TxDirection::Deposit, 100, 100, true);
        state.record_tx_completion(TxDirection::Deposit, 100, 100, true);

        let metrics = state.metrics();
        assert_eq!(metrics.tx_count, 3);
        assert_eq!(metrics.success_rate, 1.0);

        // One failure
        state.record_tx_completion(TxDirection::Deposit, 100, 100, false);

        let metrics = state.metrics();
        assert_eq!(metrics.tx_count, 4);
        assert_eq!(metrics.success_rate, 0.75); // 3/4

        // Half and half
        state.record_tx_completion(TxDirection::Deposit, 100, 100, false);
        state.record_tx_completion(TxDirection::Deposit, 100, 100, false);

        let metrics = state.metrics();
        assert_eq!(metrics.tx_count, 6);
        assert_eq!(metrics.success_rate, 0.5); // 3/6
    }

    #[test]
    fn test_success_rate_with_zero_count() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));
        let metrics = state.metrics();

        // Should not panic with division by zero
        assert_eq!(metrics.success_rate, 0.0);
        assert_eq!(metrics.tx_count, 0);
    }

    #[test]
    fn test_latency_averaging() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Record transactions with different latencies
        state.record_tx_completion(TxDirection::Deposit, 100, 1000, true); // 1s
        state.record_tx_completion(TxDirection::Deposit, 100, 2000, true); // 2s
        state.record_tx_completion(TxDirection::Deposit, 100, 3000, true); // 3s

        let metrics = state.metrics();
        assert_eq!(metrics.latency_count, 3);
        assert_eq!(metrics.latency_sum_ms, 6000);
        // Average should be 2000ms = 2.0s
        assert_eq!(metrics.avg_latency_secs, 2.0);
    }

    #[test]
    fn test_latency_averaging_with_zero_count() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));
        let metrics = state.metrics();

        // Should not panic with division by zero
        assert_eq!(metrics.avg_latency_secs, 0.0);
        assert_eq!(metrics.latency_count, 0);
    }

    #[test]
    fn test_overflow_protection() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Set to near max
        {
            let mut guard = state.metrics.write().unwrap();
            guard.total_deposited = u128::MAX - 100;
        }

        // This should saturate instead of overflow
        state.record_tx_completion(TxDirection::Deposit, 200, 100, true);

        let metrics = state.metrics();
        assert_eq!(metrics.total_deposited, u128::MAX);
    }

    #[test]
    fn test_update_metrics_totals() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        state.update_metrics_totals(1000, 500, 25);

        let metrics = state.metrics();
        assert_eq!(metrics.total_deposited, 1000);
        assert_eq!(metrics.total_withdrawn, 500);
        assert_eq!(metrics.total_fees, 25);

        // Cumulative update
        state.update_metrics_totals(500, 300, 10);

        let metrics = state.metrics();
        assert_eq!(metrics.total_deposited, 1500);
        assert_eq!(metrics.total_withdrawn, 800);
        assert_eq!(metrics.total_fees, 35);
    }

    #[test]
    fn test_push_hourly_count() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Push some counts
        state.push_hourly_count(10);
        state.push_hourly_count(20);
        state.push_hourly_count(30);

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), 3);
        assert_eq!(metrics.hourly_tx_counts[0], 10);
        assert_eq!(metrics.hourly_tx_counts[1], 20);
        assert_eq!(metrics.hourly_tx_counts[2], 30);
    }

    #[test]
    fn test_hourly_count_capacity_limit() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Push more than capacity
        for i in 0..(HOURLY_TX_CAPACITY + 5) {
            state.push_hourly_count(i as u64);
        }

        let metrics = state.metrics();
        // Should be capped at HOURLY_TX_CAPACITY (24)
        assert_eq!(metrics.hourly_tx_counts.len(), HOURLY_TX_CAPACITY);
        // First element should be 5 (the 5 oldest were popped)
        assert_eq!(metrics.hourly_tx_counts[0], 5);
    }

    #[test]
    fn test_incremental_latency_averaging() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // First tx: 1000ms latency
        state.record_tx_completion(TxDirection::Deposit, 100, 1000, true);
        let metrics = state.metrics();
        assert_eq!(metrics.avg_latency_secs, 1.0);

        // Second tx: 3000ms latency (average should be 2000ms = 2.0s)
        state.record_tx_completion(TxDirection::Deposit, 100, 3000, true);
        let metrics = state.metrics();
        assert_eq!(metrics.avg_latency_secs, 2.0);

        // Third tx: 2000ms latency (average should be 2000ms = 2.0s)
        state.record_tx_completion(TxDirection::Deposit, 100, 2000, true);
        let metrics = state.metrics();
        assert_eq!(metrics.avg_latency_secs, 2.0);
    }

    // --- Batch status tests ---

    #[test]
    fn test_valid_batch_status_transitions() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Start: Idle -> Processing
        state.update_batch_status(BatchStatus::Processing {
            batch_id: 1,
            progress_pct: 0,
        });
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Processing { batch_id: 1, .. }
        ));

        // Processing -> AwaitingSignatures
        state.update_batch_status(BatchStatus::AwaitingSignatures {
            batch_id: 1,
            collected: 2,
            required: 4,
        });
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::AwaitingSignatures {
                batch_id: 1,
                collected: 2,
                required: 4
            }
        ));

        // AwaitingSignatures -> Submitting
        state.update_batch_status(BatchStatus::Submitting { batch_id: 1 });
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Submitting { batch_id: 1 }
        ));

        // Submitting -> Idle (completion)
        state.update_batch_status(BatchStatus::Idle);
        let network = state.network();
        assert!(matches!(network.batch_status, BatchStatus::Idle));
    }

    #[test]
    fn test_invalid_batch_status_transitions() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Idle -> Submitting (invalid, should remain Idle)
        state.update_batch_status(BatchStatus::Submitting { batch_id: 1 });
        let network = state.network();
        assert!(matches!(network.batch_status, BatchStatus::Idle));

        // Idle -> AwaitingSignatures (invalid)
        state.update_batch_status(BatchStatus::AwaitingSignatures {
            batch_id: 1,
            collected: 2,
            required: 4,
        });
        let network = state.network();
        assert!(matches!(network.batch_status, BatchStatus::Idle));
    }

    #[test]
    fn test_batch_progress_updates() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Start processing
        state.update_batch_progress(1, 0);
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Processing {
                batch_id: 1,
                progress_pct: 0
            }
        ));

        // Update progress (same batch)
        state.update_batch_progress(1, 50);
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Processing {
                batch_id: 1,
                progress_pct: 50
            }
        ));

        // Update progress to 100%
        state.update_batch_progress(1, 100);
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Processing {
                batch_id: 1,
                progress_pct: 100
            }
        ));
    }

    #[test]
    fn test_signature_count_updates() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Start with processing, then move to awaiting signatures
        state.update_batch_status(BatchStatus::Processing {
            batch_id: 1,
            progress_pct: 100,
        });
        state.update_signature_count(1, 0, 4);

        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::AwaitingSignatures {
                batch_id: 1,
                collected: 0,
                required: 4
            }
        ));

        // Update signature count (same batch)
        state.update_signature_count(1, 2, 4);
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::AwaitingSignatures {
                batch_id: 1,
                collected: 2,
                required: 4
            }
        ));

        // All signatures collected
        state.update_signature_count(1, 4, 4);
        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::AwaitingSignatures {
                batch_id: 1,
                collected: 4,
                required: 4
            }
        ));
    }

    #[test]
    fn test_batch_error_recovery() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Processing -> Idle (error during processing)
        state.update_batch_status(BatchStatus::Processing {
            batch_id: 1,
            progress_pct: 50,
        });
        state.update_batch_status(BatchStatus::Idle);
        let network = state.network();
        assert!(matches!(network.batch_status, BatchStatus::Idle));

        // AwaitingSignatures -> Idle (timeout)
        state.update_batch_status(BatchStatus::Processing {
            batch_id: 2,
            progress_pct: 100,
        });
        state.update_batch_status(BatchStatus::AwaitingSignatures {
            batch_id: 2,
            collected: 1,
            required: 4,
        });
        state.update_batch_status(BatchStatus::Idle);
        let network = state.network();
        assert!(matches!(network.batch_status, BatchStatus::Idle));
    }

    #[test]
    fn test_batch_id_mismatch_in_updates() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Start processing batch 1
        state.update_batch_progress(1, 0);

        // Try to update batch 2's progress (different batch - should be rejected)
        state.update_batch_progress(2, 50);
        let network = state.network();
        // Should still be batch 1 at 0%
        assert!(matches!(
            network.batch_status,
            BatchStatus::Processing {
                batch_id: 1,
                progress_pct: 0
            }
        ));
    }

    #[test]
    fn test_processing_to_submitting_shortcut() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Processing -> Submitting (valid for batches that don't need signatures)
        state.update_batch_status(BatchStatus::Processing {
            batch_id: 1,
            progress_pct: 100,
        });
        state.update_batch_status(BatchStatus::Submitting { batch_id: 1 });

        let network = state.network();
        assert!(matches!(
            network.batch_status,
            BatchStatus::Submitting { batch_id: 1 }
        ));
    }

    // --- Hourly rotation tests ---

    #[test]
    fn test_rotate_hourly_counts() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Fill with initial data (10 hours of counts)
        for i in 0..10 {
            state.push_hourly_count(i * 10);
        }

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), 10);
        assert_eq!(metrics.hourly_tx_counts[0], 0);
        assert_eq!(metrics.hourly_tx_counts[9], 90);

        // Rotate (shift left, add 0 at end)
        state.rotate_hourly_counts();

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), 11);
        // Should have added 0 at end: [0, 10, 20, ..., 90, 0]
        assert_eq!(metrics.hourly_tx_counts[10], 0);
    }

    #[test]
    fn test_rotate_hourly_counts_at_capacity() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Fill to capacity (24 hours)
        for i in 0..HOURLY_TX_CAPACITY {
            state.push_hourly_count(i as u64);
        }

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), HOURLY_TX_CAPACITY);

        // Rotate (should drop oldest, add 0)
        state.rotate_hourly_counts();

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), HOURLY_TX_CAPACITY);
        // Oldest (0) should be dropped, newest should be 0
        assert_eq!(metrics.hourly_tx_counts[0], 1);
        assert_eq!(metrics.hourly_tx_counts[HOURLY_TX_CAPACITY - 1], 0);
    }

    #[test]
    fn test_rotate_hourly_counts_empty() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Rotate on empty state
        state.rotate_hourly_counts();

        let metrics = state.metrics();
        assert_eq!(metrics.hourly_tx_counts.len(), 1);
        assert_eq!(metrics.hourly_tx_counts[0], 0);
    }

    #[test]
    fn test_update_nockchain_api_status() {
        let state = BridgeStatus::new(Arc::new(RwLock::new(Vec::new())));

        // Default is Connecting
        let network = state.network();
        assert!(matches!(
            network.nockchain_api_status,
            NockchainApiStatus::Connecting { attempt: 0, .. }
        ));

        // Update to Connected
        state.update_nockchain_api_status(NockchainApiStatus::connected());
        let network = state.network();
        assert!(matches!(
            network.nockchain_api_status,
            NockchainApiStatus::Connected { .. }
        ));

        // Update to Disconnected
        state.update_nockchain_api_status(NockchainApiStatus::disconnected("error".to_string()));
        let network = state.network();
        assert!(matches!(
            network.nockchain_api_status,
            NockchainApiStatus::Disconnected { .. }
        ));
        assert_eq!(network.nockchain_api_status.last_error(), Some("error"));

        // Update to Connecting with attempt
        state.update_nockchain_api_status(NockchainApiStatus::connecting(
            5,
            Some("retry".to_string()),
        ));
        let network = state.network();
        match &network.nockchain_api_status {
            NockchainApiStatus::Connecting {
                attempt,
                last_error,
                ..
            } => {
                assert_eq!(*attempt, 5);
                assert_eq!(last_error.as_deref(), Some("retry"));
            }
            _ => panic!("expected Connecting"),
        }
    }
}
