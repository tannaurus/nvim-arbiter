//! Shared types used across the arbiter plugin.
//!
//! Enums, structs, and type aliases that define the plugin's data model.
//! Timestamps use `std::time::SystemTime` with `duration_since(UNIX_EPOCH)`
//! for `i64` epoch seconds. No external time crate needed.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Origin of a thread (user- or agent-initiated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThreadOrigin {
    /// Thread started by the user via comment.
    #[default]
    User,
    /// Thread started by the agent via self-review.
    Agent,
}

impl std::fmt::Display for ThreadOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadOrigin::User => write!(f, "you"),
            ThreadOrigin::Agent => write!(f, "agent"),
        }
    }
}

/// Resolution status of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadStatus {
    /// Thread is open; awaiting resolution.
    Open,
    /// Thread has been resolved.
    Resolved,
    /// Thread is stale; its anchor line was lost.
    #[serde(alias = "Binned")]
    Stale,
}

impl std::fmt::Display for ThreadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadStatus::Open => write!(f, "open"),
            ThreadStatus::Resolved => write!(f, "resolved"),
            ThreadStatus::Stale => write!(f, "stale"),
        }
    }
}

/// Context in which the thread was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadContext {
    /// Created during normal review workflow.
    Review,
    /// Created during agent self-review.
    SelfReview,
}

/// Role of a message author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Human user.
    User,
    /// AI agent.
    Agent,
}

/// Git status of a file in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// File was modified.
    Modified,
    /// File was added (new).
    Added,
    /// File was deleted.
    Deleted,
    /// File is untracked by git.
    Untracked,
}

impl std::fmt::Display for FileStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileStatus::Modified => write!(f, "modified"),
            FileStatus::Added => write!(f, "added"),
            FileStatus::Deleted => write!(f, "deleted"),
            FileStatus::Untracked => write!(f, "untracked"),
        }
    }
}

/// Review approval status of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus {
    /// Not yet reviewed.
    Unreviewed,
    /// Approved by the reviewer.
    Approved,
    /// Marked as needing changes.
    NeedsChanges,
}

impl std::fmt::Display for ReviewStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewStatus::Unreviewed => write!(f, "unreviewed"),
            ReviewStatus::Approved => write!(f, "approved"),
            ReviewStatus::NeedsChanges => write!(f, "needs-changes"),
        }
    }
}

/// Display-only projection of a thread for the diff engine.
///
/// Produced by `threads::to_summaries()`, consumed by `diff::render()`.
/// The diff module does not import `threads`; it receives this struct instead.
#[derive(Debug, Clone)]
pub struct ThreadSummary {
    /// Unique thread identifier.
    pub id: String,
    /// Who started the thread.
    pub origin: ThreadOrigin,
    /// 1-based line number in the source file.
    pub line: u32,
    /// Truncated first message for display.
    pub preview: String,
    /// Current resolution status.
    pub status: ThreadStatus,
}

/// Which session lifecycle to use for a backend CLI call.
#[derive(Debug, Clone)]
pub enum BackendOp {
    /// Start a new session.
    NewSession,
    /// Resume a specific session by ID.
    Resume(String),
    /// Continue the most recent session.
    ContinueLatest,
}

/// Options for a backend CLI call.
#[derive(Debug, Clone)]
pub struct BackendOpts {
    /// Session lifecycle (new, resume, continue).
    pub op: BackendOp,
    /// Prompt text to send.
    pub prompt: String,
    /// If true, run in read-only/plan mode.
    pub ask_mode: bool,
    /// If true, stream partial output via `OnStream`.
    pub stream: bool,
    /// Optional JSON schema for structured output (Claude only).
    pub json_schema: Option<String>,
}

/// Result returned from a backend CLI call.
#[derive(Debug, Clone)]
pub struct BackendResult {
    /// Response text from the agent.
    pub text: String,
    /// Session ID for resuming the conversation.
    pub session_id: String,
    /// Error message if the call failed.
    pub error: Option<String>,
}

/// Arc-wrapped callback for streaming output.
///
/// Arc-wrapped so the adapter can clone into each `dispatch::schedule` call
/// when streaming multiple chunks.
pub type OnStream = Arc<dyn Fn(&str) + Send + Sync>;

/// One-shot callback for completion of a backend call.
pub type OnComplete = Box<dyn FnOnce(BackendResult) + Send>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_origin_display() {
        assert_eq!(ThreadOrigin::User.to_string(), "you");
        assert_eq!(ThreadOrigin::Agent.to_string(), "agent");
    }

    #[test]
    fn thread_origin_serde_roundtrip() {
        for v in [ThreadOrigin::User, ThreadOrigin::Agent] {
            let j = serde_json::to_string(&v).unwrap();
            let r: ThreadOrigin = serde_json::from_str(&j).unwrap();
            assert_eq!(v, r);
        }
    }

    #[test]
    fn thread_status_display() {
        assert_eq!(ThreadStatus::Open.to_string(), "open");
        assert_eq!(ThreadStatus::Resolved.to_string(), "resolved");
        assert_eq!(ThreadStatus::Stale.to_string(), "stale");
    }

    #[test]
    fn thread_status_serde_roundtrip() {
        for v in [
            ThreadStatus::Open,
            ThreadStatus::Resolved,
            ThreadStatus::Stale,
        ] {
            let j = serde_json::to_string(&v).unwrap();
            let r: ThreadStatus = serde_json::from_str(&j).unwrap();
            assert_eq!(v, r);
        }
    }

    #[test]
    fn thread_context_serde_roundtrip() {
        for v in [ThreadContext::Review, ThreadContext::SelfReview] {
            let j = serde_json::to_string(&v).unwrap();
            let r: ThreadContext = serde_json::from_str(&j).unwrap();
            assert_eq!(v, r);
        }
    }

    #[test]
    fn role_serde_roundtrip() {
        for v in [Role::User, Role::Agent] {
            let j = serde_json::to_string(&v).unwrap();
            let r: Role = serde_json::from_str(&j).unwrap();
            assert_eq!(v, r);
        }
    }

    #[test]
    fn file_status_display() {
        assert_eq!(FileStatus::Modified.to_string(), "modified");
        assert_eq!(FileStatus::Added.to_string(), "added");
        assert_eq!(FileStatus::Deleted.to_string(), "deleted");
        assert_eq!(FileStatus::Untracked.to_string(), "untracked");
    }

    #[test]
    fn review_status_display() {
        assert_eq!(ReviewStatus::Unreviewed.to_string(), "unreviewed");
        assert_eq!(ReviewStatus::Approved.to_string(), "approved");
        assert_eq!(ReviewStatus::NeedsChanges.to_string(), "needs-changes");
    }

    #[test]
    fn review_status_serde_roundtrip() {
        for v in [
            ReviewStatus::Unreviewed,
            ReviewStatus::Approved,
            ReviewStatus::NeedsChanges,
        ] {
            let j = serde_json::to_string(&v).unwrap();
            let r: ReviewStatus = serde_json::from_str(&j).unwrap();
            assert_eq!(v, r);
        }
    }

    #[test]
    fn thread_summary_constructable() {
        let _ = ThreadSummary {
            id: "t-1".to_string(),
            origin: ThreadOrigin::User,
            line: 22,
            preview: "fix this".to_string(),
            status: ThreadStatus::Open,
        };
    }
}
