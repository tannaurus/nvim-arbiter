# arbiter

[![CI](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml/badge.svg)](https://github.com/tannaurus/nvim-arbiter/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tannaurus/nvim-arbiter?style=flat&label=release&include_prereleases)](https://github.com/tannaurus/nvim-arbiter/releases/latest)
[![Neovim](https://img.shields.io/badge/neovim-0.10%2B-57A143?logo=neovim&logoColor=white)](https://neovim.io)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)]()

> **Experimental** - This plugin is under active development. APIs, commands, and keymaps may change without notice.

Review workbench for Neovim. PR-style diffs, line-anchored threads, and a structured feedback loop with AI coding agents. Built in Rust with [nvim-oxi](https://github.com/noib3/nvim-oxi). Works with Cursor CLI and Claude Code CLI.

**[Documentation](https://tannaurus.github.io/nvim-arbiter/)**

## Quick start

```lua
-- lazy.nvim
return {
  "tannaurus/nvim-arbiter",
  tag = "v0.0.8",
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

## Requirements

| Dependency | Version | Notes |
|------------|---------|-------|
| Neovim | 0.10+ | |
| Rust toolchain | stable | Required to compile the native library |
| Git | any recent | |
| Cursor CLI or Claude Code CLI | | At least one backend |

macOS and Linux only.

## Features

- **PR-style diffs** -- Dedicated review tabpage with file panel and diff viewer.
- **Line-anchored threads** -- Comment on a diff line, get a streaming response from the agent.
- **Review memory** -- Conventions you enforce get extracted and fed into future prompts.
- **Project rules** -- File-aware instructions from markdown files with TOML frontmatter.
- **Agent self-review** -- The agent reviews its own diff before you start.
- **Session persistence** -- Review state restored when you reopen.

See the [full documentation](https://tannaurus.github.io/nvim-arbiter/) for configuration, keybindings, commands, integrations, and architecture details.

## License

[MIT](LICENSE)
