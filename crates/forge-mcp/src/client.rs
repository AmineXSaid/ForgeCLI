//! An MCP client session: initialize, then tools, resources and prompts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::ServerConfig;
use crate::transport::{HttpTransport, McpError, SseTransport, StdioTransport, Transport};

/// The protocol revision Forge speaks; older servers answer with theirs.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub cwd: PathBuf,
    /// For connecting and initializing (`MCP_TIMEOUT`, default 30 s).
    pub timeout: Duration,
    /// For each request after that (`MCP_TOOL_TIMEOUT`, default 10 min).
    pub request_timeout: Duration,
}

impl ConnectOptions {
    pub fn new(cwd: &Path) -> Self {
        let ms = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or(Duration::from_millis(d))
        };
        ConnectOptions {
            cwd: cwd.to_path_buf(),
            timeout: ms("MCP_TIMEOUT", 30_000),
            request_timeout: ms("MCP_TOOL_TIMEOUT", 600_000),
        }
    }
}

/// One tool as the server describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// `annotations.readOnlyHint`: used only to run calls in parallel, never to skip a prompt.
    pub read_only_hint: bool,
}

pub struct McpClient {
    pub name: String,
    transport: Box<dyn Transport>,
    pub server_info: Value,
    pub capabilities: Value,
    pub instructions: Option<String>,
    pub protocol_version: String,
    request_timeout: Duration,
}

impl McpClient {
    pub async fn connect(name: &str, config: &ServerConfig, opts: &ConnectOptions) -> Result<Self, McpError> {
        let roots = vec![opts.cwd.display().to_string()];
        let transport: Box<dyn Transport> = match config {
            ServerConfig::Stdio { command, args, env } => {
                Box::new(StdioTransport::spawn(command, args, env, &opts.cwd, roots)?)
            }
            ServerConfig::Http { url, headers } => Box::new(HttpTransport::new(url, headers, roots)),
            ServerConfig::Sse { url, headers } => {
                Box::new(SseTransport::connect(url, headers, roots, opts.timeout).await?)
            }
        };
        let init = transport
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"roots": {"listChanged": false}},
                    "clientInfo": {"name": "forge", "title": "ForgeCLI", "version": env!("CARGO_PKG_VERSION")},
                }),
                opts.timeout,
            )
            .await;
        let init = match init {
            Ok(v) => v,
            Err(e) => {
                transport.close().await;
                return Err(e);
            }
        };
        let version = init.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION).to_string();
        if !SUPPORTED_VERSIONS.contains(&version.as_str()) {
            tracing::warn!(server = name, version, "MCP server speaks an unknown protocol version; trying anyway");
        }
        transport.set_protocol_version(&version);
        transport.notify("notifications/initialized", Value::Null).await?;
        Ok(McpClient {
            name: name.to_string(),
            transport,
            server_info: init.get("serverInfo").cloned().unwrap_or(Value::Null),
            capabilities: init.get("capabilities").cloned().unwrap_or(Value::Null),
            instructions: init.get("instructions").and_then(Value::as_str).map(str::to_string),
            protocol_version: version,
            request_timeout: opts.request_timeout,
        })
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.get(capability).is_some_and(|c| !c.is_null())
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.transport.request(method, params, self.request_timeout).await
    }

    /// Every page of a `*/list` method.
    async fn list_all(&self, method: &str, key: &str) -> Result<Vec<Value>, McpError> {
        let mut out = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request(method, params).await?;
            out.extend(page.get(key).and_then(Value::as_array).cloned().unwrap_or_default());
            cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        if !self.has("tools") {
            return Ok(vec![]);
        }
        Ok(self
            .list_all("tools/list", "tools")
            .await?
            .into_iter()
            .filter_map(|t| {
                Some(ToolInfo {
                    name: t.get("name")?.as_str()?.to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    read_only_hint: t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect())
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({"name": name, "arguments": arguments})).await
    }

    /// Tell the server a request was abandoned (best effort).
    pub async fn cancelled(&self, reason: &str) {
        let _ = self.transport.notify("notifications/cancelled", json!({"reason": reason})).await;
    }

    pub async fn list_resources(&self) -> Result<Vec<Value>, McpError> {
        if !self.has("resources") {
            return Ok(vec![]);
        }
        self.list_all("resources/list", "resources").await
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Value, McpError> {
        self.request("resources/read", json!({"uri": uri})).await
    }

    pub async fn list_prompts(&self) -> Result<Vec<Value>, McpError> {
        if !self.has("prompts") {
            return Ok(vec![]);
        }
        self.list_all("prompts/list", "prompts").await
    }

    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request("prompts/get", json!({"name": name, "arguments": arguments})).await
    }

    /// The server announced a new tool list since it was last read.
    pub fn tools_changed(&self) -> bool {
        self.transport.dispatch().tools_changed.swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn close(&self) {
        self.transport.close().await;
    }
}
