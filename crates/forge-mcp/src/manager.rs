//! Connects every configured server at startup and exposes their tools.

use std::sync::Arc;

use forge_tools::Tool;
use serde_json::{json, Value};

use crate::client::{ConnectOptions, McpClient, ToolInfo};
use crate::config::{NamedServer, Resolved};
use crate::tools::{ListResources, McpTool, ReadResource};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connected,
    Failed(String),
    /// A `.mcp.json` server the user has not approved (or disabled).
    Skipped(String),
}

impl Status {
    /// The SDK status word: `connected`, `failed`, `needs-auth`, `pending`, or `disabled`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Connected => "connected",
            Status::Failed(_) => "failed",
            Status::Skipped(_) => "disabled",
        }
    }
}

pub struct ServerEntry {
    pub name: String,
    pub scope: &'static str,
    pub transport: &'static str,
    pub status: Status,
    pub client: Option<Arc<McpClient>>,
    pub tools: Vec<ToolInfo>,
    /// `prompts/list` entries; they become `/mcp__<server>__<prompt>` commands.
    pub prompts: Vec<Value>,
}

#[derive(Default)]
pub struct McpManager {
    pub servers: Vec<ServerEntry>,
}

impl McpManager {
    /// Connect to every server at once; a failing server is reported, not fatal.
    pub async fn connect(resolved: &Resolved, opts: &ConnectOptions) -> Self {
        let env = |k: &str| std::env::var(k).ok();
        let futs = resolved.servers.iter().map(|s: &NamedServer| {
            let opts = opts.clone();
            async move {
                let mut entry = ServerEntry {
                    name: s.name.clone(),
                    scope: s.scope.as_str(),
                    transport: s.config.transport(),
                    status: Status::Connected,
                    client: None,
                    tools: vec![],
                    prompts: vec![],
                };
                let config = match s.config.expanded(&env) {
                    Ok(c) => c,
                    Err(e) => {
                        entry.status = Status::Failed(e);
                        return entry;
                    }
                };
                match McpClient::connect(&s.name, &config, &opts).await {
                    Ok(c) => match c.list_tools().await {
                        Ok(tools) => {
                            entry.tools = tools;
                            entry.prompts = c.list_prompts().await.unwrap_or_default();
                            entry.client = Some(Arc::new(c));
                        }
                        Err(e) => {
                            c.close().await;
                            entry.status = Status::Failed(format!("tools/list: {e}"));
                        }
                    },
                    Err(e) => entry.status = Status::Failed(e.to_string()),
                }
                entry
            }
        });
        let mut servers: Vec<ServerEntry> = futures::future::join_all(futs).await;
        for s in &resolved.skipped {
            servers.push(ServerEntry {
                name: s.name.clone(),
                scope: "project",
                transport: "",
                status: Status::Skipped(s.reason.clone()),
                client: None,
                tools: vec![],
                prompts: vec![],
            });
        }
        McpManager { servers }
    }

    pub fn clients(&self) -> Vec<Arc<McpClient>> {
        self.servers.iter().filter_map(|s| s.client.clone()).collect()
    }

    /// Every server tool, plus the resource tools when a server offers resources.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = vec![];
        for s in &self.servers {
            if let Some(c) = &s.client {
                for t in &s.tools {
                    out.push(Arc::new(McpTool::new(&s.name, t.clone(), c.clone())));
                }
            }
        }
        let with_resources: Vec<Arc<McpClient>> = self.clients().into_iter().filter(|c| c.has("resources")).collect();
        if !with_resources.is_empty() {
            out.push(Arc::new(ListResources { clients: with_resources.clone() }));
            out.push(Arc::new(ReadResource { clients: with_resources }));
        }
        out
    }

    /// Slash-command names of every server prompt: `mcp__<server>__<prompt>`.
    pub fn prompt_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .flat_map(|s| {
                s.prompts
                    .iter()
                    .filter_map(|p| p.get("name").and_then(Value::as_str))
                    .map(|p| crate::tools::tool_name(&s.name, p))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Run `/mcp__<server>__<prompt> args`: the prompt's messages as one text. Arguments
    /// fill the prompt's declared arguments in order; the last one takes the rest.
    pub async fn get_prompt(&self, command: &str, args: &str) -> Option<Result<String, String>> {
        for s in &self.servers {
            let Some(c) = &s.client else { continue };
            for p in &s.prompts {
                let Some(name) = p.get("name").and_then(Value::as_str) else { continue };
                if crate::tools::tool_name(&s.name, name) != command {
                    continue;
                }
                let declared: Vec<String> = p
                    .get("arguments")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter().filter_map(|x| x.get("name").and_then(Value::as_str).map(str::to_string)).collect()
                    })
                    .unwrap_or_default();
                let words: Vec<&str> = args.split_whitespace().collect();
                let mut map = serde_json::Map::new();
                for (i, d) in declared.iter().enumerate() {
                    let v = if i + 1 == declared.len() {
                        words.get(i..).map(|w| w.join(" "))
                    } else {
                        words.get(i).map(|w| w.to_string())
                    };
                    if let Some(v) = v.filter(|v| !v.is_empty()) {
                        map.insert(d.clone(), json!(v));
                    }
                }
                let r = c.get_prompt(name, Value::Object(map)).await.map_err(|e| e.to_string()).map(|v| {
                    v.get("messages")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|m| {
                            crate::tools::convert_content(
                                &json!({"content": [m.get("content").cloned().unwrap_or(Value::Null)]}),
                            )
                            .text_content()
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                });
                return Some(r);
            }
        }
        None
    }

    /// `[{name, status}]` for `system/init` and `mcp_status`.
    pub fn status(&self) -> Vec<(String, String)> {
        self.servers.iter().map(|s| (s.name.clone(), s.status.as_str().to_string())).collect()
    }

    /// The `mcp_status` control response's `mcpServers` list.
    pub fn status_json(&self) -> Value {
        json!(self
            .servers
            .iter()
            .map(|s| {
                let mut v = json!({"name": s.name, "status": s.status.as_str(), "scope": s.scope});
                if let Some(c) = &s.client {
                    v["serverInfo"] = c.server_info.clone();
                    v["tools"] = json!(s.tools.iter().map(|t| json!({"name": t.name})).collect::<Vec<_>>());
                }
                if let Status::Failed(e) | Status::Skipped(e) = &s.status {
                    v["error"] = json!(e);
                }
                v
            })
            .collect::<Vec<_>>())
    }

    /// Server instructions for the system prompt (what each server says about itself).
    pub fn instructions(&self) -> Option<String> {
        let parts: Vec<String> = self
            .servers
            .iter()
            .filter_map(|s| {
                let text = s.client.as_ref()?.instructions.as_ref()?.trim().to_string();
                (!text.is_empty()).then(|| format!("## {}\n{}", s.name, text.chars().take(4000).collect::<String>()))
            })
            .collect();
        (!parts.is_empty()).then(|| format!("# MCP server instructions\n\n{}", parts.join("\n\n")))
    }

    /// Warnings for the person: servers that failed or were skipped.
    pub fn warnings(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter_map(|s| match &s.status {
                Status::Failed(e) => Some(format!("MCP server {} failed to start: {e}", s.name)),
                Status::Skipped(r) if r != "disabled" => Some(format!("MCP server {} (.mcp.json) {r}", s.name)),
                _ => None,
            })
            .collect()
    }

    pub async fn shutdown(&self) {
        for c in self.clients() {
            c.close().await;
        }
    }
}
