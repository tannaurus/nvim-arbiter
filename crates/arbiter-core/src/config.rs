//! Configuration deserialization from Lua tables.
//!
//! Config is stored in a `OnceLock<Config>` for global access.
//! Deserializes from the Lua table passed to `setup()`.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Plugin configuration. Deserialized from the Lua table passed to setup().
/// Missing fields use defaults via `#[serde(default)]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Backend: "cursor" or "claude".
    pub backend: BackendKind,
    /// Optional model override.
    pub model: Option<String>,
    /// Workspace root. Defaults to cwd at setup time.
    pub workspace: Option<String>,
    /// Show thread signs in normal buffers.
    pub inline_indicators: bool,
    /// Which file panel implementation to use.
    pub file_panel: FilePanelKind,
    /// Review-specific options.
    pub review: ReviewConfig,
    /// Prompt templates.
    pub prompts: PromptConfig,
    /// Thread window appearance.
    pub thread_window: ThreadWindowConfig,
    /// Keybinding overrides.
    pub keymaps: KeymapConfig,
    /// When true, resolving a thread sends the conversation to the agent
    /// to extract generalizable coding conventions. Extracted rules are
    /// injected into future thread prompts so the agent "learns" from
    /// your feedback over the course of the review. Each extraction
    /// costs one additional backend call.
    pub learn_rules: bool,
    /// Extra CLI flags appended to every backend invocation.
    ///
    /// These are passed verbatim after all built-in flags. Useful for
    /// backend-specific options like `--yolo` (Cursor) or
    /// `--dangerously-skip-permissions` (Claude).
    pub extra_args: Vec<String>,
    /// Per-workspace overrides, keyed by absolute directory path.
    /// Only `default_ref` is supported currently.
    pub workspaces: HashMap<String, WorkspaceOverride>,
    /// Custom icons for review status signs in the nvim-tree file panel.
    /// Any field left unset auto-detects: Nerd Font glyphs if
    /// nvim-web-devicons is installed, Unicode otherwise.
    pub icons: IconConfig,
    /// Additional directories to search for project rule files.
    /// Searched after the default locations (`~/.config/arbiter/rules/`
    /// and `.arbiter/rules/` in the workspace). Later sources win
    /// when rule descriptions conflict.
    pub rules_dirs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendKind::Cursor,
            model: None,
            workspace: std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from)),
            inline_indicators: false,
            file_panel: FilePanelKind::Builtin,
            learn_rules: true,
            review: ReviewConfig::default(),
            prompts: PromptConfig::default(),
            thread_window: ThreadWindowConfig::default(),
            keymaps: KeymapConfig::default(),
            extra_args: Vec::new(),
            workspaces: HashMap::new(),
            icons: IconConfig::default(),
            rules_dirs: Vec::new(),
        }
    }
}

/// Per-workspace configuration overrides.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WorkspaceOverride {
    /// Default ref branch for this workspace (e.g. "develop", "trunk").
    pub default_ref: Option<String>,
}

/// Backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendKind {
    #[default]
    Cursor,
    Claude,
}

impl<'de> Deserialize<'de> for BackendKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "cursor" => Ok(BackendKind::Cursor),
            "claude" => Ok(BackendKind::Claude),
            _ => Err(serde::de::Error::custom(format!(
                "invalid backend: {s}. Use 'cursor' or 'claude'"
            ))),
        }
    }
}

/// File panel implementation selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilePanelKind {
    /// Built-in tree rendered into a scratch buffer.
    #[default]
    Builtin,
    /// Use nvim-tree as the file panel. Requires nvim-tree to be installed.
    NvimTree,
}

impl<'de> Deserialize<'de> for FilePanelKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().replace('-', "_").as_str() {
            "builtin" => Ok(FilePanelKind::Builtin),
            "nvim_tree" | "nvimtree" => Ok(FilePanelKind::NvimTree),
            _ => Err(serde::de::Error::custom(format!(
                "invalid file_panel: {s}. Use 'builtin' or 'nvim-tree'"
            ))),
        }
    }
}

