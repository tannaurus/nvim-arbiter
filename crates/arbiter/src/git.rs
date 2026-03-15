//! Async git command execution.
//!
//! Spawns git on a background thread and schedules callbacks on the
//! Neovim main thread via `dispatch::schedule`. All git operations
//! that produce output use this pattern so Neovim API calls in the
//! callback run on the main thread.

use std::process::Command;

/// Result of a git command execution.
///
/// Contains stdout, stderr, and exit code. On binary-not-found or
/// spawn failure, exit_code is -1 and stderr describes the error.
#[derive(Debug, Clone)]
pub struct GitResult {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Exit code; -1 if git was not found or failed to spawn.
    pub exit_code: i32,
}

impl GitResult {
    /// Returns true if the command succeeded (exit code 0).
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Runs a git command in the given directory on a background thread.
///
/// The callback is invoked on the main Neovim thread via `dispatch::schedule`.
/// If git is not found on PATH, the callback receives a result with
/// exit_code -1 and a descriptive stderr message.
pub fn run<F>(cwd: &str, args: &[&str], callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    let cwd = cwd.to_string();
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();

    std::thread::spawn(move || {
        let result = run_git_sync(&cwd, &args);
        crate::dispatch::schedule(move || callback(result));
    });
}

fn run_git_sync(cwd: &str, args: &[String]) -> GitResult {
    let output = match Command::new("git").args(args).current_dir(cwd).output() {
        Ok(o) => o,
        Err(e) => {
            return GitResult {
                stdout: String::new(),
                stderr: format!("git: {e}"),
                exit_code: -1,
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    GitResult {
        stdout,
        stderr,
        exit_code,
    }
}

/// Resolves the merge-base between HEAD and the given ref.
///
/// Returns the merge-base commit hash, or the ref itself if merge-base
/// fails (e.g. no common ancestor). This ensures diffs only show changes
/// on the current branch, matching GitHub/GitLab PR behavior.
fn resolve_merge_base(cwd: &str, ref_name: &str) -> String {
    if ref_name.is_empty() {
        return String::new();
    }
    let result = run_git_sync(
        cwd,
        &[
            "merge-base".to_string(),
            "HEAD".to_string(),
            ref_name.to_string(),
        ],
    );
    if result.success() {
        result.stdout.trim().to_string()
    } else {
        ref_name.to_string()
    }
}

/// Runs `git diff <merge-base> -- <file>` and invokes the callback with the unified diff output.
///
/// Uses the merge-base of HEAD and the ref so only branch changes appear.
pub fn diff<F>(cwd: &str, ref_name: &str, file: &str, callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    let cwd = cwd.to_string();
    let ref_name = ref_name.to_string();
    let file = file.to_string();
    std::thread::spawn(move || {
        let base = resolve_merge_base(&cwd, &ref_name);
        let args: Vec<&str> = if base.is_empty() {
            vec!["diff", "--", &file]
        } else {
            vec!["diff", &base, "--", &file]
        };
        let result = run_git_sync(
            &cwd,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        crate::dispatch::schedule(move || callback(result));
    });
}

/// Runs `git diff --name-status <merge-base>` and invokes the callback with the file list.
///
/// Uses the merge-base of HEAD and the ref so only branch changes appear.
pub fn diff_names<F>(cwd: &str, ref_name: &str, callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    let cwd = cwd.to_string();
    let ref_name = ref_name.to_string();
    std::thread::spawn(move || {
        let base = resolve_merge_base(&cwd, &ref_name);
        let args: Vec<&str> = if base.is_empty() {
            vec!["diff", "--name-status"]
        } else {
            vec!["diff", "--name-status", &base]
        };
        let result = run_git_sync(
            &cwd,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        crate::dispatch::schedule(move || callback(result));
    });
}

/// Runs `git ls-files --others --exclude-standard` and invokes the callback with untracked paths.
pub fn untracked<F>(cwd: &str, callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    run(
        cwd,
        &["ls-files", "--others", "--exclude-standard"],
        callback,
    );
}

/// Runs `git show <merge-base>:<file>` and invokes the callback with file content at the base.
///
/// Uses the merge-base so side-by-side shows the file as it was when the branch diverged.
pub fn show<F>(cwd: &str, ref_name: &str, file: &str, callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    let cwd = cwd.to_string();
    let ref_name = ref_name.to_string();
    let file = file.to_string();
    std::thread::spawn(move || {
        let base = resolve_merge_base(&cwd, &ref_name);
        let effective_ref = if base.is_empty() { ref_name } else { base };
        let obj = format!("{effective_ref}:{file}");
        let result = run_git_sync(&cwd, &["show".to_string(), obj]);
        crate::dispatch::schedule(move || callback(result));
    });
}

/// Runs `git diff <merge-base>` (full repo) and invokes the callback with the unified diff output.
///
/// Uses the merge-base so only branch changes appear.
pub fn diff_full<F>(cwd: &str, ref_name: &str, callback: F)
where
    F: FnOnce(GitResult) + Send + 'static,
{
    let cwd = cwd.to_string();
    let ref_name = ref_name.to_string();
    std::thread::spawn(move || {
        let base = resolve_merge_base(&cwd, &ref_name);
        let args: Vec<&str> = if base.is_empty() {
            vec!["diff"]
        } else {
            vec!["diff", &base]
        };
        let result = run_git_sync(
            &cwd,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        crate::dispatch::schedule(move || callback(result));
    });
}

/// Returns the modification time of a file as epoch seconds, or None if missing.
///
/// Synchronous; uses `std::fs::metadata`. Safe to call from any thread.
pub fn file_mtime(path: &str) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_mtime_existing_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("arbiter_file_mtime_test");
        std::fs::write(&path, "x").expect("write");
        let mtime = file_mtime(path.to_str().unwrap());
        assert!(mtime.is_some());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_mtime_missing_file() {
        let mtime = file_mtime("/nonexistent/path/12345");
        assert!(mtime.is_none());
    }

    #[test]
    fn run_git_sync_status_in_repo() {
        let dir = std::env::temp_dir().join("arbiter_git_sync_test");
        let _ = std::fs::create_dir_all(&dir);
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .output();
        let result = run_git_sync(
            dir.to_str().unwrap(),
            &["status".to_string(), "--porcelain".to_string()],
        );
        assert!(result.success());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_git_sync_invalid_dir() {
        let result = run_git_sync("/nonexistent/dir/12345", &["status".to_string()]);
        assert!(!result.success());
    }

    #[test]
    fn run_git_sync_diff_names_empty_repo() {
        let dir = std::env::temp_dir().join("arbiter_git_diff_names_test");
        let _ = std::fs::create_dir_all(&dir);
        let _ = Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .output();
        let result = run_git_sync(
            dir.to_str().unwrap(),
            &["diff".to_string(), "--name-status".to_string()],
        );
        assert!(result.success());
        assert!(result.stdout.trim().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_result_success_checks_exit_code() {
        let ok = GitResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        assert!(ok.success());

        let fail = GitResult {
            stdout: String::new(),
            stderr: "error".to_string(),
            exit_code: 1,
        };
        assert!(!fail.success());

        let not_found = GitResult {
            stdout: String::new(),
            stderr: "not found".to_string(),
            exit_code: -1,
        };
        assert!(!not_found.success());
    }
}
