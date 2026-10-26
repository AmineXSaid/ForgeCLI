//! End-to-end: what each permission mode asks before, with the real `forge`
//! binary over stream-json and `--permission-prompt-tool stdio`, as Forge's
//! desktop drives it. The model reads and edits a file, then deletes another.

use std::path::PathBuf;

use forge_api::MockTurn;
use forge_test_host::{command, forge_bin, Host, MockApi};
use serde_json::{json, Value};

struct Env {
    _dir: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(cwd.join("notes.txt"), "original").unwrap();
    std::fs::write(cwd.join("junk.txt"), "junk").unwrap();
    Env { _dir: dir, cwd, home }
}

/// Run one turn in `mode`: an edit, then `rm junk.txt`. Every question
/// asked is allowed; returns the tools asked about, in order.
async fn asked_in(e: &Env, mode: &str, extra: &[&str]) -> Vec<String> {
    let notes = e.cwd.join("notes.txt");
    // A file is read before it is edited (ForgeCLI refuses an unread edit).
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": notes})),
        MockTurn::tool("Edit", json!({"file_path": notes, "old_string": "original", "new_string": "changed"})),
        MockTurn::tool("Bash", json!({"command": "rm junk.txt"})),
        MockTurn::text("done"),
    ])
    .await;
    let mut args = vec![
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        mode,
    ];
    args.extend_from_slice(extra);
    let mut h = Host::spawn(command(&forge_bin(), &e.cwd, &e.home, &api.url, &args));
    h.send_user("tidy up").await;
    let mut asked = vec![];
    loop {
        let v: Value = h.until(|v| v["type"] == "result" || v["type"] == "control_request").await;
        if v["type"] == "result" {
            break;
        }
        if v["request"]["subtype"] == "can_use_tool" {
            asked.push(v["request"]["tool_name"].as_str().unwrap_or_default().to_string());
            let id = v["request_id"].as_str().unwrap().to_string();
            let input = v["request"]["input"].clone();
            h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                "response": {"behavior": "allow", "updatedInput": input}}}))
                .await;
        }
    }
    h.close_stdin();
    h.wait().await;
    asked
}

#[tokio::test]
async fn manual_asks_before_the_edit_and_the_command() {
    let e = env();
    assert_eq!(asked_in(&e, "default", &[]).await, ["Edit", "Bash"]);
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
    assert!(!e.cwd.join("junk.txt").exists());
}

#[tokio::test]
async fn edit_automatically_edits_without_asking_but_asks_before_a_deletion() {
    let e = env();
    assert_eq!(asked_in(&e, "acceptEdits", &[]).await, ["Bash"]);
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
}

#[tokio::test]
async fn plan_changes_nothing() {
    let e = env();
    let asked = asked_in(&e, "plan", &[]).await;
    assert!(!asked.contains(&"Edit".to_string()), "{asked:?}");
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "original", "no edit in Plan");
    assert!(e.cwd.join("junk.txt").exists(), "no deletion in Plan");
}

#[tokio::test]
async fn bypass_asks_nothing_once_allowed() {
    let e = env();
    let asked = asked_in(&e, "bypassPermissions", &["--allow-dangerously-skip-permissions"]).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
    assert!(!e.cwd.join("junk.txt").exists());
}
