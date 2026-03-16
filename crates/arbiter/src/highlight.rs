//! Highlight group registration.
//!
//! Registers custom highlight groups via `api::set_hl`.
//! Each group links to a built-in group for colorscheme inheritance.

use nvim_oxi::api::opts::SetHighlightOpts;
use nvim_oxi::api::{self};

/// Registers all arbiter highlight groups.
///
/// Each group links to a built-in Neovim group so they inherit
/// the user's colorscheme. Users can override with `nvim_set_hl()`.
pub fn setup() -> nvim_oxi::Result<()> {
    let groups: [(&str, &str); 14] = [
        ("ArbiterDiffAdd", "DiffAdd"),
        ("ArbiterDiffDelete", "DiffDelete"),
        ("ArbiterDiffChange", "DiffChange"),
        ("ArbiterDiffFile", "Title"),
        ("ArbiterThreadUser", "Comment"),
        ("ArbiterThreadAgent", "WarningMsg"),
        ("ArbiterThreadResolved", "NonText"),
        ("ArbiterStatusApproved", "DiagnosticOk"),
        ("ArbiterStatusChanges", "DiagnosticError"),
        ("ArbiterStatusPending", "NonText"),
        ("ArbiterHunkNew", "DiffAdd"),
        ("ArbiterHunkAccepted", "Comment"),
        ("ArbiterIndicatorUser", "DiagnosticHint"),
        ("ArbiterIndicatorAgent", "DiagnosticWarn"),
    ];

    for (name, link_to) in groups {
        let opts = SetHighlightOpts::builder().link(link_to).build();
        api::set_hl(0, name, &opts)?;
    }

    let _ = api::command("sign define ArbiterThreadOpen text=● texthl=DiagnosticWarn");
    let _ = api::command("sign define ArbiterThreadResolved text=✓ texthl=DiagnosticOk");

    Ok(())
}
