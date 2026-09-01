use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::transports::ws::WsConnect;
use backon::Retryable;
use op_alloy::network::Optimism;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::core::loop_policy::BaseObserverLoopPolicy;
use crate::observability::metrics;
use crate::shared::base::{query_withdrawal_contract_gate, validate_base_chain_id};
use crate::shared::errors::BridgeError;

fn is_rate_limit_error<E: std::fmt::Display>(err: &E) -> bool {
    let text = err.to_string().to_lowercase();
    text.contains("rate limit") || text.contains("-32005")
}

/// In-memory monotonic tracker for the latest confirmed Base height observed
/// by the sequencer process.
#[derive(Debug)]
pub struct SequencerBaseHeightTracker {
    latest_confirmed_base_height: AtomicU64,
    last_advanced_at_unix_ms: AtomicU64,
    withdrawals_enabled: AtomicU8,
    ready_notify: Notify,
}

impl Default for SequencerBaseHeightTracker {
    fn default() -> Self {
        Self {
            latest_confirmed_base_height: AtomicU64::new(0),
            last_advanced_at_unix_ms: AtomicU64::new(0),
            withdrawals_enabled: AtomicU8::new(0),
            ready_notify: Notify::new(),
        }
    }
}

impl SequencerBaseHeightTracker {
    /// Returns the most recent confirmed Base height the watcher has observed.
    pub fn latest_confirmed_base_height(&self) -> Option<u64> {
        let height = self.latest_confirmed_base_height.load(Ordering::SeqCst);
        (height > 0).then_some(height)
    }
    pub fn latest_confirmed_base_observation(&self) -> Option<(u64, u64)> {
        let height = self.latest_confirmed_base_height()?;
        let observed_at = self.last_advanced_at_unix_ms.load(Ordering::SeqCst);
        (observed_at > 0).then_some((height, observed_at))
    }
    pub fn withdrawals_enabled(&self) -> Option<bool> {
        match self.withdrawals_enabled.load(Ordering::SeqCst) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    pub fn record_withdrawals_enabled(&self, enabled: Option<bool>) {
        self.withdrawals_enabled.store(
            match enabled {
                Some(false) => 1,
                Some(true) => 2,
                None => 0,
            },
            Ordering::SeqCst,
        );
    }

    /// Monotonically advances the tracked confirmed Base height and records
    /// when real chain progress was last observed.
    ///
    /// Returns `true` when the height advanced and `false` when the supplied
    /// height was stale or equal to the current value.
    pub fn record_confirmed_base_observation(&self, height: u64, observed_at_unix_ms: u64) -> bool {
        if observed_at_unix_ms == 0 {
            return false;
        }
        loop {
            let current = self.latest_confirmed_base_height.load(Ordering::SeqCst);
            if height <= current {
                return false;
            }
            if self
                .latest_confirmed_base_height
                .compare_exchange(current, height, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                self.last_advanced_at_unix_ms
                    .store(observed_at_unix_ms, Ordering::SeqCst);
                self.ready_notify.notify_waiters();
                return true;
            }
        }
    }

    pub fn record_confirmed_base_height(&self, height: u64) -> bool {
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or_default();
        self.record_confirmed_base_observation(height, observed_at_unix_ms)
    }

    /// Waits until the watcher has observed at least one confirmed Base height.
    pub async fn wait_for_initial_confirmed_base_height(&self) -> u64 {
        loop {
            if let Some(height) = self.latest_confirmed_base_height() {
                return height;
            }
            self.ready_notify.notified().await;
        }
    }
}

async fn connect_provider(
    ws_url: &str,
    policy: BaseObserverLoopPolicy,
) -> Result<DynProvider<Optimism>, BridgeError> {
    let connect = || async {
        ProviderBuilder::<_, _, Optimism>::default()
            .connect_ws(WsConnect::new(ws_url.to_string()))
            .await
    };
    connect
        .retry(policy.rpc_retry.exponential_builder())
        .notify(|err, dur| {
            warn!(
                target: "nockchain.withdrawal_sequencer.base_height",
                error = %err,
                backoff_secs = dur.as_secs(),
                "failed to connect base height watcher, will retry"
            );
        })
        .await
        .map(|provider| provider.erased())
        .map_err(|err| {
            BridgeError::Runtime(format!(
                "failed to connect base height watcher at {ws_url}: {err}"
            ))
        })
}

fn confirmed_base_height(chain_tip: u64, confirmation_depth: u64) -> Option<u64> {
    let confirmed_height = if confirmation_depth == 0 {
        chain_tip
    } else {
        chain_tip.saturating_sub(confirmation_depth)
    };
    (confirmed_height > 0).then_some(confirmed_height)
}
fn validate_observed_message_inbox(
    expected: Option<Address>,
    observed: Address,
) -> Result<(), BridgeError> {
    if let Some(expected) = expected {
        if expected != observed {
            return Err(BridgeError::Config(format!(
                "public withdrawal MessageInbox mismatch: expected {expected}, observed {observed}"
            )));
        }
    }
    Ok(())
}

/// Polls the Base websocket for the latest confirmed height and persists that
/// monotonic progress into the sequencer's in-memory tracker.
pub async fn run_confirmed_base_height_watcher(
    ws_url: String,
    expected_chain_id: u64,
    confirmation_depth: u64,
    nock_contract_address: Address,
    expected_message_inbox_address: Option<Address>,
    tracker: Arc<SequencerBaseHeightTracker>,
    policy: BaseObserverLoopPolicy,
) -> Result<(), BridgeError> {
    let mut provider = connect_provider(&ws_url, policy).await?;
    validate_base_chain_id(
        &provider, expected_chain_id, "sequencer Base height watcher",
    )
    .await?;

    loop {
        let chain_tip = match (|| async { provider.get_block_number().await })
            .retry(policy.rpc_retry.exponential_builder())
            .when(is_rate_limit_error)
            .notify(|err, dur| {
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_height",
                    error = %err,
                    backoff_secs = dur.as_secs(),
                    "failed to fetch base tip height, will retry"
                );
            })
            .await
        {
            Ok(tip) => tip,
            Err(err) => {
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_height",
                    error = %err,
                    "failed to fetch base tip height after retries, reconnecting watcher"
                );
                provider = connect_provider(&ws_url, policy).await?;
                continue;
            }
        };

