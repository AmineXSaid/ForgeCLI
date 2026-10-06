//! The Messages API over HTTPS with SSE streaming.

use std::time::Duration;

use forge_types::{MessagesRequest, StreamEvent};
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::sse::SseDecoder;
use crate::{backoff_delay, ApiError, EventStream, Provider};

// Wire constants required by the Messages API: its default host, its version
// header and value, and its beta header. They are protocol, not branding.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
const VERSION_HEADER: &str = "anthropic-version";
const BETA_HEADER: &str = "anthropic-beta";

const KNOWN_EVENTS: &[&str] = &[
    "message_start",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
    "message_delta",
    "message_stop",
    "ping",
    "error",
];

#[derive(Debug, Clone)]
pub struct MessagesConfig {
    pub base_url: String,
    /// Sent as `x-api-key`.
    pub api_key: Option<String>,
    /// Sent as `Authorization: Bearer` (gateways, proxies).
    pub auth_token: Option<String>,
    pub betas: Vec<String>,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    /// Longest silence allowed between reads of a response.
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for MessagesConfig {
    fn default() -> Self {
        MessagesConfig {
            base_url: DEFAULT_BASE_URL.into(),
            api_key: None,
            auth_token: None,
            betas: vec![],
            extra_headers: vec![],
            max_retries: 3,
            // No total deadline (long generations stream for many minutes); a stall of this long between reads fails the request.
            timeout: Duration::from_secs(300),
            user_agent: format!("forgecli/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl MessagesConfig {
    /// Read `FORGE_BASE_URL`, `FORGE_API_KEY`, `FORGE_AUTH_TOKEN`,
    /// `FORGE_CUSTOM_HEADERS` ("Name: value" per line) and `FORGE_MAX_RETRIES`.
    pub fn from_env() -> Self {
        let mut c = MessagesConfig::default();
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(u) = get("FORGE_BASE_URL") {
            c.base_url = u;
        }
        c.api_key = get("FORGE_API_KEY");
        c.auth_token = get("FORGE_AUTH_TOKEN");
        if let Some(h) = get("FORGE_CUSTOM_HEADERS") {
            c.extra_headers = parse_header_lines(&h);
        }
        if let Some(n) = get("FORGE_MAX_RETRIES").and_then(|v| v.parse().ok()) {
            c.max_retries = n;
        }
        c
    }

    pub fn has_credentials(&self) -> bool {
        self.api_key.is_some() || self.auth_token.is_some()
    }

    /// Where the credential came from, for `system/init.apiKeySource`.
    pub fn key_source(&self) -> &'static str {
        if self.api_key.is_some() {
            "FORGE_API_KEY"
        } else if self.auth_token.is_some() {
            "FORGE_AUTH_TOKEN"
        } else {
            "none"
        }
    }
}

pub fn parse_header_lines(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

pub struct MessagesProvider {
    config: MessagesConfig,
    http: reqwest::Client,
}

impl MessagesProvider {
    pub fn new(config: MessagesConfig) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(config.timeout)
            .build()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(MessagesProvider { config, http })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url.trim_end_matches('/'), path)
    }

    fn headers(&self, extra_betas: &[String]) -> Result<HeaderMap, ApiError> {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(VERSION_HEADER, HeaderValue::from_static(API_VERSION));
        if let Some(k) = &self.config.api_key {
            h.insert("x-api-key", header_value(k)?);
        }
        if let Some(t) = &self.config.auth_token {
            h.insert(reqwest::header::AUTHORIZATION, header_value(&format!("Bearer {t}"))?);
        }
        let mut betas = self.config.betas.clone();
        betas.extend(extra_betas.iter().filter(|b| !self.config.betas.contains(b)).cloned());
        if !betas.is_empty() {
            h.insert(BETA_HEADER, header_value(&betas.join(","))?);
        }
        for (k, v) in &self.config.extra_headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| ApiError::Parse(e.to_string()))?;
            h.insert(name, header_value(v)?);
        }
        Ok(h)
    }

    async fn send_once(&self, body: &Value, betas: &[String]) -> Result<reqwest::Response, ApiError> {
        let resp = self
            .http
            .post(self.endpoint("/v1/messages"))
            .headers(self.headers(betas)?)
            .json(body)
            .send()
            .await
            .map_err(network_error)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(http_error(resp).await)
    }
}

/// A transport failure with its root cause ("connection refused", "timed out", ...).
pub(crate) fn network_error(e: reqwest::Error) -> ApiError {
    ApiError::Network(describe_network_error(&e))
}

/// "could not connect to <url>: <root cause>", for any reqwest failure.
pub fn describe_network_error(e: &reqwest::Error) -> String {
    let mut msg = if e.is_timeout() {
        "request timed out".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        "request failed".to_string()
    };
    if let Some(url) = e.url() {
        msg.push_str(&format!(" to {}", url.as_str().split('?').next().unwrap_or("")));
    }
    let mut source = std::error::Error::source(e);
    let mut last = None;
    while let Some(s) = source {
        last = Some(s.to_string());
        source = s.source();
    }
    if let Some(cause) = last {
        msg.push_str(&format!(": {cause}"));
    }
    msg
}

