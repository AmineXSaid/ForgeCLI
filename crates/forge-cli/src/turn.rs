//! One user input: a slash command handled here, or a turn for the model.

use std::sync::Arc;

use forge_core::slash::{command_text, Slash, SlashContext};
use forge_engine::{Engine, TurnResult};
use forge_types::MessageContent;

/// What slash commands can reach: custom commands, skills and MCP prompts.
pub struct Commands {
    pub commands: Vec<forge_agents::CommandDef>,
    pub skills: Vec<forge_agents::SkillDef>,
    pub mcp: Option<Arc<forge_mcp::McpManager>>,
}

pub async fn run(engine: &mut Engine, cmds: &Commands, content: MessageContent) -> TurnResult {
    let Some(text) = command_text(&content) else { return engine.submit(content).await };
    let cwd = engine.tool_ctx().project_dir.clone();
    let ctx = SlashContext { commands: &cmds.commands, skills: &cmds.skills, mcp: cmds.mcp.clone(), cwd: &cwd };
    match ctx.dispatch(&text).await {
        Slash::NotACommand => engine.submit(content).await,
        Slash::Prompt(p) => engine.submit(MessageContent::Text(p)).await,
        Slash::Compact(instructions) => match engine.compact(instructions.as_deref()).await {
            Ok(info) => engine
                .local_result(format!("Compacted the conversation (about {} tokens before).", info.pre_tokens), false),
            Err(e) => engine.local_result(format!("Could not compact: {e}"), true),
        },
        Slash::Clear => {
            engine.clear();
            engine.local_result("Conversation cleared.", false)
        }
        Slash::Cost => engine.local_result(engine.cost_report(), false),
        Slash::Local(t) => engine.local_result(t, false),
        Slash::Error(e) => engine.local_result(e, true),
    }
}
