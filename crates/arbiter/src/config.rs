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
pub(crate) struct Config {
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
        }
    }
}

/// Per-workspace configuration overrides.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspaceOverride {
    /// Default ref branch for this workspace (e.g. "develop", "trunk").
    pub default_ref: Option<String>,
}

/// Backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BackendKind {
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
pub(crate) enum FilePanelKind {
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
pub(crate) struct IconConfig {
    /// Icon shown for approved files. Examples: "✔", "", "👍"
    pub approved: Option<String>,
    /// Icon shown for files needing changes. Examples: "✘", "", "❌"
    pub needs_changes: Option<String>,
    /// Icon shown for unreviewed files. Examples: "○", "", "⏳"
    pub unreviewed: Option<String>,
}

/// Diff highlighting style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DiffStyle {
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
pub(crate) struct ReviewConfig {
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
pub(crate) struct ThreadWindowConfig {
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
pub(crate) enum PanelPosition {
    Top,
    Bottom,
    Left,
    Right,
}

impl PanelPosition {
    /// Returns the vim split command prefix and whether the split is vertical.
    pub(crate) fn split_cmd(self, size: u32) -> String {
        let size = size.max(5);
        match self {
            PanelPosition::Top => format!("topleft {size}split"),
            PanelPosition::Bottom => format!("botright {size}split"),
            PanelPosition::Left => format!("topleft vertical {size}split"),
            PanelPosition::Right => format!("botright vertical {size}split"),
        }
    }

    pub(crate) fn is_vertical(self) -> bool {
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
pub(crate) const SELF_REVIEW_FORMAT_SUFFIX: &str = concat!(
    " For each concern, output a line in exactly this format:\n\n",
    "THREAD|file/path|line_number|your question or concern\n\n",
    "Example:\n",
    "THREAD|src/lib.rs|42|This unwrap could panic if the input is empty\n\n",
    "Output ONLY THREAD lines, no other text.",
);

/// Prompt template configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct PromptConfig {
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
pub(crate) struct KeymapConfig {
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
    /// List only resolved (binned) threads.
    pub list_threads_binned: String,
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
            list_threads_binned: "<Leader>atb".to_string(),
            list_threads_open: "<Leader>ato".to_string(),
            resolve_thread: "<Leader>aR".to_string(),
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
    pub(crate) fn default_ref_for(&self, cwd: &str) -> Option<&str> {
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

                if cwd_canon.starts_with(resolved) && resolved.len() > best_literal_len {
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
    pub(crate) fn state_dir(&self) -> std::path::PathBuf {
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
pub(crate) fn set_config(config: Config) {
    let _ = CONFIG.set(config);
}

/// Returns the global config.
///
/// Returns default config if setup() has not been called or deserialization failed.
pub(crate) fn get() -> &'static Config {
    CONFIG
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Config::default))
}
