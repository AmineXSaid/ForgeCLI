//! What the engine reports, and the ports it calls out through.

use forge_permissions::Suggestion;
use forge_types::{ApiMessage, Message, StreamEvent};
use serde_json::Value;

/// Everything the engine reports while a turn runs.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A raw stream event (for `--include-partial-messages` and live rendering).
    Stream { event: StreamEvent, parent_tool_use_id: Option<String> },
    /// The user's prompt was accepted and stored (for `--replay-user-messages`).
    PromptAccepted { message: Message, uuid: String },
    /// A complete assistant message.
    Assistant { message: ApiMessage, uuid: String, parent_tool_use_id: Option<String> },
    /// A user-role message the engine added: a tool result or a synthetic note.
    User {
        message: Message,
        uuid: String,
        tool_use_result: Option<Value>,
        is_meta: bool,
        parent_tool_use_id: Option<String>,
    },
    /// `system/<subtype>` (model_fallback, compact_boundary, api_retry, ...).
    System { subtype: String, data: Value },
    /// A tool has been running for a while.
    ToolProgress { tool_use_id: String, tool_name: String, elapsed_secs: f64 },
    /// Text for the person at the keyboard (hook messages, warnings). Not sent to the model.
    Notice { level: NoticeLevel, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: EngineEvent);
}

/// Discards events.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

/// Collects events (tests).
#[derive(Default)]
pub struct VecSink(pub std::sync::Mutex<Vec<EngineEvent>>);

impl EventSink for VecSink {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl VecSink {
    pub fn take(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// A permission prompt for the person or the SDK host.
#[derive(Debug, Clone)]
pub struct PermissionPrompt {
    pub tool_name: String,
    pub tool_use_id: String,
    pub input: Value,
    pub reason: String,
    pub suggestions: Vec<Suggestion>,
    pub blocked_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionAnswer {
    Allow {
        updated_input: Option<Value>,
        /// Permission updates to apply (rules, mode, directories), in SDK shape.
        updated_permissions: Vec<Value>,
    },
    Deny {
        message: String,
        /// Stop the whole turn, not just this tool.
        interrupt: bool,
    },
}

/// Answers permission prompts (TUI dialog, SDK host, or nobody).
#[async_trait::async_trait]
pub trait PermissionPrompter: Send + Sync {
    async fn ask(&self, prompt: PermissionPrompt) -> PermissionAnswer;
}

/// Wraps a prompter so that only one prompt is open at a time, across the
/// main conversation and every sub-agent that shares it (contract C2).
pub struct SerializedPrompter {
    inner: std::sync::Arc<dyn PermissionPrompter>,
    lock: tokio::sync::Mutex<()>,
}

impl SerializedPrompter {
    pub fn new(inner: std::sync::Arc<dyn PermissionPrompter>) -> Self {
        SerializedPrompter { inner, lock: tokio::sync::Mutex::new(()) }
    }
}

#[async_trait::async_trait]
impl PermissionPrompter for SerializedPrompter {
    async fn ask(&self, prompt: PermissionPrompt) -> PermissionAnswer {
        let _g = self.lock.lock().await;
        self.inner.ask(prompt).await
    }
}

/// Forwards a sub-agent's conversation to the parent's sink, tagged with the
/// parent's `Task` tool-use id (stream-json `parent_tool_use_id`).
pub struct ForwardSink {
    pub parent: std::sync::Arc<dyn EventSink>,
    pub parent_tool_use_id: String,
}

impl EventSink for ForwardSink {
    fn emit(&self, event: EngineEvent) {
        let tag = Some(self.parent_tool_use_id.clone());
        match event {
            EngineEvent::Assistant { message, uuid, .. } => {
                self.parent.emit(EngineEvent::Assistant { message, uuid, parent_tool_use_id: tag })
            }
            EngineEvent::User { message, uuid, tool_use_result, is_meta, .. } => {
                self.parent.emit(EngineEvent::User { message, uuid, tool_use_result, is_meta, parent_tool_use_id: tag })
            }
            EngineEvent::Notice { .. } => self.parent.emit(event),
            // Partial chunks, prompts and system events of a sub-agent stay inside it.
            _ => {}
        }
    }
}

/// Contract C1: with nobody to ask, prompts are denied with instructions.
pub struct DenyPrompter;

#[async_trait::async_trait]
impl PermissionPrompter for DenyPrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
        // Contract C8: no person to answer.
        let message = match p.tool_name.as_str() {
            "AskUserQuestion" => Some(
                "There is no interactive user in this run, so nobody can answer. Proceed with your best judgement \
                 and state the assumptions you made."
                    .to_string(),
            ),
            "ExitPlanMode" => Some(
                "There is no interactive user in this run, so the plan cannot be approved and the session stays in \
                 plan mode. Give the plan as your final answer."
                    .to_string(),
            ),
            "EnterPlanMode" => {
                Some("There is no interactive user in this run to agree to plan mode. Continue without it.".to_string())
            }
            _ => None,
        };
        if let Some(message) = message {
            return PermissionAnswer::Deny { message, interrupt: false };
        }
        PermissionAnswer::Deny {
            message: format!(
                "Permission to use {} was denied: running non-interactively and nobody can approve it ({}). To allow it, \
                 rerun with --allowedTools \"{}\", choose a --permission-mode, or add an allow rule to settings.",
                p.tool_name, p.reason, p.tool_name
            ),
            interrupt: false,
        }
    }
}
