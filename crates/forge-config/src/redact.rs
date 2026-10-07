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

/// Mask secrets inside free text (a shell command, a tool result, a log line):
/// well-known key prefixes, bearer tokens, and `key=value` assignments whose
/// name says it is a secret.
pub fn redact_text(s: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<[regex::Regex; 3]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            regex::Regex::new(r"\b(sk-[A-Za-z0-9_\-]{16,}|gh[po]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[bp]-[A-Za-z0-9\-]{10,}|AKIA[A-Z0-9]{16})")
                .unwrap(),
            regex::Regex::new(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{8,}").unwrap(),
            regex::Regex::new(
                r#"(?i)\b([A-Z0-9_]*(?:api[_-]?key|token|secret|password|passwd|credential)[A-Z0-9_]*)(\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s"',;]+)"#,
            )
            .unwrap(),
        ]
    });
    let s = res[0].replace_all(s, "<redacted>");
    let s = res[1].replace_all(&s, "$1 <redacted>");
    res[2].replace_all(&s, "$1$2<redacted>").into_owned()
}

/// [`redact`], and [`redact_text`] on every string: for copies of transcripts
/// and logs that leave the session (`/feedback`).
pub fn redact_deep(v: &Value) -> Value {
    match redact(v) {
        Value::String(s) => Value::String(redact_text(&s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_deep).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, redact_deep(&v))).collect()),
        other => other,
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
    fn redacts_secrets_inside_text() {
        let t = "export FORGE_API_KEY=abc123 && curl -H 'Authorization: Bearer abcdefgh123' x; key sk-abcdefghijklmnop1234 ok";
        let r = redact_text(t);
        assert_eq!(
            r,
            "export FORGE_API_KEY=<redacted> && curl -H 'Authorization: Bearer <redacted>' x; key <redacted> ok"
        );
        assert_eq!(redact_text("password: \"hunter2 two\" done"), "password: <redacted> done");
        assert_eq!(redact_text("tokens used: 1200"), "tokens used: 1200", "no assignment, no masking");
        let v = json!({"message": {"content": [{"text": "my token=s3cr3t"}]}, "apiKey": "x"});
        let d = redact_deep(&v);
        assert_eq!(d["message"]["content"][0]["text"], "my token=<redacted>");
        assert_eq!(d["apiKey"], "<redacted>");
    }

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
