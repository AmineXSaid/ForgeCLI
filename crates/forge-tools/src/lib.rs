//! Tools the model can call.
//!
//! A [`Tool`] declares its schema and how it is checked (read-only or not,
//! safe to run concurrently or not, what a permission rule matches against)
//! and implements `call`. The engine owns ordering, permissions and hooks;
//! tools only do the work.

pub mod builtin;
mod context;
mod files;
pub mod html;
pub mod injection;
mod registry;
pub mod sandbox;
pub mod shells;
mod util;

pub use context::{Checkpointer, ToolContext};
pub use files::FileState;
pub use registry::ToolRegistry;
pub use util::{expand_path, fit_output, truncate_middle};

use forge_permissions::Subject;
use forge_types::{ContentBlock, ToolResultContent};
use serde_json::Value;

/// What a tool returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub content: ToolResultContent,
    pub is_error: bool,
    /// Structured result for SDK hosts (`tool_use_result` on the user message).
    pub structured: Option<Value>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        ToolOutput { content: ToolResultContent::Text(s.into()), is_error: false, structured: None }
    }

    pub fn error(s: impl Into<String>) -> Self {
        ToolOutput { content: ToolResultContent::Text(s.into()), is_error: true, structured: None }
    }

    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        ToolOutput { content: ToolResultContent::Blocks(blocks), is_error: false, structured: None }
    }

    pub fn with_structured(mut self, v: Value) -> Self {
        self.structured = Some(v);
        self
    }

    pub fn text_content(&self) -> String {
        self.content.to_text()
    }
}

/// Text shown to the model when a call is aborted by an interrupt (contract C3).
pub const INTERRUPTED: &str = "Interrupted by user";

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// Built-in description (the engine may replace it from `FORGE_PROMPTS_DIR`).
    fn description(&self) -> String;

    fn input_schema(&self) -> Value;

    /// The call cannot modify anything.
    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    /// The call may run in parallel with other concurrency-safe calls.
    fn is_concurrency_safe(&self, input: &Value) -> bool {
        self.is_read_only(input)
    }

    /// The call is a question for the person (AskUserQuestion, plan approval): it always
    /// asks, whatever the mode or rules allow (contract C8).
    fn needs_user(&self) -> bool {
        false
    }

    /// Will this call run inside the OS sandbox? Sandboxed calls need no permission prompt.
    fn sandboxed(&self, _input: &Value, _ctx: &ToolContext) -> bool {
        false
    }

    /// What permission rules match against for this call.
    fn permission_subject(&self, _input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::None
    }

    /// Reject malformed input before permissions are asked.
    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        validate_required(&self.input_schema(), input)
    }

    /// Present to the model? (e.g. tools needing an unavailable backend are hidden)
    fn is_enabled(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput;
}

/// Minimal schema check: object input, required keys present, primitive types right.
pub fn validate_required(schema: &Value, input: &Value) -> Result<(), String> {
    let Some(obj) = input.as_object() else { return Err("input must be a JSON object".into()) };
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for k in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(k) || obj[k].is_null() {
                return Err(format!("The required parameter `{k}` is missing"));
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (k, v) in obj {
            let Some(ty) = props.get(k).and_then(|p| p.get("type")).and_then(Value::as_str) else { continue };
            let ok = match ty {
                "string" => v.is_string(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => true,
            };
            if !ok && !v.is_null() {
                return Err(format!(
                    "The parameter `{k}` type is expected as `{ty}` but provided as `{}`",
                    type_name(v)
                ));
            }
        }
    }
    Ok(())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
