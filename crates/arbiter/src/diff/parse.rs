//! Unified diff parsing.
//!
//! Parses raw git diff output into structured hunks. Pure string parsing,
//! no Neovim dependency.

use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// A single hunk within a unified diff.
///
/// Buffer positions (`buf_start`, `buf_end`) are 0-based and inclusive.
/// They are populated by the renderer after accounting for injected lines.
#[derive(Debug, Clone)]
pub struct Hunk {
    /// First buffer line of this hunk (0-based).
    pub buf_start: usize,
    /// Last buffer line of this hunk (0-based).
    pub buf_end: usize,
    /// Start line in the old file (1-based).
    pub old_start: usize,
    /// Number of lines in the old file.
    pub old_count: usize,
    /// Start line in the new file (1-based).
    pub new_start: usize,
    /// Number of lines in the new file.
    pub new_count: usize,
    /// Raw header line (e.g. `@@ -1,3 +1,4 @@`).
    pub header: String,
    /// Hash of the hunk's content lines (excluding header).
    pub content_hash: String,
}

/// Source file location for a buffer line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// File path (from diff header or caller).
    pub file: String,
    /// 1-based line number in the source file.
    pub line: usize,
}

/// Computes a content hash for change detection.
///
/// Uses SHA256 truncated to 12 hex chars. Deterministic.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    result[..6.min(result.len())]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// Parses raw unified diff text into hunks.
///
/// Handles standard, single-line (count omitted = 1), no-newline-at-EOF,
/// binary, rename, and empty diffs. Returns empty vec for empty or
/// non-diff input. `buf_start`/`buf_end` reflect the actual line positions
/// in the full diff text (including file headers).
pub fn parse_hunks(diff_text: &str) -> Vec<Hunk> {
    let lines: Vec<&str> = diff_text.lines().collect();
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@ ") {
            let hunk_start = i;
            let mut content = Vec::new();
            let mut j = i + 1;
            while j < lines.len()
                && !lines[j].starts_with("@@ ")
                && !lines[j].starts_with("diff --git ")
            {
                let l = lines[j];
                if l != "\\ No newline at end of file"
                    && (l.starts_with('+')
                        || l.starts_with('-')
                        || l.starts_with(' ')
                        || l.starts_with('\t'))
                {
                    content.push(l);
                }
                j += 1;
            }
            if let Some(mut hunk) = parse_hunk_header(line, &content) {
                hunk.buf_start = hunk_start;
                hunk.buf_end = j - 1;
                hunks.push(hunk);
            }
            i = j;
        } else {
            i += 1;
        }
    }

    hunks
}

fn parse_hunk_header(header: &str, content: &[&str]) -> Option<Hunk> {
    let inner = header
        .trim_start_matches("@@ ")
        .trim_end_matches(" @@")
        .trim_end_matches(" @@");
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let (old_start, old_count) = parse_range(parts[0], '-')?;
    let (new_start, new_count) = parse_range(parts[1], '+')?;

    let content_str = content.join("\n");
    let hash = content_hash(&content_str);

    Some(Hunk {
        buf_start: 0,
        buf_end: 0,
        old_start,
        old_count,
        new_start,
        new_count,
        header: header.to_string(),
        content_hash: hash,
    })
}

fn parse_range(part: &str, prefix: char) -> Option<(usize, usize)> {
    let s = part.strip_prefix(prefix)?;
    let (start_str, count_str) = match s.split_once(',') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (s.trim(), "1"),
    };
    let start: usize = start_str.parse().ok()?;
    let count: usize = count_str.parse().unwrap_or(1);
    Some((start, count))
}

