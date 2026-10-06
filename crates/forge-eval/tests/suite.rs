//! Every task in evals/tasks must fail on its starting files and pass after its reference solution.

#[tokio::test]
async fn all_tasks_are_valid() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tasks");
    let tasks = forge_eval::load_tasks(&dir, &[]).unwrap();
    assert!(tasks.len() >= 8);
    let scratch = tempfile::tempdir().unwrap();
    let mut bad = vec![];
    for t in &tasks {
        if let Err(e) = forge_eval::validate_task(t, scratch.path()).await {
            bad.push(format!("{}: {e}", t.spec.id));
        }
    }
    assert!(bad.is_empty(), "invalid tasks:\n{}", bad.join("\n"));
}
