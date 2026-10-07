//! A scripted provider for tests: each request pops the next [`MockTurn`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_types::{
    ApiErrorBody, ApiMessage, ContentBlock, Delta, MessageDeltaBody, MessagesRequest, Role, StopReason, StreamEvent,
    Usage,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::{ApiError, EventStream, Provider};

/// One scripted response.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum MockTurn {
    /// Stream this message, pausing `delay` between events.
    Message { message: ApiMessage, delay: Duration },
    /// Fail the request before streaming.
    HttpError { status: u16, kind: String, message: String },
    /// Stream a `message_start` and then an `error` event.
    StreamError { kind: String, message: String },
}

impl MockTurn {
    pub fn text(text: &str) -> Self {
        Self::blocks(vec![ContentBlock::text(text)], StopReason::EndTurn)
    }

    /// One assistant message with any number of `(name, input)` tool calls.
    pub fn tools(calls: &[(&str, Value)]) -> Self {
        Self::blocks(
            calls
                .iter()
                .enumerate()
                .map(|(i, (name, input))| ContentBlock::ToolUse {
                    id: format!("toolu_{:02}_{}", i, name.to_ascii_lowercase()),
                    name: (*name).into(),
                    input: input.clone(),
                    cache_control: None,
                })
                .collect(),
            StopReason::ToolUse,
        )
    }

    pub fn tool(name: &str, input: Value) -> Self {
        Self::tools(&[(name, input)])
    }

    pub fn blocks(content: Vec<ContentBlock>, stop: StopReason) -> Self {
        MockTurn::Message {
            message: ApiMessage {
                id: format!("msg_mock_{}", forge_types::new_uuid()),
                kind: "message".into(),
                role: Role::Assistant,
                model: "mock-model".into(),
                content,
                stop_reason: Some(stop),
                stop_sequence: None,
                usage: Usage { input_tokens: 100, output_tokens: 20, ..Default::default() },
            },
            delay: Duration::ZERO,
        }
    }

    pub fn with_delay(self, d: Duration) -> Self {
        match self {
            MockTurn::Message { message, .. } => MockTurn::Message { message, delay: d },
            other => other,
        }
    }

    pub fn with_usage(self, usage: Usage) -> Self {
        match self {
            MockTurn::Message { mut message, delay } => {
                message.usage = usage;
                MockTurn::Message { message, delay }
            }
            other => other,
        }
    }

    pub fn http_error(status: u16, kind: &str) -> Self {
        MockTurn::HttpError { status, kind: kind.into(), message: format!("mock {kind}") }
    }
}

type Responder = Box<dyn Fn(&MessagesRequest) -> MockTurn + Send + Sync>;

/// A provider that answers from a script and records every request.
pub struct MockProvider {
    queue: Mutex<VecDeque<MockTurn>>,
    responder: Option<Responder>,
    requests: Arc<Mutex<Vec<MessagesRequest>>>,
    /// What `list_models` answers (`None`: the endpoint doesn't list models).
    models: Mutex<Option<Vec<String>>>,
}

impl MockProvider {
    pub fn new(turns: impl IntoIterator<Item = MockTurn>) -> Self {
        MockProvider {
            queue: Mutex::new(turns.into_iter().collect()),
            responder: None,
            requests: Default::default(),
            models: Mutex::new(None),
        }
    }

    /// Make `list_models` answer with these ids, as an endpoint that lists its models would.
    pub fn set_models(&self, ids: &[&str]) {
        *self.models.lock().unwrap() = Some(ids.iter().map(|s| s.to_string()).collect());
    }

    /// Answer each request with a function of it (used when the queue is empty).
    pub fn with_responder(f: impl Fn(&MessagesRequest) -> MockTurn + Send + Sync + 'static) -> Self {
        MockProvider {
            queue: Mutex::new(VecDeque::new()),
            responder: Some(Box::new(f)),
            requests: Default::default(),
            models: Mutex::new(None),
        }
    }

