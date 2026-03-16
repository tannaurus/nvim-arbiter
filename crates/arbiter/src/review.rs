//! Review lifecycle and workbench management.
//!
//! Creates the tabpage, file panel, diff panel. Manages agent mode state.
//! Holds `Review` in a thread-local `RefCell<Option<Review>>`.

use crate::backend;
use crate::config;
use crate::diff::{self, Hunk};
use crate::file_panel;
use crate::git;
use crate::poll;
use crate::prompts;
use crate::state;
use crate::threads;
use crate::types::Role;
use crate::types::ThreadOrigin;
use crate::types::ThreadStatus;
use crate::types::{FileStatus, ReviewStatus};
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::opts::SetKeymapOpts;
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self};
use nvim_oxi::Dictionary;
use nvim_oxi::IntoResult;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static ACTIVE: RefCell<Option<Review>> = const { RefCell::new(None) };
    static SUMMARY_WIN: RefCell<Option<nvim_oxi::api::Window>> = const { RefCell::new(None) };
}

fn safe_callback<F: FnOnce() + std::panic::UnwindSafe>(f: F) {
    if let Err(payload) = std::panic::catch_unwind(f) {
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        close();
        api::err_writeln(&format!("[arbiter] panic: {msg}"));
    }
}

fn close_summary_float() {
    SUMMARY_WIN.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
}

/// Panel (buffer + window) for file list or diff.
#[derive(Debug, Clone)]
pub struct Panel {
    /// Buffer handle.
    pub buf: nvim_oxi::api::Buffer,
    /// Window handle.
    pub win: nvim_oxi::api::Window,
}

/// Side-by-side diff view (ref vs working tree).
#[derive(Debug, Clone)]
pub struct SideBySide {
    /// Buffer for ref content.
    pub old_buf: nvim_oxi::api::Buffer,
    /// Buffer for working tree content.
    pub new_buf: nvim_oxi::api::Buffer,
    /// Window for the ref buffer.
    pub old_win: nvim_oxi::api::Window,
    /// Window for the working tree buffer.
    pub new_win: nvim_oxi::api::Window,
}

/// Central runtime object for the review workbench.
///
/// One exists per open review. Dropped on close.
pub struct Review {
    /// Ref to diff against (e.g. "main"); empty = working tree.
    pub ref_name: String,
    /// Working directory captured at open time.
    pub cwd: String,
    /// Tabpage handle.
    pub tabpage: nvim_oxi::api::TabPage,
    /// File panel (left).
    pub file_panel: Panel,
    /// Diff panel (right).
    pub diff_panel: Panel,
    /// File list: path, FileStatus, ReviewStatus.
    pub files: Vec<(String, FileStatus, ReviewStatus)>,
    /// Path -> index in files.
    pub file_index: HashMap<String, usize>,
    /// Currently selected file path.
    pub current_file: Option<String>,
    /// Threads for the current ref.
    pub threads: Vec<threads::Thread>,
    /// Line-to-path mapping for file panel (1-based buffer line -> path).
    pub file_panel_line_to_path: HashMap<usize, String>,
    /// Thread ID -> buffer line in diff panel.
    pub thread_buf_lines: HashMap<String, usize>,
    /// Hunks for the current file (from last render). Used for ]c/[c.
    pub current_hunks: Vec<Hunk>,
    /// Path -> content hash for approved files. Used for ga persistence.
    pub file_content_hash: HashMap<String, String>,
    /// Whether to show resolved threads.
    pub show_resolved: bool,
    /// Whether side-by-side view is active.
    pub side_by_side: bool,
    /// Side-by-side buffers and window, if active.
    pub sbs: Option<SideBySide>,
    /// Plugin config snapshot.
    pub config: config::Config,
    /// Collapsed directory paths in the file panel.
    pub collapsed_dirs: std::collections::HashSet<String>,
    /// Directory path at each buffer line in the file panel (1-based).
    pub file_panel_line_to_dir: HashMap<usize, String>,
    /// Inline thread indicators: 0-based buffer line -> thread ID.
    /// Placed at the actual diff content line where a thread is anchored.
    pub thread_inline_marks: HashMap<usize, String>,
    /// Thread to open after the next diff render completes.
    pub pending_thread_open: Option<String>,
    /// Raw diff text for the current file. Retained so individual hunks
    /// can be extracted for staging without re-running git.
    pub current_diff_text: String,
    /// Per-file accepted hunk content hashes (review checklist).
    /// Used to fold and dim accepted hunks in the diff panel. Persisted in review state.
    pub accepted_hunks: HashMap<String, HashSet<String>>,
}

/// Runs a closure with the active Review, if one exists.
///
/// Returns `None` if no review is active or if the state is already
/// borrowed (re-entrancy guard). This prevents `RefCell` panics when
/// Neovim API calls inside the closure trigger autocmds that re-enter.
pub fn with_active<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Review) -> R,
{
    ACTIVE.with(|cell| {
        let mut opt = cell.try_borrow_mut().ok()?;
        let review = opt.as_mut()?;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(review))) {
            Ok(val) => Some(val),
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                api::err_writeln(&format!("[arbiter] panic in with_active: {msg}"));
                None
            }
        }
    })
}

/// Returns true if a review workbench is open.
///
/// Returns `false` if the state is currently borrowed (re-entrancy).
pub fn is_active() -> bool {
    ACTIVE.with(|cell| cell.try_borrow().map(|opt| opt.is_some()).unwrap_or(false))
}

/// Opens the thread window for the thread at the given absolute file path and line.
pub fn open_thread_at(abs_file: &str, line: u32) {
    let abs = std::path::Path::new(abs_file);
    with_active(|r| {
        let cwd_path = std::path::Path::new(&r.cwd);
        let rel = abs
            .strip_prefix(cwd_path)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or(abs_file);
        let tid = r
            .threads
            .iter()
            .find(|t| t.file == rel && t.line == line)
            .map(|t| t.id.clone());
        let Some(tid) = tid else {
            api::err_writeln(&format!("[arbiter] no thread at {rel}:{line}"));
            return;
        };
        let needs_file_switch = r.current_file.as_deref() != Some(rel);
        if needs_file_switch {
            r.pending_thread_open = Some(tid.clone());
            let rel = rel.to_string();
            select_file_impl(r, &rel);
        } else {
            scroll_to_thread_and_open(r, &tid);
        }
    });
}

