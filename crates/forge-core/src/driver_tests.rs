//! Slash commands through the driver, against a mock provider (contract C17).

use std::path::Path;
use std::sync::Arc;

use forge_api::{MockProvider, MockTurn};
use forge_config::SettingSource;
use forge_engine::{DenyPrompter, NullSink, TurnResult};
use forge_types::MessageContent;
use serde_json::Value;

use crate::commands::Surface;
use crate::{build_session, Driver, Flow, LaunchOptions};

struct T {
    _dir: tempfile::TempDir,
    proj: std::path::PathBuf,
    p: Arc<MockProvider>,
    d: Driver,
}

fn driver_with(f: impl FnOnce(&Path, &mut LaunchOptions)) -> T {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(proj.join(".forge")).unwrap();
    let proj = proj.canonicalize().unwrap();
    let p = Arc::new(MockProvider::new(vec![]));
    let mut o = LaunchOptions {
        cwd: proj.clone(),
        provider: Some(p.clone()),
        store_root: Some(dir.path().join("store")),
        // Never touch the real user settings from a unit test.
        setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
        ..Default::default()
    };
    f(dir.path(), &mut o);
    let rebuild = o.clone();
    let s = build_session(o, Arc::new(NullSink), Arc::new(DenyPrompter)).unwrap();
    let mut d = Driver::new(s, Surface::Print, None);
    d.set_rebuild(rebuild, Arc::new(NullSink), Arc::new(DenyPrompter));
    T { _dir: dir, proj, p, d }
}

fn driver() -> T {
    driver_with(|_, _| {})
}

/// Every result one input produced.
async fn run_all(d: &mut Driver, text: &str) -> Vec<TurnResult> {
    let mut out = vec![];
    let mut report = |r: &TurnResult| out.push(r.clone());
    assert_eq!(d.input(MessageContent::Text(text.into()), &mut report).await, Flow::Continue, "unexpected exit");
    out
}

async fn run(d: &mut Driver, text: &str) -> TurnResult {
    run_all(d, text).await.pop().expect("a result")
}

async fn local(d: &mut Driver, text: &str) -> String {
    let r = run(d, text).await;
    assert_eq!(r.num_turns, 0, "{text} should answer locally");
    assert!(!r.is_error, "{text} failed: {:?}", r.result);
    r.result.unwrap_or_default()
}

