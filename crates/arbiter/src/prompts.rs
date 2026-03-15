//! Prompt formatting for backend calls.
//!
//! Assembles comments with file, line, and surrounding code context.

/// Formats a comment prompt with file, line, and surrounding code context.
///
/// Used when sending new comments to the agent.
pub fn format_comment_prompt(
    file: &str,
    line: u32,
    comment: &str,
    anchor_content: &str,
    context: &[String],
) -> String {
    let ctx_block = format_context_block(file, line, anchor_content, context);
    format!("{ctx_block}\nComment: {comment}")
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
) -> String {
    let ctx_block = format_context_block(file, line, anchor_content, context);
    let mut prompt = ctx_block;

    if !prior_messages.is_empty() {
        prompt.push_str("\n\nThread history:\n");
        for (role, text) in prior_messages {
            prompt.push_str(&format!("[{role}]: {text}\n"));
        }
    }

    prompt.push_str(&format!("\nReply: {reply}"));
    prompt
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
