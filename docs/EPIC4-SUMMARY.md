# Epic 4 (UI Shell + Wiring) - Implementation Summary

## What Was Built

### E4-1: Config and Highlights (COMPLETE)

- **config.rs**
  - `Config` with `ReviewConfig`, `PromptConfig`, `KeymapConfig`, `BackendKind`
  - `#[serde(default)]` on all nested structs; PRD defaults
  - `OnceLock<Config>` for global access; `set_config()` / `get()`
  - `BackendKind` custom `Deserialize` (cursor/claude; error on invalid)
- **highlight.rs**
  - 13 highlight groups registered via `api::set_hl` with `link` to built-ins:
    - ArbiterDiffAdd, ArbiterDiffDelete, ArbiterDiffChange, ArbiterDiffFile
    - ArbiterThreadUser, ArbiterThreadAgent, ArbiterThreadResolved
    - ArbiterStatusApproved, ArbiterStatusChanges, ArbiterStatusPending
    - ArbiterHunkNew, ArbiterIndicatorUser, ArbiterIndicatorAgent
- **lib.rs**
  - `setup(opts)` deserializes Lua `Dictionary` into `Config` (serde + `into_deserializer`)
  - On error: uses defaults and warns
  - Calls `highlight::setup()`, notifies "arbiter loaded"

### E4-2 through E4-12c (NOT YET IMPLEMENTED)

Remaining Epic 4 issues are still stubs. The foundation (config, highlights, setup wiring) is in place so the rest can build on it.

---

## Blockers and Next Steps

1. **E4-2 (Workbench lifecycle)** – Needs `Review` struct, `thread_local RefCell<Option<Review>>`, `open`/`close`, `git::diff_names`/`git::untracked` wiring. Requires:
   - `api::command("tabnew")` for new tabpage
   - `api::create_buf`, `api::open_win` for splits (left file panel, right diff panel)
   - `FileEntry` type (either in review.rs or types.rs with module dependency care)

2. **Module boundaries** – `check_boundaries.sh` passes. Diff cannot import threads/backend/review; threads cannot import git/diff/backend/review; backend cannot import threads/diff/review/git.

3. **FileEntry placement** – RFD lists it in types.rs, but `FileEntry` has `hunks: Vec<Hunk>` and `Hunk` lives in diff. Putting `FileEntry` in types would create types→diff→types. Prefer `FileEntry` in review.rs.

---

## Verification Results

| Check              | Result  |
|--------------------|---------|
| `cargo fmt`        | Pass    |
| `cargo build`      | Pass (with dead-code warnings from Epics 0–3) |
| `cargo test`       | Pass (75 arbiter + 3 arbiter-tests)    |
| `scripts/check_boundaries.sh` | Pass    |

---

## Config Test Coverage (E4-1)

- Default config has PRD values
- Deserialization from Lua table works; `#[serde(default)]` merges partial tables
- Invalid `backend` triggers custom error
- 13 highlight groups: integration would need `#[nvim_oxi::test]` with Neovim; not yet added

---

## Recommended Implementation Order

1. E4-2: Workbench lifecycle (open/close, panels, keymaps)
2. E4-3: File panel + diff panel rendering (wire `select_file`, `file_panel::render`, `diff::render`)
3. E2-3: Thread UI (window.rs, input.rs, list view)
4. E4-4: Navigation and review marking
5. E4-6: Thread UI integration
6. E4-7: Command registration and agent mode
7. E4-8: Backend wiring
8. E4-9: Polling and live refresh
9. E4-10: Turn cycling
10. E4-11: Self-review and global commands
11. E4-12a–c: Side-by-side, inline indicators, error hardening
