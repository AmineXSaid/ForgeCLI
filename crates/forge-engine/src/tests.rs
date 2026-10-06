use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_api::{MockProvider, MockTurn};
use forge_hooks::{HookBase, HookRunner, HooksConfig};
use forge_permissions::{PermissionMode, RuleSet, Subject};
use forge_session::{FileHistory, SessionStore, Transcript};
use forge_tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use forge_types::sdk::ResultSubtype;
use forge_types::{ContentBlock, MessageContent};
use serde_json::{json, Value};

use super::*;

const SID: &str = "00000000-0000-4000-8000-000000000001";

/// A test tool: sleeps `ms`, returns `tag`. Concurrency-safe; read-only unless `writes`.
struct Sleepy {
    name: &'static str,
    writes: bool,
    safe: bool,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Tool for Sleepy {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> String {
        "test".into()
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"ms": {"type": "number"}, "tag": {"type": "string"}}, "required": ["tag"]})
    }
    fn is_read_only(&self, _: &Value) -> bool {
        !self.writes
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        self.safe
    }
    fn permission_subject(&self, input: &Value, _: &ToolContext) -> Subject {
        Subject::Name(input["tag"].as_str().unwrap_or("").into())
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let ms = input["ms"].as_u64().unwrap_or(0);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
            _ = ctx.cancel.cancelled() => return ToolOutput::error(forge_tools::INTERRUPTED),
        }
        let tag = input["tag"].as_str().unwrap_or("").to_string();
        self.log.lock().unwrap().push(tag.clone());
        ToolOutput::text(tag)
    }
}

/// Counts concurrent prompts; answers with a fixed decision.
struct CountingPrompter {
    active: AtomicUsize,
    max: AtomicUsize,
    allow: bool,
}

#[async_trait::async_trait]
impl PermissionPrompter for CountingPrompter {
    async fn ask(&self, _p: PermissionPrompt) -> PermissionAnswer {
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.allow {
            PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] }
        } else {
            PermissionAnswer::Deny { message: String::new(), interrupt: false }
        }
    }
}

struct Harness {
    dir: tempfile::TempDir,
    provider: Arc<MockProvider>,
    sink: Arc<VecSink>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    fn new(turns: Vec<MockTurn>) -> Self {
        Harness {
            dir: tempfile::tempdir().unwrap(),
            provider: Arc::new(MockProvider::new(turns)),
            sink: Arc::new(VecSink::default()),
            log: Arc::new(Mutex::new(vec![])),
        }
    }

    fn cwd(&self) -> std::path::PathBuf {
        self.dir.path().canonicalize().unwrap().join("proj")
    }

    fn engine_with(
        &self,
        cfg: EngineConfig,
        mode: PermissionMode,
        prompter: Arc<dyn PermissionPrompter>,
        hooks: Value,
    ) -> Engine {
        let cwd = self.cwd();
        std::fs::create_dir_all(&cwd).unwrap();
        let store = SessionStore::new(self.dir.path().join("store"));
        let transcript = Arc::new(Transcript::create(&store, &cwd, SID, None, true).unwrap());
        let mut tools = ToolRegistry::new();
        forge_tools::builtin::register_core(&mut tools);
        for (name, writes, safe) in [("SafeA", false, true), ("Mut", true, false), ("AskSafe", true, true)] {
            tools.register(Arc::new(Sleepy { name, writes, safe, log: self.log.clone() }));
        }
        let (hooks_cfg, errs) = HooksConfig::from_settings(Some(&hooks));
        assert!(errs.is_empty());
        let hooks = HookRunner::new(
            hooks_cfg,
            HookBase { session_id: SID.into(), transcript_path: None, cwd: cwd.clone(), project_dir: cwd.clone() },
        );
        let parts = EngineParts {
            provider: self.provider.clone(),
            tools,
            tool_ctx: ToolContext::new(&cwd),
            permissions: forge_permissions::Engine::new(mode, RuleSet::default(), &cwd, &[]),
            hooks,
            prompter,
            sink: self.sink.clone(),
            transcript,
            history: Arc::new(FileHistory::new(self.dir.path().join("hist"))),
            system: vec![forge_types::SystemBlock::text("sys")],
        };
        Engine::new(cfg, parts).unwrap()
    }

