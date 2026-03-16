//! Backend adapter, queue, and CLI integration.
//!
//! Abstracts over Cursor and Claude CLI. FIFO queue for all calls.
//! Setup stores config and selects adapter; send enqueues items.

mod adapter;
mod claude;
mod cursor;
mod queue;

use crate::types::{BackendOp, BackendOpts, OnComplete, OnStream};
pub use adapter::Adapter;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Minimal config for backend setup. Workspace defaults to cwd at setup time.
#[derive(Debug, Clone)]
pub struct BackendConfig {
    /// `"cursor"` or `"claude"`.
    pub backend: String,
    /// Optional model override.
    pub model: Option<String>,
    /// Workspace root. Passed to CLI as --workspace or --add-dir.
    pub workspace: Option<String>,
    /// Extra CLI flags appended to every invocation.
    pub extra_args: Vec<String>,
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

/// Initializes the backend. Stores config and selects adapter.
///
/// For E3-1, uses a no-op adapter until E3-2 provides real implementations.
/// Call `setup_with_adapter` from tests to inject a mock.
pub fn setup(config: BackendConfig) {
    let adapter: Arc<dyn Adapter + Send + Sync> = match config.backend.as_str() {
        "cursor" => Arc::new(cursor::CursorAdapter::new(config)),
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
pub fn send(opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete) {
    send_tagged(opts, on_stream, callback, None);
}

/// Enqueues a CLI call with an optional tag for scoped cancellation.
pub fn send_tagged(
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

/// Cancels all pending items and causes in-flight callbacks to no-op.
pub fn cancel_all() {
    queue::cancel_all()
}

/// Cancels only queued/in-flight items tagged with `tag`.
///
/// Other requests are left untouched. Use this instead of `cancel_all`
/// when replying in a thread to avoid interrupting unrelated sessions.
/// If the in-flight request matches, its child process is killed so the
/// adapter thread unblocks and the queue advances immediately.
pub fn cancel_tagged(tag: &str) {
    let was_inflight = queue::inflight_tag().as_deref() == Some(tag);
    queue::cancel_tagged(tag);
    if was_inflight {
        kill_tracked_children();
    }
}

/// Returns the tag (thread ID) of the currently in-flight request, if any.
pub fn inflight_tag() -> Option<String> {
    queue::inflight_tag()
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
pub fn shutdown() {
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
pub fn notify_if_missing_binary(error: &str) {
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
pub fn send_comment(
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
pub fn thread_reply(
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

/// Catch up (continue review session with summarization prompt).
pub fn catch_up(session_id: Option<&str>, prompt: &str, callback: OnComplete) {
    let op = session_id
        .map(|s| BackendOp::Resume(s.to_string()))
        .unwrap_or(BackendOp::ContinueLatest);
    send(
        BackendOpts {
            op,
            prompt: prompt.to_string(),
            ask_mode: false,
            stream: false,
            json_schema: None,
        },
        None,
        callback,
    );
}

/// Self-review (new session, no stream, optional json_schema for Claude).
pub fn self_review(prompt: &str, json_schema: Option<String>, callback: OnComplete) {
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

/// Re-anchor (resume session in ask mode).
pub fn re_anchor(session_id: &str, prompt: &str, callback: OnComplete) {
    send(
        BackendOpts {
            op: BackendOp::Resume(session_id.to_string()),
            prompt: prompt.to_string(),
            ask_mode: true,
            stream: false,
            json_schema: None,
        },
        None,
        callback,
    );
}

/// Send a free-form prompt (new session). Optionally streams.
pub fn send_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete) {
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
pub fn continue_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete) {
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
pub struct ParsedThread {
    pub file: String,
    pub line: u32,
    pub message: String,
}

/// Parses THREAD|file|line|message lines from Cursor self-review text.
///
/// Lenient: strips markdown code fences, bullet prefixes (`- `, `* `),
/// backticks; accepts `THREAD:`, `THREAD ` or `THREAD|` prefix.
/// Invalid lines are silently discarded.
pub fn parse_self_review_text(text: &str) -> Vec<ParsedThread> {
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

    /// Serializes tests that touch global queue state (ADAPTER, QUEUE,
    /// PROCESSING, GENERATION). Without this, concurrent tests can increment
    /// GENERATION via `cancel_all`, causing another test's guarded callbacks
    /// to no-op on the generation check.
    static QUEUE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _lock = QUEUE_LOCK.lock().expect("test lock");
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
        let _lock = QUEUE_LOCK.lock().expect("test lock");
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
        let _lock = QUEUE_LOCK.lock().expect("test lock");
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
        let _lock = QUEUE_LOCK.lock().expect("test lock");
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
        assert!(parse_self_review_text("").is_empty());
    }

    #[test]
    fn parse_self_review_missing_fields_discarded() {
        let t = parse_self_review_text("THREAD|a|1\n"); // only 2 parts
        assert!(t.is_empty());
    }

    #[test]
    fn parse_self_review_lenient_strips_markdown() {
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
        let _lock = QUEUE_LOCK.lock().expect("test lock");
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
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        assert_eq!(children_count(), 0);
        shutdown();
        assert!(is_shutdown());
        assert_eq!(children_count(), 0);
    }

    #[test]
    fn shutdown_is_idempotent() {
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
        let _lock = CHILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_shutdown();

        assert!(!is_shutdown());
        shutdown();
        assert!(is_shutdown());
    }
}
