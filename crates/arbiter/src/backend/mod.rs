//! Backend adapter, queue, and CLI integration.
//!
//! Abstracts over Cursor and Claude CLI. FIFO queue for all calls.
//! Setup stores config and selects adapter; send enqueues items.

mod adapter;
mod claude;
mod cursor;
mod queue;
mod response;

use crate::types::{BackendOp, BackendOpts, OnComplete, OnStream};
pub(crate) use adapter::Adapter;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Minimal config for backend setup. Workspace defaults to cwd at setup time.
#[derive(Debug, Clone)]
pub(crate) struct BackendConfig {
    /// `"cursor"` or `"claude"`.
    pub(crate) backend: String,
    /// Optional model override.
    pub(crate) model: Option<String>,
    /// Workspace root. Passed to CLI as --workspace or --add-dir.
    pub(crate) workspace: Option<String>,
    /// Extra CLI flags appended to every invocation.
    pub(crate) extra_args: Vec<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            backend: "cursor".to_string(),
            model: None,
            workspace: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from)),
            extra_args: Vec::new(),
        }
    }
}

/// Registers a callback invoked when a tagged queue item starts processing.
/// Used by the review module to update the thread panel status from
/// "queued" to "thinking" when the queue advances.
pub(crate) fn set_on_item_started(cb: Box<dyn Fn(&str) + Send + Sync>) {
    queue::set_on_item_started(cb);
}

/// Registers a callback invoked when the queue drains and no items remain
/// in-flight. Used to clear activity indicators in the thread list popup.
pub(crate) fn set_on_queue_idle(cb: Box<dyn Fn() + Send + Sync>) {
    queue::set_on_queue_idle(cb);
}

/// Initializes the backend. Stores config and selects adapter.
///
/// For E3-1, uses a no-op adapter until E3-2 provides real implementations.
/// Call `setup_with_adapter` from tests to inject a mock.
pub(crate) fn setup(config: BackendConfig) {
    let adapter: Arc<dyn Adapter + Send + Sync> = match config.backend.as_str() {
        "claude" => Arc::new(claude::ClaudeAdapter::new(config)),
        _ => Arc::new(cursor::CursorAdapter::new(config)),
    };
    queue::set_adapter(adapter);
}

/// Injects an adapter for testing. Overwrites any previous setup.
#[cfg(test)]
fn setup_with_adapter(adapter: Arc<dyn Adapter + Send + Sync>) {
    queue::set_adapter(adapter);
}

/// Enqueues a CLI call. Callback runs on main thread via `dispatch::schedule`.
pub(crate) fn send(opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete) {
    send_tagged(opts, on_stream, callback, None);
}

/// Enqueues a CLI call at the front of the queue so it runs next.
pub(crate) fn send_priority(opts: BackendOpts, callback: OnComplete) {
    queue::push_front(queue::QueueItem {
        opts,
        on_stream: None,
        callback,
        tag: None,
    });
}

/// Enqueues a CLI call with an optional tag for scoped cancellation.
pub(crate) fn send_tagged(
    opts: BackendOpts,
    on_stream: Option<OnStream>,
    callback: OnComplete,
    tag: Option<String>,
) {
    queue::push(queue::QueueItem {
        opts,
        on_stream,
        callback,
        tag,
    });
}

/// Returns true if the queue has pending items or one is in flight.
#[cfg(test)]
fn is_busy() -> bool {
    queue::is_busy()
}

/// Cancels all pending items, kills in-flight child processes, and
/// causes in-flight callbacks to no-op.
pub(crate) fn cancel_all() {
    queue::cancel_all();
    kill_tracked_children();
}

/// Cancels only queued/in-flight items tagged with `tag`.
///
/// Other requests are left untouched. Use this instead of `cancel_all`
/// when replying in a thread to avoid interrupting unrelated sessions.
/// If the in-flight request matches, its child process is killed so the
/// adapter thread unblocks and the queue advances immediately.
pub(crate) fn cancel_tagged(tag: &str) {
    let was_inflight = queue::inflight_tag().as_deref() == Some(tag);
    queue::cancel_tagged(tag);
    if was_inflight {
        kill_tracked_children();
    }
}

/// Returns the tag (thread ID) of the currently in-flight request, if any.
pub(crate) fn inflight_tag() -> Option<String> {
    queue::inflight_tag()
}

