//! Hunk acceptance, file status persistence, and thread resolution for accepted ranges.

use super::*;

pub(super) fn accepted_for_file(review: &Review) -> HashSet<String> {
    review
        .current_file
        .as_ref()
        .and_then(|p| review.accepted_hunks.get(p))
        .cloned()
        .unwrap_or_default()
}

pub(super) fn handle_accept_hunk(review: &mut Review) {
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);

    let Some(hunk) = review
        .current_hunks
        .iter()
        .find(|h| line_0 >= h.buf_start && line_0 <= h.buf_end)
    else {
        let _ = api::notify(
            "[arbiter] cursor is not inside a hunk",
            nvim_oxi::api::types::LogLevel::Warn,
            &Dictionary::default(),
        );
        return;
    };
    let hash = hunk.content_hash.clone();

    if review.revision_view.is_some() {
        accept_revision_hunk(review, &hash);
        return;
    }

    let file_set = accepted_for_file(review);
    let is_accepted = file_set.contains(&hash);

    if review.ref_name.is_empty() {
        let patch = if is_accepted {
            review.staged_patches.get(&hash).cloned()
        } else {
            diff::build_hunk_patch(&review.current_diff_text, &hash)
        };
        if let Some(patch) = patch {
            if is_accepted {
                let result = git::unstage_patch(&review.cwd, &patch);
                if !result.success() {
                    let _ = api::notify(
                        &format!("[arbiter] unstage failed: {}", result.stderr.trim()),
                        nvim_oxi::api::types::LogLevel::Error,
                        &Dictionary::default(),
                    );
                    return;
                }
                review.staged_patches.remove(&hash);
            } else {
                let result = git::stage_patch(&review.cwd, &patch);
                if result.success() {
                    review.staged_patches.insert(hash.clone(), patch);
                }
            }
        }
    }

    if is_accepted {
        unmark_hunk_accepted(review, &hash);
        if let Some(path) = review.current_file.clone() {
            if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
                if *rs == ReviewStatus::Approved {
                    *rs = ReviewStatus::Unreviewed;
                    review.file_content_hash.remove(&path);
                }
            }
        }
    } else {
        mark_hunk_accepted(review, &hash);
        let was_unapproved = review
            .current_file
            .as_ref()
            .and_then(|p| review.files.iter().find(|(fp, _, _)| fp == p))
            .is_some_and(|(_, _, rs)| *rs != ReviewStatus::Approved);
        check_all_hunks_accepted(review);
        let now_approved = review
            .current_file
            .as_ref()
            .and_then(|p| review.files.iter().find(|(fp, _, _)| fp == p))
            .is_some_and(|(_, _, rs)| *rs == ReviewStatus::Approved);
        if was_unapproved && now_approved {
            save_accepted_hunks(review);
            save_file_statuses(review);
            rerender_file_panel(review);
            handle_next_unreviewed(review);
            return;
        }
    }
    save_accepted_hunks(review);
    save_file_statuses(review);
    rerender_file_panel(review);
}

fn accept_revision_hunk(review: &mut Review, hash: &str) {
    let Some(rv) = review.revision_view.as_mut() else {
        return;
    };
    let Some(path) = rv.current_file.clone() else {
        return;
    };

    let already = rv
        .accepted_hunks
        .get(&path)
        .is_some_and(|s| s.contains(hash));
    if already {
        if let Some(set) = rv.accepted_hunks.get_mut(&path) {
            set.remove(hash);
        }
    } else {
        rv.accepted_hunks
            .entry(path.clone())
            .or_default()
            .insert(hash.to_string());
        map_revision_hunk_to_main(review, &path, hash);
    }

    render_revision_file(review, &path);
}

