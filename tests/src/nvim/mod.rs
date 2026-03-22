//! Neovim integration tests using `#[nvim_oxi::test]`.

use arbiter::setup;
use nvim_oxi::api::{self, types::LogLevel};
use nvim_oxi::Dictionary;

#[nvim_oxi::test]
fn nvim_setup_smoke() {
    let opts = Dictionary::default();
    let result = setup(opts);
    assert!(result.is_ok(), "setup should succeed");
    let _ = api::notify(
        "nvim_oxi::test smoke passed",
        LogLevel::Info,
        &Dictionary::default(),
    );
}

#[nvim_oxi::test]
fn nvim_statusline_before_review() {
    let s = arbiter::statusline();
    assert!(s.is_empty(), "statusline should be empty before any review");
}

#[nvim_oxi::test]
fn nvim_setup_with_config() {
    let opts = Dictionary::from_iter([
        ("backend", nvim_oxi::Object::from("cursor")),
        ("inline_indicators", nvim_oxi::Object::from(false)),
    ]);
    let result = setup(opts);
    assert!(result.is_ok(), "setup with config should succeed");
}

#[nvim_oxi::test]
fn nvim_commands_registered_after_setup() {
    let opts = Dictionary::default();
    setup(opts).expect("setup");
    let result: i32 = api::call_function("exists", (":ArbiterCompare",))
        .expect("exists(:ArbiterCompare) should succeed");
    assert_eq!(
        result, 2,
        "ArbiterCompare should be registered as a user command"
    );
}
