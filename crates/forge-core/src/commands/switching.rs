//! Commands that move between conversations: `/clear`, `/resume`, `/branch`,
//! `/cd`, `/reload-skills`, `/reload-plugins` and `/rewind`.

use std::fmt::Write as _;

use super::run::{err, ok};
use super::session::clean_title;
use super::{Exec, Surface};
use crate::driver::{Driver, Switch};

pub(super) async fn clear(d: &mut Driver, args: &str) -> Exec {
    let old = d.info.session_id.clone();
    if let Some(g) = d.goal.as_mut().filter(|g| g.is_active()) {
        g.status = crate::goal::Status::Cleared;
        d.goal_changed();
    }
    if !args.is_empty() {
        d.engine.transcript().set_title(&clean_title(args));
    }
    if !d.can_switch() {
        d.subtasks.orphan_all();
        d.engine.clear();
        d.goal = None;
        return ok("Conversation cleared.");
    }
    if let Err(e) = d.switch(Switch::New).await {
        return err(format!("Could not start a new conversation: {e}"));
    }
    let name = if args.is_empty() { String::new() } else { format!(" as \"{}\"", clean_title(args)) };
    ok(format!("Conversation cleared. The previous one is saved{name}: /resume {old} brings it back."))
}

fn age(t: std::time::SystemTime) -> String {
    let secs = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

fn sessions(d: &Driver) -> Vec<forge_session::SessionSummary> {
    let store = match &d.rebuild_store() {
        Some(root) => forge_session::SessionStore::new(root.clone()),
        None => forge_session::SessionStore::default_store(),
    };
    store.list(&d.info.cwd).into_iter().filter(|s| s.session_id != d.info.session_id).collect()
}

pub(super) async fn resume(d: &mut Driver, args: &str) -> Exec {
    if matches!(d.surface, Surface::Print | Surface::Stream) {
        return err("/resume works in the interactive session. In print mode, use --resume <session> (or --continue).");
    }
    let list = sessions(d);
    if args.is_empty() {
        if list.is_empty() {
            return ok("No other conversations in this directory.");
        }
        let mut s = String::from("Conversations in this directory, newest first:\n");
        for (i, x) in list.iter().take(20).enumerate() {
            let what = x.title.clone().unwrap_or_else(|| x.first_prompt.lines().next().unwrap_or("").to_string());
            let what: String = what.chars().take(70).collect();
            let _ = writeln!(
                s,
                "  {:>2}. {what}  ({}, {} messages{}) {}",
                i + 1,
                age(x.modified),
                x.message_count,
                x.git_branch.as_deref().map(|b| format!(", {b}")).unwrap_or_default(),
                &x.session_id[..8.min(x.session_id.len())]
            );
        }
        s.push_str("\nResume one with /resume <number, id or name>.");
        return ok(s);
    }
    let pick = if let Ok(n) = args.parse::<usize>() {
        list.get(n.wrapping_sub(1)).cloned()
    } else {
        let lower = args.to_lowercase();
        let by_id: Vec<_> = list.iter().filter(|s| s.session_id.starts_with(args)).cloned().collect();
        let by_name: Vec<_> = list
            .iter()
            .filter(|s| s.title.as_deref().is_some_and(|t| t.to_lowercase().contains(&lower)))
            .cloned()
            .collect();
        match (by_id.len(), by_name.len()) {
            (1, _) => by_id.into_iter().next(),
            (0, 1) => by_name.into_iter().next(),
            (0, 0) => None,
            _ => return err(format!("\"{args}\" matches more than one conversation; use more of the id.")),
        }
    };
    let Some(target) = pick else {
        return err(format!("No conversation matches \"{args}\". /resume lists them."));
    };
    match d.switch(Switch::Resume(target.session_id.clone())).await {
        Ok(()) => ok(format!(
            "Resumed {}{} ({} messages).",
            target.session_id,
            target.title.map(|t| format!(" \"{t}\"")).unwrap_or_default(),
            d.engine.state.messages.len()
        )),
        Err(e) => err(format!("Could not resume: {e}")),
    }
}

pub(super) async fn branch(d: &mut Driver, args: &str) -> Exec {
    let old = d.info.session_id.clone();
    let title = d.engine.transcript().title();
    if let Err(e) = d.switch(Switch::Branch).await {
        return err(format!("Could not branch: {e}"));
    }
    let name =
        if args.is_empty() { title.map(|t| clean_title(&format!("{t} (branch)"))) } else { Some(clean_title(args)) };
    if let Some(n) = &name {
        d.engine.transcript().set_title(n);
    }
    ok(format!(
        "Branched into {}{}. The original stays as it was: /resume {old} returns to it.",
        d.info.session_id,
        name.map(|n| format!(" \"{n}\"")).unwrap_or_default()
    ))
}

pub(super) async fn cd(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return err("Usage: /cd <directory>");
    }
    let path = forge_tools::expand_path(args, &d.info.cwd);
    let dir = match path.canonicalize() {
        Ok(p) if p.is_dir() => p,
        Ok(_) => return err(format!("{} is not a directory.", path.display())),
        Err(e) => return err(format!("{}: {e}", path.display())),
    };
    if dir == d.info.cwd {
        return ok(format!("Already in {}.", dir.display()));
    }
    let old = d.info.session_id.clone();
    if let Err(e) = d.switch(Switch::Cd(dir.clone())).await {
        return err(format!("Could not move: {e}"));
    }
    d.engine.remind(format!(
        "The user moved this session to {}. Work there from now on; paths are relative to it.",
        dir.display()
    ));
    ok(format!(
        "Now working in {} (conversation {}, continued from {old}). Commands, skills and settings come from the new \
         directory; MCP servers stay as they were.",
        dir.display(),
        d.info.session_id
    ))
}

