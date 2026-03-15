//! Thread CRUD, re-anchoring, filtering, and projection.
//!
//! All functions take `&[Thread]` or `&mut [Thread]`; no imports from
//! git, diff, backend, or review. Pure logic, no Neovim API.

mod input;
pub(crate) mod window;

pub use input::{close as input_close, open, open_for_line, OnCancel, OnSubmit};
pub use window::{
    append_message, append_streaming, close as window_close, current_thread_id as window_thread_id,
    is_open as window_is_open, open as window_open, replace_last_agent_message, OnClose,
    OnReplyRequested,
};

use crate::types::{Role, ThreadOrigin, ThreadStatus};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub use crate::types::ThreadSummary;

/// A single message in a thread conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message author.
    pub role: Role,
    /// Message text.
    pub text: String,
    /// Unix timestamp when the message was created.
    pub ts: i64,
}

/// Options for thread creation.
#[derive(Debug, Clone, Default)]
pub struct CreateOpts {
    /// If true, comment is stored locally and not yet sent.
    pub pending: bool,
    /// If true, comment is sent immediately.
    pub immediate: bool,
    /// If true, thread auto-resolves when file change is detected.
    pub auto_resolve: bool,
    /// Origin of the thread (user or agent).
    pub origin: ThreadOrigin,
    /// Exact text of the anchored line (for re-anchoring). Passed by caller.
    pub anchor_content: String,
    /// Surrounding lines for context verification during re-anchoring.
    pub anchor_context: Vec<String>,
}

/// Persisted thread: a comment anchored to a file line with conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    /// Unique identifier.
    pub id: String,
    /// Origin (user or agent).
    pub origin: ThreadOrigin,
    /// File path relative to workspace.
    pub file: String,
    /// 1-based line number in the file.
    pub line: u32,
    /// Exact text of the anchored line (for re-anchoring).
    pub anchor_content: String,
    /// Surrounding lines for context verification during re-anchoring.
    pub anchor_context: Vec<String>,
    /// Resolution status.
    pub status: ThreadStatus,
    /// If true, thread auto-resolves when file changes.
    pub auto_resolve: bool,
    /// Unix timestamp when auto_resolve was set.
    pub auto_resolve_at: Option<i64>,
    /// Context in which the thread was created.
    pub context: crate::types::ThreadContext,
    /// Backend session ID if the thread has been sent.
    pub session_id: Option<String>,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// If true, comment is pending (not yet sent).
    pub pending: bool,
}

/// Options for filtering threads.
#[derive(Debug, Clone, Default)]
pub struct FilterOpts {
    /// Filter by origin; None means no filter.
    pub origin: Option<ThreadOrigin>,
    /// Filter by status; None means no filter.
    pub status: Option<ThreadStatus>,
}

/// Creates a new thread with UUID, initial message, and anchor data.
pub fn create(file: &str, line: u32, text: &str, opts: CreateOpts) -> Thread {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let msg = Message {
        role: Role::User,
        text: text.to_string(),
        ts,
    };
    Thread {
        id: Uuid::new_v4().to_string(),
        origin: opts.origin,
        file: file.to_string(),
        line,
        anchor_content: opts.anchor_content,
        anchor_context: opts.anchor_context,
        status: ThreadStatus::Open,
        auto_resolve: opts.auto_resolve,
        auto_resolve_at: if opts.auto_resolve { Some(ts) } else { None },
        context: crate::types::ThreadContext::Review,
        session_id: None,
        messages: vec![msg],
        pending: opts.pending,
    }
}

/// Appends a message to a thread's conversation.
pub fn add_message(thread: &mut Thread, role: Role, text: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    thread.messages.push(Message {
        role,
        text: text.to_string(),
        ts,
    });
}

/// Sets thread status to Resolved.
pub fn resolve(thread: &mut Thread) {
    thread.status = ThreadStatus::Resolved;
}

/// Sets thread status to Binned (anchor lost).
pub fn bin(thread: &mut Thread) {
    thread.status = ThreadStatus::Binned;
}

/// Resolves all open threads; ignores Resolved and Binned.
pub fn resolve_all(threads: &mut [Thread]) {
    for t in threads.iter_mut() {
        if t.status == ThreadStatus::Open {
            t.status = ThreadStatus::Resolved;
        }
    }
}

