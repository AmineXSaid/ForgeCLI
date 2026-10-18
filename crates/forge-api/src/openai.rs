//! OpenAI-compatible `/v1/chat/completions` endpoints (custom gateways,
//! local servers). Requests are translated from the Messages shape and the
//! streamed chunks are translated back into Messages API stream events, so the
//! engine never sees the difference.

use std::time::Duration;

use forge_types::{
    ApiMessage, ContentBlock, Delta, MediaSource, Message, MessageDeltaBody, MessagesRequest, Role, StopReason,
    StreamEvent, Usage,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::messages::{http_error, spawn_sse_pump};
use crate::{backoff_delay, ApiError, EventStream, Provider};

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL up to and including `/v1` (e.g. `http://localhost:11434/v1`).
    pub base_url: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    pub timeout: Duration,
    /// The variable or setting that chose `base_url`.
    pub url_from: Option<&'static str>,
    /// Set when `api_key` came from a helper command (its setting name).
    pub key_helper: Option<&'static str>,
}

impl OpenAiConfig {
    /// The endpoint and where its key came from, for error hints.
    pub fn origin(&self) -> crate::auth::Origin {
        use crate::auth::KeyFrom;
        let key_from = match (self.api_key.is_some(), self.key_helper) {
            (false, _) => KeyFrom::None,
            (true, Some(h)) => KeyFrom::Helper(h),
            (true, None) => KeyFrom::OpenAiKey,
        };
        crate::auth::Origin { backend: crate::auth::Backend::OpenAi, url_from: self.url_from, key_from }
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        OpenAiConfig {
            base_url: "http://localhost:8000/v1".into(),
            api_key: None,
            extra_headers: vec![],
            max_retries: 2,
            timeout: Duration::from_secs(300),
            url_from: None,
            key_helper: None,
        }
    }
}

pub struct OpenAiProvider {
    config: OpenAiConfig,
    http: reqwest::Client,
    /// What this endpoint refused, per model, so later turns send what it
    /// takes first time: the keys it refused (`adapt_to_refusal`).
    refused: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
}

/// Apply what an endpoint refused before to a fresh body.
fn apply_refused(body: &mut Value, refused: &[String]) {
    let Some(o) = body.as_object_mut() else { return };
    // Both length names refused (a gateway that turns one into the other):
    // send no length limit at all.
    if LENGTH_KEYS.iter().all(|k| refused.iter().any(|r| r == k)) {
        for k in LENGTH_KEYS {
            o.remove(k);
        }
        return;
    }
    for key in refused {
        match key.as_str() {
            "max_tokens" | "max_completion_tokens" => {
                let other = if key == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
                if let Some(v) = o.remove(key.as_str()) {
                    o.insert(other.into(), v);
                }
            }
            k => {
                o.remove(k);
            }
        }
    }
}

const LENGTH_KEYS: [&str; 2] = ["max_tokens", "max_completion_tokens"];

/// The keys `before` had that `after` lacks: what a refusal took out.
fn dropped_keys(before: &Value, after: &Value) -> Vec<String> {
    before
        .as_object()
        .map(|o| o.keys().filter(|k| after.get(k.as_str()).is_none()).cloned().collect())
        .unwrap_or_default()
}

impl OpenAiProvider {
    pub fn new(config: OpenAiConfig) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(config.timeout)
            .build()
            .map_err(|e| ApiError::network(e.to_string()))?;
        Ok(OpenAiProvider { config, http, refused: Default::default() })
    }
}

/// Translate a Messages request into a chat-completions body.
pub fn to_chat_request(req: &MessagesRequest) -> Value {
    let mut messages = Vec::new();
    let system: Vec<&str> = req.system.iter().map(|b| b.text.as_str()).collect();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system.join("\n\n")}));
    }
    for m in &req.messages {
        push_message(&mut messages, m);
    }
    let tools: Vec<Value> = req
        .tools
        .iter()
        .filter(|t| t.kind.is_none())
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.input_schema
            }})
        })
        .collect();
    let reasoning = is_reasoning_model(&req.model);
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    // OpenAI's reasoning models take `max_completion_tokens` and only their
    // default temperature; everything else takes `max_tokens`.
    body[if reasoning { "max_completion_tokens" } else { "max_tokens" }] = json!(req.max_tokens);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(t) = req.temperature.filter(|_| !reasoning) {
        body["temperature"] = json!(t);
    }
    body
}

