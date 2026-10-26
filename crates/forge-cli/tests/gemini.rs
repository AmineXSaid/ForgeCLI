//! End-to-end: the real `forge` binary on the native Gemini wire, against an
//! endpoint as strict as Google's about thought signatures.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use forge_test_host::{forge_bin, MockGemini};
use serde_json::{json, Value};

async fn forge(cwd: &Path, home: &Path, api: &MockGemini, args: &[&str]) -> (i32, String, String) {
    let mut c = tokio::process::Command::new(forge_bin());
    c.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("FORGE_HOME", home.join(".forge"))
        .env("FORGE_GEMINI_BASE_URL", &api.url)
        .env("FORGE_GEMINI_API_KEY", "test-key")
        .env("FORGE_MAX_RETRIES", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(60), c.output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

fn read(path: &Path) -> Value {
    json!({"name": "Read", "args": {"file_path": path.to_string_lossy()}})
}

#[tokio::test]
async fn a_gemini_3_tool_loop_keeps_its_signatures_through_steps_and_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    for (f, text) in [("a.txt", "alpha"), ("b.txt", "beta"), ("c.txt", "gamma")] {
        std::fs::write(cwd.join(f), text).unwrap();
    }
    let api = MockGemini::start(vec![
        // Two calls at once: only the first is signed, as Gemini sends them.
        vec![
            json!({"text": "Reading both.", "thought": true, "thoughtSignature": "THINK-1"}),
            json!({"functionCall": read(&cwd.join("a.txt")), "thoughtSignature": "SIG-A"}),
            json!({"functionCall": read(&cwd.join("b.txt"))}),
        ],
        // A second step in the same turn.
        vec![json!({"functionCall": read(&cwd.join("c.txt")), "thoughtSignature": "SIG-C"})],
        vec![json!({"text": "They say alpha, beta and gamma."})],
        // The next session continues with every signed call in its history.
        vec![json!({"text": "Still alpha, beta and gamma."})],
    ])
    .await;
    let args = [
        "-p",
        "What do a.txt, b.txt and c.txt say?",
        "--model",
        "gemini-3-flash-preview",
        "--dangerously-skip-permissions",
    ];
    let (code, out, err) = forge(&cwd, &home, &api, &args).await;
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}\nviolations: {:?}", api.violations());
    assert!(out.contains("alpha, beta and gamma"), "{out}");

    let again = ["-p", "--continue", "And now?", "--model", "gemini-3-flash-preview"];
    let (code, out, err) = forge(&cwd, &home, &api, &again).await;
    assert_eq!(code, 0, "stdout: {out}\nstderr: {err}\nviolations: {:?}", api.violations());
    assert!(out.contains("Still alpha"), "{out}");
    assert!(api.violations().is_empty(), "{:?}", api.violations());

    // The resumed request replayed every signature from the transcript.
    let last = api.requests().last().unwrap().to_string();
    for sig in ["THINK-1", "SIG-A", "SIG-C"] {
        assert!(last.contains(sig), "{sig} was lost on resume");
    }
}

#[tokio::test]
async fn a_lost_signature_is_what_the_strict_endpoint_refuses() {
    let issued = vec![("Read{\"file_path\":\"a\"}".to_string(), "SIG".to_string())];
    let body = json!({"contents": [
        {"role": "user", "parts": [{"text": "go"}]},
        {"role": "model", "parts": [{"functionCall": {"name": "Read", "args": {"file_path": "a"}}}]},
        {"role": "user", "parts": [{"functionResponse": {"name": "Read", "response": {"output": "x"}}}]}
    ]});
    let why = forge_test_host::gemini_violation("/v1beta/models/gemini-3-flash:streamGenerateContent", &body, &issued);
    assert!(why.is_some_and(|w| w.contains("thought_signature")));
}
