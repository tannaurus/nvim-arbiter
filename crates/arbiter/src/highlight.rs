//! Highlight group registration.
//!
//! Defines custom highlight groups with explicit GitHub-inspired colors.
//! All groups are registered with `default = true` so they apply out of
//! the box but can be overridden by users via `vim.api.nvim_set_hl()`.

use nvim_oxi::api::{self};

/// Registers all arbiter highlight groups.
///
/// Diff colors use GitHub's palette: green backgrounds for additions,
/// red backgrounds for deletions, blue for hunk headers. Non-diff
/// groups link to built-in Neovim groups for colorscheme inheritance.
///
/// All groups use `default = true`, so user overrides take precedence.
/// To customize, add to your Neovim config:
///
/// ```lua
/// vim.api.nvim_set_hl(0, "ArbiterDiffAdd", { bg = "#your_color", fg = "#text" })
/// ```
pub(crate) fn setup() -> nvim_oxi::Result<()> {
    let is_dark = is_dark_background();

    if is_dark {
        set_hl_default("ArbiterDiffAdd", Some("#0d4429"), Some("#aff5b4"), false)?;
        set_hl_default("ArbiterDiffDelete", Some("#67060c"), Some("#ffd7d5"), false)?;
        set_hl_default("ArbiterDiffChange", Some("#0c2d6b"), Some("#79c0ff"), false)?;
        set_hl_default(
            "ArbiterDiffHunkHeader",
            Some("#1a1e2e"),
            Some("#79c0ff"),
            true,
        )?;
        set_hl_default("ArbiterDiffContext", None, Some("#8b949e"), false)?;
        set_hl_default("ArbiterGutterAdd", None, Some("#3fb950"), false)?;
        set_hl_default("ArbiterGutterDelete", None, Some("#f85149"), false)?;
        set_hl_default("ArbiterGutterHunkHeader", None, Some("#79c0ff"), true)?;
        set_hl_default("ArbiterGutterContext", None, Some("#484f58"), false)?;
        set_hl_default("ArbiterSignApproved", None, Some("#3fb950"), true)?;
        set_hl_default("ArbiterSignPending", None, Some("#6e7681"), false)?;
    } else {
        set_hl_default("ArbiterDiffAdd", Some("#dafbe1"), Some("#116329"), false)?;
        set_hl_default("ArbiterDiffDelete", Some("#ffebe9"), Some("#82071e"), false)?;
        set_hl_default("ArbiterDiffChange", Some("#ddf4ff"), Some("#0550ae"), false)?;
        set_hl_default(
            "ArbiterDiffHunkHeader",
            Some("#ddf4ff"),
            Some("#0550ae"),
            true,
        )?;
        set_hl_default("ArbiterDiffContext", None, Some("#656d76"), false)?;
        set_hl_default("ArbiterGutterAdd", None, Some("#1a7f37"), false)?;
        set_hl_default("ArbiterGutterDelete", None, Some("#cf222e"), false)?;
        set_hl_default("ArbiterGutterHunkHeader", None, Some("#0550ae"), true)?;
        set_hl_default("ArbiterGutterContext", None, Some("#8c959f"), false)?;
        set_hl_default("ArbiterSignApproved", None, Some("#1a7f37"), true)?;
        set_hl_default("ArbiterSignPending", None, Some("#656d76"), false)?;
    }

    let linked: &[(&str, &str)] = &[
        ("ArbiterDiffFile", "Title"),
        ("ArbiterThreadUser", "Comment"),
        ("ArbiterThreadAgent", "WarningMsg"),
        ("ArbiterThreadResolved", "NonText"),
        ("ArbiterStatusApproved", "DiagnosticOk"),
        ("ArbiterStatusPending", "NonText"),
        ("ArbiterHunkNew", "DiffAdd"),
        ("ArbiterHunkAccepted", "Comment"),
        ("ArbiterIndicatorUser", "DiagnosticHint"),
        ("ArbiterIndicatorAgent", "DiagnosticWarn"),
        ("ArbiterRuleLearned", "DiagnosticOk"),
        ("ArbiterRevisionSummary", "DiagnosticInfo"),
        ("ArbiterRevisionFile", "NonText"),
        ("ArbiterSimilarHeader", "DiagnosticHint"),
        ("ArbiterSimilarRef", "NonText"),
        ("ArbiterHeading", "Title"),
        ("ArbiterBold", "Bold"),
        ("ArbiterInlineCode", "@markup.raw"),
        ("ArbiterCodeBlock", "CursorLine"),
    ];
    for (name, link_to) in linked {
        let _ = api::command(&format!("highlight default link {name} {link_to}"));
    }

    let _ = api::command("sign define ArbiterThreadOpen text=● texthl=DiagnosticWarn");
    let _ = api::command("sign define ArbiterThreadResolved text=✓ texthl=DiagnosticOk");

    Ok(())
}

fn is_dark_background() -> bool {
    api::get_option_value::<String>("background", &nvim_oxi::api::opts::OptionOpts::default())
        .map(|s| s == "dark")
        .unwrap_or(true)
}

fn set_hl_default(
    name: &str,
    bg: Option<&str>,
    fg: Option<&str>,
    bold: bool,
) -> nvim_oxi::Result<()> {
    let mut parts = vec!["highlight", "default", name];
    let bg_val;
    let fg_val;
    if let Some(b) = bg {
        bg_val = format!("guibg={b}");
        parts.push(&bg_val);
    }
    if let Some(f) = fg {
        fg_val = format!("guifg={f}");
        parts.push(&fg_val);
    }
    if bold {
        parts.push("gui=bold");
    }
    api::command(&parts.join(" "))?;
    Ok(())
}
