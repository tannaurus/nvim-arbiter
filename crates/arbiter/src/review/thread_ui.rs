//! Thread panel, thread list, comment creation, and thread navigation.

use super::*;

fn make_thread_reply_callback(
    thread_id: String,
    file: String,
    line: u32,
) -> threads::OnReplyRequested {
    Box::new(move || {
        let title = format!("Reply at {file}:{line}");
        let tid = thread_id.clone();
        let file_for_notify = file.clone();
        let on_submit: threads::OnSubmit = Box::new(move |text: String| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            with_active(|r| {
                if let Some(t) = r.threads.iter_mut().find(|x| x.id == tid) {
                    threads::add_message(t, Role::User, &text);
                    if t.status == ThreadStatus::Resolved {
                        t.status = ThreadStatus::Open;
                    }
                    if threads::window_is_open() {
                        let _ = threads::append_message(Role::User, &text);
                    }
                    let session_id = t.session_id.clone();
                    let is_resumed = session_id.is_some();
                    let state_dir = r.config.state_dir();
                    let ws_hash = state::workspace_hash(Path::new(&r.cwd)).clone();
                    let ref_name = r.ref_name.clone();
                    let sd = state_dir.clone();
                    let ws = ws_hash.clone();
                    let rn = ref_name.clone();
                    let sd2 = state_dir.clone();
                    let ws2 = ws_hash.clone();
                    let rn2 = ref_name.clone();
                    let stream_tid = tid.clone();
                    let on_stream: crate::types::OnStream =
                        std::sync::Arc::new(move |chunk: &str| {
                            backend::append_inflight_stream(chunk);
                            if threads::window_thread_id().as_deref() == Some(&stream_tid) {
                                let _ = threads::append_streaming(chunk);
                            }
                        });
                    let prior_messages: Vec<(String, String)> = t
                        .messages
                        .iter()
                        .map(|m| {
                            let role = match m.role {
                                Role::User => "user",
                                Role::Agent => "agent",
                            };
                            (role.to_string(), m.text.clone())
                        })
                        .collect();
                    let t_file = t.file.clone();
                    let t_line = t.line;
                    let t_anchor = t.anchor_content.clone();
                    let t_context = t.anchor_context.clone();
                    let t_similar = t.similar_threads.clone();
                    let similar_ctx = gather_similar_context(&r.threads, &t_similar);
                    let nearby_diff =
                        prompts::extract_nearby_diff(&r.current_diff_text, t_line, 50);
                    let project_rules_text = rules::format_for_prompt(&rules::resolve(
                        &r.project_rules,
                        rules::Scenario::Thread,
                        Some(&t_file),
                    ));
                    let review_ctx = prompts::ReviewContext {
                        ref_name: &r.ref_name,
                        file_diff: &nearby_diff,
                        review_rules: &r.review_rules,
                        project_rules: project_rules_text,
                    };
                    let prompt = prompts::format_reply_prompt(
                        &prompts::ReplyContext {
                            file: &t_file,
                            line: t_line,
                            reply: &text,
                            anchor_content: &t_anchor,
                            context: &t_context,
                            prior_messages: &prior_messages,
                            is_resumed,
                        },
                        &review_ctx,
                        &similar_ctx,
                    );
                    let prompt_len = prompt.len();
                    threads::set_last_prompt(prompt.clone());
                    let file_notify = file_for_notify.clone();
                    let reply_tag = tid.clone();
                    let status_tid = tid.clone();
                    let was_inflight = backend::inflight_tag().as_deref() == Some(tid.as_str());
                    let was_queued = backend::queue_position(&tid).is_some();
                    backend::cancel_tagged(&tid);
                    if (was_inflight || was_queued)
                        && threads::window_thread_id().as_deref() == Some(tid.as_str())
                    {
                        let _ = threads::append_interrupted();
                    }
                    let snap_cell: Arc<std::sync::Mutex<Option<BeforeSnapshot>>> =
                        Arc::new(std::sync::Mutex::new(None));
                    {
                        let snap_cell = snap_cell.clone();
                        let cwd = r.cwd.clone();
                        let ref_name = r.ref_name.clone();
                        let paths: Vec<String> =
                            r.files.iter().map(|(p, _, _)| p.clone()).collect();
                        std::thread::spawn(move || {
                            let mut all_paths = paths;
                            for dp in revision::diff_names_sync(&cwd, &ref_name) {
                                if !all_paths.contains(&dp) {
                                    all_paths.push(dp);
                                }
                            }
                            let snap = (
                                cwd.clone(),
                                ref_name.clone(),
                                revision::snapshot_files(&cwd, &all_paths),
                            );
                            if let Ok(mut guard) = snap_cell.lock() {
                                *guard = Some(snap);
                            }
                        });
                    }
                    backend::thread_reply(
                        session_id.as_deref(),
                        &prompt,
                        Some(on_stream),
                        Box::new(move |res| {
                            let before_snapshot = snap_cell.lock().ok().and_then(|mut g| g.take());
                            let msg = res
                                .error
                                .as_ref()
                                .map(|e| format!("[Error] {e}"))
                                .unwrap_or(res.text);
                            let had_error = res.error.is_some();
                            if let Some(e) = res.error.as_ref() {
                                backend::notify_if_missing_binary(e);
                                let _ = api::notify(
                                    &format!("Reply failed: {e}"),
                                    nvim_oxi::api::types::LogLevel::Warn,
                                    &Dictionary::default(),
                                );
                                if threads::window_thread_id().as_deref() == Some(&tid) {
                                    let _ = threads::replace_last_agent_message(&msg);
                                }
                            }
                            with_active(|r| {
                                if let Some(t) = r.threads.iter_mut().find(|x| x.id == tid) {
                                    threads::add_message(t, Role::Agent, &msg);
                                    if !res.session_id.is_empty() {
                                        t.session_id = Some(res.session_id);
                                    }
                                    if !had_error {
                                        maybe_capture_revision(r, &tid, &before_snapshot);
                                    }
                                    state::save_threads(&sd, &ws, &rn, &r.threads);
                                    if let Some(p) = r.current_file.clone() {
                                        select_file_impl(r, &p);
                                    }
                                }
                                if !had_error {
                                    maybe_queue_extraction(r, &tid);
                                }
                            });
                            if !threads::window_is_open() {
                                let preview: String = msg.chars().take(60).collect();
                                let _ = api::notify(
                                        &format!(
                                            "[arbiter] Agent replied on {file_notify}:{line}: {preview}"
                                        ),
                                        nvim_oxi::api::types::LogLevel::Info,
                                        &Dictionary::default(),
                                    );
                            }
                        }),
                        Some(reply_tag),
                    );
                    show_thread_queue_status(&status_tid, Some(prompt_len));
                    state::save_threads(&sd2, &ws2, &rn2, &r.threads);
                    if let Some(p) = r.current_file.clone() {
                        select_file_impl(r, &p);
                    }
                }
            });
        });
        let on_cancel: threads::OnCancel = Box::new(|| {});
        if let Some(win) = threads::window_handle() {
            let _ = threads::open_below(&title, on_submit, on_cancel, &win);
        } else {
            let _ = threads::open(&title, on_submit, on_cancel);
        }
    })
}

