# arbiter: Technical Design (RFD)

> **Implementation note (2026-03):** This document represents the
> original technical design. The current implementation differs in
> several areas. See the PRD implementation note for a summary of
> changes. Key differences relevant to this document:
>
> - All `g`-prefixed keymaps moved to `<Leader>a`-prefixed.
> - Batch comments (`gc`) and `:AgentSubmitReview` removed. All
>   comments are now sent immediately. These are documented below as
>   future considerations.
> - Thread lists use the quickfix list, not a custom float.
> - `dispatch.rs` module added for safe cross-thread callback
>   marshalling via `libuv::AsyncHandle`.
> - Build system uses `Taskfile.yml` instead of Makefile.
> - `chrono` added for timestamp formatting; `thiserror` removed.

This document covers the engineering design for arbiter. It is the
companion to the PRD, which defines the product requirements. Read the
PRD first.

## Implementation Language

The plugin is implemented in Rust using `nvim-oxi`, which compiles to
a shared library (`.so` / `.dylib`) that Neovim loads as a Lua module.
From the user's perspective, `require("arbiter")` works like any
Lua plugin. From the developer's perspective, all logic is Rust with
compile-time type safety, zero-cost abstractions, and Cargo-managed
dependencies.

### Why Rust

- **Type safety at the seams.** The work stream contracts (see below)
  are enforced by the compiler. A thread function that takes
  `&[Thread]` cannot accidentally receive a `Review`.
- **Predictable async.** Background work (git, CLI) runs on OS threads
  with `std::process::Command`. Results are sent back to Neovim's
  main thread via `nvim_oxi::schedule`. No callback pyramids, no
  coroutine gotchas.
- **Serde for persistence.** `Thread`, `ReviewState`, and `Config` are
  `#[derive(Serialize, Deserialize)]`. Persistence is one line.
- **Performance.** Diff parsing, hunk indexing, content hashing, and
  re-anchoring are CPU-bound string operations that benefit from
  compiled code.

### Neovim Integration

`nvim-oxi` provides typed Rust bindings to the Neovim C API:

- `nvim_oxi::api::{Buffer, Window, TabPage}` for handle types
- `nvim_oxi::api::*` for all `nvim_buf_*`, `nvim_win_*`, etc.
- `nvim_oxi::schedule(callback)` to run a closure on the main thread
  from a background thread
- The `#[nvim_oxi::plugin]` attribute macro generates the shared
  library entry point

The plugin exposes a single Lua-callable function, `setup()`, via the
plugin entry point. Commands and keymaps are registered via the
Neovim API from Rust.

**API validation (E0-0):** Before building the full scaffold, a
throwaway spike should validate the critical `nvim-oxi` APIs:
libuv timers, buffer-local keymap callbacks, user commands with
args, `schedule` from background threads, extmarks, floating
windows, and the `#[nvim_oxi::test]` harness. If any API does not
work as designed, the architecture adjusts before implementation
begins.

### Async Architecture

Neovim is single-threaded. All Neovim API calls must happen on the
main thread. Background work (git commands, CLI calls) runs on OS
threads and sends results back via `nvim_oxi::schedule`.

```
Main thread (Neovim)          Background threads
────────────────────          ──────────────────
  setup()
  :Arbiter
    → spawn git diff ──────────→ std::process::Command
                                   git diff --name-status
                                   ↓
    ← nvim_oxi::schedule ◄────── send result back
    render file panel
    render diff panel
    start poll timer
```

No tokio runtime. No async/await. The pattern is simple:

1. Spawn `std::thread::spawn` for blocking I/O
2. Capture results
3. Call `nvim_oxi::schedule(move || { ... })` to run Neovim API
   calls on the main thread

Timers (for polling) use `nvim_oxi::libuv::TimerHandle`, which
provides type-safe access to Neovim's libuv event loop directly from
Rust. No Lua shim needed.

## Crate Structure

The project is a Cargo workspace. The main plugin is a `cdylib` crate
in `crates/arbiter/`. Integration tests live in a separate `cdylib`
crate in `tests/` (required by `nvim-oxi`'s test infrastructure; see
Testing Strategy below).

```
arbiter/
├── Cargo.toml              -- workspace root
├── crates/
│   └── arbiter/
│       ├── Cargo.toml      -- [lib] crate-type = ["cdylib"]
│       └── src/
│           ├── lib.rs            -- #[nvim_oxi::plugin], setup(), command registration
│           ├── config.rs         -- Config struct, defaults, deserialization
│           ├── error.rs          -- NvimAgentError, thiserror-based error types
│           ├── review.rs         -- Review lifecycle, workbench management
│           ├── diff/
│           │   ├── mod.rs        -- re-exports, DiffEngine public API
│           │   ├── parse.rs      -- hunk parsing from unified diff text
│           │   └── render.rs     -- buffer rendering, highlight application
│           ├── file_panel.rs     -- file tree rendering, status icons
│           ├── threads/
│           │   ├── mod.rs        -- Thread CRUD, re-anchoring, filtering
│           │   ├── window.rs     -- floating thread window
│           │   └── input.rs      -- comment input float
│           ├── state.rs          -- JSON persistence (review state + threads + sessions)
│           ├── git.rs            -- async git command execution
│           ├── backend/
│           │   ├── mod.rs        -- Adapter trait, dispatch, public API
│           │   ├── queue.rs      -- FIFO call queue
│           │   ├── cursor.rs     -- Cursor CLI adapter
│           │   └── claude.rs     -- Claude Code CLI adapter
│           ├── turn.rs           -- agent/human turn cycling, snapshots
│           ├── poll.rs           -- mtime polling, file list refresh
│           ├── highlight.rs      -- highlight group definitions
│           └── types.rs          -- shared types (enums, small structs)
├── tests/
│   ├── Cargo.toml          -- [lib] crate-type = ["cdylib"], nvim-oxi "test" feature
│   ├── build.rs            -- nvim_oxi::tests::build()
│   └── src/                -- see Testing Strategy section
├── scripts/
│   └── check_boundaries.sh -- module boundary enforcement
├── plugin/
│   └── arbiter.lua      -- require("arbiter") (loads the .so)
└── docs/
    ├── prd.md
    ├── rfd.md
    └── issues.md
```

Each Rust module is a file or directory with a `mod.rs`. No global
mutable state; all mutable state lives in a `Review` struct held
behind `RefCell<Option<Review>>` in a thread-local, accessed via
`review::with_active(|r| ...)`. The `Review` is created by
`:Arbiter` and dropped when `q` closes the workbench.

## Core Data Structures

All types live in their owning module. Shared enums (`Turn`,
`ThreadOrigin`, etc.) live in `types.rs`. Types that are persisted to
disk derive `Serialize` and `Deserialize`.

### Review (review.rs)

The central runtime object. One exists per open review workbench.
Dropped on close. Not serialized.

```rust
pub struct Review {
    pub ref_name: String,
    /// Working directory captured at review::open() time.
    pub cwd: String,
    pub tabpage: TabPage,
    pub file_panel: Panel,
    pub diff_panel: Panel,
    pub response_buf: Option<Buffer>,
    pub response_win: Option<Window>,

    pub files: Vec<FileEntry>,
    pub file_index: HashMap<String, usize>,
    pub current_file: Option<String>,

    pub threads: Vec<Thread>,
    pub thread_index: HashMap<String, usize>,

    pub review_session_id: Option<String>,
    pub turn: Turn,
    pub snapshot_hash: Option<String>,

    pub show_resolved: bool,
    pub side_by_side: bool,
    pub sbs: Option<SideBySide>,
    pub thread_buf_lines: HashMap<String, usize>,

    pub config: Config,
}

pub struct Panel {
    pub buf: Buffer,
    pub win: Window,
}
```

### FileEntry (types.rs)

```rust
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub status: FileStatus,
    pub review_status: ReviewStatus,
    pub content_hash: String,
    pub updated_at: i64,
    pub hunks: Vec<Hunk>,
    pub thread_count: usize,
    pub mtime: i64,
    pub prev_hunk_hashes: HashSet<String>,
}
```

### Hunk (diff/parse.rs)

```rust
#[derive(Debug, Clone)]
pub struct Hunk {
    pub buf_start: usize,
    pub buf_end: usize,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
    pub content_hash: String,
}
```

### Thread (threads/mod.rs)

Persisted to disk.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub origin: ThreadOrigin,
    pub file: String,
    pub line: u32,
    pub anchor_content: String,
    pub anchor_context: Vec<String>,
    pub status: ThreadStatus,
    pub auto_resolve: bool,
    pub auto_resolve_at: Option<i64>,
    pub context: ThreadContext,
    pub session_id: Option<String>,
    pub messages: Vec<Message>,
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub ts: i64,
}
```

### ThreadSummary (threads/mod.rs)

Display-only projection of a Thread, used at the boundary between
the thread data layer and the diff engine. The diff module does not
import `threads`; it receives this struct instead.

```rust
#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub id: String,
    pub origin: ThreadOrigin,
    pub line: u32,
    pub preview: String,
    pub status: ThreadStatus,
}
```

Produced by `threads::to_summaries()`, consumed by `diff::render()`.

### Shared Enums (types.rs)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Turn { Agent, Human }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadOrigin { User, Agent }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadStatus { Open, Resolved, Binned }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadContext { Review, SelfReview }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role { User, Agent }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus { Modified, Added, Deleted, Untracked }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewStatus { Unreviewed, Approved, NeedsChanges }
```

### BackendResult and BackendOpts (backend/mod.rs)

```rust
#[derive(Debug, Clone)]
pub struct BackendResult {
    pub text: String,
    pub session_id: String,
    pub error: Option<String>,
}

/// Which session lifecycle to use for this CLI call.
#[derive(Debug, Clone)]
pub enum BackendOp {
    /// Start a new session.
    NewSession,
    /// Resume a specific session by ID.
    Resume(String),
    /// Continue the most recent session.
    ContinueLatest,
}

#[derive(Debug)]
pub struct BackendOpts {
    pub op: BackendOp,
    pub prompt: String,
    pub ask_mode: bool,
    pub stream: bool,
    pub json_schema: Option<String>,
}

/// Arc-wrapped so the adapter can clone into each nvim_oxi::schedule
/// call when streaming multiple chunks.
pub type OnStream = Arc<dyn Fn(&str) + Send + Sync>;
pub type OnComplete = Box<dyn FnOnce(BackendResult) + Send>;
```

## Agent Mode

Agent mode is the plugin's operational state, tracked by whether a
`Review` object exists. It is not a Neovim mode.

### State Tracking

```rust
// In review.rs:
use std::cell::RefCell;

thread_local! {
    static ACTIVE_REVIEW: RefCell<Option<Review>> = RefCell::new(None);
}

pub fn with_active<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Review) -> R,
{
    ACTIVE_REVIEW.with(|cell| {
        cell.borrow_mut().as_mut().map(f)
    })
}

pub fn is_active() -> bool {
    ACTIVE_REVIEW.with(|cell| cell.borrow().is_some())
}
```

### Command Classification

Commands are split into two categories at registration time:

**Global commands** (always available): `Arbiter`, `ArbiterSend`,
`ArbiterContinue`, `ArbiterCatchUp`, `ArbiterList`, `ArbiterResume`.

**Gated commands** (require active review): `ArbiterSelfReview`,
`ArbiterResolveAll`, `ArbiterRefresh`, `AgentTurn`, `HumanTurn`.

> **Future consideration:** `AgentSubmitReview` was originally listed
> here for batch comment submission. Removed in favor of immediate
> submission.

Gated commands use a guard:

```rust
fn with_review_cmd(f: impl Fn(&mut Review)) {
    match review::with_active(|r| f(r)) {
        Some(()) => {}
        None => {
            api::notify(
                "No active review. Run :Arbiter first.",
                LogLevel::Warn,
            );
        }
    }
}
```

**Keymaps** are buffer-local, set when the review opens and cleared
when it closes. They exist only on workbench buffers. No guard needed
since the buffers themselves only exist during agent mode.

### Response Routing

Global commands that produce agent responses route output based on
whether a workbench is open:

```rust
fn route_response(text: &str) {
    let routed = review::with_active(|r| {
        review::show_response(r, text);
    });
    if routed.is_none() {
        show_floating_response(text);
    }
}

fn show_floating_response(text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let buf = api::create_buf(false, true).unwrap();
    buf.set_lines(0, -1, false, &lines).unwrap();
    buf.set_option("filetype", "markdown").unwrap();
    buf.set_option("buftype", "nofile").unwrap();

    let width = 80.min(api::get_option::<i64>("columns").unwrap() as usize - 10);
    let height = (lines.len() + 2).min(api::get_option::<i64>("lines").unwrap() as usize - 6);

    let win_config = WindowConfig::builder()
        .relative(WindowRelativeTo::Editor)
        .width(width as u32)
        .height(height as u32)
        .row((api::get_option::<i64>("lines").unwrap() as u32 - height as u32) / 2)
        .col((api::get_option::<i64>("columns").unwrap() as u32 - width as u32) / 2)
        .border(WindowBorder::Rounded)
        .style(WindowStyle::Minimal)
        .title("Agent Response")
        .build();

    api::open_win(&buf, true, &win_config).unwrap();
    // set q and <Esc> to close
}
```

