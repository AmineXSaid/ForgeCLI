use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use forge_api::models::{model_info, ModelInfo};
use forge_api::{ApiError, MessageAccumulator, Provider};
use forge_hooks::{Exit2Effect, HookEvent, HookRunner};
use forge_permissions::Engine as PermEngine;
use forge_session::{FileHistory, Transcript};
use forge_tools::{ToolContext, ToolRegistry};
use forge_types::sdk::{PermissionDenial, ResultSubtype};
use forge_types::{
    ApiMessage, ContentBlock, Message, MessageContent, MessagesRequest, StopReason, StreamEvent, SystemBlock, Usage,
};
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::events::{EngineEvent, EventSink, NoticeLevel, PermissionPrompter};
use crate::request::{apply_cache_breakpoints, normalize};
use crate::snapshot::{EngineSnapshot, RequestParts, TurnProgress};
use crate::{INTERRUPT_MARKER, INTERRUPT_MARKER_TOOLS};

/// USD per million tokens (settings `modelPricing` override).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Pricing {
    fn cost(&self, u: &Usage) -> f64 {
        (u.input_tokens as f64 * self.input
            + u.output_tokens as f64 * self.output
            + u.cache_read_input_tokens as f64 * self.cache_read
            + u.cache_creation_input_tokens as f64 * self.cache_write)
            / 1_000_000.0
    }

    fn from_model(m: &ModelInfo) -> Self {
        Pricing {
            input: m.input_price,
            output: m.output_price,
            cache_read: m.cache_read_price,
            cache_write: m.cache_write_price(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub model: String,
    pub fallback_models: Vec<String>,
    /// Output cap per request (default 32,000, capped by the model).
    pub max_output_tokens: u32,
    pub effort: Option<String>,
    /// `None` = model default, `Some(0)` = off, `Some(n)` = on / budget.
    pub max_thinking_tokens: Option<u32>,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    /// `--json-schema`: structured output for the final answer.
    pub json_schema: Option<Value>,
    pub pricing: HashMap<String, Pricing>,
    /// Text prepended (as a system reminder) to the first user message of a new session.
    pub initial_context: Option<String>,
    pub metadata_user_id: Option<String>,
    /// Context window used for compaction thresholds (`--autocompact`); default: the model's window.
    pub autocompact_window: Option<u64>,
    /// Summarize automatically when the window is nearly full (contract C9).
    pub auto_compact: bool,
    /// Runs as a sub-agent: SubagentStop instead of Stop, no session hooks.
    pub is_subagent: bool,
    /// The verification loop; `None` turns it off. Never runs in sub-agents.
    pub verify: Option<crate::verify::VerifyConfig>,
    /// After a `max_tokens` stop, raise the output cap to [`ESCALATED_OUTPUT_TOKENS`]
    /// for the rest of the turn (off when the user set the cap).
    pub escalate_output: bool,
    /// Start in fast mode (`fastMode` setting).
    pub fast: bool,
}

/// The output cap after a `max_tokens` stop (capped by the model).
pub const ESCALATED_OUTPUT_TOKENS: u32 = 64_000;
/// Times one turn may continue a reply cut off by `max_tokens`.
const MAX_CONTINUATIONS: u32 = 3;
const CONTINUE_PROMPT: &str = "<system-reminder>\nYour reply was cut off by the output token limit. Continue \
                               exactly where it stopped: no apology, no recap, and do not repeat what you already \
                               wrote.\n</system-reminder>";

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            model: forge_api::models::DEFAULT_MODEL.into(),
            fallback_models: vec![],
            max_output_tokens: 32_000,
            effort: None,
            max_thinking_tokens: None,
            max_turns: None,
            max_budget_usd: None,
            json_schema: None,
            pricing: HashMap::new(),
            initial_context: None,
            metadata_user_id: None,
            autocompact_window: None,
            auto_compact: true,
            is_subagent: false,
            verify: None,
            escalate_output: true,
            fast: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "--max-budget-usd needs pricing for model {0}, which ForgeCLI does not know. Add it under \"modelPricing\" in settings (input/output/cacheRead/cacheWrite USD per million tokens) or remove the budget."
    )]
    UnknownPricing(String),
}

/// The beta flag fast mode needs.
pub const FAST_MODE_BETA: &str = "fast-mode-2026-02-01";
/// Fast mode's price over the model's standard rates (input and output alike).
pub const FAST_PRICE_MULTIPLIER: f64 = 2.0;

/// Settings a host may change while the engine runs.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub model: String,
    pub max_thinking_tokens: Option<u32>,
    pub effort: Option<String>,
    /// Fast mode (`/fast`): faster output at a higher price, where the model supports it.
    pub fast: bool,
}

/// A cloneable handle for controlling a running engine (interrupt, mode, model).
#[derive(Clone)]
pub struct EngineHandle {
    cancel: Arc<Mutex<CancellationToken>>,
    pub permissions: Arc<RwLock<PermEngine>>,
    pub runtime: Arc<RwLock<Runtime>>,
    transcript: Arc<Transcript>,
    /// The engine's state as last published, replaced whole (C17, immediate commands).
    snapshot: Arc<RwLock<Arc<EngineSnapshot>>>,
}

impl EngineHandle {
    /// Interrupt the running turn (contract C3).
    pub fn interrupt(&self) {
        self.cancel.lock().unwrap().cancel();
    }

    /// Start interruptible work outside a turn (a `!command`): Ctrl-C and a
    /// host's `interrupt` cancel the returned token until the next turn starts.
    pub fn new_token(&self) -> CancellationToken {
        let t = CancellationToken::new();
        *self.cancel.lock().unwrap() = t.clone();
        t
    }

    /// The current turn's token. [`EngineHandle::interrupt`] cancels it until the
    /// next turn starts, so work between turns (a goal check) can watch it too.
    pub fn turn_token(&self) -> CancellationToken {
        self.cancel.lock().unwrap().clone()
    }

    pub fn set_permission_mode(&self, mode: forge_permissions::PermissionMode) {
        self.permissions.write().unwrap().mode = mode;
        self.transcript.append_system("permission_mode", json!({"mode": mode.as_str()}));
    }

    pub fn set_model(&self, model: &str) {
        self.runtime.write().unwrap().model = forge_api::resolve_model(model);
    }

    pub fn set_max_thinking_tokens(&self, n: Option<u32>) {
        self.runtime.write().unwrap().max_thinking_tokens = n;
    }

    pub fn set_effort(&self, effort: Option<String>) {
        self.runtime.write().unwrap().effort = effort;
    }

    pub fn set_fast(&self, on: bool) {
        self.runtime.write().unwrap().fast = on;
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.read().unwrap().clone()
    }

    pub fn model(&self) -> String {
        self.runtime.read().unwrap().model.clone()
    }

    /// The engine's state as last published ([`Engine::publish`]). The lock is held only to
    /// clone the `Arc`.
    pub fn snapshot(&self) -> Arc<EngineSnapshot> {
        self.snapshot.read().unwrap().clone()
    }
}

/// The parts an engine is assembled from (built by `forge-core`).
pub struct EngineParts {
    pub provider: Arc<dyn Provider>,
    pub tools: ToolRegistry,
    pub tool_ctx: ToolContext,
    pub permissions: PermEngine,
    pub hooks: HookRunner,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub sink: Arc<dyn EventSink>,
    pub transcript: Arc<Transcript>,
    pub history: Arc<FileHistory>,
    pub system: Vec<SystemBlock>,
}

/// Persists an accepted `PermissionUpdate` (SDK shape) to settings.
pub type PermissionUpdateHandler = Box<dyn Fn(&Value) + Send + Sync>;

/// State shared with concurrently running tool pipelines.
pub(crate) struct Shared {
    pub provider: Arc<dyn Provider>,
    pub tools: ToolRegistry,
    pub tool_ctx: ToolContext,
    pub permissions: Arc<RwLock<PermEngine>>,
    pub hooks: HookRunner,
    pub prompter: Arc<dyn PermissionPrompter>,
    /// Serializes permission prompts (contract C2).
    pub prompt_lock: tokio::sync::Mutex<()>,
    pub sink: Arc<dyn EventSink>,
    pub transcript: Arc<Transcript>,
    pub history: Arc<FileHistory>,
    /// Persists accepted permission updates (`updatedPermissions`) to settings.
    pub on_permission_update: Mutex<Option<PermissionUpdateHandler>>,
}