/// Removes a thread from the list by index.
///
/// Panics if index is out of bounds.
pub fn dismiss(threads: &mut Vec<Thread>, index: usize) {
    threads.remove(index);
}

/// Content-match re-anchoring.
///
/// For each thread anchored to `file`, searches `new_contents` for
/// `anchor_content`. If found, verifies at least one `anchor_context`
/// line exists within 5 lines. Updates `line` on match.
/// Returns indices of unmatched threads (in original order).
pub fn reanchor_by_content(threads: &mut [Thread], file: &str, new_contents: &str) -> Vec<usize> {
    let lines: Vec<&str> = new_contents.lines().collect();
    let mut unmatched = Vec::new();
    for (i, t) in threads.iter_mut().enumerate() {
        if t.file != file {
            continue;
        }
        if t.anchor_content.is_empty() {
            unmatched.push(i);
            continue;
        }
        let Some(anchor_idx) = lines.iter().position(|l| *l == t.anchor_content) else {
            unmatched.push(i);
            continue;
        };
        let line_num = anchor_idx + 1;
        let has_context = t.anchor_context.is_empty()
            || t.anchor_context.iter().any(|ctx| {
                let lo = anchor_idx.saturating_sub(5);
                let hi = (anchor_idx + 6).min(lines.len());
                lines[lo..hi].contains(&ctx.as_str())
            });
        if has_context {
            t.line = line_num as u32;
        } else {
            unmatched.push(i);
        }
    }
    unmatched
}

/// Returns threads for a file, sorted by line ascending.
pub fn for_file<'a>(threads: &'a [Thread], file: &str) -> Vec<&'a Thread> {
    let mut out: Vec<&Thread> = threads.iter().filter(|t| t.file == file).collect();
    out.sort_by_key(|t| t.line);
    out
}

/// Returns indices of pending (batch, not yet sent) threads.
pub fn pending_indices(threads: &[Thread]) -> Vec<usize> {
    threads
        .iter()
        .enumerate()
        .filter(|(_, t)| t.pending)
        .map(|(i, _)| i)
        .collect()
}

/// Filters threads by optional origin and/or status.
pub fn filter<'a>(threads: &'a [Thread], opts: &FilterOpts) -> Vec<&'a Thread> {
    threads
        .iter()
        .filter(|t| {
            opts.origin.is_none_or(|o| t.origin == o) && opts.status.is_none_or(|s| t.status == s)
        })
        .collect()
}

/// Returns indices into `threads` for all threads, sorted by
/// (file_order index, line). Takes `&[String]` as file order.
pub fn sorted_global(threads: &[Thread], file_order: &[String]) -> Vec<usize> {
    let file_idx: std::collections::HashMap<&str, usize> = file_order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let mut indices: Vec<usize> = (0..threads.len()).collect();
    indices.sort_by_key(|&i| {
        let t = &threads[i];
        let order = file_idx.get(t.file.as_str()).copied().unwrap_or(usize::MAX);
        (order, t.line)
    });
    indices
}

/// Returns the index of the next thread in the sorted list.
/// Wraps from end to start.
pub fn next_thread(sorted: &[usize], current: Option<usize>) -> Option<usize> {
    let len = sorted.len();
    if len == 0 {
        return None;
    }
    let idx = match current {
        Some(c) => sorted
            .iter()
            .position(|&x| x == c)
            .map(|p| p + 1)
            .unwrap_or(0),
        None => 0,
    };
    Some(sorted[idx % len])
}

/// Returns the index of the previous thread in the sorted list.
/// Wraps from start to end.
pub fn prev_thread(sorted: &[usize], current: Option<usize>) -> Option<usize> {
    let len = sorted.len();
    if len == 0 {
        return None;
    }
    let idx = match current {
        Some(c) => sorted.iter().position(|&x| x == c).unwrap_or(0),
        None => 0,
    };
    let prev = if idx == 0 { len - 1 } else { idx - 1 };
    Some(sorted[prev])
}

