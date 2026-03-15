# arbiter: Issue Breakdown

> **Implementation note (2026-03):** This document represents the
> original issue breakdown. The current implementation differs in
> several areas. See the PRD implementation note for details. Key
> differences:
>
> - All `g`-prefixed keymaps moved to `<Leader>a`-prefixed.
> - Batch comments (`gc`) and `:AgentSubmitReview` removed. All
>   comments are now sent immediately. References below are marked
>   as future considerations.
> - Thread lists use the quickfix list, not a custom float.
> - Build system uses `Taskfile.yml` instead of Makefile.
> - `Adapter` trait no longer has `binary_name` method.
> - `thiserror` removed; `chrono` added.

Issues are organized into epics (one per work stream). Completing all
epics delivers all PRD features. Each issue ends with a self-review.
Each epic ends with an epic-level review.

Dependencies between issues are noted with `Depends:`. Issues without
a `Depends:` line can start immediately.

## Issue Approach

Every issue is built using the same four pillars. Apply them
throughout implementation, not as an afterthought.

**Rust skill** (`.cursor/rules/rust-style.mdc`): Follow the Rust style
rules. Run `cargo fmt` on every changed crate before considering the
issue complete. Prefer method chains over `match` for `Option`/`Result`.
Use `?`, `map_err(From::from)`, turbofish where appropriate. No
`unwrap`/`expect` without a `// SAFETY:` comment. Narrowest visibility.
Derive standard traits unless a manual impl is required.

**Modern Rust**: Use current stable features: `let`-else, `if let`,
`impl Trait`, `#[derive]` with serde, `From`/`TryFrom` for conversions,
`.ok()`/`.and_then()`/`.map()` chains, `Option::is_some_and`, etc. No
deprecated patterns.

**Documentation**: `//!` on every module. `///` on every `pub` item.
Doc comments use bulleted lists for enumerations. No emdashes. Write
for both plain source and rendered docs.

**Testing**: L1 (pure `#[test]`) for logic with no Neovim dependency.
L2 (`#[nvim_oxi::test]`) for Neovim API interactions. L3 (e2e with
`TempGitRepo` + `MockAdapter`) for full workflows. Every public
function has at least one positive and one negative test where
applicable.

**Self-review**: At the end of each issue, run the self-review
checklist. Every self-review must include: (1) Run `cargo fmt`. (2)
All modules have `//!` docs, all public items have `///` docs. (3)
All required tests pass. (4) RFD/PRD contract for this issue is
satisfied. (5) No `unwrap`/`expect` without `// SAFETY:` comment. If
any item fails or gaps are found, create a follow-up issue before
considering the issue done. Do not leave gaps for "later."

---

## Epic 0: Foundation

Shared project scaffold and types. Must complete before any other
epic begins.

### E0-0: nvim-oxi API spike

**Context:** The entire design assumes specific `nvim-oxi` APIs
exist and work as expected. Before investing in the full scaffold,
validate the critical assumptions with a throwaway spike.

**Requirements:**

Build a minimal `cdylib` crate that exercises each of these in a
single Neovim session:

1. **Timer:** Create a repeating timer via `nvim-oxi`'s `libuv`
   feature. Verify it fires a callback every N ms. Stop and drop it.
2. **Buffer-local keymap with callback:** Create a buffer, set a
   buffer-local keymap with a Rust closure callback. Verify the
   closure fires when the key is pressed.
3. **User command with args:** Register a user command with `nargs`
   that receives arguments in the callback.
4. **`schedule` from background thread:** Spawn `std::thread::spawn`,
   call `nvim_oxi::schedule` from it, verify the closure runs on
   the main thread and can call Neovim API functions.
5. **Extmarks:** Set an extmark with `sign_text` and `virt_text`.
   Verify it renders.
6. **Floating window:** Open a floating window with border and title.
7. **`#[nvim_oxi::test]`:** Write one test using the test macro in
   a separate `cdylib` test crate. Verify `cargo test` spawns
   Neovim and runs it.

**Deliverable:** A working spike (can be discarded afterward) and
a short list of any APIs that don't work as expected, with
workarounds.

**This issue gates E0-1.** If any critical API is missing, the
architecture must be adjusted before the scaffold is built.

**Self-review:** Run `cargo fmt`. Verify all 7 API touchpoints were
exercised. Document any workarounds needed. If any critical API does
not work as assumed, create an E0-0a issue to adjust the architecture
before proceeding.

### E0-1: Project scaffold and shared types

**Context:** Nothing exists yet. This issue creates the Cargo project,
configures nvim-oxi, defines all shared types, and produces a plugin
that Neovim can load (even though it does nothing).

**Requirements:**

- Cargo workspace: root `Cargo.toml` with two members:
  `crates/arbiter` (the plugin, `cdylib`) and `tests/` (the test
  crate, `cdylib` with nvim-oxi `test` feature)
- `crates/arbiter/Cargo.toml` with dependencies: `nvim-oxi`
  (with `neovim-0-10` and `libuv` features), `serde`, `serde_json`,
  `sha2`, `uuid` (with `v4` feature), `thiserror` (for error types)
- `tests/Cargo.toml`: `cdylib` with `nvim-oxi` `test` feature,
  depends on `arbiter` crate, `tempfile` for temp git repos
- `tests/build.rs`: calls `nvim_oxi::tests::build()`
- `crates/arbiter/src/lib.rs` with `#[nvim_oxi::plugin]` entry
  point that exports a `setup` function accepting a Lua table (logs
  "arbiter loaded" via `api::notify`)
- `src/types.rs` with all shared enums: `Turn`, `ThreadOrigin`,
  `ThreadStatus`, `ThreadContext`, `Role`, `FileStatus`,
  `ReviewStatus`. All derive `Debug, Clone, Copy, PartialEq, Eq`.
  Persisted enums also derive `Serialize, Deserialize`.
  Add `impl Display` for enums that appear in user-facing text.
- `src/types.rs` also contains: `ThreadSummary`, `BackendOp` (enum:
  `NewSession`, `Resume(String)`, `ContinueLatest`), `BackendOpts`,
  `BackendResult`, `OnStream` (`Arc<dyn Fn(&str) + Send + Sync>`,
  Arc-wrapped so it can be cloned into multiple `schedule` calls
  during streaming), `OnComplete` type aliases
- `src/error.rs`: project-wide error type using `thiserror`. Define
  `NvimAgentError` with variants for git failures, backend failures,
  state I/O, config deserialization, and Neovim API errors. All
  modules return `Result<T, NvimAgentError>` or module-specific
  error types that convert via `From`.
- Timestamps: use `std::time::SystemTime` with
  `duration_since(UNIX_EPOCH)` for `i64` epoch seconds. No external
  time crate needed. Document this convention in `types.rs`.
- `plugin/arbiter.lua` that does `require("arbiter")`
- Module stubs: empty `crates/arbiter/src/{config,review,git,
  state,error,file_panel,highlight,poll,turn}.rs`,
  `src/diff/mod.rs`, `src/threads/mod.rs`, `src/backend/mod.rs`
  (each with `//!` module doc, no public API yet)
