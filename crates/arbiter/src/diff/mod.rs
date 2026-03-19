//! Diff parsing and rendering.
//!
//! Parses unified diff text into hunks, renders to Neovim buffers
//! with thread summaries and highlighting. Uses `ThreadSummary` from
//! `types.rs` (not `Thread` from threads) for stream decoupling.

mod parse;
mod render;

pub(crate) use parse::{
    buf_line_to_source, build_hunk_patch, detect_hunk_changes, parse_hunks, source_to_buf_line,
    synthesize_untracked, Hunk,
};
pub(crate) use render::{
    apply_highlights, close_side_by_side, open_side_by_side, render, set_hunk_folds, win_exec,
};
