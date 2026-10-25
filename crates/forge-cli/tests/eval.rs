//! forge-eval end to end: the real forge binary on a real task, with a scripted model.

use std::path::{Path, PathBuf};

use forge_api::MockTurn;
use forge_eval::{load_tasks, run_task, RunOptions};
use forge_test_host::{forge_bin, MockApi};
use serde_json::json;

fn tasks_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tasks")
}

fn options(out: &Path, home: &Path, api: &MockApi) -> RunOptions {
    RunOptions {
        forge: forge_bin(),
        args: vec!["--dangerously-skip-permissions".into()],
        env: vec![
            ("FORGE_BASE_URL".into(), api.url.clone()),
            ("FORGE_API_KEY".into(), "test".into()),
            ("FORGE_HOME".into(), home.join(".forge").display().to_string()),
            ("HOME".into(), home.display().to_string()),
            ("FORGE_MAX_RETRIES".into(), "0".into()),
        ],
        label: "test".into(),
        out_dir: out.to_path_buf(),
        repeat: 1,
        jobs: 1,
    }
}

#[tokio::test]
async fn eval_measures_a_pass_and_a_false_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let task = load_tasks(&tasks_dir(), &["fix-median".into()]).unwrap().remove(0);

    // A run that fixes the bug.
    let out = root.join("good");
    let ws = out.join("fix-median-1/workspace");
    let file = ws.join("stats.py");
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": file})),
        MockTurn::tool(
            "Edit",
            json!({
                "file_path": file,
                "old_string": "    if len(ordered) % 2 == 0:\n        return ordered[mid]",
                "new_string": "    if len(ordered) % 2 == 0:\n        return (ordered[mid - 1] + ordered[mid]) / 2",
            }),
        ),
        MockTurn::tool("Bash", json!({"command": "python3 -m unittest -q test_stats"})),
        MockTurn::text("Fixed the even-length median; `python3 -m unittest test_stats` passes."),
    ])
    .await;
    let rec = run_task(&task, 1, &options(&out, &root, &api)).await.unwrap();
    assert!(rec.passed, "check output: {}", rec.check_output);
    assert!(!rec.false_finish && !rec.agent_error);
    assert_eq!((rec.turns, rec.tool_calls, rec.tool_errors, rec.verify_reminders), (4, 3, 0, 0));
    assert!(rec.cost_usd > 0.0);
    assert!(out.join("fix-median-1/transcript.jsonl").exists());

    // A run that claims success without changing anything.
    let out = root.join("lazy");
    let api = MockApi::start(vec![MockTurn::text("All tests pass now.")]).await;
    let rec = run_task(&task, 1, &options(&out, &root, &api)).await.unwrap();
    assert!(!rec.passed);
    assert!(rec.false_finish, "a success claim with a failing check is a false finish");
}

/// A skill written for the reference CLI (`.claude/skills`, named arguments,
/// `$ARGUMENTS[N]`, `$0`, `${CLAUDE_SKILL_DIR}`) runs from `/name args` with
/// everything filled in, and the task passes.
#[tokio::test]
async fn a_reference_cli_skill_runs_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let task = load_tasks(&tasks_dir(), &["reference-skill".into()]).unwrap().remove(0);
    let out = root.join("run");
    let ws = out.join("reference-skill-1/workspace");
    let script = ws.join(".claude/skills/version-notes/scripts/notes.py");
    let api = MockApi::start(vec![
        MockTurn::tool(
            "Bash",
            json!({"command": format!("python3 {} changes.txt 1.4.0 > RELEASE-1.4.0.md", script.display())}),
        ),
        MockTurn::text("Wrote RELEASE-1.4.0.md."),
    ])
    .await;
    let rec = run_task(&task, 1, &options(&out, &root, &api)).await.unwrap();
    let reqs = api.requests();
    assert!(rec.passed, "check output: {}\nrequests: {}", rec.check_output, serde_json::to_string(&reqs).unwrap());
    let first = serde_json::to_string(&reqs[0]["messages"]).unwrap();
    for want in [
        "Write the release notes for version 1.4.0.".to_string(),
        format!("python3 {} changes.txt 1.4.0", script.display()),
        "RELEASE-1.4.0.md".to_string(),
        // Its `!` line ran before the model saw it.
        "The changes, as of now:\\nfix: a crash when the config is empty".to_string(),
    ] {
        assert!(first.contains(&want), "missing {want:?} in {first}");
    }
    assert!(!first.contains("CLAUDE_SKILL_DIR") && !first.contains("$ARGUMENTS"), "{first}");
}
