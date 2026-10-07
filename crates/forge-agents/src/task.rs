use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use forge_api::Provider;
use forge_engine::{
    Engine, EngineConfig, EngineHandle, EngineParts, EnvInfo, EventSink, ForwardSink, PermissionPrompter, TurnResult,
    TurnState,
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
    /// The parent session's sandbox, shared so `/sandbox` reaches sub-agents too.
    pub sandbox: forge_tools::SandboxCell,
    /// The session's shell (Bash tool), the same for sub-agents.
    pub shell: forge_tools::shells::ShellChoice,
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

/// What a child engine is built from, besides what it shares with the session.
pub struct ChildSpec<'a> {
    /// Engine settings (`is_subagent` is forced on).
    pub cfg: EngineConfig,
    pub tools: ToolRegistry,
    pub system: Vec<forge_types::SystemBlock>,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub sink: Arc<dyn EventSink>,
    /// A fork starts from this conversation; `None` starts it empty.
    pub seed: Option<&'a TurnState>,
    /// Checkpoint its edits under this turn of the session's file history
    /// (added with `FileHistory::add_turn`), not the user's current turn: a
    /// background subtask's edits aren't part of whatever prompt runs meanwhile.
    pub checkpoint_turn: Option<String>,
}

/// Snapshots files under one fixed turn of the session's file history.
struct TurnCheckpointer {
    history: Arc<FileHistory>,
    turn: String,
}

impl forge_tools::Checkpointer for TurnCheckpointer {
    fn before_write(&self, path: &std::path::Path) {
        self.history.snapshot_in(&self.turn, path);
    }
}

/// A child engine and its id (its transcript's session id).
pub struct Child {
    pub id: String,
    pub engine: Engine,
}

impl AgentRuntime {
    /// Settings for a fresh sub-agent on `model`: the session's base settings and the parent's
    /// current thinking budget and effort, without the run's limits.
    pub fn child_config(&self, model: &str) -> EngineConfig {
        let mut cfg = self.base.clone();
        cfg.model = model.to_string();
        cfg.fallback_models = vec![];
        cfg.max_budget_usd = None;
        cfg.max_turns = None;
        cfg.json_schema = None;
        cfg.is_subagent = true;
        cfg.initial_context = self.memory_context.clone();
        if let Some(p) = self.parent.get() {
            let rt = p.handle.runtime();
            cfg.max_thinking_tokens = rt.max_thinking_tokens;
            cfg.effort = rt.effort;
        }
        cfg
    }

    /// Build a child engine (the Task tool's agents, `/subtask`). It shares the session's
    /// provider, hooks, sandbox, working directories and file history, starts with a copy of the
    /// parent's permission rules and mode, and gets its own transcript, tool context (Bash cwd,
    /// todos, read state, shells) and conversation.
    pub fn child(&self, spec: ChildSpec<'_>) -> Result<Child, String> {
        let parent = self.parent.get().ok_or("Sub-agents are not available in this session.")?;
        let id = uuid::Uuid::new_v4().to_string();
        let transcript = match &self.store {
            Some(store) => Transcript::create(store, &self.project_dir, &id, None, true),
            None => Transcript::create(
                &SessionStore::new(PathBuf::from("/nonexistent")),
                &self.project_dir,
                &id,
                None,
                false,
            ),
        }
        .map_err(|e| e.to_string())?;
        let mut tool_ctx = ToolContext::new(&self.project_dir);
        tool_ctx.working_dirs = self.working_dirs.clone();
        tool_ctx.env = self.env.clone();
        tool_ctx.sandbox = self.sandbox.clone();
        tool_ctx.shell = self.shell.clone();
        tool_ctx.session_id = self.session_id.clone();
        if let Some(turn) = spec.checkpoint_turn {
            tool_ctx.checkpointer = Some(Arc::new(TurnCheckpointer { history: parent.history.clone(), turn }));
        }
        let mut cfg = spec.cfg;
        cfg.is_subagent = true;
        let permissions = parent.handle.permissions.read().unwrap().clone();
        let parts = EngineParts {
            provider: self.provider.clone(),
            tools: spec.tools,
            tool_ctx,
            permissions,
            hooks: self.hooks.clone(),
            prompter: spec.prompter,
            sink: spec.sink,
            transcript: Arc::new(transcript),
            history: parent.history.clone(),
            system: spec.system,
        };
        let mut engine = Engine::new(cfg, parts).map_err(|e| e.to_string())?;
        if let Some(seed) = spec.seed {
            engine.seed(seed);
        }
        Ok(Child { id, engine })
    }
}

