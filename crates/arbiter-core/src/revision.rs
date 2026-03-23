//! Revision capture and diff generation.
//!
//! Snapshots file contents before and after agent responses to produce
//! per-response diffs that can be viewed in isolation.

use crate::threads::{Revision, RevisionFile};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Reads the current content of each file in `paths` from disk.
///
/// Returns a map of path to content. Files that don't exist map to `None`.
pub fn snapshot_files(cwd: &str, paths: &[String]) -> HashMap<String, Option<String>> {
    let base = Path::new(cwd);
    paths
        .iter()
        .map(|p| {
            let content = std::fs::read_to_string(base.join(p)).ok();
            (p.clone(), content)
        })
        .collect()
}

/// Runs `git diff --name-only HEAD` synchronously to detect files the agent
/// may have created or modified that weren't in the original file list.
///
/// Uses HEAD when ref_name is empty (working tree mode) to stay consistent
/// with the main diff pipeline.
///
/// Returns relative paths. Fast enough to call from a callback (<50ms).
pub fn diff_names_sync(cwd: &str, ref_name: &str) -> Vec<String> {
    let effective_ref = if ref_name.is_empty() {
        "HEAD"
    } else {
        ref_name
    };
    let args = vec!["diff", "--name-only", &effective_ref];
    let output = match std::process::Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Compares before and after snapshots, returning changed files only.
///
/// Also checks `new_paths` for files not present in the before snapshot
/// (files the agent may have created).
pub fn build_revision(
    thread: &crate::threads::Thread,
    before: &HashMap<String, Option<String>>,
    after: &HashMap<String, Option<String>>,
    new_paths: &[String],
    message_index: usize,
) -> Option<Revision> {
    let mut files = Vec::new();

    for (path, before_content) in before {
        let after_content = after.get(path).cloned().flatten();
        if before_content.as_deref() != after_content.as_deref() {
            files.push(RevisionFile {
                path: path.clone(),
                before: before_content.clone(),
                after: after_content,
            });
        }
    }

    for path in new_paths {
        if before.contains_key(path) {
            continue;
        }
        if let Some(Some(content)) = after.get(path) {
            files.push(RevisionFile {
                path: path.clone(),
                before: None,
                after: Some(content.clone()),
            });
        }
    }

    if files.is_empty() {
        return None;
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Some(Revision {
        index: thread.revisions.len() as u32 + 1,
        ts,
        message_index,
        files,
    })
}

/// Generates unified diff text from before/after file content.
///
/// Produces output compatible with the existing `diff::parse` and
/// `diff::render` pipeline so revision diffs can use the same renderer.
pub fn generate_unified_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let old_lines: Vec<&str> = before.map(|s| s.lines().collect()).unwrap_or_default();
    let new_lines: Vec<&str> = after.map(|s| s.lines().collect()).unwrap_or_default();

    let ops = lcs_diff(&old_lines, &new_lines);
    if ops.is_empty() {
        return String::new();
    }

    let old_name = if before.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".to_string()
    };
    let new_name = if after.is_some() {
        format!("b/{path}")
    } else {
        "/dev/null".to_string()
    };

    let mut out = format!("diff --git a/{path} b/{path}\n");
    if before.is_none() {
        out.push_str("new file mode 100644\n");
    } else if after.is_none() {
        out.push_str("deleted file mode 100644\n");
    }
    out.push_str(&format!("--- {old_name}\n"));
    out.push_str(&format!("+++ {new_name}\n"));

    let hunks = group_into_hunks(&ops);
    if hunks.is_empty() {
        return String::new();
    }
    for hunk in hunks {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));
        for op in &hunk.ops {
            match op {
                DiffOp::Equal(line) => out.push_str(&format!(" {line}\n")),
                DiffOp::Remove(line) => out.push_str(&format!("-{line}\n")),
                DiffOp::Add(line) => out.push_str(&format!("+{line}\n")),
            }
        }
    }

    out
}

/// Generates the unified diff for a single file within a revision.
pub fn revision_file_diff(rf: &RevisionFile) -> String {
    generate_unified_diff(&rf.path, rf.before.as_deref(), rf.after.as_deref())
}