/// Conversation state that persists across turns.
#[derive(Debug, Default, Clone)]
pub struct TurnState {
    pub messages: Vec<Message>,
    /// Transcript uuid of each message (same index as `messages`).
    pub uuids: Vec<String>,
    /// Uuids of synthetic user messages (reminders, interrupt markers, summaries).
    pub meta: HashSet<String>,
    pub microcompacted: HashSet<String>,
    pub total_usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    pub context_tokens: u64,
}

/// Outcome of one [`Engine::submit`].
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub subtype: ResultSubtype,
    pub is_error: bool,
    pub result: Option<String>,
    pub stop_reason: Option<String>,
    pub num_turns: u32,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    pub permission_denials: Vec<PermissionDenial>,
    pub errors: Vec<String>,
    pub structured_output: Option<Value>,
    /// The user's prompt was blocked by a UserPromptSubmit hook (shown to the user, not the model).
    pub prompt_blocked: Option<String>,
    /// The turn failed in a way the next turn would too: credentials, billing,
    /// a missing model, or a conversation too long even after compaction.
    pub fatal: bool,
    /// The endpoint refused the credentials (401/403) or none were set: print
    /// mode exits with the configuration status.
    pub auth_failed: bool,
    /// Tool calls the model made in this turn.
    pub tool_calls: u32,
}

pub struct Engine {
    pub cfg: EngineConfig,
    pub(crate) shared: Arc<Shared>,
    pub state: TurnState,
    handle: EngineHandle,
    system: Vec<SystemBlock>,
    session_started: bool,
    /// Memory and environment context, re-attached after compaction.
    session_context: Option<String>,
    verify: crate::verify::Tracker,
    guard: crate::stuck::LoopGuard,
    /// This turn's raised output cap, after a `max_tokens` stop.
    output_cap: Option<u32>,
    /// After `/clear`: attach the memory context to the next prompt again.
    reattach_context: bool,
    /// Notes for the model, attached to the next prompt (`/add-dir`, ...).
    reminders: Vec<String>,
    /// SessionStart's `source`: startup, resume or clear.
    start_source: String,
}

/// A person's prompt in the conversation: a point `/rewind` can go back to.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptPoint {
    /// Index in `TurnState::messages`.
    pub index: usize,
    /// Transcript uuid (the file-history turn id).
    pub uuid: String,
    pub text: String,
    /// Files changed at or after this prompt.
    pub changed_files: Vec<std::path::PathBuf>,
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactInfo {
    pub trigger: String,
    pub pre_tokens: u64,
    pub summary: String,
}

/// Per-turn accounting.
#[derive(Default)]
pub(crate) struct TurnAcc {
    pub api_calls: u32,
    pub tool_calls: u32,
    pub api_ms: u64,
    pub usage: Usage,
    pub denials: Vec<PermissionDenial>,
    pub errors: Vec<String>,
    pub stop: Option<String>,
}

enum StreamOutcome {
    Done(ApiMessage),
    Interrupted(Option<ApiMessage>),
    Failed(ApiError),
}

impl Engine {
    pub fn new(cfg: EngineConfig, parts: EngineParts) -> Result<Self, EngineError> {
        if cfg.max_budget_usd.is_some() {
            for m in std::iter::once(&cfg.model).chain(cfg.fallback_models.iter()) {
                if model_info(m).is_none() && !cfg.pricing.contains_key(m) {
                    return Err(EngineError::UnknownPricing(m.clone()));
                }
            }
        }
        let runtime = Runtime {
            model: cfg.model.clone(),
            max_thinking_tokens: cfg.max_thinking_tokens,
            effort: cfg.effort.clone(),
            fast: cfg.fast,
        };
        let permissions = Arc::new(RwLock::new(parts.permissions));
        let handle = EngineHandle {
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            permissions: permissions.clone(),
            runtime: Arc::new(RwLock::new(runtime)),
            transcript: parts.transcript.clone(),
            snapshot: Default::default(),
        };
        let shared = Arc::new(Shared {
            provider: parts.provider,
            tools: parts.tools,
            tool_ctx: parts.tool_ctx,
            permissions,
            hooks: parts.hooks,
            prompter: parts.prompter,
            prompt_lock: tokio::sync::Mutex::new(()),
            sink: parts.sink,
            transcript: parts.transcript,
            history: parts.history,
            on_permission_update: Mutex::new(None),
        });
        let session_context = cfg.initial_context.clone();
        Ok(Engine {
            cfg,
            shared,
            state: TurnState::default(),
            handle,
            system: parts.system,
            session_started: false,
            session_context,
            verify: Default::default(),
            guard: Default::default(),
            output_cap: None,
            reattach_context: false,
            reminders: vec![],
            start_source: "startup".into(),
        })
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.shared.tools
    }

    /// The model provider, for side requests (titles, summaries) outside the conversation.
    pub fn provider(&self) -> Arc<dyn Provider> {
        self.shared.provider.clone()
    }

    /// Add a user message to the conversation without running a turn
    /// (`!command` output when the model shouldn't answer it).
    pub fn add_user_note(&mut self, text: impl Into<String>) {
        self.push_user(Message::user_text(text.into()), false, None, true);
    }

    /// The session goes on in a rebuilt engine (`/reload-*`): no SessionStart.
    pub fn skip_session_start(&mut self) {
        self.session_started = true;
    }

    /// Attach the memory and environment context to the next prompt again
    /// (the session moved to another directory).
    pub fn reattach_context(&mut self) {
        self.reattach_context = self.session_context.is_some();
    }

    /// What SessionStart hooks see as the `source` (`startup`, `resume`, `clear`).
    pub fn set_start_source(&mut self, source: &str) {
        self.start_source = source.to_string();
    }

    /// Notes still waiting for the next prompt (a session switch carries them over).
    pub fn take_reminders(&mut self) -> Vec<String> {
        std::mem::take(&mut self.reminders)
    }

    /// Start from another engine's conversation (a forked sub-agent, `/subtask`): its messages,
    /// cleared tool results and context size. Not its spend, which stays with the session that
    /// made it, and not its transcript chain or task list, unlike [`Engine::restore`].
    pub fn seed(&mut self, from: &TurnState) {
        self.state.messages = from.messages.clone();
        self.state.uuids = from.uuids.clone();
        self.state.meta = from.meta.clone();
        self.state.microcompacted = from.microcompacted.clone();
        // Automatic compaction measures this before the first request.
        self.state.context_tokens = from.context_tokens;
        self.session_started = !self.state.messages.is_empty();
    }

    /// Tell the model something with the next prompt (a system reminder).
    pub fn remind(&mut self, text: impl Into<String>) {
        self.reminders.push(text.into());
    }

    /// The window compaction is measured against, for `model`.
    pub fn context_window(&self, model: &str) -> u64 {
        self.compact_window(model)
    }

    /// The context size at which automatic compaction runs, for `model`.
    pub fn autocompact_at(&self, model: &str) -> u64 {
        forge_compact::autocompact_threshold(self.compact_window(model), self.cfg.max_output_tokens)
    }

    /// The model provider's short name (`messages`, `openai`, `mock`).
    pub fn provider_name(&self) -> &str {
        self.shared.provider.name()
    }

    pub fn tool_ctx(&self) -> &ToolContext {
        &self.shared.tool_ctx
    }

    pub fn transcript(&self) -> &Arc<Transcript> {
        &self.shared.transcript
    }

    pub fn history(&self) -> &Arc<FileHistory> {
        &self.shared.history
    }

    pub fn hooks(&self) -> &HookRunner {
        &self.shared.hooks
    }

    /// The prompter tools of this engine ask through (share it with sub-agents).
    pub fn prompter(&self) -> Arc<dyn PermissionPrompter> {
        self.shared.prompter.clone()
    }

