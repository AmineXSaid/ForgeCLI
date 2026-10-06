//! Keep secrets out of anything ForgeCLI prints.

use serde_json::Value;

/// Keys whose values are secrets (`FORGE_API_KEY`, `apiKeyHelper` output, tokens, passwords, ...).
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["key", "token", "secret", "password", "passwd", "credential", "auth", "cookie", "session_id"]
        .iter()
        .any(|w| k.contains(w))
        && !k.ends_with("helper")
}

/// A copy of `v` with secret-looking values replaced by `"<redacted>"`.
pub fn redact(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let out = if is_secret_key(k) && (val.is_string() || val.is_number()) {
                        Value::String("<redacted>".into())
                    } else {
                        redact(val)
                    };
                    (k.clone(), out)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(redact).collect()),
        Value::String(s) if looks_like_secret(s) => Value::String("<redacted>".into()),
        other => other.clone(),
    }
}

/// Values that are secrets whatever their key: well-known key prefixes and bearer tokens.
fn looks_like_secret(s: &str) -> bool {
    let t = s.trim();
    ["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA"].iter().any(|p| t.starts_with(p)) && t.len() >= 20
        || t.to_ascii_lowercase().starts_with("bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_by_key_and_by_value() {
        let v = json!({
            "model": "opus",
            "env": {"FORGE_API_KEY": "abc", "OTHER": "sk-abcdefghijklmnopqrstuvwxyz", "PATH": "/bin"},
            "apiKeyHelper": "cat ~/.key",
            "headers": ["Authorization: Bearer xyz", "Bearer abcdef"],
        });
        let r = redact(&v);
        assert_eq!(r["model"], "opus");
        assert_eq!(r["env"]["FORGE_API_KEY"], "<redacted>");
        assert_eq!(r["env"]["OTHER"], "<redacted>");
        assert_eq!(r["env"]["PATH"], "/bin");
        assert_eq!(r["apiKeyHelper"], "cat ~/.key", "the helper command is not itself a secret");
        assert_eq!(r["headers"][1], "<redacted>");
    }
}
