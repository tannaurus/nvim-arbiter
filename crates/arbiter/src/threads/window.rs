//! Thread panel for viewing thread messages and composing replies.
//!
//! Opens as a split panel (default: right side, configurable via
//! `thread_window.position`). `<CR>` opens the input float for reply;
//! `q` closes.

use super::Message;
use crate::config;
use crate::panel::{self, SEPARATOR, STATUS_PREFIX};
use crate::types::Role;

use nvim_oxi::api::opts::{CreateAutocmdOpts, OptionOpts, SetKeymapOpts};
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self, Buffer, Window};
use nvim_oxi::IntoResult;
use std::cell::RefCell;
use std::sync::Arc;

const INTERRUPTED_PREFIX: &str = "  ⚠ interrupted  ";
const REVISION_PREFIX: &str = "  ◆ ";

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static BUFFER: RefCell<Option<Buffer>> = const { RefCell::new(None) };
    static THREAD_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static ON_CLOSE: RefCell<Option<OnClose>> = const { RefCell::new(None) };
    static ON_REVISION: RefCell<Option<OnRevisionSelected>> = const { RefCell::new(None) };
    static ON_SIMILAR: RefCell<Option<OnSimilarSelected>> = const { RefCell::new(None) };
    static SIMILAR_MAP: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
}

/// Callback invoked when the user requests to reply (presses `<CR>`).
pub type OnReplyRequested = Box<dyn Fn() + Send + Sync>;

/// Callback invoked when the thread panel is closed via `q`.
pub type OnClose = Arc<dyn Fn() + Send + Sync>;

/// Callback invoked when the user presses `<CR>` on a revision summary line.
/// Receives the 1-based revision index.
pub type OnRevisionSelected = Arc<dyn Fn(u32) + Send + Sync>;

/// Callback invoked when the user presses `<CR>` on a similar-thread line.
/// Receives the thread ID of the similar thread.
pub type OnSimilarSelected = Arc<dyn Fn(String) + Send + Sync>;

/// Event handlers for the thread panel.
pub struct WindowCallbacks {
    pub on_reply: OnReplyRequested,
    pub on_close: Option<OnClose>,
    pub on_revision: Option<OnRevisionSelected>,
    pub on_similar: Option<OnSimilarSelected>,
}

