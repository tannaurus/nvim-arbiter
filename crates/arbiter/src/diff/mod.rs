//! Diff parsing and rendering.
//!
//! Parsing comes from `arbiter_core::diff`. Rendering uses Neovim buffers.

mod render;

pub(crate) use arbiter_core::diff::{
    buf_line_to_source, build_hunk_patch, detect_hunk_changes, parse_hunks, source_to_buf_line,
    synthesize_untracked, Hunk,
};
pub(crate) use render::{
    apply_highlights, close_side_by_side, open_side_by_side, render, set_hunk_folds, toggle_style,
    win_exec, RenderResult,
};
