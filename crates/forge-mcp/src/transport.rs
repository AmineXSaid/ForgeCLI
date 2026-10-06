//! JSON-RPC transports: stdio (newline-delimited), streamable HTTP, and the
//! older HTTP+SSE pair.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_api::sse::SseDecoder;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum McpError {
    #[error("{0}")]
    Transport(String),
    #[error("{message} (JSON-RPC error {code})")]
    Rpc { code: i64, message: String },
    #[error("no response within {}s", .0.as_secs())]
    Timeout(Duration),
    #[error("the server closed the connection{0}")]
    Closed(String),
}

type Reply = Result<Value, McpError>;

/// Routes incoming messages: responses to their waiting request, server
/// requests to an answer, notifications to flags.
#[derive(Default)]
pub struct Dispatch {
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next_id: AtomicU64,
    /// `notifications/tools/list_changed` arrived.
    pub tools_changed: AtomicBool,
    /// Directories offered for `roots/list`.
    pub roots: Vec<String>,
}

impl Dispatch {
    pub fn new(roots: Vec<String>) -> Self {
        Dispatch { roots, next_id: AtomicU64::new(1), ..Default::default() }
    }

    fn register(&self) -> (u64, oneshot::Receiver<Reply>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    fn forget(&self, id: u64) {
        self.pending.lock().unwrap().remove(&id);
    }

    /// Fail every waiting request (the connection is gone).
    fn close(&self, why: &str) {
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(McpError::Closed(why.to_string())));
        }
    }

    /// Handle one incoming message (or batch); returns the answers to send back.
    pub fn incoming(&self, v: Value) -> Vec<Value> {
        if let Value::Array(items) = v {
            return items.into_iter().flat_map(|i| self.incoming(i)).collect();
        }
        let method = v.get("method").and_then(Value::as_str);
        let id = v.get("id").cloned().filter(|i| !i.is_null());
        match (method, id) {
            (None, Some(id)) => {
                let Some(n) = id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())) else { return vec![] };
                if let Some(tx) = self.pending.lock().unwrap().remove(&n) {
                    let reply = match v.get("error") {
                        Some(e) => Err(McpError::Rpc {
                            code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                            message: e.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
                        }),
                        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(reply);
                }
                vec![]
            }
            (Some(m), Some(id)) => {
                let result = match m {
                    "ping" => Ok(json!({})),
                    "roots/list" => Ok(json!({"roots": self.roots.iter().map(|r| json!({
                        "uri": format!("file://{r}"),
                        "name": Path::new(r).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    })).collect::<Vec<_>>()})),
                    _ => Err(json!({"code": -32601, "message": format!("method not supported by this client: {m}")})),
                };
                vec![match result {
                    Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                    Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
                }]
            }
            (Some(m), None) => {
                if m == "notifications/tools/list_changed" {
                    self.tools_changed.store(true, Ordering::SeqCst);
                }
                tracing::debug!(method = m, "mcp notification");
                vec![]
            }
            (None, None) => vec![],
        }
    }
}

fn request_body(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification_body(method: &str, params: Value) -> Value {
    if params.is_null() {
        json!({"jsonrpc": "2.0", "method": method})
    } else {
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    }
}

async fn await_reply(rx: oneshot::Receiver<Reply>, timeout: Duration, d: &Dispatch, id: u64) -> Reply {
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(McpError::Closed(String::new())),
        Err(_) => {
            d.forget(id);
            Err(McpError::Timeout(timeout))
        }
    }
}

#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply;
    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError>;
    /// The protocol version agreed at initialization (HTTP sends it as a header).
    fn set_protocol_version(&self, _v: &str) {}
    async fn close(&self);
    fn dispatch(&self) -> &Dispatch;
}

// ---------------------------------------------------------------- stdio

pub struct StdioTransport {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    child: Mutex<Option<Child>>,
    dispatch: Arc<Dispatch>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl StdioTransport {
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &std::collections::BTreeMap<String, String>,
        cwd: &Path,
        roots: Vec<String>,
    ) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| McpError::Transport(format!("could not start `{command}`: {e}")))?;
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take().expect("piped stdin")));
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let dispatch = Arc::new(Dispatch::new(roots));
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));

        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut t = tail.lock().unwrap();
                if t.len() == 20 {
                    t.pop_front();
                }
                t.push_back(l);
            }
        });
        let (d, w, tail) = (dispatch.clone(), stdin.clone(), stderr_tail.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    tracing::debug!(line, "mcp: non-JSON line on stdout");
                    continue;
                };
                for answer in d.incoming(v) {
                    let mut w = w.lock().await;
                    let _ = w.write_all(format!("{answer}\n").as_bytes()).await;
                    let _ = w.flush().await;
                }
            }
            // Give the stderr reader a moment to collect the last words.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let last: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
            let why = if last.is_empty() { String::new() } else { format!(": {}", last.join(" | ")) };
            d.close(&why);
        });
        Ok(StdioTransport { stdin, child: Mutex::new(Some(child)), dispatch, stderr_tail })
    }

    async fn write(&self, v: &Value) -> Result<(), McpError> {
        let mut w = self.stdin.lock().await;
        let r = async {
            w.write_all(format!("{v}\n").as_bytes()).await?;
            w.flush().await
        }
        .await;
        r.map_err(|e| {
            let last: Vec<String> = self.stderr_tail.lock().unwrap().iter().cloned().collect();
            McpError::Closed(format!(
                " ({e}){}",
                if last.is_empty() { String::new() } else { format!(": {}", last.join(" | ")) }
            ))
        })
    }
}