    fn engine(&self) -> Engine {
        self.engine_with(EngineConfig::default(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}))
    }

    fn transcript_text(&self) -> String {
        let p = SessionStore::new(self.dir.path().join("store")).session_path(&self.cwd(), SID).unwrap();
        std::fs::read_to_string(p).unwrap()
    }
}

fn prompt(s: &str) -> MessageContent {
    MessageContent::Text(s.into())
}

fn tool_results(e: &Engine) -> Vec<(String, String, bool)> {
    e.state
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                Some((tool_use_id.clone(), content.to_text(), is_error.unwrap_or(false)))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn text_turn() {
    let h = Harness::new(vec![MockTurn::text("Hello there")]);
    let mut e = h.engine();
    let r = e.submit(prompt("hi")).await;
    assert_eq!(r.subtype, ResultSubtype::Success);
    assert_eq!(r.result.as_deref(), Some("Hello there"));
    assert_eq!(r.num_turns, 1);
    assert!(r.total_cost_usd > 0.0);
    let req = &h.provider.requests()[0];
    assert_eq!(req.model, "claude-opus-5-5");
    assert!(req.tools.iter().any(|t| t.name == "Bash"));
    assert!(h.transcript_text().contains("Hello there"));
}

#[tokio::test]
async fn tool_loop_reads_file() {
    let h = Harness::new(vec![]);
    std::fs::create_dir_all(h.cwd()).unwrap();
    std::fs::write(h.cwd().join("a.txt"), "secret-content").unwrap();
    h.provider.push(MockTurn::tool("Read", json!({"file_path": h.cwd().join("a.txt")})));
    h.provider.push(MockTurn::text("done"));
    let mut e = h.engine();
    let r = e.submit(prompt("read it")).await;
    assert_eq!(r.result.as_deref(), Some("done"));
    assert_eq!(r.num_turns, 2);
    let second = &h.provider.requests()[1];
    let last = second.messages.last().unwrap();
    assert!(
        matches!(&last.content[0], ContentBlock::ToolResult { content, .. } if content.to_text().contains("secret-content"))
    );
}

#[tokio::test]
async fn c2_result_order_survives_concurrency() {
    let calls = vec![
        ("SafeA", json!({"tag": "r1", "ms": 150})),
        ("SafeA", json!({"tag": "r2", "ms": 100})),
        ("SafeA", json!({"tag": "r3", "ms": 10})),
        ("Mut", json!({"tag": "edit", "ms": 10})),
        ("SafeA", json!({"tag": "r4", "ms": 80})),
        ("SafeA", json!({"tag": "r5", "ms": 5})),
    ];
    let h = Harness::new(vec![MockTurn::tools(&calls), MockTurn::text("ok")]);
    let mut e = h.engine();
    let start = Instant::now();
    e.submit(prompt("go")).await;
    let elapsed = start.elapsed();
    let results: Vec<String> = tool_results(&e).into_iter().map(|r| r.1).collect();
    assert_eq!(results, vec!["r1", "r2", "r3", "edit", "r4", "r5"], "results in tool_use order");
    let log = h.log.lock().unwrap().clone();
    assert_eq!(&log[..3], &["r3", "r2", "r1"], "first batch ran concurrently and finished inverted");
    assert_eq!(log[3], "edit", "unsafe tool ran alone between batches");
    assert_eq!(&log[4..], &["r5", "r4"]);
    assert!(elapsed < Duration::from_millis(150 + 100 + 10 + 10 + 80 + 5), "batches overlapped: {elapsed:?}");
    // The API saw all results in one user message, in order.
    let req = &h.provider.requests()[1];
    let ids: Vec<String> = req
        .messages
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 6);
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
}

