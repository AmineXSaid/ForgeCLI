//! docs/CHECKLIST.md must stay runnable: every `forge ...` command in it
//! parses, every slash command exists, and the print-mode checks whose output
//! a mock can stand in for say what Forge really prints. A renamed flag or
//! command breaks the build, not the manual run.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use forge_api::MockTurn;
use forge_test_host::{command, forge_bin, MockApi};
use serde_json::json;

fn repo_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// The code in the checklist: each fenced line and each inline `span`.
fn code() -> Vec<String> {
    let doc = std::fs::read_to_string(repo_file("docs/CHECKLIST.md")).unwrap();
    let mut out = vec![];
    let mut prose = String::new();
    let mut fenced = false;
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(line.trim().to_string());
        } else {
            prose.push_str(line.trim());
            prose.push(' ');
        }
    }
    // Inline spans may wrap across lines; the prose was joined with spaces.
    out.extend(prose.split('`').skip(1).step_by(2).map(str::to_string));
    out
}

const OPERATORS: &[&str] = &["|", "||", "&&", ";", ">", ">>", "2>", "<", "{", "}"];

/// Every `forge ...` invocation in the checklist, as its arguments.
fn forge_commands() -> Vec<Vec<String>> {
    let mut out = vec![];
    for c in code() {
        let Some(words) = shlex::split(&c) else { continue };
        for seg in words.split(|w| OPERATORS.contains(&w.as_str())) {
            // `NO_COLOR=1 forge ...`: environment assignments first.
            let seg: Vec<&String> = seg.iter().skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
            if seg.first().map(|w| w.as_str()) == Some("forge") {
                out.push(seg[1..].iter().map(|s| s.to_string()).collect());
            }
        }
    }
    out
}

