//! `/goal` (contract C18): a condition Forge keeps working toward. After each
//! turn a small model checks the conversation against it. When the goal isn't
//! met, the driver starts another turn. The loop ends when the goal is met,
//! judged impossible or cleared, or a guard pauses it.

use std::time::Instant;

use forge_types::{ContentBlock, Message, Role};
use serde_json::{json, Value};

/// Longest condition accepted.
pub const MAX_CONDITION: usize = 4000;
/// Consecutive turns without a tool call before the loop pauses.
pub const MAX_IDLE_TURNS: u32 = 3;
/// How much of the conversation the evaluator sees, in characters (the newest part).
const TRANSCRIPT_CHARS: usize = 60_000;
/// How much of each tool result it sees (the end, where results usually are).
const RESULT_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Active,
    Achieved,
    Failed(String),
    Cleared,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Achieved => "achieved",
            Status::Failed(_) => "failed",
            Status::Cleared => "cleared",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Goal {
    pub condition: String,
    pub status: Status,
    /// Turns checked so far.
    pub checks: u32,
    /// Consecutive goal turns without a tool call.
    pub idle: u32,
    /// The loop stopped (a guard, an error, an interrupt) until the next prompt.
    pub paused: Option<String>,
    pub last_reason: Option<String>,
    pub started: Instant,
    /// Session cost when the goal was set.
    pub cost_at_start: f64,
}

impl Goal {
    pub fn new(condition: &str, cost: f64) -> Self {
        Goal {
            condition: condition.to_string(),
            status: Status::Active,
            checks: 0,
            idle: 0,
            paused: None,
            last_reason: None,
            started: Instant::now(),
            cost_at_start: cost,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == Status::Active
    }

    /// The loop stopped without meeting the goal: judged impossible, ended by
    /// an error, or paused by a guard.
    pub fn ended_unmet(&self) -> bool {
        match self.status {
            Status::Failed(_) | Status::Cleared => true,
            Status::Active => self.paused.is_some(),
            Status::Achieved => false,
        }
    }

    /// The transcript record.
    pub fn record(&self) -> Value {
        let reason = match &self.status {
            Status::Failed(r) => Some(r.clone()),
            _ => self.last_reason.clone(),
        };
        json!({"condition": self.condition, "status": self.status.as_str(), "reason": reason})
    }

    /// A goal from a resumed transcript, if it was still active.
    pub fn restore(record: &Value, cost: f64) -> Option<Goal> {
        (record.get("status").and_then(Value::as_str) == Some("active"))
            .then(|| record.get("condition").and_then(Value::as_str))
            .flatten()
            .map(|c| Goal::new(c, cost))
    }
}

/// Words that clear the goal: `/goal clear`.
pub fn is_clear_word(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "clear" | "stop" | "off" | "reset" | "none" | "cancel")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Met,
    NotMet,
    Impossible,
}

/// Read the evaluator's answer: a JSON object, tolerating text around it.
pub fn parse_verdict(text: &str) -> (Verdict, String) {
    let json = text.find('{').zip(text.rfind('}')).filter(|(a, b)| a < b).map(|(a, b)| &text[a..=b]);
    if let Some(v) = json.and_then(|j| serde_json::from_str::<Value>(j).ok()) {
        let reason = v.get("reason").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let verdict = match v.get("verdict").and_then(Value::as_str).map(|s| s.to_ascii_lowercase()) {
            Some(s) if s == "met" => Verdict::Met,
            Some(s) if s == "impossible" => Verdict::Impossible,
            Some(s) if s == "not_met" || s == "not met" => Verdict::NotMet,
            _ => return (Verdict::NotMet, "the check gave no clear verdict".into()),
        };
        return (verdict, reason);
    }
    (Verdict::NotMet, "the check's answer could not be read".into())
}