fn counts(d: &Driver) -> [(&'static str, Vec<String>); 5] {
    let c = &d.catalog;
    [
        ("skills", c.skills.iter().map(|s| s.name.clone()).collect()),
        ("commands", c.commands.iter().map(|s| s.name.clone()).collect()),
        ("agents", c.agents.iter().map(|s| s.name.clone()).collect()),
        ("output styles", c.styles.iter().map(|s| s.name.clone()).collect()),
        ("plugins", c.plugins.iter().map(|s| s.name.clone()).collect()),
    ]
}

pub(super) async fn reload(d: &mut Driver, what: &str) -> Exec {
    let before = counts(d);
    if let Err(e) = d.switch(Switch::Reload).await {
        return err(format!("Could not reload: {e}"));
    }
    let after = counts(d);
    let mut parts = vec![];
    for ((name, old), (_, new)) in before.iter().zip(after.iter()) {
        let added = new.iter().filter(|n| !old.contains(n)).count();
        let removed = old.iter().filter(|n| !new.contains(n)).count();
        let delta = if added + removed == 0 { String::new() } else { format!(" (+{added}, -{removed})") };
        parts.push(format!("{} {name}{delta}", new.len()));
    }
    let note =
        if what == "plugins" { " Plugin hooks apply now; plugin MCP servers start with a new session." } else { "" };
    ok(format!("Reloaded: {}.{note}", parts.join(", ")))
}

const ACTIONS: &str = "  both            restore code and conversation to just before that prompt\n\
  conversation    restore the conversation only\n\
  code            restore files only\n\
  summarize-from  summarize the conversation from that prompt on\n\
  summarize-to    summarize the conversation before that prompt";

pub(super) async fn rewind(d: &mut Driver, args: &str) -> Exec {
    let points = d.engine.prompt_points();
    if points.is_empty() {
        return err("Nothing to rewind yet.");
    }
    let mut words = args.splitn(3, char::is_whitespace);
    let (which, action, extra) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
    if which.is_empty() {
        let mut s = String::from("Prompts, oldest first:\n");
        for (i, p) in points.iter().enumerate() {
            let first: String = p.text.lines().next().unwrap_or("").chars().take(70).collect();
            let files = match p.changed_files.len() {
                0 => String::new(),
                n => format!("  · {n} file(s) changed since"),
            };
            let _ = writeln!(s, "  {:>2}. {first}{files}", i + 1);
        }
        let _ = write!(s, "\nRewind with /rewind <number> <action>:\n{ACTIONS}");
        return ok(s);
    }
    let point = match which.parse::<usize>() {
        Ok(n) => points.get(n.wrapping_sub(1)),
        Err(_) => points.iter().find(|p| p.uuid == which),
    };
    let Some(point) = point.cloned() else {
        return err(format!("No prompt {which}. /rewind lists them."));
    };
    let action = if action.is_empty() { "both" } else { action };
    let code = |d: &Driver| -> Result<String, String> {
        if point.changed_files.is_empty() {
            return Ok("No files to restore.".into());
        }
        let plan = d.engine.rewind_code(point.index)?;
        Ok(format!(
            "Restored {} file(s){}.",
            plan.restore.len(),
            if plan.delete.is_empty() { String::new() } else { format!(", deleted {} new one(s)", plan.delete.len()) }
        ))
    };
    let prompt_back = |text: &str| format!("Your prompt was:\n\n{text}");
    match action {
        "both" => {
            let files = match code(d) {
                Ok(m) => m,
                Err(e) => return err(format!("Could not restore files: {e}")),
            };
            if let Err(e) = d.engine.rewind_conversation(point.index) {
                return err(e);
            }
            ok(format!("Rewound the conversation to before prompt {which}. {files}\n\n{}", prompt_back(&point.text)))
        }
        "conversation" => match d.engine.rewind_conversation(point.index) {
            Ok(()) => ok(format!(
                "Rewound the conversation to before prompt {which}; files are unchanged.\n\n{}",
                prompt_back(&point.text)
            )),
            Err(e) => err(e),
        },
        "code" => match code(d) {
            Ok(m) => ok(format!("{m} The conversation is unchanged.")),
            Err(e) => err(format!("Could not restore files: {e}")),
        },
        "summarize-from" | "summarize-to" => {
            let len = d.engine.state.messages.len();
            let (from, to) = if action == "summarize-from" { (point.index, len) } else { (0, point.index) };
            if from >= to {
                return err("Nothing before that prompt to summarize.");
            }
            match d.engine.summarize_range(from, to, Some(extra)).await {
                Ok(info) => ok(format!(
                    "Summarized the conversation {} prompt {which} (about {} tokens before).",
                    if action == "summarize-from" { "from" } else { "before" },
                    info.pre_tokens
                )),
                Err(e) => err(format!("Could not summarize: {e}")),
            }
        }
        other => err(format!("Unknown action {other:?}. Actions:\n{ACTIONS}")),
    }
}