/// Custom icons for review status signs.
///
/// All fields are optional. When unset, auto-detects Nerd Font glyphs
/// (if nvim-web-devicons is available) and falls back to Unicode.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IconConfig {
    /// Icon shown for approved files. Examples: "✔", "", "👍"
    pub approved: Option<String>,
    /// Icon shown for files needing changes. Examples: "✘", "", "❌"
    pub needs_changes: Option<String>,
    /// Icon shown for unreviewed files. Examples: "○", "", "⏳"
    pub unreviewed: Option<String>,
}

/// Diff highlighting style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffStyle {
    /// Full-line background colors (GitHub-style). Replaces syntax highlighting.
    #[default]
    Full,
    /// Colored gutter signs only. Preserves syntax highlighting on the line content.
    Signs,
}

impl<'de> Deserialize<'de> for DiffStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "full" => Ok(DiffStyle::Full),
            "signs" => Ok(DiffStyle::Signs),
            _ => Err(serde::de::Error::custom(format!(
                "invalid diff_style: {s}. Use 'full' or 'signs'"
            ))),
        }
    }
}

/// Review-specific configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Default ref for diff (e.g. "main"); nil = unstaged changes.
    pub default_ref: Option<String>,
    /// Start in side-by-side view.
    pub side_by_side: bool,
    /// Collapse approved hunks.
    pub fold_approved: bool,
    /// Diff highlighting style: "full" (colored backgrounds) or "signs" (gutter markers).
    pub diff_style: DiffStyle,
    /// Auto-resolve timeout in seconds.
    pub auto_resolve_timeout: u64,
    /// File poll interval in ms.
    pub poll_interval: u64,
    /// File list refresh interval in ms.
    pub file_list_interval: u64,
    /// State directory path.
    pub state_dir: Option<String>,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            default_ref: None,
            side_by_side: false,
            fold_approved: false,
            diff_style: DiffStyle::Full,
            auto_resolve_timeout: 60,
            poll_interval: 2000,
            file_list_interval: 5000,
            state_dir: None,
        }
    }
}

/// Thread panel appearance configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThreadWindowConfig {
    /// Panel split direction: "right", "left", "top", or "bottom".
    pub position: PanelPosition,
    /// Panel size in lines (top/bottom) or columns (left/right).
    pub size: u32,
    /// `chrono` format string for message timestamps.
    /// See <https://docs.rs/chrono/latest/chrono/format/strftime/>.
    pub date_format: String,
}

impl Default for ThreadWindowConfig {
    fn default() -> Self {
        Self {
            position: PanelPosition::Right,
            size: 60,
            date_format: "%Y-%m-%d %H:%M".to_string(),
        }
    }
}

/// Split direction for the thread panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPosition {
    Top,
    Bottom,
    Left,
    Right,
}

impl PanelPosition {
    /// Returns the vim split command prefix and whether the split is vertical.
    pub fn split_cmd(self, size: u32) -> String {
        let size = size.max(5);
        match self {
            PanelPosition::Top => format!("topleft {size}split"),
            PanelPosition::Bottom => format!("botright {size}split"),
            PanelPosition::Left => format!("topleft vertical {size}split"),
            PanelPosition::Right => format!("botright vertical {size}split"),
        }
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, PanelPosition::Left | PanelPosition::Right)
    }
}

impl<'de> Deserialize<'de> for PanelPosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "top" => Ok(PanelPosition::Top),
            "bottom" => Ok(PanelPosition::Bottom),
            "left" => Ok(PanelPosition::Left),
            "right" => Ok(PanelPosition::Right),
            _ => Err(serde::de::Error::custom(format!(
                "invalid panel position '{s}'; expected top, bottom, left, or right"
            ))),
        }
    }
}

