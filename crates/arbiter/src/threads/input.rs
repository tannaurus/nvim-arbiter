//! Input panel for new comments and thread replies.
//!
//! Opens as a small bottom split. `<CR>` submits; `q` or `<Esc>` cancels.

use crate::config;
use nvim_oxi::api::opts::{OptionOpts, SetKeymapOpts};
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self, Buffer, Window};
use std::cell::RefCell;

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static BUFFER: RefCell<Option<Buffer>> = const { RefCell::new(None) };
}

const INPUT_HEIGHT: u32 = 5;

/// Callback invoked when the user submits (presses `<CR>`).
pub type OnSubmit = Box<dyn FnOnce(String) + Send>;

/// Callback invoked when the user cancels (presses `q` or `<Esc>`).
pub type OnCancel = Box<dyn FnOnce() + Send>;

/// Opens a small bottom split for entering text.
///
/// `title` is shown as the first line of the buffer.
/// `<CR>` invokes `on_submit` with the buffer content; `q` or `<Esc>` invokes `on_cancel`.
pub fn open(title: &str, on_submit: OnSubmit, on_cancel: OnCancel) -> nvim_oxi::Result<()> {
    if is_open() {
        close();
    }

    let mut buf = api::create_buf(false, false)?;

    let tw = &config::get().thread_window;
    let input_height = if tw.position.is_vertical() {
        INPUT_HEIGHT
    } else {
        INPUT_HEIGHT.min(tw.size / 3).max(3)
    };

    api::command(&format!("botright {input_height}split"))?;
    let mut win = api::get_current_win();
    win.set_buf(&buf)?;

    api::command("setlocal buftype=nofile")?;
    api::command("setlocal bufhidden=wipe")?;
    api::command("setlocal noswapfile")?;
    api::command("setlocal modifiable")?;

    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("number", false, &win_opts);
    let _ = api::set_option_value("relativenumber", false, &win_opts);
    let _ = api::set_option_value("signcolumn", "no", &win_opts);
    let _ = api::set_option_value("wrap", true, &win_opts);
    let _ = api::set_option_value("winfixheight", true, &win_opts);

    let header = format!("── {title} ── (Enter to submit, q to cancel)");
    buf.set_lines(0..0, false, [header.as_str(), ""])?;

    let ns = api::create_namespace("arbiter-input");
    let _ = buf.add_highlight(ns, "NonText", 0, 0..);

    let submit_cell = std::sync::Arc::new(std::sync::Mutex::new(Some(on_submit)));
    let cancel_cell = std::sync::Arc::new(std::sync::Mutex::new(Some(on_cancel)));

    let submit_cell_s = submit_cell.clone();
    let cancel_cell_s = cancel_cell.clone();
    let opts_submit = SetKeymapOpts::builder()
        .callback(move |_| {
            if let (Ok(mut submit_guard), Ok(mut cancel_guard)) =
                (submit_cell_s.lock(), cancel_cell_s.lock())
            {
                if let Some(f) = submit_guard.take() {
                    let _ = cancel_guard.take();
                    let content = BUFFER.with(|c| {
                        c.borrow()
                            .as_ref()
                            .and_then(|b| {
                                let lc = b.line_count().ok()?;
                                if lc <= 1 {
                                    return Some(String::new());
                                }
                                let lines = b.get_lines(1..lc, false).ok()?;
                                let vec: Vec<String> =
                                    lines.into_iter().map(|s| s.to_string()).collect();
                                Some(vec.join("\n").trim().to_string())
                            })
                            .unwrap_or_default()
                    });
                    close();
                    f(content);
                }
            }
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_submit);

    let opts_cancel = SetKeymapOpts::builder()
        .callback(move |_| {
            if let (Ok(mut submit_guard), Ok(mut cancel_guard)) =
                (submit_cell.lock(), cancel_cell.lock())
            {
                if let Some(f) = cancel_guard.take() {
                    let _ = submit_guard.take();
                    close();
                    f();
                }
            }
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts_cancel);
    let _ = buf.set_keymap(Mode::Normal, "<Esc>", "", &opts_cancel);

    WINDOW.with(|c| *c.borrow_mut() = Some(win.clone()));
    BUFFER.with(|c| *c.borrow_mut() = Some(buf));

    let _ = api::set_current_win(&win);
    let _ = win.set_cursor(2, 0);
    api::command("lua vim.schedule(function() vim.cmd('startinsert') end)")?;

    Ok(())
}

/// Opens the input panel for a specific file:line (e.g. for new comments).
///
/// Convenience wrapper that formats the title as "Comment at file:line".
pub fn open_for_line(
    file: &str,
    line: u32,
    on_submit: OnSubmit,
    on_cancel: OnCancel,
) -> nvim_oxi::Result<()> {
    open(&format!("Comment at {file}:{line}"), on_submit, on_cancel)
}

/// Closes the input panel and cleans up.
pub fn close() {
    WINDOW.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
    BUFFER.with(|c| {
        c.borrow_mut().take();
    });
}

/// Returns true if the input panel is open.
pub fn is_open() -> bool {
    WINDOW.with(|c| c.borrow().is_some())
}
