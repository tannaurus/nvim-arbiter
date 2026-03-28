//! nvim-tree file panel adapter.
//!
//! Delegates file panel rendering to nvim-tree via a thin Lua bridge.
//! All filtering decisions, ancestor computation, and sign configuration
//! are computed here in Rust; the Lua adapter only stores the result
//! and forwards nvim-tree API calls.

use super::FilePanel;
use crate::config;
use crate::types::{FileStatus, ReviewStatus};
use nvim_oxi::api::{self, Buffer, Window};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

static HAS_DEVICONS: OnceLock<bool> = OnceLock::new();

fn detect_nerd_font() -> bool {
    api::command("lua vim.g._arbiter_has_devicons = pcall(require, 'nvim-web-devicons')").is_ok()
        && api::get_var::<bool>("_arbiter_has_devicons").unwrap_or(false)
}

fn resolve_icon(user: &Option<String>, nerd: bool, nerd_icon: &str, unicode: &str) -> String {
    if let Some(custom) = user {
        return custom.clone();
    }
    if nerd {
        nerd_icon.to_string()
    } else {
        unicode.to_string()
    }
}

pub(crate) struct NvimTreeFilePanel {
    buf: Buffer,
    win: Window,
    cwd: String,
    has_nerd_font: bool,
}

impl NvimTreeFilePanel {
    pub(crate) fn new(buf: Buffer, win: Window, cwd: String) -> Self {
        let has_nerd_font = *HAS_DEVICONS.get_or_init(detect_nerd_font);
        Self {
            buf,
            win,
            cwd,
            has_nerd_font,
        }
    }
}

fn build_visible_set(files: &[(String, FileStatus, ReviewStatus)]) -> Vec<String> {
    let mut visible = HashSet::new();
    for (path, _, _) in files {
        visible.insert(path.clone());
        let mut current = path.as_str();
        while let Some(pos) = current.rfind('/') {
            let ancestor = &current[..pos];
            if !visible.insert(ancestor.to_string()) {
                break;
            }
            current = ancestor;
        }
    }
    let mut sorted: Vec<String> = visible.into_iter().collect();
    sorted.sort();
    sorted
}

fn build_sign_map(
    files: &[(String, FileStatus, ReviewStatus)],
    nerd: bool,
) -> HashMap<String, (String, &'static str)> {
    let icons = &config::get().icons;
    files
        .iter()
        .map(|(p, _, rs)| {
            let (text, hl) = match rs {
                ReviewStatus::Approved => (
                    resolve_icon(&icons.approved, nerd, "\u{f00c}", "✔"),
                    "ArbiterSignApproved",
                ),
                ReviewStatus::Unreviewed => (
                    resolve_icon(&icons.unreviewed, nerd, "\u{f10c}", "○"),
                    "ArbiterSignPending",
                ),
            };
            (p.clone(), (text, hl))
        })
        .collect()
}

fn serialize_state(files: &[(String, FileStatus, ReviewStatus)], nerd: bool) -> (String, String) {
    let visible = build_visible_set(files);
    let signs = build_sign_map(files, nerd);
    let visible_json = serde_json::to_string(&visible).unwrap_or_else(|_| "[]".to_string());
    let signs_json = {
        let map: HashMap<&str, serde_json::Value> = signs
            .iter()
            .map(|(p, (text, hl))| (p.as_str(), serde_json::json!({"text": text, "hl": hl})))
            .collect();
        serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string())
    };
    (visible_json, signs_json)
}

impl FilePanel for NvimTreeFilePanel {
    fn render(
        &mut self,
        files: &[(String, FileStatus, ReviewStatus)],
        _open_thread_counts: &HashMap<String, usize>,
    ) -> nvim_oxi::Result<()> {
        let (visible_json, signs_json) = serialize_state(files, self.has_nerd_font);
        let _ = api::set_var("_arbiter_cwd", self.cwd.as_str());
        let _ = api::set_var("_arbiter_visible", visible_json.as_str());
        let _ = api::set_var("_arbiter_signs", signs_json.as_str());
        let _ = api::command(
            "lua require('arbiter.nvim_tree_adapter').set_state(vim.g._arbiter_cwd, vim.g._arbiter_visible, vim.g._arbiter_signs)",
        );
        Ok(())
    }

    fn path_at_line(&self, _line: usize) -> Option<String> {
        lua_string_call("file_at_cursor", &self.cwd)
    }