async fn fails(d: &mut Driver, text: &str) -> String {
    let r = run(d, text).await;
    assert!(r.is_error, "{text} should fail, got {:?}", r.result);
    r.result.unwrap_or_default()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn model_effort_and_fast_reach_the_request() {
    let mut t = driver();
    let d = &mut t.d;
    let list = local(d, "/model").await;
    assert!(list.contains("Current model: Opus 5.5") && list.contains("claude-haiku-4-5"), "{list}");

    let out = local(d, "/effort low").await;
    assert_eq!(out, "Set effort to low. (This session only.)");
    let out = local(d, "/fast on").await;
    assert!(out.starts_with("Fast mode on"), "{out}");
    t.p.push(MockTurn::text("a"));
    run(d, "hi").await;
    let req = &t.p.requests()[0];
    assert_eq!(req.output_config.as_ref().unwrap()["effort"], "low");
    assert_eq!(req.speed.as_deref(), Some("fast"));
    assert_eq!(req.betas, vec![forge_engine::FAST_MODE_BETA.to_string()]);

    // Haiku has neither effort nor fast mode: both stop applying, and both say so.
    let out = local(d, "/model haiku").await;
    assert!(out.contains("Set model to Haiku 4.5 (claude-haiku-4-5)."), "{out}");
    assert!(out.contains("Effort low isn't available") && out.contains("Fast mode isn't available"), "{out}");
    assert!(d.engine.system()[0].text.contains("Haiku 4.5"), "the environment names the new model");
    assert_eq!(d.info.init.model, "claude-haiku-4-5");
    t.p.push(MockTurn::text("b"));
    run(d, "again").await;
    let req = &t.p.requests()[1];
    assert_eq!(req.model, "claude-haiku-4-5");
    assert!(req.speed.is_none() && req.betas.is_empty() && req.output_config.is_none());
    assert!(fails(d, "/fast on").await.contains("isn't available for Haiku 4.5"));
    assert!(fails(d, "/effort high").await.contains("doesn't support effort"));

    local(d, "/model opus").await;
    assert!(fails(d, "/effort extreme").await.contains("Choose one of: low, medium, high, xhigh, max"));
    assert_eq!(local(d, "/effort max").await, "Set effort to max. (max lasts for this session only.)");
    assert!(local(d, "/effort").await.starts_with("Effort: max"));
    local(d, "/effort auto").await;
    assert!(local(d, "/effort status").await.contains("auto (Opus 5.5's default: medium)"));
    assert_eq!(local(d, "/fast off").await, "Fast mode off. (This session only.)");
}

#[tokio::test]
async fn output_style_survives_an_sdk_system_prompt() {
    let mut t = driver();
    let d = &mut t.d;
    let list = local(d, "/output-style").await;
    assert!(list.contains("* default") && list.contains("explanatory"), "{list}");
    local(d, "/output-style explanatory").await;
    assert_eq!(d.info.init.output_style, "explanatory");
    let sys = d.engine.system()[0].text.clone();
    assert!(sys.contains("# Output style"), "{sys}");
    // An initialize request rebuilds the prompt but keeps the style.
    d.set_system_prompt(None, Some("Host instructions.".into()));
    let sys = d.engine.system()[0].text.clone();
    assert!(sys.contains("# Output style") && sys.ends_with("Host instructions."), "{sys}");
    assert!(fails(d, "/output-style nope").await.contains("No output style named \"nope\""));
}

#[tokio::test]
async fn config_validates_then_writes_and_applies() {
    let mut t = driver();
    let local_file = t.proj.join(".forge/settings.local.json");
    let d = &mut t.d;
    assert!(local(d, "/config --help").await.contains("autoCompactWindow"));
    assert!(fails(d, "/config bogus=1").await.contains("Unknown setting \"bogus\""));
    assert!(fails(d, "/config effortLevel=extreme").await.contains("expected one of"));
    assert!(fails(d, "/config autoCompactEnabled=maybe").await.contains("expected true or false"));
    assert!(fails(d, "/config permissions.defaultMode=bypassPermissions").await.contains("expected one of"));
    assert!(fails(d, "/config outputStyle=nope").await.contains("No output style"));
    assert!(!local_file.exists(), "nothing is written when a value is invalid");

    let out = local(d, "/config outputStyle=learning autoCompactWindow=200k effortLevel=high --scope local").await;
    assert!(out.contains("outputStyle = \"learning\". Saved in local project settings"), "{out}");
    let v = read_json(&local_file);
    assert_eq!((v["outputStyle"].as_str(), v["autoCompactWindow"].as_u64()), (Some("learning"), Some(200_000)));
    assert_eq!(v["effortLevel"], "high");
    assert_eq!(d.info.init.output_style, "learning");
    assert_eq!(d.engine.cfg.autocompact_window, Some(200_000));
    assert_eq!(d.engine.handle().runtime().effort.as_deref(), Some("high"));
    let shown = local(d, "/config").await;
    assert!(shown.contains("outputStyle              \"learning\"  [localSettings]"), "{shown}");

    // key= removes it again.
    local(d, "/config effortLevel= --scope local").await;
    assert!(read_json(&local_file).get("effortLevel").is_none());
    assert_eq!(d.engine.handle().runtime().effort, None);
    local(d, "/config permissions.defaultMode=plan --scope local").await;
    assert_eq!(d.engine.handle().permissions.read().unwrap().mode.as_str(), "plan");
}

#[tokio::test]
async fn config_reports_a_layer_that_overrides_it() {
    let mut t = driver_with(|dir, _| {
        let local = dir.join("proj/.forge/settings.local.json");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(local, r#"{"model": "haiku"}"#).unwrap();
    });
    let out = local(&mut t.d, "/config model=sonnet --scope project").await;
    assert!(out.contains("localSettings also sets model and takes precedence"), "{out}");
    // It still applies to this session.
    assert_eq!(t.d.engine.handle().model(), "claude-sonnet-5-5");
    assert_eq!(read_json(&t.proj.join(".forge/settings.json"))["model"], "sonnet");
}

#[tokio::test]
async fn permissions_add_list_and_remove() {
    let mut t = driver();
    let local_file = t.proj.join(".forge/settings.local.json");
    let d = &mut t.d;
    assert!(fails(d, "/permissions add maybe Read").await.contains("Usage"));
    assert!(fails(d, "/permissions add allow Bash(").await.contains("Invalid rule"));
    let out = local(d, "/permissions add allow Bash(git status:*)").await;
    assert!(out.starts_with("Added allow rule Bash(git status:*). Saved in local project settings"), "{out}");
    assert_eq!(read_json(&local_file)["permissions"]["allow"][0], "Bash(git status:*)");
    local(d, "/permissions add deny WebFetch --scope session").await;
    let list = local(d, "/permissions").await;
    assert!(list.contains("Bash(git status:*)  [localSettings]") && list.contains("WebFetch  [session]"), "{list}");
    let perms = d.engine.handle().permissions.read().unwrap().clone();
    assert_eq!((perms.rules.allow.len(), perms.rules.deny.len()), (1, 1));

    let out = local(d, "/permissions remove Bash(git status:*)").await;
    assert!(out.contains("allow (session)") && out.contains("settings.local.json"), "{out}");
    assert_eq!(read_json(&local_file)["permissions"]["allow"], serde_json::json!([]));
    assert!(d.engine.handle().permissions.read().unwrap().rules.allow.is_empty());
    assert!(fails(d, "/permissions remove Bash(git status:*)").await.contains("No rule"));
}

#[tokio::test]
async fn add_dir_widens_access_and_tells_the_model() {
    let mut t = driver();
    let other = t._dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let other = other.canonicalize().unwrap();
    let d = &mut t.d;
    assert!(fails(d, "/add-dir").await.contains("Usage"));
    assert!(fails(d, "/add-dir /no/such/dir").await.contains("/no/such/dir"));
    let out = local(d, &format!("/add-dir {} --save", other.display())).await;
    assert!(out.contains("Added working directory") && out.contains("Saved in local project settings"), "{out}");
    assert!(d.engine.tool_ctx().in_working_dirs(&other.join("x.txt")));
    assert!(d.engine.handle().permissions.read().unwrap().in_working_dirs(&other.join("x.txt")));
    let saved = read_json(&t.proj.join(".forge/settings.local.json"));
    assert_eq!(saved["permissions"]["additionalDirectories"][0], other.display().to_string());
    assert!(local(d, &format!("/add-dir {}", other.display())).await.contains("already inside"));
    t.p.push(MockTurn::text("ok"));
    run(d, "go").await;
    let first = serde_json::to_string(&t.p.requests()[0].messages[0]).unwrap();
    assert!(first.contains("The user added a working directory"), "{first}");
}

#[tokio::test]
async fn autocompact_and_sandbox() {
    let mut t = driver();
    let d = &mut t.d;
    assert!(local(d, "/autocompact").await.starts_with("Auto-compact is on"));
    local(d, "/autocompact 300k").await;
    assert_eq!(d.engine.cfg.autocompact_window, Some(300_000));
    assert!(local(d, "/autocompact").await.contains("of a 300k window (set)"));
    local(d, "/autocompact auto").await;
    assert_eq!(d.engine.cfg.autocompact_window, None);
    local(d, "/autocompact off").await;
    assert!(!d.engine.cfg.auto_compact);
    assert!(fails(d, "/autocompact 5").await.contains("Usage"));

    assert!(local(d, "/sandbox").await.starts_with("Sandbox: off"));
    assert!(fails(d, "/sandbox maybe").await.contains("Usage"));
    match forge_tools::sandbox::backend() {
        Some(_) => {
            local(d, "/sandbox read-only").await;
            assert_eq!(
                d.engine.tool_ctx().sandbox_policy().map(|p| p.mode),
                Some(forge_tools::sandbox::SandboxMode::ReadOnly)
            );
            local(d, "/sandbox off").await;
            assert!(d.engine.tool_ctx().sandbox_policy().is_none());
        }
        None => assert!(fails(d, "/sandbox on").await.contains("No sandbox is available")),
    }
}

#[tokio::test]
async fn rename_export_context_and_diff() {
    let mut t = driver();
    let d = &mut t.d;
    assert!(fails(d, "/rename").await.contains("Nothing to name yet"));
    assert_eq!(local(d, "/rename   \"My\x07  session.\"  ").await, "Session renamed to: My session");
    assert_eq!(d.engine.transcript().title().as_deref(), Some("My session"));

    t.p.push(MockTurn::text("The parser drops the last token."));
    run(d, "why does the parser fail?").await;
    // No name given: the small model suggests one.
    t.p.push(MockTurn::text("\"Parser drops last token.\""));
    assert_eq!(local(d, "/rename").await, "Session renamed to: Parser drops last token");
    let reqs = t.p.requests();
    let titled = reqs.last().unwrap();
    assert_eq!(titled.model, forge_api::models::SMALL_FAST_MODEL);
    assert!(titled.tools.is_empty());

    let file = t.proj.join("out/chat.txt");
    let out = local(d, "/export out/chat.txt").await;
    assert_eq!(out, format!("Conversation exported to: {}", file.display()));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("> why does the parser fail?") && text.contains("The parser drops the last token."),
        "{text}"
    );
    assert!(text.contains("(Parser drops last token)"), "the title is in the header");

    let ctx = local(d, "/context").await;
    for want in ["System prompt", "Built-in tools", "Messages", "Free", "the last request measured"] {
        assert!(ctx.contains(want), "{ctx}");
    }
    assert!(local(d, "/context all").await.contains("Tools:\n  "));
    assert!(fails(d, "/context some").await.contains("Usage"));

    // Not a git repository: /diff shows what Forge changed, against the original.
    let f = t.proj.join("notes.txt");
    std::fs::write(&f, "one\n").unwrap();
    let turn = d.engine.state.uuids[0].clone();
    d.engine.history().begin_turn(&turn);
    d.engine.history().snapshot(&f);
    std::fs::write(&f, "one\ntwo\n").unwrap();
    let diff = local(d, "/diff").await;
    assert!(diff.contains("--- a/notes.txt") && diff.contains("+two"), "{diff}");
    assert!(diff.contains("1. \"why does the parser fail?\": notes.txt"), "{diff}");
}

#[tokio::test]
async fn debug_needs_a_logger() {
    // No front end registered a logger in unit tests.
    let mut t = driver();
    if crate::debug::active().is_none() {
        assert!(fails(&mut t.d, "/debug").await.contains("Could not turn on debug logging"));
    }
}

#[tokio::test]
async fn interactive_surfaces_save_defaults() {
    let mut t = driver();
    let user = t._dir.path().join("home/settings.json");
    t.d.surface = Surface::Repl;
    t.d.info.user_settings = user.clone();
    let d = &mut t.d;
    let out = local(d, "/model sonnet").await;
    assert!(out.contains(&format!("Saved in user settings ({}).", user.display())), "{out}");
    local(d, "/effort high").await;
    local(d, "/effort max").await;
    local(d, "/autocompact 400k").await;
    local(d, "/output-style learning").await;
    let v = read_json(&user);
    assert_eq!((v["model"].as_str(), v["effortLevel"].as_str()), (Some("sonnet"), Some("high")), "max isn't saved");
    assert_eq!(v["autoCompactWindow"], 400_000);
    assert_eq!(read_json(&t.proj.join(".forge/settings.local.json"))["outputStyle"], "learning");
    local(d, "/effort auto").await;
    assert!(read_json(&user).get("effortLevel").is_none());

    // Print mode changes the session only.
    d.surface = Surface::Print;
    local(d, "/model haiku").await;
    assert_eq!(read_json(&user)["model"], "sonnet");
}

fn verdict(v: &str, reason: &str) -> MockTurn {
    MockTurn::text(&serde_json::json!({"verdict": v, "reason": reason}).to_string())
}

fn last_user_text(req: &forge_types::MessagesRequest) -> String {
    serde_json::to_string(req.messages.last().unwrap()).unwrap()
}

#[tokio::test]
async fn goal_runs_until_the_check_passes() {
    let mut t = driver();
    let glob = || MockTurn::tool("Glob", serde_json::json!({"pattern": "*"}));
    for turn in [
        glob(),
        MockTurn::text("Started."),
        verdict("not_met", "the tests were never run"),
        glob(),
        MockTurn::text("Ran them; all pass."),
        verdict("met", "the transcript shows passing tests"),
    ] {
        t.p.push(turn);
    }
    let results = run_all(&mut t.d, "/goal the tests pass").await;
    assert_eq!(results.len(), 2, "the first turn, then one continuation");
    assert_eq!(results[1].result.as_deref(), Some("Ran them; all pass."));
    let g = t.d.goal.as_ref().unwrap();
    assert_eq!((g.status.as_str(), g.checks), ("achieved", 2));

    let reqs = t.p.requests();
    assert_eq!(reqs.len(), 6);
    assert!(last_user_text(&reqs[0]).contains("the tests pass"));
    // The check runs on the small model, without tools, over the transcript.
    assert_eq!(reqs[2].model, forge_api::models::SMALL_FAST_MODEL);
    assert!(reqs[2].tools.is_empty());
    let check = last_user_text(&reqs[2]);
    assert!(check.contains("Goal: the tests pass") && check.contains("TOOL CALL Glob"), "{check}");
    let cont = last_user_text(&reqs[3]);
    assert!(
        cont.contains("Goal check: not met yet. the tests were never run")
            && cont.contains("Keep working toward the goal: the tests pass"),
        "{cont}"
    );
    let status = local(&mut t.d, "/goal").await;
    assert!(status.contains("Status: achieved") && status.contains("Checked 2 time(s)"), "{status}");

    // The goal is recorded outside the message chain.
    let path = t.d.engine.transcript().path().unwrap().to_path_buf();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    assert_eq!(loaded.goal.unwrap()["status"], "achieved");
}

#[tokio::test]
async fn goal_pauses_without_progress_and_resumes_on_a_prompt() {
    let mut t = driver();
    for _ in 0..3 {
        t.p.push(MockTurn::text("Thinking about it."));
        t.p.push(verdict("not_met", "nothing done"));
    }
    let results = run_all(&mut t.d, "/goal ship it").await;
    assert_eq!(results.len(), 3, "three turns without a tool, then a pause");
    let g = t.d.goal.as_ref().unwrap();
    assert_eq!((g.is_active(), g.paused.as_deref()), (true, Some("no progress")));
    assert!(local(&mut t.d, "/goal").await.contains("(paused: no progress)"));

    // The next prompt resumes it: its turn is checked again.
    t.p.push(MockTurn::text("ok"));
    t.p.push(verdict("impossible", "there is nothing to ship"));
    let results = run_all(&mut t.d, "try again").await;
    assert_eq!(results.len(), 1);
    let g = t.d.goal.as_ref().unwrap();
    assert_eq!(g.status, crate::goal::Status::Failed("there is nothing to ship".into()));
    assert!(local(&mut t.d, "/goal").await.contains("Last check: there is nothing to ship"));
}

#[tokio::test]
async fn goal_is_cleared_by_fatal_errors_and_by_request() {
    let mut t = driver();
    t.p.push(MockTurn::tool("Glob", serde_json::json!({"pattern": "*"})));
    t.p.push(MockTurn::text("halfway"));
    t.p.push(verdict("not_met", "more to do"));
    t.p.push(MockTurn::http_error(401, "authentication_error"));
    let results = run_all(&mut t.d, "/goal finish").await;
    assert_eq!(results.len(), 2);
    assert!(results[1].is_error && results[1].fatal);
    assert_eq!(t.d.goal.as_ref().unwrap().status, crate::goal::Status::Cleared);

    // A transient error only pauses it.
    let mut t = driver();
    t.p.push(MockTurn::text("a"));
    t.p.push(verdict("not_met", "x"));
    t.p.push(MockTurn::http_error(400, "invalid_request_error"));
    run_all(&mut t.d, "/goal finish").await;
    let g = t.d.goal.as_ref().unwrap();
    assert_eq!((g.is_active(), g.paused.as_deref()), (true, Some("error")));

    assert_eq!(local(&mut t.d, "/goal stop").await, "Goal cleared: finish");
    assert_eq!(local(&mut t.d, "/goal").await, "No goal set. Set one with /goal <condition>.");
    assert_eq!(local(&mut t.d, "/goal cancel").await, "No goal set.");
    assert!(fails(&mut t.d, &format!("/goal {}", "x".repeat(4001))).await.contains("too long"));

    // /clear ends it too.
    t.d.goal = Some(crate::goal::Goal::new("y", 0.0));
    local(&mut t.d, "/clear").await;
    assert!(t.d.goal.is_none());
}

#[tokio::test]
async fn goal_is_refused_when_hooks_are_disabled_and_restored_on_resume() {
    let mut t = driver_with(|dir, _| {
        std::fs::write(dir.join("proj/.forge/settings.json"), r#"{"disableAllHooks": true}"#).unwrap();
    });
    assert!(fails(&mut t.d, "/goal x").await.contains("disableAllHooks"));

    // An active goal comes back with --resume.
    let mut t = driver();
    t.p.push(MockTurn::text("on it"));
    t.p.push(MockTurn::http_error(400, "invalid_request_error"));
    run_all(&mut t.d, "/goal keep going").await;
    assert!(t.d.goal.as_ref().unwrap().is_active());
    let id = t.d.info.session_id.clone();
    let store = t._dir.path().join("store");
    let s = build_session(
        LaunchOptions {
            cwd: t.proj.clone(),
            provider: Some(t.p.clone()),
            store_root: Some(store),
            setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
            resume: crate::Resume::Id(id),
            ..Default::default()
        },
        Arc::new(NullSink),
        Arc::new(DenyPrompter),
    )
    .unwrap();
    let d = Driver::new(s, Surface::Print, None);
    let g = d.goal.as_ref().unwrap();
    assert_eq!((g.condition.as_str(), g.is_active()), ("keep going", true));
}

#[tokio::test]
async fn btw_answers_without_touching_the_conversation() {
    let mut t = driver();
    t.p.push(MockTurn::text("Edited main.rs."));
    run(&mut t.d, "fix the bug").await;
    let before = t.d.engine.state.messages.clone();
    assert!(local(&mut t.d, "/btw").await.starts_with("No side questions yet"));

    t.p.push(MockTurn::text("main.rs"));
    assert_eq!(local(&mut t.d, "/btw which file did you edit?").await, "main.rs");
    t.p.push(MockTurn::text("No."));
    assert_eq!(local(&mut t.d, "/btw any others?").await, "No.");
    assert_eq!(t.d.engine.state.messages, before, "side questions stay out of the conversation");

    let reqs = t.p.requests();
    let side = &reqs[2];
    assert_eq!(side.tool_choice, Some(serde_json::json!({"type": "none"})));
    assert!(!side.tools.is_empty(), "tools stay listed, so the cached prefix holds");
    assert_eq!(side.system, reqs[0].system);
    let text = serde_json::to_string(&side.messages).unwrap();
    assert!(text.contains("which file did you edit?") && text.contains("main.rs") && text.contains("any others?"));
    assert!(local(&mut t.d, "/btw").await.starts_with("/btw any others?"));

    // The next real turn doesn't see them.
    t.p.push(MockTurn::text("done"));
    run(&mut t.d, "next").await;
    let text = serde_json::to_string(&t.p.requests().last().unwrap().messages).unwrap();
    assert!(!text.contains("any others?"));
}

#[tokio::test]
async fn recap_plan_and_shell_mode() {
    let mut t = driver();
    assert!(fails(&mut t.d, "/recap").await.contains("Nothing to recap"));
    t.p.push(MockTurn::text("Done."));
    run(&mut t.d, "add a flag").await;
    t.p.push(MockTurn::text("Added --verbose;\nnothing open."));
    assert_eq!(local(&mut t.d, "/recap").await, "Added --verbose; nothing open.");

    assert!(local(&mut t.d, "/plan").await.starts_with("Plan mode on"));
    assert_eq!(t.d.engine.handle().permissions.read().unwrap().mode.as_str(), "plan");
    t.p.push(MockTurn::text("Here's a plan."));
    let r = run(&mut t.d, "/plan add caching").await;
    assert_eq!(r.result.as_deref(), Some("Here's a plan."));

    // `!command` runs in the shell and the model sees command and output.
    t.p.push(MockTurn::text("It printed hi."));
    let r = run(&mut t.d, "!echo hi; echo oops >&2; exit 3").await;
    assert_eq!(r.result.as_deref(), Some("It printed hi."));
    let prompt = last_user_text(t.p.requests().last().unwrap());
    assert!(
        prompt.contains("<shell-command>echo hi; echo oops >&2; exit 3</shell-command>")
            && prompt.contains("exit-code=\\\"3\\\"")
            && prompt.contains("hi\\noops"),
        "{prompt}"
    );
}

#[tokio::test]
async fn shell_mode_can_skip_the_model() {
    let mut t = driver_with(|dir, _| {
        std::fs::write(dir.join("proj/.forge/settings.json"), r#"{"respondToBashCommands": false}"#).unwrap();
    });
    let calls = t.p.requests().len();
    assert_eq!(local(&mut t.d, "!echo hi").await, "hi");
    assert_eq!(t.p.requests().len(), calls, "no model call");
    let last = serde_json::to_string(t.d.engine.state.messages.last().unwrap()).unwrap();
    assert!(last.contains("<shell-command>echo hi</shell-command>"), "{last}");
    assert!(fails(&mut t.d, "!exit 2").await.is_empty());
    // A lone "!" is an ordinary prompt.
    t.p.push(MockTurn::text("?"));
    assert_eq!(run(&mut t.d, "!").await.num_turns, 1);
}

fn write_turn(path: &Path, content: &str) -> MockTurn {
    MockTurn::tool("Write", serde_json::json!({"file_path": path, "content": content}))
}

/// Edits allowed, and no verification reminders (they would take scripted replies).
fn accept_edits(dir: &Path, o: &mut LaunchOptions) {
    o.permission_mode = Some("acceptEdits".into());
    std::fs::write(dir.join("proj/.forge/settings.json"), r#"{"verification": {"enabled": false}}"#).unwrap();
}

#[tokio::test]
async fn rewind_restores_code_and_conversation() {
    let mut t = driver_with(accept_edits);
    let a = t.proj.join("a.txt");
    for turn in [
        write_turn(&a, "one"),
        MockTurn::text("created"),
        write_turn(&a, "two"),
        MockTurn::text("changed"),
        MockTurn::text("hi"),
    ] {
        t.p.push(turn);
    }
    run(&mut t.d, "create a").await;
    run(&mut t.d, "change a").await;
    run(&mut t.d, "hello").await;
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "two");

    let list = local(&mut t.d, "/rewind").await;
    assert!(list.contains("1. create a  · 1 file(s) changed since"), "{list}");
    assert!(list.contains("2. change a  · 1 file(s) changed since") && list.contains("3. hello\n"), "{list}");

    // Code only: the file goes back, the conversation stays.
    let n = t.d.engine.state.messages.len();
    assert_eq!(local(&mut t.d, "/rewind 2 code").await, "Restored 1 file(s). The conversation is unchanged.");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "one");
    assert_eq!(t.d.engine.state.messages.len(), n);

    // Conversation only: back to before "change a", and the prompt comes back.
    let out = local(&mut t.d, "/rewind 2 conversation").await;
    assert!(out.ends_with("Your prompt was:\n\nchange a"), "{out}");
    assert_eq!(t.d.engine.prompt_points().len(), 1);
    // The rewind survives a resume.
    let path = t.d.engine.transcript().path().unwrap().to_path_buf();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    assert_eq!(loaded.messages.len(), t.d.engine.state.messages.len());

    // Both, from the first prompt: a.txt didn't exist before it.
    let out = local(&mut t.d, "/rewind 1 both").await;
    assert!(out.contains("Restored 0 file(s), deleted 1 new one(s).") && out.ends_with("create a"), "{out}");
    assert!(!a.exists());
    assert!(t.d.engine.state.messages.is_empty());
    assert!(fails(&mut t.d, "/rewind").await.contains("Nothing to rewind"));
}

#[tokio::test]
async fn rewind_summarizes_part_of_the_conversation() {
    let mut t = driver();
    for (q, a) in [("one", "A1"), ("two", "A2"), ("three", "A3")] {
        t.p.push(MockTurn::text(a));
        run(&mut t.d, q).await;
    }
    assert!(fails(&mut t.d, "/rewind 1 summarize-to").await.contains("Nothing before"));
    assert!(fails(&mut t.d, "/rewind 9 both").await.contains("No prompt 9"));
    assert!(fails(&mut t.d, "/rewind 1 sideways").await.contains("Unknown action"));

    t.p.push(MockTurn::text("<summary>Asked one and two.</summary>"));
    let out = local(&mut t.d, "/rewind 3 summarize-to keep names").await;
    assert!(out.starts_with("Summarized the conversation before prompt 3"), "{out}");
    let req = t.p.requests().last().unwrap().clone();
    assert_eq!(req.tool_choice, Some(serde_json::json!({"type": "none"})));
    let sent = serde_json::to_string(&req.messages).unwrap();
    assert!(sent.contains("\"two\"") && !sent.contains("three") && sent.contains("keep names"), "{sent}");
    let msgs = &t.d.engine.state.messages;
    assert_eq!(msgs.len(), 3, "summary, then the third prompt and its answer");
    assert!(
        msgs[0].text().contains("before this point was summarized") && msgs[0].text().contains("Asked one and two.")
    );
    assert_eq!(t.d.engine.prompt_points().len(), 1, "a summary isn't a prompt");

    // On disk the same conversation comes back.
    let path = t.d.engine.transcript().path().unwrap().to_path_buf();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    let texts: Vec<String> = loaded.messages.iter().map(|e| e.message.text()).collect();
    assert_eq!(texts.len(), 3);
    assert!(texts[0].contains("Asked one and two.") && texts[1] == "three" && texts[2] == "A3", "{texts:?}");

    // Summarize from a prompt on: the end is replaced.
    t.p.push(MockTurn::text("Asked three."));
    local(&mut t.d, "/rewind 1 summarize-from").await;
    let msgs = &t.d.engine.state.messages;
    assert_eq!(msgs.len(), 2);
    assert!(msgs[1].text().contains("from this point on was summarized") && msgs[1].text().contains("Asked three."));
}

#[tokio::test]
async fn clear_resume_and_branch_switch_sessions() {
    let mut t = driver();
    t.p.push(
        MockTurn::text("first answer").with_usage(forge_types::Usage { input_tokens: 1_000_000, ..Default::default() }),
    );
    run(&mut t.d, "first").await;
    let first = t.d.info.session_id.clone();
    let cost = t.d.engine.state.total_cost_usd;
    assert!(cost > 0.0);
    let live = t.d.live();

    let out = local(&mut t.d, "/clear old work").await;
    assert_eq!(
        out,
        format!("Conversation cleared. The previous one is saved as \"old work\": /resume {first} brings it back.")
    );
    let second = t.d.info.session_id.clone();
    assert_ne!(first, second);
    assert_eq!(live.session_id(), second, "front ends follow the switch");
    assert!(t.d.engine.state.messages.is_empty());
    assert_eq!(t.d.engine.state.total_cost_usd, cost, "the cost so far carries over");

    // /resume is for the interactive session.
    assert!(fails(&mut t.d, "/resume").await.contains("--resume"));
    t.d.surface = Surface::Repl;
    t.p.push(MockTurn::text("second answer"));
    run(&mut t.d, "second").await;
    let list = local(&mut t.d, "/resume").await;
    assert!(list.contains("1. old work") && list.contains(&first[..8]), "{list}");
    assert!(fails(&mut t.d, "/resume nothing-like-it").await.contains("No conversation matches"));
    let out = local(&mut t.d, "/resume old work").await;
    assert_eq!(out, format!("Resumed {first} \"old work\" (2 messages)."));
    assert_eq!(t.d.engine.state.messages[0].text(), "first");
    assert_eq!(t.d.engine.transcript().title().as_deref(), Some("old work"));

    // A branch copies the conversation under a new id; the original stays put.
    let out = local(&mut t.d, "/branch try another way").await;
    let branch = t.d.info.session_id.clone();
    assert_eq!(
        out,
        format!(
            "Branched into {branch} \"try another way\". The original stays as it was: /resume {first} returns to it."
        )
    );
    assert_eq!(t.d.engine.state.messages.len(), 2);
    t.p.push(MockTurn::text("branch answer"));
    run(&mut t.d, "in the branch").await;
    let store = forge_session::SessionStore::new(t._dir.path().join("store"));
    let original = forge_session::LoadedSession::load(&store.find(&first).unwrap(), None).unwrap();
    assert_eq!(original.messages.len(), 2, "the original is untouched");
    let copy = forge_session::LoadedSession::load(&store.find(&branch).unwrap(), None).unwrap();
    assert_eq!(copy.messages.len(), 4);
}

#[tokio::test]
async fn cd_and_reload_rebuild_the_session() {
    let mut t = driver();
    t.p.push(MockTurn::text("ok"));
    run(&mut t.d, "remember 42").await;
    let other = t._dir.path().join("other");
    std::fs::create_dir_all(other.join(".forge/skills/deploy")).unwrap();
    std::fs::write(other.join(".forge/skills/deploy/SKILL.md"), "---\ndescription: Deploy it\n---\nRun make deploy.")
        .unwrap();
    let other = other.canonicalize().unwrap();
    assert!(fails(&mut t.d, "/cd /no/such/dir").await.contains("/no/such/dir"));
    let first = t.d.info.session_id.clone();
    let out = local(&mut t.d, &format!("/cd {}", other.display())).await;
    assert!(out.starts_with(&format!("Now working in {}", other.display())), "{out}");
    assert_ne!(t.d.info.session_id, first);
    assert_eq!(t.d.engine.tool_ctx().project_dir, other);
    assert_eq!(t.d.info.cwd, other);
    assert!(t.d.catalog.skills.iter().any(|s| s.name == "deploy"), "the new directory's skills load");
    t.p.push(MockTurn::text("still 42"));
    run(&mut t.d, "what number?").await;
    let req = serde_json::to_string(&t.p.requests().last().unwrap().messages).unwrap();
    assert!(req.contains("remember 42") && req.contains("moved this session to"), "{req}");

    // A new skill appears after /reload-skills, in the same conversation.
    std::fs::create_dir_all(other.join(".forge/skills/lint")).unwrap();
    std::fs::write(other.join(".forge/skills/lint/SKILL.md"), "---\ndescription: Lint it\n---\nRun make lint.")
        .unwrap();
    let id = t.d.info.session_id.clone();
    let n = t.d.engine.state.messages.len();
    let out = local(&mut t.d, "/reload-skills").await;
    assert!(out.starts_with("Reloaded: 13 skills (+1, -0)"), "{out}");
    assert_eq!((t.d.info.session_id.clone(), t.d.engine.state.messages.len()), (id, n));
    assert!(local(&mut t.d, "/skills").await.contains("lint - Lint it"));
}

/// Run the scheduler `scale` times faster than real time.
fn fast_clock(d: &Driver, scale: f64) {
    d.scheduler.as_ref().unwrap().lock().unwrap().clock = crate::schedule::Clock::scaled(chrono::Local::now(), scale);
}

async fn fire_next(d: &mut Driver) -> Vec<TurnResult> {
    let wait = d.next_wait().expect("a task is pending");
    assert!(wait < std::time::Duration::from_secs(5), "{wait:?}");
    tokio::time::sleep(wait).await;
    let mut out = vec![];
    let mut report = |r: &TurnResult| out.push(r.clone());
    assert!(d.run_due(&mut report).await);
    out
}

#[tokio::test]
async fn loop_with_an_interval_schedules_and_runs_now() {
    let mut t = driver();
    fast_clock(&t.d, 3000.0);
    t.p.push(MockTurn::text("deploy is green"));
    let r = run(&mut t.d, "/loop 5m check the deploy").await;
    assert_eq!(r.result.as_deref(), Some("deploy is green"), "it runs at once");
    let tasks = local(&mut t.d, "/tasks").await;
    assert!(
        tasks.contains("[scheduled, every 5 minutes (*/5 * * * *)]") && tasks.contains("check the deploy"),
        "{tasks}"
    );
    assert!(t.d.has_pending());

    t.p.push(MockTurn::text("still green"));
    let fired = fire_next(&mut t.d).await;
    assert_eq!(fired[0].result.as_deref(), Some("still green"));
    assert!(last_user_text(t.p.requests().last().unwrap()).contains("check the deploy"));
    assert!(t.d.has_pending(), "recurring");

    let id = t.d.scheduler.as_ref().unwrap().lock().unwrap().tasks[0].id.clone();
    assert!(local(&mut t.d, &format!("/tasks stop {id}")).await.starts_with(&format!("Deleted scheduled task {id}")));
    assert!(!t.d.has_pending());
    // Recorded in the transcript.
    let path = t.d.engine.transcript().path().unwrap().to_path_buf();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    assert_eq!(loaded.schedule, Some(serde_json::json!([])));
    assert!(fails(&mut t.d, "/loop 0m x").await.contains("Usage: /loop"));
}

#[tokio::test]
async fn self_paced_loops_reschedule_fall_back_and_stop() {
    let mut t = driver();
    fast_clock(&t.d, 30_000.0);
    // Iteration 1: the model schedules the next one.
    t.p.push(MockTurn::tool(
        "ScheduleWakeup",
        serde_json::json!({"delaySeconds": 120, "reason": "CI takes a few minutes", "prompt": "/loop watch CI"}),
    ));
    t.p.push(MockTurn::text("CI running"));
    run(&mut t.d, "/loop watch CI").await;
    let first = last_user_text(&t.p.requests()[0]);
    assert!(
        first.contains("self-paced loop") && first.contains("/loop watch CI") && first.contains("watch CI"),
        "{first}"
    );
    assert!(t.d.has_pending());

    // Iteration 2: neither reschedules nor stops: one fallback check is queued.
    t.p.push(MockTurn::text("still running"));
    fire_next(&mut t.d).await;
    let pending = t.d.scheduler.as_ref().unwrap().lock().unwrap().tasks.clone();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].describe().contains("fallback check"), "{:?}", pending[0]);

    // Iteration 3 (the fallback) misses again: the loop ends.
    t.p.push(MockTurn::text("hmm"));
    fire_next(&mut t.d).await;
    assert!(!t.d.has_pending());

    // A model that says stop ends it at once.
    t.p.push(MockTurn::tool("ScheduleWakeup", serde_json::json!({"stop": true})));
    t.p.push(MockTurn::text("all done"));
    run(&mut t.d, "/loop babysit the PR").await;
    assert!(!t.d.has_pending());
    assert!(t.d.scheduler.as_ref().unwrap().lock().unwrap().stopped);
}

