//! Self-review thread creation, similarity detection, and apply confirmation.

use super::*;

pub(super) fn show_apply_confirmation(items: Vec<(String, u32, String)>) {
    let count = items.len();
    let mut lines = Vec::with_capacity(count + 3);
    lines.push(format!(
        " Apply {} self-review item{}?  <Enter> confirm  q cancel",
        count,
        if count == 1 { "" } else { "s" },
    ));
    lines.push(String::new());
    for (i, (file, line, msg)) in items.iter().enumerate() {
        let preview: String = msg.chars().take(80).collect();
        lines.push(format!("  {}. {}:{} - {}", i + 1, file, line, preview));
    }

    let mut buf = match api::create_buf(false, true) {
        Ok(b) => b,
        Err(_) => return,
    };
    let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
    let _ = api::set_option_value("buftype", "nofile", &buf_opts);
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let _ = buf.set_lines(0..0, false, refs);
    let _ = api::set_option_value("modifiable", false, &buf_opts);

    let cols =
        api::get_option_value::<i64>("columns", &OptionOpts::builder().build()).unwrap_or(80);
    let rows = api::get_option_value::<i64>("lines", &OptionOpts::builder().build()).unwrap_or(24);
    let max_line = lines.iter().map(|l| l.len()).max().unwrap_or(40);
    let width = (max_line as u32 + 4).min(cols as u32 - 4);
    let height = (lines.len() as u32 + 2).min(rows as u32 - 4);
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
            "Apply Self-Review".to_string().into(),
        ))
        .build();

    let win = match api::open_win(&buf, true, &config) {
        Ok(w) => w,
        Err(_) => return,
    };

    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("cursorline", false, &win_opts);
    let _ = api::set_option_value("number", false, &win_opts);

    let ns = api::create_namespace("arbiter-apply-confirm");
    let _ = buf.add_highlight(ns, "ArbiterDiffFile", 0, 0..);
    for i in 2..lines.len() {
        let _ = buf.add_highlight(ns, "Comment", i, 0..);
    }

    let win_cell = std::sync::Arc::new(std::sync::Mutex::new(Some(win)));
    let win_cell_q = win_cell.clone();

    let opts_cr = SetKeymapOpts::builder()
        .callback(move |_| {
            if let Ok(mut g) = win_cell.lock() {
                if let Some(w) = g.take() {
                    let _ = w.close(false);
                }
            }
            execute_apply(items.clone());
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
    let _ = buf.set_keymap(Mode::Normal, "<Esc>", "", &opts_q);
}

fn execute_apply(items: Vec<(String, u32, String)>) {
    review::with_active(|r| {
        let feedback: Vec<prompts::FeedbackItem<'_>> = items
            .iter()
            .map(|(file, line, msg)| prompts::FeedbackItem {
                file,
                line: *line,
                message: msg,
            })
            .collect();

        let project_rules_text = rules::format_for_prompt(&rules::resolve(
            &r.project_rules,
            rules::Scenario::SelfReview,
            None,
        ));
        let review_ctx = prompts::ReviewContext {
            ref_name: &r.ref_name,
            file_diff: "",
            review_rules: &r.review_rules,
            project_rules: project_rules_text,
        };

        let Some(prompt) = prompts::format_apply_feedback_prompt(&feedback, &review_ctx) else {
            return;
        };

        let count = items.len();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for t in r.threads.iter_mut() {
            if t.origin == ThreadOrigin::Agent && t.status == ThreadStatus::Open {
                t.auto_resolve = true;
                t.auto_resolve_at = Some(now);
            }
        }

        let state_dir = r.config.state_dir();
        let ws_hash = state::workspace_hash(Path::new(&r.cwd));
        state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);

        let _ = api::notify(
            &format!("Applying {count} self-review items..."),
            LogLevel::Info,
            &Dictionary::default(),
        );

        backend::send_prompt(
            &prompt,
            None,
            Box::new(move |res| {
                if let Some(e) = &res.error {
                    let _ = api::notify(
                        &format!("Apply failed: {e}"),
                        LogLevel::Error,
                        &Dictionary::default(),
                    );
                    backend::notify_if_missing_binary(e);
                    return;
                }
                let _ = api::notify(
                    &format!("Applied {count} self-review items"),
                    LogLevel::Info,
                    &Dictionary::default(),
                );
            }),
        );
    });
}