fn scroll_to_thread_and_open(review: &mut Review, tid: &str) {
    let Some(t) = review.threads.iter().find(|t| t.id == tid) else {
        return;
    };
    let line_refs: Vec<String> = review
        .diff_panel
        .buf
        .get_lines(.., false)
        .map(|iter| iter.map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let str_refs: Vec<&str> = line_refs.iter().map(|s| s.as_str()).collect();
    if let Some(buf_line) =
        diff::source_to_buf_line(&review.current_hunks, t.line as usize, &str_refs)
    {
        let target = buf_line + 1;
        let _ = api::set_current_win(&review.diff_panel.win);
        let _ = review.diff_panel.win.set_cursor(target, 0);
        let _ = api::command("normal! zz");
    }
    open_thread_panel(review, t);
}

/// Opens the review workbench in a new tabpage.
///
/// Creates tabpage, file panel (left vsplit), diff panel (right), runs
/// `git::diff_names` and `git::untracked`, loads state, renders file panel,
/// selects first file, sets keymaps, starts poll timer.
pub fn open(ref_name: Option<&str>) -> nvim_oxi::Result<()> {
    if is_active() {
        let tab_valid =
            with_active(|r| r.tabpage.get_number().is_ok() && r.diff_panel.win.is_valid())
                .unwrap_or(false);
        if tab_valid {
            close();
            return Ok(());
        }
        close();
    }

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let config = config::get().clone();
    let ref_name = ref_name
        .or_else(|| config.default_ref_for(&cwd))
        .unwrap_or("")
        .to_string();

    api::command("tabnew")?;
    let tabpage = api::get_current_tabpage();

    let setup_result = (|| -> nvim_oxi::Result<(
        nvim_oxi::api::Buffer,
        nvim_oxi::api::Window,
        nvim_oxi::api::Buffer,
        nvim_oxi::api::Window,
    )> {
        let mut diff_panel_buf = api::create_buf(false, true)?;
        api::set_option_value("buftype", "nofile", &OptionOpts::builder().buffer(diff_panel_buf.clone()).build())?;
        api::set_option_value("modifiable", false, &OptionOpts::builder().buffer(diff_panel_buf.clone()).build())?;
        api::set_option_value("filetype", "diff", &OptionOpts::builder().buffer(diff_panel_buf.clone()).build())?;
        diff_panel_buf.set_name("[arbiter] diff")?;

        let mut diff_panel_win = api::get_current_win();
        diff_panel_win.set_buf(&diff_panel_buf)?;
        let dp_opts = OptionOpts::builder().win(diff_panel_win.clone()).build();
        api::set_option_value("winbar", " [arbiter] diff", &dp_opts)?;

        api::command("topleft vertical 40split")?;
        let mut file_panel_win = api::get_current_win();

        let mut file_panel_buf = api::create_buf(false, true)?;
        api::set_option_value("buftype", "nofile", &OptionOpts::builder().buffer(file_panel_buf.clone()).build())?;
        api::set_option_value("modifiable", false, &OptionOpts::builder().buffer(file_panel_buf.clone()).build())?;
        file_panel_buf.set_name("[arbiter] files")?;
        file_panel_win.set_buf(&file_panel_buf)?;
        let fp_opts = OptionOpts::builder().win(file_panel_win.clone()).build();
        api::set_option_value("foldenable", false, &fp_opts)?;
        api::set_option_value("winfixwidth", true, &fp_opts)?;
        api::set_option_value("winbar", " [arbiter] files", &fp_opts)?;

        api::command("wincmd l")?;

        Ok((file_panel_buf, file_panel_win, diff_panel_buf, diff_panel_win))
    })();

    let (file_panel_buf, file_panel_win, diff_panel_buf, diff_panel_win) = match setup_result {
        Ok(r) => r,
        Err(e) => {
            let _ = api::set_current_tabpage(&tabpage);
            let _ = api::command("tabclose");
            let _ = api::notify(
                &format!("[arbiter] failed to create panels: {e}"),
                nvim_oxi::api::types::LogLevel::Error,
                &Dictionary::default(),
            );
            return Ok(());
        }
    };

    let state_dir = config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&cwd));
    let review_state = state::load_review(&state_dir, &ws_hash, &ref_name);
    let threads = state::load_threads(&state_dir, &ws_hash, &ref_name);

    git::diff_names(&cwd, &ref_name, {
        let cwd = cwd.clone();
        let ref_name = ref_name.clone();
        let file_panel_buf = file_panel_buf.clone();
        let file_panel_win = file_panel_win.clone();
        let diff_panel_win = diff_panel_win.clone();
        let tabpage = tabpage.clone();
        let config = config.clone();
        let review_state = review_state.clone();
        let threads = threads.clone();
        move |result| {
            if !result.success() {
                let _ = api::set_current_tabpage(&tabpage);
                let _ = api::command("tabclose");
                let _ = api::notify(
                    &format!("[arbiter] git diff failed: {}", result.stderr),
                    nvim_oxi::api::types::LogLevel::Error,
                    &Dictionary::default(),
                );
                return;
            }

            let mut files = parse_diff_names(&result.stdout, &review_state);
            let mut file_index: HashMap<String, usize> = HashMap::new();
            for (i, (path, _, _)) in files.iter().enumerate() {
                file_index.insert(path.clone(), i);
            }

            git::untracked(&cwd, {
                let cwd = cwd.clone();
                let mut file_panel_buf = file_panel_buf.clone();
                let file_panel_win = file_panel_win.clone();
                let diff_panel_win = diff_panel_win.clone();
                let tabpage = tabpage.clone();
                let config = config.clone();
                let review_state = review_state.clone();
                let threads = threads.clone();
                let file_index = file_index.clone();
                move |untracked_result| {
                    if untracked_result.success() {
                        for line in untracked_result.stdout.lines() {
                            let path = line.trim();
                            if !path.is_empty() && !file_index.contains_key(path) {
                                let rs = review_state
                                    .files
                                    .get(path)
                                    .map(|f| f.status)
                                    .unwrap_or(ReviewStatus::Unreviewed);
                                files.push((path.to_string(), FileStatus::Untracked, rs));
                            }
                        }
                    }

                    let mut file_index: HashMap<String, usize> = HashMap::new();
                    for (i, (path, _, _)) in files.iter().enumerate() {
                        file_index.insert(path.clone(), i);
                    }

                    let collapsed = std::collections::HashSet::new();
                    let open_thread_counts = open_thread_count_map(&threads);
                    let render_result = match file_panel::render(
                        &mut file_panel_buf,
                        &files,
                        &collapsed,
                        &open_thread_counts,
                    ) {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = api::set_current_tabpage(&tabpage);
                            let _ = api::command("tabclose");
                            let _ = api::notify(
                                &format!("[arbiter] failed to render file panel: {e}"),
                                nvim_oxi::api::types::LogLevel::Error,
                                &Dictionary::default(),
                            );
                            return;
                        }
                    };

                    let current_file = files
                        .iter()
                        .find(|(_, _, rs)| *rs != ReviewStatus::Approved)
                        .or_else(|| files.first())
                        .map(|(p, _, _)| p.clone());

                    let mut review = Review {
                        ref_name: ref_name.clone(),
                        cwd: cwd.clone(),
                        tabpage: tabpage.clone(),
                        file_panel: Panel {
                            buf: file_panel_buf.clone(),
                            win: file_panel_win.clone(),
                        },
                        diff_panel: Panel {
                            buf: diff_panel_buf.clone(),
                            win: diff_panel_win.clone(),
                        },
                        files: files.clone(),
                        file_index: file_index.clone(),
                        current_file: current_file.clone(),
                        threads: threads.clone(),
                        file_panel_line_to_path: render_result.line_to_path,
                        thread_buf_lines: HashMap::new(),
                        current_hunks: Vec::new(),
                        file_content_hash: review_state
                            .files
                            .iter()
                            .filter(|(_, f)| !f.content_hash.is_empty())
                            .map(|(p, f)| (p.clone(), f.content_hash.clone()))
                            .collect(),
                        show_resolved: false,
                        side_by_side: false,
                        sbs: None,
                        config: config.clone(),
                        collapsed_dirs: collapsed,
                        file_panel_line_to_dir: render_result.line_to_dir,
                        thread_inline_marks: HashMap::new(),
                        pending_thread_open: None,
                        current_diff_text: String::new(),
                        accepted_hunks: review_state
                            .files
                            .iter()
                            .filter(|(_, f)| !f.accepted_hunks.is_empty())
                            .map(|(p, f)| (p.clone(), f.accepted_hunks.iter().cloned().collect()))
                            .collect(),
                    };

                    set_close_keymap(&mut review.file_panel.buf);
                    set_close_keymap(&mut review.diff_panel.buf);
                    set_file_panel_keymaps(&mut review.file_panel.buf);
                    set_diff_panel_keymaps(&mut review.diff_panel.buf, &review.config);

                    if let Some(path) = &current_file {
                        select_file_impl(&mut review, path);
                        let panel_line = review
                            .file_panel_line_to_path
                            .iter()
                            .find(|(_, p)| *p == path)
                            .map(|(l, _)| *l);
                        if let Some(line) = panel_line {
                            let _ = review.file_panel.win.set_cursor(line + 1, 0);
                        }
                    }

                    poll::start(&cwd);

                    let tab_nr = review.tabpage.get_number().unwrap_or(0);
                    ACTIVE.with(|cell| {
                        *cell.borrow_mut() = Some(review);
                    });

                    let tab_pattern = format!("{tab_nr}");
                    let opts = nvim_oxi::api::opts::CreateAutocmdOpts::builder()
                        .patterns([tab_pattern.as_str()])
                        .callback(|_| {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if !is_active() {
                                    return;
                                }
                                let tab_gone = with_active(|r| r.tabpage.get_number().is_err())
                                    .unwrap_or(true);
                                if tab_gone {
                                    close();
                                }
                            }));
                            Ok::<bool, nvim_oxi::Error>(true)
                        })
                        .build();
                    let _ = api::create_autocmd(["TabClosed"], &opts);
                }
            });
        }
    });

    Ok(())
}

pub(crate) fn parse_diff_names(
    stdout: &str,
    review_state: &state::ReviewState,
) -> Vec<(String, FileStatus, ReviewStatus)> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (status_ch, path) = match line.split_once('\t') {
            Some((s, p)) => (s.chars().next().unwrap_or('M'), p.trim()),
            None => continue,
        };
        let fs = match status_ch {
            'A' => FileStatus::Added,
            'D' => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        let rs = review_state
            .files
            .get(path)
            .map(|f| f.status)
            .unwrap_or(ReviewStatus::Unreviewed);
        out.push((path.to_string(), fs, rs));
    }
    out
}

fn set_close_keymap(buf: &mut nvim_oxi::api::Buffer) {
    let opts = SetKeymapOpts::builder()
        .callback(|_| safe_callback(close))
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts);
}

fn set_file_panel_keymaps(buf: &mut nvim_oxi::api::Buffer) {
    let opts = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|review| {
                let (row, _) = review
                    .file_panel
                    .win
                    .get_cursor()
                    .into_result()
                    .unwrap_or((1, 0));
                let line = row;
                if let Some(path) = file_panel::path_at_line(&review.file_panel_line_to_path, line)
                {
                    select_file_impl(review, &path);
                    let _ = api::set_current_win(&review.diff_panel.win);
                } else if let Some(dir) = review.file_panel_line_to_dir.get(&line).cloned() {
                    if review.collapsed_dirs.contains(&dir) {
                        review.collapsed_dirs.remove(&dir);
                    } else {
                        review.collapsed_dirs.insert(dir);
                    }
                    rerender_file_panel(review);
                }
            });
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts);
}

