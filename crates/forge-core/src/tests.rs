use super::*;
use forge_api::{MockProvider, MockTurn};
use forge_engine::{DenyPrompter, NullSink};
use forge_types::MessageContent;

fn opts(dir: &Path, provider: Arc<MockProvider>) -> LaunchOptions {
    LaunchOptions {
        cwd: dir.join("proj"),
        provider: Some(provider),
        store_root: Some(dir.join("store")),
        setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
        ..Default::default()
    }
}

fn setup() -> (tempfile::TempDir, Arc<MockProvider>) {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("proj/.forge")).unwrap();
    (d, Arc::new(MockProvider::new(vec![])))
}

#[tokio::test]
async fn builds_from_settings_and_flags() {
    let (d, p) = setup();
    std::fs::write(
        d.path().join("proj/.forge/settings.json"),
        r#"{"model":"sonnet","permissions":{"defaultMode":"acceptEdits","deny":["Bash(rm *)"]}}"#,
    )
    .unwrap();
    std::fs::write(d.path().join("proj/FORGE.md"), "Always answer in French.").unwrap();
    let mut o = opts(d.path(), p.clone());
    o.disallowed_tools = vec!["NotebookEdit WebFetch(domain:x.com)".into()];
    o.tools = Some(vec!["Bash,Read,Edit,NotebookEdit".into()]);
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_eq!(s.init.model, "claude-sonnet-5-5");
    assert_eq!(s.init.permission_mode, "acceptEdits");
    assert_eq!(s.init.tools, vec!["Bash", "Read", "Edit"]);
    let mut e = s.engine;
    p.push(MockTurn::text("Bonjour"));
    e.submit(MessageContent::Text("hi".into())).await;
    let first = &p.requests()[0].messages[0];
    assert!(first.content.iter().any(|b| b.as_text().map(|t| t.contains("Always answer in French.")).unwrap_or(false)));
    let perm = e.handle().permissions.read().unwrap().clone();
    assert_eq!(perm.rules.deny.len(), 2);
}

#[tokio::test]
async fn continue_and_fork() {
    let (d, p) = setup();
    p.push(MockTurn::text("first answer"));
    let s = build_session(opts(d.path(), p.clone()), Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    let first_id = s.session_id.clone();
    let mut e = s.engine;
    e.submit(MessageContent::Text("first".into())).await;

    let mut o = opts(d.path(), p.clone());
    o.resume = Resume::Latest;
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_eq!(s.session_id, first_id);
    assert_eq!(s.engine.state.messages.len(), 2);

    let mut o = opts(d.path(), p.clone());
    o.resume = Resume::Id(first_id.clone());
    o.fork_session = true;
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_ne!(s.session_id, first_id);
    assert_eq!(s.engine.state.messages.len(), 2);
    let mut e = s.engine;
    p.push(MockTurn::text("forked answer"));
    e.submit(MessageContent::Text("more".into())).await;
    assert_eq!(p.requests().last().unwrap().messages.len(), 3);

    let mut o = opts(d.path(), p.clone());
    o.resume = Resume::Id("../../etc/passwd".into());
    assert!(build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).is_err());
}