pub const EVALUATOR_PROMPT: &str = "You check whether a coding agent has reached a goal. You get the goal and a \
transcript of the agent's work, newest last. Judge only from evidence in the transcript: commands that ran and their \
output, files changed, test results. Don't take the agent's word for something the transcript doesn't show.\n\n\
Reply with only a JSON object: {\"verdict\": \"met\" | \"not_met\" | \"impossible\", \"reason\": \"one or two \
sentences\"}.\n- met: the evidence shows the goal holds now.\n- impossible: it can't be reached from here (it \
contradicts itself, needs access or information the agent can't get, or the work showed it can't be done).\n- \
not_met: anything else; the reason says what is still missing or unproven.";

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

fn is_reminder(text: &str) -> bool {
    text.trim_start().starts_with("<system-reminder>")
}

/// The conversation for the evaluator: prompts, replies, tool calls with their
/// input, and the end of each tool result. The newest part when it is long.
pub fn evaluator_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        for b in &m.content {
            match (m.role, b) {
                (Role::User, ContentBlock::Text { text, .. }) if !is_reminder(text) => {
                    out.push_str(&format!("USER: {}\n\n", text.trim()));
                }
                (Role::User, ContentBlock::ToolResult { content, is_error, .. }) => {
                    let tag = if *is_error == Some(true) { "TOOL ERROR" } else { "TOOL RESULT" };
                    out.push_str(&format!("{tag}: {}\n\n", tail(content.to_text().trim(), RESULT_CHARS)));
                }
                (Role::Assistant, ContentBlock::Text { text, .. }) if !text.trim().is_empty() => {
                    out.push_str(&format!("AGENT: {}\n\n", text.trim()));
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    out.push_str(&format!("TOOL CALL {name}: {}\n\n", tail(&input.to_string(), RESULT_CHARS)));
                }
                _ => {}
            }
        }
    }
    tail(out.trim_end(), TRANSCRIPT_CHARS)
}

/// The prompt that continues the work after a check.
pub fn continue_prompt(condition: &str, reason: &str) -> String {
    let reason = if reason.is_empty() { String::new() } else { format!(" {reason}") };
    format!(
        "<system-reminder>\nGoal check: not met yet.{reason}\nKeep working toward the goal: {condition}\n\
         </system-reminder>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verdicts_tolerantly() {
        assert_eq!(parse_verdict(r#"{"verdict":"met","reason":"tests pass"}"#), (Verdict::Met, "tests pass".into()));
        assert_eq!(
            parse_verdict("Sure.\n```json\n{\"verdict\": \"NOT_MET\", \"reason\": \"2 fail\"}\n```"),
            (Verdict::NotMet, "2 fail".into())
        );
        assert_eq!(parse_verdict(r#"{"verdict":"impossible","reason":"no network"}"#).0, Verdict::Impossible);
        assert_eq!(parse_verdict("met!").0, Verdict::NotMet, "no JSON is never a pass");
        assert_eq!(parse_verdict(r#"{"verdict":"maybe"}"#).0, Verdict::NotMet);
    }

    #[test]
    fn records_restore_only_active_goals() {
        let g = Goal::new("tests pass", 0.0);
        let rec = g.record();
        assert_eq!(rec["status"], "active");
        assert_eq!(Goal::restore(&rec, 1.0).unwrap().condition, "tests pass");
        let mut done = g.clone();
        done.status = Status::Achieved;
        assert!(Goal::restore(&done.record(), 0.0).is_none());
        assert!(is_clear_word("Stop") && !is_clear_word("stopwatch"));
    }

    #[test]
    fn transcript_keeps_the_end_of_results() {
        let long = format!("{}PASSED", "x".repeat(5000));
        let msgs = vec![
            Message::user_text("<system-reminder>\nmemory\n</system-reminder>"),
            Message::user_text("make tests pass"),
            Message::assistant(vec![ContentBlock::ToolUse {
                id: "t".into(),
                name: "Bash".into(),
                input: json!({"command": "cargo test"}),
                cache_control: None,
            }]),
            Message::user(vec![ContentBlock::tool_result("t", long, false)]),
        ];
        let t = evaluator_transcript(&msgs);
        assert!(t.starts_with("USER: make tests pass"), "{t}");
        assert!(t.contains("TOOL CALL Bash: {\"command\":\"cargo test\"}") && t.contains("PASSED"));
        assert!(!t.contains("memory"));
    }
}
