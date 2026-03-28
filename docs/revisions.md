# Revisions

## Problem

When an agent responds to a review thread, it may modify multiple files across the codebase. In the current workflow, these changes are only visible as part of the full branch diff. For large or multi-file changes, it is difficult to see what a single agent response actually did. The reviewer has no way to isolate "the changes produced by this specific response" from everything else in the working tree.

## Overview

A **revision** is a before/after snapshot of file contents produced by a single agent response. Each agent response that modifies files creates one revision, attached to its parent thread. Revisions are viewed in a dedicated mode that takes over the workbench, showing only the revision's changes.

Revisions also support the same hunk acceptance flow as the main compare view. Accepting hunks in a revision maps those acceptances back to the full branch diff.

## Data Model

### Revision

Each revision captures what one agent response changed:

- **index** - sequential within the thread (1, 2, 3...)
- **timestamp** - when the revision was captured
- **message_index** - which agent message in the thread produced this revision
- **files** - list of file snapshots (only files that changed)

### RevisionFile

A single file's before/after state within a revision:

- **path** - relative file path
- **before** - file content before the agent response; absent if the file was created
- **after** - file content after the agent response; absent if the file was deleted

### Storage

Revisions are stored on the `Thread` struct and persist in `threads.json` alongside conversation messages. No new persistence files are needed.

### RevisionRef

When a user comments while viewing a revision, the message carries metadata linking it back:

- **revision_index** - which revision the comment refers to
- **file** - file within the revision
- **line** - line number in the revision's diff

Comments with a `RevisionRef` are displayed in the thread with context like "(on revision 2, foo.rs:14)".

## Snapshot Capture

### When to snapshot

Snapshots are captured around backend dispatch calls that can produce file changes: initial comments and thread replies. Extraction prompts and ask-mode calls do not modify files and are excluded.

### Before-snapshot

Immediately before `backend::send_comment()` or `backend::thread_reply()` is called, read the current content of every file in the review's file list. Store as a map of path to content. Files that don't exist on disk yet map to absent.

### After-snapshot

In the completion callback, after the agent process has exited:

1. Re-read the same files to get post-response content.
2. Run a synchronous `git diff --name-only` to detect files the agent may have created that weren't in the original file list.
3. Read any newly created files.
4. Compare before and after for each path. Only keep files where content differs.
5. If any files changed, build a `Revision` and push it onto the thread.

### Timing

The completion callback fires after the CLI process exits, so file changes should already be on disk. The file poll timer may have triggered re-renders during the agent's work, but this does not affect snapshots since they read directly from the filesystem.

### No-change responses

If the agent responds with only text (explanation, question, error) and no files change, no revision is created.

## Revision View Mode

### Layout

Revision view is a full workbench mode. When entered:

- **File panel (left)** - shows only files from the revision, not the full file list
- **Diff panel (right)** - shows the before/after diff for the selected revision file
- **Thread window** - stays open showing the conversation
- **Winbar** - both panels indicate revision mode (e.g., "src/auth.rs (revision 2 of 3)")

When exited, the workbench restores: file panel returns to the full file list, diff panel returns to the full branch diff for the previously selected file.

### Entry Points

**From the thread window (summary line)**: After a revision is captured, a summary line is inserted into the thread conversation:

```
  revision 2 - 3 files changed (+42 -18)
    src/auth.rs  (+28 -12)
    src/config.rs  (+8 -4)
    src/main.rs  (+6 -2)
```

Pressing `<CR>` on a summary line enters revision view for that revision. Summary lines are visually distinct from message text (different highlight group, dimmed or styled as metadata).

**From the thread window (cycling)**: `]r` / `[r` cycle through the thread's revisions. If not in revision view, enters at the first or last revision respectively. If already in revision view, advances to the next or previous revision.

**From the diff panel**: A keymap (e.g., `<Leader>rv`) enters revision view for the current thread. If the thread has no revisions, displays a message. If it has revisions, opens the first one. `]r` / `[r` navigate from there.

### Exiting

