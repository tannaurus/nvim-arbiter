---
sidebar_position: 6
title: Integrations
---

# Integrations

## nvim-tree

Arbiter ships a basic builtin file panel, but [nvim-tree](https://github.com/nvim-tree/nvim-tree.lua) is the recommended file panel for most users. It provides file-type icons, review status signs (approved, unreviewed), collapsible directories with familiar keybindings, and automatic filtering to show only changed files during a review.

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

## Telescope

Arbiter ships a [Telescope](https://github.com/nvim-telescope/telescope.nvim) extension with two pickers scoped to your review files:

- **`review_files`** -- Fuzzy-find only files in the current review. Shows review status (approved/unreviewed) next to each file. Selecting a file navigates to it in the review workbench.
- **`review_grep`** -- Live grep scoped to review files only. Selecting a match navigates to the file and line in the workbench.

No Telescope configuration is needed. The extension registers itself automatically when Telescope is installed. Use it via command or the default keymaps:

```vim
:Telescope arbiter review_files
:Telescope arbiter review_grep
```

Telescope is optional. The keymaps show an error if Telescope is not installed.

## Custom picker integration

For users of other file pickers (fzf-lua, fff, etc.), Arbiter exposes a Lua API to query the review file list:

```lua
local info = require("arbiter").review_files()
-- Returns nil when no review is active.
-- Otherwise: { cwd = "/abs/path", files = { { path = "src/foo.rs", status = "approved" }, ... } }
```

Use `info.files` to build a scoped search in any tool. To navigate back into the workbench, call `:ArbiterFile <path> [line]`.

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
