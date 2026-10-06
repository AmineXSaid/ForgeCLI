use std::time::Duration;

use forge_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::shells::{run_command, ShellStatus};
use crate::{truncate_middle, Tool, ToolContext, ToolOutput, INTERRUPTED};

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// First words of commands that only read.
const READ_ONLY_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "pwd", "echo", "grep", "rg", "find", "which", "file", "stat", "tree", "du",
    "df", "date", "whoami", "uname", "env", "printenv", "sort", "uniq", "cut", "diff", "basename", "dirname",
    "realpath", "readlink", "true", "false", "type", "less", "more", "jq",
];
const READ_ONLY_GIT: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "remote",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "tag",
    "config --get",
];

/// Is every part of the command line known to be read-only?
pub fn command_is_read_only(command: &str) -> bool {
    let parts = forge_permissions::split_compound(command);
    !parts.is_empty()
        && parts.iter().all(|p| {
            if p.contains('>') || p.contains("$(") || p.contains('`') {
                return false;
            }
            let mut words = p.split_whitespace();
            match words.next() {
                Some("git") => {
                    let rest: Vec<&str> = words.collect();
                    let rest = rest.join(" ");
                    READ_ONLY_GIT.iter().any(|g| rest == *g || rest.starts_with(&format!("{g} ")))
                        && !rest.contains("--output")
                }
                Some("find") => !p.contains("-exec") && !p.contains("-delete") && !p.contains("-fprint"),
                Some("sed") => p.contains(" -n") && !p.contains(" -i"),
                Some(w) => READ_ONLY_COMMANDS.contains(&w),
                None => false,
            }
        })
}

pub struct Bash;

#[async_trait::async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> String {
        "Run a shell command in a persistent working directory and return its output.\n\n\
         - The working directory carries over between calls while it stays inside the project's working \
           directories; otherwise it is reset to the project directory.\n\
         - Quote paths that contain spaces. Prefer absolute paths over `cd`.\n\
         - Default timeout 2 minutes, maximum 10 minutes (`timeout` in milliseconds).\n\
         - Output beyond 30,000 characters is truncated in the middle.\n\
         - Use `run_in_background` for long-running processes (servers, watchers) and read their output \
           later with BashOutput.\n\
         - Prefer the dedicated tools for reading (Read), searching (Grep, Glob) and editing (Edit, Write) \
           files instead of cat, grep, find, sed or echo redirection.\n\
         - Chain dependent commands with `&&`; independent commands can be separate calls."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The command to execute"},
                "timeout": {"type": "number", "description": "Optional timeout in milliseconds (max 600000)"},
                "description": {"type": "string", "description": "A short (5-10 word) description of what the command does"},
                "run_in_background": {"type": "boolean", "description": "Run the command in the background; read output with BashOutput"}
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, input: &Value) -> bool {
        !input.get("run_in_background").and_then(Value::as_bool).unwrap_or(false)
            && command_is_read_only(str_arg(input, "command"))
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Command(str_arg(input, "command").to_string())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let command = str_arg(&input, "command").to_string();
        if command.trim().is_empty() {
            return ToolOutput::error("command must not be empty");
        }
        let cwd = ctx.shell_cwd();
        let cwd = if cwd.is_dir() { cwd } else { ctx.project_dir.clone() };

        if input.get("run_in_background").and_then(Value::as_bool).unwrap_or(false) {
            return match ctx.shells.spawn(&command, &cwd, &ctx.env) {
                Ok(sh) => ToolOutput::text(format!(
                    "Command running in background with ID: {}. Use BashOutput to read its output.",
                    sh.id
                ))
                .with_structured(json!({"backgroundTaskId": sh.id, "stdout": "", "stderr": "", "interrupted": false})),
                Err(e) => ToolOutput::error(format!("Failed to start command: {e}")),
            };
        }

        let ms = input.get("timeout").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
        let res = match run_command(&command, &cwd, &ctx.env, Duration::from_millis(ms), &ctx.cancel).await {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Failed to run command: {e}")),
        };

        let mut note = String::new();
        if let Some(dir) = &res.final_cwd {
            if ctx.in_working_dirs(dir) {
                *ctx.shell_cwd.lock().unwrap() = dir.clone();
            } else {
                *ctx.shell_cwd.lock().unwrap() = ctx.project_dir.clone();
                note = format!("\nShell cwd was reset to {}", ctx.project_dir.display());
            }
        }

        let max = ctx.max_output_chars;
        let stdout = truncate_middle(res.stdout.trim_end(), max);
        let stderr = truncate_middle(res.stderr.trim_end(), max);
        let mut text = stdout.clone();
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&stderr);
        }
        let structured = json!({
            "stdout": stdout, "stderr": stderr, "interrupted": res.interrupted || res.timed_out,
            "exitCode": res.code,
        });
        if res.interrupted {
            return ToolOutput::error(format!("{text}\n{INTERRUPTED}").trim_start().to_string())
                .with_structured(structured);
        }
        if res.timed_out {
            return ToolOutput::error(format!("{text}\nCommand timed out after {}ms", ms).trim_start().to_string())
                .with_structured(structured);
        }
        text.push_str(&note);
        match res.code {
            Some(0) => {
                if text.trim().is_empty() {
                    text = "(no output)".into();
                }
                ToolOutput::text(text).with_structured(structured)
            }
            code => {
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
                ToolOutput::error(format!("Exit code {code}\n{text}").trim_end().to_string())
                    .with_structured(structured)
            }
        }
    }
}