#[tokio::test]
async fn loop_md_and_scheduled_commands() {
    let mut t = driver_with(|dir, _| {
        std::fs::write(dir.join("proj/.forge/loop.md"), "Tidy one thing.").unwrap();
    });
    fast_clock(&t.d, 3000.0);
    t.p.push(MockTurn::text("tidied"));
    run(&mut t.d, "/loop 10m").await;
    assert!(last_user_text(t.p.requests().last().unwrap()).contains("Tidy one thing."));
    let id = t.d.scheduler.as_ref().unwrap().lock().unwrap().tasks[0].id.clone();
    local(&mut t.d, &format!("/tasks stop {id}")).await;

    // A built-in command in a scheduled prompt reaches the model as text.
    t.p.push(MockTurn::tool(
        "CronCreate",
        serde_json::json!({"cron": "* * * * *", "prompt": "/help", "recurring": false}),
    ));
    t.p.push(MockTurn::text("scheduled"));
    run(&mut t.d, "remind me").await;
    let created = serde_json::to_string(&t.p.requests().last().unwrap().messages).unwrap();
    assert!(created.contains("Scheduled ") && created.contains("CronDelete"), "{created}");
    t.p.push(MockTurn::text("I can't run /help for you"));
    fire_next(&mut t.d).await;
    assert!(last_user_text(t.p.requests().last().unwrap()).contains("\"/help\""));
    assert!(!t.d.has_pending(), "a one-shot is gone after it fires");
}

