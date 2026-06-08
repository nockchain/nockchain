//! Catch-up signal for the libp2p driver.
//!
//! Phase 1 of the catch-up prefetch epic computes whether the local node is at
//! tip, catching up, or cold, and exposes the answer plus a few derived numbers
//! for metrics. Later phases use the signal to decide when block-range prefetch
//! should replace singleton block requests.
//!
//! The signal blends three inputs:
//!
//! * `frontier`: `P2PState::first_negative`, the next height the kernel has
//!   not yet confirmed. Advances when the kernel emits a `%seen %block` for a
//!   new heaviest height (see `driver.rs` Seen-effect handling).
//! * `max_deferred_height`: highest height held in
//!   `P2PState::deferred_heard_blocks`. Future blocks heard from gossip that
//!   we cannot yet process are positive evidence we are behind tip.
//! * `peer_observed_max_height`: highest height observed in any successful
//!   inbound block response. A peer that recently served us height `H`
//!   demonstrably has it; if `H >> frontier`, we are demonstrably behind.
//!
//! The mode rules use asymmetric thresholds so we enter `CatchingUp`
//! aggressively and exit conservatively, preventing flapping near tip.
//!
//! Hysteresis is applied only on the `CatchingUp -> Tip` transition: we
//! require the "drained" condition (`max_deferred_height - frontier <= 1`)
//! to hold for `HYSTERESIS_MS` before declaring `Tip`. Read paths that gate
//! live traffic call `refresh_mode` to let this timer expire without waiting
//! for another block/deferred event.

use std::time::{Duration, Instant};

/// Enter `CatchingUp` when this many heard-but-undprocessed blocks sit above
/// the kernel frontier in the deferred queue. Set low enough that a small
/// burst of gossip arrivals while we process one block does not trip it, but
/// high enough that the buffer reflects sustained backlog rather than
/// transient skew.
pub const BEHIND_TIP_DEFERRED_THRESHOLD: u64 = 8;

/// Enter `CatchingUp` when a peer demonstrably has a height this far above
/// our frontier. Larger than the deferred threshold because peer-observed
/// height is a stronger signal of real distance to tip than buffered gossip.
pub const BEHIND_TIP_PEER_OBSERVED_THRESHOLD: u64 = 32;

/// How long the drained condition must hold before `CatchingUp -> Tip`.
pub const HYSTERESIS_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// First boot: frontier is still 0 and we have not yet validated any
    /// block. Distinct from `CatchingUp` because a node that just came up
    /// has no buffered gossip and no prior peer observations to lean on.
    Cold,
    /// Demonstrably behind tip and worth treating that way.
    CatchingUp,
    /// At or near tip.
    Tip,
}

