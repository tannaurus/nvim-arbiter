//! Thread CRUD, re-anchoring, filtering, and projection.
//!
//! All functions take `&[Thread]` or `&mut [Thread]`; no imports from
//! git, diff, backend, or review. Pure logic, no Neovim API.

use crate::types::{Role, ThreadOrigin, ThreadStatus, ThreadSummary};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// A single message in a thread conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message author.
    pub role: Role,
    /// Message text.
    pub text: String,
    /// Unix timestamp when the message was created.
    pub ts: i64,
    /// Present when this message was authored while viewing a revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_context: Option<RevisionRef>,
}

/// Reference to a specific location within a revision.
///
/// Attached to messages that were authored while viewing a revision diff,
/// linking the comment back to the exact file and line within that revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionRef {
    /// Which revision (by index) the comment refers to.
    pub revision_index: u32,
    /// File within the revision.
    pub file: String,
    /// Line number in the revision's diff.
    pub line: u32,
}

/// A single file's before/after content within a revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionFile {
    /// Relative file path.
    pub path: String,
    /// File content before the agent response. None if the file was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// File content after the agent response. None if the file was deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Reference to another thread with a similar issue.
///
/// Cross-links threads that the agent identified as addressing
/// the same class of problem during self-review. Navigating to
/// a similar ref jumps the diff panel to that thread's file/line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarRef {
    /// Thread ID of the similar thread.
    pub thread_id: String,
    /// File path of the similar thread.
    pub file: String,
    /// Line number of the similar thread.
    pub line: u32,
    /// Preview text (truncated first message).
    pub preview: String,
}

/// A snapshot of changes produced by a single agent response.
///
/// Each non-trivial agent response (one that modifies files) creates a
/// revision on its parent thread. Revisions can be viewed in isolation
/// to understand exactly what one response changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revision {
    /// Sequential index within the thread (1, 2, 3...).
    pub index: u32,
    /// Unix timestamp when captured.
    pub ts: i64,
    /// Index into the thread's `messages` vec for the agent message
    /// that produced this revision.
    pub message_index: usize,
    /// Per-file before/after snapshots (only files that changed).
    pub files: Vec<RevisionFile>,
}

/// Options for thread creation.
#[derive(Debug, Clone, Default)]
pub struct CreateOpts {
    /// If true, comment is stored locally and not yet sent.
    pub pending: bool,
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
    /// Revisions captured per agent response that modifies files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<Revision>,
    /// Cross-references to threads with similar issues, identified
    /// during self-review similarity analysis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub similar_threads: Vec<SimilarRef>,
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
        revision_context: None,
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
        revisions: Vec::new(),
        similar_threads: Vec::new(),
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
        revision_context: None,
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
/// No-op if the index is out of bounds.
pub fn dismiss(threads: &mut Vec<Thread>, index: usize) {
    if index < threads.len() {
        threads.remove(index);
    }
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
    fn dismiss_out_of_bounds_is_noop() {
        let mut threads = vec![create("a.rs", 1, "a", CreateOpts::default())];
        dismiss(&mut threads, 5);
        assert_eq!(threads.len(), 1);
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
    fn reanchor_by_content_duplicate_lines() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "dup line".to_string();
        let mut threads = vec![t];
        let contents = "dup line\nother\ndup line\n";
        let unmatched = reanchor_by_content(&mut threads, "f.rs", contents);
        assert!(unmatched.is_empty());
        assert_eq!(threads[0].line, 1);
    }

    #[test]
    fn reanchor_context_missing() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.anchor_content = "let x = 1;".to_string();
        t.anchor_context = vec!["must be nearby".to_string()];
        let mut threads = vec![t];
        let contents = "let x = 1;\nother line\n";
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
        assert_eq!(order, vec![2, 1, 0]);
    }

    #[test]
    fn sorted_global_missing_file() {
        let t1 = create("a.rs", 1, "a", CreateOpts::default());
        let t2 = create("unknown.rs", 5, "b", CreateOpts::default());
        let threads = vec![t1, t2];
        let order = sorted_global(&threads, &["a.rs".to_string()]);
        assert_eq!(order, vec![0, 1]);
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
    fn next_thread_current_not_in_sorted() {
        let sorted = vec![0, 1, 2];
        assert_eq!(next_thread(&sorted, Some(999)), Some(0));
    }

    #[test]
    fn prev_thread_current_not_in_sorted() {
        let sorted = vec![0, 1, 2];
        assert_eq!(prev_thread(&sorted, Some(999)), Some(2));
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
    fn to_summaries_empty_messages() {
        let mut t = create("f.rs", 1, "x", CreateOpts::default());
        t.messages.clear();
        let summaries = to_summaries(&[t]);
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].preview.is_empty());
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
    fn check_auto_resolve_no_timestamp() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.auto_resolve = true;
        t.auto_resolve_at = None;
        let mut threads = vec![t];
        let timed = check_auto_resolve_timeouts(&mut threads, 60, 100);
        assert_eq!(timed.len(), 1);
        assert!(!threads[0].auto_resolve);
    }

    #[test]
    fn create_has_empty_similar_threads() {
        let t = create("f.rs", 1, "hi", CreateOpts::default());
        assert!(t.similar_threads.is_empty());
    }

    #[test]
    fn similar_threads_serialized_and_deserialized() {
        let mut t = create("f.rs", 1, "hi", CreateOpts::default());
        t.similar_threads.push(SimilarRef {
            thread_id: "abc-123".to_string(),
            file: "other.rs".to_string(),
            line: 10,
            preview: "same issue".to_string(),
        });
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("similar_threads"));
        let deser: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.similar_threads.len(), 1);
        assert_eq!(deser.similar_threads[0].thread_id, "abc-123");
        assert_eq!(deser.similar_threads[0].file, "other.rs");
        assert_eq!(deser.similar_threads[0].line, 10);
        assert_eq!(deser.similar_threads[0].preview, "same issue");
    }

    #[test]
    fn similar_threads_skipped_when_empty() {
        let t = create("f.rs", 1, "hi", CreateOpts::default());
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("similar_threads"));
    }
}