#[tokio::test]
async fn c2_prompts_are_serialized() {
    let calls: Vec<(&str, Value)> = (0..4).map(|i| ("AskSafe", json!({"tag": format!("t{i}")}))).collect();
    let h = Harness::new(vec![MockTurn::tools(&calls), MockTurn::text("ok")]);
    let prompter = Arc::new(CountingPrompter { active: AtomicUsize::new(0), max: AtomicUsize::new(0), allow: true });
    let mut e = h.engine_with(EngineConfig::default(), PermissionMode::Default, prompter.clone(), json!({}));
    e.submit(prompt("go")).await;
    assert_eq!(prompter.max.load(Ordering::SeqCst), 1, "never two prompts at once");
    assert_eq!(h.log.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn c1_headless_denies_and_continues() {
    let h = Harness::new(vec![
        MockTurn::tools(&[("Bash", json!({"command": "touch made.txt"})), ("SafeA", json!({"tag": "fine"}))]),
        MockTurn::text("I could not run it"),
    ]);
    let mut e = h.engine_with(EngineConfig::default(), PermissionMode::Default, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("make a file")).await;
    assert_eq!(r.subtype, ResultSubtype::Success, "denial does not fail the turn");
    assert_eq!(r.permission_denials.len(), 1);
    assert_eq!(r.permission_denials[0].tool_name, "Bash");
    let results = tool_results(&e);
    assert!(results[0].2 && results[0].1.contains("--allowedTools"), "{:?}", results[0]);
    assert_eq!(results[1].1, "fine");
    assert!(!h.cwd().join("made.txt").exists());
}

#[tokio::test]
async fn c3_interrupt_during_stream() {
    let h =
        Harness::new(vec![MockTurn::text("a long answer that streams slowly").with_delay(Duration::from_millis(100))]);
    let mut e = h.engine();
    let handle = e.handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(350)).await;
        handle.interrupt();
    });
    let r = e.submit(prompt("talk")).await;
    assert_eq!(r.stop_reason.as_deref(), Some("interrupted"));
    assert_eq!(r.subtype, ResultSubtype::Success);
    let last = e.state.messages.last().unwrap();
    assert_eq!(last.text(), INTERRUPT_MARKER);
    assert!(h.transcript_text().contains(INTERRUPT_MARKER));
    // The next turn works normally.
    h.provider.push(MockTurn::text("back"));
    assert_eq!(e.submit(prompt("again")).await.result.as_deref(), Some("back"));
}

#[tokio::test]
async fn c3_interrupt_aborts_running_and_pending_tools() {
    let h = Harness::new(vec![MockTurn::tools(&[
        ("SafeA", json!({"tag": "slow", "ms": 5000})),
        ("Mut", json!({"tag": "never", "ms": 1})),
    ])]);
    let mut e = h.engine();
    let handle = e.handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.interrupt();
    });
    let start = Instant::now();
    let r = e.submit(prompt("go")).await;
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(r.stop_reason.as_deref(), Some("interrupted"));
    let results = tool_results(&e);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.2 && r.1.contains(forge_tools::INTERRUPTED)), "{results:?}");
    assert!(h.log.lock().unwrap().is_empty(), "the pending tool never ran");
    assert_eq!(e.state.messages.last().unwrap().text(), INTERRUPT_MARKER_TOOLS);
}

#[tokio::test]
async fn c6_fallback_on_overload_for_this_turn_only() {
    let h = Harness::new(vec![
        MockTurn::http_error(529, "overloaded_error"),
        MockTurn::text("from fallback"),
        MockTurn::text("primary again"),
    ]);
    let cfg = EngineConfig { fallback_models: vec!["claude-sonnet-5-5".into()], ..Default::default() };
    let mut e = h.engine_with(cfg, PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("hi")).await;
    assert_eq!(r.result.as_deref(), Some("from fallback"));
    let reqs = h.provider.requests();
    assert_eq!(reqs[0].model, "claude-opus-5-5");
    assert_eq!(reqs[1].model, "claude-sonnet-5-5");
    assert!(h
        .sink
        .take()
        .iter()
        .any(|ev| matches!(ev, EngineEvent::System { subtype, .. } if subtype == "model_fallback")));
    assert!(h.transcript_text().contains("model_fallback"));
    e.submit(prompt("again")).await;
    assert_eq!(h.provider.requests()[2].model, "claude-opus-5-5", "next turn starts on the primary");
}

#[tokio::test]
async fn non_overload_errors_fail_the_turn() {
    let h = Harness::new(vec![MockTurn::http_error(400, "invalid_request_error")]);
    let cfg = EngineConfig { fallback_models: vec!["claude-sonnet-5-5".into()], ..Default::default() };
    let mut e = h.engine_with(cfg, PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("hi")).await;
    assert_eq!(r.subtype, ResultSubtype::ErrorDuringExecution);
    assert!(r.is_error);
    assert_eq!(h.provider.requests().len(), 1);
}

