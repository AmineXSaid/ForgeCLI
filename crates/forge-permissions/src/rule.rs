use std::fmt;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;

use crate::paths::normalize;
use crate::shell::split_compound;
use crate::{MatchMode, Request, Subject, EDIT_TOOLS, READ_TOOLS};

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("invalid permission rule {rule:?}: {why}")]
pub struct RuleError {
    pub rule: String,
    pub why: String,
}

/// The part inside the parentheses of `Tool(spec)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSpec {
    /// `Tool` alone: every call.
    Any,
    /// Bash `git commit *` / legacy `git commit:*`: the command or anything after it.
    Prefix(String),
    /// Bash spec with a `*` elsewhere: shell-style wildcard over the command.
    Wildcard(String),
    /// Bash spec without wildcards: the exact command.
    Exact(String),
    /// Read / Edit family: a gitignore-style path pattern.
    Path(String),
    /// WebFetch `domain:example.com` (subdomains included).
    Domain(String),
    /// Any other tool: compared with the call's [`Subject::Name`] or command.
    Literal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub tool: String,
    pub spec: RuleSpec,
    raw_spec: Option<String>,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.raw_spec {
            Some(s) => write!(f, "{}({s})", self.tool),
            None => write!(f, "{}", self.tool),
        }
    }
}

fn is_bash(tool: &str) -> bool {
    tool == "Bash" || tool == "PowerShell"
}

impl Rule {
    pub fn parse(raw: &str) -> Result<Rule, RuleError> {
        let raw = raw.trim();
        let err = |why: &str| RuleError { rule: raw.to_string(), why: why.to_string() };
        let (tool, spec) = match raw.find('(') {
            Some(open) => {
                if !raw.ends_with(')') {
                    return Err(err("missing closing parenthesis"));
                }
                (&raw[..open], Some(raw[open + 1..raw.len() - 1].trim()))
            }
            None => (raw, None),
        };
        let tool = tool.trim();
        if tool.is_empty() || !tool.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '*' | '.')) {
            return Err(err("tool name must be letters, digits, '_', '-' or '*'"));
        }
        let spec_kind = match spec {
            None | Some("") | Some("*") => RuleSpec::Any,
            Some(s) if is_bash(tool) => {
                if let Some(p) = s.strip_suffix(":*") {
                    RuleSpec::Prefix(p.trim().to_string())
                } else if let Some(p) = s.strip_suffix(" *") {
                    if p.contains('*') {
                        RuleSpec::Wildcard(s.to_string())
                    } else {
                        RuleSpec::Prefix(p.trim().to_string())
                    }
                } else if s.contains('*') {
                    RuleSpec::Wildcard(s.to_string())
                } else {
                    RuleSpec::Exact(s.to_string())
                }
            }
            Some(s) if tool == "WebFetch" => match s.strip_prefix("domain:") {
                Some(d) if !d.is_empty() => RuleSpec::Domain(d.to_ascii_lowercase()),
                _ => return Err(err("WebFetch rules take domain:<host>")),
            },
            Some(s) if READ_TOOLS.contains(&tool) || EDIT_TOOLS.contains(&tool) => {
                GlobBuilder::new(s).build().map_err(|e| err(&e.to_string()))?;
                RuleSpec::Path(s.to_string())
            }
            Some(s) => RuleSpec::Literal(s.to_string()),
        };
        Ok(Rule {
            tool: tool.to_string(),
            raw_spec: match spec_kind {
                RuleSpec::Any => None,
                _ => spec.map(str::to_string),
            },
            spec: spec_kind,
        })
    }

    /// Does the rule's tool part cover `tool`?
    pub fn covers_tool(&self, tool: &str) -> bool {
        let t = self.tool.as_str();
        if t == tool || t == "*" {
            return true;
        }
        if t == "Edit" && EDIT_TOOLS.contains(&tool) {
            return true;
        }
        if t == "Read" && READ_TOOLS.contains(&tool) {
            return true;
        }
        // mcp__server and mcp__server__* cover every tool of that server.
        if let Some(server) = t.strip_suffix("__*").or(Some(t)).filter(|s| s.starts_with("mcp__")) {
            if server.matches("__").count() == 1 && tool.starts_with(&format!("{server}__")) {
                return true;
            }
        }
        false
    }

    pub fn matches(&self, req: &Request, cwd: &Path, mode: MatchMode) -> bool {
        if !self.covers_tool(req.tool) {
            return false;
        }
        match (&self.spec, &req.subject) {
            (RuleSpec::Any, _) => true,
            (RuleSpec::Prefix(_) | RuleSpec::Wildcard(_) | RuleSpec::Exact(_), Subject::Command(cmd)) => {
                let parts = split_compound(cmd);
                if parts.is_empty() {
                    return false;
                }
                let one = |p: &String| self.command_matches(p);
                match mode {
                    MatchMode::All => parts.iter().all(one),
                    MatchMode::Any => parts.iter().any(one),
                }
            }
            (RuleSpec::Path(pattern), Subject::Path { path, .. }) => path_matches(pattern, path, cwd),
            (RuleSpec::Domain(d), Subject::Url(u)) => {
                host_of(u).map(|h| h == *d || h.ends_with(&format!(".{d}"))).unwrap_or(false)
            }
            (RuleSpec::Literal(l), Subject::Name(n)) => l == n,
            (RuleSpec::Literal(l), Subject::Command(c)) => l == c,
            _ => false,
        }
    }

    fn command_matches(&self, part: &str) -> bool {
        let part = part.trim();
        match &self.spec {
            RuleSpec::Prefix(p) => part == p || part.starts_with(&format!("{p} ")),
            RuleSpec::Exact(e) => part == e,
            RuleSpec::Wildcard(w) => wildcard(w, part),
            _ => false,
        }
    }
}

