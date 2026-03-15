//! Cross-thread dispatch to the Neovim main thread.
//!
//! `nvim_oxi::schedule` panics when called from background threads because
//! the Lua state is stored in a thread-local that's only initialized on
//! the main thread. This module provides `schedule()` as a safe replacement
//! using `nvim_oxi::libuv::AsyncHandle`, which signals the main-thread
//! event loop via libuv's `uv_async_send`.
//!
//! Callbacks are wrapped in `catch_unwind` so a panic in one callback
//! does not crash Neovim. On panic, the active review is closed (cleaning
//! up the tabpage) and a user-visible error is displayed.

use nvim_oxi::libuv::AsyncHandle;
use std::collections::VecDeque;
use std::sync::Mutex;

type MainThreadFn = Box<dyn FnOnce() + Send + 'static>;

static QUEUE: Mutex<VecDeque<MainThreadFn>> = Mutex::new(VecDeque::new());
static HANDLE: Mutex<Option<AsyncHandle>> = Mutex::new(None);

/// Initializes the dispatcher. Must be called once from the main thread
/// during plugin setup, before any background threads call `schedule()`.
pub fn init() -> nvim_oxi::Result<()> {
    let handle = AsyncHandle::new(drain)?;
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    *HANDLE.lock().expect("dispatch handle lock") = Some(handle);
    Ok(())
}

fn drain() {
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    let items: Vec<MainThreadFn> = QUEUE
        .lock()
        .expect("dispatch queue lock")
        .drain(..)
        .collect();
    for f in items {
        nvim_oxi::schedule(move |_| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(()) => {}
                Err(payload) => {
                    let msg = panic_message(&payload);
                    crate::review::close();
                    let _ = nvim_oxi::api::notify(
                        &format!("[arbiter] internal error: {msg}"),
                        nvim_oxi::api::types::LogLevel::Error,
                        &nvim_oxi::Dictionary::default(),
                    );
                }
            }
        });
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Schedules a closure to run on the Neovim main thread.
///
/// Safe to call from any thread. The closure runs on the next libuv
/// event loop iteration via `AsyncHandle::send()`. If the closure
/// panics, the review workbench is closed and an error is displayed.
pub fn schedule<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    QUEUE
        .lock()
        .expect("dispatch queue lock")
        .push_back(Box::new(f));
    // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
    if let Some(ref handle) = *HANDLE.lock().expect("dispatch handle lock") {
        let _ = handle.send();
    }
}
