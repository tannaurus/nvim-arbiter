---
sidebar_position: 9
title: Architecture
---

# Architecture

The plugin is written in Rust using [nvim-oxi](https://github.com/noib3/nvim-oxi) for typed bindings to Neovim's C API. The codebase is split into two crates:

## arbiter-core

Pure-logic library with no Neovim dependency. All domain types, configuration, prompt formatting, diff parsing, thread data model, persistence, and revision building live here. This crate is testable on any platform without a Neovim runtime.

| Module | Purpose |
|--------|---------|
| `types` | Shared domain types (review status, thread status, backend ops) |
| `config` | Configuration deserialization with per-workspace overrides |
| `diff` | Unified diff parser, hunk extraction, and patch building |
| `threads` | Thread CRUD, anchoring, filtering, and projection |
| `prompts` | Prompt formatting for reviews, replies, self-review, and rule extraction |
| `rules` | Scenario-scoped rule system with glob matching and TOML frontmatter |
| `state` | JSON persistence of review state, threads, and sessions |
| `revision` | Revision snapshot building and unified diff generation |

## arbiter

cdylib plugin loaded by Neovim. Contains all UI code, nvim-oxi bindings, and process management. Depends on `arbiter-core` for domain logic.

| Module | Purpose |
|--------|---------|
| `backend/` | CLI adapter shim (Cursor, Claude) with FIFO queue, streaming, and process lifecycle |
| `review/` | Review workbench: lifecycle, keymaps, navigation, hunk acceptance, thread UI, and revision view |
| `commands/` | User command registration and self-review orchestration |
| `diff/render` | Diff buffer rendering and syntax highlighting |
| `file_panel/` | File panel implementations (builtin tree, nvim-tree adapter) |
| `prompt_panel` | Long-lived prompt conversations in a floating window |
| `panel` | Shared rendering utilities (timestamps, streaming, status lines) |
| `dispatch` | Safe cross-thread callback dispatch via `libuv::AsyncHandle` |
| `git` | Async git operations (merge-base, diff, show, stash) and staging/unstaging |
| `poll` | Periodic file and file-list refresh via libuv timers |
| `activity` | Backend busy/idle tracking for statusline display |
| `highlight` | Custom highlight groups and sign definitions |