#[async_trait::async_trait]
impl Transport for StdioTransport {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let (id, rx) = self.dispatch.register();
        if let Err(e) = self.write(&request_body(id, method, params)).await {
            self.dispatch.forget(id);
            return Err(e);
        }
        await_reply(rx, timeout, &self.dispatch, id).await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.write(&notification_body(method, params)).await
    }

    async fn close(&self) {
        let child = self.child.lock().unwrap().take();
        if let Some(mut c) = child {
            let _ = c.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), c.wait()).await;
        }
    }

    fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }
}

// ---------------------------------------------------------------- HTTP

fn http_client() -> reqwest::Client {
    reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).build().unwrap_or_default()
}

fn header_map(headers: &std::collections::BTreeMap<String, String>) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) =
            (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v))
        {
            h.insert(name, value);
        }
    }
    h
}

/// Streamable HTTP: every message is a POST; the answer is JSON or an SSE stream.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    dispatch: Arc<Dispatch>,
}

impl HttpTransport {
    pub fn new(url: &str, headers: &std::collections::BTreeMap<String, String>, roots: Vec<String>) -> Self {
        HttpTransport {
            client: http_client(),
            url: url.to_string(),
            headers: header_map(headers),
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
            dispatch: Arc::new(Dispatch::new(roots)),
        }
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header("accept", "application/json, text/event-stream")
            .json(body);
        if let Some(s) = self.session_id.lock().unwrap().clone() {
            rb = rb.header("mcp-session-id", s);
        }
        if let Some(v) = self.protocol_version.lock().unwrap().clone() {
            rb = rb.header("mcp-protocol-version", v);
        }
        rb
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response, McpError> {
        let resp =
            self.post(body).send().await.map_err(|e| McpError::Transport(forge_api::describe_network_error(&e)))?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *self.session_id.lock().unwrap() = Some(s.to_string());
        }
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let hint = match status.as_u16() {
                401 | 403 => " (check the server's credentials: `headers` in its config)",
                404 if self.session_id.lock().unwrap().is_some() => " (the server ended the session)",
                _ => "",
            };
            return Err(McpError::Transport(format!(
                "HTTP {status}{hint}: {}",
                text.chars().take(300).collect::<String>()
            )));
        }
        Ok(resp)
    }

    /// Feed a response body (JSON or SSE) to the dispatcher until `rx` resolves or the body ends.
    async fn drain(&self, resp: reqwest::Response, rx: &mut oneshot::Receiver<Reply>) -> Option<Reply> {
        let is_sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));
        if !is_sse {
            let text = resp.text().await.ok()?;
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                self.answer(self.dispatch.incoming(v)).await;
            }
            return rx.try_recv().ok();
        }
        let mut stream = resp.bytes_stream();
        let mut dec = SseDecoder::new();
        loop {
            tokio::select! {
                r = &mut *rx => return r.ok(),
                chunk = stream.next() => match chunk {
                    Some(Ok(bytes)) => {
                        for ev in dec.push(&bytes) {
                            if let Ok(v) = serde_json::from_str::<Value>(&ev.data) {
                                let answers = self.dispatch.incoming(v);
                                self.answer(answers).await;
                            }
                        }
                    }
                    _ => return rx.try_recv().ok(),
                }
            }
        }
    }

    async fn answer(&self, answers: Vec<Value>) {
        for a in answers {
            let _ = self.post(&a).send().await;
        }
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let (id, mut rx) = self.dispatch.register();
        let body = request_body(id, method, params);
        let work = async {
            let resp = self.send(&body).await?;
            match self.drain(resp, &mut rx).await {
                Some(r) => r,
                None => Err(McpError::Transport("the server sent no response to the request".into())),
            }
        };
        match tokio::time::timeout(timeout, work).await {
            Ok(r) => {
                self.dispatch.forget(id);
                r
            }
            Err(_) => {
                self.dispatch.forget(id);
                Err(McpError::Timeout(timeout))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.send(&notification_body(method, params)).await.map(|_| ())
    }

    fn set_protocol_version(&self, v: &str) {
        *self.protocol_version.lock().unwrap() = Some(v.to_string());
    }

    async fn close(&self) {
        let sid = self.session_id.lock().unwrap().clone();
        if let Some(s) = sid {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.client.delete(&self.url).headers(self.headers.clone()).header("mcp-session-id", s).send(),
            )
            .await;
        }
    }

    fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }
}