- Test crate stubs: `tests/src/lib.rs` with mod declarations,
  `tests/src/fixtures.rs`, `tests/src/helpers.rs`
- `//!` module doc on every file
- `///` doc on every public type and variant

**Test requirements:**

- `cargo build` succeeds for both workspace members
- `cargo test` passes (types constructable, Display impls correct,
  serde round-trips for all persisted enums)
- `#[nvim_oxi::test]` smoke test in `tests/src/nvim/`: Neovim loads
  the plugin and `setup()` returns without error
- `nvim --headless -c "lua require('arbiter')" -c "qa"` exits 0

**Self-review:** Apply the Issue approach. Verify all RFD-defined
enums are present. Verify `Cargo.toml` does not pin dependency
versions unnecessarily. Verify every public item has a doc comment.
Verify `NvimAgentError` has variants covering all known failure
categories. Verify both crates compile and the test crate's
`build.rs` runs. Run `cargo fmt`. If any item fails or gaps are
found, create a follow-up issue.

### E0-2: CI, build, and test infrastructure

**Depends:** E0-1

**Context:** Establish the CI pipeline, build tooling, and shared test
utilities. Every subsequent issue depends on this infrastructure. The
testing architecture is defined in the RFD Testing Strategy section;
this issue implements it.

**Requirements:**

- `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy`,
  `cargo build`, `cargo test`, `scripts/check_boundaries.sh`. CI
  installs Neovim (required by `nvim-oxi` test harness). Runs on
  push and PR.
- `Makefile` with targets: `build` (compile to .so/.dylib in correct
  Neovim plugin path), `install` (symlink or copy), `test`
  (`cargo test`), `lint` (`cargo clippy`), `fmt` (`cargo fmt`)
- `scripts/check_boundaries.sh`: module boundary enforcement script
  (see RFD). Verifies diff/ does not import threads/backend/review,
  threads/ does not import git/diff/backend/review, backend/ does
  not import threads/diff/review/git.
- `tests/src/fixtures.rs`: embedded test data. Const strings for:
  simple diff, multi-hunk diff, multi-file diff names output,
  JSON backend response, streaming JSON lines, self-review text
  response, thread state JSON, review state JSON.
- `tests/src/helpers.rs`:
  - `TempGitRepo`: creates tempdir, git init, initial commit.
    Methods: `write_file`, `add_and_commit`, `path`. Cleanup on drop.
  - `MockAdapter`: implements `Adapter` with `Mutex<VecDeque>` for
    responses and `Mutex<Vec>` for call recording. Methods: `new`,
    `calls` (returns recorded opts).
  - `ThreadBuilder`: builder pattern for `Thread` with sensible
    defaults. Methods: `new(file, line)`, `origin`, `status`,
    `message`, `anchor`, `pending`, `auto_resolve`, `build`.
  - `assert_buf_lines`: helper that extracts buffer lines and
    compares against expected strings.
- `tests/src/lib.rs`: mod declarations for `fixtures`, `helpers`,
  `unit/`, `nvim/`, `e2e/`

**Test requirements:**

- CI workflow passes on a clean checkout with no warnings
- `check_boundaries.sh` exits 0 on the scaffold (no violations)
- `TempGitRepo::new()` creates a valid repo (`git status` exits 0)
- `MockAdapter` returns canned responses in FIFO order and records
  call opts
- `ThreadBuilder` produces threads with correct defaults and
  overrides
- `assert_buf_lines` correctly compares buffer content

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify CI
installs Neovim and all tests can run. Verify `check_boundaries.sh`
covers all stream isolation rules from the RFD. Verify fixture data
matches the formats produced by real git and CLI commands. Verify the
Makefile build target produces a loadable `.so`/`.dylib`. If anything
is missed, create a follow-up issue.

---

## Epic 1: Git + Diff Engine (Stream 1)

Async git commands and unified diff parsing/rendering. Zero
dependency on threads, backend, or review.

### E1-1: Async git command runner

**Depends:** E0-1

**Context:** Every other module needs git. This is the foundation.
The module spawns git on a background thread and schedules the
callback on Neovim's main thread via `nvim_oxi::schedule`.

**Requirements:**

- `src/git.rs`: `GitResult` struct, `run()` function
- Convenience wrappers: `diff`, `diff_names`, `untracked`, `show`,
  `stash_create`, `diff_hash`
- `file_mtime` (synchronous, `std::fs::metadata`)
- Error case: git not found on PATH (return `GitResult` with
  exit_code -1 and descriptive stderr)
- `//!` module doc explaining the async model
- `///` doc on every public function

**Test requirements:**

L1 (unit, `#[test]`):
- `file_mtime` returns `Some` for existing file, `None` for missing

L2 (nvim, `#[nvim_oxi::test]`):
- `diff_names` on a `TempGitRepo` with known changes: callback
  receives correct file list
- `diff` on a single file: callback receives valid unified diff text
- `show` retrieves file content at HEAD
- `untracked` lists files not in git
- `stash_create` returns a hash string
- Error case: command on non-existent directory returns non-zero exit

**Self-review:** Run `cargo fmt`. Verify all RFD convenience wrappers
are implemented. Verify callbacks are always scheduled via
`nvim_oxi::schedule`. Verify no `unwrap` or `expect` without a SAFETY
comment. If anything is missed, create a new issue.

### E1-2: Diff parser

**Depends:** E0-1

**Context:** Parses raw unified diff text (from `git diff` output)
into structured `Hunk` objects. Pure string parsing with zero
Neovim dependency. This is the most unit-testable module in the
project.

**Requirements:**

- `src/diff/parse.rs`: `Hunk` struct (in `diff/mod.rs`), `parse_hunks`
  function
- Parse `@@ -old_start,old_count +new_start,new_count @@` headers
- Handle edge cases: single-line hunks (count omitted = 1), no
  newline at EOF marker, binary file markers, empty diffs
- `content_hash` per hunk (hash of the hunk's content lines,
  excluding the header)
- `SourceLocation` struct
- `buf_line_to_source`: given a hunk list with `buf_start`/`buf_end`
  populated, map a buffer line to the corresponding source file line
- `synthesize_untracked`: produce a synthetic all-additions diff for
  a file (every line prefixed with `+`)
- `detect_hunk_changes`: compare old content hashes against new hunks,
  return set of `buf_start` lines for new/changed hunks

**Test requirements:**

All L1 (pure `#[test]`, no Neovim):
- `parse_hunks`: standard multi-hunk diff (count, ranges, headers)
- `parse_hunks`: empty input returns empty vec
- `parse_hunks`: single-line change (count omitted = 1)
- `parse_hunks`: "no newline at end of file" marker ignored
- `parse_hunks`: rename diff format
- `parse_hunks`: binary file marker
- `buf_line_to_source`: correct mapping for added, deleted, context
  lines
- `buf_line_to_source`: `None` for lines outside any hunk
- `buf_line_to_source`: correct when summary lines are injected
  (offset accounting)