fn set_diff_panel_keymaps(buf: &mut nvim_oxi::api::Buffer, config: &config::Config) {
    let next_hunk = config.keymaps.next_hunk.clone();
    let prev_hunk = config.keymaps.prev_hunk.clone();
    let next_file = config.keymaps.next_file.clone();
    let prev_file = config.keymaps.prev_file.clone();
    let next_thread = config.keymaps.next_thread.clone();
    let prev_thread = config.keymaps.prev_thread.clone();
    let approve = config.keymaps.approve.clone();
    let needs_changes = config.keymaps.needs_changes.clone();
    let reset_status = config.keymaps.reset_status.clone();
    let comment = config.keymaps.comment.clone();
    let auto_resolve = config.keymaps.auto_resolve.clone();
    let open_thread = config.keymaps.open_thread.clone();
    let list_threads = config.keymaps.list_threads.clone();
    let list_threads_agent = config.keymaps.list_threads_agent.clone();
    let list_threads_user = config.keymaps.list_threads_user.clone();
    let list_threads_binned = config.keymaps.list_threads_binned.clone();
    let list_threads_open = config.keymaps.list_threads_open.clone();
    let resolve_thread = config.keymaps.resolve_thread.clone();
    let toggle_resolved = config.keymaps.toggle_resolved.clone();
    let re_anchor = config.keymaps.re_anchor.clone();
    let refresh = config.keymaps.refresh.clone();
    let toggle_sbs = config.keymaps.toggle_side_by_side.clone();
    let cancel_request = config.keymaps.cancel_request.clone();
    let next_unreviewed = config.keymaps.next_unreviewed.clone();
    let prev_unreviewed = config.keymaps.prev_unreviewed.clone();
    let accept_hunk = config.keymaps.accept_hunk.clone();

    let opts_cancel_request = SetKeymapOpts::builder()
        .callback(|_| {
            backend::cancel_all();
            let _ = api::notify(
                "[arbiter] cancelled pending requests",
                nvim_oxi::api::types::LogLevel::Info,
                &Dictionary::default(),
            );
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &cancel_request, "", &opts_cancel_request);

    let opts_toggle_sbs = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_toggle_sbs);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &toggle_sbs, "", &opts_toggle_sbs);

    let opts_next_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_hunk, "", &opts_next_hunk);

    let opts_prev_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_hunk, "", &opts_prev_hunk);

    let opts_next_file = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_file);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_file, "", &opts_next_file);

    let opts_prev_file = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_file);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_file, "", &opts_prev_file);

    let opts_cr = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_diff_cr);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_cr);

    let opts_approve = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_ga);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &approve, "", &opts_approve);

    let opts_needs = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_gx);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &needs_changes, "", &opts_needs);

    let opts_reset = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_gr);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &reset_status, "", &opts_reset);

    let opts_refresh = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                refresh_file(r);
                refresh_file_list(r);
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &refresh, "", &opts_refresh);

    let opts_comment = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| handle_immediate_comment(r, false));
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &comment, "", &opts_comment);

    let opts_auto_resolve = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| handle_immediate_comment(r, true));
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &auto_resolve, "", &opts_auto_resolve);

    let opts_open_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_open_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &open_thread, "", &opts_open_thread);

    let opts_list_threads = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_list_threads);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &list_threads, "", &opts_list_threads);

    let opts_list_threads_agent = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                handle_list_threads_filtered(
                    r,
                    threads::FilterOpts {
                        origin: Some(ThreadOrigin::Agent),
                        status: None,
                    },
                );
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(
        Mode::Normal,
        &list_threads_agent,
        "",
        &opts_list_threads_agent,
    );

    let opts_list_threads_user = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                handle_list_threads_filtered(
                    r,
                    threads::FilterOpts {
                        origin: Some(ThreadOrigin::User),
                        status: None,
                    },
                );
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(
        Mode::Normal,
        &list_threads_user,
        "",
        &opts_list_threads_user,
    );

    let opts_list_threads_binned = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                handle_list_threads_filtered(
                    r,
                    threads::FilterOpts {
                        origin: None,
                        status: Some(ThreadStatus::Binned),
                    },
                );
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(
        Mode::Normal,
        &list_threads_binned,
        "",
        &opts_list_threads_binned,
    );

    let opts_resolve_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_resolve_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &resolve_thread, "", &opts_resolve_thread);

    let opts_toggle_resolved = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_g_q);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &toggle_resolved, "", &opts_toggle_resolved);

    let opts_re_anchor = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_reanchor);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &re_anchor, "", &opts_re_anchor);

    let opts_next_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_thread, "", &opts_next_thread);

    let opts_prev_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_thread, "", &opts_prev_thread);

    let opts_list_threads_open = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                handle_list_threads_filtered(
                    r,
                    threads::FilterOpts {
                        origin: None,
                        status: Some(ThreadStatus::Open),
                    },
                );
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(
        Mode::Normal,
        &list_threads_open,
        "",
        &opts_list_threads_open,
    );

    let opts_next_unreviewed = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_next_unreviewed);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_unreviewed, "", &opts_next_unreviewed);

    let opts_prev_unreviewed = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_prev_unreviewed);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_unreviewed, "", &opts_prev_unreviewed);

    let opts_accept_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_accept_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &accept_hunk, "", &opts_accept_hunk);
}

fn nav_next_hunk(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);
    let next = review
        .current_hunks
        .iter()
        .find(|h| h.buf_start > line_0)
        .or_else(|| review.current_hunks.first());
    if let Some(hunk) = next {
        scroll_to_hunk(review, hunk.buf_start, hunk.buf_end);
    }
}

fn nav_prev_hunk(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);
    let prev = review
        .current_hunks
        .iter()
        .rfind(|h| h.buf_start < line_0)
        .or_else(|| review.current_hunks.last());
    if let Some(hunk) = prev {
        scroll_to_hunk(review, hunk.buf_start, hunk.buf_end);
    }
}

fn scroll_to_hunk(review: &mut Review, buf_start: usize, buf_end: usize) {
    let wid = review.diff_panel.win.handle();
    let start_1 = (buf_start + 1) as i64;
    let end_1 = (buf_end + 1) as i64;
    let _ = review.diff_panel.win.set_cursor(buf_end + 1, 0);
    diff::win_exec(wid, &format!("normal! {start_1}Gzt{end_1}G"));
}

fn accepted_for_file(review: &Review) -> HashSet<String> {
    review
        .current_file
        .as_ref()
        .and_then(|p| review.accepted_hunks.get(p))
        .cloned()
        .unwrap_or_default()
}

fn handle_accept_hunk(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);

    let Some(hunk) = review
        .current_hunks
        .iter()
        .find(|h| line_0 >= h.buf_start && line_0 <= h.buf_end)
    else {
        let _ = api::notify(
            "[arbiter] cursor is not inside a hunk",
            nvim_oxi::api::types::LogLevel::Warn,
            &Dictionary::default(),
        );
        return;
    };
    let hash = hunk.content_hash.clone();

    let file_set = accepted_for_file(review);
    if file_set.contains(&hash) {
        unmark_hunk_accepted(review, &hash);
        if let Some(path) = review.current_file.clone() {
            if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
                if *rs == ReviewStatus::Approved {
                    *rs = ReviewStatus::Unreviewed;
                    review.file_content_hash.remove(&path);
                }
            }
        }
    } else {
        mark_hunk_accepted(review, &hash);
        check_all_hunks_accepted(review);
    }
    save_accepted_hunks(review);
    save_file_statuses(review);
    rerender_file_panel(review);
}

fn mark_hunk_accepted(review: &mut Review, content_hash: &str) {
    if let Some(path) = review.current_file.clone() {
        review
            .accepted_hunks
            .entry(path)
            .or_default()
            .insert(content_hash.to_string());
    }

    let Some((buf_start, buf_end, new_start, new_count)) = review
        .current_hunks
        .iter()
        .find(|h| h.content_hash == content_hash)
        .map(|h| (h.buf_start, h.buf_end, h.new_start, h.new_count))
    else {
        return;
    };

    if let Some(path) = review.current_file.clone() {
        resolve_threads_in_range(review, &path, new_start, new_count);
    }

    let ns = api::create_namespace("arbiter-diff");
    for i in buf_start..=buf_end {
        let _ = review.diff_panel.buf.clear_namespace(ns, i..i + 1);
        let _ = review
            .diff_panel
            .buf
            .add_highlight(ns, "ArbiterHunkAccepted", i, 0..);
    }

    update_accepted_fold_state(review);

    let wid = review.diff_panel.win.handle();
    let start = (buf_start + 1) as i64;
    diff::win_exec(wid, &format!("{start}foldclose"));
}

fn unmark_hunk_accepted(review: &mut Review, content_hash: &str) {
    if let Some(path) = review.current_file.as_ref() {
        if let Some(set) = review.accepted_hunks.get_mut(path) {
            set.remove(content_hash);
            if set.is_empty() {
                review.accepted_hunks.remove(path);
            }
        }
    }

    if let Some(path) = review.current_file.clone() {
        select_file_impl(review, &path);
    }
}

fn resolve_threads_in_range(review: &mut Review, path: &str, new_start: usize, new_count: usize) {
    let end = new_start + new_count;
    let mut changed = false;
    for t in &mut review.threads {
        if t.file == path
            && t.status == ThreadStatus::Open
            && (t.line as usize) >= new_start
            && (t.line as usize) < end
        {
            threads::resolve(t);
            changed = true;
        }
    }
    if changed {
        save_threads(review);
    }
}

fn resolve_threads_for_file(review: &mut Review, path: &str) {
    let mut changed = false;
    for t in &mut review.threads {
        if t.file == path && t.status == ThreadStatus::Open {
            threads::resolve(t);
            changed = true;
        }
    }
    if changed {
        save_threads(review);
    }
}

fn save_threads(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    state::save_threads(&sd, &ws_hash, &review.ref_name, &review.threads);
}

fn check_all_hunks_accepted(review: &mut Review) {
    if review.current_hunks.is_empty() {
        return;
    }
    let file_set = accepted_for_file(review);
    let all_accepted = review
        .current_hunks
        .iter()
        .all(|h| file_set.contains(&h.content_hash));
    if !all_accepted {
        return;
    }
    let Some(path) = review.current_file.clone() else {
        return;
    };
    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
        if *rs != ReviewStatus::Approved {
            *rs = ReviewStatus::Approved;
            let full = Path::new(&review.cwd).join(&path);
            let contents = std::fs::read_to_string(&full).unwrap_or_default();
            review
                .file_content_hash
                .insert(path, state::content_hash(&contents));
        }
    }
}

fn save_file_statuses(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let mut rs = state::ReviewState::default();
    for (p, _, status) in &review.files {
        rs.files
            .insert(p.clone(), file_state_for(review, p, *status));
    }
    state::save_review(&sd, &ws_hash, &review.ref_name, &rs);
}