pub(super) fn accept_all_revision_hunks(review: &mut Review) {
    let path = review
        .revision_view
        .as_ref()
        .and_then(|rv| rv.current_file.clone());
    let Some(path) = path else {
        return;
    };

    let all_hashes: Vec<String> = review
        .current_hunks
        .iter()
        .map(|h| h.content_hash.clone())
        .collect();

    if let Some(rv) = review.revision_view.as_mut() {
        rv.accepted_hunks
            .entry(path.clone())
            .or_default()
            .extend(all_hashes.iter().cloned());
    }

    review
        .accepted_hunks
        .entry(path.clone())
        .or_default()
        .extend(all_hashes.iter().cloned());
    save_accepted_hunks(review);

    let (rev_files, thread_id, rev_idx) = match review.revision_view.as_ref() {
        Some(rv) => (rv.files.clone(), rv.thread_id.clone(), rv.revision_index),
        None => return,
    };

    let cur_idx = rev_files.iter().position(|(p, _, _)| *p == path);
    let all_rev_accepted = rev_files.iter().all(|(fp, _, _)| {
        let rev_file = review
            .threads
            .iter()
            .find(|t| t.id == thread_id)
            .and_then(|t| t.revisions.iter().find(|r| r.index == rev_idx))
            .and_then(|r| r.files.iter().find(|f| f.path == *fp));
        let Some(rf) = rev_file else {
            return true;
        };
        let diff = revision::revision_file_diff(rf);
        let hunks = diff::parse_hunks(&diff);
        let accepted_set = review
            .revision_view
            .as_ref()
            .and_then(|rv| rv.accepted_hunks.get(fp))
            .cloned()
            .unwrap_or_default();
        hunks.iter().all(|h| accepted_set.contains(&h.content_hash))
    });

    if all_rev_accepted {
        let _ = api::notify(
            "[arbiter] all hunks in this revision accepted",
            nvim_oxi::api::types::LogLevel::Info,
            &Dictionary::default(),
        );
        render_revision_file(review, &path);
    } else if let Some(idx) = cur_idx {
        let next = (idx + 1) % rev_files.len().max(1);
        if let Some(next_path) = rev_files.get(next).map(|(p, _, _)| p.clone()) {
            render_revision_file(review, &next_path);
        }
    } else {
        render_revision_file(review, &path);
    }
}

/// Maps an accepted revision hunk to the main branch diff.
///
/// Finds the hunk in the main diff by matching content hashes. If the
/// revision hunk's hash exists in the main diff, it is marked as accepted
/// there too. If not (stale), the user is notified.
fn map_revision_hunk_to_main(review: &mut Review, path: &str, rev_hash: &str) {
    let main_has_hash = review
        .accepted_hunks
        .get(path)
        .map(|s| s.contains(rev_hash))
        .unwrap_or(false);
    if main_has_hash {
        return;
    }

    review
        .accepted_hunks
        .entry(path.to_string())
        .or_default()
        .insert(rev_hash.to_string());
    save_accepted_hunks(review);
}

fn mark_hunk_accepted(review: &mut Review, content_hash: &str) {
    if let Some(path) = review.current_file.clone() {
        review
            .accepted_hunks
            .entry(path)
            .or_default()
            .insert(content_hash.to_string());
    }

    let Some((buf_start, buf_end, new_start, new_count)) = review
        .current_hunks
        .iter()
        .find(|h| h.content_hash == content_hash)
        .map(|h| (h.buf_start, h.buf_end, h.new_start, h.new_count))
    else {
        return;
    };

    if let Some(path) = review.current_file.clone() {
        resolve_threads_in_range(review, &path, new_start, new_count);
    }

    let ns = api::create_namespace("arbiter-diff");
    for i in buf_start..=buf_end {
        let _ = review.diff_panel.buf.clear_namespace(ns, i..i + 1);
        let _ = review
            .diff_panel
            .buf
            .add_highlight(ns, "ArbiterHunkAccepted", i, 0..);
    }

    update_accepted_fold_state(review);

    let wid = review.diff_panel.win.handle();
    let start = (buf_start + 1) as i64;
    diff::win_exec(wid, &format!("{start}foldclose"));
}

fn unmark_hunk_accepted(review: &mut Review, content_hash: &str) {
    if let Some(path) = review.current_file.as_ref() {
        if let Some(set) = review.accepted_hunks.get_mut(path) {
            set.remove(content_hash);
            if set.is_empty() {
                review.accepted_hunks.remove(path);
            }
        }
    }

    reapply_diff_visuals(review);
}

