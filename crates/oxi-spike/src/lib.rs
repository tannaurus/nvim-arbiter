//! E0-0 nvim-oxi API spike.
//!
//! Minimal cdylib validating 7 touchpoints:
//! 1. Timer via libuv, 2. buffer-local keymap with callback,
//! 3. user command with nargs, 4. schedule from background thread,
//! 5. extmarks with sign_text/virt_text, 6. floating window,
//! 7. #[nvim_oxi::test] in separate test crate (tests/).
//!
//! This crate can be discarded after validation. See docs/e0-0-workarounds.md.

use nvim_oxi::api::{self, types::Mode, LogLevel};
use nvim_oxi::libuv::TimerHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Runs all 6 in-process spike validations (7th is in tests crate).
fn run_spike() -> nvim_oxi::Result<()> {
    // (1) Timer via libuv
    let count = AtomicU32::new(0);
    let count_clone = std::sync::Arc::new(count);
    let cc = count_clone.clone();
    let timer = TimerHandle::start(
        Duration::ZERO,
        Duration::from_millis(50),
        move |t: &mut TimerHandle| {
            let n = cc.fetch_add(1, Ordering::SeqCst);
            if n >= 2 {
                let _ = t.stop();
            }
        },
    )?;
    std::thread::sleep(Duration::from_millis(150));
    drop(timer);
    api::notify("Spike: Timer OK", LogLevel::Info);

    // (2) Buffer-local keymap with callback
    let mut buf = api::create_buf(false, true)?;
    buf.set_lines(.., false, ["Press <C-g>"]).into_result()?;
    let opts = api::opts::SetKeymapOpts::builder()
        .callback(|| api::notify("Spike: Keymap OK", LogLevel::Info))
        .noremap(true)
        .silent(true)
        .build();
    buf.set_keymap(Mode::Normal, "<C-g>", "", &opts).into_result()?;
    api::set_current_buf(&buf).into_result()?;

    // (3) User command with nargs - registered below

    // (4) schedule from background thread
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        nvim_oxi::schedule(move |_| {
            api::notify("Spike: schedule OK", LogLevel::Info);
            let _ = tx.send(());
        });
    });
    rx.recv_timeout(Duration::from_secs(2)).map_err(|_| {
        nvim_oxi::Error::from("schedule timeout")
    })?;

    // (5) Extmarks with sign_text and virt_text
    let ns = api::create_namespace("spike", true).unwrap_or(0);
    let ext_opts = api::opts::SetExtmarkOpts::builder()
        .sign_text("S")
        .sign_hl_group("Error")
        .virt_text([("virt", "Comment")])
        .virt_text_pos(api::types::ExtmarkVirtTextPosition::Overlay)
        .build();
    buf.set_extmark(ns, 0, 0, &ext_opts).into_result()?;
    api::notify("Spike: Extmarks OK", LogLevel::Info);

    // (6) Floating window
    let win_opts = api::opts::OpenWinOpts::builder()
        .relative(api::types::WindowRelativeTo::Editor)
        .width(40)
        .height(5)
        .row(2.0)
        .col(2.0)
        .border("rounded")
        .title("Spike Float")
        .build();
    let _win = api::open_win(&buf, false, &win_opts)?;
    api::notify("Spike: Float OK", LogLevel::Info);

    Ok(())
}

#[nvim_oxi::plugin]
fn oxi_spike() -> nvim_oxi::Result<()> {
    api::create_user_command(
        "SpikeTest",
        run_spike,
        &api::opts::CreateCommandOpts::builder()
            .nargs(api::types::CommandNargs::None)
            .build(),
    )?;

    api::create_user_command(
        "SpikeArgs",
        |args: api::CommandArgs| {
            let v: Vec<String> = args.args().map(|s| s.to_string()).collect();
            api::notify(&format!("Spike: got {} args: {:?}", v.len(), v), LogLevel::Info);
        },
        &api::opts::CreateCommandOpts::builder()
            .nargs(api::types::CommandNargs::Any)
            .build(),
    )?;

    api::notify("oxi-spike loaded", LogLevel::Info);
    Ok(())
}