/// Format instructions appended to every self-review prompt.
///
/// This is not user-configurable because the parser in
/// `backend::parse_self_review_text` depends on this exact wire format.
pub const SELF_REVIEW_FORMAT_SUFFIX: &str = concat!(
    " For each concern, output a line in exactly this format:\n\n",
    "THREAD|file/path|line_number|your question or concern\n\n",
    "Example:\n",
    "THREAD|src/lib.rs|42|This unwrap could panic if the input is empty\n\n",
    "Output ONLY THREAD lines, no other text.",
);

/// Prompt template configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    /// Catch-up summarization prompt.
    pub catch_up: String,
    /// Self-review guidance. Format instructions for the THREAD wire format
    /// are appended automatically; this field controls only the review
    /// direction (what to look for, tone, scope, etc.).
    pub self_review: String,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            catch_up: "Summarize the changes you've made and the current state of the project."
                .to_string(),
            self_review:
                "Review this diff and flag anything you're uncertain about or want feedback on."
                    .to_string(),
        }
    }
}

/// Keymap overrides for the review workbench.
///
/// All fields accept Neovim keymap notation (e.g. `<Leader>s`, `]c`).
/// Set any field to override the default binding.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
    /// Jump to next hunk in the diff panel.
    pub next_hunk: String,
    /// Jump to previous hunk in the diff panel.
    pub prev_hunk: String,
    /// Select the next file in the file panel.
    pub next_file: String,
    /// Select the previous file in the file panel.
    pub prev_file: String,
    /// Jump to next thread summary in the diff panel.
    pub next_thread: String,
    /// Jump to previous thread summary in the diff panel.
    pub prev_thread: String,
    /// Toggle side-by-side diff view (opens in a new tabpage).
    pub toggle_side_by_side: String,
    /// Mark current file as approved.
    pub approve: String,
    /// Mark current file as needs-changes.
    pub needs_changes: String,
    /// Reset current file's review status to unreviewed.
    pub reset_status: String,
    /// Add a comment and send it to the agent.
    pub comment: String,
    /// Add a comment with auto-resolve (auto-approved once the agent applies it).
    pub auto_resolve: String,
    /// Open the thread conversation at the cursor.
    pub open_thread: String,
    /// List all threads in a floating window.
    pub list_threads: String,
    /// List only agent-created threads.
    pub list_threads_agent: String,
    /// List only user-created threads.
    pub list_threads_user: String,
    /// List only stale threads (anchor lost).
    pub list_threads_stale: String,
    /// List only open (unresolved) threads.
    pub list_threads_open: String,
    /// Resolve the thread at the cursor.
    pub resolve_thread: String,
    /// Toggle showing resolved threads in the diff panel.
    pub toggle_resolved: String,
    /// Re-anchor a thread to the current cursor position.
    pub re_anchor: String,
    /// Refresh the current file diff and file list.
    pub refresh: String,
    /// Cancel all pending backend requests.
    pub cancel_request: String,
    /// Jump to next file with open threads or non-approved status.
    pub next_unreviewed: String,
    /// Jump to previous file with open threads or non-approved status.
    pub prev_unreviewed: String,
    /// Toggle the hunk under the cursor as accepted in the review checklist.
    pub accept_hunk: String,
    /// Go back to the previous file after auto-advance on approval.
    pub file_back: String,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            next_hunk: "]c".to_string(),
            prev_hunk: "[c".to_string(),
            next_file: "]f".to_string(),
            prev_file: "[f".to_string(),
            next_thread: "]t".to_string(),
            prev_thread: "[t".to_string(),
            toggle_side_by_side: "<Leader>s".to_string(),
            approve: "<Leader>aa".to_string(),
            needs_changes: "<Leader>ax".to_string(),
            reset_status: "<Leader>ar".to_string(),
            comment: "<Leader>ac".to_string(),
            auto_resolve: "<Leader>aA".to_string(),
            open_thread: "<Leader>ao".to_string(),
            list_threads: "<Leader>at".to_string(),
            list_threads_agent: "<Leader>ata".to_string(),
            list_threads_user: "<Leader>atu".to_string(),
            list_threads_stale: "<Leader>atb".to_string(),
            list_threads_open: "<Leader>ato".to_string(),
            resolve_thread: "<Leader>ar".to_string(),
            toggle_resolved: "<Leader>a?".to_string(),
            re_anchor: "<Leader>aP".to_string(),
            refresh: "<Leader>aU".to_string(),
            cancel_request: "<Leader>aK".to_string(),
            next_unreviewed: "<Leader>an".to_string(),
            prev_unreviewed: "<Leader>ap".to_string(),
            accept_hunk: "<Leader>as".to_string(),
            file_back: "<C-o>".to_string(),
        }
    }
}

