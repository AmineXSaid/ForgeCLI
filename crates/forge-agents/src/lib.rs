//! Sub-agents (docs/GOALS.md, pillar 1: context management).
//!
//! The `Task` tool runs an agent in a fresh context: its own conversation,
//! system prompt and tool set. Only its final report returns to the caller,
//! so exploration and side work do not fill the main context. Several Task
//! calls in one message run in parallel. Sub-agents share the session's
//! permission rules, prompt lock, file checkpoints and budget, and cannot
//! start sub-agents of their own.

pub mod attach;
pub mod commands;
mod definitions;
pub mod frontmatter;
pub mod plugins;
pub mod skills;
pub mod styles;
mod task;

pub use commands::{load_commands, CommandDef};
pub use definitions::{builtin_agents, load_agents, parse_agent_markdown, parse_agents_json, AgentDef, AgentSource};
pub use plugins::{load_plugins, Plugin};
pub use skills::{load_skills, SkillDef, SkillTool};
pub use styles::{load_styles, OutputStyle};
pub use task::{fork_tools, status_of, usage_of, AgentRuntime, Child, ChildSpec, ParentLink, TaskTool};