pub struct BashOutput;

#[async_trait::async_trait]
impl Tool for BashOutput {
    fn name(&self) -> &str {
        "BashOutput"
    }

    fn description(&self) -> String {
        "Read new output from a background shell started with Bash `run_in_background`. Returns only output \
         produced since the last read, plus the shell's status. `filter` keeps only lines matching a regex."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bash_id": {"type": "string", "description": "The ID of the background shell"},
                "filter": {"type": "string", "description": "Optional regex; only matching lines are returned"}
            },
            "required": ["bash_id"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let id = str_arg(&input, "bash_id");
        let Some(sh) = ctx.shells.get(id) else { return ToolOutput::error(format!("No shell found with ID: {id}")) };
        let (mut out, mut err) = sh.take_new_output();
        if let Some(f) = input.get("filter").and_then(Value::as_str).filter(|f| !f.is_empty()) {
            let re = match regex::Regex::new(f) {
                Ok(r) => r,
                Err(e) => return ToolOutput::error(format!("Invalid filter regex: {e}")),
            };
            let keep = |s: &str| s.lines().filter(|l| re.is_match(l)).collect::<Vec<_>>().join("\n");
            out = keep(&out);
            err = keep(&err);
        }
        let status = sh.status();
        let code = match status {
            ShellStatus::Completed(Some(c)) => format!("\n<exit_code>{c}</exit_code>"),
            _ => String::new(),
        };
        ToolOutput::text(format!(
            "<status>{}</status>{code}\n<stdout>\n{}\n</stdout>\n<stderr>\n{}\n</stderr>",
            status.label(),
            truncate_middle(out.trim_end(), ctx.max_output_chars),
            truncate_middle(err.trim_end(), ctx.max_output_chars)
        ))
    }
}

pub struct KillShell;

#[async_trait::async_trait]
impl Tool for KillShell {
    fn name(&self) -> &str {
        "KillShell"
    }

    fn description(&self) -> String {
        "Stop a background shell by its ID.".into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"shell_id": {"type": "string", "description": "The ID of the background shell to kill"}},
            "required": ["shell_id"],
            "additionalProperties": false
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let id = str_arg(&input, "shell_id");
        match ctx.shells.get(id) {
            Some(sh) => {
                sh.kill();
                ToolOutput::text(format!("Successfully killed shell: {id} ({})", sh.command))
            }
            None => ToolOutput::error(format!("No shell found with ID: {id}")),
        }
    }
}
