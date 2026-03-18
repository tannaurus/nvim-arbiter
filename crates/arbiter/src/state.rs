//! JSON persistence for review state, threads, and sessions.
//!
//! State is stored in `~/.local/share/nvim/arbiter/` (or a custom
//! path from config). Handles missing and corrupt JSON files gracefully
//! by returning defaults.

use crate::threads::Thread;
use crate::types::ReviewStatus;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Persisted review state (file statuses per path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewState {
    /// File path to file state.
    pub files: HashMap<String, FileState>,
    /// Generalizable coding conventions extracted from resolved threads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_rules: Vec<String>,
}

/// State for a single file in the review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    /// Review approval status.
    pub status: ReviewStatus,
    /// Content hash for change detection.
    pub content_hash: String,
    /// Unix timestamp of last update.
    pub updated_at: i64,
    /// Content hashes of individually accepted hunks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_hunks: Vec<String>,
}

/// A persisted session record for `:ArbiterList` / `:ArbiterResume`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Backend session ID.
    pub session_id: String,
    /// Unix timestamp when created.
    pub created_at: i64,
    /// Preview of last prompt (first 80 chars).
    pub last_prompt_preview: String,
    /// Thread ID if this session is per-thread; otherwise absent.
    pub thread_id: Option<String>,
}

/// Container for session records in sessions.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsFile {
    /// Session records, newest last.
    pub sessions: Vec<SessionRecord>,
}

/// Container for threads in threads.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadsFile {
    pub threads: Vec<Thread>,
}

fn sanitize_ref(ref_name: &str) -> String {
    ref_name
        .chars()
        .map(|c| if c == '/' { '_' } else { c })
        .collect()
}

fn ensure_dir(path: &Path) {
    if let Err(e) = fs::create_dir_all(path) {
        eprintln!(
            "[arbiter] failed to create state dir {}: {e}",
            path.display()
        );
    }
}

