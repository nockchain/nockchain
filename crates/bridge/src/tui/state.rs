//! TUI state management.
//!
//! Local UI state lives here; shared BridgeStatus lives in bridge_status and is wrapped by TuiStatus.

use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use tokio::sync::Notify;

pub use crate::bridge_status::BridgeStatus;
use crate::tui::types::{DepositLogSnapshot, DepositLogView, FocusedPanel, UiMode};

/// Capacity for log buffer.
/// Set to 100,000 to retain many hours of logs (~20MB memory at 200 bytes/line average).
/// At typical activity levels (1-2 logs/sec), this covers 14-28 hours of history.
pub const LOG_CAPACITY: usize = 100_000;

/// Log buffer type alias.
pub type LogBuffer = Arc<RwLock<VecDeque<String>>>;

/// Create a new log buffer.
pub fn new_log_buffer() -> LogBuffer {
    Arc::new(RwLock::new(VecDeque::with_capacity(LOG_CAPACITY)))
}

/// TUI wrapper around BridgeStatus holding TUI-only state.
#[derive(Clone, Debug)]
pub struct TuiStatus {
    inner: BridgeStatus,
    /// Deposit log snapshot (newest-first).
    deposit_log: Arc<RwLock<DepositLogSnapshot>>,
    /// Deposit log view (offset/limit).
    deposit_log_view: Arc<RwLock<DepositLogView>>,
    /// Whether the deposit log panel is currently focused.
    deposit_log_active: Arc<RwLock<bool>>,
    /// Notifies the TUI deposit log poller of view/activity/data changes.
    deposit_log_notify: Arc<Notify>,
    /// Log buffer.
    logs: LogBuffer,
    /// Temporary status message (text, expiry time, duration in seconds).
    status_message: Arc<RwLock<Option<(String, Instant, u64)>>>,
}

impl TuiStatus {
    /// Wrap an existing BridgeStatus with TUI-specific state.
    pub fn new(inner: BridgeStatus, logs: LogBuffer) -> Self {
        Self {
            inner,
            deposit_log: Arc::new(RwLock::new(DepositLogSnapshot::default())),
            deposit_log_view: Arc::new(RwLock::new(DepositLogView::default())),
            deposit_log_active: Arc::new(RwLock::new(false)),
            deposit_log_notify: Arc::new(Notify::new()),
            logs,
            status_message: Arc::new(RwLock::new(None)),
        }
    }

    /// Clone out the wrapped BridgeStatus for non-TUI helpers that take it by value.
    pub fn bridge_status(&self) -> BridgeStatus {
        self.inner.clone()
    }

