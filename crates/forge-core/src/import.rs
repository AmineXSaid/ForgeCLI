//! `/import`: bring MCP servers and instructions over from other coding
//! agents (Codex, Gemini CLI, Cursor).
//!
//! Where things go follows the trust model (contract C16):
//! - servers from the user's own config (`~/.codex`, `~/.gemini`,
//!   `~/.cursor`) go to Forge's user settings, which need no approval;
//! - servers shipped in the repository (`.cursor/mcp.json`) go to the
//!   project's `.mcp.json`, which still needs `forge mcp approve`.
//!
//! Instructions are appended to FORGE.md under an "Imported from" heading, at
//! most once. AGENTS.md needs no import: Forge reads it already.

use std::path::{Path, PathBuf};

use forge_mcp::config::ServerConfig;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Codex,
    Gemini,
    Cursor,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Codex, Source::Gemini, Source::Cursor];

    pub fn parse(s: &str) -> Option<Source> {
        match s.to_ascii_lowercase().as_str() {
            "codex" => Some(Source::Codex),
            "gemini" => Some(Source::Gemini),
            "cursor" => Some(Source::Cursor),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Codex => "Codex",
            Source::Gemini => "Gemini CLI",
            Source::Cursor => "Cursor",
        }
    }
}

/// One server to add.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerImport {
    pub source: Source,
    pub name: String,
    pub config: ServerConfig,
    /// Where it was found.
    pub from: PathBuf,
    /// The file it goes to (user settings or the project's `.mcp.json`).
    pub to: PathBuf,
}

/// Instructions to append to a FORGE.md.
#[derive(Debug, Clone, PartialEq)]
pub struct InstructionImport {
    pub source: Source,
    pub from: Vec<PathBuf>,
    pub text: String,
    pub heading: String,
    pub to: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Plan {
    pub servers: Vec<ServerImport>,
    pub instructions: Vec<InstructionImport>,
    /// Things left out, and why.
    pub skipped: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty() && self.instructions.is_empty()
    }
}

/// Where the plan reads and writes.
pub struct Places<'a> {
    pub home: &'a Path,
    pub project: &'a Path,
    /// Forge's user settings file.
    pub user_settings: &'a Path,
    /// Forge's config directory (for the user-level FORGE.md).
    pub config_dir: &'a Path,
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().filter(|t| !t.trim().is_empty())
}

/// MCP server names Forge already has in `path` (settings or `.mcp.json`).
fn names_in(path: &Path) -> Vec<String> {
    read(path)
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("mcpServers").and_then(Value::as_object).map(|m| m.keys().cloned().collect()))
        .unwrap_or_default()
}

/// Gemini CLI's server entries: `url` is SSE, `httpUrl` streamable HTTP.
fn gemini_entry(v: &Value) -> Value {
    let mut out = v.clone();
    if let Some(o) = out.as_object_mut() {
        if let Some(u) = o.remove("httpUrl") {
            o.insert("type".into(), "http".into());
            o.insert("url".into(), u);
        } else if o.contains_key("url") && !o.contains_key("type") {
            o.insert("type".into(), "sse".into());
        }
    }
    out
}

