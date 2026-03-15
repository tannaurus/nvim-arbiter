//! Neovim integration tests using `#[nvim_oxi::test]`.

use arbiter::setup;
use nvim_oxi::api::{self, types::LogLevel};
use nvim_oxi::Dictionary;

/// Smoke test: setup() runs without error inside Neovim.
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
