use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use forge_api::Provider;
use forge_engine::{
    Engine, EngineConfig, EngineHandle, EngineParts, EnvInfo, EventSink, ForwardSink, PermissionPrompter,
};
use forge_hooks::HookRunner;
use forge_permissions::Subject;
use forge_session::{FileHistory, SessionStore, Transcript};
use forge_tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use forge_types::{ContentBlock, MessageContent};
use serde_json::{json, Value};

use crate::definitions::AgentDef;

/// What a sub-agent inherits from the session that runs it (set once the main engine exists).
pub struct ParentLink {
    pub handle: EngineHandle,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub history: Arc<FileHistory>,
}

/// Everything the Task tool needs to start sub-agents.
pub struct AgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub agents: Vec<AgentDef>,
    pub project_dir: PathBuf,
    pub working_dirs: Arc<RwLock<Vec<PathBuf>>>,
    pub env: Arc<std::collections::HashMap<String, String>>,
    /// The session's shell sandbox, inherited by sub-agents.
    pub sandbox: Option<Arc<forge_tools::sandbox::SandboxPolicy>>,
    /// Tools beyond the built-ins (MCP servers' tools), offered to sub-agents too.
    pub extra_tools: Vec<Arc<dyn Tool>>,
    /// Where sub-agent transcripts go (`None` = not persisted).
    pub store: Option<SessionStore>,
    pub session_id: String,
    pub hooks: HookRunner,
    /// The main session's sink; sub-agent messages are forwarded with `parent_tool_use_id`.
    pub sink: Arc<dyn EventSink>,
    /// Engine settings shared with the main session (pricing, output cap, compaction).
    pub base: EngineConfig,
    /// Memory files, given to sub-agents as context.
    pub memory_context: Option<String>,
    pub parent: OnceLock<ParentLink>,
}

pub struct TaskTool {
    pub rt: Arc<AgentRuntime>,
}

impl TaskTool {
    fn agent(&self, name: &str) -> Option<&AgentDef> {
        let n = if name.trim().is_empty() { "general-purpose" } else { name.trim() };
        self.rt.agents.iter().find(|a| a.name == n || a.name.eq_ignore_ascii_case(n))
    }

