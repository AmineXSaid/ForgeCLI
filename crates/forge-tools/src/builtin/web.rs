//! WebFetch and WebSearch.
//!
//! WebFetch downloads a page, turns HTML into Markdown and, when a backend
//! is attached, has a small model answer the call's `prompt` from the page.
//! That keeps whole pages out of the main context (GOALS pillar 1).
//! WebSearch delegates to the backend (the provider's web-search server
//! tool). Both go through permissions (`WebFetch(domain:...)` rules).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_permissions::Subject;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::str_arg;
use crate::{fit_output, Tool, ToolContext, ToolOutput, INTERRUPTED};

/// What the web tools need from a model provider.
#[async_trait::async_trait]
pub trait WebBackend: Send + Sync {
    /// Answer `prompt` from `content` (fetched from `url`) with a small, fast model.
    async fn summarize(
        &self,
        url: &str,
        content: &str,
        prompt: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String>;
    /// Whether [`WebBackend::search`] works with this provider.
    fn can_search(&self) -> bool;
    async fn search(
        &self,
        query: &str,
        allowed: &[String],
        blocked: &[String],
        cancel: &CancellationToken,
    ) -> Result<String, String>;
}

const MAX_BYTES: usize = 10 * 1024 * 1024;
const KEEP_CHARS: usize = 50_000;
const SUMMARIZE_CHARS: usize = 100_000;
const CACHE_FOR: Duration = Duration::from_secs(15 * 60);

/// Hosts never fetched: cloud metadata endpoints (SSRF).
fn forbidden_host(host: &str) -> bool {
    host.starts_with("169.254.")
        || host == "metadata.google.internal"
        || host == "metadata"
        || host == "[fd00:ec2::254]"
}

fn same_site(a: &str, b: &str) -> bool {
    a.trim_start_matches("www.") == b.trim_start_matches("www.")
}

pub struct WebFetch {
    pub backend: Option<Arc<dyn WebBackend>>,
    cache: Mutex<HashMap<String, (Instant, String)>>,
}

impl WebFetch {
    pub fn new(backend: Option<Arc<dyn WebBackend>>) -> Self {
        WebFetch { backend, cache: Mutex::new(HashMap::new()) }
    }

    /// The page as Markdown (or plain text), or a message explaining why not.
    async fn fetch(&self, url: &reqwest::Url) -> Result<String, String> {
        if let Some((at, md)) = self.cache.lock().unwrap().get(url.as_str()) {
            if at.elapsed() < CACHE_FOR {
                return Ok(md.clone());
            }
        }
        let origin = url.host_str().unwrap_or_default().to_string();
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            let host = attempt.url().host_str().unwrap_or_default().to_string();
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if same_site(&host, &origin) && !forbidden_host(&host) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        });
        let client = reqwest::Client::builder()
            .redirect(policy)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("ForgeCLI/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .get(url.clone())
            .header("accept", "text/markdown, text/html;q=0.9, text/plain;q=0.8, */*;q=0.5")
            .send()
            .await
            .map_err(|e| format!("Could not fetch {url}: {}", forge_api_error(&e)))?;
        let status = resp.status();
        if status.is_redirection() {
            let to = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("?");
            let to = url.join(to).map(|u| u.to_string()).unwrap_or(to.to_string());
            return Err(format!(
                "{url} redirects to a different host: {to}\nFetch that URL instead (it needs its own permission)."
            ));
        }
        if !status.is_success() {
            return Err(format!("{url} returned HTTP {status}."));
        }
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        let final_url = resp.url().to_string();
        let mut body: Vec<u8> = vec![];
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Reading {url} failed: {e}"))?;
            if body.len() + chunk.len() > MAX_BYTES {
                return Err(format!("{url} is larger than {} MB; not fetched.", MAX_BYTES / (1024 * 1024)));
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body);
        let textual = ctype.is_empty()
            || ctype.starts_with("text/")
            || ctype.contains("json")
            || ctype.contains("xml")
            || ctype.contains("javascript");
        if !textual {
            return Err(format!("{url} is {ctype}, not text; WebFetch reads text and HTML pages only."));
        }
        let md = if ctype.contains("html") || (ctype.is_empty() && text.trim_start().starts_with('<')) {
            crate::html::to_markdown(&text, Some(&final_url))
        } else {
            text.into_owned()
        };
        self.cache.lock().unwrap().insert(url.to_string(), (Instant::now(), md.clone()));
        Ok(md)
    }
}

fn forge_api_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg = s.to_string();
        source = s.source();
    }
    msg
}