/// Build the plan: what each source has that Forge doesn't.
pub fn plan(sources: &[Source], at: &Places) -> Plan {
    let mut p = Plan::default();
    let project_mcp = at.project.join(".mcp.json");
    let mut taken: Vec<String> = names_in(at.user_settings);
    taken.extend(names_in(&project_mcp));
    let mut add = |p: &mut Plan, source: Source, from: &Path, to: &Path, servers: Vec<(String, ServerConfig)>| {
        for (name, config) in servers {
            if taken.contains(&name) {
                p.skipped
                    .push(format!("MCP server {name} ({}): Forge already has a server with that name", source.name()));
                continue;
            }
            taken.push(name.clone());
            p.servers.push(ServerImport { source, name, config, from: from.to_path_buf(), to: to.to_path_buf() });
        }
    };
    for &source in sources {
        match source {
            Source::Codex => {
                let f = at.home.join(".codex/config.toml");
                if let Some(text) = read(&f) {
                    match parse_toml(&text) {
                        Ok(v) => {
                            let servers = v.get("mcp_servers").cloned().unwrap_or(Value::Null);
                            let (ok, errs) = forge_mcp::config::parse_servers(&servers);
                            p.skipped.extend(errs.into_iter().map(|e| format!("{e} ({})", f.display())));
                            add(&mut p, source, &f, at.user_settings, ok);
                        }
                        Err(e) => p.skipped.push(format!("{}: {e}", f.display())),
                    }
                }
            }
            Source::Gemini => {
                let f = at.home.join(".gemini/settings.json");
                if let Some(v) = read(&f).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                    let map: Map<String, Value> = v
                        .get("mcpServers")
                        .and_then(Value::as_object)
                        .map(|m| m.iter().map(|(k, e)| (k.clone(), gemini_entry(e))).collect())
                        .unwrap_or_default();
                    let (ok, errs) = forge_mcp::config::parse_servers(&Value::Object(map));
                    p.skipped.extend(errs.into_iter().map(|e| format!("{e} ({})", f.display())));
                    add(&mut p, source, &f, at.user_settings, ok);
                }
                for (from, to, scope) in [
                    (at.home.join(".gemini/GEMINI.md"), at.config_dir.join("FORGE.md"), "your instructions"),
                    (at.project.join("GEMINI.md"), at.project.join("FORGE.md"), "project instructions"),
                ] {
                    if let Some(text) = read(&from) {
                        p.instructions.push(InstructionImport {
                            source,
                            from: vec![from],
                            text: text.trim().to_string(),
                            heading: format!("## Imported from {} ({scope})", source.name()),
                            to,
                        });
                    }
                }
            }
            Source::Cursor => {
                for (f, to) in [
                    (at.home.join(".cursor/mcp.json"), at.user_settings.to_path_buf()),
                    (at.project.join(".cursor/mcp.json"), project_mcp.clone()),
                ] {
                    if let Some(v) = read(&f).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                        let (ok, errs) = forge_mcp::config::parse_servers(&v);
                        p.skipped.extend(errs.into_iter().map(|e| format!("{e} ({})", f.display())));
                        add(&mut p, source, &f, &to, ok);
                    }
                }
                let mut from = vec![];
                let mut parts = vec![];
                let rules = at.project.join(".cursor/rules");
                let mut files: Vec<PathBuf> =
                    std::fs::read_dir(&rules).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
                files.retain(|f| matches!(f.extension().and_then(|e| e.to_str()), Some("mdc" | "md")));
                files.sort();
                files.insert(0, at.project.join(".cursorrules"));
                for f in files {
                    if let Some(text) = read(&f) {
                        let (_, body) = forge_agents::frontmatter::parse(&text);
                        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        parts.push(format!("### {name}\n\n{}", body.trim()));
                        from.push(f);
                    }
                }
                if !parts.is_empty() {
                    p.instructions.push(InstructionImport {
                        source,
                        from,
                        text: parts.join("\n\n"),
                        heading: "## Imported from Cursor rules".into(),
                        to: at.project.join("FORGE.md"),
                    });
                }
            }
        }
    }
    // Instructions already imported once stay as they are.
    p.instructions.retain(|i| {
        let done = read(&i.to).is_some_and(|t| t.lines().any(|l| l.trim() == i.heading));
        if done {
            p.skipped.push(format!(
                "{}: already imported into {}",
                i.heading.trim_start_matches("## "),
                i.to.display()
            ));
        }
        !done
    });
    p
}

/// Apply the plan. Returns a line per change.
pub fn apply(p: &Plan) -> Result<Vec<String>, String> {
    let mut done = vec![];
    for s in &p.servers {
        forge_mcp::config::write_server(&s.to, &s.name, Some(&s.config))
            .map_err(|e| format!("could not write {}: {e}", s.to.display()))?;
        done.push(format!("Added MCP server {} ({}) to {}", s.name, s.config.transport(), s.to.display()));
    }
    for i in &p.instructions {
        let mut text = std::fs::read_to_string(&i.to).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("{}\n\n{}\n", i.heading, i.text));
        if let Some(dir) = i.to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&i.to, text).map_err(|e| format!("could not write {}: {e}", i.to.display()))?;
        done.push(format!("Appended {} to {}", i.heading.trim_start_matches("## "), i.to.display()));
    }
    Ok(done)
}

