//! Which endpoint this session talks to, with which key, and where each came
//! from. One resolver for the provider, `forge doctor` and `/status`.
//!
//! Rules:
//! - Keys are per provider. `FORGE_API_KEY` / `FORGE_AUTH_TOKEN` (and
//!   `apiKeyHelper`) go only to the Messages endpoint; `FORGE_OPENAI_API_KEY`
//!   (and `openai.apiKeyHelper`) only to the OpenAI-compatible one. A key is
//!   never sent to the other host: a mix-up is named instead.
//! - A project's checked-in `.forge/settings.json` can't choose where the key
//!   goes (`baseUrl`, `openai.baseUrl`) or run a key helper; the user's own
//!   settings (user, local, `--settings`, managed) can.
//! - Without a key, the Messages path refuses to start (it never accepts
//!   keyless requests); the OpenAI path starts, silently for a local server,
//!   with a warning for a remote one.

use std::path::PathBuf;

use forge_api::auth::{is_local_url, parse_base_url, Backend};
use forge_config::{LoadedSettings, SettingSource};

/// The layers that may pick the endpoint or run a key helper.
const TRUSTED: [SettingSource; 4] =
    [SettingSource::User, SettingSource::Local, SettingSource::Flag, SettingSource::Managed];

/// Settings that send the key somewhere or run a command for it.
const SENSITIVE: [(&str, &str); 4] = [
    ("/baseUrl", "baseUrl"),
    ("/openai/baseUrl", "openai.baseUrl"),
    ("/apiKeyHelper", "apiKeyHelper"),
    ("/openai/apiKeyHelper", "openai.apiKeyHelper"),
];

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub backend: Option<Backend>,
    /// The base URL (cleaned), or `None` for the Messages API's default host.
    pub url: Option<String>,
    /// The variable or setting that chose the URL.
    pub url_from: Option<&'static str>,
    /// The settings file, when a setting chose the URL.
    pub url_file: Option<PathBuf>,
    /// The URL is on this machine or a private network.
    pub local: bool,
    /// The key: `x-api-key` on Messages, the bearer token on OpenAI.
    pub key: Option<String>,
    pub key_var: Option<&'static str>,
    /// `FORGE_AUTH_TOKEN` (Messages only).
    pub token: Option<String>,
    /// A key helper to run when no key is set: (command, setting, file).
    pub helper: Option<(String, &'static str, Option<PathBuf>)>,
    /// Variables that were wrapped in quotes (removed).
    pub quoted: Vec<&'static str>,
    /// Keys for the other provider that are set but never sent here.
    pub others: Vec<&'static str>,
    /// Shown once at startup.
    pub warnings: Vec<String>,
    /// Configurations that can never work: refuse to start.
    pub problems: Vec<String>,
}

impl Resolution {
    pub fn backend(&self) -> Backend {
        self.backend.unwrap_or(Backend::Messages)
    }

    /// "FORGE_OPENAI_BASE_URL" or "openai.baseUrl in <file>".
    pub fn url_origin(&self) -> String {
        match (self.url_from, &self.url_file) {
            (Some(s), Some(f)) => format!("{s} in {}", f.display()),
            (Some(s), None) => s.to_string(),
            (None, _) => "default".into(),
        }
    }
}

/// Trim, and drop one pair of surrounding quotes (what `set X="..."` in cmd.exe
/// leaves). `Ok(None)` when nothing is left; `Err` for a value that can't be a key.
fn clean(var: &str, raw: &str, quoted: &mut bool) -> Result<Option<String>, String> {
    let mut v = raw.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            v = v[1..v.len() - 1].trim();
            *quoted = true;
        }
    }
    if v.is_empty() {
        return Ok(None);
    }
    if !v.chars().all(|c| c.is_ascii_graphic()) {
        return Err(format!(
            "{var} can't be used: it contains a space, a control character or a non-ASCII character. A key is one \
             word of plain characters; check how it was set."
        ));
    }
    Ok(Some(v.to_string()))
}

/// The highest-precedence trusted layer with a string at `pointer`.
fn trusted_str(settings: &LoadedSettings, pointer: &str) -> Option<(String, Option<PathBuf>)> {
    settings
        .layers
        .iter()
        .filter(|l| TRUSTED.contains(&l.source))
        .filter_map(|l| l.value.pointer(pointer).and_then(|v| v.as_str()).map(|s| (l.source.rank(), s, &l.path)))
        .max_by_key(|(rank, _, _)| *rank)
        .map(|(_, s, p)| (s.to_string(), p.clone()))
}

