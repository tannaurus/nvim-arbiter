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
    /// Static project rules resolved for this scenario/file.
    pub project_rules: String,
}

/// Summary of a similar thread's conversation, provided as optional context
/// so the agent can draw on solutions applied to related issues.
#[derive(Debug)]
pub(crate) struct SimilarThreadContext {
    pub file: String,
    pub line: u32,
    pub status: String,
    pub messages: Vec<(String, String)>,
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

/// Thread-level context for a reply prompt.
pub(crate) struct ReplyContext<'a> {
    pub file: &'a str,
    pub line: u32,
    pub reply: &'a str,
    pub anchor_content: &'a str,
    pub context: &'a [String],
    pub prior_messages: &'a [(String, String)],
}

/// Formats a reply prompt with file/line context.
///
/// When resuming an existing session the agent already has prior conversation
/// history, but including the location keeps the reply grounded. When starting
/// a new session (no prior session_id) this context is essential.
///
/// If the thread has similar threads with relevant discussion, their
/// conversations are appended so the agent can factor in prior solutions
/// without being told to apply them blindly.
pub(crate) fn format_reply_prompt(
    thread: &ReplyContext<'_>,
    review_ctx: &ReviewContext<'_>,
    similar_threads: &[SimilarThreadContext],
) -> String {
    let preamble = format_review_preamble(review_ctx, thread.file);
    let ctx_block = format_context_block(
        thread.file,
        thread.line,
        thread.anchor_content,
        thread.context,
    );
    let mut prompt = format!("{preamble}{ctx_block}");

    if !thread.prior_messages.is_empty() {
        prompt.push_str("\n\nThread history:\n");
        for (role, text) in thread.prior_messages {
            prompt.push_str(&format!("[{role}]: {text}\n"));
        }
    }

    if !similar_threads.is_empty() {
        prompt.push_str(
            "\n\nRelated threads (for context only - apply solutions from these \
             only when directly relevant to this thread's issue):\n",
        );
        for (i, st) in similar_threads.iter().enumerate() {
            prompt.push_str(&format!(
                "\n--- Similar thread {} ({}, {}:{}) ---\n",
                i + 1,
                st.status,
                st.file,
                st.line,
            ));
            for (role, text) in &st.messages {
                prompt.push_str(&format!("[{role}]: {text}\n"));
            }
        }
    }

    prompt.push_str(&format!("\nReply: {}", thread.reply));
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

/// A single feedback item from a self-review thread.
pub(crate) struct FeedbackItem<'a> {
    pub file: &'a str,
    pub line: u32,
    pub message: &'a str,
}

/// Formats a prompt asking the agent to apply all outstanding self-review feedback.
///
/// Groups feedback items by file so the agent can work through them systematically.
/// Returns `None` if the list is empty.
pub(crate) fn format_apply_feedback_prompt(
    items: &[FeedbackItem<'_>],
    review_ctx: &ReviewContext<'_>,
) -> Option<String> {
    if items.is_empty() {
        return None;
    }

    let mut prompt = String::new();

    if !review_ctx.ref_name.is_empty() {
        prompt.push_str(&format!(
            "This is a code review of changes compared against `{}`.\n\n",
            review_ctx.ref_name
        ));
    }

    if !review_ctx.project_rules.is_empty() {
        prompt.push_str(&review_ctx.project_rules);
    }

    if !review_ctx.review_rules.is_empty() {
        prompt.push_str("Reviewer preferences (established during this review):\n");
        for rule in review_ctx.review_rules {
            prompt.push_str(&format!("- {rule}\n"));
        }
        prompt.push('\n');
    }

    prompt.push_str(&format!(
        "Apply all {} of the following review comments to the codebase. \
         Each item specifies a file, line number, and the requested change.\n\n",
        items.len()
    ));

    for (i, item) in items.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. {}:{} - {}\n",
            i + 1,
            item.file,
            item.line,
            item.message
        ));
    }

    Some(prompt)
}