/// Computes line-count stats for a revision file (additions, deletions).
///
/// Counts actual diff operations rather than comparing total line counts,
/// so modified lines are reflected as both an addition and a deletion.
pub fn revision_file_stats(rf: &RevisionFile) -> (usize, usize) {
    let old_lines: Vec<&str> = rf
        .before
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let new_lines: Vec<&str> = rf
        .after
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let ops = lcs_diff(&old_lines, &new_lines);
    let mut added = 0usize;
    let mut removed = 0usize;
    for op in &ops {
        match op {
            DiffOp::Add(_) => added += 1,
            DiffOp::Remove(_) => removed += 1,
            DiffOp::Equal(_) => {}
        }
    }
    (added, removed)
}

#[derive(Debug)]
enum DiffOp<'a> {
    Equal(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

#[derive(Debug)]
struct DiffHunk<'a> {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    ops: Vec<DiffOp<'a>>,
}

fn lcs_diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<DiffOp<'a>> {
    let n = old.len();
    let m = new.len();

    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            ops.push(DiffOp::Equal(old[i]));
            i += 1;
            j += 1;
        } else if j < m && (i >= n || table[i][j + 1] >= table[i + 1][j]) {
            ops.push(DiffOp::Add(new[j]));
            j += 1;
        } else {
            ops.push(DiffOp::Remove(old[i]));
            i += 1;
        }
    }
    ops
}