/// Resolve the endpoint and key. Pure: reads `env` and `settings` only.
pub fn resolve(settings: &LoadedSettings, env: &dyn Fn(&str) -> Option<String>) -> Resolution {
    let mut r = Resolution::default();
    let var = |name: &'static str, r: &mut Resolution| -> Option<String> {
        let raw = env(name)?;
        let mut quoted = false;
        match clean(name, &raw, &mut quoted) {
            Ok(v) => {
                if quoted && v.is_some() {
                    r.quoted.push(name);
                }
                v
            }
            Err(p) => {
                r.problems.push(p);
                None
            }
        }
    };

    // Project settings that would choose the endpoint or run a command are ignored.
    for layer in settings.layers.iter().filter(|l| l.source == SettingSource::Project) {
        for (pointer, key) in SENSITIVE {
            if layer.value.pointer(pointer).is_some() {
                let file =
                    layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "project settings".into());
                let why = if key.ends_with("Helper") {
                    "a project's shared settings can't run a command to fetch your key"
                } else {
                    "a project's shared settings can't choose where your key is sent"
                };
                r.warnings.push(format!(
                    "Ignored {key} in {file}: {why}. Put it in {} or .forge/settings.local.json.",
                    forge_config::config_dir().join("settings.json").display()
                ));
            }
        }
    }

    // 1. The URL, and so the backend.
    let (url, from, file) = if let Some(u) = var("FORGE_OPENAI_BASE_URL", &mut r) {
        r.backend = Some(Backend::OpenAi);
        (Some(u), Some("FORGE_OPENAI_BASE_URL"), None)
    } else if let Some((u, f)) = trusted_str(settings, "/openai/baseUrl") {
        r.backend = Some(Backend::OpenAi);
        (Some(u.trim().to_string()), Some("openai.baseUrl"), f)
    } else if let Some(u) = var("FORGE_BASE_URL", &mut r) {
        r.backend = Some(Backend::Messages);
        (Some(u), Some("FORGE_BASE_URL"), None)
    } else if let Some((u, f)) = trusted_str(settings, "/baseUrl") {
        r.backend = Some(Backend::Messages);
        (Some(u.trim().to_string()), Some("baseUrl"), f)
    } else {
        r.backend = Some(Backend::Messages);
        (None, None, None)
    };
    r.url_from = from;
    r.url_file = file;
    if let Some(u) = &url {
        match parse_base_url(u) {
            Ok(parsed) => {
                r.local = is_local_url(&parsed);
                if parsed.path().trim_end_matches('/').ends_with("/chat/completions") {
                    r.warnings.push(format!(
                        "{} ends in /chat/completions, which Forge adds itself, so requests would go to \
                         {}/chat/completions. Set it to the part before /chat/completions.",
                        r.url_from.unwrap_or("The URL"),
                        u.trim_end_matches('/')
                    ));
                }
            }
            Err(_) => r.problems.push(format!(
                "{} is not a valid http:// or https:// URL: \"{}\"",
                r.url_from.unwrap_or("The URL"),
                forge_config::redact_text(u)
            )),
        }
        r.url = Some(u.trim_end_matches('/').to_string());
    }

    // 2. The key, for this backend only.
    match r.backend() {
        Backend::OpenAi => {
            r.key = var("FORGE_OPENAI_API_KEY", &mut r);
            if r.key.is_some() {
                r.key_var = Some("FORGE_OPENAI_API_KEY");
            } else if let Some((cmd, f)) = trusted_str(settings, "/openai/apiKeyHelper") {
                r.helper = Some((cmd, "openai.apiKeyHelper", f));
            }
            r.others = ["FORGE_API_KEY", "FORGE_AUTH_TOKEN", "OPENAI_API_KEY"]
                .into_iter()
                .filter(|k| env(k).is_some_and(|v| !v.trim().is_empty()))
                .collect();
            if r.key.is_none() && r.helper.is_none() && !r.local && r.problems.is_empty() {
                let url = r.url.clone().unwrap_or_default();
                let base = format!("No key for {url}: FORGE_OPENAI_API_KEY is not set, so requests carry none.");
                r.warnings.push(match r.others.as_slice() {
                    [] => format!("{base} Set it if this endpoint needs a key."),
                    [one] => format!(
                        "{base} {one} is set, but it is never sent to an OpenAI-compatible endpoint; if it holds \
                         this endpoint's key, set FORGE_OPENAI_API_KEY to it."
                    ),
                    many => format!(
                        "{base} {} are set, but they are never sent to an OpenAI-compatible endpoint; if one holds \
                         this endpoint's key, set FORGE_OPENAI_API_KEY to it.",
                        and_list(many)
                    ),
                });
            }
        }
        Backend::Messages => {
            r.key = var("FORGE_API_KEY", &mut r);
            if r.key.is_some() {
                r.key_var = Some("FORGE_API_KEY");
            }
            r.token = var("FORGE_AUTH_TOKEN", &mut r);
            if r.key.is_none() && r.token.is_none() {
                if let Some((cmd, f)) = trusted_str(settings, "/apiKeyHelper") {
                    r.helper = Some((cmd, "apiKeyHelper", f));
                }
            }
            let openai_key = env("FORGE_OPENAI_API_KEY").is_some_and(|v| !v.trim().is_empty());
            if r.key.is_none() && r.token.is_none() && r.helper.is_none() {
                r.problems.push(if openai_key {
                    "no API key for the Messages API. FORGE_OPENAI_API_KEY is set, but it is only used with an \
                     OpenAI-compatible endpoint: set FORGE_OPENAI_BASE_URL to that endpoint's URL (for example \
                     https://gateway.example.com/v1)."
                        .into()
                } else {
                    "no API key for the Messages API: set FORGE_API_KEY (or FORGE_AUTH_TOKEN for a gateway that \
                     expects a bearer token). To use an OpenAI-compatible endpoint instead, set \
                     FORGE_OPENAI_BASE_URL and FORGE_OPENAI_API_KEY."
                        .into()
                });
            } else if openai_key {
                r.warnings.push(
                    "FORGE_OPENAI_API_KEY is set but unused: it applies only with FORGE_OPENAI_BASE_URL (or \
                     openai.baseUrl)."
                        .into(),
                );
            }
        }
    }

    // 3. Plain http to a remote host would expose the key.
    if let (Some(u), false) = (&r.url, r.local) {
        if u.starts_with("http://") && (r.key.is_some() || r.token.is_some()) {
            let host = parse_base_url(u).ok().and_then(|p| p.host_str().map(str::to_string)).unwrap_or_default();
            let key_var = r.key_var.unwrap_or("FORGE_AUTH_TOKEN");
            r.warnings.push(format!(
                "{} uses http:// for {host}, so {key_var} would travel unencrypted. Use https:// if the endpoint \
                 supports it.",
                r.url_from.unwrap_or("The URL")
            ));
        }
    }

    // 4. Quotes, and credentials placed where Forge doesn't read them.
    for q in r.quoted.clone() {
        r.warnings.push(format!(
            "{q} was wrapped in quotes; Forge removed them. In cmd.exe, write set {q}=value without quotes."
        ));
    }
    for name in ["FORGE_API_KEY", "FORGE_AUTH_TOKEN", "FORGE_OPENAI_API_KEY", "FORGE_BASE_URL", "FORGE_OPENAI_BASE_URL"]
    {
        if env(name).is_some() {
            continue;
        }
        if let Some(layer) = settings.layers.iter().find(|l| l.value.pointer(&format!("/env/{name}")).is_some()) {
            let file = layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "settings".into());
            r.warnings.push(format!(
                "{name} in the env block of {file} only reaches tools; Forge reads its own endpoint and key from \
                 its environment. Set it in your shell or system environment instead."
            ));
        }
    }
    r
}