/// Describe a server for the plan listing.
pub fn describe_server(c: &ServerConfig) -> String {
    match c {
        ServerConfig::Stdio { command, args, env } => {
            let mut s =
                std::iter::once(command.as_str()).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
            if !env.is_empty() {
                s.push_str(&format!(" (env: {})", env.keys().cloned().collect::<Vec<_>>().join(", ")));
            }
            s
        }
        ServerConfig::Http { url, .. } | ServerConfig::Sse { url, .. } => url.clone(),
    }
}

// ---- A TOML subset, enough for Codex's config.toml ----
//
// Tables (`[a.b]`, quoted keys), `key = value` with dotted keys, basic and
// literal strings (and their triple-quoted forms), integers, floats,
// booleans, arrays (multi-line too) and inline tables. Array-of-tables
// (`[[x]]`) and dates are accepted but read as plain values or skipped.

struct Toml<'a> {
    s: &'a [u8],
    i: usize,
}

impl Toml<'_> {
    fn err(&self, what: &str) -> String {
        let line = self.s[..self.i.min(self.s.len())].iter().filter(|b| **b == b'\n').count() + 1;
        format!("TOML line {line}: {what}")
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Spaces and tabs (and newlines and comments when `lines`).
    fn skip(&mut self, lines: bool) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' => self.i += 1,
                b'\n' if lines => self.i += 1,
                b'#' => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn key_part(&mut self) -> Result<String, String> {
        match self.peek() {
            Some(b'"') | Some(b'\'') => match self.value()? {
                Value::String(s) => Ok(s),
                _ => Err(self.err("bad key")),
            },
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
                    self.i += 1;
                }
                if start == self.i {
                    return Err(self.err("expected a key"));
                }
                Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
        }
    }

    fn key(&mut self) -> Result<Vec<String>, String> {
        let mut parts = vec![];
        loop {
            self.skip(false);
            parts.push(self.key_part()?);
            self.skip(false);
            if self.peek() == Some(b'.') {
                self.i += 1;
            } else {
                return Ok(parts);
            }
        }
    }

    fn string(&mut self, quote: u8) -> Result<String, String> {
        let triple = self.s[self.i..].starts_with(&[quote, quote, quote]);
        self.i += if triple { 3 } else { 1 };
        if triple && self.peek() == Some(b'\n') {
            self.i += 1;
        }
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err(self.err("unterminated string")) };
            if c == quote && (!triple || self.s[self.i..].starts_with(&[quote, quote, quote])) {
                self.i += if triple { 3 } else { 1 };
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            if c == b'\n' && !triple {
                return Err(self.err("newline in a string"));
            }
            self.i += 1;
            if c == b'\\' && quote == b'"' {
                let Some(e) = self.peek() else { return Err(self.err("bad escape")) };
                self.i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'u' | b'U' => {
                        let n = if e == b'u' { 4 } else { 8 };
                        let hex = std::str::from_utf8(self.s.get(self.i..self.i + n).ok_or(self.err("bad escape"))?)
                            .map_err(|_| self.err("bad escape"))?;
                        let ch =
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).ok_or(self.err("bad escape"))?;
                        self.i += n;
                        out.extend_from_slice(ch.to_string().as_bytes());
                    }
                    b'\n' if triple => self.skip(true),
                    _ => return Err(self.err("bad escape")),
                }
            } else {
                out.push(c);
            }
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip(false);
        match self.peek() {
            Some(q @ (b'"' | b'\'')) => self.string(q).map(Value::String),
            Some(b'[') => {
                self.i += 1;
                let mut items = vec![];
                loop {
                    self.skip(true);
                    if self.peek() == Some(b']') {
                        self.i += 1;
                        return Ok(Value::Array(items));
                    }
                    items.push(self.value()?);
                    self.skip(true);
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return Err(self.err("expected , or ] in an array")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    self.skip(false);
                    if self.peek() == Some(b'}') {
                        self.i += 1;
                        return Ok(Value::Object(m));
                    }
                    let k = self.key()?;
                    if self.peek() != Some(b'=') {
                        return Err(self.err("expected = in an inline table"));
                    }
                    self.i += 1;
                    let v = self.value()?;
                    insert(&mut m, &k, v).map_err(|e| self.err(&e))?;
                    self.skip(false);
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {}
                        _ => return Err(self.err("expected , or } in an inline table")),
                    }
                }
            }
            _ => {
                let start = self.i;
                while self
                    .peek()
                    .is_some_and(|c| !matches!(c, b',' | b']' | b'}' | b'\n' | b'#' | b'\r' | b' ' | b'\t'))
                {
                    self.i += 1;
                }
                let raw = String::from_utf8_lossy(&self.s[start..self.i]).trim().to_string();
                match raw.as_str() {
                    "true" => Ok(Value::Bool(true)),
                    "false" => Ok(Value::Bool(false)),
                    "" => Err(self.err("expected a value")),
                    r => Ok(r
                        .replace('_', "")
                        .parse::<i64>()
                        .map(Value::from)
                        .or_else(|_| r.parse::<f64>().map(Value::from))
                        .unwrap_or(Value::String(raw))),
                }
            }
        }
    }
}

