//! MCP server configuration: where servers are declared, which are trusted,
//! and `${VAR}` expansion.
//!
//! Sources, lowest precedence first (a later one replaces a server by name):
//! 1. the project's `.mcp.json`, used only for servers the user approved;
//! 2. `mcpServers` in settings (user, project, local, flag and managed layers);
//! 3. `--mcp-config` files or JSON strings.
//!
//! With `--strict-mcp-config` only the third source is read.
//!
//! A `.mcp.json` arrives with the repository, so its servers (which are
//! commands Forge would run) are trusted only when the user's own settings
//! say so: `enableAllProjectMcpServers` or `enabledMcpjsonServers` in the user
//! or local layer. The project's checked-in settings can't approve them.
//!
//! `disabledMcpjsonServers` in the user or local layer (`/mcp disable` writes
//! the local one) always wins: it turns off `.mcp.json` servers and servers
//! from `mcpServers` in settings. `--mcp-config` and plugin servers are chosen
//! per run and aren't affected. A server turned off this way stays in
//! [`Resolved::skipped`] with its config (a `.mcp.json` one only when the user
//! also trusts it), so `/mcp enable` can start it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use forge_config::{LoadedSettings, SettingSource};
use serde_json::{json, Map, Value};

/// The settings key listing servers the user turned off.
pub const DISABLED_KEY: &str = "disabledMcpjsonServers";
/// [`Skipped::reason`] for a server the user turned off.
pub const DISABLED: &str = "disabled";

#[derive(Debug, Clone, PartialEq)]
pub enum ServerConfig {
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    Http { url: String, headers: BTreeMap<String, String> },
    Sse { url: String, headers: BTreeMap<String, String> },
}

impl ServerConfig {
    pub fn transport(&self) -> &'static str {
        match self {
            ServerConfig::Stdio { .. } => "stdio",
            ServerConfig::Http { .. } => "http",
            ServerConfig::Sse { .. } => "sse",
        }
    }

    /// The JSON form written to settings and `.mcp.json`.
    pub fn to_json(&self) -> Value {
        match self {
            ServerConfig::Stdio { command, args, env } => {
                let mut v = json!({"type": "stdio", "command": command, "args": args});
                if !env.is_empty() {
                    v["env"] = json!(env);
                }
                v
            }
            ServerConfig::Http { url, headers } | ServerConfig::Sse { url, headers } => {
                let mut v = json!({"type": self.transport(), "url": url});
                if !headers.is_empty() {
                    v["headers"] = json!(headers);
                }
                v
            }
        }
    }

    /// One line for `forge mcp list`, with secrets left out.
    pub fn summary(&self) -> String {
        match self {
            ServerConfig::Stdio { command, args, .. } => {
                let mut s = command.clone();
                for a in args {
                    s.push(' ');
                    s.push_str(a);
                }
                format!("{s} (stdio)")
            }
            ServerConfig::Http { url, .. } => format!("{url} (http)"),
            ServerConfig::Sse { url, .. } => format!("{url} (sse)"),
        }
    }

    /// `${VAR}` and `${VAR:-default}` replaced from `env` everywhere a value may hold them.
    pub fn expanded(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<ServerConfig, String> {
        let x = |s: &str| expand_vars(s, env);
        let map = |m: &BTreeMap<String, String>| -> Result<BTreeMap<String, String>, String> {
            m.iter().map(|(k, v)| Ok((k.clone(), x(v)?))).collect()
        };
        Ok(match self {
            ServerConfig::Stdio { command, args, env: e } => ServerConfig::Stdio {
                command: x(command)?,
                args: args.iter().map(|a| x(a)).collect::<Result<_, _>>()?,
                env: map(e)?,
            },
            ServerConfig::Http { url, headers } => ServerConfig::Http { url: x(url)?, headers: map(headers)? },
            ServerConfig::Sse { url, headers } => ServerConfig::Sse { url: x(url)?, headers: map(headers)? },
        })
    }
}