- `synthesize_untracked`: correct header, all `+` lines, handles
  empty file
- `detect_hunk_changes`: new hunk detected, unchanged ignored,
  all-new, all-unchanged, empty inputs
- `content_hash`: deterministic, different inputs produce different
  hashes

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
`parse_hunks` handles all git diff formats mentioned in the RFD.
Verify no Neovim API imports in this file. If anything is missed,
create a follow-up issue.

### E1-3: Diff renderer

**Depends:** E1-2

**Context:** Takes parsed hunks and thread summaries, renders them
into a Neovim buffer. This is the bridge between pure parsing and
the Neovim UI. Uses `ThreadSummary` from `types.rs` (not `Thread`
from the threads module) to maintain stream decoupling.

**Requirements:**

- `src/diff/render.rs`: `render()` function
- Build buffer content: file header line, thread summary lines
  (from `&[ThreadSummary]`), then raw diff lines
- Update `Hunk.buf_start` / `Hunk.buf_end` to account for injected
  header and summary lines
- Return `(Vec<Hunk>, HashMap<String, usize>)` where the map is
  thread_id to buffer line
- `apply_highlights`: highlight `+` lines, `-` lines, `@@` lines,
  file header, thread summaries (each origin gets a different
  highlight group)
- Respect `show_resolved` parameter (filter thread summaries)
- `open_side_by_side` / `close_side_by_side` for two-buffer diff mode
- Use `nvim_oxi::api` for all buffer operations

**Test requirements:**

L2 (nvim, `#[nvim_oxi::test]`):
- `render` with known diff text and 2 thread summaries: verify
  buffer line count, file header present, thread summary lines
  present, diff lines present in correct order
- `render` with `show_resolved = false`: resolved summary omitted
- `render` returns `thread_buf_lines` mapping IDs to correct lines
- `render` returns hunks with `buf_start`/`buf_end` offset by the
  number of injected header + summary lines
- `apply_highlights`: `+` lines get `ArbiterDiffAdd`, `-` lines get
  `ArbiterDiffDelete`, `@@` lines get `ArbiterDiffChange`
- `open_side_by_side`: creates two buffers with correct content,
  both in diff mode
- `close_side_by_side`: cleans up buffers and windows

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
`render` imports `ThreadSummary` from `types.rs`, not from `threads`.
Verify all highlight group names match those in `highlight.rs`.
Verify side-by-side creates and cleans up buffers correctly. If
anything is missed, create a follow-up issue.

---

## Epic 2: Thread Data Layer (Stream 2)

Thread CRUD, re-anchoring, persistence. Zero dependency on git, diff
rendering, backend, or review.

### E2-1: Thread CRUD and persistence

**Depends:** E0-1

**Context:** The core data operations on threads. Every thread
function takes `&[Thread]` or `&mut [Thread]`, never `&Review`.
Persistence uses serde_json to `~/.local/share/nvim/arbiter/`.

**Requirements:**

- `src/threads/mod.rs`: `Thread`, `Message`, `CreateOpts` structs
- `create()`: generates UUID, sets initial message, captures
  `anchor_content` and `anchor_context` (passed in via `CreateOpts`
  or derived from parameters)
- `add_message()`, `resolve()`, `bin()`, `dismiss()`, `resolve_all()`
- `src/state.rs`: `ReviewState`, `FileState`, `SessionRecord` structs
- `load_review`, `save_review`, `load_threads`, `save_threads`
- `load_sessions`, `save_sessions`, `add_session`: persist session
  records (session_id, created_at, last_prompt_preview, thread_id
  if per-thread) to `{state_dir}/{ws_hash}/sessions.json`. Required
  by `:ArbiterList` / `:ArbiterResume`.
- `workspace_hash` (SHA256 of path, truncated to 12 hex chars)
- `content_hash` (fast hash for change detection)
- Create state directory if it doesn't exist
- Handle missing/corrupt JSON files gracefully (return defaults)

**Test requirements:**

- `create` produces a thread with UUID, correct origin, status Open
- `add_message` appends to messages vec with timestamp
- `resolve` sets status to Resolved
- `bin` sets status to Binned
- `dismiss` removes thread from vec by index
- `resolve_all` resolves all Open threads, ignores Resolved/Binned
- `save_threads` then `load_threads` round-trips correctly
- `save_review` then `load_review` round-trips correctly
- `load_threads` on missing file returns empty vec
- `load_review` on missing file returns default ReviewState
- `save_sessions` then `load_sessions` round-trips correctly
- `add_session` appends to existing session list
- `workspace_hash` is deterministic (same input = same output)
- `workspace_hash` differs for different paths

**Self-review:** Run `cargo fmt`. Verify all CRUD operations from the
RFD contract are implemented. Verify state directory creation is
handled. Verify no `unwrap` on file I/O (use defaults on error). If
anything is missed, create a new issue.

### E2-2: Thread operations

**Depends:** E2-1

**Context:** Re-anchoring, ordering, filtering, navigation, and
projection. These are the query and mutation operations that the UI
layer calls to manage threads. All pure logic, no Neovim API.

**Requirements:**

- `reanchor_by_content`: for each thread on the given file, search
  `new_contents` for `anchor_content`. If found, verify at least one
  `anchor_context` line exists within 5 lines. Update `line` on
  match. Return indices of unmatched threads.
- `for_file`: return threads for a file, sorted by line ascending
- `pending_indices`: return indices where `pending == true`
- `filter`: filter by optional `origin` and/or `status`
- `sorted_global`: sort threads by (file order index, line). Takes
  `&[String]` file order. Returns indices into the threads slice.
- `next_thread` / `prev_thread`: given a sorted index list and an
  optional current position, return the next/previous index. Wrap
  around at the ends.
- `to_summaries`: project `&[Thread]` to `Vec<ThreadSummary>`.
  Preview is first 40 chars of the first message.
- `check_auto_resolve_timeouts`: for each thread with
  `auto_resolve == true` and `status == Open`, if
  `now - auto_resolve_at > timeout_secs`, set `auto_resolve = false`
  and clear `auto_resolve_at`. Return indices of timed-out threads.

**Test requirements:**

- `reanchor_by_content`: thread with shifted anchor (inserted lines
  above) re-anchors to new line number
- `reanchor_by_content`: thread with deleted anchor goes to unmatched
- `reanchor_by_content`: thread with modified anchor (different text)
  goes to unmatched
- `reanchor_by_content`: context line verification (anchor found but
  context missing = unmatched)
- `for_file`: correct filtering and sort order
- `filter`: each combination of origin/status
- `sorted_global`: ordering respects file order then line
- `next_thread` / `prev_thread`: forward/backward traversal, wrapping
- `to_summaries`: correct preview truncation, all fields mapped
- `check_auto_resolve_timeouts`: thread within timeout not touched,
  thread past timeout reverted

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
all thread query functions from the RFD contract are implemented.
Verify `sorted_global` takes `&[String]` not `&Review`. Verify no
import of `crate::review` or `crate::diff`. If anything is missed,
create a follow-up issue.

---

