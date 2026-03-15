//! Integration and unit tests for arbiter.
//!
//! Uses `#[nvim_oxi::test]` for Neovim API integration tests and
//! standard `#[test]` for pure Rust unit tests.

pub mod e2e;
pub mod fixtures;
pub mod helpers;
pub mod nvim;
pub mod unit;
