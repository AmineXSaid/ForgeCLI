//! End-to-end: the real `forge` binary against a scripted Messages API.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

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
    let cwd = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
}

async fn run(e: &Env, api: &MockApi, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = command(&forge_bin(), &e.cwd, &e.home, &api.url, args);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).await.unwrap();
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

fn stream_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"];
    v.extend_from_slice(extra);
    v
}

#[tokio::test]
async fn print_text_and_request_shape() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Hello from the mock")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "say hello"], None).await;
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out.trim(), "Hello from the mock");
    let req = &api.requests()[0];
    assert_eq!(req["model"], "claude-opus-5-5");
    assert_eq!(req["stream"], true);
    assert!(req["system"][0]["text"].as_str().unwrap().starts_with("You are Forge"));
    assert!(req["tools"].as_array().unwrap().iter().any(|t| t["name"] == "Edit"));
    assert_eq!(req["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"], "say hello");
}

#[tokio::test]
async fn print_json_and_piped_stdin() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("42")]).await;
    let (code, out, _) =
        run(&e, &api, &["-p", "--output-format", "json", "what is this?"], Some("some piped data")).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "result");
    assert_eq!(v["subtype"], "success");
    assert_eq!(v["result"], "42");
    assert_eq!(v["num_turns"], 1);
    assert!(v["session_id"].as_str().unwrap().len() == 36);
    let sent = api.requests()[0]["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(sent, "some piped data\n\nwhat is this?");
}

#[tokio::test]
async fn print_attaches_at_mentions() {
    let e = env();
    std::fs::write(e.cwd.join("notes.md"), "Ship the parser on Friday.").unwrap();
    let api = MockApi::start(vec![MockTurn::text("Friday.")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "summarize @notes.md"], None).await;
    assert_eq!((code, out.trim()), (0, "Friday."), "stderr: {err}");
    let content = api.requests()[0]["messages"][0]["content"].as_array().unwrap().clone();
    let texts: Vec<&str> = content.iter().filter_map(|b| b["text"].as_str()).collect();
    assert!(texts.contains(&"summarize @notes.md"), "{texts:?}");
    let note = texts.last().unwrap();
    assert!(note.starts_with("<system-reminder>") && note.contains("Ship the parser on Friday."), "{note}");
}

#[tokio::test]
async fn print_errors() {
    let e = env();
    let api = MockApi::start(vec![]).await;
    let (code, out, err) = run(&e, &api, &["-p"], Some("")).await;
    assert_eq!(code, 2, "usage error");
    assert!(err.contains("no prompt") && err.contains("hint:"), "{err}");
    assert!(out.is_empty(), "nothing on stdout in text mode");
    let (code, _, err) = run(&e, &api, &["-p", "--permission-mode", "yolo", "x"], None).await;
    assert_eq!(code, 2);
    assert!(err.contains("invalid --permission-mode"), "{err}");
    let api = MockApi::start(vec![MockTurn::http_error(401, "authentication_error")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "--output-format", "json", "x"], None).await;
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "result");
    assert_eq!(v["subtype"], "error_during_execution");
    assert_eq!(v["exit_code"], 1);
    let msg = v["errors"][0].as_str().unwrap();
    assert!(msg.contains("401") && msg.contains("FORGE_API_KEY"), "the error names the next step: {msg}");
    assert!(!err.contains("{"), "stderr carries no JSON: {err}");
}