#[tokio::test]
async fn c7_unknown_model_budget_fails_closed() {
    let h = Harness::new(vec![]);
    let cwd = h.cwd();
    std::fs::create_dir_all(&cwd).unwrap();
    let build = |cfg: EngineConfig| {
        let store = SessionStore::new(h.dir.path().join("s2"));
        let parts = EngineParts {
            provider: h.provider.clone(),
            tools: ToolRegistry::new(),
            tool_ctx: ToolContext::new(&cwd),
            permissions: forge_permissions::Engine::new(PermissionMode::Default, RuleSet::default(), &cwd, &[]),
            hooks: HookRunner::default(),
            prompter: Arc::new(DenyPrompter),
            sink: Arc::new(NullSink),
            transcript: Arc::new(Transcript::create(&store, &cwd, SID, None, false).unwrap()),
            history: Arc::new(FileHistory::new(h.dir.path().join("h2"))),
            system: vec![],
        };
        Engine::new(cfg, parts)
    };
    let err = build(EngineConfig { model: "my-local-model".into(), max_budget_usd: Some(1.0), ..Default::default() })
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::UnknownPricing(m) if m == "my-local-model"));
    let err = build(EngineConfig {
        fallback_models: vec!["other-unknown".into()],
        max_budget_usd: Some(1.0),
        ..Default::default()
    })
    .err()
    .unwrap();
    assert!(matches!(err, EngineError::UnknownPricing(m) if m == "other-unknown"));
    let mut pricing = std::collections::HashMap::new();
    pricing
        .insert("my-local-model".to_string(), Pricing { input: 1.0, output: 1.0, cache_read: 0.1, cache_write: 1.25 });
    assert!(build(EngineConfig {
        model: "my-local-model".into(),
        max_budget_usd: Some(1.0),
        pricing,
        ..Default::default()
    })
    .is_ok());
    assert!(
        build(EngineConfig { model: "my-local-model".into(), ..Default::default() }).is_ok(),
        "no budget, no check"
    );
}

#[tokio::test]
async fn c7_budget_and_max_turns_stop_the_run() {
    let big = forge_types::Usage { input_tokens: 1_000_000, output_tokens: 0, ..Default::default() };
    let h =
        Harness::new(vec![MockTurn::tool("SafeA", json!({"tag": "x"})).with_usage(big), MockTurn::text("unreached")]);
    let cfg = EngineConfig { max_budget_usd: Some(1.0), ..Default::default() };
    let mut e = h.engine_with(cfg, PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("go")).await;
    assert_eq!(r.subtype, ResultSubtype::ErrorMaxBudgetUsd);
    assert!(h.log.lock().unwrap().is_empty(), "tools of the over-budget message are not run");

    let h =
        Harness::new(vec![MockTurn::tool("SafeA", json!({"tag": "1"})), MockTurn::tool("SafeA", json!({"tag": "2"}))]);
    let cfg = EngineConfig { max_turns: Some(1), ..Default::default() };
    let mut e = h.engine_with(cfg, PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("go")).await;
    assert_eq!(r.subtype, ResultSubtype::ErrorMaxTurns);
    assert_eq!(r.num_turns, 1);
}

#[tokio::test]
async fn hooks_in_the_loop() {
    let hooks = json!({
        "PreToolUse": [{"matcher": "Mut", "hooks": [{"type": "command", "command": "echo 'Mut is not allowed here' >&2; exit 2"}]}],
        "Stop": [{"hooks": [{"type": "command", "command": "cat | grep -q '\"stop_hook_active\":false' && { echo 'run the tests first' >&2; exit 2; } || exit 0"}]}],
        "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo 'Project codename: TEAL'"}]}]
    });
    let h = Harness::new(vec![
        MockTurn::tool("Mut", json!({"tag": "x"})),
        MockTurn::text("first stop"),
        MockTurn::text("second stop"),
    ]);
    let mut e =
        h.engine_with(EngineConfig::default(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), hooks);
    let r = e.submit(prompt("go")).await;
    assert_eq!(r.result.as_deref(), Some("second stop"), "Stop hook made the model continue once");
    let results = tool_results(&e);
    assert!(results[0].2 && results[0].1.contains("Mut is not allowed"));
    assert!(h.log.lock().unwrap().is_empty());
    let first = &h.provider.requests()[0];
    assert!(first.messages[0].content.iter().any(|b| b.as_text().map(|t| t.contains("TEAL")).unwrap_or(false)));
    let third = &h.provider.requests()[2];
    assert!(third.messages.last().unwrap().text().contains("run the tests first"));
}

