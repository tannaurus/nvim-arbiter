//! Prompt formatting for backend calls.
//!
//! Assembles comments with file, line, surrounding code context,
//! and the file's diff against the comparison branch.

/// Review-level context injected into every thread prompt.
#[derive(Debug)]
pub(crate) struct ReviewContext<'a> {
    /// Branch being compared against (e.g. "main"). Empty = working tree.
    pub ref_name: &'a str,
    /// Full unified diff for the current file. Gives the agent visibility
    /// into what was added, removed, or changed - not just the current source.
    pub file_diff: &'a str,
    /// Generalizable conventions extracted from resolved threads.
    pub review_rules: &'a [String],
}

/// Formats a comment prompt with file, line, and surrounding code context.
///
/// Used when sending new comments to the agent.
pub(crate) fn format_comment_prompt(
    file: &str,
    line: u32,
    comment: &str,
    anchor_content: &str,
    context: &[String],
    review_ctx: &ReviewContext<'_>,
) -> String {
    let preamble = format_review_preamble(review_ctx, file);
    let ctx_block = format_context_block(file, line, anchor_content, context);
    format!("{preamble}{ctx_block}\nComment: {comment}")
}

/// Formats a reply prompt with file/line context.
///
/// When resuming an existing session the agent already has prior conversation
/// history, but including the location keeps the reply grounded. When starting
/// a new session (no prior session_id) this context is essential.
pub(crate) fn format_reply_prompt(
    file: &str,
    line: u32,
    reply: &str,
    anchor_content: &str,
    context: &[String],
    prior_messages: &[(String, String)],
    review_ctx: &ReviewContext<'_>,
) -> String {
    let preamble = format_review_preamble(review_ctx, file);
    let ctx_block = format_context_block(file, line, anchor_content, context);
    let mut prompt = format!("{preamble}{ctx_block}");

    if !prior_messages.is_empty() {
        prompt.push_str("\n\nThread history:\n");
        for (role, text) in prior_messages {
            prompt.push_str(&format!("[{role}]: {text}\n"));
        }
    }

    prompt.push_str(&format!("\nReply: {reply}"));
    prompt
}

/// Formats the extraction prompt that asks the agent to distill generalizable
/// coding conventions from a thread conversation.
///
/// Returns `None` if the thread has fewer than 3 messages. This ensures the
/// user has responded at least once after the agent's initial reply, so
/// extraction only runs after the user has had a chance to confirm or reject
/// the agent's direction.
pub(crate) fn format_extraction_prompt(
    messages: &[(String, String)],
    existing_rules: &[String],
) -> Option<String> {
    if messages.len() < 3 {
        return None;
    }
    let mut prompt = String::from(
        "Below is a code review thread between a reviewer and an AI agent.\n\n\
         Your task: determine whether the reviewer has EXPLICITLY confirmed or approved \
         a coding convention, style rule, or pattern that should apply broadly across \
         the codebase.\n\n\
         Rules for extraction:\n\
         - Only extract a rule if the reviewer has clearly signed off on it. Look for \
         explicit approval: \"yes\", \"that's right\", \"good\", \"do that everywhere\", \
         \"always use X\", agreement with the agent's approach, etc.\n\
         - Do NOT extract rules from the reviewer's initial comment alone. They may be \
         exploring, asking a question, or proposing something they haven't committed to.\n\
         - Do NOT extract rules from the agent's suggestions unless the reviewer explicitly \
         agreed with them.\n\
         - Do NOT extract rules about one-off fixes, specific variable names, or decisions \
         that only apply to the code under review.\n\
         - Do NOT repeat or re-add any existing rule. If a convention is already covered by \
         an existing rule, do not emit it again.\n\
         - If this thread's discussion refines or clarifies an existing rule, you may \
         rephrase it using the REPHRASE format shown below. Only rephrase when the \
         reviewer's feedback genuinely changes the meaning or scope of the rule.\n\
         - Be very conservative. When in doubt, respond with NONE. Most threads should \
         produce no rules.\n\n",
    );

    if !existing_rules.is_empty() {
        prompt.push_str("Existing rules (do NOT repeat these):\n");
        for rule in existing_rules {
            prompt.push_str(&format!("- {rule}\n"));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "If no confirmed, broadly-applicable rules were established, respond with exactly: NONE\n\n\
         Otherwise, output each action on its own line:\n\
         - New rule: RULE|<rule text>\n\
         - Rephrase existing rule: REPHRASE|<exact existing rule text>|<new wording>\n\n\
         Examples:\n\
         RULE|Prefer map_err over match for error transformation\n\
         REPHRASE|Use constants instead of string literals|Use constants or enums instead of repeated string literals\n\n\
         Thread:\n",
    );
    for (role, text) in messages {
        prompt.push_str(&format!("[{role}]: {text}\n"));
    }
    Some(prompt)
}

/// An action parsed from the extraction response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtractionAction {
    /// A newly established rule.
    Add(String),
    /// Replace an existing rule's text with a refined version.
    Rephrase { old: String, new: String },
}