### Inline Thread Indicators

When `config.inline_indicators = true`, the plugin registers a
`BufEnter` autocmd. On each buffer entry, it looks up threads for
the current file and places sign-column extmarks on anchored lines.

```rust
fn apply_inline_indicators(buf: &Buffer, threads: &[Thread]) {
    let ns = api::create_namespace("arbiter-inline");
    buf.clear_namespace(ns, 0, -1);

    let file = buf.get_name().unwrap_or_default();
    let file = relative_path(&file);

    for t in threads.iter().filter(|t| t.file == file && t.status == ThreadStatus::Open) {
        let hl = match t.origin {
            ThreadOrigin::Agent => "ArbiterIndicatorAgent",
            ThreadOrigin::User => "ArbiterIndicatorUser",
        };
        let _ = buf.set_extmark(ns, t.line.saturating_sub(1) as usize, 0, &ExtmarkOpts {
            sign_text: Some("▎"),
            sign_hl_group: Some(hl),
            ..Default::default()
        });
    }
}
```

The cost is one `state::load_threads()` call per `BufEnter` when no
review is active (reads a single JSON file, typically <50KB). When a
review is active, the in-memory `review.threads` is used instead.

### Statusline

The statusline function is always available, exported as a
Lua-callable function:

```rust
#[nvim_oxi::module]
fn arbiter() -> Result<Dictionary> {
    // ...
    Ok(Dictionary::from_iter([
        ("statusline", Object::from(Function::from_fn(statusline))),
    ]))
}

fn statusline(_: ()) -> Result<String> {
    let result = review::with_active(|r| {
        let turn = match r.turn {
            Turn::Agent => "[AGENT]",
            Turn::Human => "[HUMAN]",
        };
        let approved = r.files.iter()
            .filter(|f| f.review_status == ReviewStatus::Approved)
            .count();
        format!("{} [REVIEW {}/{}]", turn, approved, r.files.len())
    });
    Ok(result.unwrap_or_default())
}
```

Returns an empty string when no review is active.

## Module Contracts

Contracts are expressed as `pub fn` signatures. Each module's public
API is its contract. Rust's module visibility enforces these
boundaries at compile time.

### lib.rs

Plugin entry point. Exports `setup()` and `statusline()` as
Lua-callable functions.

```rust
/// Called by the user in their Neovim config via require("arbiter").setup({...}).
/// Deserializes the Lua table into Config, registers commands and autocmds.
pub fn setup(config: LuaTable) -> Result<()>;

/// Statusline component. Always safe to call.
pub fn statusline() -> String;
```

### config.rs

```rust
/// Plugin configuration. Deserialized from the Lua table passed to setup().
/// Missing fields use defaults via #[serde(default)].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub backend: BackendKind,
    pub model: Option<String>,
    /// Workspace root. Defaults to cwd at setup() time.
    /// Passed to CLI adapters as --workspace.
    pub workspace: Option<String>,
    pub inline_indicators: bool,
    pub review: ReviewConfig,
    pub prompts: PromptConfig,
    pub keymaps: KeymapConfig,
}

impl Default for Config { /* opinionated defaults from PRD */ }
```

### review.rs

Owns the `Review` lifecycle.

```rust
/// Opens the review workbench in a new tabpage.
pub fn open(ref_name: Option<&str>, config: &Config);

/// Closes the workbench, persists state, cleans up.
pub fn close();

/// Refreshes the diff panel for the current file.
pub fn refresh_file(review: &mut Review);

/// Refreshes the file list.
pub fn refresh_file_list(review: &mut Review);

/// Switches the diff panel to a different file.
pub fn select_file(review: &mut Review, path: &str);

/// Runs a closure with the active Review, if one exists.
pub fn with_active<F, R>(f: F) -> Option<R>
where F: FnOnce(&mut Review) -> R;
```

`open()` is the main entry point. It:

1. Creates a new tabpage
2. Creates the file panel buffer and window (vertical split, left,
   fixed width)
3. Creates the diff panel buffer and window (fills remaining space)
4. Spawns `git::diff_names()` to populate the file list
5. Spawns `git::untracked()` to add untracked files
6. Loads persisted review state from disk (`state.rs`)
7. Loads persisted threads from disk (`state.rs`)
8. Renders the file panel (`file_panel::render()`)
9. Selects the first unreviewed file and renders its diff
10. Sets up buffer-local keymaps on both panels
11. Starts the poll timer (`poll::start()`)
12. Stores the `Review` in the thread-local

`close()` reverses all of it: stops timers, persists state, drops
the `Review`, closes the tabpage.

### diff (diff/mod.rs)

Parses git diff output and renders into a buffer.

```rust
/// Parses raw unified diff text into structured hunks.
pub fn parse_hunks(diff_text: &str) -> Vec<Hunk>;

/// Renders diff into a buffer. Inserts file header and thread summary
/// lines above the diff content. Returns hunks with buffer positions
/// and the thread-to-buffer-line mapping.
pub fn render(
    buf: &Buffer,
    diff_text: &str,
    summaries: &[ThreadSummary],
    file_path: &str,
    show_resolved: bool,
) -> (Vec<Hunk>, HashMap<String, usize>);

/// Applies syntax highlighting to the diff buffer.
pub fn apply_highlights(buf: &Buffer, hunks: &[Hunk], summaries: &[ThreadSummary]);

/// Maps a buffer line to a source file location.
pub fn buf_line_to_source(hunks: &[Hunk], buf_line: usize) -> Option<SourceLocation>;

/// Produces a synthetic all-additions diff for an untracked file.
pub fn synthesize_untracked(contents: &str, path: &str) -> String;

/// Compares two hunk sets by content hash. Returns buf_start lines
/// for new/changed hunks.
pub fn detect_hunk_changes(
    old_hashes: &HashSet<String>,
    new_hunks: &[Hunk],
) -> HashSet<usize>;

#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub file: String,
    pub line: usize,
}
```

**Rendering pipeline:**

1. Receive raw `git diff` output (`&str`)
2. Parse into `Vec<Hunk>` via `parse_hunks()`
3. Build buffer content: file header line, thread summary lines,
   then the raw diff lines
4. Write to buffer via `buf.set_lines()`
5. Apply highlights via `buf.add_highlight()` per line prefix
6. Return hunks with `buf_start`/`buf_end` updated to account for
   the injected header and thread lines

The buffer is always re-rendered from scratch. No incremental updates.

### file_panel.rs

```rust
/// Renders the file panel buffer content from the file list.
pub fn render(buf: &Buffer, files: &[FileEntry], config: &Config);

/// Returns the file path at the given buffer line, or None.
pub fn path_at_line(line: usize) -> Option<String>;
```

The file panel is re-rendered from scratch on every update. The
rendering builds a tree structure from flat file paths, prepends
status icons, and appends the summary section.

The line-to-path mapping is stored in a `thread_local!` wrapping
`RefCell<Vec<Option<String>>>`, indexed by line number.

### threads (threads/mod.rs)

Thread CRUD and lifecycle. All functions take `&mut Vec<Thread>` or
`&[Thread]` rather than `&mut Review` to stay decoupled from the
review lifecycle. The UI layer passes `&mut review.threads`.

```rust
/// Creates a new thread with a UUID, initial message, and anchor data.
pub fn create(file: &str, line: u32, text: &str, opts: CreateOpts) -> Thread;

pub struct CreateOpts {
    pub pending: bool,
    pub immediate: bool,
    pub auto_resolve: bool,
    pub origin: ThreadOrigin,
}

/// Appends a message to a thread's conversation.
pub fn add_message(thread: &mut Thread, role: Role, text: &str);

/// Sets thread status to Resolved.
pub fn resolve(thread: &mut Thread);

/// Sets thread status to Binned (anchor lost).
pub fn bin(thread: &mut Thread);

/// Content-match re-anchoring. Mutates matched threads in place.
/// Returns indices of unmatched threads.
pub fn reanchor_by_content(
    threads: &mut [Thread],
    file: &str,
    new_contents: &str,
) -> Vec<usize>;

/// Returns threads for a file, sorted by line ascending.
pub fn for_file(threads: &[Thread], file: &str) -> Vec<&Thread>;

/// Returns indices of pending (batch, not yet sent) threads.
pub fn pending_indices(threads: &[Thread]) -> Vec<usize>;

/// Resolves all open threads.
pub fn resolve_all(threads: &mut [Thread]);

/// Removes a thread from the list by index.
pub fn dismiss(threads: &mut Vec<Thread>, index: usize);

/// Filters threads by origin and/or status.
pub fn filter(threads: &[Thread], opts: &FilterOpts) -> Vec<&Thread>;

pub struct FilterOpts {
    pub origin: Option<ThreadOrigin>,
    pub status: Option<ThreadStatus>,
}

/// Returns all open threads sorted by (file_order index, line).
pub fn sorted_global(threads: &[Thread], file_order: &[String]) -> Vec<usize>;

/// Returns the index of the next/prev thread in a sorted index list.
pub fn next_thread(sorted: &[usize], current: Option<usize>) -> Option<usize>;
pub fn prev_thread(sorted: &[usize], current: Option<usize>) -> Option<usize>;

/// Projects threads into display-only summaries for the diff engine.
pub fn to_summaries(threads: &[Thread]) -> Vec<ThreadSummary>;

/// Checks auto-resolve timeouts. Mutates timed-out threads (clears
/// auto_resolve flag). Returns indices of timed-out threads.
pub fn check_auto_resolve_timeouts(
    threads: &mut [Thread],
    timeout_secs: u64,
    now: i64,
) -> Vec<usize>;
```

**Content-based re-anchoring algorithm:**

```
for each thread anchored to `file`:
  search new_file_contents for thread.anchor_content
  if found:
    verify at least 1 anchor_context line exists nearby (within 5 lines)
    if verified: update thread.line to the new line number
    else: add to unmatched list
  else:
    add to unmatched list

return unmatched indices
```

The search is `str::find`, not fuzzy. If the anchor line was
rewritten, content matching should fail and the thread should go to
the bin rather than silently attaching to the wrong line.

### threads/window.rs

Manages the floating window for viewing and replying to a thread.

```rust
/// Opens a floating window for the given thread.
pub fn open(thread: &Thread) -> Window;

/// Closes the thread window if open.
pub fn close();

/// Appends a message to the currently open thread window.
pub fn append_message(role: Role, text: &str);

/// Appends streaming text to the last agent message.
pub fn append_streaming(chunk: &str);

/// Returns true if a thread window is currently open.
pub fn is_open() -> bool;
```

**Window layout:**

```
width:  min(80, editor_width - 10)
height: min(message_lines + 5, editor_height - 6)
anchor: NW, row: 3, col: center
border: rounded
```

The buffer is fully readonly. `<CR>` in normal mode opens a small
input float (via `threads::input`) for composing a reply. `q` closes
the thread window.

### threads/input.rs

Small floating buffer for entering text. Used for both new comments
and thread replies.

```rust
/// Opens the input float. Fires on_submit with the entered text.
/// The float appears anchored below the thread window or at the
/// cursor position for new comments.
pub fn open(on_submit: impl FnOnce(String) + 'static);

/// Opens the input float pre-configured for a specific file/line
/// (used by gc/gC to create new threads).
pub fn open_for_line(file: &str, line: u32, on_submit: impl FnOnce(String) + 'static);

/// Closes the input float.
pub fn close();
```

**Window layout:**

```
width:  60, height: 5
anchor: cursor-relative (below current line)
border: rounded
title:  "Comment on {file}:{line}"
```

`<CR>` in normal mode submits. `q` or `<Esc>` cancels.

### state.rs

Persistence layer. Reads and writes JSON files.

```rust
/// Loads review state from disk. Returns default if file doesn't exist.
pub fn load_review(state_dir: &Path, ws_hash: &str, ref_name: &str) -> ReviewState;

/// Saves review state to disk.
pub fn save_review(state_dir: &Path, ws_hash: &str, ref_name: &str, state: &ReviewState);

/// Loads threads from disk.
pub fn load_threads(state_dir: &Path, ws_hash: &str, ref_name: &str) -> Vec<Thread>;

/// Saves threads to disk.
pub fn save_threads(state_dir: &Path, ws_hash: &str, ref_name: &str, threads: &[Thread]);

/// SHA256 of the workspace path, truncated to 12 hex chars.
pub fn workspace_hash(path: &Path) -> String;

/// Fast content hash for change detection.
pub fn content_hash(text: &str) -> String;

/// Persisted review state (file statuses).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewState {
    pub files: HashMap<String, FileState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub status: ReviewStatus,
    pub content_hash: String,
    pub updated_at: i64,
}
```

**When state is saved:** On every review status change, on every
thread mutation, and on `review::close()`. Saves are synchronous
(`serde_json::to_writer` + `std::fs::File`). The files are small
(<100KB even for large reviews).

