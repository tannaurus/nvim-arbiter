//! Inline thread indicators in normal editing buffers.
//!
//! When `config.inline_indicators` is true, places sign extmarks on
//! lines with open threads. `go` on an indicator line opens the thread
//! in a standalone float.

use crate::config;
use crate::review;
use crate::state;
use crate::threads;
use crate::types::{ThreadOrigin, ThreadStatus};
use nvim_oxi::api::opts::{CreateAutocmdOpts, SetKeymapOpts};
use nvim_oxi::api::types::Mode;
use nvim_oxi::api::{self};
use std::path::Path;

const NS_INLINE: &str = "arbiter-inline";

fn update_indicators() {
    if !config::get().inline_indicators {
        return;
    }
    let mut buf = api::get_current_buf();
    let buf_opts = nvim_oxi::api::opts::OptionOpts::builder()
        .buffer(buf.clone())
        .build();
    let buftype = api::get_option_value::<String>("buftype", &buf_opts).unwrap_or_default();
    if !buftype.is_empty() {
        return;
    }
    let path = match buf.get_name() {
        Ok(p) => p,
        Err(_) => return,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let rel_path = path
        .strip_prefix(&cwd)
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let rel_path = rel_path.trim_start_matches('/').to_string();

    let threads = if review::is_active() {
        review::with_active(|r| {
            threads::for_file(&r.threads, &rel_path)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    } else {
        let sd = config::get().state_dir();
        let ws_hash = state::workspace_hash(Path::new(&cwd));
        let ref_name = config::get()
            .review
            .default_ref
            .as_deref()
            .unwrap_or("")
            .to_string();
        state::load_threads(&sd, &ws_hash, &ref_name)
    };

    let rel_path = rel_path.trim_start_matches('/').to_string();
    let open_threads: Vec<_> = threads
        .into_iter()
        .filter(|t| t.file == rel_path && t.status == ThreadStatus::Open)
        .collect();

    let ns = api::create_namespace(NS_INLINE);
    let _ = buf.clear_namespace(ns, 0..usize::MAX);

    let go_key = config::get().keymaps.open_thread.clone();
    if open_threads.is_empty() {
        let _ = buf.del_keymap(Mode::Normal, &go_key);
        return;
    }

    let opts = nvim_oxi::api::opts::SetExtmarkOpts::builder();
    for t in &open_threads {
        let line = t.line.saturating_sub(1) as usize;
        let hl = match t.origin {
            ThreadOrigin::User => "ArbiterIndicatorUser",
            ThreadOrigin::Agent => "ArbiterIndicatorAgent",
        };
        let ext_opts = opts
            .clone()
            .hl_group(hl)
            .sign_text("▎")
            .sign_hl_group(hl)
            .build();
        let _ = buf.set_extmark(ns, line, 0, &ext_opts);
    }

    let open_threads_cell = std::sync::Arc::new(std::sync::Mutex::new(open_threads));
    let opts_go = SetKeymapOpts::builder()
        .callback(move |_| {
            let (row, _) = api::get_current_win().get_cursor().ok().unwrap_or((1, 0));
            let line = row as u32;
            if let Ok(threads) = open_threads_cell.lock() {
                if let Some(t) = threads.iter().find(|x| x.line == line) {
                    let on_reply: threads::OnReplyRequested = Box::new(|| {});
                    let _ = threads::window_open(
                        &t.id,
                        &t.file,
                        t.line,
                        &t.messages,
                        on_reply,
                        None,
                        None,
                    );
                }
            }
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &go_key, "", &opts_go);
}

/// Registers BufEnter and BufLeave autocmds for inline indicators when config enables it.
///
/// BufEnter updates indicators for the current buffer. BufLeave clears them.
pub(crate) fn setup() -> nvim_oxi::Result<()> {
    if !config::get().inline_indicators {
        return Ok(());
    }
    let opts_enter = CreateAutocmdOpts::builder()
        .callback(|_| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(update_indicators));
            Ok::<bool, nvim_oxi::Error>(false)
        })
        .build();
    api::create_autocmd(["BufEnter"], &opts_enter)?;
    let opts_leave = CreateAutocmdOpts::builder()
        .callback(|_| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(clear_indicators));
            Ok::<bool, nvim_oxi::Error>(false)
        })
        .build();
    api::create_autocmd(["BufLeave"], &opts_leave)?;
    Ok(())
}

/// Clears inline indicators from the current buffer.
fn clear_indicators() {
    if !config::get().inline_indicators {
        return;
    }
    let mut buf = api::get_current_buf();
    let ns = api::create_namespace(NS_INLINE);
    let _ = buf.clear_namespace(ns, 0..usize::MAX);
}