/// OpenAI's reasoning models: the o-series, GPT-5 and Codex. A gateway
/// (OmniRoute, OpenRouter, LiteLLM) puts a provider before the id
/// (`cx/gpt-5-codex`, `openai/o3`), so only the last segment is read.
pub fn is_reasoning_model(model: &str) -> bool {
    let id = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    id.contains("codex")
        || id.starts_with("gpt-5")
        || ["o1", "o3", "o4"].iter().any(|p| id == *p || id.starts_with(&format!("{p}-")))
}

/// The length parameter a (lowercased) refusal names: the one it names first,
/// as in "'max_tokens' is not supported ... use 'max_completion_tokens'" or
/// "Unrecognized request argument: max_completion_tokens". Named alone, a
/// parameter counts as refused only in so many words: "max_tokens is too
/// large" is about its value, not its name.
fn refused_length_key(m: &str) -> Option<&'static str> {
    let refusal =
        ["unsupported", "not supported", "unrecognized", "unknown", "not permitted", "extra inputs", "not allowed"]
            .iter()
            .any(|w| m.contains(w));
    let (old, new) = (m.find("max_tokens"), m.find("max_completion_tokens"));
    match (old, new) {
        (Some(a), Some(b)) if a < b => Some("max_tokens"),
        (Some(_), None) if refusal => Some("max_tokens"),
        (_, Some(_)) if refusal || old.is_some() => Some("max_completion_tokens"),
        _ => None,
    }
}

/// Change the body as a refusal asks, when it names a parameter this client
/// sets: `max_tokens` for `max_completion_tokens` (OpenAI's reasoning models,
/// under any name a gateway gives them), no `temperature`, no
/// `stream_options`. True when something changed, so the request is worth
/// sending again.
pub fn adapt_to_refusal(body: &mut Value, err: &ApiError) -> bool {
    let ApiError::Http { status: 400 | 422, message, .. } = err else { return false };
    let m = message.to_ascii_lowercase();
    let Some(o) = body.as_object_mut() else { return false };
    let mut changed = false;
    if let Some(from) = refused_length_key(&m) {
        let to = if from == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
        if let Some(v) = o.remove(from) {
            o.insert(to.into(), v);
            changed = true;
        }
    }
    if m.contains("temperature") && o.remove("temperature").is_some() {
        changed = true;
    }
    if m.contains("stream_options") && o.remove("stream_options").is_some() {
        changed = true;
    }
    changed
}

fn push_message(out: &mut Vec<Value>, m: &Message) {
    match m.role {
        Role::Assistant => {
            let text: String = m.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("");
            let calls: Vec<Value> = m
                .tool_uses()
                .map(|(id, name, input)| {
                    json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}})
                })
                .collect();
            let mut msg =
                json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            out.push(msg);
        }
        Role::User => {
            let mut parts = Vec::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                        let mut text = content.to_text();
                        if is_error == &Some(true) {
                            text = format!("Error: {text}");
                        }
                        out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": text}));
                    }
                    ContentBlock::Text { text, .. } => parts.push(json!({"type": "text", "text": text})),
                    ContentBlock::Image { source: MediaSource::Base64 { media_type, data }, .. } => parts.push(
                        json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}}),
                    ),
                    ContentBlock::Image { source: MediaSource::Url { url }, .. } => {
                        parts.push(json!({"type": "image_url", "image_url": {"url": url}}))
                    }
                    _ => {}
                }
            }
            if !parts.is_empty() {
                out.push(json!({"role": "user", "content": parts}));
            }
        }
    }
}

