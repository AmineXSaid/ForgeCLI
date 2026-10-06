//! Installation checks, shared by `forge doctor` and `/doctor`.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub ok: bool,
    pub name: &'static str,
    pub detail: String,
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
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
    out.push(Check {
        ok: true,
        name: "sandbox",
        detail: match forge_tools::sandbox::backend() {
            Some(b) => format!("{b:?} available (use --sandbox workspace-write)"),
            None => "unavailable: install bubblewrap (Linux) to confine shell commands".into(),
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