/// Maps a buffer line to the corresponding source file location.
///
/// `lines` is the full buffer content; `file_path` is the path being displayed.
/// Returns None for lines outside any hunk. Uses new file line for additions
/// and context, old file line for deletions.
pub fn buf_line_to_source(
    hunks: &[Hunk],
    buf_line: usize,
    lines: &[impl AsRef<str>],
    file_path: &str,
) -> Option<SourceLocation> {
    for h in hunks {
        if buf_line < h.buf_start || buf_line > h.buf_end {
            continue;
        }
        let mut old_line = h.old_start;
        let mut new_line = h.new_start;
        for i in h.buf_start..=buf_line {
            let Some(line) = lines.get(i).map(|s| s.as_ref()) else {
                break;
            };
            if line == "\\ No newline at end of file" {
                continue;
            }
            if i == buf_line {
                return Some(SourceLocation {
                    file: file_path.to_string(),
                    line: if line.starts_with('-') && !line.starts_with("---") {
                        old_line
                    } else {
                        new_line
                    },
                });
            }
            if line.starts_with('-') && !line.starts_with("---") {
                old_line += 1;
            } else if line.starts_with('+') && !line.starts_with("+++") {
                new_line += 1;
            } else if line.starts_with("@@ ") {
                continue;
            } else if line.starts_with(' ') || line.starts_with('\t') || line.is_empty() {
                old_line += 1;
                new_line += 1;
            }
        }
    }
    None
}

/// Maps a source line number to the closest buffer line within the diff hunks.
///
/// Scans each hunk's content to find where `source_line` appears in the
/// new-file line numbering. Returns the 0-based buffer line, or `None`
/// if the line is not covered by any hunk.
pub fn source_to_buf_line(
    hunks: &[Hunk],
    source_line: usize,
    lines: &[impl AsRef<str>],
) -> Option<usize> {
    for h in hunks {
        let mut new_line = h.new_start;
        for i in h.buf_start..=h.buf_end {
            let Some(line) = lines.get(i).map(|s| s.as_ref()) else {
                break;
            };
            if line.starts_with("@@ ") || line == "\\ No newline at end of file" {
                continue;
            }
            let is_delete = line.starts_with('-') && !line.starts_with("---");
            if !is_delete && new_line == source_line {
                return Some(i);
            }
            if !is_delete {
                new_line += 1;
            }
        }
    }
    None
}

/// Produces a synthetic all-additions diff for an untracked file.
///
/// Every line is prefixed with `+`. Handles empty file.
pub fn synthesize_untracked(contents: &str, path: &str) -> String {
    let mut out = format!("diff --git a/{path} b/{path}\n");
    out.push_str("new file mode 100644\n");
    out.push_str("index 0000000..0000000 100644\n");
    out.push_str("--- /dev/null\n");
    out.push_str(&format!("+++ b/{path}\n"));
    out.push_str("@@ -0,0 +1,");
    let line_count = contents.lines().count().max(1);
    out.push_str(&line_count.to_string());
    out.push_str(" @@\n");
    for line in contents.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !contents.ends_with('\n') && !contents.is_empty() {
        out.push('+');
        out.push('\n');
    }
    out
}

/// Extracts a minimal valid git patch for a single hunk from raw diff text.
/// Compares old content hashes against new hunks.
///
/// Returns buf_start lines for hunks that are new or changed.
pub fn detect_hunk_changes(old_hashes: &HashSet<String>, new_hunks: &[Hunk]) -> HashSet<usize> {
    new_hunks
        .iter()
        .filter(|h| !old_hashes.contains(&h.content_hash))
        .map(|h| h.buf_start)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_HUNK_DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 000..111 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn foo() {}
+pub fn bar() {}
@@ -5,4 +6,5 @@
 pub fn baz() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
 }
";

    const SIMPLE_DIFF: &str = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello world\");
+    println!(\"goodbye\");
 }
