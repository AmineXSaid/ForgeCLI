//! The session task: it owns the [`Driver`] and runs what the UI sends it,
//! one input at a time. Engine events, questions for the person and command
//! answers go back to the UI as [`UiEvent`]s. The loop mirrors the line REPL
//! (`repl.rs`): scheduled tasks fire and finished subtasks are handed back
//! while the session is idle.

use std::sync::Arc;
use std::time::Duration;

use forge_core::commands::picker::picker;
use forge_core::commands::Surface;
use forge_core::{Driver, Flow};
use forge_engine::{EngineEvent, EventSink, PermissionAnswer, PermissionPrompt, PermissionPrompter, TurnResult};
use forge_types::MessageContent;
use tokio::sync::{mpsc, oneshot};

use super::app::{CommandInfo, StatusView, UiEvent};

/// From the UI to the session task.
#[derive(Debug, Clone, PartialEq)]
pub enum ToSession {
    Input(String),
    /// The picker for a command's second step (`/rewind 2`).
    Picker(String),
    Exit,
}

/// Sends every engine event to the UI. The channel is unbounded, so the engine never waits.
pub struct TuiSink(pub mpsc::UnboundedSender<UiEvent>);

impl EventSink for TuiSink {
    fn emit(&self, event: EngineEvent) {
        let _ = self.0.send(UiEvent::Engine(event));
    }
}

/// Asks the person through a dialog and waits for the answer.
pub struct TuiPrompter(pub mpsc::UnboundedSender<UiEvent>);

#[async_trait::async_trait]
impl PermissionPrompter for TuiPrompter {
    async fn ask(&self, prompt: PermissionPrompt) -> PermissionAnswer {
        let gone = || PermissionAnswer::Deny { message: "The UI closed.".into(), interrupt: true };
        let (reply, answer) = oneshot::channel();
        if self.0.send(UiEvent::Ask { prompt, reply }).is_err() {
            return gone();
        }
        answer.await.unwrap_or_else(|_| gone())
    }
}

/// What the status line shows, read from the driver.
pub fn status(d: &Driver) -> StatusView {
    let model = d.handle().model();
    let mode = d.handle().permissions.read().unwrap().mode.as_str().to_string();
    let window = d.engine.context_window(&model);
    let context_pct = (window > 0).then(|| ((d.engine.state.context_tokens * 100 / window).min(100)) as u8);
    StatusView { model, mode, cwd: d.info.cwd.display().to_string(), cost: d.engine.state.total_cost_usd, context_pct }
}