/// Parses the extraction response for `RULE|` and `REPHRASE|` lines.
///
/// Returns an empty vec if the response contains "NONE" or no valid actions.
pub(crate) fn parse_extraction_response(response: &str) -> Vec<ExtractionAction> {
    if response.trim() == "NONE" {
        return Vec::new();
    }
    response
        .lines()
        .filter_map(|line| {
            if let Some(rule) = line.strip_prefix("RULE|") {
                let rule = rule.trim();
                if rule.is_empty() {
                    return None;
                }
                return Some(ExtractionAction::Add(rule.to_string()));
            }
            if let Some(rest) = line.strip_prefix("REPHRASE|") {
                let parts: Vec<&str> = rest.splitn(2, '|').collect();
                if parts.len() == 2 {
                    let old = parts[0].trim();
                    let new = parts[1].trim();
                    if !old.is_empty() && !new.is_empty() {
                        return Some(ExtractionAction::Rephrase {
                            old: old.to_string(),
                            new: new.to_string(),
                        });
                    }
                }
            }
            None
        })
        .collect()
}

fn format_review_preamble(ctx: &ReviewContext<'_>, file: &str) -> String {
    let mut out = String::new();

    if !ctx.ref_name.is_empty() {
        out.push_str(&format!(
            "This is a code review of changes on the working branch compared against `{}`.\n\n",
            ctx.ref_name
        ));
    }

    if !ctx.review_rules.is_empty() {
        out.push_str("Reviewer preferences (established during this review):\n");
        for rule in ctx.review_rules {
            out.push_str(&format!("- {rule}\n"));
        }
        out.push('\n');
    }

    if !ctx.file_diff.is_empty() {
        out.push_str(&format!(
            "Diff for {file} (lines prefixed with `-` were removed, `+` were added):\n```diff\n{}\n```\n\n",
            ctx.file_diff.trim_end()
        ));
    }

    out
}

