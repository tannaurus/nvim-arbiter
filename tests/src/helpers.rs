//! Test helpers for integration tests.
//!
//! - TempGitRepo: temporary git repo with initial commit

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// Temporary git repository for integration tests.
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

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dirs");
        }
        std::fs::write(path, content).expect("write file");
    }

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
