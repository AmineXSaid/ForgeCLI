//! What Google's Gemini models require of a request, whatever wire carries it:
//! the native Gemini API (`gemini.rs`), or an OpenAI-compatible endpoint in
//! front of them (Google's own, or a gateway such as OmniRoute).
//!
//! - Tool names: at most 64 characters, `[A-Za-z_][A-Za-z0-9_.:-]*`. Longer or
//!   other names (an MCP tool's `mcp__server__tool` can be long) are sent as a
//!   short stable alias and mapped back when the model calls them.
//! - Tool schemas: the subset Gemini's `parameters` accepts (OpenAPI 3.0
//!   style): no `$schema`, `$id`, `$comment`, `$ref`/`$defs` (inlined),
//!   `examples`, `default`, `additionalProperties`; `const` becomes a one-value
//!   `enum`; a type list with `"null"` becomes `nullable`; `oneOf` becomes
//!   `anyOf`. An object without properties takes no parameters at all.
//! - Effort: Gemini 3 and later take a thinking level (`minimal` on Flash,
//!   `low`, `medium`, `high`); 2.5 takes a token budget.

use std::collections::HashMap;

use serde_json::{Map, Value};

/// The model's own id, past any gateway prefix (`gemini/`, `gc/`, `vertex/`,
/// `models/`), lowercased.
pub fn model_id(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase()
}

/// A Gemini model, under any name a gateway gives it.
pub fn is_gemini(model: &str) -> bool {
    model_id(model).starts_with("gemini")
}

/// The Gemini generation: (major, minor), e.g. `gemini-3.1-pro` → (3, 1);
/// `None` for a name without one (`gemini-flash-latest`).
pub fn version(model: &str) -> Option<(u32, u32)> {
    let id = model_id(model);
    let rest = id.strip_prefix("gemini-")?;
    let v = rest.split('-').next()?;
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// Gemini 3 or later: thinking levels and thought signatures (an unnamed
/// `-latest` alias counts as current).
pub fn is_gemini3_or_later(model: &str) -> bool {
    is_gemini(model) && version(model).is_none_or(|(major, _)| major >= 3)
}

/// A Flash model (the only kind that takes the `minimal` level).
pub fn is_flash(model: &str) -> bool {
    model_id(model).contains("flash")
}

/// The thinking level for ForgeCLI's effort: `low`, `medium` and `high` as
/// they are, `xhigh` and `max` as `high`, `minimal` on Flash (else `low`).
pub fn thinking_level(effort: &str, model: &str) -> Option<&'static str> {
    Some(match effort {
        "minimal" if is_flash(model) => "minimal",
        "minimal" | "low" => "low",
        "medium" => "medium",
        "high" | "xhigh" | "max" => "high",
        _ => return None,
    })
}

/// A 2.5 model's thinking budget for an effort (`-1`: the model decides).
pub fn thinking_budget(effort: &str) -> i64 {
    match effort {
        "minimal" => 512,
        "low" => 2048,
        "medium" => 8192,
        "high" => 24576,
        _ => -1,
    }
}

/// Whether `name` can be sent to Gemini as it is.
pub fn valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    first_ok && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
}

/// Tool names as sent, and back: an alias for each name Gemini wouldn't take.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolNames {
    to_alias: HashMap<String, String>,
    to_name: HashMap<String, String>,
}

impl ToolNames {
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut t = ToolNames::default();
        for n in names {
            if valid_tool_name(n) {
                continue;
            }
            let alias = alias_for(n);
            t.to_name.insert(alias.clone(), n.to_string());
            t.to_alias.insert(n.to_string(), alias);
        }
        t
    }

    /// The name as sent.
    pub fn sent(&self, name: &str) -> String {
        self.to_alias.get(name).cloned().unwrap_or_else(|| name.to_string())
    }

    /// The tool a name the model used stands for.
    pub fn original(&self, sent: &str) -> String {
        self.to_name.get(sent).cloned().unwrap_or_else(|| sent.to_string())
    }
}

/// A short stable alias: the name's valid characters, cut, and a hash of the
/// whole name so two long names never meet.
fn alias_for(name: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    let mut clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if !clean.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        clean.insert(0, '_');
    }
    clean.truncate(46);
    format!("{clean}_{h:016x}")
}

/// Keywords Gemini's `parameters` schema refuses.
const DROPPED: &[&str] =
    &["$schema", "$id", "$comment", "examples", "default", "additionalProperties", "$defs", "definitions"];

/// `schema` as Gemini's `parameters` takes it, or `None` when the tool takes
/// no arguments (an object with no properties is refused).
pub fn sanitize_schema(schema: &Value) -> Option<Value> {
    let defs = schema.get("$defs").or_else(|| schema.get("definitions")).cloned().unwrap_or(Value::Null);
    let out = clean(schema, &defs, 0);
    let empty_object = out.get("type").and_then(Value::as_str) == Some("object")
        && out.get("properties").and_then(Value::as_object).is_none_or(Map::is_empty);
    (!empty_object).then_some(out)
}

