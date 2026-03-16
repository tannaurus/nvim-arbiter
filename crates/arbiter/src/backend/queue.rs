//! FIFO queue for backend CLI calls.
//!
//! Serializes all adapter invocations. Max concurrency 1.
//! Uses generation counter for cancel_all; DrainGuard ensures
//! process_next runs even when callbacks panic.

use crate::backend::Adapter;
use crate::types::{BackendOpts, OnComplete, OnStream};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static ADAPTER: Mutex<Option<Arc<dyn Adapter + Send + Sync>>> = Mutex::new(None);

/// Stores the adapter. Overwrites any previous adapter (allows test injection).
pub(super) fn set_adapter(adapter: Arc<dyn Adapter + Send + Sync>) {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    *ADAPTER.lock().expect("adapter lock") = Some(adapter);
}

fn get_adapter() -> Option<Arc<dyn Adapter + Send + Sync>> {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    ADAPTER.lock().expect("adapter lock").clone()
}

/// A single queued backend call.
pub(super) struct QueueItem {
    pub opts: BackendOpts,
    pub on_stream: Option<OnStream>,
    pub callback: OnComplete,
    /// Optional tag for scoped cancellation (typically a thread ID).
    pub tag: Option<String>,
}

static QUEUE: Mutex<VecDeque<QueueItem>> = Mutex::new(VecDeque::new());
static PROCESSING: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static INFLIGHT_TAG: Mutex<Option<String>> = Mutex::new(None);

/// Drop guard that calls `process_next()` on drop.
///
/// Ensures the queue continues draining even if a callback panics.
struct DrainGuard;

impl Drop for DrainGuard {
    fn drop(&mut self) {
        process_next();
    }
}

/// Pushes an item onto the queue and kicks processing if idle.
pub(super) fn push(item: QueueItem) {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    QUEUE.lock().expect("queue lock").push_back(item);
    if !PROCESSING.swap(true, Ordering::SeqCst) {
        process_next();
    }
}

/// Processes the next item. Called by push and by DrainGuard.
fn process_next() {
    // Pop and set the in-flight tag atomically so cancel_tagged always
    // sees the correct tag for the item being processed.
    let item = {
        // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
        let mut q = QUEUE.lock().expect("queue lock");
        let item = q.pop_front();
        // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
        *INFLIGHT_TAG.lock().expect("inflight_tag lock") =
            item.as_ref().and_then(|i| i.tag.clone());
        item
    };
    match item {
        Some(item) => {
            crate::activity::set_busy(true);
            let gen = GENERATION.load(Ordering::SeqCst);
            let Some(adapter) = get_adapter() else {
                crate::dispatch::schedule(|| {
                    nvim_oxi::api::err_writeln(
                        "[arbiter] backend adapter not initialized; run setup() first",
                    );
                });
                PROCESSING.store(false, Ordering::SeqCst);
                crate::activity::set_busy(false);
                return;
            };
            let guarded_stream = item.on_stream.map(|cb| -> OnStream {
                let gen_at_start = gen;
                std::sync::Arc::new(move |chunk: &str| {
                    if GENERATION.load(Ordering::SeqCst) == gen_at_start {
                        cb(chunk);
                    }
                })
            });
            adapter.execute(
                item.opts,
                guarded_stream,
                Box::new(move |result| {
                    let _guard = DrainGuard;
                    if GENERATION.load(Ordering::SeqCst) != gen {
                        return;
                    }
                    (item.callback)(result);
                }),
            );
        }
        None => {
            // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
            *INFLIGHT_TAG.lock().expect("inflight_tag lock") = None;
            PROCESSING.store(false, Ordering::SeqCst);
            crate::activity::set_busy(false);
        }
    }
}

/// Returns the number of items waiting in the queue (excludes in-flight).
pub(super) fn pending_count() -> usize {
    QUEUE.lock().expect("queue lock").len()
}

/// Returns true if the queue has pending items or one is in flight.
#[cfg(test)]
pub(super) fn is_busy() -> bool {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    PROCESSING.load(Ordering::SeqCst) || !QUEUE.lock().expect("queue lock").is_empty()
}

/// Returns the tag of the currently in-flight request, if any.
pub(super) fn inflight_tag() -> Option<String> {
    INFLIGHT_TAG.lock().expect("inflight_tag lock").clone()
}

/// Cancels all pending items and causes in-flight callbacks to no-op.
pub(super) fn cancel_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    QUEUE.lock().expect("queue lock").clear();
    *INFLIGHT_TAG.lock().expect("inflight_tag lock") = None;
    PROCESSING.store(false, Ordering::SeqCst);
    crate::activity::set_busy(false);
}

/// Cancels queued items with the given tag and invalidates the in-flight
/// request if it carries the same tag. Other requests are left untouched.
pub(super) fn cancel_tagged(tag: &str) {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    QUEUE
        .lock()
        .expect("queue lock")
        .retain(|item| item.tag.as_deref() != Some(tag));

    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    let inflight = INFLIGHT_TAG.lock().expect("inflight_tag lock");
    if inflight.as_deref() == Some(tag) {
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}
