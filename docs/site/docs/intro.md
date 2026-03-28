---
sidebar_position: 1
title: Why arbiter
---

# Why arbiter

Agents can produce a 30-file changeset in minutes. Reviewing it shouldn't take all day.

A chat window is one big conversation. Imagine doing a PR review where every comment to the author went into a single thread. You'd constantly be saying "going back to that thing on line 42..." and hoping they follow. PR reviews solved this with **threads**: each comment is its own scoped conversation anchored to a specific line, and everyone knows what's being discussed. Arbiter gives agents the same structure.

## Review memory

In the best code reviews, the author doesn't just fix what you point out. They pick up on the pattern and apply it across the whole changeset. Review memory brings that to agents: conventions you enforce in one thread get extracted and fed into every subsequent prompt, so the agent applies your preferences before you ask.

This isn't a replacement for skills or system prompts. It supplements them with things you notice during the review itself. For example:

- *"We're moving off callbacks to async/await in this refactor. Don't introduce new callback-style code."* Specific to this effort, not a forever rule.
- *"The design doc says these endpoints return 201 for creates, not 200. Apply that to all the new handlers."* A spec decision for this feature, not a global convention.
- *"This module returns `Option` not `Result` since absence isn't an error here."* A recent design call that hasn't been codified anywhere.

These are decisions made during or around the review. They'd rot in a skill file, but for the next 20 threads in this session you want the agent to know them.

## Features at a glance

- **PR-style diffs** -- Dedicated tabpage with file panel and diff viewer. Diff against a branch via `git merge-base`, or diff unstaged working tree changes.
- **Line-anchored threads** -- Comment on a diff line, get a streaming response. Threads persist across sessions.
- **Review memory** -- Conventions you enforce get extracted and fed into future prompts automatically.
- **Project rules** -- Persistent, file-aware instructions loaded from markdown files with TOML frontmatter.
- **Similar threads** -- Similarity pass groups threads that flag the same class of issue. Cross-references appear in the thread panel.
- **Progress tracking** -- Approve files, accept hunks. In working tree mode, accepting hunks stages them in git.
- **Self-review** -- The agent reviews its own diff and flags concerns before you start.
- **Prompt panel** -- Long-lived agent conversations in a floating window with independent context.
- **Live diffs** -- Filesystem polling picks up the agent's changes without manual refresh.
- **Auto-resolve** -- Self-review threads resolve automatically once the agent applies the fix.
- **Session persistence** -- Review state, threads, and conversations restored when you reopen.