fn insert(m: &mut Map<String, Value>, key: &[String], v: Value) -> Result<(), String> {
    let (last, parents) = key.split_last().ok_or("empty key")?;
    let mut cur = m;
    for k in parents {
        let entry = cur.entry(k.clone()).or_insert_with(|| Value::Object(Map::new()));
        cur = entry.as_object_mut().ok_or(format!("{k} is not a table"))?;
    }
    if cur.contains_key(last) {
        return Err(format!("{last} is set twice"));
    }
    cur.insert(last.clone(), v);
    Ok(())
}

/// Parse the TOML subset into JSON.
pub fn parse_toml(text: &str) -> Result<Value, String> {
    let mut t = Toml { s: text.as_bytes(), i: 0 };
    let mut root = Map::new();
    let mut table: Vec<String> = vec![];
    loop {
        t.skip(true);
        let Some(c) = t.peek() else { break };
        if c == b'[' {
            let array = t.s[t.i..].starts_with(b"[[");
            t.i += if array { 2 } else { 1 };
            table = t.key()?;
            let close: &[u8] = if array { b"]]" } else { b"]" };
            if !t.s[t.i..].starts_with(close) {
                return Err(t.err("expected ] after a table name"));
            }
            t.i += close.len();
            // Arrays of tables aren't needed here: their entries are kept as one table.
            let mut cur = &mut root;
            for k in &table {
                let entry = cur.entry(k.clone()).or_insert_with(|| Value::Object(Map::new()));
                cur = entry.as_object_mut().ok_or_else(|| t.err(&format!("{k} is not a table")))?;
            }
        } else {
            let key = t.key()?;
            if t.peek() != Some(b'=') {
                return Err(t.err("expected ="));
            }
            t.i += 1;
            let v = t.value()?;
            let mut full = table.clone();
            full.extend(key);
            insert(&mut root, &full, v).map_err(|e| t.err(&e))?;
        }
        t.skip(false);
        if t.peek().is_some_and(|c| c != b'\n') {
            return Err(t.err("unexpected text after a value"));
        }
    }
    Ok(Value::Object(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_codex_style_toml() {
        let v = parse_toml(
            r#"
# Codex settings
model = "o4"
approval_policy = 'on-request'

[mcp_servers.docs]
command = "npx"
args = [
  "-y",   # the package runner
  "docs-mcp",
]
env = { "DOCS_TOKEN" = "abc", MODE = "fast" }

[mcp_servers."web search".env]
KEY = "xA\n"

[mcp_servers.remote]
url = "https://mcp.example.com/mcp"
startup_timeout_sec = 20
enabled = true
"#,
        )
        .unwrap();
        assert_eq!(v["model"], "o4");
        assert_eq!(v["approval_policy"], "on-request");
        assert_eq!(v["mcp_servers"]["docs"]["args"], json!(["-y", "docs-mcp"]));
        assert_eq!(v["mcp_servers"]["docs"]["env"], json!({"DOCS_TOKEN": "abc", "MODE": "fast"}));
        assert_eq!(v["mcp_servers"]["web search"]["env"]["KEY"], "xA\n");
        assert_eq!(v["mcp_servers"]["remote"]["startup_timeout_sec"], 20);
        assert_eq!(v["mcp_servers"]["remote"]["enabled"], true);
        let triple = parse_toml("a = \"\"\"\nline one\nline two\"\"\"\nb = '''raw \\n'''").unwrap();
        assert_eq!((triple["a"].as_str(), triple["b"].as_str()), (Some("line one\nline two"), Some("raw \\n")));
        for bad in ["a = ", "a = \"open", "[t\nb = 1", "a = 1 b", "a = 1\na = 2", "a = [1 2]"] {
            assert!(parse_toml(bad).is_err(), "{bad:?}");
        }
    }

    fn places<'a>(home: &'a Path, project: &'a Path, user: &'a Path, config: &'a Path) -> Places<'a> {
        Places { home, project, user_settings: user, config_dir: config }
    }

    #[test]
    fn plans_and_applies_imports_once() {
        let d = tempfile::tempdir().unwrap();
        let (home, project, config) = (d.path().join("home"), d.path().join("proj"), d.path().join("cfg"));
        let user = config.join("settings.json");
        let w = |p: PathBuf, t: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        w(home.join(".codex/config.toml"), "[mcp_servers.docs]\ncommand = \"npx\"\nargs = [\"docs-mcp\"]\n");
        w(
            home.join(".gemini/settings.json"),
            r#"{"mcpServers": {"web": {"httpUrl": "https://w.example/mcp"}, "events": {"url": "https://e.example/sse"}, "docs": {"command": "other"}}}"#,
        );
        w(project.join("GEMINI.md"), "Use pnpm.");
        w(
            project.join(".cursor/mcp.json"),
            r#"{"mcpServers": {"repo-db": {"command": "db-mcp", "env": {"DB": "x"}}}}"#,
        );
        w(project.join(".cursor/rules/style.mdc"), "---\ndescription: style\nglobs: *.ts\n---\nPrefer named exports.");
        w(project.join(".cursorrules"), "Write tests first.");
        w(user.clone(), r#"{"mcpServers": {"existing": {"command": "x"}}}"#);

        let at = places(&home, &project, &user, &config);
        let p = plan(&Source::ALL, &at);
        let names: Vec<(&str, &str)> =
            p.servers.iter().map(|s| (s.name.as_str(), if s.to == user { "user" } else { "project" })).collect();
        assert_eq!(names, [("docs", "user"), ("web", "user"), ("events", "user"), ("repo-db", "project")]);
        assert!(p.skipped.iter().any(|s| s.contains("MCP server docs (Gemini CLI)")), "{:?}", p.skipped);
        assert_eq!(p.servers[1].config.transport(), "http");
        assert_eq!(p.servers[2].config.transport(), "sse");
        assert_eq!(p.instructions.len(), 2);
        assert!(
            p.instructions[1].text.contains("### .cursorrules\n\nWrite tests first.")
                && p.instructions[1].text.contains("### style\n\nPrefer named exports.")
        );
        assert!(!p.instructions[1].text.contains("globs"));

        let done = apply(&p).unwrap();
        assert_eq!(done.len(), 6);
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(&user).unwrap()).unwrap();
        assert_eq!(settings["mcpServers"]["docs"]["command"], "npx");
        assert!(settings["mcpServers"]["existing"].is_object(), "existing servers stay");
        let repo: Value = serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(repo["mcpServers"]["repo-db"]["env"]["DB"], "x");
        let forge_md = std::fs::read_to_string(project.join("FORGE.md")).unwrap();
        assert!(forge_md.contains("## Imported from Gemini CLI (project instructions)\n\nUse pnpm."), "{forge_md}");

        // Again: nothing new.
        let again = plan(&Source::ALL, &at);
        assert!(again.is_empty(), "{again:?}");
        assert!(again.skipped.iter().any(|s| s.contains("already imported")));
        assert!(plan(&[Source::Codex], &places(&d.path().join("nobody"), &project, &user, &config)).is_empty());
    }
}
