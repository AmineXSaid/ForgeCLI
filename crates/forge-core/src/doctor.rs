//! Installation checks, shared by `forge doctor` and `/doctor`.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub ok: bool,
    /// Works, with something worth knowing (an unknown model's guessed limits).
    pub note: bool,
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
        Err(m) => return Check { note: false, ok: false, name: "shell", detail: m.to_string() },
    };
    let failed = |why: String| Check {
        note: false,
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
    Check { note: false, ok: true, name: "shell", detail }
}

/// The provider, its URL and its key, from the same resolver sessions use.
/// No network and no helper command: `forge doctor --probe` does those.
pub fn endpoint_checks(settings: &forge_config::LoadedSettings, env: &dyn Fn(&str) -> Option<String>) -> Vec<Check> {
    use forge_api::auth::Backend;
    let r = crate::endpoint::resolve(settings, env);
    let set = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
    let provider = match (r.backend(), r.url_from) {
        (Backend::OpenAi, _) => {
            let mut d = format!("OpenAI-compatible endpoint, chosen by {}", r.url_origin());
            if set("FORGE_BASE_URL") {
                d.push_str("; FORGE_BASE_URL is set but not used");
            }
            d
        }
        (Backend::Messages, _) => {
            "Messages API (set FORGE_OPENAI_BASE_URL to use an OpenAI-compatible endpoint)".to_string()
        }
    };
    let url_problem = r.problems.iter().find(|p| p.contains("is not a valid http"));
    let suffix = r.warnings.iter().find(|w| w.contains("ends in /chat/completions"));
    let ignored: Vec<&String> =
        r.warnings.iter().filter(|w| w.starts_with("Ignored ") && !w.contains("Helper")).collect();
    let (url_ok, mut url_detail) = match (url_problem, suffix, &r.url) {
        (Some(p), _, _) => (false, p.clone()),
        (None, Some(w), _) => (false, w.clone()),
        (None, None, Some(u)) => (true, format!("{u} (from {})", r.url_origin())),
        (None, None, None) => (true, "default (the Messages API's own host)".to_string()),
    };
    if r.warnings.iter().any(|w| w.contains("would travel unencrypted")) {
        url_detail.push_str("; warning: http:// sends the key unencrypted");
    }
    for w in &ignored {
        url_detail.push_str(&format!("; {}", w.split(". Put it in").next().unwrap_or(w).to_lowercase()));
    }
    let quoted = |var: &str| {
        if r.quoted.contains(&var) {
            "; it was wrapped in quotes, which Forge removes"
        } else {
            ""
        }
    };
    let key_problem = r.problems.iter().find(|p| p.contains("can't be used"));
    let (cred_ok, cred) = if let Some(p) = key_problem {
        (false, p.clone())
    } else {
        match r.backend() {
            Backend::OpenAi => match (&r.key, &r.helper) {
                (Some(k), _) => (
                    true,
                    format!(
                        "FORGE_OPENAI_API_KEY ({}), sent as Authorization: Bearer{}",
                        crate::endpoint::mask(k),
                        quoted("FORGE_OPENAI_API_KEY")
                    ),
                ),
                (None, Some((_, setting, file))) => (
                    true,
                    format!(
                        "{setting}{} (not run; `forge doctor --probe` runs it)",
                        file.as_ref().map(|f| format!(" in {}", f.display())).unwrap_or_default()
                    ),
                ),
                (None, None) if r.local => (true, "none (a server on this machine or network often needs none)".into()),
                (None, None) if r.others.is_empty() => {
                    (true, "none: requests carry no key. Set FORGE_OPENAI_API_KEY if this endpoint needs one".into())
                }
                (None, None) => (
                    false,
                    format!(
                        "none for this endpoint: FORGE_OPENAI_API_KEY is not set. {} {}, but {} never sent to an \
                         OpenAI-compatible endpoint; set FORGE_OPENAI_API_KEY to this endpoint's key",
                        crate::endpoint::and_list(&r.others),
                        if r.others.len() == 1 { "is set" } else { "are set" },
                        if r.others.len() == 1 { "it is" } else { "they are" },
                    ),
                ),
            },
            Backend::Messages => {
                let mut parts = vec![];
                if let Some(k) = &r.key {
                    parts.push(format!(
                        "FORGE_API_KEY ({}), sent as x-api-key{}",
                        crate::endpoint::mask(k),
                        quoted("FORGE_API_KEY")
                    ));
                }
                if let Some(t) = &r.token {
                    parts.push(format!(
                        "FORGE_AUTH_TOKEN ({}), sent as Authorization: Bearer{}",
                        crate::endpoint::mask(t),
                        quoted("FORGE_AUTH_TOKEN")
                    ));
                }
                if parts.is_empty() {
                    if let Some((_, setting, file)) = &r.helper {
                        parts.push(format!(
                            "{setting}{} (not run; `forge doctor --probe` runs it)",
                            file.as_ref().map(|f| format!(" in {}", f.display())).unwrap_or_default()
                        ));
                    }
                }
                if parts.is_empty() {
                    let mut d = "missing: set FORGE_API_KEY (or FORGE_AUTH_TOKEN for a gateway that expects a bearer \
                                 token)"
                        .to_string();
                    if set("FORGE_OPENAI_API_KEY") {
                        d.push_str(". FORGE_OPENAI_API_KEY is set, but it is only used with FORGE_OPENAI_BASE_URL.");
                    }
                    (false, d)
                } else {
                    (true, parts.join(" and "))
                }
            }
        }
    };
    vec![
        Check { note: false, ok: true, name: "provider", detail: provider },
        Check { note: false, ok: url_ok, name: "endpoint", detail: url_detail },
        Check { note: false, ok: cred_ok, name: "credentials", detail: cred },
    ]
}