fn update_accepted_fold_state(review: &mut Review) {
    let file_accepted = accepted_for_file(review);
    let accepted_buf_starts: HashSet<usize> = review
        .current_hunks
        .iter()
        .filter(|h| file_accepted.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect();

    let mut dict = nvim_oxi::Dictionary::new();
    for bs in &accepted_buf_starts {
        let key = nvim_oxi::String::from(format!("{}", bs + 1));
        dict.insert(key, nvim_oxi::Object::from(true));
    }
    let _ = review
        .diff_panel
        .buf
        .set_var("arbiter_accepted_folds", dict);
}

fn file_state_for(review: &Review, path: &str, status: ReviewStatus) -> state::FileState {
    let ch = review
        .file_content_hash
        .get(path)
        .cloned()
        .unwrap_or_default();
    let ah = review
        .accepted_hunks
        .get(path)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    state::FileState {
        status,
        content_hash: ch,
        updated_at: now_secs(),
        accepted_hunks: ah,
    }
}

fn save_accepted_hunks(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let mut rs = state::load_review(&sd, &ws_hash, &review.ref_name);
    if let Some(path) = review.current_file.as_ref() {
        let file_accepted: Vec<String> = review
            .accepted_hunks
            .get(path)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let entry = rs
            .files
            .entry(path.clone())
            .or_insert_with(|| state::FileState {
                status: ReviewStatus::Unreviewed,
                content_hash: String::new(),
                updated_at: 0,
                accepted_hunks: Vec::new(),
            });
        entry.accepted_hunks = file_accepted;
    }
    state::save_review(&sd, &ws_hash, &review.ref_name, &rs);
}

fn nav_next_file(review: &mut Review) {
    let Some(ref path) = review.current_file else {
        return;
    };
    let idx = review.file_index.get(path).copied().unwrap_or(0);
    let next_idx = (idx + 1) % review.files.len().max(1);
    let path = review.files.get(next_idx).map(|(p, _, _)| p.clone());
    if let Some(path) = path {
        select_file_impl(review, &path);
    }
}

fn nav_prev_file(review: &mut Review) {
    let Some(ref path) = review.current_file else {
        return;
    };
    let idx = review.file_index.get(path).copied().unwrap_or(0);
    let prev_idx = if idx == 0 {
        review.files.len().saturating_sub(1)
    } else {
        idx - 1
    };
    let path = review.files.get(prev_idx).map(|(p, _, _)| p.clone());
    if let Some(path) = path {
        select_file_impl(review, &path);
    }
}

fn open_thread_count_map(threads: &[threads::Thread]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for t in threads {
        if t.status == ThreadStatus::Open {
            *counts.entry(t.file.clone()).or_default() += 1;
        }
    }
    counts
}

fn file_has_open_threads(threads: &[threads::Thread], path: &str) -> bool {
    threads
        .iter()
        .any(|t| t.file == path && t.status == ThreadStatus::Open)
}

fn is_unreviewed_file(review: &Review, idx: usize) -> bool {
    let Some((path, _, rs)) = review.files.get(idx) else {
        return false;
    };
    if *rs != ReviewStatus::Approved {
        return true;
    }
    file_has_open_threads(&review.threads, path)
}

fn handle_next_unreviewed(review: &mut Review) {
    if review.files.is_empty() {
        return;
    }
    let current_idx = review
        .current_file
        .as_ref()
        .and_then(|p| review.file_index.get(p).copied())
        .unwrap_or(0);
    let len = review.files.len();
    for offset in 1..=len {
        let idx = (current_idx + offset) % len;
        if is_unreviewed_file(review, idx) {
            if let Some((path, _, _)) = review.files.get(idx) {
                let path = path.clone();
                select_file_impl(review, &path);
            }
            return;
        }
    }
}

fn handle_prev_unreviewed(review: &mut Review) {
    if review.files.is_empty() {
        return;
    }
    let current_idx = review
        .current_file
        .as_ref()
        .and_then(|p| review.file_index.get(p).copied())
        .unwrap_or(0);
    let len = review.files.len();
    for offset in 1..=len {
        let idx = (current_idx + len - offset) % len;
        if is_unreviewed_file(review, idx) {
            if let Some((path, _, _)) = review.files.get(idx) {
                let path = path.clone();
                select_file_impl(review, &path);
            }
            return;
        }
    }
}

fn make_thread_reply_callback(
    thread_id: String,
    file: String,
    line: u32,
) -> threads::OnReplyRequested {
    Box::new(move || {
        let title = format!("Reply at {file}:{line}");
        let tid = thread_id.clone();
        let file_for_notify = file.clone();
        let _ = threads::open(
            &title,
            Box::new(move |text: String| {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return;
                }
                with_active(|r| {
                    if let Some(t) = r.threads.iter_mut().find(|x| x.id == tid) {
                        threads::add_message(t, Role::User, &text);
                        if threads::window_is_open() {
                            let _ = threads::append_message(Role::User, &text);
                        }
                        let session_id = t.session_id.clone();
                        let state_dir = r.config.state_dir();
                        let ws_hash = state::workspace_hash(Path::new(&r.cwd)).clone();
                        let ref_name = r.ref_name.clone();
                        let sd = state_dir.clone();
                        let ws = ws_hash.clone();
                        let rn = ref_name.clone();
                        let sd2 = state_dir.clone();
                        let ws2 = ws_hash.clone();
                        let rn2 = ref_name.clone();
                        let stream_tid = tid.clone();
                        let on_stream: crate::types::OnStream =
                            std::sync::Arc::new(move |chunk: &str| {
                                if threads::window_thread_id().as_deref() == Some(&stream_tid) {
                                    let _ = threads::append_streaming(chunk);
                                }
                            });
                        let prior_messages: Vec<(String, String)> = t
                            .messages
                            .iter()
                            .map(|m| {
                                let role = match m.role {
                                    Role::User => "user",
                                    Role::Agent => "agent",
                                };
                                (role.to_string(), m.text.clone())
                            })
                            .collect();
                        let prompt = prompts::format_reply_prompt(
                            &t.file,
                            t.line,
                            &text,
                            &t.anchor_content,
                            &t.anchor_context,
                            &prior_messages,
                        );
                        let file_notify = file_for_notify.clone();
                        let reply_tag = tid.clone();
                        backend::cancel_tagged(&tid);
                        backend::thread_reply(
                            session_id.as_deref(),
                            &prompt,
                            Some(on_stream),
                            Box::new(move |res| {
                                let msg = res
                                    .error
                                    .as_ref()
                                    .map(|e| format!("[Error] {e}"))
                                    .unwrap_or(res.text);
                                if let Some(ref e) = res.error {
                                    backend::notify_if_missing_binary(e);
                                    let _ = api::notify(
                                        &format!("Reply failed: {e}"),
                                        nvim_oxi::api::types::LogLevel::Warn,
                                        &Dictionary::default(),
                                    );
                                    if threads::window_thread_id().as_deref() == Some(&tid) {
                                        let _ = threads::replace_last_agent_message(&msg);
                                    }
                                }
                                with_active(|r| {
                                    if let Some(t) = r.threads.iter_mut().find(|x| x.id == tid) {
                                        threads::add_message(t, Role::Agent, &msg);
                                        if !res.session_id.is_empty() {
                                            t.session_id = Some(res.session_id);
                                        }
                                        state::save_threads(&sd, &ws, &rn, &r.threads);
                                        if let Some(ref p) = r.current_file {
                                            let p = p.clone();
                                            select_file_impl(r, &p);
                                        }
                                    }
                                });
                                if !threads::window_is_open() {
                                    let preview: String = msg.chars().take(60).collect();
                                    let _ = api::notify(
                                        &format!(
                                            "[arbiter] Agent replied on {file_notify}:{line}: {preview}"
                                        ),
                                        nvim_oxi::api::types::LogLevel::Info,
                                        &Dictionary::default(),
                                    );
                                }
                            }),
                            Some(reply_tag),
                        );
                        state::save_threads(&sd2, &ws2, &rn2, &r.threads);
                        if let Some(ref p) = r.current_file {
                            let p = p.clone();
                            select_file_impl(r, &p);
                        }
                    }
                });
            }),
            Box::new(|| {}),
        );
    })
}

fn open_thread_panel(review: &Review, t: &threads::Thread) {
    clear_thread_anchor();
    let on_reply = make_thread_reply_callback(t.id.clone(), t.file.clone(), t.line);
    let on_close: threads::OnClose = Arc::new(clear_thread_anchor);
    let _ = threads::window_open(
        &t.id,
        &t.file,
        t.line,
        &t.messages,
        on_reply,
        Some(on_close),
    );
    place_thread_anchor(review, t.line);
}

