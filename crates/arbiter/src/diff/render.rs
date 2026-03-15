//! Diff buffer rendering and highlighting.
//!
//! Builds buffer content from diff text and thread summaries,
//! applies highlights, and provides side-by-side diff mode.

use crate::types::{ThreadOrigin, ThreadStatus};
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::{self, Buffer, Window};
use std::collections::{HashMap, HashSet};

use super::parse::{self, Hunk};
use crate::types::ThreadSummary;

fn set_buffer_lines(buf: &mut Buffer, lines: &[String]) -> nvim_oxi::Result<()> {
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    api::set_option_value(
        "modifiable",
        true,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    let line_count = buf.line_count()?;
    buf.set_lines(0..line_count, false, refs)?;
    api::set_option_value(
        "modifiable",
        false,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    Ok(())
}

/// Renders diff text and thread summaries into a buffer.
///
/// Builds: file header line, thread summary lines (filtered by
/// `show_resolved`), then raw diff lines. Returns hunks with
/// `buf_start`/`buf_end` offset by injected lines, and a map
/// of thread_id to buffer line.
///
/// If `new_hunk_buf_starts` is provided, ArbiterHunkNew highlight is applied
/// to those hunk header lines.
pub fn render(
    buf: &mut Buffer,
    diff_text: &str,
    summaries: &[ThreadSummary],
    file_path: &str,
    show_resolved: bool,
    new_hunk_buf_starts: Option<&HashSet<usize>>,
) -> nvim_oxi::Result<(Vec<Hunk>, HashMap<String, usize>)> {
    let hunks = parse::parse_hunks(diff_text);
    let diff_lines: Vec<String> = diff_text.lines().map(|s| s.to_string()).collect();

    let header = format!("── {} ({}) ──", file_path, summaries.len());
    let visible: Vec<&ThreadSummary> = summaries
        .iter()
        .filter(|s| show_resolved || s.status != ThreadStatus::Resolved)
        .collect();

    let mut all_lines: Vec<String> = Vec::with_capacity(1 + visible.len() + diff_lines.len());
    all_lines.push(header.clone());
    let mut thread_buf_lines = HashMap::new();
    for (i, s) in visible.iter().enumerate() {
        let summary_line = format!(
            " [{}] :{}  {} [{}]",
            s.origin,
            s.line,
            s.preview.chars().take(40).collect::<String>(),
            s.status
        );
        let buf_line = 1 + i;
        thread_buf_lines.insert(s.id.clone(), buf_line);
        all_lines.push(summary_line);
    }

    let base_offset = 1 + visible.len();
    let mut adjusted_hunks = Vec::with_capacity(hunks.len());
    let mut separator_count = 0;
    for (idx, h) in hunks.iter().enumerate() {
        if idx > 0 {
            let prev = &hunks[idx - 1];
            let gap_start = prev.buf_end + 1;
            let gap_end = h.buf_start;
            for i in gap_start..gap_end {
                if let Some(l) = diff_lines.get(i) {
                    all_lines.push(l.clone());
                }
            }
            all_lines.push(String::new());
            separator_count += 1;
        } else {
            for i in 0..h.buf_start {
                if let Some(l) = diff_lines.get(i) {
                    all_lines.push(l.clone());
                }
            }
        }
        for i in h.buf_start..=h.buf_end {
            if let Some(l) = diff_lines.get(i) {
                all_lines.push(l.clone());
            }
        }
        adjusted_hunks.push(Hunk {
            buf_start: h.buf_start + base_offset + separator_count,
            buf_end: h.buf_end + base_offset + separator_count,
            old_start: h.old_start,
            old_count: h.old_count,
            new_start: h.new_start,
            new_count: h.new_count,
            header: h.header.clone(),
            content_hash: h.content_hash.clone(),
        });
    }
    if let Some(last) = hunks.last() {
        for i in (last.buf_end + 1)..diff_lines.len() {
            if let Some(l) = diff_lines.get(i) {
                all_lines.push(l.clone());
            }
        }
    } else {
        all_lines.extend(diff_lines.clone());
    }

    set_buffer_lines(buf, &all_lines)?;

    apply_highlights(
        buf,
        &adjusted_hunks,
        &visible,
        &all_lines,
        new_hunk_buf_starts,
    )?;

    Ok((adjusted_hunks, thread_buf_lines))
}

/// Applies syntax highlighting to the diff buffer.
///
/// If `new_hunk_buf_starts` contains a hunk's buf_start, also adds ArbiterHunkNew.
pub fn apply_highlights(
    buf: &mut Buffer,
    hunks: &[Hunk],
    summaries: &[&ThreadSummary],
    lines: &[String],
    new_hunk_buf_starts: Option<&HashSet<usize>>,
) -> nvim_oxi::Result<()> {
    let ns = api::create_namespace("arbiter-diff");
    let _ = buf.clear_namespace(ns, 0..usize::MAX);

    let mut line_idx = 0;
    buf.add_highlight(ns, "ArbiterDiffFile", line_idx, 0..)?;
    line_idx += 1;

    for s in summaries {
        let hl = match s.origin {
            ThreadOrigin::User => "ArbiterThreadUser",
            ThreadOrigin::Agent => "ArbiterThreadAgent",
        };
        if s.status == ThreadStatus::Resolved {
            buf.add_highlight(ns, "ArbiterThreadResolved", line_idx, 0..)?;
        } else {
            buf.add_highlight(ns, hl, line_idx, 0..)?;
        }
        line_idx += 1;
    }

    let offset = line_idx;
    for h in hunks {
        let is_new = new_hunk_buf_starts
            .map(|s| s.contains(&h.buf_start))
            .unwrap_or(false);
        if is_new {
            buf.add_highlight(ns, "ArbiterHunkNew", h.buf_start, 0..)?;
        }
        buf.add_highlight(ns, "ArbiterDiffChange", h.buf_start, 0..)?;
        for i in (h.buf_start + 1)..=h.buf_end {
            if i >= offset && i < lines.len() {
                let s = &lines[i];
                if s.starts_with('+') && !s.starts_with("+++") {
                    buf.add_highlight(ns, "ArbiterDiffAdd", i, 0..)?;
                } else if s.starts_with('-') && !s.starts_with("---") {
                    buf.add_highlight(ns, "ArbiterDiffDelete", i, 0..)?;
                } else if s.starts_with("@@ ") {
                    buf.add_highlight(ns, "ArbiterDiffChange", i, 0..)?;
                }
            }
        }
    }

    Ok(())
}

/// Opens two buffers (ref and working tree) in side-by-side diff mode.
///
/// Opens a new tabpage with two vertical splits. Left is the reference
/// version, right is the working tree version. Both get the file's
/// filetype for syntax highlighting, and buffer names indicate which
/// side is which. Closing both windows closes the tabpage and returns
/// to the review workbench.
pub fn open_side_by_side(
    ref_content: &str,
    working_content: &str,
    file_path: &str,
) -> nvim_oxi::Result<(Buffer, Window, Buffer, Window)> {
    let mut left = api::create_buf(false, true)?;
    let mut right = api::create_buf(false, true)?;

    let left_lines: Vec<String> = ref_content.lines().map(|s| s.to_string()).collect();
    let right_lines: Vec<String> = working_content.lines().map(|s| s.to_string()).collect();

    set_buffer_lines(&mut left, &left_lines)?;
    set_buffer_lines(&mut right, &right_lines)?;

    api::set_option_value(
        "buftype",
        "nofile",
        &OptionOpts::builder().buffer(left.clone()).build(),
    )?;
    api::set_option_value(
        "modifiable",
        false,
        &OptionOpts::builder().buffer(left.clone()).build(),
    )?;
    left.set_name(format!("[ref] {file_path}"))?;
    api::set_option_value(
        "buftype",
        "nofile",
        &OptionOpts::builder().buffer(right.clone()).build(),
    )?;
    api::set_option_value(
        "modifiable",
        false,
        &OptionOpts::builder().buffer(right.clone()).build(),
    )?;
    right.set_name(format!("[working] {file_path}"))?;

    let ext = file_path.rsplit('.').next().unwrap_or("");
    let ft = match ext {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "lua" => "lua",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "sh",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "java" => "java",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "ex" | "exs" => "elixir",
        "zig" => "zig",
        _ => "",
    };
    if !ft.is_empty() {
        let _ = api::set_option_value(
            "filetype",
            ft,
            &OptionOpts::builder().buffer(left.clone()).build(),
        );
        let _ = api::set_option_value(
            "filetype",
            ft,
            &OptionOpts::builder().buffer(right.clone()).build(),
        );
    }

    api::command("tabnew")?;
    let mut left_win = api::get_current_win();
    left_win.set_buf(&left)?;
    api::command("diffthis")?;

    api::command("vsplit")?;
    api::command("wincmd l")?;
    let mut right_win = api::get_current_win();
    right_win.set_buf(&right)?;
    api::command("diffthis")?;

    Ok((left, left_win, right, right_win))
}

/// Closes side-by-side diff tab.
///
/// Disables diff mode in both windows, then closes them. Since the
/// side-by-side opens in its own tabpage, closing both windows
/// automatically returns to the review workbench tab.
pub fn close_side_by_side(
    _left_buf: &Buffer,
    left_win: &Window,
    _right_buf: &Buffer,
    right_win: &Window,
) -> nvim_oxi::Result<()> {
    let _ = api::set_current_win(left_win);
    let _ = api::command("diffoff");
    let _ = api::set_current_win(right_win);
    let _ = api::command("diffoff");
    let _ = left_win.clone().close(false);
    let _ = right_win.clone().close(false);
    Ok(())
}

/// Creates manual folds for each hunk in the diff buffer.
///
/// Sets foldmethod=manual and creates one fold per hunk. When `file_approved`
/// is true, hunks start folded. Fold text shows line count and "approved" when
/// the file is approved. Caller should pass the diff panel window so fold
/// commands run in the correct buffer context.
pub fn set_hunk_folds(
    buf: &mut Buffer,
    win: &Window,
    hunks: &[Hunk],
    file_approved: bool,
) -> nvim_oxi::Result<()> {
    let win_opts = OptionOpts::builder().win(win.clone()).build();
    api::set_option_value("foldmethod", "manual", &win_opts)?;
    api::set_option_value("foldenable", true, &win_opts)?;
    buf.set_var("agent_file_approved", file_approved)?;
    api::set_option_value(
        "foldtext",
        "v:folddashes.(v:foldend-v:foldstart+1).' lines'.(get(b:,'agent_file_approved',0)?' [approved]':'')",
        &win_opts,
    )?;

    let wid = win.handle();
    let line_count = buf.line_count().unwrap_or(0) as i64;

    win_exec(wid, "normal! zE");
    for h in hunks {
        let start = (h.buf_start + 1) as i64;
        let end = (h.buf_end + 1) as i64;
        if start >= 1 && end <= line_count && start <= end {
            win_exec(wid, &format!("{start},{end}fold"));
        }
    }
    if file_approved {
        win_exec(wid, "normal! zM");
    } else {
        win_exec(wid, "normal! zR");
    }
    Ok(())
}

fn win_exec(wid: i32, cmd: &str) {
    let escaped = cmd.replace('\'', "''");
    let _ = api::command(&format!("call win_execute({wid}, '{escaped}')"));
}
