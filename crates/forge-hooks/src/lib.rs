//! Hooks: user commands run at lifecycle events.
//!
//! Configuration (settings `hooks`):
//! `{"PreToolUse": [{"matcher": "Bash|Edit", "hooks": [{"type": "command", "command": "...", "timeout": 60}]}]}`.
//! Each matching command gets the event as JSON on stdin. Exit 0 = success
//! (stdout may be a JSON decision), exit 2 = block, anything else = a
//! non-blocking error. What "block" means depends on the event; the engine
//! applies that (see [`HookEvent::exit2_effect`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use forge_platform::shell::ShellChoice;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    Stop,
    SubagentStop,
    SessionStart,
    SessionEnd,
    PreCompact,
    Notification,
}

/// Who sees a hook's stderr when it exits with code 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit2Effect {
    /// Blocks the action; stderr goes to the model.
    BlockToModel,
    /// Cannot block (it already happened); stderr goes to the model.
    FeedbackToModel,
    /// Blocks the action; stderr goes to the user only.
    BlockToUser,
    /// Nothing to block; stderr goes to the user.
    UserOnly,
}

impl HookEvent {
    pub const ALL: [HookEvent; 10] = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PostToolUseFailure,
        HookEvent::UserPromptSubmit,
        HookEvent::Stop,
        HookEvent::SubagentStop,
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
        HookEvent::PreCompact,
        HookEvent::Notification,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::Notification => "Notification",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// Contract C11.
    pub fn exit2_effect(&self) -> Exit2Effect {
        match self {
            HookEvent::PreToolUse | HookEvent::Stop | HookEvent::SubagentStop => Exit2Effect::BlockToModel,
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => Exit2Effect::FeedbackToModel,
            HookEvent::UserPromptSubmit => Exit2Effect::BlockToUser,
            HookEvent::PreCompact | HookEvent::Notification | HookEvent::SessionStart | HookEvent::SessionEnd => {
                Exit2Effect::UserOnly
            }
        }
    }

