//! Permission decisions.
//!
//! A tool call is described by a [`Request`] (tool name, what it touches,
//! whether it only reads). [`Engine::decide`] combines the permission mode,
//! the allow / ask / deny rules and the working directories into a
//! [`Decision`]. Order: deny rules, then bypass, then ask rules, then allow
//! rules, then the mode's defaults.

mod mode;
mod paths;
mod rule;
mod shell;

pub use mode::PermissionMode;
pub use paths::{is_within, normalize};
pub use rule::{Rule, RuleError, RuleSpec};
pub use shell::split_compound;

use std::path::{Path, PathBuf};

/// What a tool call touches, for matching rule specifiers.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    /// A shell command line (Bash).
    Command(String),
    /// A filesystem path; `write` for tools that modify it.
    Path { path: PathBuf, write: bool },
    /// A URL (WebFetch).
    Url(String),
    /// A named target: the skill for Skill, the agent type for Task.
    Name(String),
    /// Nothing rule-matchable beyond the tool name.
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request<'a> {
    pub tool: &'a str,
    pub subject: Subject,
    /// The call cannot change anything (Read, Grep, `git status`, ...).
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Behavior {
    Allow,
    Ask,
    Deny,
}

/// Why a decision was made (surfaced to the model and the host).
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Rule { behavior: Behavior, rule: String },
    Mode(PermissionMode),
    ReadOnlyInWorkingDir,
    OutsideWorkingDirs(PathBuf),
    PlanMode,
    Default,
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Rule { behavior, rule } => write!(f, "matched {behavior:?} rule {rule}"),
            Reason::Mode(m) => write!(f, "permission mode {}", m.as_str()),
            Reason::ReadOnlyInWorkingDir => write!(f, "read-only access inside the working directories"),
            Reason::OutsideWorkingDirs(p) => write!(f, "{} is outside the working directories", p.display()),
            Reason::PlanMode => write!(f, "plan mode allows only read-only tools"),
            Reason::Default => write!(f, "this tool requires permission"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow { reason: Reason },
    Ask { reason: Reason, suggestions: Vec<Suggestion> },
    Deny { reason: Reason },
}

impl Decision {
    pub fn behavior(&self) -> Behavior {
        match self {
            Decision::Allow { .. } => Behavior::Allow,
            Decision::Ask { .. } => Behavior::Ask,
            Decision::Deny { .. } => Behavior::Deny,
        }
    }
}

/// A change the user can accept along with an approval ("always allow").
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Suggestion {
    AddRules { rules: Vec<String>, behavior: Behavior, destination: String },
    SetMode { mode: String, destination: String },
    AddDirectories { directories: Vec<String>, destination: String },
}

/// Rules from every source, already merged.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub allow: Vec<Rule>,
    pub ask: Vec<Rule>,
    pub deny: Vec<Rule>,
}

impl RuleSet {
    /// Parse rule strings; invalid ones are returned separately so callers can warn.
    pub fn from_strings(allow: &[String], ask: &[String], deny: &[String]) -> (Self, Vec<RuleError>) {
        let mut errors = vec![];
        let mut parse = |list: &[String]| -> Vec<Rule> {
            list.iter()
                .flat_map(|s| split_rule_list(s))
                .filter_map(|s| Rule::parse(&s).map_err(|e| errors.push(e)).ok())
                .collect()
        };
        let set = RuleSet { allow: parse(allow), ask: parse(ask), deny: parse(deny) };
        (set, errors)
    }

    pub fn add(&mut self, behavior: Behavior, rule: Rule) {
        let list = match behavior {
            Behavior::Allow => &mut self.allow,
            Behavior::Ask => &mut self.ask,
            Behavior::Deny => &mut self.deny,
        };
        if !list.contains(&rule) {
            list.push(rule);
        }
    }
}