impl SyncMode {
    pub fn as_metric_value(self) -> f64 {
        match self {
            SyncMode::Cold => 0.0,
            SyncMode::CatchingUp => 1.0,
            SyncMode::Tip => 2.0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::Cold => "cold",
            SyncMode::CatchingUp => "catching_up",
            SyncMode::Tip => "tip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CatchUpSignal {
    frontier: u64,
    max_deferred_height: u64,
    peer_observed_max_height: u64,
    mode: SyncMode,
    /// First time the drained condition was observed continuously while in
    /// `CatchingUp`. Cleared when the condition breaks. Used for hysteresis.
    drained_since: Option<Instant>,
    /// Total number of `mode` transitions since construction.
    transitions: u64,
    behind_tip_threshold: u64,
    peer_observed_threshold: u64,
    hysteresis: Duration,
}

impl Default for CatchUpSignal {
    fn default() -> Self {
        Self {
            frontier: 0,
            max_deferred_height: 0,
            peer_observed_max_height: 0,
            mode: SyncMode::Cold,
            drained_since: None,
            transitions: 0,
            behind_tip_threshold: BEHIND_TIP_DEFERRED_THRESHOLD,
            peer_observed_threshold: BEHIND_TIP_PEER_OBSERVED_THRESHOLD,
            hysteresis: Duration::from_millis(HYSTERESIS_MS),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModeTransition {
    pub from: SyncMode,
    pub to: SyncMode,
}

impl CatchUpSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure(
        &mut self,
        behind_tip_threshold: u64,
        peer_observed_threshold: u64,
        hysteresis: Duration,
    ) {
        self.behind_tip_threshold = behind_tip_threshold.max(1);
        self.peer_observed_threshold = peer_observed_threshold.max(1);
        self.hysteresis = hysteresis;
    }

    #[cfg(test)]
    fn with_thresholds(deferred: u64, peer_observed: u64, hysteresis: Duration) -> Self {
        Self {
            behind_tip_threshold: deferred,
            peer_observed_threshold: peer_observed,
            hysteresis,
            ..Self::default()
        }
    }

    pub fn mode(&self) -> SyncMode {
        self.mode
    }

    /// True when the node is demonstrably behind tip (`SyncMode::CatchingUp`).
    /// Used to suppress all outgoing gossip while catching up. That includes
    /// historic block rebroadcasts, tx submission gossip, and mining output.
    /// A catching-up node stays quiet until this returns to `false`.
    pub fn is_catching_up(&self) -> bool {
        matches!(self.mode, SyncMode::CatchingUp)
    }

    pub fn transitions(&self) -> u64 {
        self.transitions
    }

    pub fn frontier(&self) -> u64 {
        self.frontier
    }

    pub fn max_deferred_height(&self) -> u64 {
        self.max_deferred_height
    }

    pub fn peer_observed_max_height(&self) -> u64 {
        self.peer_observed_max_height
    }

    /// Lower bound on `tip - frontier`, derived from the strongest available
    /// signal. Saturates at 0 if no signal indicates we are behind.
    pub fn behind_tip_estimate(&self) -> u64 {
        let from_deferred = self.max_deferred_height.saturating_sub(self.frontier);
        let from_peer = self.peer_observed_max_height.saturating_sub(self.frontier);
        from_deferred.max(from_peer)
    }

    /// Number of deferred-heard blocks ahead of the frontier that would be
    /// processed when frontier advances. Saturates at 0.
    pub fn deferred_blocks_above_frontier(&self) -> u64 {
        self.max_deferred_height.saturating_sub(self.frontier)
    }

    /// Notify the signal that the kernel frontier advanced. Should be called
    /// after `P2PState::first_negative` is bumped on a `%seen %block` effect
    /// for a new heaviest height.
    pub fn note_frontier_advance(
        &mut self,
        now: Instant,
        new_frontier: u64,
    ) -> Option<ModeTransition> {
        if new_frontier > self.frontier {
            self.frontier = new_frontier;
        }
        self.recompute_mode(now)
    }

    /// Notify the signal that the deferred-heard-block buffer changed (item
    /// inserted, removed, or drained). Caller passes the current
    /// `BTreeMap::last_key_value` height (or `None` if the buffer is empty).
    pub fn note_deferred_max_height(
        &mut self,
        now: Instant,
        max_deferred_height: Option<u64>,
    ) -> Option<ModeTransition> {
        self.max_deferred_height = max_deferred_height.unwrap_or(0);
        self.recompute_mode(now)
    }

    /// Notify the signal that a successful response from a peer carried a
    /// block at the given height. Used to update
    /// `peer_observed_max_height`.
    pub fn note_peer_response_height(
        &mut self,
        now: Instant,
        height: u64,
    ) -> Option<ModeTransition> {
        if height > self.peer_observed_max_height {
            self.peer_observed_max_height = height;
        }
        self.recompute_mode(now)
    }

    /// Recompute the mode without changing any signal inputs. This lets
    /// `CatchingUp -> Tip` hysteresis expire on read-side gates.
    pub fn refresh_mode(&mut self, now: Instant) -> Option<ModeTransition> {
        self.recompute_mode(now)
    }

    fn recompute_mode(&mut self, now: Instant) -> Option<ModeTransition> {
        let next = self.next_mode(now);
        if next == self.mode {
            return None;
        }
        let transition = ModeTransition {
            from: self.mode,
            to: next,
        };
        self.mode = next;
        self.transitions = self.transitions.saturating_add(1);
        Some(transition)
    }

    fn next_mode(&mut self, now: Instant) -> SyncMode {
        let deferred_above = self.deferred_blocks_above_frontier();
        let peer_above = self.peer_observed_max_height.saturating_sub(self.frontier);
        let drained = deferred_above <= 1;

        match self.mode {
            SyncMode::Cold => {
                if deferred_above >= self.behind_tip_threshold
                    || peer_above >= self.peer_observed_threshold
                {
                    self.drained_since = None;
                    SyncMode::CatchingUp
                } else if self.frontier > 0 {
                    // We have processed at least one block, and nothing
                    // suggests we are behind, declare Tip.
                    SyncMode::Tip
                } else {
                    // Cold is sticky until either CatchingUp evidence shows
                    // up or the kernel advances the frontier.
                    SyncMode::Cold
                }
            }
            SyncMode::CatchingUp => {
                if drained {
                    let since = self.drained_since.get_or_insert(now);
                    if now.saturating_duration_since(*since) >= self.hysteresis {
                        self.drained_since = None;
                        SyncMode::Tip
                    } else {
                        SyncMode::CatchingUp
                    }
                } else {
                    self.drained_since = None;
                    SyncMode::CatchingUp
                }
            }
            SyncMode::Tip => {
                if deferred_above >= self.behind_tip_threshold
                    || peer_above >= self.peer_observed_threshold
                {
                    self.drained_since = None;
                    SyncMode::CatchingUp
                } else {
                    SyncMode::Tip
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn starts_cold() {
        let signal = CatchUpSignal::new();
        assert_eq!(signal.mode(), SyncMode::Cold);
        assert_eq!(signal.behind_tip_estimate(), 0);
        assert_eq!(signal.transitions(), 0);
    }

    #[test]
    fn cold_to_tip_when_first_advance_with_no_backlog() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        let t = signal.note_frontier_advance(now, 1);
        assert_eq!(t.map(|t| t.to), Some(SyncMode::Tip));
        assert_eq!(signal.mode(), SyncMode::Tip);
    }

    #[test]
    fn cold_to_catching_up_when_deferred_above_threshold() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        // Heard a block via gossip 100 above us before we ever advanced.
        let t = signal.note_deferred_max_height(now, Some(100));
        assert_eq!(t.map(|t| t.to), Some(SyncMode::CatchingUp));
    }

    #[test]
    fn cold_to_catching_up_via_peer_observation() {
        let now = t0();
        let mut signal = CatchUpSignal::with_thresholds(
            BEHIND_TIP_DEFERRED_THRESHOLD,
            BEHIND_TIP_PEER_OBSERVED_THRESHOLD,
            Duration::from_millis(HYSTERESIS_MS),
        );
        let t = signal.note_peer_response_height(now, 100);
        assert_eq!(t.map(|t| t.to), Some(SyncMode::CatchingUp));
    }

    #[test]
    fn tip_to_catching_up_when_backlog_grows() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        signal.note_frontier_advance(now, 50);
        assert_eq!(signal.mode(), SyncMode::Tip);
        let t = signal.note_deferred_max_height(now, Some(50 + BEHIND_TIP_DEFERRED_THRESHOLD));
        assert_eq!(t.map(|t| t.to), Some(SyncMode::CatchingUp));
    }

    #[test]
    fn catching_up_to_tip_requires_hysteresis() {
        let mut signal = CatchUpSignal::with_thresholds(8, 32, Duration::from_millis(30_000));
        let now = t0();
        signal.note_deferred_max_height(now, Some(100));
        signal.note_frontier_advance(now, 1);
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        // Drain the buffer.
        let drained_at = now + Duration::from_millis(10_000);
        let t = signal.note_deferred_max_height(drained_at, None);
        assert!(t.is_none(), "must not flip to Tip without hysteresis");
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        // Still inside hysteresis window, no transition.
        let still_inside = drained_at + Duration::from_millis(20_000);
        let t = signal.note_frontier_advance(still_inside, 101);
        assert!(t.is_none());
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        // Past the hysteresis window, transition to Tip.
        let past_hysteresis = drained_at + Duration::from_millis(30_001);
        let t = signal.note_frontier_advance(past_hysteresis, 102);
        assert_eq!(t.map(|t| t.to), Some(SyncMode::Tip));
    }

    #[test]
    fn refresh_exits_catching_up_after_hysteresis_without_new_input() {
        let mut signal = CatchUpSignal::with_thresholds(8, 32, Duration::from_millis(30_000));
        let now = t0();
        signal.note_deferred_max_height(now, Some(100));
        signal.note_frontier_advance(now, 1);
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        let drained_at = now + Duration::from_millis(10_000);
        let t = signal.note_deferred_max_height(drained_at, None);
        assert!(t.is_none(), "must not flip to Tip without hysteresis");
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        let still_inside = drained_at + Duration::from_millis(20_000);
        let t = signal.refresh_mode(still_inside);
        assert!(t.is_none());
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        let past_hysteresis = drained_at + Duration::from_millis(30_001);
        let t = signal.refresh_mode(past_hysteresis);
        assert_eq!(t.map(|t| t.to), Some(SyncMode::Tip));
        assert!(!signal.is_catching_up());
    }

    #[test]
    fn drained_resets_when_backlog_returns() {
        let mut signal = CatchUpSignal::with_thresholds(8, 32, Duration::from_millis(30_000));
        let now = t0();
        signal.note_deferred_max_height(now, Some(100));
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        // Briefly drained.
        let briefly_drained = now + Duration::from_millis(10_000);
        signal.note_deferred_max_height(briefly_drained, None);
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        // Backlog returns before hysteresis elapses; drained timer should
        // reset, so even if we drain again we restart the clock.
        let backlog_returns = briefly_drained + Duration::from_millis(5_000);
        signal.note_deferred_max_height(backlog_returns, Some(200));
        assert_eq!(signal.mode(), SyncMode::CatchingUp);

        let drained_again = backlog_returns + Duration::from_millis(5_000);
        signal.note_deferred_max_height(drained_again, None);

        // 25s after the first drain, but only 5s after the most recent one,
        // we must still be CatchingUp because the hysteresis clock restarts.
        let probe = drained_again + Duration::from_millis(20_000);
        let t = signal.note_frontier_advance(probe, 201);
        assert!(t.is_none());
        assert_eq!(signal.mode(), SyncMode::CatchingUp);
    }

    #[test]
    fn behind_tip_estimate_uses_strongest_signal() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        signal.note_frontier_advance(now, 100);
        signal.note_deferred_max_height(now, Some(110));
        signal.note_peer_response_height(now, 500);
        assert_eq!(signal.behind_tip_estimate(), 400);
    }

    #[test]
    fn frontier_advance_does_not_regress() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        signal.note_frontier_advance(now, 100);
        signal.note_frontier_advance(now, 50);
        assert_eq!(signal.frontier(), 100);
    }

    #[test]
    fn is_catching_up_only_in_catching_up_mode() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        // Cold at boot: not suppressed (we do not yet know we are behind).
        assert_eq!(signal.mode(), SyncMode::Cold);
        assert!(!signal.is_catching_up());

        // Demonstrably behind tip: suppressed.
        signal.note_deferred_max_height(now, Some(100));
        assert_eq!(signal.mode(), SyncMode::CatchingUp);
        assert!(signal.is_catching_up());

        // Back at tip: not suppressed.
        let mut tip_signal = CatchUpSignal::new();
        tip_signal.note_frontier_advance(now, 1);
        assert_eq!(tip_signal.mode(), SyncMode::Tip);
        assert!(!tip_signal.is_catching_up());
    }

    #[test]
    fn transitions_counter_increments_only_on_change() {
        let now = t0();
        let mut signal = CatchUpSignal::new();
        signal.note_frontier_advance(now, 1); // Cold -> Tip (+1)
        signal.note_frontier_advance(now, 2); // Tip -> Tip (no change)
        signal.note_deferred_max_height(now, Some(100)); // Tip -> CatchingUp (+1)
        signal.note_deferred_max_height(now, Some(200)); // CatchingUp -> CatchingUp
        assert_eq!(signal.transitions(), 2);
    }
}