/// Translates chat-completion chunks into Messages API stream events.
#[derive(Debug, Default)]
pub struct ChunkTranslator {
    started: bool,
    finished: bool,
    next_index: usize,
    /// (kind, block index, openai tool index)
    open: Option<(OpenKind, usize, Option<u64>)>,
    stop: Option<StopReason>,
    usage: Usage,
    model: String,
    /// A tool call was streamed: the turn ends in tool use whatever
    /// `finish_reason` says (some providers send "stop" with tool calls).
    saw_tool: bool,
}

/// Tool-call ids for providers that send none, unique in this process.
static NEXT_CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Text,
    Thinking,
    Tool,
}

impl ChunkTranslator {
    pub fn new(model: &str) -> Self {
        ChunkTranslator { model: model.into(), ..Default::default() }
    }

    fn ensure_started(&mut self, out: &mut Vec<StreamEvent>, id: Option<&str>) {
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                message: ApiMessage {
                    id: id.unwrap_or("chatcmpl").to_string(),
                    kind: "message".into(),
                    role: Role::Assistant,
                    model: self.model.clone(),
                    content: vec![],
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Usage::default(),
                },
            });
        }
    }

    fn close(&mut self, out: &mut Vec<StreamEvent>) {
        if let Some((_, index, _)) = self.open.take() {
            out.push(StreamEvent::ContentBlockStop { index });
        }
    }

    fn open(&mut self, out: &mut Vec<StreamEvent>, kind: OpenKind, block: ContentBlock, tool: Option<u64>) -> usize {
        self.close(out);
        let index = self.next_index;
        self.next_index += 1;
        out.push(StreamEvent::ContentBlockStart { index, content_block: block });
        self.open = Some((kind, index, tool));
        index
    }

    /// Feed one `data:` payload.
    pub fn push(&mut self, data: &str) -> Result<Vec<StreamEvent>, ApiError> {
        let mut out = Vec::new();
        if data.trim() == "[DONE]" {
            self.finish_into(&mut out);
            return Ok(out);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| ApiError::Parse(format!("{e}: {data}")))?;
        if let Some(err) = v.get("error") {
            return Err(ApiError::Stream {
                kind: "api_error".into(),
                message: err.get("message").and_then(Value::as_str).unwrap_or("unknown error").into(),
            });
        }
        self.ensure_started(&mut out, v.get("id").and_then(Value::as_str));
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage.input_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            self.usage.output_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
            if let Some(c) = u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64) {
                self.usage.cache_read_input_tokens = c;
                self.usage.input_tokens = self.usage.input_tokens.saturating_sub(c);
            }
        }
        let Some(choice) = v.pointer("/choices/0") else { return Ok(out) };
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        let reasoning = delta.get("reasoning_content").or_else(|| delta.get("reasoning")).and_then(Value::as_str);
        if let Some(r) = reasoning.filter(|r| !r.is_empty()) {
            let index = match self.open {
                Some((OpenKind::Thinking, i, _)) => i,
                _ => self.open(
                    &mut out,
                    OpenKind::Thinking,
                    ContentBlock::Thinking { thinking: String::new(), signature: String::new() },
                    None,
                ),
            };
            out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::ThinkingDelta { thinking: r.into() } });
        }
        if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            let index = match self.open {
                Some((OpenKind::Text, i, _)) => i,
                _ => self.open(&mut out, OpenKind::Text, ContentBlock::text(""), None),
            };
            out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::TextDelta { text: text.into() } });
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let tidx = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let index = match self.open {
                    Some((OpenKind::Tool, i, Some(t))) if t == tidx => i,
                    _ => {
                        // Without an id, results couldn't be matched to calls: give one.
                        let id = call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|i| !i.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                format!("call_forge_{}", NEXT_CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                            });
                        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
                        self.saw_tool = true;
                        self.open(
                            &mut out,
                            OpenKind::Tool,
                            ContentBlock::ToolUse { id, name, input: json!({}), cache_control: None },
                            Some(tidx),
                        )
                    }
                };
                // Arguments come as JSON text, in pieces; some providers send
                // the object itself, whole.
                let args = match call.pointer("/function/arguments") {
                    Some(Value::String(a)) => a.clone(),
                    Some(o @ Value::Object(_)) => o.to_string(),
                    _ => String::new(),
                };
                if !args.is_empty() {
                    out.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::InputJsonDelta { partial_json: args },
                    });
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop = Some(match reason {
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::Refusal,
                _ => StopReason::EndTurn,
            });
        }
        Ok(out)
    }

    /// A whole completion (`"object": "chat.completion"`), from a server that
    /// answered a streaming request without streaming: its message as one
    /// chunk, then the end.
    pub fn push_completion(&mut self, body: &Value) -> Result<Vec<StreamEvent>, ApiError> {
        let mut message = body.pointer("/choices/0/message").cloned().unwrap_or(Value::Null);
        if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for (i, c) in calls.iter_mut().enumerate() {
                if c.get("index").is_none() {
                    c["index"] = json!(i);
                }
            }
        }
        let mut chunk = json!({
            "id": body.get("id"),
            "usage": body.get("usage"),
            "choices": [{"delta": message, "finish_reason": body.pointer("/choices/0/finish_reason")}],
        });
        if let Some(e) = body.get("error") {
            chunk["error"] = e.clone();
        }
        let mut out = self.push(&chunk.to_string())?;
        self.finish_into(&mut out);
        Ok(out)
    }

    /// Close everything; idempotent.
    pub fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(out, None);
        self.close(out);
        let stop = match self.stop {
            None | Some(StopReason::EndTurn) if self.saw_tool => StopReason::ToolUse,
            s => s.unwrap_or(StopReason::EndTurn),
        };
        out.push(StreamEvent::MessageDelta {
            delta: MessageDeltaBody { stop_reason: Some(stop), stop_sequence: None },
            usage: self.usage.clone(),
        });
        out.push(StreamEvent::MessageStop);
    }
}

