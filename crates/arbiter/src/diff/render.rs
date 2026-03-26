//! Diff buffer rendering and highlighting.
//!
//! Builds buffer content from diff text and thread summaries,
//! applies highlights, and provides side-by-side diff mode.

use crate::config::{self, DiffStyle};
use crate::types::{ThreadOrigin, ThreadStatus};
use nvim_oxi::api::opts::{OptionOpts, SetExtmarkOpts};
use nvim_oxi::api::types::ExtmarkHlMode;
use nvim_oxi::api::{self, Buffer, Window};
use std::collections::{HashMap, HashSet};

use crate::types::ThreadSummary;
use arbiter_core::diff::{self as parse, Hunk};

fn filetype_for_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
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
    }
}

/// Strips the single-character diff prefix (`+`, `-`, or ` `) from a line,
/// leaving hunk headers (`@@`) and file markers (`---`/`+++`) intact.
fn strip_diff_prefix(line: &str) -> String {
    if line.starts_with("@@")
        || line.starts_with("---")
        || line.starts_with("+++")
        || line.is_empty()
    {
        return line.to_string();
    }
    let first = line.as_bytes().first().copied().unwrap_or(0);
    if first == b'+' || first == b'-' || first == b' ' {
        line[1..].to_string()
    } else {
        line.to_string()
    }
}

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

/// Result of rendering a diff buffer.
pub(crate) struct RenderResult {
    /// Hunks with buf_start/buf_end offset by injected header/summary lines.
    pub hunks: Vec<Hunk>,
    /// Map of thread_id to the buffer line showing that thread's summary.
    pub thread_buf_lines: HashMap<String, usize>,
    /// All lines written to the buffer (header + summaries + diff).
    pub lines: Vec<String>,
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
pub(crate) fn render(
    buf: &mut Buffer,
    diff_text: &str,
    summaries: &[ThreadSummary],
    file_path: &str,
    show_resolved: bool,
    new_hunk_buf_starts: Option<&HashSet<usize>>,
    accepted_hashes: &HashSet<String>,
) -> nvim_oxi::Result<RenderResult> {
    let hunks = parse::parse_hunks(diff_text);
    let diff_lines: Vec<String> = diff_text.lines().map(|s| s.to_string()).collect();

    let header = format!("── {} ({}) ──", file_path, summaries.len());
    let visible: Vec<&ThreadSummary> = summaries
        .iter()
        .filter(|s| show_resolved || s.status != ThreadStatus::Resolved)
        .collect();

    let mut all_lines: Vec<String> = Vec::with_capacity(1 + visible.len() + diff_lines.len());
    all_lines.push(header);
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
        all_lines.extend(diff_lines);
    }

    let style = config::get().review.diff_style;

    if style == DiffStyle::Signs {
        let display_lines: Vec<String> = all_lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                if i >= base_offset {
                    strip_diff_prefix(line)
                } else {
                    line.clone()
                }
            })
            .collect();
        set_buffer_lines(buf, &display_lines)?;

        let ft = filetype_for_path(file_path);
        if !ft.is_empty() {
            let buf_nr = buf.handle();
            let _ = api::command(&format!(
                "lua pcall(vim.treesitter.stop, {buf_nr}); vim.bo[{buf_nr}].filetype = '{ft}'"
            ));
        }
    } else {
        set_buffer_lines(buf, &all_lines)?;
        let _ = api::set_option_value(
            "filetype",
            "diff",
            &OptionOpts::builder().buffer(buf.clone()).build(),
        );
    }

    let accepted_buf_starts: HashSet<usize> = adjusted_hunks
        .iter()
        .filter(|h| accepted_hashes.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect();

    apply_highlights(
        buf,
        &adjusted_hunks,
        &visible,
        &all_lines,
        new_hunk_buf_starts,
        &accepted_buf_starts,
    )?;

    Ok(RenderResult {
        hunks: adjusted_hunks,
        thread_buf_lines,
        lines: all_lines,
    })
}

