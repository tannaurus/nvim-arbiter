//! File tree rendering and status icons.
//!
//! Builds the left panel tree from flat file paths.
//! Provides `path_at_line()` for mapping buffer lines to file paths.

use crate::types::{FileStatus, ReviewStatus};
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::{self, Buffer};
use std::collections::HashMap;

/// Status icon for display in the file panel.
fn status_icon(review_status: ReviewStatus, file_status: FileStatus) -> &'static str {
    if file_status == FileStatus::Untracked {
        return "+";
    }
    match review_status {
        ReviewStatus::Approved => "✓",
        ReviewStatus::NeedsChanges => "✗",
        ReviewStatus::Unreviewed => "·",
    }
}

/// Entry for building the tree: either a directory or a file.
#[derive(Debug)]
enum TreeEntry {
    Dir(String),
    File {
        path: String,
        status: FileStatus,
        review_status: ReviewStatus,
    },
}

struct TreeResult {
    lines: Vec<String>,
    line_to_path: HashMap<usize, String>,
    line_to_dir: HashMap<usize, String>,
}

/// Builds tree lines from flat paths.
///
/// Sorts paths and deduplicates directory components. Directories in
/// `collapsed` are rendered with a `▸` indicator and their children are hidden.
/// Expanded directories show `▾`. Returns file-line and dir-line mappings.
fn build_tree(
    files: &[(String, FileStatus, ReviewStatus)],
    collapsed: &std::collections::HashSet<String>,
    open_thread_counts: &HashMap<String, usize>,
) -> TreeResult {
    let mut path_to_status: HashMap<&str, (FileStatus, ReviewStatus)> = HashMap::new();
    for (path, fs, rs) in files {
        path_to_status.insert(path.as_str(), (*fs, *rs));
    }

    let mut paths: Vec<&str> = files.iter().map(|(p, _, _)| p.as_str()).collect();
    paths.sort();

    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut seen_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in &paths {
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            let dir_path = parts[..i].join("/");
            if seen_dirs.insert(dir_path.clone()) {
                entries.push(TreeEntry::Dir(dir_path));
            }
        }
        let (fs, rs) = path_to_status
            .get(*path)
            .copied()
            .unwrap_or((FileStatus::Modified, ReviewStatus::Unreviewed));
        entries.push(TreeEntry::File {
            path: (*path).to_string(),
            status: fs,
            review_status: rs,
        });
    }

    let mut lines: Vec<String> = Vec::new();
    let mut line_to_path: HashMap<usize, String> = HashMap::new();
    let mut line_to_dir: HashMap<usize, String> = HashMap::new();

    for entry in &entries {
        let is_hidden = match entry {
            TreeEntry::Dir(path) => collapsed
                .iter()
                .any(|c| path.starts_with(&format!("{c}/")) && c != path),
            TreeEntry::File { path, .. } => {
                collapsed.iter().any(|c| path.starts_with(&format!("{c}/")))
            }
        };
        if is_hidden {
            continue;
        }

        let line_idx = lines.len();
        match entry {
            TreeEntry::Dir(path) => {
                let depth = path.matches('/').count();
                let name = path.rsplit('/').next().unwrap_or(path);
                let indent = "  ".repeat(depth);
                let arrow = if collapsed.contains(path) {
                    "▸"
                } else {
                    "▾"
                };
                lines.push(format!("{indent}{arrow} {name}/"));
                line_to_dir.insert(line_idx, path.clone());
            }
            TreeEntry::File {
                path,
                status,
                review_status,
            } => {
                let depth = path.matches('/').count();
                let name = path.rsplit('/').next().unwrap_or(path);
                let indent = "  ".repeat(depth);
                let icon = status_icon(*review_status, *status);
                let thread_tag = open_thread_counts
                    .get(path)
                    .filter(|&&c| c > 0)
                    .map(|c| format!(" [{c}T]"))
                    .unwrap_or_default();
                let review_tag = match review_status {
                    ReviewStatus::Approved => " [✓]",
                    ReviewStatus::NeedsChanges => " [✗]",
                    ReviewStatus::Unreviewed => "",
                };
                lines.push(format!("{indent}{icon} {name}{thread_tag}{review_tag}"));
                line_to_path.insert(line_idx, path.clone());
            }
        }
    }

    let approved = files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::Approved)
        .count();
    let needs = files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::NeedsChanges)
        .count();
    let unreviewed = files
        .iter()
        .filter(|(_, _, rs)| *rs == ReviewStatus::Unreviewed)
        .count();
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!(
        "── {} files │ ✓ {approved} │ ✗ {needs} │ · {unreviewed} ──",
        files.len()
    ));

    TreeResult {
        lines,
        line_to_path,
        line_to_dir,
    }
}

