//! Prompt panel for long-lived agent conversations.
//!
//! Opens as a centered floating window at 80% screen size. Maintains
//! session context across the review via `Review.prompt_messages` and
//! `Review.prompt_session_id`. `<CR>` opens the input float for composing;
//! `q` closes the panel (preserving session state).

use crate::backend;
use crate::panel::{self, SEPARATOR, STATUS_PREFIX};
use crate::review;
use crate::threads;
use crate::types::Role;

use nvim_oxi::api::opts::{CreateAutocmdOpts, OptionOpts, SetKeymapOpts};
use nvim_oxi::api::types::{Mode, WindowBorder, WindowRelativeTo};
use nvim_oxi::api::{self, Buffer, Window};
use std::cell::RefCell;
use std::sync::Arc;
const DEFAULT_CONVERSATION: &str = "main";

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static BUFFER: RefCell<Option<Buffer>> = const { RefCell::new(None) };
    static CONVERSATION_ID: RefCell<String> = RefCell::new(DEFAULT_CONVERSATION.to_string());
}

/// Opens the prompt panel as a centered floating window.
///
/// If already open and showing the same conversation, focuses it.
/// If open but showing a different conversation, closes and reopens
/// with the requested one. Renders any prior messages from the
/// conversation. Sets a winbar with the conversation name and message count.
pub(crate) fn open(conversation_id: &str) -> nvim_oxi::Result<()> {
    let id = if conversation_id.is_empty() {
        DEFAULT_CONVERSATION
    } else {
        conversation_id
    };

    if is_open() {
        let same = CONVERSATION_ID.with(|c| *c.borrow() == id);
        if same {
            WINDOW.with(|c| {
                if let Some(win) = c.borrow().as_ref() {
                    let _ = api::set_current_win(win);
                }
            });
            return Ok(());
        }
        close();
    }

    CONVERSATION_ID.with(|c| *c.borrow_mut() = id.to_string());

    let (messages, msg_count) = review::with_active(|r| {
        let conv = r
            .prompt_conversations
            .entry(id.to_string())
            .or_insert_with(|| review::PromptConversation {
                messages: Vec::new(),
                session_id: None,
            });
        (conv.messages.clone(), conv.messages.len())
    })
    .unwrap_or_default();

    let mut buf = api::create_buf(false, true)?;
    let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
    api::set_option_value("buftype", "nofile", &buf_opts)?;
    crate::panel::disable_syntax(&buf);

    let mut lines: Vec<String> = Vec::new();
    let mut highlights: Vec<(usize, &str)> = Vec::new();

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

    if lines.is_empty() {
        lines.push("  Press Enter to start a conversation.".to_string());
        highlights.push((0, "NonText"));
    }

    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    buf.set_lines(0..0, false, refs)?;
    api::set_option_value("modifiable", false, &buf_opts)?;

    let ns = api::create_namespace("arbiter-prompt-panel");
    for (line_idx, hl) in &highlights {
        let _ = buf.add_highlight(ns, hl, *line_idx, 0..);
    }

    let cols = api::get_option_value::<i64>("columns", &OptionOpts::default())?;
    let rows = api::get_option_value::<i64>("lines", &OptionOpts::default())?;
    let width = ((cols as f64) * 0.8) as u32;
    let height = ((rows as f64) * 0.8) as u32;
    let row = ((rows as f64) - (height as f64)) / 2.0;
    let col = ((cols as f64) - (width as f64)) / 2.0;

    let config = nvim_oxi::api::types::WindowConfig::builder()
        .relative(WindowRelativeTo::Editor)
        .width(width)
        .height(height)
        .row(row)
        .col(col)
        .border(WindowBorder::Rounded)
        .build();

    let win = api::open_win(&buf, true, &config)?;

    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("number", false, &win_opts);
    let _ = api::set_option_value("relativenumber", false, &win_opts);
    let _ = api::set_option_value("signcolumn", "no", &win_opts);
    let _ = api::set_option_value("cursorline", true, &win_opts);
    let _ = api::set_option_value("wrap", true, &win_opts);

    let winbar = format!(
        "%#ArbiterDiffFile# Prompt: {id} %#Comment# {} message{}  <Enter> reply  q close",
        msg_count,
        if msg_count == 1 { "" } else { "s" },
    );
    let _ = api::set_option_value("winbar", winbar.as_str(), &win_opts);

    let opts_reply = SetKeymapOpts::builder()
        .callback(move |_| {
            open_input();
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_reply);

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

    let _ = api::create_autocmd(
        ["BufWipeout"],
        &CreateAutocmdOpts::builder()
            .buffer(buf_for_autocmd)
            .callback(|_| {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    WINDOW.with(|c| c.borrow_mut().take());
                    BUFFER.with(|c| c.borrow_mut().take());
                }));
                false
            })
            .build(),
    );

    scroll_to_bottom_if_needed();

    Ok(())
}