";

    #[test]
    fn parse_hunks_standard_multi_hunk() {
        let hunks = parse_hunks(MULTI_HUNK_DIFF);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 2);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);
        assert_eq!(hunks[1].old_start, 5);
        assert_eq!(hunks[1].new_start, 6);
    }

    #[test]
    fn parse_hunks_empty_input() {
        assert!(parse_hunks("").is_empty());
    }

    #[test]
    fn parse_hunks_single_line_count_omitted() {
        let diff = "@@ -1 +1 @@\n+line\n";
        let hunks = parse_hunks(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_count, 1);
        assert_eq!(hunks[0].new_count, 1);
    }

    #[test]
    fn parse_hunks_no_newline_at_eof_ignored() {
        let diff = "@@ -1,1 +1,2 @@\n-a\n+b\n\\ No newline at end of file\n";
        let hunks = parse_hunks(diff);
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn parse_hunks_binary_marker() {
        let diff = "Binary files a/x and b/x differ\n";
        let hunks = parse_hunks(diff);
        assert!(hunks.is_empty());
    }

    #[test]
    fn content_hash_deterministic() {
        let h1 = content_hash("foo");
        let h2 = content_hash("foo");
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_different_inputs() {
        let h1 = content_hash("foo");
        let h2 = content_hash("bar");
        assert_ne!(h1, h2);
    }

    #[test]
    fn synthesize_untracked_format() {
        let out = synthesize_untracked("line1\nline2\n", "src/foo.rs");
        assert!(out.starts_with("diff --git"));
        assert!(out.contains("+line1"));
        assert!(out.contains("+line2"));
    }

    #[test]
    fn synthesize_untracked_empty_file() {
        let out = synthesize_untracked("", "x");
        assert!(out.contains("@@ -0,0 +1,1 @@"));
    }

    #[test]
    fn detect_hunk_changes_new_detected() {
        let old: HashSet<String> = HashSet::new();
        let hunks = parse_hunks(SIMPLE_DIFF);
        let changed = detect_hunk_changes(&old, &hunks);
        assert!(!changed.is_empty());
    }

    #[test]
    fn detect_hunk_changes_unchanged_ignored() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let old: HashSet<String> = hunks.iter().map(|h| h.content_hash.clone()).collect();
        let changed = detect_hunk_changes(&old, &hunks);
        assert!(changed.is_empty());
    }

    fn simple_diff_all_lines() -> Vec<String> {
        SIMPLE_DIFF.lines().map(String::from).collect()
    }

    #[test]
    fn buf_line_to_source_addition_line() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        assert_eq!(hunks[0].buf_start, 4);
        let loc = buf_line_to_source(&hunks, 7, &lines, "src/main.rs");
        assert_eq!(
            loc,
            Some(SourceLocation {
                file: "src/main.rs".to_string(),
                line: 2,
            })
        );
    }

    #[test]
    fn buf_line_to_source_deletion_line() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        let loc = buf_line_to_source(&hunks, 6, &lines, "src/main.rs");
        assert_eq!(
            loc,
            Some(SourceLocation {
                file: "src/main.rs".to_string(),
                line: 2,
            })
        );
    }

    #[test]
    fn buf_line_to_source_context_line() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        let loc = buf_line_to_source(&hunks, 5, &lines, "src/main.rs");
        assert_eq!(
            loc,
            Some(SourceLocation {
                file: "src/main.rs".to_string(),
                line: 1,
            })
        );
    }

    #[test]
    fn buf_line_to_source_file_header() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        assert!(buf_line_to_source(&hunks, 0, &lines, "src/main.rs").is_none());
        assert!(buf_line_to_source(&hunks, 3, &lines, "src/main.rs").is_none());
    }

    #[test]
    fn buf_line_to_source_outside_hunk() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        assert!(buf_line_to_source(&hunks, 20, &lines, "src/main.rs").is_none());
    }

    #[test]
    fn buf_line_to_source_empty_hunks() {
        let lines = simple_diff_all_lines();
        assert!(buf_line_to_source(&[], 0, &lines, "src/main.rs").is_none());
    }

    #[test]
    fn source_to_buf_line_finds_context() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        let result = source_to_buf_line(&hunks, 1, &lines);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn source_to_buf_line_finds_addition() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        let result = source_to_buf_line(&hunks, 3, &lines);
        assert_eq!(result, Some(8));
    }

    #[test]
    fn source_to_buf_line_outside_hunk() {
        let hunks = parse_hunks(SIMPLE_DIFF);
        let lines = simple_diff_all_lines();
        assert!(source_to_buf_line(&hunks, 99, &lines).is_none());
    }

    #[test]
    fn source_to_buf_line_empty_hunks() {
        let lines = simple_diff_all_lines();
        assert!(source_to_buf_line(&[], 1, &lines).is_none());
    }
}