/// Shell-style `*` wildcard (matches any run of characters, including none).
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    if !rest.starts_with(parts[0]) {
        return false;
    }
    rest = &rest[parts[0].len()..];
    for (i, p) in parts.iter().enumerate().skip(1) {
        if i == parts.len() - 1 {
            return rest.ends_with(p);
        }
        match rest.find(p) {
            Some(pos) => rest = &rest[pos + p.len()..],
            None => return false,
        }
    }
    true
}

/// Resolve a rule's path pattern: `//abs`, `~/home`, otherwise relative to cwd.
fn resolve_pattern(pattern: &str, cwd: &Path) -> String {
    if let Some(rest) = pattern.strip_prefix("//") {
        format!("/{rest}")
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest).to_string_lossy().into_owned()
    } else if let Some(rest) = pattern.strip_prefix('/') {
        cwd.join(rest).to_string_lossy().into_owned()
    } else {
        let rest = pattern.strip_prefix("./").unwrap_or(pattern);
        if rest.contains('/') || rest.starts_with("**") {
            cwd.join(rest).to_string_lossy().into_owned()
        } else {
            // A bare name like `*.env` matches at any depth, gitignore-style.
            cwd.join("**").join(rest).to_string_lossy().into_owned()
        }
    }
}

fn path_matches(pattern: &str, path: &Path, cwd: &Path) -> bool {
    path_matches_on(cfg!(windows), pattern, path, cwd)
}

/// On Windows the filesystem ignores case and accepts both separators, so a
/// rule must too (a deny on `./secrets/**` also covers `.\Secrets\key`).
fn path_matches_on(windows: bool, pattern: &str, path: &Path, cwd: &Path) -> bool {
    let target: PathBuf = normalize(path, cwd);
    let resolved = resolve_pattern(pattern, cwd);
    let (target, resolved) = if windows {
        (PathBuf::from(crate::paths::fold(&target)), crate::paths::fold(Path::new(&resolved)))
    } else {
        (target, resolved)
    };
    let has_glob = resolved.contains(['*', '?', '[', '{']);
    if !has_glob {
        let p = PathBuf::from(&resolved);
        return target == p || target.starts_with(&p);
    }
    let Ok(glob) = GlobBuilder::new(&resolved).literal_separator(true).backslash_escape(!windows).build() else {
        return false;
    };
    let m = glob.compile_matcher();
    // A directory pattern (`src/**`) also matches the directory itself.
    m.is_match(&target) || resolved.strip_suffix("/**").map(|d| target == Path::new(d)).unwrap_or(false)
}

/// Lower-case host of an http(s) URL.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host_port = rest.split(['/', '?', '#']).next()?;
    let host = host_port.rsplit_once('@').map(|(_, h)| h).unwrap_or(host_port);
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_rules_ignore_case_and_take_git_bash_paths() {
        let cwd = Path::new(r"C:\Users\me\proj");
        assert!(path_matches("./secrets/**", Path::new(r".\Secrets\key.pem"), cwd));
        assert!(path_matches("./secrets/**", Path::new(r"C:\USERS\me\proj\secrets\key.pem"), cwd));
        assert!(path_matches("./secrets/**", Path::new("/c/Users/me/proj/secrets/key.pem"), cwd));
        assert!(path_matches("*.env", Path::new(r"src\Prod.ENV"), cwd));
        assert!(!path_matches("./secrets/**", Path::new(r".\public\key.pem"), cwd));
    }
}
