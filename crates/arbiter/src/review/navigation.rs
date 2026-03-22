//! Hunk and file navigation in the diff panel.

use super::*;

pub(super) fn nav_next_hunk(review: &mut Review) {
    let accepted = accepted_for_file(review);
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);
    let next = review
        .current_hunks
        .iter()
        .find(|h| h.buf_start > line_0 && !accepted.contains(&h.content_hash));
    if let Some(hunk) = next {
        scroll_to_hunk(review, hunk.buf_start, hunk.buf_end);
        return;
    }
    nav_hunk_cross_file(review, true);
}

pub(super) fn nav_prev_hunk(review: &mut Review) {
    let accepted = accepted_for_file(review);
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);
    let prev = review
        .current_hunks
        .iter()
        .rfind(|h| h.buf_end < line_0 && !accepted.contains(&h.content_hash));
    if let Some(hunk) = prev {
        scroll_to_hunk(review, hunk.buf_start, hunk.buf_end);
        return;
    }
    nav_hunk_cross_file(review, false);
}

fn nav_hunk_cross_file(review: &mut Review, forward: bool) {
    let Some(path) = review.current_file.clone() else {
        return;
    };
    let idx = review.file_index.get(&path).copied().unwrap_or(0);
    let len = review.files.len();
    if len == 0 {
        return;
    }
    let next_idx = if forward {
        (idx + 1) % len
    } else if idx == 0 {
        len - 1
    } else {
        idx - 1
    };
    if let Some((next_path, _, _)) = review.files.get(next_idx) {
        let next_path = next_path.clone();
        review.pending_hunk_nav = Some(forward);
        navigate_to_file(review, &next_path);
    }
}

fn scroll_to_hunk(review: &mut Review, buf_start: usize, buf_end: usize) {
    let wid = review.diff_panel.win.handle();
    let start_1 = (buf_start + 1) as i64;
    let end_1 = (buf_end + 1) as i64;
    let _ = review.diff_panel.win.set_cursor(buf_end + 1, 0);
    diff::win_exec(wid, &format!("normal! {start_1}Gzt{end_1}G"));
}

pub(super) fn apply_pending_hunk_nav(review: &mut Review) {
    let Some(forward) = review.pending_hunk_nav.take() else {
        return;
    };
    let accepted = accepted_for_file(review);
    let unaccepted: Vec<_> = review
        .current_hunks
        .iter()
        .filter(|h| !accepted.contains(&h.content_hash))
        .collect();
    let target = if forward {
        unaccepted.first()
    } else {
        unaccepted.last()
    };
    if let Some(hunk) = target {
        scroll_to_hunk(review, hunk.buf_start, hunk.buf_end);
    }
}

pub(super) fn apply_pending_scroll_top(review: &mut Review) {
    if !review.pending_scroll_top {
        return;
    }
    review.pending_scroll_top = false;
    let _ = review.diff_panel.win.set_cursor(1, 0);
}

/// Moves the cursor to the first hunk if it is not already inside one.
///
/// Called after rendering so the cursor never sits on a thread summary
/// or file header line where `<Leader>as` and `<Leader>aa` would silently
/// do the wrong thing.
pub(super) fn snap_cursor_to_hunk(review: &mut Review, hunks: &[Hunk]) {
    if hunks.is_empty() {
        return;
    }
    let (row, _) = review
        .diff_panel
        .win
        .get_cursor()
        .into_result()
        .unwrap_or((1, 0));
    let line_0 = row.saturating_sub(1);
    let inside = hunks
        .iter()
        .any(|h| line_0 >= h.buf_start && line_0 <= h.buf_end);
    if !inside {
        let _ = review.diff_panel.win.set_cursor(hunks[0].buf_start + 1, 0);
    }
}

