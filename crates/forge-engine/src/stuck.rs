//! Stuck-loop detection (GOALS pillar 2).
//!
//! Models in trouble repeat themselves: the same failing call, the same
//! read or command with the same result, edits that undo each other, or a
//! long run of failures. The guard watches one user turn's tool calls and,
//! once per pattern, adds a reminder after the tool results telling the model
//! to step back and change approach.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde_json::{json, Value};

/// Identical failing calls before a reminder.
const REPEATED_FAILURES: u32 = 3;
/// Identical calls with identical results before a reminder.
const NO_PROGRESS: u32 = 3;
/// Consecutive failing calls (any tool) before a reminder.
const ERROR_STREAK: u32 = 5;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Stuck {
    RepeatedFailure { tool: String, count: u32 },
    NoProgress { tool: String, count: u32 },
    Oscillating { file: String },
    ErrorStreak { count: u32 },
}

impl Stuck {
    pub fn record(&self) -> Value {
        match self {
            Stuck::RepeatedFailure { tool, count } => json!({"kind": "repeated_failure", "tool": tool, "count": count}),
            Stuck::NoProgress { tool, count } => json!({"kind": "no_progress", "tool": tool, "count": count}),
            Stuck::Oscillating { file } => json!({"kind": "oscillating_edits", "file": file}),
            Stuck::ErrorStreak { count } => json!({"kind": "error_streak", "count": count}),
        }
    }

    pub fn text(&self) -> String {
        let body = match self {
            Stuck::RepeatedFailure { tool, count } => format!(
                "This exact {tool} call has now failed {count} times. Repeating it will not change the result. Read \
                 the error, check your assumptions (the file's current contents, the path, the command's syntax), \
                 and try a different approach."
            ),
            Stuck::NoProgress { tool, count } => format!(
                "This exact {tool} call has returned the same result {count} times. Nothing has changed between \
                 calls, so use the result you already have, or change something before running it again."
            ),
            Stuck::Oscillating { file } => format!(
                "Your edits to {file} are undoing each other. Stop editing it: re-read the file and the failing \
                 output, decide on one approach, then make that change."
            ),
            Stuck::ErrorStreak { count } => format!(
                "The last {count} tool calls all failed. Step back: re-read the task and the errors, then change \
                 approach. If something outside your control is blocking you, say what it is."
            ),
        };
        format!("<system-reminder>\n{body}\n</system-reminder>")
    }
}

fn hash<T: Hash>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// One user turn's view of the tool calls.
#[derive(Debug, Default)]
pub(crate) struct LoopGuard {
    failures: HashMap<u64, u32>,
    results: HashMap<(u64, u64), u32>,
    /// (file, old, new) of every successful edit.
    edits: Vec<(String, String, String)>,
    streak: u32,
    fired: HashSet<String>,
}

impl LoopGuard {
    /// Note one finished call; returns a pattern the first time it shows.
    pub fn observe(&mut self, tool: &str, input: &Value, output: &str, is_error: bool) -> Option<Stuck> {
        let key = hash(&(tool, input.to_string()));
        let mut found = None;
        if is_error {
            self.streak += 1;
            let n = self.failures.entry(key).or_insert(0);
            *n += 1;
            if *n >= REPEATED_FAILURES {
                found = Some((format!("fail:{key}"), Stuck::RepeatedFailure { tool: tool.into(), count: *n }));
            } else if self.streak >= ERROR_STREAK {
                found = Some(("streak".into(), Stuck::ErrorStreak { count: self.streak }));
            }
        } else {
            self.streak = 0;
            if tool != "TodoWrite" {
                let n = self.results.entry((key, hash(&output))).or_insert(0);
                *n += 1;
                if *n >= NO_PROGRESS {
                    found = Some((format!("same:{key}"), Stuck::NoProgress { tool: tool.into(), count: *n }));
                }
            }
            for (file, old, new) in edits_of(tool, input) {
                if self.edits.iter().any(|(f, o, n)| *f == file && *o == new && *n == old) {
                    found = Some((format!("osc:{file}"), Stuck::Oscillating { file: file.clone() }));
                }
                self.edits.push((file, old, new));
            }
        }
        let (id, stuck) = found?;
        self.fired.insert(id).then_some(stuck)
    }
}

/// The (file, old, new) replacements an edit call made.
fn edits_of(tool: &str, input: &Value) -> Vec<(String, String, String)> {
    let file = input.get("file_path").and_then(Value::as_str).unwrap_or_default().to_string();
    let pair = |e: &Value| {
        (
            file.clone(),
            e.get("old_string").and_then(Value::as_str).unwrap_or_default().to_string(),
            e.get("new_string").and_then(Value::as_str).unwrap_or_default().to_string(),
        )
    };
    match tool {
        "Edit" => vec![pair(input)],
        "MultiEdit" => {
            input.get("edits").and_then(Value::as_array).map(|a| a.iter().map(pair).collect()).unwrap_or_default()
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_failures_fire_once() {
        let mut g = LoopGuard::default();
        let call = json!({"command": "make"});
        assert_eq!(g.observe("Bash", &call, "err", true), None);
        assert_eq!(g.observe("Bash", &call, "err", true), None);
        let s = g.observe("Bash", &call, "err", true).unwrap();
        assert_eq!(s, Stuck::RepeatedFailure { tool: "Bash".into(), count: 3 });
        assert!(s.text().contains("failed 3 times"));
        assert_eq!(g.observe("Bash", &call, "err", true), None, "once per call");
    }

    #[test]
    fn same_result_three_times_is_no_progress() {
        let mut g = LoopGuard::default();
        let read = json!({"file_path": "/a"});
        g.observe("Read", &read, "x", false);
        g.observe("Read", &read, "y", false);
        assert_eq!(g.observe("Read", &read, "y", false), None, "a changed result is progress");
        assert!(matches!(g.observe("Read", &read, "y", false), Some(Stuck::NoProgress { count: 3, .. })));
        let todo = json!({"todos": []});
        for _ in 0..4 {
            assert_eq!(g.observe("TodoWrite", &todo, "ok", false), None);
        }
    }

    #[test]
    fn edits_that_undo_each_other() {
        let mut g = LoopGuard::default();
        let fwd = json!({"file_path": "/f.py", "old_string": "a + b", "new_string": "a - b"});
        let back = json!({"file_path": "/f.py", "old_string": "a - b", "new_string": "a + b"});
        assert_eq!(g.observe("Edit", &fwd, "ok", false), None);
        assert_eq!(g.observe("Edit", &back, "ok", false), Some(Stuck::Oscillating { file: "/f.py".into() }));
        let multi = json!({"file_path": "/g.py", "edits": [{"old_string": "x", "new_string": "y"}]});
        let undo = json!({"file_path": "/g.py", "edits": [{"old_string": "y", "new_string": "x"}]});
        g.observe("MultiEdit", &multi, "ok", false);
        assert!(matches!(g.observe("MultiEdit", &undo, "ok", false), Some(Stuck::Oscillating { .. })));
    }

    #[test]
    fn a_streak_of_different_failures() {
        let mut g = LoopGuard::default();
        for i in 0..4 {
            assert_eq!(g.observe("Bash", &json!({"command": format!("c{i}")}), "e", true), None);
        }
        assert_eq!(g.observe("Bash", &json!({"command": "c4"}), "e", true), Some(Stuck::ErrorStreak { count: 5 }));
        g.observe("Bash", &json!({"command": "ok"}), "fine", false);
        for i in 0..5 {
            assert_eq!(g.observe("Bash", &json!({"command": format!("d{i}")}), "e", true), None, "once per turn");
        }
    }
}
