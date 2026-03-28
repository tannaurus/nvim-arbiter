---
sidebar_position: 4
title: Workflow
---

# Workflow

Arbiter works in two modes depending on how you open it:

- **Working tree review** (`:Arbiter`) -- Diffs unstaged changes against HEAD. Use this when you're iterating with an agent in real time and haven't committed yet. You see exactly what the agent has changed since your last commit. Accepting hunks (`<Leader>as`) and approving files (`<Leader>aa`) stage changes in git, so you can build up a commit as you review. Unstaging only reverses what Arbiter staged; pre-existing staged content is preserved.

- **Branch review** (`:ArbiterCompare main`) -- Diffs your current branch against a base ref using `git merge-base`, so you only see changes introduced by your branch. This matches what a GitHub/GitLab PR would show. Use this when you're reviewing a full feature branch before merging. Accepting hunks and approving files in this mode is visual-only (no git staging). If you set `review.default_ref` in your config, `:ArbiterCompare` with no argument uses that ref.

Both modes use the same workbench, threads, and feedback loop. The only difference is what the diff is computed against.

## The review loop

1. **The agent works.** You give Cursor or Claude Code a task. It writes code across multiple files.

2. **You open the workbench.** `:Arbiter` opens a review tabpage for unstaged changes, or `:ArbiterCompare main` diffs against a branch. The left panel shows changed files (like a PR file list). The right panel shows the diff, starting on the first file you haven't approved yet.

3. **You review file by file.** Select files in the left panel with `<CR>`. Jump between hunks with `]c`/`[c`. Collapse directories you don't care about. Use `<Leader>s` for a side-by-side view when you need it.

4. **You give feedback.** Press `<Leader>ac` on any line to leave a comment. A thread opens immediately and your comment is sent to the agent. The agent's response streams in real-time. This is the core interaction: every piece of feedback is a thread anchored to a specific line, just like a PR review comment.

5. **The agent revises.** The agent reads your feedback and makes changes. Arbiter polls the filesystem and updates the diff automatically.

6. **You track progress.** Mark files as approved (`<Leader>aa`) as you go. Accept individual hunks with `<Leader>as` to track your progress within a file. When all hunks are accepted, the file is auto-approved. In working tree mode, accepting a hunk stages it in git; toggling it back unstages only that hunk. Use `]u`/`[u` to jump between files you haven't reviewed yet. Run `:ArbiterSummary` for a summary of where you stand.

7. **Repeat.** Continue reviewing, commenting, and approving until the changeset looks right. Close the workbench with `q` when you're done. Your review state is persisted to disk and restored if you reopen.

## Agent self-review

Before you start reviewing, run `:ArbiterSelfReview`. The agent reviews its own diff and flags anything it's uncertain about. Its concerns appear as threads anchored to the relevant lines, giving you a head start on where to focus. You can pass an optional prompt to steer what the agent focuses on:

```vim
:ArbiterSelfReview check error handling and edge cases
```

To have the agent act on all its own feedback at once, run `:ArbiterApply`. This bundles every open self-review thread into a single prompt telling the agent to fix them all. Each thread is marked as auto-resolve, so they'll close automatically once the agent applies the changes.

## Side-by-side diff

Press `<Leader>s` on any file to open a side-by-side diff in a new tabpage using Neovim's native `:diffthis`. The left buffer shows the file at the merge-base, the right shows the working copy. Both get syntax highlighting. Press `<Leader>s` again (or `:tabclose`) to return.

## Changing the comparison branch

The default comparison branch is set in your config (globally or per-workspace). You can also change it on the fly:

```vim
:ArbiterRef develop   " switch to comparing against develop
:ArbiterRef           " clear the base (switch to working tree mode)
```

See [Per-workspace ref override](/docs/configuration#per-workspace-ref-override) for configuring defaults per repository.