    /// Events whose plain (non-JSON) stdout on exit 0 is added to the model's context.
    pub fn stdout_is_context(&self) -> bool {
        matches!(self, HookEvent::UserPromptSubmit | HookEvent::SessionStart)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookCommand {
    pub command: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Matcher {
    /// `None` matches everything.
    pub pattern: Option<String>,
    pub hooks: Vec<HookCommand>,
}

impl Matcher {
    pub fn matches(&self, value: Option<&str>) -> bool {
        let Some(p) = self.pattern.as_deref().filter(|p| !p.is_empty() && *p != "*") else { return true };
        let Some(v) = value else { return false };
        if p == v {
            return true;
        }
        regex::Regex::new(&format!("^(?:{p})$")).map(|r| r.is_match(v)).unwrap_or(false)
    }
}

/// Parsed `hooks` settings.
#[derive(Debug, Clone, Default)]
pub struct HooksConfig {
    pub events: HashMap<HookEvent, Vec<Matcher>>,
}

impl HooksConfig {
    /// Parse the settings `hooks` object; unknown events and malformed entries are reported.
    pub fn from_settings(v: Option<&Value>) -> (Self, Vec<String>) {
        let mut cfg = HooksConfig::default();
        let mut errors = vec![];
        let Some(obj) = v.and_then(Value::as_object) else { return (cfg, errors) };
        for (name, list) in obj {
            let Some(event) = HookEvent::parse(name) else {
                errors.push(format!("unknown hook event {name}"));
                continue;
            };
            for m in list.as_array().into_iter().flatten() {
                let hooks: Vec<HookCommand> = m
                    .get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|h| {
                        if h.get("type").and_then(Value::as_str).unwrap_or("command") != "command" {
                            errors.push(format!("{name}: only command hooks are supported"));
                            return None;
                        }
                        let command = h.get("command").and_then(Value::as_str)?.to_string();
                        let timeout = h
                            .get("timeout")
                            .and_then(Value::as_f64)
                            .map(Duration::from_secs_f64)
                            .unwrap_or(DEFAULT_TIMEOUT);
                        Some(HookCommand { command, timeout })
                    })
                    .collect();
                if hooks.is_empty() {
                    continue;
                }
                let pattern = m.get("matcher").and_then(Value::as_str).map(str::to_string);
                cfg.events.entry(event).or_default().push(Matcher { pattern, hooks });
            }
        }
        (cfg, errors)
    }

    pub fn is_empty(&self) -> bool {
        self.events.values().all(|v| v.is_empty())
    }

    pub fn has(&self, event: HookEvent) -> bool {
        self.events.get(&event).map(|v| !v.is_empty()).unwrap_or(false)
    }
}

/// Combined result of all hooks that ran for one event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookOutcome {
    /// Exit 2 or `decision: "block"`: the reason text (stderr or `reason`).
    pub blocked: Option<String>,
    /// PreToolUse `permissionDecision`: "allow" | "deny" | "ask" with reason.
    pub permission: Option<(String, String)>,
    pub updated_input: Option<Value>,
    pub additional_context: Vec<String>,
    /// `continue: false`: stop the whole run, with this reason.
    pub stop: Option<String>,
    /// `systemMessage`s and non-blocking errors to show the user.
    pub user_messages: Vec<String>,
    pub suppress_output: bool,
    /// How many hook commands ran.
    pub ran: usize,
}

fn rank(decision: &str) -> u8 {
    match decision {
        "deny" => 3,
        "ask" => 2,
        "allow" => 1,
        _ => 0,
    }
}

impl HookOutcome {
    fn absorb(&mut self, event: HookEvent, code: Option<i32>, stdout: &str, stderr: &str, command: &str) {
        self.ran += 1;
        match code {
            Some(0) => {
                let trimmed = stdout.trim();
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(Value::Object(o)) => self.absorb_json(event, &Value::Object(o)),
                    _ if event.stdout_is_context() && !trimmed.is_empty() => {
                        self.additional_context.push(trimmed.to_string())
                    }
                    _ => {}
                }
            }
            Some(2) => {
                let msg = if stderr.trim().is_empty() {
                    format!("blocked by hook: {command}")
                } else {
                    stderr.trim().to_string()
                };
                self.blocked = Some(match self.blocked.take() {
                    Some(prev) => format!("{prev}\n{msg}"),
                    None => msg,
                });
            }
            other => {
                let code = other.map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
                self.user_messages.push(format!("{} hook error (exit {code}): {}", event.as_str(), stderr.trim()));
            }
        }
    }

    fn absorb_json(&mut self, event: HookEvent, v: &Value) {
        if v.get("continue").and_then(Value::as_bool) == Some(false) {
            self.stop = Some(v.get("stopReason").and_then(Value::as_str).unwrap_or("stopped by hook").to_string());
        }
        if v.get("suppressOutput").and_then(Value::as_bool) == Some(true) {
            self.suppress_output = true;
        }
        if let Some(m) = v.get("systemMessage").and_then(Value::as_str) {
            self.user_messages.push(m.to_string());
        }
        if v.get("decision").and_then(Value::as_str) == Some("block") {
            let reason = v.get("reason").and_then(Value::as_str).unwrap_or("blocked by hook").to_string();
            self.blocked = Some(match self.blocked.take() {
                Some(prev) => format!("{prev}\n{reason}"),
                None => reason,
            });
        }
        let Some(hso) = v.get("hookSpecificOutput") else { return };
        if event == HookEvent::PreToolUse {
            if let Some(d) = hso.get("permissionDecision").and_then(Value::as_str) {
                let reason = hso.get("permissionDecisionReason").and_then(Value::as_str).unwrap_or("").to_string();
                if self.permission.as_ref().map(|(p, _)| rank(d) > rank(p)).unwrap_or(true) {
                    self.permission = Some((d.to_string(), reason));
                }
            }
            if let Some(u) = hso.get("updatedInput").filter(|u| u.is_object()) {
                self.updated_input = Some(u.clone());
            }
        }
        if let Some(c) = hso.get("additionalContext").and_then(Value::as_str) {
            self.additional_context.push(c.to_string());
        }
    }
}

