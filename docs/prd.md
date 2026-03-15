# arbiter: Product Requirements Document

> **Implementation note (2026-03):** This document represents the
> original design vision. The current implementation differs in several
> areas. Key changes:
>
> - All keymaps moved from `g`-prefixed (`gc`, `ga`, `gC`, etc.) to
>   `<Leader>a`-prefixed (`<Leader>ac`, `<Leader>aa`, `<Leader>aC`,
>   etc.) to avoid conflicts with Neovim built-in motions.
> - **Batch comments removed.** All comments are now sent to the agent
>   immediately on submission. The batch/immediate distinction
>   (`gc` vs `gC`) and `:AgentSubmitReview` are documented below as
>   future considerations.
> - Thread lists use the native quickfix list instead of a custom float.
> - Additional keymaps added: `<Leader>aK` (cancel request),
>   `<Leader>aTo` (list open threads), `<Leader>an`/`<Leader>ap`
>   (next/prev unreviewed file).
> - `thread_window` config added for window size and timestamp format.
> - `chrono` used for timestamp formatting.
> - Build system uses `Taskfile.yml` (go-task) instead of Makefile.
>
> See the README for the current configuration and keymap reference.

## Problem

Developers using AI coding agents (Cursor, Claude Code) spend most of
their time in a loop: the agent writes code, the human reviews it. Today
that review step is broken.

The agent runs in one terminal tab. The human reviews in another. For
small changes, `git diff` works. For anything beyond a few files, the
developer either squints at a wall of terminal diff output or pushes a
throwaway branch to GitLab/GitHub just to get a usable diff viewer. Both
options are slow, context-destroying, and have no feedback channel back
to the agent.

The core problem is not diffing. The core problem is that reviewing
agent-generated code is a fundamentally different activity than reviewing
human-written code. The reviewer didn't write any of it. They need to
understand intent, verify correctness, and steer the agent when
something is wrong - all without leaving their editor.

No tool exists for this workflow.

## Solution

arbiter is a Neovim plugin that turns diff review into a
collaborative feedback loop between the human and the agent. It
provides:

1. A structured diff viewer built for reviewing large, multi-file
   changesets.
2. A thread-based comment system (modeled on GitHub PR reviews) where
   each comment becomes a conversation with the agent.
3. Live refresh so the reviewer sees agent changes as they happen.
4. Turn cycling so the human can make manual edits and the agent
   automatically picks up what changed.

The plugin delegates all LLM interaction to the Cursor and Claude Code
CLIs. It is not a chat interface or an LLM wrapper. It is a review
workbench with an agent feedback channel.

## Principles

**Neovim-native.** Every interaction uses standard motions and gestures.
No custom modal dialogs, no mouse-required interactions, no invented
navigation paradigms. If you can navigate Neovim, you can navigate a
review.

**The agent is invisible infrastructure.** The user configures a backend
once. They never see session IDs, CLI flags, or output formats. The
plugin handles all of it.

**Threads are the unit of feedback.** Every piece of feedback is a
thread. Threads have conversations, threads have resolution, threads
survive restarts. Not free-form chat, not inline annotations. Threads.

**Polling over watching.** File system watchers are unreliable across
platforms. We poll the currently viewed file on a timer. Simple,
predictable, works everywhere.

**Rust-first.** The plugin is implemented in Rust via `nvim-oxi`,
compiled to a shared library that Neovim loads as a Lua module.
The user calls `require("arbiter").setup({...})` in their config;
everything behind that call is compiled Rust. No Lua plugin
dependencies (no plenary, no nui). The only runtime requirements are
`git` and at least one of `agent` (Cursor) or `claude` (Claude Code).
Building from source requires the Rust toolchain.

## Agent Mode

arbiter introduces a plugin-level operational state called **agent
mode**. This is not a Neovim mode (like normal, insert, or visual). It
is a state managed by the plugin that determines which features are
active. Think of it as a layer on top of Neovim's mode system.

**Agent mode is active when a review workbench is open.** Opening the
workbench (`:Arbiter`) enters agent mode. Closing it (`q`) exits
agent mode. The user is always in a Neovim mode (normal, insert, etc.)
while simultaneously being in or out of agent mode.

### Why a Distinct Mode

Without this boundary, the plugin would either:

1. Register global keymaps that conflict with normal editing, or
2. Leave every feature available everywhere, creating confusion about
   which commands make sense in which context.