/// "a, b and c".
pub(crate) fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A key as it may be shown: the last four characters and the length.
pub fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n >= 16 {
        let tail: String = s.chars().skip(n - 4).collect();
        format!("...{tail}, {n} characters")
    } else {
        format!("{n} characters")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_config::SettingsLayer;
    use serde_json::json;
    use std::collections::HashMap;

    fn run(vars: &[(&str, &str)], layers: Vec<SettingsLayer>) -> Resolution {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let settings = LoadedSettings { layers, ..Default::default() };
        resolve(&settings, &|k| vars.get(k).cloned())
    }

    fn layer(source: SettingSource, value: serde_json::Value) -> SettingsLayer {
        SettingsLayer { source, path: Some(PathBuf::from(format!("/{source:?}/settings.json"))), value }
    }

    #[test]
    fn forge_api_key_is_never_used_for_the_openai_endpoint() {
        let r = run(
            &[("FORGE_OPENAI_BASE_URL", "https://gw.example.com/v1"), ("FORGE_API_KEY", "sk-messages-0123456789")],
            vec![],
        );
        assert_eq!(r.backend(), Backend::OpenAi);
        assert!(r.key.is_none() && r.problems.is_empty());
        assert_eq!(
            r.warnings,
            ["No key for https://gw.example.com/v1: FORGE_OPENAI_API_KEY is not set, so requests carry none. \
              FORGE_API_KEY is set, but it is never sent to an OpenAI-compatible endpoint; if it holds this \
              endpoint's key, set FORGE_OPENAI_API_KEY to it."]
        );
        assert!(!format!("{r:?}").contains("sk-messages") || r.key.is_none());
    }

    #[test]
    fn local_openai_server_needs_no_key_and_no_warning() {
        let r = run(&[("FORGE_OPENAI_BASE_URL", "http://localhost:11434/v1"), ("FORGE_API_KEY", "x")], vec![]);
        assert!(r.warnings.is_empty() && r.problems.is_empty(), "{r:?}");
        assert!(r.local);
    }

    #[test]
    fn quoted_and_padded_values_are_cleaned() {
        let r = run(
            &[
                ("FORGE_OPENAI_BASE_URL", "\"https://gw.example.com/v1\""),
                ("FORGE_OPENAI_API_KEY", "  \"sk-abc0123456789\" "),
            ],
            vec![],
        );
        assert_eq!(r.key.as_deref(), Some("sk-abc0123456789"));
        assert_eq!(r.url.as_deref(), Some("https://gw.example.com/v1"));
        assert!(r.warnings.iter().any(|w| w.starts_with("FORGE_OPENAI_API_KEY was wrapped in quotes")), "{r:?}");
    }

    #[test]
    fn unusable_values_refuse_to_start() {
        let r =
            run(&[("FORGE_OPENAI_BASE_URL", "https://gw.example.com/v1"), ("FORGE_OPENAI_API_KEY", "sk abc")], vec![]);
        assert!(r.problems[0].starts_with("FORGE_OPENAI_API_KEY can't be used"), "{r:?}");
        let r = run(&[("FORGE_OPENAI_BASE_URL", "gw.example.com/v1")], vec![]);
        assert!(r.problems[0].starts_with("FORGE_OPENAI_BASE_URL is not a valid http:// or https:// URL"), "{r:?}");
    }

    #[test]
    fn project_settings_cannot_choose_the_endpoint_or_helper() {
        let project = layer(
            SettingSource::Project,
            json!({"baseUrl": "https://evil.example", "openai": {"baseUrl": "https://evil.example/v1", "apiKeyHelper": "x"}, "apiKeyHelper": "touch ran"}),
        );
        let r = run(&[("FORGE_API_KEY", "k")], vec![project.clone()]);
        assert_eq!(r.backend(), Backend::Messages);
        assert!(r.url.is_none() && r.helper.is_none());
        assert_eq!(r.warnings.iter().filter(|w| w.starts_with("Ignored ")).count(), 4, "{:?}", r.warnings);
        let user = layer(SettingSource::User, json!({"openai": {"baseUrl": "https://gw.example.com/v1"}}));
        let r = run(&[("FORGE_OPENAI_API_KEY", "k")], vec![user, project]);
        assert_eq!((r.backend(), r.url_from), (Backend::OpenAi, Some("openai.baseUrl")));
        assert_eq!(r.url.as_deref(), Some("https://gw.example.com/v1"));
    }

    #[test]
    fn openai_key_without_url_explains_itself() {
        let r = run(&[("FORGE_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.problems[0].contains("set FORGE_OPENAI_BASE_URL"), "{r:?}");
        let r = run(&[], vec![]);
        assert!(r.problems[0].starts_with("no API key for the Messages API: set FORGE_API_KEY"), "{r:?}");
    }

    #[test]
    fn chat_completions_suffix_and_plain_http_are_flagged() {
        let r = run(
            &[("FORGE_OPENAI_BASE_URL", "https://gw.example.com/v1/chat/completions"), ("FORGE_OPENAI_API_KEY", "k")],
            vec![],
        );
        assert!(r.warnings.iter().any(|w| w.contains("ends in /chat/completions")), "{r:?}");
        let r = run(&[("FORGE_OPENAI_BASE_URL", "http://gw.example.com/v1"), ("FORGE_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.warnings.iter().any(|w| w.contains("would travel unencrypted")), "{r:?}");
        let r = run(&[("FORGE_OPENAI_BASE_URL", "http://192.168.1.2/v1"), ("FORGE_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.warnings.is_empty(), "{r:?}");
    }

    #[test]
    fn credential_vars_in_the_env_block_are_flagged() {
        let user = layer(SettingSource::User, json!({"env": {"FORGE_OPENAI_API_KEY": "x"}}));
        let r = run(&[("FORGE_API_KEY", "k")], vec![user]);
        assert!(r.warnings.iter().any(|w| w.starts_with("FORGE_OPENAI_API_KEY in the env block")), "{r:?}");
    }

    #[test]
    fn masks_show_only_the_tail() {
        assert_eq!(mask("sk-abcdefghijklmnopqrstuv1234"), "...1234, 29 characters");
        assert_eq!(mask("abc"), "3 characters");
    }
}