/// Appends a streaming chunk to the in-flight accumulator.
///
/// Called from `on_stream` callbacks so that `open_active_thread` can
/// display text that arrived before the thread window was opened.
pub(crate) fn append_inflight_stream(chunk: &str) {
    queue::append_inflight_stream(chunk);
}

/// Returns the accumulated streaming text for the in-flight request.
pub(crate) fn inflight_stream() -> String {
    queue::inflight_stream()
}

/// Returns the number of requests waiting in the queue (excludes in-flight).
pub(crate) fn pending_count() -> usize {
    queue::pending_count()
}

/// Returns the 0-based queue position of the request tagged with `tag`,
/// or `None` if it is not waiting (already in-flight or absent).
pub(crate) fn queue_position(tag: &str) -> Option<usize> {
    queue::queue_position(tag)
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static CHILDREN: Mutex<Vec<Arc<Mutex<Child>>>> = Mutex::new(Vec::new());

/// Shared handle to a spawned child process.
///
/// Ownership is shared between the adapter thread (reads stdout, waits
/// for exit) and the shutdown path (kills the process on `VimLeavePre`).
pub(crate) type SharedChild = Arc<Mutex<Child>>;

/// Returns true if shutdown has been requested.
pub(crate) fn is_shutdown() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

#[cfg(test)]
fn reset_shutdown() {
    SHUTDOWN.store(false, Ordering::SeqCst);
    CHILDREN.lock().expect("children lock").clear();
}

/// Registers a child process handle for cleanup on shutdown.
pub(crate) fn track_child(handle: &SharedChild) {
    CHILDREN
        .lock()
        .expect("children lock")
        .push(Arc::clone(handle));
}

/// Kills all tracked child processes without removing them from the list.
///
/// The adapter thread will call `untrack_child` when it resumes after
/// the stdout pipe closes. Used by `cancel_tagged` to unblock the
/// in-flight adapter so the queue can advance to the next request.
fn kill_tracked_children() {
    let children = CHILDREN.lock().expect("children lock");
    for h in children.iter() {
        let child = &mut *h.lock().expect("child lock");
        let _ = child.kill();
    }
}

/// Removes a child handle after it exits normally.
pub(crate) fn untrack_child(handle: &SharedChild) {
    CHILDREN
        .lock()
        .expect("children lock")
        .retain(|h| !Arc::ptr_eq(h, handle));
}

/// Cancels all pending work and kills all spawned agent processes.
///
/// Called from `VimLeavePre` to ensure no orphaned CLI processes survive
/// after Neovim exits. Killing the child closes its stdout pipe, which
/// unblocks any `BufReader::lines()` loop in the adapter thread.
pub(crate) fn shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
    cancel_all();
    let handles: Vec<SharedChild> = CHILDREN.lock().expect("children lock").drain(..).collect();
    for h in handles {
        let child = &mut *h.lock().expect("child lock");
        let _ = child.kill();
        let _ = child.wait();
    }
}

static MISSING_BINARY_NOTIFIED: AtomicBool = AtomicBool::new(false);

/// Notifies once at ERROR when the backend error indicates a missing CLI binary.
///
/// Call from BackendResult callbacks when `res.error` is present.
/// Prevents repeated ERROR notifications.
pub(crate) fn notify_if_missing_binary(error: &str) {
    let is_missing = error.contains("not found")
        || error.contains("No such file")
        || error.contains("binary not found")
        || error.contains("not found on PATH");
    if is_missing && !MISSING_BINARY_NOTIFIED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let _ = nvim_oxi::api::notify(
            &format!("Agent CLI not found: {error}. Install the CLI or fix PATH."),
            nvim_oxi::api::types::LogLevel::Error,
            &nvim_oxi::Dictionary::default(),
        );
    }
}

/// Send a comment (new session). Optionally streams when `on_stream` is provided.
///
/// `tag` associates this request with a thread for scoped cancellation.
pub(crate) fn send_comment(
    prompt: &str,
    on_stream: Option<OnStream>,
    callback: OnComplete,
    tag: Option<String>,
) {
    send_tagged(
        BackendOpts {
            op: BackendOp::NewSession,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: on_stream.is_some(),
            json_schema: None,
        },
        on_stream,
        callback,
        tag,
    );
}

