//! The public CLI contract (docs/CLI.md), checked at the process boundary:
//! stdout, stderr and exit status separately, with a clean environment.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use forge_api::MockTurn;
use forge_test_host::{forge_bin, MockApi};
use serde_json::Value;

struct Env {
    _dir: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

fn env_with_cwd(name: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cwd = root.join(name);
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
}

fn env() -> Env {
    env_with_cwd("project")
}

/// A clean environment: no credentials, no network endpoint, isolated home.
fn forge(e: &Env, args: &[&str]) -> Command {
    let mut c = Command::new(forge_bin());
    c.args(args)
        .current_dir(&e.cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &e.home)
        .env("FORGE_HOME", e.home.join(".forge"))
        .stdin(Stdio::null());
    c
}

fn run<C: std::borrow::BorrowMut<Command>>(mut c: C) -> (i32, String, String) {
    let o: Output = c.borrow_mut().output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn with_api(mut c: Command, api: &MockApi) -> Command {
    c.env("FORGE_BASE_URL", &api.url)
        .env("FORGE_API_KEY", "sk-test-key-0123456789abcdef")
        .env("FORGE_MAX_RETRIES", "0");
    c
}

#[test]
fn help_and_version_need_no_credentials_or_network() {
    let e = env();
    for args in [
        &["--help"][..],
        &["-h"],
        &["config", "--help"],
        &["completion", "--help"],
        &["doctor", "--help"],
        &["help", "config"],
    ] {
        let (code, out, err) = run(forge(&e, args));
        assert_eq!(code, 0, "{args:?}: {err}");
        assert!(out.contains("Usage"), "{args:?}: help goes to stdout");
        assert!(err.is_empty(), "{args:?}: {err}");
    }
    let (code, out, _) = run(forge(&e, &["--version"]));
    assert_eq!(code, 0);
    assert!(out.trim().ends_with("(ForgeCLI)"));
}

#[test]
fn usage_errors_exit_2_on_stderr() {
    let e = env();
    for args in [
        &["--bogus"][..],
        &["-p", "--output-format", "yaml", "x"],
        &["-p", "--max-turns", "0", "x"],
        &["-p", "--effort", "extreme", "x"],
        &["-p", "--session-id", "not-a-uuid", "x"],
        &["-p", "--add-dir", "/does/not/exist", "x"],
        &["config", "nope"],
    ] {
        let (code, out, err) = run(forge(&e, args));
        assert_eq!(code, 2, "{args:?}: {err}");
        assert!(out.is_empty(), "{args:?}: stdout must stay empty, got {out}");
        assert!(!err.is_empty());
    }
}

#[test]
fn missing_credentials_exit_3_with_a_next_step() {
    let e = env();
    let (code, out, err) = run(forge(&e, &["-p", "hello"]));
    assert_eq!(code, 3);
    assert!(out.is_empty());
    assert!(err.contains("FORGE_API_KEY") && err.contains("forge doctor"), "{err}");
    // In JSON mode stdout holds exactly one error document.
    let (code, out, _) = run(forge(&e, &["-p", "--output-format", "json", "hello"]));
    assert_eq!(code, 3);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["exit_code"], 3);
}

#[test]
fn interactive_mode_needs_a_terminal_and_input() {
    let e = env();
    let (code, out, err) = run(forge(&e, &[]));
    assert_eq!(code, 2);
    assert!(out.is_empty());
    assert!(err.contains("needs a terminal") && err.contains("forge -p"), "{err}");
    let (code, _, err) = run(forge(&e, &["--no-input"]));
    assert_eq!(code, 2);
    assert!(err.contains("--no-input"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_results_on_stdout_diagnostics_on_stderr() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("the answer")]).await;
    let (code, out, err) = run(with_api(forge(&e, &["-p", "q"]), &api));
    assert_eq!((code, out.as_str()), (0, "the answer\n"), "{err}");
    let api = MockApi::start(vec![MockTurn::http_error(500, "api_error")]).await;
    let (code, out, err) = run(with_api(forge(&e, &["-p", "q"]), &api));
    assert_eq!(code, 1);
    assert!(out.is_empty(), "errors never go to stdout: {out}");
    assert_eq!(err.matches("API Error").count(), 1, "the error is reported once: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompts_starting_with_a_dash_and_paths_with_spaces() {
    let e = env_with_cwd("my project dir");
    let api = MockApi::start(vec![MockTurn::text("ok")]).await;
    let (code, out, err) = run(with_api(forge(&e, &["-p", "--", "-v means verbose?"]), &api));
    assert_eq!((code, out.trim()), (0, "ok"), "{err}");
    let sent = api.requests()[0]["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"].clone();
    assert_eq!(sent, "-v means verbose?");
}

#[test]
fn color_follows_no_color_and_flag() {
    let e = env();
    let (_, _, err) = run(forge(&e, &["-p", "x"]).env("NO_COLOR", "1"));
    assert!(!err.contains('\x1b'), "{err:?}");
    let (_, _, err) = run(forge(&e, &["-p", "x", "--color", "always"]));
    assert!(err.contains('\x1b'), "{err:?}");
    let (_, _, err) = run(forge(&e, &["-p", "x", "--color", "never"]).env("FORCE_COLOR", "1"));
    assert!(!err.contains('\x1b'), "--color never wins over FORCE_COLOR");
}

#[test]
fn config_redacts_secrets_and_shows_origin_and_paths() {
    let e = env();
    std::fs::create_dir_all(e.cwd.join(".forge")).unwrap();
    std::fs::write(
        e.cwd.join(".forge/settings.json"),
        r#"{"model": "sonnet", "env": {"FORGE_API_KEY": "sk-secret-value-1234567890", "DEBUG": "1"}}"#,
    )
    .unwrap();
    let (code, out, _) = run(forge(&e, &["config", "list"]));
    assert_eq!(code, 0);
    assert!(!out.contains("sk-secret-value"), "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["env"]["FORGE_API_KEY"], "<redacted>");
    assert_eq!(v["env"]["DEBUG"], "1");
    let (_, out, _) = run(forge(&e, &["config", "list", "--origin"]));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["model"]["source"], "projectSettings");
    assert!(v["model"]["file"].as_str().unwrap().ends_with(".forge/settings.json"));
    let (code, out, _) = run(forge(&e, &["config", "get", "env.FORGE_API_KEY"]));
    assert_eq!((code, out.trim()), (0, "<redacted>"));
    let (code, _, err) = run(forge(&e, &["config", "get", "nope.nothing"]));
    assert_eq!(code, 1);
    assert!(err.contains("not set"));
    let (code, out, _) = run(forge(&e, &["config", "paths"]));
    assert_eq!(code, 0);
    assert!(out.contains(&e.home.join(".forge").display().to_string()), "{out}");
    assert!(out.contains("state") && out.contains("cache"));
}

#[test]
fn completions_are_generated_from_the_command_definitions() {
    let e = env();
    for shell in ["bash", "zsh", "fish"] {
        let (code, out, _) = run(forge(&e, &["completion", shell]));
        assert_eq!(code, 0);
        assert!(out.contains("forge") && out.contains("output-format"), "{shell}");
    }
}

#[test]
fn closed_stdout_pipe_does_not_panic() {
    let e = env();
    let mut c = forge(&e, &["completion", "bash"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_interrupts_cleanly_and_exits_130() {
    let e = env();
    let api =
        MockApi::start(vec![MockTurn::text("slow answer that keeps going").with_delay(Duration::from_millis(400))])
            .await;
    let mut c = with_api(forge(&e, &["-p", "--output-format", "stream-json", "--verbose", "go"]), &api);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    unsafe { libc_kill(child.id() as i32) };
    let out = tokio::task::spawn_blocking(move || child.wait_with_output()).await.unwrap().unwrap();
    assert_eq!(out.status.code(), Some(130));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "result", "a result is still written: {stdout}");
    assert_eq!(last["stop_reason"], "interrupted");
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

unsafe fn libc_kill(pid: i32) {
    kill(pid, 2);
}

#[test]
fn unreachable_endpoint_names_the_next_step() {
    let e = env();
    let (code, out, err) = run(forge(&e, &["-p", "q"])
        .env("FORGE_BASE_URL", "http://127.0.0.1:9")
        .env("FORGE_API_KEY", "x")
        .env("FORGE_MAX_RETRIES", "0"));
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert!(err.contains("could not connect") && err.contains("FORGE_BASE_URL"), "{err}");
}

#[test]
fn doctor_reports_missing_credentials_with_status_3() {
    let e = env();
    let (code, out, _) = run(forge(&e, &["doctor"]));
    assert_eq!(code, 3);
    assert!(out.contains("FAIL") && out.contains("FORGE_API_KEY"), "{out}");
    let (code, _, _) = run(forge(&e, &["doctor"]).env("FORGE_API_KEY", "x"));
    assert_eq!(code, 0);
    let _ = Path::new("/");
}
