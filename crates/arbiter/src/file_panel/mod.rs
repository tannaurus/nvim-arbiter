//! File panel abstraction and implementations.
//!
//! The `FilePanel` trait defines how the review workbench interacts with its
//! file list panel. Each implementation lives in its own submodule.

mod builtin;
mod nvim_tree;

pub(crate) use builtin::BuiltinFilePanel;
pub(crate) use nvim_tree::NvimTreeFilePanel;

use nvim_oxi::api::{Buffer, Window};
use std::collections::HashMap;

use crate::types::{FileStatus, ReviewStatus};

/// Abstraction over the file list panel in the review workbench.
///
/// The review workbench needs a panel that:
/// - Displays changed files with review status
/// - Maps cursor positions to file/directory paths for selection
/// - Supports directory collapsing
/// - Provides window/buffer handles for layout and keymap registration
pub(crate) trait FilePanel {
    /// Re-render the panel with updated file list and thread counts.
    fn render(
        &mut self,
        files: &[(String, FileStatus, ReviewStatus)],
        open_thread_counts: &HashMap<String, usize>,
    ) -> nvim_oxi::Result<()>;

    /// File path at the given 1-based buffer line, if any.
    fn path_at_line(&self, line: usize) -> Option<String>;

    /// Directory path at the given 1-based buffer line, if any.
    fn dir_at_line(&self, line: usize) -> Option<String>;

    /// Toggle collapse state for a directory path.
    fn toggle_collapse(&mut self, dir: &str);

    /// Move the panel cursor to the line showing `path`.
    fn highlight_file(&mut self, path: &str);

    /// The panel's window handle.
    fn window(&self) -> &Window;

    /// Mutable reference to the panel's buffer, for keymap registration.
    fn buffer_mut(&mut self) -> &mut Buffer;

    /// Buffer handle integer, for identity checks and `bwipeout`.
    fn buf_handle(&self) -> i32;

    /// Called when the review workbench closes.
    ///
    /// Implementations can use this to tear down any runtime state
    /// (e.g. restoring filters on nvim-tree).
    fn cleanup(&mut self) {}

    /// Whether the caller should wipe this panel's buffer on close.
    ///
    /// Returns `false` for panels that manage their own buffer lifecycle
    /// (e.g. nvim-tree). Wiping an externally-managed buffer corrupts the
    /// plugin's internal state and causes hangs on re-open.
    fn should_wipe_buffer(&self) -> bool {
        true
    }
}
