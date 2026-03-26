//! User command registration and agent mode gating.
//!
//! Registers global and gated commands. Gated commands require an active
//! review and show a notification when none is open.

mod self_review;

use crate::backend;
use crate::config;
use crate::git;
use crate::prompt_panel;
use crate::prompts;
use crate::response_panel;
use crate::review;
use crate::rules;
use crate::state;
use crate::threads;
use crate::types::{ThreadOrigin, ThreadStatus};
use nvim_oxi::api::opts::CreateAutocmdOpts;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::opts::SetKeymapOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, LogLevel, Mode};
use nvim_oxi::api::types::{WindowBorder, WindowRelativeTo, WindowTitle};
use nvim_oxi::api::{self, Buffer};
use nvim_oxi::Dictionary;
use std::cell::RefCell;
use std::path::Path;

thread_local! {
    static RESUMED_SESSION: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_resumed_session(id: Option<String>) {
    RESUMED_SESSION.with(|c| *c.borrow_mut() = id);
}

fn get_resumed_session() -> Option<String> {
    RESUMED_SESSION.with(|c| c.borrow().clone())
}

/// Runs a closure with the active review. If no review is active,
/// shows a notification and returns without invoking the closure.
fn with_review_cmd(f: impl FnOnce(&mut review::Review)) {
    if review::with_active(f).is_none() {
        let _ = api::notify(
            "No active review. Run :Arbiter first.",
            LogLevel::Warn,
            &Dictionary::default(),
        );
    }
}

/// Registers all user commands. Called from setup().
pub(crate) fn register_commands() -> nvim_oxi::Result<()> {
    api::create_user_command(
        "Arbiter",
        |_args: CommandArgs| {
            let _ = review::open(None);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterCompare",
        |args: CommandArgs| {
            let ref_name = args
                .fargs
                .first()
                .cloned()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let cwd = std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| ".".to_string());
                    config::get().default_ref_for(&cwd).map(String::from)
                });
            if let Some(r) = ref_name {
                let _ = review::open(Some(&r));
            } else {
                api::err_writeln(
                    "[arbiter] No ref specified. Pass one as an argument (:ArbiterCompare main) or set review.default_ref in your config.",
                );
            }
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::ZeroOrOne)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterPrompt",
        |args: CommandArgs| {
            let conv_id = args.fargs.join(" ");
            with_review_cmd(|_r| {
                let _ = prompt_panel::toggle(&conv_id);
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Any)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterSend",
        |args: CommandArgs| {
            let prompt = args.fargs.join(" ");
            if prompt.trim().is_empty() {
                let _ = api::notify(
                    "ArbiterSend requires a prompt",
                    LogLevel::Warn,
                    &Dictionary::default(),
                );
                return;
            }
            let _ = response_panel::open_or_reuse("Agent Response");
            let _ = response_panel::append("Waiting for agent...\n");
            let on_stream = std::sync::Arc::new(|text: &str| {
                let _ = response_panel::append_streaming(text);
            });
            backend::send_prompt(
                &prompt,
                Some(on_stream),
                Box::new(|res| {
                    if let Some(e) = res.error.as_ref() {
                        let _ = response_panel::append(&format!("\nError: {e}"));
                    }
                }),
            );
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::OneOrMore)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterContinue",
        |args: CommandArgs| {
            let prompt = args.fargs.join(" ");
            let _ = response_panel::open_or_reuse("Agent Response");
            let _ = response_panel::append("Waiting for agent...\n");
            let session_id = get_resumed_session();
            let on_stream = std::sync::Arc::new(|text: &str| {
                let _ = response_panel::append_streaming(text);
            });
            if let Some(sid) = session_id {
                backend::thread_reply(
                    Some(&sid),
                    prompt.trim(),
                    Some(on_stream),
                    Box::new(|res| {
                        if let Some(e) = res.error.as_ref() {
                            let _ = response_panel::append(&format!("\nError: {e}"));
                        }
                    }),
                    None,
                );
            } else {
                backend::continue_prompt(
                    prompt.trim(),
                    Some(on_stream),
                    Box::new(|res| {
                        if let Some(e) = res.error.as_ref() {
                            let _ = response_panel::append(&format!("\nError: {e}"));
                        }
                    }),
                );
            }
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Any)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterCatchUp",
        |_args: CommandArgs| {
            let cfg = config::get();
            let prompt = cfg.prompts.catch_up.clone();
            let _ = response_panel::open_or_reuse("Agent Catch Up");
            let _ = response_panel::append("Waiting for agent...\n");
            let session_id = get_resumed_session();
            backend::catch_up(
                session_id.as_deref(),
                &prompt,
                Box::new(|res| {
                    if let Some(e) = res.error.as_ref() {
                        let _ = response_panel::append(&format!("\nError: {e}"));
                    } else {
                        let _ = response_panel::append(&res.text);
                    }
                }),
            );
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterList",
        |_args: CommandArgs| {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
            let sd = config::get().state_dir();
            let ws_hash = state::workspace_hash(Path::new(&cwd));
            let sessions = state::load_sessions(&sd, &ws_hash);
            if sessions.is_empty() {
                let _ = api::notify("No sessions found", LogLevel::Info, &Dictionary::default());
                return;
            }
            let mut lines = vec!["── Sessions ──".to_string()];
            for (i, s) in sessions.iter().rev().enumerate() {
                let preview = s.last_prompt_preview.chars().take(50).collect::<String>();
                lines.push(format!(" {} {}  {}", i + 1, s.session_id, preview));
            }
            lines.push("q to close, <CR> to select".to_string());
            let mut buf = match api::create_buf(false, true) {
                Ok(b) => b,
                Err(_) => return,
            };
            let _ = api::set_option_value(
                "buftype",
                "nofile",
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
            let _ = buf.set_lines(0..0, false, refs);
            let _ = api::set_option_value(
                "modifiable",
                false,
                &OptionOpts::builder().buffer(buf.clone()).build(),
            );
            let cols = api::get_option_value::<i64>("columns", &OptionOpts::builder().build())
                .unwrap_or(80);
            let rows =
                api::get_option_value::<i64>("lines", &OptionOpts::builder().build()).unwrap_or(24);
            let width = 60.min(cols as u32 - 6);
            let height = (sessions.len() + 3).min(rows as usize) as u32;
            let row = ((rows as f64) - (height as f64)) / 2.0;
            let col = ((cols as f64) - (width as f64)) / 2.0;
            let config = nvim_oxi::api::types::WindowConfig::builder()
                .relative(WindowRelativeTo::Editor)
                .width(width)
                .height(height)
                .row(row)
                .col(col)
                .border(WindowBorder::Rounded)
                .title(WindowTitle::SimpleString("Sessions".to_string().into()))
                .build();
            let win = match api::open_win(&buf, true, &config) {
                Ok(w) => w,
                Err(_) => return,
            };
            let session_ids: Vec<String> = sessions
                .iter()
                .rev()
                .map(|s| s.session_id.clone())
                .collect();
            let win_cell = std::sync::Arc::new(std::sync::Mutex::new(Some(win)));
            let win_cell_q = win_cell.clone();
            let opts_cr = SetKeymapOpts::builder()
                .callback(move |_| {
                    let (row, _) = api::get_current_win().get_cursor().ok().unwrap_or((1, 0));
                    let idx = row.saturating_sub(2);
                    if idx < session_ids.len() {
                        set_resumed_session(Some(session_ids[idx].clone()));
                        if let Ok(mut g) = win_cell.lock() {
                            if let Some(w) = g.take() {
                                let _ = w.close(false);
                            }
                        }
                        let _ = api::notify(
                            &format!("Resumed session {}", session_ids[idx]),
                            LogLevel::Info,
                            &Dictionary::default(),
                        );
                    }
                })
                .noremap(true)
                .silent(true)
                .build();
            let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_cr);
            let opts_q = SetKeymapOpts::builder()
                .callback(move |_| {
                    if let Ok(mut g) = win_cell_q.lock() {
                        if let Some(w) = g.take() {
                            let _ = w.close(false);
                        }
                    }
                })
                .noremap(true)
                .silent(true)
                .build();
            let _ = buf.set_keymap(Mode::Normal, "q", "", &opts_q);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterResume",
        |args: CommandArgs| {
            let id = args.fargs.first().cloned().unwrap_or_default();
            if id.is_empty() {
                let _ = api::notify(
                    "ArbiterResume requires a session ID",
                    LogLevel::Warn,
                    &Dictionary::default(),
                );
                return;
            }
            let prompt = args.fargs.get(1..).map(|v| v.join(" ")).unwrap_or_default();
            set_resumed_session(Some(id.clone()));
            if prompt.trim().is_empty() {
                let _ = api::notify(
                    &format!("Resumed session {id}. Use :ArbiterContinue to send prompts."),
                    LogLevel::Info,
                    &Dictionary::default(),
                );
                return;
            }
            let _ = response_panel::open_or_reuse("Agent Response");
            let _ = response_panel::append("Waiting for agent...\n");
            let on_stream = std::sync::Arc::new(|text: &str| {
                let _ = response_panel::append_streaming(text);
            });
            backend::thread_reply(
                Some(&id),
                prompt.trim(),
                Some(on_stream),
                Box::new(|res| {
                    if let Some(e) = res.error.as_ref() {
                        let _ = response_panel::append(&format!("\nError: {e}"));
                    }
                }),
                None,
            );
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::OneOrMore)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterSelfReview",
        |args: CommandArgs| {
            with_review_cmd(|r| {
                let cwd = r.cwd.clone();
                let ref_name = r.ref_name.clone();
                let cfg = config::get();
                let user_context = args.fargs.join(" ");
                let prompt_guidance = if user_context.is_empty() {
                    cfg.prompts.self_review.clone()
                } else {
                    format!(
                        "{}\n\nAdditional focus: {user_context}",
                        cfg.prompts.self_review
                    )
                };
                let is_cursor = matches!(cfg.backend, config::BackendKind::Cursor);
                let project_rules_text = rules::format_for_prompt(&rules::resolve(
                    &r.project_rules,
                    rules::Scenario::SelfReview,
                    None,
                ));
                let view_context = if ref_name.is_empty() {
                    "You are reviewing uncommitted working-tree changes (git diff). \
                     Line numbers in the diff correspond to the current file on disk."
                        .to_string()
                } else {
                    format!(
                        "You are reviewing all changes on the current branch compared against \
                         `{ref_name}` (git diff $(git merge-base {ref_name} HEAD)). \
                         Line numbers reference the new (HEAD) side of the diff."
                    )
                };
                git::diff_full(&cwd, &ref_name, move |result| {
                    let diff_text = if result.success() {
                        result.stdout
                    } else {
                        let _ = api::notify(
                            &format!("git diff failed: {}", result.stderr),
                            LogLevel::Error,
                            &Dictionary::default(),
                        );
                        return;
                    };
                    let mut full_template = format!(
                        "{view_context}\n\n{}{}",
                        prompt_guidance,
                        config::SELF_REVIEW_FORMAT_SUFFIX
                    );
                    if !project_rules_text.is_empty() {
                        full_template = format!("{project_rules_text}\n{full_template}");
                    }
                    let prompt = full_template.replace("%s", &diff_text);
                    let json_schema = if is_cursor {
                        None
                    } else {
                        Some(r#"{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":"integer"},"message":{"type":"string"}},"required":["file","line","message"]}}"#.to_string())
                    };
                    backend::self_review(
                        &prompt,
                        json_schema,
                        Box::new(move |res| {
                            if let Some(e) = &res.error {
                                let _ = api::notify(
                                    &format!("Self-review failed: {e}"),
                                    LogLevel::Error,
                                    &Dictionary::default(),
                                );
                                return;
                            }
                            let parsed = if is_cursor {
                                backend::parse_self_review_text(&res.text)
                                    .into_iter()
                                    .map(|p| (p.file, p.line, p.message))
                                    .collect::<Vec<_>>()
                            } else {
                                serde_json::from_str::<Vec<serde_json::Value>>(&res.text)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter_map(|v| {
                                        let obj = v.as_object()?;
                                        let file = obj.get("file")?.as_str()?.to_string();
                                        let line = obj.get("line")?.as_u64()? as u32;
                                        let message = obj.get("message")?.as_str()?.to_string();
                                        Some((file, line, message))
                                    })
                                    .collect::<Vec<_>>()
                            };
                            if let Some(new_thread_ids) =
                                review::with_active(|r| self_review::create_threads(r, &parsed))
                            {
                                self_review::detect_similar(new_thread_ids);
                            }
                        }),
                    );
                });
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Any)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterApply",
        |_args: CommandArgs| {
            with_review_cmd(|r| {
                let items: Vec<(String, u32, String)> = r
                    .threads
                    .iter()
                    .filter(|t| t.origin == ThreadOrigin::Agent && t.status == ThreadStatus::Open)
                    .map(|t| {
                        let msg = t
                            .messages
                            .first()
                            .map(|m| m.text.clone())
                            .unwrap_or_default();
                        (t.file.clone(), t.line, msg)
                    })
                    .collect();

                if items.is_empty() {
                    let _ = api::notify(
                        "No open self-review threads to apply",
                        LogLevel::Info,
                        &Dictionary::default(),
                    );
                    return;
                }

                self_review::show_apply_confirmation(items);
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterResolveAll",
        |_args: CommandArgs| {
            with_review_cmd(|r| {
                crate::threads::resolve_all(&mut r.threads);
                if crate::threads::window_is_open() {
                    crate::threads::window_close();
                }
                let state_dir = r.config.state_dir();
                let ws_hash = crate::state::workspace_hash(std::path::Path::new(&r.cwd));
                crate::state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                if let Some(p) = r.current_file.clone() {
                    review::select_file_impl(r, &p);
                }
                review::rerender_file_panel(r);
                let _ = api::notify(
                    "Resolved all open threads",
                    LogLevel::Info,
                    &Dictionary::default(),
                );
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterRefresh",
        |_args: CommandArgs| {
            with_review_cmd(|r| {
                review::refresh_file(r);
                review::refresh_file_list(r);
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterRef",
        |args: CommandArgs| {
            let new_ref = args
                .fargs
                .first()
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            with_review_cmd(|r| {
                r.ref_name = new_ref.clone();
                let _ = api::notify(
                    &format!(
                        "[arbiter] ref set to {}",
                        if new_ref.is_empty() {
                            "working tree (no base)"
                        } else {
                            &new_ref
                        }
                    ),
                    LogLevel::Info,
                    &Dictionary::default(),
                );
                review::refresh_file_list(r);
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::ZeroOrOne)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterSummary",
        |_args: CommandArgs| {
            with_review_cmd(review::show_summary);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterActiveThread",
        |_args: CommandArgs| {
            with_review_cmd(review::open_active_thread);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterOpenThread",
        |args: CommandArgs| {
            let file = args.fargs.first().cloned().unwrap_or_default();
            let line: u32 = args.fargs.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            if file.is_empty() || line == 0 {
                api::err_writeln("[arbiter] usage: ArbiterOpenThread <file> <line>");
                return;
            }
            review::open_thread_at(&file, line);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Any)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterRules",
        |_args: CommandArgs| {
            with_review_cmd(open_rules_window);
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterToggleRules",
        |_args: CommandArgs| {
            with_review_cmd(|r| {
                r.learn_rules = !r.learn_rules;
                let state = if r.learn_rules { "enabled" } else { "disabled" };
                let _ = api::notify(
                    &format!("[arbiter] rule learning {state}"),
                    LogLevel::Info,
                    &Dictionary::default(),
                );
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterReloadRules",
        |_args: CommandArgs| {
            with_review_cmd(|r| {
                r.project_rules = rules::load_all(&r.cwd, &r.config.rules_dirs);
                let _ = api::notify(
                    &format!("[arbiter] reloaded {} project rules", r.project_rules.len()),
                    LogLevel::Info,
                    &Dictionary::default(),
                );
            });
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    api::create_user_command(
        "ArbiterReset",
        |_args: CommandArgs| {
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string());
            let state_dir = config::get().state_dir();
            let ws_hash = state::workspace_hash(Path::new(&cwd));
            let ws_dir = state_dir.join(&ws_hash);
            if ws_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&ws_dir) {
                    let _ = api::notify(
                        &format!("[arbiter] failed to remove state: {e}"),
                        LogLevel::Error,
                        &Dictionary::default(),
                    );
                    return;
                }
            }
            if review::is_active() {
                review::close();
            }
            let _ = api::notify(
                "[arbiter] workspace state cleared",
                LogLevel::Info,
                &Dictionary::default(),
            );
        },
        &CreateCommandOpts::builder()
            .nargs(CommandNArgs::Zero)
            .build(),
    )?;

    Ok(())
}

fn open_rules_window(r: &mut review::Review) {
    let review_rules = r.review_rules.clone();
    let project_rules = &r.project_rules;

    let mut lines: Vec<String> = Vec::new();
    if !project_rules.is_empty() {
        lines.push("## Project Rules (read-only, from disk)".to_string());
        lines.push(String::new());
        for pr in project_rules {
            let scenarios: Vec<&str> = pr
                .scenarios
                .iter()
                .map(|s| match s {
                    rules::Scenario::Thread => "thread",
                    rules::Scenario::SelfReview => "self_review",
                })
                .collect();
            let scope = if scenarios.is_empty() {
                "all".to_string()
            } else {
                scenarios.join(", ")
            };
            let globs = if pr.match_patterns.is_empty() {
                "*".to_string()
            } else {
                pr.match_patterns.join(", ")
            };
            lines.push(format!("- **{}** [{}] ({})", pr.description, scope, globs));
            for content_line in pr.content.lines() {
                lines.push(format!("  {content_line}"));
            }
        }
        lines.push(String::new());
    }
    lines.push("## Learned Rules (one per line, :w to save, q to close)".to_string());
    lines.push(String::new());
    for rule in &review_rules {
        lines.push(rule.clone());
    }

    let mut buf = match api::create_buf(false, true) {
        Ok(b) => b,
        Err(_) => return,
    };
    let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
    let _ = api::set_option_value("buftype", "acwrite", &buf_opts);
    crate::panel::disable_syntax(&buf);

    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    if !refs.is_empty() {
        let _ = buf.set_lines(0..0, false, refs);
    }
    let _ = api::set_option_value("modified", false, &buf_opts);

    let cols =
        api::get_option_value::<i64>("columns", &OptionOpts::builder().build()).unwrap_or(80);
    let rows = api::get_option_value::<i64>("lines", &OptionOpts::builder().build()).unwrap_or(24);
    let width = ((cols as f64) * 0.8) as u32;
    let height = ((rows as f64) * 0.8) as u32;
    let row = ((rows as f64) - (height as f64)) / 2.0;
    let col = ((cols as f64) - (width as f64)) / 2.0;

    let config = nvim_oxi::api::types::WindowConfig::builder()
        .relative(WindowRelativeTo::Editor)
        .width(width)
        .height(height)
        .row(row)
        .col(col)
        .border(WindowBorder::Rounded)
        .title(WindowTitle::SimpleString(
            " Review Rules (one per line, :w to save, q to close) "
                .to_string()
                .into(),
        ))
        .build();
    let win = match api::open_win(&buf, true, &config) {
        Ok(w) => w,
        Err(_) => return,
    };
    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("number", false, &win_opts);
    let _ = api::set_option_value("relativenumber", false, &win_opts);
    let _ = api::set_option_value("wrap", true, &win_opts);
    let _ = api::set_option_value("cursorline", true, &win_opts);

    let win_cell = std::sync::Arc::new(std::sync::Mutex::new(Some(win)));

    let save_buf = buf.clone();
    let save_opts = SetKeymapOpts::builder()
        .callback(move |_| {
            save_rules_from_buf(&save_buf);
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<C-s>", "", &save_opts);

    let buf_for_write = buf.clone();
    let _ = api::create_autocmd(
        ["BufWriteCmd"],
        &CreateAutocmdOpts::builder()
            .buffer(buf.clone())
            .callback(move |_args| {
                save_rules_from_buf(&buf_for_write);
                let _ = api::set_option_value(
                    "modified",
                    false,
                    &OptionOpts::builder().buffer(buf_for_write.clone()).build(),
                );
                true
            })
            .build(),
    );

    let opts_q = SetKeymapOpts::builder()
        .callback(move |_| {
            if let Ok(mut g) = win_cell.lock() {
                if let Some(w) = g.take() {
                    let _ = w.close(false);
                }
            }
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts_q);
}

fn extract_learned_rules(buf_content: &[String]) -> Vec<String> {
    let start = buf_content
        .iter()
        .position(|l| l.starts_with("## Learned Rules"))
        .map(|i| i + 1)
        .unwrap_or(0);

    buf_content[start..]
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn save_rules_from_buf(buf: &Buffer) {
    let line_count = buf.line_count().unwrap_or(0);
    if line_count == 0 {
        review::with_active(|r| {
            r.review_rules.clear();
            review::save_file_statuses_pub(r);
        });
        return;
    }
    let lines = match buf.get_lines(0..line_count, true) {
        Ok(l) => l,
        Err(_) => return,
    };
    let all: Vec<String> = lines
        .into_iter()
        .map(|l| l.to_string_lossy().to_string())
        .collect();

    let rules = extract_learned_rules(&all);
    review::with_active(|r| {
        r.review_rules = rules;
        review::save_file_statuses_pub(r);
    });
    let _ = api::notify(
        "[arbiter] review rules saved",
        LogLevel::Info,
        &Dictionary::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_learned_rules_finds_header() {
        let buf = vec![
            "## Project Rules (read-only, from disk)".to_string(),
            "- some project rule".to_string(),
            "".to_string(),
            "## Learned Rules (one per line, :w to save, q to close)".to_string(),
            "".to_string(),
            "always use snake_case".to_string(),
            "prefer match over if-let chains".to_string(),
        ];
        let rules = extract_learned_rules(&buf);
        assert_eq!(
            rules,
            vec!["always use snake_case", "prefer match over if-let chains"]
        );
    }

    #[test]
    fn extract_learned_rules_no_header() {
        let buf = vec!["some random line".to_string(), "another line".to_string()];
        let rules = extract_learned_rules(&buf);
        assert_eq!(rules, vec!["some random line", "another line"]);
    }

    #[test]
    fn extract_learned_rules_empty_after_header() {
        let buf = vec![
            "## Project Rules".to_string(),
            "- rule one".to_string(),
            "## Learned Rules".to_string(),
            "".to_string(),
            "   ".to_string(),
        ];
        let rules = extract_learned_rules(&buf);
        assert!(rules.is_empty());
    }
}
