# arbiter

[![CI](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml/badge.svg)](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tannaurus/nvim-arbiter?style=flat&label=release&include_prereleases)](https://github.com/tannaurus/nvim-arbiter/releases/latest)
[![Neovim](https://img.shields.io/badge/neovim-0.10%2B-57A143?logo=neovim&logoColor=white)](https://neovim.io)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)]()

> **Experimental** - This plugin is under active development. APIs, commands, and keymaps may change without notice.

Review workbench for Neovim. PR-style diffs, line-anchored threads, and a structured feedback loop with AI coding agents. Built in Rust with [nvim-oxi](https://github.com/noib3/nvim-oxi). Works with Cursor CLI and Claude Code CLI.

## Why arbiter

Agents can produce a 30-file changeset in minutes. Reviewing it shouldn't take all day.

A chat window is one big conversation. Imagine doing a PR review where every comment to the author went into a single thread. You'd constantly be saying "going back to that thing on line 42..." and hoping they follow. PR reviews solved this with threads: each comment is its own scoped conversation anchored to a specific line, and everyone knows what's being discussed. Arbiter gives agents the same structure.

In the best code reviews, the author doesn't just fix what you point out. They pick up on the pattern and apply it across the whole changeset. Review memory brings that to agents: conventions you enforce in one thread get extracted and fed into every subsequent prompt, so the agent applies your preferences before you ask. This isn't a replacement for skills or system prompts. It supplements them with things you notice during the review itself. For example:

- "We're moving off callbacks to async/await in this refactor. Don't introduce new callback-style code." Specific to this effort, not a forever rule.
- "The design doc says these endpoints return 201 for creates, not 200. Apply that to all the new handlers." A spec decision for this feature, not a global convention.
- "This module returns `Option` not `Result` since absence isn't an error here." A recent design call that hasn't been codified anywhere.

These are decisions made during or around the review. They'd rot in a skill file, but for the next 20 threads in this session you want the agent to know them.

## Features

- **PR-style diffs** -- Dedicated tabpage with file panel and diff viewer, scoped to the current branch via `git merge-base`.
- **Line-anchored threads** -- Comment on a diff line, get a streaming response. Threads persist across sessions.
- **Review memory** -- Conventions you enforce get extracted and fed into future prompts automatically.
- **Progress tracking** -- Approve files, accept hunks, filter threads by status. State saved to disk.
- **Self-review** -- The agent reviews its own diff and flags concerns before you start.
- **Live diffs** -- Filesystem polling picks up the agent's changes without manual refresh.
- **Auto-resolve** -- Simple feedback ("rename this") resolves itself once the agent applies the fix.
- **Session persistence** -- Review state, threads, and conversations restored when you reopen.

## Workflow

Arbiter is designed for a specific loop: an AI agent writes code, you review it, you give feedback, the agent revises, and you review again.

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
  tag = "v0.0.4", -- pin to a release tag
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
  -- "cursor" or "claude". Determines which CLI binary is invoked.
  backend = "cursor",

  -- Model name passed to the backend CLI (e.g. "claude-sonnet-4-20250514").
  -- nil uses the backend's default model.
  model = nil,

  -- Absolute path to the project root. Passed as the working directory
  -- for all git and backend CLI operations. Defaults to cwd at setup time.
  workspace = nil,

  -- When true, places sign-column markers in normal editing buffers
  -- at lines that have an active thread. Clicking a marker opens the thread.
  inline_indicators = false,

  -- When true, resolving a thread sends the conversation to the agent to
  -- extract generalizable coding conventions. Extracted rules are injected
  -- into future thread prompts so the agent learns from your feedback.
  -- Each extraction costs one additional backend call per agent response.
  -- Toggle at runtime with :ArbiterToggleRules. View/edit with :ArbiterRules.
  learn_rules = true,

  review = {
    -- Git ref to diff against (e.g. "main", "develop").
    -- The diff uses merge-base so only your branch's changes appear.
    -- nil = diff unstaged working-tree changes with no base ref.
    default_ref = nil,

    -- Start the review workbench in side-by-side (vertical split) mode
    -- instead of unified diff.
    side_by_side = false,

    -- Automatically fold hunks that have been accepted via the accept_hunk
    -- keymap. Folded hunks are dimmed and collapsed in the diff panel.
    fold_approved = false,

    -- Seconds to wait before auto-resolve comments are marked resolved.
    -- Auto-resolve comments (sent via the auto_resolve keymap) are
    -- accepted automatically once the agent applies the requested change
    -- and this timeout elapses without further edits.
    auto_resolve_timeout = 60,

    -- How often (ms) to poll the current file for on-disk changes and
    -- re-render the diff panel.
    poll_interval = 2000,

    -- How often (ms) to refresh the file list panel to pick up new,
    -- deleted, or renamed files.
    file_list_interval = 5000,

    -- Directory for persisting review state, threads, and session history.
    -- Each workspace gets a subdirectory keyed by a hash of its path.
    -- Default: ~/.local/share/nvim/arbiter
    state_dir = nil,
  },

  thread_window = {
    -- Where the thread conversation panel opens relative to the diff.
    -- "right", "left", "top", or "bottom".
    position = "right",

    -- Panel width in columns (left/right) or height in lines (top/bottom).
    size = 60,

    -- strftime format string for message timestamps in the thread panel.
    -- See https://docs.rs/chrono/latest/chrono/format/strftime/
    date_format = "%Y-%m-%d %H:%M",
  },

  prompts = {
    -- Prompt sent by :ArbiterCatchUp. Useful for resuming after a break.
    catch_up = "Summarize the changes you've made and the current state of the project.",

    -- Review direction sent by :ArbiterSelfReview. Controls what the agent
    -- looks for (tone, scope, strictness). The THREAD|file|line|message
    -- output format instructions are appended automatically.
    self_review = "Review this diff and flag anything you're uncertain about or want feedback on.",
  },

  -- Extra CLI flags appended verbatim to every backend invocation.
  -- Useful for backend-specific options like --dangerously-skip-permissions
  -- (Claude) or --yolo (Cursor). Use with caution.
  extra_args = {},

  -- Per-workspace overrides keyed by absolute path or regex pattern.
  -- See "Per-workspace ref override" below for matching rules.
  workspaces = {
    ["/path/to/repo"] = {
      default_ref = "develop",
    },
  },

  -- All keymaps accept Neovim notation (e.g. "<Leader>s", "<C-o>", "]c").
  -- Only active inside the review workbench tabpage.
  keymaps = {
    next_hunk = "]c",             -- Jump to next diff hunk
    prev_hunk = "[c",             -- Jump to previous diff hunk
    next_file = "]f",             -- Next file in the file list
    prev_file = "[f",             -- Previous file in the file list
    next_thread = "]t",           -- Next open thread (crosses files)
    prev_thread = "[t",           -- Previous open thread (crosses files)
    toggle_side_by_side = "<Leader>s",  -- Toggle unified / side-by-side view
    approve = "<Leader>aa",       -- Approve file (or resolve thread at cursor)
    needs_changes = "<Leader>ax", -- Mark file as needs-changes
    reset_status = "<Leader>ar",  -- Reset file to unreviewed
    comment = "<Leader>ac",       -- Comment on the line and send to the agent
    auto_resolve = "<Leader>aA",  -- Comment with auto-resolve on agent fix
    open_thread = "<Leader>ao",   -- Open thread conversation at cursor
    list_threads = "<Leader>at",  -- Thread list popup (grouped by status)
    list_threads_agent = "<Leader>ata", -- Thread list (agent threads only)
    list_threads_user = "<Leader>atu",  -- Thread list (user threads only)
    list_threads_binned = "<Leader>atb", -- Thread list (binned only)
    list_threads_open = "<Leader>ato",   -- Thread list (open only)
    resolve_thread = "<Leader>aR",       -- Resolve/reopen thread at cursor
    toggle_resolved = "<Leader>a?",      -- Toggle display of resolved threads
    re_anchor = "<Leader>aP",     -- Re-anchor thread to current cursor line
    refresh = "<Leader>aU",       -- Refresh file list and current diff
    cancel_request = "<Leader>aK", -- Cancel all pending backend requests
    next_unreviewed = "<Leader>an", -- Jump to next unreviewed file
    prev_unreviewed = "<Leader>ap", -- Jump to previous unreviewed file
    accept_hunk = "<Leader>as",   -- Accept/unaccept hunk under cursor
    file_back = "<C-o>",          -- Navigate back through file history
  },
})
```

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
  "allowedCommands": ["git", "cargo fmt", "cargo clippy", "rustfmt"]
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
| `:ArbiterOpenThread <file> <line>` | Open the thread at the given file and line number. |
| `:ArbiterResolveAll` | Resolve all open threads. |
| `:ArbiterSummary` | Show review summary popup (file/thread counts). |
| `:ArbiterRules` | Open an editable popup with the current review rules. `:w` saves, `q` closes. |
| `:ArbiterToggleRules` | Toggle automatic rule extraction on thread resolution. |

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
| `<Leader>at` | Open thread list popup (grouped by status). |
| `<Leader>ata` | Open thread list filtered to agent-created threads. |
| `<Leader>atu` | Open thread list filtered to user-created threads. |
| `<Leader>atb` | Open thread list filtered to binned threads. |
| `<Leader>ato` | Open thread list filtered to open threads. |
| `<Leader>aR` | Resolve the thread at the cursor. |
| `<Leader>a?` | Toggle display of resolved threads. |
| `<Leader>aP` | Re-anchor a thread to the current cursor position. |
| `<Leader>aK` | Cancel all pending backend requests. |

### Other

| Default | Action |
|---------|--------|
| `<CR>` | Open the thread at the cursor line (or jump to source if no thread). |
| `<Leader>s` | Toggle side-by-side diff view. |
| `<Leader>aU` | Refresh file list and current diff. |
| `<C-o>` | Navigate back through file history (works across file jumps, thread jumps, and auto-advance). |
| `q` | Close the review workbench. |

### File panel

| Key | Action |
|-----|--------|
| `<CR>` | Select file, or toggle directory collapse. |

### Thread list popup

When the thread list popup is open (via `<Leader>at` and variants):

| Key | Action |
|-----|--------|
| `<CR>` | Navigate to the thread's file/line and open the thread window. |
| `dd` | Resolve the thread (Open/Binned) or permanently delete it (Resolved). |
| `q` / `Esc` | Close the popup. |

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