/// Opens a split panel for the given thread.
///
/// Split direction and size are controlled by `thread_window.position`
/// and `thread_window.size` in the plugin config.
/// `<CR>` invokes `on_reply`; `q` invokes `on_close` then closes the panel.
pub fn open(
    thread_id: &str,
    file: &str,
    line: u32,
    messages: &[Message],
    callbacks: WindowCallbacks,
) -> nvim_oxi::Result<()> {
    let ea_was_on =
        api::get_option_value::<bool>("equalalways", &OptionOpts::default()).unwrap_or(true);
    if ea_was_on {
        let _ = api::set_option_value("equalalways", false, &OptionOpts::default());
    }

    if is_open() {
        close();
    }

    let mut buf = api::create_buf(false, true)?;
    let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
    api::set_option_value("buftype", "nofile", &buf_opts)?;
    api::set_option_value("filetype", "markdown", &buf_opts)?;

    let mut lines: Vec<String> = Vec::new();
    let mut highlights: Vec<(usize, &str)> = Vec::new();

    let title_line = format!("── {file}:{line} [{thread_id}] ──");
    highlights.push((0, "ArbiterDiffFile"));
    lines.push(title_line);
    lines.push(String::new());

    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            let sep_idx = lines.len();
            lines.push(SEPARATOR.to_string());
            highlights.push((sep_idx, "NonText"));
            lines.push(String::new());
        }

        let (author, hl) = match m.role {
            Role::User => ("you", "ArbiterThreadUser"),
            Role::Agent => ("agent", "ArbiterThreadAgent"),
        };
        let ts_str = panel::format_ts(m.ts);
        let author_line = if ts_str.is_empty() {
            format!("┊ {author}")
        } else {
            format!("┊ {author}  {ts_str}")
        };
        let line_idx = lines.len();
        highlights.push((line_idx, hl));
        lines.push(author_line);

        for l in m.text.lines() {
            lines.push(format!("  {l}"));
        }
        lines.push(String::new());
    }

    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    buf.set_lines(0..0, false, refs)?;
    api::set_option_value("modifiable", false, &buf_opts)?;

    let ns = api::create_namespace("arbiter-thread-win");
    for (line_idx, hl) in &highlights {
        let _ = buf.add_highlight(ns, hl, *line_idx, 0..);
    }

    let tw = &config::get().thread_window;
    let split_cmd = tw.position.split_cmd(tw.size);

    let saved_win = api::get_current_win();
    api::command(&split_cmd)?;
    let mut win = api::get_current_win();
    win.set_buf(&buf)?;

    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("number", false, &win_opts);
    let _ = api::set_option_value("relativenumber", false, &win_opts);
    let _ = api::set_option_value("signcolumn", "no", &win_opts);
    let _ = api::set_option_value("cursorline", true, &win_opts);
    let _ = api::set_option_value("wrap", true, &win_opts);
    if tw.position.is_vertical() {
        let _ = api::set_option_value("winfixwidth", true, &win_opts);
    } else {
        let _ = api::set_option_value("winfixheight", true, &win_opts);
    }

    let on_reply_cell = Arc::new(callbacks.on_reply);
    let opts = SetKeymapOpts::builder()
        .callback(move |_| {
            if let Some(rev_idx) = revision_at_cursor() {
                let cb = ON_REVISION.with(|c| c.borrow().clone());
                if let Some(cb) = cb {
                    cb(rev_idx);
                }
            } else if let Some(tid) = similar_at_cursor() {
                let cb = ON_SIMILAR.with(|c| c.borrow().clone());
                if let Some(cb) = cb {
                    cb(tid);
                }
            } else {
                on_reply_cell();
            }
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts);

    let opts_close = SetKeymapOpts::builder()
        .callback(move |_| {
            close();
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts_close);

    let buf_for_autocmd = buf.clone();
    WINDOW.with(|c| *c.borrow_mut() = Some(win));
    BUFFER.with(|c| *c.borrow_mut() = Some(buf));
    THREAD_ID.with(|c| *c.borrow_mut() = Some(thread_id.to_string()));
    ON_CLOSE.with(|c| *c.borrow_mut() = callbacks.on_close);
    ON_REVISION.with(|c| *c.borrow_mut() = callbacks.on_revision);
    ON_SIMILAR.with(|c| *c.borrow_mut() = callbacks.on_similar);
    SIMILAR_MAP.with(|c| c.borrow_mut().clear());

    let _ = api::create_autocmd(
        ["BufWipeout"],
        &CreateAutocmdOpts::builder()
            .buffer(buf_for_autocmd)
            .callback(|_| {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    WINDOW.with(|c| c.borrow_mut().take());
                    BUFFER.with(|c| c.borrow_mut().take());
                    let cb = ON_CLOSE.with(|c| c.borrow_mut().take());
                    THREAD_ID.with(|c| c.borrow_mut().take());
                    ON_REVISION.with(|c| c.borrow_mut().take());
                    ON_SIMILAR.with(|c| c.borrow_mut().take());
                    SIMILAR_MAP.with(|c| c.borrow_mut().clear());
                    if let Some(cb) = cb {
                        cb();
                    }
                }));
                false
            })
            .build(),
    );

    let _ = api::set_current_win(&saved_win);

    if ea_was_on {
        let _ = api::set_option_value("equalalways", true, &OptionOpts::default());
    }

    Ok(())
}

