//! Model providers.
//!
//! Every backend implements [`Provider`]: given a [`MessagesRequest`] it
//! yields the Messages API stream-event sequence (`message_start`, block
//! start/delta/stop, `message_delta`, `message_stop`). Other backends
//! translate into that sequence, so the agent loop sees one protocol.

pub mod accumulate;
pub mod messages;
pub mod mock;
pub mod models;
pub mod openai;
pub mod sse;

use std::pin::Pin;
use std::time::Duration;

use forge_types::{MessagesRequest, StreamEvent};
use futures::Stream;
use tokio_util::sync::CancellationToken;

pub use accumulate::{MessageAccumulator, TRUNCATED_INPUT};
pub use messages::{describe_network_error, MessagesConfig, MessagesProvider};
pub use mock::{MockProvider, MockTurn};
pub use models::{resolve_model, ModelInfo, ThinkingStyle};
pub use openai::{OpenAiConfig, OpenAiProvider};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ApiError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("API error {status} ({kind}): {message}")]
    Http { status: u16, kind: String, message: String, retry_after: Option<Duration> },
    #[error("network error: {0}")]
    Network(String),
    #[error("stream error ({kind}): {message}")]
    Stream { kind: String, message: String },
    #[error("could not parse API response: {0}")]
    Parse(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("no credentials: set FORGE_API_KEY or FORGE_AUTH_TOKEN")]
    MissingCredentials,
}

impl ApiError {
    /// Whether retrying the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Http { status, .. } => matches!(status, 408 | 409 | 429 | 500..=599),
            ApiError::Network(_) => true,
            ApiError::Stream { kind, .. } => kind == "overloaded_error" || kind == "api_error",
            _ => false,
        }
    }

    /// An error retrying won't fix: credentials, billing, a missing model.
    pub fn is_unrecoverable(&self) -> bool {
        match self {
            ApiError::MissingCredentials => true,
            ApiError::Http { status, kind, message, .. } => {
                matches!(status, 401..=404)
                    || matches!(kind.as_str(), "authentication_error" | "permission_error" | "billing_error")
                    || message.to_ascii_lowercase().contains("credit balance")
            }
            _ => false,
        }
    }

    /// Overloaded (529) - the trigger for `--fallback-model`.
    pub fn is_overloaded(&self) -> bool {
        match self {
            ApiError::Http { status, kind, .. } => *status == 529 || kind == "overloaded_error",
            ApiError::Stream { kind, .. } => kind == "overloaded_error",
            _ => false,
        }
    }

    /// The next useful action for a person reading this error.
    pub fn hint(&self) -> Option<&'static str> {
        Some(match self {
            ApiError::Http { status: 401 | 403, .. } => {
                "Check FORGE_API_KEY (or FORGE_AUTH_TOKEN for a gateway); `forge doctor` shows what is configured."
            }
            ApiError::Http { status: 404, .. } => "Check the model name (--model or FORGE_MODEL) and FORGE_BASE_URL.",
            ApiError::Http { status: 413, .. } => "The request is too large: run /compact or start a new session.",
            ApiError::Http { status: 429, .. } => "Rate limited: wait a moment and retry, or lower parallel runs.",
            e if e.is_overloaded() => "The API is overloaded: retry later, or pass --fallback-model.",
            e if e.is_prompt_too_long() => "The conversation is too long: run /compact or start a new session.",
            ApiError::Http { status: 500..=599, .. } => "The API failed on its side: retry in a moment.",
            ApiError::Network(_) => "Check FORGE_BASE_URL, your network connection and proxy settings.",
            ApiError::MissingCredentials => "Run `forge doctor` to see what is configured.",
            _ => return None,
        })
    }

    /// One line for people: what failed, the cause, and what to do next.
    pub fn describe(&self) -> String {
        match self.hint() {
            Some(h) => format!("{self}. {h}"),
            None => self.to_string(),
        }
    }

    /// The prompt does not fit the context window.
    pub fn is_prompt_too_long(&self) -> bool {
        match self {
            ApiError::Http { status: 400, message, .. } => {
                let m = message.to_ascii_lowercase();
                m.contains("prompt is too long") || m.contains("context window") || m.contains("too many tokens")
            }
            _ => false,
        }
    }
}

/// A source of model turns.
/// Run one request to completion and return the whole message (for side
/// requests such as summaries; the agent loop streams instead).
pub async fn complete(
    provider: &dyn Provider,
    request: MessagesRequest,
    cancel: &CancellationToken,
) -> Result<forge_types::ApiMessage, ApiError> {
    use futures::StreamExt;
    let mut stream = provider.stream(request, cancel.clone()).await?;
    let mut acc = MessageAccumulator::new();
    loop {
        let next = tokio::select! {
            n = stream.next() => n,
            _ = cancel.cancelled() => return Err(ApiError::Cancelled),
        };
        match next {
            Some(Ok(ev)) => acc.push(&ev)?,
            Some(Err(e)) => return Err(e),
            None => break,
        }
    }
    acc.finish()
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Short identifier (`messages`, `openai`, `mock`).
    fn name(&self) -> &str;

    /// Start a streaming request. Implementations retry transient failures
    /// that happen before the first event; once events flow, errors surface
    /// in the stream.
    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError>;

    /// Count input tokens for a request, if the backend supports it.
    async fn count_tokens(&self, _request: &MessagesRequest) -> Option<u64> {
        None
    }
}

/// Exponential backoff with an upper bound; honours `retry-after` when given.
pub fn backoff_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d.min(Duration::from_secs(60));
    }
    let base = 500u64.saturating_mul(1u64 << attempt.min(6));
    Duration::from_millis(base.min(32_000))
}