/// Applies syntax highlighting to the diff buffer.
///
/// When `diff_style` is `Full`, applies full-line background colors (the default).
/// When `Signs`, places colored gutter signs while preserving syntax highlighting.
///
/// If `new_hunk_buf_starts` contains a hunk's buf_start, also adds ArbiterHunkNew.
/// Accepted hunks (buf_start in `accepted_buf_starts`) get ArbiterHunkAccepted on all lines.
pub(crate) fn apply_highlights(
    buf: &mut Buffer,
    hunks: &[Hunk],
    summaries: &[&ThreadSummary],
    lines: &[String],
    new_hunk_buf_starts: Option<&HashSet<usize>>,
    accepted_buf_starts: &HashSet<usize>,
) -> nvim_oxi::Result<()> {
    let ns = api::create_namespace("arbiter-diff");
    let _ = buf.clear_namespace(ns, 0..usize::MAX);

    let style = config::get().review.diff_style;

    let mut line_idx = 0;
    let replace_opts = |hl: &str| {
        SetExtmarkOpts::builder()
            .end_col(0)
            .end_row(line_idx + 1)
            .hl_group(hl)
            .hl_mode(ExtmarkHlMode::Replace)
            .build()
    };
    let _ = buf.set_extmark(ns, line_idx, 0, &replace_opts("ArbiterDiffFile"));
    line_idx += 1;

    for s in summaries {
        let hl = if s.status == ThreadStatus::Resolved {
            "ArbiterThreadResolved"
        } else {
            match s.origin {
                ThreadOrigin::User => "ArbiterThreadUser",
                ThreadOrigin::Agent => "ArbiterThreadAgent",
            }
        };
        let opts = SetExtmarkOpts::builder()
            .end_col(0)
            .end_row(line_idx + 1)
            .hl_group(hl)
            .hl_mode(ExtmarkHlMode::Replace)
            .build();
        let _ = buf.set_extmark(ns, line_idx, 0, &opts);
        line_idx += 1;
    }

    let offset = line_idx;
    match style {
        DiffStyle::Full => apply_full_highlights(
            buf,
            ns,
            hunks,
            lines,
            offset,
            new_hunk_buf_starts,
            accepted_buf_starts,
        ),
        DiffStyle::Signs => apply_sign_highlights(
            buf,
            ns,
            hunks,
            lines,
            offset,
            new_hunk_buf_starts,
            accepted_buf_starts,
        ),
    }
}

fn apply_full_highlights(
    buf: &mut Buffer,
    ns: u32,
    hunks: &[Hunk],
    lines: &[String],
    offset: usize,
    new_hunk_buf_starts: Option<&HashSet<usize>>,
    accepted_buf_starts: &HashSet<usize>,
) -> nvim_oxi::Result<()> {
    for h in hunks {
        let is_accepted = accepted_buf_starts.contains(&h.buf_start);
        if is_accepted {
            for i in h.buf_start..=h.buf_end {
                buf.add_highlight(ns, "ArbiterHunkAccepted", i, 0..)?;
            }
            continue;
        }
        let is_new = new_hunk_buf_starts
            .map(|s| s.contains(&h.buf_start))
            .unwrap_or(false);
        if is_new {
            buf.add_highlight(ns, "ArbiterHunkNew", h.buf_start, 0..)?;
        }
        buf.add_highlight(ns, "ArbiterDiffHunkHeader", h.buf_start, 0..)?;
        for i in (h.buf_start + 1)..=h.buf_end {
            if i >= offset && i < lines.len() {
                let s = &lines[i];
                if s.starts_with('+') && !s.starts_with("+++") {
                    buf.add_highlight(ns, "ArbiterDiffAdd", i, 0..)?;
                } else if s.starts_with('-') && !s.starts_with("---") {
                    buf.add_highlight(ns, "ArbiterDiffDelete", i, 0..)?;
                } else if s.starts_with("@@ ") {
                    buf.add_highlight(ns, "ArbiterDiffHunkHeader", i, 0..)?;
                } else if s.starts_with(' ') {
                    buf.add_highlight(ns, "ArbiterDiffContext", i, 0..)?;
                }
            }
        }
    }
    Ok(())
}

