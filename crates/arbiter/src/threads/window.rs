//! Thread panel for viewing thread messages and composing replies.
//!
//! Opens as a split panel (default: right side, configurable via
//! `thread_window.position`). `<CR>` opens the input float for reply;
//! `q` closes.

use super::Message;
use crate::config;
use crate::types::Role;
use chrono::Local;
use nvim_oxi::api::opts::{CreateAutocmdOpts, OptionOpts, SetKeymapOpts};
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self, Buffer, Window};
use std::cell::RefCell;
use std::sync::Arc;

const SEPARATOR: &str = "  ────────────────────────────────";
const STATUS_PREFIX: &str = "  ⏳ ";

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static BUFFER: RefCell<Option<Buffer>> = const { RefCell::new(None) };
    static THREAD_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static ON_CLOSE: RefCell<Option<OnClose>> = const { RefCell::new(None) };
}

/// Callback invoked when the user requests to reply (presses `<CR>`).
pub type OnReplyRequested = Box<dyn Fn() + Send + Sync>;

/// Callback invoked when the thread panel is closed via `q`.
pub type OnClose = Arc<dyn Fn() + Send + Sync>;

fn format_ts(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    let fmt = &config::get().thread_window.date_format;
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_default()
}

fn format_now() -> String {
    let fmt = &config::get().thread_window.date_format;
    Local::now().format(fmt).to_string()
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
    on_reply: OnReplyRequested,
    on_close: Option<OnClose>,
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
        let ts_str = format_ts(m.ts);
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

    let on_reply_cell = Arc::new(on_reply);
    let opts = SetKeymapOpts::builder()
        .callback(move |_| {
            on_reply_cell();
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
    ON_CLOSE.with(|c| *c.borrow_mut() = on_close);

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
        let Some(ref mut buf) = *guard else {
            return Ok(());
        };
        let (author, hl) = match role {
            Role::User => ("you", "ArbiterThreadUser"),
            Role::Agent => ("agent", "ArbiterThreadAgent"),
        };
        let author_line = format!("┊ {author}  {}", format_now());

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
        let Some(ref mut buf) = *guard else {
            return Ok(());
        };
        clear_status(buf)?;
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
        let Some(ref mut buf) = *guard else {
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

/// Removes the trailing status line if present. Returns the updated line count.
fn clear_status(buf: &mut Buffer) -> nvim_oxi::Result<usize> {
    let line_count = buf.line_count()?;
    if line_count == 0 {
        return Ok(0);
    }
    let has_status = buf
        .get_lines((line_count - 1)..line_count, false)?
        .next()
        .map(|s| s.to_string_lossy().starts_with(STATUS_PREFIX))
        .unwrap_or(false);
    if has_status {
        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;
        buf.set_lines((line_count - 1)..line_count, false, Vec::<&str>::new())?;
        api::set_option_value("modifiable", false, &buf_opts)?;
        Ok(line_count - 1)
    } else {
        Ok(line_count)
    }
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
        let Some(ref mut buf) = *guard else {
            return Ok(());
        };

        clear_status(buf)?;

        let line_count = buf.line_count()?;
        let all_lines: Vec<String> = buf
            .get_lines(0..line_count, false)?
            .map(|s| s.to_string())
            .collect();

        let last_author_is_agent = all_lines
            .iter()
            .rev()
            .find(|l| l.starts_with("┊ "))
            .map(|l| l.starts_with("┊ agent"))
            .unwrap_or(false);

        let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
        api::set_option_value("modifiable", true, &buf_opts)?;

        if !last_author_is_agent {
            let author_line = format!("┊ agent  {}", format_now());
            let has_content = line_count > 0;
            let mut insert: Vec<String> = Vec::new();
            if has_content {
                insert.push(SEPARATOR.to_string());
                insert.push(String::new());
            }
            let author_offset = insert.len();
            insert.push(author_line);
            for l in text.split('\n') {
                insert.push(format!("  {l}"));
            }
            let refs: Vec<&str> = insert.iter().map(|s| s.as_str()).collect();
            buf.set_lines(line_count..line_count, false, refs)?;

            let ns = api::create_namespace("arbiter-thread-win");
            if has_content {
                let _ = buf.add_highlight(ns, "NonText", line_count, 0..);
            }
            let _ = buf.add_highlight(ns, "ArbiterThreadAgent", line_count + author_offset, 0..);
        } else {
            let last_idx = line_count.saturating_sub(1);
            let last = all_lines.last().map(|s| s.as_str()).unwrap_or_default();

            let segments: Vec<&str> = text.split('\n').collect();
            let first = segments[0];
            let combined = if last.starts_with("  ") {
                format!("{last}{first}")
            } else {
                format!("  {first}")
            };
            buf.set_lines(last_idx..=last_idx, false, [combined.as_str()])?;

            if segments.len() > 1 {
                let new_count = buf.line_count()?;
                let remaining: Vec<String> =
                    segments[1..].iter().map(|l| format!("  {l}")).collect();
                let refs: Vec<&str> = remaining.iter().map(|s| s.as_str()).collect();
                buf.set_lines(new_count..new_count, false, refs)?;
            }
        }

        api::set_option_value("modifiable", false, &buf_opts)?;

        scroll_to_bottom(buf);

        Ok(())
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
        let Some(ref mut buf) = *guard else {
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
        if let Some(ref mut win) = *w.borrow_mut() {
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
    if let Some(cb) = cb {
        cb();
    }
}

/// Returns true if the thread panel is open.
pub fn is_open() -> bool {
    WINDOW.with(|c| c.borrow().is_some())
}

/// Returns the thread ID currently displayed in the panel, if any.
pub fn current_thread_id() -> Option<String> {
    THREAD_ID.with(|c| c.borrow().clone())
}