/// Closes the prompt panel without clearing session state.
pub(crate) fn close() {
    threads::input_close();
    WINDOW.with(|c| {
        if let Some(win) = c.borrow_mut().take() {
            let _ = win.close(false);
        }
    });
    BUFFER.with(|c| {
        c.borrow_mut().take();
    });
}

/// Returns true if the prompt panel float is currently open.
pub(crate) fn is_open() -> bool {
    WINDOW.with(|c| c.borrow().as_ref().map(|w| w.is_valid()).unwrap_or(false))
}

/// Toggles the prompt panel open/closed for the given conversation.
///
/// If the panel is already open showing a different conversation,
/// it switches to the requested one instead of closing.
pub(crate) fn toggle(conversation_id: &str) -> nvim_oxi::Result<()> {
    let id = if conversation_id.is_empty() {
        DEFAULT_CONVERSATION
    } else {
        conversation_id
    };

    if is_open() {
        let same = CONVERSATION_ID.with(|c| *c.borrow() == id);
        if same {
            close();
            return Ok(());
        }
    }
    open(id)
}

fn open_input() {
    let _ = threads::open(
        "Prompt",
        Box::new(|text: String| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            send_message(text);
        }),
        Box::new(|| {}),
    );
}

fn send_message(text: String) {
    let conv_id = CONVERSATION_ID.with(|c| c.borrow().clone());

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let user_msg = threads::Message {
        role: Role::User,
        text: text.clone(),
        ts,
        revision_context: None,
    };

    let session_id = review::with_active(|r| {
        let conv = r
            .prompt_conversations
            .entry(conv_id.clone())
            .or_insert_with(|| review::PromptConversation {
                messages: Vec::new(),
                session_id: None,
            });
        conv.messages.push(user_msg);
        conv.session_id.clone()
    });

    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        if let Some(buf) = guard.as_mut() {
            let _ = render_message(buf, Role::User, &text);
        }
    });
    update_winbar();

    let _ = append_status("thinking...");

    let on_stream: crate::types::OnStream = Arc::new(move |chunk: &str| {
        let _ = append_streaming(chunk);
    });

    let callback: crate::types::OnComplete = Box::new(move |res| {
        let msg = res
            .error
            .as_ref()
            .map(|e| format!("[Error] {e}"))
            .unwrap_or_else(|| res.text.clone());

        if let Some(e) = res.error.as_ref() {
            backend::notify_if_missing_binary(e);
        }

        BUFFER.with(|c| {
            let mut guard = c.borrow_mut();
            if let Some(buf) = guard.as_mut() {
                let _ = replace_last_agent_message(buf, &msg);
            }
        });

        let agent_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        review::with_active(|r| {
            let conv = r
                .prompt_conversations
                .entry(conv_id.clone())
                .or_insert_with(|| review::PromptConversation {
                    messages: Vec::new(),
                    session_id: None,
                });
            conv.messages.push(threads::Message {
                role: Role::Agent,
                text: msg,
                ts: agent_ts,
                revision_context: None,
            });
            if !res.session_id.is_empty() {
                conv.session_id = Some(res.session_id);
            }
        });

        update_winbar();
    });

    if let Some(sid) = session_id.flatten() {
        backend::thread_reply(Some(&sid), &text, Some(on_stream), callback, None);
    } else {
        backend::send_prompt(&text, Some(on_stream), callback);
    }
}