/// How a child's turn ended: `completed`, `interrupted` or `error`.
pub fn status_of(r: &TurnResult) -> &'static str {
    if r.is_error {
        "error"
    } else if r.stop_reason.as_deref() == Some("interrupted") {
        "interrupted"
    } else {
        "completed"
    }
}

/// A child's spend, as `Engine::record_subagent_usage` takes it (`subagentUsage`).
pub fn usage_of(r: &TurnResult) -> Value {
    json!({"costUsd": r.total_cost_usd, "usage": r.usage, "modelUsage": r.model_usage})
}

/// Tools a fork lists but may not run: they would reach the person, change the session's
/// schedule, or start agents that ask through the person's prompter.
const NOT_IN_FORKS: &[(&str, &str)] = &[
    ("Task", "Background subtasks can't start other agents."),
    (
        "AskUserQuestion",
        "Nobody can answer questions in a background subtask: decide, and list your assumptions in the report.",
    ),
    ("EnterPlanMode", "Plan mode isn't available in a background subtask."),
    ("ExitPlanMode", "Plan mode isn't available in a background subtask."),
    ("CronCreate", "Background subtasks can't schedule tasks."),
    ("CronDelete", "Background subtasks can't change scheduled tasks."),
    ("ScheduleWakeup", "Background subtasks can't schedule tasks."),
];

/// A fork's tools: the parent's, in the same order and with the same text, so the request
/// prefix matches and the parent's prompt cache is read; refused tools stay listed.
pub fn fork_tools(parent: &ToolRegistry) -> ToolRegistry {
    let mut reg = parent.clone();
    for &(name, why) in NOT_IN_FORKS {
        reg.wrap(name, |inner| Arc::new(Unavailable { inner, why }));
    }
    reg
}

struct Unavailable {
    inner: Arc<dyn Tool>,
    why: &'static str,
}

#[async_trait::async_trait]
impl Tool for Unavailable {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    /// Refused before any permission check.
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn validate(&self, _input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        Err(self.why.to_string())
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        ToolOutput::error(self.why)
    }
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
        forge_tools::builtin::set_shell(&mut reg, &self.rt.shell);
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
        let dirs: Vec<PathBuf> = self.rt.working_dirs.read().unwrap().iter().skip(1).cloned().collect();
        let mut env = EnvInfo::collect(&self.rt.project_dir, &dirs, &model);
        env.shell = forge_platform::shell::env_line(&self.rt.shell);
        let mut sys = forge_types::SystemBlock::text(format!("{}\n\n{}", agent.prompt, env.render()));
        sys.cache_control = Some(forge_types::CacheControl::ephemeral());
        let spec = ChildSpec {
            cfg: self.rt.child_config(&model),
            tools: self.registry_for(&agent),
            system: vec![sys],
            prompter: parent.prompter.clone(),
            sink: Arc::new(ForwardSink { parent: self.rt.sink.clone(), parent_tool_use_id: ctx.tool_use_id.clone() }),
            seed: None,
            checkpoint_turn: None,
        };
        let Child { id: child_id, engine: mut child } = match self.rt.child(spec) {
            Ok(c) => c,
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
            "status": status_of(&result),
            "totalToolUseCount": tool_uses,
            "totalDurationMs": started.elapsed().as_millis() as u64,
            "subagentUsage": usage_of(&result),
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