Agent mode solves both: review-specific keymaps (`<Leader>ac`,
`<Leader>aa`, `]c`, `<Leader>aT`, etc.) only exist inside the
workbench buffers. The user's normal editing environment is never
touched. There is zero keymap pollution.

### What Requires Agent Mode

All review-specific features are gated behind an active workbench:

- **Diff navigation**: `]c`/`[c`, `]f`/`[f`, `]t`/`[t`
- **Review marking**: `<Leader>aa`, `<Leader>ax`, `<Leader>ar`, `<Leader>as`
- **Thread operations**: `<Leader>ac`, `<Leader>aC`, `<Leader>aA`, `<Leader>ao`, `<Leader>aR`, `<Leader>a?`, `<Leader>aT`
- **Live diff refresh** (polling)
- **Side-by-side toggle** (`<Leader>s`)
- **Turn cycling** (`<Leader>at` toggle, `:AgentTurn`, `:HumanTurn`)
- **Manual refresh** (`:ArbiterRefresh`)
- **Self-review** (`:ArbiterSelfReview`)
- **Resolve all** (`:ArbiterResolveAll`)

If the user runs one of these commands without an active workbench,
the plugin shows a notification: "No active review. Run :Arbiter
first."

### What Works Outside Agent Mode

Some features are valuable without opening the full workbench. These
are registered as global commands on `setup()` and always available:

| Command                 | Purpose                                      |
|-------------------------|----------------------------------------------|
| `:Arbiter [ref]`    | Enter agent mode (open the workbench)        |
| `:ArbiterSend <prompt>`   | Send a prompt to the agent (new session)     |
| `:ArbiterContinue <prompt>` | Continue the review session                |
| `:ArbiterCatchUp`         | Ask the agent for a context summary          |
| `:ArbiterList`            | List prior agent sessions                    |
| `:ArbiterResume <id>`     | Reconnect to a specific session              |

These are useful in normal editing because:

- **`:ArbiterSend`**: You're reading code, you want to ask the agent
  something. No need to open a full workbench for a quick question.
- **`:ArbiterCatchUp`**: You just opened a terminal tab and want to
  remember where you left off before deciding whether to start a
  review.
- **`:ArbiterList` / `:ArbiterResume`**: Session management is not
  review-specific. You might want to resume a conversation from a
  previous session without entering review mode.
- **`:ArbiterContinue`**: You have an ongoing review session and want to
  give the agent a follow-up instruction while you're editing.

Responses from these commands open in a floating window (if no
workbench is open) or in the response panel (if one is).

### Inline Thread Indicators (Outside Agent Mode)

When the user is editing a file that has open threads (from a prior or
active review), the plugin shows sign-column indicators on the
anchored lines:

```
  15   fn handle_login(
  16     req: LoginRequest,
  17   ) -> Result<Response> {
▎ 18-    let user = db.find(req.email);
  19+    let user = db
```

The `▎` sign on line 18 indicates an open thread. The user can press
`go` on that line to open the thread in a floating window, even
outside the workbench. This bridges agent mode and normal editing:
you don't need to open the full review to see or respond to a thread
you just passed while reading code.

This feature is opt-in (`config.inline_indicators = false` by default)
because it requires scanning the thread state for the current buffer's
file path on every `BufEnter`. The cost is low (JSON file read, no
git calls) but the visual noise may not be wanted by all users.

### Statusline

The statusline component (`require("arbiter").statusline()`) works
everywhere. It returns:

- `""` when no review is active
- `"[AGENT]"` during agent turn
- `"[HUMAN]"` during human turn
- `"[REVIEW n/m]"` showing review progress (n approved out of m files)

The user can include this in their statusline configuration regardless
of whether they're in agent mode.

## User Experience

### Opening a Review

```
:Arbiter              " diff of all uncommitted changes
:Arbiter main         " diff against a branch
:Arbiter HEAD~3       " diff against a ref
```

A new tabpage opens with two panels:

```
┌─ File Panel ──────────┐┌─ Diff Panel ──────────────────────┐
│                        ││                                    │
│  src/                  ││ ── handler.rs (1 thread) ────────  │
│    auth/               ││                                    │
│  ✓  handler.rs         ││  [you] :22  handle empty email..  │
│  ✗  middleware.rs      ││                                    │
│     models/            ││ @@ -15,8 +15,12 @@                │
│  ·  user.rs            ││   fn handle_login(                 │
│  tests/                ││     req: LoginRequest,             │
│  ·  auth_test.rs       ││   ) -> Result<Response> {          │
│                        ││-    let user = db.find(req.email); │
│ ── Review ──────────── ││+    let user = db                  │
│ ✓ 1 approved           ││+      .find_by_email(&req.email)   │
│ ✗ 1 needs changes      ││+      .await                       │
│ · 2 unreviewed         ││+      .map_err(AuthError::from)?;  │
│                        ││                                    │
└────────────────────────┘└────────────────────────────────────┘
```

The **file panel** (left) is a tree-style listing of changed files with
review status icons (✓ approved, ✗ needs-changes, · unreviewed). A
summary section shows counts. It is a scratch buffer with its own
keymaps. `<CR>` switches the diff panel to the selected file.

The **diff panel** (right) shows the unified diff for the currently
selected file. Above the diff content, a thread summary section lists
all threads on this file. The panel is a scratch buffer rendered by the
plugin with custom highlighting.

Both panels support standard Neovim motions: `j`/`k`, `/` search,
`G`, `gg`, etc.

The workbench is an isolated tabpage. `q` closes the tab and returns
to the user's editing environment. Nothing about the user's buffer
list, window layout, or marks is affected.

### Navigation

| Gesture       | Action                                        |
|---------------|-----------------------------------------------|
| `j` / `k`     | Move between lines                            |
| `]c` / `[c`   | Jump to next/previous hunk                    |
| `]f` / `[f`   | Jump to next/previous file                    |
| `]t` / `[t`   | Jump to next/previous thread (cross-file)     |
| `<CR>`        | Open file at cursor line / open thread         |
| `zo` / `zc`   | Expand/collapse sections                       |
| `<Leader>s`   | Toggle unified / side-by-side diff             |
| `q`           | Close the review workbench                     |

`]t` / `[t` are the fastest way to move through a review. They jump
between thread summary lines. If the next thread is in a different
file, the diff panel switches automatically.

**Side-by-side** is a toggle, not a mode. When activated, the plugin
opens two buffers (ref version and working tree version) with Neovim's
built-in diff mode (`:diffthis`). Neovim handles scroll binding, diff
highlighting, and folds. Toggling back returns to unified view. Off by
default.

### Marking Review Progress

| Gesture | Action                          |
|---------|---------------------------------|
| `ga`    | Mark file as approved            |
| `gx`    | Mark file as needs-changes       |
| `gr`    | Reset file to unreviewed         |
| `gs`    | Show review summary (float)      |

Review state persists to disk. Close Neovim, reopen tomorrow, run
`:Arbiter main` again, and the same files are still marked. State
is stored in `~/.local/share/nvim/arbiter/` (outside the git tree,
never tracked).

When a file's content changes after being approved, the plugin resets
it to unreviewed and highlights it in the file panel. The reviewer
only needs to look at what's new.

### Review Comments and Threads

Feedback is structured as threads, not free-form chat. Every comment
starts a thread. Every thread is a conversation between the reviewer
and the agent.

**Adding a comment:**

Navigate to a diff line. Press `gc` (batch) or `gC` (immediate).

A floating multi-line input buffer appears. Type the comment. Press
`<CR>` in normal mode to submit.

The comment becomes a thread, displayed in the thread summary above
the file's diff:

```
│ ── handler.rs (2 threads) ──────────────────────
│
│  [you] :22  handle the case where email is..  [open]
│  [you] :35  expiry check before sig valid..   [open]
│
│ @@ -15,8 +15,12 @@
```

**All comments are sent immediately.** When you submit a comment,
the thread opens and the agent's response streams in. There is no
batching step.

> **Future consideration: Batch mode.** An earlier design included a
> batch/immediate distinction where `gc` saved comments locally and
> `:AgentSubmitReview` sent them all at once. This would let a
> reviewer read the entire diff, leave comments, and submit them as a
> coherent set. This could be revisited if users find the immediate
> model too disruptive for large reviews.

**Opening a thread:**

Press `go` or `<CR>` on a thread summary line. A floating window shows
the full conversation:

```
┌─ Thread: handler.rs:22 ──────────── [you] [open] ──┐
│                                                      │
│  You:    handle the case where email is empty         │
│                                                      │
│  Agent:  I'll add a guard clause. Should it return    │
│          400 Bad Request or use a default?             │
│                                                      │
│  You:    400, with a structured error body.            │
│                                                      │
│  Agent:  Done. I've added validation at line 18.      │
│                                                      │
│ ──────────────────────────────────────────────────── │
│ Reply:                                                │
└──────────────────────────────────────────────────────┘
```