/// Split a `--allowedTools` value: commas or whitespace separate rules, but
/// not inside parentheses (`Bash(git commit *)` stays whole).
pub fn split_rule_list(raw: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' | ' ' | '\t' | '\n' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Tools that edit files: `Edit(...)` rules cover all of them.
pub const EDIT_TOOLS: &[&str] = &["Edit", "MultiEdit", "Write", "NotebookEdit"];
/// Tools that read files: `Read(...)` rules cover all of them.
pub const READ_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "NotebookRead"];

/// Commands `acceptEdits` mode allows inside the working directories.
const ACCEPT_EDITS_COMMANDS: &[&str] = &["mkdir", "touch", "mv", "cp", "rm", "rmdir", "sed"];

#[derive(Debug, Clone)]
pub struct Engine {
    pub mode: PermissionMode,
    pub rules: RuleSet,
    pub cwd: PathBuf,
    /// cwd plus `--add-dir` directories.
    pub working_dirs: Vec<PathBuf>,
}

impl Engine {
    pub fn new(mode: PermissionMode, rules: RuleSet, cwd: &Path, extra_dirs: &[PathBuf]) -> Self {
        let cwd = normalize(cwd, Path::new("/"));
        let mut working_dirs = vec![cwd.clone()];
        for d in extra_dirs {
            let d = normalize(d, &cwd);
            if !working_dirs.contains(&d) {
                working_dirs.push(d);
            }
        }
        Engine { mode, rules, cwd, working_dirs }
    }

    pub fn add_directory(&mut self, dir: &Path) {
        let d = normalize(dir, &self.cwd);
        if !self.working_dirs.contains(&d) {
            self.working_dirs.push(d);
        }
    }

    pub fn in_working_dirs(&self, path: &Path) -> bool {
        let p = normalize(path, &self.cwd);
        self.working_dirs.iter().any(|d| is_within(&p, d))
    }