/// Expand `${VAR}` / `${VAR:-default}`; a missing variable without a default is an error.
pub fn expand_vars(s: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let inner = &rest[start + 2..start + end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        match env(name).filter(|v| !v.is_empty()).or_else(|| default.map(str::to_string)) {
            Some(v) => out.push_str(&v),
            None => return Err(format!("environment variable {name} is not set")),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default()
}

/// Parse one server entry.
pub fn parse_server(v: &Value) -> Result<ServerConfig, String> {
    let kind = v.get("type").and_then(Value::as_str);
    let url = v.get("url").and_then(Value::as_str);
    match (kind, url) {
        (Some("http") | Some("streamable-http") | Some("streamableHttp"), Some(u)) | (None, Some(u)) => {
            Ok(ServerConfig::Http { url: u.into(), headers: string_map(v.get("headers")) })
        }
        (Some("sse"), Some(u)) => Ok(ServerConfig::Sse { url: u.into(), headers: string_map(v.get("headers")) }),
        (Some("http") | Some("sse"), None) => Err("\"url\" is required".into()),
        (Some("stdio") | None, None) => {
            let command = v.get("command").and_then(Value::as_str).ok_or("\"command\" is required")?;
            let args = v
                .get("args")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            Ok(ServerConfig::Stdio { command: command.into(), args, env: string_map(v.get("env")) })
        }
        (Some(other), _) => Err(format!("unknown transport type {other:?} (expected stdio, http or sse)")),
    }
}

/// Parse `{"mcpServers": {...}}` (or the bare map). Bad entries become errors, not failures.
pub fn parse_servers(v: &Value) -> (Vec<(String, ServerConfig)>, Vec<String>) {
    let map = v.get("mcpServers").unwrap_or(v);
    let mut ok = vec![];
    let mut errs = vec![];
    for (name, entry) in map.as_object().into_iter().flatten() {
        if !valid_name(name) {
            errs.push(format!("MCP server name {name:?}: use letters, digits, '-' and '_'"));
            continue;
        }
        match parse_server(entry) {
            Ok(c) => ok.push((name.clone(), c)),
            Err(e) => errs.push(format!("MCP server {name}: {e}")),
        }
    }
    (ok, errs)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where a server was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `.mcp.json` in the project.
    Project,
    /// `mcpServers` in settings.
    Settings,
    /// `--mcp-config`.
    Flag,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Settings => "settings",
            Scope::Flag => "flag",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedServer {
    pub name: String,
    pub scope: Scope,
    pub config: ServerConfig,
}

/// A `.mcp.json` server Forge did not start, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
    /// The config of a server the user turned off, so this session can turn it
    /// back on; `None` while a `.mcp.json` server needs approval.
    pub config: Option<NamedServer>,
}

#[derive(Debug, Default)]
pub struct Resolved {
    pub servers: Vec<NamedServer>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<String>,
}

pub fn project_file(project: &Path) -> PathBuf {
    project.join(".mcp.json")
}

/// The layers the user controls: user and local settings.
fn user_layers(settings: &LoadedSettings) -> impl Iterator<Item = &forge_config::SettingsLayer> {
    settings.layers.iter().filter(|l| matches!(l.source, SettingSource::User | SettingSource::Local))
}

fn lists(layer: &forge_config::SettingsLayer, key: &str, name: &str) -> bool {
    layer.value.get(key).and_then(Value::as_array).is_some_and(|a| a.iter().any(|v| v.as_str() == Some(name)))
}

/// The user or local layers whose `disabledMcpjsonServers` names `name`.
pub fn disabled_in<'a>(settings: &'a LoadedSettings, name: &str) -> Vec<&'a forge_config::SettingsLayer> {
    user_layers(settings).filter(|l| lists(l, DISABLED_KEY, name)).collect()
}

/// The user's own settings trust this `.mcp.json` server (disabled or not).
fn project_trusted(settings: &LoadedSettings, name: &str) -> bool {
    user_layers(settings).any(|l| {
        lists(l, "enabledMcpjsonServers", name) || l.value.get("enableAllProjectMcpServers") == Some(&json!(true))
    })
}

/// The user's decision about one `.mcp.json` server, from user and local settings only.
pub fn project_server_approved(settings: &LoadedSettings, name: &str) -> Option<bool> {
    if !disabled_in(settings, name).is_empty() {
        return Some(false);
    }
    project_trusted(settings, name).then_some(true)
}