/// Formats a prompt asking the agent to group self-review threads by similarity.
///
/// Each item is `(index, file, line, message)`. The agent returns `GROUP|i,j,k`
/// lines where the indices reference the input list.
/// Returns `None` if fewer than 2 items (grouping requires at least a pair).
pub(crate) fn format_similarity_prompt(items: &[(usize, &str, u32, &str)]) -> Option<String> {
    if items.len() < 2 {
        return None;
    }
    let mut prompt = String::from(
        "Below is a list of code review comments from a self-review.\n\n\
         Your task: identify groups of comments that address the SAME CLASS of problem.\n\
         Two comments are 'similar' when fixing one implies the same kind of fix should \
         be applied at the other locations - e.g. the same missing error handling pattern, \
         the same naming convention violation, the same API misuse.\n\n\
         Rules:\n\
         - Only group comments that are genuinely about the same underlying issue.\n\
         - A comment can appear in at most one group.\n\
         - Groups must have at least 2 members.\n\
         - If no comments are similar, respond with exactly: NONE\n\n\
         Output format - one line per group:\n\
         GROUP|index1,index2,...\n\n\
         Example:\n\
         GROUP|0,3,7\n\
         GROUP|2,5\n\n\
         Comments:\n",
    );
    for (idx, file, line, message) in items {
        prompt.push_str(&format!("{idx}. {file}:{line} - {message}\n"));
    }
    Some(prompt)
}

