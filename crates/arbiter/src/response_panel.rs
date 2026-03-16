//! Response panel and float for backend output.
//!
//! When a review workbench is open, uses a horizontal split at the bottom.
//! When no workbench, uses a centered floating window. Reused across calls;
//! streaming appends to the buffer. `q` closes the split/float only.

use crate::review;
use nvim_oxi::api::opts::{OptionOpts, SetKeymapOpts};
use nvim_oxi::api::types::{Mode, WindowBorder, WindowRelativeTo, WindowTitle};
use nvim_oxi::api::{self, Buffer, Window};
use std::cell::RefCell;

thread_local! {
    static PANEL_BUF: RefCell<Option<Buffer>> = const { RefCell::new(None) };
    static PANEL_WIN: RefCell<Option<Window>> = const { RefCell::new(None) };
    static FLOAT_BUF: RefCell<Option<Buffer>> = const { RefCell::new(None) };
    static FLOAT_WIN: RefCell<Option<Window>> = const { RefCell::new(None) };
}

const PANEL_HEIGHT: u32 = 10;
const FLOAT_WIDTH: u32 = 70;
const FLOAT_HEIGHT: u32 = 20;

/// Opens or reuses the response panel.
///
/// When a review workbench is active, creates a horizontal split at the bottom.
/// When no workbench, creates a centered float. Reuses existing buffer when open.
/// If already open, adds a separator line for the new response.
pub fn open_or_reuse(title: &str) -> nvim_oxi::Result<()> {
    if review::is_active() {
        if panel_is_open() {
            let _ = append(&format!("\n── {title} ──\n"));
            return Ok(());
        }
        review::with_active(|r| {
            let _ = api::set_current_tabpage(&r.tabpage);
            let _ = api::set_current_win(&r.diff_panel.win);
        });
        open_panel(title)
    } else {
        if float_is_open() {
            close_float();
        }
        open_float(title)
    }
}