pub fn open_active_thread(review: &mut Review) {
    let Some(tid) = backend::inflight_tag() else {
        let _ = api::notify(
            "[arbiter] no active thread",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    };
    let needs_file_switch = review
        .threads
        .iter()
        .find(|t| t.id == tid)
        .map(|t| review.current_file.as_deref() != Some(&t.file))
        .unwrap_or(false);
    if needs_file_switch {
        if let Some(file) = review
            .threads
            .iter()
            .find(|t| t.id == tid)
            .map(|t| t.file.clone())
        {
            select_file_impl(review, &file);
        }
    }
    open_thread_by_id(review, &tid);
}

fn open_thread_by_id(review: &Review, tid: &str) -> bool {
    if let Some(t) = review.threads.iter().find(|x| x.id == tid) {
        open_thread_panel(review, t);
        return true;
    }
    false
}

fn place_thread_anchor(review: &Review, source_line: u32) {
    let ns = api::create_namespace("arbiter-thread-anchor");
    let mut buf = review.diff_panel.buf.clone();
    let line_count = buf.line_count().unwrap_or(0);
    let all_lines: Vec<String> = buf
        .get_lines(0..line_count, false)
        .map(|iter| iter.map(|s| s.to_string()).collect())
        .unwrap_or_default();

    if let Some(buf_line) =
        diff::source_to_buf_line(&review.current_hunks, source_line as usize, &all_lines)
    {
        let _ = buf.add_highlight(ns, "CursorLine", buf_line, 0..);
    }
}

fn clear_thread_anchor() {
    let ns = api::create_namespace("arbiter-thread-anchor");
    with_active(|r| {
        let mut buf = r.diff_panel.buf.clone();
        let _ = buf.clear_namespace(ns, ..);
    });
}

fn handle_diff_cr(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line_0 = row.saturating_sub(1);

    if let Some(tid) = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line_0)
        .map(|(t, _)| t.clone())
    {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(tid) = review.thread_inline_marks.get(&buf_line_0).cloned() {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(ref path) = review.current_file {
        let lc = review.diff_panel.buf.line_count().unwrap_or(1);
        let lines: Vec<String> = (0..lc)
            .map(|i| {
                review
                    .diff_panel
                    .buf
                    .get_lines(i..=i, false)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();
        let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        if let Some(loc) =
            diff::buf_line_to_source(&review.current_hunks, buf_line_0, &line_refs, path)
        {
            let full = Path::new(&review.cwd).join(&loc.file);
            if full.exists() {
                let _ = api::command(&format!("tabnew +{} {}", loc.line, full.display()));
            }
        }
    }
}

fn handle_open_thread(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line_0 = row.saturating_sub(1);

    if let Some(tid) = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line_0)
        .map(|(t, _)| t.clone())
    {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(tid) = review.thread_inline_marks.get(&buf_line_0).cloned() {
        open_thread_by_id(review, &tid);
        return;
    }

    let Some((path, src_line, _, _)) = get_source_loc(review) else {
        return;
    };
    if let Some(t) = review
        .threads
        .iter()
        .find(|t| t.file == path && t.line == src_line)
    {
        open_thread_panel(review, t);
    } else {
        api::err_writeln(&format!("[arbiter] no thread at {path}:{src_line}"));
    }
}

fn handle_toggle_sbs(review: &mut Review) {
    if review.side_by_side {
        if let Some(ref sbs) = review.sbs {
            let _ =
                diff::close_side_by_side(&sbs.old_buf, &sbs.old_win, &sbs.new_buf, &sbs.new_win);
        }
        review.sbs = None;
        review.side_by_side = false;
        return;
    }
    let Some(ref path) = review.current_file else {
        return;
    };
    let is_untracked = review
        .files
        .iter()
        .find(|(p, _, _)| p == path)
        .map(|(_, fs, _)| *fs == FileStatus::Untracked)
        .unwrap_or(false);
    let cwd = review.cwd.clone();
    let ref_name = review.ref_name.clone();
    let path = path.clone();
    if is_untracked {
        let full_path = Path::new(&cwd).join(&path);
        let working_content = std::fs::read_to_string(&full_path).unwrap_or_default();
        if let Ok((old_buf, old_win, new_buf, new_win)) =
            diff::open_side_by_side("", &working_content, &path)
        {
            review.sbs = Some(SideBySide {
                old_buf,
                new_buf,
                old_win,
                new_win,
            });
            review.side_by_side = true;
        }
        return;
    }
    let path_for_show = path.clone();
    let cwd_clone = cwd.clone();
    git::show(&cwd, &ref_name, &path_for_show, move |result| {
        let ref_content = if result.success() {
            result.stdout
        } else {
            String::new()
        };
        let full_path = Path::new(&cwd_clone).join(&path);
        let working_content = std::fs::read_to_string(&full_path).unwrap_or_default();
        if let Ok((old_buf, old_win, new_buf, new_win)) =
            diff::open_side_by_side(&ref_content, &working_content, &path)
        {
            with_active(|r| {
                r.sbs = Some(SideBySide {
                    old_buf,
                    new_buf,
                    old_win,
                    new_win,
                });
                r.side_by_side = true;
            });
        }
    });
}

fn handle_ga(review: &mut Review) {
    if try_resolve_thread_at_cursor(review) {
        return;
    }

    let Some(path) = review.current_file.clone() else {
        return;
    };
    let current_status = review
        .files
        .iter()
        .find(|(p, _, _)| *p == path)
        .map(|(_, _, rs)| *rs);
    let new_status = if current_status == Some(ReviewStatus::Approved) {
        ReviewStatus::Unreviewed
    } else {
        ReviewStatus::Approved
    };
    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
        *rs = new_status;
    }
    if new_status == ReviewStatus::Approved {
        let full = Path::new(&review.cwd).join(&path);
        let contents = std::fs::read_to_string(&full).unwrap_or_default();
        let hash = state::content_hash(&contents);
        review.file_content_hash.insert(path.clone(), hash);

        let all_hashes: HashSet<String> = review
            .current_hunks
            .iter()
            .map(|h| h.content_hash.clone())
            .collect();
        review.accepted_hunks.insert(path.clone(), all_hashes);

        resolve_threads_for_file(review, &path);
        select_file_impl(review, &review.current_file.clone().unwrap_or_default());
    } else {
        review.file_content_hash.remove(&path);
        review.accepted_hunks.remove(&path);

        select_file_impl(review, &path);
    }
    save_file_statuses(review);
    rerender_file_panel(review);
}

fn handle_gx(review: &mut Review) {
    let Some(path) = review.current_file.clone() else {
        return;
    };
    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
        *rs = ReviewStatus::NeedsChanges;
    }
    review.accepted_hunks.remove(&path);
    save_file_statuses(review);
    rerender_file_panel(review);
    select_file_impl(review, &path);
}

fn handle_gr(review: &mut Review) {
    let Some(path) = review.current_file.clone() else {
        return;
    };
    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
        *rs = ReviewStatus::Unreviewed;
    }
    review.file_content_hash.remove(&path);
    review.accepted_hunks.remove(&path);
    save_file_statuses(review);
    rerender_file_panel(review);
    select_file_impl(review, &path);
    if review.config.review.fold_approved {
        if let Some(h) = hunk_at_cursor(review) {
            let _ = api::set_current_win(&review.diff_panel.win);
            let _ = api::command(&format!("{}foldopen", h.buf_start + 1));
        }
    }
}

fn hunk_at_cursor(review: &Review) -> Option<&Hunk> {
    let (row, _) = review.diff_panel.win.get_cursor().into_result().ok()?;
    let line = row.saturating_sub(1);
    review
        .current_hunks
        .iter()
        .find(|h| line >= h.buf_start && line <= h.buf_end)
}

pub fn show_summary(review: &mut Review) {
    let approved = review
        .files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::Approved)
        .count();
    let needs = review
        .files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::NeedsChanges)
        .count();
    let unreviewed = review
        .files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::Unreviewed)
        .count();
    let open_threads = review
        .threads
        .iter()
        .filter(|t| t.status == ThreadStatus::Open)
        .count();
    let resolved = review
        .threads
        .iter()
        .filter(|t| t.status == ThreadStatus::Resolved)
        .count();
    let text = format!(
        "Files: {} total | ✓ {} approved | ✗ {} needs changes | · {} unreviewed\nThreads: {} open | {} resolved",
        review.files.len(),
        approved,
        needs,
        unreviewed,
        open_threads,
        resolved
    );
    let mut buf = match api::create_buf(false, true) {
        Ok(b) => b,
        Err(_) => return,
    };
    let refs: Vec<&str> = text.lines().collect();
    let _ = buf.set_lines(0..0, false, refs);
    let _ = api::set_option_value(
        "buftype",
        "nofile",
        &OptionOpts::builder().buffer(buf.clone()).build(),
    );
    let _ = api::set_option_value(
        "modifiable",
        false,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    );
    let cols = match api::get_option_value::<i64>("columns", &OptionOpts::builder().build()) {
        Ok(c) => c,
        Err(_) => return,
    };
    let rows = match api::get_option_value::<i64>("lines", &OptionOpts::builder().build()) {
        Ok(r) => r,
        Err(_) => return,
    };
    let width = (cols as u32).saturating_sub(6).clamp(10, 50);
    let height = (rows as u32).saturating_sub(6).clamp(2, 3);
    let row = ((rows as f64) - (height as f64)) / 2.0;
    let col = ((cols as f64) - (width as f64)) / 2.0;
    let config = nvim_oxi::api::types::WindowConfig::builder()
        .relative(nvim_oxi::api::types::WindowRelativeTo::Editor)
        .width(width)
        .height(height)
        .row(row)
        .col(col)
        .border(nvim_oxi::api::types::WindowBorder::Rounded)
        .title(nvim_oxi::api::types::WindowTitle::SimpleString(
            "Review Summary".to_string().into(),
        ))
        .build();
    let win = match api::open_win(&buf, true, &config) {
        Ok(w) => w,
        Err(_) => return,
    };
    SUMMARY_WIN.with(|c| *c.borrow_mut() = Some(win));
    let opts = SetKeymapOpts::builder()
        .callback(|_| close_summary_float())
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts);
    let _ = buf.set_keymap(Mode::Normal, "<Esc>", "", &opts);
}

fn try_resolve_thread_at_cursor(review: &mut Review) -> bool {
    let (row, _) = match review.diff_panel.win.get_cursor().into_result() {
        Ok(r) => r,
        Err(_) => return false,
    };
    let buf_line = row.saturating_sub(1);
    let tid = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line)
        .map(|(id, _)| id.clone());
    let Some(tid) = tid else { return false };
    let Some(t) = review.threads.iter_mut().find(|t| t.id == tid) else {
        return false;
    };
    if t.status == ThreadStatus::Open {
        threads::resolve(t);
    } else {
        t.status = ThreadStatus::Open;
    }
    save_threads(review);
    if let Some(path) = review.current_file.clone() {
        select_file_impl(review, &path);
    }
    true
}

fn get_thread_at_cursor(review: &Review) -> Option<&threads::Thread> {
    let (row, _) = review.diff_panel.win.get_cursor().into_result().ok()?;
    let buf_line = row.saturating_sub(1);
    let tid = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line)?
        .0;
    review.threads.iter().find(|t| t.id == *tid)
}

fn get_source_loc(review: &Review) -> Option<(String, u32, String, Vec<String>)> {
    let path = match review.current_file.as_ref() {
        Some(p) => p,
        None => {
            api::err_writeln("[arbiter] no file selected - select a file first");
            return None;
        }
    };
    let (row, _) = match review.diff_panel.win.get_cursor().into_result() {
        Ok(pos) => pos,
        Err(e) => {
            api::err_writeln(&format!("[arbiter] get_cursor failed: {e}"));
            return None;
        }
    };
    let buf_line = row.saturating_sub(1);
    let lc = match review.diff_panel.buf.line_count() {
        Ok(n) => n,
        Err(e) => {
            api::err_writeln(&format!("[arbiter] line_count failed: {e}"));
            return None;
        }
    };
    let lines: Vec<String> = match review.diff_panel.buf.get_lines(0..lc, false) {
        Ok(l) => l.into_iter().map(|s| s.to_string()).collect(),
        Err(e) => {
            api::err_writeln(&format!("[arbiter] get_lines failed: {e}"));
            return None;
        }
    };
    let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let loc = diff::buf_line_to_source(&review.current_hunks, buf_line, &line_refs, path);
    let loc = match loc {
        Some(l) => l,
        None => {
            let hunk_info = if review.current_hunks.is_empty() {
                "no hunks loaded".to_string()
            } else {
                review
                    .current_hunks
                    .iter()
                    .map(|h| format!("{}-{}", h.buf_start, h.buf_end))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            api::err_writeln(&format!(
                "[arbiter] buf_line={buf_line} not in any hunk (hunks: {hunk_info})"
            ));
            return None;
        }
    };
    let full = Path::new(&review.cwd).join(&loc.file);
    let contents = std::fs::read_to_string(&full).unwrap_or_default();
    let file_lines: Vec<&str> = contents.lines().collect();
    let line_num = loc.line;
    let anchor_content = file_lines
        .get(line_num.saturating_sub(1))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let ctx_start = line_num.saturating_sub(2).saturating_sub(1);
    let ctx_end = (line_num + 2).min(file_lines.len().max(1));
    let context: Vec<String> = file_lines
        .get(ctx_start..ctx_end)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.to_string())
        .collect();
    Some((path.clone(), line_num as u32, anchor_content, context))
}

