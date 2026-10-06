//! Model Context Protocol support.
//!
//! - [`config`]: where servers are declared, which are trusted, `${VAR}` expansion.
//! - [`transport`]: stdio, streamable HTTP and HTTP+SSE JSON-RPC transports.
//! - [`client`]: one server session (initialize, tools, resources, prompts).
//! - [`tools`]: server tools as Forge tools (`mcp__<server>__<tool>`), plus
//!   `ListMcpResourcesTool` and `ReadMcpResourceTool`.
//! - [`manager`]: connects every server at startup and reports status.
//! - [`server`]: `forge mcp serve`.

pub mod client;
pub mod config;
pub mod manager;
pub mod server;
pub mod tools;
pub mod transport;

pub use client::{ConnectOptions, McpClient, ToolInfo};
pub use config::{resolve, NamedServer, Resolved, Scope, ServerConfig};
pub use manager::{McpManager, Status};
pub use transport::McpError;