#[tokio::test]
async fn user_prompt_hook_can_block() {
    let hooks = json!({"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo 'no secrets in prompts' >&2; exit 2"}]}]});
    let h = Harness::new(vec![MockTurn::text("unreached")]);
    let mut e =
        h.engine_with(EngineConfig::default(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), hooks);
    let r = e.submit(prompt("my password is hunter2")).await;
    assert_eq!(r.prompt_blocked.as_deref(), Some("no secrets in prompts"));
    assert!(h.provider.requests().is_empty());
    assert!(e.state.messages.is_empty(), "blocked prompt is erased");
}

#[tokio::test]
async fn edits_are_checkpointed_for_rewind() {
    let h = Harness::new(vec![]);
    std::fs::create_dir_all(h.cwd()).unwrap();
    let f = h.cwd().join("f.txt");
    std::fs::write(&f, "before").unwrap();
    h.provider.push(MockTurn::tool("Read", json!({"file_path": f})));
    h.provider.push(MockTurn::tool("Edit", json!({"file_path": f, "old_string": "before", "new_string": "after"})));
    h.provider.push(MockTurn::text("edited"));
    let mut e = h.engine();
    e.submit(prompt("edit")).await;
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "after");
    let turn = e.state.uuids[0].clone();
    e.history().rewind(&turn, false).unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "before");
}

#[tokio::test]
async fn resumed_history_is_sent() {
    let h = Harness::new(vec![MockTurn::text("one"), MockTurn::text("two")]);
    {
        let mut e = h.engine();
        e.submit(prompt("first")).await;
    }
    let path = SessionStore::new(h.dir.path().join("store")).session_path(&h.cwd(), SID).unwrap();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    let mut e = h.engine();
    e.restore(&loaded);
    e.submit(prompt("second")).await;
    let req = &h.provider.requests()[1];
    assert_eq!(req.messages.len(), 3);
    assert_eq!(req.messages[1].text(), "one");
    let _ = Path::new("/");
}

#[tokio::test]
async fn cache_breakpoints_on_system_tools_and_last_message() {
    let h = Harness::new(vec![MockTurn::tool("SafeA", json!({"tag": "x"})), MockTurn::text("done")]);
    let mut e = h.engine();
    e.submit(prompt("go")).await;
    for req in h.provider.requests() {
        let count_msgs = req
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| {
                let v = serde_json::to_value(b).unwrap();
                v.get("cache_control").is_some()
            })
            .count();
        assert_eq!(count_msgs, 1, "exactly one message breakpoint");
        let last_block = serde_json::to_value(req.messages.last().unwrap().content.last().unwrap()).unwrap();
        assert!(last_block.get("cache_control").is_some(), "on the last block");
        assert!(req.system[0].cache_control.is_some());
        assert!(req.tools.last().unwrap().cache_control.is_some());
        assert!(req.tools[..req.tools.len() - 1].iter().all(|t| t.cache_control.is_none()));
    }
}

fn usage(input: u64) -> forge_types::Usage {
    forge_types::Usage { input_tokens: input, output_tokens: 10, ..Default::default() }
}

fn small_window() -> EngineConfig {
    // threshold = 60_000 - 32_000 - 13_000 = 15_000; micro at 7_500.
    EngineConfig { autocompact_window: Some(60_000), ..Default::default() }
}

fn result_text(req: &forge_types::MessagesRequest, id: &str) -> Option<String> {
    req.messages.iter().flat_map(|m| m.content.iter()).find_map(|b| match b {
        ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == id => Some(content.to_text()),
        _ => None,
    })
}