// ---------------------------------------------------------------- HTTP + SSE (older servers)

/// The 2024-11-05 transport: a GET event stream that first names the POST
/// endpoint, then carries every response.
pub struct SseTransport {
    client: reqwest::Client,
    headers: reqwest::header::HeaderMap,
    endpoint: String,
    dispatch: Arc<Dispatch>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SseTransport {
    pub async fn connect(
        url: &str,
        headers: &std::collections::BTreeMap<String, String>,
        roots: Vec<String>,
        timeout: Duration,
    ) -> Result<Self, McpError> {
        let client = http_client();
        let headers = header_map(headers);
        let resp = client
            .get(url)
            .headers(headers.clone())
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| McpError::Transport(forge_api::describe_network_error(&e)))?;
        if !resp.status().is_success() {
            return Err(McpError::Transport(format!("HTTP {} opening the event stream", resp.status())));
        }
        let base = reqwest::Url::parse(url).map_err(|e| McpError::Transport(format!("bad url {url}: {e}")))?;
        let mut stream = resp.bytes_stream();
        let mut dec = SseDecoder::new();
        let mut early = vec![];
        let endpoint = tokio::time::timeout(timeout, async {
            while let Some(Ok(bytes)) = stream.next().await {
                for ev in dec.push(&bytes) {
                    if ev.event.as_deref() == Some("endpoint") {
                        return base.join(ev.data.trim()).map(|u| u.to_string()).ok();
                    }
                    early.push(ev);
                }
            }
            None
        })
        .await
        .map_err(|_| McpError::Timeout(timeout))?
        .ok_or_else(|| McpError::Transport("the event stream sent no `endpoint` event".into()))?;
        let dispatch = Arc::new(Dispatch::new(roots));
        let (d, c, h, ep) = (dispatch.clone(), client.clone(), headers.clone(), endpoint.clone());
        let reader = tokio::spawn(async move {
            let handle = |data: &str| serde_json::from_str::<Value>(data).map(|v| d.incoming(v)).unwrap_or_default();
            let mut answers: Vec<Value> = early.iter().flat_map(|ev| handle(&ev.data)).collect();
            loop {
                for a in answers.drain(..) {
                    let _ = c.post(&ep).headers(h.clone()).json(&a).send().await;
                }
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        for ev in dec.push(&bytes) {
                            if ev.event.as_deref().unwrap_or("message") == "message" {
                                answers.extend(handle(&ev.data));
                            }
                        }
                    }
                    _ => break,
                }
            }
            d.close("");
        });
        Ok(SseTransport { client, headers, endpoint, dispatch, reader: Mutex::new(Some(reader)) })
    }

    async fn post(&self, body: &Value) -> Result<(), McpError> {
        let resp = self
            .client
            .post(&self.endpoint)
            .headers(self.headers.clone())
            .json(body)
            .send()
            .await
            .map_err(|e| McpError::Transport(forge_api::describe_network_error(&e)))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(McpError::Transport(format!("HTTP {} posting to {}", resp.status(), self.endpoint)))
        }
    }
}

#[async_trait::async_trait]
impl Transport for SseTransport {
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let (id, rx) = self.dispatch.register();
        if let Err(e) = self.post(&request_body(id, method, params)).await {
            self.dispatch.forget(id);
            return Err(e);
        }
        await_reply(rx, timeout, &self.dispatch, id).await
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.post(&notification_body(method, params)).await
    }

    async fn close(&self) {
        if let Some(h) = self.reader.lock().unwrap().take() {
            h.abort();
        }
    }

    fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_routes_responses_requests_and_notifications() {
        let d = Dispatch::new(vec!["/work/app".into()]);
        let (id, mut rx) = d.register();
        assert!(d.incoming(json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}})).is_empty());
        assert_eq!(rx.try_recv().unwrap(), Ok(json!({"ok": true})));

        let (id, mut rx) = d.register();
        d.incoming(json!({"jsonrpc": "2.0", "id": id.to_string(), "error": {"code": -32602, "message": "bad"}}));
        assert_eq!(rx.try_recv().unwrap(), Err(McpError::Rpc { code: -32602, message: "bad".into() }));

        let answers = d.incoming(json!([
            {"jsonrpc": "2.0", "id": "s1", "method": "ping"},
            {"jsonrpc": "2.0", "id": "s2", "method": "roots/list"},
            {"jsonrpc": "2.0", "id": "s3", "method": "sampling/createMessage"},
            {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}
        ]));
        assert_eq!(answers[0], json!({"jsonrpc": "2.0", "id": "s1", "result": {}}));
        assert_eq!(answers[1]["result"]["roots"][0], json!({"uri": "file:///work/app", "name": "app"}));
        assert_eq!(answers[2]["error"]["code"], -32601);
        assert!(d.tools_changed.load(Ordering::SeqCst));

        let (_, mut rx) = d.register();
        d.close(": bye");
        assert_eq!(rx.try_recv().unwrap(), Err(McpError::Closed(": bye".into())));
    }
}