        match query_withdrawal_contract_gate(&provider, nock_contract_address).await {
            Ok((observed_message_inbox, enabled)) => {
                if let Err(error) = validate_observed_message_inbox(
                    expected_message_inbox_address, observed_message_inbox,
                ) {
                    tracker.record_withdrawals_enabled(None);
                    return Err(error);
                }
                tracker.record_withdrawals_enabled(Some(enabled));
            }
            Err(error) => {
                tracker.record_withdrawals_enabled(None);
                warn!(
                    target: "nockchain.withdrawal_sequencer.base_height",
                    error = %error,
                    "failed to observe withdrawal contract gate"
                );
            }
        }
        let Some(confirmed_height) = confirmed_base_height(chain_tip, confirmation_depth) else {
            continue;
        };

        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| BridgeError::Runtime(format!("system time before unix epoch: {err}")))
            .and_then(|duration| {
                u64::try_from(duration.as_millis()).map_err(|err| {
                    BridgeError::ValueConversion(format!(
                        "Base observation timestamp overflow: {err}"
                    ))
                })
            })?;
        if tracker.record_confirmed_base_observation(confirmed_height, observed_at_unix_ms) {
            metrics::init_metrics()
                .sequencer_withdrawal_base_confirmed_height
                .swap(confirmed_height as f64);
            info!(
                target: "nockchain.withdrawal_sequencer.base_height",
                chain_tip,
                confirmed_height,
                "advanced sequencer confirmed base height"
            );
        }

        sleep(policy.poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::Address;

    use super::{
        confirmed_base_height, validate_observed_message_inbox, SequencerBaseHeightTracker,
    };

    #[test]
    fn confirmed_base_height_is_monotonic() {
        let tracker = SequencerBaseHeightTracker::default();

        assert_eq!(tracker.latest_confirmed_base_height(), None);

        assert!(tracker.record_confirmed_base_height(100));
        assert_eq!(tracker.latest_confirmed_base_height(), Some(100));

        assert!(!tracker.record_confirmed_base_height(100));
        assert!(!tracker.record_confirmed_base_height(99));
        assert_eq!(tracker.latest_confirmed_base_height(), Some(100));

        assert!(tracker.record_confirmed_base_height(101));
        assert_eq!(tracker.latest_confirmed_base_height(), Some(101));
    }
    #[test]
    fn observation_freshness_advances_only_with_chain_height() {
        let tracker = SequencerBaseHeightTracker::default();
        assert_eq!(tracker.latest_confirmed_base_observation(), None);
        assert!(tracker.record_confirmed_base_observation(100, 1_000));
        assert_eq!(
            tracker.latest_confirmed_base_observation(),
            Some((100, 1_000))
        );
        assert!(!tracker.record_confirmed_base_observation(100, 2_000));
        assert_eq!(
            tracker.latest_confirmed_base_observation(),
            Some((100, 1_000))
        );
        assert!(tracker.record_confirmed_base_observation(101, 2_000));
        assert_eq!(
            tracker.latest_confirmed_base_observation(),
            Some((101, 2_000))
        );
    }

    #[test]
    fn withdrawal_gate_observation_is_tristate() {
        let tracker = SequencerBaseHeightTracker::default();
        assert_eq!(tracker.withdrawals_enabled(), None);
        tracker.record_withdrawals_enabled(Some(false));
        assert_eq!(tracker.withdrawals_enabled(), Some(false));
        tracker.record_withdrawals_enabled(Some(true));
        assert_eq!(tracker.withdrawals_enabled(), Some(true));
        tracker.record_withdrawals_enabled(None);
        assert_eq!(tracker.withdrawals_enabled(), None);
    }

    #[test]
    fn configured_message_inbox_must_match_the_observed_nock_pair() {
        let observed = Address::from([0x11; 20]);
        assert!(validate_observed_message_inbox(None, observed).is_ok());
        assert!(validate_observed_message_inbox(Some(observed), observed).is_ok());
        assert!(
            validate_observed_message_inbox(Some(Address::from([0x22; 20])), observed).is_err()
        );
    }

    #[test]
    fn zero_depth_uses_current_tip() {
        assert_eq!(confirmed_base_height(0, 0), None);
        assert_eq!(confirmed_base_height(25, 0), Some(25));
    }

    #[test]
    fn positive_depth_preserves_existing_subtraction_behavior() {
        assert_eq!(confirmed_base_height(100, 1), Some(99));
        assert_eq!(confirmed_base_height(100, 100), None);
    }
}