/// Render result containing both file and directory line mappings.
pub struct RenderResult {
    /// 1-based buffer line -> file path.
    pub line_to_path: HashMap<usize, String>,
    /// 1-based buffer line -> directory path.
    pub line_to_dir: HashMap<usize, String>,
}

/// Renders the file panel buffer from flat file paths.
///
/// Builds a tree structure with status icons and collapse indicators.
/// Directories in `collapsed` are shown with `▸` and their children hidden.
///
/// - `files`: (path, FileStatus, ReviewStatus) for each file
/// - `collapsed`: set of directory paths currently collapsed
pub fn render(
    buf: &mut Buffer,
    files: &[(String, FileStatus, ReviewStatus)],
    collapsed: &std::collections::HashSet<String>,
    open_thread_counts: &HashMap<String, usize>,
) -> nvim_oxi::Result<RenderResult> {
    let result = build_tree(files, collapsed, open_thread_counts);
    let refs: Vec<&str> = result.lines.iter().map(|s| s.as_str()).collect();

    api::set_option_value(
        "modifiable",
        true,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;
    let line_count = buf.line_count()?;
    buf.set_lines(0..line_count, false, refs)?;
    api::set_option_value(
        "modifiable",
        false,
        &OptionOpts::builder().buffer(buf.clone()).build(),
    )?;

    let mut buf_line_to_path: HashMap<usize, String> = HashMap::new();
    for (line_0, path) in result.line_to_path {
        buf_line_to_path.insert(line_0 + 1, path);
    }
    let mut buf_line_to_dir: HashMap<usize, String> = HashMap::new();
    for (line_0, dir) in result.line_to_dir {
        buf_line_to_dir.insert(line_0 + 1, dir);
    }
    Ok(RenderResult {
        line_to_path: buf_line_to_path,
        line_to_dir: buf_line_to_dir,
    })
}

/// Returns the file path at the given buffer line (1-based).
///
/// Returns `None` for directory lines or the summary section.
pub fn path_at_line(line_to_path: &HashMap<usize, String>, line: usize) -> Option<String> {
    line_to_path.get(&line).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_at_line_returns_path_for_file_line() {
        let mut m = HashMap::new();
        m.insert(1, "src/foo.rs".to_string());
        m.insert(3, "src/bar.rs".to_string());
        assert_eq!(path_at_line(&m, 1), Some("src/foo.rs".to_string()));
        assert_eq!(path_at_line(&m, 3), Some("src/bar.rs".to_string()));
    }

    #[test]
    fn path_at_line_returns_none_for_missing_line() {
        let m: HashMap<usize, String> = HashMap::new();
        assert!(path_at_line(&m, 5).is_none());
    }

    #[test]
    fn build_tree_expanded_shows_all() {
        let files = vec![
            (
                "src/foo.rs".to_string(),
                FileStatus::Modified,
                ReviewStatus::Unreviewed,
            ),
            (
                "src/bar.rs".to_string(),
                FileStatus::Modified,
                ReviewStatus::Approved,
            ),
        ];
        let collapsed = std::collections::HashSet::new();
        let counts = HashMap::new();
        let result = build_tree(&files, &collapsed, &counts);
        assert!(result.lines[0].contains("▾ src/"));
        assert!(result.lines[1].contains("bar.rs"));
        assert!(result.lines[2].contains("foo.rs"));
        assert_eq!(result.line_to_dir.get(&0), Some(&"src".to_string()));
        assert!(result.line_to_path.contains_key(&1));
        assert!(result.line_to_path.contains_key(&2));
    }

    #[test]
    fn build_tree_collapsed_hides_children() {
        let files = vec![
            (
                "src/foo.rs".to_string(),
                FileStatus::Modified,
                ReviewStatus::Unreviewed,
            ),
            (
                "src/bar.rs".to_string(),
                FileStatus::Modified,
                ReviewStatus::Approved,
            ),
            (
                "README.md".to_string(),
                FileStatus::Modified,
                ReviewStatus::Unreviewed,
            ),
        ];
        let mut collapsed = std::collections::HashSet::new();
        collapsed.insert("src".to_string());
        let counts = HashMap::new();
        let result = build_tree(&files, &collapsed, &counts);
        assert!(result.lines[0].contains("README.md"));
        assert!(result.lines[1].contains("▸ src/"));
        assert!(!result.lines.iter().any(|l| l.contains("foo.rs")));
        assert!(!result.lines.iter().any(|l| l.contains("bar.rs")));
        assert!(result.line_to_path.values().any(|p| p == "README.md"));
        assert!(!result.line_to_path.values().any(|p| p.starts_with("src/")));
    }
}