/// Projects threads into display-only summaries for the diff engine.
///
/// Preview is first 40 chars of the first message.
pub fn to_summaries(threads: &[Thread]) -> Vec<ThreadSummary> {
    threads
        .iter()
        .map(|t| {
            let preview = t
                .messages
                .first()
                .map(|m| m.text.chars().take(40).collect::<String>())
                .unwrap_or_default();
            ThreadSummary {
                id: t.id.clone(),
                origin: t.origin,
                line: t.line,
                preview,
                status: t.status,
            }
        })
        .collect()
}

/// Checks auto-resolve timeouts.
///
/// For each thread with `auto_resolve == true` and `status == Open`,
/// if `now - auto_resolve_at > timeout_secs`, clears `auto_resolve` and
/// `auto_resolve_at`. Returns indices of timed-out threads.
pub fn check_auto_resolve_timeouts(
    threads: &mut [Thread],
    timeout_secs: u64,
    now: i64,
) -> Vec<usize> {
    let mut timed_out = Vec::new();
    for (i, t) in threads.iter_mut().enumerate() {
        if !t.auto_resolve || t.status != ThreadStatus::Open {
            continue;
        }
        let at = t.auto_resolve_at.unwrap_or(0);
        if now.saturating_sub(at) > timeout_secs as i64 {
            t.auto_resolve = false;
            t.auto_resolve_at = None;
            timed_out.push(i);
        }
    }
    timed_out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_produces_thread_with_uuid_and_open_status() {
        let t = create(
            "src/main.rs",
            22,
            "fix this",
            CreateOpts {
                origin: ThreadOrigin::User,
                ..Default::default()
            },
        );
        assert!(!t.id.is_empty());
        assert_eq!(t.origin, ThreadOrigin::User);
        assert_eq!(t.status, ThreadStatus::Open);
        assert_eq!(t.file, "src/main.rs");
        assert_eq!(t.line, 22);
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].text, "fix this");
        assert_eq!(t.messages[0].role, Role::User);
    }

    #[test]
    fn add_message_appends_with_timestamp() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        add_message(&mut t, Role::Agent, "done");
        assert_eq!(t.messages.len(), 2);
        assert_eq!(t.messages[1].role, Role::Agent);
        assert_eq!(t.messages[1].text, "done");
        assert!(t.messages[1].ts > 0);
    }

    #[test]
    fn resolve_sets_status() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        resolve(&mut t);
        assert_eq!(t.status, ThreadStatus::Resolved);
    }

    #[test]
    fn bin_sets_status() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        bin(&mut t);
        assert_eq!(t.status, ThreadStatus::Binned);
    }

    #[test]
    fn dismiss_removes_by_index() {
        let mut threads = vec![
            create("a.rs", 1, "a", CreateOpts::default()),
            create("b.rs", 2, "b", CreateOpts::default()),
        ];
        dismiss(&mut threads, 0);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].file, "b.rs");
    }

    #[test]
    fn resolve_all_resolves_open_only() {
        let t1 = create("a.rs", 1, "a", CreateOpts::default());
        let mut t2 = create("b.rs", 2, "b", CreateOpts::default());
        resolve(&mut t2);
        let mut threads = vec![t1, t2];
        resolve_all(&mut threads);
        assert_eq!(threads[0].status, ThreadStatus::Resolved);
        assert_eq!(threads[1].status, ThreadStatus::Resolved);
    }

    #[test]
    fn reanchor_shifted_anchor() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "  let x = 1;".to_string();
        t.anchor_context = vec!["  }".to_string()];
        let mut threads = vec![t];
        let contents = "fn foo() {\n  let y = 0;\n  let x = 1;\n  }\n";
        let unmatched = reanchor_by_content(&mut threads, "f.rs", contents);
        assert!(unmatched.is_empty());
        assert_eq!(threads[0].line, 3);
    }

    #[test]
    fn reanchor_deleted_anchor() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "deleted line".to_string();
        t.anchor_context = vec!["context".to_string()];
        let mut threads = vec![t];
        let contents = "other line\n";
        let unmatched = reanchor_by_content(&mut threads, "f.rs", contents);
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0], 0);
    }

    #[test]
    fn reanchor_modified_anchor() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "let x = 1;".to_string();
        let mut threads = vec![t];
        let contents = "let x = 2;\n";
        let unmatched = reanchor_by_content(&mut threads, "f.rs", contents);
        assert_eq!(unmatched.len(), 1);
    }

    #[test]
    fn reanchor_context_missing() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "let x = 1;".to_string();
        t.anchor_context = vec!["must be nearby".to_string()];
        let mut threads = vec![t];
        let contents = "let x = 1;\nother line\n"; // anchor found but context >5 lines away
        let unmatched = reanchor_by_content(&mut threads, "f.rs", contents);
        assert_eq!(unmatched.len(), 1);
    }

    #[test]
    fn for_file_filters_and_sorts() {
        let t1 = create("a.rs", 10, "a", CreateOpts::default());
        let t2 = create("a.rs", 5, "b", CreateOpts::default());
        let t3 = create("b.rs", 1, "c", CreateOpts::default());
        let threads = vec![t1, t2, t3];
        let for_a = for_file(&threads, "a.rs");
        assert_eq!(for_a.len(), 2);
        assert_eq!(for_a[0].line, 5);
        assert_eq!(for_a[1].line, 10);
    }

    #[test]
    fn filter_combinations() {
        let t1 = {
            let mut t = create("a.rs", 1, "a", CreateOpts::default());
            t.origin = ThreadOrigin::Agent;
            t
        };
        let mut t2 = create("b.rs", 2, "b", CreateOpts::default());
        resolve(&mut t2);
        let threads = vec![t1, t2];
        let user = filter(
            &threads,
            &FilterOpts {
                origin: Some(ThreadOrigin::User),
                status: None,
            },
        );
        assert_eq!(user.len(), 1);
        let open = filter(
            &threads,
            &FilterOpts {
                origin: None,
                status: Some(ThreadStatus::Open),
            },
        );
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].origin, ThreadOrigin::Agent);
    }

    #[test]
    fn sorted_global_respects_file_order_and_line() {
        let t1 = create("b.rs", 5, "a", CreateOpts::default());
        let t2 = create("a.rs", 10, "b", CreateOpts::default());
        let t3 = create("a.rs", 3, "c", CreateOpts::default());
        let threads = vec![t1, t2, t3];
        let order = sorted_global(&threads, &["a.rs".into(), "b.rs".into()]);
        assert_eq!(order, vec![2, 1, 0]); // a.rs:3, a.rs:10, b.rs:5
    }

    #[test]
    fn next_prev_thread_wrap() {
        let sorted = vec![0, 1, 2];
        assert_eq!(next_thread(&sorted, None), Some(0));
        assert_eq!(next_thread(&sorted, Some(0)), Some(1));
        assert_eq!(next_thread(&sorted, Some(2)), Some(0));
        assert_eq!(prev_thread(&sorted, None), Some(2));
        assert_eq!(prev_thread(&sorted, Some(0)), Some(2));
        assert_eq!(prev_thread(&sorted, Some(2)), Some(1));
    }

    #[test]
    fn to_summaries_truncates_preview() {
        let mut t = create("f.rs", 1, "x", CreateOpts::default());
        t.messages[0].text = "a".repeat(50);
        let threads = vec![t];
        let summaries = to_summaries(&threads);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].preview.len(), 40);
    }

    #[test]
    fn test_auto_resolve_timeout_expired() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.auto_resolve = true;
        t.auto_resolve_at = Some(100);
        let mut threads = vec![t];
        let timed = check_auto_resolve_timeouts(&mut threads, 60, 200);
        assert_eq!(timed.len(), 1);
        assert!(!threads[0].auto_resolve);
        assert!(threads[0].auto_resolve_at.is_none());
    }

    #[test]
    fn test_auto_resolve_within_timeout_untouched() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.auto_resolve = true;
        t.auto_resolve_at = Some(100);
        let mut threads = vec![t];
        let timed = check_auto_resolve_timeouts(&mut threads, 60, 150);
        assert!(timed.is_empty());
        assert!(threads[0].auto_resolve);
    }

    #[test]
    fn test_pending_indices() {
        let mut t1 = create("a.rs", 1, "a", CreateOpts::default());
        t1.pending = true;
        let t2 = create("b.rs", 2, "b", CreateOpts::default());
        let threads = vec![t1, t2];
        assert_eq!(pending_indices(&threads), vec![0]);
    }
}
