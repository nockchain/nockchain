//! Tracking for driver work that must survive until it completes.
//!
//! A kernel may emit `%exit` in the same effect list as an effect whose driver
//! performs a network round-trip. Effects are broadcast to drivers concurrently,
//! so nothing orders the round-trip before shutdown: `NockApp::run` returns, the
//! runtime is dropped, and the in-flight future is aborted with no diagnostic --
//! cancellation is not an error, so neither side logs anything.
//!
//! Effects reach every driver in emission order, so a driver that observes an
//! outbound-work effect can register it before the later `%exit` is handled.
//! The performing driver completes it, and the exit path blocks until the count
//! reaches zero.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::Notify;

static PENDING: AtomicUsize = AtomicUsize::new(0);

fn notify() -> &'static Notify {
    static NOTIFY: OnceLock<Notify> = OnceLock::new();
    NOTIFY.get_or_init(Notify::new)
}

/// Record outbound work that shutdown must wait for.
pub fn register() {
    PENDING.fetch_add(1, Ordering::SeqCst);
}

/// Mark one unit of outbound work finished, whatever its outcome. A failed send
/// still counts: the exit path waits for resolution, not for success.
pub fn complete() {
    if PENDING
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
    {
        notify().notify_waiters();
    }
}

/// Block until all registered work resolves, or `timeout` elapses. Returns
/// whether it drained. The timeout bounds the case where the effect was
/// registered but no driver is attached to perform it.
pub async fn drain(timeout: Duration) -> bool {
    if PENDING.load(Ordering::SeqCst) == 0 {
        return true;
    }
    tokio::time::timeout(timeout, async {
        loop {
            // Subscribe before re-checking so a completion landing in between
            // cannot be missed.
            let waiter = notify().notified();
            if PENDING.load(Ordering::SeqCst) == 0 {
                return;
            }
            waiter.await;
        }
    })
    .await
    .is_ok()
}