pub(super) fn open_thread_panel(review: &Review, t: &threads::Thread) {
    clear_thread_anchor();
    let on_reply = make_thread_reply_callback(t.id.clone(), t.file.clone(), t.line);
    let on_close: threads::OnClose = Arc::new(clear_thread_anchor);
    let on_revision: threads::OnRevisionSelected = Arc::new(|rev_idx, file| {
        with_active(|r| handle_revision_selected(r, rev_idx, file.as_deref()));
    });
    let on_similar: threads::OnSimilarSelected = Arc::new(|tid| {
        with_active(|r| handle_similar_selected(r, &tid));
    });
    let _ = threads::window_open(
        &t.id,
        &t.file,
        t.line,
        &t.messages,
        threads::WindowCallbacks {
            on_reply,
            on_close: Some(on_close),
            on_revision: Some(on_revision),
            on_similar: Some(on_similar),
        },
    );
    for rev in &t.revisions {
        let stats: Vec<(String, usize, usize)> = rev
            .files
            .iter()
            .map(|rf| {
                let (a, r) = revision::revision_file_stats(rf);
                (rf.path.clone(), a, r)
            })
            .collect();
        let _ = threads::append_revision_summary(rev.index, rev.files.len(), &stats);
    }
    if !t.similar_threads.is_empty() {
        let _ = threads::append_similar_threads(&t.similar_threads);
    }
    place_thread_anchor(review, t.line);
    let is_inflight = backend::inflight_tag().as_deref() == Some(&t.id);
    let is_queued = backend::queue_position(&t.id).is_some();
    if is_inflight || is_queued {
        show_thread_queue_status(&t.id, None);
    }
}