#[tokio::test]
async fn advisor_is_consulted_only_while_set() {
    let mut t = driver();
    let tool_names = |r: &forge_types::MessagesRequest| r.tools.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
    assert!(local(&mut t.d, "/advisor").await.starts_with("No advisor is set"));
    t.p.push(MockTurn::text("ok"));
    run(&mut t.d, "refactor the parser").await;
    assert!(!tool_names(&t.p.requests()[0]).contains(&"Advisor".to_string()), "hidden until set");

    let out = local(&mut t.d, "/advisor sonnet").await;
    assert_eq!(out, "Advisor set to Sonnet 5.5 (claude-sonnet-5-5). Forge can now ask it for advice; each question is a request to that model. (This session only.)");
    let cost_before = t.d.engine.state.total_cost_usd;
    t.p.push(MockTurn::tool("Advisor", serde_json::json!({"question": "Is splitting the lexer safe?"})));
    t.p.push(
        MockTurn::text("Add a regression test for nested quotes first.")
            .with_usage(forge_types::Usage { input_tokens: 1_000_000, ..Default::default() }),
    );
    t.p.push(MockTurn::text("Will do."));
    run(&mut t.d, "go ahead").await;
    let reqs = t.p.requests();
    assert!(tool_names(&reqs[1]).contains(&"Advisor".to_string()));
    let ask = &reqs[2];
    assert_eq!(ask.model, "claude-sonnet-5-5");
    assert!(ask.tools.is_empty() && ask.system[0].text.contains("advising a coding agent"));
    let q = last_user_text(ask);
    assert!(
        q.contains("USER: refactor the parser") && q.contains("The agent asks:\\nIs splitting the lexer safe?"),
        "{q}"
    );
    let result = serde_json::to_string(&reqs[3].messages).unwrap();
    assert!(result.contains("Add a regression test for nested quotes first."), "{result}");
    // A million input tokens on Sonnet 5.5: $2, counted in the session.
    assert!(t.d.engine.state.total_cost_usd - cost_before >= 2.0, "{}", t.d.engine.state.total_cost_usd);
    assert!(t.d.engine.state.model_usage.contains_key("claude-sonnet-5-5"));

    // Off: hidden again, and a call by name fails.
    local(&mut t.d, "/advisor off").await;
    t.p.push(MockTurn::tool("Advisor", serde_json::json!({"question": "?"})));
    t.p.push(MockTurn::text("fine"));
    run(&mut t.d, "again").await;
    let last = t.p.requests().last().unwrap().clone();
    assert!(!tool_names(&last).contains(&"Advisor".to_string()));
    assert!(serde_json::to_string(&last.messages).unwrap().contains("is_error"), "the hidden tool isn't run");
}
