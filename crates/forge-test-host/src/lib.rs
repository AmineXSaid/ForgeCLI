//! Test support for end-to-end runs of the `forge` binary.
//!
//! * [`MockApi`] is a tiny HTTP server that answers `POST /v1/messages` with
//!   scripted SSE responses and records every request body.
//! * [`Host`] spawns `forge` with stream-json in and out and talks to it the
//!   way an IDE extension does.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_api::MockTurn;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A scripted Messages API endpoint.
pub struct MockApi {
    pub url: String,
    turns: Arc<Mutex<VecDeque<MockTurn>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl MockApi {
    pub async fn start(turns: Vec<MockTurn>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let turns = Arc::new(Mutex::new(VecDeque::from(turns)));
        let requests = Arc::new(Mutex::new(vec![]));
        let (t, r) = (turns.clone(), requests.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let (t, r) = (t.clone(), r.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    let (head_end, len) = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            break (pos + 4, len);
                        }
                    };
                    while buf.len() < head_end + len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                    r.lock().unwrap().push(body.clone());
                    let turn = t.lock().unwrap().pop_front().unwrap_or_else(|| MockTurn::text("(mock api exhausted)"));
                    let response = match turn {
                        MockTurn::Message { mut message, delay } => {
                            message.model = body["model"].as_str().unwrap_or("mock").to_string();
                            let _ = sock
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                                )
                                .await;
                            for ev in forge_api::mock::events_for(&message) {
                                let data = serde_json::to_string(&ev).unwrap();
                                let kind =
                                    serde_json::to_value(&ev).unwrap()["type"].as_str().unwrap_or("").to_string();
                                if !delay.is_zero() {
                                    tokio::time::sleep(delay).await;
                                }
                                if sock.write_all(format!("event: {kind}\ndata: {data}\n\n").as_bytes()).await.is_err()
                                {
                                    return;
                                }
                            }
                            return;
                        }
                        MockTurn::HttpError { status, kind, message } => {
                            let body =
                                serde_json::json!({"type": "error", "error": {"type": kind, "message": message}})
                                    .to_string();
                            format!("HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                        }
                        MockTurn::StreamError { kind, message } => {
                            let err = serde_json::json!({"type": "error", "error": {"type": kind, "message": message}})
                                .to_string();
                            format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\nevent: error\ndata: {err}\n\n")
                        }
                    };
                    let _ = sock.write_all(response.as_bytes()).await;
                });
            }
        });
        MockApi { url, turns, requests }
    }

    pub fn push(&self, turn: MockTurn) {
        self.turns.lock().unwrap().push_back(turn);
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// Path of the built `forge` binary (set by Cargo for integration tests).
pub fn forge_bin() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_BIN_EXE_forge").unwrap_or_else(|_| "forge".into()))
}

/// A `forge` process driven over stream-json.
pub struct Host {
    pub child: Child,
    stdin: Option<ChildStdin>,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    /// Every line read so far.
    pub transcript: Vec<Value>,
}

/// Environment for a hermetic run: its own FORGE_HOME and the mock endpoint.
pub fn command(bin: &Path, cwd: &Path, home: &Path, api: &str, args: &[&str]) -> Command {
    let mut c = Command::new(bin);
    c.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("FORGE_HOME", home.join(".forge"))
        .env("FORGE_BASE_URL", api)
        .env("FORGE_API_KEY", "test-key")
        .env("FORGE_MAX_RETRIES", "0")
        .kill_on_drop(true);
    c
}

impl Host {
    pub fn spawn(mut cmd: Command) -> Self {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = cmd.spawn().expect("spawn forge");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Host { child, stdin, stdout, transcript: vec![] }
    }

    pub async fn send(&mut self, v: Value) {
        let line = format!("{}\n", serde_json::to_string(&v).unwrap());
        self.stdin.as_mut().expect("stdin open").write_all(line.as_bytes()).await.unwrap();
    }

    pub async fn send_user(&mut self, text: &str) {
        self.send(serde_json::json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null, "session_id": ""})).await;
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line, or `None` at EOF / timeout.
    pub async fn next(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(Duration::from_secs(20), self.stdout.next_line()).await.ok()?.ok()??;
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("non-JSON line from forge ({e}): {line}"));
        self.transcript.push(v.clone());
        Some(v)
    }

    /// Read until a line satisfies `pred`; returns it.
    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            match self.next().await {
                Some(v) if pred(&v) => return v,
                Some(_) => {}
                None => panic!("stream ended before the expected line; got: {:#?}", self.transcript),
            }
        }
    }

    pub async fn until_type(&mut self, ty: &str) -> Value {
        self.until(|v| v["type"] == ty).await
    }

    pub async fn wait(mut self) -> (i32, Vec<Value>) {
        self.close_stdin();
        while self.next().await.is_some() {}
        let code = tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .ok()
            .and_then(|r| r.ok())
            .and_then(|s| s.code())
            .unwrap_or(-1);
        (code, self.transcript)
    }
}