## Epic 3: Backend Shim (Stream 3)

CLI adapter, queue, streaming. Zero dependency on threads, diff, git,
review, or Neovim buffer/window APIs.

### E3-1: Adapter trait and FIFO queue

**Depends:** E0-1

**Context:** The queue serializes all CLI calls. The `Adapter` trait
abstracts over Cursor and Claude. This issue builds the plumbing
without any real CLI integration.

**Requirements:**

- `src/backend/mod.rs`: `Adapter` trait with `execute` and
  `binary_name`. Module-level `setup()` that stores config and
  selects adapter. `send()` that constructs a `QueueItem` and pushes.
- `src/backend/queue.rs`: `QueueItem` struct, `VecDeque` +
  `Mutex` + `AtomicBool` + `AtomicU64` (generation counter) queue.
  `push()` and `process_next()`. Recursive drain via callbacks.
  `DrainGuard` (drop guard on `process_next`) ensures the queue
  drains even if a callback panics.
- Convenience methods on `backend/mod.rs`: `send_comment`,
  `thread_reply`, `catch_up`, `handback`, `self_review`, `re_anchor`,
  `send_prompt`, `continue_prompt`. Each constructs `BackendOpts`
  with the correct `BackendOp` variant and flags, then calls `send`.
- `is_busy()` returns true if queue is processing
- `cancel_all()`: increments generation counter (so in-flight
  callbacks no-op on generation mismatch), clears pending items,
  resets processing flag. Called by `review::close()` to prevent
  callbacks from updating a dropped `Review`.

**Test requirements:**

- Queue ordering: push 3 items with a mock adapter, verify callbacks
  fire in FIFO order
- Sequential execution: mock adapter that tracks concurrent calls,
  verify max concurrency is 1
- `is_busy` returns true while processing, false when idle
- `cancel_all` prevents pending item callbacks from firing
- `cancel_all` causes in-flight callbacks to no-op (generation check)
- DrainGuard: if a callback panics, the next item still processes
- Convenience methods set correct `BackendOpts` fields (e.g.
  `thread_reply` uses `BackendOp::Resume`, `self_review` uses
  `BackendOp::NewSession` and does not set `stream`)

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify all
convenience methods from the RFD contract are implemented. Verify
`Mutex` lock is not held across callback invocations (would deadlock).
Verify `DrainGuard` is tested. Verify the queue handles the empty case
(no items, processing = false). If anything is missed, create a
follow-up issue.

### E3-2: CLI adapters

**Depends:** E3-1

**Context:** Cursor and Claude CLI adapters. Each implements the
`Adapter` trait. The adapter builds CLI args, spawns the process on
a background thread, parses JSON output, and schedules the callback.

**Requirements:**

- `src/backend/cursor.rs`: `CursorAdapter` implementing `Adapter`.
  `build_args` method. Binary name: `agent`. Passes `--workspace`
  from `config.workspace` when set.
- `src/backend/claude.rs`: `ClaudeAdapter` implementing `Adapter`.
  `build_args` method. Binary name: `claude`. Passes `--add-dir`
  from `config.workspace` when set.
- Both adapters read `config.workspace` (defaults to cwd at setup
  time) and include the workspace root in every CLI invocation.
- Flag differences per RFD: streaming flags, ask mode flags,
  json_schema, workspace/add-dir
- `execute`: spawn `std::process::Command` on background thread,
  capture stdout/stderr, parse JSON with `serde_json`,
  `nvim_oxi::schedule` the callback
- JSON response struct for parsing:
  `{ "session_id": "...", "result": "..." }`
- Handle non-zero exit codes: set `error` on `BackendResult`
- Handle malformed JSON: set `error`, put raw text in `text` field
- Binary existence check: `which` equivalent via `std::process::Command`
  or path search

**Test requirements:**

- `CursorAdapter::build_args` with `BackendOp::Resume(id)`:
  includes `--resume <id>`
- `CursorAdapter::build_args` with `BackendOp::ContinueLatest`:
  includes `--continue`
- `CursorAdapter::build_args` with `BackendOp::NewSession`:
  no session flags
- `CursorAdapter::build_args` with stream: includes `stream-json`
  and `--stream-partial-output`
- `CursorAdapter::build_args` with ask_mode: includes `--mode ask`
- `CursorAdapter::build_args` with model: includes `--model`
- `CursorAdapter::build_args` with workspace: includes `--workspace`
- `ClaudeAdapter::build_args` with ask_mode: includes
  `--permission-mode plan`
- `ClaudeAdapter::build_args` with json_schema: includes
  `--json-schema`
- `ClaudeAdapter::build_args` with workspace: includes `--add-dir`
- JSON parsing: valid response extracts `session_id` and `result`
- JSON parsing: malformed JSON sets `error`
- Binary not found: `execute` returns `BackendResult` with error
  message indicating the binary was not found on PATH
- Session expiry: `execute` with `BackendOp::Resume` that fails
  retries with `BackendOp::NewSession` and returns the new session ID

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
all CLI flag mappings from the PRD table are implemented. Verify
session expiry retry logic is tested. Verify each adapter's
`binary_name` is correct. Verify `execute` never calls Neovim API
from the background thread (only via `schedule`). If anything is
missed, create a follow-up issue.

### E3-3: Streaming and self-review parsing

**Depends:** E3-2

**Context:** Streaming support for real-time responses and self-review
parsing for agent-initiated threads.

**Requirements:**

- Streaming: when `opts.stream == true`, read stdout line by line.
  Each line is a JSON object. For `assistant` events, extract text
  and call `on_stream`. The final `result` event produces the full
  `BackendResult`.
- Use `std::io::BufReader` with `lines()` on the child process
  stdout for line-buffered reading
- Self-review parsing for Cursor: `parse_self_review_text` function.
  Parses `THREAD|file|line|message` lines. Returns
  `Vec<(String, u32, String)>`.
- Self-review for Claude: the `self_review` convenience method sets
  `json_schema` on opts. Claude adapter includes `--json-schema`.
  Response is JSON array parsed by caller (not the adapter).

**Test requirements:**

- Streaming: mock a process that outputs 3 JSON lines, verify
  `on_stream` called 3 times with correct text
- Streaming: final result contains full text
- `parse_self_review_text`: valid input with 3 threads
- `parse_self_review_text`: input with mixed valid/invalid lines
  (invalid silently discarded)
- `parse_self_review_text`: empty input returns empty vec
- `parse_self_review_text`: line with missing fields discarded
- `parse_self_review_text` leniency: strips markdown code fences,
  bullet prefixes, backticks; accepts `THREAD:` or `THREAD ` prefix
  variant per RFD

**Self-review:** Apply the Issue approach. Verify streaming callback
is called via
`nvim_oxi::schedule`. Verify `parse_self_review_text` regex/pattern
matches the RFD prompt template format exactly.

---

## Epic 4: UI Shell + Wiring (Stream 4)

Integrates streams 1-3 into the Neovim workbench. Issues are ordered
to align with PRD milestones.

### E4-1: Config and highlights

**Depends:** E0-1