/// Every server to start, by the precedence above.
pub fn resolve(settings: &LoadedSettings, project: &Path, flag_configs: &[String], strict: bool) -> Resolved {
    let mut r = Resolved::default();
    let mut by_name: Vec<NamedServer> = vec![];
    // A later source replaces an earlier server of the same name, started or skipped.
    let put = |s: NamedServer, list: &mut Vec<NamedServer>, skipped: &mut Vec<Skipped>| {
        list.retain(|o| o.name != s.name);
        skipped.retain(|o| o.name != s.name);
        list.push(s);
    };
    if !strict {
        if let Ok(text) = std::fs::read_to_string(project_file(project)) {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => {
                    let (servers, errs) = parse_servers(&v);
                    r.warnings.extend(errs.into_iter().map(|e| format!(".mcp.json: {e}")));
                    for (name, config) in servers {
                        let server = NamedServer { name: name.clone(), scope: Scope::Project, config };
                        match project_server_approved(settings, &name) {
                            Some(true) => put(server, &mut by_name, &mut r.skipped),
                            Some(false) => {
                                let config = project_trusted(settings, &name).then_some(server);
                                r.skipped.push(Skipped { name, reason: DISABLED.into(), config });
                            }
                            None => r.skipped.push(Skipped {
                                name,
                                reason: "needs approval: run `forge mcp approve <name>` to trust this project's \
                                         server"
                                    .into(),
                                config: None,
                            }),
                        }
                    }
                }
                Err(e) => r.warnings.push(format!(".mcp.json: {e}")),
            }
        }
        if let Some(v) = settings.get("/mcpServers") {
            let (servers, errs) = parse_servers(v);
            r.warnings.extend(errs.into_iter().map(|e| format!("settings: {e}")));
            for (name, config) in servers {
                let server = NamedServer { name: name.clone(), scope: Scope::Settings, config };
                if disabled_in(settings, &name).is_empty() {
                    put(server, &mut by_name, &mut r.skipped);
                } else {
                    // Off (`/mcp disable`); kept with its config so `/mcp enable` can start it.
                    by_name.retain(|o| o.name != name);
                    r.skipped.retain(|o| o.name != name);
                    r.skipped.push(Skipped { name, reason: DISABLED.into(), config: Some(server) });
                }
            }
        }
    }
    for raw in flag_configs {
        let text = if raw.trim_start().starts_with('{') {
            raw.clone()
        } else {
            match std::fs::read_to_string(raw) {
                Ok(t) => t,
                Err(e) => {
                    r.warnings.push(format!("--mcp-config {raw}: {e}"));
                    continue;
                }
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                let (servers, errs) = parse_servers(&v);
                r.warnings.extend(errs.into_iter().map(|e| format!("--mcp-config: {e}")));
                for (name, config) in servers {
                    put(NamedServer { name, scope: Scope::Flag, config }, &mut by_name, &mut r.skipped);
                }
            }
            Err(e) => r.warnings.push(format!("--mcp-config {raw}: {e}")),
        }
    }
    r.servers = by_name;
    r
}

/// Put `name` on (or take it off) the `disabledMcpjsonServers` list in one
/// settings file, keeping every other key. Returns whether the file changed.
/// A file that isn't a JSON object is an error, never replaced.
pub fn set_disabled(path: &Path, name: &str, disabled: bool) -> std::io::Result<bool> {
    let current: Vec<Value> = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => vec![],
        Ok(text) => {
            let v = serde_json::from_str::<Value>(&text).ok().filter(Value::is_object).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} is not a JSON object", path.display()))
            })?;
            v.get(DISABLED_KEY).and_then(Value::as_array).cloned().unwrap_or_default()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e),
    };
    if current.iter().any(|v| v.as_str() == Some(name)) == disabled {
        return Ok(false);
    }
    let mut list = current;
    if disabled {
        list.push(json!(name));
    } else {
        list.retain(|v| v.as_str() != Some(name));
    }
    if list.is_empty() {
        forge_config::remove_setting(path, &[DISABLED_KEY])?;
    } else {
        forge_config::write_setting(path, &[DISABLED_KEY], Value::Array(list))?;
    }
    Ok(true)
}