/// Reply to a thread (resume session). Optionally streams.
///
/// `tag` associates this request with a thread for scoped cancellation.
pub(crate) fn thread_reply(
    session_id: Option<&str>,
    prompt: &str,
    on_stream: Option<OnStream>,
    callback: OnComplete,
    tag: Option<String>,
) {
    let op = session_id
        .map(|s| BackendOp::Resume(s.to_string()))
        .unwrap_or(BackendOp::NewSession);
    send_tagged(
        BackendOpts {
            op,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: on_stream.is_some(),
            json_schema: None,
        },
        on_stream,
        callback,
        tag,
    );
}

/// Self-review (new session, no stream, optional json_schema for Claude).
pub(crate) fn self_review(prompt: &str, json_schema: Option<String>, callback: OnComplete) {
    send(
        BackendOpts {
            op: BackendOp::NewSession,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: false,
            json_schema,
        },
        None,
        callback,
    );
}

/// Send a free-form prompt (new session). Optionally streams.
pub(crate) fn send_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete) {
    send(
        BackendOpts {
            op: BackendOp::NewSession,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: on_stream.is_some(),
            json_schema: None,
        },
        on_stream,
        callback,
    );
}

/// Continue the latest session. Optionally streams.
pub(crate) fn continue_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete) {
    send(
        BackendOpts {
            op: BackendOp::ContinueLatest,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: on_stream.is_some(),
            json_schema: None,
        },
        on_stream,
        callback,
    );
}

/// Parsed thread from Cursor self-review text response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedThread {
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) message: String,
}