### git.rs

Async git command runner. Spawns `git` on a background thread and
schedules the callback on the Neovim main thread.

```rust
/// Result of a git command.
pub struct GitResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a git command asynchronously.
pub fn run(args: &[&str], cwd: &str, callback: impl FnOnce(GitResult) + Send + 'static);

/// Convenience wrappers:
pub fn diff(ref_name: &str, file: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn diff_names(ref_name: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn untracked(cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn show(ref_name: &str, file: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn stash_create(cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn diff_hash(hash: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn file_mtime(path: &str) -> Option<i64>;
```

**Implementation:**

```rust
pub fn run(args: &[&str], cwd: &str, callback: impl FnOnce(GitResult) + Send + 'static) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let cwd = cwd.to_string();

    std::thread::spawn(move || {
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&cwd)
            .output();

        let result = match output {
            Ok(out) => GitResult {
                exit_code: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(e) => GitResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: e.to_string(),
            },
        };

        nvim_oxi::schedule(move |_| callback(result));
    });
}
```

The callback runs on the Neovim main thread via `nvim_oxi::schedule`,
so it can safely call Neovim API functions.

`file_mtime` is synchronous (single `std::fs::metadata` call).

### backend/mod.rs

Shim dispatch layer. All methods accept fully assembled prompt
strings. The backend module does not import `threads` or `review`.
This is the primary decoupling point between Stream 3 and the rest.

```rust
/// Initializes the backend.
pub fn setup(config: &Config);

/// Enqueues a CLI call.
pub fn send(opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete);

/// Convenience methods. Each constructs BackendOpts with the correct
/// BackendOp variant and flags, then calls send().
pub fn send_comment(prompt: &str, callback: OnComplete);
pub fn thread_reply(session_id: Option<&str>, prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);
pub fn catch_up(session_id: Option<&str>, prompt: &str, callback: OnComplete);
pub fn handback(session_id: Option<&str>, prompt: &str, callback: OnComplete);
pub fn self_review(prompt: &str, callback: OnComplete);
pub fn re_anchor(session_id: &str, prompt: &str, callback: OnComplete);
pub fn send_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);
pub fn continue_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);

/// Returns true if the queue has pending items.
pub fn is_busy() -> bool;

/// Cancels all pending queue items and marks the current in-flight
/// callback as stale. Called by review::close().
pub fn cancel_all();
```

### backend/queue.rs

FIFO queue for CLI calls.

```rust
struct QueueItem {
    opts: BackendOpts,
    on_stream: Option<OnStream>,
    callback: OnComplete,
}

static QUEUE: LazyLock<Mutex<VecDeque<QueueItem>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
static PROCESSING: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn push(item: QueueItem) {
    QUEUE.lock().unwrap().push_back(item);
    if !PROCESSING.swap(true, Ordering::SeqCst) {
        process_next();
    }
}

/// A drop guard that calls process_next() even if the callback panics.
/// Without this, a panicking callback would leave PROCESSING=true and
/// the queue would hang forever.
struct DrainGuard;
impl Drop for DrainGuard {
    fn drop(&mut self) { process_next(); }
}

fn process_next() {
    let item = QUEUE.lock().unwrap().pop_front();
    match item {
        Some(item) => {
            let gen = GENERATION.load(Ordering::SeqCst);
            let adapter = get_adapter();
            adapter.execute(item.opts, item.on_stream, move |result| {
                if GENERATION.load(Ordering::SeqCst) != gen {
                    return; // cancelled
                }
                let _guard = DrainGuard;
                (item.callback)(result);
            });
        }
        None => {
            PROCESSING.store(false, Ordering::SeqCst);
        }
    }
}

pub fn cancel_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    QUEUE.lock().unwrap().clear();
    PROCESSING.store(false, Ordering::SeqCst);
}
```

The queue drains via callbacks with a `DrainGuard` that ensures
`process_next()` is called even if a callback panics. `cancel_all()`
increments the generation counter so in-flight callbacks no-op, clears
pending items, and resets the processing flag.

### backend/cursor.rs and backend/claude.rs

Each adapter implements the `Adapter` trait:

```rust
pub trait Adapter: Send + Sync {
    fn execute(&self, opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete);
    fn binary_name(&self) -> &str;
}
```

The adapter builds CLI args from `BackendOpts`, spawns the process
on a background thread, parses the JSON response, and schedules the
callback with a `BackendResult`.

**Cursor adapter flag assembly:**

```rust
fn build_args(&self, opts: &BackendOpts) -> Vec<String> {
    let fmt = if opts.stream { "stream-json" } else { "json" };
    let mut args = vec![
        "-p".into(), opts.prompt.clone(),
        "--output-format".into(), fmt.into(),
    ];

    match &opts.op {
        BackendOp::Resume(sid) => args.extend(["--resume".into(), sid.clone()]),
        BackendOp::ContinueLatest => args.push("--continue".into()),
        BackendOp::NewSession => {}
    }
    if opts.ask_mode {
        args.extend(["--mode".into(), "ask".into()]);
    }
    if opts.stream {
        args.push("--stream-partial-output".into());
    }
    if let Some(ref model) = self.config.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(ref dir) = self.config.workspace {
        args.extend(["--workspace".into(), dir.clone()]);
    }
    args
}
```

**Claude adapter differences:** `--include-partial-messages` for
streaming, `--permission-mode plan` for ask mode, `--json-schema`
for structured output, `--add-dir` for workspace.

**CLI stability risk:** The Claude CLI is open-source with versioned
releases and is the more stable of the two backends. The Cursor CLI
(`agent`) is proprietary; its flags (`--resume`, `--output-format`,
`--stream-partial-output`, etc.) are undocumented and may change in
any Cursor update. The Cursor adapter should be treated as
best-effort and may require maintenance after Cursor updates. Claude
is the recommended primary backend. If a Cursor CLI flag changes, the
adapter's `build_args` is the only place that needs updating.

**JSON response parsing:** Both CLIs return
`{ "session_id": "...", "result": "..." }` in JSON mode. Parsed
with `serde_json::from_str`. For streaming, each stdout line is a
JSON object; the adapter calls `on_stream` for each `assistant` event.

**Session expiry handling:** CLI sessions may expire or be garbage
collected by the CLI process (especially across CLI restarts or
updates). When `BackendOp::Resume(session_id)` fails because the
session no longer exists, the adapter should:

1. Detect the failure (non-zero exit or error in the JSON response)
2. Log a warning: "Session {id} expired, starting new session"
3. Retry the same prompt with `BackendOp::NewSession`
4. Return the new `session_id` in the `BackendResult`

The wiring layer updates the thread's `session_id` with the new
value. This makes session expiry transparent to the user. The retry
happens inside `adapter.execute()`, not in the queue.

### poll.rs

```rust
use nvim_oxi::libuv::TimerHandle;

thread_local! {
    static FILE_TIMER: RefCell<Option<TimerHandle>> = RefCell::new(None);
    static LIST_TIMER: RefCell<Option<TimerHandle>> = RefCell::new(None);
}

/// Starts both poll timers for the given review.
pub fn start(config: &ReviewConfig);

/// Stops and drops both timers.
pub fn stop();

/// Switches the file poll target to a new file.
pub fn set_target(file_path: &str);
```