#[tokio::test]
async fn c9_micro_is_sticky_across_requests() {
    let h = Harness::new(vec![]);
    std::fs::create_dir_all(h.cwd()).unwrap();
    let big = h.cwd().join("big.txt");
    std::fs::write(&big, "line of text\n".repeat(1500)).unwrap();
    let mut e = h.engine_with(small_window(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    // Turn 1 reads the big file; later turns are plain. Usage stays between micro and auto thresholds.
    h.provider.push(MockTurn::tool("Read", json!({"file_path": big})).with_usage(usage(9_000)));
    h.provider.push(MockTurn::text("read it").with_usage(usage(9_000)));
    for i in 2..=5 {
        h.provider.push(MockTurn::text(&format!("answer {i}")).with_usage(usage(9_000)));
    }
    for i in 1..=5 {
        e.submit(prompt(&format!("turn {i}"))).await;
    }
    let reqs = h.provider.requests();
    let first_id = "toolu_00_read";
    assert!(result_text(&reqs[1], first_id).unwrap().contains("line of text"), "fresh result sent in full");
    let cleared_at = reqs
        .iter()
        .position(|r| result_text(r, first_id).as_deref() == Some(request::CLEARED_RESULT))
        .expect("cleared eventually");
    assert!(
        reqs[cleared_at..].iter().all(|r| result_text(r, first_id).as_deref() == Some(request::CLEARED_RESULT)),
        "stays cleared"
    );
    let t = h.transcript_text();
    assert_eq!(t.matches("\"subtype\":\"microcompact\"").count(), 1, "recorded once");
    assert!(t.contains("line of text"), "the transcript keeps the original");
}

#[tokio::test]
async fn c9_auto_triggers_at_threshold() {
    let h = Harness::new(vec![
        MockTurn::text("first answer").with_usage(usage(20_000)),
        MockTurn::text("<analysis>a</analysis><summary>The user asked about X.</summary>"),
        MockTurn::text("second answer"),
    ]);
    let mut e = h.engine_with(small_window(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    e.submit(prompt("first")).await;
    let r = e.submit(prompt("second")).await;
    assert_eq!(r.result.as_deref(), Some("second answer"));
    let reqs = h.provider.requests();
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[1].tool_choice, Some(json!({"type": "none"})), "summary request uses no tools");
    assert!(reqs[1].messages.last().unwrap().text().contains("<summary>"));
    let after = &reqs[2];
    assert_eq!(after.messages.len(), 1, "history replaced by the summary plus the new prompt");
    let t = after.messages[0].text();
    assert!(t.contains("The user asked about X.") && t.contains("second"), "{t}");
    assert!(!t.contains("Continue the work"), "compaction before a new prompt does not tell the model to continue");
    assert!(h.sink.take().iter().any(|ev| matches!(ev, EngineEvent::System { subtype, data } if subtype == "compact_boundary" && data["compact_metadata"]["trigger"] == "auto")));
    // A resume loads only what follows the boundary.
    let path = SessionStore::new(h.dir.path().join("store")).session_path(&h.cwd(), SID).unwrap();
    let loaded = forge_session::LoadedSession::load(&path, None).unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert!(loaded.messages[0].message.text().contains("The user asked about X."));
}

#[tokio::test]
async fn manual_compact_passes_instructions() {
    let h = Harness::new(vec![MockTurn::text("hello"), MockTurn::text("<summary>short</summary>")]);
    let mut e = h.engine();
    e.submit(prompt("hi")).await;
    let info = e.compact(Some("keep the API details")).await.unwrap();
    assert_eq!(info.trigger, "manual");
    assert_eq!(info.summary, "short");
    assert!(h.provider.requests()[1].messages.last().unwrap().text().contains("keep the API details"));
    assert_eq!(e.state.messages.len(), 1);
}

#[tokio::test]
async fn prompt_too_long_compacts_and_retries() {
    let h = Harness::new(vec![
        MockTurn::text("ok").with_usage(usage(100)),
        MockTurn::HttpError {
            status: 400,
            kind: "invalid_request_error".into(),
            message: "prompt is too long: 1000001 tokens > 1000000 maximum".into(),
        },
        MockTurn::text("<summary>compressed</summary>"),
        MockTurn::text("answered after compaction"),
    ]);
    let mut e = h.engine();
    e.submit(prompt("one")).await;
    let r = e.submit(prompt("two")).await;
    assert_eq!(r.result.as_deref(), Some("answered after compaction"));
    let last = h.provider.requests().last().unwrap().clone();
    assert!(last.messages[0].text().contains("Continue the work"), "mid-turn compaction tells the model to continue");
}

fn verifying(commands: &[&str]) -> EngineConfig {
    EngineConfig {
        verify: Some(crate::verify::VerifyConfig {
            commands: commands.iter().map(|c| c.to_string()).collect(),
            max_reminders: 1,
        }),
        ..Default::default()
    }
}

fn verifying_engine(h: &Harness, commands: &[&str]) -> Engine {
    h.engine_with(verifying(commands), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}))
}

fn last_user_text(req: &forge_types::MessagesRequest) -> String {
    req.messages.last().unwrap().content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn verify_reminds_once_when_changes_are_unchecked() {
    let h = Harness::new(vec![]);
    let f = h.cwd().join("a.txt");
    h.provider.push(MockTurn::tool("Write", json!({"file_path": f, "content": "x"})));
    h.provider.push(MockTurn::text("done"));
    h.provider.push(MockTurn::tool("Bash", json!({"command": "true"})));
    h.provider.push(MockTurn::text("verified: `true` passed"));
    let mut e = verifying_engine(&h, &["true"]);
    let r = e.submit(prompt("write a")).await;
    assert_eq!(r.result.as_deref(), Some("verified: `true` passed"));
    assert_eq!(r.num_turns, 4);
    let reminder = last_user_text(&h.provider.requests()[2]);
    assert!(reminder.contains("`a.txt`") && reminder.contains("`true`"), "{reminder}");
    assert!(h.transcript_text().contains(r#""subtype":"verification""#));
}

#[tokio::test]
async fn verify_accepts_a_check_after_the_last_change() {
    let h = Harness::new(vec![]);
    let f = h.cwd().join("a.txt");
    h.provider.push(MockTurn::tool("Write", json!({"file_path": f, "content": "x"})));
    h.provider.push(MockTurn::tool("Bash", json!({"command": "true"})));
    h.provider.push(MockTurn::text("done"));
    let mut e = verifying_engine(&h, &["true"]);
    let r = e.submit(prompt("write a")).await;
    assert_eq!((r.num_turns, r.result.as_deref()), (3, Some("done")));

    // A check before the change is not evidence; the reminder is capped per turn.
    h.provider.push(MockTurn::tool("Bash", json!({"command": "true"})));
    h.provider.push(MockTurn::tool("Write", json!({"file_path": f, "content": "y"})));
    h.provider.push(MockTurn::text("done"));
    h.provider.push(MockTurn::text("not verified: no time"));
    let r = e.submit(prompt("again")).await;
    assert_eq!((r.num_turns, r.result.as_deref()), (4, Some("not verified: no time")));

    // No changes, no reminder; and without the loop configured, none either.
    h.provider.push(MockTurn::text("just talking"));
    assert_eq!(e.submit(prompt("hi")).await.num_turns, 1);
    let mut plain = h.engine();
    h.provider.push(MockTurn::tool("Write", json!({"file_path": f, "content": "z"})));
    h.provider.push(MockTurn::text("done"));
    assert_eq!(plain.submit(prompt("write")).await.num_turns, 2);
}

#[tokio::test]
async fn verify_sees_shell_writes_through_git_and_failed_checks() {
    let h = Harness::new(vec![]);
    let cwd = h.cwd();
    std::fs::create_dir_all(&cwd).unwrap();
    for args in [&["init", "-q"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
        assert!(std::process::Command::new("git").args(args).current_dir(&cwd).status().unwrap().success());
    }
    h.provider.push(MockTurn::tool("Bash", json!({"command": "ls"})));
    h.provider.push(MockTurn::text("looked"));
    let mut e = verifying_engine(&h, &["false"]);
    assert_eq!(e.submit(prompt("look")).await.num_turns, 2, "read-only commands change nothing");

    h.provider.push(MockTurn::tool("Bash", json!({"command": "echo hi > b.txt"})));
    h.provider.push(MockTurn::text("wrote"));
    h.provider.push(MockTurn::text("ok"));
    let r = e.submit(prompt("write via shell")).await;
    assert_eq!(r.num_turns, 3);
    let reminder = last_user_text(&h.provider.requests()[4]);
    assert!(reminder.contains("through shell commands"), "{reminder}");

    // A failed check with nothing after it: the answer must own up to it.
    h.provider.push(MockTurn::tool("Write", json!({"file_path": cwd.join("c.txt"), "content": "x"})));
    h.provider.push(MockTurn::tool("Bash", json!({"command": "false"})));
    h.provider.push(MockTurn::text("done"));
    h.provider.push(MockTurn::text("the check fails"));
    let r = e.submit(prompt("change and check")).await;
    assert_eq!(r.num_turns, 4);
    let n = h.provider.requests().len();
    let reminder = last_user_text(&h.provider.requests()[n - 1]);
    assert!(reminder.contains("The last check (`false`) failed"), "{reminder}");
}

#[tokio::test]
async fn loop_guard_reminds_after_repeated_failures() {
    let h = Harness::new(vec![]);
    for _ in 0..3 {
        h.provider.push(MockTurn::tool("Bash", json!({"command": "exit 3"})));
    }
    h.provider.push(MockTurn::text("giving up"));
    let mut e = h.engine();
    let r = e.submit(prompt("build")).await;
    assert_eq!(r.num_turns, 4);
    let reqs = h.provider.requests();
    assert!(!last_user_text(&reqs[2]).contains("failed 3 times"), "not before the third failure");
    let text = last_user_text(&reqs[3]);
    assert!(text.contains("This exact Bash call has now failed 3 times"), "{text}");
    assert!(h.transcript_text().contains(r#""subtype":"loop_guard""#));
}

#[tokio::test]
async fn max_tokens_continues_text_and_answers_cut_off_calls() {
    use forge_types::StopReason;
    let h = Harness::new(vec![
        MockTurn::blocks(vec![ContentBlock::text("The first half, ")], StopReason::MaxTokens),
        MockTurn::text("and the second half."),
    ]);
    let mut e = h.engine();
    let r = e.submit(prompt("write a lot")).await;
    assert_eq!(r.result.as_deref(), Some("The first half, and the second half."));
    assert_eq!((r.num_turns, r.stop_reason.as_deref()), (2, Some("end_turn")));
    let reqs = h.provider.requests();
    assert_eq!((reqs[0].max_tokens, reqs[1].max_tokens), (32_000, 64_000), "the cap is raised after a cut-off");
    assert!(last_user_text(&reqs[1]).contains("cut off by the output token limit"));

    // A tool call cut off mid-input is answered with an error, not run, and the turn goes on.
    let f = h.cwd().join("big.txt");
    h.provider.push(MockTurn::blocks(
        vec![ContentBlock::ToolUse {
            id: "toolu_cut".into(),
            name: "Write".into(),
            input: json!({ forge_api::TRUNCATED_INPUT: "incomplete JSON" }),
            cache_control: None,
        }],
        StopReason::MaxTokens,
    ));
    h.provider.push(MockTurn::text("will write in parts"));
    let r = e.submit(prompt("write big file")).await;
    assert_eq!(r.num_turns, 2);
    assert!(!f.exists());
    let (_, text, is_error) = tool_results(&e).into_iter().find(|(id, _, _)| id == "toolu_cut").unwrap();
    assert!(is_error && text.contains("smaller steps"), "{text}");
    assert_eq!(h.provider.requests()[2].max_tokens, 32_000, "the raised cap lasts one turn");

    // Without escalation (a user-set cap), continuation still happens at the same cap.
    let h = Harness::new(vec![
        MockTurn::blocks(vec![ContentBlock::text("a")], StopReason::MaxTokens),
        MockTurn::blocks(vec![ContentBlock::text("b")], StopReason::MaxTokens),
        MockTurn::blocks(vec![ContentBlock::text("c")], StopReason::MaxTokens),
        MockTurn::blocks(vec![ContentBlock::text("d")], StopReason::MaxTokens),
    ]);
    let cfg = EngineConfig { escalate_output: false, ..Default::default() };
    let mut e = h.engine_with(cfg, PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    let r = e.submit(prompt("go")).await;
    assert_eq!((r.num_turns, r.result.as_deref(), r.stop_reason.as_deref()), (4, Some("abcd"), Some("max_tokens")));
    assert!(h.provider.requests().iter().all(|q| q.max_tokens == 32_000));
}
