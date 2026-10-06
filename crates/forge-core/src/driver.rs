//! The session driver: what every front end (print mode, stream-json, the line
//! REPL, the TUI) runs inputs through. It owns the engine and the command
//! catalog. A message is one of:
//! - a slash command, handled by [`crate::commands::execute`];
//! - `!command`, run in the shell (shell mode);
//! - a turn for the model.
//!
//! After a model turn, an active `/goal` is checked, and the driver keeps
//! starting turns until it is met (contract C18). Every turn's result goes
//! to the front end as it finishes.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use forge_config::LoadedSettings;
use forge_engine::{Engine, EngineHandle, EventSink, NoticeLevel, PermissionPrompter, TurnResult};
use forge_session::{Entry, FileHistory, LoadedSession};
use forge_types::sdk::{InitInfo, ResultSubtype};
use forge_types::MessageContent;

use crate::commands::{self, Catalog, Surface};
use crate::goal::{self, Goal, Status, Verdict};
use crate::{build_session, LaunchOptions, PromptSpec, Resume, Session};

/// Facts about the session for `/status`, `/doctor` and friends.
pub struct SessionInfo {
    pub session_id: String,
    pub init: InitInfo,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub cwd: PathBuf,
    /// The user settings file commands save defaults to.
    pub user_settings: PathBuf,
}

/// Activity totals across this process's inputs (for `/usage`).
#[derive(Debug, Default, Clone)]
pub struct Activity {
    pub turns: u32,
    pub api_time: Duration,
    pub prompts: u32,
}

/// The session a front end is talking to. It stays valid when the driver
/// switches sessions (`/clear`, `/resume`, `/branch`, `/cd`), so Ctrl-C and
/// SDK control requests always reach the current engine.
#[derive(Clone)]
pub struct Live(Arc<RwLock<(EngineHandle, Arc<FileHistory>, String)>>);

impl Live {
    fn new(engine: &Engine, session_id: &str) -> Self {
        Live(Arc::new(RwLock::new((engine.handle(), engine.history().clone(), session_id.to_string()))))
    }

    fn set(&self, engine: &Engine, session_id: &str) {
        *self.0.write().unwrap() = (engine.handle(), engine.history().clone(), session_id.to_string());
    }

    pub fn handle(&self) -> EngineHandle {
        self.0.read().unwrap().0.clone()
    }

    pub fn history(&self) -> Arc<FileHistory> {
        self.0.read().unwrap().1.clone()
    }

    pub fn session_id(&self) -> String {
        self.0.read().unwrap().2.clone()
    }
}

/// What the driver needs to build another session (switching sessions).
#[derive(Clone)]
pub struct Rebuild {
    pub opts: LaunchOptions,
    pub sink: Arc<dyn EventSink>,
    pub prompter: Arc<dyn PermissionPrompter>,
}

/// Where a session switch goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Switch {
    /// A new, empty session (`/clear`).
    New,
    /// A saved session (`/resume`).
    Resume(String),
    /// A copy of this conversation under a new id (`/branch`).
    Branch,
    /// A copy of this conversation in another directory (`/cd`).
    Cd(PathBuf),
    /// This session again, with commands, skills, plugins and settings re-read.
    Reload,
}

/// `/btw` exchanges kept as context for the next side question.
pub const MAX_SIDE_QUESTIONS: usize = 20;

pub struct Driver {
    pub engine: Engine,
    pub catalog: Catalog,
    pub info: SessionInfo,
    pub prompt: PromptSpec,
    pub surface: Surface,
    pub started: Instant,
    pub activity: Activity,
    /// The session's `/goal`, while one is set (and the last one, after it ends).
    pub goal: Option<Goal>,
    /// Earlier `/btw` questions and answers, oldest first.
    pub side_questions: Vec<(String, String)>,
    live: Live,
    rebuild: Option<Rebuild>,
    /// Scheduled prompts (`None` when FORGE_DISABLE_CRON is set).
    pub scheduler: Option<crate::schedule_tools::SharedScheduler>,
    /// The self-paced `/loop` iteration in progress, if any.
    pub(crate) self_paced: Option<SelfPaced>,
    /// The pending wakeup is a fallback check.
    pub(crate) fallback_pending: bool,
}

