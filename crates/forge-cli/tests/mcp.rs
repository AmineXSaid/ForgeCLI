//! `forge mcp ...` and MCP tools in a session, end to end. Forge's own
//! `forge mcp serve` is the server under test.

use std::path::PathBuf;
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
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
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

fn self_server() -> Value {
    json!({"command": forge_bin(), "args": ["mcp", "serve"]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_list_get_remove() {
    let e = env();
    let bin = forge_bin().display().to_string();
    let (code, out, err) = forge(&e, "http://unused", &["mcp", "add", "self", "--", &bin, "mcp", "serve"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Added MCP server self (stdio) in local scope"), "{out}");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(e.cwd.join(".forge/settings.local.json")).unwrap()).unwrap();
    assert_eq!(saved["mcpServers"]["self"]["args"], json!(["mcp", "serve"]));

    let (code, out, _) = forge(&e, "http://unused", &["mcp", "list"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("self: ") && out.contains("connected, ") && out.contains(" tools"), "{out}");

    let (code, out, _) = forge(
        &e,
        "http://unused",
        &[
            "mcp",
            "add",
            "-t",
            "http",
            "-s",
            "user",
            "-H",
            "Authorization: Bearer s3cret",
            "web",
            "http://127.0.0.1:9/mcp",
        ],
    )
    .await;
    assert_eq!(code, 0, "{out}");
    let (_, out, _) = forge(&e, "http://unused", &["mcp", "get", "web"]).await;
    assert!(out.contains("\"Authorization\":\"***\"") && !out.contains("s3cret"), "secrets hidden: {out}");
    assert!(out.contains("Status: failed"), "{out}");
    let (code, _, _) = forge(&e, "http://unused", &["mcp", "list"]).await;
    assert_eq!(code, 1, "a failing server makes `list` fail");

    let (_, out, _) = forge(&e, "http://unused", &["mcp", "get", "self"]).await;
    assert!(out.contains("mcp__self__Read"), "{out}");
    let (code, out, _) = forge(&e, "http://unused", &["mcp", "remove", "web"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("from user"), "{out}");
    let (code, _, err) = forge(&e, "http://unused", &["mcp", "remove", "web"]).await;
    assert_eq!(code, 1, "{err}");
    let (code, _, err) = forge(&e, "http://unused", &["mcp", "add", "bad name", "x"]).await;
    assert_eq!(code, 2, "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_uses_mcp_tools_and_reports_status() {
    let e = env();
    std::fs::write(e.cwd.join("notes.txt"), "the answer is 42\n").unwrap();
    let api = MockApi::start(vec![
        MockTurn::tool("mcp__self__Read", json!({"file_path": e.cwd.join("notes.txt")})),
        MockTurn::text("It says 42."),
    ])
    .await;
    let config = json!({"mcpServers": {"self": self_server()}}).to_string();
    let mut host = Host::spawn(command(
        &forge_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &[
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--allowedTools",
            "mcp__self",
            "--mcp-config",
            &config,
        ],
    ));
    let init = host.until(|v| v["subtype"] == "init").await;
    assert_eq!(init["mcp_servers"], json!([{"name": "self", "status": "connected"}]));
    assert!(init["tools"].as_array().unwrap().iter().any(|t| t == "mcp__self__Read"));

    host.send(json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "mcp_status"}})).await;
    let status = host.until(|v| v["type"] == "control_response").await;
    let servers = &status["response"]["response"]["mcpServers"];
    assert_eq!(servers[0]["name"], "self");
    assert_eq!(servers[0]["serverInfo"]["name"], "forge");

    host.send_user("what does notes.txt say?").await;
    let result = host.until_type("result").await;
    assert_eq!(result["result"], "It says 42.");
    let tool_result = host
        .transcript
        .iter()
        .find(|v| v["type"] == "user" && v["message"]["content"][0]["type"] == "tool_result")
        .cloned()
        .unwrap();
    assert!(tool_result.to_string().contains("the answer is 42"), "{tool_result}");
    let tool_names: Vec<String> = api.requests()[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(tool_names.contains(&"mcp__self__Bash".to_string()));
    let (code, _) = host.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_servers_wait_for_approval() {
    let e = env();
    std::fs::write(e.cwd.join(".mcp.json"), json!({"mcpServers": {"repo": self_server()}}).to_string()).unwrap();
    // The checked-in project settings cannot approve the repository's own servers.
    std::fs::create_dir_all(e.cwd.join(".forge")).unwrap();
    std::fs::write(e.cwd.join(".forge/settings.json"), r#"{"enableAllProjectMcpServers": true}"#).unwrap();
    let api = MockApi::start(vec![MockTurn::text("ok"), MockTurn::text("ok")]).await;
    let (code, out, err) = forge(&e, &api.url, &["-p", "--output-format", "stream-json", "hi"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("MCP server repo (.mcp.json) needs approval"), "{err}");
    let init: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(init["mcp_servers"], json!([{"name": "repo", "status": "disabled"}]));
    assert!(!init["tools"].to_string().contains("mcp__repo"));

    let (code, out, _) = forge(&e, &api.url, &["mcp", "approve", "repo"]).await;
    assert_eq!(code, 0, "{out}");
    let (code, out, err) = forge(&e, &api.url, &["-p", "--output-format", "stream-json", "hi"]).await;
    assert_eq!(code, 0, "{err}");
    let init: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(init["mcp_servers"], json!([{"name": "repo", "status": "connected"}]));

    // --strict-mcp-config ignores .mcp.json and settings.
    let api = MockApi::start(vec![MockTurn::text("ok")]).await;
    let (_, out, _) = forge(&e, &api.url, &["-p", "--output-format", "stream-json", "--strict-mcp-config", "hi"]).await;
    let init: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(init["mcp_servers"], json!([]));
}