pub(super) fn create_threads(
    r: &mut review::Review,
    parsed: &[(String, u32, String)],
) -> Vec<(String, String, u32, String)> {
    let mut new_ids: Vec<(String, String, u32, String)> = Vec::new();
    for (file, line, message) in parsed {
        let full_path = Path::new(&r.cwd).join(file);
        let contents = std::fs::read_to_string(&full_path).unwrap_or_default();
        let file_lines: Vec<&str> = contents.lines().collect();
        let anchor_content = file_lines
            .get(line.saturating_sub(1) as usize)
            .map(|s| s.to_string())
            .unwrap_or_default();
        let ctx_start = line.saturating_sub(2) as usize;
        let ctx_end = (*line + 2).min(file_lines.len() as u32) as usize;
        let context: Vec<String> = file_lines
            .get(ctx_start..ctx_end)
            .unwrap_or(&[])
            .iter()
            .map(|s| s.to_string())
            .collect();
        let thread = threads::create(
            file,
            *line,
            message,
            threads::CreateOpts {
                pending: false,
                auto_resolve: false,
                origin: ThreadOrigin::Agent,
                anchor_content,
                anchor_context: context,
            },
        );
        new_ids.push((thread.id.clone(), file.clone(), *line, message.clone()));
        r.threads.push(thread);
    }
    let sd = r.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&r.cwd));
    state::save_threads(&sd, &ws_hash, &r.ref_name, &r.threads);
    if let Some(path) = r.current_file.clone() {
        review::select_file_impl(r, &path);
    }
    let _ = api::notify(
        "Self-review complete, detecting similar threads...",
        LogLevel::Info,
        &Dictionary::default(),
    );
    new_ids
}

pub(super) fn detect_similar(new_thread_ids: Vec<(String, String, u32, String)>) {
    let items: Vec<(usize, &str, u32, &str)> = new_thread_ids
        .iter()
        .enumerate()
        .map(|(i, (_, file, line, msg))| (i, file.as_str(), *line, msg.as_str()))
        .collect();
    let Some(prompt) = prompts::format_similarity_prompt(&items) else {
        let _ = api::notify(
            "Self-review complete",
            LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    };
    backend::self_review(
        &prompt,
        None,
        Box::new(move |res| {
            if res.error.is_some() {
                let _ = api::notify(
                    "Self-review complete (similarity detection skipped)",
                    LogLevel::Info,
                    &Dictionary::default(),
                );
                return;
            }
            let groups = prompts::parse_similarity_response(&res.text);
            if groups.is_empty() {
                let _ = api::notify(
                    "Self-review complete (no similar threads found)",
                    LogLevel::Info,
                    &Dictionary::default(),
                );
                return;
            }
            review::with_active(|r| {
                for group in &groups {
                    let group_refs: Vec<(String, String, u32, String)> = group
                        .iter()
                        .filter_map(|&idx| new_thread_ids.get(idx))
                        .cloned()
                        .collect();
                    for &idx in group {
                        let Some((tid, _, _, _)) = new_thread_ids.get(idx) else {
                            continue;
                        };
                        let Some(thread) = r.threads.iter_mut().find(|t| t.id == *tid) else {
                            continue;
                        };
                        let similar: Vec<threads::SimilarRef> = group_refs
                            .iter()
                            .filter(|(id, _, _, _)| id != tid)
                            .map(|(id, file, line, msg)| threads::SimilarRef {
                                thread_id: id.clone(),
                                file: file.clone(),
                                line: *line,
                                preview: msg.chars().take(60).collect(),
                            })
                            .collect();
                        thread.similar_threads = similar;
                    }
                }
                let sd = r.config.state_dir();
                let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                state::save_threads(&sd, &ws_hash, &r.ref_name, &r.threads);
                if let Some(path) = r.current_file.clone() {
                    review::select_file_impl(r, &path);
                }
            });
            let group_count = groups.len();
            let _ = api::notify(
                &format!(
                    "Self-review complete ({group_count} similar group{} found)",
                    if group_count == 1 { "" } else { "s" }
                ),
                LogLevel::Info,
                &Dictionary::default(),
            );
        }),
    );
}