fn handle_immediate_comment(review: &mut Review, auto_resolve: bool) {
    let Some((path, line, anchor_content, context)) = get_source_loc(review) else {
        return;
    };
    let path = path.clone();
    let path_for_open = path.clone();
    let anchor_content = anchor_content.clone();
    let context = context.clone();
    if let Err(e) = threads::open_for_line(
        &path_for_open,
        line,
        Box::new(move |text: String| {
            if text.trim().is_empty() {
                return;
            }
            let trimmed = text.trim().to_string();

            let thread = with_active(|r| {
                let thread = threads::create(
                    &path,
                    line,
                    &trimmed,
                    threads::CreateOpts {
                        pending: false,
                        immediate: true,
                        auto_resolve,
                        origin: ThreadOrigin::User,
                        anchor_content: anchor_content.clone(),
                        anchor_context: context.clone(),
                    },
                );
                r.threads.push(thread.clone());
                let state_dir = r.config.state_dir();
                let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                if let Some(ref p) = r.current_file {
                    let p = p.clone();
                    select_file_impl(r, &p);
                }
                thread
            });

            let Some(thread) = thread else { return };
            let tid = thread.id.clone();
            let on_reply = make_thread_reply_callback(tid.clone(), path.clone(), line);
            let on_close: threads::OnClose = Arc::new(clear_thread_anchor);
            let _ = threads::window_open(
                &thread.id,
                &thread.file,
                thread.line,
                &thread.messages,
                on_reply,
                Some(on_close),
            );
            with_active(|r| place_thread_anchor(r, thread.line));

            let prompt =
                prompts::format_comment_prompt(&path, line, &trimmed, &anchor_content, &context);
            let stream_tid = tid.clone();
            let on_stream: crate::types::OnStream = std::sync::Arc::new(move |chunk: &str| {
                if threads::window_thread_id().as_deref() == Some(&stream_tid) {
                    let _ = threads::append_streaming(chunk);
                }
            });
            let file_notify = path.clone();
            let comment_tag = tid.clone();
            let window_tid = tid.clone();
            backend::send_comment(
                &prompt,
                Some(on_stream),
                Box::new(move |res| {
                    let msg = res
                        .error
                        .as_ref()
                        .map(|e| format!("[Error] {e}"))
                        .unwrap_or(res.text);
                    if res.error.is_some() {
                        backend::notify_if_missing_binary(&msg);
                        let _ = api::notify(
                            &format!("Comment failed: {msg}"),
                            nvim_oxi::api::types::LogLevel::Warn,
                            &Dictionary::default(),
                        );
                        if threads::window_thread_id().as_deref() == Some(&window_tid) {
                            let _ = threads::replace_last_agent_message(&msg);
                        }
                    }
                    with_active(|r| {
                        if let Some(t) = r.threads.iter_mut().find(|t| t.id == tid) {
                            t.session_id = Some(res.session_id.clone()).filter(|s| !s.is_empty());
                            threads::add_message(t, Role::Agent, &msg);
                        }
                        let state_dir = r.config.state_dir();
                        let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                        state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                        if let Some(ref p) = r.current_file {
                            let p = p.clone();
                            select_file_impl(r, &p);
                        }
                    });
                    if !threads::window_is_open() && res.error.is_none() {
                        let preview: String = msg.chars().take(60).collect();
                        let _ = api::notify(
                            &format!("[arbiter] Agent replied on {file_notify}:{line}: {preview}"),
                            nvim_oxi::api::types::LogLevel::Info,
                            &Dictionary::default(),
                        );
                    }
                }),
                Some(comment_tag),
            );
        }),
        Box::new(|| {}),
    ) {
        api::err_writeln(&format!("[arbiter] open comment float failed: {e}"));
    }
}

fn lua_quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\0', "");
    format!("\"{escaped}\"")
}

fn handle_list_threads(review: &mut Review) {
    handle_list_threads_filtered(review, threads::FilterOpts::default());
}

fn handle_list_threads_filtered(review: &mut Review, opts: threads::FilterOpts) {
    let mut filtered = threads::filter(&review.threads, &opts);
    if filtered.is_empty() {
        api::err_writeln("[arbiter] no threads match filter");
        return;
    }
    filtered.sort_by(|a, b| {
        let ts_a = a.messages.last().map(|m| m.ts).unwrap_or(0);
        let ts_b = b.messages.last().map(|m| m.ts).unwrap_or(0);
        ts_b.cmp(&ts_a)
    });
    let date_fmt = &config::get().thread_window.date_format;
    let cwd = &review.cwd;
    let mut lua_items = String::from("{");
    for (i, t) in filtered.iter().enumerate() {
        if i > 0 {
            lua_items.push(',');
        }
        let abs_path = format!("{}/{}", cwd, t.file);
        let preview = t
            .messages
            .first()
            .map(|m| m.text.chars().take(60).collect::<String>())
            .unwrap_or_default();
        let ts_display = t
            .messages
            .last()
            .and_then(|m| chrono::DateTime::from_timestamp(m.ts, 0))
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format(date_fmt)
                    .to_string()
            })
            .unwrap_or_default();
        let status_icon = match t.status {
            ThreadStatus::Open => "●",
            ThreadStatus::Resolved => "✓",
            ThreadStatus::Binned => "✗",
        };
        let label = format!("{status_icon} {ts_display}  {preview}");
        lua_items.push_str(&format!(
            "{{filename={},lnum={},col=1,text={}}}",
            lua_quote(&abs_path),
            t.line,
            lua_quote(&label),
        ));
    }
    lua_items.push('}');
    let lua = format!(
        concat!(
            "vim.fn.setqflist({{}},\" \",{{title=\"Threads\",items={}}}) ",
            "vim.cmd(\"copen\") ",
            "vim.api.nvim_buf_set_keymap(0,\"n\",\"dd\",\"\",{{",
            "callback=function() ",
            "local i=vim.fn.line(\".\") ",
            "local q=vim.fn.getqflist() ",
            "table.remove(q,i) ",
            "vim.fn.setqflist(q,\"r\") ",
            "end,noremap=true,silent=true}}) ",
            "vim.api.nvim_buf_set_keymap(0,\"n\",\"<CR>\",\"\",{{",
            "callback=function() ",
            "local e=vim.fn.getqflist()[vim.fn.line(\".\")] ",
            "if e then ",
            "vim.cmd(\"ArbiterOpenThread \"..vim.fn.bufname(e.bufnr)..\" \"..e.lnum) ",
            "end ",
            "end,noremap=true,silent=true}}) ",
            "vim.api.nvim_buf_set_keymap(0,\"n\",\"gf\",\"\",{{",
            "callback=function() ",
            "local e=vim.fn.getqflist()[vim.fn.line(\".\")] ",
            "if e then ",
            "vim.cmd(\"edit +\"..e.lnum..\" \"..vim.fn.bufname(e.bufnr)) ",
            "end ",
            "end,noremap=true,silent=true}})",
        ),
        lua_items
    );
    let _ = api::command(&format!("lua {lua}"));
}

fn handle_resolve_thread(review: &mut Review) {
    let Some(t) = get_thread_at_cursor(review) else {
        return;
    };
    let tid = t.id.clone();
    if let Some(t) = review.threads.iter_mut().find(|x| x.id == tid) {
        threads::resolve(t);
        if threads::window_thread_id().as_deref() == Some(tid.as_str()) {
            threads::window_close();
        }
        let state_dir = review.config.state_dir();
        let ws_hash = state::workspace_hash(Path::new(&review.cwd));
        state::save_threads(&state_dir, &ws_hash, &review.ref_name, &review.threads);
        if let Some(ref p) = review.current_file {
            let p = p.clone();
            select_file_impl(review, &p);
        }
    }
}

fn handle_g_q(review: &mut Review) {
    review.show_resolved = !review.show_resolved;
    if let Some(ref path) = review.current_file.clone() {
        select_file_impl(review, path);
    }
}

fn handle_reanchor(review: &mut Review) {
    let Some(t) = get_thread_at_cursor(review) else {
        return;
    };
    let sid = match t.session_id.as_deref() {
        Some(s) => s.to_string(),
        None => return,
    };
    let Some((path, line, anchor_content, context)) = get_source_loc(review) else {
        return;
    };
    let prompt = prompts::format_comment_prompt(&path, line, "", &anchor_content, &context);
    let sid2 = sid.clone();
    backend::re_anchor(
        &sid,
        &prompt,
        Box::new(move |res| {
            with_active(|r| {
                if let Some(t) = r
                    .threads
                    .iter_mut()
                    .find(|x| x.session_id.as_deref() == Some(sid2.as_str()))
                {
                    threads::add_message(t, Role::Agent, &res.text);
                    let state_dir = r.config.state_dir();
                    let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                    state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                    if let Some(ref p) = r.current_file {
                        let p = p.clone();
                        select_file_impl(r, &p);
                    }
                }
            });
        }),
    );
}

fn nav_next_thread(review: &mut Review) {
    nav_thread_directed(review, true);
}

fn nav_prev_thread(review: &mut Review) {
    nav_thread_directed(review, false);
}

fn nav_thread_directed(review: &mut Review, forward: bool) {
    let file_order: Vec<String> = review.files.iter().map(|(p, _, _)| p.clone()).collect();
    let all_sorted = threads::sorted_global(&review.threads, &file_order);
    let sorted: Vec<usize> = all_sorted
        .into_iter()
        .filter(|&i| {
            review
                .threads
                .get(i)
                .is_some_and(|t| t.status == ThreadStatus::Open)
        })
        .collect();

    if sorted.is_empty() {
        let _ = api::notify(
            "[arbiter] no open threads",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    }

    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line = row.saturating_sub(1);
    let current = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line)
        .and_then(|(tid, _)| review.threads.iter().position(|t| t.id == *tid));

    let target_idx = if forward {
        threads::next_thread(&sorted, current)
    } else {
        threads::prev_thread(&sorted, current)
    };

    if let Some(idx) = target_idx {
        if let Some(t) = review.threads.get(idx) {
            let target_file = t.file.clone();
            let target_id = t.id.clone();
            if target_file != review.current_file.as_deref().unwrap_or_default() {
                select_file_impl(review, &target_file);
            }
            if let Some(&target_line) = review.thread_buf_lines.get(&target_id) {
                let _ = review.diff_panel.win.set_cursor(target_line + 1, 0);
            }
        }
    }
}

