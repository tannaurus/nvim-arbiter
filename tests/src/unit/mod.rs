//! Pure Rust unit tests (no Neovim dependency).

#[cfg(test)]
use crate::helpers::TempGitRepo;

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
fn unit_temp_git_repo_write_and_commit() {
    let repo = TempGitRepo::new();
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.add_and_commit("add main");
    let out = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(repo.path())
        .output()
        .expect("git log");
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(log.contains("add main"));
}