`q` or `<Esc>` from revision view restores the full compare workbench. Alternatively, closing the thread window could also exit revision view, since the revision is scoped to a thread.

## Hunk Acceptance in Revision View

### Goal

Accepting hunks in revision view should feel identical to accepting hunks in the main compare view. The same keybinding, the same visual feedback (dimming, folding). Accepting all hunks in a revision should propagate to the main compare so the reviewer doesn't have to re-accept the same changes.

### Mapping revision hunks to main-diff hunks

The existing acceptance model is content-hash based. Each hunk has a SHA256 hash of its diff content. Accepted hunk hashes are stored per-file and used to dim/fold hunks in the diff panel.

When the user accepts a hunk in revision view:

1. Compute the unified diff of the revision file (before vs after).
2. Parse into hunks and compute content hashes for each.
3. Accept the hunk visually in revision view (dim, fold).
4. Find the corresponding hunk in the main branch diff by matching "after" line content.
5. If the main-diff hunk's "after" lines match the revision hunk's "after" lines, mark the main-diff hunk as accepted using its content hash.

### Stale detection

A revision hunk is **stale** if its "after" content no longer matches what is on disk. This happens when another thread's revision (or manual user edits) modifies the same lines after this revision was captured.

When the user tries to accept a stale revision hunk:

- The hunk is still accepted in revision view (it's a historical snapshot).
- The mapping to the main diff fails because the content no longer matches.
- The user is notified: "This hunk has been modified since this revision. Review the current state in the main diff."

### Accept-all for a revision

A keybinding (e.g., `<Leader>aa` reused from the main compare) accepts all hunks in the current revision file. If all files in the revision have all hunks accepted, the revision is considered fully accepted.

When all hunks in a revision are accepted:

- Each hunk is mapped to the main diff as described above.
- Stale hunks are reported but do not block the overall acceptance.
- The revision summary line in the thread is updated to reflect acceptance (e.g., checkmark or "accepted" label).

### Content-hash bridge

The bridge between revision acceptance and main-diff acceptance relies on a key property: if the agent's change is the only thing that touched a region, the revision's "after" content and the main diff's "new" content for that region will be identical, producing the same content hash.

When they diverge (because another revision or manual edit touched the same lines), the hashes won't match. This is correct behavior: the reviewer should inspect the final state, not auto-accept something that has been further modified.

## Thread Summary Lines

After a revision is captured, a summary is appended to the thread conversation in the thread window. The summary includes:

- Revision number
- Number of files changed
- Per-file line counts (additions/removals)

Summary lines use a distinct highlight group so they read as metadata rather than conversation. They are interactive: `<CR>` on a summary line enters revision view.

Summary lines are not stored as thread messages. They are rendered dynamically from the revision data when the thread window opens or when a revision is captured during an active thread window session.

## Edge Cases

### Agent creates new files

The after-snapshot includes a `git diff --name-only` check to detect files the agent created that weren't in the original file list. These are included in the revision with `before: absent`.

### Agent deletes files

If a file existed in the before-snapshot but is gone after, it's included with `after: absent`. The revision diff shows the file as fully removed.

### Overlapping revisions across threads

Thread A's revision touches lines 10-20 of `foo.rs`. Thread B's revision also touches lines 15-25. Each revision is independent and shows its own before/after. The overlap is visible when accepting: accepting thread A's revision in the main diff will show thread B's overlapping hunk as stale or partially matching.

### Interrupted responses

If the user cancels an agent response mid-stream, the after-snapshot still captures whatever changes made it to disk. If no files changed, no revision is created. If some files changed (partial application), a revision is created for the partial changes.

### Large revisions

An agent response that modifies many files produces a large revision. The revision file panel handles this the same way the main file panel handles a large file list. Storage size is bounded by the number of files and their sizes. Compression is a future optimization if needed.

### Revision view and file polling

While in revision view, file poll changes should not alter the revision display. The revision is a frozen snapshot. File list polling should be suppressed or ignored while in revision mode to avoid confusing the user with changes to a static view.