    fn first_match<'r>(&self, rules: &'r [Rule], req: &Request) -> Option<&'r Rule> {
        rules.iter().find(|r| r.matches(req, &self.cwd, MatchMode::Any))
    }

    /// The allow rule(s) covering `req`, joined for display.
    fn allow_rule_for(&self, req: &Request) -> Option<String> {
        if let Subject::Command(cmd) = &req.subject {
            let parts = split_compound(cmd);
            if parts.is_empty() {
                return None;
            }
            let mut used: Vec<String> = vec![];
            for part in parts {
                let sub = Request { tool: req.tool, subject: Subject::Command(part), read_only: req.read_only };
                let r = self.rules.allow.iter().find(|r| r.matches(&sub, &self.cwd, MatchMode::All))?;
                let name = r.to_string();
                if !used.contains(&name) {
                    used.push(name);
                }
            }
            return Some(used.join(", "));
        }
        self.rules.allow.iter().find(|r| r.matches(req, &self.cwd, MatchMode::All)).map(|r| r.to_string())
    }

    pub fn decide(&self, req: &Request) -> Decision {
        // 1. Deny rules always win. For compound commands a deny matches if any part does.
        if let Some(r) = self.first_match(&self.rules.deny, req) {
            return Decision::Deny { reason: Reason::Rule { behavior: Behavior::Deny, rule: r.to_string() } };
        }
        // 2. Bypass skips prompts, not deny rules.
        if self.mode == PermissionMode::BypassPermissions {
            return Decision::Allow { reason: Reason::Mode(self.mode) };
        }
        // 3. Ask rules.
        if let Some(r) = self.first_match(&self.rules.ask, req) {
            return self.ask_or_deny(Reason::Rule { behavior: Behavior::Ask, rule: r.to_string() }, req);
        }
        // 4. Allow rules. For compound commands every part must be allowed,
        //    possibly each by a different rule.
        if let Some(rule) = self.allow_rule_for(req) {
            return Decision::Allow { reason: Reason::Rule { behavior: Behavior::Allow, rule } };
        }
        // 5. Mode defaults.
        let path_outside = match &req.subject {
            Subject::Path { path, .. } if !self.in_working_dirs(path) => Some(normalize(path, &self.cwd)),
            _ => None,
        };
        if req.read_only {
            return match path_outside {
                None => Decision::Allow { reason: Reason::ReadOnlyInWorkingDir },
                Some(p) => self.ask_or_deny(Reason::OutsideWorkingDirs(p), req),
            };
        }
        if self.mode == PermissionMode::Plan {
            return Decision::Deny { reason: Reason::PlanMode };
        }
        if self.mode == PermissionMode::AcceptEdits && path_outside.is_none() {
            let is_edit = matches!(req.subject, Subject::Path { write: true, .. }) || EDIT_TOOLS.contains(&req.tool);
            let is_fs_command = match &req.subject {
                Subject::Command(c) => split_compound(c).iter().all(|part| {
                    let first = part.split_whitespace().next().unwrap_or("");
                    ACCEPT_EDITS_COMMANDS.contains(&first) && !part.contains("..") && !part.contains(" /")
                }),
                _ => false,
            };
            if is_edit || is_fs_command {
                return Decision::Allow { reason: Reason::Mode(self.mode) };
            }
        }
        if self.mode == PermissionMode::Auto {
            if let Subject::Command(c) = &req.subject {
                if mode::auto_mode_allows(c) {
                    return Decision::Allow { reason: Reason::Mode(self.mode) };
                }
            } else if matches!(req.subject, Subject::Path { write: true, .. }) && path_outside.is_none() {
                return Decision::Allow { reason: Reason::Mode(self.mode) };
            }
        }
        let reason = path_outside.map(Reason::OutsideWorkingDirs).unwrap_or(Reason::Default);
        self.ask_or_deny(reason, req)
    }

    fn ask_or_deny(&self, reason: Reason, req: &Request) -> Decision {
        if self.mode == PermissionMode::DontAsk {
            return Decision::Deny { reason };
        }
        Decision::Ask { reason, suggestions: self.suggest(req) }
    }

    /// "Always allow" options offered alongside a prompt.
    pub fn suggest(&self, req: &Request) -> Vec<Suggestion> {
        let dest = "localSettings".to_string();
        match &req.subject {
            Subject::Command(c) => {
                let rules: Vec<String> = split_compound(c)
                    .iter()
                    .filter_map(|part| {
                        let words: Vec<&str> = part.split_whitespace().collect();
                        let n = if words.len() > 1 && !words[1].starts_with('-') { 2 } else { 1 };
                        (!words.is_empty())
                            .then(|| format!("{}({} *)", req.tool, words[..n.min(words.len())].join(" ")))
                    })
                    .collect();
                if rules.is_empty() {
                    vec![]
                } else {
                    vec![Suggestion::AddRules { rules, behavior: Behavior::Allow, destination: dest }]
                }
            }
            Subject::Path { path, write } => {
                let p = normalize(path, &self.cwd);
                if !self.in_working_dirs(&p) {
                    let dir =
                        if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or(p.clone()) };
                    vec![Suggestion::AddDirectories {
                        directories: vec![dir.display().to_string()],
                        destination: "session".into(),
                    }]
                } else if *write {
                    vec![Suggestion::SetMode { mode: "acceptEdits".into(), destination: "session".into() }]
                } else {
                    vec![]
                }
            }
            Subject::Url(u) => match rule::host_of(u) {
                Some(h) => vec![Suggestion::AddRules {
                    rules: vec![format!("{}(domain:{h})", req.tool)],
                    behavior: Behavior::Allow,
                    destination: dest,
                }],
                None => vec![],
            },
            Subject::Name(n) => vec![Suggestion::AddRules {
                rules: vec![format!("{}({n})", req.tool)],
                behavior: Behavior::Allow,
                destination: dest,
            }],
            Subject::None => {
                vec![Suggestion::AddRules {
                    rules: vec![req.tool.to_string()],
                    behavior: Behavior::Allow,
                    destination: dest,
                }]
            }
        }
    }
}

/// How a rule matches a compound shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Any sub-command matching is enough (deny / ask).
    Any,
    /// Every sub-command must match (allow).
    All,
}

#[cfg(test)]
mod tests;