pub(crate) fn open_active_thread(review: &mut Review) {
    let Some(tid) = backend::inflight_tag() else {
        let _ = api::notify(
            "[arbiter] no active thread",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    };
    let needs_file_switch = review
        .threads
        .iter()
        .find(|t| t.id == tid)
        .map(|t| review.current_file.as_deref() != Some(&t.file))
        .unwrap_or(false);
    if needs_file_switch {
        if let Some(file) = review
            .threads
            .iter()
            .find(|t| t.id == tid)
            .map(|t| t.file.clone())
        {
            navigate_to_file(review, &file);
        }
    }
    open_thread_by_id(review, &tid);
    let streamed = backend::inflight_stream();
    if !streamed.is_empty() {
        let _ = threads::append_streaming(&streamed);
    }
}

fn open_thread_by_id(review: &Review, tid: &str) -> bool {
    if let Some(t) = review.threads.iter().find(|x| x.id == tid) {
        open_thread_panel(review, t);
        return true;
    }
    false
}

fn place_thread_anchor(review: &Review, source_line: u32) {
    let ns = api::create_namespace("arbiter-thread-anchor");
    let mut buf = review.diff_panel.buf.clone();
    let line_refs: Vec<&str> = review
        .current_diff_lines
        .iter()
        .map(|s| s.as_str())
        .collect();

    if let Some(buf_line) =
        diff::source_to_buf_line(&review.current_hunks, source_line as usize, &line_refs)
    {
        let _ = buf.add_highlight(ns, "CursorLine", buf_line, 0..);
    }
}

fn clear_thread_anchor() {
    let ns = api::create_namespace("arbiter-thread-anchor");
    with_active(|r| {
        let mut buf = r.diff_panel.buf.clone();
        let _ = buf.clear_namespace(ns, 0..usize::MAX);
    });
}

pub(super) fn handle_diff_cr(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line_0 = row.saturating_sub(1);

    if let Some(tid) = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line_0)
        .map(|(t, _)| t.clone())
    {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(tid) = review.thread_inline_marks.get(&buf_line_0).cloned() {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(path) = review.current_file.clone() {
        let line_refs: Vec<&str> = review
            .current_diff_lines
            .iter()
            .map(|s| s.as_str())
            .collect();
        if let Some(loc) =
            diff::buf_line_to_source(&review.current_hunks, buf_line_0, &line_refs, &path)
        {
            let full = Path::new(&review.cwd).join(&loc.file);
            if full.exists() {
                let _ = api::command(&format!("tabnew +{} {}", loc.line, full.display()));
            }
        }
    }
}

pub(super) fn handle_open_thread(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line_0 = row.saturating_sub(1);

    if let Some(tid) = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line_0)
        .map(|(t, _)| t.clone())
    {
        open_thread_by_id(review, &tid);
        return;
    }

    if let Some(tid) = review.thread_inline_marks.get(&buf_line_0).cloned() {
        open_thread_by_id(review, &tid);
        return;
    }

    let Some((path, src_line, _, _)) = get_source_loc(review) else {
        return;
    };
    if let Some(t) = review
        .threads
        .iter()
        .find(|t| t.file == path && t.line == src_line)
    {
        open_thread_panel(review, t);
    } else {
        api::err_writeln(&format!("[arbiter] no thread at {path}:{src_line}"));
    }
}

pub(super) fn try_resolve_thread_at_cursor(review: &mut Review) -> bool {
    let (row, _) = match review.diff_panel.win.get_cursor().into_result() {
        Ok(r) => r,
        Err(_) => return false,
    };
    let buf_line = row.saturating_sub(1);
    let tid = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line)
        .map(|(id, _)| id.clone());
    let Some(tid) = tid else { return false };
    let Some(t) = review.threads.iter_mut().find(|t| t.id == tid) else {
        return false;
    };
    if t.status == ThreadStatus::Open {
        threads::resolve(t);
    } else {
        t.status = ThreadStatus::Open;
    }
    save_threads(review);
    if let Some(path) = review.current_file.clone() {
        select_file_impl(review, &path);
    }
    true
}

fn get_source_loc(review: &Review) -> Option<(String, u32, String, Vec<String>)> {
    let path = match review.current_file.as_ref() {
        Some(p) => p,
        None => {
            api::err_writeln("[arbiter] no file selected - select a file first");
            return None;
        }
    };
    let (row, _) = match review.diff_panel.win.get_cursor().into_result() {
        Ok(pos) => pos,
        Err(e) => {
            api::err_writeln(&format!("[arbiter] get_cursor failed: {e}"));
            return None;
        }
    };
    let buf_line = row.saturating_sub(1);
    let line_refs: Vec<&str> = review
        .current_diff_lines
        .iter()
        .map(|s| s.as_str())
        .collect();
    let loc = diff::buf_line_to_source(&review.current_hunks, buf_line, &line_refs, path);
    let loc = match loc {
        Some(l) => l,
        None => {
            let hunk_info = if review.current_hunks.is_empty() {
                "no hunks loaded".to_string()
            } else {
                review
                    .current_hunks
                    .iter()
                    .map(|h| format!("{}-{}", h.buf_start, h.buf_end))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            api::err_writeln(&format!(
                "[arbiter] buf_line={buf_line} not in any hunk (hunks: {hunk_info})"
            ));
            return None;
        }
    };
    let full = Path::new(&review.cwd).join(&loc.file);
    let contents = std::fs::read_to_string(&full).unwrap_or_default();
    let file_lines: Vec<&str> = contents.lines().collect();
    let line_num = loc.line;
    let anchor_content = file_lines
        .get(line_num.saturating_sub(1))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let ctx_start = line_num.saturating_sub(2).saturating_sub(1);
    let ctx_end = (line_num + 2).min(file_lines.len().max(1));
    let context: Vec<String> = file_lines
        .get(ctx_start..ctx_end)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.to_string())
        .collect();
    Some((path.clone(), line_num as u32, anchor_content, context))
}

