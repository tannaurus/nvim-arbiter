//! Project rules: persistent, file-aware instructions loaded from disk.
//!
//! Rules are markdown files with TOML frontmatter in `.arbiter/rules/`
//! (workspace), `~/.config/arbiter/rules/` (global), or user-configured
//! directories. They are matched by file glob and scenario, then injected
//! into agent prompts.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// When a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scenario {
    /// Any agent conversation anchored to a file (comment, reply, apply).
    Thread,
    /// The `:ArbiterSelfReview` sweep of the whole diff.
    SelfReview,
}

/// Where the rule was loaded from (later sources win on description conflicts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuleSource {
    Global,
    Workspace,
    Custom(String),
}

/// A single project rule loaded from a markdown file.
#[derive(Debug, Clone)]
pub(crate) struct Rule {
    /// Human-readable name (from frontmatter `description`).
    pub description: String,
    /// The instruction body (everything after the closing `---`).
    pub content: String,
    /// Glob patterns matched against the file path. Empty = always match.
    pub match_patterns: Vec<String>,
    /// Which scenarios this rule applies to. Empty = all scenarios.
    pub scenarios: Vec<Scenario>,
    /// Where the rule was loaded from. Retained for debugging and dedup semantics.
    #[allow(dead_code)]
    pub source: RuleSource,
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    #[serde(rename = "match")]
    match_patterns: Vec<String>,
    #[serde(default)]
    scenarios: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::Single(s) => Ok(vec![s]),
        StringOrVec::Multiple(v) => Ok(v),
    }
}

/// Errors from parsing a rule file.
#[derive(Debug)]
pub(crate) struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parses a rule from the raw text of a `.md` file.
pub(crate) fn parse(text: &str, source: RuleSource) -> Result<Rule, ParseError> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return Err(ParseError(
            "missing frontmatter (no opening ---)".to_string(),
        ));
    }
    let after_open = &trimmed[3..];
    let close_pos = after_open
        .find("\n---")
        .ok_or_else(|| ParseError("missing closing --- in frontmatter".to_string()))?;
    let frontmatter_text = &after_open[..close_pos];
    let body_start = close_pos + 4; // skip "\n---"
    let body = after_open[body_start..].trim();
    if body.is_empty() {
        return Err(ParseError("rule body is empty".to_string()));
    }

    let fm: Frontmatter = toml::from_str(frontmatter_text)
        .map_err(|e| ParseError(format!("frontmatter parse error: {e}")))?;

    let description = fm
        .description
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ParseError("missing required field: description".to_string()))?;

    let scenarios: Vec<Scenario> = fm
        .scenarios
        .iter()
        .filter_map(|s| match s.as_str() {
            "thread" => Some(Scenario::Thread),
            "self_review" => Some(Scenario::SelfReview),
            _ => None,
        })
        .collect();

    Ok(Rule {
        description,
        content: body.to_string(),
        match_patterns: fm.match_patterns,
        scenarios,
        source,
    })
}

/// Loads all `.md` rule files from a directory. Returns an empty vec if the
/// directory does not exist. Malformed files are skipped with an eprintln warning.
pub(crate) fn load_from_dir(dir: &Path, source: RuleSource) -> Vec<Rule> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(rule) = parse(&text, source.clone()) {
            rules.push(rule);
        }
    }
    rules
}

/// Loads rules from all configured sources in priority order.
///
/// Later sources override earlier ones when descriptions match:
/// global < workspace < custom dirs (in order).
pub(crate) fn load_all(cwd: &str, extra_dirs: &[String]) -> Vec<Rule> {
    let mut all = Vec::new();

    let global_dir = dirs_global();
    all.extend(load_from_dir(&global_dir, RuleSource::Global));

    let workspace_dir = Path::new(cwd).join(".arbiter").join("rules");
    all.extend(load_from_dir(&workspace_dir, RuleSource::Workspace));

    for dir in extra_dirs {
        let expanded = expand_tilde(dir);
        all.extend(load_from_dir(&expanded, RuleSource::Custom(dir.clone())));
    }

    all
}

/// Resolves which rules apply for a given scenario and optional file path.
///
/// Deduplicates by description: later sources win.
pub(crate) fn resolve<'a>(
    rules: &'a [Rule],
    scenario: Scenario,
    file: Option<&str>,
) -> Vec<&'a Rule> {
    let mut matched: Vec<&Rule> = rules
        .iter()
        .filter(|r| {
            if !r.scenarios.is_empty() && !r.scenarios.contains(&scenario) {
                return false;
            }
            if r.match_patterns.is_empty() {
                return true;
            }
            let Some(f) = file else {
                return false;
            };
            r.match_patterns.iter().any(|pat| {
                glob::Pattern::new(pat)
                    .map(|p| p.matches(f))
                    .unwrap_or(false)
            })
        })
        .collect();

    dedup_by_description(&mut matched);
    matched
}

