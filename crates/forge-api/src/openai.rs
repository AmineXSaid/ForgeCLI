//! OpenAI-compatible `/v1/chat/completions` endpoints (custom gateways,
//! local servers). Requests are translated from the Messages shape and the
//! streamed chunks are translated back into Anthropic stream events, so the
//! engine never sees the difference.

use std::time::Duration;

use forge_types::{
    ApiMessage, ContentBlock, Delta, MediaSource, Message, MessageDeltaBody, MessagesRequest, Role, StopReason,
    StreamEvent, Usage,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::anthropic::{http_error, spawn_sse_pump};
use crate::{backoff_delay, ApiError, EventStream, Provider};

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL up to and including `/v1` (e.g. `http://localhost:11434/v1`).
    pub base_url: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    pub timeout: Duration,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        OpenAiConfig {
            base_url: "http://localhost:8000/v1".into(),
            api_key: None,
            extra_headers: vec![],
            max_retries: 2,
            timeout: Duration::from_secs(600),
        }
    }
}

pub struct OpenAiProvider {
    config: OpenAiConfig,
    http: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(config: OpenAiConfig) -> Result<Self, ApiError> {
        let http =
            reqwest::Client::builder().timeout(config.timeout).build().map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(OpenAiProvider { config, http })
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
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    body
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

/// Translates chat-completion chunks into Anthropic stream events.
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
}

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
                        let id = call.get("id").and_then(Value::as_str).unwrap_or("call").to_string();
                        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string();
                        self.open(
                            &mut out,
                            OpenKind::Tool,
                            ContentBlock::ToolUse { id, name, input: json!({}), cache_control: None },
                            Some(tidx),
                        )
                    }
                };
                if let Some(args) =
                    call.pointer("/function/arguments").and_then(Value::as_str).filter(|a| !a.is_empty())
                {
                    out.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::InputJsonDelta { partial_json: args.into() },
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

    /// Close everything; idempotent.
    pub fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(out, None);
        self.close(out);
        out.push(StreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(self.stop.unwrap_or(StopReason::EndTurn)),
                stop_sequence: None,
            },
            usage: self.usage.clone(),
        });
        out.push(StreamEvent::MessageStop);
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let body = to_chat_request(&request);
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let mut attempt = 0;
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
                r = rb.send() => r.map_err(|e| ApiError::Network(e.to_string())),
            };
            let res = match res {
                Ok(r) if r.status().is_success() => Ok(r),
                Ok(r) => Err(http_error(r).await),
                Err(e) => Err(e),
            };
            match res {
                Ok(r) => break r,
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    tokio::time::sleep(backoff_delay(attempt, None)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        };
        let mut tr = ChunkTranslator::new(&request.model);
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
    use forge_types::{SystemBlock, ToolSpec};

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
            stream: true,
        };
        let body = to_chat_request(&req);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "Bash");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(body["tools"][0]["function"]["name"], "Bash");
    }
}
