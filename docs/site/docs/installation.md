---
sidebar_position: 2
title: Installation
---

# Installation

## Requirements

| Dependency | Version | Notes |
|------------|---------|-------|
| Neovim | 0.10+ | Uses `vim.uv`, `vim.fs`, and nvim-oxi 0.11 API features |
| Rust toolchain | stable | `cargo` and `rustc` must be on `$PATH` to compile the native library |
| Git | any recent | The plugin shells out to `git` for diffs, merge-base, file lists, etc. |
| Cursor CLI **or** Claude Code CLI | | At least one: `agent` (via [Cursor CLI](https://docs.cursor.com/cli)) or `claude` (via `npm install -g @anthropic-ai/claude-code`) |
| nvim-tree | recommended | Recommended for the file panel. A basic builtin tree ships by default, but nvim-tree provides file icons, review status signs, and familiar keybindings. See [Integrations](/docs/integrations#nvim-tree). |

**Platform support:** macOS and Linux. No Windows support.

**State directory:** Review state, threads, and sessions are persisted to `~/.local/share/nvim/arbiter/` by default. Override with `review.state_dir` in config. Persisted state is version-stamped; upgrading the plugin automatically discards stale caches.

## lazy.nvim

```lua
return {
  "tannaurus/nvim-arbiter",
  tag = "v0.0.8", -- pin to a release tag
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

## packer.nvim

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

## Manual

1. Clone the repo into your Neovim packages directory or anywhere on your `runtimepath`:

```bash
git clone https://github.com/tannaurus/nvim-arbiter.git \
  ~/.local/share/nvim/site/pack/plugins/start/arbiter
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

## How the native library loads

On install or update, the `build` hook calls `arbiter.build.download_or_build_binary()` which:

1. **Tries to download a prebuilt binary** from GitHub Releases matching the current git tag (e.g. `v0.1.0`). Binaries are available for Linux (glibc, x86_64/aarch64) and macOS (x86_64/aarch64). If the current commit is not a tagged release, the download is skipped.
2. **Validates the download** by loading it with `package.loadlib` before replacing the current binary (atomic `.tmp` rename).
3. **Falls back to `cargo build --release`** if no prebuilt binary is available or the download fails.

At load time, `lua/arbiter/init.lua` searches multiple paths for the compiled library:
- `target/release/libarbiter.{dylib,so}` (relative to plugin root)
- `$CARGO_TARGET_DIR/release/libarbiter.{dylib,so}` (if set)

If no library is found, it triggers the download-or-build process automatically.

The library is loaded directly from the build output via `package.loadlib` (not Lua `require`), which avoids macOS code signature invalidation from file copies.

## Health check

Run `:checkhealth arbiter` to verify your installation. It checks:
- Binary exists and loads correctly
- `cargo`, `git`, and a backend CLI are on `$PATH`
- All library search paths

## Build from source

```bash
cargo build --release
```

Output: `target/release/libarbiter.dylib` (macOS) or `libarbiter.so` (Linux).
