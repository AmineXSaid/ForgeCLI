//! Tools that talk to the person: AskUserQuestion, EnterPlanMode and
//! ExitPlanMode. Each always asks (contract C8). The answer comes back in
//! the call's input (`answers`), and the engine switches permission modes
//! for the plan tools.

use serde_json::{json, Value};

use crate::{Tool, ToolContext, ToolOutput};

pub struct AskUserQuestion;

#[async_trait::async_trait]
impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn description(&self) -> String {
        "Ask the user 1-4 multiple-choice questions when a decision is genuinely theirs: requirements that are \
         ambiguous, a choice between approaches with real trade-offs, or preferences you cannot infer.\n\n\
         - Each question has a short `header` (at most 12 characters) and 2-4 `options` with a `label` and a \
           `description`; the user can always answer in their own words instead.\n\
         - Put the recommended option first and add \"(Recommended)\" to its label.\n\
         - Set `multiSelect` when several options can apply.\n\
         - Do not ask what you can find out from the code, and do not use it to ask for approval of a plan \
           (ExitPlanMode does that)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array", "minItems": 1, "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {"type": "string", "description": "The full question, ending with ?"},
                            "header": {"type": "string", "description": "A short label (max 12 characters)"},
                            "options": {
                                "type": "array", "minItems": 2, "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string"},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {"type": "boolean", "default": false}
                        },
                        "required": ["question", "header", "options", "multiSelect"]
                    }
                },
                "answers": {"type": "object", "description": "Filled in by the user interface", "additionalProperties": {"type": "string"}}
            },
            "required": ["questions"]
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        let qs = input.get("questions").and_then(Value::as_array).ok_or("questions must be a list")?;
        if qs.is_empty() || qs.len() > 4 {
            return Err("ask 1 to 4 questions".into());
        }
        for q in qs {
            let n = q.get("options").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            if !(2..=4).contains(&n) {
                return Err("each question needs 2 to 4 options".into());
            }
            if q.get("question").and_then(Value::as_str).unwrap_or_default().trim().is_empty() {
                return Err("each question needs its text".into());
            }
        }
        Ok(())
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let Some(answers) = input.get("answers").and_then(Value::as_object).filter(|a| !a.is_empty()) else {
            return ToolOutput::error(
                "No answers were given. Proceed with your best judgement and state your assumptions.",
            );
        };
        let lines: Vec<String> = answers
            .iter()
            .map(|(q, a)| format!("\"{q}\" = \"{}\"", a.as_str().map(str::to_string).unwrap_or_else(|| a.to_string())))
            .collect();
        ToolOutput::text(format!("The user answered: {}. Continue with these answers in mind.", lines.join(", ")))
            .with_structured(json!({"questions": input.get("questions"), "answers": answers}))
    }
}

pub struct EnterPlanMode;

#[async_trait::async_trait]
impl Tool for EnterPlanMode {
    fn name(&self) -> &str {
        "EnterPlanMode"
    }

    fn description(&self) -> String {
        "Switch to plan mode before a large or risky change: explore read-only and design the approach, then \
         present it with ExitPlanMode for the user's approval. Asks the user first. Skip it for small, clear tasks."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        ToolOutput::text(
            "Plan mode is on. Explore with read-only tools, design the approach, and present it with ExitPlanMode; \
             do not change files until the plan is approved.",
        )
        .with_structured(json!({"mode": "plan"}))
    }
}

pub struct ExitPlanMode;

#[async_trait::async_trait]
impl Tool for ExitPlanMode {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn description(&self) -> String {
        "In plan mode, present your implementation plan for approval. Give the whole plan in `plan` (Markdown): \
         the steps, the files to change, and how you will verify the result. If the user approves, plan mode ends \
         and you can start; if not, revise the plan. Use it only to approve a plan for changing code, not to ask \
         questions (use AskUserQuestion)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"plan": {"type": "string", "description": "The plan, in Markdown"}},
            "required": ["plan"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let plan = input.get("plan").and_then(Value::as_str).unwrap_or_default();
        ToolOutput::text("The user approved the plan and plan mode is off. Start the implementation, keeping the task list up to date.")
            .with_structured(json!({"plan": plan, "mode": "default"}))
    }
}
