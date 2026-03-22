//! Review lifecycle and workbench management.
//!
//! Creates the tabpage, file panel, diff panel. Manages agent mode state.
//! Holds `Review` in a thread-local `RefCell<Option<Review>>`.

mod acceptance;
mod keymaps;
mod navigation;
mod revision_view;
mod thread_ui;

use acceptance::*;
use keymaps::*;
use navigation::*;
use revision_view::*;
use thread_ui::*;

pub(crate) use acceptance::save_file_statuses_pub;
pub(crate) use thread_ui::open_active_thread;

use crate::backend;
use crate::config;
use crate::config::FilePanelKind;
use crate::diff::{self, Hunk};
use crate::file_panel::{BuiltinFilePanel, FilePanel, NvimTreeFilePanel};
use crate::git;
use crate::poll;
use crate::prompts;
use crate::revision;
use crate::rules;
use crate::state;
use crate::threads;
use crate::types::Role;
use crate::types::ThreadOrigin;
use crate::types::ThreadStatus;
use crate::types::{FileStatus, ReviewStatus, ThreadSummary};
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
    static THREAD_LIST_WIN: RefCell<Option<nvim_oxi::api::Window>> = const { RefCell::new(None) };
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
pub(crate) struct Panel {
    /// Buffer handle.
    pub buf: nvim_oxi::api::Buffer,
    /// Window handle.
    pub win: nvim_oxi::api::Window,
}

/// Side-by-side diff view (ref vs working tree).
#[derive(Debug, Clone)]
pub(crate) struct SideBySide {
    /// Buffer for ref content.
    pub old_buf: nvim_oxi::api::Buffer,
    /// Buffer for working tree content.
    pub new_buf: nvim_oxi::api::Buffer,
    /// Window for the ref buffer.
    pub old_win: nvim_oxi::api::Window,
    /// Window for the working tree buffer.
    pub new_win: nvim_oxi::api::Window,
}

type BeforeSnapshot = (String, String, HashMap<String, Option<String>>);

/// A single named prompt conversation with its message history and backend session.
pub(crate) struct PromptConversation {
    pub messages: Vec<threads::Message>,
    pub session_id: Option<String>,
}

/// Central runtime object for the review workbench.
///
/// One exists per open review. Dropped on close.
pub(crate) struct Review {
    /// Ref to diff against (e.g. "main"); empty = working tree.
    pub ref_name: String,
    /// Working directory captured at open time.
    pub cwd: String,
    /// Tabpage handle.
    pub tabpage: nvim_oxi::api::TabPage,
    /// File panel (left).
    pub file_panel: Box<dyn FilePanel>,
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
    /// Inline thread indicators: 0-based buffer line -> thread ID.
    /// Placed at the actual diff content line where a thread is anchored.
    pub thread_inline_marks: HashMap<usize, String>,
    /// Thread to open after the next diff render completes.
    pub pending_thread_open: Option<String>,
    /// Raw diff text for the current file. Retained so individual hunks
    /// can be extracted for staging without re-running git.
    pub current_diff_text: String,
    /// Original diff lines with `+`/`-`/` ` prefixes intact.
    /// In Signs mode the buffer lines have prefixes stripped for syntax
    /// highlighting, but `buf_line_to_source` needs the originals to
    /// correctly map buffer positions to source lines.
    pub current_diff_lines: Vec<String>,
    /// Per-file accepted hunk content hashes (review checklist).
    /// Used to fold and dim accepted hunks in the diff panel. Persisted in review state.
    pub accepted_hunks: HashMap<String, HashSet<String>>,
    /// Patches staged by arbiter during this session (content_hash -> patch text).
    /// Only hunks staged by arbiter are tracked here; pre-existing staged hunks
    /// are not included, so unstaging only reverses what arbiter staged.
    pub staged_patches: HashMap<String, String>,
    /// File navigation history for the `file_back` keymap.
    ///
    /// Pushed whenever the user navigates to a different file (next/prev file,
    /// next/prev unreviewed, thread jumps, file panel selection, etc.).
    /// Not pushed for re-renders of the current file or for `handle_file_back` itself.
    pub file_history: Vec<String>,
    /// When set, the next diff render will jump to the first (true) or last
    /// (false) unaccepted hunk. Used for cross-file `]c` / `[c` navigation.
    pub pending_hunk_nav: Option<bool>,
    /// When true, the next diff render will scroll the cursor to line 1.
    /// Set when advancing to the next unreviewed file after approval.
    pub pending_scroll_top: bool,
    /// Generalizable coding conventions extracted from resolved threads.
    /// Injected into every thread prompt so the agent "learns" the reviewer's
    /// preferences over the course of the review.
    pub review_rules: Vec<String>,
    /// Runtime toggle for the learn_rules feature. Initialized from
    /// `config.learn_rules` and can be toggled with `:ArbiterToggleRules`.
    pub learn_rules: bool,
    /// Active revision view state. When `Some`, the workbench is showing
    /// a revision diff instead of the full branch diff.
    pub revision_view: Option<RevisionViewState>,
    /// Named prompt conversations keyed by conversation ID.
    pub prompt_conversations: HashMap<String, PromptConversation>,
    /// Static project rules loaded from disk.
    pub project_rules: Vec<rules::Rule>,
}

