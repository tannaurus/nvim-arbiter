//! Revision view mode for inspecting individual thread revisions.

use super::*;

/// Captures a revision on the thread if the agent modified any files.
///
/// All file I/O and git commands run on a background thread. The result
/// is dispatched back to the main thread to update the review state.
pub(super) fn maybe_capture_revision(
    review: &mut Review,
    thread_id: &str,
    before_snapshot: &Option<BeforeSnapshot>,
) {
    let Some((cwd, ref_name, before)) = before_snapshot else {
        return;
    };
    let Some(t) = review.threads.iter().find(|t| t.id == thread_id) else {
        return;
    };
    let msg_idx = t.messages.len().saturating_sub(1);
    let rev_next_index = t.revisions.len() as u32 + 1;
    let paths: Vec<String> = review.files.iter().map(|(p, _, _)| p.clone()).collect();
    let cwd = cwd.clone();
    let ref_name = ref_name.clone();
    let before = before.clone();
    let tid = thread_id.to_string();
    std::thread::spawn(move || {
        let mut before = before;
        let mut after = revision::snapshot_files(&cwd, &paths);
        let new_paths = revision::diff_names_sync(&cwd, &ref_name);
        for path in &new_paths {
            if !after.contains_key(path) {
                let full = Path::new(&cwd).join(path);
                after.insert(path.clone(), std::fs::read_to_string(&full).ok());
            }
            if !before.contains_key(path) {
                before.insert(path.clone(), git::show_sync(&cwd, &ref_name, path));
            }
        }
        crate::dispatch::schedule(move || {
            with_active(|r| {
                let Some(t) = r.threads.iter().find(|t| t.id == tid) else {
                    return;
                };
                let Some(rev) = revision::build_revision(t, &before, &after, &new_paths, msg_idx)
                else {
                    return;
                };
                let rev_index = rev.index;
                let file_count = rev.files.len();
                let stats: Vec<(String, usize, usize)> = rev
                    .files
                    .iter()
                    .map(|rf| {
                        let (a, rm) = revision::revision_file_stats(rf);
                        (rf.path.clone(), a, rm)
                    })
                    .collect();
                if let Some(t) = r.threads.iter_mut().find(|t| t.id == tid) {
                    t.revisions.push(rev);
                }
                if threads::window_thread_id().as_deref() == Some(tid.as_str()) {
                    let _ = threads::append_revision_summary(rev_index, file_count, &stats);
                }
                let state_dir = r.config.state_dir();
                let ws_hash = state::workspace_hash(Path::new(&r.cwd));
                state::save_threads(&state_dir, &ws_hash, &r.ref_name, &r.threads);
            });
        });
    });
    let _ = rev_next_index;
}

fn enter_revision_view(review: &mut Review, thread_id: &str, rev_index: u32) {
    let Some(t) = review.threads.iter().find(|t| t.id == thread_id) else {
        api::err_writeln("[arbiter] thread not found");
        return;
    };
    let Some(rev) = t.revisions.iter().find(|r| r.index == rev_index) else {
        api::err_writeln("[arbiter] revision not found");
        return;
    };

    let files: Vec<(String, FileStatus, ReviewStatus)> = rev
        .files
        .iter()
        .map(|rf| {
            let status = if rf.before.is_none() {
                FileStatus::Added
            } else if rf.after.is_none() {
                FileStatus::Deleted
            } else {
                FileStatus::Modified
            };
            (rf.path.clone(), status, ReviewStatus::Unreviewed)
        })
        .collect();

    let first_file = files.first().map(|(p, _, _)| p.clone());
    let saved_file = review.current_file.clone();

    review.revision_view = Some(RevisionViewState {
        thread_id: thread_id.to_string(),
        revision_index: rev_index,
        files: files.clone(),
        current_file: first_file.clone(),
        accepted_hunks: HashMap::new(),
        saved_file,
    });

    let open_thread_counts = HashMap::new();
    let _ = review.file_panel.render(&files, &open_thread_counts);

    if let Some(path) = first_file {
        render_revision_file(review, &path);
    }
}

pub(super) fn exit_revision_view(review: &mut Review) {
    let saved_file = review
        .revision_view
        .as_ref()
        .and_then(|rv| rv.saved_file.clone());
    review.revision_view = None;

    let open_thread_counts = open_thread_count_map(&review.threads);
    let _ = review.file_panel.render(&review.files, &open_thread_counts);

    if let Some(path) = saved_file.or_else(|| review.current_file.clone()) {
        select_file_impl(review, &path);
    }
}