fn header_value(v: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(v).map_err(|e| ApiError::Parse(format!("invalid header value: {e}")))
}

/// Turn a non-2xx response into an [`ApiError::Http`].
pub(crate) async fn http_error(resp: reqwest::Response) -> ApiError {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .map(Duration::from_secs_f64);
    let text = resp.text().await.unwrap_or_default();
    let (kind, message) = match serde_json::from_str::<Value>(&text) {
        Ok(v) => (
            v.pointer("/error/type").and_then(Value::as_str).unwrap_or("error").to_string(),
            v.pointer("/error/message").and_then(Value::as_str).map(str::to_string).unwrap_or(text.clone()),
        ),
        Err(_) => ("error".to_string(), text),
    };
    ApiError::Http { status, kind, message, retry_after }
}

/// Parse one SSE payload; `Ok(None)` for event types this client does not know.
pub fn parse_event(data: &str) -> Result<Option<StreamEvent>, ApiError> {
    let v: Value = serde_json::from_str(data).map_err(|e| ApiError::Parse(format!("{e}: {data}")))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
    if !KNOWN_EVENTS.contains(&kind.as_str()) {
        tracing::debug!(kind = %kind, "skipping unknown stream event");
        return Ok(None);
    }
    serde_json::from_value(v).map(Some).map_err(|e| ApiError::Parse(format!("{kind}: {e}")))
}

/// Pump an SSE byte stream into a channel of [`StreamEvent`]s.
pub(crate) fn spawn_sse_pump<F>(resp: reqwest::Response, cancel: CancellationToken, mut parse: F) -> EventStream
where
    F: FnMut(&crate::sse::SseEvent) -> Vec<Result<StreamEvent, ApiError>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<StreamEvent, ApiError>>(64);
    tokio::spawn(async move {
        let mut body = resp.bytes_stream();
        let mut decoder = SseDecoder::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(Err(ApiError::Cancelled)).await;
                    return;
                }
                c = body.next() => c,
            };
            let (events, finished) = match chunk {
                Some(Ok(bytes)) => (decoder.push(&bytes), false),
                Some(Err(e)) => {
                    let _ = tx.send(Err(ApiError::Network(e.to_string()))).await;
                    return;
                }
                None => (decoder.finish().into_iter().collect(), true),
            };
            for ev in &events {
                for item in parse(ev) {
                    if tx.send(item).await.is_err() {
                        return;
                    }
                }
            }
            if finished {
                return;
            }
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

#[async_trait::async_trait]
impl Provider for MessagesProvider {
    fn name(&self) -> &str {
        "messages"
    }

    async fn stream(&self, mut request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        if !self.config.has_credentials() {
            return Err(ApiError::MissingCredentials);
        }
        request.stream = true;
        let body = serde_json::to_value(&request).map_err(|e| ApiError::Parse(e.to_string()))?;
        let mut attempt = 0;
        let resp = loop {
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                r = self.send_once(&body, &request.betas) => r,
            };
            match res {
                Ok(r) => break r,
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    let wait = backoff_delay(
                        attempt,
                        match &e {
                            ApiError::Http { retry_after, .. } => *retry_after,
                            _ => None,
                        },
                    );
                    tracing::warn!(attempt, ?wait, error = %e, "retrying API request");
                    attempt += 1;
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(e) => return Err(e),
            }
        };
        Ok(spawn_sse_pump(resp, cancel, |ev| match parse_event(&ev.data) {
            Ok(Some(e)) => vec![Ok(e)],
            Ok(None) => vec![],
            Err(e) => vec![Err(e)],
        }))
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        let mut body = serde_json::to_value(request).ok()?;
        let obj = body.as_object_mut()?;
        for k in ["max_tokens", "stream", "metadata", "temperature"] {
            obj.remove(k);
        }
        let resp = self
            .http
            .post(self.endpoint("/v1/messages/count_tokens"))
            .headers(self.headers(&request.betas).ok()?)
            .json(&body)
            .send()
            .await
            .ok()?;
        let v: Value = resp.json().await.ok()?;
        v.get("input_tokens").and_then(Value::as_u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_events_are_skipped() {
        assert!(parse_event(r#"{"type":"something_new","x":1}"#).unwrap().is_none());
        assert!(matches!(parse_event(r#"{"type":"ping"}"#).unwrap(), Some(StreamEvent::Ping)));
    }

    #[test]
    fn custom_headers_parse() {
        let h = parse_header_lines("X-A: 1\nbad line\nX-B:two");
        assert_eq!(h, vec![("X-A".into(), "1".into()), ("X-B".into(), "two".into())]);
    }
}