    pub fn system(&self) -> &[SystemBlock] {
        &self.system
    }

    pub fn set_system(&mut self, system: Vec<SystemBlock>) {
        self.system = system;
    }

    pub fn set_permission_update_handler(&self, f: PermissionUpdateHandler) {
        *self.shared.on_permission_update.lock().unwrap() = Some(f);
    }

    /// Load a resumed conversation (contract C5).
    pub fn restore(&mut self, loaded: &forge_session::LoadedSession) {
        self.state.messages = loaded.messages.iter().map(|e| e.message.clone()).collect();
        self.state.uuids = loaded.messages.iter().map(|e| e.uuid.clone()).collect();
        self.state.meta = loaded.messages.iter().filter(|e| e.is_meta).map(|e| e.uuid.clone()).collect();
        self.state.microcompacted = loaded.microcompacted.clone();
        // A resumed session still gets SessionStart (source "resume") with its next
        // prompt; a sub-agent's context is already in its messages.
        if self.cfg.is_subagent {
            self.session_started = !self.state.messages.is_empty();
        }
        self.shared.transcript.continue_from(loaded);
        // The plan survives a resume: the last TodoWrite list becomes the current one.
        let last_todos = self.state.messages.iter().rev().flat_map(|m| m.tool_uses().collect::<Vec<_>>()).find_map(
            |(_, name, input)| {
                (name == "TodoWrite").then(|| input.get("todos").and_then(Value::as_array).cloned()).flatten()
            },
        );
        if let Some(todos) = last_todos.or_else(|| loaded.todos.clone()) {
            *self.shared.tool_ctx.todos.lock().unwrap() = todos;
        }
    }

    /// The current task list, for re-attaching after compaction (GOALS pillar 5).
    fn plan_context(&self) -> Option<String> {
        let todos = self.shared.tool_ctx.todos.lock().unwrap().clone();
        if todos.is_empty() {
            return None;
        }
        let lines: Vec<String> = todos
            .iter()
            .map(|t| {
                format!(
                    "- [{}] {}",
                    t.get("status").and_then(Value::as_str).unwrap_or("pending"),
                    t.get("content").and_then(Value::as_str).unwrap_or_default()
                )
            })
            .collect();
        Some(format!(
            "Your task list (TodoWrite) at the time of compaction. Keep it up to date as you continue:\n{}",
            lines.join("\n")
        ))
    }

    pub fn emit(&self, ev: EngineEvent) {
        self.shared.sink.emit(ev);
    }

    fn notice(&self, level: NoticeLevel, text: impl Into<String>) {
        self.emit(EngineEvent::Notice { level, text: text.into() });
    }