impl Config {
    /// Returns the effective `default_ref` for the given workspace directory.
    ///
    /// Matching is tried in two passes:
    ///
    /// 1. Literal path prefix - keys that start with `/` (or `~`) are
    ///    canonicalized and matched as a directory prefix. Longest match wins.
    /// 2. Regex - all other keys are compiled as a regex and tested against
    ///    the canonical cwd. Among regex matches, the longest pattern string wins.
    ///
    /// Literal matches always take priority over regex matches.
    /// Falls back to `review.default_ref` if nothing matches.
    pub fn default_ref_for(&self, cwd: &str) -> Option<&str> {
        let canonical = std::path::Path::new(cwd)
            .canonicalize()
            .ok()
            .and_then(|p| p.to_str().map(String::from));
        let cwd_canon = canonical.as_deref().unwrap_or(cwd);

        let mut best_literal_len = 0usize;
        let mut best_literal: Option<&WorkspaceOverride> = None;
        let mut best_regex_len = 0usize;
        let mut best_regex: Option<&WorkspaceOverride> = None;

        for (key, ws_override) in &self.workspaces {
            if key.starts_with('/') || key.starts_with('~') {
                let expanded = if key.starts_with('~') {
                    std::env::var("HOME")
                        .ok()
                        .map(|home| format!("{}{}", home, &key[1..]))
                        .unwrap_or_else(|| key.clone())
                } else {
                    key.clone()
                };
                let key_canon = std::path::Path::new(&expanded)
                    .canonicalize()
                    .ok()
                    .and_then(|p| p.to_str().map(String::from));
                let resolved = key_canon.as_deref().unwrap_or(&expanded);

                let is_prefix =
                    std::path::Path::new(cwd_canon).starts_with(std::path::Path::new(resolved));
                if is_prefix && resolved.len() > best_literal_len {
                    best_literal_len = resolved.len();
                    best_literal = Some(ws_override);
                }
            } else if let Ok(re) = regex::Regex::new(key) {
                if re.is_match(cwd_canon) && key.len() > best_regex_len {
                    best_regex_len = key.len();
                    best_regex = Some(ws_override);
                }
            }
        }

        best_literal
            .or(best_regex)
            .and_then(|o| o.default_ref.as_deref())
            .or(self.review.default_ref.as_deref())
    }

    /// Returns the state directory for persisting review/thread data.
    pub fn state_dir(&self) -> std::path::PathBuf {
        self.review
            .state_dir
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share/nvim/arbiter")
            })
    }
}

/// Global config instance.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Fallback when setup() has not been called.
static DEFAULT: OnceLock<Config> = OnceLock::new();

/// Stores config in the global OnceLock.
///
/// Called from setup() after deserializing. Succeeds only on first call.
pub fn set_config(config: Config) {
    let _ = CONFIG.set(config);
}