fn apply_sign_highlights(
    buf: &mut Buffer,
    ns: u32,
    hunks: &[Hunk],
    lines: &[String],
    offset: usize,
    new_hunk_buf_starts: Option<&HashSet<usize>>,
    accepted_buf_starts: &HashSet<usize>,
) -> nvim_oxi::Result<()> {
    let ext_builder = nvim_oxi::api::opts::SetExtmarkOpts::builder();

    for h in hunks {
        let is_accepted = accepted_buf_starts.contains(&h.buf_start);
        if is_accepted {
            for i in h.buf_start..=h.buf_end {
                buf.add_highlight(ns, "ArbiterHunkAccepted", i, 0..)?;
            }
            continue;
        }
        let is_new = new_hunk_buf_starts
            .map(|s| s.contains(&h.buf_start))
            .unwrap_or(false);
        if is_new {
            buf.add_highlight(ns, "ArbiterHunkNew", h.buf_start, 0..)?;
        }

        let header_opts = ext_builder
            .clone()
            .sign_text("┃")
            .sign_hl_group("ArbiterGutterHunkHeader")
            .number_hl_group("ArbiterGutterHunkHeader")
            .build();
        let _ = buf.set_extmark(ns, h.buf_start, 0, &header_opts);
        buf.add_highlight(ns, "ArbiterDiffHunkHeader", h.buf_start, 0..)?;

        for i in (h.buf_start + 1)..=h.buf_end {
            if i >= offset && i < lines.len() {
                let s = &lines[i];
                let (sign_hl, sign_char) = if s.starts_with('+') && !s.starts_with("+++") {
                    ("ArbiterGutterAdd", "▌")
                } else if s.starts_with('-') && !s.starts_with("---") {
                    ("ArbiterGutterDelete", "▌")
                } else if s.starts_with("@@ ") {
                    ("ArbiterGutterHunkHeader", "┃")
                } else if s.starts_with(' ') {
                    ("ArbiterGutterContext", "│")
                } else {
                    continue;
                };
                let opts = ext_builder
                    .clone()
                    .sign_text(sign_char)
                    .sign_hl_group(sign_hl)
                    .number_hl_group(sign_hl)
                    .build();
                let _ = buf.set_extmark(ns, i, 0, &opts);
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
pub(crate) fn open_side_by_side(
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

    let ft = filetype_for_path(file_path);
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
pub(crate) fn close_side_by_side(
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
/// is true, all hunks start folded. Accepted hunks (buf_start in
/// `accepted_buf_starts`) are always folded and get "[accepted]" in their fold
/// text via an inline foldtext expression.
pub(crate) fn set_hunk_folds(
    buf: &mut Buffer,
    win: &Window,
    hunks: &[Hunk],
    file_approved: bool,
    accepted_buf_starts: &HashSet<usize>,
) -> nvim_oxi::Result<()> {
    let win_opts = OptionOpts::builder().win(win.clone()).build();
    api::set_option_value("foldmethod", "manual", &win_opts)?;
    api::set_option_value("foldenable", true, &win_opts)?;
    buf.set_var("agent_file_approved", file_approved)?;

    let mut staged_dict = nvim_oxi::Dictionary::new();
    for h in hunks {
        if accepted_buf_starts.contains(&h.buf_start) {
            let key = nvim_oxi::String::from(format!("{}", h.buf_start + 1));
            staged_dict.insert(key, nvim_oxi::Object::from(true));
        }
    }
    buf.set_var("arbiter_accepted_folds", staged_dict)?;
    api::set_option_value(
        "foldtext",
        concat!(
            "v:folddashes.(v:foldend-v:foldstart+1).' lines'",
            ".(has_key(get(b:,'arbiter_accepted_folds',{}),string(v:foldstart))",
            "?' [accepted]'",
            ":(get(b:,'agent_file_approved',0)?' [approved]':''))",
        ),
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
        for h in hunks {
            if accepted_buf_starts.contains(&h.buf_start) {
                let start = (h.buf_start + 1) as i64;
                win_exec(wid, &format!("{start}foldclose"));
            }
        }
    }
    Ok(())
}

pub(crate) fn win_exec(wid: i32, cmd: &str) {
    let escaped = cmd.replace('\'', "''");
    let _ = api::command(&format!("call win_execute({wid}, '{escaped}')"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetype_for_known_extensions() {
        assert_eq!(filetype_for_path("src/main.rs"), "rust");
        assert_eq!(filetype_for_path("app.tsx"), "typescript");
        assert_eq!(filetype_for_path("lib.py"), "python");
        assert_eq!(filetype_for_path("go.mod"), "");
        assert_eq!(filetype_for_path("README.md"), "markdown");
        assert_eq!(filetype_for_path("config.yaml"), "yaml");
        assert_eq!(filetype_for_path("config.yml"), "yaml");
        assert_eq!(filetype_for_path("Makefile"), "");
        assert_eq!(filetype_for_path("script.sh"), "sh");
        assert_eq!(filetype_for_path("script.zsh"), "sh");
        assert_eq!(filetype_for_path("main.go"), "go");
        assert_eq!(filetype_for_path("file.toml"), "toml");
        assert_eq!(filetype_for_path("file.json"), "json");
        assert_eq!(filetype_for_path("file.html"), "html");
        assert_eq!(filetype_for_path("file.css"), "css");
        assert_eq!(filetype_for_path("file.java"), "java");
        assert_eq!(filetype_for_path("file.swift"), "swift");
        assert_eq!(filetype_for_path("file.zig"), "zig");
    }

    #[test]
    fn filetype_for_dotfiles_and_no_extension() {
        assert_eq!(filetype_for_path(".gitignore"), "");
        assert_eq!(filetype_for_path("Dockerfile"), "");
        assert_eq!(filetype_for_path(""), "");
    }

    #[test]
    fn filetype_for_deep_paths() {
        assert_eq!(filetype_for_path("a/b/c/d.rs"), "rust");
        assert_eq!(filetype_for_path("src/components/Button.tsx"), "typescript");
    }

    #[test]
    fn strip_diff_prefix_additions() {
        assert_eq!(strip_diff_prefix("+added line"), "added line");
        assert_eq!(strip_diff_prefix("+"), "");
    }

    #[test]
    fn strip_diff_prefix_deletions() {
        assert_eq!(strip_diff_prefix("-removed line"), "removed line");
        assert_eq!(strip_diff_prefix("-"), "");
    }

    #[test]
    fn strip_diff_prefix_context() {
        assert_eq!(strip_diff_prefix(" context line"), "context line");
        assert_eq!(strip_diff_prefix(" "), "");
    }

    #[test]
    fn strip_diff_prefix_preserves_hunk_headers() {
        assert_eq!(
            strip_diff_prefix("@@ -1,3 +1,4 @@ fn main"),
            "@@ -1,3 +1,4 @@ fn main"
        );
    }

    #[test]
    fn strip_diff_prefix_preserves_file_markers() {
        assert_eq!(strip_diff_prefix("--- a/foo.rs"), "--- a/foo.rs");
        assert_eq!(strip_diff_prefix("+++ b/foo.rs"), "+++ b/foo.rs");
        assert_eq!(strip_diff_prefix("--- /dev/null"), "--- /dev/null");
        assert_eq!(strip_diff_prefix("+++ /dev/null"), "+++ /dev/null");
    }

    #[test]
    fn strip_diff_prefix_preserves_empty_lines() {
        assert_eq!(strip_diff_prefix(""), "");
    }

    #[test]
    fn strip_diff_prefix_preserves_non_diff_content() {
        assert_eq!(
            strip_diff_prefix("diff --git a/f b/f"),
            "diff --git a/f b/f"
        );
        assert_eq!(
            strip_diff_prefix("index abc..def 100644"),
            "index abc..def 100644"
        );
    }

    #[test]
    fn strip_diff_prefix_content_with_markers() {
        assert_eq!(strip_diff_prefix("+- list item"), "- list item");
        assert_eq!(strip_diff_prefix(" - list item"), "- list item");
        assert_eq!(strip_diff_prefix("++ heading"), "+ heading");
    }
}
