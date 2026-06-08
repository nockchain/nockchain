use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::transports::ws::WsConnect;
use backon::Retryable;
use op_alloy::network::Optimism;
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::core::loop_policy::BaseObserverLoopPolicy;
use crate::observability::metrics;
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
    ready_notify: Notify,
}

impl Default for SequencerBaseHeightTracker {
    fn default() -> Self {
        Self {
            latest_confirmed_base_height: AtomicU64::new(0),
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

    /// Monotonically advances the tracked confirmed Base height.
    ///
    /// Returns `true` when the height advanced and `false` when the supplied
    /// height was stale or equal to the current value.
    pub fn record_confirmed_base_height(&self, height: u64) -> bool {
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
                self.ready_notify.notify_waiters();
                return true;
            }
        }
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

/// Polls the Base websocket for the latest confirmed height and persists that
/// monotonic progress into the sequencer's in-memory tracker.
pub async fn run_confirmed_base_height_watcher(
    ws_url: String,
    confirmation_depth: u64,
    tracker: Arc<SequencerBaseHeightTracker>,
    policy: BaseObserverLoopPolicy,
) -> Result<(), BridgeError> {
    let mut provider = connect_provider(&ws_url, policy).await?;

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

        let Some(confirmed_height) = confirmed_base_height(chain_tip, confirmation_depth) else {
            continue;
        };

        if tracker.record_confirmed_base_height(confirmed_height) {
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
    use super::{confirmed_base_height, SequencerBaseHeightTracker};

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