/// Parses THREAD|file|line|message lines from Cursor self-review text.
///
/// Lenient: strips markdown code fences, bullet prefixes (`- `, `* `),
/// backticks; accepts `THREAD:`, `THREAD ` or `THREAD|` prefix.
/// Invalid lines are silently discarded.
pub(crate) fn parse_self_review_text(text: &str) -> Vec<ParsedThread> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let trimmed = trimmed
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim_start_matches('`')
                .trim_end_matches('`')
                .trim();
            let rest = trimmed
                .strip_prefix("THREAD")
                .map(|r| r.trim_start_matches(':').trim_start_matches(' '))
                .and_then(|r| r.strip_prefix('|'))?;
            let parts: Vec<&str> = rest.splitn(3, '|').collect();
            if parts.len() == 3 {
                let line_num = parts[1].trim().parse::<u32>().ok()?;
                Some(ParsedThread {
                    file: parts[0].trim().to_string(),
                    line: line_num,
                    message: parts[2].trim().to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BackendResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static QUEUE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn acquire_queue_lock() -> std::sync::MutexGuard<'static, ()> {
        QUEUE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct MockAdapter {
        responses: std::sync::Mutex<std::collections::VecDeque<BackendResult>>,
        call_count: AtomicUsize,
    }

    impl MockAdapter {
        fn new(responses: Vec<BackendResult>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl Adapter for MockAdapter {
        fn execute(&self, _opts: BackendOpts, _: Option<OnStream>, callback: OnComplete) {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let result = self
                .responses
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or_else(|| BackendResult {
                    text: String::new(),
                    session_id: String::new(),
                    error: None,
                });
            (callback)(result);
        }
    }

    #[test]
    fn queue_fifo_order() {
        let _lock = acquire_queue_lock();
        setup_with_adapter(Arc::new(MockAdapter::new(vec![
            BackendResult {
                text: "a".to_string(),
                session_id: String::new(),
                error: None,
            },
            BackendResult {
                text: "b".to_string(),
                session_id: String::new(),
                error: None,
            },
            BackendResult {
                text: "c".to_string(),
                session_id: String::new(),
                error: None,
            },
        ])));

        let results = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        send(
            BackendOpts {
                op: BackendOp::NewSession,
                prompt: "1".to_string(),
                ask_mode: false,
                stream: false,
                json_schema: None,
            },
            None,
            Box::new({
                let r = Arc::clone(&results);
                move |res| r.lock().expect("lock").push(res.text.clone())
            }),
        );
        send(
            BackendOpts {
                op: BackendOp::NewSession,
                prompt: "2".to_string(),
                ask_mode: false,
                stream: false,
                json_schema: None,
            },
            None,
            Box::new({
                let r = Arc::clone(&results);
                move |res| r.lock().expect("lock").push(res.text.clone())
            }),
        );
        send(
            BackendOpts {
                op: BackendOp::NewSession,
                prompt: "3".to_string(),
                ask_mode: false,
                stream: false,
                json_schema: None,
            },
            None,
            Box::new({
                let r = Arc::clone(&results);
                move |res| r.lock().expect("lock").push(res.text.clone())
            }),
        );

        while is_busy() {
            std::thread::yield_now();
        }

        assert_eq!(
            results.lock().expect("lock").as_slice(),
            &["a", "b", "c"],
            "FIFO order"
        );
    }

    #[test]
    fn is_busy_returns_true_while_processing() {
        let _lock = acquire_queue_lock();
        let adapter = Arc::new(MockAdapter::new(vec![BackendResult {
            text: String::new(),
            session_id: String::new(),
            error: None,
        }]));
        setup_with_adapter(adapter.clone());

        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = Arc::clone(&done);
        send(
            BackendOpts {
                op: BackendOp::NewSession,
                prompt: "x".to_string(),
                ask_mode: false,
                stream: false,
                json_schema: None,
            },
            None,
            Box::new(move |_| d.store(true, Ordering::SeqCst)),
        );

        while !done.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        while is_busy() {
            std::thread::yield_now();
        }
        assert!(!is_busy());
    }

    struct RecordingAdapter(Arc<std::sync::Mutex<Option<BackendOpts>>>);

    impl Adapter for RecordingAdapter {
        fn execute(&self, opts: BackendOpts, _: Option<OnStream>, callback: OnComplete) {
            // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
            *self.0.lock().expect("lock") = Some(opts);
            (callback)(BackendResult {
                text: String::new(),
                session_id: String::new(),
                error: None,
            });
        }
    }

    #[test]
    fn thread_reply_uses_resume() {
        let _lock = acquire_queue_lock();
        let opts_cell = Arc::new(std::sync::Mutex::new(None));
        let opts_cell_clone = Arc::clone(&opts_cell);

        setup_with_adapter(Arc::new(RecordingAdapter(opts_cell_clone)));
        thread_reply(Some("sess-1"), "reply", None, Box::new(|_| {}), None);

        while is_busy() {
            std::thread::yield_now();
        }

        // SAFETY: Mutex poisoning indicates a prior panic, not a recoverable condition.
        let opts = opts_cell.lock().expect("lock").take().expect("opts");
        assert!(matches!(opts.op, BackendOp::Resume(ref s) if s == "sess-1"));
        assert!(!opts.stream);
    }

    #[test]
    fn self_review_uses_new_session_no_stream() {
        let _lock = acquire_queue_lock();
        let opts_cell = Arc::new(std::sync::Mutex::new(None));
        let opts_cell_clone = Arc::clone(&opts_cell);

        setup_with_adapter(Arc::new(RecordingAdapter(opts_cell_clone)));
        self_review("prompt", None, Box::new(|_| {}));

        while is_busy() {
            std::thread::yield_now();
        }

        let opts = opts_cell.lock().expect("lock").take().expect("opts");
        assert!(matches!(opts.op, BackendOp::NewSession));
        assert!(!opts.stream);
    }

    #[test]
    fn parse_self_review_valid_input() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text(
            "THREAD|src/main.rs|22|Should this return 401 or 403?\n\
             THREAD|src/auth.rs|15|Consider caching the token lookup.\n\
             THREAD|lib/foo.rs|1|Missing error handling.",
        );
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].file, "src/main.rs");
        assert_eq!(t[0].line, 22);
        assert_eq!(t[0].message, "Should this return 401 or 403?");
        assert_eq!(t[1].file, "src/auth.rs");
        assert_eq!(t[1].line, 15);
        assert_eq!(t[2].file, "lib/foo.rs");
    }

    #[test]
    fn parse_self_review_mixed_valid_invalid() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text(
            "THREAD|a.rs|1|ok\n\
             garbage line\n\
             THREAD|b.rs|2|also ok",
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].file, "a.rs");
        assert_eq!(t[1].file, "b.rs");
    }

    #[test]
    fn parse_self_review_empty() {
        let _lock = acquire_queue_lock();
        assert!(parse_self_review_text("").is_empty());
    }

    #[test]
    fn parse_self_review_missing_fields_discarded() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text("THREAD|a|1\n");
        assert!(t.is_empty());
    }

    #[test]
    fn parse_self_review_lenient_strips_markdown() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text(
            "- THREAD|a.rs|1|msg\n\
             * THREAD|b.rs|2|msg\n\
             `THREAD|c.rs|3|msg`",
        );
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].file, "a.rs");
        assert_eq!(t[1].file, "b.rs");
        assert_eq!(t[2].file, "c.rs");
    }

    #[test]
    fn parse_self_review_thread_prefix_variants() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text(
            "THREAD: |x.rs|1|msg\n\
             THREAD |y.rs|2|msg",
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].file, "x.rs");
        assert_eq!(t[1].file, "y.rs");
    }

    #[test]
    fn cancel_all_clears_pending() {
        let _lock = acquire_queue_lock();
        setup_with_adapter(Arc::new(MockAdapter::new(vec![])));

        for i in 0..3 {
            send(
                BackendOpts {
                    op: BackendOp::NewSession,
                    prompt: i.to_string(),
                    ask_mode: false,
                    stream: false,
                    json_schema: None,
                },
                None,
                Box::new(|_| {}),
            );
        }

        cancel_all();

        while is_busy() {
            std::thread::yield_now();
        }
        assert!(!is_busy());
    }

    #[test]
    fn parse_self_review_text_preserves_pipe_in_message() {
        let _lock = acquire_queue_lock();
        let t = parse_self_review_text("THREAD|file.rs|1|message with | pipe");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].file, "file.rs");
        assert_eq!(t[0].line, 1);
        assert_eq!(t[0].message, "message with | pipe");
    }

    static CHILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn spawn_sleeper() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep should be available")
    }

    fn kill_handle(handle: &SharedChild) {
        let child = &mut *handle.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }

    fn children_count() -> usize {
        CHILDREN.lock().expect("lock").len()
    }

    #[test]
    fn track_and_untrack_child() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        let handle: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        track_child(&handle);
        assert_eq!(children_count(), 1);

        untrack_child(&handle);
        assert_eq!(children_count(), 0);

        kill_handle(&handle);
    }

    #[test]
    fn untrack_only_removes_matching_handle() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        let h1: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        let h2: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        track_child(&h1);
        track_child(&h2);
        assert_eq!(children_count(), 2);

        untrack_child(&h1);
        assert_eq!(children_count(), 1);

        untrack_child(&h2);
        assert_eq!(children_count(), 0);

        kill_handle(&h1);
        kill_handle(&h2);
    }

    #[test]
    fn untrack_nonexistent_handle_is_noop() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        let tracked: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        let untracked: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        track_child(&tracked);
        assert_eq!(children_count(), 1);

        untrack_child(&untracked);
        assert_eq!(children_count(), 1);

        kill_handle(&tracked);
        kill_handle(&untracked);
    }

    #[test]
    fn shutdown_kills_tracked_children() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        let h1: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        let h2: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        track_child(&h1);
        track_child(&h2);

        shutdown();

        assert!(is_shutdown());
        assert_eq!(children_count(), 0);

        for h in [&h1, &h2] {
            let exited = h.lock().expect("lock").try_wait().ok().flatten().is_some();
            assert!(exited, "child should have exited after shutdown");
        }
    }

    #[test]
    fn shutdown_with_no_children_is_noop() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        assert_eq!(children_count(), 0);
        shutdown();
        assert!(is_shutdown());
        assert_eq!(children_count(), 0);
    }

    #[test]
    fn shutdown_is_idempotent() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        let h: SharedChild = Arc::new(Mutex::new(spawn_sleeper()));
        track_child(&h);

        shutdown();
        assert!(is_shutdown());
        assert_eq!(children_count(), 0);

        shutdown();
        assert!(is_shutdown());
        assert_eq!(children_count(), 0);
    }

    #[test]
    fn is_shutdown_lifecycle() {
        let _queue = acquire_queue_lock();
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        assert!(!is_shutdown());
        shutdown();
        assert!(is_shutdown());
    }

    struct BlockFirstAdapter {
        first_entered: AtomicBool,
        barrier: Arc<std::sync::Barrier>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Adapter for BlockFirstAdapter {
        fn execute(&self, opts: BackendOpts, _: Option<OnStream>, callback: OnComplete) {
            self.calls.lock().expect("lock").push(opts.prompt.clone());
            if !self.first_entered.swap(true, Ordering::SeqCst) {
                let barrier = Arc::clone(&self.barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (callback)(BackendResult {
                        text: String::new(),
                        session_id: String::new(),
                        error: None,
                    });
                });
            } else {
                (callback)(BackendResult {
                    text: String::new(),
                    session_id: String::new(),
                    error: None,
                });
            }
        }
    }

    fn block_first_adapter() -> (Arc<BlockFirstAdapter>, Arc<std::sync::Barrier>) {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let adapter = Arc::new(BlockFirstAdapter {
            first_entered: AtomicBool::new(false),
            barrier: Arc::clone(&barrier),
            calls: std::sync::Mutex::new(Vec::new()),
        });
        (adapter, barrier)
    }

    fn opts(prompt: &str) -> BackendOpts {
        BackendOpts {
            op: BackendOp::NewSession,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        }
    }

    #[test]
    fn send_priority_inserts_at_front() {
        let _lock = acquire_queue_lock();
        cancel_all();
        let (adapter, barrier) = block_first_adapter();
        setup_with_adapter(adapter.clone());

        send(opts("blocker"), None, Box::new(|_| {}));
        send(opts("regular"), None, Box::new(|_| {}));
        send_priority(opts("priority"), Box::new(|_| {}));

        barrier.wait();
        while is_busy() {
            std::thread::yield_now();
        }

        let calls = adapter.calls.lock().expect("lock");
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0], "blocker");
        assert_eq!(calls[1], "priority");
        assert_eq!(calls[2], "regular");
    }

    #[test]
    fn cancel_tagged_only_removes_matching() {
        let _lock = acquire_queue_lock();
        cancel_all();
        let (adapter, barrier) = block_first_adapter();
        setup_with_adapter(adapter.clone());

        send(opts("blocker"), None, Box::new(|_| {}));
        send_tagged(
            opts("keep-1"),
            None,
            Box::new(|_| {}),
            Some("keep".to_string()),
        );
        send_tagged(
            opts("remove-1"),
            None,
            Box::new(|_| {}),
            Some("remove".to_string()),
        );
        send_tagged(
            opts("keep-2"),
            None,
            Box::new(|_| {}),
            Some("keep".to_string()),
        );

        cancel_tagged("remove");
        barrier.wait();

        while is_busy() {
            std::thread::yield_now();
        }

        let calls = adapter.calls.lock().expect("lock");
        assert_eq!(calls.len(), 3);
        assert!(!calls.contains(&"remove-1".to_string()));
    }

    #[test]
    fn pending_count_accuracy() {
        let _lock = acquire_queue_lock();
        cancel_all();
        let (adapter, barrier) = block_first_adapter();
        setup_with_adapter(adapter.clone());

        send(opts("blocker"), None, Box::new(|_| {}));
        for i in 0..5 {
            send(opts(&i.to_string()), None, Box::new(|_| {}));
        }

        assert_eq!(pending_count(), 5);

        barrier.wait();
        while is_busy() {
            std::thread::yield_now();
        }
    }

    #[test]
    fn queue_position_finds_tag() {
        let _lock = acquire_queue_lock();
        cancel_all();
        let (adapter, barrier) = block_first_adapter();
        setup_with_adapter(adapter.clone());

        send(opts("blocker"), None, Box::new(|_| {}));
        send_tagged(opts("a"), None, Box::new(|_| {}), Some("alpha".to_string()));
        send_tagged(opts("b"), None, Box::new(|_| {}), Some("beta".to_string()));
        send_tagged(opts("c"), None, Box::new(|_| {}), Some("gamma".to_string()));

        assert_eq!(queue_position("alpha"), Some(0));
        assert_eq!(queue_position("beta"), Some(1));
        assert_eq!(queue_position("gamma"), Some(2));
        assert_eq!(queue_position("nonexistent"), None);

        barrier.wait();
        while is_busy() {
            std::thread::yield_now();
        }
    }

    #[test]
    fn thread_reply_no_session_uses_new_session() {
        let _lock = acquire_queue_lock();
        let opts_cell = Arc::new(std::sync::Mutex::new(None));
        let opts_cell_clone = Arc::clone(&opts_cell);

        setup_with_adapter(Arc::new(RecordingAdapter(opts_cell_clone)));
        thread_reply(None, "hello", None, Box::new(|_| {}), None);

        while is_busy() {
            std::thread::yield_now();
        }

        let opts = opts_cell.lock().expect("lock").take().expect("opts");
        assert!(matches!(opts.op, BackendOp::NewSession));
    }

    #[test]
    fn inflight_stream_accumulation() {
        let _lock = acquire_queue_lock();
        cancel_all();

        append_inflight_stream("hello ");
        append_inflight_stream("world");
        assert_eq!(inflight_stream(), "hello world");

        cancel_all();
    }

    #[test]
    fn concurrent_enqueue_no_deadlock() {
        let _lock = acquire_queue_lock();
        cancel_all();

        let completed = Arc::new(AtomicUsize::new(0));
        let responses: Vec<BackendResult> = (0..10)
            .map(|_| BackendResult {
                text: String::new(),
                session_id: String::new(),
                error: None,
            })
            .collect();
        setup_with_adapter(Arc::new(MockAdapter::new(responses)));

        let barrier = Arc::new(std::sync::Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let barrier = Arc::clone(&barrier);
                let completed = Arc::clone(&completed);
                std::thread::spawn(move || {
                    barrier.wait();
                    send(
                        BackendOpts {
                            op: BackendOp::NewSession,
                            prompt: format!("thread-{i}"),
                            ask_mode: false,
                            stream: false,
                            json_schema: None,
                        },
                        None,
                        Box::new(move |_| {
                            completed.fetch_add(1, Ordering::SeqCst);
                        }),
                    );
                })
            })
            .collect();

        let enqueue_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let enqueue_started = std::time::Instant::now();
        for h in handles {
            h.join().expect("thread should not panic");
        }
        assert!(
            enqueue_started.elapsed() < std::time::Duration::from_secs(10),
            "enqueue threads should finish within timeout"
        );

        while is_busy() {
            assert!(
                std::time::Instant::now() < enqueue_deadline,
                "queue should drain within timeout"
            );
            std::thread::yield_now();
        }

        assert_eq!(completed.load(Ordering::SeqCst), 10);
    }

    struct SlowAdapter {
        started: Arc<std::sync::Barrier>,
        gate: Arc<std::sync::Barrier>,
    }

    impl Adapter for SlowAdapter {
        fn execute(&self, _opts: BackendOpts, _: Option<OnStream>, callback: OnComplete) {
            let started = Arc::clone(&self.started);
            let gate = Arc::clone(&self.gate);
            std::thread::spawn(move || {
                started.wait();
                gate.wait();
                (callback)(BackendResult {
                    text: "slow-done".to_string(),
                    session_id: String::new(),
                    error: None,
                });
            });
        }
    }

    #[test]
    fn cancel_all_during_processing_no_panic() {
        let _lock = acquire_queue_lock();
        cancel_all();

        let started = Arc::new(std::sync::Barrier::new(2));
        let gate = Arc::new(std::sync::Barrier::new(2));
        setup_with_adapter(Arc::new(SlowAdapter {
            started: Arc::clone(&started),
            gate: Arc::clone(&gate),
        }));

        let slow_result = Arc::new(std::sync::Mutex::new(None::<String>));
        let sr = Arc::clone(&slow_result);
        send(
            opts("slow"),
            None,
            Box::new(move |res| {
                *sr.lock().expect("lock") = Some(res.text.clone());
            }),
        );

        started.wait();
        cancel_all();
        gate.wait();

        while is_busy() {
            std::thread::yield_now();
        }

        assert!(
            slow_result.lock().expect("lock").is_none(),
            "cancelled callback should not have fired"
        );

        let recovery_done = Arc::new(AtomicBool::new(false));
        let rd = Arc::clone(&recovery_done);
        let started2 = Arc::new(std::sync::Barrier::new(2));
        let gate2 = Arc::new(std::sync::Barrier::new(2));
        setup_with_adapter(Arc::new(SlowAdapter {
            started: Arc::clone(&started2),
            gate: Arc::clone(&gate2),
        }));

        send(
            opts("recovery"),
            None,
            Box::new(move |_| {
                rd.store(true, Ordering::SeqCst);
            }),
        );

        started2.wait();
        gate2.wait();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !recovery_done.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "recovery item should complete"
            );
            std::thread::yield_now();
        }
    }
}