    fn registry_for(&self, agent: &AgentDef) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        forge_tools::builtin::register_core(&mut reg);
        for t in &self.rt.extra_tools {
            reg.register(t.clone());
        }
        if let Some(allowed) = &agent.tools {
            // `mcp__server` in an agent's list covers every tool of that server.
            reg.retain(|n| allowed.iter().any(|a| a == n || n.starts_with(&format!("{a}__"))));
        }
        // No nested sub-agents, and only the main session talks to the person.
        reg.retain(|n| !matches!(n, "Task" | "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"));
        reg
    }
}

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self) -> String {
        let mut s = String::from(
            "Run a sub-agent on a task in its own context, and get back only its final report.\n\n\
             Use it for:\n\
             - open-ended searches across many files, where you only need the conclusion;\n\
             - investigations or side work whose details would crowd your context;\n\
             - independent pieces of work that can run in parallel (several Task calls in one message run \
               concurrently).\n\n\
             The sub-agent cannot see this conversation. Give it a complete, self-contained prompt: the goal, \
             what you already know, constraints, and exactly what to report back. Its report is returned to \
             you, not shown to the user, so relay what matters. Read a specific known file yourself instead \
             of starting an agent.\n\nAvailable agents (`subagent_type`):\n",
        );
        for a in &self.rt.agents {
            let tools = match &a.tools {
                Some(t) => t.join(", "),
                None => "all tools".into(),
            };
            s.push_str(&format!("- {}: {} (Tools: {tools})\n", a.name, a.description));
        }
        s
    }

    fn input_schema(&self) -> Value {
        let names: Vec<&str> = self.rt.agents.iter().map(|a| a.name.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "A short (3-5 word) description of the task"},
                "prompt": {"type": "string", "description": "The complete, self-contained task for the agent"},
                "subagent_type": {"type": "string", "enum": names, "description": "Which agent to run (default general-purpose)"}
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        })
    }

    /// Launching an agent needs no prompt: every tool it calls is checked on its own.
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(input.get("subagent_type").and_then(Value::as_str).unwrap_or("general-purpose").to_string())
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        forge_tools::validate_required(&self.input_schema(), input)?;
        let name = input.get("subagent_type").and_then(Value::as_str).unwrap_or("");
        if self.agent(name).is_none() {
            let names: Vec<&str> = self.rt.agents.iter().map(|a| a.name.as_str()).collect();
            return Err(format!("Unknown subagent_type {name:?}. Available: {}", names.join(", ")));
        }
        let _ = ctx;
        Ok(())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(parent) = self.rt.parent.get() else {
            return ToolOutput::error("Sub-agents are not available in this session.");
        };
        let agent = self.agent(input.get("subagent_type").and_then(Value::as_str).unwrap_or("")).cloned().unwrap();
        let prompt = input.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
        let started = Instant::now();

        let model = match &agent.model {
            Some(m) => forge_api::resolve_model(m),
            None => parent.handle.model(),
        };
        let child_id = uuid::Uuid::new_v4().to_string();
        let transcript = match &self.rt.store {
            Some(store) => Transcript::create(store, &self.rt.project_dir, &child_id, None, true),
            None => Transcript::create(
                &SessionStore::new(PathBuf::from("/nonexistent")),
                &self.rt.project_dir,
                &child_id,
                None,
                false,
            ),
        };
        let transcript = match transcript {
            Ok(t) => Arc::new(t),
            Err(e) => return ToolOutput::error(format!("Could not start the sub-agent: {e}")),
        };
        let mut tool_ctx = ToolContext::new(&self.rt.project_dir);
        tool_ctx.working_dirs = self.rt.working_dirs.clone();
        tool_ctx.env = self.rt.env.clone();
        tool_ctx.sandbox = self.rt.sandbox.clone();
        tool_ctx.session_id = self.rt.session_id.clone();

        let dirs: Vec<PathBuf> = self.rt.working_dirs.read().unwrap().iter().skip(1).cloned().collect();
        let env = EnvInfo::collect(&self.rt.project_dir, &dirs, &model);
        let system_text = format!("{}\n\n{}", agent.prompt, env.render());
        let mut sys = forge_types::SystemBlock::text(system_text);
        sys.cache_control = Some(forge_types::CacheControl::ephemeral());

        let mut cfg = self.rt.base.clone();
        cfg.model = model.clone();
        cfg.fallback_models = vec![];
        cfg.max_budget_usd = None;
        cfg.max_turns = None;
        cfg.json_schema = None;
        cfg.is_subagent = true;
        cfg.initial_context = self.rt.memory_context.clone();
        cfg.max_thinking_tokens = parent.handle.runtime.read().unwrap().max_thinking_tokens;
        cfg.effort = parent.handle.runtime.read().unwrap().effort.clone();
        let permissions = parent.handle.permissions.read().unwrap().clone();
        let parts = EngineParts {
            provider: self.rt.provider.clone(),
            tools: self.registry_for(&agent),
            tool_ctx,
            permissions,
            hooks: self.rt.hooks.clone(),
            prompter: parent.prompter.clone(),
            sink: Arc::new(ForwardSink { parent: self.rt.sink.clone(), parent_tool_use_id: ctx.tool_use_id.clone() }),
            transcript,
            history: parent.history.clone(),
            system: vec![sys],
        };
        let mut child = match Engine::new(cfg, parts) {
            Ok(e) => e,
            Err(e) => return ToolOutput::error(format!("Could not start the sub-agent: {e}")),
        };
        // Interrupting the caller interrupts the agent.
        let handle = child.handle();
        let cancel = ctx.cancel.clone();
        let watcher = tokio::spawn(async move {
            cancel.cancelled().await;
            handle.interrupt();
        });
        let result = child.submit(MessageContent::Text(prompt)).await;
        watcher.abort();

        let tool_uses = child
            .state
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .count();
        let structured = json!({
            "agentType": agent.name,
            "agentId": child_id,
            "status": if result.is_error { "error" } else if result.stop_reason.as_deref() == Some("interrupted") { "interrupted" } else { "completed" },
            "totalToolUseCount": tool_uses,
            "totalDurationMs": started.elapsed().as_millis() as u64,
            "subagentUsage": {
                "costUsd": result.total_cost_usd,
                "usage": result.usage,
                "modelUsage": result.model_usage,
            },
        });
        let text = result.result.clone().unwrap_or_default();
        if result.is_error {
            let detail = if text.is_empty() { result.errors.join("; ") } else { text };
            return ToolOutput::error(format!("The {} agent failed: {detail}", agent.name)).with_structured(structured);
        }
        if result.stop_reason.as_deref() == Some("interrupted") {
            return ToolOutput::error(forge_tools::INTERRUPTED).with_structured(structured);
        }
        let text = if text.trim().is_empty() { "(the agent finished without a report)".to_string() } else { text };
        ToolOutput::text(text).with_structured(structured)
    }
}