/// Parse and normalize the URL: `http` becomes `https` except for local hosts.
pub fn normalize_url(raw: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(raw.trim()).map_err(|e| format!("Invalid URL {raw:?}: {e}"))?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let host = url.host_str().unwrap_or_default();
            let local = host == "localhost" || host.starts_with("127.") || host == "[::1]";
            if !local {
                let _ = url.set_scheme("https");
            }
        }
        s => return Err(format!("Unsupported URL scheme {s:?}: use http or https.")),
    }
    let host = url.host_str().unwrap_or_default();
    if host.is_empty() || forbidden_host(host) {
        return Err(format!("Fetching {host:?} is not allowed (cloud metadata endpoints are blocked)."));
    }
    Ok(url)
}

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "WebFetch"
    }

    fn description(&self) -> String {
        "Fetch a web page and get an answer about it.\n\n\
         - Give the full `url` and a `prompt` saying what you need from the page; a small model reads the page and \
           answers, so the whole page does not enter the conversation.\n\
         - HTML is converted to Markdown; plain text and JSON are kept as they are. http URLs are upgraded to https.\n\
         - A redirect to another host is reported, not followed: fetch the new URL yourself.\n\
         - Results are cached for 15 minutes. Pages larger than 10 MB, and binary files, are refused.\n\
         - Prefer an MCP tool for a site when one exists (they are named mcp__...)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The URL to fetch"},
                "prompt": {"type": "string", "description": "What to find out from the page"}
            },
            "required": ["url", "prompt"],
            "additionalProperties": false
        })
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        let url = str_arg(input, "url");
        Subject::Url(normalize_url(url).map(|u| u.to_string()).unwrap_or_else(|_| url.to_string()))
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        normalize_url(str_arg(input, "url")).map(|_| ())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let url = match normalize_url(str_arg(&input, "url")) {
            Ok(u) => u,
            Err(e) => return ToolOutput::error(e),
        };
        let prompt = str_arg(&input, "prompt").trim().to_string();
        let started = Instant::now();
        let page = tokio::select! {
            r = self.fetch(&url) => r,
            _ = ctx.cancel.cancelled() => return ToolOutput::error(INTERRUPTED),
        };
        let page = match page {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let bytes = page.len();
        let structured = |result: &str| json!({"url": url.as_str(), "bytes": bytes, "durationMs": started.elapsed().as_millis() as u64, "result": result});
        if let (Some(b), false) = (&self.backend, prompt.is_empty()) {
            let cut: String = page.chars().take(SUMMARIZE_CHARS).collect();
            match b.summarize(url.as_str(), &cut, &prompt, &ctx.cancel).await {
                Ok(answer) => return ToolOutput::text(answer.clone()).with_structured(structured(&answer)),
                Err(e) => tracing::warn!(error = %e, "WebFetch summary failed; returning the page"),
            }
        }
        let text = fit_output(ctx, &page, KEEP_CHARS, "page");
        ToolOutput::text(text.clone()).with_structured(structured(&text))
    }
}

pub struct WebSearch {
    pub backend: Arc<dyn WebBackend>,
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "WebSearch"
    }

    fn description(&self) -> String {
        "Search the web, for information newer than your training or outside the repository.\n\n\
         - Returns results with titles and URLs; read a page further with WebFetch.\n\
         - `allowed_domains` / `blocked_domains` limit which sites results come from.\n\
         - Cite the URLs you rely on in your answer."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 2, "description": "The search query"},
                "allowed_domains": {"type": "array", "items": {"type": "string"}, "description": "Only these domains"},
                "blocked_domains": {"type": "array", "items": {"type": "string"}, "description": "Never these domains"}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let list = |k: &str| -> Vec<String> {
            input
                .get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        let query = str_arg(&input, "query").trim().to_string();
        if query.len() < 2 {
            return ToolOutput::error("The query must have at least 2 characters.");
        }
        match self.backend.search(&query, &list("allowed_domains"), &list("blocked_domains"), &ctx.cancel).await {
            Ok(text) => ToolOutput::text(format!("Web search results for \"{query}\":\n\n{text}"))
                .with_structured(json!({"query": query})),
            Err(e) => ToolOutput::error(format!("Web search failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_upgraded_and_metadata_hosts_refused() {
        assert_eq!(normalize_url("http://docs.rs/serde").unwrap().as_str(), "https://docs.rs/serde");
        assert_eq!(normalize_url("http://localhost:8080/x").unwrap().scheme(), "http");
        assert!(normalize_url("http://169.254.169.254/latest/meta-data").unwrap_err().contains("not allowed"));
        assert!(normalize_url("file:///etc/passwd").unwrap_err().contains("Unsupported URL scheme"));
        assert!(normalize_url("not a url").is_err());
        assert!(same_site("www.example.com", "example.com") && !same_site("evil.com", "example.com"));
    }
}
