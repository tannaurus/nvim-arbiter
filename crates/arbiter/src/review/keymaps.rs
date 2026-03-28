//! Keymap bindings for the review workbench.

use super::*;

pub(super) fn set_close_keymap(buf: &mut nvim_oxi::api::Buffer) {
    let opts = SetKeymapOpts::builder()
        .callback(|_| safe_callback(close))
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "q", "", &opts);
}

fn set_thread_list_keymaps(buf: &mut nvim_oxi::api::Buffer, config: &config::Config) {
    let list_threads = config.keymaps.list_threads.clone();

    let opts_list_threads = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_list_threads);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &list_threads, "", &opts_list_threads);
}

pub(super) fn set_file_panel_keymaps(buf: &mut nvim_oxi::api::Buffer, config: &config::Config) {
    let opts = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|review| {
                let (row, _) = review
                    .file_panel
                    .window()
                    .get_cursor()
                    .into_result()
                    .unwrap_or((1, 0));
                let line = row;
                if let Some(path) = review.file_panel.path_at_line(line) {
                    if !ensure_diff_panel(review) {
                        return;
                    }
                    if review.revision_view.is_some() {
                        render_revision_file(review, &path);
                    } else {
                        navigate_to_file(review, &path);
                    }
                    let _ = api::set_current_win(&review.diff_panel.win);
                } else if let Some(dir) = review.file_panel.dir_at_line(line) {
                    review.file_panel.toggle_collapse(&dir);
                    rerender_file_panel(review);
                }
            });
        })
        .noremap(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts);
    set_thread_list_keymaps(buf, config);
}

pub(super) fn set_diff_panel_keymaps(buf: &mut nvim_oxi::api::Buffer, config: &config::Config) {
    let next_hunk = config.keymaps.next_hunk.clone();
    let prev_hunk = config.keymaps.prev_hunk.clone();
    let next_file = config.keymaps.next_file.clone();
    let prev_file = config.keymaps.prev_file.clone();
    let next_thread = config.keymaps.next_thread.clone();
    let prev_thread = config.keymaps.prev_thread.clone();
    let approve = config.keymaps.approve.clone();
    let reset_status = config.keymaps.reset_status.clone();
    let comment = config.keymaps.comment.clone();
    let open_thread = config.keymaps.open_thread.clone();
    let toggle_sbs = config.keymaps.toggle_side_by_side.clone();
    let cancel_request = config.keymaps.cancel_request.clone();
    let next_unreviewed = config.keymaps.next_unreviewed.clone();
    let prev_unreviewed = config.keymaps.prev_unreviewed.clone();
    let accept_hunk = config.keymaps.accept_hunk.clone();
    let active_thread = config.keymaps.active_thread.clone();
    let toggle_diff_style = config.keymaps.toggle_diff_style.clone();
    let file_back = config.keymaps.file_back.clone();
    let find_file = config.keymaps.find_file.clone();
    let grep = config.keymaps.grep.clone();

    let opts_cancel_request = SetKeymapOpts::builder()
        .callback(|_| {
            let had_inflight = backend::inflight_tag();
            let win_tid = threads::window_thread_id();
            let had_queued = win_tid
                .as_ref()
                .and_then(|id| backend::queue_position(id))
                .is_some();
            backend::cancel_all();
            let show_interrupted = match (&had_inflight, &win_tid) {
                (Some(tag), Some(wid)) if tag == wid => true,
                (_, Some(_)) if had_queued => true,
                _ => false,
            };
            if show_interrupted {
                let _ = threads::append_interrupted();
            }
            let _ = api::notify(
                "[arbiter] cancelled pending requests",
                nvim_oxi::api::types::LogLevel::Info,
                &Dictionary::default(),
            );
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &cancel_request, "", &opts_cancel_request);

    let opts_toggle_sbs = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_toggle_sbs);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &toggle_sbs, "", &opts_toggle_sbs);

    let opts_toggle_diff_style = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|review| {
                let next = crate::diff::toggle_style();
                let label = match next {
                    config::DiffStyle::Full => "full",
                    config::DiffStyle::Signs => "signs",
                };
                let _ = api::notify(
                    &format!("[arbiter] diff style: {label}"),
                    nvim_oxi::api::types::LogLevel::Info,
                    &nvim_oxi::Dictionary::default(),
                );
                refresh_file(review);
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(
        Mode::Normal,
        &toggle_diff_style,
        "",
        &opts_toggle_diff_style,
    );

    let opts_next_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_hunk, "", &opts_next_hunk);

    let opts_prev_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_hunk, "", &opts_prev_hunk);

    let opts_next_file = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_file);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_file, "", &opts_next_file);

    let opts_prev_file = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_file);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_file, "", &opts_prev_file);

    let opts_cr = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_diff_cr);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<CR>", "", &opts_cr);

    let opts_approve = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_ga);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &approve, "", &opts_approve);

    let opts_reset = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_gr);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &reset_status, "", &opts_reset);

    let opts_comment = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_immediate_comment);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &comment, "", &opts_comment);

    let opts_open_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_open_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &open_thread, "", &opts_open_thread);

    let opts_active_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(open_active_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &active_thread, "", &opts_active_thread);

    set_thread_list_keymaps(buf, config);

    let opts_next_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_next_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_thread, "", &opts_next_thread);

    let opts_prev_thread = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(nav_prev_thread);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_thread, "", &opts_prev_thread);

    let opts_next_unreviewed = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_next_unreviewed);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &next_unreviewed, "", &opts_next_unreviewed);

    let opts_prev_unreviewed = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_prev_unreviewed);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &prev_unreviewed, "", &opts_prev_unreviewed);

    let opts_accept_hunk = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_accept_hunk);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &accept_hunk, "", &opts_accept_hunk);

    let opts_file_back = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_file_back);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &file_back, "", &opts_file_back);

    let opts_next_rev = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_next_revision);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "]r", "", &opts_next_rev);

    let opts_prev_rev = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_prev_revision);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "[r", "", &opts_prev_rev);

    let opts_find_file = SetKeymapOpts::builder()
        .callback(|_| {
            let _ = api::command("Telescope arbiter review_files");
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &find_file, "", &opts_find_file);

    let opts_grep = SetKeymapOpts::builder()
        .callback(|_| {
            let _ = api::command("Telescope arbiter review_grep");
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, &grep, "", &opts_grep);

    let opts_enter_rev = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(handle_enter_revision_view);
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<Leader>rv", "", &opts_enter_rev);

    let opts_exit_rev = SetKeymapOpts::builder()
        .callback(|_| {
            with_active(|r| {
                if r.revision_view.is_some() {
                    exit_revision_view(r);
                }
            });
            Ok::<(), nvim_oxi::Error>(())
        })
        .noremap(true)
        .nowait(true)
        .silent(true)
        .build();
    let _ = buf.set_keymap(Mode::Normal, "<Esc>", "", &opts_exit_rev);
}