/// Loads review state from disk. Returns default if file does not exist or is corrupt.
pub fn load_review(state_dir: &Path, ws_hash: &str, ref_name: &str) -> ReviewState {
    let ref_safe = sanitize_ref(ref_name);
    let path = state_dir.join(ws_hash).join(format!("{ref_safe}.json"));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return ReviewState::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Saves review state to disk. Creates directories as needed.
pub fn save_review(state_dir: &Path, ws_hash: &str, ref_name: &str, state: &ReviewState) {
    let ref_safe = sanitize_ref(ref_name);
    let dir = state_dir.join(ws_hash);
    ensure_dir(&dir);
    let path = dir.join(format!("{ref_safe}.json"));
    if let Err(e) = fs::write(
        &path,
        serde_json::to_string_pretty(state).unwrap_or_default(),
    ) {
        eprintln!(
            "[arbiter] failed to save review state to {}: {e}",
            path.display()
        );
    }
}

/// Loads threads from disk. Returns empty vec if file does not exist or is corrupt.
pub fn load_threads(state_dir: &Path, ws_hash: &str, ref_name: &str) -> Vec<Thread> {
    let ref_safe = sanitize_ref(ref_name);
    let path = state_dir
        .join(ws_hash)
        .join(format!("{ref_safe}_threads.json"));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let file: ThreadsFile = serde_json::from_slice(&bytes).unwrap_or_default();
    file.threads
}

/// Saves threads to disk. Creates directories as needed.
pub fn save_threads(state_dir: &Path, ws_hash: &str, ref_name: &str, threads: &[Thread]) {
    let ref_safe = sanitize_ref(ref_name);
    let dir = state_dir.join(ws_hash);
    ensure_dir(&dir);
    let path = dir.join(format!("{ref_safe}_threads.json"));
    let file = ThreadsFile {
        threads: threads.to_vec(),
    };
    if let Err(e) = fs::write(
        &path,
        serde_json::to_string_pretty(&file).unwrap_or_default(),
    ) {
        eprintln!(
            "[arbiter] failed to save threads to {}: {e}",
            path.display()
        );
    }
}

/// Loads session records from disk. Returns empty vec if file does not exist or is corrupt.
pub fn load_sessions(state_dir: &Path, ws_hash: &str) -> Vec<SessionRecord> {
    let path = state_dir.join(ws_hash).join("sessions.json");
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let file: SessionsFile = serde_json::from_slice(&bytes).unwrap_or_default();
    file.sessions
}

/// Saves session records to disk. Creates directories as needed.
pub fn save_sessions(state_dir: &Path, ws_hash: &str, sessions: &[SessionRecord]) {
    let dir = state_dir.join(ws_hash);
    ensure_dir(&dir);
    let path = dir.join("sessions.json");
    let file = SessionsFile {
        sessions: sessions.to_vec(),
    };
    if let Err(e) = fs::write(
        &path,
        serde_json::to_string_pretty(&file).unwrap_or_default(),
    ) {
        eprintln!(
            "[arbiter] failed to save sessions to {}: {e}",
            path.display()
        );
    }
}

/// Appends a session record to the persisted list and saves.
pub fn add_session(state_dir: &Path, ws_hash: &str, record: SessionRecord) {
    let mut sessions = load_sessions(state_dir, ws_hash);
    sessions.push(record);
    save_sessions(state_dir, ws_hash, &sessions);
}

/// SHA256 of the workspace path, truncated to 12 hex chars.
pub fn workspace_hash(path: &Path) -> String {
    let path_str = path.to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(path_str.as_bytes());
    let result = hasher.finalize();
    result.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Fast content hash for change detection (SHA256, first 12 hex chars).
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    result.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn workspace_hash_deterministic() {
        let p = Path::new("/home/user/proj");
        let h1 = workspace_hash(p);
        let h2 = workspace_hash(p);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn workspace_hash_different_paths() {
        let h1 = workspace_hash(Path::new("/a"));
        let h2 = workspace_hash(Path::new("/b"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_deterministic() {
        let h1 = content_hash("hello");
        let h2 = content_hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn content_hash_different_inputs() {
        assert_ne!(content_hash("a"), content_hash("b"));
    }

    #[test]
    fn save_load_review_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let mut state = ReviewState::default();
        state.files.insert(
            "src/main.rs".to_string(),
            FileState {
                status: ReviewStatus::Approved,
                content_hash: "abc123".to_string(),
                updated_at: 1710000000,
                accepted_hunks: Vec::new(),
            },
        );
        save_review(dir, &ws, "main", &state);
        let loaded = load_review(dir, &ws, "main");
        assert_eq!(loaded.files.len(), 1);
        let f = loaded.files.get("src/main.rs").unwrap();
        assert_eq!(f.status, ReviewStatus::Approved);
        assert_eq!(f.content_hash, "abc123");
    }

    #[test]
    fn save_load_review_rules_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let state = ReviewState {
            review_rules: vec![
                "Prefer map_err over match".to_string(),
                "Use constants".to_string(),
            ],
            ..Default::default()
        };
        save_review(dir, &ws, "main", &state);
        let loaded = load_review(dir, &ws, "main");
        assert_eq!(loaded.review_rules.len(), 2);
        assert_eq!(loaded.review_rules[0], "Prefer map_err over match");
        assert_eq!(loaded.review_rules[1], "Use constants");
    }

    #[test]
    fn load_review_missing_returns_default() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_review(tmp.path(), "nonexistent", "main");
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn save_load_threads_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let t = crate::threads::create(
            "src/main.rs",
            22,
            "fix this",
            crate::threads::CreateOpts {
                anchor_content: "let x = 1;".to_string(),
                anchor_context: vec!["}".to_string()],
                ..Default::default()
            },
        );
        let threads = vec![t.clone()];
        save_threads(dir, &ws, "main", &threads);
        let loaded = load_threads(dir, &ws, "main");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, t.id);
        assert_eq!(loaded[0].line, 22);
    }

    #[test]
    fn load_threads_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_threads(tmp.path(), "nonexistent", "main");
        assert!(loaded.is_empty());
    }

    #[test]
    fn save_load_sessions_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let rec = SessionRecord {
            session_id: "sess-1".to_string(),
            created_at: 1710000000,
            last_prompt_preview: "fix the bug".to_string(),
            thread_id: Some("t-1".to_string()),
        };
        save_sessions(dir, &ws, &[rec.clone()]);
        let loaded = load_sessions(dir, &ws);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].session_id, "sess-1");
    }

    #[test]
    fn add_session_appends() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        add_session(
            dir,
            &ws,
            SessionRecord {
                session_id: "s1".to_string(),
                created_at: 1,
                last_prompt_preview: "a".to_string(),
                thread_id: None,
            },
        );
        add_session(
            dir,
            &ws,
            SessionRecord {
                session_id: "s2".to_string(),
                created_at: 2,
                last_prompt_preview: "b".to_string(),
                thread_id: None,
            },
        );
        let loaded = load_sessions(dir, &ws);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].session_id, "s1");
        assert_eq!(loaded[1].session_id, "s2");
    }
}
