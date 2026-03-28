---
sidebar_position: 7
title: Commands
---

# Commands

## Global commands

Available anytime, regardless of whether a review is active.

| Command | Description |
|---------|-------------|
| `:Arbiter` | Open the review workbench for unstaged working tree changes. |
| `:ArbiterCompare [ref]` | Open the review workbench diffed against a branch. Uses `review.default_ref` if no argument given. |
| `:ArbiterSend <prompt>` | Send a prompt to the agent. Response streams into a panel. |
| `:ArbiterContinue [prompt]` | Continue the current session with an optional follow-up. |
| `:ArbiterList` | List saved sessions in a floating window. `<CR>` to select. |
| `:ArbiterResume <id> [prompt]` | Resume a specific session by ID. |

## Review commands

Require an active review.

| Command | Description |
|---------|-------------|
| `:ArbiterPrompt [name]` | Toggle the prompt panel. Without an argument, opens the default "main" conversation. With a name (may be multiple words), opens or switches to that named conversation. Each conversation maintains independent message history and backend session. |
| `:ArbiterRef [branch]` | Change the comparison branch on the fly. No argument clears the base. |
| `:ArbiterActiveThread` | Open the thread window for the agent that is currently thinking. |
| `:ArbiterSelfReview [prompt]` | Run agent self-review on the current diff. Creates agent threads. Optional prompt steers focus. |
| `:ArbiterApply` | Send all open self-review feedback to the agent in a single prompt. Marks each thread as auto-resolve. |
| `:ArbiterOpenThread <file> <line>` | Open the thread at the given file and line number. |
| `:ArbiterFile <path> [line]` | Navigate to a file in the review. Optional line number scrolls to that source line in the diff. Used by Telescope integration. |
| `:ArbiterResolveAll` | Resolve all open threads. |
| `:ArbiterSummary` | Show review summary popup (file/thread counts). |
| `:ArbiterRules` | Open an editable popup with the current review rules. `:w` saves, `q` closes. |
| `:ArbiterToggleRules` | Toggle automatic rule extraction on agent responses. |
| `:ArbiterReloadRules` | Re-read project rule files from disk and report the count. |
| `:ArbiterReset` | Delete all persisted state for the current workspace and close the review. |
