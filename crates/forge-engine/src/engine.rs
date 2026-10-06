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
use crate::request::{apply_cache_breakpoints, normalize, thinking_params};
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
}

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

/// Settings a host may change while the engine runs.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub model: String,
    pub max_thinking_tokens: Option<u32>,
    pub effort: Option<String>,
}

/// A cloneable handle for controlling a running engine (interrupt, mode, model).
#[derive(Clone)]
pub struct EngineHandle {
    cancel: Arc<Mutex<CancellationToken>>,
    pub permissions: Arc<RwLock<PermEngine>>,
    pub runtime: Arc<RwLock<Runtime>>,
    transcript: Arc<Transcript>,
}

impl EngineHandle {
    /// Interrupt the running turn (contract C3).
    pub fn interrupt(&self) {
        self.cancel.lock().unwrap().cancel();
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

    pub fn model(&self) -> String {
        self.runtime.read().unwrap().model.clone()
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
}

pub struct Engine {
    pub cfg: EngineConfig,
    pub(crate) shared: Arc<Shared>,
    pub state: TurnState,
    handle: EngineHandle,
    system: Vec<SystemBlock>,
    session_started: bool,
}

/// Per-turn accounting.
#[derive(Default)]
pub(crate) struct TurnAcc {
    pub api_calls: u32,
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
        };
        let permissions = Arc::new(RwLock::new(parts.permissions));
        let handle = EngineHandle {
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            permissions: permissions.clone(),
            runtime: Arc::new(RwLock::new(runtime)),
            transcript: parts.transcript.clone(),
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
        Ok(Engine { cfg, shared, state: TurnState::default(), handle, system: parts.system, session_started: false })
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.shared.tools
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
        self.state.microcompacted = loaded.microcompacted.clone();
        self.session_started = !self.state.messages.is_empty();
        self.shared.transcript.continue_from(loaded);
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

    fn system_event(&self, subtype: &str, data: Value) {
        self.shared.transcript.append_system(subtype, data.clone());
        self.emit(EngineEvent::System { subtype: subtype.into(), data });
    }

    fn build_request(&self, model: &str) -> MessagesRequest {
        let rt = self.handle.runtime.read().unwrap().clone();
        let info = forge_api::models::model_info_or_default(model);
        let (thinking, output_config, max_tokens) =
            thinking_params(&info, rt.max_thinking_tokens, rt.effort.as_deref(), self.cfg.max_output_tokens);
        let mut output_config = output_config;
        if let Some(schema) = &self.cfg.json_schema {
            let oc = output_config.get_or_insert_with(|| json!({}));
            oc["format"] = json!({"type": "json_schema", "schema": schema});
        }
        let mut messages = normalize(&self.state.messages, &self.state.microcompacted);
        apply_cache_breakpoints(&mut messages);
        let mut tools = self.shared.tools.specs();
        if let Some(last) = tools.last_mut() {
            last.cache_control = Some(forge_types::CacheControl::ephemeral());
        }
        MessagesRequest {
            model: model.to_string(),
            max_tokens,
            messages,
            system: self.system.clone(),
            tools,
            tool_choice: None,
            thinking,
            temperature: None,
            metadata: self.cfg.metadata_user_id.as_ref().map(|u| json!({"user_id": u})),
            output_config,
            stream: true,
        }
    }

    /// Stream one model response, emitting events. Keeps a usable partial on interrupt.
    async fn stream_once(&self, req: MessagesRequest, cancel: &CancellationToken) -> StreamOutcome {
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
                    self.emit(EngineEvent::Stream { event: ev, parent_tool_use_id: None });
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

    fn record_usage(&mut self, model: &str, usage: &Usage, turn: &mut TurnAcc) {
        turn.usage.add(usage);
        self.state.total_usage.add(usage);
        self.state.context_tokens = usage.context_tokens();
        let cost = self.pricing_for(model).map(|p| p.cost(usage)).unwrap_or(0.0);
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

    /// A fresh cancellation token for this turn.
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
        let prompt_text: String = blocks.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n");

        // SessionStart on the first turn of the process.
        if !self.session_started {
            self.session_started = true;
            let o = self
                .shared
                .hooks
                .run(HookEvent::SessionStart, Some("startup"), self.mode_str(), json!({"source": "startup"}), &cancel)
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            let mut context = vec![];
            if let Some(c) = self.cfg.initial_context.take() {
                context.push(c);
            }
            context.extend(o.additional_context);
            if !context.is_empty() && self.state.messages.is_empty() {
                blocks.insert(
                    0,
                    ContentBlock::text(format!("<system-reminder>\n{}\n</system-reminder>", context.join("\n\n"))),
                );
            }
        }

        // UserPromptSubmit (contract C11: exit 2 blocks and erases the prompt).
        let o = self
            .shared
            .hooks
            .run(HookEvent::UserPromptSubmit, None, self.mode_str(), json!({"prompt": prompt_text}), &cancel)
            .await;
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

        let user_msg = Message::user(blocks);
        let user_uuid = self.push_user(user_msg.clone(), false, None, false);
        self.emit(EngineEvent::PromptAccepted { message: user_msg, uuid: user_uuid.clone() });
        self.shared.history.begin_turn(&user_uuid);

        let mut model;
        // Set once a fallback took over; it then holds for the rest of the turn (contract C6).
        let mut fallback: Option<String> = None;
        let mut fallbacks = self.cfg.fallback_models.clone().into_iter();
        let mut stop_hook_active = false;
        let mut last_text: Option<String> = None;

        loop {
            if cancel.is_cancelled() {
                self.push_user(Message::user_text(INTERRUPT_MARKER), true, None, true);
                return self.finish(started, turn, ResultSubtype::Success, last_text, Some("interrupted".into()), None);
            }
            if let Some(max) = self.cfg.max_turns {
                if turn.api_calls >= max {
                    turn.errors.push(format!("Reached maximum number of turns ({max})"));
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
            let req = self.build_request(&model);
            let t0 = Instant::now();
            let outcome = self.stream_once(req, &cancel).await;
            turn.api_ms += t0.elapsed().as_millis() as u64;
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
                    let text = format!("API Error: {e}");
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text.clone());
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorDuringExecution,
                        Some(text),
                        Some("api_error".into()),
                        None,
                    );
                }
            };
            turn.api_calls += 1;
            self.record_usage(&model, &msg.usage.clone(), &mut turn);
            let text = msg.to_message().text();
            if !text.is_empty() {
                last_text = Some(text);
            }
            let stop_reason = msg.stop_reason;
            let tool_uses: Vec<(String, String, Value)> =
                msg.to_message().tool_uses().map(|(a, b, c)| (a.to_string(), b.to_string(), c.clone())).collect();
            self.push_assistant(msg);

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
                    turn.errors.push(format!("Reached maximum budget (${budget})"));
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

            if !tool_uses.is_empty() {
                let results = crate::exec::run_tools(&self.shared, &tool_uses, &cancel, self.mode_str()).await;
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
                    let block = ContentBlock::ToolResult {
                        tool_use_id: r.id.clone(),
                        content: r.output.content.clone(),
                        is_error: if r.output.is_error { Some(true) } else { None },
                        cache_control: None,
                    };
                    self.push_user(Message::user(vec![block]), false, r.output.structured.clone(), true);
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

            // Stop hook (contract C11: exit 2 makes the model keep going).
            let o = self
                .shared
                .hooks
                .run(HookEvent::Stop, None, self.mode_str(), json!({"stop_hook_active": stop_hook_active}), &cancel)
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            if let (Some(feedback), None) = (o.blocked.clone(), o.stop.clone()) {
                if HookEvent::Stop.exit2_effect() == Exit2Effect::BlockToModel {
                    stop_hook_active = true;
                    self.push_user(Message::user_text(format!("Stop hook feedback:\n{feedback}")), true, None, true);
                    continue;
                }
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
