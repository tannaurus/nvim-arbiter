---
sidebar_position: 3
title: Configuration
---

# Configuration

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
  -- See the integrations page for nvim-tree setup details.
  file_panel = "builtin",

  -- Review status icons shown in the nvim-tree file panel sign column.
  -- Unset fields auto-detect: Nerd Font if nvim-web-devicons is installed,
  -- Unicode otherwise.
  icons = {
    approved = nil,      -- default: "" (nerd) or "✔" (unicode)
    unreviewed = nil,    -- default: "" (nerd) or "○" (unicode)
  },

  -- When true, every agent response triggers an extraction call to distill
  -- generalizable coding conventions from the conversation. Each extraction
  -- costs one additional backend call per response.
  -- Toggle at runtime with :ArbiterToggleRules. View/edit with :ArbiterRules.
  learn_rules = true,

  -- Additional directories to search for project rule files.
  -- Searched after the default locations (~/.config/arbiter/rules/ and
  -- .arbiter/rules/ in the workspace).
  rules_dirs = {},

  review = {
    -- Default git ref for :ArbiterCompare (e.g. "main", "develop").
    -- nil = :ArbiterCompare requires an explicit argument.
    default_ref = nil,

    -- Start the review workbench in side-by-side mode.
    side_by_side = false,

    -- Automatically fold hunks that have been accepted.
    fold_approved = false,

    -- Diff highlighting style:
    --   "full"  - full-line background colors (GitHub PR style). Default.
    --   "signs" - colored gutter signs only, preserving syntax highlighting.
    diff_style = "full",

    -- Seconds to wait before auto-resolve threads are marked resolved.
    auto_resolve_timeout = 60,

    -- How often (ms) to poll the current file for on-disk changes.
    poll_interval = 2000,

    -- How often (ms) to refresh the file list panel.
    file_list_interval = 5000,

    -- Directory for persisting review state, threads, and session history.
    -- Default: ~/.local/share/nvim/arbiter
    state_dir = nil,
  },

  thread_window = {
    -- Where the thread panel opens: "right", "left", "top", or "bottom".
    position = "right",

    -- Panel width in columns (left/right) or height in lines (top/bottom).
    size = 60,

    -- strftime format string for message timestamps.
    date_format = "%Y-%m-%d %H:%M",
  },

  prompts = {
    -- Direction sent by :ArbiterSelfReview.
    self_review = "Review this diff and flag anything you're uncertain about or want feedback on.",
  },

  -- Extra CLI flags appended verbatim to every backend invocation.
  extra_args = {},

  -- Per-workspace overrides keyed by absolute path or regex pattern.
  workspaces = {
    ["/path/to/repo"] = {
      default_ref = "develop",
    },
  },

  -- Keymaps active inside the review workbench tabpage.
  keymaps = {
    next_hunk = "]c",
    prev_hunk = "[c",
    next_file = "]f",
    prev_file = "[f",
    next_thread = "]t",
    prev_thread = "[t",
    toggle_side_by_side = "<Leader>s",
    approve = "<Leader>aa",
    reset_status = "<Leader>ar",
    comment = "<Leader>ac",
    open_thread = "<Leader>ao",
    list_threads = "<Leader>at",
    cancel_request = "<Leader>aK",
    next_unreviewed = "]u",
    prev_unreviewed = "[u",
    accept_hunk = "<Leader>as",
    active_thread = "<Leader>aT",
    toggle_diff_style = "<Leader>ad",
    file_back = "<C-o>",
    find_file = "<Leader>af",
    grep = "<Leader>ag",
  },
})
```

## Per-workspace ref override

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

## Backend permissions

When arbiter sends feedback to the agent, the agent may need to run shell commands (e.g. `git`, `cargo fmt`) to apply changes. By default, both Cursor and Claude Code require interactive approval for shell commands. Since arbiter runs the CLI non-interactively, the agent will simply report that it cannot execute the command rather than prompting you.

**Recommended:** Configure your backend's built-in allowlists rather than disabling permissions entirely.

**Cursor CLI** -- Create or edit `~/.cursor/cli.json`:

```json
{
  "enabledTools": ["shell"],
  "allowedCommands": ["git", "cargo fmt", "cargo clippy", "rustfmt"]
}
```

**Claude Code** -- See the [Claude Code docs](https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/overview) for configuring allowed tools.

:::warning
Using `--yolo` (Cursor) or `--dangerously-skip-permissions` (Claude) via `extra_args` is discouraged. These flags allow the agent to run arbitrary commands without approval, including destructive operations like `rm -rf` or `git push --force`.
:::