/// Appends a full message to the thread panel with a separator.
pub fn append_message(role: Role, text: &str) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let (author, hl) = match role {
            Role::User => ("you", "ArbiterThreadUser"),
            Role::Agent => ("agent", "ArbiterThreadAgent"),
        };
        let author_line = format!("┊ {author}  {}", panel::format_now());

        let line_count = buf.line_count()?;
        let mut new_lines: Vec<String> = Vec::new();
        if line_count > 0 {
            new_lines.push(SEPARATOR.to_string());
            new_lines.push(String::new());
        }
        let author_offset = new_lines.len();
        new_lines.push(author_line);
        for l in text.lines() {
            new_lines.push(format!("  {l}"));
        }
        new_lines.push(String::new());

        let refs: Vec<&str> = new_lines.iter().map(|s| s.as_str()).collect();
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        buf.set_lines(line_count..line_count, false, refs)?;
        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        if line_count > 0 {
            let _ = buf.add_highlight(ns, "NonText", line_count, 0..);
        }
        let _ = buf.add_highlight(ns, hl, line_count + author_offset, 0..);

        scroll_to_bottom(buf);

        Ok(())
    })
}

/// Replaces the last agent message block with authoritative final text.
///
/// Called when the backend result arrives to fix any streaming artifacts
/// (e.g. missing newlines at chunk boundaries).
pub fn replace_last_agent_message(text: &str) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        panel::clear_status(buf)?;
        let line_count = buf.line_count()?;
        let all_lines: Vec<String> = buf
            .get_lines(0..line_count, false)?
            .map(|s| s.to_string())
            .collect();

        let agent_header_idx = all_lines.iter().rposition(|l| l.starts_with("┊ agent"));
        let Some(header_idx) = agent_header_idx else {
            return Ok(());
        };

        let content_start = header_idx + 1;
        let mut new_lines: Vec<String> = Vec::new();
        for l in text.lines() {
            new_lines.push(format!("  {l}"));
        }
        new_lines.push(String::new());

        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        let refs: Vec<&str> = new_lines.iter().map(|s| s.as_str()).collect();
        buf.set_lines(content_start..line_count, false, refs)?;
        api::set_option_value("modifiable", false, &buf_opts)?;

        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Appends a status line (e.g. "thinking..." or "queued") to the thread panel.
///
/// If a status line already exists it is replaced. Automatically cleared
/// when the first streaming chunk arrives via `append_streaming`.
pub fn append_status(message: &str) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;

        let line_count = buf.line_count()?;
        let status_text = format!("{STATUS_PREFIX}{message}");

        let existing_status = if line_count > 0 {
            buf.get_lines((line_count - 1)..line_count, false)?
                .next()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| s.starts_with(STATUS_PREFIX))
                .is_some()
        } else {
            false
        };

        if existing_status {
            buf.set_lines((line_count - 1)..line_count, false, [status_text.as_str()])?;
        } else {
            buf.set_lines(line_count..line_count, false, [status_text.as_str()])?;
        }

        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        let status_line = if existing_status {
            line_count - 1
        } else {
            line_count
        };
        let _ = buf.add_highlight(ns, "NonText", status_line, 0..);

        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Appends an "interrupted" line with a timestamp to the thread panel.
///
/// Replaces any existing status line. Used when a request is cancelled
/// (via the cancel keymap or by a new reply superseding an in-flight request).
pub fn append_interrupted() -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;

        let line_count = panel::clear_status(buf)?;
        let text = format!("{INTERRUPTED_PREFIX}{}", panel::format_now());
        buf.set_lines(line_count..line_count, false, [text.as_str()])?;

        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        let _ = buf.add_highlight(ns, "WarningMsg", line_count, 0..);

        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Appends streaming text from the agent.
///
/// On the first chunk (when the last author line is not `agent`), inserts
/// a separator, an `agent` header with timestamp, then the content.
/// On subsequent chunks, appends to the existing agent content,
/// splitting on newlines to create new buffer lines as needed.
/// Auto-scrolls the panel to the bottom after each append.
pub fn append_streaming(text: &str) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        panel::append_streaming_to_buf(buf, text, "arbiter-thread-win")?;
        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Appends a revision summary block to the thread panel.