fn group_into_hunks<'a>(ops: &[DiffOp<'a>]) -> Vec<DiffHunk<'a>> {
    let context_lines = 3;
    let mut hunks: Vec<DiffHunk<'a>> = Vec::new();

    let mut change_ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        if !matches!(ops[i], DiffOp::Equal(_)) {
            let start = i;
            while i < ops.len() && !matches!(ops[i], DiffOp::Equal(_)) {
                i += 1;
            }
            change_ranges.push((start, i));
        } else {
            i += 1;
        }
    }

    if change_ranges.is_empty() {
        return hunks;
    }

    let mut groups: Vec<(usize, usize)> = Vec::new();
    let (mut gs, mut ge) = change_ranges[0];
    for &(cs, ce) in &change_ranges[1..] {
        if cs.saturating_sub(ge) <= context_lines * 2 {
            ge = ce;
        } else {
            groups.push((gs, ge));
            gs = cs;
            ge = ce;
        }
    }
    groups.push((gs, ge));

    for (gs, ge) in groups {
        let ctx_start = gs.saturating_sub(context_lines);
        let ctx_end = (ge + context_lines).min(ops.len());

        let mut old_line = 1usize;
        let mut new_line = 1usize;
        for op in &ops[..ctx_start] {
            match op {
                DiffOp::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                DiffOp::Remove(_) => old_line += 1,
                DiffOp::Add(_) => new_line += 1,
            }
        }

        let old_start = old_line;
        let new_start = new_line;
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        let mut hunk_ops: Vec<DiffOp<'a>> = Vec::new();

        for op in &ops[ctx_start..ctx_end] {
            match op {
                DiffOp::Equal(s) => {
                    hunk_ops.push(DiffOp::Equal(s));
                    old_count += 1;
                    new_count += 1;
                }
                DiffOp::Remove(s) => {
                    hunk_ops.push(DiffOp::Remove(s));
                    old_count += 1;
                }
                DiffOp::Add(s) => {
                    hunk_ops.push(DiffOp::Add(s));
                    new_count += 1;
                }
            }
        }

        hunks.push(DiffHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            ops: hunk_ops,
        });
    }

    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_diff_addition() {
        let diff = generate_unified_diff("foo.rs", None, Some("line1\nline2\n"));
        assert!(diff.contains("+++ b/foo.rs"));
        assert!(diff.contains("--- /dev/null"));
        assert!(diff.contains("+line1"));
        assert!(diff.contains("+line2"));
    }

    #[test]
    fn generate_diff_deletion() {
        let diff = generate_unified_diff("foo.rs", Some("line1\nline2\n"), None);
        assert!(diff.contains("--- a/foo.rs"));
        assert!(diff.contains("+++ /dev/null"));
        assert!(diff.contains("-line1"));
        assert!(diff.contains("-line2"));
    }

    #[test]
    fn generate_diff_modification() {
        let diff = generate_unified_diff("foo.rs", Some("old line\n"), Some("new line\n"));
        assert!(diff.contains("-old line"));
        assert!(diff.contains("+new line"));
    }

    #[test]
    fn generate_diff_no_change() {
        let diff = generate_unified_diff("foo.rs", Some("same\n"), Some("same\n"));
        assert!(diff.is_empty());
    }

    #[test]
    fn generate_unified_diff_roundtrip_parseable() {
        let before = "fn main() {\n    println!(\"hello\");\n}\n";
        let after = "fn main() {\n    println!(\"world\");\n    return;\n}\n";
        let diff = generate_unified_diff("test.rs", Some(before), Some(after));

        assert!(!diff.is_empty());
        let hunks = crate::diff::parse_hunks(&diff);
        assert!(!hunks.is_empty());
        assert!(hunks[0].old_start >= 1);
        assert!(hunks[0].new_start >= 1);
    }

    #[test]
    fn build_revision_no_changes() {
        let thread = crate::threads::create("f.rs", 1, "hi", crate::threads::CreateOpts::default());
        let snap: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("content".to_string()))]
                .into_iter()
                .collect();
        let rev = build_revision(&thread, &snap, &snap, &[], 1);
        assert!(rev.is_none());
    }

    #[test]
    fn build_revision_with_changes() {
        let thread = crate::threads::create("f.rs", 1, "hi", crate::threads::CreateOpts::default());
        let before: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("old".to_string()))]
                .into_iter()
                .collect();
        let after: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("new".to_string()))]
                .into_iter()
                .collect();
        let rev = build_revision(&thread, &before, &after, &[], 1).unwrap();
        assert_eq!(rev.index, 1);
        assert_eq!(rev.files.len(), 1);
        assert_eq!(rev.files[0].path, "f.rs");
        assert_eq!(rev.files[0].before.as_deref(), Some("old"));
        assert_eq!(rev.files[0].after.as_deref(), Some("new"));
    }

    #[test]
    fn build_revision_index_continuity() {
        let mut thread =
            crate::threads::create("f.rs", 1, "hi", crate::threads::CreateOpts::default());
        thread.revisions.push(Revision {
            index: 1,
            ts: 100,
            message_index: 1,
            files: vec![],
        });
        thread.revisions.push(Revision {
            index: 2,
            ts: 200,
            message_index: 2,
            files: vec![],
        });

        let before: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("old".to_string()))]
                .into_iter()
                .collect();
        let after: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("new".to_string()))]
                .into_iter()
                .collect();
        let rev = build_revision(&thread, &before, &after, &[], 3).unwrap();
        assert_eq!(rev.index, 3);
    }

    #[test]
    fn build_revision_detects_new_files() {
        let thread = crate::threads::create("f.rs", 1, "hi", crate::threads::CreateOpts::default());
        let before: HashMap<String, Option<String>> = HashMap::new();
        let after: HashMap<String, Option<String>> =
            [("new.rs".to_string(), Some("content".to_string()))]
                .into_iter()
                .collect();
        let rev = build_revision(&thread, &before, &after, &["new.rs".to_string()], 1).unwrap();
        assert_eq!(rev.files.len(), 1);
        assert_eq!(rev.files[0].path, "new.rs");
        assert!(rev.files[0].before.is_none());
    }

    #[test]
    fn build_revision_file_deletion() {
        let thread = crate::threads::create("f.rs", 1, "hi", crate::threads::CreateOpts::default());
        let before: HashMap<String, Option<String>> =
            [("f.rs".to_string(), Some("content".to_string()))]
                .into_iter()
                .collect();
        let after: HashMap<String, Option<String>> =
            [("f.rs".to_string(), None)].into_iter().collect();
        let rev = build_revision(&thread, &before, &after, &[], 1).unwrap();
        assert_eq!(rev.files.len(), 1);
        assert_eq!(rev.files[0].before.as_deref(), Some("content"));
        assert!(rev.files[0].after.is_none());
    }

    #[test]
    fn revision_file_stats_counts() {
        let rf = RevisionFile {
            path: "f.rs".to_string(),
            before: Some("a\nb\nc\n".to_string()),
            after: Some("a\nb\nc\nd\ne\n".to_string()),
        };
        let (added, removed) = revision_file_stats(&rf);
        assert_eq!(added, 2);
        assert_eq!(removed, 0);
    }

    #[test]
    fn revision_file_stats_modified_lines() {
        let rf = RevisionFile {
            path: "f.rs".to_string(),
            before: Some("aaa\nbbb\nccc\n".to_string()),
            after: Some("aaa\nBBB\nCCC\n".to_string()),
        };
        let (added, removed) = revision_file_stats(&rf);
        assert_eq!(added, 2);
        assert_eq!(removed, 2);
    }

    #[test]
    fn revision_file_stats_pure_deletion() {
        let rf = RevisionFile {
            path: "f.rs".to_string(),
            before: Some("x\ny\nz\n".to_string()),
            after: Some("x\n".to_string()),
        };
        let (added, removed) = revision_file_stats(&rf);
        assert_eq!(added, 0);
        assert_eq!(removed, 2);
    }

    #[test]
    fn revision_file_stats_empty_both() {
        let rf = RevisionFile {
            path: "f.rs".to_string(),
            before: None,
            after: None,
        };
        let (added, removed) = revision_file_stats(&rf);
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
    }

    #[test]
    fn group_into_hunks_context_boundary() {
        let old_lines = vec![
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o",
        ];
        let mut modified = old_lines.clone();
        modified[0] = "A";
        modified[14] = "O";

        let ops = lcs_diff(&old_lines, &modified);
        let hunks = group_into_hunks(&ops);

        assert_eq!(hunks.len(), 2);
        assert!(hunks[0].old_count > 0);
        assert!(hunks[1].old_count > 0);
    }

    fn init_repo(dir: &std::path::Path) {
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .expect("git config email");
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .expect("git config name");
    }

    fn commit_all(dir: &std::path::Path, msg: &str) {
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(dir)
            .output()
            .expect("git commit");
    }

    #[test]
    fn diff_names_sync_empty_ref_uses_head() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        init_repo(dir);

        std::fs::write(dir.join("a.txt"), "initial\n").expect("write");
        commit_all(dir, "initial");

        std::fs::write(dir.join("a.txt"), "modified\n").expect("write");

        let names = diff_names_sync(dir.to_str().unwrap(), "");
        assert!(names.contains(&"a.txt".to_string()));
    }

    #[test]
    fn diff_names_sync_explicit_ref() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        init_repo(dir);

        std::fs::write(dir.join("a.txt"), "v1\n").expect("write");
        commit_all(dir, "v1");

        std::fs::write(dir.join("a.txt"), "v2\n").expect("write");
        commit_all(dir, "v2");

        let names = diff_names_sync(dir.to_str().unwrap(), "HEAD~1");
        assert!(names.contains(&"a.txt".to_string()));
    }

    #[test]
    fn diff_names_sync_no_changes_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        init_repo(dir);

        std::fs::write(dir.join("a.txt"), "same\n").expect("write");
        commit_all(dir, "initial");

        let names = diff_names_sync(dir.to_str().unwrap(), "");
        assert!(names.is_empty());
    }

    #[test]
    fn snapshot_unified_diff_modification() {
        let before = "\
use std::io;

fn main() {
    let name = \"world\";
    println!(\"hello {}\", name);
    do_work();
}

fn do_work() {
    println!(\"working...\");
}
";
        let after = "\
use std::io;
use std::fmt;

fn main() {
    let name = \"universe\";
    println!(\"hello {}\", name);
    setup();
    do_work();
}

fn do_work() {
    println!(\"working hard...\");
    cleanup();
}
";
        let output = generate_unified_diff("src/main.rs", Some(before), Some(after));
        insta::assert_snapshot!("unified_diff_modification", output);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn generate_unified_diff_never_panics(
            before in "[a-z\n]{0,50}",
            after in "[a-z\n]{0,50}",
        ) {
            let diff = generate_unified_diff("test.rs", Some(&before), Some(&after));
            if !diff.is_empty() {
                let hunks = crate::diff::parse_hunks(&diff);
                for hunk in &hunks {
                    prop_assert!(hunk.buf_start <= hunk.buf_end);
                }
            }
        }
    }
}
