//! Context compaction (contract C9).
//!
//! Two mechanisms keep a long session inside the context window:
//! * **micro-compaction** clears the content of old, large tool results that
//!   can be fetched again; the cleared ids are sticky so the prompt prefix
//!   changes once, not on every request;
//! * **auto-compaction** replaces the conversation with a structured summary
//!   when the window is nearly full (or on `/compact`).
//!
//! This crate holds the pure parts: thresholds, eligibility and the summary
//! prompt. The engine runs them.

use std::collections::{HashMap, HashSet};

use forge_types::{ContentBlock, Message, Role};

/// Tools whose output can be produced again by re-running them.
pub const REFETCHABLE: &[&str] = &["Read", "Grep", "Glob", "LS", "Bash", "BashOutput", "WebFetch", "WebSearch"];
/// Tool results at most this long are never cleared.
pub const MICRO_MIN_CHARS: usize = 8_000;
/// Results from this many most recent user turns are kept.
pub const MICRO_KEEP_TURNS: usize = 3;
/// Reserved for the summary itself and the next reply.
pub const AUTOCOMPACT_BUFFER: u64 = 13_000;

/// Context tokens at which auto-compaction runs.
pub fn autocompact_threshold(window: u64, max_output: u32) -> u64 {
    window.saturating_sub(u64::from(max_output.min(32_000))).saturating_sub(AUTOCOMPACT_BUFFER)
}

/// Context tokens above which micro-compaction runs (half the auto threshold).
pub fn micro_threshold(window: u64, max_output: u32) -> u64 {
    autocompact_threshold(window, max_output) / 2
}

/// Is this user message a person's turn (not just tool results)?
fn is_user_turn(m: &Message) -> bool {
    m.role == Role::User
        && m.content.iter().any(|b| matches!(b, ContentBlock::Text { .. }))
        && !m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Tool-result ids that micro-compaction may clear now.
pub fn micro_candidates(messages: &[Message], already: &HashSet<String>) -> Vec<String> {
    let turns: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| is_user_turn(m)).map(|(i, _)| i).collect();
    if turns.len() <= MICRO_KEEP_TURNS {
        return vec![];
    }
    let cutoff = turns[turns.len() - MICRO_KEEP_TURNS];
    let mut tool_of: HashMap<&str, &str> = HashMap::new();
    for m in &messages[..cutoff] {
        for b in &m.content {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                tool_of.insert(id, name);
            }
        }
    }
    let mut out = vec![];
    for m in &messages[..cutoff] {
        for b in &m.content {
            if let ContentBlock::ToolResult { tool_use_id, content, .. } = b {
                let name = tool_of.get(tool_use_id.as_str()).copied().unwrap_or("");
                let refetchable = REFETCHABLE.contains(&name) || name.starts_with("mcp__");
                if refetchable && !already.contains(tool_use_id) && content.to_text().len() > MICRO_MIN_CHARS {
                    out.push(tool_use_id.clone());
                }
            }
        }
    }
    out
}

/// Rough token estimate for text that has not been through the API yet.
pub fn estimate_tokens(messages: &[Message]) -> u64 {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.to_text().len(),
            ContentBlock::ToolUse { input, .. } => input.to_string().len(),
            ContentBlock::Thinking { thinking, .. } => thinking.len(),
            ContentBlock::Image { .. } | ContentBlock::Document { .. } => 6_000,
            _ => 200,
        })
        .sum();
    (chars / 4) as u64
}

/// The instruction appended to the conversation to ask for a summary.
pub fn summary_instruction(custom: Option<&str>) -> String {
    let mut s = String::from(
        "Stop working on the task for a moment. The conversation is about to be replaced by a summary of it, \
         and you will continue from that summary alone, so write it now. Do not call any tools.\n\n\
         First think through the conversation in order inside <analysis> tags: what the user asked for at each \
         point, what you did, the decisions made, the files touched, the errors met and how they were fixed, and \
         any feedback the user gave. Then write the summary inside <summary> tags with these sections:\n\n\
         1. Goal: what the user wants overall, and any constraints or preferences they stated.\n\
         2. Key facts: technologies, conventions and decisions that matter for continuing.\n\
         3. Files: every file read, created or changed, why it matters, and the essential code (signatures, the \
            exact lines changed) needed to continue without re-reading everything.\n\
         4. Problems: errors hit, their causes and fixes, and anything still unresolved.\n\
         5. User messages: each message the user sent, in order, close to verbatim (not tool results).\n\
         6. Open tasks: work the user asked for that is not finished.\n\
         7. Current state: exactly what was being done right before this summary, with file names and code.\n\
         8. Next step: the immediate next action, only if it follows directly from the user's latest request; \
            quote the relevant request so the intent is not lost.\n\n\
         Be precise and complete; prefer concrete names, paths and code over general descriptions.",
    );
    if let Some(c) = custom.filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!("\n\nAdditional instructions for this summary:\n{}", c.trim()));
    }
    s
}

