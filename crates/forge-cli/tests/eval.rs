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
        MockTurn::text("Fixed the even-length median."),
    ])
    .await;
    let rec = run_task(&task, 1, &options(&out, &root, &api)).await.unwrap();
    assert!(rec.passed, "check output: {}", rec.check_output);
    assert!(!rec.false_finish && !rec.agent_error);
    assert_eq!((rec.turns, rec.tool_calls, rec.tool_errors), (3, 2, 0));
    assert!(rec.cost_usd > 0.0);
    assert!(out.join("fix-median-1/transcript.jsonl").exists());

    // A run that claims success without changing anything.
    let out = root.join("lazy");
    let api = MockApi::start(vec![MockTurn::text("All tests pass now.")]).await;
    let rec = run_task(&task, 1, &options(&out, &root, &api)).await.unwrap();
    assert!(!rec.passed);
    assert!(rec.false_finish, "a success claim with a failing check is a false finish");
}