/// Closes the workbench, persists state, cancels backend, stops timers.
pub fn close() {
    poll::stop();
    backend::cancel_all();
    threads::window_close();

    let review = ACTIVE.with(|cell| cell.try_borrow_mut().ok().and_then(|mut opt| opt.take()));

    let Some(review) = review else { return };

    let state_dir = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let ref_name = review.ref_name.clone();

    let mut review_state = state::ReviewState::default();
    for (p, _, rs) in &review.files {
        review_state
            .files
            .insert(p.clone(), file_state_for(&review, p, *rs));
    }
    state::save_review(&state_dir, &ws_hash, &ref_name, &review_state);
    state::save_threads(&state_dir, &ws_hash, &ref_name, &review.threads);

    if let Some(ref sbs) = review.sbs {
        let _ = diff::close_side_by_side(&sbs.old_buf, &sbs.old_win, &sbs.new_buf, &sbs.new_win);
    }

    let diff_nr = review.diff_panel.buf.handle();
    let file_nr = review.file_panel.buf.handle();

    let _ = api::set_current_tabpage(&review.tabpage);
    let _ = api::command("tabclose");
    let _ = api::command(&format!("silent! bwipeout! {diff_nr} {file_nr}"));
}

fn place_thread_signs(review: &Review) {
    let buf_nr = review.diff_panel.buf.handle();
    let _ = api::command(&format!(
        "sign unplace * group=ArbiterThread buffer={buf_nr}"
    ));
    let mut sign_id = 1;
    for (tid, &buf_line) in &review.thread_buf_lines {
        let sign_name = review
            .threads
            .iter()
            .find(|t| t.id == *tid)
            .map(|t| match t.status {
                ThreadStatus::Resolved => "ArbiterThreadResolved",
                _ => "ArbiterThreadOpen",
            });
        if let Some(name) = sign_name {
            let _ = api::command(&format!(
                "sign place {sign_id} line={buf_line} name={name} group=ArbiterThread buffer={buf_nr}"
            ));
            sign_id += 1;
        }
    }
}

fn place_inline_thread_indicators(review: &mut Review) {
    let ns = api::create_namespace("arbiter-thread-inline");
    let _ = review.diff_panel.buf.clear_namespace(ns, 0..usize::MAX);
    review.thread_inline_marks.clear();

    let Some(ref current_file) = review.current_file else {
        return;
    };

    let file_threads: Vec<_> = review
        .threads
        .iter()
        .filter(|t| t.file == *current_file)
        .filter(|t| review.show_resolved || t.status != ThreadStatus::Resolved)
        .collect();

    if file_threads.is_empty() {
        return;
    }

    let lc = review.diff_panel.buf.line_count().unwrap_or(0);
    let lines: Vec<String> = (0..lc)
        .map(|i| {
            review
                .diff_panel
                .buf
                .get_lines(i..=i, false)
                .ok()
                .and_then(|v| v.into_iter().next())
                .map(|s| s.to_string())
                .unwrap_or_default()
        })
        .collect();
    let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();

    for t in &file_threads {
        let Some(buf_line) =
            diff::source_to_buf_line(&review.current_hunks, t.line as usize, &line_refs)
        else {
            continue;
        };

        review.thread_inline_marks.insert(buf_line, t.id.clone());

        let msg_count = t.messages.len();
        let status_icon = match t.status {
            ThreadStatus::Resolved => "✓",
            _ => "●",
        };
        let preview: String = t
            .messages
            .first()
            .map(|m| m.text.chars().take(30).collect())
            .unwrap_or_default();
        let label = format!(" {status_icon} [{msg_count}] {preview}");
        let hl = match t.status {
            ThreadStatus::Resolved => "ArbiterThreadResolved",
            _ => "ArbiterIndicatorAgent",
        };

        let opts = nvim_oxi::api::opts::SetExtmarkOpts::builder()
            .virt_text([(label.as_str(), hl)])
            .virt_text_pos(nvim_oxi::api::types::ExtmarkVirtTextPosition::Eol)
            .build();
        let _ = review.diff_panel.buf.set_extmark(ns, buf_line, 0, &opts);
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Refreshes the diff panel for the current file.
///
/// On file mtime change: git::diff, re-render, detect_hunk_changes,
/// ArbiterHunkNew extmarks, reset approved to Unreviewed, reanchor,
/// bin unmatched, check_auto_resolve_timeouts, preserve cursor/scroll.
pub fn refresh_file(review: &mut Review) {
    let Some(ref path) = review.current_file else {
        return;
    };

    let (row, col) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let saved_row = row;
    let saved_col = col;

    let is_untracked = review
        .files
        .iter()
        .find(|(p, _, _)| p == path)
        .map(|(_, fs, _)| *fs == FileStatus::Untracked)
        .unwrap_or(false);

    let cwd = review.cwd.clone();
    let ref_name = review.ref_name.clone();
    let path = path.clone();
    let config = review.config.clone();
    let auto_resolve_timeout = config.review.auto_resolve_timeout;

    if is_untracked {
        let full_path = Path::new(&cwd).join(&path);
        let contents = std::fs::read_to_string(&full_path).unwrap_or_default();
        let diff_text = diff::synthesize_untracked(&contents, &path);
        refresh_file_with_diff(
            review,
            &path,
            &diff_text,
            saved_row,
            saved_col,
            auto_resolve_timeout,
        );
        return;
    }

    let path_for_cb = path.clone();
    let path_for_git = path_for_cb.clone();
    git::diff(&cwd, &ref_name, &path_for_git, move |result| {
        let diff_text = if result.success() {
            result.stdout
        } else {
            String::new()
        };
        with_active(|r| {
            refresh_file_with_diff(
                r,
                &path_for_cb,
                &diff_text,
                saved_row,
                saved_col,
                auto_resolve_timeout,
            );
        });
    });
}

fn refresh_file_with_diff(
    review: &mut Review,
    path: &str,
    diff_text: &str,
    saved_row: usize,
    saved_col: usize,
    auto_resolve_timeout: u64,
) {
    let old_hashes: std::collections::HashSet<String> = review
        .current_hunks
        .iter()
        .map(|h| h.content_hash.clone())
        .collect();

    let new_hunks = diff::parse_hunks(diff_text);
    let new_hashes: std::collections::HashSet<String> =
        new_hunks.iter().map(|h| h.content_hash.clone()).collect();
    let changed_raw = diff::detect_hunk_changes(&old_hashes, &new_hunks);

    let stale_hashes: Vec<String> = old_hashes
        .iter()
        .filter(|h| !new_hashes.contains(*h))
        .cloned()
        .collect();
    if !stale_hashes.is_empty() {
        if let Some(set) = review.accepted_hunks.get_mut(path) {
            for h in &stale_hashes {
                set.remove(h);
            }
            if set.is_empty() {
                review.accepted_hunks.remove(path);
            }
        }
    }

    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| p == path) {
        if *rs == ReviewStatus::Approved {
            let full = Path::new(&review.cwd).join(path);
            let contents = std::fs::read_to_string(&full).unwrap_or_default();
            let new_hash = state::content_hash(&contents);
            if review.file_content_hash.get(path) != Some(&new_hash) {
                *rs = ReviewStatus::Unreviewed;
                review.file_content_hash.remove(path);
            }
        }
    }

    let full_path = Path::new(&review.cwd).join(path);
    let contents = std::fs::read_to_string(&full_path).unwrap_or_default();
    let unmatched = threads::reanchor_by_content(&mut review.threads, path, &contents);
    for i in unmatched.into_iter().rev() {
        if let Some(t) = review.threads.get_mut(i) {
            threads::bin(t);
        }
    }

    let now = now_secs();
    let _timed_out =
        threads::check_auto_resolve_timeouts(&mut review.threads, auto_resolve_timeout, now);

    let file_threads: Vec<threads::Thread> = threads::for_file(&review.threads, path)
        .into_iter()
        .cloned()
        .collect();
    let summaries = threads::to_summaries(&file_threads);
    let visible_count = summaries
        .iter()
        .filter(|s| review.show_resolved || s.status != ThreadStatus::Resolved)
        .count();
    let offset = 1 + visible_count;
    let new_hunk_buf_starts: std::collections::HashSet<usize> =
        changed_raw.iter().map(|&raw| raw + offset).collect();

    review.current_diff_text = diff_text.to_string();

    let file_accepted = review.accepted_hunks.get(path).cloned().unwrap_or_default();
    if let Ok((hunks, thread_buf_lines)) = diff::render(
        &mut review.diff_panel.buf,
        diff_text,
        &summaries,
        path,
        review.show_resolved,
        Some(&new_hunk_buf_starts),
        &file_accepted,
    ) {
        review.current_hunks = hunks.clone();
        review.thread_buf_lines = thread_buf_lines;
        let file_approved = review
            .files
            .iter()
            .find(|(p, _, _)| *p == path)
            .map(|(_, _, rs)| *rs == ReviewStatus::Approved)
            .unwrap_or(false);
        let accepted_buf_starts: HashSet<usize> = hunks
            .iter()
            .filter(|h| file_accepted.contains(&h.content_hash))
            .map(|h| h.buf_start)
            .collect();
        if !hunks.is_empty() {
            let _ = diff::set_hunk_folds(
                &mut review.diff_panel.buf,
                &review.diff_panel.win,
                &hunks,
                review.config.review.fold_approved && file_approved,
                &accepted_buf_starts,
            );
        }
    }
    place_thread_signs(review);
    place_inline_thread_indicators(review);

    let state_dir = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let mut review_state = state::ReviewState::default();
    for (p, _, rs) in &review.files {
        review_state
            .files
            .insert(p.clone(), file_state_for(review, p, *rs));
    }
    state::save_review(&state_dir, &ws_hash, &review.ref_name, &review_state);
    state::save_threads(&state_dir, &ws_hash, &review.ref_name, &review.threads);

    let _ = review.diff_panel.win.set_cursor(saved_row, saved_col);
    update_diff_winbar(review, path);
    rerender_file_panel(review);
}

/// Refreshes the file list.
///
/// On file list change: git::diff_names + untracked, rebuild files,
/// reset approved when content changed, refresh file panel.
pub fn refresh_file_list(review: &mut Review) {
    let cwd = review.cwd.clone();
    let ref_name = review.ref_name.clone();
    let state_dir = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&cwd));
    let review_state = state::load_review(&state_dir, &ws_hash, &ref_name);

    git::diff_names(&cwd, &ref_name, {
        let cwd = cwd.clone();
        let ref_name = ref_name.clone();
        let state_dir = state_dir.clone();
        let ws_hash = ws_hash.clone();
        let review_state = review_state.clone();
        move |result| {
            if !result.success() {
                let _ = api::notify(
                    &format!("git diff failed: {}", result.stderr),
                    nvim_oxi::api::types::LogLevel::Warn,
                    &Dictionary::default(),
                );
                return;
            }

            let mut files = parse_diff_names(&result.stdout, &review_state);
            let paths_seen: std::collections::HashSet<String> =
                files.iter().map(|(p, _, _)| p.clone()).collect();

            git::untracked(&cwd, {
                let state_dir = state_dir.clone();
                let ws_hash = ws_hash.clone();
                let ref_name = ref_name.clone();
                move |untracked_result| {
                    if untracked_result.success() {
                        for line in untracked_result.stdout.lines() {
                            let p = line.trim();
                            if !p.is_empty() && !paths_seen.contains(p) {
                                let rs = review_state
                                    .files
                                    .get(p)
                                    .map(|f| f.status)
                                    .unwrap_or(ReviewStatus::Unreviewed);
                                files.push((p.to_string(), FileStatus::Untracked, rs));
                            }
                        }
                    }

                    let mut file_index: HashMap<String, usize> = HashMap::new();
                    for (i, (path, _, _)) in files.iter().enumerate() {
                        file_index.insert(path.clone(), i);
                    }

                    with_active(|r| {
                        r.files = files.clone();
                        r.file_index = file_index;
                        let mut to_remove: Vec<String> = Vec::new();
                        for (path, _, rs) in r.files.iter_mut() {
                            if *rs == ReviewStatus::Approved {
                                let p = path.as_str();
                                let full = Path::new(&r.cwd).join(p);
                                if let Ok(contents) = std::fs::read_to_string(&full) {
                                    let new_hash = state::content_hash(&contents);
                                    if r.file_content_hash.get(p) != Some(&new_hash) {
                                        *rs = ReviewStatus::Unreviewed;
                                        to_remove.push(path.clone());
                                    }
                                }
                            }
                        }
                        for p in to_remove {
                            r.file_content_hash.remove(&p);
                        }
                        rerender_file_panel(r);
                        let mut review_state = state::ReviewState::default();
                        for (p, _, rs) in &r.files {
                            review_state
                                .files
                                .insert(p.clone(), file_state_for(r, p, *rs));
                        }
                        state::save_review(&state_dir, &ws_hash, &ref_name, &review_state);
                    });
                }
            });
        }
    });
}