    /// Get deposit log snapshot.
    pub fn deposit_log_snapshot(&self) -> DepositLogSnapshot {
        self.deposit_log
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Update deposit log snapshot.
    pub fn update_deposit_log_snapshot(&self, snapshot: DepositLogSnapshot) {
        if let Ok(mut guard) = self.deposit_log.write() {
            *guard = snapshot;
        }
    }

    /// Get deposit log view (offset/limit).
    pub fn deposit_log_view(&self) -> DepositLogView {
        self.deposit_log_view
            .read()
            .map(|guard| *guard)
            .unwrap_or_default()
    }

    /// Update deposit log view (offset/limit).
    pub fn set_deposit_log_view(&self, view: DepositLogView) {
        let mut changed = false;
        if let Ok(mut guard) = self.deposit_log_view.write() {
            if *guard != view {
                *guard = view;
                changed = true;
            }
        }
        if changed {
            self.notify_deposit_log_refresh();
        }
    }

    /// Check whether the deposit log panel is focused.
    pub fn deposit_log_active(&self) -> bool {
        self.deposit_log_active
            .read()
            .map(|guard| *guard)
            .unwrap_or(false)
    }

    /// Update whether the deposit log panel is focused.
    pub fn set_deposit_log_active(&self, active: bool) {
        let mut should_notify = false;
        if let Ok(mut guard) = self.deposit_log_active.write() {
            if *guard != active {
                *guard = active;
                should_notify = active;
            }
        }
        if should_notify {
            self.notify_deposit_log_refresh();
        }
    }

    /// Access the deposit log notifier for async refresh triggers.
    pub fn deposit_log_notifier(&self) -> Arc<Notify> {
        self.deposit_log_notify.clone()
    }

    /// Notify listeners that the deposit log snapshot should refresh.
    pub fn notify_deposit_log_refresh(&self) {
        self.deposit_log_notify.notify_one();
    }

    /// Get log lines.
    pub fn log_lines(&self) -> VecDeque<String> {
        self.logs
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Push a log line. Silently drops if lock is poisoned (background thread panicked).
    pub fn push_log(&self, line: String) {
        if let Ok(mut guard) = self.logs.write() {
            if guard.len() >= LOG_CAPACITY {
                guard.pop_front();
            }
            guard.push_back(line);
        }
    }

    // --- Status message methods ---

    /// Set a temporary status message that will auto-expire.
    ///
    /// The message will be displayed in the status bar until `duration_secs` seconds
    /// have elapsed since it was set. The render loop should check expiry and clear
    /// expired messages.
    ///
    /// # Arguments
    /// * `msg` - The status message text
    /// * `duration_secs` - How long the message should be displayed (in seconds)
    pub fn set_status_message(&self, msg: String, duration_secs: u64) {
        if let Ok(mut guard) = self.status_message.write() {
            *guard = Some((msg, Instant::now(), duration_secs));
        }
    }

    /// Get the current status message if present and not expired.
    ///
    /// Returns None if:
    /// - No message is set
    /// - The message has expired (elapsed time > duration)
    ///
    /// Returns Some((message, instant)) if message is still valid.
    pub fn status_message(&self) -> Option<(String, Instant)> {
        self.status_message
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .map(|(text, instant, _duration)| (text, instant))
    }

    /// Check if the current status message has expired and clear it if so.
    ///
    /// Should be called from the render loop on each tick.
    /// Returns true if a message was cleared.
    pub fn check_and_clear_expired_status(&self) -> bool {
        if let Ok(mut guard) = self.status_message.write() {
            if let Some((_, instant, duration_secs)) = guard.as_ref() {
                if instant.elapsed().as_secs() >= *duration_secs {
                    *guard = None;
                    return true;
                }
            }
        }
        false
    }

    /// Clear the current status message immediately.
    pub fn clear_status_message(&self) {
        if let Ok(mut guard) = self.status_message.write() {
            *guard = None;
        }
    }
}

impl Deref for TuiStatus {
    type Target = BridgeStatus;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tui_status_tests {
    use super::*;

    fn make_status() -> TuiStatus {
        let health = Arc::new(RwLock::new(Vec::new()));
        let core = BridgeStatus::new(health);
        TuiStatus::new(core, new_log_buffer())
    }

    #[test]
    fn test_set_status_message() {
        let state = make_status();

        // Initially no status message
        assert!(state.status_message().is_none());

        // Set a status message
        state.set_status_message("Copied to clipboard!".to_string(), 3);

        // Should have a message now
        let msg = state.status_message();
        assert!(msg.is_some());
        let (text, _instant) = msg.unwrap();
        assert_eq!(text, "Copied to clipboard!");
    }

    #[test]
    fn test_status_message_expiry_check() {
        let state = make_status();

        // Set a message that expires in 0 seconds (immediately expired)
        state.set_status_message("Test message".to_string(), 0);

        // check_and_clear_expired_status should return true (cleared an expired message)
        let cleared = state.check_and_clear_expired_status();
        assert!(cleared);

        // After clearing, should be None
        assert!(state.status_message().is_none());
    }

    #[test]
    fn test_status_message_overwrite() {
        let state = make_status();

        // Set first message
        state.set_status_message("First message".to_string(), 5);
        let msg = state.status_message();
        assert_eq!(msg.unwrap().0, "First message");

        // Set second message (should overwrite)
        state.set_status_message("Second message".to_string(), 5);
        let msg = state.status_message();
        assert_eq!(msg.unwrap().0, "Second message");
    }

    #[test]
    fn test_clear_status_message() {
        let state = make_status();

        // Set a message
        state.set_status_message("Test".to_string(), 5);
        assert!(state.status_message().is_some());

        // Clear it
        state.clear_status_message();
        assert!(state.status_message().is_none());
    }
}

/// Local UI state (not shared, owned by the TUI app).
#[derive(Clone, Debug, Default)]
pub struct LocalUiState {
    /// Current UI mode.
    pub mode: UiMode,
    /// Currently focused panel.
    pub focused_panel: FocusedPanel,
    /// Deposit log scroll offset (newest-first).
    pub deposit_log_offset: usize,
    /// Health table selection index.
    pub health_selection: Option<usize>,
    /// Proposal list selection index.
    pub proposal_selection: Option<usize>,
    /// Alert list selection index.
    pub alert_selection: Option<usize>,
    /// Whether to force a full redraw.
    pub force_redraw: bool,
    /// Track last focused panel to detect panel changes.
    pub last_focused_panel: FocusedPanel,
    /// Auto-jump to last deposit nonce on open.
    pub deposit_log_jump_pending: bool,
}

impl LocalUiState {
    pub fn new() -> Self {
        Self {
            force_redraw: true,
            last_focused_panel: FocusedPanel::Health,
            ..Default::default()
        }
    }

    /// Toggle help overlay.
    pub fn toggle_help(&mut self) {
        self.mode = if self.mode == UiMode::Help {
            UiMode::Normal
        } else {
            UiMode::Help
        };
        self.force_redraw = true;
    }

    /// Move focus to next panel.
    pub fn focus_next(&mut self) {
        self.focused_panel = self.focused_panel.next();
    }

    /// Move focus to previous panel.
    pub fn focus_prev(&mut self) {
        self.focused_panel = self.focused_panel.prev();
    }
}