/// Session facts every hook input carries.
#[derive(Debug, Clone, Default)]
pub struct HookBase {
    pub session_id: String,
    pub transcript_path: Option<PathBuf>,
    pub cwd: PathBuf,
    pub project_dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct HookRunner {
    pub config: HooksConfig,
    pub base: HookBase,
    /// `--bare` / `--safe-mode`: hooks never run.
    pub disabled: bool,
    /// The session's shell; `None` uses the one found from the process environment.
    pub shell: Option<ShellChoice>,
}

impl HookRunner {
    pub fn new(config: HooksConfig, base: HookBase) -> Self {
        HookRunner { config, base, disabled: false, shell: None }
    }

    pub fn enabled_for(&self, event: HookEvent) -> bool {
        !self.disabled && self.config.has(event)
    }

    /// Run every matching hook for `event` in parallel and combine the results.
    /// `match_value` is what matchers test (tool name, SessionStart source, PreCompact trigger).
    pub async fn run(
        &self,
        event: HookEvent,
        match_value: Option<&str>,
        mode: &str,
        fields: Value,
        cancel: &CancellationToken,
    ) -> HookOutcome {
        let mut out = HookOutcome::default();
        if !self.enabled_for(event) {
            return out;
        }
        let commands: Vec<HookCommand> = self.config.events[&event]
            .iter()
            .filter(|m| m.matches(match_value))
            .flat_map(|m| m.hooks.iter().cloned())
            .collect();
        if commands.is_empty() {
            return out;
        }
        tracing::debug!(event = event.as_str(), matcher = ?match_value, hooks = commands.len(), "running hooks");
        let mut input = json!({
            "session_id": self.base.session_id,
            "transcript_path": self.base.transcript_path,
            "cwd": self.base.cwd,
            "permission_mode": mode,
            "hook_event_name": event.as_str(),
        });
        if let (Value::Object(a), Value::Object(b)) = (&mut input, fields) {
            a.extend(b);
        }
        let stdin = serde_json::to_string(&input).unwrap_or_default();
        let shell = self.shell.as_ref().unwrap_or_else(|| forge_platform::shell::detect());
        let runs = commands.iter().map(|c| run_one(shell, c, &stdin, &self.base, cancel));
        for (cmd, (code, stdout, stderr)) in commands.iter().zip(futures::future::join_all(runs).await) {
            out.absorb(event, code, &stdout, &stderr, &cmd.command);
        }
        out
    }
}

async fn run_one(
    shell: &ShellChoice,
    cmd: &HookCommand,
    stdin: &str,
    base: &HookBase,
    cancel: &CancellationToken,
) -> (Option<i32>, String, String) {
    let not_started = |why: String| (Some(1), String::new(), format!("could not start hook `{}`: {why}", cmd.command));
    let shell = match shell {
        Ok(s) => s,
        Err(m) => return not_started(m.to_string()),
    };
    // Windows: the script file must outlive the process.
    let (std_cmd, _script) = match shell.command(&shell.script(&cmd.command, None)) {
        Ok(c) => c,
        Err(e) => return not_started(e.to_string()),
    };
    let mut std_cmd = std_cmd;
    forge_platform::process::no_window(&mut std_cmd);
    let mut c = Command::from(std_cmd);
    c.current_dir(&base.cwd)
        .env("FORGE_PROJECT_DIR", &base.project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return not_started(format!("could not start {}: {e}", shell.program.display())),
    };
    if let Some(mut si) = child.stdin.take() {
        let data = stdin.as_bytes().to_vec();
        tokio::spawn(async move {
            let _ = si.write_all(&data).await;
        });
    }
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let read = async {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let _ = tokio::join!(so.read_to_end(&mut a), se.read_to_end(&mut b));
        let status = child.wait().await.ok().and_then(|s| s.code());
        (status, String::from_utf8_lossy(&a).into_owned(), String::from_utf8_lossy(&b).into_owned())
    };
    tokio::select! {
        r = read => r,
        _ = tokio::time::sleep(cmd.timeout) => (Some(124), String::new(), format!("hook timed out after {:?}: {}", cmd.timeout, cmd.command)),
        _ = cancel.cancelled() => (Some(130), String::new(), "hook cancelled".into()),
    }
}

#[cfg(test)]
mod tests;