Two `TimerHandle` timers (via `nvim-oxi`'s `libuv` feature):

1. **File poll timer** (default 2s): checks the current file's mtime
   via `std::fs::metadata`. If changed, triggers `review::refresh_file`.

2. **File list timer** (default 5s): spawns `git::diff_names` +
   `git::untracked`. If the file set changed, triggers
   `review::refresh_file_list`.

### turn.rs

```rust
/// Enters agent turn. Diffs against snapshot if returning from human turn.
pub fn enter_agent();

/// Enters human turn. Takes working tree snapshot.
pub fn enter_human();
```

**Snapshot flow:**

```
enter_human:
  git::stash_create(cwd, |result| {
    review.snapshot_hash = if result.stdout.is_empty() { None } else { Some(result.stdout.trim()) };
  })

enter_agent:
  if let Some(hash) = review.snapshot_hash.take() {
    git::diff_hash(&hash, cwd, |result| {
      if !result.stdout.is_empty() {
        backend::handback(session_id, prompt_with_diff, |_| {
          review::refresh_file(review);
          review::refresh_file_list(review);
        });
      }
    })
  }
```

### highlight.rs

Defines all custom highlight groups on plugin load.

```rust
pub fn setup();
```

Groups:

| Group                | Default link | Purpose                        |
|----------------------|-------------|--------------------------------|
| `ArbiterDiffAdd`       | `DiffAdd`    | Added lines in unified diff    |
| `ArbiterDiffDelete`    | `DiffDelete` | Deleted lines                  |
| `ArbiterDiffChange`    | `DiffChange` | Hunk headers                   |
| `ArbiterDiffFile`      | `Title`      | File header line               |
| `ArbiterThreadUser`    | `Comment`    | User thread summary line       |
| `ArbiterThreadAgent`   | `WarningMsg` | Agent thread summary line      |
| `ArbiterThreadResolved`| `NonText`    | Resolved thread line           |
| `ArbiterStatusApproved`| `DiagnosticOk` | ✓ in file panel             |
| `ArbiterStatusChanges` | `DiagnosticError` | ✗ in file panel           |
| `ArbiterStatusPending` | `NonText`    | · in file panel                |
| `ArbiterHunkNew`       | `DiffAdd`    | Hunk that appeared on refresh  |
| `ArbiterIndicatorUser` | `DiagnosticHint` | Inline sign for user thread |
| `ArbiterIndicatorAgent`| `DiagnosticWarn` | Inline sign for agent thread|

All groups link to built-in groups by default so they inherit the
user's colorscheme. Users can override with `nvim_set_hl()`.

## Keymap Binding Strategy

All keymaps are buffer-local, set when the review opens and cleared
when it closes. No global keymaps are ever created.

**Diff panel keymaps** (set on the diff buffer):

| Key              | Action                                      |
|------------------|---------------------------------------------|
| `]c` / `[c`      | Next / previous hunk                        |
| `]f` / `[f`      | Next / previous file                        |
| `]t` / `[t`      | Next / previous thread (cross-file)         |
| `<Leader>ac`     | Add comment at cursor line (sent immediately) |
| `<Leader>aC`     | Same as `<Leader>ac`                        |
| `<Leader>aA`     | Add auto-resolve comment at cursor line     |
| `<Leader>aa`     | Toggle file approval                        |
| `<Leader>ax`     | Mark file needs-changes                     |
| `<Leader>ar`     | Reset file to unreviewed                    |
| `<Leader>as`     | Show review summary float                   |
| `<Leader>aT`     | Thread list (quickfix, + `a`/`u`/`b`/`o` filters) |
| `<Leader>a?`     | Toggle resolved thread visibility           |
| `<Leader>aK`     | Cancel pending backend requests             |
| `<Leader>an`     | Next unreviewed file                        |
| `<Leader>ap`     | Previous unreviewed file                    |
| `<Leader>s`      | Toggle side-by-side diff                    |
| `<CR>`           | Open thread (on summary line) or source file|
| `zo` / `zc`      | Open / close fold (built-in, no binding)    |
| `q`              | Close the workbench                         |

Keymaps are set via `nvim_oxi::api::set_keymap` with buffer-local
scope. Each keymap closure captures no direct references to `Review`;
instead, it calls `review::with_active(|r| ...)` to access the
current review state. This avoids lifetime issues with closures.

```rust
fn set_diff_keymaps(buf: &Buffer, config: &KeymapConfig) {
    let buf_id = buf.handle();

    buf.set_keymap("n", &config.next_hunk, "", &SetKeymapOpts {
        callback: Some(Box::new(move |_| {
            review::with_active(|r| {
                // find next hunk.buf_start after cursor line, move cursor
            });
        })),
        silent: true,
        ..Default::default()
    });

    buf.set_keymap("n", &config.comment, "", &SetKeymapOpts {
        callback: Some(Box::new(move |_| {
            review::with_active(|r| {
                let cursor = api::get_current_win().get_cursor().unwrap();
                if let Some(source) = diff::buf_line_to_source(&r.hunks(), cursor.0) {
                    threads::input::open(&source.file, source.line as u32, |text| {
                        review::with_active(|r| {
                            let thread = threads::create(&source.file, source.line as u32, &text,
                                CreateOpts { pending: true, ..Default::default() });
                            r.threads.push(thread);
                            state::save_threads(...);
                            review::refresh_file(r);
                        });
                    });
                }
            });
        })),
        silent: true,
        ..Default::default()
    });

    // ... same pattern for all other keymaps
}
```

**File panel keymaps** (set on the file panel buffer):

```rust
api::set_keymap(buf, "n", "<CR>", move |_| {
    if let Some(path) = file_panel::path_at_line(&buf, cursor_line) {
        review::select_file(path);
    }
});
```

**Thread window keymaps** (set on the thread window buffer):

```rust
api::set_keymap(buf, "n", "<CR>", move |_| {
    // opens thread input float for reply
    threads::input::open(thread_id);
});

api::set_keymap(buf, "n", "q", move |_| {
    threads::window::close();
});
```

## Control Flow: Key Operations

### :Arbiter main

```
user runs :Arbiter main
  → review.open("main")
    → vim.cmd("tabnew")
    → create file panel buffer + window
    → create diff panel buffer + window
    → git.diff_names("main", cwd, function(files)
        → git.untracked(cwd, function(untracked)
            → merge files + untracked into review.files
            → state.load_review(...) → apply persisted statuses
            → state.load_threads(...) → restore threads
            → file_panel.render(buf, review.files)
            → review.select_file(review, first_unreviewed_file)
              → git.diff("main", file, cwd, function(diff_text)
                  → diff.render(buf, diff_text, threads_for_file, file)
                  → store hunks in review
                  → apply_highlights()
                end)
            → poll.start(review)
          end)
      end)
```

### `<Leader>ac` (add comment, immediate)

```
user presses <Leader>ac on diff line 18
  → keymap handler fires
  → diff.buf_line_to_source(hunks, 18) → { file = "handler.rs", line = 22 }
  → comment_input.open("handler.rs", 22, callback)
    → user types comment, presses <CR>
    → callback("handle empty email")
      → threads.create(review, "handler.rs", 22, "handle empty email")
      → thread_window.open(thread)  -- opens immediately with user message
      → backend.send_comment(thread, on_stream, on_complete)
        → on_stream: append text to thread window in real-time
        → on_complete: add agent message, persist, re-render
      → state.save_threads(...)
```

> **Future consideration: Batch mode.** An earlier design included a
> separate `gc` keymap that saved comments locally with
> `pending = true`, and `:AgentSubmitReview` to send all pending
> comments at once. This would support a "read everything first, then
> submit all feedback" workflow. Could be revisited for large reviews.

### Poll tick (file changed)

```
timer fires (every 2s)
  → vim.loop.fs_stat(current_file_path, function(stat)
      if stat.mtime > review.file_index[path].mtime:
        → review.file_index[path].mtime = stat.mtime
        → git.diff(ref, path, cwd, function(new_diff)
            → old_hunks = review.current_hunks
            → new_hunks = diff.parse_hunks(new_diff)
            → if file was approved and hunks changed:
                → review.file_index[path].review_status = "unreviewed"
            → check auto-resolve threads:
                for each thread with auto_resolve and status "open":
                  if file matches and content changed:
                    threads.resolve(thread)
            → threads.reanchor_by_content(review, path, new_file_contents)
              → unmatched threads → threads.bin(thread)
            → save cursor position
            → diff.render(buf, new_diff, threads, path)
            → restore cursor position
            → file_panel.render(...)
            → state.save_review(...)
            → state.save_threads(...)
          end)
    end)
```

## Command Registration

Commands are registered in `setup()` via `nvim_oxi::api::create_user_command`,
split into global (always available) and gated (require active review).

```rust
fn register_commands() {
    // Global commands (no review required)

    api::create_user_command("Arbiter", |args| {
        let ref_name = if args.args.is_empty() { None } else { Some(args.args.as_str()) };
        review::open(ref_name, &CONFIG.get().unwrap());
    }, &CommandOpts { nargs: Some("?".into()), ..Default::default() });

    api::create_user_command("ArbiterSend", |args| {
        let prompt = args.args.clone();
        backend::send_prompt(&prompt, None, Box::new(|result| {
            route_response(&result.text);
        }));
    }, &CommandOpts { nargs: Some("+".into()), ..Default::default() });

    api::create_user_command("ArbiterContinue", |args| {
        let prompt = args.args.clone();
        backend::continue_prompt(&prompt, None, Box::new(|result| {
            route_response(&result.text);
        }));
    }, &CommandOpts { nargs: Some("+".into()), ..Default::default() });

    api::create_user_command("ArbiterCatchUp", |_| {
        review::with_active(|r| {
            let sid = r.review_session_id.clone();
            let prompt = r.config.prompts.catch_up.clone();
            backend::catch_up(sid.as_deref(), &prompt, Box::new(|result| {
                route_response(&result.text);
            }));
        }).unwrap_or_else(|| {
            // No review active; still useful for context recall
            let config = CONFIG.get().unwrap();
            backend::catch_up(None, &config.prompts.catch_up, Box::new(|result| {
                route_response(&result.text);
            }));
        });
    }, &CommandOpts::default());

    // Gated commands (require active review)

    // Future consideration: AgentSubmitReview was planned for batch
    // comment submission but removed in favor of immediate submission.

    api::create_user_command("ArbiterResolveAll", |_| {
        with_review_cmd(|r| { threads::resolve_all(&mut r.threads); });
    }, &CommandOpts::default());

    api::create_user_command("ArbiterRefresh", |_| {
        with_review_cmd(|r| {
            review::refresh_file(r);
            review::refresh_file_list(r);
        });
    }, &CommandOpts::default());

    api::create_user_command("AgentTurn", |_| {
        with_review_cmd(|_| { turn::enter_agent(); });
    }, &CommandOpts::default());

    api::create_user_command("HumanTurn", |_| {
        with_review_cmd(|_| { turn::enter_human(); });
    }, &CommandOpts::default());
}
```

## Detailed Feature Design

The sections below describe the logic for individual features.
Control flow walkthroughs use pseudo-code with `→` arrows and are
language-independent. Some sections contain code snippets showing
Neovim API call patterns; the Rust implementation follows the same
logic using `nvim_oxi::api` equivalents (e.g. `api::open_win` for
`nvim_open_win`, `buf.set_lines()` for `nvim_buf_set_lines`, etc.).

## Response Panel

Free-form prompt responses (`:ArbiterSend`, `:ArbiterContinue`,
`:ArbiterCatchUp`) are displayed in a horizontal scratch buffer at the
bottom of the workbench.

```rust
/// Opens or reuses the response panel. Returns the buffer handle.
pub fn show_response(text: &str);

/// Closes the response panel.
pub fn close_response();
```

**Creation flow:**

```rust
pub fn show_response(text: &str) {
    with_active(|review| {
        let buf = review.response_buf.get_or_insert_with(|| {
            let buf = api::create_buf(false, true);
            buf.set_option("filetype", "markdown");
            buf.set_option("buftype", "nofile");
            buf
        });

        if review.response_win.map_or(true, |w| !w.is_valid()) {
            api::set_current_win(review.diff_panel.win);
            api::command("botright split");
            let win = api::get_current_win();
            win.set_height(12);
            win.set_buf(buf);
            api::set_keymap(buf, "n", "q", |_| close_response());
            review.response_win = Some(win);
        }

        let lines: Vec<&str> = text.lines().collect();
        buf.set_lines(0, -1, false, &lines);
        api::set_current_win(review.diff_panel.win);
    });
}
```

The response buffer is reused across calls. `q` in the response buffer
closes the split without closing the workbench. The response panel is
also closed when the workbench closes.

For streaming responses, the adapter calls a streaming callback that
appends lines incrementally:

```rust
pub fn stream_to_response(chunk: &str) {
    with_active(|review| {
        if let Some(ref buf) = review.response_buf {
            let lines: Vec<&str> = chunk.lines().collect();
            buf.set_lines(-1, -1, false, &lines);
        }
    });
}
```

## Cross-File Thread Navigation

`]t` / `[t` navigate between threads globally, across all files.

### Global Thread Order

Threads are sorted by (file_index, line) where file_index is the
file's position in `review.files` (the same order shown in the file
panel).

```rust
/// Returns all open threads sorted globally by file order then line.
pub fn sorted_global(files: &[FileEntry], threads: &[Thread]) -> Vec<&Thread>;

/// Returns the next thread relative to the given one. Wraps at ends.
pub fn next_thread(files: &[FileEntry], threads: &[Thread], current_id: &str) -> Option<&Thread>;

/// Returns the previous thread relative to the given one. Wraps at ends.
pub fn prev_thread(files: &[FileEntry], threads: &[Thread], current_id: &str) -> Option<&Thread>;
```

The file-index ordering is derived from the review's `files` list,
which is built from `git diff --name-status` output order (the same
order git uses in diffs).

### Cross-File Jump Logic

```
user presses ]t
  → determine current thread (thread whose summary line the cursor
    is on, or the first thread in the current file after cursor)
  → next = threads.next_thread(review, current)
  → if next.file ~= review.current_file:
      review.select_file(review, next.file)
  → move cursor to the thread's summary line in the diff buffer
```

The summary line position is known because `diff.render()` returns
the buffer line ranges for each thread summary. The thread-to-buffer-
line mapping is stored in `review.thread_buf_lines`:

```rust
/// Maps thread IDs to their summary line's buffer position.
/// Populated during diff::render(), invalidated on re-render.
thread_buf_lines: HashMap<String, usize>,
```

This is populated during `diff.render()` and invalidated on re-render.

## `<CR>` Context Detection in Diff Panel

`<CR>` in the diff panel has two behaviors depending on cursor
position:

```rust
api::set_keymap(diff_buf, "n", "<CR>", move |_| {
    let line = api::get_current_win().get_cursor().0;

    if let Some(thread_id) = review.thread_at_buf_line(line) {
        threads::window::open(&thread_id);
        return;
    }

    if let Some(source) = diff::buf_line_to_source(&review.current_hunks, line) {
        api::command(&format!("tabnew {}", source.file));
        api::get_current_win().set_cursor(source.line, 0);
    }
});
```

`review.thread_at_buf_line()` checks the `thread_buf_lines` mapping.
If the cursor is on a thread summary line, the thread window opens.
Otherwise, the source file opens in a new tab at the corresponding
line.

## `gC` Immediate Comment Control Flow

```
user presses gC on diff line 18
  → keymap handler fires
  → diff.buf_line_to_source(hunks, 18) → { file = "handler.rs", line = 22 }
  → comment_input.open("handler.rs", 22, callback)
    → user types comment, presses <CR>
    → callback("handle empty email")
      → thread = threads.create(review, "handler.rs", 22, "handle empty email",
          { pending = false, immediate = true })
      → backend.send("Send", {
          prompt = format_comment_prompt(thread),
          stream = true,
        }, function(result)
          → thread.session_id = result.session_id
          → threads.add_message(thread, "agent", result.text)
          → state.save_threads(...)
          → review.refresh_file(review)
        end)
      → review.refresh_file(review)
```

All comments are sent immediately on submission. The thread window
opens with the user's message, and the agent's streaming response is
appended in real time via the streaming callback.

## `gA` Auto-Resolve with Timeout

```
user presses gA on diff line 18
  → same as gC, but:
    → thread = threads.create(review, ..., { auto_resolve = true })
    → thread.auto_resolve_at = os.time()
    → backend.send(...)
    → review.refresh_file(review)
```

**Timeout checking** happens in the poll tick. On every file poll tick
(every 2s), in addition to checking mtime:

```
for each thread with auto_resolve == true and status == "open":
  if os.time() - thread.auto_resolve_at > config.auto_resolve_timeout:
    thread.auto_resolve = false
    thread.auto_resolve_at = nil
    state.save_threads(...)
    vim.notify("Auto-resolve timed out: " .. thread.file .. ":" .. thread.line)
```

The thread reverts to a normal open thread. The user is notified.

Auto-resolve success is also checked in the poll tick (already
documented): when the file changes and the thread's anchor content
is modified, the thread is resolved.

**Limitation:** Auto-resolve only triggers when the specific anchor
line is modified. If the agent addresses the feedback by changing
a different part of the file (e.g., a rename on a different line),
auto-resolve will not trigger and the timeout will expire. In this
case the thread reverts to a normal open thread for manual review.
This is the intended behavior: auto-resolve is a convenience for
changes that directly affect the commented line, not a general
"did the agent fix it" detector.

## Streaming Responses in Thread Windows

When a thread reply or immediate comment is sent with
`opts.stream = true`, the adapter produces incremental output. The
wiring from adapter through queue to thread window:

**QueueItem gains a streaming callback:**

```rust
struct QueueItem {
    opts: BackendOpts,
    on_stream: Option<OnStream>,
    callback: OnComplete,
}
```

**Queue passes the streaming callback to the adapter:**

```rust
adapter.execute(item.opts, item.on_stream, move |result| {
    let _guard = DrainGuard;
    (item.callback)(result);
});
```

**Adapter calls on_stream for each assistant event:**

```rust
// In the stream-json stdout processing thread:
for line in BufReader::new(stdout).lines().flatten() {
    if let Ok(event) = serde_json::from_str::<StreamEvent>(&line) {
        if event.event_type == "assistant" {
            if let Some(text) = event.text() {
                if let Some(ref on_stream) = on_stream {
                    let cb = Arc::clone(on_stream);
                    let chunk = text.to_string();
                    nvim_oxi::schedule(move || cb(&chunk));
                }
            }
        }
    }
    output.push_str(&line);
}
```

**Thread window receives streaming chunks:**