fn render_message(buf: &mut Buffer, role: Role, text: &str) -> nvim_oxi::Result<()> {
    let (author, hl) = match role {
        Role::User => ("you", "ArbiterThreadUser"),
        Role::Agent => ("agent", "ArbiterThreadAgent"),
    };
    let author_line = format!("┊ {author}  {}", panel::format_now());

    let line_count = buf.line_count()?;

    let first_line_is_placeholder = if line_count == 1 {
        buf.get_lines(0..1, false)?
            .next()
            .map(|s| s.to_string_lossy().contains("Press Enter"))
            .unwrap_or(false)
    } else {
        false
    };

    let mut new_lines: Vec<String> = Vec::new();
    if !first_line_is_placeholder && line_count > 0 {
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
    if first_line_is_placeholder {
        buf.set_lines(0..1, false, refs)?;
    } else {
        buf.set_lines(line_count..line_count, false, refs)?;
    }
    api::set_option_value("modifiable", false, &buf_opts)?;

    let ns = api::create_namespace("arbiter-prompt-panel");
    let base = if first_line_is_placeholder {
        0
    } else {
        line_count
    };
    if !first_line_is_placeholder && line_count > 0 {
        let _ = buf.add_highlight(ns, "NonText", base, 0..);
    }
    let _ = buf.add_highlight(ns, hl, base + author_offset, 0..);

    scroll_to_bottom_if_needed();

    Ok(())
}

fn append_status(message: &str) -> nvim_oxi::Result<()> {
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

        let ns = api::create_namespace("arbiter-prompt-panel");
        let status_line = if existing_status {
            line_count - 1
        } else {
            line_count
        };
        let _ = buf.add_highlight(ns, "NonText", status_line, 0..);

        scroll_to_bottom_if_needed();
        Ok(())
    })
}

fn append_streaming(text: &str) -> nvim_oxi::Result<()> {
    BUFFER.with(|c| {
        let mut guard = c.borrow_mut();
        let Some(buf) = guard.as_mut() else {
            return Ok(());
        };
        panel::append_streaming_to_buf(buf, text, "arbiter-prompt-panel")?;
        scroll_to_bottom_if_needed();
        Ok(())
    })
}

fn replace_last_agent_message(buf: &mut Buffer, text: &str) -> nvim_oxi::Result<()> {
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

    scroll_to_bottom_if_needed();
    Ok(())
}

fn scroll_to_bottom_if_needed() {
    WINDOW.with(|w| {
        let guard = w.borrow();
        if let Some(win) = guard.as_ref() {
            BUFFER.with(|b| {
                if let Some(buf) = b.borrow().as_ref() {
                    if let Ok(cnt) = buf.line_count() {
                        let _ = win.clone().set_cursor(cnt, 0);
                    }
                }
            });
        }
    });
}

fn update_winbar() {
    let conv_id = CONVERSATION_ID.with(|c| c.borrow().clone());
    let msg_count = review::with_active(|r| {
        r.prompt_conversations
            .get(&conv_id)
            .map(|c| c.messages.len())
            .unwrap_or(0)
    })
    .unwrap_or(0);
    WINDOW.with(|c| {
        if let Some(win) = c.borrow().as_ref() {
            let winbar = format!(
                "%#ArbiterDiffFile# Prompt: {conv_id} %#Comment# {} message{}  <Enter> reply  q close",
                msg_count,
                if msg_count == 1 { "" } else { "s" },
            );
            let win_opts = OptionOpts::builder().win(win.clone()).build();
            let _ = api::set_option_value("winbar", winbar.as_str(), &win_opts);
        }
    });
}