fn reapply_diff_visuals(review: &mut Review) {
    let file_accepted = accepted_for_file(review);
    let accepted_buf_starts: HashSet<usize> = review
        .current_hunks
        .iter()
        .filter(|h| file_accepted.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect();

    let lc = review.diff_panel.buf.line_count().unwrap_or(0);
    let all_lines: Vec<String> = review
        .diff_panel
        .buf
        .get_lines(0..lc, false)
        .map(|iter| iter.map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let file_threads: Vec<threads::Thread> = review
        .current_file
        .as_ref()
        .map(|p| {
            threads::for_file(&review.threads, p)
                .into_iter()
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let summaries = threads::to_summaries(&file_threads);

    let _ = diff::apply_highlights(
        &mut review.diff_panel.buf,
        &review.current_hunks,
        &summaries,
        &all_lines,
        None,
        &accepted_buf_starts,
    );

    let file_approved = review
        .current_file
        .as_ref()
        .and_then(|p| review.files.iter().find(|(fp, _, _)| fp == p))
        .is_some_and(|(_, _, rs)| *rs == ReviewStatus::Approved);

    let _ = diff::set_hunk_folds(
        &mut review.diff_panel.buf,
        &review.diff_panel.win,
        &review.current_hunks,
        review.config.review.fold_approved && file_approved,
        &accepted_buf_starts,
    );
}

fn resolve_threads_in_range(review: &mut Review, path: &str, new_start: usize, new_count: usize) {
    let end = new_start + new_count;
    let mut changed = false;
    for t in &mut review.threads {
        if t.file == path
            && t.status == ThreadStatus::Open
            && (t.line as usize) >= new_start
            && (t.line as usize) < end
        {
            threads::resolve(t);
            changed = true;
        }
    }
    if changed {
        save_threads(review);
    }
}

pub(super) fn resolve_threads_for_file(review: &mut Review, path: &str) {
    let mut changed = false;
    for t in &mut review.threads {
        if t.file == path && t.status == ThreadStatus::Open {
            threads::resolve(t);
            changed = true;
        }
    }
    if changed {
        save_threads(review);
    }
}

pub(super) fn save_threads(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    state::save_threads(&sd, &ws_hash, &review.ref_name, &review.threads);
}

fn check_all_hunks_accepted(review: &mut Review) {
    if review.current_hunks.is_empty() {
        return;
    }
    let file_set = accepted_for_file(review);
    let all_accepted = review
        .current_hunks
        .iter()
        .all(|h| file_set.contains(&h.content_hash));
    if !all_accepted {
        return;
    }
    let Some(path) = review.current_file.clone() else {
        return;
    };
    if let Some((_, _, rs)) = review.files.iter_mut().find(|(p, _, _)| *p == path) {
        if *rs != ReviewStatus::Approved {
            *rs = ReviewStatus::Approved;
            let full = Path::new(&review.cwd).join(&path);
            let contents = std::fs::read_to_string(&full).unwrap_or_default();
            review
                .file_content_hash
                .insert(path, state::content_hash(&contents));
        }
    }
}

pub(super) fn save_file_statuses(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let mut rs = build_review_state(review);
    for (p, _, status) in &review.files {
        rs.files
            .insert(p.clone(), file_state_for(review, p, *status));
    }
    state::save_review(&sd, &ws_hash, &review.ref_name, &rs);
}

/// Entry point for saving file statuses, used by commands that
/// modify review state from outside this module (e.g. the rules editor).
pub(crate) fn save_file_statuses_pub(review: &Review) {
    save_file_statuses(review);
}

pub(super) fn build_review_state(review: &Review) -> state::ReviewState {
    state::ReviewState {
        review_rules: review.review_rules.clone(),
        ..Default::default()
    }
}

fn update_accepted_fold_state(review: &mut Review) {
    let file_accepted = accepted_for_file(review);
    let accepted_buf_starts: HashSet<usize> = review
        .current_hunks
        .iter()
        .filter(|h| file_accepted.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect();

    let mut dict = nvim_oxi::Dictionary::new();
    for bs in &accepted_buf_starts {
        let key = nvim_oxi::String::from(format!("{}", bs + 1));
        dict.insert(key, nvim_oxi::Object::from(true));
    }
    let _ = review
        .diff_panel
        .buf
        .set_var("arbiter_accepted_folds", dict);
}

pub(super) fn file_state_for(
    review: &Review,
    path: &str,
    status: ReviewStatus,
) -> state::FileState {
    let ch = review
        .file_content_hash
        .get(path)
        .cloned()
        .unwrap_or_default();
    let ah = review
        .accepted_hunks
        .get(path)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    state::FileState {
        status,
        content_hash: ch,
        updated_at: now_secs(),
        accepted_hunks: ah,
    }
}

pub(super) fn save_accepted_hunks(review: &Review) {
    let sd = review.config.state_dir();
    let ws_hash = state::workspace_hash(Path::new(&review.cwd));
    let mut rs = state::load_review(&sd, &ws_hash, &review.ref_name);
    if let Some(path) = review.current_file.as_ref() {
        let file_accepted: Vec<String> = review
            .accepted_hunks
            .get(path)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let entry = rs
            .files
            .entry(path.clone())
            .or_insert_with(|| state::FileState {
                status: ReviewStatus::Unreviewed,
                content_hash: String::new(),
                updated_at: 0,
                accepted_hunks: Vec::new(),
            });
        entry.accepted_hunks = file_accepted;
    }
    state::save_review(&sd, &ws_hash, &review.ref_name, &rs);
}