```rust
// In the gC / thread reply keymap handler:
let on_stream: OnStream = Arc::new(|chunk| {
    threads::window::append_streaming(chunk);
});

backend::send(
    BackendOpts {
        op: BackendOp::Resume(session_id),
        prompt,
        stream: true,
        ..Default::default()
    },
    Some(on_stream),
    Box::new(move |result| {
        threads::add_message(&thread_id, Role::Agent, &result.text);
    }),
);
```

`threads::window::append_streaming(chunk)` appends text to the last
agent message line in the thread window buffer without creating a new
message entry. When the final result arrives, the full text replaces
the streamed fragments.

## Review Summary Float (`gs`)

```rust
pub fn show_summary();
```

**Implementation:**

```rust
pub fn show_summary() {
    with_active(|review| {
        let (mut approved, mut needs_changes, mut unreviewed) = (0, 0, 0);
        for f in &review.files {
            match f.review_status {
                ReviewStatus::Approved => approved += 1,
                ReviewStatus::NeedsChanges => needs_changes += 1,
                ReviewStatus::Unreviewed => unreviewed += 1,
            }
        }

        let open = threads::filter(&review.threads, Some(ThreadStatus::Open)).len();
        let resolved = threads::filter(&review.threads, Some(ThreadStatus::Resolved)).len();
        let binned = threads::filter(&review.threads, Some(ThreadStatus::Binned)).len();

        let lines = vec![
            "Review Progress".into(),
            String::new(),
            format!("V {approved} approved"),
            format!("X {needs_changes} needs changes"),
            format!("- {unreviewed} unreviewed"),
            String::new(),
            format!("Threads: {open} open, {resolved} resolved, {binned} binned"),
        ];

        let buf = api::create_buf(false, true);
        buf.set_lines(0, -1, false, &lines);

        let width = 36;
        let height = lines.len() as u32;
        let editor_height = api::get_option::<u32>("lines");
        let editor_width = api::get_option::<u32>("columns");

        api::open_win(&buf, true, &WindowConfig {
            relative: "editor",
            width,
            height,
            row: (editor_height - height) / 2,
            col: (editor_width - width) / 2,
            border: "rounded",
            style: "minimal",
        });

        api::set_keymap(&buf, "n", "q", |_| api::command("close"));
        api::set_keymap(&buf, "n", "<Esc>", |_| api::command("close"));
    });
}
```

A simple centered float that closes on `q` or `<Esc>`. No persistence,
no interaction beyond reading.

## Thread List View (`gT` and Filtering)

`gT` opens a floating window listing all threads across all files.
`gTa`, `gTu`, `gTb` open the same view pre-filtered.

```rust
pub fn show_list(filter: Option<ThreadFilter>);

pub struct ThreadFilter {
    pub origin: Option<ThreadOrigin>,
    pub status: Option<ThreadStatus>,
}
```

**Implementation:**

The thread list is a scratch buffer in a floating window. Each line
shows one thread:

```
[you]   handler.rs:22    handle empty email         [open]
[agent] handler.rs:35    Should this return 401?     [open]
[you]   user.rs:8        field should be Option      [resolved]
```

Buffer-local keymaps:

| Key    | Action                                       |
|--------|----------------------------------------------|
| `<CR>` | Open the selected thread's floating window   |
| `gR`   | Resolve the selected thread                  |
| `dd`   | Dismiss (delete) a binned thread             |
| `gP`   | Request agent re-anchoring for a binned thread |
| `q`    | Close the thread list                        |