    pub fn push(&self, turn: MockTurn) {
        self.queue.lock().unwrap().push_back(turn);
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<MessagesRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn request_log(&self) -> Arc<Mutex<Vec<MessagesRequest>>> {
        self.requests.clone()
    }
}

/// The stream-event sequence the API would send for `msg`.
pub fn events_for(msg: &ApiMessage) -> Vec<StreamEvent> {
    let mut start = msg.clone();
    start.content = vec![];
    start.stop_reason = None;
    start.usage = Usage { input_tokens: msg.usage.input_tokens, output_tokens: 1, ..msg.usage.clone() };
    let mut out = vec![StreamEvent::MessageStart { message: start }];
    for (index, block) in msg.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text, .. } => {
                out.push(StreamEvent::ContentBlockStart { index, content_block: ContentBlock::text("") });
                // Split into two deltas so tests exercise accumulation.
                let mid = text.char_indices().nth(text.chars().count() / 2).map(|(i, _)| i).unwrap_or(0);
                for part in [&text[..mid], &text[mid..]] {
                    if !part.is_empty() {
                        out.push(StreamEvent::ContentBlockDelta {
                            index,
                            delta: Delta::TextDelta { text: part.into() },
                        });
                    }
                }
            }
            ContentBlock::Thinking { thinking, signature } => {
                out.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlock::Thinking { thinking: String::new(), signature: String::new() },
                });
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::ThinkingDelta { thinking: thinking.clone() },
                });
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::SignatureDelta { signature: signature.clone() },
                });
            }
            ContentBlock::ToolUse { id, name, input, .. } => {
                out.push(StreamEvent::ContentBlockStart {
                    index,
                    content_block: ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: Value::Object(Default::default()),
                        cache_control: None,
                    },
                });
                out.push(StreamEvent::ContentBlockDelta {
                    index,
                    delta: Delta::InputJsonDelta { partial_json: serde_json::to_string(input).unwrap() },
                });
            }
            other => out.push(StreamEvent::ContentBlockStart { index, content_block: other.clone() }),
        }
        out.push(StreamEvent::ContentBlockStop { index });
    }
    out.push(StreamEvent::MessageDelta {
        delta: MessageDeltaBody { stop_reason: msg.stop_reason, stop_sequence: None },
        usage: Usage { output_tokens: msg.usage.output_tokens, ..Default::default() },
    });
    out.push(StreamEvent::MessageStop);
    out
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        self.models.lock().unwrap().clone().map(Ok)
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let turn = {
            let next = self.queue.lock().unwrap().pop_front();
            match (next, &self.responder) {
                (Some(t), _) => t,
                (None, Some(f)) => f(&request),
                (None, None) => MockTurn::text("(mock script exhausted)"),
            }
        };
        self.requests.lock().unwrap().push(request.clone());
        let (message, delay) = match turn {
            MockTurn::HttpError { status, kind, message } => {
                return Err(ApiError::Http { status, kind, message, retry_after: None })
            }
            MockTurn::StreamError { kind, message } => {
                let start = events_for(&MockTurn::text("").into_message().0).remove(0);
                let items = vec![Ok(start), Ok(StreamEvent::Error { error: ApiErrorBody { kind, message } })];
                return Ok(Box::pin(futures::stream::iter(items)));
            }
            MockTurn::Message { mut message, delay } => {
                message.model = request.model.clone();
                (message, delay)
            }
        };
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in events_for(&message) {
                if !delay.is_zero() {
                    tokio::select! {
                        _ = cancel.cancelled() => { let _ = tx.send(Err(ApiError::Cancelled)).await; return; }
                        _ = tokio::time::sleep(delay) => {}
                    }
                } else if cancel.is_cancelled() {
                    let _ = tx.send(Err(ApiError::Cancelled)).await;
                    return;
                }
                if tx.send(Ok(ev)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        Some(serde_json::to_string(request).map(|s| s.len() as u64 / 4).unwrap_or(0))
    }
}

impl MockTurn {
    fn into_message(self) -> (ApiMessage, Duration) {
        match self {
            MockTurn::Message { message, delay } => (message, delay),
            _ => unreachable!("only message turns carry a message"),
        }
    }
}
