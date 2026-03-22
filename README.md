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

- **PR-style diffs** -- Dedicated tabpage with file panel and diff viewer. Diff against a branch via `git merge-base`, or diff unstaged working tree changes.
- **Line-anchored threads** -- Comment on a diff line, get a streaming response. Threads persist across sessions.
- **Review memory** -- Conventions you enforce get extracted and fed into future prompts automatically.
- **Project rules** -- Persistent, file-aware instructions loaded from markdown files with TOML frontmatter. Scope rules to specific file types and scenarios so the agent gets the right context for every prompt. See [Project rules](#project-rules).
- **Similar threads** -- After self-review, a similarity pass groups threads that flag the same class of issue. Cross-references appear in the thread panel so you can navigate related feedback.
- **Progress tracking** -- Approve files, accept hunks, filter threads by status. In working tree mode, accepting hunks stages them in git. State saved to disk.
- **Self-review** -- The agent reviews its own diff and flags concerns before you start.
- **Prompt panel** -- Long-lived agent conversations in a floating window. Multiple named conversations maintain independent context across the review. `:ArbiterPrompt` opens the default conversation; `:ArbiterPrompt security review` opens a named one.
- **Live diffs** -- Filesystem polling picks up the agent's changes without manual refresh.
- **Auto-resolve** -- Simple feedback ("rename this") resolves itself once the agent applies the fix.
- **Session persistence** -- Review state, threads, and conversations restored when you reopen. Stale caches from older plugin versions are automatically discarded.

## Workflow

Arbiter works in two modes depending on how you open it:

- **Working tree review** (`:Arbiter`) - Diffs unstaged changes against HEAD. Use this when you're iterating with an agent in real time and haven't committed yet. You see exactly what the agent has changed since your last commit. Accepting hunks (`<Leader>as`) and approving files (`<Leader>aa`) stage changes in git, so you can build up a commit as you review. Unstaging only reverses what Arbiter staged; pre-existing staged content is preserved.
- **Branch review** (`:ArbiterCompare main`) - Diffs your current branch against a base ref using `git merge-base`, so you only see changes introduced by your branch. This matches what a GitHub/GitLab PR would show. Use this when you're reviewing a full feature branch before merging. Accepting hunks and approving files in this mode is visual-only (no git staging). If you set `review.default_ref` in your config, `:ArbiterCompare` with no argument uses that ref.

Both modes use the same workbench, threads, and feedback loop. The only difference is what the diff is computed against.

### The review loop

1. **The agent works.** You give Cursor or Claude Code a task. It writes code across multiple files.

2. **You open the workbench.** `:Arbiter` opens a review tabpage for unstaged changes, or `:ArbiterCompare main` diffs against a branch. The left panel shows changed files (like a PR file list). The right panel shows the diff, starting on the first file you haven't approved yet.

3. **You review file by file.** Select files in the left panel with `<CR>`. Jump between hunks with `]c`/`[c`. Collapse directories you don't care about. Use `<Leader>s` for a side-by-side view when you need it.

4. **You give feedback.** Press `<Leader>ac` on any line to leave a comment. A thread opens immediately and your comment is sent to the agent. The agent's response streams in real-time. This is the core interaction - every piece of feedback is a thread anchored to a specific line, just like a PR review comment.

5. **The agent revises.** The agent reads your feedback and makes changes. Arbiter polls the filesystem and updates the diff automatically - you see changes appear without refreshing.

6. **You track progress.** Mark files as approved (`<Leader>aa`) or needs-changes (`<Leader>ax`) as you go. Accept individual hunks with `<Leader>as` to track your progress within a file - when all hunks are accepted, the file is auto-approved. In working tree mode, accepting a hunk stages it in git (`git apply --cached`); toggling it back unstages only that hunk, leaving any pre-existing staged content untouched. In branch review mode, acceptance is visual-only. Use `<Leader>an`/`<Leader>ap` to jump between files you haven't reviewed yet. Run `:ArbiterSummary` for a summary of where you stand.

7. **Repeat.** Continue reviewing, commenting, and approving until the changeset looks right. Close the workbench with `q` when you're done. Your review state (approvals, threads, conversations) is persisted to disk and restored if you reopen.

### Quick feedback with auto-resolve

For simple requests like "rename this variable" or "add a docstring here", use `<Leader>aA` instead of `<Leader>ac`. This creates a thread that auto-resolves once the agent applies the change - you don't have to manually close it.

### Agent self-review

Before you start reviewing, run `:ArbiterSelfReview`. The agent reviews its own diff and flags anything it's uncertain about. Its concerns appear as threads anchored to the relevant lines, giving you a head start on where to focus.

To have the agent act on all its own feedback at once, run `:ArbiterApply`. This bundles every open self-review thread into a single prompt telling the agent to fix them all. Each thread is marked as auto-resolve, so they'll close automatically once the agent applies the changes.

### Catching up

If you step away and come back, `:ArbiterCatchUp` asks the agent to summarize what it's done. `:ArbiterList` shows saved sessions you can resume.

### Side-by-side diff

Press `<Leader>s` on any file to open a side-by-side diff in a new tabpage using Neovim's native `:diffthis`. The left buffer shows the file at the merge-base, the right shows the working copy. Both get syntax highlighting. Press `<Leader>s` again (or `:tabclose`) to return.

### Changing the comparison branch

The default comparison branch is set in your config (globally or per-workspace). You can also change it on the fly:

- `:ArbiterRef develop` - switch to comparing against `develop`
- `:ArbiterRef` - clear the base (switch to working tree mode)

See [Per-workspace ref override](#per-workspace-ref-override) for configuring defaults per repository.

## Requirements

| Dependency | Version | Notes |
|------------|---------|-------|
| Neovim | 0.10+ | Uses `vim.uv`, `vim.fs`, and nvim-oxi 0.11 API features |
| Rust toolchain | stable | `cargo` and `rustc` must be on `$PATH` to compile the native library |
| Git | any recent | The plugin shells out to `git` for diffs, merge-base, file lists, etc. |
| Cursor CLI **or** Claude Code CLI | | At least one: `cursor` (via Cursor editor) or `claude` (via `npm install -g @anthropic-ai/claude-code`) |
| nvim-tree | recommended | Recommended for the file panel. A basic builtin tree ships by default, but nvim-tree provides file icons, review status signs, and familiar keybindings. See [Using nvim-tree](#using-nvim-tree). |

**Platform support:** macOS and Linux. No Windows support.

**State directory:** Review state, threads, and sessions are persisted to `~/.local/share/nvim/arbiter/` by default. Override with `review.state_dir` in config. Persisted state is version-stamped; upgrading the plugin automatically discards stale caches.

## Installation

### lazy.nvim

```lua
return {
  "tannaurus/nvim-arbiter",
  tag = "v0.0.7", -- pin to a release tag
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

1. **Tries to download a prebuilt binary** from GitHub Releases matching the current git tag (e.g. `v0.1.0`). Binaries are available for Linux (glibc, x86_64/aarch64) and macOS (x86_64/aarch64). If the current commit is not a tagged release, the download is skipped.
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

### Plugin integrations

#### nvim-tree

Arbiter ships a basic builtin file panel, but [nvim-tree](https://github.com/nvim-tree/nvim-tree.lua) is the recommended file panel for most users. It provides file-type icons, review status signs (approved, needs changes, unreviewed), collapsible directories with familiar keybindings, and automatic filtering to show only changed files during a review.

To enable it, set `file_panel = "nvim-tree"` in your arbiter config and wire arbiter's filter into your nvim-tree setup:

```lua
require("nvim-tree").setup({
  -- your existing config ...
  filters = {
    custom = require("arbiter.nvim_tree_adapter").filter,
  },
})
```

The filter is context-aware: when no review is active, it returns `false` for everything and nvim-tree behaves normally. When a review is open, it hides files that aren't part of the changeset. The filter is cleared automatically when the review closes.

If you skip the `filters.custom` step, the nvim-tree panel will still work but will show all files in the project, not just changed ones.

#### Statusline

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

### Options

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

  -- File panel implementation: "builtin" or "nvim-tree".
  -- "builtin" renders a simple tree into a scratch buffer (default).
  -- "nvim-tree" is recommended for a richer experience: file icons,
  -- review status signs, and familiar navigation. See "Using nvim-tree".
  file_panel = "builtin",

  -- Review status icons shown in the nvim-tree file panel sign column.
  -- Any string works (Unicode, Nerd Font glyphs, emoji).
  -- Unset fields auto-detect: Nerd Font if nvim-web-devicons is installed,
  -- Unicode otherwise.
  icons = {
    approved = nil,      -- default: "" (nerd) or "✔" (unicode)
    needs_changes = nil,  -- default: "" (nerd) or "✘" (unicode)
    unreviewed = nil,    -- default: "" (nerd) or "○" (unicode)
  },

  -- When true, every agent response triggers an extraction call to distill
  -- generalizable coding conventions from the conversation. Extracted rules
  -- are injected into future thread prompts so the agent learns from your
  -- feedback. Each extraction costs one additional backend call per response.
  -- Toggle at runtime with :ArbiterToggleRules. View/edit with :ArbiterRules.
  learn_rules = true,

  -- Additional directories to search for project rule files.
  -- Searched after the default locations (~/.config/arbiter/rules/ and
  -- .arbiter/rules/ in the workspace).
  rules_dirs = {},

  review = {
    -- Default git ref for :ArbiterCompare (e.g. "main", "develop").
    -- The diff uses merge-base so only your branch's changes appear.
    -- nil = :ArbiterCompare requires an explicit argument.
    default_ref = nil,

    -- Start the review workbench in side-by-side (vertical split) mode
    -- instead of unified diff.
    side_by_side = false,

    -- Automatically fold hunks that have been accepted via the accept_hunk
    -- keymap. Folded hunks are dimmed and collapsed in the diff panel.
    fold_approved = false,

    -- Diff highlighting style:
    --   "full"  - full-line background colors (green/red). Replaces syntax
    --            highlighting with diff colors (GitHub PR style). Default.
    --   "signs" - colored gutter signs only. Preserves the file's syntax
    --            highlighting on the line content. Diff prefix characters
    --            (+/-/space) are stripped from the buffer so treesitter
    --            parses clean source code.
    diff_style = "full",

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
    approve = "<Leader>aa",       -- Approve file (stages all hunks in working tree mode; resolve thread at cursor)
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
    accept_hunk = "<Leader>as",   -- Accept/unaccept hunk under cursor (stages/unstages in working tree mode)
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

**Resolution order** when opening a branch review (`:ArbiterCompare` with no argument):

1. Explicit argument (`:ArbiterCompare some-branch`)
2. Longest-matching literal path override from `workspaces`
3. Longest-matching regex override from `workspaces`
4. Global `review.default_ref`

If none of these resolve, `:ArbiterCompare` shows an error. Use `:Arbiter` for unstaged changes instead.

You can also change the ref on the fly during an active review with `:ArbiterRef`.

### Project rules

Project rules are persistent instructions loaded from disk and injected into agent prompts. They complement learned rules (which are extracted from your review conversations and live only for the session) with durable, version-controllable guidance that applies across sessions and team members.

Use project rules for things like:

- Language or framework conventions that apply to specific file types
- Architecture constraints scoped to certain directories
- Review standards you want enforced during self-review but not in normal thread conversations

#### File format

Each rule is a `.md` file with TOML frontmatter:

```markdown
---
description = "Rust error handling"
match = ["**/*.rs"]
scenarios = ["thread"]
---
Prefer map_err over match for error transformation.
Use the ? operator for propagation. Avoid unwrap and expect.
```

**Frontmatter fields:**

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `description` | yes | string | Human-readable name. Used for deduplication and display in `:ArbiterRules`. |
| `match` | no | string or list | Glob patterns matched against the file path. Omit to match all files. Accepts a single string (`"*.rs"`) or a list (`["**/*.rs", "**/*.toml"]`). |
| `scenarios` | no | list | When this rule applies: `"thread"` (comments and replies), `"self_review"` (`:ArbiterSelfReview`). Omit to apply in all scenarios. Unknown values are ignored. |

The body (everything after the closing `---`) is the instruction text sent to the agent. It can be any length.

#### Search directories

Rules are loaded from three locations, in order:

1. **Global** -- `~/.config/arbiter/rules/`
2. **Workspace** -- `.arbiter/rules/` relative to the project root
3. **Custom** -- any directories listed in the `rules_dirs` config option

Only `.md` files are loaded. Subdirectories are not traversed. Malformed files are silently skipped.

#### Resolution

When Arbiter builds a prompt, it resolves which rules apply based on two filters:

1. **Scenario** -- if the rule specifies `scenarios`, it must include the current context (`thread` or `self_review`). Rules with no `scenarios` field apply everywhere.
2. **File glob** -- if the rule specifies `match` patterns, at least one must match the file path. Rules with no `match` field apply to all files. During self-review (which operates on the whole diff, not a single file), glob-scoped rules are skipped since there is no single file to match against.

Matched rules are formatted and prepended to the agent prompt as a "Project rules" block.

#### Relationship to learned rules

Learned rules (`learn_rules = true`) are extracted from your review conversations and accumulate during a session. They capture conventions you enforce in the moment, like "don't introduce callback-style code in this refactor." They reset when you close the review.

Project rules are the opposite: written ahead of time, versioned in your repo, and applied consistently. The two systems work together. Project rules set the baseline; learned rules adapt to the current review.

#### Commands

| Command | Description |
|---------|-------------|
| `:ArbiterRules` | Open an editable popup showing all active rules (learned + project). `:w` saves, `q` closes. |
| `:ArbiterToggleRules` | Toggle automatic rule extraction on agent responses. |
| `:ArbiterReloadRules` | Re-read project rule files from disk and report the count. Useful after adding or editing rule files without restarting the review. |

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
| `:Arbiter` | Open the review workbench for unstaged working tree changes. |
| `:ArbiterCompare [ref]` | Open the review workbench diffed against a branch. Uses `review.default_ref` if no argument given. |
| `:ArbiterSend <prompt>` | Send a prompt to the agent. Response streams into a panel. |
| `:ArbiterContinue [prompt]` | Continue the current session with an optional follow-up. |
| `:ArbiterCatchUp` | Ask the agent to summarize where it left off. |
| `:ArbiterList` | List saved sessions in a floating window. `<CR>` to select. |
| `:ArbiterResume <id> [prompt]` | Resume a specific session by ID. |

### Review commands (require an active review)

| Command | Description |
|---------|-------------|
| `:ArbiterPrompt [name]` | Toggle the prompt panel. Opens a floating conversation window. Without an argument, opens the default "main" conversation. With a name (may be multiple words), opens or switches to that named conversation. Each conversation maintains independent message history and backend session. |
| `:ArbiterRef [branch]` | Change the comparison branch on the fly. No argument clears the base. |
| `:ArbiterActiveThread` | Open the thread window for the agent that is currently thinking. |
| `:ArbiterSelfReview` | Run agent self-review on the current diff. Creates agent threads. |
| `:ArbiterApply` | Send all open self-review feedback to the agent in a single prompt. Marks each thread as auto-resolve so they close once the agent applies the changes. |
| `:ArbiterRefresh` | Refresh the file list and current file diff. |
| `:ArbiterOpenThread <file> <line>` | Open the thread at the given file and line number. |
| `:ArbiterResolveAll` | Resolve all open threads. |
| `:ArbiterSummary` | Show review summary popup (file/thread counts). |
| `:ArbiterRules` | Open an editable popup with the current review rules. `:w` saves, `q` closes. |
| `:ArbiterToggleRules` | Toggle automatic rule extraction on agent responses. |
| `:ArbiterReloadRules` | Re-read project rule files from disk and report the count. |

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
| `<Leader>aa` | Toggle approval on current file. In working tree mode, stages all hunks on approve and unstages on unapprove. Resolves thread if cursor is on a thread summary. |
| `<Leader>ax` | Mark as needs-changes |
| `<Leader>ar` | Reset to unreviewed |
| `<Leader>as` | Accept/unaccept the hunk under the cursor. In working tree mode, stages/unstages the hunk in git. Auto-approves file when all hunks accepted. |
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
| `<CR>` | Reply to the thread (opens input split below the thread panel). |
| `q` | Close the thread window. |

### Prompt panel

When the prompt panel is open (via `:ArbiterPrompt`):

| Key | Action |
|-----|--------|
| `<CR>` | Open input to send a message. |
| `q` | Close the prompt panel (conversation state is preserved). |

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

## Architecture

The plugin is written in Rust using `nvim-oxi` for typed bindings to Neovim's C API. Key modules:

- `backend/` - CLI adapter shim (Cursor, Claude) with FIFO queue, streaming support, and shared response parsing
- `diff/` - Unified diff parser and buffer renderer
- `threads/` - Thread CRUD, anchoring, re-anchoring, filtering, and thread panel
- `review/` - Core review workbench: lifecycle, keymaps, navigation, hunk acceptance, thread UI, and revision view
- `commands/` - User command registration and self-review orchestration
- `prompt_panel.rs` - Long-lived prompt conversations in a floating window with named sessions
- `panel.rs` - Shared rendering utilities (timestamps, streaming, status lines) used by thread and prompt panels
- `dispatch.rs` - Safe cross-thread callback dispatch via `libuv::AsyncHandle`
- `git.rs` - Async git operations (merge-base, diff, show, stash) and synchronous staging/unstaging
- `state.rs` - JSON persistence of review state, threads, and sessions
- `config.rs` - Configuration deserialization with per-workspace overrides
- `rules.rs` - Scenario-scoped rule system with glob matching and TOML frontmatter
- `file_panel/` - File panel trait and implementations (builtin tree, nvim-tree adapter)
- `poll.rs` - Periodic file and file-list refresh via libuv timers
- `activity.rs` - Backend busy/idle tracking for statusline display
- `highlight.rs` - Custom highlight groups and sign definitions