/// Parses `GROUP|0,3,7` lines into groups of indices.
pub(crate) fn parse_similarity_response(text: &str) -> Vec<Vec<usize>> {
    if text.trim() == "NONE" {
        return Vec::new();
    }
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("GROUP|")?;
            let indices: Vec<usize> = rest
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if indices.len() >= 2 {
                Some(indices)
            } else {
                None
            }
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

    if !ctx.project_rules.is_empty() {
        out.push_str(&ctx.project_rules);
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
    fn parse_extraction_none_then_rule() {
        let input = "NONE\nRULE|some rule";
        let actions = parse_extraction_response(input);
        assert_eq!(
            actions,
            vec![ExtractionAction::Add("some rule".to_string())]
        );
    }

    #[test]
    fn parse_extraction_pipe_in_text() {
        let input = "RULE|Use foo|bar pattern for errors";
        let actions = parse_extraction_response(input);
        assert_eq!(
            actions,
            vec![ExtractionAction::Add(
                "Use foo|bar pattern for errors".to_string()
            )]
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
            project_rules: String::new(),
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
    fn preamble_includes_project_rules() {
        let ctx = ReviewContext {
            project_rules: "Project rules:\n- [Style] Use constants\n\n".to_string(),
            ..empty_ctx()
        };
        let p = format_review_preamble(&ctx, "a.rs");
        assert!(p.contains("Project rules:"));
        assert!(p.contains("[Style] Use constants"));
    }

    #[test]
    fn preamble_omits_project_rules_when_empty() {
        let ctx = empty_ctx();
        let p = format_review_preamble(&ctx, "a.rs");
        assert!(!p.contains("Project rules:"));
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
            project_rules: String::new(),
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
    fn format_context_block_line_zero() {
        let result = format_context_block("test.rs", 0, "let x = 1;", &["let x = 1;".to_string()]);
        assert!(result.contains("File: test.rs"));
        assert!(result.contains("Line: 0"));
        assert!(result.contains("let x = 1;"));
    }

    #[test]
    fn apply_feedback_none_when_empty() {
        let ctx = empty_ctx();
        assert!(format_apply_feedback_prompt(&[], &ctx).is_none());
    }

    #[test]
    fn apply_feedback_includes_all_items() {
        let ctx = empty_ctx();
        let items = vec![
            FeedbackItem {
                file: "src/main.rs",
                line: 22,
                message: "Should this return 401 or 403?",
            },
            FeedbackItem {
                file: "src/auth.rs",
                line: 15,
                message: "Consider caching the token lookup.",
            },
        ];
        let prompt = format_apply_feedback_prompt(&items, &ctx).unwrap();
        assert!(prompt.contains("2 of the following review comments"));
        assert!(prompt.contains("1. src/main.rs:22 - Should this return 401 or 403?"));
        assert!(prompt.contains("2. src/auth.rs:15 - Consider caching the token lookup."));
    }

    #[test]
    fn apply_feedback_includes_review_context() {
        let rules = vec!["Prefer map_err".to_string()];
        let ctx = ReviewContext {
            ref_name: "main",
            file_diff: "",
            review_rules: &rules,
            project_rules: String::new(),
        };
        let items = vec![FeedbackItem {
            file: "a.rs",
            line: 1,
            message: "fix this",
        }];
        let prompt = format_apply_feedback_prompt(&items, &ctx).unwrap();
        assert!(prompt.contains("compared against `main`"));
        assert!(prompt.contains("- Prefer map_err"));
    }

    #[test]
    fn apply_feedback_single_item() {
        let ctx = empty_ctx();
        let items = vec![FeedbackItem {
            file: "lib.rs",
            line: 42,
            message: "Missing error handling",
        }];
        let prompt = format_apply_feedback_prompt(&items, &ctx).unwrap();
        assert!(prompt.contains("1 of the following"));
        assert!(prompt.contains("1. lib.rs:42 - Missing error handling"));
    }

    #[test]
    fn reply_prompt_includes_diff_context() {
        let rules = vec!["Prefer map_err".to_string()];
        let ctx = ReviewContext {
            ref_name: "develop",
            file_diff: "@@ -1,3 +1,3 @@",
            review_rules: &rules,
            project_rules: String::new(),
        };
        let msgs = vec![("user".to_string(), "hi".to_string())];
        let prompt = format_reply_prompt(
            &ReplyContext {
                file: "a.rs",
                line: 5,
                reply: "ok",
                anchor_content: "let x = 1;",
                context: &[],
                prior_messages: &msgs,
            },
            &ctx,
            &[],
        );
        assert!(prompt.contains("compared against `develop`"));
        assert!(prompt.contains("- Prefer map_err"));
        assert!(prompt.contains("Reply: ok"));
    }

    #[test]
    fn reply_prompt_includes_similar_thread_context() {
        let ctx = ReviewContext {
            ref_name: "",
            file_diff: "",
            review_rules: &[],
            project_rules: String::new(),
        };
        let similar = vec![SimilarThreadContext {
            file: "other.rs".to_string(),
            line: 10,
            status: "resolved".to_string(),
            messages: vec![
                ("agent".to_string(), "Use map_err here".to_string()),
                ("user".to_string(), "Good call, done".to_string()),
            ],
        }];
        let prompt = format_reply_prompt(
            &ReplyContext {
                file: "a.rs",
                line: 5,
                reply: "thoughts?",
                anchor_content: "let x = 1;",
                context: &[],
                prior_messages: &[],
            },
            &ctx,
            &similar,
        );
        assert!(prompt.contains("Related threads"));
        assert!(prompt.contains("resolved"));
        assert!(prompt.contains("other.rs:10"));
        assert!(prompt.contains("Use map_err here"));
        assert!(prompt.contains("Good call, done"));
        assert!(prompt.contains("Reply: thoughts?"));
    }

    #[test]
    fn reply_prompt_with_prior_messages() {
        let ctx = empty_ctx();
        let prior = vec![
            ("user".to_string(), "initial comment".to_string()),
            ("agent".to_string(), "I'll fix that".to_string()),
        ];
        let prompt = format_reply_prompt(
            &ReplyContext {
                file: "a.rs",
                line: 5,
                reply: "looks good",
                anchor_content: "let x = 1;",
                context: &[],
                prior_messages: &prior,
            },
            &ctx,
            &[],
        );
        assert!(prompt.contains("Thread history:"));
        assert!(prompt.contains("[user]: initial comment"));
        assert!(prompt.contains("[agent]: I'll fix that"));
        assert!(prompt.contains("Reply: looks good"));
    }

    #[test]
    fn reply_prompt_omits_similar_when_empty() {
        let ctx = ReviewContext {
            ref_name: "",
            file_diff: "",
            review_rules: &[],
            project_rules: String::new(),
        };
        let prompt = format_reply_prompt(
            &ReplyContext {
                file: "a.rs",
                line: 5,
                reply: "ok",
                anchor_content: "let x = 1;",
                context: &[],
                prior_messages: &[],
            },
            &ctx,
            &[],
        );
        assert!(!prompt.contains("Related threads"));
    }

    #[test]
    fn similarity_prompt_none_for_single_item() {
        let items = vec![(0, "a.rs", 1u32, "msg")];
        assert!(format_similarity_prompt(&items).is_none());
    }

    #[test]
    fn similarity_prompt_none_for_empty() {
        assert!(format_similarity_prompt(&[]).is_none());
    }

    #[test]
    fn similarity_prompt_includes_items() {
        let items = vec![
            (0, "a.rs", 1u32, "missing error handling"),
            (1, "b.rs", 5u32, "same issue here"),
        ];
        let prompt = format_similarity_prompt(&items).unwrap();
        assert!(prompt.contains("0. a.rs:1 - missing error handling"));
        assert!(prompt.contains("1. b.rs:5 - same issue here"));
        assert!(prompt.contains("GROUP|"));
    }

    #[test]
    fn parse_similarity_none() {
        assert!(parse_similarity_response("NONE").is_empty());
        assert!(parse_similarity_response("  NONE  ").is_empty());
    }

    #[test]
    fn parse_similarity_valid_groups() {
        let text = "GROUP|0,3,7\nGROUP|2,5\n";
        let groups = parse_similarity_response(text);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], vec![0, 3, 7]);
        assert_eq!(groups[1], vec![2, 5]);
    }

    #[test]
    fn parse_similarity_single_index_discarded() {
        let text = "GROUP|0\nGROUP|1,2\n";
        let groups = parse_similarity_response(text);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec![1, 2]);
    }

    #[test]
    fn parse_similarity_duplicate_indices() {
        let text = "GROUP|0,0,1\nGROUP|2,3\n";
        let groups = parse_similarity_response(text);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], vec![0, 0, 1]);
        assert_eq!(groups[1], vec![2, 3]);
    }

    #[test]
    fn parse_similarity_ignores_garbage() {
        let text = "some random text\nGROUP|0,1\nmore garbage\n";
        let groups = parse_similarity_response(text);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], vec![0, 1]);
    }

    #[test]
    fn parse_similarity_empty() {
        assert!(parse_similarity_response("").is_empty());
    }

    #[test]
    fn snapshot_review_preamble_full() {
        let rules = vec![
            "Prefer map_err over match".to_string(),
            "Always use ? for propagation".to_string(),
        ];
        let ctx = ReviewContext {
            ref_name: "main",
            file_diff: "@@ -10,4 +10,6 @@\n fn process() {\n-    old_call();\n+    new_call();\n+    extra_step();\n }",
            review_rules: &rules,
            project_rules: "Project rules:\n- [Style] Use constants instead of string literals\n- [Error] Return Result, never panic\n\n".to_string(),
        };
        let output = format_review_preamble(&ctx, "src/engine.rs");
        insta::assert_snapshot!("review_preamble_full_context", output);
    }

    #[test]
    fn snapshot_reply_prompt_with_history_and_similar() {
        let rules = vec!["Prefer map_err over match".to_string()];
        let ctx = ReviewContext {
            ref_name: "develop",
            file_diff: "@@ -1,3 +1,4 @@\n fn main() {\n+    setup();\n     run();\n }",
            review_rules: &rules,
            project_rules: String::new(),
        };
        let prior = vec![
            (
                "user".to_string(),
                "This error handling looks wrong, should use map_err.".to_string(),
            ),
            (
                "agent".to_string(),
                "You're right, I'll refactor to use map_err with a custom error type.".to_string(),
            ),
        ];
        let similar = vec![SimilarThreadContext {
            file: "src/handler.rs".to_string(),
            line: 42,
            status: "resolved".to_string(),
            messages: vec![
                (
                    "agent".to_string(),
                    "Replaced match block with map_err(AppError::from).".to_string(),
                ),
                ("user".to_string(), "Looks good, approved.".to_string()),
            ],
        }];
        let context_lines = vec![
            "fn main() {".to_string(),
            "    setup();".to_string(),
            "    run();".to_string(),
            "}".to_string(),
        ];
        let output = format_reply_prompt(
            &ReplyContext {
                file: "src/main.rs",
                line: 2,
                reply: "Can you also add error handling for setup()?",
                anchor_content: "    setup();",
                context: &context_lines,
                prior_messages: &prior,
            },
            &ctx,
            &similar,
        );
        insta::assert_snapshot!("reply_prompt_with_history_and_similar", output);
    }

    #[test]
    fn snapshot_apply_feedback_prompt() {
        let rules = vec!["Use constants for status codes".to_string()];
        let ctx = ReviewContext {
            ref_name: "main",
            file_diff: "",
            review_rules: &rules,
            project_rules: "Project rules:\n- [Naming] Use snake_case for all function names\n\n"
                .to_string(),
        };
        let items = vec![
            FeedbackItem {
                file: "src/api/handler.rs",
                line: 45,
                message: "Replace magic number 200 with HTTP_OK constant",
            },
            FeedbackItem {
                file: "src/api/handler.rs",
                line: 78,
                message: "Add timeout parameter to this HTTP call",
            },
            FeedbackItem {
                file: "src/db/connection.rs",
                line: 12,
                message: "Connection pool size should come from config, not hardcoded",
            },
        ];
        let output = format_apply_feedback_prompt(&items, &ctx).unwrap();
        insta::assert_snapshot!("apply_feedback_prompt_with_rules", output);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn parse_extraction_response_never_panics(input in ".*") {
            let _ = parse_extraction_response(&input);
        }

        #[test]
        fn parse_similarity_response_never_panics(input in ".*") {
            let _ = parse_similarity_response(&input);
        }
    }
}