/// Add or replace a server in a JSON file holding `mcpServers` (settings or `.mcp.json`).
pub fn write_server(path: &Path, name: &str, config: Option<&ServerConfig>) -> std::io::Result<bool> {
    use std::io::{Error, ErrorKind};
    // A file that exists but can't be read as a JSON object is left alone.
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => Value::Object(Map::new()),
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) if v.is_object() => v,
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("{} is not a JSON object; fix it, then try again", path.display()),
                ))
            }
        },
        Err(e) if e.kind() == ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(e),
    };
    if !root.get("mcpServers").is_some_and(Value::is_object) {
        root["mcpServers"] = Value::Object(Map::new());
    }
    let servers = root["mcpServers"].as_object_mut().expect("object");
    let existed = match config {
        Some(c) => servers.insert(name.to_string(), c.to_json()).is_some(),
        None => servers.remove(name).is_some(),
    };
    if config.is_none() && !existed {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)? + "\n")?;
    std::fs::rename(&tmp, path)?;
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_config::SettingsLayer;

    fn settings(layers: Vec<(SettingSource, Value)>) -> LoadedSettings {
        let mut merged = json!({});
        let mut out = vec![];
        for (source, value) in layers {
            forge_config::deep_merge(&mut merged, &value);
            out.push(SettingsLayer { source, path: None, value });
        }
        LoadedSettings { merged, layers: out, errors: vec![] }
    }

    #[test]
    fn parses_every_transport_and_rejects_bad_entries() {
        let (ok, errs) = parse_servers(&json!({"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "server-fs", "."], "env": {"K": "v"}},
            "web": {"type": "http", "url": "https://mcp.example.com/mcp", "headers": {"Authorization": "Bearer x"}},
            "old": {"type": "sse", "url": "https://mcp.example.com/sse"},
            "bare-url": {"url": "https://x/mcp"},
            "bad": {"type": "carrier-pigeon", "url": "x"},
            "no cmd": {"args": []},
            "nocmd": {"args": []}
        }}));
        assert_eq!(ok.len(), 4);
        assert_eq!(ok[0].1.transport(), "stdio");
        assert_eq!(ok[1].1.transport(), "http");
        assert_eq!(ok[2].1.transport(), "sse");
        assert_eq!(ok[3].1.transport(), "http");
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert_eq!(parse_server(&ok[0].1.to_json()).unwrap(), ok[0].1, "round trip");
    }

    #[test]
    fn expands_environment_variables() {
        let env = |k: &str| (k == "TOKEN").then(|| "s3cret".to_string());
        assert_eq!(expand_vars("Bearer ${TOKEN}", &env).unwrap(), "Bearer s3cret");
        assert_eq!(expand_vars("${PORT:-8080}/x", &env).unwrap(), "8080/x");
        assert!(expand_vars("${MISSING}", &env).unwrap_err().contains("MISSING"));
        let c = ServerConfig::Http {
            url: "https://h/${PATHX:-mcp}".into(),
            headers: [("A".into(), "${TOKEN}".into())].into(),
        };
        let x = c.expanded(&env).unwrap();
        assert_eq!(
            x,
            ServerConfig::Http { url: "https://h/mcp".into(), headers: [("A".into(), "s3cret".into())].into() }
        );
    }

    #[test]
    fn project_servers_need_the_users_approval() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(".mcp.json"),
            r#"{"mcpServers": {"repo-tool": {"command": "./evil"}, "docs": {"command": "docs-mcp"}}}"#,
        )
        .unwrap();
        // The repository's own settings cannot approve its servers.
        let s = settings(vec![(SettingSource::Project, json!({"enableAllProjectMcpServers": true}))]);
        let r = resolve(&s, d.path(), &[], false);
        assert!(r.servers.is_empty());
        assert_eq!(r.skipped.len(), 2);
        assert!(r.skipped[0].reason.contains("forge mcp approve"));

        let s = settings(vec![
            (SettingSource::User, json!({"enableAllProjectMcpServers": true})),
            (SettingSource::Local, json!({"disabledMcpjsonServers": ["repo-tool"]})),
        ]);
        let r = resolve(&s, d.path(), &[], false);
        assert_eq!(r.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["docs"]);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!((r.skipped[0].name.as_str(), r.skipped[0].reason.as_str()), ("repo-tool", DISABLED));
        assert_eq!(
            r.skipped[0].config.as_ref().map(|c| c.scope),
            Some(Scope::Project),
            "trusted by the user, so /mcp enable can start it"
        );

        let s = settings(vec![(SettingSource::Local, json!({"enabledMcpjsonServers": ["repo-tool"]}))]);
        let names: Vec<String> = resolve(&s, d.path(), &[], false).servers.into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["repo-tool"]);
    }

    #[test]
    fn precedence_and_strict_mode() {
        let d = tempfile::tempdir().unwrap();
        let s = settings(vec![(SettingSource::User, json!({"mcpServers": {"a": {"command": "from-settings"}}}))]);
        let flag = r#"{"mcpServers": {"a": {"command": "from-flag"}, "b": {"command": "b"}}}"#.to_string();
        let r = resolve(&s, d.path(), std::slice::from_ref(&flag), false);
        let a = r.servers.iter().find(|s| s.name == "a").unwrap();
        assert_eq!((a.scope, a.config.summary()), (Scope::Flag, "from-flag (stdio)".to_string()));
        let r = resolve(&s, d.path(), &[], true);
        assert!(r.servers.is_empty(), "strict reads only --mcp-config");
        let r = resolve(&s, d.path(), &["/no/such/file.json".into()], false);
        assert!(r.warnings[0].contains("/no/such/file.json"));
    }

    #[test]
    fn writes_servers_atomically_and_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        std::fs::write(&f, r#"{"model": "x"}"#).unwrap();
        let c = ServerConfig::Stdio { command: "srv".into(), args: vec!["--x".into()], env: BTreeMap::new() };
        assert!(!write_server(&f, "s", Some(&c)).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["model"], "x");
        assert_eq!(v["mcpServers"]["s"]["command"], "srv");
        assert!(write_server(&f, "s", None).unwrap());
        assert!(!write_server(&f, "s", None).unwrap(), "removing a missing server reports false");
    }

    #[test]
    fn the_disable_list_turns_off_settings_servers_but_not_flag_ones() {
        let d = tempfile::tempdir().unwrap();
        let s = settings(vec![
            (
                SettingSource::User,
                json!({"mcpServers": {"docs": {"command": "docs-mcp"}, "web": {"url": "https://w/mcp"}}}),
            ),
            (SettingSource::Local, json!({"disabledMcpjsonServers": ["docs", "cli"]})),
        ]);
        let flag = r#"{"mcpServers": {"cli": {"command": "from-flag"}}}"#.to_string();
        let r = resolve(&s, d.path(), std::slice::from_ref(&flag), false);
        let started: Vec<&str> = r.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(started, ["web", "cli"], "--mcp-config servers are chosen per run");
        assert_eq!(r.skipped.len(), 1);
        assert_eq!((r.skipped[0].name.as_str(), r.skipped[0].reason.as_str()), ("docs", DISABLED));
        assert_eq!(
            r.skipped[0].config.as_ref().map(|c| (c.scope, c.config.summary())),
            Some((Scope::Settings, "docs-mcp (stdio)".to_string())),
            "kept so /mcp enable can start it"
        );
        // The project's checked-in settings can't turn the user's servers off.
        let s = settings(vec![
            (SettingSource::User, json!({"mcpServers": {"docs": {"command": "docs-mcp"}}})),
            (SettingSource::Project, json!({"disabledMcpjsonServers": ["docs"]})),
        ]);
        assert_eq!(resolve(&s, d.path(), &[], false).servers.len(), 1);
    }

    #[test]
    fn a_later_source_replaces_a_skipped_server_of_the_same_name() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".mcp.json"), r#"{"mcpServers": {"x": {"command": "./repo-x"}}}"#).unwrap();
        let s = settings(vec![(SettingSource::User, json!({"mcpServers": {"x": {"command": "my-x"}}}))]);
        let r = resolve(&s, d.path(), &[], false);
        assert_eq!(r.servers.iter().map(|s| s.config.summary()).collect::<Vec<_>>(), ["my-x (stdio)"]);
        assert!(r.skipped.is_empty(), "one server per name: {:?}", r.skipped);
    }

    #[test]
    fn set_disabled_edits_one_list_and_keeps_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".forge/settings.local.json");
        assert!(set_disabled(&f, "docs", true).unwrap(), "creates the file");
        assert!(!set_disabled(&f, "docs", true).unwrap(), "already there");
        forge_config::write_setting(&f, &["model"], json!("x")).unwrap();
        assert!(set_disabled(&f, "web", true).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!((v["model"].clone(), v[DISABLED_KEY].clone()), (json!("x"), json!(["docs", "web"])));
        assert!(set_disabled(&f, "docs", false).unwrap() && set_disabled(&f, "web", false).unwrap());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v, json!({"model": "x"}), "an empty list is removed");
        std::fs::write(&f, "{not json").unwrap();
        assert!(set_disabled(&f, "docs", true).is_err(), "a broken file is left for the person to fix");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{not json");
    }
}