/// State for the revision view mode.
///
/// When active, the file panel shows only revision files and the diff
/// panel shows the revision's before/after diff instead of the branch diff.
pub(crate) struct RevisionViewState {
    /// Thread ID owning the revision.
    pub thread_id: String,
    /// 1-based revision index within the thread.
    pub revision_index: u32,
    /// Files from the revision as a simplified file list.
    pub files: Vec<(String, FileStatus, ReviewStatus)>,
    /// Currently selected file within the revision.
    pub current_file: Option<String>,
    /// Accepted hunk hashes within the revision view.
    pub accepted_hunks: HashMap<String, HashSet<String>>,
    /// Saved state from the main compare for restoration on exit.
    pub saved_file: Option<String>,
}

/// Runs a closure with the active Review, if one exists.
///
/// Returns `None` if no review is active or if the state is already
/// borrowed (re-entrancy guard). This prevents `RefCell` panics when
/// Neovim API calls inside the closure trigger autocmds that re-enter.
pub(crate) fn with_active<F, R>(f: F) -> Option<R>
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
pub(crate) fn is_active() -> bool {
    ACTIVE.with(|cell| cell.try_borrow().map(|opt| opt.is_some()).unwrap_or(false))
}

/// Opens the thread window for the thread at the given absolute file path and line.
pub(crate) fn open_thread_at(abs_file: &str, line: u32) {
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
            navigate_to_file(r, &rel);
        } else {
            scroll_to_thread_and_open(r, &tid);
        }
    });
}