/// Book-keeping for a self-paced loop: did the iteration reschedule or stop?
#[derive(Debug, Clone)]
pub(crate) struct SelfPaced {
    /// The `/loop` input, for the fallback wakeup.
    pub input: String,
    /// The scheduler's wakeup counter when the iteration started.
    pub mark: u64,
    /// This iteration is the fallback check: if it doesn't reschedule, the loop ends.
    pub fallback: bool,
}

/// What to do after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// `/exit`: end the session.
    Exit,
}

/// Receives each turn's result as it finishes (one input can run several
/// turns, when a goal is set). A local command's answer is a result with
/// `num_turns == 0` and no stop reason.
pub type Report<'a> = dyn FnMut(&TurnResult) + Send + 'a;

/// Longest `!command` output kept, in characters.
const SHELL_OUTPUT_CHARS: usize = 30_000;
const SHELL_TIMEOUT: Duration = Duration::from_secs(120);

impl Driver {
    pub fn new(s: Session, surface: Surface, mcp: Option<Arc<forge_mcp::McpManager>>) -> Self {
        let cwd = s.engine.tool_ctx().project_dir.clone();
        let cost = s.engine.state.total_cost_usd;
        let goal = s.resumed.as_ref().and_then(|r| r.goal.as_ref()).and_then(|g| Goal::restore(g, cost));
        let live = Live::new(&s.engine, &s.session_id);
        Driver {
            engine: s.engine,
            catalog: Catalog {
                commands: s.commands,
                skills: s.skills,
                styles: s.styles,
                plugins: s.plugins,
                agents: s.agents,
                mcp,
            },
            info: SessionInfo {
                session_id: s.session_id,
                init: s.init,
                settings: s.settings,
                warnings: s.warnings,
                cwd,
                user_settings: forge_config::forge_home().join("settings.json"),
            },
            prompt: s.prompt,
            surface,
            started: Instant::now(),
            activity: Activity::default(),
            goal,
            side_questions: vec![],
            live,
            rebuild: None,
            scheduler: s.scheduler,
            self_paced: None,
            fallback_pending: false,
        }
    }

    pub fn handle(&self) -> EngineHandle {
        self.engine.handle()
    }

    /// The current session, for Ctrl-C handlers and SDK control requests.
    pub fn live(&self) -> Live {
        self.live.clone()
    }

    /// Let the driver build other sessions: `opts` as the session was launched.
    pub fn set_rebuild(
        &mut self,
        opts: LaunchOptions,
        sink: Arc<dyn EventSink>,
        prompter: Arc<dyn PermissionPrompter>,
    ) {
        self.rebuild = Some(Rebuild { opts, sink, prompter });
    }

    pub fn can_switch(&self) -> bool {
        self.rebuild.is_some()
    }

    /// Where sessions are stored, when the launch options set it.
    pub(crate) fn rebuild_store(&self) -> Option<PathBuf> {
        self.rebuild.as_ref().and_then(|r| r.opts.store_root.clone())
    }

    /// The conversation in memory, as a loaded session.
    pub fn snapshot(&self) -> LoadedSession {
        let st = &self.engine.state;
        let t = self.engine.transcript();
        LoadedSession {
            session_id: self.info.session_id.clone(),
            cwd: self.info.cwd.clone(),
            messages: st
                .uuids
                .iter()
                .zip(&st.messages)
                .map(|(u, m)| Entry { uuid: u.clone(), message: m.clone(), is_meta: false })
                .collect(),
            additional_dirs: self.prompt.env.additional_dirs.clone(),
            microcompacted: st.microcompacted.clone(),
            title: t.title(),
            last_uuid: t.last_uuid(),
            permission_mode: Some(self.engine.handle().permissions.read().unwrap().mode.as_str().to_string()),
            model: Some(self.engine.handle().model()),
            worktree: None,
            todos: Some(self.engine.tool_ctx().todos.lock().unwrap().clone()).filter(|t| !t.is_empty()),
            goal: self.goal.as_ref().map(Goal::record),
            schedule: self.scheduler.as_ref().map(|s| s.lock().unwrap().record()),
        }
    }

