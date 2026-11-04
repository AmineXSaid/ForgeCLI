//! Repair of tool-call arguments a model ended on purpose but wrote as invalid JSON.
//!
//! Only for calls the model finished itself (stop reason `tool_use` or `end_turn`):
//! a call cut off by `max_tokens` is incomplete, and running a repaired half of it
//! would be wrong. The repairs are the mistakes open-weight models make in long
//! arguments: a missing comma between fields, raw newlines or tabs inside a string,
//! a missing closing quote or brace, and stray closers after a complete object.

use serde_json::{Map, Value};

/// At most this many fixes are tried on one input.
const MAX_FIXES: usize = 8;

/// The repaired arguments, when a small fix turns `raw` into a JSON object.
pub fn repair_json(raw: &str) -> Option<Map<String, Value>> {
    let mut s = escape_controls_in_strings(raw.trim());
    let mut closed = false;
    for _ in 0..MAX_FIXES {
        let err = match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => return Some(m),
            Ok(_) => return None,
            Err(e) => e,
        };
        let msg = err.to_string();
        if err.is_eof() {
            if closed {
                return None;
            }
            s = close_open(&s)?;
            closed = true;
        } else if msg.starts_with("trailing characters") {
            return leading_object(&s);
        } else if msg.starts_with("expected `,` or `}`") || msg.starts_with("expected `,` or `]`") {
            let at = byte_offset(&s, err.line(), err.column())?;
            s = insert_comma(&s, at)?;
        } else {
            return None;
        }
    }
    None
}

/// Raw control characters inside strings, escaped as JSON requires.
fn escape_controls_in_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_string, mut escaped) = (false, false);
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            } else if (c as u32) < 0x20 {
                match c {
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    _ => out.push_str(&format!("\\u{:04x}", c as u32)),
                }
                continue;
            }
        } else if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

/// `s` with an open string and open brackets closed, if it ends after a complete value.
fn close_open(s: &str) -> Option<String> {
    let mut stack = Vec::new();
    let (mut in_string, mut escaped) = (false, false);
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' if stack.pop() != Some(c) => return None,
            _ => {}
        }
    }
    let mut out = s.to_string();
    if in_string {
        if escaped {
            out.pop();
        }
        out.push('"');
    } else {
        let trimmed = out.trim_end();
        // A dangling key or separator means the content isn't complete: don't guess.
        if trimmed.ends_with(':') || trimmed.ends_with(',') {
            return None;
        }
    }
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    Some(out)
}

/// The first value of `s`, when it is an object followed only by stray closers.
fn leading_object(s: &str) -> Option<Map<String, Value>> {
    let mut stream = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    let Some(Ok(Value::Object(m))) = stream.next() else { return None };
    let rest = &s[stream.byte_offset()..];
    rest.chars().all(|c| c.is_whitespace() || c == '}' || c == ']').then_some(m)
}

/// A comma before the value that starts at `at`, if one does start there.
fn insert_comma(s: &str, at: usize) -> Option<String> {
    let rest = s.get(at..)?;
    let start = rest.len() - rest.trim_start().len();
    let next = rest.trim_start().chars().next()?;
    if !(next == '"' || next == '{' || next == '[' || next == '-' || next.is_ascii_digit()) {
        return None;
    }
    let pos = at + start;
    Some(format!("{},{}", &s[..pos], &s[pos..]))
}

/// The byte offset of a 1-based line and column as serde_json reports them.
fn byte_offset(s: &str, line: usize, column: usize) -> Option<usize> {
    let line_start: usize = s.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum();
    let at = line_start + column.saturating_sub(1);
    (at <= s.len() && s.is_char_boundary(at)).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixed(raw: &str) -> Option<Value> {
        repair_json(raw).map(Value::Object)
    }

    #[test]
    fn valid_input_is_unchanged() {
        assert_eq!(fixed(r#"{"command": "ls"}"#), Some(json!({"command": "ls"})));
    }

    #[test]
    fn a_missing_comma_between_fields() {
        let raw = r#"{"command": "python3 run.py", "timeout": 600000 "description": "run it"}"#;
        assert_eq!(fixed(raw), Some(json!({"command": "python3 run.py", "timeout": 600000, "description": "run it"})));
        let raw = r#"{"command": "a" "description": "b"}"#;
        assert_eq!(fixed(raw), Some(json!({"command": "a", "description": "b"})));
    }

    #[test]
    fn raw_newlines_and_tabs_inside_strings() {
        let raw = "{\"command\": \"python3 - <<'PY'\nprint(1)\n\tprint(2)\nPY\"}";
        assert_eq!(fixed(raw), Some(json!({"command": "python3 - <<'PY'\nprint(1)\n\tprint(2)\nPY"})));
    }

    #[test]
    fn missing_closing_quote_or_brace() {
        assert_eq!(fixed(r#"{"command": "ls -la""#), Some(json!({"command": "ls -la"})));
        assert_eq!(fixed(r#"{"command": "ls -la"#), Some(json!({"command": "ls -la"})));
        assert_eq!(fixed(r#"{"edits": [{"a": 1}"#), Some(json!({"edits": [{"a": 1}]})));
    }

    #[test]
    fn stray_closers_after_the_object() {
        assert_eq!(fixed(r#"{"command": "ls"}}"#), Some(json!({"command": "ls"})));
        assert_eq!(fixed(r#"{"command": "ls"} ]"#), Some(json!({"command": "ls"})));
    }

    #[test]
    fn what_cannot_be_repaired_safely_is_left_alone() {
        assert_eq!(fixed(r#"{"command": "ls"} {"x": 1}"#), None, "two objects");
        assert_eq!(fixed(r#"{"command":"#), None, "a dangling key");
        assert_eq!(fixed(r#"{"command": "ls","#), None, "a dangling comma");
        assert_eq!(fixed(r#"["ls"]"#), None, "not an object");
        assert_eq!(fixed("not json at all"), None);
        assert_eq!(fixed(r#"{"a": 1 x}"#), None, "junk that doesn't start a value");
    }
}
