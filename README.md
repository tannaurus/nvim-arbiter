# arbiter

[![CI](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml/badge.svg)](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tannaurus/nvim-arbiter?style=flat&label=release)](https://github.com/tannaurus/nvim-arbiter/releases/latest)
[![Neovim](https://img.shields.io/badge/neovim-0.10%2B-57A143?logo=neovim&logoColor=white)](https://neovim.io)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)]()

> **Experimental** - This plugin is under active development. APIs, commands, and keymaps may change without notice.

Agentic review workbench for Neovim. Provides a structured diff viewer and thread-based review system for collaborating with AI coding agents (Cursor CLI, Claude Code CLI).

Built in Rust with [nvim-oxi](https://github.com/noib3/nvim-oxi), compiled to a native shared library.

## Features

- **Review workbench** - Dedicated tabpage with a file panel (left) and diff panel (right)
- **PR-style diffs** - Uses `git merge-base` so diffs only show changes introduced by the current branch, matching GitHub/GitLab PR behavior
- **Thread-based comments** - Anchor comments to specific lines, track conversations with the agent
- **Side-by-side diff** - Native `:diffthis` view with syntax highlighting in a separate tabpage
- **Directory folding** - Collapse/expand directories in the file panel with `<CR>`
- **Review status tracking** - Mark files as approved, needs-changes, or unreviewed; persisted across sessions
- **Hunk acceptance checklist** - Accept individual hunks to track progress within a file; auto-approves the file when all hunks are accepted
- **Auto-resolve** - Simple feedback that auto-approves once the agent applies the change
- **Self-review** - Agent reviews its own diff and flags uncertainties as threads
- **Thread filters** - View all threads, or filter by agent-created, user-created, or resolved
- **Live polling** - Automatically refreshes diffs and file lists on a configurable interval
- **Session persistence** - Review state, threads, and session history saved to disk
- **Per-workspace config** - Override the default comparison branch per repository
- **Backend shim** - Transparent support for both Cursor CLI and Claude Code CLI
- **Process cleanup** - Agent CLI processes are killed when Neovim exits; no orphaned processes

## Workflow

Arbiter is designed for a specific loop: an AI agent writes code, you review it, you give feedback, the agent revises, and you review again. This is fundamentally different from reviewing human-written code - you didn't write any of it, so you need to understand intent, verify correctness, and steer the agent, all without leaving Neovim.

### The review loop

1. **The agent works.** You give Cursor or Claude Code a task. It writes code across multiple files.

2. **You open the workbench.** `:Arbiter main` opens a dedicated review tabpage. The left panel shows changed files (like a PR file list). The right panel shows the diff, starting on the first file you haven't approved yet. Only changes introduced by your branch appear - arbiter uses `git merge-base` so the diff matches what a GitHub/GitLab PR would show.

3. **You review file by file.** Select files in the left panel with `<CR>`. Jump between hunks with `]c`/`[c`. Collapse directories you don't care about. Use `<Leader>s` for a side-by-side view when you need it.

4. **You give feedback.** Press `<Leader>ac` on any line to leave a comment. A thread opens immediately and your comment is sent to the agent. The agent's response streams in real-time. This is the core interaction - every piece of feedback is a thread anchored to a specific line, just like a PR review comment.

5. **The agent revises.** The agent reads your feedback and makes changes. Arbiter polls the filesystem and updates the diff automatically - you see changes appear without refreshing.

6. **You track progress.** Mark files as approved (`<Leader>aa`) or needs-changes (`<Leader>ax`) as you go. Accept individual hunks with `<Leader>as` to track your progress within a file - when all hunks are accepted, the file is auto-approved. Use `<Leader>an`/`<Leader>ap` to jump between files you haven't reviewed yet. Run `:ArbiterSummary` for a summary of where you stand.

7. **Repeat.** Continue reviewing, commenting, and approving until the changeset looks right. Close the workbench with `q` when you're done. Your review state (approvals, threads, conversations) is persisted to disk and restored if you reopen.

### Quick feedback with auto-resolve

For simple requests like "rename this variable" or "add a docstring here", use `<Leader>aA` instead of `<Leader>ac`. This creates a thread that auto-resolves once the agent applies the change - you don't have to manually close it.

### Agent self-review

Before you start reviewing, run `:ArbiterSelfReview`. The agent reviews its own diff and flags anything it's uncertain about. Its concerns appear as threads anchored to the relevant lines, giving you a head start on where to focus.

### Catching up

If you step away and come back, `:ArbiterCatchUp` asks the agent to summarize what it's done. `:ArbiterList` shows saved sessions you can resume.

### Side-by-side diff

Press `<Leader>s` on any file to open a side-by-side diff in a new tabpage using Neovim's native `:diffthis`. The left buffer shows the file at the merge-base, the right shows the working copy. Both get syntax highlighting. Press `<Leader>s` again (or `:tabclose`) to return.

### Changing the comparison branch

The default comparison branch is set in your config (globally or per-workspace). You can also change it on the fly:

- `:ArbiterRef develop` - switch to comparing against `develop`
- `:ArbiterRef` - clear the base (diff against working tree)

See [Per-workspace ref override](#per-workspace-ref-override) for configuring defaults per repository.

## Requirements

| Dependency | Version | Notes |
|------------|---------|-------|
| Neovim | 0.10+ | Uses `vim.uv`, `vim.fs`, and nvim-oxi 0.11 API features |
| Rust toolchain | stable | `cargo` and `rustc` must be on `$PATH` to compile the native library |
| Git | any recent | The plugin shells out to `git` for diffs, merge-base, file lists, etc. |
| Cursor CLI **or** Claude Code CLI | | At least one: `cursor` (via Cursor editor) or `claude` (via `npm install -g @anthropic-ai/claude-code`) |

**Platform support:** macOS and Linux. No Windows support.

**State directory:** Review state, threads, and sessions are persisted to `~/.local/share/nvim/arbiter/` by default. Override with `review.state_dir` in config.

## Installation

### lazy.nvim

```lua
return {
  "tannaurus/nvim-arbiter",
  tag = "v0.0.2", -- pin to a release tag
  build = function()
    require("arbiter.build").download_or_build_binary()
  end,
  opts = {
    backend = "cursor",
    review = {
      default_ref = "main",
    },
  },
}
```

When the plugin is installed or updated to a tagged commit, the `build` function first tries to download a prebuilt binary from the matching GitHub Release. If no prebuilt is available, it falls back to `cargo build --release`. If the library isn't found at load time, the plugin triggers this process automatically. You can rebuild at any time with lazy.nvim's `gb` key.

### packer.nvim

```lua
use {
  "tannaurus/nvim-arbiter",
  run = "cargo build --release",
  config = function()
    require("arbiter").setup({
      backend = "cursor",
      review = { default_ref = "main" },
    })
  end,
}
```

### Manual

1. Clone the repo into your Neovim packages directory or anywhere on your `runtimepath`:

```bash
git clone https://github.com/tannaurus/nvim-arbiter.git ~/.local/share/nvim/site/pack/plugins/start/arbiter
cd ~/.local/share/nvim/site/pack/plugins/start/arbiter
cargo build --release
```

2. Add to your `init.lua`:

```lua
require("arbiter").setup({
  backend = "cursor",
  review = {
    default_ref = "main",
  },
})
```

### How the native library loads

On install or update, the `build` hook calls `arbiter.build.download_or_build_binary()` which:

1. **Tries to download a prebuilt binary** from GitHub Releases matching the current git tag (e.g. `v0.1.0`). Binaries are available for Linux (glibc/musl, x86_64/aarch64) and macOS (x86_64/aarch64). If the current commit is not a tagged release, the download is skipped.
2. **Validates the download** by loading it with `package.loadlib` before replacing the current binary (atomic `.tmp` rename).
3. **Falls back to `cargo build --release`** if no prebuilt binary is available or the download fails.

At load time, `lua/arbiter/init.lua` searches multiple paths for the compiled library:
- `target/release/libarbiter.{dylib,so}` (relative to plugin root)
- `$CARGO_TARGET_DIR/release/libarbiter.{dylib,so}` (if set)

If no library is found, it triggers the download-or-build process automatically.

The library is loaded directly from the build output via `package.loadlib` (not Lua `require`), which avoids macOS code signature invalidation from file copies. You can rebuild at any time with lazy.nvim's `gb` key.

### Health check

Run `:checkhealth arbiter` to verify your installation. It checks:
- Binary exists and loads correctly
- `cargo`, `git`, and a backend CLI are on `$PATH`
- All library search paths

## Configuration

All fields are optional. Missing fields use the defaults shown below.

```lua
require("arbiter").setup({
  -- Backend CLI: "cursor" or "claude"
  backend = "cursor",

  -- Optional model override passed to the CLI
  model = nil,

  -- Workspace root (defaults to cwd)
  workspace = nil,

  -- Show thread indicators in the sign column of normal editing buffers
  inline_indicators = false,

  review = {
    -- Git ref to diff against (e.g. "main", "develop").
    -- Uses merge-base so only branch changes appear.
    -- nil = diff against working tree with no base.
    default_ref = nil,

    -- Open in side-by-side view by default
    side_by_side = false,

    -- Automatically fold approved hunks
    fold_approved = false,

    -- Seconds before auto-resolve comments are accepted
    auto_resolve_timeout = 60,

    -- File content poll interval (ms)
    poll_interval = 2000,

    -- File list refresh interval (ms)
    file_list_interval = 5000,

    -- Where to persist review state.
    -- Default: ~/.local/share/nvim/arbiter
    state_dir = nil,
  },

  -- Thread panel appearance
  thread_window = {
    -- Split direction: "right", "left", "top", "bottom"
    position = "right",

    -- Panel size in columns (left/right) or lines (top/bottom)
    size = 60,

    -- chrono format string for message timestamps
    -- See https://docs.rs/chrono/latest/chrono/format/strftime/
    date_format = "%Y-%m-%d %H:%M",
  },

  prompts = {
    -- Prompt sent by :ArbiterCatchUp
    catch_up = "Summarize the changes you've made and the current state of the project.",

    -- Review guidance sent by :ArbiterSelfReview.
    -- Format instructions (THREAD|file|line|message) are appended automatically.
    self_review = "Review this diff and flag anything you're uncertain about or want feedback on.",
  },

  -- Extra CLI flags appended to every backend invocation.
  extra_args = {},

  -- Per-workspace overrides, keyed by absolute directory path.
  -- The longest matching path wins. Useful when a repo uses a
  -- non-standard primary branch.
  workspaces = {
    ["/path/to/repo"] = {
      default_ref = "develop",
    },
  },

  -- All keymaps can be overridden. These are the defaults:
  keymaps = {
    next_hunk = "]c",
    prev_hunk = "[c",
    next_file = "]f",
    prev_file = "[f",
    next_thread = "]t",
    prev_thread = "[t",
    toggle_side_by_side = "<Leader>s",
    approve = "<Leader>aa",
    needs_changes = "<Leader>ax",
    reset_status = "<Leader>ar",
    comment = "<Leader>ac",
    auto_resolve = "<Leader>aA",
    open_thread = "<Leader>ao",
    list_threads = "<Leader>aT",
    list_threads_agent = "<Leader>aTa",
    list_threads_user = "<Leader>aTu",
    list_threads_binned = "<Leader>aTb",
    list_threads_open = "<Leader>aTo",
    resolve_thread = "<Leader>aR",
    toggle_resolved = "<Leader>a?",
    re_anchor = "<Leader>aP",
    refresh = "<Leader>aU",
    cancel_request = "<Leader>aK",
    next_unreviewed = "<Leader>an",
    prev_unreviewed = "<Leader>ap",
    accept_hunk = "<Leader>as",
    file_back = "<C-o>",
  },
})
```

### Configuration reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `backend` | `string` | `"cursor"` | `"cursor"` or `"claude"` |
| `model` | `string?` | `nil` | Model override passed to the CLI |
| `workspace` | `string?` | cwd | Workspace root |
| `inline_indicators` | `bool` | `false` | Show thread signs in normal buffers |
| `review.default_ref` | `string?` | `nil` | Global default git ref for diffs |
| `review.side_by_side` | `bool` | `false` | Start in side-by-side view |
| `review.fold_approved` | `bool` | `false` | Fold approved hunks |
| `review.auto_resolve_timeout` | `number` | `60` | Auto-resolve timeout (seconds) |
| `review.poll_interval` | `number` | `2000` | File content poll interval (ms) |
| `review.file_list_interval` | `number` | `5000` | File list refresh interval (ms) |
| `review.state_dir` | `string?` | `~/.local/share/nvim/arbiter` | State persistence directory |
| `thread_window.position` | `string` | `"right"` | Split direction: `"right"`, `"left"`, `"top"`, `"bottom"` |
| `thread_window.size` | `number` | `60` | Panel size in columns (left/right) or lines (top/bottom) |
| `thread_window.date_format` | `string` | `"%Y-%m-%d %H:%M"` | Timestamp format ([chrono strftime](https://docs.rs/chrono/latest/chrono/format/strftime/)) |
| `extra_args` | `string[]` | `{}` | Extra CLI flags appended to every backend invocation |
| `workspaces` | `table` | `{}` | Per-workspace overrides (see above) |

### Per-workspace ref override

When a repository uses a branch other than `main` as its primary branch, configure it in `workspaces`. Keys can be **absolute paths** or **regex patterns**:

```lua
workspaces = {
  ["~/work/my-project"] = {
    default_ref = "trunk",
  },

  -- Regex: match all repos under a specific org directory
  ["/Users/me/work/acme/.*"] = {
    default_ref = "develop",
  },
}
```

**Matching rules:**

- Keys starting with `/` or `~` are treated as **literal path prefixes**. The longest prefix match wins.
- All other keys are compiled as **regex patterns** and tested against the full canonical directory path. The longest pattern string wins among regex matches.
- Literal path matches always take priority over regex matches.

**Resolution order** when opening a review (`:Arbiter` with no argument):

1. Explicit argument (`:Arbiter some-branch`)
2. Longest-matching literal path override from `workspaces`
3. Longest-matching regex override from `workspaces`
4. Global `review.default_ref`
5. No base (diffs against working tree)

You can also change the ref on the fly during an active review with `:ArbiterRef`.

### Backend permissions

When arbiter sends feedback to the agent, the agent may need to run shell commands (e.g. `git`, `cargo fmt`) to apply changes. By default, both Cursor and Claude Code require interactive approval for shell commands. Since arbiter runs the CLI non-interactively, the agent will simply report that it cannot execute the command rather than prompting you.

**Recommended:** Configure your backend's built-in allowlists rather than disabling permissions entirely.

**Cursor CLI** - Create or edit `~/.cursor/cli.json`:

```json
{
  "enabledTools": ["shell"],
  "allowedCommands": ["git", "cargo fmt", "rustfmt"]
}
```

**Claude Code** - See the [Claude Code docs](https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/overview) for configuring allowed tools.

Using `--yolo` (Cursor) or `--dangerously-skip-permissions` (Claude) via `extra_args` is discouraged. These flags allow the agent to run arbitrary commands without approval, including destructive operations like `rm -rf` or `git push --force`.

## Commands

### Global commands (available anytime)

| Command | Description |
|---------|-------------|
| `:Arbiter [ref]` | Open the review workbench. Optional ref overrides the default. |
| `:ArbiterSend <prompt>` | Send a prompt to the agent. Response streams into a panel. |
| `:ArbiterContinue [prompt]` | Continue the current session with an optional follow-up. |
| `:ArbiterCatchUp` | Ask the agent to summarize where it left off. |
| `:ArbiterList` | List saved sessions in a floating window. `<CR>` to select. |
| `:ArbiterResume <id> [prompt]` | Resume a specific session by ID. |

### Review commands (require an active review)

| Command | Description |
|---------|-------------|
| `:ArbiterRef [branch]` | Change the comparison branch on the fly. No argument clears the base. |
| `:ArbiterActiveThread` | Open the thread window for the agent that is currently thinking. |
| `:ArbiterSelfReview` | Run agent self-review on the current diff. Creates agent threads. |
| `:ArbiterRefresh` | Refresh the file list and current file diff. |
| `:ArbiterResolveAll` | Resolve all open threads. |
| `:ArbiterSummary` | Show review summary popup (file/thread counts). |

## Keybindings

All keybindings are active in the review workbench tabpage and are fully configurable via the `keymaps` config table.

### Navigation

| Default | Action |
|---------|--------|
| `]c` / `[c` | Next / previous hunk (scrolls hunk into view) |
| `]f` / `[f` | Next / previous file |
| `]t` / `[t` | Next / previous open thread (skips resolved, crosses files) |

### Review status

| Default | Action |
|---------|--------|
| `<Leader>aa` | Toggle approval on current file (or resolve thread if cursor is on a thread summary) |
| `<Leader>ax` | Mark as needs-changes |
| `<Leader>ar` | Reset to unreviewed |
| `<Leader>as` | Accept/unaccept the hunk under the cursor (auto-approves file when all hunks accepted) |
| `<Leader>an` | Jump to next unreviewed file |
| `<Leader>ap` | Jump to previous unreviewed file |

### Comments and threads

| Default | Action |
|---------|--------|
| `<Leader>ac` | Add a comment and send to the agent. Opens the thread window with streaming response. |
| `<Leader>aA` | Add a comment with auto-resolve (auto-approves once the agent applies the change). |
| `<Leader>ao` | Open the thread conversation at the cursor. |
| `<Leader>aT` | List all threads in a quickfix list. |
| `<Leader>aTa` | List agent-created threads only. |
| `<Leader>aTu` | List user-created threads only. |
| `<Leader>aTb` | List resolved (binned) threads only. |
| `<Leader>aTo` | List open (unresolved) threads only. |
| `<Leader>aR` | Resolve the thread at the cursor. |
| `<Leader>a?` | Toggle display of resolved threads. |
| `<Leader>aP` | Re-anchor a thread to the current cursor position. |
| `<Leader>aK` | Cancel all pending backend requests. |

### Other

| Default | Action |
|---------|--------|
| `<Leader>s` | Toggle side-by-side diff view. |
| `<Leader>aU` | Refresh file list and current diff. |
| `<C-o>` | Navigate back through file history (works across file jumps, thread jumps, and auto-advance). |
| `q` | Close the review workbench. |

### File panel

| Key | Action |
|-----|--------|
| `<CR>` | Select file, or toggle directory collapse. |

### Thread quickfix list

When a thread list is open (via `<Leader>aT` and variants):

| Key | Action |
|-----|--------|
| `<CR>` | Open the thread detail window and navigate the diff panel to that file/line. |
| `dd` | Remove the entry from the quickfix list. |
| `gf` | Jump to the source file at the thread's line. |

### Comment input float

When the input float opens (via `<Leader>ac` or `<Leader>aA`), you're placed in Insert mode:

| Key | Mode | Action |
|-----|------|--------|
| (type normally) | Insert | Write your comment. Enter adds a newline. |
| `Esc` | Insert | Exit to Normal mode. |
| `Enter` | Normal | Submit the comment. |
| `q` / `Esc` | Normal | Cancel and close the float. |

### Thread detail window

When a thread conversation is open:

| Key | Action |
|-----|--------|
| `<CR>` | Reply to the thread (opens input float). |
| `q` | Close the thread window. |

## Build from source

```bash
task build
```

Or directly:

```bash
cargo build --release
```

Output: `target/release/libarbiter.dylib` (macOS) or `libarbiter.so` (Linux).

Other tasks: `task install`, `task test`, `task lint`, `task fmt`, `task check` (runs all three).

## Statusline

The plugin exposes a statusline component that shows backend activity. Call it from your statusline config:

```lua
-- lualine example
lualine_x = {
  { function() return require("arbiter").statusline() end },
}

-- Plain statusline
vim.o.statusline = vim.o.statusline .. " %{v:lua.require('arbiter').statusline()}"
```

When the agent is processing a request, the component shows a spinner with elapsed time (e.g. `⠋ thinking 5s`). With an active review, it shows progress (e.g. `[REVIEW 2/5]`). When idle with no review, it returns an empty string.

## Architecture

The plugin is written in Rust using `nvim-oxi` for typed bindings to Neovim's C API. Key modules:

- `backend/` - CLI adapter shim (Cursor, Claude) with FIFO queue and streaming support
- `diff/` - Unified diff parser and buffer renderer
- `threads/` - Thread CRUD, anchoring, re-anchoring, filtering, and thread panel
- `review.rs` - Core review workbench state and UI orchestration
- `dispatch.rs` - Safe cross-thread callback dispatch via `libuv::AsyncHandle`
- `git.rs` - Async git operations (merge-base, diff, show, stash)
- `state.rs` - JSON persistence of review state, threads, and sessions
- `config.rs` - Configuration deserialization with per-workspace overrides
- `file_panel.rs` - Tree rendering with directory folding
- `poll.rs` - Periodic file and file-list refresh via libuv timers
- `activity.rs` - Backend busy/idle tracking for statusline display
- `highlight.rs` - Custom highlight groups and sign definitions
