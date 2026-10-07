//! Installation checks, shared by `forge doctor` and `/doctor`.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub ok: bool,
    pub name: &'static str,
    pub detail: String,
}

fn which(bin: &str) -> bool {
    forge_platform::process::find_program(bin).is_some()
}

/// The shell commands will run in, and whether it runs a test command.
fn shell_check(settings: &forge_config::LoadedSettings) -> Check {
    use forge_platform::shell::{Found, ShellKind};
    let shell = match crate::session_shell(settings) {
        Ok(s) => s,
        Err(m) => return Check { ok: false, name: "shell", detail: m.to_string() },
    };
    let failed = |why: String| Check {
        ok: false,
        name: "shell",
        detail: format!(
            "{} was found but did not run a test command ({why}). Reinstall it, or set FORGE_SHELL to another shell.",
            shell.describe()
        ),
    };
    let (mut cmd, script) = match shell.command(&shell.script("echo forge-shell-ok", None)) {
        Ok(c) => c,
        Err(e) => return failed(format!("could not start it: {e}")),
    };
    forge_platform::process::no_window(&mut cmd);
    cmd.stdin(std::process::Stdio::null());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cmd.output());
        drop(script);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Err(_) => return failed("no answer within 10 s".into()),
        Ok(Err(e)) => return failed(format!("could not start it: {e}")),
        Ok(Ok(out)) if !String::from_utf8_lossy(&out.stdout).contains("forge-shell-ok") => {
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err.lines().next().unwrap_or("").trim();
            return failed(format!("exit code {}: {first}", out.status.code().unwrap_or(-1)));
        }
        Ok(Ok(_)) => {}
    }
    let found = match shell.found {
        Found::EnvVar => "set by FORGE_SHELL",
        Found::SettingsEnv => "set by FORGE_SHELL in a settings file",
        Found::NextToGit => "found next to git.exe on PATH",
        Found::Standard => "standard location",
        Found::Path => "found on PATH",
    };
    let mut detail = format!("{}, {found}", shell.describe());
    if shell.kind == ShellKind::PowerShell && shell.os == forge_platform::shell::Os::Windows {
        detail.push_str(
            "; Git Bash wasn't found, so commands run in PowerShell. Install Git for Windows \
             (https://git-scm.com/downloads/win) to use bash",
        );
    }
    Check { ok: true, name: "shell", detail }
}

/// Checks that need no session: settings files, credentials, tools on PATH, directories.
pub fn checks(cwd: &Path) -> Vec<Check> {
    let mut out = vec![];
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(cwd));
    out.push(Check {
        ok: settings.errors.is_empty(),
        name: "settings",
        detail: format!(
            "{} layer(s) loaded{}",
            settings.layers.len(),
            if settings.errors.is_empty() {
                String::new()
            } else {
                format!("; invalid: {}", settings.errors.join("; "))
            }
        ),
    });
    let has = |k: &str| std::env::var(k).map(|v| !v.trim().is_empty()).unwrap_or(false);
    let openai = has("FORGE_OPENAI_BASE_URL");
    let creds = has("FORGE_API_KEY") || has("FORGE_AUTH_TOKEN") || settings.str("/apiKeyHelper").is_some();
    out.push(Check {
        ok: openai || creds,
        name: "credentials",
        detail: if openai {
            "OpenAI-compatible endpoint (FORGE_OPENAI_BASE_URL)".into()
        } else if creds {
            "found (FORGE_API_KEY, FORGE_AUTH_TOKEN or apiKeyHelper)".into()
        } else {
            "missing: set FORGE_API_KEY".into()
        },
    });
    out.push(Check {
        ok: true,
        name: "endpoint",
        detail: std::env::var("FORGE_BASE_URL").unwrap_or_else(|_| "default".into()),
    });
    let git = which("git");
    out.push(Check {
        ok: git,
        name: "git",
        detail: if git { "found".into() } else { "not found: git status and worktrees are unavailable".into() },
    });
    out.push(shell_check(&settings));
    out.push(Check {
        ok: true,
        name: "sandbox",
        detail: match forge_tools::sandbox::backend() {
            Some(b) => format!("{b:?} available (use --sandbox workspace-write)"),
            None => format!("unavailable: {}", forge_tools::sandbox::unavailable_reason()),
        },
    });
    out.push(Check { ok: true, name: "config dir", detail: forge_config::config_dir().display().to_string() });
    out.push(Check { ok: true, name: "state dir", detail: forge_config::state_dir().display().to_string() });
    out
}

/// Plain-text rendering: `ok  name  detail` / `FAIL name  detail`.
pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| format!("{} {:<12} {}", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail))
        .collect::<Vec<_>>()
        .join("\n")
}
