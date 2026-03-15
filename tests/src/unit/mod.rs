//! Pure Rust unit tests (no Neovim dependency).

#[cfg(test)]
use crate::helpers::{TempGitRepo, ThreadBuilder};
#[cfg(test)]
use arbiter::types::{ThreadOrigin, ThreadStatus};

#[test]
fn unit_temp_git_repo_creates_valid_repo() {
    let repo = TempGitRepo::new();
    let out = std::process::Command::new("git")
        .args(["status"])
        .current_dir(repo.path())
        .output()
        .expect("git status");
    assert!(out.status.success());
}

#[test]
fn unit_thread_builder_defaults_and_overrides() {
    let summary = ThreadBuilder::new("src/main.rs", 22)
        .origin(ThreadOrigin::Agent)
        .status(ThreadStatus::Resolved)
        .message("user", "fix this")
        .anchor("let x = 1", vec!["  fn foo() {"])
        .pending()
        .build_summary();
    assert_eq!(summary.origin, ThreadOrigin::Agent);
    assert_eq!(summary.status, ThreadStatus::Resolved);
    assert_eq!(summary.line, 22);
    assert!(summary.preview.contains("fix"));
}