pub(super) fn render_revision_file(review: &mut Review, path: &str) {
    let Some(rv) = review.revision_view.as_mut() else {
        return;
    };
    rv.current_file = Some(path.to_string());
    review.file_panel.highlight_file(path);

    let rev_data = {
        let t = review.threads.iter().find(|t| t.id == rv.thread_id);
        t.and_then(|t| t.revisions.iter().find(|r| r.index == rv.revision_index))
            .and_then(|r| r.files.iter().find(|f| f.path == path))
            .cloned()
    };
    let Some(rf) = rev_data else {
        return;
    };

    let diff_text = revision::revision_file_diff(&rf);
    review.current_diff_text = diff_text.clone();

    let accepted = rv.accepted_hunks.get(path).cloned().unwrap_or_default();
    let summaries = Vec::new();
    if let Ok(diff::RenderResult {
        hunks,
        thread_buf_lines,
        lines: diff_lines,
    }) = diff::render(
        &mut review.diff_panel.buf,
        &diff_text,
        &summaries,
        path,
        false,
        None,
        &accepted,
    ) {
        review.current_hunks = hunks.clone();
        review.current_diff_lines = diff_lines;
        review.thread_buf_lines = thread_buf_lines;
        let accepted_buf_starts: HashSet<usize> = hunks
            .iter()
            .filter(|h| accepted.contains(&h.content_hash))
            .map(|h| h.buf_start)
            .collect();
        if !hunks.is_empty() {
            let _ = diff::set_hunk_folds(
                &mut review.diff_panel.buf,
                &review.diff_panel.win,
                &hunks,
                false,
                &accepted_buf_starts,
            );
        }
    }

    let rv = review.revision_view.as_ref();
    if let Some(rv) = rv {
        let total = review
            .threads
            .iter()
            .find(|t| t.id == rv.thread_id)
            .map(|t| t.revisions.len() as u32)
            .unwrap_or(0);
        let title = format!(" {path} (revision {} of {total})", rv.revision_index);
        let win_opts = OptionOpts::builder()
            .win(review.diff_panel.win.clone())
            .build();
        let _ = api::set_option_value("winbar", title.as_str(), &win_opts);
    }
}

pub(super) fn handle_next_revision(review: &mut Review) {
    if let Some(rv) = &review.revision_view {
        let tid = rv.thread_id.clone();
        let next = rv.revision_index + 1;
        let exists = review
            .threads
            .iter()
            .find(|t| t.id == tid)
            .map(|t| t.revisions.iter().any(|r| r.index == next))
            .unwrap_or(false);
        if exists {
            enter_revision_view(review, &tid, next);
        } else {
            api::err_writeln("[arbiter] no next revision");
        }
    } else {
        handle_enter_revision_view(review);
    }
}

pub(super) fn handle_prev_revision(review: &mut Review) {
    if let Some(rv) = &review.revision_view {
        if rv.revision_index <= 1 {
            api::err_writeln("[arbiter] no previous revision");
            return;
        }
        let tid = rv.thread_id.clone();
        let prev = rv.revision_index - 1;
        enter_revision_view(review, &tid, prev);
    } else {
        handle_enter_revision_view(review);
    }
}

pub(super) fn handle_revision_selected(review: &mut Review, rev_index: u32) {
    let tid = threads::window_thread_id();
    let Some(tid) = tid else {
        return;
    };
    enter_revision_view(review, &tid, rev_index);
}

pub(super) fn gather_similar_context(
    all_threads: &[threads::Thread],
    similar_refs: &[threads::SimilarRef],
) -> Vec<prompts::SimilarThreadContext> {
    similar_refs
        .iter()
        .filter_map(|sr| {
            let sibling = all_threads.iter().find(|t| t.id == sr.thread_id)?;
            if sibling.messages.len() < 2 {
                return None;
            }
            let messages = sibling
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
            let status = format!("{}", sibling.status);
            Some(prompts::SimilarThreadContext {
                file: sibling.file.clone(),
                line: sibling.line,
                status,
                messages,
            })
        })
        .collect()
}

pub(super) fn handle_similar_selected(review: &mut Review, target_thread_id: &str) {
    let target = review
        .threads
        .iter()
        .find(|t| t.id == target_thread_id)
        .cloned();
    let Some(t) = target else {
        return;
    };
    navigate_to_file(review, &t.file);
    open_thread_panel(review, &t);
}

pub(super) fn handle_enter_revision_view(review: &mut Review) {
    let tid = threads::window_thread_id();
    let Some(tid) = tid else {
        api::err_writeln("[arbiter] no thread open");
        return;
    };
    let Some(t) = review.threads.iter().find(|t| t.id == tid) else {
        return;
    };
    if t.revisions.is_empty() {
        let _ = api::notify(
            "[arbiter] this thread has no revisions",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        return;
    }
    enter_revision_view(review, &tid, 1);
}

/// Queues a rule-extraction call at the front of the backend queue
/// so it runs before the next pending thread reply.
pub(super) fn maybe_queue_extraction(review: &Review, thread_id: &str) {
    if !review.learn_rules {
        return;
    }
    let Some(t) = review.threads.iter().find(|t| t.id == thread_id) else {
        return;
    };
    let messages: Vec<(String, String)> = t
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
    let existing_rules = review.review_rules.clone();
    let Some(prompt) = prompts::format_extraction_prompt(&messages, &existing_rules) else {
        return;
    };
    let tid = thread_id.to_string();
    backend::send_priority(
        crate::types::BackendOpts {
            op: crate::types::BackendOp::NewSession,
            prompt,
            ask_mode: true,
            stream: false,
            json_schema: None,
        },
        Box::new(move |res| {
            if res.error.is_some() {
                return;
            }
            let actions = prompts::parse_extraction_response(&res.text);
            if actions.is_empty() {
                return;
            }
            with_active(|r| {
                let mut changed = Vec::new();
                for action in actions {
                    match action {
                        prompts::ExtractionAction::Add(rule) => {
                            if !r.review_rules.contains(&rule) {
                                r.review_rules.push(rule.clone());
                                changed.push(rule);
                            }
                        }
                        prompts::ExtractionAction::Rephrase { old, new } => {
                            if let Some(pos) = r.review_rules.iter().position(|r| *r == old) {
                                r.review_rules[pos] = new.clone();
                                changed.push(format!("{old} -> {new}"));
                            }
                        }
                    }
                }
                if !changed.is_empty() {
                    save_file_statuses(r);
                    if threads::window_thread_id().as_deref() == Some(tid.as_str()) {
                        let _ = threads::append_learned_rules(&changed);
                    }
                }
            });
        }),
    );
}