/// Replace values that change between runs.
fn normalize(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, val) in m.iter_mut() {
                    match k.as_str() {
                        "uuid" | "session_id" | "id" | "tool_use_id" | "cwd" | "duration_ms" | "duration_api_ms"
                        | "pid" | "total_cost_usd" | "costUSD" | "forge_version" => {
                            if !val.is_null() {
                                *val = json!(format!("<{k}>"));
                            }
                        }
                        _ => walk(val),
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[tokio::test]
async fn stream_json_golden_basic() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Hi!")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "--output-format", "stream-json", "--verbose", "hello"], None).await;
    assert_eq!(code, 0, "{err}");
    let lines: Vec<Value> = out.lines().map(|l| normalize(serde_json::from_str(l).unwrap())).collect();
    let types: Vec<String> = lines
        .iter()
        .map(|l| format!("{}/{}", l["type"].as_str().unwrap(), l["subtype"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(types, vec!["system/init", "assistant/", "result/success"]);
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixtures/stream-json-basic.jsonl");
    let rendered: String = lines.iter().map(|l| serde_json::to_string(l).unwrap() + "\n").collect();
    if std::env::var_os("FORGE_UPDATE_GOLDEN").is_some() || !golden_path.exists() {
        std::fs::write(&golden_path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap();
    assert_eq!(rendered, golden, "stream-json shape changed; rerun with FORGE_UPDATE_GOLDEN=1 if intended");
}

#[tokio::test]
async fn host_permission_prompt_allow_and_deny() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("Bash", json!({"command": "touch created.txt", "description": "make a file"})),
        MockTurn::text("made it"),
        MockTurn::tool("Bash", json!({"command": "touch second.txt"})),
        MockTurn::text("ok, not made"),
    ])
    .await;
    let mut h = Host::spawn(command(
        &forge_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &stream_args(&["--permission-prompt-tool", "stdio"]),
    ));
    h.send(json!({"type": "control_request", "request_id": "init_1", "request": {"subtype": "initialize"}})).await;
    let init_resp = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "init_1").await;
    assert_eq!(init_resp["response"]["subtype"], "success");
    assert!(init_resp["response"]["response"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["value"] == "claude-opus-5-5"));

    h.send_user("create a file").await;
    let req = h.until(|v| v["type"] == "control_request").await;
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    assert_eq!(req["request"]["tool_name"], "Bash");
    assert_eq!(req["request"]["input"]["command"], "touch created.txt");
    assert_eq!(req["request"]["permission_suggestions"][0]["type"], "addRules");
    let id = req["request_id"].as_str().unwrap().to_string();
    h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "updatedInput": {"command": "touch created.txt"}}}})).await;
    let tool_result = h.until(|v| v["type"] == "user").await;
    assert_eq!(tool_result["message"]["content"][0]["type"], "tool_result");
    assert!(tool_result["tool_use_result"].is_object());
    let r = h.until_type("result").await;
    assert_eq!(r["result"], "made it");
    assert!(e.cwd.join("created.txt").exists());

    h.send_user("another").await;
    let req = h.until(|v| v["type"] == "control_request").await;
    let id = req["request_id"].as_str().unwrap().to_string();
    h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "deny", "message": "not today"}}})).await;
    let tr = h.until(|v| v["type"] == "user").await;
    assert_eq!(tr["message"]["content"][0]["is_error"], true);
    assert_eq!(tr["message"]["content"][0]["content"], "not today");
    let r = h.until_type("result").await;
    assert_eq!(r["permission_denials"][0]["tool_name"], "Bash");
    assert!(!e.cwd.join("second.txt").exists());
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn host_controls_mode_model_and_interrupt() {
    let e = env();
    let api =
        MockApi::start(
            vec![MockTurn::text("a slow answer streaming in pieces").with_delay(Duration::from_millis(300))],
        )
        .await;
    let mut h =
        Host::spawn(command(&forge_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--include-partial-messages"])));
    h.send(json!({"type": "control_request", "request_id": "m1", "request": {"subtype": "set_permission_mode", "mode": "plan"}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["subtype"], "success");
    h.send(json!({"type": "control_request", "request_id": "m2", "request": {"subtype": "set_permission_mode", "mode": "nope"}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["subtype"], "error");
    // Bypassing permissions takes --allow-dangerously-skip-permissions at launch.
    h.send(json!({"type": "control_request", "request_id": "m4", "request": {"subtype": "set_permission_mode", "mode": "bypassPermissions"}})).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m4").await;
    assert_eq!(r["response"]["subtype"], "error");
    assert!(r["response"]["error"].as_str().unwrap().contains("--allow-dangerously-skip-permissions"));
    h.send(
        json!({"type": "control_request", "request_id": "m3", "request": {"subtype": "set_model", "model": "sonnet"}}),
    )
    .await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m3").await;

    h.send_user("talk slowly").await;
    h.until_type("stream_event").await;
    h.send(json!({"type": "control_request", "request_id": "int", "request": {"subtype": "interrupt"}})).await;
    let r = h.until_type("result").await;
    assert_eq!(r["stop_reason"], "interrupted");
    assert_eq!(r["subtype"], "success");
    assert_eq!(api.requests()[0]["model"], "claude-sonnet-5-5");
    // The system prompt names the model the host switched to.
    let system = api.requests()[0]["system"].to_string();
    assert!(system.contains("Sonnet 5.5") && !system.contains("Opus 5.5"), "{system}");
    let unknown = json!({"type": "control_request", "request_id": "u", "request": {"subtype": "teleport"}});
    h.send(unknown).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "u").await;
    assert_eq!(r["response"]["subtype"], "error");
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn replay_uuid_drives_rewind_files() {
    let e = env();
    let f = e.cwd.join("notes.txt");
    std::fs::write(&f, "original").unwrap();
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": f})),
        MockTurn::tool("Edit", json!({"file_path": f, "old_string": "original", "new_string": "changed"})),
        MockTurn::text("edited"),
    ])
    .await;
    let mut h = Host::spawn(command(
        &forge_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &stream_args(&["--replay-user-messages", "--permission-mode", "acceptEdits"]),
    ));
    h.send_user("edit the notes").await;
    let replay = h.until(|v| v["type"] == "user" && v["isReplay"] == true).await;
    let uuid = replay["uuid"].as_str().unwrap().to_string();
    h.until_type("result").await;
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "changed");
    h.send(json!({"type": "control_request", "request_id": "rw1", "request": {"subtype": "rewind_files", "user_message_id": uuid, "dry_run": true}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["response"]["canRewind"], true);
    assert_eq!(r["response"]["response"]["filesChanged"][0], f.display().to_string());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "changed", "dry run leaves the file");
    h.send(json!({"type": "control_request", "request_id": "rw2", "request": {"subtype": "rewind_files", "user_message_id": uuid}})).await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "rw2").await;
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    h.wait().await;
}

#[tokio::test]
async fn headless_denies_and_reports() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("Write", json!({"file_path": e.cwd.join("x.txt"), "content": "x"})),
        MockTurn::text("could not write"),
    ])
    .await;
    let (code, out, _) = run(&e, &api, &["-p", "--output-format", "json", "write x"], None).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"][0]["tool_name"], "Write");
    assert!(!e.cwd.join("x.txt").exists());
    let api = MockApi::start(vec![
        MockTurn::tool("Write", json!({"file_path": e.cwd.join("y.txt"), "content": "y"})),
        MockTurn::text("wrote"),
    ])
    .await;
    let (code, _, _) = run(&e, &api, &["-p", "write y", "--allowedTools", "Write"], None).await;
    assert_eq!(code, 0);
    assert!(e.cwd.join("y.txt").exists());
}