    fn mode_str(&self) -> &'static str {
        self.shared.permissions.read().unwrap().mode.as_str()
    }

    fn pricing_for(&self, model: &str) -> Option<Pricing> {
        self.cfg.pricing.get(model).copied().or_else(|| model_info(model).map(Pricing::from_model))
    }

    /// Record a message in state and transcript; returns its uuid.
    pub(crate) fn push_user(
        &mut self,
        msg: Message,
        is_meta: bool,
        tool_use_result: Option<Value>,
        emit: bool,
    ) -> String {
        let extra = match &tool_use_result {
            Some(v) => json!({"toolUseResult": v}),
            None => json!({}),
        };
        let uuid = self.shared.transcript.append_user(&msg, is_meta, extra);
        if emit {
            self.emit(EngineEvent::User {
                message: msg.clone(),
                uuid: uuid.clone(),
                tool_use_result,
                is_meta,
                parent_tool_use_id: None,
            });
        }
        if is_meta {
            self.state.meta.insert(uuid.clone());
        }
        self.state.messages.push(msg);
        self.state.uuids.push(uuid.clone());
        uuid
    }

    fn push_assistant(&mut self, msg: ApiMessage) -> String {
        let uuid = self.shared.transcript.append_assistant(&msg);
        self.emit(EngineEvent::Assistant { message: msg.clone(), uuid: uuid.clone(), parent_tool_use_id: None });
        self.state.messages.push(msg.to_message());
        self.state.uuids.push(uuid.clone());
        uuid
    }

    /// Tell the host about something (`system/<subtype>`) without recording it.
    pub fn announce(&self, subtype: &str, data: Value) {
        self.emit(EngineEvent::System { subtype: subtype.into(), data });
    }

    fn system_event(&self, subtype: &str, data: Value) {
        self.shared.transcript.append_system(subtype, data.clone());
        self.emit(EngineEvent::System { subtype: subtype.into(), data });
    }

    fn build_request(&self, model: &str) -> MessagesRequest {
        let rt = self.handle.runtime.read().unwrap().clone();
        crate::snapshot::assemble(
            model,
            &rt,
            RequestParts {
                messages: &self.state.messages,
                microcompacted: &self.state.microcompacted,
                system: &self.system,
                tools: self.shared.tools.specs(),
                max_output_tokens: self.output_cap.unwrap_or(self.cfg.max_output_tokens),
                json_schema: self.cfg.json_schema.as_ref(),
                metadata_user_id: self.cfg.metadata_user_id.as_deref(),
                provider_name: self.shared.provider.name(),
            },
        )
    }

    /// Publish the state for readers outside the turn ([`EngineHandle::snapshot`]): the driver
    /// calls this while idle; the engine itself after each model call, each tool batch and at
    /// the end of a turn.
    pub fn publish(&self) {
        self.publish_with(None);
    }

    fn publish_with(&self, turn: Option<TurnProgress>) {
        // Nobody reads a sub-agent's snapshot (its caller reports for it): skip the copy.
        if self.cfg.is_subagent {
            return;
        }
        let snap = EngineSnapshot {
            messages: Arc::new(self.state.messages.clone()),
            microcompacted: Arc::new(self.state.microcompacted.clone()),
            system: Arc::new(self.system.clone()),
            tools: Arc::new(self.shared.tools.specs()),
            total_usage: self.state.total_usage.clone(),
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            context_tokens: self.state.context_tokens,
            turn,
            provider_name: self.shared.provider.name().to_string(),
            max_output_tokens: self.cfg.max_output_tokens,
            json_schema: self.cfg.json_schema.clone(),
            metadata_user_id: self.cfg.metadata_user_id.clone(),
            autocompact_window: self.cfg.autocompact_window,
            auto_compact: self.cfg.auto_compact,
        };
        *self.handle.snapshot.write().unwrap() = Arc::new(snap);
    }

    fn publish_turn(&self, started: Instant, turn: &TurnAcc) {
        self.publish_with(Some(TurnProgress {
            started,
            api_calls: turn.api_calls,
            api_ms: turn.api_ms,
            tool_calls: turn.tool_calls,
        }));
    }

    /// Stream one model response, emitting events. Keeps a usable partial on interrupt.
    async fn stream_once(&self, req: MessagesRequest, cancel: &CancellationToken) -> StreamOutcome {
        self.stream_inner(req, cancel, true).await
    }

    async fn stream_inner(&self, req: MessagesRequest, cancel: &CancellationToken, emit: bool) -> StreamOutcome {
        let mut stream = match self.shared.provider.stream(req, cancel.clone()).await {
            Ok(s) => s,
            Err(ApiError::Cancelled) => return StreamOutcome::Interrupted(None),
            Err(e) => return StreamOutcome::Failed(e),
        };
        let mut acc = MessageAccumulator::new();
        let mut stopped: HashSet<usize> = HashSet::new();
        loop {
            let next = tokio::select! {
                n = stream.next() => n,
                _ = cancel.cancelled() => Some(Err(ApiError::Cancelled)),
            };
            match next {
                Some(Ok(ev)) => {
                    if let StreamEvent::ContentBlockStop { index } = &ev {
                        stopped.insert(*index);
                    }
                    if let Err(e) = acc.push(&ev) {
                        return StreamOutcome::Failed(e);
                    }
                    if emit {
                        self.emit(EngineEvent::Stream { event: ev, parent_tool_use_id: None });
                    }
                }
                Some(Err(ApiError::Cancelled)) => {
                    let partial = acc.snapshot().cloned().map(|mut m| {
                        // Keep text and finished tool calls only (contract C3).
                        let mut i = 0;
                        m.content.retain(|b| {
                            let keep = match b {
                                ContentBlock::ToolUse { .. } => stopped.contains(&i),
                                ContentBlock::Text { text, .. } => !text.is_empty(),
                                ContentBlock::Thinking { signature, .. } => {
                                    !signature.is_empty() && stopped.contains(&i)
                                }
                                _ => stopped.contains(&i),
                            };
                            i += 1;
                            keep
                        });
                        m.stop_reason = None;
                        m
                    });
                    return StreamOutcome::Interrupted(partial.filter(|m| !m.content.is_empty()));
                }
                Some(Err(e)) => return StreamOutcome::Failed(e),
                None => break,
            }
        }
        match acc.finish() {
            Ok(m) => StreamOutcome::Done(m),
            Err(e) => StreamOutcome::Failed(e),
        }
    }

    /// Requests for `model` go out in fast mode: it's on and the model offers it.
    fn sends_fast(&self, model: &str) -> bool {
        crate::snapshot::sends_fast(&self.handle.runtime(), model, self.shared.provider.name())
    }

    fn record_usage(&mut self, model: &str, usage: &Usage, turn: &mut TurnAcc) {
        turn.usage.add(usage);
        self.state.total_usage.add(usage);
        self.state.context_tokens = usage.context_tokens();
        let mut cost = self.pricing_for(model).map(|p| p.cost(usage)).unwrap_or(0.0);
        if self.sends_fast(model) {
            cost *= FAST_PRICE_MULTIPLIER;
        }
        self.state.total_cost_usd += cost;
        let window = model_info(model).map(|m| m.context_window).unwrap_or(0);
        let e = self.state.model_usage.entry(model.to_string()).or_insert_with(|| {
            json!({"inputTokens": 0, "outputTokens": 0, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0,
                   "webSearchRequests": 0, "costUSD": 0.0, "contextWindow": window})
        });
        let add = |e: &mut Value, k: &str, n: u64| e[k] = json!(e[k].as_u64().unwrap_or(0) + n);
        add(e, "inputTokens", usage.input_tokens);
        add(e, "outputTokens", usage.output_tokens);
        add(e, "cacheReadInputTokens", usage.cache_read_input_tokens);
        add(e, "cacheCreationInputTokens", usage.cache_creation_input_tokens);
        if let Some(n) =
            usage.server_tool_use.as_ref().and_then(|s| s.get("web_search_requests")).and_then(Value::as_u64)
        {
            add(e, "webSearchRequests", n);
        }
        e["costUSD"] = json!(e["costUSD"].as_f64().unwrap_or(0.0) + cost);
    }

    fn compact_window(&self, model: &str) -> u64 {
        self.cfg.autocompact_window.unwrap_or_else(|| forge_api::models::model_info_or_default(model).context_window)
    }

    /// Micro-compact and, if the window is nearly full, summarize (contract C9).
    async fn maybe_compact(
        &mut self,
        model: &str,
        continue_work: bool,
        cancel: &CancellationToken,
        turn: &mut TurnAcc,
    ) {
        let window = self.compact_window(model);
        let max_out = self.cfg.max_output_tokens;
        let used = self.state.context_tokens;
        if used > forge_compact::micro_threshold(window, max_out) {
            let ids = forge_compact::micro_candidates(&self.state.messages, &self.state.microcompacted);
            if !ids.is_empty() {
                self.shared.transcript.append_system("microcompact", json!({"toolUseIds": ids}));
                self.shared.tool_ctx.files.forget_views(&ids);
                self.state.microcompacted.extend(ids);
            }
        }
        if self.cfg.auto_compact
            && used >= forge_compact::autocompact_threshold(window, max_out)
            && !self.state.messages.is_empty()
        {
            if let Err(e) = self.compact_inner(None, "auto", continue_work, model, cancel, turn).await {
                self.notice(NoticeLevel::Warning, format!("Automatic compaction failed: {e}"));
            }
        }
    }

    /// Summarize the conversation now (`/compact [instructions]`).
    pub async fn compact(&mut self, instructions: Option<&str>) -> Result<CompactInfo, String> {
        let cancel = self.new_turn_token();
        let model = self.handle.model();
        let mut turn = TurnAcc::default();
        self.compact_inner(instructions, "manual", false, &model, &cancel, &mut turn).await
    }

    async fn compact_inner(
        &mut self,
        instructions: Option<&str>,
        trigger: &str,
        continue_work: bool,
        model: &str,
        cancel: &CancellationToken,
        turn: &mut TurnAcc,
    ) -> Result<CompactInfo, String> {
        if self.state.messages.is_empty() {
            return Err("nothing to compact yet".into());
        }
        tracing::debug!(trigger, context_tokens = self.state.context_tokens, "compacting");
        let o = self
            .shared
            .hooks
            .run(
                HookEvent::PreCompact,
                Some(trigger),
                self.mode_str(),
                json!({"trigger": trigger, "custom_instructions": instructions.unwrap_or("")}),
                cancel,
            )
            .await;
        for m in o.user_messages.iter().chain(o.blocked.iter()) {
            self.notice(NoticeLevel::Warning, m.clone());
        }
        let mut extra = instructions.map(str::to_string).unwrap_or_default();
        for c in &o.additional_context {
            extra.push('\n');
            extra.push_str(c);
        }
        let mut req = self.build_request(model);
        let ask = ContentBlock::text(forge_compact::summary_instruction(
            Some(extra.as_str()).filter(|e| !e.trim().is_empty()),
        ));
        match req.messages.last_mut() {
            Some(last) if last.role == forge_types::Role::User => last.content.push(ask),
            _ => req.messages.push(Message::user(vec![ask])),
        }
        req.tool_choice = Some(json!({"type": "none"}));
        req.output_config = req.output_config.map(|mut oc| {
            if let Some(o) = oc.as_object_mut() {
                o.remove("format");
            }
            oc
        });
        req.max_tokens = req.max_tokens.min(20_000);
        let t0 = Instant::now();
        let outcome = self.stream_inner(req, cancel, false).await;
        turn.api_ms += t0.elapsed().as_millis() as u64;
        let msg = match outcome {
            StreamOutcome::Done(m) => m,
            StreamOutcome::Interrupted(_) => return Err("interrupted".into()),
            StreamOutcome::Failed(e) => return Err(e.to_string()),
        };
        self.record_usage(model, &msg.usage.clone(), turn);
        let summary = forge_compact::extract_summary(&msg.to_message().text());
        if summary.is_empty() {
            return Err("the model returned an empty summary".into());
        }
        let pre_tokens = self.state.context_tokens;
        self.shared.transcript.append_compact_boundary(json!({"trigger": trigger, "preTokens": pre_tokens}));
        self.emit(EngineEvent::System {
            subtype: "compact_boundary".into(),
            data: json!({"compact_metadata": {"trigger": trigger, "pre_tokens": pre_tokens}}),
        });
        self.state.messages.clear();
        self.state.uuids.clear();
        self.state.microcompacted.clear();
        let context: Vec<String> = self.session_context.iter().cloned().chain(self.plan_context()).collect();
        let context = (!context.is_empty()).then(|| context.join("\n\n"));
        let first = forge_compact::summary_message(&summary, context.as_deref(), continue_work);
        self.push_user(first, true, None, false);
        let todos = self.shared.tool_ctx.todos.lock().unwrap().clone();
        if !todos.is_empty() {
            self.shared.transcript.append_system("todos", json!({"todos": todos}));
        }
        self.state.context_tokens = forge_compact::estimate_tokens(&self.state.messages);
        self.shared.tool_ctx.files.forget_all_views();
        Ok(CompactInfo { trigger: trigger.into(), pre_tokens, summary })
    }

    /// The person's prompts, oldest first: user messages with text that are
    /// neither tool results nor only system reminders.
    pub fn prompt_points(&self) -> Vec<PromptPoint> {
        let mut out = vec![];
        for (i, m) in self.state.messages.iter().enumerate() {
            if m.role != forge_types::Role::User
                || m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            {
                continue;
            }
            let uuid = self.state.uuids.get(i).cloned().unwrap_or_default();
            if self.state.meta.contains(&uuid) {
                continue;
            }
            let Some(text) =
                m.content.iter().filter_map(|b| b.as_text()).find(|t| !t.trim_start().starts_with("<system-reminder>"))
            else {
                continue;
            };
            out.push(PromptPoint {
                index: i,
                changed_files: self
                    .checkpoint_turn(i)
                    .map(|t| self.shared.history.changed_since(t))
                    .unwrap_or_default(),
                uuid,
                text: text.trim().to_string(),
            });
        }
        out
    }

    /// `/rewind`: drop the conversation from the message at `index` on (a
    /// prompt point). The transcript branches from the message before it.
    pub fn rewind_conversation(&mut self, index: usize) -> Result<(), String> {
        if index >= self.state.messages.len() {
            return Err("no such message".into());
        }
        let to = self.state.uuids.get(index).cloned();
        self.state.messages.truncate(index);
        self.state.uuids.truncate(index);
        self.state.context_tokens = forge_compact::estimate_tokens(&self.state.messages);
        self.shared.tool_ctx.files.forget_all_views();
        self.shared.transcript.set_leaf(self.state.uuids.last().cloned());
        self.shared.transcript.append_system("rewind", json!({"to": to}));
        self.settle_after_rewrite();
        Ok(())
    }

    /// After the conversation was cut or rewritten: keep what the transcript
    /// would rebuild on resume in step with memory. Cleared tool results that
    /// are still in the conversation are recorded again on the new chain, and
    /// the task list is the last TodoWrite's that remains.
    fn settle_after_rewrite(&mut self) {
        let present: HashSet<String> = self
            .state
            .messages
            .iter()
            .flat_map(|m| m.tool_uses().map(|(id, _, _)| id.to_string()).collect::<Vec<_>>())
            .collect();
        self.state.microcompacted.retain(|id| present.contains(id));
        if !self.state.microcompacted.is_empty() {
            let mut ids: Vec<&String> = self.state.microcompacted.iter().collect();
            ids.sort();
            self.shared.transcript.append_system("microcompact", json!({"toolUseIds": ids}));
        }
        let todos = self
            .state
            .messages
            .iter()
            .rev()
            .flat_map(|m| m.tool_uses().collect::<Vec<_>>())
            .find_map(|(_, name, input)| {
                (name == "TodoWrite").then(|| input.get("todos").and_then(Value::as_array).cloned()).flatten()
            })
            .unwrap_or_default();
        *self.shared.tool_ctx.todos.lock().unwrap() = todos;
    }

    /// `/rewind` with code: restore files to how they were before the prompt at `index`.
    pub fn rewind_code(&self, index: usize) -> Result<forge_session::RewindPlan, String> {
        if index >= self.state.uuids.len() {
            return Err("no such message".into());
        }
        match self.checkpoint_turn(index) {
            Some(turn) => self.shared.history.rewind(turn, false),
            None => Ok(forge_session::RewindPlan::default()),
        }
    }

    /// The first checkpointed user turn at or after message `index`: a message
    /// that started no turn of its own (a `!command` note) rewinds the turns after it.
    fn checkpoint_turn(&self, index: usize) -> Option<&String> {
        self.state.uuids.iter().skip(index).find(|u| self.shared.history.knows(u))
    }

    /// `/rewind` summarize: replace messages `from..to` (prompt boundaries, or
    /// the end) with a summary, keeping the messages around them.
    pub async fn summarize_range(
        &mut self,
        from: usize,
        to: usize,
        instructions: Option<&str>,
    ) -> Result<CompactInfo, String> {
        let len = self.state.messages.len();
        if from >= to || to > len {
            return Err("nothing to summarize there".into());
        }
        let model = self.handle.model();
        let cancel = self.new_turn_token();
        let mut req = self.build_request(&model);
        let mut slice = normalize(&self.state.messages[from..to], &self.state.microcompacted);
        let ask = ContentBlock::text(forge_compact::summary_instruction(instructions.filter(|i| !i.trim().is_empty())));
        match slice.last_mut() {
            Some(last) if last.role == forge_types::Role::User => last.content.push(ask),
            _ => slice.push(Message::user(vec![ask])),
        }
        apply_cache_breakpoints(&mut slice);
        req.messages = slice;
        req.tool_choice = Some(json!({"type": "none"}));
        req.output_config = req.output_config.map(|mut oc| {
            if let Some(o) = oc.as_object_mut() {
                o.remove("format");
            }
            oc
        });
        req.max_tokens = req.max_tokens.min(20_000);
        let msg = match self.stream_inner(req, &cancel, false).await {
            StreamOutcome::Done(m) => m,
            StreamOutcome::Interrupted(_) => return Err("interrupted".into()),
            StreamOutcome::Failed(e) => return Err(e.to_string()),
        };
        self.record_usage(&model, &msg.usage.clone(), &mut TurnAcc::default());
        let summary = forge_compact::extract_summary(&msg.to_message().text());
        if summary.is_empty() {
            return Err("the model returned an empty summary".into());
        }
        let pre_tokens = forge_compact::estimate_tokens(&self.state.messages);
        let which = if to == len { "from this point on" } else { "before this point" };
        let note = Message::user_text(format!(
            "<system-reminder>\nThe conversation {which} was summarized at the user's request.\n</system-reminder>\n\n{summary}"
        ));
        let before: Vec<(String, Message)> =
            self.state.uuids[..from].iter().cloned().zip(self.state.messages[..from].iter().cloned()).collect();
        let after: Vec<(String, Message)> =
            self.state.uuids[to..].iter().cloned().zip(self.state.messages[to..].iter().cloned()).collect();
        let t = &self.shared.transcript;
        if to == len {
            // Branch from the last kept message: nothing to copy.
            t.set_leaf(before.last().map(|(u, _)| u.clone()));
        } else {
            t.append_compact_boundary(json!({"trigger": "partial", "preTokens": pre_tokens}));
            for (u, m) in &before {
                t.rewrite(u, m, self.state.meta.contains(u));
            }
        }
        let note_uuid = t.append_user(&note, true, json!({}));
        for (u, m) in &after {
            t.rewrite(u, m, self.state.meta.contains(u));
        }
        self.state.meta.insert(note_uuid.clone());
        let (mut uuids, mut messages): (Vec<String>, Vec<Message>) = before.into_iter().unzip();
        uuids.push(note_uuid);
        messages.push(note);
        let (au, am): (Vec<String>, Vec<Message>) = after.into_iter().unzip();
        uuids.extend(au);
        messages.extend(am);
        self.state.uuids = uuids;
        self.state.messages = messages;
        self.state.context_tokens = forge_compact::estimate_tokens(&self.state.messages);
        self.shared.tool_ctx.files.forget_all_views();
        self.settle_after_rewrite();
        Ok(CompactInfo { trigger: "partial".into(), pre_tokens, summary })
    }

    /// `/clear`: forget the conversation (the transcript keeps it before a boundary), the task
    /// list and what was read. Memory files come back with the next prompt.
    pub fn clear(&mut self) {
        self.shared
            .transcript
            .append_compact_boundary(json!({"trigger": "clear", "preTokens": self.state.context_tokens}));
        self.state.messages.clear();
        self.state.uuids.clear();
        self.state.microcompacted.clear();
        self.state.context_tokens = 0;
        self.shared.tool_ctx.todos.lock().unwrap().clear();
        self.shared.tool_ctx.files.forget_all_views();
        self.reattach_context = self.session_context.is_some();
    }

    /// A result for something answered without the model (a local slash command).
    pub fn local_result(&self, text: impl Into<String>, is_error: bool) -> TurnResult {
        TurnResult {
            subtype: if is_error { ResultSubtype::ErrorDuringExecution } else { ResultSubtype::Success },
            is_error,
            result: Some(text.into()),
            stop_reason: None,
            num_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            usage: Usage::default(),
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            permission_denials: vec![],
            errors: vec![],
            structured_output: None,
            prompt_blocked: None,
            fatal: false,
            auth_failed: false,
            tool_calls: 0,
        }
    }

    /// Add a sub-agent's spend (`{costUsd, usage, modelUsage}`) to this session: a Task call's as
    /// its result arrives, a subtask's when it is handed back. Budgets include both.
    pub fn record_subagent_usage(&mut self, sub: &Value) {
        let cost = sub.get("costUsd").and_then(Value::as_f64).unwrap_or(0.0);
        self.state.total_cost_usd += cost;
        if let Some(usage) = sub.get("usage").and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok()) {
            self.state.total_usage.add(&usage);
        }
        if let Some(Value::Object(per_model)) = sub.get("modelUsage") {
            for (model, mu) in per_model {
                let e = self.state.model_usage.entry(model.clone()).or_insert_with(|| json!({}));
                for (k, v) in mu.as_object().into_iter().flatten() {
                    let merged = match (e.get(k), v) {
                        (Some(a), b) if a.is_u64() && b.is_u64() => {
                            json!(a.as_u64().unwrap_or(0) + b.as_u64().unwrap_or(0))
                        }
                        (Some(a), b) if a.is_number() && b.is_number() => {
                            json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0))
                        }
                        (_, b) => b.clone(),
                    };
                    e[k] = merged;
                }
            }
        }
    }

    /// A fresh cancellation token for this turn.
    /// `/btw`: answer a side question with the conversation as context, but no
    /// tools, and without adding to the conversation. Earlier side exchanges
    /// Count a request made beside the conversation (a goal check, a title)
    /// toward the session's cost, without changing the context size.
    pub fn record_side_usage(&mut self, model: &str, usage: &Usage) {
        let context = self.state.context_tokens;
        self.record_usage(model, usage, &mut TurnAcc::default());
        self.state.context_tokens = context;
    }

    /// ride along. The request keeps the conversation's system prompt and
    /// tools, so the cached prefix is reused; its cost counts toward the session.
    pub async fn side_question(&mut self, question: &str, earlier: &[(String, String)]) -> Result<String, String> {
        let model = self.handle.model();
        self.publish();
        let req =
            crate::snapshot::side_question_request(&self.handle.snapshot(), &self.handle.runtime(), question, earlier);
        let cancel = self.new_turn_token();
        let msg = forge_api::complete(self.shared.provider.as_ref(), req, &cancel).await.map_err(|e| match e {
            ApiError::Cancelled => "interrupted".to_string(),
            e => e.describe(),
        })?;
        let context = self.state.context_tokens;
        self.record_usage(&model, &msg.usage, &mut TurnAcc::default());
        self.state.context_tokens = context;
        let text = msg.to_message().text();
        if text.trim().is_empty() {
            return Err("the model gave no answer".into());
        }
        Ok(text.trim().to_string())
    }

    fn new_turn_token(&self) -> CancellationToken {
        let t = CancellationToken::new();
        *self.handle.cancel.lock().unwrap() = t.clone();
        t
    }

    /// Run one user turn to completion.
    pub async fn submit(&mut self, prompt: MessageContent) -> TurnResult {
        let started = Instant::now();
        let cancel = self.new_turn_token();
        let mut turn = TurnAcc::default();
        let mut blocks = prompt.into_blocks();
        // Hooks see the prompt as typed: reminders that ride along with it (`@path` attachments)
        // are left out, unless they are all there is.
        let texts: Vec<&str> = blocks.iter().filter_map(|b| b.as_text()).collect();
        let typed: Vec<&str> =
            texts.iter().copied().filter(|t| !t.trim_start().starts_with("<system-reminder>")).collect();
        let prompt_text = if typed.is_empty() { texts.join("\n") } else { typed.join("\n") };

        // Sub-agents get their context but no session hooks.
        if !self.session_started && self.cfg.is_subagent {
            self.session_started = true;
            if let Some(c) = self.cfg.initial_context.take() {
                blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{c}\n</system-reminder>")));
            }
        }
        // SessionStart on the first turn of the process.
        if !self.session_started {
            self.session_started = true;
            let o = self
                .shared
                .hooks
                .run(
                    HookEvent::SessionStart,
                    Some(&self.start_source),
                    self.mode_str(),
                    json!({"source": self.start_source}),
                    &cancel,
                )
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            let mut context = vec![];
            // A resumed conversation already starts with the memory context.
            if let Some(c) = self.cfg.initial_context.take().filter(|_| self.state.messages.is_empty()) {
                context.push(c);
            }
            context.extend(o.additional_context);
            if !context.is_empty() {
                blocks.insert(
                    0,
                    ContentBlock::text(format!("<system-reminder>\n{}\n</system-reminder>", context.join("\n\n"))),
                );
            }
        }

        if std::mem::take(&mut self.reattach_context) {
            if let Some(c) = &self.session_context {
                blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{c}\n</system-reminder>")));
            }
        }
        if !self.reminders.is_empty() && !self.cfg.is_subagent {
            let notes = std::mem::take(&mut self.reminders).join("\n\n");
            blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{notes}\n</system-reminder>")));
        }

        // UserPromptSubmit (contract C11: exit 2 blocks and erases the prompt). Not for sub-agents.
        let o = if self.cfg.is_subagent {
            forge_hooks::HookOutcome::default()
        } else {
            self.shared
                .hooks
                .run(HookEvent::UserPromptSubmit, None, self.mode_str(), json!({"prompt": prompt_text}), &cancel)
                .await
        };
        for m in &o.user_messages {
            self.notice(NoticeLevel::Warning, m.clone());
        }
        if let Some(reason) = o.blocked.clone().or_else(|| o.stop.clone()) {
            return self.finish(
                started,
                turn,
                ResultSubtype::Success,
                None,
                Some("prompt_blocked".into()),
                Some(reason),
            );
        }
        if !o.additional_context.is_empty() {
            blocks.push(ContentBlock::text(format!(
                "<system-reminder>\n{}\n</system-reminder>",
                o.additional_context.join("\n")
            )));
        }

        let model0 = self.handle.model();
        self.maybe_compact(&model0, false, &cancel, &mut turn).await;
        let user_msg = Message::user(blocks);
        let user_uuid = self.push_user(user_msg.clone(), false, None, false);
        self.emit(EngineEvent::PromptAccepted { message: user_msg, uuid: user_uuid.clone() });
        // Sub-agents write into the caller's checkpoint turn, so rewinding the caller undoes them too.
        if !self.cfg.is_subagent {
            self.shared.history.begin_turn(&user_uuid);
        }
        self.verify = Default::default();
        self.guard = Default::default();
        self.output_cap = None;
        if self.verifying() {
            self.verify.fingerprint = self.worktree_fingerprint().await;
        }

        let mut model;
        // Set once a fallback took over; it then holds for the rest of the turn (contract C6).
        let mut fallback: Option<String> = None;
        let mut fallbacks = self.cfg.fallback_models.clone().into_iter();
        let mut stop_hook_active = false;
        let mut compacted_for_length = false;
        let mut last_text: Option<String> = None;
        let mut continuations = 0;
        let mut continuing = false;
        let mut budget_warned = false;

        loop {
            if cancel.is_cancelled() {
                self.push_user(Message::user_text(INTERRUPT_MARKER), true, None, true);
                return self.finish(started, turn, ResultSubtype::Success, last_text, Some("interrupted".into()), None);
            }
            if let Some(max) = self.cfg.max_turns {
                if turn.api_calls >= max {
                    let text = format!("Reached maximum number of turns ({max})");
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorMaxTurns,
                        last_text,
                        Some("max_turns".into()),
                        None,
                    );
                }
            }
            // A host may switch models between calls; a fallback wins for this turn.
            model = fallback.clone().unwrap_or_else(|| self.handle.model());
            if turn.api_calls > 0 {
                self.maybe_compact(&model, true, &cancel, &mut turn).await;
            }
            let req = self.build_request(&model);
            tracing::debug!(
                model = %req.model,
                messages = req.messages.len(),
                tools = req.tools.len(),
                max_tokens = req.max_tokens,
                output_config = ?req.output_config,
                speed = ?req.speed,
                context_tokens = self.state.context_tokens,
                "model request"
            );
            let t0 = Instant::now();
            let outcome = self.stream_once(req, &cancel).await;
            turn.api_ms += t0.elapsed().as_millis() as u64;
            match &outcome {
                StreamOutcome::Done(m) => tracing::debug!(
                    ms = t0.elapsed().as_millis() as u64,
                    stop_reason = ?m.stop_reason,
                    input_tokens = m.usage.input_tokens,
                    output_tokens = m.usage.output_tokens,
                    cache_read = m.usage.cache_read_input_tokens,
                    tool_calls = m.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count(),
                    "model response"
                ),
                StreamOutcome::Interrupted(_) => tracing::debug!("model response interrupted"),
                StreamOutcome::Failed(e) => tracing::debug!(error = %e, "model request failed"),
            }
            let msg = match outcome {
                StreamOutcome::Done(m) => m,
                StreamOutcome::Interrupted(partial) => {
                    let mut marker = INTERRUPT_MARKER;
                    if let Some(p) = partial {
                        let ids: Vec<String> = p.to_message().tool_uses().map(|(id, _, _)| id.to_string()).collect();
                        self.push_assistant(p);
                        for id in ids {
                            self.push_user(
                                Message::user(vec![ContentBlock::tool_result(id, forge_tools::INTERRUPTED, true)]),
                                false,
                                None,
                                true,
                            );
                            marker = INTERRUPT_MARKER_TOOLS;
                        }
                    }
                    self.push_user(Message::user_text(marker), true, None, true);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("interrupted".into()),
                        None,
                    );
                }
                StreamOutcome::Failed(e) if e.is_prompt_too_long() && !compacted_for_length => {
                    compacted_for_length = true;
                    self.notice(
                        NoticeLevel::Warning,
                        "The conversation no longer fits the context window; compacting.",
                    );
                    match self.compact_inner(None, "auto", true, &model, &cancel, &mut turn).await {
                        Ok(_) => continue,
                        Err(err) => {
                            let text = format!("API Error: {e} (compaction failed: {err})");
                            self.notice(NoticeLevel::Error, text.clone());
                            turn.errors.push(text.clone());
                            let mut r = self.finish(
                                started,
                                turn,
                                ResultSubtype::ErrorDuringExecution,
                                Some(text),
                                Some("api_error".into()),
                                None,
                            );
                            r.fatal = true;
                            return r;
                        }
                    }
                }
                StreamOutcome::Failed(e) => {
                    let switch = e.is_overloaded() || matches!(&e, ApiError::Http { status: 404, .. });
                    if switch {
                        if let Some(next) = fallbacks.next() {
                            self.system_event(
                                "model_fallback",
                                json!({"from": model, "to": next, "reason": e.to_string()}),
                            );
                            self.notice(
                                NoticeLevel::Warning,
                                format!("{model} unavailable ({e}); switching to {next} for this turn"),
                            );
                            fallback = Some(next);
                            continue;
                        }
                    }
                    let text = format!("API Error: {}", e.describe());
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text.clone());
                    let mut r = self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorDuringExecution,
                        Some(text),
                        Some("api_error".into()),
                        None,
                    );
                    // Still too long after compacting: the next turn would fail the same way.
                    r.fatal = e.is_unrecoverable() || (e.is_prompt_too_long() && compacted_for_length);
                    r.auth_failed = e.is_auth_failure();
                    return r;
                }
            };
            turn.api_calls += 1;
            self.record_usage(&model, &msg.usage.clone(), &mut turn);
            let text = msg.to_message().text();
            if continuing {
                // A reply continued after `max_tokens` is one answer.
                last_text = Some(format!("{}{text}", last_text.take().unwrap_or_default()));
            } else if !text.is_empty() {
                last_text = Some(text);
            }
            let stop_reason = msg.stop_reason;
            let tool_uses: Vec<(String, String, Value)> =
                msg.to_message().tool_uses().map(|(a, b, c)| (a.to_string(), b.to_string(), c.clone())).collect();
            self.push_assistant(msg);
            self.publish_turn(started, &turn);

            // Budget (contract C7): checked after every call.
            if let Some(budget) = self.cfg.max_budget_usd {
                if self.state.total_cost_usd >= budget {
                    for (id, _, _) in &tool_uses {
                        self.push_user(
                            Message::user(vec![ContentBlock::tool_result(
                                id.clone(),
                                "Not run: the session's budget is used up.",
                                true,
                            )]),
                            true,
                            None,
                            false,
                        );
                    }
                    let text = format!("Reached maximum budget (${budget})");
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorMaxBudgetUsd,
                        last_text,
                        Some("max_budget".into()),
                        None,
                    );
                }
            }

            // Output cut off (GOALS pillar 2): a larger cap for the rest of the turn; a cut-off tool
            // call is answered with an error by exec; cut-off text is continued.
            continuing = false;
            if stop_reason == Some(StopReason::MaxTokens) {
                if self.cfg.escalate_output
                    && self.output_cap.is_none()
                    && self.cfg.max_output_tokens < ESCALATED_OUTPUT_TOKENS
                {
                    self.output_cap = Some(ESCALATED_OUTPUT_TOKENS);
                    self.system_event(
                        "output_limit",
                        json!({"from": self.cfg.max_output_tokens, "to": ESCALATED_OUTPUT_TOKENS}),
                    );
                }
                if tool_uses.is_empty() && continuations < MAX_CONTINUATIONS {
                    continuations += 1;
                    continuing = true;
                    self.push_user(Message::user_text(CONTINUE_PROMPT), true, None, true);
                    continue;
                }
            }

            if !tool_uses.is_empty() {
                turn.tool_calls += tool_uses.len() as u32;
                let results = crate::exec::run_tools(&self.shared, &tool_uses, &cancel, self.mode_str()).await;
                if self.verifying() {
                    self.track_verification(&tool_uses, &results).await;
                }
                let stuck: Vec<crate::stuck::Stuck> = tool_uses
                    .iter()
                    .zip(&results)
                    .filter(|(_, r)| r.denial.is_none())
                    .filter_map(|((_, name, input), r)| {
                        self.guard.observe(name, input, &r.output.text_content(), r.output.is_error)
                    })
                    .collect();
                let mut interrupted = cancel.is_cancelled();
                for r in results {
                    if let Some(d) = r.denial {
                        turn.denials.push(d);
                    }
                    if r.interrupt_turn {
                        interrupted = true;
                    }
                    if let Some(s) = r.stop {
                        turn.stop = Some(s);
                    }
                    if let Some(sub) = r.output.structured.as_ref().and_then(|s| s.get("subagentUsage")) {
                        self.record_subagent_usage(sub);
                    }
                    // A request a tool made itself (the Advisor): priced here, custom pricing included.
                    if let Some(side) = r.output.structured.as_ref().and_then(|s| s.get("sideUsage")) {
                        let model = side["model"].as_str().unwrap_or_default().to_string();
                        if let Ok(u) = serde_json::from_value::<Usage>(side["usage"].clone()) {
                            self.record_side_usage(&model, &u);
                        }
                    }
                    let block = ContentBlock::ToolResult {
                        tool_use_id: r.id.clone(),
                        content: r.output.content.clone(),
                        is_error: if r.output.is_error { Some(true) } else { None },
                        cache_control: None,
                    };
                    self.push_user(Message::user(vec![block]), false, r.output.structured.clone(), true);
                }
                self.publish_turn(started, &turn);
                if let (Some(s), false) = (stuck.first(), interrupted) {
                    self.system_event("loop_guard", s.record());
                    self.push_user(Message::user_text(s.text()), true, None, true);
                }
                if !budget_warned && !interrupted {
                    if let Some(text) = self.budget_warning(turn.api_calls) {
                        budget_warned = true;
                        self.push_user(Message::user_text(text), true, None, true);
                    }
                }
                if interrupted {
                    cancel.cancel();
                    self.push_user(Message::user_text(INTERRUPT_MARKER_TOOLS), true, None, true);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("interrupted".into()),
                        None,
                    );
                }
                if let Some(reason) = turn.stop.clone() {
                    self.notice(NoticeLevel::Info, format!("Stopped by hook: {reason}"));
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("hook_stopped".into()),
                        None,
                    );
                }
                continue;
            }
            if stop_reason == Some(StopReason::PauseTurn) {
                continue;
            }

            // Verification loop (GOALS pillar 3): no finishing on unchecked changes.
            if stop_reason != Some(StopReason::Refusal) {
                if let Some(text) = self.verification_reminder().await {
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // Stop / SubagentStop hook (contract C11: exit 2 makes the model keep going).
            let stop_event = if self.cfg.is_subagent { HookEvent::SubagentStop } else { HookEvent::Stop };
            let o = self
                .shared
                .hooks
                .run(stop_event, None, self.mode_str(), json!({"stop_hook_active": stop_hook_active}), &cancel)
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            if let (Some(feedback), None) = (o.blocked.clone(), o.stop.clone()) {
                if stop_event.exit2_effect() == Exit2Effect::BlockToModel {
                    stop_hook_active = true;
                    self.push_user(Message::user_text(format!("Stop hook feedback:\n{feedback}")), true, None, true);
                    continue;
                }
            }
            // `continue: false` from a Stop hook: the run stops here, nothing follows it.
            if let Some(reason) = o.stop.clone() {
                self.notice(NoticeLevel::Info, format!("Stopped by hook: {reason}"));
                turn.stop = Some(reason);
                return self.finish(
                    started,
                    turn,
                    ResultSubtype::Success,
                    last_text,
                    Some("hook_stopped".into()),
                    None,
                );
            }
            let reason = match stop_reason {
                Some(StopReason::EndTurn) | None => "end_turn",
                Some(StopReason::MaxTokens) => "max_tokens",
                Some(StopReason::StopSequence) => "stop_sequence",
                Some(StopReason::Refusal) => "refusal",
                Some(StopReason::ModelContextWindowExceeded) => "model_context_window_exceeded",
                Some(StopReason::ToolUse) => "tool_use",
                Some(StopReason::PauseTurn) => "pause_turn",
                Some(StopReason::Other) => "other",
            };
            return self.finish(started, turn, ResultSubtype::Success, last_text, Some(reason.into()), None);
        }
    }

    /// A heads-up when this run's turn or spending limit is close (GOALS pillar 5), so the model
    /// wraps up with a report instead of being cut off mid-change.
    fn budget_warning(&self, calls: u32) -> Option<String> {
        let turns_left = self.cfg.max_turns.filter(|m| *m >= 6).map(|m| m.saturating_sub(calls)).filter(|l| *l <= 3);
        let money_left = self
            .cfg
            .max_budget_usd
            .map(|b| b - self.state.total_cost_usd)
            .filter(|left| self.cfg.max_budget_usd.map(|b| *left <= b * 0.15).unwrap_or(false));
        let what = match (turns_left, money_left) {
            (Some(t), _) => format!("{t} model call{} left in this run (--max-turns)", if t == 1 { "" } else { "s" }),
            (None, Some(m)) => format!("${m:.2} of the run's budget left (--max-budget-usd)"),
            (None, None) => return None,
        };
        Some(format!(
            "<system-reminder>\nYou have {what}. Wrap up: finish the most important change, check it, and end with \
             what is done, what was verified and what remains.\n</system-reminder>"
        ))
    }

    fn verifying(&self) -> bool {
        self.cfg.verify.is_some() && !self.cfg.is_subagent
    }

    async fn worktree_fingerprint(&self) -> Option<u64> {
        let dir = self.shared.tool_ctx.project_dir.clone();
        tokio::task::spawn_blocking(move || forge_git::worktree_fingerprint(&dir)).await.ok().flatten()
    }

    /// Note checks and possible shell writes among the calls just run.
    async fn track_verification(&mut self, calls: &[(String, String, Value)], results: &[crate::exec::CallResult]) {
        let Some(vc) = self.cfg.verify.clone() else { return };
        let mut refresh = false;
        for ((_, name, input), r) in calls.iter().zip(results) {
            if name != "Bash"
                || r.denial.is_some()
                || input.get("run_in_background").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
            let changed = self.shared.history.writes_since(0);
            if crate::verify::is_check(command, &vc.commands, &changed) {
                self.verify.write_mark = r.writes_after;
                self.verify.shell_may_have_changed = false;
                self.verify.last_check = Some((command.to_string(), r.output.is_error));
                refresh = true;
            } else if !self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true) {
                self.verify.shell_may_have_changed = true;
            }
        }
        if refresh {
            self.verify.fingerprint = self.worktree_fingerprint().await;
        }
    }

    /// The reminder to send before the turn may end, if one is due.
    async fn verification_reminder(&mut self) -> Option<String> {
        let vc = self.cfg.verify.clone()?;
        if self.cfg.is_subagent || self.verify.reminders >= vc.max_reminders {
            return None;
        }
        let writes = self.shared.history.writes_len();
        let changed_this_turn = writes > 0 || self.verify.shell_may_have_changed;
        if !changed_this_turn || self.shared.tools.get("Bash").is_none() {
            return None;
        }
        // Checks that can't run make a reminder a wasted call.
        let probe = vc.commands.first().cloned().unwrap_or_else(|| "true".into());
        let req = forge_permissions::Request::new("Bash", forge_permissions::Subject::Command(probe), false);
        if matches!(self.shared.permissions.read().unwrap().decide(&req), forge_permissions::Decision::Deny { .. }) {
            return None;
        }
        let root = &self.shared.tool_ctx.project_dir;
        let files: Vec<String> = self
            .shared
            .history
            .writes_since(self.verify.write_mark)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
            .collect();
        let shell = files.is_empty()
            && self.verify.shell_may_have_changed
            && self.verify.fingerprint.is_some()
            && self.worktree_fingerprint().await != self.verify.fingerprint;
        let reminder = if !files.is_empty() || shell {
            crate::verify::Reminder::Unchecked { files, shell }
        } else {
            match &self.verify.last_check {
                Some((command, true)) if writes > 0 => {
                    crate::verify::Reminder::LastCheckFailed { command: command.clone() }
                }
                _ => return None,
            }
        };
        self.verify.reminders += 1;
        self.system_event("verification", reminder.record());
        self.notice(NoticeLevel::Info, "Changes not verified yet: asking the model to run the project's checks.");
        Some(reminder.text(&vc.commands))
    }

    fn finish(
        &mut self,
        started: Instant,
        turn: TurnAcc,
        subtype: ResultSubtype,
        result: Option<String>,
        stop_reason: Option<String>,
        prompt_blocked: Option<String>,
    ) -> TurnResult {
        let structured_output = if self.cfg.json_schema.is_some() && subtype == ResultSubtype::Success {
            result.as_deref().and_then(|r| serde_json::from_str::<Value>(r.trim()).ok())
        } else {
            None
        };
        let is_error = subtype != ResultSubtype::Success;
        self.publish();
        TurnResult {
            subtype,
            is_error,
            result,
            stop_reason,
            num_turns: turn.api_calls,
            duration_ms: started.elapsed().as_millis() as u64,
            duration_api_ms: turn.api_ms,
            usage: turn.usage,
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            permission_denials: turn.denials,
            errors: turn.errors,
            structured_output,
            prompt_blocked,
            fatal: false,
            auth_failed: false,
            tool_calls: turn.tool_calls,
        }
    }

    /// End the session: SessionEnd hooks, background shells.
    pub async fn end_session(&self, reason: &str) {
        let c = CancellationToken::new();
        let _ = tokio::time::timeout(
            Duration::from_secs(70),
            self.shared.hooks.run(HookEvent::SessionEnd, None, self.mode_str(), json!({"reason": reason}), &c),
        )
        .await;
        self.shared.tool_ctx.shells.kill_all();
    }
}