Type a reply at the bottom. The reply is sent to the thread's
dedicated CLI session. The agent's response streams in and is appended
to the conversation.

**Resolving a thread:**

| Gesture             | Action                           |
|---------------------|----------------------------------|
| `gR`                | Resolve thread under cursor       |
| `:ArbiterResolveAll`  | Resolve all threads               |
| `g?`                | Toggle visibility of resolved     |

Only the reviewer resolves threads. The agent can respond and make
code changes, but it cannot close a conversation. This is the same
model as GitHub: the author doesn't resolve their own review comments.

**Auto-resolve:**

Press `gA` instead of `gc`/`gC`. The comment is sent immediately, and
when the polling timer detects the file changed, the thread is
auto-resolved and the corresponding diff section is auto-approved.

This is for mechanical feedback like "rename `foo` to `bar`" where
there's nothing to discuss and no need to re-review. If no file change
is detected within 60 seconds, the thread reverts to a normal open
thread.

### Agent-Initiated Threads

The agent can also initiate threads via `:ArbiterSelfReview`.

This command sends the current diff to the agent and asks it to review
its own work. The response is structured: the plugin uses
`--json-schema` (Claude Code) to extract thread data as JSON, or a
hardened prompt template (Cursor) to parse threads from text.

Agent threads appear in the same thread summary, distinguished by
`[agent]` prefix and a different highlight:

```
│  [agent] :22  Should this return 401 or 403?   [open]
│  [you]   :35  expiry check before sig valid..  [open]
```

The reviewer interacts with agent threads the same way: `go` to open,
reply, `gR` to resolve. Replying starts a new session for that thread.

