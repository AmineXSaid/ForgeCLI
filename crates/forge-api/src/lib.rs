//! Model providers.
//!
//! Every backend implements [`Provider`]: given a [`MessagesRequest`] it
//! yields the Messages API stream-event sequence (`message_start`, block
//! start/delta/stop, `message_delta`, `message_stop`). Other backends
//! translate into that sequence, so the agent loop sees one protocol.

pub mod accumulate;
pub mod auth;
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
    #[error("API error {status} ({kind}){}: {message}", from_url(url))]
    Http {
        status: u16,
        kind: String,
        message: String,
        retry_after: Option<Duration>,
        /// The URL that answered (no credentials or query), or empty.
        url: String,
        /// Which endpoint and key the request used, for the hint.
        origin: Option<Box<auth::Origin>>,
    },
    #[error("network error: {message}")]
    Network { message: String, origin: Option<Box<auth::Origin>> },
    #[error("stream error ({kind}): {message}")]
    Stream { kind: String, message: String },
    #[error("could not parse API response: {0}")]
    Parse(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("no credentials: set FORGE_API_KEY or FORGE_AUTH_TOKEN")]
    MissingCredentials,
}

fn from_url(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else {
        format!(" from {url}")
    }
}

impl ApiError {
    /// A transport failure.
    pub fn network(message: impl Into<String>) -> Self {
        ApiError::Network { message: message.into(), origin: None }
    }

    /// Attach the endpoint a request went to, so the hint names the right variable.
    pub fn with_origin(mut self, o: auth::Origin) -> Self {
        match &mut self {
            ApiError::Http { origin, .. } | ApiError::Network { origin, .. } => *origin = Some(Box::new(o)),
            _ => {}
        }
        self
    }

    /// The endpoint refused the credentials (or none were set): the user must fix a key.
    pub fn is_auth_failure(&self) -> bool {
        match self {
            ApiError::MissingCredentials => true,
            ApiError::Http { status, kind, .. } => {
                matches!(status, 401 | 403) || matches!(kind.as_str(), "authentication_error" | "permission_error")
            }
            _ => false,
        }
    }

    /// Whether retrying the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Http { status, .. } => matches!(status, 408 | 409 | 429 | 500..=599),
            ApiError::Network { .. } => true,
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
    pub fn hint(&self) -> Option<String> {
        let origin = match self {
            ApiError::Http { origin, .. } | ApiError::Network { origin, .. } => origin.as_deref().copied(),
            _ => None,
        };
        Some(match self {
            ApiError::Http { status: 401 | 403, .. } => match origin {
                Some(o) => o.auth_hint(),
                None => "Check the endpoint's key; `forge doctor` shows what is configured.".into(),
            },
            ApiError::Http { status: 404, .. } => match origin {
                Some(o) => o.not_found_hint(),
                None => "Check the model name (--model or FORGE_MODEL) and the endpoint URL.".into(),
            },
            ApiError::Http { status: 413, .. } => {
                "The request is too large: run /compact or start a new session.".into()
            }
            ApiError::Http { status: 429, .. } => {
                "Rate limited: wait a moment and retry, or lower maxConcurrentRequests.".into()
            }
            e if e.is_overloaded() => "The API is overloaded: retry later, or pass --fallback-model.".into(),
            e if e.is_prompt_too_long() => "The conversation is too long: run /compact or start a new session.".into(),
            ApiError::Http { status: 500..=599, .. } => "The API failed on its side: retry in a moment.".into(),
            ApiError::Network { .. } => match origin {
                Some(o) => o.network_hint(),
                None => "Check the endpoint URL, your network connection and proxy settings.".into(),
            },
            ApiError::MissingCredentials => "Run `forge doctor` to see what is configured.".into(),
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

    /// The models this endpoint offers, for `/model`. `None`: it doesn't
    /// list them (any model id can still be set by hand).
    /// `forge doctor --probe`: one authenticated request that changes nothing
    /// (the model list); `Some(Ok(n))` with the number of models listed.
    async fn probe(&self) -> Option<Result<usize, ApiError>> {
        self.list_models().await.map(|r| r.map(|m| m.len()))
    }

    /// The endpoint and where its key came from (`None` for test providers).
    fn origin(&self) -> Option<auth::Origin> {
        None
    }

    /// The base URL requests go to, when it isn't the provider's default.
    fn base_url(&self) -> Option<String> {
        None
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
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