/// Model ids from an OpenAI-style `GET /models` answer (`{"data": [{"id": ...}]}`), sorted.
pub fn model_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> =
        body["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The limits a model-list entry reports, under the names gateways use
/// (OpenRouter `context_length`, vLLM `max_model_len`, LiteLLM `max_input_tokens`, ...).
pub fn listed_limits(entry: &Value) -> crate::models::Limits {
    let first = |paths: &[&str]| paths.iter().find_map(|p| entry.pointer(p).and_then(Value::as_u64)).filter(|n| *n > 0);
    crate::models::Limits {
        context_window: first(&[
            "/context_length",
            "/context_window",
            "/max_model_len",
            "/max_context_length",
            "/max_input_tokens",
            "/top_provider/context_length",
            "/model_info/max_input_tokens",
        ]),
        max_output: first(&[
            "/max_output_tokens",
            "/max_completion_tokens",
            "/top_provider/max_completion_tokens",
            "/model_info/max_output_tokens",
        ])
        .map(|n| n.min(u64::from(u32::MAX)) as u32),
    }
}

/// Record the limits each listed model reports, so compaction uses the real window.
pub fn record_listed_limits(body: &Value) {
    for m in body["data"].as_array().into_iter().flatten() {
        if let Some(id) = m["id"].as_str() {
            let l = listed_limits(m);
            if l != crate::models::Limits::default() {
                crate::models::listed_limits(id, l);
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn origin(&self) -> Option<crate::auth::Origin> {
        Some(self.config.origin())
    }

    fn base_url(&self) -> Option<String> {
        Some(crate::auth::display_url(&self.config.base_url))
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        let url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        let mut rb = self.http.get(&url).timeout(Duration::from_secs(15));
        if let Some(k) = &self.config.api_key {
            rb = rb.bearer_auth(k);
        }
        for (k, v) in &self.config.extra_headers {
            rb = rb.header(k, v);
        }
        let r = async {
            let res = rb.send().await.map_err(crate::messages::network_error)?;
            if !res.status().is_success() {
                return Err(http_error(res).await);
            }
            let body: Value = res.json().await.map_err(|e| ApiError::Parse(e.to_string()))?;
            record_listed_limits(&body);
            Ok(model_ids(&body))
        }
        .await;
        Some(r.map_err(|e| e.with_origin(self.config.origin())))
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let mut body = to_chat_request(&request);
        if let Some(r) = self.refused.lock().ok().and_then(|m| m.get(&request.model).cloned()) {
            apply_refused(&mut body, &r);
        }
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        // Another try with the body each refusal asks for (see `adapt_to_refusal`),
        // a few times at most: each one takes a key out or swaps a length name.
        let mut adaptations = 0;
        let mut refused_now: Vec<String> = Vec::new();
        let resp = loop {
            let mut rb = self.http.post(&url).json(&body);
            if let Some(k) = &self.config.api_key {
                rb = rb.bearer_auth(k);
            }
            for (k, v) in &self.config.extra_headers {
                rb = rb.header(k, v);
            }
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                r = rb.send() => r.map_err(crate::messages::network_error),
            };
            let res = match res {
                Ok(r) if r.status().is_success() => Ok(r),
                Ok(r) => Err(http_error(r).await),
                Err(e) => Err(e),
            };
            match res {
                Ok(r) => break r,
                Err(e)
                    if adaptations < 3 && {
                        let before = body.clone();
                        // A length name refused again after it was swapped away: a
                        // gateway turns one name into the other, so refuse both.
                        let again = match &e {
                            ApiError::Http { status: 400 | 422, message, .. } => {
                                refused_length_key(&message.to_ascii_lowercase())
                                    .filter(|k| refused_now.iter().any(|r| r == k) && body.get(k).is_none())
                            }
                            _ => None,
                        };
                        if again.is_some() {
                            for k in LENGTH_KEYS {
                                if !refused_now.iter().any(|r| r == k) {
                                    refused_now.push(k.to_string());
                                }
                            }
                        }
                        (adapt_to_refusal(&mut body, &e) || again.is_some()) && {
                            for k in dropped_keys(&before, &body) {
                                if !refused_now.contains(&k) {
                                    refused_now.push(k);
                                }
                            }
                            apply_refused(&mut body, &refused_now);
                            // Remembered for this model's later turns.
                            if let Ok(mut m) = self.refused.lock() {
                                let keys = m.entry(request.model.clone()).or_default();
                                for k in &refused_now {
                                    if !keys.contains(k) {
                                        keys.push(k.clone());
                                    }
                                }
                            }
                            true
                        }
                    } =>
                {
                    tracing::warn!(error = %e, "the endpoint refused a parameter; sending again without it");
                    adaptations += 1;
                }
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    let retry_after = match &e {
                        ApiError::Http { retry_after, .. } => *retry_after,
                        _ => None,
                    };
                    tracing::warn!(attempt, error = %e, "retrying API request");
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                        _ = tokio::time::sleep(backoff_delay(attempt, retry_after)) => {}
                    }
                    attempt += 1;
                }
                Err(e) => return Err(e.with_origin(self.config.origin())),
            }
        };
        let mut tr = ChunkTranslator::new(&request.model);
        // Asked to stream, some servers answer with the whole completion.
        let whole = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/json"));
        if whole {
            let body: Value = resp.json().await.map_err(|e| ApiError::Parse(format!("a whole completion: {e}")))?;
            let events = tr.push_completion(&body)?;
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        Ok(spawn_sse_pump(resp, cancel, move |ev| match tr.push(&ev.data) {
            Ok(events) => events.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageAccumulator;

    #[test]
    fn reads_context_windows_from_a_models_listing() {
        let body = json!({"data": [
            {"id": "router-model", "context_length": 163_840, "top_provider": {"max_completion_tokens": 8192}},
            {"id": "vllm-model", "max_model_len": 32_768},
            {"id": "plain"}
        ]});
        let l = listed_limits(&body["data"][0]);
        assert_eq!((l.context_window, l.max_output), (Some(163_840), Some(8192)));
        assert_eq!(listed_limits(&body["data"][1]).context_window, Some(32_768));
        assert_eq!(listed_limits(&body["data"][2]), crate::models::Limits::default());
        record_listed_limits(&body);
        assert_eq!(crate::models::model_info_or_default("vllm-model").context_window, 32_768);
    }

    #[test]
    fn reads_model_ids_from_a_models_listing() {
        let body =
            json!({"object": "list", "data": [{"id": "qwen3-coder"}, {"id": "llama-4"}, {"id": "llama-4"}, {"x": 1}]});
        assert_eq!(model_ids(&body), ["llama-4", "qwen3-coder"]);
        assert!(model_ids(&json!({})).is_empty());
    }
    use forge_types::{SystemBlock, ToolSpec};

    fn http_400(message: &str) -> ApiError {
        ApiError::Http {
            status: 400,
            kind: "invalid_request_error".into(),
            message: message.into(),
            retry_after: None,
            url: String::new(),
            origin: None,
        }
    }

    #[test]
    fn knows_openai_reasoning_models_behind_any_gateway() {
        for id in [
            "gpt-5",
            "gpt-5-codex",
            "gpt-5.1-codex-mini",
            "o3",
            "o4-mini",
            "o1",
            "cx/gpt-5-codex",
            "openai/o3-pro",
            "codex-mini-latest",
            "GPT-5",
        ] {
            assert!(is_reasoning_model(id), "{id}");
        }
        for id in [
            "gpt-4o",
            "gpt-4.1",
            "qwen3-coder:30b",
            "auto/best-coding",
            "oc/big-pickle",
            "o",
            "omni-moderation",
            "llama-3.3-70b",
        ] {
            assert!(!is_reasoning_model(id), "{id}");
        }
    }

    #[test]
    fn reasoning_models_get_max_completion_tokens_and_no_temperature() {
        let req = |model: &str| MessagesRequest {
            model: model.into(),
            max_tokens: 1000,
            messages: vec![Message::user_text("hi")],
            system: vec![],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: Some(0.2),
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        let b = to_chat_request(&req("cx/gpt-5-codex"));
        assert_eq!(b["max_completion_tokens"], 1000);
        assert!(b.get("max_tokens").is_none() && b.get("temperature").is_none(), "{b}");
        let b = to_chat_request(&req("gpt-4o"));
        assert_eq!(b["max_tokens"], 1000);
        assert!((b["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6, "{b}");
        assert!(b.get("max_completion_tokens").is_none());
    }

    #[test]
    fn a_refused_parameter_is_changed_once_as_asked() {
        // OpenAI's words, for a reasoning model under a name nobody recognises.
        let mut b = json!({"model": "dva/anything", "max_tokens": 64, "temperature": 0.5, "stream_options": {"include_usage": true}});
        let e = http_400("Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.");
        assert!(adapt_to_refusal(&mut b, &e));
        assert_eq!(b["max_completion_tokens"], 64);
        assert!(b.get("max_tokens").is_none());
        // Already as asked: nothing more to change, so no endless retrying.
        assert!(!adapt_to_refusal(&mut b, &e));
        let e = http_400("Unsupported value: 'temperature' does not support 0.5 with this model. Only the default (1) value is supported.");
        assert!(adapt_to_refusal(&mut b, &e) && b.get("temperature").is_none());
        // A server that only knows max_tokens, refusing the newer name.
        let mut b = json!({"max_completion_tokens": 64});
        assert!(adapt_to_refusal(&mut b, &http_400("Unrecognized request argument: max_completion_tokens")));
        assert_eq!(b["max_tokens"], 64);
        let mut b = json!({"max_tokens": 64, "stream_options": {}});
        assert!(adapt_to_refusal(&mut b, &http_400("stream_options: Extra inputs are not permitted")));
        assert!(b.get("stream_options").is_none() && b["max_tokens"] == 64);
        // A value refused, not the name: the request is left as it is.
        let mut b = json!({"max_tokens": 64000});
        assert!(!adapt_to_refusal(
            &mut b,
            &http_400("max_tokens is too large: 64000. This model supports at most 16384 completion tokens.")
        ));
        assert_eq!(b["max_tokens"], 64000);
        // Anything else is not a parameter problem.
        let mut b = json!({"max_tokens": 64});
        assert!(!adapt_to_refusal(&mut b, &http_400("The model `x` does not exist")));
        let server_error = ApiError::Http {
            status: 500,
            kind: "server_error".into(),
            message: "max_completion_tokens".into(),
            retry_after: None,
            url: String::new(),
            origin: None,
        };
        assert!(!adapt_to_refusal(&mut b, &server_error));
    }

    #[test]
    fn what_was_refused_is_applied_to_the_next_turn() {
        let before = json!({"max_tokens": 64, "temperature": 0.5, "stream": true});
        let mut after = before.clone();
        assert!(adapt_to_refusal(&mut after, &http_400("Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.")));
        let refused = dropped_keys(&before, &after);
        assert_eq!(refused, vec!["max_tokens".to_string()]);
        let mut next = json!({"max_tokens": 128, "temperature": 0.5, "stream": true});
        apply_refused(&mut next, &refused);
        assert_eq!(next, json!({"max_completion_tokens": 128, "temperature": 0.5, "stream": true}));
        let mut next = json!({"max_tokens": 128, "temperature": 0.5});
        apply_refused(&mut next, &["temperature".to_string()]);
        assert_eq!(next, json!({"max_tokens": 128}));
    }

    /// Feed chunks, return the message they make.
    fn assemble(chunks: &[&str]) -> forge_types::ApiMessage {
        let mut tr = ChunkTranslator::new("m");
        let mut acc = MessageAccumulator::default();
        for c in chunks {
            for ev in tr.push(c).unwrap() {
                acc.push(&ev).unwrap();
            }
        }
        let mut tail = vec![];
        tr.finish_into(&mut tail);
        for ev in tail {
            acc.push(&ev).unwrap();
        }
        acc.finish().unwrap()
    }

    #[test]
    fn tool_calls_end_in_tool_use_even_when_the_provider_says_stop() {
        let m = assemble(&[
            r#"{"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"Read","arguments":"{\"file_path\":\"x\"}"}}]}}]}"#,
            r#"{"id":"c","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        // No finish_reason at all, as some streams end: still tool use.
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"LS","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        // A cut-off turn stays cut off.
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"LS","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::MaxTokens));
        // Text alone ends the turn.
        let m = assemble(&[r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#]);
        assert_eq!(m.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn tool_calls_without_ids_get_distinct_ones_and_object_arguments_are_read() {
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"Read","arguments":{"file_path":"a"}}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"","function":{"name":"Read","arguments":{"file_path":"b"}}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        let calls: Vec<(String, Value)> = m
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, input, .. } => Some((id.clone(), input.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].0, calls[1].0, "{calls:?}");
        assert!(calls.iter().all(|(id, _)| !id.is_empty()));
        assert_eq!(calls[0].1, json!({"file_path": "a"}));
        assert_eq!(calls[1].1, json!({"file_path": "b"}));
    }

    #[test]
    fn a_whole_completion_reads_as_a_stream_would() {
        let body = json!({
            "id": "cmpl", "object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "Let me look.", "reasoning_content": "Check the file.",
                "tool_calls": [
                    {"id": "t1", "type": "function", "function": {"name": "Read", "arguments": "{\"file_path\":\"a\"}"}},
                    {"id": "t2", "type": "function", "function": {"name": "Read", "arguments": "{\"file_path\":\"b\"}"}}
                ]}}],
            "usage": {"prompt_tokens": 9, "completion_tokens": 5}
        });
        let mut tr = ChunkTranslator::new("m");
        let mut acc = MessageAccumulator::default();
        for ev in tr.push_completion(&body).unwrap() {
            acc.push(&ev).unwrap();
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(m.usage.input_tokens, 9);
        let tools = m.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
        assert_eq!(tools, 2, "{:?}", m.content);
        assert!(m.content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. } if text == "Let me look.")));
        assert!(m.content.iter().any(|b| matches!(b, ContentBlock::Thinking { .. })));
        // An error answered whole is an error.
        let mut tr = ChunkTranslator::new("m");
        assert!(tr.push_completion(&json!({"error": {"message": "rate limited"}})).is_err());
    }

    #[test]
    fn translates_tool_call_chunks() {
        let mut tr = ChunkTranslator::new("local");
        let mut events = vec![];
        for chunk in [
            r#"{"id":"c1","choices":[{"delta":{"content":"Let me look."}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"Read","arguments":"{\"file_"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"path\":\"/x\"}"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":4}}"#,
            "[DONE]",
        ] {
            events.extend(tr.push(chunk).unwrap());
        }
        let mut acc = MessageAccumulator::new();
        for e in &events {
            acc.push(e).unwrap();
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.to_message().text(), "Let me look.");
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        let (id, name, input) =
            m.to_message().tool_uses().map(|(a, b, c)| (a.to_string(), b.to_string(), c.clone())).next().unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("call_a", "Read"));
        assert_eq!(input["file_path"], "/x");
        assert_eq!(m.usage.input_tokens, 11);
    }

    #[test]
    fn request_translation_splits_tool_results() {
        let req = MessagesRequest {
            model: "m".into(),
            max_tokens: 100,
            messages: vec![
                Message::user_text("hi"),
                Message::assistant(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    cache_control: None,
                }]),
                Message::user(vec![ContentBlock::tool_result("t1", "a\nb", false)]),
            ],
            system: vec![SystemBlock::text("sys")],
            tools: vec![ToolSpec {
                name: "Bash".into(),
                description: "run".into(),
                input_schema: json!({"type":"object"}),
                kind: None,
                extra: Default::default(),
                cache_control: None,
            }],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        let body = to_chat_request(&req);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "Bash");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(body["tools"][0]["function"]["name"], "Bash");
    }

    /// A gateway in front of a reasoning model: it turns `max_completion_tokens`
    /// into `max_tokens`, which the model refuses. Answers with a whole JSON
    /// completion. Returns its address and the bodies it was sent.
    async fn rewriting_gateway() -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let body = loop {
                    let mut chunk = [0u8; 4096];
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break serde_json::from_slice::<Value>(&buf[end + 4..end + 4 + len]).unwrap();
                        }
                    }
                };
                log.lock().unwrap().push(body.clone());
                let mut up = body;
                if let Some(v) = up.as_object_mut().unwrap().remove("max_completion_tokens") {
                    up["max_tokens"] = v;
                }
                let (status, reply) = if up.get("max_tokens").is_some() {
                    (
                        "400 Bad Request",
                        json!({"error": {"message": "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.", "type": "invalid_request_error"}}),
                    )
                } else {
                    (
                        "200 OK",
                        json!({"id": "x", "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]}),
                    )
                };
                let out = reply.to_string();
                let head = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", out.len());
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(out.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    #[tokio::test]
    async fn a_gateway_that_refuses_both_length_names_gets_neither() {
        use futures::StreamExt;
        let (base_url, seen) = rewriting_gateway().await;
        let provider = OpenAiProvider::new(OpenAiConfig {
            base_url,
            api_key: None,
            extra_headers: vec![],
            max_retries: 0,
            timeout: Duration::from_secs(10),
            url_from: None,
            key_helper: None,
        })
        .unwrap();
        let req = || MessagesRequest {
            model: "vllm/house-reasoner".into(),
            max_tokens: 1000,
            messages: vec![Message::user_text("hi")],
            system: vec![],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        for _ in 0..2 {
            let mut events = provider.stream(req(), CancellationToken::new()).await.unwrap();
            let mut acc = MessageAccumulator::default();
            while let Some(ev) = events.next().await {
                acc.push(&ev.unwrap()).unwrap();
            }
            assert_eq!(acc.finish().unwrap().content.iter().filter_map(|b| b.as_text()).collect::<String>(), "ok");
        }
        let lengths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|b| LENGTH_KEYS.iter().filter(|k| b.get(**k).is_some()).copied().collect::<Vec<_>>())
            .collect();
        // Turn one: max_tokens refused, the swap refused too, then no limit.
        // Turn two goes straight to no limit.
        assert_eq!(lengths, vec![vec!["max_tokens"], vec!["max_completion_tokens"], vec![], vec![]]);
    }

    #[test]
    fn both_length_names_refused_means_no_limit_is_sent() {
        let mut b = json!({"max_tokens": 64, "stream": true});
        apply_refused(&mut b, &["max_tokens".into(), "max_completion_tokens".into()]);
        assert_eq!(b, json!({"stream": true}));
    }
}
