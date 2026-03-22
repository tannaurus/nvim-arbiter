//! Built-in file panel that renders a tree with status icons into a scratch buffer.

use super::FilePanel;
use crate::types::{FileStatus, ReviewStatus};
use nvim_oxi::api::opts::OptionOpts;
use nvim_oxi::api::{self, Buffer, Window};
use std::collections::{HashMap, HashSet};

pub(crate) struct BuiltinFilePanel {
    buf: Buffer,
    win: Window,
    line_to_path: HashMap<usize, String>,
    line_to_dir: HashMap<usize, String>,
    collapsed_dirs: HashSet<String>,
}

impl BuiltinFilePanel {
    pub(crate) fn new(buf: Buffer, win: Window) -> Self {
        Self {
            buf,
            win,
            line_to_path: HashMap::new(),
            line_to_dir: HashMap::new(),
            collapsed_dirs: HashSet::new(),
        }
    }
}

impl FilePanel for BuiltinFilePanel {
    fn render(
        &mut self,
        files: &[(String, FileStatus, ReviewStatus)],
        open_thread_counts: &HashMap<String, usize>,
    ) -> nvim_oxi::Result<()> {
        let result = build_tree(files, &self.collapsed_dirs, open_thread_counts);
        let refs: Vec<&str> = result.lines.iter().map(|s| s.as_str()).collect();

        api::set_option_value(
            "modifiable",
            true,
            &OptionOpts::builder().buffer(self.buf.clone()).build(),
        )?;
        let line_count = self.buf.line_count()?;
        self.buf.set_lines(0..line_count, false, refs)?;
        api::set_option_value(
            "modifiable",
            false,
            &OptionOpts::builder().buffer(self.buf.clone()).build(),
        )?;

        self.line_to_path.clear();
        for (line_0, path) in result.line_to_path {
            self.line_to_path.insert(line_0 + 1, path);
        }
        self.line_to_dir.clear();
        for (line_0, dir) in result.line_to_dir {
            self.line_to_dir.insert(line_0 + 1, dir);
        }
        Ok(())
    }

    fn path_at_line(&self, line: usize) -> Option<String> {
        self.line_to_path.get(&line).cloned()
    }

    fn dir_at_line(&self, line: usize) -> Option<String> {
        self.line_to_dir.get(&line).cloned()
    }

    fn toggle_collapse(&mut self, dir: &str) {
        if self.collapsed_dirs.contains(dir) {
            self.collapsed_dirs.remove(dir);
        } else {
            self.collapsed_dirs.insert(dir.to_string());
        }
    }

    fn highlight_file(&mut self, path: &str) {
        let panel_line = self
            .line_to_path
            .iter()
            .find(|(_, p)| p.as_str() == path)
            .map(|(l, _)| *l);
        if let Some(line) = panel_line {
            let _ = self.win.set_cursor(line, 0);
        }
    }

    fn window(&self) -> &Window {
        &self.win
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buf
    }