///
/// Shows the revision number, file count, and per-file line stats.
/// Summary lines use a distinct highlight group and are non-message
/// metadata rendered from revision data.
pub fn append_revision_summary(
    rev_index: u32,
    file_count: usize,
    stats: &[(String, usize, usize)],
) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let line_count = buf.line_count()?;
        let mut new_lines: Vec<String> = Vec::new();
        if line_count > 0 {
            new_lines.push(SEPARATOR.to_string());
            new_lines.push(String::new());
        }
        let total_added: usize = stats.iter().map(|(_, a, _)| a).sum();
        let total_removed: usize = stats.iter().map(|(_, _, r)| r).sum();
        let header_offset = new_lines.len();
        new_lines.push(format!(
            "{REVISION_PREFIX}revision {rev_index} - {file_count} file{} changed (+{total_added} -{total_removed})",
            if file_count == 1 { "" } else { "s" }
        ));
        for (path, added, removed) in stats {
            new_lines.push(format!("    {path}  (+{added} -{removed})"));
        }
        new_lines.push(String::new());

        let refs: Vec<&str> = new_lines.iter().map(|s| s.as_str()).collect();
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        buf.set_lines(line_count..line_count, false, refs)?;
        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        if line_count > 0 {
            let _ = buf.add_highlight(ns, "NonText", line_count, 0..);
        }
        let _ = buf.add_highlight(
            ns,
            "ArbiterRevisionSummary",
            line_count + header_offset,
            0..,
        );
        for i in 0..stats.len() {
            let _ = buf.add_highlight(
                ns,
                "ArbiterRevisionFile",
                line_count + header_offset + 1 + i,
                0..,
            );
        }

        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Appends similar-thread cross-references to the thread panel.
///
/// Each entry is rendered as a clickable line. Pressing `<CR>` on one
/// navigates the diff panel to that thread's file and line.
pub fn append_similar_threads(refs: &[super::SimilarRef]) -> nvim_oxi::Result<()> {
    if refs.is_empty() {
        return Ok(());
    }
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let line_count = buf.line_count()?;
        let mut new_lines: Vec<String> = Vec::new();
        if line_count > 0 {
            new_lines.push(SEPARATOR.to_string());
            new_lines.push(String::new());
        }
        let header_offset = new_lines.len();
        new_lines.push("  ◇ similar issues".to_string());

        let mut map_entries: Vec<(usize, String)> = Vec::new();
        for r in refs {
            let preview: String = r.preview.chars().take(40).collect();
            let entry_line = line_count + new_lines.len();
            new_lines.push(format!("    {}:{} - {preview}", r.file, r.line));
            map_entries.push((entry_line, r.thread_id.clone()));
        }
        new_lines.push(String::new());

        let str_refs: Vec<&str> = new_lines.iter().map(|s| s.as_str()).collect();
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        buf.set_lines(line_count..line_count, false, str_refs)?;
        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        if line_count > 0 {
            let _ = buf.add_highlight(ns, "NonText", line_count, 0..);
        }
        let _ = buf.add_highlight(ns, "ArbiterSimilarHeader", line_count + header_offset, 0..);
        for i in 0..refs.len() {
            let _ = buf.add_highlight(
                ns,
                "ArbiterSimilarRef",
                line_count + header_offset + 1 + i,
                0..,
            );
        }

        SIMILAR_MAP.with(|m| m.borrow_mut().extend(map_entries));

        scroll_to_bottom(buf);
        Ok(())
    })
}

/// Returns the thread ID if the cursor is on a similar-thread line.
pub fn similar_at_cursor() -> Option<String> {
    WINDOW.with(|w| {
        let guard = w.borrow();
        let win = guard.as_ref()?;
        let (row, _) = win.get_cursor().into_result().ok()?;
        let buf_line = row.saturating_sub(1);
        SIMILAR_MAP.with(|m| {
            m.borrow()
                .iter()
                .find(|(line, _)| *line == buf_line)
                .map(|(_, tid)| tid.clone())
        })
    })
}