**Context:** Configuration deserialization from Lua tables and
highlight group registration. Must be done first since every other
S4 module reads config.

**Requirements:**

- `src/config.rs`: `Config`, `ReviewConfig`, `PromptConfig`,
  `KeymapConfig`, `BackendKind` structs. All derive `Deserialize`
  with `#[serde(default)]`. `impl Default` with PRD defaults.
- Config stores in a `OnceLock<Config>` for global access
- `src/highlight.rs`: `setup()` registers all 13 highlight groups
  from the RFD table using `api::set_hl`
- Update `lib.rs` `setup()`: deserialize Lua table into Config,
  store in OnceLock, call `highlight::setup()`

**Test requirements:**

- Default config has all fields populated with PRD values
- Deserializing an empty table produces default config
- Deserializing a partial table merges with defaults
- Deserializing invalid `backend` value produces error
- All 13 highlight groups created (integration test)

**Self-review:** Run `cargo fmt`. Verify every config field from the
PRD Configuration section is present. Verify highlight group names
match the RFD table. If anything is missed, create a new issue.

### E4-2: Workbench lifecycle

**Depends:** E4-1, E1-1

**Context:** Opening and closing the review workbench. Creates the
tabpage, file panel window, diff panel window. Sets up agent mode
state tracking.

**Requirements:**

- `src/review.rs`: `Review` struct (includes `cwd: String` captured
  at open time), `Panel` struct, `SideBySide` struct
- `thread_local!` with `RefCell<Option<Review>>`
- `with_active`, `is_active`, `open`, `close`
- `open`: captures `cwd` via `std::env::current_dir()`, creates
  tabpage, file panel (left vsplit, fixed width), diff panel (fills
  remaining), calls `git::diff_names` and `git::untracked`, loads
  state, renders file panel and first file
- `close`: persists state, calls `backend::cancel_all()`, stops
  timers, drops `Review`, closes tab