fn open_panel(_title: &str) -> nvim_oxi::Result<()> {
    PANEL_WIN.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
    PANEL_BUF.with(|c| {
        c.borrow_mut().take();
    });

    api::command(&format!("botright {PANEL_HEIGHT}split"))?;
    let mut buf = api::create_buf(false, true)?;
    api::set_option_value(
        "buftype",
        "nofile",
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    api::set_option_value(
        "modifiable",
        true,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    buf.set_lines(0..0, false, ["── Agent Response ──"])?;

    let mut win = api::get_current_win();
    win.set_buf(&buf)?;

    let opts = SetKeymapOpts::builder()
        .callback(|_| close_panel())
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts);

    PANEL_BUF.with(|c| *c.borrow_mut() = Some(buf));
    PANEL_WIN.with(|c| *c.borrow_mut() = Some(win));

    Ok(())
}

fn open_float(title: &str) -> nvim_oxi::Result<()> {
    let mut buf = api::create_buf(false, true)?;
    api::set_option_value(
        "buftype",
        "nofile",
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    api::set_option_value(
        "modifiable",
        true,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    buf.set_lines(0..0, false, ["── Agent Response ──"])?;

    let cols = api::get_option_value::<i64>("columns", &OptionOpts::builder().build())?;
    let rows = api::get_option_value::<i64>("lines", &OptionOpts::builder().build())?;
    let width = FLOAT_WIDTH.min(cols as u32 - 6);
    let height = FLOAT_HEIGHT.min(rows as u32 - 6);
    let row = ((rows as f64) - (height as f64)) / 2.0;
    let col = ((cols as f64) - (width as f64)) / 2.0;

    let config = nvim_oxi::api::types::WindowConfig::builder()
        .relative(WindowRelativeTo::Editor)
        .width(width)
        .height(height)
        .row(row)
        .col(col)
        .border(WindowBorder::Rounded)
        .title(WindowTitle::SimpleString(title.to_string().into()))
        .build();

    let win = api::open_win(&buf, true, &config)?;

    let opts = SetKeymapOpts::builder()
        .callback(|_| close_float())
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts);

    FLOAT_BUF.with(|c| *c.borrow_mut() = Some(buf));
    FLOAT_WIN.with(|c| *c.borrow_mut() = Some(win));

    Ok(())
}

/// Appends text to the active response panel or float.
pub fn append(text: &str) -> nvim_oxi::Result<()> {
    let mut appended = false;

    PANEL_BUF.with(|c| {
        let mut guard = c.borrow_mut();
        if let Some(ref mut buf) = *guard {
            let _ = api::set_option_value(
                "modifiable",
                true,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            let lc = buf.line_count().unwrap_or(0);
            let lines: Vec<&str> = text.lines().collect();
            if !lines.is_empty() {
                let _ = buf.set_lines(lc..lc, false, lines);
            }
            let _ = api::set_option_value(
                "modifiable",
                false,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            appended = true;
        }
    });

    if !appended {
        FLOAT_BUF.with(|c| {
            let mut guard = c.borrow_mut();
            if let Some(ref mut buf) = *guard {
                let _ = api::set_option_value(
                    "modifiable",
                    true,
                    &OptionOpts::builder().buffer(buf.clone()).build(),
                );
                let lc = buf.line_count().unwrap_or(0);
                let lines: Vec<&str> = text.lines().collect();
                if !lines.is_empty() {
                    let _ = buf.set_lines(lc..lc, false, lines);
                }
                let _ = api::set_option_value(
                    "modifiable",
                    false,
                    &OptionOpts::builder().buffer(buf.clone()).build(),
                );
            }
        });
    }

    Ok(())
}

/// Appends streaming text to the last line of the active response.
pub fn append_streaming(text: &str) -> nvim_oxi::Result<()> {
    PANEL_BUF.with(|c| {
        let mut guard = c.borrow_mut();
        if let Some(ref mut buf) = *guard {
            let _ = api::set_option_value(
                "modifiable",
                true,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            let lc = buf.line_count().unwrap_or(1);
            let last_idx = lc.saturating_sub(1);
            let last = buf
                .get_lines(last_idx..=last_idx, false)?
                .next()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let combined = format!("{last}{text}");
            let refs: Vec<&str> = combined.split('\n').collect();
            let _ = buf.set_lines(last_idx..=last_idx, false, refs);
            let _ = api::set_option_value(
                "modifiable",
                false,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            return Ok(());
        }
        drop(guard);
        FLOAT_BUF.with(|c| {
            let mut guard = c.borrow_mut();
            if let Some(ref mut buf) = *guard {
                let _ = api::set_option_value(
                    "modifiable",
                    true,
                    &OptionOpts::builder().buffer(buf.clone()).build(),
                );
                let lc = buf.line_count().unwrap_or(1);
                let last_idx = lc.saturating_sub(1);
                let last = buf
                    .get_lines(last_idx..=last_idx, false)?
                    .next()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let combined = format!("{last}{text}");
                let refs: Vec<&str> = combined.split('\n').collect();
                let _ = buf.set_lines(last_idx..=last_idx, false, refs);
                let _ = api::set_option_value(
                    "modifiable",
                    false,
                    &OptionOpts::builder().buffer(buf.clone()).build(),
                );
            }
            Ok(())
        })
    })
}

fn close_panel() {
    PANEL_WIN.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
    PANEL_BUF.with(|c| {
        c.borrow_mut().take();
    });
}

fn close_float() {
    FLOAT_WIN.with(|c| {
        let mut opt = c.borrow_mut();
        if let Some(win) = opt.take() {
            let _ = win.close(false);
        }
    });
    FLOAT_BUF.with(|c| {
        c.borrow_mut().take();
    });
}

/// Returns true if the response panel (split) is open.
pub fn panel_is_open() -> bool {
    PANEL_WIN.with(|c| c.borrow().is_some())
}

/// Returns true if the response float is open.
pub fn float_is_open() -> bool {
    FLOAT_WIN.with(|c| c.borrow().is_some())
}
