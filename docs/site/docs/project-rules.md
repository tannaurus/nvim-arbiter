---
sidebar_position: 5
title: Project Rules
---

# Project Rules

Project rules are persistent instructions loaded from disk and injected into agent prompts. They complement learned rules (which are extracted from your review conversations and persist with your review state) with durable, version-controllable guidance that applies across sessions and team members.

Use project rules for things like:

- Language or framework conventions that apply to specific file types
- Architecture constraints scoped to certain directories
- Review standards you want enforced during self-review but not in normal thread conversations

## File format

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

### Frontmatter fields

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `description` | yes | string | Human-readable name. Used for deduplication and display in `:ArbiterRules`. |
| `match` | no | string or list | Glob patterns matched against the file path. Omit to match all files. Accepts a single string (`"*.rs"`) or a list (`["**/*.rs", "**/*.toml"]`). |
| `scenarios` | no | list | When this rule applies: `"thread"` (comments and replies), `"self_review"` (`:ArbiterSelfReview`). Omit to apply in all scenarios. Unknown values are ignored. |

The body (everything after the closing `---`) is the instruction text sent to the agent. It can be any length.

## Search directories

Rules are loaded from three locations, in order:

1. **Global** -- `~/.config/arbiter/rules/`
2. **Workspace** -- `.arbiter/rules/` relative to the project root
3. **Custom** -- any directories listed in the `rules_dirs` config option

Only `.md` files are loaded. Subdirectories are not traversed. Malformed files are silently skipped.

## Resolution

When Arbiter builds a prompt, it resolves which rules apply based on two filters:

1. **Scenario** -- if the rule specifies `scenarios`, it must include the current context (`thread` or `self_review`). Rules with no `scenarios` field apply everywhere.
2. **File glob** -- if the rule specifies `match` patterns, at least one must match the file path. Rules with no `match` field apply to all files. During self-review (which operates on the whole diff, not a single file), glob-scoped rules are skipped since there is no single file to match against.

Matched rules are formatted and prepended to the agent prompt as a "Project rules" block.

## Relationship to learned rules

Learned rules (`learn_rules = true`) are extracted from your review conversations and accumulate during a session. They capture conventions you enforce in the moment, like "don't introduce callback-style code in this refactor." They persist with your review state and are restored when you reopen. Use `:ArbiterReset` to clear them along with all other persisted state.

Project rules are the opposite: written ahead of time, versioned in your repo, and applied consistently. The two systems work together. Project rules set the baseline; learned rules adapt to the current review.

## Related commands

| Command | Description |
|---------|-------------|
| `:ArbiterRules` | Open an editable popup showing all active rules (learned + project). `:w` saves, `q` closes. |
| `:ArbiterToggleRules` | Toggle automatic rule extraction on agent responses. |
| `:ArbiterReloadRules` | Re-read project rule files from disk and report the count. |