- `refresh_file` and `refresh_file_list` as stubs (log "not yet
  implemented"). Full implementation wired in E4-9.
- `q` keymap on both panel buffers calls `close`
- Buffers are scratch (`buftype=nofile`, `modifiable=false` for diff)

**Test requirements:**

- Integration test: `open` creates a new tabpage with 2 windows
- Integration test: `is_active` returns true after open, false after
  close
- Integration test: `close` removes the tabpage
- Integration test: opening when already open shows notification

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
`open` follows the 12-step sequence from the RFD. Verify `close`
persists state before dropping. If anything is missed, create a
follow-up issue.

### E4-3: File panel and diff panel rendering

**Depends:** E4-2, E1-2, E1-3, E2-1

**Context:** Renders the file tree in the left panel and the diff in
the right panel. Wires `git::diff_names` output through
`file_panel::render` and `diff::render`.

**Requirements:**

- `src/file_panel.rs`: `render()` builds tree from flat paths with
  status icons (✓/✗/·). Appends summary section. Stores line-to-path
  mapping.
- `path_at_line()` returns the file path for a given buffer line
- `<CR>` on file panel: calls `review::select_file`
- `review::select_file`: calls `git::diff`, then `diff::render` with
  thread summaries from `threads::to_summaries`
- Diff panel `filetype` set for syntax highlighting of diff content

**Test requirements:**

- Integration test: file panel contains expected tree structure for
  a known file list
- Integration test: status icons correct (✓ for approved, etc.)
- Integration test: `path_at_line` returns correct path
- Integration test: `select_file` updates diff panel content

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
file panel tree rendering matches PRD mockup. Verify summary counts
are correct. Verify `path_at_line` handles directory lines (returns
None). If anything is missed, create a follow-up issue.

### E2-3: Thread UI components

**Depends:** E2-1, E4-1

**Context:** The thread window (floating conversation view), comment
input float, and thread list view. These are Neovim UI components
that live in the `threads/` module but require Neovim API access.
Placed in Epic 4 because they are UI work, not pure data logic.

**Requirements:**

- `src/threads/window.rs`: `open`, `close`, `append_message`,
  `append_streaming`, `is_open`. Floating window with thread header
  and messages. Buffer is fully readonly. `<CR>` opens the input
  float for composing a reply. `q` closes.
- `src/threads/input.rs`: `open`, `open_for_line`, `close`. Small
  floating input buffer used for both new comments (gc/gC) and
  thread replies. `<CR>` submits, `q`/`<Esc>` cancels.
- Thread list view: floating window listing all threads. Buffer-local
  keymaps for `<CR>` (open thread), `gR` (resolve), `dd` (dismiss
  binned), `gP` (re-anchor), `q` (close). Accepts `ThreadFilter`.
- Line-to-thread mapping (same pattern as file panel path mapping)

**Test requirements:**

- Integration test: `window::open` creates a floating window with
  correct dimensions
- Integration test: `window::append_message` adds a line to the buffer
- Integration test: `input::open` creates a floating window with
  title containing file:line
- Integration test: thread list renders correct number of lines for
  given thread count

**Self-review:** Apply the Issue approach. Run `cargo fmt`. Verify
window dimensions match RFD spec. Verify all thread list keymaps from
the RFD are bound. Verify `close` cleans up buffers and windows. If
anything is missed, create a follow-up issue.

### E4-4: Navigation and review marking

**Depends:** E4-3, E2-1

**Context:** Navigation keymaps (`]c`/`[c`, `]f`/`[f`, `<CR>`) and
review marking (`ga`/`gx`/`gr`, `gs` summary float). These are
closely related: both are buffer-local keymaps on the diff panel
that interact with the file list and review state.

**Requirements:**

- `]c`/`[c`: find next/prev `hunk.buf_start` relative to cursor
- `]f`/`[f`: select next/prev file in the files list
- `<CR>` dual behavior: if cursor on thread summary line, open
  thread window; otherwise open source file in new tab
- All keymaps buffer-local on diff panel buffer
- Cursor wraps at boundaries (last hunk → first hunk)
- `ga`: set current file's `review_status` to `Approved`, compute
  and store `content_hash` (via `state::content_hash` on the current
  diff text), persist. The stored hash enables cross-session
  invalidation: on next `open`, if the file's hash differs from the
  stored hash, reset to Unreviewed.
- `gx`: set to `NeedsChanges`, persist
- `gr`: set to `Unreviewed`, clear `content_hash`, persist
- `gs`: centered floating window with file counts and thread counts
- File panel re-renders on status change (icons update)
- `q`/`<Esc>` closes the summary float

**Test requirements:**

- Integration test: `]c` moves cursor to next hunk start line
- Integration test: `]f` switches to next file and re-renders
- Integration test: `<CR>` on diff line opens source file
- Integration test: `ga` changes status and persists to disk
- Integration test: file panel icon updates after status change
- Integration test: `gs` opens a float with correct counts
- State survives close and reopen

**Self-review:** Apply the Issue approach. Verify all navigation and
review marking keymaps from the PRD table are bound. Verify wrapping
behavior. Verify state persistence calls `state::save_review`. Run
`cargo fmt`. If anything is missed, create a follow-up issue.

### E4-6: Thread UI integration

**Depends:** E4-4, E2-2, E2-3

**Context:** Wire thread CRUD into the workbench. `<Leader>ac` adds
comments (sent immediately), `]t`/`[t` navigate threads cross-file,
`<Leader>aR` resolves, `<Leader>a?` toggles resolved visibility,
`<Leader>aT` opens thread list (quickfix).

**Requirements:**

- `<Leader>ac`: open comment input, map diff buffer line to source
  file/line via `diff::buf_line_to_source`, read the source file to
  extract `anchor_content` (the line text) and `anchor_context`
  (surrounding lines), create thread, open thread window, send to
  agent immediately, stream response into thread window

> **Future consideration:** The original design had `gc` create
> threads with `pending=true` (batched). Batch submission was handled
> by `:AgentSubmitReview` in E4-8.
- `]t`/`[t`: call `threads::sorted_global` with file order, then
  `next_thread`/`prev_thread`. If next thread is in a different file,
  call `select_file` first, then move cursor to summary line.
- `gR`: resolve thread at cursor, re-render
- `g?`: toggle `review.show_resolved`, re-render
- `gT`: open thread list float. `gTa`/`gTu`/`gTb` pre-filter
  (read next char via `api::exec_lua` for `vim.fn.getcharstr`)
- Thread summaries appear above diff content (via `diff::render`)
- Thread window opens on `<CR>` over summary line or `go`

**Test requirements:**

- Integration test: `gc` creates a thread, summary appears in diff
- Integration test: `]t` navigates to next thread summary line
- Integration test: `]t` switches file when next thread is in
  different file
- Integration test: `gR` resolves thread, summary disappears
  (with `show_resolved=false`)
- Integration test: `g?` toggles resolved thread visibility

**Self-review:** Apply the Issue approach. Verify all thread keymaps
from the PRD are bound. Verify `gc` reads the *source file* (not the
diff buffer) for `anchor_content` and `anchor_context`. Verify `gT`
filter dispatch matches RFD spec. Run `cargo fmt`. If anything is
missed, create a follow-up issue.

### E4-7: Command registration and agent mode

**Depends:** E4-4, E4-6

**Context:** Register all user commands. Global commands work without
a review. Gated commands show a notification if no review is active.

**Requirements:**

- Register all global commands: `Arbiter`, `ArbiterSend`,
  `ArbiterContinue`, `ArbiterCatchUp`, `ArbiterList`, `ArbiterResume`
- Register all gated commands: `ArbiterSelfReview`,
  `ArbiterResolveAll`, `ArbiterRefresh`, `AgentTurn`, `HumanTurn`

> **Future consideration:** `AgentSubmitReview` was originally listed
> here for batch comment submission. Removed.
- Commands whose backend wiring is not yet built (E4-8, E4-11) are
  registered as stubs that show "Not yet implemented" via
  `api::notify`. This ensures the command namespace is established
  early and users see a clear message rather than "unknown command."
- `with_review_cmd` guard wrapper
- Statusline function exported as Lua-callable
- `gU` keymap on diff panel buffer: manual refresh (calls
  `review::refresh_file` + `review::refresh_file_list`)

**Test requirements:**

- Integration test: gated command without review shows notification
- Integration test: global command without review does not error
- Integration test: statusline returns empty string with no review
- Integration test: statusline returns `[AGENT] [REVIEW 0/n]` with
  active review

**Self-review:** Apply the Issue approach. Verify every command from
the PRD is registered. Verify gated vs global classification matches
the Agent Mode section. Run `cargo fmt`. If anything is missed,
create a follow-up issue.

### E4-8: Backend wiring

**Depends:** E4-6, E3-1, E3-2, E3-3

**Context:** Connect threads to the backend. All comments are sent
immediately. `<Leader>aA` sends with auto-resolve. Streaming
responses appear in thread windows.

**Requirements:**

- `<Leader>ac`/`<Leader>aC`: create thread, open thread window,
  call `backend::send_comment` with `on_stream` wired to
  `thread_window::append_streaming`. On result: set `session_id`,
  add agent message, persist, re-render.
- `<Leader>aA`: same as `<Leader>ac` but with `auto_resolve=true`,
  set `auto_resolve_at` to current time

> **Future consideration:** `:AgentSubmitReview` was originally
> planned here to send each pending batch comment as its own new
> session. Removed in favor of immediate submission.
- Thread window reply (`<CR>` opens input float, submit calls
  `backend::thread_reply` with thread's `session_id`)
- Prompt assembly: format comment with file, line, and surrounding
  code context
- `gP` (re-anchor): call `backend::re_anchor` in ask mode

**Test requirements:**

- Integration test with mock adapter: `<Leader>ac` sends prompt,
  callback updates thread with session_id and agent message
- Integration test: `<Leader>aA` sets auto_resolve fields on thread
- Integration test: thread reply uses existing session_id
- Integration test: streaming response appends to thread window

**Self-review:** Apply the Issue approach. Verify all 10 shim operations
from the PRD are callable through the wiring. Verify prompt format
includes file context. Run `cargo fmt`. If anything is missed, create a
follow-up issue.

### E4-9: Polling and live refresh

**Depends:** E4-8, E2-2

**Context:** Poll the current file for changes. On change: re-render
diff, reset approved status, check auto-resolve, re-anchor threads,
mark new hunks.

**Requirements:**

- `src/poll.rs`: `start`, `stop`, `set_target`. Two timers via
  Neovim timer API.
- File poll (2s default): check mtime, if changed: call `git::diff`,
  re-render, detect hunk changes, place `ArbiterHunkNew` extmarks
- File list poll (5s default): call `git::diff_names` +
  `git::untracked`, if set changed: re-render file panel
- On file change: if file was Approved, reset to Unreviewed
- On file change: call `threads::reanchor_by_content`, bin unmatched
- On file change: call `threads::check_auto_resolve_timeouts`
- Preserve cursor position and scroll across re-renders
- `gU` / `:ArbiterRefresh`: manual trigger

**Test requirements:**

L2 (nvim, `#[nvim_oxi::test]`):
- `set_target` changes the poll target file
- `start`/`stop` lifecycle: timer created on start, removed on stop

L3 (e2e, `#[nvim_oxi::test]` with `TempGitRepo`):
- Modify a tracked file on disk, trigger manual refresh (`gU`),
  verify diff panel content updated
- File list refresh: add a new file, trigger refresh, verify it
  appears in file panel
- Approved file reset: approve a file, modify it, refresh, verify
  status reset to Unreviewed
- Thread re-anchor: create thread, shift anchor by adding lines
  above, refresh, verify thread line updated
- Thread bin: create thread, delete anchor line, refresh, verify
  thread status is Binned
- New hunk indicators: modify a file, refresh, verify `ArbiterHunkNew`
  extmarks on changed hunks
- Cursor/scroll preservation: record cursor position, refresh,
  verify cursor restored
- Auto-resolve timeout check called on refresh tick

**Self-review:** Apply the Issue approach. Verify all 5 poll-tick
actions from the PRD Live Diff Refresh section are implemented.
Verify timers are stopped in `poll::stop` and on `review::close`. Run
`cargo fmt`. If anything is missed, create a follow-up issue.

### E4-10: Turn cycling

**Depends:** E4-9

**Context:** Agent turn / human turn. Snapshot on human entry, diff
and handback on return to agent.

**Requirements:**

- `src/turn.rs`: `enter_agent`, `enter_human`
- `enter_human`: `git::stash_create` to snapshot, stop polling
- `enter_agent`: if snapshot exists, `git::diff_hash` to compute
  user edits, send via `backend::handback`, refresh workbench,
  resume polling
- `<Leader>a`: toggle between turns
- Statusline shows `[AGENT]` or `[HUMAN]`
- If workbench closed during human turn, reopening enters agent turn

**Test requirements:**

- Integration test: enter human creates snapshot hash
- Integration test: enter agent with changes sends handback
- Integration test: enter agent with no changes switches silently
- Integration test: statusline reflects current turn

**Self-review:** Apply the Issue approach. Verify snapshot flow
matches RFD. Verify poll stop/start on turn transitions. Run
`cargo fmt`. If anything is missed, create a follow-up issue.

### E4-11: Self-review and global commands

**Depends:** E4-8

**Context:** Agent-initiated threads via `:ArbiterSelfReview`. Global
commands for session management and free-form prompts. Response
panel.

**Requirements:**

- `:ArbiterSelfReview`: get current diff, send via `backend::self_review`,
  parse response (Cursor: `parse_self_review_text`, Claude: JSON),
  create threads with `origin=Agent`
- `:ArbiterCatchUp`: send catch-up prompt via `backend::catch_up`,
  route response
- `:ArbiterSend`: send prompt via `backend::send_prompt`, route response
- `:ArbiterContinue`: send via `backend::continue_prompt`, route
  response
- `:ArbiterList`: load session records from `state::load_sessions`,
  display in a floating window with session_id, timestamp, and
  preview. `<CR>` to select, `q` to close.
- `:ArbiterResume <id>` or `:ArbiterResume <id> <prompt>`: resume a
  specific session by ID. If a prompt is provided, send it
  immediately. If no prompt, open the session for the next command.
- Response panel: horizontal split at bottom of workbench, reused
  across calls, `q` closes split only. Streaming appends.
- Standalone floating response: when no workbench is open, responses
  show in a centered float

**Test requirements:**

- Integration test with mock adapter: `:ArbiterSelfReview` creates
  agent threads from parsed response
- Integration test: response panel appears at bottom of workbench
- Integration test: response routes to float when no workbench
- Integration test: `:ArbiterCatchUp` works both with and without
  active review

**Self-review:** Apply the Issue approach. Verify all M4 deliverables
from the PRD are implemented. Verify agent threads display with
`[agent]` prefix. Run `cargo fmt`. If anything is missed, create a
follow-up issue.

### E4-12a: Side-by-side and fold support

**Depends:** E4-9, E1-3

**Context:** Side-by-side diff toggle and fold support for approved
hunks.

**Requirements:**

- `<Leader>s` toggle: call `diff::open_side_by_side` /
  `close_side_by_side`. Pass thread summaries for virtual text
  indicators in the right buffer.
- Fold support: manual folds per hunk after render. `zo`/`zc` work
  natively. `fold_approved` config: approved hunks stay folded.
  Fold text shows line count and "approved" indicator.
- On `ga` with `fold_approved` enabled, fold the current hunk. On
  `gr`, unfold it.

**Test requirements:**

- Integration test: folds created after render, `zo`/`zc` work
- Integration test: `fold_approved` keeps approved hunks folded
- Integration test: side-by-side creates two buffers with diffthis
- Integration test: toggling back to unified cleans up side-by-side

**Self-review:** Apply the Issue approach. Verify fold ranges match
hunk boundaries. Verify side-by-side buffers have correct filetype
set. Run `cargo fmt`. If anything is missed, create a follow-up issue.

### E4-12b: Inline thread indicators and untracked files

**Depends:** E4-9, E2-2

**Context:** Optional sign-column indicators in normal editing
buffers and support for untracked files in the review.

**Requirements:**

- Inline thread indicators (`config.inline_indicators`): `BufEnter`
  autocmd places sign extmarks on files with open threads. `go`
  on an indicator line opens the thread in a floating window,
  even outside the workbench.
- When no review is active, load threads from disk via
  `state::load_threads`. When active, use in-memory
  `review.threads`.
- Untracked file support: `git::untracked` results fed through
  `diff::synthesize_untracked` for rendering. Status icon shows
  `+` for untracked files.

**Test requirements:**

- Integration test: inline indicators appear on file with open threads
- Integration test: `go` on indicator opens thread window
- Integration test: untracked file renders as all-additions diff
- Integration test: untracked files appear in file panel with `+` icon

**Self-review:** Apply the Issue approach. Verify inline indicators
are cleared on `BufLeave`. Verify `go` outside workbench creates a
standalone float. Run `cargo fmt`. If anything is missed, create a
follow-up issue.

### E4-12c: Error hardening and documentation

**Depends:** E4-12a, E4-12b

**Context:** Final error handling, documentation pass, and user-facing
README.

**Requirements:**

- Error handling sweep: git failures show notification without
  crashing workbench, CLI failures show in thread window as inline
  error message, missing binary disables agent features with
  one-time ERROR notification, JSON parse errors logged with raw
  text preserved.
- `review::close` calls `backend::cancel_all` to prevent stale
  callbacks.
- Module-level `//!` docs on all files. `///` docs on all pub items.
- README.md with: project description, installation (lazy.nvim and
  manual), build from source, configuration reference, keybinding
  reference, usage guide with screenshots/examples.
- `cargo doc --no-deps` produces clean output (no warnings)

**Test requirements:**

- Integration test: git failure does not crash workbench
- Integration test: closing workbench with pending queue items does
  not panic
- `cargo doc --no-deps` produces no warnings
- `cargo clippy` produces no warnings

**Self-review:** Apply the Issue approach. Verify all M6 deliverables
from the PRD are implemented. Verify all PRD features are covered
across all epics (cross-reference checklist below). Run full test
suite. Run `cargo fmt`. If anything is missed, create a follow-up
issue.

---

## Epic-Level Reviews

Each epic review runs after all issues in that epic are complete. Apply
the standard approach: Rust skill compliance, documentation, testing.
**Any gaps identified become new issues.** Do not defer.

### Epic 0 Review

After completing E0-0 through E0-2:

- [ ] E0-0 spike confirms all 7 nvim-oxi APIs work (or documents
      workarounds)
- [ ] Both crates build and `cargo test` passes
- [ ] `cargo fmt` has been run on all changed code
- [ ] Every public item has a `///` doc comment
- [ ] No `unwrap`/`expect` without `// SAFETY:` comment
- [ ] CI workflow passes on clean checkout
- [ ] `check_boundaries.sh` passes
- [ ] Any gaps identified become new issues

### Epic 1 Review

After completing E1-1 through E1-3:

- [ ] All `git.rs` convenience wrappers work with real git repos
- [ ] `parse_hunks` handles all standard diff formats
- [ ] `render` produces correct buffer content and highlights
- [ ] `buf_line_to_source` mapping is accurate
- [ ] `detect_hunk_changes` correctly identifies new hunks
- [ ] Side-by-side creates and destroys buffers cleanly
- [ ] No imports from `crate::threads`, `crate::backend`, or
      `crate::review` in any S1 module
- [ ] All pub items have `///` doc comments
- [ ] `cargo test` passes for all S1 tests

### Epic 2 Review

After completing E2-1 and E2-2 (E2-3 is in Epic 4):

- [ ] Thread CRUD round-trips through persistence
- [ ] Re-anchoring handles shifted, deleted, and modified anchors
- [ ] Global ordering and navigation wrap correctly
- [ ] No imports from `crate::git`, `crate::diff`, `crate::backend`,
      or `crate::review` in `threads/mod.rs`
- [ ] All pub items have `///` doc comments
- [ ] `cargo test` passes for all S2 tests
- [ ] `cargo fmt` applied, no `unwrap`/`expect` without `// SAFETY:`
- [ ] Any gaps identified become new issues

### Epic 3 Review

After completing E3-1 through E3-3:

- [ ] Queue executes items in FIFO order, max concurrency 1
- [ ] Both adapters build correct CLI args for all flag combinations
- [ ] JSON parsing handles valid, malformed, and error responses
- [ ] Streaming calls `on_stream` for each event
- [ ] Self-review text parsing handles edge cases
- [ ] No imports from `crate::threads`, `crate::diff`, `crate::review`,
      or `crate::git` in any S3 module (only `nvim_oxi::schedule`)
- [ ] All pub items have `///` doc comments
- [ ] `cargo test` passes for all S3 tests
- [ ] `cargo fmt` applied, no `unwrap`/`expect` without `// SAFETY:`
- [ ] Any gaps identified become new issues

### Epic 4 Review

After completing E4-1 through E4-12c:

- [ ] `:Arbiter` opens workbench, `q` closes cleanly
- [ ] All navigation keymaps work (]c, [c, ]f, [f, ]t, [t, <CR>)
- [ ] All review marking keymaps work (<Leader>aa, <Leader>ax, <Leader>ar, <Leader>as)
- [ ] All thread keymaps work (<Leader>ac, <Leader>aC, <Leader>aA, <Leader>ao, <Leader>aR, <Leader>a?, <Leader>aT, <Leader>aU, <Leader>aP)
- [ ] All comments sent immediately (no batch mode)
- [ ] `:ArbiterSelfReview` creates agent threads
- [ ] Polling detects file changes and re-renders
- [ ] Turn cycling snapshots and hands back correctly
- [ ] All global commands work without review
- [ ] `:ArbiterList` lists persisted sessions
- [ ] `:ArbiterResume <id> <prompt>` works with and without prompt
- [ ] Side-by-side toggle works
- [ ] Fold support works
- [ ] Inline indicators work (when enabled)
- [ ] Queue cancellation on close prevents stale callbacks
- [ ] Error cases handled gracefully
- [ ] README exists with installation and usage
- [ ] `cargo test` passes for all tests
- [ ] `cargo doc --no-deps` produces no warnings
- [ ] `cargo fmt` applied across all Epics 1-4
- [ ] Any gaps identified become new issues

---

## PRD Feature Coverage Checklist

Cross-reference every PRD feature to its implementing issue:

| PRD Feature | Issue(s) |
|---|---|
| `:Arbiter [ref]` | E4-2 |
| File panel (tree, icons, summary) | E4-3 |
| Diff panel (unified, highlighting) | E1-3, E4-3 |
| `]c`/`[c` hunk navigation | E4-4 |
| `]f`/`[f` file navigation | E4-4 |
| `]t`/`[t` thread navigation | E4-6 |
| `<CR>` open file/thread | E4-4, E4-6 |
| `zo`/`zc` fold support | E4-12a |
| `<Leader>s` side-by-side | E1-3, E4-12a |
| `q` close workbench | E4-2 |
| `ga`/`gx`/`gr` review marking | E4-4 |
| `gs` review summary float | E4-4 |
| Review state persistence | E2-1, E4-4 |
| Content hash on approval | E4-4 |
| `<Leader>ac` comment (immediate) | E4-6, E4-8 |
| `<Leader>aC` comment (immediate) | E4-6, E4-8 |
| `gA` auto-resolve | E4-8 |
| `gU` manual refresh | E4-7 |
| Thread window (go, `<CR>`) | E2-3, E4-6 |
| Thread reply (in window) | E4-8 |
| `gR` resolve thread | E4-6 |
| `:ArbiterResolveAll` | E4-7 |
| `g?` toggle resolved | E4-6 |
| `gT` thread list + filters | E2-3, E4-6 |
| `dd` dismiss binned | E2-3 |
| `gP` agent re-anchor | E4-8 |
| Thread re-anchoring (content) | E2-2 |
| Thread re-anchoring (bin) | E4-9 |
| Thread persistence | E2-1 |
| ~~`:AgentSubmitReview`~~ | ~~E4-8~~ (future consideration) |
| `:ArbiterSelfReview` | E3-3, E4-11 |
| Agent threads `[agent]` display | E4-6, E4-11 |
| Live diff refresh (polling) | E4-9 |
| Hunk change indicators | E1-2, E4-9 |
| Approved file reset on change | E4-9 |
| Auto-resolve timeout | E2-2, E4-9 |
| Turn cycling (`<Leader>a`) | E4-10 |
| `:AgentTurn` / `:HumanTurn` | E4-10 |
| Statusline | E4-7 |
| `:ArbiterCatchUp` | E4-11 |
| `:ArbiterSend` / `:ArbiterContinue` | E4-11 |
| `:ArbiterList` / `:ArbiterResume` | E4-11 |
| `:ArbiterResume <id> <prompt>` | E4-11 |
| Session persistence | E2-1 |
| `:ArbiterRefresh` | E4-7, E4-9 |
| Response panel | E4-11 |
| Response routing (float vs panel) | E4-11 |
| Backend shim (Cursor) | E3-2 |
| Backend shim (Claude) | E3-2 |
| Session management (per-thread) | E3-1, E4-8 |
| Call queue (FIFO) | E3-1 |
| Queue cancellation on close | E3-1, E4-12c |
| Streaming responses | E3-3, E4-8 |
| Agent mode gating | E4-7 |
| Global commands (outside mode) | E4-7, E4-11 |
| Inline thread indicators | E4-12b |
| `fold_approved` config | E4-12a |
| Untracked file diffs | E1-2, E4-12b |
| Config (all options) | E4-1 |
| Error handling | E4-12c |
| Error types | E0-1 |
| Highlight groups | E4-1 |
| CI pipeline | E0-2 |
| Build / install tooling | E0-2 |
| README | E4-12c |
| Test infrastructure | E0-2 |