/// The `/` menu's rows: every command the TUI can run.
pub fn commands(d: &Driver) -> Vec<CommandInfo> {
    d.catalog
        .catalog_json(Surface::Tui)
        .iter()
        .map(|c| CommandInfo {
            name: c["name"].as_str().unwrap_or("").to_string(),
            args: c["argumentHint"].as_str().unwrap_or("").to_string(),
            description: c["description"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

/// What a finished turn shows. A model turn's text already streamed, so only
/// local answers and failures are shown here.
pub fn reply_for(r: &TurnResult) -> Option<UiEvent> {
    if let Some(b) = &r.prompt_blocked {
        return Some(UiEvent::Reply { text: format!("Prompt blocked by a hook: {b}"), is_error: true });
    }
    if r.num_turns == 0 && r.stop_reason.is_none() {
        return Some(UiEvent::Reply { text: r.result.clone().unwrap_or_default(), is_error: r.is_error });
    }
    if r.stop_reason.as_deref() == Some("interrupted") {
        return Some(UiEvent::Reply { text: "Interrupted · What should Forge do instead?".into(), is_error: false });
    }
    if r.is_error {
        let text = if r.errors.is_empty() { r.result.clone().unwrap_or_default() } else { r.errors.join("\n") };
        let text = if text.trim().is_empty() { "The turn failed.".to_string() } else { text };
        return Some(UiEvent::Reply { text, is_error: true });
    }
    None
}

/// Run the session until the UI says exit, `/exit` runs, or the UI is gone.
pub async fn run(mut driver: Driver, mut rx: mpsc::UnboundedReceiver<ToSession>, ui: mpsc::UnboundedSender<UiEvent>) {
    let send = ui.clone();
    let mut report = move |r: &TurnResult| {
        if let Some(ev) = reply_for(r) {
            let _ = send.send(ev);
        }
    };
    let _ = ui.send(UiEvent::Commands(commands(&driver)));
    let mut session_id = driver.info.session_id.clone();
    // Changes each time a subtask finishes (C20).
    let mut finished = driver.subtasks.watch();
    let far = Duration::from_secs(365 * 86_400);
    loop {
        // Subtasks that finished during the last turn are handed back before the next input.
        let _ = *finished.borrow_and_update();
        driver.deliver_subtasks();
        if driver.info.session_id != session_id {
            // /clear, /resume, /branch, /cd and the reloads can change the commands.
            session_id = driver.info.session_id.clone();
            let _ = ui.send(UiEvent::Commands(commands(&driver)));
        }
        let _ = ui.send(UiEvent::Status(status(&driver)));
        let _ = ui.send(UiEvent::Idle);
        let wait = driver.next_wait();
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(ToSession::Picker(text)) => match picker(&driver, &text) {
                    Some(p) => {
                        let _ = ui.send(UiEvent::Picker(p));
                    }
                    None => {
                        let _ = ui.send(UiEvent::Reply { text: "Nothing to choose from.".into(), is_error: true });
                    }
                },
                Some(ToSession::Input(text)) => {
                    // A command typed without its choice opens a picker instead.
                    if let Some(p) = picker(&driver, &text) {
                        let _ = ui.send(UiEvent::Picker(p));
                    } else if driver.input(MessageContent::Text(text), &mut report).await == Flow::Exit {
                        let _ = ui.send(UiEvent::Exit);
                        break;
                    }
                }
                Some(ToSession::Exit) | None => break,
            },
            // A scheduled task fires while idle; the UI shows the session busy until it ends.
            _ = tokio::time::sleep(wait.unwrap_or(far)), if wait.is_some() => {
                if driver.task_due() {
                    let _ = ui.send(UiEvent::Busy);
                    driver.run_due(&mut report).await;
                }
            }
            // One finished while idle: the next pass hands it back.
            _ = finished.changed() => {}
        }
    }
    driver.shutdown("prompt_input_exit").await;
}

/// Build the session for the UI, as the line REPL does: launch options, MCP,
/// then a driver whose events and questions go to `ui`.
pub async fn build(
    o: &crate::args::Opts,
    ui: mpsc::UnboundedSender<UiEvent>,
) -> Result<(Driver, Vec<String>), crate::exit::Fail> {
    let mut lo = crate::launch_options(o)?;
    let (mcp, mcp_warnings) = forge_core::connect_mcp(&lo).await;
    lo.mcp = Some(mcp.clone());
    let sink: Arc<dyn EventSink> = Arc::new(TuiSink(ui.clone()));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(TuiPrompter(ui));
    let rebuild = (lo.clone(), sink.clone(), prompter.clone());
    let session = match forge_core::build_session(lo, sink, prompter) {
        Ok(s) => s,
        Err(e) => {
            mcp.shutdown().await;
            return Err(crate::exit::Fail::from(e));
        }
    };
    let warnings: Vec<String> = session.warnings.iter().chain(&mcp_warnings).cloned().collect();
    let mut driver = Driver::new(session, Surface::Tui, Some(mcp));
    driver.set_rebuild(rebuild.0, rebuild.1, rebuild.2);
    Ok((driver, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_api::{MockProvider, MockTurn};
    use forge_config::SettingSource;
    use forge_core::LaunchOptions;
    use forge_types::{ContentBlock, Delta, StreamEvent};
    use serde_json::{json, Value};

    /// The command catalog row as the stream-json `initialize` response has it.
    fn row(c: &CommandInfo) -> Value {
        json!({"name": c.name, "argumentHint": c.args, "description": c.description})
    }

    struct T {
        _dir: tempfile::TempDir,
        p: Arc<MockProvider>,
        to: mpsc::UnboundedSender<ToSession>,
        ui: mpsc::UnboundedReceiver<UiEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    /// A session on a mock provider, running on its own task (as `driver_with` in forge-core's tests).
    fn start(f: impl FnOnce(&mut LaunchOptions)) -> T {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(proj.join(".forge")).unwrap();
        let proj = proj.canonicalize().unwrap();
        let p = Arc::new(MockProvider::new(vec![]));
        let mut o = LaunchOptions {
            cwd: proj,
            provider: Some(p.clone()),
            store_root: Some(dir.path().join("store")),
            setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
            ..Default::default()
        };
        f(&mut o);
        let (ui_tx, ui) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TuiSink(ui_tx.clone()));
        let prompter: Arc<dyn PermissionPrompter> = Arc::new(TuiPrompter(ui_tx.clone()));
        let s = forge_core::build_session(o.clone(), sink.clone(), prompter.clone()).unwrap();
        let mut d = Driver::new(s, Surface::Tui, None);
        d.set_rebuild(o, sink, prompter);
        let (to, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(d, rx, ui_tx));
        T { _dir: dir, p, to, ui, task }
    }

    /// Events up to and including the next `Idle` (or `Exit`).
    async fn until_idle(ui: &mut mpsc::UnboundedReceiver<UiEvent>) -> Vec<UiEvent> {
        let mut out = vec![];
        loop {
            let ev =
                tokio::time::timeout(Duration::from_secs(10), ui.recv()).await.expect("timed out").expect("closed");
            let end = matches!(ev, UiEvent::Idle | UiEvent::Exit);
            out.push(ev);
            if end {
                return out;
            }
        }
    }

    fn streamed_text(evs: &[UiEvent]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                UiEvent::Engine(EngineEvent::Stream {
                    event: StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. },
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn turns_commands_questions_and_exit_go_through_the_channels() {
        let mut t = start(|_| {});
        let first = until_idle(&mut t.ui).await;
        let cmds = first.iter().find_map(|e| match e {
            UiEvent::Commands(c) => Some(c.clone()),
            _ => None,
        });
        let cmds = cmds.expect("the catalog comes first");
        assert!(cmds.iter().any(|c| row(c)
            == json!({"name": "compact", "argumentHint": "[instructions]",
            "description": "Free context by summarizing the conversation so far"})));
        assert!(first.iter().any(|e| matches!(e, UiEvent::Status(s) if s.mode == "default" && !s.model.is_empty())));

        // A text turn: the text arrives as engine events, then Idle; nothing else to show.
        t.p.push(MockTurn::text("Hello there"));
        t.to.send(ToSession::Input("hi".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert_eq!(streamed_text(&evs), "Hello there");
        assert!(!evs.iter().any(|e| matches!(e, UiEvent::Reply { .. })));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Status(s) if s.cost >= 0.0)));

        // A local command answers with a Reply.
        t.to.send(ToSession::Input("/status".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let reply = evs.iter().find_map(|e| match e {
            UiEvent::Reply { text, is_error: false } => Some(text.clone()),
            _ => None,
        });
        assert!(reply.expect("a reply").contains("Model"), "status lists the model");
        t.to.send(ToSession::Input("/nope".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs
            .iter()
            .any(|e| matches!(e, UiEvent::Reply { text, is_error: true } if text.contains("Unknown command"))));

        // A permission prompt arrives as Ask; the oneshot answer reaches the tool.
        t.p.push(MockTurn::tool("Bash", json!({"command": "touch marker && echo forge-tui-ok"})));
        t.p.push(MockTurn::text("done"));
        t.to.send(ToSession::Input("run it".into())).unwrap();
        // Queued while the turn runs: the session takes it after Idle.
        t.p.push(MockTurn::text("second answer"));
        t.to.send(ToSession::Input("and then".into())).unwrap();
        let mut asked = false;
        let mut result = None;
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), t.ui.recv()).await.unwrap().unwrap();
            match ev {
                UiEvent::Ask { prompt, reply } => {
                    assert_eq!(prompt.tool_name, "Bash");
                    asked = true;
                    reply.send(PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] }).unwrap();
                }
                UiEvent::Engine(EngineEvent::User { message, .. }) => {
                    for b in &message.content {
                        if let ContentBlock::ToolResult { content, .. } = b {
                            result = Some(content.to_text());
                        }
                    }
                }
                UiEvent::Idle => break,
                _ => {}
            }
        }
        assert!(asked, "the Bash call asked first");
        assert!(result.unwrap_or_default().contains("forge-tui-ok"), "the allowed command ran");
        let evs = until_idle(&mut t.ui).await;
        assert_eq!(streamed_text(&evs), "second answer", "the queued input ran after Idle");

        // A command without its choice opens a picker; a second step is asked for by text.
        t.to.send(ToSession::Input("/model".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Picker(p) if p.choices.iter().any(|c| c.current))));
        t.to.send(ToSession::Picker("/rewind 1".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Picker(p) if p.choices.len() >= 4)));

        // /exit ends the session with Exit.
        t.to.send(ToSession::Input("/exit".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(matches!(evs.last(), Some(UiEvent::Exit)));
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_closed_ui_denies_questions_and_ends_the_session() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        let p = TuiPrompter(tx);
        let a = p
            .ask(PermissionPrompt {
                tool_name: "Bash".into(),
                tool_use_id: "t".into(),
                input: json!({}),
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            })
            .await;
        assert_eq!(a, PermissionAnswer::Deny { message: "The UI closed.".into(), interrupt: true });

        let mut t = start(|_| {});
        until_idle(&mut t.ui).await;
        drop(t.to);
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[test]
    fn replies_for_local_answers_failures_and_blocked_prompts() {
        let base = TurnResult {
            subtype: forge_types::sdk::ResultSubtype::Success,
            is_error: false,
            result: Some("ok".into()),
            stop_reason: None,
            num_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            usage: Default::default(),
            total_cost_usd: 0.0,
            model_usage: Default::default(),
            permission_denials: vec![],
            errors: vec![],
            structured_output: None,
            prompt_blocked: None,
            fatal: false,
            tool_calls: 0,
        };
        assert!(matches!(reply_for(&base), Some(UiEvent::Reply { text, is_error: false }) if text == "ok"));
        let model = TurnResult { num_turns: 1, stop_reason: Some("end_turn".into()), ..base.clone() };
        assert!(reply_for(&model).is_none(), "streamed already");
        let failed = TurnResult { is_error: true, errors: vec!["overloaded".into()], ..model.clone() };
        assert!(matches!(reply_for(&failed), Some(UiEvent::Reply { text, is_error: true }) if text == "overloaded"));
        let stopped = TurnResult { is_error: true, stop_reason: Some("interrupted".into()), ..model.clone() };
        assert!(
            matches!(reply_for(&stopped), Some(UiEvent::Reply { text, is_error: false }) if text.starts_with("Interrupted"))
        );
        let blocked = TurnResult { prompt_blocked: Some("no secrets".into()), ..model };
        assert!(matches!(reply_for(&blocked), Some(UiEvent::Reply { is_error: true, .. })));
    }
}