`:ArbiterSelfReview` is the only mechanism for agent-initiated threads
in v1. Proactive threads during implementation (the agent raising
questions while it's working) are deferred. The self-review workflow
covers the core use case without requiring system prompt injection.

### Thread Re-Anchoring

When the agent modifies files, thread anchors may become stale.

**Tier 1 - Content matching.** The plugin stores each thread's anchor
line content and surrounding context. On refresh, it searches for that
content in the file. If found at a new line number, the anchor updates
silently. This is free and handles the common case (lines shifted due
to insertions above).

**Tier 2 - The bin.** If content matching fails, the thread moves to
the bin. The bin is a section in the thread list (`gTb`) showing
orphaned threads. The reviewer can:

- `dd` to dismiss
- `gR` to resolve
- `gP` to request agent re-anchoring (sends the thread context to the
  agent in ask mode; the agent suggests a new location)

Agent re-anchoring is user-initiated. The plugin never makes surprise
CLI calls.

### Live Diff Refresh

The plugin polls the currently viewed file for changes every 2 seconds
(`vim.loop.new_timer()`, configurable). On each tick, it checks the
file's mtime. If the file changed:

1. The diff is recomputed and the panel updates in place.
2. Cursor position and scroll are preserved.
3. New/modified hunks are visually marked.
4. Approved files that changed are reset to unreviewed.
5. Pending auto-resolve threads on this file are resolved.

Only the currently viewed file is polled. The file list refreshes on a
slower interval (5 seconds) via `git diff --name-only`.

Manual refresh: `gU` or `:ArbiterRefresh`.

### Turn Cycling

The plugin introduces two turns: **agent turn** and **human turn**.

| Gesture / Command | Action                                    |
|-------------------|-------------------------------------------|
| `:AgentTurn`      | Enter agent turn. Snapshot working tree.   |
| `:HumanTurn`      | Enter human turn. Normal editing.          |
| `<Leader>a`       | Toggle turns.                              |

The current turn shows in the statusline: `[AGENT]` or `[HUMAN]`.

**Agent turn** is the review workbench. The user navigates diffs,
approves files, leaves comments.

**Human turn** is normal Neovim editing. The user makes manual changes
to files. When they toggle back to agent turn:

1. The plugin computes `git diff <snapshot>` where `<snapshot>` is a
   commit hash from `git stash create` (run when entering human turn).
2. If the diff is non-empty, it's sent to the agent via the review
   session: "The user made these manual edits: [diff]. Continue with
   these in mind."
3. The review workbench re-opens with the updated diff.

If the diff is empty, the turn switches silently.

### Catch Up

When the user opens Neovim in a new tab (or comes back later):

```
:ArbiterCatchUp
```

This continues the review session with a summarization prompt. The
agent responds with a summary of what it changed and where things
stand. The response appears in a scratch buffer at the bottom.

```
:ArbiterList          " list prior sessions
:ArbiterResume <id>   " reconnect to a specific session
```

### Free-Form Prompts

Outside of review comments:

```
:ArbiterSend <prompt>         " new session
:ArbiterContinue <prompt>     " continue review session
:ArbiterResume <id> <prompt>  " resume specific session
```

Responses stream into a horizontal scratch buffer at the bottom of the
workbench, with `filetype=markdown`.

## Backend CLI Shim

The plugin supports Cursor (`agent`) and Claude Code (`claude`) behind
a unified interface. The backend is configured once and never exposed
to the user.

### Session Management

Each thread gets its own CLI session. When the first message in a
thread is sent, the plugin starts a new session and captures the
`session_id` from the JSON response. All subsequent replies use
`--resume <id>`.

A separate **review session** handles non-thread operations: CatchUp,
Handback, SelfReview, and free-form prompts.

The user's interactive agent session in the other terminal is never
touched. Thread sessions are created by the plugin and only the plugin
uses them.

### Call Queue

All CLI calls go through a FIFO queue and execute sequentially. No
concurrent CLI access. This is simple and avoids every category of
race condition.

### CLI Flag Mapping

| Operation       | Cursor (`agent`)              | Claude Code (`claude`)          |
|-----------------|-------------------------------|---------------------------------|
| Non-interactive | `-p "prompt"`                 | `-p "prompt"`                   |
| Output format   | `--output-format json`        | `--output-format json`          |
| Resume session  | `--resume <id>`               | `--resume <id>`                 |
| Continue latest | `--continue`                  | `--continue`                    |
| Streaming       | `--output-format stream-json --stream-partial-output` | `--output-format stream-json --include-partial-messages` |
| Model           | `--model <m>`                 | `--model <m>`                   |
| Ask mode        | `--mode ask`                  | `--permission-mode plan`        |
| JSON schema     | (not supported)               | `--json-schema '{...}'`         |
| Workspace       | `--workspace <dir>`           | `--add-dir <dir>`               |

### Shim Operations

| # | Operation       | Description |
|---|-----------------|-------------|
| 1 | Send            | New session, non-interactive. Returns response + session ID. |
| 2 | Resume          | Resume specific session by ID. |
| 3 | Continue        | Continue most recent session. |
| 4 | Stream          | Same as Send/Resume/Continue but with streaming output. |
| 5 | CatchUp         | Continue review session with summarization prompt. |
| 6 | Handback        | Continue review session with user-edits diff. |
| 7 | ThreadReply     | Resume thread session with reply + conversation history. |
| 8 | SelfReview      | New session with diff + structured output schema. |
| 9 | ReAnchor        | Resume thread session in ask mode with anchor context. |

> **Future consideration:** Operation 8 was originally `SubmitReview`
> (send each pending batch comment as its own new session). Removed
> when batch mode was dropped in favor of immediate submission.

## Diff Viewer Architecture

The diff viewer is built from scratch. Not based on diffview.nvim.

### Why Not diffview.nvim

diffview.nvim is designed for passive viewing. It has no concept of
review state, threads, or agent interaction. Extending it would require
either deep modification (fragile, breaks on updates) or hacky layering
(limited control). Its last commit was August 2024. We don't need most
of what it provides (Mercurial, 3-way merge, file history).

We adopt its good patterns: tabpage isolation, file panel as a buffer,
async git commands. We build the rest.

### Neovim Primitives

| Primitive                | Purpose                                    |
|--------------------------|--------------------------------------------|
| `vim.diff()`             | Programmatic hunk extraction               |
| Diff mode (`diffthis`)   | Side-by-side with scroll binding           |
| `nvim_buf_set_extmark()` | Thread indicators, hunk markers             |
| `nvim_open_win()`        | Thread windows, comment input               |
| `nvim_create_buf()`      | Scratch buffers for all panels              |
| Tabpage                  | Workbench isolation                         |
| `vim.loop.new_timer()`   | Polling                                     |
| `vim.loop.spawn()`       | Async git commands                          |

No external diff libraries. No FFI. No dependencies beyond Neovim and
git.

### Diff Rendering

**Unified (default):** The plugin runs `git diff <ref> -- <file>`,
writes the output into a scratch buffer, and applies custom
highlighting per line prefix (`+` = `DiffAdd`, `-` = `DiffDelete`,
`@@` = `DiffChange`). Thread summaries are rendered between the file
header and the first hunk as regular buffer lines with their own
highlight group.

**Side-by-side (toggle):** Opens two buffers (ref version via
`git show` and working tree version) in a vertical split with
`:diffthis`. Neovim handles everything. Thread summaries appear as
virtual text in the right buffer.

### Git Interface

All git commands are async via `vim.loop.spawn`. The `git` module
exposes:

- `diff(ref, file, cb)` - unified diff for a file
- `diff_names(ref, cb)` - changed files with status (A/M/D)
- `untracked(cb)` - untracked files
- `show(ref, file, cb)` - file at a ref (for side-by-side)
- `stash_create(cb)` - snapshot for turn cycling
- `diff_hash(hash, cb)` - diff against snapshot

For untracked files, the plugin synthesizes an all-additions diff
(every line prefixed with `+`).

### Hunk Index

The plugin parses unified diff output into a hunk index per file:

- Buffer line range (for cursor positioning)
- Old/new file line ranges
- Content hash (for change detection on refresh)

This powers `]c`/`[c` navigation, auto-resolve detection, and thread
anchor mapping.

## Data Model

### Review State

Stored as JSON at `{state_dir}/{workspace_hash}/{diff_ref}.json`:

```json
{
  "files": {
    "src/auth/handler.rs": {
      "status": "approved",
      "content_hash": "a1b2c3",
      "updated_at": "2026-03-14T12:00:00Z"
    }
  }
}
```

### Threads

Stored at `{state_dir}/{workspace_hash}/{diff_ref}_threads.json`:

```json
{
  "threads": [
    {
      "id": "t-001",
      "origin": "user",
      "file": "src/auth/handler.rs",
      "line": 22,
      "anchor_content": "let user = db.find(req.email);",
      "anchor_context": ["  ) -> Result<Response> {", "    let user = ..."],
      "status": "open",
      "auto_resolve": false,
      "context": "review",
      "session_id": "abc-123",
      "messages": [
        { "role": "user", "text": "handle empty email", "ts": "..." },
        { "role": "agent", "text": "I'll add a guard...", "ts": "..." }
      ]
    }
  ]
}
```

All state is stored in `~/.local/share/nvim/arbiter/`, outside the
git tree. Never tracked by git. Survives Neovim restarts. Resolved
threads are retained but hidden.

## Configuration

```lua
-- The plugin is a compiled Rust shared library loaded as a Lua module.
-- Configuration is passed as a Lua table, deserialized into Rust types.
require("arbiter").setup({
  backend = "cursor",           -- "cursor" or "claude"
  model = nil,                  -- optional, use backend default
  inline_indicators = false,    -- show thread signs in normal buffers

  review = {
    default_ref = nil,          -- e.g. "main"; nil = unstaged changes
    side_by_side = false,       -- start in unified view
    fold_approved = false,      -- collapse approved hunks
    auto_resolve_timeout = 60,  -- seconds
    poll_interval = 2000,       -- ms, current file
    file_list_interval = 5000,  -- ms, file list refresh
    state_dir = vim.fn.stdpath("data") .. "/arbiter",
  },

  prompts = {
    catch_up = "Summarize the changes you've made and the current state of the project.",
    handback = "The user made the following manual edits while you were paused:\n\n%s\n\nContinue working with these changes in mind.",
    self_review = "Review this diff and flag anything you're uncertain about or want feedback on. For each concern, specify the file, line number, and your question.",
  },

  keymaps = {
    -- navigation
    next_hunk          = "]c",
    prev_hunk          = "[c",
    next_file          = "]f",
    prev_file          = "[f",
    next_thread        = "]t",
    prev_thread        = "[t",
    toggle_side_by_side = "<Leader>s",

    -- review
    approve            = "ga",
    needs_changes      = "gx",
    reset_status       = "gr",
    summary            = "gs",

    -- threads
    comment            = "gc",
    comment_immediate  = "gC",
    auto_resolve       = "gA",
    open_thread        = "go",
    list_threads       = "gT",
    resolve_thread     = "gR",
    toggle_resolved    = "g?",
    re_anchor          = "gP",

    -- turns
    toggle_turn        = "<Leader>a",

    -- refresh
    refresh            = "gU",
  },
})
```

Every keymap is overridable. Every prompt template is overridable.
Every interval is configurable. Defaults are opinionated and usable
out of the box.

## Milestones

Milestones are delivery checkpoints. Work streams (defined in the
RFD) are the parallel development tracks that feed into them. The
mapping is not 1:1. Streams 1-3 can run in parallel from day one.
Stream 4 scaffolds early and integrates progressively.

### M0.5: Navigable Diff Viewer (early value)

A usable diff viewer that provides immediate value before any thread
or agent features are built. The user can install the plugin and
start using it to review agent-generated changes.

**Issues:** E0-1, E0-2, E1-1, E1-2, E1-3, E4-1, E4-2, E4-3, E4-4

Deliverables:
- `:Arbiter [ref]` opens the workbench
- File panel with tree view and status icons
- Diff panel with unified diff, custom highlighting
- `]c`/`[c`, `]f`/`[f` navigation
- `<CR>` opens source file at the corresponding line
- `ga`/`gx`/`gr`/`gs` review marking
- Review state persistence across restarts
- `q` closes the workbench

This milestone is useful on its own as a standalone diff reviewer.
No threads, no agent integration, no polling. A developer can use
it immediately to review any branch diff inside Neovim.

### M1: Review Workbench (complete)

Builds on M0.5 by adding thread CRUD (local-only, no agent) and
the full review state lifecycle.

**Work streams:** S1 complete, S2 (Thread Data) partial (state
persistence only), S4 (UI Shell) partial (thread data wiring).

Deliverables (in addition to M0.5):
- Thread CRUD and persistence
- Thread summary lines in diff panel
- Content-based thread re-anchoring
- Session persistence (`:ArbiterList` / `:ArbiterResume` stubs)

### M2: Threads

Comment and thread system, including the floating thread window.
No agent backend yet; threads are local-only.

**Work streams:** S2 (Thread Data) complete, S4 (UI Shell) partial
(thread window, comment input, thread navigation keymaps).

Deliverables:
- `gc` to add a comment (stored locally)
- Thread summary display above file diffs
- `go` opens floating thread window
- `]t`/`[t` thread navigation (cross-file)
- `gR` to resolve, `g?` to toggle resolved
- Thread persistence

### M3: Backend Shim and Agent Integration

Connect threads to the agent CLI. This is where the review becomes
a conversation.

**Work streams:** S3 (Backend Shim) complete, S4 (UI Shell) partial
(wiring thread operations to backend calls, response routing).

Deliverables:
- Backend shim with Cursor and Claude Code adapters
- Session management (per-thread sessions, review session)
- Call queue (FIFO, sequential execution)
- `<Leader>ac`/`<Leader>aC` immediate comment (sent to agent, response streams in thread window)
- `<Leader>aA` auto-resolve
- Streaming responses in thread windows

> **Future consideration:** `:AgentSubmitReview` batch submission was
> originally planned here but removed in favor of immediate submission.

### M4: Self-Review and Catch-Up

Agent-initiated threads and session re-orientation.

**Work streams:** S3 additions (self-review parsing), S4 additions
(command registration for global commands, response panel).

Deliverables:
- `:ArbiterSelfReview` with structured output parsing
- Agent threads with `[agent]` display
- `:ArbiterCatchUp` summarization
- `:ArbiterList` / `:ArbiterResume`
- Free-form prompts (`:ArbiterSend`, `:ArbiterContinue`)

### M5: Live Refresh and Turn Cycling

Real-time updates and the human/agent turn loop.

**Work streams:** S4 additions (poll.lua, turn.lua wiring).

Deliverables:
- Per-file mtime polling
- File list periodic refresh
- Automatic review state reset on file change
- Thread re-anchoring (content matching + bin)
- Turn cycling (`<Leader>a`, snapshot, handback)
- Statusline integration (`[AGENT]` / `[HUMAN]`)

### M6: Side-by-Side and Polish

Final features and UX refinements.

**Work streams:** S1 additions (side-by-side), S4 additions (thread
list view, fold support, inline indicators).

Deliverables:
- Side-by-side toggle via built-in diff mode
- Fold approved hunks (optional)
- Thread filtering (`gTa`, `gTu`, `gTb`)
- Untracked file support (synthesized diffs)
- Inline thread indicators (opt-in)
- Error handling and edge case hardening
- Documentation and README
