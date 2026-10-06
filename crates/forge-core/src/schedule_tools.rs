//! The scheduling tools (contract C19): `CronCreate`, `CronList`, `CronDelete`
//! and `ScheduleWakeup`. They change only this session's task list, so they
//! need no permission prompt; the prompts they schedule run with the
//! session's usual permissions.

use std::sync::{Arc, Mutex};

use forge_session::Transcript;
use forge_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{json, Value};

use crate::schedule::{Scheduler, Task, MAX_TASKS, WAKEUP_MAX, WAKEUP_MIN};

pub type SharedScheduler = Arc<Mutex<Scheduler>>;

/// The four tools, sharing one scheduler.
pub fn tools(s: &SharedScheduler, t: &Arc<Transcript>) -> Vec<Arc<dyn Tool>> {
    let x = || Ctx { s: s.clone(), t: t.clone() };
    vec![Arc::new(CronCreate(x())), Arc::new(CronList(x())), Arc::new(CronDelete(x())), Arc::new(ScheduleWakeup(x()))]
}

pub struct Ctx {
    s: SharedScheduler,
    t: Arc<Transcript>,
}

impl Ctx {
    fn save(&self, s: &Scheduler) {
        self.t.set_schedule(s.record());
    }
}

fn when(t: &Task) -> String {
    t.due.format("%Y-%m-%d %H:%M").to_string()
}

pub struct CronCreate(Ctx);
pub struct CronList(Ctx);
pub struct CronDelete(Ctx);
pub struct ScheduleWakeup(Ctx);

#[async_trait::async_trait]
impl Tool for CronCreate {
    fn name(&self) -> &str {
        "CronCreate"
    }

    fn description(&self) -> String {
        format!(
            "Schedule a prompt to run in this session on a cron schedule: 5 fields in local time (minute hour \
             day-of-month month day-of-week), e.g. \"*/10 * * * *\" or \"30 9 * * 1-5\". Use it when the user wants \
             something done at a time or on a cadence.\n\n\
             - Prompts run only while the session is idle, between turns. One that comes due during a turn runs \
               after it, once (no catch-up).\n\
             - recurring (default true) repeats; false runs once. Recurring tasks expire after 7 days.\n\
             - Runs are spread a little: recurring ones may start up to 30 minutes late, one-shots at :00 or :30 \
               up to 90 seconds early.\n\
             - At most {MAX_TASKS} tasks per session. The result gives the task id for CronDelete."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cron": {"type": "string", "description": "5-field cron expression, local time"},
                "prompt": {"type": "string", "description": "The prompt to run"},
                "recurring": {"type": "boolean", "default": true}
            },
            "required": ["cron", "prompt"]
        })
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let cron = input["cron"].as_str().unwrap_or_default();
        let prompt = input["prompt"].as_str().unwrap_or_default();
        let recurring = input["recurring"].as_bool().unwrap_or(true);
        let mut s = self.0.s.lock().unwrap();
        match s.create(cron, prompt, recurring) {
            Ok(t) => {
                self.0.save(&s);
                let expiry = if recurring { " Recurring tasks expire after 7 days." } else { "" };
                ToolOutput::text(format!(
                    "Scheduled {}: {}. Next run around {}.{expiry} CronDelete {} cancels it.",
                    t.id,
                    t.describe(),
                    when(&t),
                    t.id
                ))
                .with_structured(json!({"id": t.id, "nextRun": t.due.to_rfc3339()}))
            }
            Err(e) => ToolOutput::error(e),
        }
    }
}

#[async_trait::async_trait]
impl Tool for CronList {
    fn name(&self) -> &str {
        "CronList"
    }

    fn description(&self) -> String {
        "List this session's scheduled tasks: id, cadence, next run and prompt.".into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        let s = self.0.s.lock().unwrap();
        if s.tasks.is_empty() {
            return ToolOutput::text("No scheduled tasks.");
        }
        let lines: Vec<String> =
            s.tasks.iter().map(|t| format!("{}  {}  next {}  {}", t.id, t.describe(), when(t), t.prompt)).collect();
        ToolOutput::text(lines.join("\n"))
    }
}

#[async_trait::async_trait]
impl Tool for CronDelete {
    fn name(&self) -> &str {
        "CronDelete"
    }

    fn description(&self) -> String {
        "Cancel a scheduled task by its id (from CronCreate or CronList).".into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]})
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let id = input["id"].as_str().unwrap_or_default();
        let mut s = self.0.s.lock().unwrap();
        match s.delete(id) {
            Some(t) => {
                self.0.save(&s);
                ToolOutput::text(format!("Deleted {} ({}).", t.id, t.describe()))
            }
            None => ToolOutput::error(format!("No scheduled task {id}. CronList shows them.")),
        }
    }
}

#[async_trait::async_trait]
impl Tool for ScheduleWakeup {
    fn name(&self) -> &str {
        "ScheduleWakeup"
    }

    fn description(&self) -> String {
        format!(
            "Pace a self-paced loop (/loop without an interval): schedule its next iteration, or end it.\n\n\
             - To continue: delaySeconds ({WAKEUP_MIN}-{WAKEUP_MAX}), reason (one short sentence on why that \
               delay), and prompt: the loop's input, starting with /loop, so the next run re-enters the loop. \
               Pick the delay from how fast the thing you're waiting for changes; don't poll faster than that.\n\
             - To end the loop (the work is done, or it can't progress): {{\"stop\": true}}.\n\
             - Only one wakeup is pending at a time; a new one replaces it. If an iteration does neither, Forge \
               checks back once after 20 minutes and then ends the loop."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "delaySeconds": {"type": "integer", "minimum": WAKEUP_MIN, "maximum": WAKEUP_MAX},
                "reason": {"type": "string"},
                "prompt": {"type": "string"},
                "stop": {"type": "boolean"}
            }
        })
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let mut s = self.0.s.lock().unwrap();
        if input["stop"].as_bool() == Some(true) {
            s.stop_wakeups();
            self.0.save(&s);
            return ToolOutput::text("The loop is stopped.");
        }
        let (Some(delay), Some(prompt)) = (input["delaySeconds"].as_u64(), input["prompt"].as_str()) else {
            return ToolOutput::error("Give delaySeconds and prompt to continue the loop, or stop: true to end it.");
        };
        let reason = input["reason"].as_str().unwrap_or_default();
        match s.wakeup(delay, prompt, reason) {
            Ok(t) => {
                self.0.save(&s);
                ToolOutput::text(format!("Next iteration around {} ({}).", when(&t), t.describe()))
            }
            Err(e) => ToolOutput::error(e),
        }
    }
}
