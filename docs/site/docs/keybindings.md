---
sidebar_position: 8
title: Keybindings
---

# Keybindings

All keybindings are active in the review workbench tabpage and are fully configurable via the `keymaps` config table.

## Navigation

| Default | Action |
|---------|--------|
| `]c` / `[c` | Next / previous hunk (scrolls hunk into view) |
| `]f` / `[f` | Next / previous file |
| `]t` / `[t` | Next / previous open thread (skips resolved, crosses files) |

## Review status

| Default | Action |
|---------|--------|
| `<Leader>aa` | Toggle approval on current file. In working tree mode, stages all hunks on approve and unstages on unapprove. Resolves thread if cursor is on a thread summary. |
| `<Leader>ar` | Reset to unreviewed |
| `<Leader>as` | Accept/unaccept the hunk under the cursor. In working tree mode, stages/unstages the hunk in git. Auto-approves file when all hunks accepted. |
| `]u` / `[u` | Next / previous unreviewed file |

## Comments and threads

| Default | Action |
|---------|--------|
| `<Leader>ac` | Add a comment and send to the agent. Opens the thread window with streaming response. |
| `<Leader>ao` | Open the thread conversation at the cursor. |
| `<Leader>at` | Open thread list popup (grouped by status). |
| `<Leader>aT` | Open the thread for the agent that is currently thinking. |
| `<Leader>aK` | Cancel all pending backend requests. |

## Other

| Default | Action |
|---------|--------|
| `<CR>` | Open the thread at the cursor line (or jump to source if no thread). |
| `<Leader>s` | Toggle side-by-side diff view. |
| `<Leader>ad` | Toggle diff highlighting style (full-line colors vs gutter signs). |
| `<Leader>af` | Fuzzy-find review files (Telescope). |
| `<Leader>ag` | Live grep across review files (Telescope). |
| `<C-o>` | Navigate back through file history (works across file jumps, thread jumps, and auto-advance). |
| `q` | Close the review workbench. |

## File panel

| Key | Action |
|-----|--------|
| `<CR>` | Select file, or toggle directory collapse. |

## Thread list popup

When the thread list popup is open (via `<Leader>at`):

| Key | Action |
|-----|--------|
| `<CR>` | Navigate to the thread's file/line and open the thread window. |
| `dd` | Resolve the thread (Open/Stale) or permanently delete it (Resolved). |
| `q` / `Esc` | Close the popup. |

## Comment input float

When the input float opens (via `<Leader>ac`), you're placed in Insert mode:

| Key | Mode | Action |
|-----|------|--------|
| (type normally) | Insert | Write your comment. Enter adds a newline. |
| `Esc` | Insert | Exit to Normal mode. |
| `Enter` | Normal | Submit the comment. |
| `q` / `Esc` | Normal | Cancel and close the float. |

## Thread detail window

When a thread conversation is open:

| Key | Action |
|-----|--------|
| `<CR>` | Reply to the thread (opens input split below the thread panel). |
| `q` | Close the thread window. |

## Prompt panel

When the prompt panel is open (via `:ArbiterPrompt`):

| Key | Action |
|-----|--------|
| `<CR>` | Open input to send a message. |
| `q` | Close the prompt panel (conversation state is preserved). |
