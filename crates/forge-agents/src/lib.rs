//! Sub-agents (docs/GOALS.md, pillar 1: context management).
//!
//! The `Task` tool runs an agent in a fresh context: its own conversation,
//! system prompt and tool set. Only its final report returns to the caller,
//! so exploration and side work do not fill the main context. Several Task
//! calls in one message run in parallel. Sub-agents share the session's
//! permission rules, prompt lock, file checkpoints and budget, and cannot
//! start sub-agents of their own.

mod definitions;
mod task;

pub use definitions::{builtin_agents, load_agents, parse_agent_markdown, parse_agents_json, AgentDef, AgentSource};
pub use task::{AgentRuntime, ParentLink, TaskTool};