pub(super) fn nav_next_file(review: &mut Review) {
    if let Some(rv) = &review.revision_view {
        let cur = rv.current_file.as_deref();
        let files = &rv.files;
        let idx = cur
            .and_then(|c| files.iter().position(|(p, _, _)| p == c))
            .unwrap_or(0);
        let next = (idx + 1) % files.len().max(1);
        if let Some(path) = files.get(next).map(|(p, _, _)| p.clone()) {
            render_revision_file(review, &path);
        }
        return;
    }
    let Some(path) = review.current_file.clone() else {
        return;
    };
    let idx = review.file_index.get(&path).copied().unwrap_or(0);
    let next_idx = (idx + 1) % review.files.len().max(1);
    let path = review.files.get(next_idx).map(|(p, _, _)| p.clone());
    if let Some(path) = path {
        navigate_to_file(review, &path);
    }
}

pub(super) fn nav_prev_file(review: &mut Review) {
    if let Some(rv) = &review.revision_view {
        let cur = rv.current_file.as_deref();
        let files = &rv.files;
        let idx = cur
            .and_then(|c| files.iter().position(|(p, _, _)| p == c))
            .unwrap_or(0);
        let prev = if idx == 0 {
            files.len().saturating_sub(1)
        } else {
            idx - 1
        };
        if let Some(path) = files.get(prev).map(|(p, _, _)| p.clone()) {
            render_revision_file(review, &path);
        }
        return;
    }
    let Some(path) = review.current_file.clone() else {
        return;
    };
    let idx = review.file_index.get(&path).copied().unwrap_or(0);
    let prev_idx = if idx == 0 {
        review.files.len().saturating_sub(1)
    } else {
        idx - 1
    };
    let path = review.files.get(prev_idx).map(|(p, _, _)| p.clone());
    if let Some(path) = path {
        navigate_to_file(review, &path);
    }
}

pub(super) fn open_thread_count_map(threads: &[threads::Thread]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for t in threads {
        if t.status == ThreadStatus::Open {
            *counts.entry(t.file.clone()).or_default() += 1;
        }
    }
    counts
}

fn file_has_open_threads(threads: &[threads::Thread], path: &str) -> bool {
    threads
        .iter()
        .any(|t| t.file == path && t.status == ThreadStatus::Open)
}

fn is_unreviewed_file(review: &Review, idx: usize) -> bool {
    let Some((path, _, rs)) = review.files.get(idx) else {
        return false;
    };
    if *rs != ReviewStatus::Approved {
        return true;
    }
    file_has_open_threads(&review.threads, path)
}

pub(super) fn handle_next_unreviewed(review: &mut Review) {
    if review.files.is_empty() {
        return;
    }
    let current_idx = review
        .current_file
        .as_ref()
        .and_then(|p| review.file_index.get(p).copied())
        .unwrap_or(0);
    let len = review.files.len();
    for offset in 1..=len {
        let idx = (current_idx + offset) % len;
        if is_unreviewed_file(review, idx) {
            if let Some((path, _, _)) = review.files.get(idx) {
                let path = path.clone();
                review.pending_scroll_top = true;
                navigate_to_file(review, &path);
            }
            return;
        }
    }
}

pub(super) fn handle_prev_unreviewed(review: &mut Review) {
    if review.files.is_empty() {
        return;
    }
    let current_idx = review
        .current_file
        .as_ref()
        .and_then(|p| review.file_index.get(p).copied())
        .unwrap_or(0);
    let len = review.files.len();
    for offset in 1..=len {
        let idx = (current_idx + len - offset) % len;
        if is_unreviewed_file(review, idx) {
            if let Some((path, _, _)) = review.files.get(idx) {
                let path = path.clone();
                review.pending_scroll_top = true;
                navigate_to_file(review, &path);
            }
            return;
        }
    }
}

pub(super) fn handle_file_back(review: &mut Review) {
    let Some(path) = review.file_history.pop() else {
        return;
    };
    select_file_impl(review, &path);
    rerender_file_panel(review);
}
