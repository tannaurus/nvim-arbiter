//! Prompt formatting for backend calls.
//!
//! Assembles comments with file, line, surrounding code context,
//! and the file's diff against the comparison branch.

/// Review-level context injected into every thread prompt.
#[derive(Debug)]
pub struct ReviewContext<'a> {
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
pub fn format_comment_prompt(
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
pub fn format_reply_prompt(
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
/// coding conventions from a resolved thread conversation.
///
/// Returns `None` if the thread has fewer than 2 messages (nothing to extract from).
pub fn format_extraction_prompt(messages: &[(String, String)]) -> Option<String> {
    if messages.len() < 2 {
        return None;
    }
    let mut prompt = String::from(
        "Below is a completed code review thread between a reviewer and an AI agent. \
         Extract any general coding conventions, style rules, or patterns that the reviewer \
         established. Only include rules that would apply broadly across the codebase, not \
         feedback specific to this particular code.\n\n\
         If no generalizable rules were established, respond with exactly: NONE\n\n\
         Otherwise, output each rule on its own line, prefixed with \"RULE|\". Example:\n\
         RULE|Prefer map_err over match for error transformation\n\
         RULE|Use constants instead of repeated string literals\n\n\
         Thread:\n",
    );
    for (role, text) in messages {
        prompt.push_str(&format!("[{role}]: {text}\n"));
    }
    Some(prompt)
}

/// Parses the extraction response for `RULE|` prefixed lines.
///
/// Returns an empty vec if the response contains "NONE" or no valid rules.
pub fn parse_extraction_response(response: &str) -> Vec<String> {
    if response.trim() == "NONE" {
        return Vec::new();
    }
    response
        .lines()
        .filter_map(|line| line.strip_prefix("RULE|").map(|r| r.trim().to_string()))
        .filter(|r| !r.is_empty())
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
        assert!(format_extraction_prompt(&msgs).is_none());
    }

    #[test]
    fn extraction_prompt_includes_messages() {
        let msgs = vec![
            ("user".to_string(), "use map_err here".to_string()),
            ("agent".to_string(), "done".to_string()),
        ];
        let prompt = format_extraction_prompt(&msgs).unwrap();
        assert!(prompt.contains("[user]: use map_err here"));
        assert!(prompt.contains("[agent]: done"));
        assert!(prompt.contains("RULE|"));
    }

    #[test]
    fn parse_extraction_none_response() {
        assert!(parse_extraction_response("NONE").is_empty());
        assert!(parse_extraction_response("  NONE  ").is_empty());
    }

    #[test]
    fn parse_extraction_valid_rules() {
        let response = "RULE|Use map_err over match\nRULE|Prefer constants\nsome other text\n";
        let rules = parse_extraction_response(response);
        assert_eq!(rules, vec!["Use map_err over match", "Prefer constants"]);
    }

    #[test]
    fn parse_extraction_empty_rule_skipped() {
        let response = "RULE|\nRULE|  \nRULE|Real rule\n";
        let rules = parse_extraction_response(response);
        assert_eq!(rules, vec!["Real rule"]);
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