fn format_context_block(file: &str, line: u32, anchor_content: &str, context: &[String]) -> String {
    let mut ctx_lines = String::new();
    let line_start = line.saturating_sub(2).max(1);
    for (i, l) in context.iter().enumerate() {
        let ln = line_start + i as u32;
        let marker = if l == anchor_content { " <--" } else { "" };
        ctx_lines.push_str(&format!("{ln:4}| {l}{marker}\n"));
    }
    format!(
        "File: {file}\nLine: {line}\n\nContext:\n{}",
        ctx_lines.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_prompt_none_for_single_message() {
        let msgs = vec![("user".to_string(), "fix this".to_string())];
        assert!(format_extraction_prompt(&msgs, &[]).is_none());
    }

    #[test]
    fn extraction_prompt_none_for_two_messages() {
        let msgs = vec![
            ("user".to_string(), "use map_err here".to_string()),
            ("agent".to_string(), "done".to_string()),
        ];
        assert!(format_extraction_prompt(&msgs, &[]).is_none());
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let msgs = vec![
            ("user".to_string(), "use map_err here".to_string()),
            ("agent".to_string(), "done".to_string()),
            ("user".to_string(), "yes, always do that".to_string()),
        ];
        let prompt = format_extraction_prompt(&msgs, &[]).unwrap();
        assert!(prompt.contains("[user]: use map_err here"));
        assert!(prompt.contains("[agent]: done"));
        assert!(prompt.contains("[user]: yes, always do that"));
        assert!(prompt.contains("RULE|"));
    }

    #[test]
    fn extraction_prompt_includes_existing_rules() {
        let msgs = vec![
            ("user".to_string(), "use map_err here".to_string()),
            ("agent".to_string(), "done".to_string()),
            ("user".to_string(), "yes".to_string()),
        ];
        let existing = vec!["Prefer constants".to_string()];
        let prompt = format_extraction_prompt(&msgs, &existing).unwrap();
        assert!(prompt.contains("Existing rules"));
        assert!(prompt.contains("- Prefer constants"));
    }

    #[test]
    fn extraction_prompt_omits_existing_rules_section_when_empty() {
        let msgs = vec![
            ("user".to_string(), "a".to_string()),
            ("agent".to_string(), "b".to_string()),
            ("user".to_string(), "c".to_string()),
        ];
        let prompt = format_extraction_prompt(&msgs, &[]).unwrap();
        assert!(!prompt.contains("Existing rules"));
    }

    #[test]
    fn parse_extraction_none_response() {
        assert!(parse_extraction_response("NONE").is_empty());
        assert!(parse_extraction_response("  NONE  ").is_empty());
    }

    #[test]
    fn parse_extraction_valid_rules() {
        let response = "RULE|Use map_err over match\nRULE|Prefer constants\nsome other text\n";
        let actions = parse_extraction_response(response);
        assert_eq!(
            actions,
            vec![
                ExtractionAction::Add("Use map_err over match".to_string()),
                ExtractionAction::Add("Prefer constants".to_string()),
            ]
        );
    }

    #[test]
    fn parse_extraction_empty_rule_skipped() {
        let response = "RULE|\nRULE|  \nRULE|Real rule\n";
        let actions = parse_extraction_response(response);
        assert_eq!(
            actions,
            vec![ExtractionAction::Add("Real rule".to_string())]
        );
    }

    #[test]
    fn parse_extraction_rephrase() {
        let response = "REPHRASE|Use constants|Use constants or enums instead of string literals\n";
        let actions = parse_extraction_response(response);
        assert_eq!(
            actions,
            vec![ExtractionAction::Rephrase {
                old: "Use constants".to_string(),
                new: "Use constants or enums instead of string literals".to_string(),
            }]
        );
    }

    #[test]
    fn parse_extraction_mixed_actions() {
        let response = "RULE|New rule\nREPHRASE|Old text|Better text\nRULE|Another rule\ngarbage\n";
        let actions = parse_extraction_response(response);
        assert_eq!(
            actions,
            vec![
                ExtractionAction::Add("New rule".to_string()),
                ExtractionAction::Rephrase {
                    old: "Old text".to_string(),
                    new: "Better text".to_string(),
                },
                ExtractionAction::Add("Another rule".to_string()),
            ]
        );
    }

    #[test]
    fn parse_extraction_rephrase_missing_parts() {
        let response = "REPHRASE|only one part\nREPHRASE||\nREPHRASE|old|\n";
        let actions = parse_extraction_response(response);
        assert!(actions.is_empty());
    }

    fn empty_ctx() -> ReviewContext<'static> {
        ReviewContext {
            ref_name: "",
            file_diff: "",
            review_rules: &[],
        }
    }

    #[test]
    fn preamble_empty_when_no_context() {
        let ctx = empty_ctx();
        assert!(format_review_preamble(&ctx, "a.rs").is_empty());
    }

    #[test]
    fn preamble_includes_ref_name() {
        let ctx = ReviewContext {
            ref_name: "main",
            ..empty_ctx()
        };
        let p = format_review_preamble(&ctx, "a.rs");
        assert!(p.contains("compared against `main`"));
    }

    #[test]
    fn preamble_includes_rules() {
        let rules = vec!["Rule A".to_string(), "Rule B".to_string()];
        let ctx = ReviewContext {
            review_rules: &rules,
            ..empty_ctx()
        };
        let p = format_review_preamble(&ctx, "a.rs");
        assert!(p.contains("Reviewer preferences"));
        assert!(p.contains("- Rule A\n"));
        assert!(p.contains("- Rule B\n"));
    }

    #[test]
    fn preamble_includes_diff() {
        let ctx = ReviewContext {
            file_diff: "-old line\n+new line",
            ..empty_ctx()
        };
        let p = format_review_preamble(&ctx, "a.rs");
        assert!(p.contains("Diff for a.rs"));
        assert!(p.contains("```diff"));
        assert!(p.contains("-old line"));
        assert!(p.contains("+new line"));
    }

    #[test]
    fn comment_prompt_includes_diff_and_ref() {
        let rules = vec!["Use constants".to_string()];
        let ctx = ReviewContext {
            ref_name: "main",
            file_diff: "-removed\n+added",
            review_rules: &rules,
        };
        let prompt = format_comment_prompt("a.rs", 5, "fix", "let x = 1;", &[], &ctx);
        assert!(prompt.contains("compared against `main`"));
        assert!(prompt.contains("-removed"));
        assert!(prompt.contains("- Use constants"));
        assert!(prompt.contains("Comment: fix"));
    }

    #[test]
    fn comment_prompt_no_context_no_preamble() {
        let ctx = empty_ctx();
        let prompt = format_comment_prompt("a.rs", 5, "fix", "let x = 1;", &[], &ctx);
        assert!(prompt.starts_with("File: a.rs"));
    }

    #[test]
    fn reply_prompt_includes_diff_context() {
        let rules = vec!["Prefer map_err".to_string()];
        let ctx = ReviewContext {
            ref_name: "develop",
            file_diff: "@@ -1,3 +1,3 @@",
            review_rules: &rules,
        };
        let msgs = vec![("user".to_string(), "hi".to_string())];
        let prompt = format_reply_prompt("a.rs", 5, "ok", "let x = 1;", &[], &msgs, &ctx);
        assert!(prompt.contains("compared against `develop`"));
        assert!(prompt.contains("- Prefer map_err"));
        assert!(prompt.contains("Reply: ok"));
    }
}