    fn buf_handle(&self) -> i32 {
        self.buf.handle()
    }
}

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
    collapsed: &HashSet<String>,
    open_thread_counts: &HashMap<String, usize>,
) -> TreeResult {
    let mut path_to_status: HashMap<&str, (FileStatus, ReviewStatus)> = HashMap::new();
    for (path, fs, rs) in files {
        path_to_status.insert(path.as_str(), (*fs, *rs));
    }

    let mut paths: Vec<&str> = files.iter().map(|(p, _, _)| p.as_str()).collect();
    paths.sort();

    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();

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

#[cfg(test)]
mod tests {
    use super::*;

    fn no_collapse() -> HashSet<String> {
        HashSet::new()
    }

    fn no_threads() -> HashMap<String, usize> {
        HashMap::new()
    }

    fn file(path: &str, fs: FileStatus, rs: ReviewStatus) -> (String, FileStatus, ReviewStatus) {
        (path.to_string(), fs, rs)
    }

    #[test]
    fn build_tree_empty_file_list() {
        let result = build_tree(&[], &no_collapse(), &no_threads());
        assert_eq!(result.lines.len(), 1);
        assert!(result.lines[0].contains("0 files"));
        assert!(result.line_to_path.is_empty());
        assert!(result.line_to_dir.is_empty());
    }

    #[test]
    fn build_tree_single_root_file() {
        let files = vec![file(
            "README.md",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert!(result.lines[0].contains("· README.md"));
        assert!(result.line_to_dir.is_empty());
        assert_eq!(result.line_to_path.get(&0), Some(&"README.md".to_string()));
    }

    #[test]
    fn build_tree_expanded_shows_all() {
        let files = vec![
            file("src/foo.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/bar.rs", FileStatus::Modified, ReviewStatus::Approved),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
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
            file("src/foo.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/bar.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("README.md", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert("src".to_string());
        let result = build_tree(&files, &collapsed, &no_threads());
        assert!(result.lines[0].contains("README.md"));
        assert!(result.lines[1].contains("▸ src/"));
        assert!(!result.lines.iter().any(|l| l.contains("foo.rs")));
        assert!(!result.lines.iter().any(|l| l.contains("bar.rs")));
        assert!(result.line_to_path.values().any(|p| p == "README.md"));
        assert!(!result.line_to_path.values().any(|p| p.starts_with("src/")));
    }

    #[test]
    fn build_tree_deep_nesting() {
        let files = vec![file(
            "a/b/c/deep.rs",
            FileStatus::Added,
            ReviewStatus::Unreviewed,
        )];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert!(result.lines[0].contains("▾ a/"));
        assert!(result.lines[1].contains("▾ b/"));
        assert!(result.lines[2].contains("▾ c/"));
        assert!(result.lines[3].contains("deep.rs"));
        assert_eq!(result.line_to_dir.len(), 3);
        assert_eq!(result.line_to_path.len(), 1);
        assert_eq!(
            result.line_to_path.get(&3),
            Some(&"a/b/c/deep.rs".to_string())
        );
    }

    #[test]
    fn build_tree_collapse_inner_dir_only() {
        let files = vec![
            file(
                "src/models/user.rs",
                FileStatus::Modified,
                ReviewStatus::Unreviewed,
            ),
            file("src/lib.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let mut collapsed = HashSet::new();
        collapsed.insert("src/models".to_string());
        let result = build_tree(&files, &collapsed, &no_threads());
        assert!(result.lines.iter().any(|l| l.contains("▾ src/")));
        assert!(result.lines.iter().any(|l| l.contains("lib.rs")));
        assert!(result.lines.iter().any(|l| l.contains("▸ models/")));
        assert!(!result.lines.iter().any(|l| l.contains("user.rs")));
    }

    #[test]
    fn build_tree_status_icons() {
        assert_eq!(
            status_icon(ReviewStatus::Unreviewed, FileStatus::Modified),
            "·"
        );
        assert_eq!(
            status_icon(ReviewStatus::Approved, FileStatus::Modified),
            "✓"
        );
        assert_eq!(
            status_icon(ReviewStatus::NeedsChanges, FileStatus::Modified),
            "✗"
        );
        assert_eq!(
            status_icon(ReviewStatus::Unreviewed, FileStatus::Untracked),
            "+"
        );
        assert_eq!(
            status_icon(ReviewStatus::Approved, FileStatus::Untracked),
            "+"
        );
    }

    #[test]
    fn build_tree_thread_count_display() {
        let files = vec![file(
            "main.rs",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let mut counts = HashMap::new();
        counts.insert("main.rs".to_string(), 3);
        let result = build_tree(&files, &no_collapse(), &counts);
        assert!(result.lines[0].contains("[3T]"));
    }

    #[test]
    fn build_tree_zero_threads_no_tag() {
        let files = vec![file(
            "main.rs",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let mut counts = HashMap::new();
        counts.insert("main.rs".to_string(), 0);
        let result = build_tree(&files, &no_collapse(), &counts);
        assert!(!result.lines[0].contains("T]"));
    }

    #[test]
    fn build_tree_review_tags() {
        let files = vec![
            file("a.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("b.rs", FileStatus::Modified, ReviewStatus::NeedsChanges),
            file("c.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        let a_line = result.lines.iter().find(|l| l.contains("a.rs")).unwrap();
        let b_line = result.lines.iter().find(|l| l.contains("b.rs")).unwrap();
        let c_line = result.lines.iter().find(|l| l.contains("c.rs")).unwrap();
        assert!(a_line.contains("[✓]"));
        assert!(b_line.contains("[✗]"));
        assert!(!c_line.contains("[✓]") && !c_line.contains("[✗]"));
    }

    #[test]
    fn build_tree_summary_line_counts() {
        let files = vec![
            file("a.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("b.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("c.rs", FileStatus::Modified, ReviewStatus::NeedsChanges),
            file("d.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("e.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        let summary = result.lines.last().unwrap();
        assert!(summary.contains("5 files"));
        assert!(summary.contains("✓ 2"));
        assert!(summary.contains("✗ 1"));
        assert!(summary.contains("· 2"));
    }

    #[test]
    fn build_tree_line_to_path_roundtrip() {
        let files = vec![
            file("src/a.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/b.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("README.md", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        for (line_idx, path) in &result.line_to_path {
            let line = &result.lines[*line_idx];
            let filename = path.rsplit('/').next().unwrap();
            assert!(
                line.contains(filename),
                "line {line_idx} ({line:?}) should contain {filename}"
            );
        }
        for (line_idx, dir) in &result.line_to_dir {
            let line = &result.lines[*line_idx];
            let dirname = dir.rsplit('/').next().unwrap();
            assert!(
                line.contains(&format!("{dirname}/")),
                "line {line_idx} ({line:?}) should contain {dirname}/"
            );
        }
    }

    #[test]
    fn build_tree_mixed_depths() {
        let files = vec![
            file("Cargo.toml", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/lib.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file(
                "tests/integration/test_main.rs",
                FileStatus::Added,
                ReviewStatus::Unreviewed,
            ),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert_eq!(result.line_to_path.len(), 3);
        assert_eq!(result.line_to_dir.len(), 3);
        assert!(result.line_to_path.values().any(|p| p == "Cargo.toml"));
        assert!(result.line_to_path.values().any(|p| p == "src/lib.rs"));
        assert!(result
            .line_to_path
            .values()
            .any(|p| p == "tests/integration/test_main.rs"));
    }

    #[test]
    fn build_tree_deduplicates_directories() {
        let files = vec![
            file("src/a.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/b.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/c.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        let dir_count = result.lines.iter().filter(|l| l.contains("▾ src/")).count();
        assert_eq!(dir_count, 1);
    }

    #[test]
    fn build_tree_root_only_files() {
        let files = vec![
            file("Cargo.toml", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("README.md", FileStatus::Modified, ReviewStatus::Approved),
            file("main.rs", FileStatus::Added, ReviewStatus::Unreviewed),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert!(result.line_to_dir.is_empty());
        assert_eq!(result.line_to_path.len(), 3);
        assert!(!result
            .lines
            .iter()
            .any(|l| l.contains("▾") || l.contains("▸")));
    }

    #[test]
    fn build_tree_duplicate_paths() {
        let files = vec![
            file("src/lib.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/lib.rs", FileStatus::Modified, ReviewStatus::Approved),
        ];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert!(result.line_to_path.values().any(|p| p == "src/lib.rs"));
        let summary = result.lines.last().unwrap();
        assert!(summary.contains("2 files"));
    }

    #[test]
    fn build_tree_empty_path_string() {
        let files = vec![file("", FileStatus::Modified, ReviewStatus::Unreviewed)];
        let result = build_tree(&files, &no_collapse(), &no_threads());
        assert!(result.line_to_path.values().any(|p| p.is_empty()));
    }

    #[test]
    fn snapshot_build_tree_representative() {
        let files = vec![
            file("Cargo.toml", FileStatus::Modified, ReviewStatus::Approved),
            file("README.md", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/lib.rs", FileStatus::Modified, ReviewStatus::Approved),
            file(
                "src/config.rs",
                FileStatus::Modified,
                ReviewStatus::NeedsChanges,
            ),
            file(
                "src/api/handler.rs",
                FileStatus::Modified,
                ReviewStatus::Unreviewed,
            ),
            file(
                "src/api/routes.rs",
                FileStatus::Added,
                ReviewStatus::Unreviewed,
            ),
            file(
                "src/db/connection.rs",
                FileStatus::Modified,
                ReviewStatus::Approved,
            ),
            file(
                "tests/integration.rs",
                FileStatus::Added,
                ReviewStatus::Unreviewed,
            ),
            file(
                "scripts/deploy.sh",
                FileStatus::Untracked,
                ReviewStatus::Unreviewed,
            ),
        ];
        let mut thread_counts = HashMap::new();
        thread_counts.insert("src/config.rs".to_string(), 2);
        thread_counts.insert("src/api/handler.rs".to_string(), 1);
        let output = build_tree(&files, &no_collapse(), &thread_counts)
            .lines
            .join("\n");
        insta::assert_snapshot!("build_tree_representative", output);
    }
}