/// Appends a "rules learned" block to the thread panel showing
/// conventions that were extracted from this thread's conversation.
pub fn append_learned_rules(rules: &[String]) -> nvim_oxi::Result<()> {
    if rules.is_empty() {
        return Ok(());
    }
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        let line_count = buf.line_count()?;
        let mut new_lines: Vec<String> = Vec::new();
        if line_count > 0 {
            new_lines.push(SEPARATOR.to_string());
            new_lines.push(String::new());
        }
        let header_offset = new_lines.len();
        new_lines.push("✦ rules learned".to_string());
        for rule in rules {
            new_lines.push(format!("  • {rule}"));
        }
        new_lines.push(String::new());

        let refs: Vec<&str> = new_lines.iter().map(|s| s.as_str()).collect();
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        buf.set_lines(line_count..line_count, false, refs)?;
        api::set_option_value("modifiable", false, &buf_opts)?;

        let ns = api::create_namespace("arbiter-thread-win");
        if line_count > 0 {
            let _ = buf.add_highlight(ns, "NonText", line_count, 0..);
        }
        let _ = buf.add_highlight(ns, "ArbiterRuleLearned", line_count + header_offset, 0..);
        for i in 0..rules.len() {
            let _ = buf.add_highlight(
                ns,
                "ArbiterRuleLearned",
                line_count + header_offset + 1 + i,
                0..,
            );
        }

        scroll_to_bottom(buf);

        Ok(())
    })
}

fn scroll_to_bottom(buf: &Buffer) {
    WINDOW.with(|w| {
        if let Some(win) = w.borrow_mut().as_mut() {
            if let Ok(cnt) = buf.line_count() {
                let _ = win.set_cursor(cnt, 0);
            }
        }
    });
}

/// Closes the thread panel, invokes the stored `on_close` callback, and
/// closes any open input panel.
pub fn close() {
    super::input_close();
    let cb = ON_CLOSE.with(|c| c.borrow_mut().take());
    WINDOW.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
    BUFFER.with(|c| {
        c.borrow_mut().take();
    });
    THREAD_ID.with(|c| {
        c.borrow_mut().take();
    });
    ON_REVISION.with(|c| c.borrow_mut().take());
    ON_SIMILAR.with(|c| c.borrow_mut().take());
    SIMILAR_MAP.with(|c| c.borrow_mut().clear());
    if let Some(cb) = cb {
        cb();
    }
}

/// Returns true if the thread panel is open.
pub fn is_open() -> bool {
    WINDOW.with(|c| c.borrow().is_some())
}

/// Returns the thread panel window handle, if open.
pub fn handle() -> Option<Window> {
    WINDOW.with(|c| c.borrow().clone())
}

/// Returns the thread ID currently displayed in the panel, if any.
pub fn current_thread_id() -> Option<String> {
    THREAD_ID.with(|c| c.borrow().clone())
}

/// Returns the revision index if the cursor is on a revision summary line.
///
/// Parses "  ◆ revision N - ..." to extract N.
pub fn revision_at_cursor() -> Option<u32> {
    WINDOW.with(|w| {
        let guard = w.borrow();
        let win = guard.as_ref()?;
        let (row, _) = win.get_cursor().into_result().ok()?;
        BUFFER.with(|b| {
            let guard = b.borrow();
            let buf = guard.as_ref()?;
            let line_idx = row.saturating_sub(1);
            let text = buf
                .get_lines(line_idx..line_idx + 1, false)
                .ok()?
                .next()?
                .to_string_lossy()
                .to_string();
            parse_revision_line(&text)
        })
    })
}

fn parse_revision_line(text: &str) -> Option<u32> {
    let rest = text.strip_prefix(REVISION_PREFIX)?;
    let rest = rest.strip_prefix("revision ")?;
    let num_end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..num_end].parse().ok()
}