#[tokio::test]
async fn max_turns_and_continue() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("LS", json!({"path": e.cwd})),
        MockTurn::tool("LS", json!({"path": e.cwd})),
    ])
    .await;
    let (code, out, _) = run(&e, &api, &["-p", "--output-format", "json", "--max-turns", "1", "look"], None).await;
    assert_eq!(code, 4, "limits exit with 4");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["subtype"], "error_max_turns");

    let api = MockApi::start(vec![MockTurn::text("remembered"), MockTurn::text("second")]).await;
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "remember 7"], None).await;
    let first: Value = serde_json::from_str(out.trim()).unwrap();
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "-c", "what number?"], None).await;
    let second: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(first["session_id"], second["session_id"]);
    let msgs = api.requests()[1]["messages"].as_array().unwrap().len();
    assert_eq!(msgs, 3, "continued session sends prior turns");
}

#[tokio::test]
async fn version_and_help() {
    let out = std::process::Command::new(forge_bin()).arg("--version").output().unwrap();
    let v = String::from_utf8_lossy(&out.stdout);
    assert!(v.trim().ends_with("(ForgeCLI)"), "{v}");
    let out = std::process::Command::new(forge_bin()).arg("--help").output().unwrap();
    let h = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--print",
        "--output-format",
        "--permission-mode",
        "--allowedTools",
        "--resume",
        "--max-budget-usd",
        "--json-schema",
    ] {
        assert!(h.contains(flag), "missing {flag}");
    }
}

#[tokio::test]
async fn retries_transient_errors() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(529, "overloaded_error"), MockTurn::text("after retry")]).await;
    let mut c = command(&forge_bin(), &e.cwd, &e.home, &api.url, &["-p", "hi"]);
    c.env("FORGE_MAX_RETRIES", "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(30), c.output()).await.unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "after retry");
    assert_eq!(api.requests().len(), 2);
}

#[tokio::test]
async fn host_sets_thinking_tokens() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("one"), MockTurn::text("two")]).await;
    let mut h = Host::spawn(command(&forge_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--model", "sonnet"])));
    h.send_user("default thinking").await;
    h.until_type("result").await;
    h.send(json!({"type": "control_request", "request_id": "t0", "request": {"subtype": "set_max_thinking_tokens", "max_thinking_tokens": 0}})).await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "t0").await;
    h.send_user("no thinking").await;
    h.until_type("result").await;
    let reqs = api.requests();
    assert_eq!(reqs[0]["thinking"]["type"], "adaptive");
    assert_eq!(reqs[1]["thinking"]["type"], "between_tools");
    h.wait().await;
}

#[tokio::test]
async fn sandbox_lets_headless_runs_build_without_prompts() {
    if forge_tools_sandbox_missing() {
        eprintln!("skipped: no sandbox backend on this machine");
        return;
    }
    let e = env();
    let calls = || {
        vec![
            MockTurn::tool("Bash", json!({"command": "mkdir -p build && echo ok > build/out.txt"})),
            MockTurn::text("built"),
        ]
    };
    // Without the sandbox, print mode denies the command (contract C1).
    let api = MockApi::start(calls()).await;
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "build it"], None).await;
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"][0]["tool_name"], "Bash");
    assert!(!e.cwd.join("build/out.txt").exists());
    // With it, the command runs confined, with no prompt.
    let api = MockApi::start(calls()).await;
    let (code, out, _) =
        run(&e, &api, &["-p", "--output-format", "json", "--sandbox", "workspace-write", "build it"], None).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"], json!([]));
    assert!(e.cwd.join("build/out.txt").exists());
}

fn forge_tools_sandbox_missing() -> bool {
    !std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--dev", "/dev", "--unshare-net", "true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
