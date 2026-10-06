use serde_json::{json, Value};

use crate::{Tool, ToolContext, ToolOutput};

pub struct TodoWrite;

#[async_trait::async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self) -> String {
        "Create and update a structured task list for the current session. Use it for work with three or more \
         steps: send the whole list each time, keep exactly one task `in_progress`, and mark tasks `completed` \
         as soon as they are done. Each task has `content` (imperative, e.g. \"Run tests\"), `status` and \
         `activeForm` (present continuous, e.g. \"Running tests\")."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string", "minLength": 1},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                            "activeForm": {"type": "string", "minLength": 1}
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // Changes only session state, never files: no prompt needed.
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        crate::validate_required(&self.input_schema(), input)?;
        for (i, t) in input["todos"].as_array().into_iter().flatten().enumerate() {
            let status = t.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(status, "pending" | "in_progress" | "completed") {
                return Err(format!("todos[{i}].status must be pending, in_progress or completed"));
            }
            if t.get("content").and_then(Value::as_str).unwrap_or("").trim().is_empty() {
                return Err(format!("todos[{i}].content must not be empty"));
            }
        }
        Ok(())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let new: Vec<Value> = input["todos"].as_array().cloned().unwrap_or_default();
        let old = std::mem::replace(&mut *ctx.todos.lock().unwrap(), new.clone());
        ToolOutput::text(
            "Todos have been modified successfully. Keep using the todo list to track your progress, and proceed \
             with the current tasks if applicable.",
        )
        .with_structured(json!({"oldTodos": old, "newTodos": new}))
    }
}
