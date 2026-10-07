//! What the engine publishes for readers outside the turn (contract C17,
//! "Immediate commands"): a read-only copy of its state, replaced whole after
//! each model call, after each tool batch, at the end of a turn and whenever
//! the driver asks ([`crate::Engine::publish`]).
//!
//! Readers clone the inner `Arc` ([`crate::EngineHandle::snapshot`]) and drop
//! the lock at once, so no guard is held for long, and never across `.await`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use forge_api::models::model_info;
use forge_types::{ContentBlock, Message, MessagesRequest, SystemBlock, ToolSpec, Usage};
use serde_json::{json, Map, Value};

use crate::engine::{Runtime, FAST_MODE_BETA};
use crate::request::{apply_cache_breakpoints, normalize, thinking_params};

/// The turn in progress, as far as it has got.
#[derive(Debug, Clone)]
pub struct TurnProgress {
    pub started: Instant,
    /// Model calls so far in this turn.
    pub api_calls: u32,
    pub api_ms: u64,
    pub tool_calls: u32,
}

/// A read-only copy of the engine's state.
#[derive(Debug, Clone, Default)]
pub struct EngineSnapshot {
    pub messages: Arc<Vec<Message>>,
    /// Tool results cleared by micro-compaction (requests show a placeholder for them).
    pub microcompacted: Arc<HashSet<String>>,
    pub system: Arc<Vec<SystemBlock>>,
    pub tools: Arc<Vec<ToolSpec>>,
    pub total_usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    /// Models used without a known price: their cost isn't in `total_cost_usd`.
    pub unpriced: Vec<String>,
    pub context_tokens: u64,
    /// `Some` while a turn runs.
    pub turn: Option<TurnProgress>,
    pub provider_name: String,
    pub max_output_tokens: u32,
    pub json_schema: Option<Value>,
    pub metadata_user_id: Option<String>,
    pub autocompact_window: Option<u64>,
    pub auto_compact: bool,
}

impl EngineSnapshot {
    /// The window compaction is measured against, for `model`.
    pub fn context_window(&self, model: &str) -> u64 {
        self.autocompact_window.unwrap_or_else(|| forge_api::models::model_info_or_default(model).context_window)
    }

    /// The context size at which automatic compaction runs, for `model`.
    pub fn autocompact_at(&self, model: &str) -> u64 {
        forge_compact::autocompact_threshold(self.context_window(model), self.max_output_tokens)
    }
}

/// Everything a model request is built from.
pub(crate) struct RequestParts<'a> {
    pub messages: &'a [Message],
    pub microcompacted: &'a HashSet<String>,
    pub system: &'a [SystemBlock],
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
    pub json_schema: Option<&'a Value>,
    pub metadata_user_id: Option<&'a str>,
    pub provider_name: &'a str,
}

pub(crate) fn sends_fast(rt: &Runtime, model: &str, provider_name: &str) -> bool {
    rt.fast && model_info(model).is_some_and(|m| m.supports_fast_mode) && provider_name != "openai"
}

/// The request for `model`: the conversation's requests and side questions share it, so a
/// side question reuses the cached prefix.
pub(crate) fn assemble(model: &str, rt: &Runtime, p: RequestParts<'_>) -> MessagesRequest {
    let info = forge_api::models::model_info_or_default(model);
    let (thinking, mut output_config, max_tokens) =
        thinking_params(&info, rt.max_thinking_tokens, rt.effort.as_deref(), p.max_output_tokens);
    if let Some(schema) = p.json_schema {
        let oc = output_config.get_or_insert_with(|| json!({}));
        oc["format"] = json!({"type": "json_schema", "schema": schema});
    }
    // Fast mode only where the model offers it; elsewhere the flag is ignored.
    let fast = sends_fast(rt, model, p.provider_name);
    let mut messages = normalize(p.messages, p.microcompacted);
    apply_cache_breakpoints(&mut messages);
    let mut tools = p.tools;
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(forge_types::CacheControl::ephemeral());
    }
    let mut system = p.system.to_vec();
    if let Some(last) = system.last_mut() {
        last.cache_control = Some(forge_types::CacheControl::ephemeral());
    }
    MessagesRequest {
        model: model.to_string(),
        max_tokens,
        messages,
        system,
        tools,
        tool_choice: None,
        thinking,
        temperature: None,
        metadata: p.metadata_user_id.map(|u| json!({"user_id": u})),
        output_config,
        speed: fast.then(|| "fast".to_string()),
        stream: true,
        betas: if fast { vec![FAST_MODE_BETA.to_string()] } else { vec![] },
    }
}

/// Sent with a `/btw` question.
const SIDE_QUESTION_NOTE: &str = "<system-reminder>\nThis is a side question from the user. Answer it briefly from \
what you already know of this conversation. You can't use tools for it, and neither the question nor your answer \
becomes part of the main conversation.\n</system-reminder>";

/// A `/btw` request: the conversation as `snap` has it (same system prompt and tools, so the
/// cached prefix is reused), earlier side questions, then `question`, with `tool_choice: none`.
/// Mid-turn, a model reply whose tool calls haven't been answered yet is left out.
pub fn side_question_request(
    snap: &EngineSnapshot,
    rt: &Runtime,
    question: &str,
    earlier: &[(String, String)],
) -> MessagesRequest {
    let mut messages: &[Message] = &snap.messages;
    if let Some(last) = messages.last() {
        if last.role == forge_types::Role::Assistant && last.tool_uses().next().is_some() {
            messages = &messages[..messages.len() - 1];
        }
    }
    let mut req = assemble(
        &rt.model,
        rt,
        RequestParts {
            messages,
            microcompacted: &snap.microcompacted,
            system: &snap.system,
            tools: snap.tools.to_vec(),
            max_output_tokens: snap.max_output_tokens,
            json_schema: snap.json_schema.as_ref(),
            metadata_user_id: snap.metadata_user_id.as_deref(),
            provider_name: &snap.provider_name,
        },
    );
    let mut extra = vec![];
    for (q, a) in earlier {
        extra.push(Message::user_text(q.clone()));
        extra.push(Message::assistant(vec![ContentBlock::text(a.clone())]));
    }
    extra.push(Message::user_text(format!("{SIDE_QUESTION_NOTE}\n\n{question}")));
    for m in extra {
        match req.messages.last_mut() {
            Some(last) if last.role == m.role => last.content.extend(m.content),
            _ => req.messages.push(m),
        }
    }
    req.tool_choice = Some(json!({"type": "none"}));
    req
}
