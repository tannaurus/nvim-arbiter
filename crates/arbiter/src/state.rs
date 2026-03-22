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

const CACHE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Persisted review state (file statuses per path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewState {
    /// Plugin version that wrote this state. Mismatched versions are
    /// discarded on load to avoid stale-cache bugs across upgrades.
    #[serde(default)]
    pub version: String,
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
    #[serde(default)]
    pub version: String,
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

/// Loads review state from disk. Returns default if file does not exist,
/// is corrupt, or was written by a different plugin version.
pub fn load_review(state_dir: &Path, ws_hash: &str, ref_name: &str) -> ReviewState {
    let ref_safe = sanitize_ref(ref_name);
    let path = state_dir.join(ws_hash).join(format!("{ref_safe}.json"));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return ReviewState::default(),
    };
    let state: ReviewState = serde_json::from_slice(&bytes).unwrap_or_default();
    if state.version != CACHE_VERSION {
        return ReviewState::default();
    }
    state
}

/// Saves review state to disk. Creates directories as needed.
/// Stamps the current plugin version into the state before writing.
pub fn save_review(state_dir: &Path, ws_hash: &str, ref_name: &str, state: &ReviewState) {
    let ref_safe = sanitize_ref(ref_name);
    let dir = state_dir.join(ws_hash);
    ensure_dir(&dir);
    let path = dir.join(format!("{ref_safe}.json"));
    let mut versioned = state.clone();
    versioned.version = CACHE_VERSION.to_string();
    let Ok(json) = serde_json::to_string_pretty(&versioned) else {
        return;
    };
    let _ = fs::write(&path, json);
}

/// Loads threads from disk. Returns empty vec if file does not exist,
/// is corrupt, or was written by a different plugin version.
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
    if file.version != CACHE_VERSION {
        return Vec::new();
    }
    file.threads
}

/// Saves threads to disk. Creates directories as needed.
/// Stamps the current plugin version before writing.
pub fn save_threads(state_dir: &Path, ws_hash: &str, ref_name: &str, threads: &[Thread]) {
    let ref_safe = sanitize_ref(ref_name);
    let dir = state_dir.join(ws_hash);
    ensure_dir(&dir);
    let path = dir.join(format!("{ref_safe}_threads.json"));
    let file = ThreadsFile {
        version: CACHE_VERSION.to_string(),
        threads: threads.to_vec(),
    };
    let Ok(json) = serde_json::to_string_pretty(&file) else {
        return;
    };
    let _ = fs::write(&path, json);
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
    fn load_review_stale_version_returns_default() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let stale_json = r#"{"version":"0.0.0","files":{"a.rs":{"status":"Approved","content_hash":"abc","updated_at":1,"accepted_hunks":[]}}}"#;
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main.json"), stale_json).unwrap();
        let loaded = load_review(dir, &ws, "main");
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn load_review_missing_version_returns_default() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let no_version_json = r#"{"files":{"a.rs":{"status":"Approved","content_hash":"abc","updated_at":1,"accepted_hunks":[]}}}"#;
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main.json"), no_version_json).unwrap();
        let loaded = load_review(dir, &ws, "main");
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn load_threads_stale_version_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let stale_json = r#"{"version":"0.0.0","threads":[]}"#;
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main_threads.json"), stale_json).unwrap();
        let loaded = load_threads(dir, &ws, "main");
        assert!(loaded.is_empty());
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
    fn save_review_stamps_current_version() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let state = ReviewState::default();
        save_review(dir, &ws, "main", &state);

        let raw = fs::read_to_string(dir.join(&ws).join("main.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"].as_str().unwrap(), CACHE_VERSION);
    }

    #[test]
    fn save_threads_stamps_current_version() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        save_threads(dir, &ws, "main", &[]);

        let raw = fs::read_to_string(dir.join(&ws).join("main_threads.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"].as_str().unwrap(), CACHE_VERSION);
    }

    #[test]
    fn load_review_corrupt_json_returns_default() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main.json"), "{{not valid json!!").unwrap();
        let loaded = load_review(dir, &ws, "main");
        assert!(loaded.files.is_empty());
        assert!(loaded.version.is_empty());
    }

    #[test]
    fn load_review_empty_string_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main.json"), "").unwrap();
        let loaded = load_review(dir, &ws, "main");
        assert!(loaded.files.is_empty());
    }

    #[test]
    fn load_threads_corrupt_json_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("main_threads.json"), "not json").unwrap();
        let loaded = load_threads(dir, &ws, "main");
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_sessions_missing_file() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_sessions(tmp.path(), "nonexistent");
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_sessions_corrupt_json() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let ref_dir = dir.join(&ws);
        fs::create_dir_all(&ref_dir).unwrap();
        fs::write(ref_dir.join("sessions.json"), "not valid json!!").unwrap();
        let loaded = load_sessions(dir, &ws);
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_review_ref_with_slash_sanitized() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);
        let state = ReviewState {
            files: {
                let mut m = HashMap::new();
                m.insert(
                    "x.rs".to_string(),
                    FileState {
                        status: ReviewStatus::Approved,
                        content_hash: "h".to_string(),
                        updated_at: 1,
                        accepted_hunks: Vec::new(),
                    },
                );
                m
            },
            ..Default::default()
        };
        save_review(dir, &ws, "feature/foo", &state);
        let loaded = load_review(dir, &ws, "feature/foo");
        assert_eq!(loaded.files.len(), 1);

        let sanitized = sanitize_ref("feature/foo");
        assert_eq!(sanitized, "feature_foo");
        assert!(dir.join(&ws).join("feature_foo.json").exists());
    }

    #[test]
    fn sanitize_ref_collision() {
        assert_eq!(sanitize_ref("a/b"), sanitize_ref("a_b"));

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let ws = workspace_hash(dir);

        let state1 = ReviewState {
            files: {
                let mut m = HashMap::new();
                m.insert(
                    "first.rs".to_string(),
                    FileState {
                        status: ReviewStatus::Approved,
                        content_hash: "aaa".to_string(),
                        updated_at: 1,
                        accepted_hunks: Vec::new(),
                    },
                );
                m
            },
            ..Default::default()
        };
        save_review(dir, &ws, "a/b", &state1);

        let state2 = ReviewState {
            files: {
                let mut m = HashMap::new();
                m.insert(
                    "second.rs".to_string(),
                    FileState {
                        status: ReviewStatus::Approved,
                        content_hash: "bbb".to_string(),
                        updated_at: 2,
                        accepted_hunks: Vec::new(),
                    },
                );
                m
            },
            ..Default::default()
        };
        save_review(dir, &ws, "a_b", &state2);

        let loaded = load_review(dir, &ws, "a/b");
        assert!(loaded.files.contains_key("second.rs"));
        assert!(!loaded.files.contains_key("first.rs"));
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn content_hash_deterministic_prop(input in ".*") {
            let a = content_hash(&input);
            let b = content_hash(&input);
            prop_assert_eq!(a, b);
        }
    }
}