/// Formats resolved rules into a block for prompt injection.
/// Returns an empty string if no rules matched.
pub(crate) fn format_for_prompt(rules: &[&Rule]) -> String {
    if rules.is_empty() {
        return String::new();
    }
    let mut out = String::from("Project rules:\n");
    for r in rules {
        out.push_str(&format!("- [{}] {}\n", r.description, r.content));
    }
    out.push('\n');
    out
}

fn dedup_by_description(rules: &mut Vec<&Rule>) {
    let mut seen = std::collections::HashMap::new();
    for (i, r) in rules.iter().enumerate() {
        seen.insert(r.description.as_str(), i);
    }
    let keep: std::collections::HashSet<usize> = seen.into_values().collect();
    let mut idx = 0;
    rules.retain(|_| {
        let k = keep.contains(&idx);
        idx += 1;
        k
    });
}

fn dirs_global() -> PathBuf {
    dirs_home()
        .map(|h| h.join(".config").join("arbiter").join("rules"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs_home() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_text(fm: &str, body: &str) -> String {
        format!("---\n{fm}\n---\n{body}")
    }

    #[test]
    fn parse_valid_rule() {
        let text = rule_text(
            "description = \"Rust style\"\nmatch = [\"**/*.rs\", \"**/*.toml\"]\nscenarios = [\"thread\"]",
            "Use map_err over match.",
        );
        let r = parse(&text, RuleSource::Workspace).unwrap();
        assert_eq!(r.description, "Rust style");
        assert_eq!(r.match_patterns, vec!["**/*.rs", "**/*.toml"]);
        assert_eq!(r.scenarios, vec![Scenario::Thread]);
        assert_eq!(r.content, "Use map_err over match.");
    }

    #[test]
    fn parse_minimal_frontmatter() {
        let text = rule_text("description = \"Always applies\"", "Do the thing.");
        let r = parse(&text, RuleSource::Global).unwrap();
        assert_eq!(r.description, "Always applies");
        assert!(r.match_patterns.is_empty());
        assert!(r.scenarios.is_empty());
        assert_eq!(r.content, "Do the thing.");
    }

    #[test]
    fn parse_missing_description_errors() {
        let text = rule_text("match = \"*.rs\"", "body");
        assert!(parse(&text, RuleSource::Global).is_err());
    }

    #[test]
    fn parse_match_single_string() {
        let text = rule_text("description = \"X\"\nmatch = \"*.rs\"", "body");
        let r = parse(&text, RuleSource::Global).unwrap();
        assert_eq!(r.match_patterns, vec!["*.rs"]);
    }

    #[test]
    fn parse_match_list() {
        let text = rule_text(
            "description = \"X\"\nmatch = [\"*.rs\", \"*.toml\"]",
            "body",
        );
        let r = parse(&text, RuleSource::Global).unwrap();
        assert_eq!(r.match_patterns, vec!["*.rs", "*.toml"]);
    }

    #[test]
    fn parse_scenarios_subset() {
        let text = rule_text("description = \"X\"\nscenarios = [\"thread\"]", "body");
        let r = parse(&text, RuleSource::Global).unwrap();
        assert_eq!(r.scenarios, vec![Scenario::Thread]);
    }

    #[test]
    fn parse_unknown_scenario_ignored() {
        let text = rule_text(
            "description = \"X\"\nscenarios = [\"thread\", \"future_thing\"]",
            "body",
        );
        let r = parse(&text, RuleSource::Global).unwrap();
        assert_eq!(r.scenarios, vec![Scenario::Thread]);
    }

    #[test]
    fn parse_no_frontmatter_errors() {
        let text = "Just some text without frontmatter.";
        assert!(parse(text, RuleSource::Global).is_err());
    }

    #[test]
    fn parse_body_after_frontmatter() {
        let text = "---\ndescription = \"X\"\n---\n\n  Hello world  \n\n";
        let r = parse(text, RuleSource::Global).unwrap();
        assert_eq!(r.content, "Hello world");
    }

    #[test]
    fn parse_empty_body_errors() {
        let text = "---\ndescription = \"X\"\n---\n   \n";
        assert!(parse(text, RuleSource::Global).is_err());
    }

    #[test]
    fn parse_whitespace_only_description() {
        let text = rule_text("description = \"   \t  \"", "body");
        let err = parse(&text, RuleSource::Global).unwrap_err();
        assert!(
            err.0.contains("description"),
            "expected missing-description error, got: {err}"
        );
    }

    #[test]
    fn parse_no_leading_newline_delimiter() {
        let text = "---\ndescription = \"x\"\n---\nbody";
        let r = parse(text, RuleSource::Global).unwrap();
        assert_eq!(r.description, "x");
        assert_eq!(r.content, "body");
    }

    fn make_rule(desc: &str, globs: &[&str], scenarios: Vec<Scenario>, source: RuleSource) -> Rule {
        Rule {
            description: desc.to_string(),
            content: "body".to_string(),
            match_patterns: globs.iter().map(|s| s.to_string()).collect(),
            scenarios,
            source,
        }
    }

    #[test]
    fn resolve_thread_matches_glob() {
        let rules = [make_rule("Rust", &["**/*.rs"], vec![], RuleSource::Global)];
        let matched = resolve(&rules, Scenario::Thread, Some("src/lib.rs"));
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn resolve_thread_glob_no_match() {
        let rules = [make_rule("Rust", &["**/*.rs"], vec![], RuleSource::Global)];
        let matched = resolve(&rules, Scenario::Thread, Some("README.md"));
        assert!(matched.is_empty());
    }

    #[test]
    fn resolve_self_review_skips_glob_rules() {
        let rules = [make_rule("Rust", &["**/*.rs"], vec![], RuleSource::Global)];
        let matched = resolve(&rules, Scenario::SelfReview, None);
        assert!(matched.is_empty());
    }

    #[test]
    fn resolve_self_review_includes_matchless() {
        let rules = [make_rule("General", &[], vec![], RuleSource::Global)];
        let matched = resolve(&rules, Scenario::SelfReview, None);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn resolve_scenario_filter() {
        let rules = [make_rule(
            "Thread only",
            &[],
            vec![Scenario::Thread],
            RuleSource::Global,
        )];
        let matched = resolve(&rules, Scenario::SelfReview, None);
        assert!(matched.is_empty());
    }

    #[test]
    fn resolve_no_scenario_restriction() {
        let rules = [make_rule("All", &[], vec![], RuleSource::Global)];
        assert_eq!(resolve(&rules, Scenario::Thread, Some("a.rs")).len(), 1);
        assert_eq!(resolve(&rules, Scenario::SelfReview, None).len(), 1);
    }

    #[test]
    fn resolve_workspace_overrides_global() {
        let mut g = make_rule("Same", &[], vec![], RuleSource::Global);
        g.content = "global".to_string();
        let mut w = make_rule("Same", &[], vec![], RuleSource::Workspace);
        w.content = "workspace".to_string();
        let rules = [g, w];
        let matched = resolve(&rules, Scenario::Thread, Some("a.rs"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].content, "workspace");
    }

    #[test]
    fn resolve_custom_overrides_workspace() {
        let mut w = make_rule("Same", &[], vec![], RuleSource::Workspace);
        w.content = "workspace".to_string();
        let mut c = make_rule("Same", &[], vec![], RuleSource::Custom("x".to_string()));
        c.content = "custom".to_string();
        let rules = [w, c];
        let matched = resolve(&rules, Scenario::Thread, Some("a.rs"));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].content, "custom");
    }

    #[test]
    fn resolve_multiple_globs() {
        let rules = [make_rule(
            "Multi",
            &["*.rs", "*.toml"],
            vec![],
            RuleSource::Global,
        )];
        assert_eq!(resolve(&rules, Scenario::Thread, Some("lib.rs")).len(), 1);
        assert_eq!(
            resolve(&rules, Scenario::Thread, Some("Cargo.toml")).len(),
            1
        );
        assert!(resolve(&rules, Scenario::Thread, Some("file.py")).is_empty());
    }

    #[test]
    fn resolve_order_preserved() {
        let mut a = make_rule("A", &[], vec![], RuleSource::Global);
        a.content = "first".to_string();
        let mut b = make_rule("B", &[], vec![], RuleSource::Workspace);
        b.content = "second".to_string();
        let rules = [a, b];
        let matched = resolve(&rules, Scenario::Thread, Some("a.rs"));
        assert_eq!(matched[0].description, "A");
        assert_eq!(matched[1].description, "B");
    }

    #[test]
    fn resolve_invalid_glob_no_match() {
        let rules = [make_rule(
            "Bad glob",
            &["[invalid"],
            vec![],
            RuleSource::Global,
        )];
        let matched = resolve(&rules, Scenario::Thread, Some("src/lib.rs"));
        assert!(matched.is_empty());
    }

    #[test]
    fn parse_unknown_scenarios_broadens_scope() {
        let text = rule_text("description = \"X\"\nscenarios = [\"bogus\"]", "body");
        let r = parse(&text, RuleSource::Global).unwrap();
        assert!(r.scenarios.is_empty());

        let rules = [r];
        assert_eq!(resolve(&rules, Scenario::Thread, Some("a.rs")).len(), 1);
        assert_eq!(resolve(&rules, Scenario::SelfReview, None).len(), 1);
    }

    #[test]
    fn end_to_end_parse_resolve_format() {
        let md = "\
---
description = \"Rust conventions\"
match = [\"**/*.rs\"]
scenarios = [\"thread\"]
---
Prefer map_err over match for error conversion.
Use ? for propagation.";

        let rule = parse(md, RuleSource::Workspace).unwrap();
        assert_eq!(rule.description, "Rust conventions");

        let rules = [rule];
        let matched = resolve(&rules, Scenario::Thread, Some("src/lib.rs"));
        assert_eq!(matched.len(), 1);

        let prompt = format_for_prompt(&matched);
        assert!(prompt.contains("Project rules:"));
        assert!(prompt.contains("Rust conventions"));
        assert!(prompt.contains("map_err"));
    }

    #[test]
    fn load_from_dir_finds_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("rule1.md"),
            "---\ndescription = \"R1\"\n---\nbody1",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("rule2.md"),
            "---\ndescription = \"R2\"\n---\nbody2",
        )
        .unwrap();
        std::fs::write(tmp.path().join("not-a-rule.txt"), "ignore me").unwrap();
        let rules = load_from_dir(tmp.path(), RuleSource::Workspace);
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn load_from_dir_missing_dir() {
        let rules = load_from_dir(Path::new("/nonexistent/dir/12345"), RuleSource::Global);
        assert!(rules.is_empty());
    }

    #[test]
    fn load_from_dir_skips_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("good.md"),
            "---\ndescription = \"Good\"\n---\nbody",
        )
        .unwrap();
        std::fs::write(tmp.path().join("bad.md"), "no frontmatter here").unwrap();
        let rules = load_from_dir(tmp.path(), RuleSource::Workspace);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].description, "Good");
    }

    #[test]
    fn load_all_merges_sources() {
        let global = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();

        std::fs::write(
            global.path().join("g.md"),
            "---\ndescription = \"Shared\"\n---\nglobal version",
        )
        .unwrap();
        std::fs::write(
            global.path().join("g2.md"),
            "---\ndescription = \"Global only\"\n---\nglobal only body",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("w.md"),
            "---\ndescription = \"Shared\"\n---\nworkspace version",
        )
        .unwrap();
        std::fs::write(
            custom.path().join("c.md"),
            "---\ndescription = \"Custom\"\n---\ncustom body",
        )
        .unwrap();

        let g_rules = load_from_dir(global.path(), RuleSource::Global);
        let w_rules = load_from_dir(workspace.path(), RuleSource::Workspace);
        let c_rules = load_from_dir(
            custom.path(),
            RuleSource::Custom(custom.path().to_string_lossy().to_string()),
        );

        let mut all = Vec::new();
        all.extend(g_rules);
        all.extend(w_rules);
        all.extend(c_rules);

        let resolved = resolve(&all, Scenario::Thread, Some("a.rs"));
        assert_eq!(resolved.len(), 3);

        let shared = resolved.iter().find(|r| r.description == "Shared").unwrap();
        assert_eq!(shared.content, "workspace version");
    }

    #[test]
    fn format_for_prompt_empty() {
        assert!(format_for_prompt(&[]).is_empty());
    }

    #[test]
    fn format_for_prompt_includes_content() {
        let r = Rule {
            description: "Style".to_string(),
            content: "Use constants".to_string(),
            match_patterns: vec![],
            scenarios: vec![],
            source: RuleSource::Global,
        };
        let out = format_for_prompt(&[&r]);
        assert!(out.contains("Project rules:"));
        assert!(out.contains("[Style] Use constants"));
    }

    #[test]
    fn expand_tilde_works() {
        let expanded = expand_tilde("~/foo/bar");
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn snapshot_format_for_prompt_multiple_rules() {
        let r1 = Rule {
            description: "Rust conventions".to_string(),
            content: "Prefer map_err over match for error transformation. Use ? for propagation."
                .to_string(),
            match_patterns: vec!["**/*.rs".to_string()],
            scenarios: vec![Scenario::Thread],
            source: RuleSource::Workspace,
        };
        let r2 = Rule {
            description: "API design".to_string(),
            content: "All handlers must return Result<Json<T>, RouteError>. Include StatusCode only for non-200 responses.".to_string(),
            match_patterns: vec!["**/handler*.rs".to_string()],
            scenarios: vec![],
            source: RuleSource::Global,
        };
        let r3 = Rule {
            description: "Testing".to_string(),
            content: "Use static fixtures parsed from JSON, not dynamically generated data. Group tests by function under test.".to_string(),
            match_patterns: vec![],
            scenarios: vec![Scenario::Thread, Scenario::SelfReview],
            source: RuleSource::Custom("~/.config/team/rules".to_string()),
        };
        let output = format_for_prompt(&[&r1, &r2, &r3]);
        insta::assert_snapshot!("format_for_prompt_multiple_rules", output);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn parse_never_panics(input in ".*") {
            let _ = parse(&input, RuleSource::Workspace);
        }
    }
}
