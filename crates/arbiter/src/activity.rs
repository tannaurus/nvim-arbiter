//! Backend activity tracking for statusline display.
//!
//! Pure state module with no Neovim API calls. Uses atomics for
//! lock-free access from the statusline polling path.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static BUSY: AtomicBool = AtomicBool::new(false);
static START_TIME: AtomicI64 = AtomicI64::new(0);

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Called from the backend queue when requests start or finish.
///
/// When `busy` is true, records the current time for elapsed display.
/// When false, clears the state.
pub(crate) fn set_busy(busy: bool) {
    if busy {
        START_TIME.store(now_millis(), Ordering::SeqCst);
        BUSY.store(true, Ordering::SeqCst);
    } else {
        BUSY.store(false, Ordering::SeqCst);
        START_TIME.store(0, Ordering::SeqCst);
    }
}

/// Returns a statusline-friendly string.
///
/// Empty when idle. Shows elapsed seconds when the backend is busy,
/// e.g. `[agent thinking... 12s]` or `[agent thinking... 12s | 2 queued]`.
pub(crate) fn statusline_component() -> String {
    if !BUSY.load(Ordering::SeqCst) {
        return String::new();
    }
    let start = START_TIME.load(Ordering::SeqCst);
    if start == 0 {
        return String::new();
    }
    let elapsed_secs = (now_millis() - start) / 1000;
    let queued = crate::backend::pending_count();
    if queued > 0 {
        format!("[agent thinking... {elapsed_secs}s | {queued} queued]")
    } else {
        format!("[agent thinking... {elapsed_secs}s]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_returns_empty() {
        set_busy(false);
        assert!(statusline_component().is_empty());
    }

    #[test]
    fn busy_lifecycle() {
        set_busy(false);
        assert!(statusline_component().is_empty());

        set_busy(true);
        let s = statusline_component();
        assert!(s.starts_with("[agent thinking..."), "got: {s}");
        assert!(s.ends_with("s]"), "got: {s}");

        set_busy(false);
        assert!(statusline_component().is_empty());
    }

    #[test]
    fn busy_then_idle_clears() {
        set_busy(true);
        set_busy(false);
        assert!(statusline_component().is_empty());
    }
}