#[tokio::test]
async fn skip_permissions_flag_beats_settings_mode() {
    let (d, p) = setup();
    let mut o = opts(d.path(), p);
    o.dangerously_skip_permissions = true;
    o.settings = Some(r#"{"permissions":{"defaultMode":"plan"}}"#.into());
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_eq!(s.init.permission_mode, "bypassPermissions");
}

#[tokio::test]
async fn sub_agents_are_offered_and_configurable() {
    let (d, p) = setup();
    std::fs::create_dir_all(d.path().join("proj/.forge/agents")).unwrap();
    std::fs::write(
        d.path().join("proj/.forge/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Reviews changes\ntools: Read, Grep\n---\nYou review.",
    )
    .unwrap();
    let s = build_session(opts(d.path(), p.clone()), Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert!(s.init.tools.contains(&"Task".to_string()));
    for a in ["general-purpose", "Explore", "Plan", "reviewer"] {
        assert!(s.init.agents.contains(&a.to_string()), "{a} missing from {:?}", s.init.agents);
    }
    let mut o = opts(d.path(), p.clone());
    o.agent = Some("reviewer".into());
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_eq!(s.init.tools, vec!["Grep", "Read"], "the session gets the agent's tools");
    assert!(s.engine.system()[0].text.contains("You review."));
    let mut o = opts(d.path(), p);
    o.tools = Some(vec!["Read".into()]);
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert_eq!(s.init.tools, vec!["Read"], "--tools without Task leaves sub-agents out");
}

#[test]
fn autocompact_values() {
    assert_eq!(parse_autocompact("auto").unwrap(), None);
    assert_eq!(parse_autocompact("200k").unwrap(), Some(200_000));
    assert_eq!(parse_autocompact("1m").unwrap(), Some(1_000_000));
    assert_eq!(parse_autocompact("150000").unwrap(), Some(150_000));
    assert!(parse_autocompact("50k").is_err());
    assert!(parse_autocompact("lots").is_err());
}

#[test]
fn permission_updates_persist_to_named_file() {
    let d = tempfile::tempdir().unwrap();
    persist_permission_update(
        d.path(),
        &json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "git *"}], "behavior": "allow", "destination": "localSettings"}),
    );
    persist_permission_update(
        d.path(),
        &json!({"type": "setMode", "mode": "acceptEdits", "destination": "projectSettings"}),
    );
    persist_permission_update(d.path(), &json!({"type": "setMode", "mode": "plan", "destination": "session"}));
    let local: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join(".forge/settings.local.json")).unwrap()).unwrap();
    assert_eq!(local["permissions"]["allow"], json!(["Bash(git *)"]));
    let project: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join(".forge/settings.json")).unwrap()).unwrap();
    assert_eq!(project["permissions"]["defaultMode"], "acceptEdits");
}

#[tokio::test]
async fn resume_restores_additional_directories() {
    let (d, p) = setup();
    let extra = d.path().join("shared");
    std::fs::create_dir_all(&extra).unwrap();
    let mut o = opts(d.path(), p.clone());
    o.add_dirs = vec![extra.clone()];
    p.push(MockTurn::text("ok"));
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    let mut e = s.engine;
    e.submit(MessageContent::Text("hi".into())).await;

    let mut o = opts(d.path(), p.clone());
    o.resume = Resume::Latest;
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    let canonical = extra.canonicalize().unwrap();
    let perm = s.engine.handle().permissions.read().unwrap().clone();
    assert!(perm.working_dirs.iter().any(|w| w == &canonical || w == &extra), "{:?}", perm.working_dirs);
    assert!(s.engine.tool_ctx().in_working_dirs(&extra.join("file.txt")));
}

#[tokio::test]
async fn c10_worktree_flag_runs_the_session_in_a_new_worktree() {
    let (d, p) = setup();
    let proj = d.path().join("proj").canonicalize().unwrap();
    let mut o = opts(d.path(), p.clone());
    o.worktree = Some("feature-x".into());
    assert!(
        matches!(build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)), Err(CoreError::Config(e)) if e.contains("git repository"))
    );

    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["commit", "-q", "--allow-empty", "-m", "first"],
    ] {
        assert!(std::process::Command::new("git").args(args).current_dir(&proj).status().unwrap().success());
    }
    let mut o = opts(d.path(), p.clone());
    o.worktree = Some("feature-x".into());
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    let wt = proj.join(".forge/worktrees/feature-x");
    assert_eq!(s.engine.tool_ctx().project_dir, wt.canonicalize().unwrap());
    assert_eq!(forge_git::current_branch(&wt).as_deref(), Some("forge/feature-x"));
    assert!(std::fs::read_to_string(proj.join(".git/info/exclude")).unwrap().contains(".forge/worktrees/"));
    assert!(!forge_git::is_dirty(&proj), "the main checkout stays clean");
    // The same name again reuses it; a generated name comes from the session id.
    let mut o = opts(d.path(), p.clone());
    o.worktree = Some("feature-x".into());
    assert!(build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).is_ok());
    let mut o = opts(d.path(), p);
    o.worktree = Some(String::new());
    o.session_id = Some("12345678-0000-4000-8000-000000000000".into());
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    assert!(s.engine.tool_ctx().project_dir.ends_with(".forge/worktrees/session-12345678"));
}
