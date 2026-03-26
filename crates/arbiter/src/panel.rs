//! Shared rendering utilities for thread and prompt panels.

use crate::config;
use chrono::Local;
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::{self, Buffer};

pub(crate) const SEPARATOR: &str = "  ────────────────────────────────";
pub(crate) const STATUS_PREFIX: &str = "  ⏳ ";

/// Disables all syntax highlighting on a buffer: legacy syntax, filetype
/// detection, and treesitter. Call after `create_buf` and before the buffer
/// is displayed in a window.
pub(crate) fn disable_syntax(buf: &Buffer) {
    let opts = OptionOpts::builder().buffer(buf.clone()).build();
    let _ = api::set_option_value("filetype", "", &opts);
    let _ = api::set_option_value("syntax", "", &opts);
    let handle = buf.handle();
    let _ = api::command(&format!("lua pcall(vim.treesitter.stop, {handle})"));
}

pub(crate) fn format_ts(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    let fmt = &config::get().thread_window.date_format;
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_default()
}

pub(crate) fn format_now() -> String {
    let fmt = &config::get().thread_window.date_format;
    Local::now().format(fmt).to_string()
}

pub(crate) fn clear_status(buf: &mut Buffer) -> nvim_oxi::Result<usize> {
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

/// Appends streaming agent text into a panel buffer.
///
/// On the first chunk (when the last author line is not `agent`), inserts
/// a separator, an `agent` header with timestamp, then the content.
/// On subsequent chunks, appends to the existing agent content.
pub(crate) fn append_streaming_to_buf(
    buf: &mut Buffer,
    text: &str,
    ns_name: &str,
) -> nvim_oxi::Result<()> {
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

        let ns = api::create_namespace(ns_name);
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
            let remaining: Vec<String> = segments[1..].iter().map(|l| format!("  {l}")).collect();
            let refs: Vec<&str> = remaining.iter().map(|s| s.as_str()).collect();
            buf.set_lines(new_count..new_count, false, refs)?;
        }
    }

    api::set_option_value("modifiable", false, &buf_opts)?;

    Ok(())
}