Each line maps to a thread via a buffer-local table (same pattern as
the file panel's path mapping).

**Filter dispatch from keymaps:**

```rust
api::set_keymap(buf, "n", &km.list_threads, move |_| {
    let c = api::call_function::<String>("getcharstr", &[]);
    let filter = match c.as_str() {
        "a" => Some(ThreadFilter { origin: Some(ThreadOrigin::Agent), status: None }),
        "u" => Some(ThreadFilter { origin: Some(ThreadOrigin::User), status: None }),
        "b" => Some(ThreadFilter { origin: None, status: Some(ThreadStatus::Binned) }),
        _   => None,
    };
    threads::show_list(filter);
});
```

`gT` alone (with no follow-up within `timeoutlen`) falls through to
the unfiltered list. `gTa`, `gTu`, `gTb` are handled by reading the
next character.

## `g?` Toggle Resolved Thread Visibility

```rust
api::set_keymap(buf, "n", &km.toggle_resolved, move |_| {
    with_active(|review| {
        review.show_resolved = !review.show_resolved;
        review::refresh_file();
    });
});
```

The `show_resolved` flag is checked in `diff::render()`:

```rust
pub fn render(buf: &Buffer, diff_text: &str, threads: &[Thread],
              file_path: &str, show_resolved: bool) {
    let visible: Vec<_> = threads.iter()
        .filter(|t| t.status != ThreadStatus::Resolved || show_resolved)
        .collect();
    // render thread summary lines from visible threads only
}
```

## `zo`/`zc` Expand/Collapse and Fold Support

In the unified diff view, sections are foldable using Neovim's built-in
fold mechanism. The diff buffer uses manual folds:

```rust
diff_win.set_option("foldmethod", "manual");
diff_win.set_option("foldenable", true);
```

After rendering, the plugin creates folds for each hunk:

```rust
for hunk in &hunks {
    api::command(&format!("{},{} fold", hunk.buf_start, hunk.buf_end));
    api::command(&format!("{} foldopen", hunk.buf_start));
}
```

`zo`/`zc` are Neovim built-in fold commands and work without any
custom keymaps. The plugin just needs to create the fold ranges.

**`fold_approved` config option:**

When `config.fold_approved = true`, approved hunks remain folded
after the initial render. The fold text shows a summary:

```rust
diff_win.set_option("foldtext", "v:lua.arbiter_foldtext()");

// Registered via exec_lua once at plugin load:
api::exec_lua(r#"
  function _G.arbiter_foldtext()
    local line = vim.fn.getline(vim.v.foldstart)
    local count = vim.v.foldend - vim.v.foldstart + 1
    return line .. " ... (" .. count .. " lines, approved)"
  end
"#, ());
```

On review status change (`ga`), if `fold_approved` is enabled, the
hunk containing the cursor is folded. On `gr` (reset), the fold is
opened.

## Hunk Change Detection on Refresh

When a file's diff is re-rendered after a poll tick detects a change,
the plugin compares old and new hunk content hashes to classify each
hunk:

```rust
use std::collections::HashSet;

pub fn detect_hunk_changes(
    old_hashes: &HashSet<String>,
    new_hunks: &[Hunk],
) -> HashSet<usize> {
    new_hunks.iter()
        .filter(|h| !old_hashes.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect()
}
```

After `diff::render()`, new/modified hunks are highlighted with
`ArbiterHunkNew` using extmarks in the sign column:

```rust
for &buf_line in &changed_hunks {
    buf.set_extmark(ns_id, buf_line - 1, 0, &ExtmarkOpts {
        sign_text: Some("!"),
        sign_hl_group: Some("ArbiterHunkNew"),
        ..Default::default()
    });
}
```

The `prev_hunk_hashes` field on `FileEntry` stores the content hash
set from the previous render. It's updated after each render.

## Side-by-Side Diff Toggle

When the user presses `<Leader>s`, the diff panel switches between
unified and side-by-side mode.

```rust
/// Opens side-by-side view for the given file.
pub fn open_side_by_side(file_path: &str, git_ref: &str);

/// Closes side-by-side view and returns to unified.
pub fn close_side_by_side();
```

**Opening side-by-side:**

```rust
pub fn open_side_by_side(file_path: &str, git_ref: &str) {
    with_active(|review| {
        review.side_by_side = true;
        let path = file_path.to_string();
        let r = git_ref.to_string();
        let cwd = review.cwd.clone();

        git::show(&r, &path, &cwd, move |old_content| {
            nvim_oxi::schedule(move || {
                with_active(|review| {
                    let new_lines: Vec<String> = std::fs::read_to_string(&path)
                        .unwrap_or_default()
                        .lines().map(String::from).collect();

                    let old_buf = api::create_buf(false, true);
                    let old_lines: Vec<&str> = old_content.lines().collect();
                    old_buf.set_lines(0, -1, false, &old_lines);
                    old_buf.set_option("modifiable", false);
                    old_buf.set_option("buftype", "nofile");

                    let new_buf = api::create_buf(false, true);
                    new_buf.set_lines(0, -1, false, &new_lines);
                    new_buf.set_option("modifiable", false);
                    new_buf.set_option("buftype", "nofile");

                    api::set_current_win(review.diff_panel.win);
                    review.diff_panel.win.set_buf(&old_buf);
                    api::command("diffthis");

                    api::command("vsplit");
                    let new_win = api::get_current_win();
                    new_win.set_buf(&new_buf);
                    api::command("diffthis");

                    let file_threads = threads::for_file(&review.threads, &path);
                    for t in &file_threads {
                        if t.status == ThreadStatus::Open || review.show_resolved {
                            let prefix = match t.origin {
                                ThreadOrigin::Agent => "[agent]",
                                ThreadOrigin::User => "[you]",
                            };
                            let preview = &t.messages[0].text[..40.min(t.messages[0].text.len())];
                            let _ = new_buf.set_extmark(ns_id, t.line - 1, 0, &ExtmarkOpts {
                                virt_text: Some(vec![(format!("{prefix} {preview}"), "ArbiterThread")]),
                                virt_text_pos: Some("eol"),
                                ..Default::default()
                            });
                        }
                    }

                    review.sbs = Some(SideBySide { old_buf, new_buf, new_win });
                    set_diff_keymaps(&old_buf);
                    set_diff_keymaps(&new_buf);
                });
            });
        });
    });
}
```

**Closing side-by-side:**

```rust
pub fn close_side_by_side() {
    with_active(|review| {
        if let Some(sbs) = review.sbs.take() {
            let _ = sbs.new_win.close(true);
            let _ = sbs.old_buf.delete(true);
            let _ = sbs.new_buf.delete(true);
        }
        review.side_by_side = false;
        if let Some(ref file) = review.current_file {
            review::select_file(file);
        }
    });
}
```

Thread summaries in side-by-side mode use `nvim_buf_set_extmark` with
`virt_text` at end-of-line on the anchor line, rather than buffer
content lines (since the buffer contains the actual file, not a diff).

## SelfReview Parsing for Cursor

Claude Code uses `--json-schema` for structured thread extraction.
Cursor does not support this flag. Instead, the Cursor adapter uses a
structured prompt and parses the text response:

**Prompt template:**

```rust
const CURSOR_SELF_REVIEW_PROMPT: &str = r#"
Review this diff and flag anything you're uncertain about.

For EACH concern, respond in EXACTLY this format on its own line:
THREAD|<file_path>|<line_number>|<your question or concern>

Only output THREAD lines. No other text.

Diff:
{diff}
"#;
```

**Response parsing:**

```rust
struct ParsedThread {
    file: String,
    line: usize,
    message: String,
}

fn parse_self_review_text(text: &str) -> Vec<ParsedThread> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Strip optional markdown code fences, bullet prefixes
            let trimmed = trimmed.trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim_start_matches('`')
                .trim_end_matches('`')
                .trim();
            // Accept "THREAD|...", "THREAD: |...", "THREAD |..."
            let rest = trimmed.strip_prefix("THREAD")
                .map(|r| r.trim_start_matches(':').trim_start_matches(' '))
                .and_then(|r| r.strip_prefix('|'))?;
            let parts: Vec<&str> = rest.splitn(3, '|').collect();
            if parts.len() == 3 {
                let line_num = parts[1].trim().parse::<usize>().ok()?;
                Some(ParsedThread {
                    file: parts[0].trim().to_string(),
                    line: line_num,
                    message: parts[2].trim().to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}
```

The parser is lenient: it strips markdown formatting, bullet
prefixes, and optional colons/spaces around the THREAD prefix.
Lines that don't match are silently discarded. If zero threads are
parsed, the plugin shows a notification: "Self-review produced no
threads." The user can retry or inspect the raw response in the
response panel.

## Turn Cycling Workbench State

When the user enters human turn (`<Leader>a` or `:HumanTurn`), the
workbench remains open but is not interactive. The diff panel shows a
banner:

```rust
pub fn enter_human() {
    with_active(|review| {
        review.turn = Turn::Human;
        let cwd = review.cwd.clone();
        git::stash_create(&cwd, move |hash| {
            nvim_oxi::schedule(move || {
                with_active(|review| {
                    review.snapshot_hash = Some(hash.unwrap_or_else(|| "HEAD".into()));
                });
            });
        });
        poll::stop();
    });
}
```

When the user returns to agent turn:

```rust
pub fn enter_agent() {
    with_active(|review| {
        if review.turn == Turn::Human {
            if let Some(hash) = review.snapshot_hash.take() {
                let cwd = review.cwd.clone();
                git::diff_hash(&hash, &cwd, move |diff| {
                    nvim_oxi::schedule(move || {
                        if !diff.is_empty() {
                            backend::handback(None, &diff, Box::new(|_| {
                                review::refresh_file();
                                review::refresh_file_list();
                            }));
                        } else {
                            review::refresh_file();
                            review::refresh_file_list();
                        }
                    });
                });
            }
        }
        review.turn = Turn::Agent;
        poll::start();
    });
}
```

The workbench is refreshed (file list + current file diff) after the
handback completes. If the workbench was closed during human turn
(user ran `q`), `:AgentTurn` or `<Leader>a` reopens it via
`review.open()`.

## `vim.diff()` vs `git diff`

The PRD lists `vim.diff()` as a primitive. The RFD uses `git diff`
(external process) for primary diff computation. This is intentional:

- `git diff` respects `.gitattributes`, rename detection, and all git
  configuration the user has set up.
- `git diff` handles binary files, submodules, and edge cases that
  `vim.diff()` (raw xdiff) does not.
- `git diff` produces output that matches what the user sees in
  `git diff` at the command line, avoiding confusion.

`vim.diff()` is used for one purpose: content-based thread
re-anchoring. When searching for anchor content in a changed file,
`vim.diff()` can compute a minimal diff between the old and new
content to determine line offset mappings. This is faster than shelling
out to git for a purely in-memory operation.

## Extmark Usage

`nvim_buf_set_extmark()` is used in three places:

1. **Hunk change indicators** (sign column): After a poll-triggered
   re-render, new/modified hunks get a sign via extmark (see Hunk
   Change Detection above).

2. **Side-by-side thread indicators** (virtual text): In side-by-side
   mode, thread summaries are shown as end-of-line virtual text on the
   anchor line (see Side-by-Side section above).

3. **Thread summary line tracking**: Each thread summary line rendered
   in the unified diff buffer gets an extmark for stable identification.
   When the buffer is scrolled or the user navigates, the extmark
   position is authoritative (survives minor buffer edits better than
   raw line numbers).

All extmarks use a single namespace:

```rust
let ns_id = api::create_namespace("arbiter");
```

The namespace is cleared and re-created on each `diff.render()` call.

## Work Streams

The plugin decomposes into four work streams that can proceed in
parallel. Each stream has a defined boundary contract. The streams
share only data structures (defined in Core Data Structures above)
and communicate through function signatures. No stream imports from
another at development time; integration happens in the wiring layer.

### Dependency Graph

```
┌──────────────┐
│   Wiring     │  lib.rs, config.rs, plugin/arbiter.lua
│  (Stream 4)  │
└──┬───┬───┬───┘
   │   │   │
   ▼   ▼   ▼
┌──────┐ ┌──────────┐ ┌─────────┐
│  UI  │ │  Thread  │ │ Backend │
│ Shell│ │  Data    │ │  Shim   │
│ (S4) │ │  (S2)    │ │  (S3)   │
└──┬───┘ └──────────┘ └─────────┘
   │
   ▼
┌──────────┐
│ Git+Diff │
│  Engine  │
│  (S1)    │
└──────────┘
```

Key observations:

- **Stream 3 (Backend Shim)** has zero inward dependencies. It can
  be built, tested, and shipped without any other stream existing.
- **Stream 1 (Git+Diff)** has zero inward dependencies. It talks to
  the `git` binary and Neovim buffer APIs only.
- **Stream 2 (Thread Data)** has zero inward dependencies. Thread
  CRUD, re-anchoring, persistence, and filtering are pure data logic.
- **Stream 4 (UI Shell + Wiring)** consumes the output of streams
  1-3 via the boundary contracts defined below. It cannot be completed
  until the contracts are stable, but can be scaffolded in parallel
  (window layout, keymap structure, panel rendering) against stubs.

### Stream 1: Git Interface + Diff Engine

**Modules:** `git.rs`, `diff/`

**Scope:**

- Async git command execution (background thread + `nvim_oxi::schedule`)
- Convenience wrappers for all git operations the plugin needs
- Unified diff parsing (raw text to `Vec<Hunk>`)
- Diff buffer rendering (hunk lines + file header + highlight ranges)
- Buffer-line-to-source-line mapping
- Untracked file diff synthesis
- Side-by-side buffer pair creation
- Hunk content hashing and change detection between renders
- Fold range creation after render

**Does not know about:** Threads, backend/CLI, review state, file
panel, polling, turns, or sessions. The `diff` module does not
`use crate::threads`. This is enforced by the crate's module tree.

**Testing:** `diff::parse_hunks()` and `diff::detect_hunk_changes()`
are pure functions testable with `#[cfg(test)]` unit tests. `git::run`
needs `git` on PATH. `diff::render()` needs a Neovim buffer (integration
test in headless Neovim).

**Boundary contract:**

```rust
// git.rs - public API

pub struct GitResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(args: &[&str], cwd: &str, callback: impl FnOnce(GitResult) + Send + 'static);
pub fn diff(ref_name: &str, file: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn diff_names(ref_name: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn untracked(cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn show(ref_name: &str, file: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn stash_create(cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn diff_hash(hash: &str, cwd: &str, cb: impl FnOnce(GitResult) + Send + 'static);
pub fn file_mtime(path: &str) -> Option<i64>;


// diff/mod.rs - public API

pub struct Hunk { /* see Core Data Structures */ }
pub struct SourceLocation { pub file: String, pub line: usize }
pub struct SideBySide { pub old_buf: Buffer, pub new_buf: Buffer, pub new_win: Window }

pub fn parse_hunks(diff_text: &str) -> Vec<Hunk>;

pub fn render(
    buf: &Buffer,
    diff_text: &str,
    summaries: &[ThreadSummary],
    file_path: &str,
    show_resolved: bool,
) -> (Vec<Hunk>, HashMap<String, usize>);

pub fn buf_line_to_source(hunks: &[Hunk], buf_line: usize) -> Option<SourceLocation>;

pub fn detect_hunk_changes(old_hashes: &HashSet<String>, new_hunks: &[Hunk]) -> HashSet<usize>;

pub fn synthesize_untracked(contents: &str, path: &str) -> String;

pub fn open_side_by_side(
    win: &Window,
    old_content: &str,
    new_content_lines: &[String],
    filetype: &str,
) -> SideBySide;

pub fn close_side_by_side(sbs: SideBySide);
```

**Data flowing across the boundary:**

| From S1 | To | Type | Purpose |
|---|---|---|---|
| `Vec<Hunk>` | S4 (UI) | Struct | Navigation, fold creation, change detection |
| `HashMap<String, usize>` | S4 (UI) | Map | Thread summary line positions in buffer |
| `HashSet<usize>` | S4 (UI) | Set | Buf lines with new/changed hunks |
| `SourceLocation` | S4 (UI) | Struct | Source location from buffer cursor |
| `SideBySide` | S4 (UI) | Struct | Handle for teardown |

**Critical design note:** `diff::render()` accepts `&[ThreadSummary]`,
not `&[Thread]`. The diff module imports `ThreadSummary` from
`types.rs` (shared types), not from the `threads` module. The UI
layer calls `threads::to_summaries()` and passes the result to
`diff::render()`. The diff engine never sees thread sessions,
messages, re-anchoring, or the backend.

### Stream 2: Thread Data Layer

**Modules:** `threads/mod.rs`, `state.rs`

**Scope:**

- Thread CRUD (create, add_message, resolve, bin, dismiss)
- Content-based re-anchoring (`str::find` + context verification)
- Global thread ordering (by file index + line)
- Thread navigation helpers (next/prev with wrapping)
- Thread filtering (by origin, status)
- Thread list view rendering (scratch buffer + keymaps)
- Review state and thread JSON persistence
- Workspace hashing (`sha2` crate) and content hashing
- Auto-resolve timeout checking (given current time)

**Does not know about:** Git commands, diff rendering, buffer layout,
backend/CLI, polling, turns, or Neovim window management (except for
the thread list float, which is self-contained within this stream).

**Does not `use`:** `crate::git`, `crate::diff`, `crate::backend`,
`crate::review`, `crate::poll`, `crate::turn`.

**Testing:** Almost entirely `#[test]` unit tests.
`threads::reanchor_by_content()` is pure string matching.
`state::load_*` / `state::save_*` is file I/O with serde_json.

**Boundary contract:**

```rust
// threads/mod.rs - public API

pub fn create(file: &str, line: u32, text: &str, opts: CreateOpts) -> Thread;
pub fn add_message(thread: &mut Thread, role: Role, text: &str);
pub fn resolve(thread: &mut Thread);
pub fn bin(thread: &mut Thread);
pub fn dismiss(threads: &mut Vec<Thread>, index: usize);
pub fn resolve_all(threads: &mut [Thread]);

pub fn reanchor_by_content(threads: &mut [Thread], file: &str, new_contents: &str) -> Vec<usize>;
pub fn for_file(threads: &[Thread], file: &str) -> Vec<&Thread>;
pub fn pending_indices(threads: &[Thread]) -> Vec<usize>;
pub fn filter(threads: &[Thread], opts: &FilterOpts) -> Vec<&Thread>;

pub fn sorted_global(threads: &[Thread], file_order: &[String]) -> Vec<usize>;
pub fn next_thread(sorted: &[usize], current: Option<usize>) -> Option<usize>;
pub fn prev_thread(sorted: &[usize], current: Option<usize>) -> Option<usize>;

pub fn to_summaries(threads: &[Thread]) -> Vec<ThreadSummary>;
pub fn check_auto_resolve_timeouts(threads: &mut [Thread], timeout_secs: u64, now: i64) -> Vec<usize>;


// state.rs - public API

pub fn load_review(state_dir: &Path, ws_hash: &str, ref_name: &str) -> ReviewState;
pub fn save_review(state_dir: &Path, ws_hash: &str, ref_name: &str, state: &ReviewState);
pub fn load_threads(state_dir: &Path, ws_hash: &str, ref_name: &str) -> Vec<Thread>;
pub fn save_threads(state_dir: &Path, ws_hash: &str, ref_name: &str, threads: &[Thread]);
pub fn workspace_hash(path: &Path) -> String;
pub fn content_hash(text: &str) -> String;
```

**Data flowing across the boundary:**

| From S2 | To | Type | Purpose |
|---|---|---|---|
| `Vec<Thread>` | S4 (UI) | Struct list | Display in thread windows, summary lines |
| `Vec<ThreadSummary>` | S1 (Diff) | Struct list | Diff engine renders summaries without importing S2 |
| `ReviewState` | S4 (UI) | Struct | File review status restoration |
| `Vec<usize>` (unmatched) | S4 (UI) | Index list | Bin notification after re-anchor |
| `Vec<usize>` (timed out) | S4 (UI) | Index list | Notification after auto-resolve timeout |

**Critical design note:** All thread functions take `&[Thread]` or
`&mut [Thread]` / `&mut Vec<Thread>` as arguments, not `&Review`.
The thread module never imports `crate::review`. The UI layer passes
`&mut review.threads` into thread functions. Similarly,
`sorted_global` takes `&[String]` (file paths) as an argument rather
than reading `review.files`.

### Stream 3: Backend Shim

**Modules:** `backend/mod.rs`, `backend/queue.rs`,
`backend/cursor.rs`, `backend/claude.rs`

**Scope:**

- FIFO call queue with sequential execution
- `Adapter` trait + dispatch (select cursor or claude based on config)
- Cursor CLI flag assembly + process spawn + JSON parsing
- Claude CLI flag assembly + process spawn + JSON parsing
- Streaming support (line-buffered stdout parsing, streaming callback)
- Session ID capture from responses
- Self-review response parsing (structured prompt for Cursor,
  JSON schema for Claude)

**Does not `use`:** `crate::threads`, `crate::diff`, `crate::review`,
`crate::git`, `crate::poll`, `crate::turn`, or any `nvim_oxi::api`
buffer/window APIs. The only Neovim integration is
`nvim_oxi::schedule()` for callback dispatch.

**Testing:** Fully testable in isolation. Replace the CLI binary with
a shell script returning canned JSON. Test queue ordering with a mock
`Adapter` impl. No Neovim UI needed. Most tests are pure `#[test]`.

**Boundary contract:**

```rust
// backend/mod.rs - public API

pub trait Adapter: Send + Sync {
    fn execute(&self, opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete);
    fn binary_name(&self) -> &str;
}

pub fn setup(config: &Config);
pub fn send(opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete);

pub fn send_comment(prompt: &str, callback: OnComplete);
pub fn thread_reply(session_id: Option<&str>, prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);
pub fn catch_up(session_id: Option<&str>, prompt: &str, callback: OnComplete);
pub fn handback(session_id: Option<&str>, prompt: &str, callback: OnComplete);
pub fn self_review(prompt: &str, callback: OnComplete);
pub fn re_anchor(session_id: &str, prompt: &str, callback: OnComplete);
pub fn send_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);
pub fn continue_prompt(prompt: &str, on_stream: Option<OnStream>, callback: OnComplete);
pub fn is_busy() -> bool;

// Types (from types.rs, re-exported here)
pub struct BackendOpts { pub op: BackendOp, pub prompt: String, ... }
pub struct BackendResult { pub text: String, pub session_id: String, pub error: Option<String> }
pub type OnStream = Arc<dyn Fn(&str) + Send + Sync>;
pub type OnComplete = Box<dyn FnOnce(BackendResult) + Send>;
```

**Data flowing across the boundary:**

| From S3 | To | Type | Purpose |
|---|---|---|---|
| `BackendResult` | S4 (UI) | Struct | Agent response + session ID |
| Streaming chunks | S4 (UI) | `&str` via `OnStream` | Real-time text for thread window |

| Into S3 | From | Type | Purpose |
|---|---|---|---|
| `BackendOpts.prompt` | S4 (UI) | `String` | Assembled from thread messages, templates |
| `BackendOpts.session_id` | S4 (UI) | `Option<String>` | From `Thread.session_id` |
| `Config` | S4 (Wiring) | Struct | Backend selection, model |

**Critical design note:** The backend never constructs prompts. It
receives fully assembled prompt strings. Prompt templates live in the
UI/wiring layer. The backend's job: take a string, send it to a CLI,
return the response. The `Adapter` trait is the only polymorphism
point in the plugin; the rest uses concrete types.

### Stream 4: UI Shell + Wiring

**Modules:** `lib.rs`, `config.rs`, `review.rs`, `file_panel.rs`,
`threads/window.rs`, `threads/input.rs`, `highlight.rs`, `poll.rs`,
`turn.rs`

**Scope:**

- Review workbench lifecycle (open, close, tabpage management)
- File panel rendering (tree view, status icons, summary counts)
- Diff panel rendering orchestration (calls S1 with S2's summaries)
- Thread window (floating window for conversations, reply handling)
- Comment input float
- All keymap binding and dispatch
- Polling orchestration (calls S1's git functions, triggers refresh)
- Turn cycling orchestration (calls S1's stash_create, S3's handback)
- Command registration and agent mode gating
- Config deserialization and validation
- Prompt assembly (formatting templates with context data)
- Response routing (workbench panel vs standalone float)
- Inline thread indicators
- Statusline

**Depends on:** S1 (`crate::git`, `crate::diff`), S2
(`crate::threads`, `crate::state`), S3 (`crate::backend`). Uses
only their public APIs.

**Testing:** Integration tests in headless Neovim. S3 is mocked with
a struct implementing `Adapter` that returns canned `BackendResult`s.

**Scaffolding (before S1-S3 are stable):**

- Window layout and tabpage management
- Keymap binding structure (using stubs for action handlers)
- File panel tree rendering (with hardcoded test data)
- Highlight group definitions
- Config struct and `Default` impl
- Command registration skeleton
- Agent mode state tracking (`RefCell<Option<Review>>`)

### Parallel Development Strategy

```
Week 1-2       Week 3-4          Week 5-6
───────────    ──────────────    ──────────
S1: git+diff ────────────────→ done
S2: threads  ────────────────→ done
S3: backend  ────────────────→ done
S4: scaffold ──→ integrate S1 ──→ integrate S2+S3 ──→ done
```

Streams 1, 2, and 3 can start simultaneously. Stream 4 starts with
scaffolding then integrates each stream's output as it stabilizes.

The shared types in `types.rs` (`Hunk`, `Thread`, `ThreadSummary`,
`FileEntry`, `BackendOpts`, `BackendResult`, `ReviewState`, and all
enums) must be agreed upon before any stream starts. Changes to these
types require coordination across streams since the compiler will
reject mismatches.

### Contract Stability Rules

1. **Struct fields are additive.** New fields can be added to `Hunk`,
   `Thread`, etc. (with defaults via `#[serde(default)]` for
   persisted types). Existing fields cannot be renamed or removed
   without cross-stream coordination.

2. **Function signatures are stable after first use.** Once Stream 4
   calls a function from S1/S2/S3, that signature is frozen. New
   `pub fn`s can be added freely.

3. **The compiler enforces boundaries.** A module that does not `use`
   another module's types cannot accidentally depend on it. This is
   the primary advantage of Rust over Lua for this architecture:
   accidental coupling is a compile error.

4. **Callbacks are the async integration primitive.** All cross-stream
   async communication uses `impl FnOnce(...) + Send + 'static`.
   The thread layer's functions are synchronous (in-memory operations)
   and can be called directly.

## Error Handling

**Git command failures:** If `git diff` or any git command exits
non-zero, the callback receives the exit code and stderr. The plugin
displays the error via `api::notify()` at `WARN` level and does not
update the affected panel. The review remains usable for other files.

**CLI failures:** If the backend CLI exits non-zero or returns
malformed JSON, the queue item's callback receives an error result.
The thread window displays the error inline: "Error: [message]". The
queue continues processing the next item.

**Missing CLI:** On first backend call, if the CLI binary is not found
(checked via `std::process::Command` or PATH search), the plugin
displays a one-time error via `api::notify()` at `ERROR` level and
disables all agent
operations. The diff viewer continues to work without agent features.

**JSON parse failures:** `serde_json::from_str()` returns `Err` on
malformed input. On failure, the raw text response is stored as the
message text and the error is logged.

**Missing git:** Checked in `review.open()`. If `git` is not
executable or the cwd is not a git repo (`git rev-parse --git-dir`
fails), the command errors with a clear message and does not open
the workbench.

## Testing Strategy

### Test Architecture

The project is a Cargo workspace with two crates:

```
arbiter/
├── Cargo.toml              # workspace root
├── crates/
│   └── arbiter/
│       ├── Cargo.toml      # [lib] crate-type = ["cdylib"]
│       └── src/            # plugin source
└── tests/
    ├── Cargo.toml          # [lib] crate-type = ["cdylib"], nvim-oxi "test" feature
    ├── build.rs            # nvim_oxi::tests::build()
    └── src/
        ├── lib.rs          # mod declarations
        ├── fixtures.rs     # shared test data (diffs, threads, configs)
        ├── helpers.rs      # temp_git_repo, MockAdapter, buffer assertions
        ├── unit/           # pure Rust tests (no nvim-oxi::test)
        │   ├── mod.rs
        │   ├── parse.rs
        │   ├── threads.rs
        │   ├── state.rs
        │   ├── queue.rs
        │   └── backend.rs
        ├── nvim/           # #[nvim_oxi::test] tests (run inside Neovim)
        │   ├── mod.rs
        │   ├── render.rs
        │   ├── review.rs
        │   ├── file_panel.rs
        │   ├── thread_window.rs
        │   ├── highlight.rs
        │   ├── keymaps.rs
        │   └── commands.rs
        └── e2e/            # full workflow tests (also #[nvim_oxi::test])
            ├── mod.rs
            ├── review_workflow.rs
            ├── thread_workflow.rs
            ├── poll_workflow.rs
            └── turn_workflow.rs
```

The split into two crates is required by `nvim-oxi`. The `tests/`
crate is a `cdylib` with `nvim-oxi`'s `test` feature enabled. Its
`build.rs` calls `nvim_oxi::tests::build()`. When `cargo test` runs,
`nvim-oxi` spawns a headless Neovim process and executes each
`#[nvim_oxi::test]` function inside it.

**Test name uniqueness:** `nvim-oxi` requires globally unique test
names across the entire test crate. Use prefixed names:
`unit_parse_*`, `unit_thread_*`, `nvim_render_*`, `e2e_review_*`.

### Test Layers

#### Layer 0: Compile-Time Guarantees

Rust's type system provides baseline guarantees that would require
tests in a dynamic language:

- **Stream decoupling**: `diff/` cannot `use crate::threads`. If
  someone adds the import, the code compiles but violates the
  architecture. Enforce this with a CI script that greps for
  prohibited imports (see CI section).
- **Contract enforcement**: `diff::render()` takes `&[ThreadSummary]`,
  not `&[Thread]`. Passing the wrong type is a compile error.
- **Exhaustive matching**: adding a variant to `ThreadStatus` forces
  every `match` to handle it.
- **Persistence format**: `#[derive(Serialize, Deserialize)]` on
  `Thread` and `ReviewState` means the serde contract is validated
  at compile time.

These are not tests but they eliminate entire classes of bugs. The
test suite builds on top of them for behavioral correctness.

#### Layer 1: Pure Unit Tests

Standard `#[test]` functions. No Neovim process. No `nvim_oxi::test`.
These run fast (`cargo test` completes in seconds) and form the bulk
of the test suite.

**Modules under test and target coverage:**

| Module | Functions | Test Focus |
|---|---|---|
| `diff::parse` | `parse_hunks`, `content_hash` | All diff formats: standard, single-line, no-newline-at-EOF, binary, empty, rename, multi-hunk. Header parsing edge cases. Hash determinism. |
| `diff::parse` | `buf_line_to_source` | Mapping for added, deleted, context, and header lines. Out-of-range returns `None`. Offset correctness when thread summary lines are injected above. |
| `diff::parse` | `synthesize_untracked` | Correct header, all lines prefixed with `+`, trailing newline. |
| `diff::parse` | `detect_hunk_changes` | New hunk detected, unchanged hunk ignored, all-new hunks, all-unchanged, empty inputs. |
| `threads` | `create` | UUID generated, initial message added, correct origin/status/pending, anchor fields set. |
| `threads` | `add_message` | Appends to messages, sets timestamp, preserves existing messages. |
| `threads` | `resolve`, `bin`, `dismiss`, `resolve_all` | Status transitions, dismiss removes from vec, resolve_all skips non-open. |
| `threads` | `reanchor_by_content` | Shifted anchor (lines inserted above), deleted anchor, modified anchor text, context verification (anchor found but context missing), multiple threads on same file, thread on different file untouched. |
| `threads` | `for_file`, `pending_indices`, `filter` | Correct filtering, sort order, each combination of FilterOpts. |
| `threads` | `sorted_global`, `next_thread`, `prev_thread` | Ordering by (file_index, line). Forward/backward traversal. Wrapping at ends. Single thread. Empty list. |
| `threads` | `to_summaries` | All fields mapped. Preview truncated to 40 chars. |
| `threads` | `check_auto_resolve_timeouts` | Thread within timeout untouched, thread past timeout reverted (auto_resolve=false, auto_resolve_at=None), non-auto-resolve threads untouched. |
| `state` | `load_review`, `save_review` | Round-trip. Missing file returns default. Corrupt file returns default (not panic). Directory creation. |
| `state` | `load_threads`, `save_threads` | Round-trip. Missing file returns empty vec. |
| `state` | `load_sessions`, `save_sessions`, `add_session` | Round-trip. Append behavior. |
| `state` | `workspace_hash` | Deterministic. Different inputs produce different hashes. Length is 12. |
| `state` | `content_hash` | Deterministic. Empty string handled. |
| `config` | `Default` | All fields populated with PRD values. |
| `config` | deserialization | Empty table = default. Partial table merges. Invalid backend = error. |
| `types` | enums | `Display` impls correct. Serde round-trips for all persisted enums. |
| `backend::queue` | `push`, `process_next` | FIFO ordering. Max concurrency = 1. `is_busy` transitions. `cancel_all` prevents pending callbacks. Empty queue = no-op. |
| `backend::cursor` | `build_args` | Each flag combination: session_id, continue, stream, ask_mode, model, json_schema (not supported). |
| `backend::claude` | `build_args` | Each flag combination: session_id, continue, stream, ask_mode, model, json_schema, workspace. |
| `backend` | JSON parsing | Valid response. Malformed JSON (error set, raw text preserved). Missing fields. |
| `backend` | `parse_self_review_text` | Valid multi-thread input. Mixed valid/invalid lines. Empty input. Missing fields discarded. |

**Error path coverage (every module):**

Each module's unit tests include explicit negative cases:

- Invalid input (empty, malformed, wrong type)
- I/O failures (state module: permission denied, disk full - via
  tempdir with read-only permissions)
- Missing dependencies (git not found, binary not on PATH)
- Boundary conditions (zero threads, max threads, very long
  strings, Unicode in paths)

#### Layer 2: Neovim Integration Tests

`#[nvim_oxi::test]` functions in `tests/src/nvim/`. Each test
spawns inside a real Neovim instance. These validate Neovim API
interactions.

| Test File | What It Tests |
|---|---|
| `highlight.rs` | `setup()` creates all 13 highlight groups. Each group links to the correct default. |
| `render.rs` | `diff::render()` produces correct buffer content (line count, line text). Thread summary lines appear before diff content. `show_resolved=false` omits resolved summaries. `thread_buf_lines` maps IDs to correct lines. `apply_highlights` sets correct extmark highlights per line prefix. |
| `file_panel.rs` | `render()` builds correct tree structure. Status icons correct. Summary section correct. `path_at_line` returns paths for file lines, `None` for directory/summary lines. |
| `review.rs` | `open()` creates tabpage with 2 windows. `is_active()` true after open, false after close. `close()` removes tabpage. Double-open shows notification. Panel buffers are scratch with correct options. |
| `thread_window.rs` | `window::open()` creates floating window with correct dimensions. `append_message()` adds formatted line. `append_streaming()` appends to last line. `close()` cleans up buffer and window. `is_open()` state transitions. |
| `keymaps.rs` | Buffer-local keymaps bound on diff panel buffer. Keymaps do not exist on non-workbench buffers. Keymaps removed on close. Verifies keymap existence via `buf.get_keymap()`. |
| `commands.rs` | All commands registered after `setup()`. Gated commands without review show notification (capture with mock `api::notify`). Global commands without review do not error. |

**Fixtures for nvim tests:**

Since these run inside Neovim, they cannot easily access the
filesystem for fixture files. Instead, fixtures are embedded as
`const &str` in `tests/src/fixtures.rs`:

```rust
pub const SIMPLE_DIFF: &str = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello world\");
+    println!(\"goodbye\");
 }
";

pub const MULTI_FILE_DIFF_NAMES: &str = "\
M\tsrc/main.rs\n\
A\tsrc/lib.rs\n\
D\tsrc/old.rs\n\
";
```

#### Layer 3: End-to-End Workflow Tests

Also `#[nvim_oxi::test]`, in `tests/src/e2e/`. These test complete
user workflows end-to-end with real git repos and a mock backend.

| Test File | Workflow Tested |
|---|---|
| `review_workflow.rs` | Open review on a temp git repo. Verify file panel lists changed files. Verify diff panel shows correct diff. Navigate with `]c`, verify cursor position. Mark file approved with `ga`, verify status persists after close+reopen. |
| `thread_workflow.rs` | Create thread with `gc`. Verify summary appears in diff. Navigate to thread with `]t`. Resolve with `gR`, verify it disappears (show_resolved=false). Toggle resolved with `g?`, verify it reappears. Create thread on different file, verify `]t` switches files. |
| `poll_workflow.rs` | Open review. Modify a file on disk. Trigger a manual refresh. Verify diff panel updated. Verify an approved file resets to unreviewed after change. Verify new hunks get `ArbiterHunkNew` extmarks. |
| `turn_workflow.rs` | Enter human turn. Verify snapshot created (`stash create`). Return to agent turn with mock backend. Verify handback sent with correct diff. Verify statusline transitions. |

**Mock backend for e2e tests:**

The `MockAdapter` is injected before each e2e test by calling
`backend::setup_with_adapter()` (a test-only function behind
`#[cfg(test)]`):

```rust
pub struct MockAdapter {
    responses: Mutex<VecDeque<BackendResult>>,
    calls: Mutex<Vec<BackendOpts>>,
}

impl MockAdapter {
    pub fn new(responses: Vec<BackendResult>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<BackendOpts> {
        self.calls.lock().unwrap().clone()
    }
}

impl Adapter for MockAdapter {
    fn execute(
        &self,
        opts: BackendOpts,
        _on_stream: Option<OnStream>,
        callback: OnComplete,
    ) {
        self.calls.lock().unwrap().push(opts);
        let result = self.responses.lock().unwrap().pop_front()
            .unwrap_or_else(|| BackendResult {
                text: String::new(),
                session_id: "mock-session".into(),
                error: None,
            });
        callback(result);
    }

    fn binary_name(&self) -> &str { "mock" }
}
```

The `MockAdapter` records every call's `BackendOpts`, which tests
inspect to verify prompt assembly, session ID usage, and flag
correctness.

**Temp git repo helper:**

```rust
pub struct TempGitRepo {
    pub dir: tempfile::TempDir,
}

impl TempGitRepo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        Command::new("git").args(["init"]).current_dir(path)
            .output().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"])
            .current_dir(path).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test"])
            .current_dir(path).output().unwrap();

        std::fs::write(path.join("initial.txt"), "initial\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(path)
            .output().unwrap();
        Command::new("git").args(["commit", "-m", "initial"])
            .current_dir(path).output().unwrap();
        Self { dir }
    }

    pub fn path(&self) -> &Path { self.dir.path() }

    pub fn write_file(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    pub fn add_and_commit(&self, message: &str) {
        let path = self.dir.path();
        Command::new("git").args(["add", "."]).current_dir(path)
            .output().unwrap();
        Command::new("git").args(["commit", "-m", message])
            .current_dir(path).output().unwrap();
    }
}
```

#### Layer 4: Backend Smoke Tests

Marked with `#[ignore]` so they don't run in CI. Manual execution
against real Cursor/Claude CLIs. Thin tests: send a trivial prompt,
verify the response parses into a valid `BackendResult` with a
non-empty `session_id`.

```rust
#[test]
#[ignore]
fn smoke_cursor_adapter() {
    let adapter = CursorAdapter::new(&Config::default());
    let opts = BackendOpts {
        prompt: "Say 'hello' and nothing else.".into(),
        ..Default::default()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    adapter.execute(opts, None, Box::new(move |result| {
        tx.send(result).unwrap();
    }));
    let result = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(result.error.is_none());
    assert!(!result.session_id.is_empty());
    assert!(!result.text.is_empty());
}
```

### Architectural Enforcement

CI includes a module boundary check that prevents accidental coupling
between streams. This is a shell script (or Rust test) that verifies
prohibited imports:

```bash
#!/usr/bin/env bash
set -euo pipefail

ERRORS=0

check_no_import() {
    local module="$1"
    local forbidden="$2"
    if rg "use crate::${forbidden}" "crates/arbiter/src/${module}" 2>/dev/null; then
        echo "ERROR: ${module} imports ${forbidden}"
        ERRORS=$((ERRORS + 1))
    fi
}

# Stream 1 (git, diff) must not import threads, backend, review
for mod in git.rs diff/; do
    for forbidden in threads backend review poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

# Stream 2 (threads, state) must not import git, diff, backend, review
for mod in threads/ state.rs; do
    for forbidden in git diff backend review poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

# Stream 3 (backend) must not import threads, diff, review, git
for mod in backend/; do
    for forbidden in threads diff review git poll turn; do
        check_no_import "$mod" "$forbidden"
    done
done

exit $ERRORS
```

This runs in CI alongside `cargo test`.

### Test Data Strategy

**Embedded fixtures** (`tests/src/fixtures.rs`): const strings for
diffs, file lists, JSON responses, and thread state. Used by both
unit and nvim tests. Single source of truth for test data.

**Generated git repos** (`TempGitRepo`): created per-test with known
files and commits. Destroyed on test completion. Never shared between
tests.

**Thread builders**: a test-only builder for constructing `Thread`
objects with sensible defaults:

```rust
pub struct ThreadBuilder {
    thread: Thread,
}

impl ThreadBuilder {
    pub fn new(file: &str, line: u32) -> Self {
        Self {
            thread: Thread {
                id: uuid::Uuid::new_v4().to_string(),
                origin: ThreadOrigin::User,
                file: file.to_string(),
                line,
                anchor_content: String::new(),
                anchor_context: Vec::new(),
                status: ThreadStatus::Open,
                auto_resolve: false,
                auto_resolve_at: None,
                context: ThreadContext::Review,
                session_id: None,
                messages: Vec::new(),
                pending: false,
            },
        }
    }

    pub fn origin(mut self, origin: ThreadOrigin) -> Self {
        self.thread.origin = origin;
        self
    }

    pub fn status(mut self, status: ThreadStatus) -> Self {
        self.thread.status = status;
        self
    }

    pub fn message(mut self, role: Role, text: &str) -> Self {
        self.thread.messages.push(Message {
            role,
            text: text.to_string(),
            ts: 0,
        });
        self
    }

    pub fn anchor(mut self, content: &str, context: Vec<&str>) -> Self {
        self.thread.anchor_content = content.to_string();
        self.thread.anchor_context = context.into_iter()
            .map(|s| s.to_string()).collect();
        self
    }

    pub fn pending(mut self) -> Self {
        self.thread.pending = true;
        self
    }

    pub fn auto_resolve(mut self, at: i64) -> Self {
        self.thread.auto_resolve = true;
        self.thread.auto_resolve_at = Some(at);
        self
    }

    pub fn build(self) -> Thread { self.thread }
}
```

### Coverage Goals

| Layer | Target | Measured By |
|---|---|---|
| Unit tests (L1) | Every public function has at least one positive and one negative test | Manual review at each issue |
| nvim tests (L2) | Every Neovim API interaction point has a test | Manual review at each epic |
| e2e tests (L3) | Every PRD user-facing workflow has a test | Coverage checklist (below) |
| Total branch | 80%+ on non-UI code | `cargo tarpaulin` or `cargo llvm-cov` in CI |

**e2e workflow coverage checklist:**

- [ ] Open review, verify file panel and diff panel
- [ ] Navigate hunks (]c/[c), verify cursor positions
- [ ] Navigate files (]f/[f), verify panel switch
- [ ] Toggle file approval (<Leader>aa), verify persistence
- [ ] Mark file needs-changes (<Leader>ax), verify icon
- [ ] Reset file (<Leader>ar), verify icon
- [ ] Show summary (<Leader>as), verify counts
- [ ] Add comment (<Leader>ac), verify thread opens and agent responds
- [ ] Add auto-resolve comment (<Leader>aA), verify timeout behavior
- [ ] Open thread window (<Leader>ao), verify conversation
- [ ] Reply in thread, verify backend called with session_id
- [ ] Resolve thread (<Leader>aR), verify removal from summary
- [ ] Toggle resolved (<Leader>a?), verify re-display
- [ ] Navigate threads cross-file (]t/[t)
- [ ] Thread list via quickfix (<Leader>aT), verify all threads listed
- [ ] Thread list filters (<Leader>aTa/<Leader>aTu/<Leader>aTb/<Leader>aTo)
- [ ] Cancel request (<Leader>aK), verify pending requests cancelled
- [ ] Self-review, verify agent threads created
- [ ] Poll detects file change, verify re-render
- [ ] Approved file reset on change
- [ ] New hunk indicators after change
- [ ] Thread re-anchor on file change
- [ ] Auto-resolve timeout
- [ ] Turn cycling: human turn snapshot, agent turn handback
- [ ] Side-by-side toggle
- [ ] Fold approved hunks
- [ ] Global commands without workbench (ArbiterSend, ArbiterCatchUp)
- [ ] ArbiterList / ArbiterResume
- [ ] Inline thread indicators (opt-in)
- [ ] Workbench close cancels pending queue items
- [ ] Git failure does not crash workbench
- [ ] Missing CLI binary shows error, diff viewer still works

### CI Pipeline

```yaml
# .github/workflows/ci.yml (simplified)
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install Neovim
        run: |
          curl -LO https://github.com/neovim/neovim/releases/download/stable/nvim-linux-x86_64.tar.gz
          tar xzf nvim-linux-x86_64.tar.gz
          echo "$PWD/nvim-linux-x86_64/bin" >> $GITHUB_PATH
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo build
      - run: cargo test
      - name: Module boundary check
        run: ./scripts/check_boundaries.sh
```

CI requires Neovim on PATH because `nvim-oxi` tests spawn a
Neovim process. The install step adds the stable Neovim binary.

### What Is Not Tested

- Real Cursor/Claude CLI behavior (manual smoke tests only)
- Neovim plugin manager installation paths (manual)
- Performance budgets (manual benchmarks, not enforced in CI)
- Visual appearance (highlight colors, window sizing)

## Performance Budget

| Operation | Target | Mechanism |
|---|---|---|
| `:Arbiter` open | <200ms to first paint | Async git, render as files arrive |
| File switch (`]f`) | <50ms | Pre-fetched diff or <50ms git diff for single file |
| Hunk jump (`]c`) | <1ms | In-memory hunk index lookup |
| Thread jump (`]t`) | <1ms | In-memory thread index lookup |
| Poll tick (no change) | <1ms | Single `fs_stat()` call |
| Poll tick (file changed) | <100ms | git diff + re-render |
| State save | <10ms | JSON encode + write small file |

The main risk is the initial `git diff --name-status` call on very
large repos with thousands of changed files. This is unlikely in the
agent review use case (agents typically change <100 files) but if
needed, we can add `--no-renames` to skip rename detection.

## Milestone Mapping

| PRD Milestone | Streams | Modules |
|---|---|---|
| M0.5: Navigable Diff Viewer | S1 complete, S4 partial | `git`, `diff`, `file_panel`, `highlight`, `review`, `config`, `lib`, `state` (review only) |
| M1: Review Workbench | S1 complete, S2 partial, S4 partial | + `threads` (CRUD + persistence), `state` (threads) |
| M2: Threads | S2 complete, S4 partial | `threads`, `thread_window`, `comment_input`, `state` (threads) |
| M3: Backend Shim | S3 complete, S4 partial | `backend/*`, wiring in `review` and `init` |
| M4: Self-Review | S3 additions, S4 additions | `backend/*` (self-review parsing), `init` (global commands), `review` (response panel) |
| M5: Live Refresh + Turns | S4 additions | `poll` (full), `turn` |
| M6: Polish | S1 additions, S4 additions | `diff` (side-by-side), `threads` (list view, filtering), `init` (inline indicators) |