pub(super) fn handle_immediate_comment(review: &mut Review) {
    let Some((path, line, anchor_content, context)) = get_source_loc(review) else {
        return;
    };
    let path = path.clone();
    let path_for_open = path.clone();
    let anchor_content = anchor_content.clone();
    let context = context.clone();
    if let Err(e) = threads::open_for_line(
        &path_for_open,
        line,
        Box::new(move |text: String| {
            if text.trim().is_empty() {
                return;
            }
            let trimmed = text.trim().to_string();

            let thread = with_active(|r| {
                let thread = threads::create(
                    &path,
                    line,
                    &trimmed,
                    threads::CreateOpts {
                        pending: false,
                        auto_resolve: false,
                        origin: ThreadOrigin::User,
                        anchor_content: anchor_content.clone(),
                        anchor_context: context.clone(),
                    },
                );
                r.threads.push(thread.clone());
                let state_dir = r.config.state_dir();
                let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                if let Some(p) = r.current_file.clone() {
                    select_file_impl(r, &p);
                }
                thread
            });

            let Some(thread) = thread else { return };
            let tid = thread.id.clone();
            let on_reply = make_thread_reply_callback(tid.clone(), path.clone(), line);
            let on_close: threads::OnClose = Arc::new(clear_thread_anchor);
            let on_revision: threads::OnRevisionSelected = Arc::new(|rev_idx, file| {
                with_active(|r| handle_revision_selected(r, rev_idx, file.as_deref()));
            });
            let on_similar: threads::OnSimilarSelected = Arc::new(|tid| {
                with_active(|r| handle_similar_selected(r, &tid));
            });
            let _ = threads::window_open(
                &thread.id,
                &thread.file,
                thread.line,
                &thread.messages,
                threads::WindowCallbacks {
                    on_reply,
                    on_close: Some(on_close),
                    on_revision: Some(on_revision),
                    on_similar: Some(on_similar),
                },
            );
            if !thread.similar_threads.is_empty() {
                let _ = threads::append_similar_threads(&thread.similar_threads);
            }
            with_active(|r| place_thread_anchor(r, thread.line));

            let prompt = with_active(|r| {
                let nearby_diff = prompts::extract_nearby_diff(&r.current_diff_text, line, 50);
                let project_rules_text = rules::format_for_prompt(&rules::resolve(
                    &r.project_rules,
                    rules::Scenario::Thread,
                    Some(&path),
                ));
                let review_ctx = prompts::ReviewContext {
                    ref_name: &r.ref_name,
                    file_diff: &nearby_diff,
                    review_rules: &r.review_rules,
                    project_rules: project_rules_text,
                };
                prompts::format_comment_prompt(
                    &path,
                    line,
                    &trimmed,
                    &anchor_content,
                    &context,
                    &review_ctx,
                )
            })
            .unwrap_or_default();
            let stream_tid = tid.clone();
            let on_stream: crate::types::OnStream = std::sync::Arc::new(move |chunk: &str| {
                backend::append_inflight_stream(chunk);
                if threads::window_thread_id().as_deref() == Some(&stream_tid) {
                    let _ = threads::append_streaming(chunk);
                }
            });
            let prompt_len = prompt.len();
            threads::set_last_prompt(prompt.clone());
            let file_notify = path.clone();
            let comment_tag = tid.clone();
            let window_tid = tid.clone();
            let status_tid = tid.clone();
            let snap_cell: Arc<std::sync::Mutex<Option<BeforeSnapshot>>> =
                Arc::new(std::sync::Mutex::new(None));
            {
                let snap_cell = snap_cell.clone();
                let (cwd, ref_name, paths) = with_active(|r| {
                    let paths: Vec<String> = r.files.iter().map(|(p, _, _)| p.clone()).collect();
                    (r.cwd.clone(), r.ref_name.clone(), paths)
                })
                .unwrap_or_default();
                std::thread::spawn(move || {
                    let mut all_paths = paths;
                    for dp in revision::diff_names_sync(&cwd, &ref_name) {
                        if !all_paths.contains(&dp) {
                            all_paths.push(dp);
                        }
                    }
                    let snap = (
                        cwd.clone(),
                        ref_name.clone(),
                        revision::snapshot_files(&cwd, &all_paths),
                    );
                    if let Ok(mut guard) = snap_cell.lock() {
                        *guard = Some(snap);
                    }
                });
            }
            backend::send_comment(
                &prompt,
                Some(on_stream),
                Box::new(move |res| {
                    let before_snapshot = snap_cell.lock().ok().and_then(|mut g| g.take());
                    let msg = res
                        .error
                        .as_ref()
                        .map(|e| format!("[Error] {e}"))
                        .unwrap_or(res.text);
                    let had_error = res.error.is_some();
                    if had_error {
                        backend::notify_if_missing_binary(&msg);
                        let _ = api::notify(
                            &format!("Comment failed: {msg}"),
                            nvim_oxi::api::types::LogLevel::Warn,
                            &Dictionary::default(),
                        );
                        if threads::window_thread_id().as_deref() == Some(&window_tid) {
                            let _ = threads::replace_last_agent_message(&msg);
                        }
                    }
                    with_active(|r| {
                        if let Some(t) = r.threads.iter_mut().find(|t| t.id == tid) {
                            t.session_id = Some(res.session_id.clone()).filter(|s| !s.is_empty());
                            threads::add_message(t, Role::Agent, &msg);
                            if !had_error {
                                maybe_capture_revision(r, &tid, &before_snapshot);
                            }
                        }
                        let state_dir = r.config.state_dir();
                        let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                        state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
                        if let Some(p) = r.current_file.clone() {
                            select_file_impl(r, &p);
                        }
                        if !had_error {
                            maybe_queue_extraction(r, &tid);
                        }
                    });
                    if !threads::window_is_open() && !had_error {
                        let preview: String = msg.chars().take(60).collect();
                        let _ = api::notify(
                            &format!("[arbiter] Agent replied on {file_notify}:{line}: {preview}"),
                            nvim_oxi::api::types::LogLevel::Info,
                            &Dictionary::default(),
                        );
                    }
                }),
                Some(comment_tag),
            );
            show_thread_queue_status(&status_tid, Some(prompt_len));
        }),
        Box::new(|| {}),
    ) {
        api::err_writeln(&format!("[arbiter] open comment float failed: {e}"));
    }
}