    /// Replace the session: a new one, a saved one, a branch of this one, or
    /// this one rebuilt. MCP connections, the running model and mode, the cost
    /// so far and the front end's [`Live`] carry over.
    pub async fn switch(&mut self, to: Switch) -> Result<(), String> {
        let rb = self.rebuild.clone().ok_or("this front end can't switch sessions")?;
        let mut opts = rb.opts.clone();
        opts.cwd = self.info.cwd.clone();
        opts.worktree = None;
        opts.mcp = self.catalog.mcp.clone();
        opts.session_id = None;
        opts.fork_session = false;
        let rt = self.engine.handle().runtime();
        opts.model = Some(rt.model.clone());
        opts.effort = rt.effort.clone();
        let mode = self.engine.handle().permissions.read().unwrap().mode;
        opts.permission_mode = Some(mode.as_str().to_string());
        let (source, ends) = match &to {
            Switch::New => {
                opts.resume = Resume::New;
                ("clear", Some("clear"))
            }
            Switch::Resume(id) => {
                opts.resume = Resume::Id(id.clone());
                ("resume", Some("resume"))
            }
            Switch::Branch => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                opts.fork_session = true;
                ("resume", Some("other"))
            }
            Switch::Cd(dir) => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                opts.fork_session = true;
                opts.cwd = dir.clone();
                ("resume", Some("other"))
            }
            Switch::Reload => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                ("resume", None)
            }
        };
        let s = build_session(opts, rb.sink.clone(), rb.prompter.clone()).map_err(|e| e.to_string())?;
        if let Some(reason) = ends {
            self.engine.end_session(reason).await;
        }
        let mut next = Driver::new(s, self.surface, self.catalog.mcp.clone());
        next.engine.set_start_source(source);
        // Carry over what the person set in this process.
        let h = next.engine.handle();
        h.set_fast(rt.fast);
        h.set_max_thinking_tokens(rt.max_thinking_tokens);
        let old = &self.engine.state;
        let st = &mut next.engine.state;
        st.total_cost_usd += old.total_cost_usd;
        st.total_usage.add(&old.total_usage);
        for (model, usage) in &old.model_usage {
            let e = st.model_usage.entry(model.clone()).or_insert_with(|| serde_json::json!({}));
            for (k, v) in usage.as_object().into_iter().flatten() {
                let sum = e.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0) + v.as_f64().unwrap_or(0.0);
                e[k] = if v.is_u64() { serde_json::json!(sum as u64) } else { serde_json::json!(sum) };
            }
        }
        if next.info.cwd == self.info.cwd {
            next.prompt.output_style = self.prompt.output_style.clone();
            next.info.init.output_style = self.info.init.output_style.clone();
        }
        next.prompt.replace = self.prompt.replace.clone();
        next.prompt.append = self.prompt.append.clone();
        next.rebuild_system();
        next.info.user_settings = self.info.user_settings.clone();
        next.activity = self.activity.clone();
        next.started = self.started;
        next.rebuild = self.rebuild.take();
        next.live = self.live.clone();
        next.live.set(&next.engine, &next.info.session_id);
        *self = next;
        Ok(())
    }

    /// Run one input. `report` gets each turn's result as it finishes.
    pub async fn input(&mut self, content: MessageContent, report: &mut Report<'_>) -> Flow {
        let result = if let Some(cmd) = shell_command(&content) {
            self.shell(&cmd).await
        } else {
            match commands::command_text(&content) {
                None => self.engine.submit(content).await,
                Some(text) => match commands::execute(self, &text).await {
                    commands::Exec::Submit(prompt) => self.engine.submit(prompt).await,
                    commands::Exec::Local { text, is_error } => self.engine.local_result(text, is_error),
                    commands::Exec::Exit => return Flow::Exit,
                },
            }
        };
        self.record(&result, true);
        report(&result);
        if result.num_turns > 0 {
            // A prompt that reached the model resumes a paused goal.
            if let Some(g) = self.goal.as_mut().filter(|g| g.is_active()) {
                g.paused = None;
                g.idle = 0;
            }
            let interrupted = result.stop_reason.as_deref() == Some("interrupted");
            self.pursue_goal(result, report).await;
            self.after_loop_iteration(interrupted);
        }
        Flow::Continue
    }

    /// Real time until the next scheduled task is due.
    pub fn next_wait(&self) -> Option<Duration> {
        self.scheduler.as_ref().and_then(|s| s.lock().unwrap().next_wait())
    }

    /// Are scheduled tasks waiting? (`-p` keeps running while they are.)
    pub fn has_pending(&self) -> bool {
        self.scheduler.as_ref().is_some_and(|s| !s.lock().unwrap().tasks.is_empty())
    }

    pub(crate) fn save_schedule(&self) {
        if let Some(s) = &self.scheduler {
            self.engine.transcript().set_schedule(s.lock().unwrap().record());
        }
    }

    /// Run the next due scheduled task, if any, as a turn. Call it only while idle.
    pub async fn run_due(&mut self, report: &mut Report<'_>) -> bool {
        let Some(task) = self.scheduler.as_ref().and_then(|s| s.lock().unwrap().take_due()) else { return false };
        self.save_schedule();
        self.engine.announce(
            "scheduled_task",
            serde_json::json!({"id": task.id, "prompt": task.prompt, "schedule": task.describe()}),
        );
        self.notice(NoticeLevel::Info, format!("Running scheduled task {}: {}", task.id, task.prompt));
        let result = if task.prompt.trim_start().starts_with("/loop") {
            match commands::execute(self, task.prompt.trim()).await {
                commands::Exec::Submit(p) => self.engine.submit(p).await,
                commands::Exec::Local { text, is_error } => self.engine.local_result(text, is_error),
                commands::Exec::Exit => return true,
            }
        } else {
            let prompt = commands::scheduled_prompt(self, &task.prompt).await;
            self.engine.submit(prompt).await
        };
        self.record(&result, false);
        report(&result);
        if result.num_turns > 0 {
            let interrupted = result.stop_reason.as_deref() == Some("interrupted");
            self.pursue_goal(result, report).await;
            self.after_loop_iteration(interrupted);
        }
        true
    }

    /// After a self-paced iteration: it continues if the model called
    /// ScheduleWakeup. If it did neither that nor stop, one fallback check
    /// comes 20 minutes later; a second miss ends the loop.
    fn after_loop_iteration(&mut self, interrupted: bool) {
        let (Some(it), Some(sched)) = (self.self_paced.take(), self.scheduler.clone()) else { return };
        let mut s = sched.lock().unwrap();
        // Ctrl-C ends a self-paced loop.
        if interrupted {
            s.stop_wakeups();
            drop(s);
            self.save_schedule();
            self.notice(NoticeLevel::Info, "The loop is stopped.");
            return;
        }
        if s.wakeups != it.mark {
            if s.stopped {
                drop(s);
                self.notice(NoticeLevel::Info, "The loop is finished.");
            }
            return;
        }
        if it.fallback {
            drop(s);
            self.notice(
                NoticeLevel::Warning,
                "The loop ended: two iterations in a row neither rescheduled nor stopped.",
            );
            return;
        }
        let secs = crate::schedule::FALLBACK_WAKEUP.as_secs();
        let input = format!("/loop {}", it.input);
        let fallback = s.wakeup(secs, &input, "fallback check: the last iteration didn't reschedule").is_ok();
        drop(s);
        if fallback {
            self.save_schedule();
            self.fallback_pending = true;
        }
    }

    fn record(&mut self, r: &TurnResult, prompt: bool) {
        if r.num_turns > 0 {
            self.activity.prompts += u32::from(prompt);
            self.activity.turns += r.num_turns;
            self.activity.api_time += Duration::from_millis(r.duration_api_ms);
        }
    }

    fn notice(&self, level: NoticeLevel, text: impl Into<String>) {
        self.engine.emit(forge_engine::EngineEvent::Notice { level, text: text.into() });
    }

    /// Record the goal and tell the host (`system/goal`).
    pub(crate) fn goal_changed(&self) {
        if let Some(g) = &self.goal {
            let rec = g.record();
            self.engine.transcript().set_goal(rec.clone());
            self.engine.announce("goal", rec);
        }
    }

    /// Keep working on an active goal: check each finished turn, and start
    /// another while the goal isn't met.
    async fn pursue_goal(&mut self, mut last: TurnResult, report: &mut Report<'_>) {
        let max_turns = self.engine.cfg.max_turns;
        let mut used = last.num_turns;
        loop {
            let Some(g) = self.goal.as_ref() else { return };
            if !g.is_active() || g.paused.is_some() {
                return;
            }
            let condition = g.condition.clone();
            // Errors and interrupts stop the loop; some end the goal.
            if last.stop_reason.as_deref() == Some("interrupted") {
                self.pause("interrupted", "Goal paused: the turn was interrupted. Send a message to continue.");
                return;
            }
            if last.prompt_blocked.is_some() {
                self.pause("prompt blocked", "Goal paused: a hook blocked the prompt.");
                return;
            }
            if last.is_error {
                let why = last.errors.first().cloned().or_else(|| last.result.clone()).unwrap_or_default();
                if last.fatal {
                    if let Some(g) = self.goal.as_mut() {
                        g.status = Status::Cleared;
                        g.last_reason = Some(why.clone());
                    }
                    self.goal_changed();
                    self.notice(
                        NoticeLevel::Error,
                        format!("Goal cleared after an error that won't go away by itself ({why}). Run /goal again to continue."),
                    );
                } else if matches!(last.subtype, ResultSubtype::ErrorMaxTurns | ResultSubtype::ErrorMaxBudgetUsd) {
                    self.pause("limit", "Goal paused: a turn or budget limit was reached.");
                } else {
                    self.pause("error", format!("Goal paused after an error: {why}. Send a message to continue."));
                }
                return;
            }
            // A turn with one model call used no tools.
            if let Some(g) = self.goal.as_mut() {
                g.idle = if last.num_turns <= 1 { g.idle + 1 } else { 0 };
            }

            let transcript = goal::evaluator_transcript(&self.engine.state.messages);
            let user = format!("Goal: {condition}\n\nTranscript:\n{transcript}");
            let answer = commands::side_request(self, goal::EVALUATOR_PROMPT, user, 400).await;
            let (verdict, reason) = match answer {
                Ok(text) => goal::parse_verdict(&text),
                Err(e) => {
                    self.pause("check failed", format!("Goal paused: the goal check failed ({e})."));
                    return;
                }
            };
            let g = self.goal.as_mut().expect("goal checked above");
            g.checks += 1;
            g.last_reason = Some(reason.clone());
            match verdict {
                Verdict::Met => {
                    g.status = Status::Achieved;
                    self.goal_changed();
                    self.notice(NoticeLevel::Info, format!("Goal achieved: {reason}"));
                    return;
                }
                Verdict::Impossible => {
                    g.status = Status::Failed(reason.clone());
                    self.goal_changed();
                    self.notice(NoticeLevel::Warning, format!("Goal can't be met: {reason}"));
                    return;
                }
                Verdict::NotMet => {
                    if g.idle >= goal::MAX_IDLE_TURNS {
                        self.pause(
                            "no progress",
                            format!(
                                "Goal paused: {} turns in a row without using a tool. The goal stays set; send a \
                                 message to continue.",
                                goal::MAX_IDLE_TURNS
                            ),
                        );
                        return;
                    }
                    if max_turns.is_some_and(|m| used >= m) {
                        self.pause("limit", "Goal paused: --max-turns reached.");
                        return;
                    }
                    self.engine.announce(
                        "goal",
                        serde_json::json!({"condition": condition, "status": "active", "reason": reason}),
                    );
                    let next =
                        self.engine.submit(MessageContent::Text(goal::continue_prompt(&condition, &reason))).await;
                    used += next.num_turns;
                    self.record(&next, false);
                    report(&next);
                    last = next;
                }
            }
        }
    }

    fn pause(&mut self, why: &str, text: impl Into<String>) {
        if let Some(g) = self.goal.as_mut() {
            g.paused = Some(why.to_string());
        }
        self.notice(NoticeLevel::Warning, text);
    }

    /// `!command`: run it in the shell, as the user, and put the command and
    /// its output in the conversation. The model answers it unless
    /// `respondToBashCommands` is false.
    async fn shell(&mut self, cmd: &str) -> TurnResult {
        let ctx = self.engine.tool_ctx().clone();
        let shell = if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
        let mut c = tokio::process::Command::new(shell);
        c.arg("-c").arg(cmd).current_dir(ctx.shell_cwd()).envs(ctx.env.iter()).kill_on_drop(true);
        c.stdin(std::process::Stdio::null());
        let (code, output) = match tokio::time::timeout(SHELL_TIMEOUT, c.output()).await {
            Ok(Ok(out)) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                let err = String::from_utf8_lossy(&out.stderr);
                if !err.trim().is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&err);
                }
                (out.status.code().unwrap_or(-1), text)
            }
            Ok(Err(e)) => (-1, format!("could not run {shell}: {e}")),
            Err(_) => (-1, format!("timed out after {}s", SHELL_TIMEOUT.as_secs())),
        };
        let output = forge_tools::truncate_middle(output.trim_end(), SHELL_OUTPUT_CHARS);
        let note = format!(
            "The user ran a shell command:\n<shell-command>{cmd}</shell-command>\n<shell-output exit-code=\"{code}\">\n{output}\n</shell-output>"
        );
        if self.info.settings.bool("/respondToBashCommands") == Some(false) {
            self.engine.add_user_note(note);
            return self.engine.local_result(output, code != 0);
        }
        if !output.is_empty() {
            self.notice(NoticeLevel::Info, format!("$ {cmd}\n{output}"));
        }
        self.engine.submit(MessageContent::Text(note)).await
    }

    /// Rebuild the system prompt from [`Driver::prompt`] after changing it.
    pub fn rebuild_system(&mut self) {
        self.engine.set_system(self.prompt.build().0);
    }

    /// An SDK host's `initialize` `systemPrompt` / `appendSystemPrompt`.
    pub fn set_system_prompt(&mut self, replace: Option<String>, append: Option<String>) {
        if replace.is_some() {
            self.prompt.replace = replace;
        }
        if append.is_some() {
            self.prompt.append = append;
        }
        self.rebuild_system();
    }

    pub async fn shutdown(&self, reason: &str) {
        self.engine.end_session(reason).await;
        if let Some(m) = &self.catalog.mcp {
            m.shutdown().await;
        }
    }
}

/// The command in a `!command` message.
fn shell_command(content: &MessageContent) -> Option<String> {
    let text = commands::command_text_any(content)?;
    let cmd = text.trim_start().strip_prefix('!')?.trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}