#[tokio::test]
async fn every_forge_command_in_the_checklist_parses() {
    let cmds = forge_commands();
    assert!(cmds.len() >= 20, "found only {} commands: {cmds:?}", cmds.len());
    let home = tempfile::tempdir().unwrap();
    for args in cmds {
        let mut all: Vec<&str> = args.iter().map(String::as_str).collect();
        // clap rejects an unknown flag or a bad value before it gets to --help.
        all.push("--help");
        let mut c = command(&forge_bin(), home.path(), home.path(), "http://127.0.0.1:9", &all);
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let out = tokio::time::timeout(Duration::from_secs(30), c.output()).await.unwrap().unwrap();
        assert!(
            out.status.success(),
            "`forge {}` doesn't parse:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn every_slash_command_in_the_checklist_exists() {
    let skills: Vec<String> = forge_agents::skills::bundled_skills(&mut vec![]).into_iter().map(|s| s.name).collect();
    let mut seen = vec![];
    for c in code() {
        for word in c.split(|ch: char| ch.is_whitespace() || ch == '"' || ch == '\'') {
            let Some(name) = word.strip_prefix('/') else { continue };
            // Paths (`/tmp/forge-check`, `/etc/...`) aren't commands.
            if name.is_empty() || name.contains(['/', '.']) || !name.starts_with(|c: char| c.is_ascii_lowercase()) {
                continue;
            }
            let name = name.trim_end_matches([',', ')', ':']);
            assert!(
                forge_core::commands::lookup(name).is_some() || skills.iter().any(|s| s == name),
                "/{name} in docs/CHECKLIST.md is neither a built-in nor a bundled skill"
            );
            seen.push(name.to_string());
        }
    }
    for want in ["goal", "usage", "btw", "diff", "context", "hooks", "agents", "keybindings", "code-review"] {
        assert!(seen.iter().any(|s| s == want), "the checklist covers /{want}");
    }
}

/// The scratch repository, made by the script the checklist uses.
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (repo, home) = (root.join("forge-check"), root.join("home"));
    std::fs::create_dir_all(&home).unwrap();
    let out = std::process::Command::new("sh").arg(repo_file("scripts/check-setup.sh")).arg(&repo).output().unwrap();
    assert!(out.status.success(), "check-setup.sh: {}", String::from_utf8_lossy(&out.stderr));
    // A second run refuses the non-empty directory; --force resets it.
    let again = std::process::Command::new("sh").arg(repo_file("scripts/check-setup.sh")).arg(&repo).output().unwrap();
    assert_eq!(again.status.code(), Some(1));
    let forced = std::process::Command::new("sh")
        .arg(repo_file("scripts/check-setup.sh"))
        .arg(&repo)
        .arg("--force")
        .output()
        .unwrap();
    assert!(forced.status.success());
    (dir, repo, home)
}

async fn forge(repo: &Path, home: &Path, api: &str, args: &[&str]) -> (i32, String, String) {
    let mut c = command(&forge_bin(), repo, home, api, args);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(60), c.output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn print_mode_checks_say_what_forge_prints() {
    let (_dir, repo, home) = setup();
    for f in ["calc.py", "test_calc.py", "notes.md", "secret.txt", ".forge/settings.json", "other-repo/main.py"] {
        assert!(repo.join(f).exists(), "the setup makes {f}");
    }
    assert!(std::fs::read_to_string(repo.join("calc.py")).unwrap().contains("a - b"), "the bug is in the working tree");

    // Check 4: an @ mention arrives with the prompt.
    let api = MockApi::start(vec![MockTurn::text("Monday.")]).await;
    let (code, out, err) = forge(&repo, &home, &api.url, &["-p", "what does @notes.md say about the CLI?"]).await;
    assert_eq!((code, out.trim()), (0, "Monday."), "{err}");
    assert!(api.requests()[0]["messages"].to_string().contains("the CLI on Monday"));

    // Check 5: a deny rule keeps the file out, and the prompt says so.
    let api = MockApi::start(vec![MockTurn::text("I can't read it.")]).await;
    let (code, _, err) =
        forge(&repo, &home, &api.url, &["-p", "what does @secret.txt say?", "--disallowedTools", "Read(secret.txt)"])
            .await;
    assert_eq!(code, 0, "{err}");
    let sent = api.requests()[0]["messages"].to_string();
    assert!(!sent.contains("4417"), "{sent}");
    assert!(sent.contains("secret.txt was not attached: blocked by a permission rule; use Read"), "{sent}");

    // Check 9: /status.
    let (code, out, _) = forge(&repo, &home, &api.url, &["-p", "/status"]).await;
    assert_eq!(code, 0);
    assert!(out.contains(&format!("Directory:      {}", repo.display())), "{out}");
    assert!(out.contains("Permissions:    default mode"), "{out}");

    // Check 10: /hooks add, the hook runs, /hooks lists and removes it.
    let log = home.join("forge-hook.log");
    let add = format!("/hooks add PreToolUse Bash echo hook-ran >> {}", log.display());
    let (code, out, err) = forge(&repo, &home, &api.url, &["-p", &add]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Added a PreToolUse hook for Bash"), "{out}");
    assert!(std::fs::read_to_string(repo.join(".forge/settings.local.json")).unwrap().contains("hook-ran"));
    let api = MockApi::start(vec![MockTurn::tool("Bash", json!({"command": "ls"})), MockTurn::text("Listed.")]).await;
    let (code, _, err) =
        forge(&repo, &home, &api.url, &["-p", "--permission-mode", "acceptEdits", "run ls with Bash"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(std::fs::read_to_string(&log).unwrap_or_default().contains("hook-ran"), "the hook ran");
    let (_, out, _) = forge(&repo, &home, &api.url, &["-p", "/hooks"]).await;
    assert!(out.contains("1. [Bash] echo hook-ran") && out.contains("local)"), "{out}");
    let (code, out, _) = forge(&repo, &home, &api.url, &["-p", "/hooks remove PreToolUse 1"]).await;
    assert_eq!(code, 0, "{out}");
    assert!(!std::fs::read_to_string(repo.join(".forge/settings.local.json")).unwrap().contains("hook-ran"));

    // Check 11: /agents create.
    let (code, out, err) = forge(
        &repo,
        &home,
        &api.url,
        &["-p", "/agents create test-writer --description 'Writes unit tests' --tools Read,Write,Bash --model sonnet"],
    )
    .await;
    assert_eq!(code, 0, "{out}{err}");
    let def = std::fs::read_to_string(repo.join(".forge/agents/test-writer.md")).unwrap();
    assert!(
        def.contains("name: test-writer") && def.contains("tools: Read, Write, Bash") && def.contains("model: sonnet")
    );
}
