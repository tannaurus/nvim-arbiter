//! Test helpers for unit, nvim, and e2e tests.
//!
//! - TempGitRepo: temporary git repo with initial commit
//! - MockAdapter: mock backend adapter with canned responses
//! - ThreadBuilder: builder for Thread with sensible defaults
//! - assert_buf_lines: compare buffer content to expected lines

use arbiter::types::{ThreadOrigin, ThreadStatus};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

/// Temporary git repository for e2e tests.
///
/// Creates a tempdir, runs git init, configures user, and makes an initial
/// commit. Cleaned up on drop.
pub struct TempGitRepo {
    pub dir: TempDir,
}

impl Default for TempGitRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TempGitRepo {
    /// Creates a new temp git repo with an initial commit.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path();
        Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(path)
            .output()
            .expect("git config email");
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .output()
            .expect("git config name");
        std::fs::write(path.join("initial.txt"), "initial\n").expect("write initial");
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(path)
            .output()
            .expect("git commit");
        Self { dir }
    }

    /// Returns the path to the repo root.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes a file, creating parent directories as needed.
    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dirs");
        }
        std::fs::write(path, content).expect("write file");
    }

    /// Stages all changes and commits with the given message.
    pub fn add_and_commit(&self, message: &str) {
        let path = self.dir.path();
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(path)
            .output()
            .expect("git commit");
    }
}

/// Mock backend adapter for e2e tests.
///
/// Returns canned responses in FIFO order and records all call options.
/// Requires the backend module to expose an Adapter trait and setup_with_adapter
/// for injection; until E3-1, this is a stub for the future API.
#[allow(dead_code)]
pub struct MockAdapter {
    responses: Mutex<VecDeque<String>>,
    calls: Mutex<Vec<String>>,
}

impl MockAdapter {
    /// Creates a new mock with the given canned responses.
    #[allow(dead_code)]
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Returns the recorded call descriptions.
    #[allow(dead_code)]
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("lock").clone()
    }
}

/// Builder for Thread objects with sensible defaults.
///
/// Used by tests to construct threads without specifying every field.
/// The full Thread type will be defined in E2-1; this is a placeholder
/// that builds a minimal representation for fixture tests.
#[allow(dead_code)]
pub struct ThreadBuilder {
    file: String,
    line: u32,
    origin: ThreadOrigin,
    status: ThreadStatus,
    message: Option<(String, String)>,
    anchor_content: String,
    anchor_context: Vec<String>,
    pending: bool,
    auto_resolve: bool,
}

impl ThreadBuilder {
    /// Creates a new builder for a thread at the given file and line.
    #[allow(dead_code)]
    pub fn new(file: &str, line: u32) -> Self {
        Self {
            file: file.to_string(),
            line,
            origin: ThreadOrigin::User,
            status: ThreadStatus::Open,
            message: None,
            anchor_content: String::new(),
            anchor_context: Vec::new(),
            pending: false,
            auto_resolve: false,
        }
    }

    #[allow(dead_code)]
    pub fn origin(mut self, origin: ThreadOrigin) -> Self {
        self.origin = origin;
        self
    }

    #[allow(dead_code)]
    pub fn status(mut self, status: ThreadStatus) -> Self {
        self.status = status;
        self
    }

    #[allow(dead_code)]
    pub fn message(mut self, role: &str, text: &str) -> Self {
        self.message = Some((role.to_string(), text.to_string()));
        self
    }

    #[allow(dead_code)]
    pub fn anchor(mut self, content: &str, context: Vec<&str>) -> Self {
        self.anchor_content = content.to_string();
        self.anchor_context = context.into_iter().map(|s| s.to_string()).collect();
        self
    }

    #[allow(dead_code)]
    pub fn pending(mut self) -> Self {
        self.pending = true;
        self
    }

    #[allow(dead_code)]
    pub fn auto_resolve(mut self) -> Self {
        self.auto_resolve = true;
        self
    }

    /// Builds a full Thread for use in thread data tests.
    #[allow(dead_code)]
    pub fn build(self) -> arbiter::threads::Thread {
        use arbiter::threads::{create, CreateOpts};
        let text = self.message.as_ref().map(|(_, t)| t.as_str()).unwrap_or("");
        let mut t = create(
            &self.file,
            self.line,
            text,
            CreateOpts {
                origin: self.origin,
                pending: self.pending,
                auto_resolve: self.auto_resolve,
                anchor_content: self.anchor_content,
                anchor_context: self.anchor_context,
                ..Default::default()
            },
        );
        t.status = self.status;
        t
    }

    /// Builds a ThreadSummary for display/fixture tests.
    #[allow(dead_code)]
    pub fn build_summary(self) -> arbiter::types::ThreadSummary {
        arbiter::types::ThreadSummary {
            id: uuid::Uuid::new_v4().to_string(),
            origin: self.origin,
            line: self.line,
            preview: self
                .message
                .as_ref()
                .map(|(_, t)| t.as_str())
                .unwrap_or("")
                .chars()
                .take(40)
                .collect(),
            status: self.status,
        }
    }
}

/// Asserts that buffer lines match the expected strings.
///
/// Fetches lines from the buffer and compares to the expected slice.
/// Used in nvim and e2e tests.
#[allow(dead_code)]
pub fn assert_buf_lines(buf: &nvim_oxi::api::Buffer, expected: &[&str]) {
    let lines: Vec<String> = buf
        .get_lines(.., false)
        .expect("get lines")
        .map(|s| s.to_string())
        .collect();
    let expected: Vec<&str> = expected.to_vec();
    assert_eq!(
        lines,
        expected,
        "buffer line mismatch: got {} lines, expected {}",
        lines.len(),
        expected.len()
    );
}