/// Sends all pending threads via `backend::send_comment` (one per thread).
///
/// Each gets its own session. On result: set session_id, add agent message,
/// mark not pending, persist, re-render.
/// Switches the diff panel to a different file.
///
pub(crate) fn rerender_file_panel(review: &mut Review) {
    let open_thread_counts = open_thread_count_map(&review.threads);
    if let Ok(result) = file_panel::render(
        &mut review.file_panel.buf,
        &review.files,
        &review.collapsed_dirs,
        &open_thread_counts,
    ) {
        review.file_panel_line_to_path = result.line_to_path;
        review.file_panel_line_to_dir = result.line_to_dir;
    }
}

fn update_diff_winbar(review: &Review, path: &str) {
    let file_threads = threads::for_file(&review.threads, path);
    let open = file_threads
        .iter()
        .filter(|t| t.status == ThreadStatus::Open)
        .count();
    let resolved = file_threads
        .iter()
        .filter(|t| t.status == ThreadStatus::Resolved)
        .count();
    let title = if open > 0 || resolved > 0 {
        format!(" {path}  ({open} open, {resolved} resolved)")
    } else {
        format!(" {path}")
    };
    let win_opts = OptionOpts::builder()
        .win(review.diff_panel.win.clone())
        .build();
    let _ = api::set_option_value("winbar", title.as_str(), &win_opts);
}

/// Calls `git::diff` (or synthesizes for untracked), then `diff::render`
/// with `threads::to_summaries`.
pub(crate) fn select_file_impl(review: &mut Review, path: &str) {
    review.current_file = Some(path.to_string());
    update_diff_winbar(review, path);
    poll::set_target(Some(path));

    let is_untracked = review
        .files
        .iter()
        .find(|(p, _, _)| p == path)
        .map(|(_, fs, _)| *fs == FileStatus::Untracked)
        .unwrap_or(false);

    if is_untracked {
        let full_path = Path::new(&review.cwd).join(path);
        let contents = std::fs::read_to_string(&full_path).unwrap_or_default();
        let diff_text = diff::synthesize_untracked(&contents, path);
        review.current_diff_text = diff_text.clone();
        let file_threads: Vec<threads::Thread> = threads::for_file(&review.threads, path)
            .into_iter()
            .cloned()
            .collect();
        let summaries = threads::to_summaries(&file_threads);
        let file_accepted = review.accepted_hunks.get(path).cloned().unwrap_or_default();
        if let Ok((hunks, thread_buf_lines)) = diff::render(
            &mut review.diff_panel.buf,
            &diff_text,
            &summaries,
            path,
            review.show_resolved,
            None,
            &file_accepted,
        ) {
            review.current_hunks = hunks.clone();
            review.thread_buf_lines = thread_buf_lines;
            let file_approved = review
                .files
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, _, rs)| *rs == ReviewStatus::Approved)
                .unwrap_or(false);
            let accepted_buf_starts: HashSet<usize> = hunks
                .iter()
                .filter(|h| file_accepted.contains(&h.content_hash))
                .map(|h| h.buf_start)
                .collect();
            if !hunks.is_empty() {
                let _ = diff::set_hunk_folds(
                    &mut review.diff_panel.buf,
                    &review.diff_panel.win,
                    &hunks,
                    review.config.review.fold_approved && file_approved,
                    &accepted_buf_starts,
                );
            }
        }
        place_thread_signs(review);
        place_inline_thread_indicators(review);
        poll::set_target(Some(path));
        if let Some(tid) = review.pending_thread_open.take() {
            scroll_to_thread_and_open(review, &tid);
        }
        return;
    }

    let cwd = review.cwd.clone();
    let ref_name = review.ref_name.clone();
    let path = path.to_string();
    let path_clone = path.clone();
    let show_resolved = review.show_resolved;
    let mut diff_buf = review.diff_panel.buf.clone();
    let file_accepted: HashSet<String> = review
        .accepted_hunks
        .get(&path)
        .cloned()
        .unwrap_or_default();
    let threads_for_file: Vec<threads::Thread> = threads::for_file(&review.threads, &path)
        .into_iter()
        .cloned()
        .collect();

    git::diff(&cwd, &ref_name, &path, move |result| {
        let diff_text = if result.success() {
            result.stdout
        } else {
            String::new()
        };
        let summaries = threads::to_summaries(&threads_for_file);
        if let Ok((hunks, thread_buf_lines)) = diff::render(
            &mut diff_buf,
            &diff_text,
            &summaries,
            &path_clone,
            show_resolved,
            None,
            &file_accepted,
        ) {
            let diff_text_clone = diff_text.clone();
            with_active(|r| {
                if !r.diff_panel.win.is_valid() {
                    return;
                }
                r.current_hunks = hunks.clone();
                r.current_diff_text = diff_text_clone;
                r.thread_buf_lines = thread_buf_lines;
                poll::set_target(Some(&path_clone));
                let file_approved = r
                    .files
                    .iter()
                    .find(|(p, _, _)| *p == path_clone)
                    .map(|(_, _, rs)| *rs == ReviewStatus::Approved)
                    .unwrap_or(false);
                let accepted_buf_starts: HashSet<usize> = hunks
                    .iter()
                    .filter(|h| file_accepted.contains(&h.content_hash))
                    .map(|h| h.buf_start)
                    .collect();
                if !hunks.is_empty() {
                    let _ = diff::set_hunk_folds(
                        &mut r.diff_panel.buf,
                        &r.diff_panel.win,
                        &hunks,
                        r.config.review.fold_approved && file_approved,
                        &accepted_buf_starts,
                    );
                }
                update_diff_winbar(r, &path_clone);
                place_thread_signs(r);
                place_inline_thread_indicators(r);
                if let Some(tid) = r.pending_thread_open.take() {
                    scroll_to_thread_and_open(r, &tid);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::FileState;

    #[test]
    fn parse_diff_names_mad() {
        let state = state::ReviewState::default();
        let out = parse_diff_names("M\tfoo.rs\nA\tbar.rs\nD\tbaz.rs\n", &state);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].0, "foo.rs");
        assert_eq!(out[0].1, FileStatus::Modified);
        assert_eq!(out[1].0, "bar.rs");
        assert_eq!(out[1].1, FileStatus::Added);
        assert_eq!(out[2].0, "baz.rs");
        assert_eq!(out[2].1, FileStatus::Deleted);
    }

    #[test]
    fn parse_diff_names_merges_review_state() {
        let mut state = state::ReviewState::default();
        state.files.insert(
            "a.rs".to_string(),
            FileState {
                status: ReviewStatus::Approved,
                content_hash: String::new(),
                updated_at: 0,
                accepted_hunks: Vec::new(),
            },
        );
        let out = parse_diff_names("M\ta.rs\n", &state);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, ReviewStatus::Approved);
    }

    #[test]
    fn parse_diff_names_skips_invalid_lines() {
        let state = state::ReviewState::default();
        let out = parse_diff_names("no-tab\n\nM\tok.rs\n", &state);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "ok.rs");
    }
}
