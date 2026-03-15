//! Mtime polling and file list refresh.
//!
//! Uses nvim-oxi libuv TimerHandle for periodic checks. File poll (2s default)
//! checks current file mtime; file list poll (5s default) runs git diff_names +
//! untracked.

use crate::config;
use crate::git;
use crate::review;
use crate::state;
use crate::types::ThreadStatus;
use nvim_oxi::libuv::TimerHandle;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

thread_local! {
    static FILE_TIMER: RefCell<Option<TimerHandle>> = const { RefCell::new(None) };
    static LIST_TIMER: RefCell<Option<TimerHandle>> = const { RefCell::new(None) };
    static TARGET_PATH: RefCell<Option<String>> = const { RefCell::new(None) };
    static LAST_MTIME: RefCell<Option<(String, i64)>> = const { RefCell::new(None) };
    static LAST_FILE_LIST: RefCell<Option<HashSet<String>>> = const { RefCell::new(None) };
}

/// Starts both poll timers. Uses config for intervals.
///
/// Called from `review::open()`.
pub fn start(_cwd: &str) {
    let cfg = config::get();
    let poll_ms = cfg.review.poll_interval;
    let list_ms = cfg.review.file_list_interval;

    FILE_TIMER.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_some() {
            return;
        }
        match TimerHandle::start(
            Duration::from_millis(poll_ms),
            Duration::from_millis(poll_ms),
            |_timer| {
                nvim_oxi::schedule(move |_| on_file_tick());
                Ok::<(), std::io::Error>(())
            },
        ) {
            Ok(h) => *opt = Some(h),
            Err(e) => {
                let _ = nvim_oxi::api::notify(
                    &format!("Failed to start file poll timer: {e}"),
                    nvim_oxi::api::types::LogLevel::Warn,
                    &nvim_oxi::Dictionary::default(),
                );
            }
        }
    });

    LIST_TIMER.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_some() {
            return;
        }
        match TimerHandle::start(
            Duration::from_millis(list_ms),
            Duration::from_millis(list_ms),
            |_timer| {
                nvim_oxi::schedule(move |_| on_list_tick());
                Ok::<(), std::io::Error>(())
            },
        ) {
            Ok(h) => *opt = Some(h),
            Err(e) => {
                let _ = nvim_oxi::api::notify(
                    &format!("Failed to start list poll timer: {e}"),
                    nvim_oxi::api::types::LogLevel::Warn,
                    &nvim_oxi::Dictionary::default(),
                );
            }
        }
    });
}

/// Stops and drops both timers.
///
/// Called from `review::close()`.
pub fn stop() {
    FILE_TIMER.with(|cell| {
        let mut opt = cell.borrow_mut();
        if let Some(mut h) = opt.take() {
            let _ = h.stop();
        }
    });
    LIST_TIMER.with(|cell| {
        let mut opt = cell.borrow_mut();
        if let Some(mut h) = opt.take() {
            let _ = h.stop();
        }
    });
    TARGET_PATH.with(|c| *c.borrow_mut() = None);
    LAST_MTIME.with(|c| *c.borrow_mut() = None);
    LAST_FILE_LIST.with(|c| *c.borrow_mut() = None);
}

/// Sets the current file path for mtime polling.
///
/// Called when the user selects a different file.
pub fn set_target(file_path: Option<&str>) {
    TARGET_PATH.with(|c| {
        *c.borrow_mut() = file_path.map(String::from);
    });
}

fn on_file_tick() {
    let path = TARGET_PATH.with(|c| c.borrow().clone());
    let Some(ref path) = path else {
        return;
    };

    let cwd = review::with_active(|r| r.cwd.clone());
    let Some(cwd) = cwd else {
        return;
    };

    let full = Path::new(&cwd).join(path);
    let path_str = full.to_string_lossy().to_string();
    let mtime = git::file_mtime(&path_str);

    let changed = LAST_MTIME.with(|cell| {
        let mut opt = cell.borrow_mut();
        let prev = opt
            .as_ref()
            .filter(|(p, _)| p == &path_str)
            .map(|(_, m)| *m);
        if let Some(m) = mtime {
            *opt = Some((path_str.clone(), m));
            prev != Some(m)
        } else {
            opt.take();
            false
        }
    });

    if changed {
        let auto_resolved = review::with_active(|r| {
            let mut resolved = Vec::new();
            for t in r.threads.iter_mut() {
                if t.auto_resolve && t.status == ThreadStatus::Open && t.file == *path {
                    t.status = ThreadStatus::Resolved;
                    let preview: String = t
                        .messages
                        .first()
                        .map(|m| m.text.chars().take(40).collect())
                        .unwrap_or_default();
                    resolved.push((preview, t.file.clone(), t.line));
                }
            }
            if !resolved.is_empty() {
                let state_dir = r.config.state_dir();
                let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
            }
            resolved
        });
        if let Some(resolved) = auto_resolved {
            for (preview, file, line) in resolved {
                let _ = nvim_oxi::api::notify(
                    &format!("[arbiter] Auto-resolved: \"{preview}\" at {file}:{line}"),
                    nvim_oxi::api::types::LogLevel::Info,
                    &nvim_oxi::Dictionary::default(),
                );
            }
        }
        review::with_active(review::refresh_file);
    }
}

fn on_list_tick() {
    let Some((cwd, ref_name)) = review::with_active(|r| (r.cwd.clone(), r.ref_name.clone())) else {
        return;
    };

    git::diff_names(&cwd, &ref_name, {
        let cwd = cwd.clone();
        move |diff_result| {
            let mut files = HashSet::new();
            if diff_result.success() {
                for line in diff_result.stdout.lines() {
                    let trimmed = line.trim();
                    if let Some((_, p)) = trimmed.split_once('\t') {
                        files.insert(p.trim().to_string());
                    }
                }
            }

            git::untracked(&cwd, {
                move |untracked_result| {
                    if untracked_result.success() {
                        for line in untracked_result.stdout.lines() {
                            let p = line.trim();
                            if !p.is_empty() {
                                files.insert(p.to_string());
                            }
                        }
                    }

                    let changed = LAST_FILE_LIST.with(|cell| {
                        let mut opt = cell.borrow_mut();
                        let prev = opt.clone();
                        *opt = Some(files.clone());
                        prev.is_none_or(|p| p != files)
                    });

                    if changed {
                        review::with_active(review::refresh_file_list);
                    }
                }
            });
        }
    });
}