pub(super) fn handle_list_threads(review: &mut Review) {
    handle_list_threads_filtered(review, threads::FilterOpts::default());
}

pub(super) struct ThreadListState {
    pub(super) line_map: Vec<Option<(String, ThreadStatus)>>,
    pub(super) filter: threads::FilterOpts,
}

pub(super) fn close_thread_list_win() {
    THREAD_LIST_WIN.with(|c| {
        if let Some(w) = c.borrow_mut().take() {
            let _ = w.close(false);
        }
    });
    THREAD_LIST_BUF.with(|c| c.borrow_mut().take());
    THREAD_LIST_STATE.with(|c| c.borrow_mut().take());
}

pub(super) fn handle_list_threads_filtered(review: &mut Review, opts: threads::FilterOpts) {
    let existing_open = THREAD_LIST_WIN.with(|c| {
        let guard = c.borrow();
        guard.as_ref().and_then(|w| w.is_valid().then(|| w.clone()))
    });
    if let Some(win) = existing_open {
        let _ = api::set_current_win(&win);
        return;
    }
    close_thread_list_win();

    let filtered = threads::filter(&review.threads, &opts);
    if filtered.is_empty() {
        api::err_writeln("[arbiter] no threads match filter");
        return;
    }
    let content = build_thread_list_lines(&filtered);

    let mut buf = match api::create_buf(false, true) {
        Ok(b) => b,
        Err(_) => return,
    };
    let buf_opts = OptionOpts::builder().buffer(buf.clone()).build();
    let _ = api::set_option_value("buftype", "nofile", &buf_opts);
    crate::panel::disable_syntax(&buf);
    let refs: Vec<&str> = content.lines.iter().map(|s| s.as_str()).collect();
    let _ = buf.set_lines(0..0, false, refs);
    let _ = api::set_option_value("modifiable", false, &buf_opts);

    apply_thread_list_highlights(&mut buf, &content.highlights);

    let cols =
        api::get_option_value::<i64>("columns", &OptionOpts::builder().build()).unwrap_or(80);
    let rows = api::get_option_value::<i64>("lines", &OptionOpts::builder().build()).unwrap_or(24);
    let width = ((cols as f64) * 0.8) as u32;
    let height = ((rows as f64) * 0.8) as u32;
    let row = ((rows as f64) - (height as f64)) / 2.0;
    let col = ((cols as f64) - (width as f64)) / 2.0;

    let title = match (&opts.origin, &opts.status) {
        (Some(ThreadOrigin::Agent), _) => " Agent Threads ",
        (Some(ThreadOrigin::User), _) => " User Threads ",
        (_, Some(ThreadStatus::Open)) => " Open Threads ",
        (_, Some(ThreadStatus::Stale)) => " Stale Threads ",
        (_, Some(ThreadStatus::Resolved)) => " Resolved Threads ",
        _ => " Threads ",
    };

    let win_config = nvim_oxi::api::types::WindowConfig::builder()
        .relative(nvim_oxi::api::types::WindowRelativeTo::Editor)
        .width(width)
        .height(height)
        .row(row)
        .col(col)
        .border(nvim_oxi::api::types::WindowBorder::Rounded)
        .title(nvim_oxi::api::types::WindowTitle::SimpleString(
            title.to_string().into(),
        ))
        .build();
    let mut win = match api::open_win(&buf, true, &win_config) {
        Ok(w) => w,
        Err(_) => return,
    };
    let win_opts = OptionOpts::builder().win(win.clone()).build();
    let _ = api::set_option_value("number", false, &win_opts);
    let _ = api::set_option_value("relativenumber", false, &win_opts);
    let _ = api::set_option_value("wrap", false, &win_opts);
    let _ = api::set_option_value("cursorline", true, &win_opts);

    let first_entry = content
        .line_map
        .iter()
        .position(|e| e.is_some())
        .unwrap_or(0)
        + 1;
    let _ = win.set_cursor(first_entry, 0);

    THREAD_LIST_WIN.with(|c| *c.borrow_mut() = Some(win.clone()));
    THREAD_LIST_BUF.with(|c| *c.borrow_mut() = Some(buf.clone()));

    let state = Arc::new(std::sync::Mutex::new(ThreadListState {
        line_map: content.line_map,
        filter: opts,
    }));
    THREAD_LIST_STATE.with(|c| *c.borrow_mut() = Some(state.clone()));

    let opts_q = SetKeymapOpts::builder()
        .callback(move |_| {
            close_thread_list_win();
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts_q);

    let opts_esc = SetKeymapOpts::builder()
        .callback(move |_| {
            close_thread_list_win();
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<Esc>", "", &opts_esc);

    let cr_state = state.clone();
    let opts_cr = SetKeymapOpts::builder()
        .callback(move |_| {
            let (cursor_row, _) = api::get_current_win().get_cursor().ok().unwrap_or((1, 0));
            let line_idx = cursor_row.saturating_sub(1);
            let entry = {
                let Ok(guard) = cr_state.lock() else {
                    return Ok::<(), nvim_oxi::Error>(());
                };
                guard.line_map.get(line_idx).and_then(|x| x.clone())
            };
            let Some((tid, _)) = entry else {
                return Ok::<(), nvim_oxi::Error>(());
            };
            close_thread_list_win();
            with_active(|r| {
                r.pending_thread_open = Some(tid);
                if let Some(file) = r
                    .threads
                    .iter()
                    .find(|t| t.id == r.pending_thread_open.as_deref().unwrap_or(""))
                    .map(|t| t.file.clone())
                {
                    navigate_to_file(r, &file);
                }
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_cr);

    let dd_buf = buf.clone();
    let dd_state = state.clone();
    let opts_dd = SetKeymapOpts::builder()
        .callback(move |_| {
            let (cursor_row, _) = api::get_current_win().get_cursor().ok().unwrap_or((1, 0));
            let line_idx = cursor_row.saturating_sub(1);
            let (entry, filter) = {
                let Ok(guard) = dd_state.lock() else {
                    return Ok::<(), nvim_oxi::Error>(());
                };
                let e = guard.line_map.get(line_idx).and_then(|x| x.clone());
                (e, guard.filter.clone())
            };
            let Some((tid, status)) = entry else {
                return Ok::<(), nvim_oxi::Error>(());
            };
            with_active(|r| {
                match status {
                    ThreadStatus::Open | ThreadStatus::Stale => {
                        if let Some(t) = r.threads.iter_mut().find(|t| t.id == tid) {
                            threads::resolve(t);
                        }
                    }
                    ThreadStatus::Resolved => {
                        if let Some(idx) = r.threads.iter().position(|t| t.id == tid) {
                            threads::dismiss(&mut r.threads, idx);
                        }
                    }
                }
                save_threads(r);

                let filtered = threads::filter(&r.threads, &filter);
                if filtered.is_empty() {
                    close_thread_list_win();
                } else {
                    let new_content = build_thread_list_lines(&filtered);
                    let mut b = dd_buf.clone();
                    update_thread_list_buf(&mut b, &new_content);

                    if let Ok(mut guard) = dd_state.lock() {
                        guard.line_map = new_content.line_map;
                    }
                }

                if let Some(p) = r.current_file.clone() {
                    select_file_impl(r, &p);
                }
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "dd", "", &opts_dd);

    let ra_buf = buf.clone();
    let ra_state = state.clone();
    let opts_resolve_all = SetKeymapOpts::builder()
        .callback(move |_| {
            let filter = {
                let Ok(guard) = ra_state.lock() else {
                    return Ok::<(), nvim_oxi::Error>(());
                };
                guard.filter.clone()
            };
            with_active(|r| {
                let to_resolve: Vec<String> = threads::filter(&r.threads, &filter)
                    .iter()
                    .filter(|t| matches!(t.status, ThreadStatus::Open | ThreadStatus::Stale))
                    .map(|t| t.id.clone())
                    .collect();
                for t in r.threads.iter_mut() {
                    if to_resolve.contains(&t.id) {
                        threads::resolve(t);
                    }
                }
                save_threads(r);

                let filtered = threads::filter(&r.threads, &filter);
                if filtered.is_empty() {
                    close_thread_list_win();
                } else {
                    let new_content = build_thread_list_lines(&filtered);
                    let mut b = ra_buf.clone();
                    update_thread_list_buf(&mut b, &new_content);
                    if let Ok(mut guard) = ra_state.lock() {
                        guard.line_map = new_content.line_map;
                    }
                }

                if let Some(p) = r.current_file.clone() {
                    select_file_impl(r, &p);
                }
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<Leader>aa", "", &opts_resolve_all);
}

struct ThreadListContent {
    lines: Vec<String>,
    line_map: Vec<Option<(String, ThreadStatus)>>,
    highlights: Vec<(usize, &'static str)>,
}

fn build_thread_list_lines(threads: &[&threads::Thread]) -> ThreadListContent {
    let inflight = backend::inflight_tag();
    let focused = threads::window_thread_id();
    let mut lines = Vec::new();
    let mut line_map: Vec<Option<(String, ThreadStatus)>> = Vec::new();
    let mut highlights: Vec<(usize, &'static str)> = Vec::new();

    let mut open: Vec<&threads::Thread> = Vec::new();
    let mut stale: Vec<&threads::Thread> = Vec::new();
    let mut resolved: Vec<&threads::Thread> = Vec::new();

    for t in threads {
        match t.status {
            ThreadStatus::Open => open.push(t),
            ThreadStatus::Stale => stale.push(t),
            ThreadStatus::Resolved => resolved.push(t),
        }
    }

    let created_ts = |t: &&threads::Thread| t.messages.first().map(|m| m.ts).unwrap_or(0);
    let sort_newest_first =
        |a: &&threads::Thread, b: &&threads::Thread| created_ts(b).cmp(&created_ts(a));
    open.sort_by(sort_newest_first);
    stale.sort_by(sort_newest_first);
    resolved.sort_by(sort_newest_first);

    let sections: [(&str, &str, &[&threads::Thread]); 3] = [
        ("● Open", "DiagnosticInfo", &open),
        ("✗ Stale", "DiagnosticWarn", &stale),
        ("✓ Resolved", "Comment", &resolved),
    ];

    let mut first_section = true;
    for (label, hl_group, group) in sections {
        if group.is_empty() {
            continue;
        }
        if !first_section {
            lines.push(String::new());
            line_map.push(None);
        }
        first_section = false;

        let header_idx = lines.len();
        lines.push(format!("  {label}"));
        line_map.push(None);
        highlights.push((header_idx, hl_group));

        for t in group {
            let preview = t
                .messages
                .first()
                .map(|m| {
                    m.text
                        .chars()
                        .take(50)
                        .collect::<String>()
                        .replace('\n', " ")
                })
                .unwrap_or_default();
            let is_inflight = inflight.as_deref() == Some(&t.id);
            let is_focused = focused.as_deref() == Some(t.id.as_str());
            let queue_pos = if is_inflight {
                None
            } else {
                backend::queue_position(&t.id)
            };
            let is_agent = t.origin == ThreadOrigin::Agent;
            let activity = if is_inflight {
                " ◐ thinking"
            } else if is_focused {
                " ◀ open"
            } else {
                match queue_pos {
                    Some(0) => " ◌ next",
                    Some(_) => " ◌ queued",
                    None if is_agent => " ▸ agent",
                    None => "",
                }
            };
            let line_idx = lines.len();
            if is_inflight {
                highlights.push((line_idx, "DiagnosticOk"));
            } else if is_focused {
                highlights.push((line_idx, "DiagnosticInfo"));
            } else if queue_pos.is_some() {
                highlights.push((line_idx, "DiagnosticHint"));
            } else if is_agent {
                highlights.push((line_idx, "NonText"));
            }
            lines.push(format!("    {}:{}  {}{activity}", t.file, t.line, preview));
            line_map.push(Some((t.id.clone(), t.status)));
        }
    }

    ThreadListContent {
        lines,
        line_map,
        highlights,
    }
}

fn update_thread_list_buf(buf: &mut nvim_oxi::api::Buffer, content: &ThreadListContent) {
    let bo = OptionOpts::builder().buffer(buf.clone()).build();
    let _ = api::set_option_value("modifiable", true, &bo);
    let lc = buf.line_count().unwrap_or(0);
    let refs: Vec<&str> = content.lines.iter().map(|s| s.as_str()).collect();
    let _ = buf.set_lines(0..lc, false, refs);
    let _ = api::set_option_value("modifiable", false, &bo);
    crate::panel::disable_syntax(buf);
    apply_thread_list_highlights(buf, &content.highlights);
}

fn apply_thread_list_highlights(buf: &mut nvim_oxi::api::Buffer, highlights: &[(usize, &str)]) {
    let ns = api::create_namespace("arbiter-thread-list");
    let _ = buf.clear_namespace(ns, 0..usize::MAX);
    for &(line, hl_group) in highlights {
        let opts = nvim_oxi::api::opts::SetExtmarkOpts::builder()
            .end_col(0)
            .end_row(line + 1)
            .hl_group(hl_group)
            .hl_mode(nvim_oxi::api::types::ExtmarkHlMode::Replace)
            .build();
        let _ = buf.set_extmark(ns, line, 0, &opts);
    }
}

/// Refreshes the thread list popup content if it is currently open.
/// Called when the active thread changes (request starts or queue drains).
pub(super) fn refresh_thread_list() {
    let win_valid =
        THREAD_LIST_WIN.with(|c| c.borrow().as_ref().map(|w| w.is_valid()).unwrap_or(false));
    if !win_valid {
        return;
    }
    let state_arc = THREAD_LIST_STATE.with(|c| c.borrow().clone());
    let Some(state_arc) = state_arc else {
        return;
    };
    let filter = {
        let Ok(guard) = state_arc.lock() else {
            return;
        };
        guard.filter.clone()
    };
    with_active(|r| {
        let filtered = threads::filter(&r.threads, &filter);
        if filtered.is_empty() {
            close_thread_list_win();
            return;
        }
        let new_content = build_thread_list_lines(&filtered);
        THREAD_LIST_BUF.with(|c| {
            if let Some(buf) = c.borrow_mut().as_mut() {
                update_thread_list_buf(buf, &new_content);
            }
        });
        if let Ok(mut guard) = state_arc.lock() {
            guard.line_map = new_content.line_map;
        }
    });
}

fn show_thread_queue_status(tid: &str, prompt_size: Option<usize>) {
    if threads::window_thread_id().as_deref() != Some(tid) {
        return;
    }
    let size_label = prompt_size.map(format_token_estimate).unwrap_or_default();
    let (msg, hl) = match backend::queue_position(tid) {
        Some(pos) => (
            format!("queued ({} ahead){size_label}", pos + 1),
            "DiagnosticHint",
        ),
        None => (format!("agent thinking...{size_label}"), "DiagnosticOk"),
    };
    let _ = threads::append_status_hl(&msg, Some(hl));
}

fn format_token_estimate(chars: usize) -> String {
    let tokens = chars / 4;
    if tokens >= 1000 {
        format!("  ~{:.1}k tokens", tokens as f64 / 1000.0)
    } else {
        format!("  ~{tokens} tokens")
    }
}

pub(super) fn nav_next_thread(review: &mut Review) {
    nav_thread_directed(review, true);
}

pub(super) fn nav_prev_thread(review: &mut Review) {
    nav_thread_directed(review, false);
}

fn nav_thread_directed(review: &mut Review, forward: bool) {
    let file_order: Vec<String> = review.files.iter().map(|(p, _, _)| p.clone()).collect();
    let all_sorted = threads::sorted_global(&review.threads, &file_order);
    let sorted: Vec<usize> = all_sorted
        .into_iter()
        .filter(|&i| {
            review
                .threads
                .get(i)
                .is_some_and(|t| t.status == ThreadStatus::Open)
        })
        .collect();

    if sorted.is_empty() {
        let _ = api::notify(
            "[arbiter] no open threads",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    }

    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let buf_line = row.saturating_sub(1);
    let current = review
        .thread_buf_lines
        .iter()
        .find(|(_, &l)| l == buf_line)
        .and_then(|(tid, _)| review.threads.iter().position(|t| t.id == *tid));

    let target_idx = if forward {
        threads::next_thread(&sorted, current)
    } else {
        threads::prev_thread(&sorted, current)
    };

    if let Some(idx) = target_idx {
        if let Some(t) = review.threads.get(idx) {
            let target_file = t.file.clone();
            let target_id = t.id.clone();
            if target_file != review.current_file.as_deref().unwrap_or_default() {
                navigate_to_file(review, &target_file);
            }
            if let Some(&target_line) = review.thread_buf_lines.get(&target_id) {
                let _ = review.diff_panel.win.set_cursor(target_line + 1, 0);
            }
        }
    }
}