/// Returns the global config.
///
/// Returns default config if setup() has not been called or deserialization failed.
pub fn get() -> &'static Config {
    CONFIG
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Config::default))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn val(s: &str) -> Value {
        Value::String(s.to_string())
    }

    #[test]
    fn split_cmd_top() {
        assert_eq!(PanelPosition::Top.split_cmd(20), "topleft 20split");
    }

    #[test]
    fn split_cmd_bottom() {
        assert_eq!(PanelPosition::Bottom.split_cmd(30), "botright 30split");
    }

    #[test]
    fn split_cmd_left() {
        assert_eq!(
            PanelPosition::Left.split_cmd(40),
            "topleft vertical 40split"
        );
    }

    #[test]
    fn split_cmd_right() {
        assert_eq!(
            PanelPosition::Right.split_cmd(60),
            "botright vertical 60split"
        );
    }

    #[test]
    fn split_cmd_clamps_small_size_to_five() {
        assert_eq!(PanelPosition::Top.split_cmd(3), "topleft 5split");
        assert_eq!(PanelPosition::Top.split_cmd(0), "topleft 5split");
        assert_eq!(PanelPosition::Top.split_cmd(4), "topleft 5split");
        assert_eq!(PanelPosition::Top.split_cmd(5), "topleft 5split");
        assert_eq!(PanelPosition::Top.split_cmd(6), "topleft 6split");
    }

    #[test]
    fn is_vertical_left_right() {
        assert!(PanelPosition::Left.is_vertical());
        assert!(PanelPosition::Right.is_vertical());
    }

    #[test]
    fn is_vertical_top_bottom() {
        assert!(!PanelPosition::Top.is_vertical());
        assert!(!PanelPosition::Bottom.is_vertical());
    }

    #[test]
    fn backend_kind_deser_lowercase() {
        assert_eq!(
            serde_json::from_value::<BackendKind>(val("cursor")).unwrap(),
            BackendKind::Cursor
        );
        assert_eq!(
            serde_json::from_value::<BackendKind>(val("claude")).unwrap(),
            BackendKind::Claude
        );
    }

    #[test]
    fn backend_kind_deser_case_insensitive() {
        assert_eq!(
            serde_json::from_value::<BackendKind>(val("CURSOR")).unwrap(),
            BackendKind::Cursor
        );
        assert_eq!(
            serde_json::from_value::<BackendKind>(val("Claude")).unwrap(),
            BackendKind::Claude
        );
    }

    #[test]
    fn backend_kind_deser_invalid() {
        assert!(serde_json::from_value::<BackendKind>(val("openai")).is_err());
    }

    #[test]
    fn file_panel_kind_deser_builtin() {
        assert_eq!(
            serde_json::from_value::<FilePanelKind>(val("builtin")).unwrap(),
            FilePanelKind::Builtin
        );
    }

    #[test]
    fn file_panel_kind_deser_nvim_tree_variants() {
        for input in &["nvim-tree", "nvim_tree", "nvimtree"] {
            assert_eq!(
                serde_json::from_value::<FilePanelKind>(val(input)).unwrap(),
                FilePanelKind::NvimTree,
                "expected NvimTree for input '{input}'"
            );
        }
    }

    #[test]
    fn file_panel_kind_deser_invalid() {
        assert!(serde_json::from_value::<FilePanelKind>(val("neo-tree")).is_err());
    }

    #[test]
    fn diff_style_deser_lowercase() {
        assert_eq!(
            serde_json::from_value::<DiffStyle>(val("full")).unwrap(),
            DiffStyle::Full
        );
        assert_eq!(
            serde_json::from_value::<DiffStyle>(val("signs")).unwrap(),
            DiffStyle::Signs
        );
    }

    #[test]
    fn diff_style_deser_case_insensitive() {
        assert_eq!(
            serde_json::from_value::<DiffStyle>(val("Full")).unwrap(),
            DiffStyle::Full
        );
        assert_eq!(
            serde_json::from_value::<DiffStyle>(val("SIGNS")).unwrap(),
            DiffStyle::Signs
        );
    }

    #[test]
    fn diff_style_deser_invalid() {
        assert!(serde_json::from_value::<DiffStyle>(val("inline")).is_err());
    }

    #[test]
    fn panel_position_deser_all_variants() {
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("top")).unwrap(),
            PanelPosition::Top
        );
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("bottom")).unwrap(),
            PanelPosition::Bottom
        );
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("left")).unwrap(),
            PanelPosition::Left
        );
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("right")).unwrap(),
            PanelPosition::Right
        );
    }

    #[test]
    fn panel_position_deser_case_insensitive() {
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("RIGHT")).unwrap(),
            PanelPosition::Right
        );
        assert_eq!(
            serde_json::from_value::<PanelPosition>(val("Top")).unwrap(),
            PanelPosition::Top
        );
    }

    #[test]
    fn panel_position_deser_invalid() {
        assert!(serde_json::from_value::<PanelPosition>(val("center")).is_err());
    }

    fn test_config(default_ref: Option<&str>, workspaces: Vec<(&str, Option<&str>)>) -> Config {
        let mut ws = HashMap::new();
        for (key, dref) in workspaces {
            ws.insert(
                key.to_string(),
                WorkspaceOverride {
                    default_ref: dref.map(|s| s.to_string()),
                },
            );
        }
        Config {
            review: ReviewConfig {
                default_ref: default_ref.map(|s| s.to_string()),
                ..ReviewConfig::default()
            },
            workspaces: ws,
            ..Config::default()
        }
    }

    #[test]
    fn default_ref_for_falls_back_to_review_default() {
        let cfg = test_config(Some("main"), vec![]);
        assert_eq!(cfg.default_ref_for("/some/path"), Some("main"));
    }

    #[test]
    fn default_ref_for_literal_path_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let path_str = path.to_str().unwrap();

        let cfg = test_config(Some("main"), vec![(path_str, Some("develop"))]);
        assert_eq!(cfg.default_ref_for(path_str), Some("develop"));
    }

    #[test]
    fn default_ref_for_literal_prefix_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let parent_str = path.to_str().unwrap();
        let child = path.join("sub/project");
        std::fs::create_dir_all(&child).unwrap();
        let child_str = child.to_str().unwrap();

        let cfg = test_config(Some("main"), vec![(parent_str, Some("develop"))]);
        assert_eq!(cfg.default_ref_for(child_str), Some("develop"));
    }

    #[test]
    fn default_ref_for_regex_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let path_str = path.to_str().unwrap();

        let cfg = test_config(Some("main"), vec![(".*", Some("trunk"))]);
        assert_eq!(cfg.default_ref_for(path_str), Some("trunk"));
    }

    #[test]
    fn default_ref_for_literal_beats_regex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let path_str = path.to_str().unwrap();

        let cfg = test_config(
            Some("main"),
            vec![(path_str, Some("develop")), (".*", Some("trunk"))],
        );
        assert_eq!(cfg.default_ref_for(path_str), Some("develop"));
    }

    #[test]
    fn default_ref_for_workspace_override_with_no_ref_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let path_str = path.to_str().unwrap();

        let cfg = test_config(Some("main"), vec![(path_str, None)]);
        assert_eq!(cfg.default_ref_for(path_str), Some("main"));
    }

    #[test]
    fn default_ref_for_empty_workspaces_no_default() {
        let cfg = test_config(None, vec![]);
        assert_eq!(cfg.default_ref_for("/some/path"), None);
    }

    #[test]
    fn state_dir_custom() {
        let cfg = Config {
            review: ReviewConfig {
                state_dir: Some("/tmp/arbiter-test-state".to_string()),
                ..ReviewConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            cfg.state_dir(),
            std::path::PathBuf::from("/tmp/arbiter-test-state")
        );
    }

    #[test]
    fn state_dir_default() {
        let cfg = Config::default();
        let home = std::env::var("HOME").unwrap_or("/tmp".to_string());
        let expected = std::path::PathBuf::from(home).join(".local/share/nvim/arbiter");
        assert_eq!(cfg.state_dir(), expected);
    }
}