/// Pull the `<summary>` body out of the model's reply (falls back to the whole text).
pub fn extract_summary(reply: &str) -> String {
    match (reply.find("<summary>"), reply.rfind("</summary>")) {
        (Some(a), Some(b)) if b > a => reply[a + "<summary>".len()..b].trim().to_string(),
        _ => {
            let without_analysis = match (reply.find("<analysis>"), reply.find("</analysis>")) {
                (Some(a), Some(b)) if b > a => format!("{}{}", &reply[..a], &reply[b + "</analysis>".len()..]),
                _ => reply.to_string(),
            };
            without_analysis.trim().to_string()
        }
    }
}

/// The user message that starts the conversation after compaction.
pub fn summary_message(summary: &str, context: Option<&str>, continue_work: bool) -> Message {
    let mut text = String::new();
    if let Some(c) = context.filter(|c| !c.trim().is_empty()) {
        text.push_str(&format!("<system-reminder>\n{c}\n</system-reminder>\n\n"));
    }
    text.push_str(
        "This session continues an earlier conversation that ran out of context. Here is a summary of it:\n\n",
    );
    text.push_str(summary);
    if continue_work {
        text.push_str(
            "\n\nContinue the work from where it stopped, without asking the user further questions. Pick up the \
             last task you were working on.",
        );
    }
    Message::user_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::ContentBlock;
    use serde_json::json;

    fn tool_round(id: &str, name: &str, size: usize) -> Vec<Message> {
        vec![
            Message::assistant(vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: json!({}),
                cache_control: None,
            }]),
            Message::user(vec![ContentBlock::tool_result(id, "x".repeat(size), false)]),
        ]
    }

    #[test]
    fn c9_micro_clears_only_eligible_results() {
        let mut msgs = vec![Message::user_text("turn 1")];
        msgs.extend(tool_round("big_read", "Read", 20_000));
        msgs.extend(tool_round("small_read", "Read", 100));
        msgs.extend(tool_round("big_edit", "Edit", 20_000));
        msgs.extend(tool_round("big_mcp", "mcp__docs__fetch", 20_000));
        msgs.push(Message::user_text("turn 2"));
        msgs.push(Message::user_text("turn 3"));
        msgs.extend(tool_round("recent_read", "Read", 20_000));
        msgs.push(Message::user_text("turn 4"));
        let c = micro_candidates(&msgs, &HashSet::new());
        assert_eq!(c, vec!["big_read".to_string(), "big_mcp".to_string()]);
        // Already-cleared ids are not reported again.
        let again = micro_candidates(&msgs, &HashSet::from(["big_read".to_string()]));
        assert_eq!(again, vec!["big_mcp".to_string()]);
        // Too few turns: nothing is old enough.
        assert!(micro_candidates(&msgs[..msgs.len() - 3], &HashSet::new()).is_empty());
    }

    #[test]
    fn thresholds() {
        assert_eq!(autocompact_threshold(200_000, 32_000), 155_000);
        assert_eq!(autocompact_threshold(1_000_000, 64_000), 955_000);
        assert_eq!(micro_threshold(200_000, 32_000), 77_500);
        assert_eq!(autocompact_threshold(10_000, 32_000), 0);
    }

    #[test]
    fn summary_parsing() {
        assert_eq!(extract_summary("<analysis>thinking</analysis>\n<summary>\nThe gist\n</summary>"), "The gist");
        assert_eq!(extract_summary("<analysis>a</analysis> plain"), "plain");
        assert!(summary_instruction(Some("focus on tests")).ends_with("focus on tests"));
        let m = summary_message("S", Some("memory"), true);
        let t = m.text();
        assert!(t.starts_with("<system-reminder>\nmemory") && t.contains("S\n\nContinue the work"));
    }
}
