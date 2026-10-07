//! `/loop [interval] [prompt]` (contract C19).
//!
//! - With an interval (a leading `5m`, or a trailing `every 2 hours`), the
//!   prompt becomes a recurring cron task and also runs now.
//! - Without one, the loop is self-paced: the prompt runs now, and the model
//!   schedules the next iteration with ScheduleWakeup, or stops the loop.
//! - Without a prompt, `.forge/loop.md` (or `loop.md` in the config
//!   directory) is the prompt, else Forge's maintenance prompt.

use forge_types::MessageContent;

use super::run::err;
use super::{parse, Exec, Invocation};
use crate::driver::{Driver, SelfPaced};
use crate::schedule::parse_loop;

/// `loop.md` is read up to this size.
const LOOP_MD_MAX: usize = 25 * 1024;

const MAINTENANCE_PROMPT: &str = "Do a maintenance pass on this project. Go down this list and stop at the \
first item that has something to do:\n1. Unfinished work from this conversation: open items in the task list, \
checks you left failing.\n2. If the current branch has an open pull request and the gh command is available, its \
CI results and review comments: fix what you can.\n3. Small, safe cleanups: a failing or flaky test, a lint \
warning, a TODO you can resolve.\nMake one focused improvement per pass, verify it, and say what you did. Don't \
start large refactors.";

fn self_paced_note(input: &str) -> String {
    format!(
        "<system-reminder>\nThis runs as a self-paced loop. When this iteration is done, decide whether the loop \
         continues: call ScheduleWakeup with delaySeconds (60-3600) chosen from what you're waiting for, a short \
         reason, and prompt set to \"/loop {input}\". Or call ScheduleWakeup with stop: true if the work is \
         finished or can't progress. If you do neither, Forge checks back once after 20 minutes, then ends the \
         loop.\n</system-reminder>"
    )
}

/// The prompt to loop on when none is given.
fn default_prompt(d: &Driver) -> String {
    let candidates = [d.info.cwd.join(".forge/loop.md"), forge_config::config_dir().join("loop.md")];
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut text = text.trim().to_string();
            if text.len() > LOOP_MD_MAX {
                let mut end = LOOP_MD_MAX;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            if !text.is_empty() {
                return text;
            }
        }
    }
    MAINTENANCE_PROMPT.to_string()
}

pub(super) async fn run(d: &mut Driver, args: &str) -> Exec {
    let Some(sched) = d.scheduler.clone() else {
        return err("Scheduling is off in this session (FORGE_DISABLE_CRON).");
    };
    let (interval, prompt) = match parse_loop(args) {
        Ok(x) => x,
        Err(e) => return err(format!("{e}. Usage: /loop [interval] [prompt]")),
    };
    let prompt = if prompt.is_empty() { default_prompt(d) } else { prompt };
    match interval {
        Some(iv) => {
            let created = sched.lock().unwrap().create(&iv.cron, &prompt, true);
            let task = match created {
                Ok(t) => t,
                Err(e) => return err(format!("Could not schedule it: {e}")),
            };
            d.save_schedule();
            let rounded = iv
                .rounded
                .map(|r| format!(" (rounded to {r}: cron can't do that interval evenly)"))
                .unwrap_or_default();
            d.engine.announce(
                "scheduled",
                serde_json::json!({"id": task.id, "cron": iv.cron, "prompt": prompt, "nextRun": task.due.to_rfc3339()}),
            );
            let text = format!(
                "Scheduled {}: {}{rounded}. It runs now, then on schedule; recurring tasks expire after 7 days, and \
                 /tasks stop {} ends it sooner.",
                task.id,
                task.describe(),
                task.id
            );
            d.engine.emit(forge_engine::EngineEvent::Notice { level: forge_engine::NoticeLevel::Info, text });
            Exec::Submit(scheduled_prompt(d, &prompt).await)
        }
        None => {
            let mark = sched.lock().unwrap().wakeups;
            let input = args.trim().to_string();
            // `run_due` marks the iteration that a fallback wakeup started.
            d.self_paced = Some(SelfPaced { input: input.clone(), mark, fallback: false });
            let body = match scheduled_prompt(d, &prompt).await {
                MessageContent::Text(t) => t,
                other => return Exec::Submit(other),
            };
            Exec::Submit(MessageContent::Text(format!("{}\n\n{body}", self_paced_note(&input))))
        }
    }
}

/// What a scheduled prompt sends: a custom command or a skill the model may
/// use expands as usual; anything else (built-in commands, user-only skills,
/// MCP prompts) reaches the model as plain text.
pub(crate) async fn scheduled_prompt(d: &Driver, prompt: &str) -> MessageContent {
    match parse(prompt, &d.catalog) {
        Invocation::Custom { def, args } => {
            match forge_agents::commands::expand(def, args, &d.info.cwd, &d.engine.tool_ctx().shell).await {
                Ok(p) => MessageContent::Text(p),
                Err(_) => MessageContent::Text(prompt.to_string()),
            }
        }
        Invocation::Skills { chain, args } if chain.iter().all(|s| s.model_invocable) => MessageContent::Text(
            chain.iter().map(|s| forge_agents::skills::skill_prompt(s, args)).collect::<Vec<_>>().join("\n\n"),
        ),
        _ => MessageContent::Text(prompt.to_string()),
    }
}