fn scroll_to_thread_and_open(review: &mut Review, tid: &str) {
    if !ensure_diff_panel(review) {
        return;
    }
    let Some(t) = review.threads.iter().find(|t| t.id == tid) else {
        return;
    };
    let str_refs: Vec<&str> = review
        .current_diff_lines
        .iter()
        .map(|s| s.as_str())
        .collect();
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
pub(crate) fn open(ref_name: Option<&str>) -> nvim_oxi::Result<()> {
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
    let ref_name = ref_name.unwrap_or("").to_string();

    api::command("tabnew")?;
    let tabpage = api::get_current_tabpage();

    let setup_result = (|| -> nvim_oxi::Result<(
        nvim_oxi::api::Buffer,
        nvim_oxi::api::Window,
        nvim_oxi::api::Buffer,
        nvim_oxi::api::Window,
        FilePanelKind,
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
        let file_panel_win = api::get_current_win();

        let (file_panel_buf, effective_panel) = match config.file_panel {
            FilePanelKind::Builtin => {
                let mut buf = api::create_buf(false, true)?;
                api::set_option_value("buftype", "nofile", &OptionOpts::builder().buffer(buf.clone()).build())?;
                api::set_option_value("modifiable", false, &OptionOpts::builder().buffer(buf.clone()).build())?;
                buf.set_name("[arbiter] files")?;
                let mut win = file_panel_win.clone();
                win.set_buf(&buf)?;
                let fp_opts = OptionOpts::builder().win(file_panel_win.clone()).build();
                api::set_option_value("foldenable", false, &fp_opts)?;
                api::set_option_value("winfixwidth", true, &fp_opts)?;
                api::set_option_value("winbar", " [arbiter] files", &fp_opts)?;
                (buf, FilePanelKind::Builtin)
            }
            FilePanelKind::NvimTree => {
                let _ = api::set_var("_arbiter_cwd", cwd.as_str());
                let _ = api::set_var("_arbiter_visible", "[]");
                let _ = api::set_var("_arbiter_signs", "{}");
                let _ = api::command(
                    "lua require('arbiter.nvim_tree_adapter').set_state(vim.g._arbiter_cwd, vim.g._arbiter_visible, vim.g._arbiter_signs)",
                );
                let open_ok = api::command(
                    "lua vim.g._arbiter_nvtree_ok = require('arbiter.nvim_tree_adapter').open(vim.g._arbiter_cwd)"
                )
                .is_ok();
                let tree_ok =
                    open_ok && api::get_var::<bool>("_arbiter_nvtree_ok").unwrap_or(false);
                if tree_ok {
                    (api::get_current_buf(), FilePanelKind::NvimTree)
                } else {
                    let mut buf = api::create_buf(false, true)?;
                    api::set_option_value("buftype", "nofile", &OptionOpts::builder().buffer(buf.clone()).build())?;
                    api::set_option_value("modifiable", false, &OptionOpts::builder().buffer(buf.clone()).build())?;
                    buf.set_name("[arbiter] files")?;
                    let mut win = file_panel_win.clone();
                    win.set_buf(&buf)?;
                    let fp_opts = OptionOpts::builder().win(file_panel_win.clone()).build();
                    api::set_option_value("foldenable", false, &fp_opts)?;
                    api::set_option_value("winfixwidth", true, &fp_opts)?;
                    api::set_option_value("winbar", " [arbiter] files", &fp_opts)?;
                    let _ = api::notify(
                        "[arbiter] nvim-tree not available, falling back to builtin panel",
                        nvim_oxi::api::types::LogLevel::Warn,
                        &Dictionary::default(),
                    );
                    (buf, FilePanelKind::Builtin)
                }
            }
        };

        api::command("wincmd l")?;

        Ok((file_panel_buf, file_panel_win, diff_panel_buf, diff_panel_win, effective_panel))
    })();

    let (file_panel_buf, file_panel_win, diff_panel_buf, diff_panel_win, effective_panel) =
        match setup_result {
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
                let file_panel_buf = file_panel_buf.clone();
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

                    let open_thread_counts = open_thread_count_map(&threads);
                    let fp: Box<dyn FilePanel> = match effective_panel {
                        FilePanelKind::Builtin => {
                            let mut builtin = BuiltinFilePanel::new(
                                file_panel_buf.clone(),
                                file_panel_win.clone(),
                            );
                            if let Err(e) = builtin.render(&files, &open_thread_counts) {
                                let _ = api::set_current_tabpage(&tabpage);
                                let _ = api::command("tabclose");
                                let _ = api::notify(
                                    &format!("[arbiter] failed to render file panel: {e}"),
                                    nvim_oxi::api::types::LogLevel::Error,
                                    &Dictionary::default(),
                                );
                                return;
                            }
                            Box::new(builtin)
                        }
                        FilePanelKind::NvimTree => {
                            let mut nvtree = NvimTreeFilePanel::new(
                                file_panel_buf.clone(),
                                file_panel_win.clone(),
                                cwd.clone(),
                            );
                            let _ = nvtree.render(&files, &open_thread_counts);
                            Box::new(nvtree)
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
                        file_panel: fp,
                        diff_panel: Panel {
                            buf: diff_panel_buf.clone(),
                            win: diff_panel_win.clone(),
                        },
                        files: files.clone(),
                        file_index: file_index.clone(),
                        current_file: current_file.clone(),
                        threads: threads.clone(),
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
                        thread_inline_marks: HashMap::new(),
                        pending_thread_open: None,
                        current_diff_text: String::new(),
                        current_diff_lines: Vec::new(),
                        accepted_hunks: review_state
                            .files
                            .iter()
                            .filter(|(_, f)| !f.accepted_hunks.is_empty())
                            .map(|(p, f)| (p.clone(), f.accepted_hunks.iter().cloned().collect()))
                            .collect(),
                        staged_patches: HashMap::new(),
                        file_history: Vec::new(),
                        pending_hunk_nav: None,
                        pending_scroll_top: false,
                        review_rules: review_state.review_rules.clone(),
                        learn_rules: config.learn_rules,
                        revision_view: None,
                        prompt_conversations: HashMap::new(),
                        project_rules: rules::load_all(&cwd, &config.rules_dirs),
                    };

                    set_close_keymap(review.file_panel.buffer_mut());
                    set_close_keymap(&mut review.diff_panel.buf);
                    set_file_panel_keymaps(review.file_panel.buffer_mut());
                    set_diff_panel_keymaps(&mut review.diff_panel.buf, &review.config);

                    if let Some(path) = &current_file {
                        select_file_impl(&mut review, path);
                        review.file_panel.highlight_file(path);
                    }

                    poll::start(&cwd);

                    backend::set_on_item_started(Box::new(|tag: &str| {
                        if threads::window_thread_id().as_deref() == Some(tag) {
                            let _ = threads::append_status("agent thinking...");
                        }
                    }));

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
            Some((s, p)) if !p.trim().is_empty() => (s.chars().next().unwrap_or('M'), p.trim()),
            _ => continue,
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

fn handle_toggle_sbs(review: &mut Review) {
    if review.side_by_side {
        if let Some(sbs) = review.sbs.as_ref() {
            let _ =
                diff::close_side_by_side(&sbs.old_buf, &sbs.old_win, &sbs.new_buf, &sbs.new_win);
        }
        review.sbs = None;
        review.side_by_side = false;
        return;
    }
    let Some(path) = review.current_file.clone() else {
        return;
    };
    let is_untracked = review
        .files
        .iter()
        .find(|(p, _, _)| *p == path)
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

    if review.revision_view.is_some() {
        accept_all_revision_hunks(review);
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
    let working_tree = review.ref_name.is_empty();

    if new_status == ReviewStatus::Approved {
        if working_tree {
            let file_accepted = accepted_for_file(review);
            for h in &review.current_hunks {
                if file_accepted.contains(&h.content_hash) {
                    continue;
                }
                if let Some(patch) =
                    diff::build_hunk_patch(&review.current_diff_text, &h.content_hash)
                {
                    let result = git::stage_patch(&review.cwd, &patch);
                    if result.success() {
                        review.staged_patches.insert(h.content_hash.clone(), patch);
                    }
                }
            }
        }

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
        save_file_statuses(review);
        rerender_file_panel(review);
        handle_next_unreviewed(review);
    } else {
        if working_tree {
            let accepted = review
                .accepted_hunks
                .get(&path)
                .cloned()
                .unwrap_or_default();
            for hash in &accepted {
                if let Some(patch) = review.staged_patches.get(hash) {
                    let result = git::unstage_patch(&review.cwd, patch);
                    if result.success() {
                        review.staged_patches.remove(hash);
                    } else {
                        let _ = api::notify(
                            &format!("[arbiter] unstage failed: {}", result.stderr.trim()),
                            nvim_oxi::api::types::LogLevel::Error,
                            &Dictionary::default(),
                        );
                    }
                }
            }
        }

        review.file_content_hash.remove(&path);
        review.accepted_hunks.remove(&path);

        select_file_impl(review, &path);
        save_file_statuses(review);
        rerender_file_panel(review);
    }
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

pub(crate) fn show_summary(review: &mut Review) {
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

/// Refreshes the diff panel for the current file.
///
/// On file mtime change: git::diff, re-render, detect_hunk_changes,
/// ArbiterHunkNew extmarks, reset approved to Unreviewed, reanchor,
/// bin unmatched, check_auto_resolve_timeouts, preserve cursor/scroll.
pub(crate) fn refresh_file(review: &mut Review) {
    let Some(path) = review.current_file.clone() else {
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
        .find(|(p, _, _)| *p == path)
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

    let full_path = Path::new(&review.cwd).join(path);
    let contents = std::fs::read_to_string(&full_path).unwrap_or_default();

    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| p == path) {
        if *rs == ReviewStatus::Approved {
            let new_hash = state::content_hash(&contents);
            if review.file_content_hash.get(path) != Some(&new_hash) {
                *rs = ReviewStatus::Unreviewed;
                review.file_content_hash.remove(path);
            }
        }
    }

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
    if let Ok((hunks, thread_buf_lines, diff_lines)) = diff::render(
        &mut review.diff_panel.buf,
        diff_text,
        &summaries,
        path,
        review.show_resolved,
        Some(&new_hunk_buf_starts),
        &file_accepted,
    ) {
        review.current_hunks = hunks.clone();
        review.current_diff_lines = diff_lines;
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
    let mut review_state = build_review_state(review);
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
pub(crate) fn refresh_file_list(review: &mut Review) {
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

                    let approved_paths: Vec<String> = files
                        .iter()
                        .filter(|(_, _, rs)| *rs == ReviewStatus::Approved)
                        .map(|(p, _, _)| p.clone())
                        .collect();
                    let known_hashes: HashMap<String, String> =
                        with_active(|r| r.file_content_hash.clone()).unwrap_or_default();
                    let cwd_bg = with_active(|r| r.cwd.clone()).unwrap_or_default();

                    std::thread::spawn({
                        let files = files.clone();
                        let file_index = file_index.clone();
                        let state_dir = state_dir.clone();
                        let ws_hash = ws_hash.clone();
                        let ref_name = ref_name.clone();
                        move || {
                            let mut stale: Vec<String> = Vec::new();
                            for p in &approved_paths {
                                let full = Path::new(&cwd_bg).join(p);
                                if let Ok(contents) = std::fs::read_to_string(&full) {
                                    let new_hash = state::content_hash(&contents);
                                    if known_hashes.get(p.as_str()) != Some(&new_hash) {
                                        stale.push(p.clone());
                                    }
                                }
                            }
                            crate::dispatch::schedule(move || {
                                with_active(|r| {
                                    r.files = files;
                                    r.file_index = file_index;
                                    for p in &stale {
                                        if let Some((_, _, rs)) =
                                            r.files.iter_mut().find(|(fp, _, _)| fp == p)
                                        {
                                            *rs = ReviewStatus::Unreviewed;
                                        }
                                        r.file_content_hash.remove(p);
                                    }
                                    rerender_file_panel(r);
                                    let mut review_state = build_review_state(r);
                                    for (p, _, rs) in &r.files {
                                        review_state
                                            .files
                                            .insert(p.clone(), file_state_for(r, p, *rs));
                                    }
                                    state::save_review(
                                        &state_dir,
                                        &ws_hash,
                                        &ref_name,
                                        &review_state,
                                    );
                                });
                            });
                        }
                    });
                }
            });
        }
    });
}

pub(crate) fn rerender_file_panel(review: &mut Review) {
    let open_thread_counts = open_thread_count_map(&review.threads);
    if let Err(e) = review.file_panel.render(&review.files, &open_thread_counts) {
        let _ = api::notify(
            &format!("[arbiter] file panel render failed: {e}"),
            nvim_oxi::api::types::LogLevel::Warn,
            &Dictionary::default(),
        );
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

/// Ensures the diff panel window exists, recreating it if it was closed.
///
/// Returns `true` if the diff panel is usable. The buffer persists across
/// window closes, so only a new window + layout fixup is needed.
fn ensure_diff_panel(review: &mut Review) -> bool {
    if review.diff_panel.win.is_valid() {
        return true;
    }
    let saved_win = api::get_current_win();
    let _ = api::set_current_win(review.file_panel.window());
    if api::command("rightbelow vsplit").is_err() {
        let _ = api::set_current_win(&saved_win);
        return false;
    }
    let mut new_win = api::get_current_win();
    if new_win.set_buf(&review.diff_panel.buf).is_err() {
        let _ = api::command("close");
        let _ = api::set_current_win(&saved_win);
        return false;
    }
    review.diff_panel.win = new_win.clone();
    let _ = api::set_current_win(review.file_panel.window());
    let _ = api::command("vertical resize 40");
    if let Some(path) = review.current_file.as_deref() {
        update_diff_winbar(review, path);
    }
    let _ = api::set_current_win(&saved_win);
    true
}

/// Navigates to a file, pushing the current file onto `file_history` if changing.
///
/// Use this for user-initiated navigation (next/prev file, thread jumps, file
/// panel clicks, etc.). For re-renders of the current file or for
/// `handle_file_back`, call `select_file_impl` directly.
fn navigate_to_file(review: &mut Review, path: &str) {
    if review.current_file.as_deref() != Some(path) {
        if let Some(prev) = review.current_file.clone() {
            review.file_history.push(prev);
        }
    }
    select_file_impl(review, path);
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
        if let Ok((hunks, thread_buf_lines, diff_lines)) = diff::render(
            &mut review.diff_panel.buf,
            &diff_text,
            &summaries,
            path,
            review.show_resolved,
            None,
            &file_accepted,
        ) {
            review.current_hunks = hunks.clone();
            review.current_diff_lines = diff_lines;
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
        apply_pending_hunk_nav(review);
        apply_pending_scroll_top(review);
        snap_cursor_to_hunk(review, &review.current_hunks.clone());
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
        if let Ok((hunks, thread_buf_lines, diff_lines)) = diff::render(
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
                r.current_diff_lines = diff_lines;
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
                apply_pending_hunk_nav(r);
                apply_pending_scroll_top(r);
                snap_cursor_to_hunk(r, &hunks);
            });
        }
    });
}

/// Closes the workbench, persists state, cancels backend, stops timers.
pub(crate) fn close() {
    poll::stop();
    backend::cancel_all();
    threads::window_close();
    crate::prompt_panel::close();
    close_thread_list_win();

    let review = ACTIVE.with(|cell| cell.try_borrow_mut().ok().and_then(|mut opt| opt.take()));

    let Some(mut review) = review else { return };

    let state_dir = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let ref_name = review.ref_name.clone();

    let mut review_state = build_review_state(&review);
    for (p, _, rs) in &review.files {
        review_state
            .files
            .insert(p.clone(), file_state_for(&review, p, *rs));
    }
    state::save_review(&state_dir, &ws_hash, &ref_name, &review_state);
    state::save_threads(&state_dir, &ws_hash, &ref_name, &review.threads);

    if let Some(sbs) = review.sbs.as_ref() {
        let _ = diff::close_side_by_side(&sbs.old_buf, &sbs.old_win, &sbs.new_buf, &sbs.new_win);
    }

    let wipe_fp = review.file_panel.should_wipe_buffer();
    let file_nr = review.file_panel.buf_handle();
    review.file_panel.cleanup();

    let diff_nr = review.diff_panel.buf.handle();

    let _ = api::set_current_tabpage(&review.tabpage);
    let _ = api::command("tabclose");
    if wipe_fp {
        let _ = api::command(&format!("silent! bwipeout! {diff_nr} {file_nr}"));
    } else {
        let _ = api::command(&format!("silent! bwipeout! {diff_nr}"));
    }
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

    let Some(current_file) = review.current_file.clone() else {
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

    let line_refs: Vec<&str> = review
        .current_diff_lines
        .iter()
        .map(|s| s.as_str())
        .collect();

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

    #[test]
    fn parse_diff_names_empty_path_after_tab() {
        let state = state::ReviewState::default();
        let out = parse_diff_names("M\t\n", &state);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn parse_diff_names_unknown_status_defaults_to_modified() {
        let state = state::ReviewState::default();
        let out = parse_diff_names("X\tfile.rs\n", &state);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "file.rs");
        assert_eq!(out[0].1, FileStatus::Modified);
    }
}
