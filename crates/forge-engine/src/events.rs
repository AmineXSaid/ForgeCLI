//! What the engine reports, and the ports it calls out through.

use forge_permissions::Suggestion;
use forge_types::{ApiMessage, Message, StreamEvent};
use serde_json::Value;

/// Everything the engine reports while a turn runs.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A raw stream event (for `--include-partial-messages` and live rendering).
    Stream { event: StreamEvent, parent_tool_use_id: Option<String> },
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

/// Contract C1: with nobody to ask, prompts are denied with instructions.
pub struct DenyPrompter;

#[async_trait::async_trait]
impl PermissionPrompter for DenyPrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
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