/// `forge doctor --probe`: one request to the endpoint (its model list), with
/// the key helper run if there is one. 10 seconds at most.
pub async fn probe(cwd: &Path) -> Check {
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(cwd));
    let provider = match crate::make_provider(&settings, &[]) {
        Ok(p) => p,
        Err(e) => return Check { note: false, ok: false, name: "probe", detail: format!("not run: {e}") },
    };
    let url = provider.base_url().unwrap_or_else(|| "the endpoint".into());
    let listed = tokio::time::timeout(std::time::Duration::from_secs(11), provider.probe()).await;
    match listed {
        Err(_) => {
            Check { note: false, ok: false, name: "probe", detail: format!("no answer from {url} within 10 seconds") }
        }
        Ok(None) => Check {
            note: false,
            ok: true,
            name: "probe",
            detail: "skipped: this provider has no model list to ask; the key is checked by the first request".into(),
        },
        Ok(Some(Ok(n))) => Check {
            note: false,
            ok: true,
            name: "probe",
            detail: format!("{url} answered the model list: the key works ({n} models listed)"),
        },
        Ok(Some(Err(forge_api::ApiError::Http { status: 404, .. }))) => Check {
            note: false,
            ok: true,
            name: "probe",
            detail: format!(
                "{url} answered 404 for the model list: the endpoint is reachable but lists no models, so the key \
                 wasn't checked"
            ),
        },
        Ok(Some(Err(e))) => Check { note: false, ok: false, name: "probe", detail: e.describe() },
    }
}

/// Checks that need no session: settings files, credentials, tools on PATH, directories.
pub fn checks(cwd: &Path) -> Vec<Check> {
    let mut out = vec![];
    let settings = forge_config::load_settings(&forge_config::SettingsOptions::new(cwd));
    out.push(Check {
        note: false,
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
    out.extend(endpoint_checks(&settings, &|k| std::env::var(k).ok()));
    let git = which("git");
    out.push(Check {
        note: false,
        ok: git,
        name: "git",
        detail: if git { "found".into() } else { "not found: git status and worktrees are unavailable".into() },
    });
    out.push(shell_check(&settings));
    out.push(Check {
        note: false,
        ok: true,
        name: "sandbox",
        detail: match forge_tools::sandbox::backend() {
            Some(b) => format!("{b:?} available (use --sandbox workspace-write)"),
            None => format!("unavailable: {}", forge_tools::sandbox::unavailable_reason()),
        },
    });
    out.push(Check {
        note: false,
        ok: true,
        name: "config dir",
        detail: forge_config::short_path(&forge_config::config_dir()),
    });
    out.push(Check {
        note: false,
        ok: true,
        name: "state dir",
        detail: forge_config::short_path(&forge_config::state_dir()),
    });
    out
}

impl Check {
    /// `ok`, `note` or `FAIL`, padded to one width.
    pub fn mark(&self) -> &'static str {
        match (self.ok, self.note) {
            (false, _) => "FAIL",
            (true, true) => "note",
            (true, false) => "ok  ",
        }
    }
}

/// Plain-text rendering: `ok   name  detail` / `FAIL name  detail`.
pub fn render(checks: &[Check]) -> String {
    checks.iter().map(|c| format!("{} {:<12} {}", c.mark(), c.name, c.detail)).collect::<Vec<_>>().join("\n")
}
