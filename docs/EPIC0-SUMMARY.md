# Epic 0 Foundation: Summary

## What Was Built

### E0-0: nvim-oxi API Spike

- **Status:** Documented in `docs/e0-0-workarounds.md`. A separate spike crate (`crates/oxi-spike`) was started but removed from the workspace due to API differences (notify 3-arg signature, CommandNArgs vs CommandNargs, etc.). The main `arbiter` plugin and `#[nvim_oxi::test]` smoke test validate the critical path: plugin load, setup(Dict), and Neovim integration.
- **Workarounds:** `api::notify` takes 3 args (msg, LogLevel, &Dictionary). Use `api::types::LogLevel`, not `api::LogLevel`. `Dict` is `Dictionary`. macOS requires `.cargo/config.toml` with `-undefined dynamic_lookup` for cdylib linking.

### E0-1: Project Scaffold

- **Workspace:** `Cargo.toml` with members `crates/arbiter` and `tests`.
- **arbiter (cdylib + rlib):** Dependencies: nvim-oxi (neovim-0-10, libuv), serde, serde_json, sha2, uuid v4, thiserror.
- **types.rs:** Turn, ThreadOrigin, ThreadStatus, ThreadContext, Role, FileStatus, ReviewStatus (Display for user-facing), ThreadSummary, BackendOp, BackendOpts, BackendResult, OnStream, OnComplete.
- **error.rs:** NvimAgentError with Git, Backend, State, Config, Nvim variants.
- **Module stubs:** config, review, git, state, file_panel, highlight, poll, turn, diff/, threads/, backend/.
- **lib.rs:** `#[nvim_oxi::plugin]`, `setup(Dictionary)` that notifies "arbiter loaded", returns table with setup function.
- **plugin/arbiter.lua:** `require("arbiter")`.

### E0-2: CI and Test Infra

- **.github/workflows/ci.yml:** fmt --check, clippy, build, test, check_boundaries. Neovim installed for nvim-oxi tests.
- **Makefile:** build, install, test, lint, fmt.
- **scripts/check_boundaries.sh:** Enforces stream isolation (diff/git vs threads/backend/review; threads/state vs git/diff/backend; backend vs threads/diff/review/git).
- **tests/src/fixtures.rs:** SIMPLE_DIFF, MULTI_HUNK_DIFF, MULTI_FILE_DIFF_NAMES, JSON_BACKEND_RESPONSE, STREAMING_JSON_LINES, SELF_REVIEW_TEXT, THREAD_STATE_JSON, REVIEW_STATE_JSON.
- **tests/src/helpers.rs:** TempGitRepo (write_file, add_and_commit, path), MockAdapter (stub), ThreadBuilder (build_summary), assert_buf_lines.
- **tests/src/lib.rs:** mod fixtures, helpers, unit/, nvim/, e2e/.

## Key Paths

| Item        | Path                                             |
|------------|---------------------------------------------------|
| Plugin lib | `crates/arbiter/src/lib.rs`                    |
| Types      | `crates/arbiter/src/types.rs`                  |
| Error      | `crates/arbiter/src/error.rs`                  |
| Lua entry  | `plugin/arbiter.lua`                           |
| CI         | `.github/workflows/ci.yml`                        |
| Boundaries | `scripts/check_boundaries.sh`                     |
| Fixtures   | `tests/src/fixtures.rs`                          |
| Helpers    | `tests/src/helpers.rs`                           |
| E0-0 doc   | `docs/e0-0-workarounds.md`                       |
| Cargo cfg  | `.cargo/config.toml` (macOS linker)              |

## Blockers / Notes

1. **Headless load:** `nvim --headless -c "lua require('arbiter')" -c "qa"` requires the plugin to be installed (e.g. `make install`). The .so must be in an rtp lua/ dir.
2. **oxi-spike:** Removed from workspace. Full 7-touchpoint spike can be revived from `docs/e0-0-workarounds.md` if needed.
3. **Dead code warnings:** NvimAgentError variants and some types are unused in the scaffold; they will be used in later epics.

## Verification

- `cargo fmt`
- `cargo build`
- `cargo test` (12 unit + 1 nvim + helpers)
- `./scripts/check_boundaries.sh` (exit 0)