fn clean(v: &Value, defs: &Value, depth: usize) -> Value {
    let Some(o) = v.as_object() else {
        return match v {
            Value::Array(a) => Value::Array(a.iter().map(|x| clean(x, defs, depth + 1)).collect()),
            other => other.clone(),
        };
    };
    // `$ref` to a local definition: inlined (cycles stop at a plain object).
    if let Some(r) = o.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or(r);
        return match defs.get(name) {
            Some(d) if depth < 8 => clean(d, defs, depth + 1),
            _ => serde_json::json!({"type": "object"}),
        };
    }
    let mut out = Map::new();
    for (k, val) in o {
        if DROPPED.contains(&k.as_str()) {
            continue;
        }
        match k.as_str() {
            "const" => {
                out.insert("enum".into(), Value::Array(vec![val.clone()]));
            }
            "oneOf" => {
                out.insert("anyOf".into(), clean(val, defs, depth + 1));
            }
            "type" => match val {
                Value::Array(types) => {
                    let real: Vec<&Value> = types.iter().filter(|t| t.as_str() != Some("null")).collect();
                    if real.len() < types.len() {
                        out.insert("nullable".into(), Value::Bool(true));
                    }
                    out.insert("type".into(), real.first().map(|t| (*t).clone()).unwrap_or(Value::from("string")));
                }
                other => {
                    out.insert("type".into(), other.clone());
                }
            },
            "properties" => {
                let props: Map<String, Value> = val
                    .as_object()
                    .map(|p| p.iter().map(|(n, s)| (n.clone(), clean(s, defs, depth + 1))).collect())
                    .unwrap_or_default();
                out.insert("properties".into(), Value::Object(props));
            }
            _ => {
                out.insert(k.clone(), clean(val, defs, depth + 1));
            }
        }
    }
    // `enum` values are strings in Gemini's schema.
    if let Some(Value::Array(values)) = out.get_mut("enum") {
        for e in values.iter_mut() {
            if !e.is_string() {
                let s = if e.is_null() { "null".to_string() } else { e.to_string() };
                *e = Value::String(s);
            }
        }
        out.entry("type").or_insert_with(|| Value::from("string"));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn knows_gemini_models_under_any_name() {
        assert!(
            is_gemini("gemini-3.1-pro-preview") && is_gemini("gc/gemini-3-flash") && is_gemini("models/gemini-2.5-pro")
        );
        assert!(!is_gemini("claude-opus-5-5") && !is_gemini("gpt-5"));
        assert_eq!(version("vertex/gemini-3.1-pro"), Some((3, 1)));
        assert_eq!(version("gemini-2.5-flash"), Some((2, 5)));
        assert!(is_gemini3_or_later("gemini-4-pro") && is_gemini3_or_later("gemini-flash-latest"));
        assert!(!is_gemini3_or_later("gemini-2.5-pro"));
        assert_eq!(thinking_level("minimal", "gemini-3-flash"), Some("minimal"));
        assert_eq!(thinking_level("minimal", "gemini-3-pro"), Some("low"));
        assert_eq!(thinking_level("max", "gemini-3-pro"), Some("high"));
        assert_eq!(thinking_level("auto", "gemini-3-pro"), None);
    }

    #[test]
    fn long_or_odd_tool_names_get_stable_aliases() {
        let long = "mcp__github-enterprise-server__list_pull_request_review_comments_for_repository";
        let names = ToolNames::new(["Read", long, "9lives"]);
        assert_eq!(names.sent("Read"), "Read");
        let alias = names.sent(long);
        assert!(alias.len() <= 64 && valid_tool_name(&alias), "{alias}");
        assert_eq!(names.original(&alias), long);
        assert!(valid_tool_name(&names.sent("9lives")));
        assert_eq!(ToolNames::new([long]).sent(long), alias, "the same alias every time");
    }

    #[test]
    fn schemas_become_what_gemini_takes() {
        let s = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": {"const": "fast"},
                "limit": {"type": ["integer", "null"], "default": 10},
                "target": {"$ref": "#/$defs/Target"},
                "level": {"enum": [1, 2]},
                "pick": {"oneOf": [{"type": "string"}, {"type": "number"}]}
            },
            "$defs": {"Target": {"type": "object", "properties": {"path": {"type": "string", "examples": ["/a"]}}}}
        });
        let out = sanitize_schema(&s).unwrap();
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": {
                    "mode": {"enum": ["fast"], "type": "string"},
                    "limit": {"type": "integer", "nullable": true},
                    "target": {"type": "object", "properties": {"path": {"type": "string"}}},
                    "level": {"enum": ["1", "2"], "type": "string"},
                    "pick": {"anyOf": [{"type": "string"}, {"type": "number"}]}
                }
            })
        );
        assert_eq!(sanitize_schema(&json!({"type": "object", "properties": {}})), None);
        assert_eq!(sanitize_schema(&json!({"type": "object"})), None);
    }
}
