//! arbiter: Neovim plugin for AI agent diff review workflow.
//!
//! Provides a structured diff viewer and thread-based review system
//! for collaborating with AI coding agents (Cursor, Claude Code).

mod activity;
mod backend;
mod commands;
mod config;
mod diff;
mod dispatch;
mod file_panel;
mod git;
mod highlight;
mod inline;
mod poll;
mod prompts;
mod response_panel;
mod review;
mod revision;
pub mod state;
pub mod threads;
pub mod types;

use nvim_oxi::api::opts::CreateAutocmdOpts;
use nvim_oxi::api::types::LogLevel;
use nvim_oxi::api::{self};
use nvim_oxi::{Dictionary, Object};
use serde::de::IntoDeserializer;
use serde::Deserialize;

/// Plugin setup. Called by the user via `require("arbiter").setup({...})`.
///
/// Deserializes the Lua table into Config, stores it, registers highlights,
/// and sets up the plugin. On config error, uses defaults and notifies.
pub fn setup(opts: Dictionary) -> nvim_oxi::Result<()> {
    let config = match <config::Config as Deserialize>::deserialize(
        Object::from(opts).into_deserializer(),
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = api::notify(
                &format!("config error: {e}, using defaults"),
                LogLevel::Warn,
                &Dictionary::default(),
            );
            config::Config::default()
        }
    };
    config::set_config(config.clone());
    dispatch::init()?;
    highlight::setup()?;
    backend::setup(backend::BackendConfig {
        backend: match config.backend {
            config::BackendKind::Cursor => "cursor".to_string(),
            config::BackendKind::Claude => "claude".to_string(),
        },
        model: config.model.clone(),
        workspace: config.workspace.clone(),
        extra_args: config.extra_args.clone(),
    });
    commands::register_commands()?;
    if config.inline_indicators {
        inline::setup()?;
    }
    api::create_autocmd(
        ["VimLeavePre"],
        &CreateAutocmdOpts::builder()
            .callback(|_| {
                backend::shutdown();
                Ok::<bool, nvim_oxi::Error>(false)
            })
            .build(),
    )?;
    let _ = api::notify("arbiter loaded", LogLevel::Info, &Dictionary::default());
    Ok(())
}

/// Statusline component. Returns empty string when no review is active.
///
/// With an active review, returns review progress counts
/// for inclusion in the user's statusline configuration.
pub fn statusline() -> String {
    let activity = activity::statusline_component();
    let review = review::with_active(|r| {
        let approved = r
            .files
            .iter()
            .filter(|(_, _, rs)| *rs == types::ReviewStatus::Approved)
            .count();
        format!("[REVIEW {}/{}]", approved, r.files.len())
    })
    .unwrap_or_default();
    if activity.is_empty() {
        review
    } else if review.is_empty() {
        activity
    } else {
        format!("{activity} {review}")
    }
}

#[nvim_oxi::plugin]
fn arbiter() -> nvim_oxi::Result<Dictionary> {
    Ok(Dictionary::from_iter([
        (
            "setup",
            Object::from(nvim_oxi::Function::<Dictionary, ()>::from_fn(setup)),
        ),
        (
            "statusline",
            Object::from(nvim_oxi::Function::<(), String>::from_fn(|_| statusline())),
        ),
    ]))
}
