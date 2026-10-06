//! Custom commands, skills, output styles and plugins, end to end (M5).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use forge_api::MockTurn;
use forge_test_host::{command, forge_bin, MockApi};
use serde_json::{json, Value};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    cwd: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, root, cwd, home }
}

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

async fn forge(e: &Env, api: &str, args: &[&str]) -> (i32, String, String) {
    let mut c = command(&forge_bin(), &e.cwd, &e.home, api, args);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(60), c.output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn first_user_text(req: &Value) -> String {
    req["messages"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_skills_styles_and_plugins() {
    let e = env();
    write(
        e.cwd.join(".forge/commands/review.md"),
        "---\ndescription: Review a file\nargument-hint: <file>\n---\nReview $ARGUMENTS carefully.",
    );
    write(
        e.cwd.join(".forge/skills/pdf/SKILL.md"),
        "---\nname: pdf\ndescription: Read PDF files\n---\nUse pdftotext -layout.",
    );
    write(e.cwd.join(".forge/settings.json"), r#"{"outputStyle": "explanatory"}"#);
    let plugin = e.root.join("plug");
    write(plugin.join("plugin.json"), r#"{"name": "kit", "description": "test kit"}"#);
    write(plugin.join("commands/hello.md"), "Say hello to $1.");
    write(plugin.join("agents/greeter.md"), "---\nname: greeter\ndescription: Greets people\n---\nGreet.");
    write(
        plugin.join("hooks/hooks.json"),
        r#"{"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo plugin-hook-ran"}]}]}}"#,
    );
    let plug = plugin.display().to_string();

    // A custom command expands before reaching the model; the output style is in the system prompt.
    let api = MockApi::start(vec![MockTurn::text("reviewed")]).await;
    let (code, out, err) = forge(
        &e,
        &api.url,
        &["-p", "--output-format", "stream-json", "--plugin-dir", &plug, "--", "/review src/main.rs"],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    let init: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    for name in ["review", "pdf", "kit:hello", "compact"] {
        assert!(init["slash_commands"].as_array().unwrap().iter().any(|c| c == name), "{name}: {init}");
    }
    let skills = init["skills"].as_array().unwrap();
    assert!(skills.contains(&json!("pdf")) && skills.contains(&json!("code-review")), "{init}");
    let reviews = init["slash_commands"].as_array().unwrap().iter().filter(|c| *c == "review").count();
    assert_eq!(reviews, 1, "the project's /review command shadows the bundled skill");
    assert_eq!(init["plugins"][0]["name"], "kit");
    assert_eq!(init["output_style"], "explanatory");
    assert!(init["agents"].as_array().unwrap().iter().any(|a| a == "greeter"));
    assert!(init["tools"].as_array().unwrap().iter().any(|t| t == "Skill"));
    let req = &api.requests()[0];
    let user = first_user_text(req);
    assert!(user.contains("Review src/main.rs carefully."), "{user}");
    assert!(user.contains("plugin-hook-ran"), "plugin hooks run: {user}");
    assert!(req["system"].to_string().contains("Insight"), "output style applied");
    let skill_tool = req["tools"].as_array().unwrap().iter().find(|t| t["name"] == "Skill").unwrap();
    assert!(skill_tool["description"].as_str().unwrap().contains("- pdf: Read PDF files"));

    // Plugin commands are namespaced; skills load as prompts; the model can load skills too.
    let api = MockApi::start(vec![MockTurn::tool("Skill", json!({"skill": "pdf"})), MockTurn::text("ok")]).await;
    let (code, _, err) = forge(&e, &api.url, &["-p", "--plugin-dir", &plug, "--", "/kit:hello Ada"]).await;
    assert_eq!(code, 0, "{err}");
    let reqs = api.requests();
    assert!(first_user_text(&reqs[0]).contains("Say hello to Ada."));
    assert!(reqs[1].to_string().contains("Use pdftotext -layout."), "the skill body comes back as the tool result");

    // Local commands answer without the model; unknown ones fail with exit 1.
    let api = MockApi::start(vec![]).await;
    let (code, out, _) = forge(&e, &api.url, &["-p", "/help"]).await;
    assert_eq!(code, 0);
    assert!(
        out.contains("/review <file> - Review a file (project)") && out.contains("/pdf - Read PDF files (skill)"),
        "{out}"
    );
    let (code, _, err) = forge(&e, &api.url, &["-p", "/nope"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("Unknown command: /nope"), "{err}");
    assert!(api.requests().is_empty(), "no model calls for local commands");
    // A path is a prompt, not a command.
    let api = MockApi::start(vec![MockTurn::text("it is a shell")]).await;
    let (code, out, _) = forge(&e, &api.url, &["-p", "/bin/sh: what is it?"]).await;
    assert_eq!((code, out.trim()), (0, "it is a shell"));
}