    fn dir_at_line(&self, _line: usize) -> Option<String> {
        lua_string_call("dir_at_cursor", &self.cwd)
    }

    fn toggle_collapse(&mut self, _dir: &str) {
        let _ = api::command("lua require('arbiter.nvim_tree_adapter').toggle_dir()");
    }

    fn highlight_file(&mut self, path: &str) {
        let abs = format!("{}/{}", self.cwd, path);
        let _ = api::set_var("_arbiter_tmp", abs.as_str());
        let _ =
            api::command("lua require('arbiter.nvim_tree_adapter').find_file(vim.g._arbiter_tmp)");
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

    fn cleanup(&mut self) {
        let _ = api::command("lua require('arbiter.nvim_tree_adapter').clear()");
    }

    fn should_wipe_buffer(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, fs: FileStatus, rs: ReviewStatus) -> (String, FileStatus, ReviewStatus) {
        (path.to_string(), fs, rs)
    }

    #[test]
    fn visible_set_empty() {
        let set = build_visible_set(&[]);
        assert!(set.is_empty());
    }

    #[test]
    fn visible_set_includes_ancestors() {
        let files = vec![file(
            "src/a/b.rs",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let set = build_visible_set(&files);
        assert!(set.contains(&"src/a/b.rs".to_string()));
        assert!(set.contains(&"src/a".to_string()));
        assert!(set.contains(&"src".to_string()));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn visible_set_deduplicates_shared_ancestors() {
        let files = vec![
            file("src/a.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
            file("src/b.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let set = build_visible_set(&files);
        assert_eq!(
            set.iter().filter(|p| *p == "src").count(),
            1,
            "shared ancestor should appear once"
        );
        assert_eq!(set.len(), 3); // src, src/a.rs, src/b.rs
    }

    #[test]
    fn visible_set_root_file_no_ancestors() {
        let files = vec![file(
            "README.md",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let set = build_visible_set(&files);
        assert_eq!(set, vec!["README.md"]);
    }

    #[test]
    fn sign_map_status_mapping() {
        let files = vec![
            file("a.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("b.rs", FileStatus::Modified, ReviewStatus::Unreviewed),
        ];
        let map = build_sign_map(&files, false);
        assert_eq!(map["a.rs"], ("✔".to_string(), "ArbiterSignApproved"));
        assert_eq!(map["b.rs"], ("○".to_string(), "ArbiterSignPending"));
    }

    #[test]
    fn serialize_state_roundtrips() {
        let files = vec![
            file("src/main.rs", FileStatus::Modified, ReviewStatus::Approved),
            file("README.md", FileStatus::Added, ReviewStatus::Unreviewed),
        ];
        let (visible_json, signs_json) = serialize_state(&files, false);

        let visible: Vec<String> = serde_json::from_str(&visible_json).unwrap();
        assert!(visible.contains(&"src/main.rs".to_string()));
        assert!(visible.contains(&"src".to_string()));
        assert!(visible.contains(&"README.md".to_string()));

        let signs: HashMap<String, serde_json::Value> = serde_json::from_str(&signs_json).unwrap();
        assert_eq!(signs["src/main.rs"]["text"], "✔");
        assert_eq!(signs["src/main.rs"]["hl"], "ArbiterSignApproved");
        assert_eq!(signs["README.md"]["text"], "○");
    }

    #[test]
    fn serialize_state_empty() {
        let (visible_json, signs_json) = serialize_state(&[], false);
        assert_eq!(visible_json, "[]");
        assert_eq!(signs_json, "{}");
    }

    #[test]
    fn visible_set_deep_nesting() {
        let files = vec![file(
            "a/b/c/d/e.rs",
            FileStatus::Modified,
            ReviewStatus::Unreviewed,
        )];
        let set = build_visible_set(&files);
        assert_eq!(set.len(), 5); // a, a/b, a/b/c, a/b/c/d, a/b/c/d/e.rs
    }
}

fn lua_string_call(func: &str, cwd: &str) -> Option<String> {
    let _ = api::set_var("_arbiter_cwd", cwd);
    let cmd = format!(
        "lua vim.g._arbiter_result = require('arbiter.nvim_tree_adapter').{func}(vim.g._arbiter_cwd) or ''"
    );
    let _ = api::command(&cmd);
    api::get_var::<String>("_arbiter_result")
        .ok()
        .filter(|s| !s.is_empty())
}
