//! Choices for a picker: commands that take their choice as an argument
//! (`/model`, `/resume`, `/rewind`, `/output-style`, `/permissions`) list
//! what can be chosen, and each choice is the command text that makes it.
//! A front end that draws pickers (the TUI) shows these instead of running
//! the bare command; choosing a row runs exactly what a person could type.

use super::{lookup, Builtin};
use crate::driver::Driver;

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Run this command text.
    Run(String),
    /// Run this command text, then put `edit` in the input box (a rewound prompt).
    RunThenEdit { command: String, edit: String },
    /// Open the picker for this command text (a second step).
    Step(String),
    /// Put this text in the input box to be finished by hand.
    Edit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub detail: String,
    pub pick: Pick,
    /// The current setting.
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub title: String,
    pub choices: Vec<Choice>,
}

fn choice(label: impl Into<String>, detail: impl Into<String>, pick: Pick) -> Choice {
    Choice { label: label.into(), detail: detail.into(), pick, current: false }
}

const REWIND_ACTIONS: [(&str, &str, &str); 5] = [
    ("both", "Restore code and conversation", "to just before that prompt"),
    ("conversation", "Restore the conversation", "files stay as they are"),
    ("code", "Restore the code", "the conversation stays as it is"),
    ("summarize-from", "Summarize from here", "replace this prompt and what follows with a summary"),
    ("summarize-to", "Summarize up to here", "replace what came before this prompt with a summary"),
];

/// The picker for `text`, when it is one of the commands above with nothing
/// chosen yet (or `/rewind <n>`, the second step). `None`: run it as typed.
pub fn picker(d: &Driver, text: &str) -> Option<Picker> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).map(|(n, a)| (n, a.trim())).unwrap_or((rest, ""));
    let spec = lookup(name)?;
    match (spec.id, args) {
        (Builtin::Model, "") => Some(models(d)),
        (Builtin::Resume, "") => resume(d),
        (Builtin::Rewind, "") => rewind(d),
        (Builtin::Rewind, n) if n.parse::<usize>().is_ok() => rewind_action(d, n),
        (Builtin::OutputStyle, "") => Some(styles(d)),
        (Builtin::Permissions, "") => Some(permissions(d)),
        _ => None,
    }
}

fn models(d: &Driver) -> Picker {
    let current = d.engine.handle().model();
    let choices = forge_api::models::MODELS
        .iter()
        .map(|m| Choice {
            current: m.id == current,
            ..choice(
                m.display_name,
                format!(
                    "{} · {} context · ${}/${} per Mtok",
                    m.id,
                    super::settings::window_label(m.context_window),
                    m.input_price,
                    m.output_price
                ),
                Pick::Run(format!("/model {}", m.id)),
            )
        })
        .collect();
    Picker { title: "Select a model".into(), choices }
}

fn resume(d: &Driver) -> Option<Picker> {
    let list = super::switching::sessions(d);
    if list.is_empty() {
        return None;
    }
    let choices = list
        .iter()
        .take(50)
        .map(|s| {
            let what = s.title.clone().unwrap_or_else(|| s.first_prompt.lines().next().unwrap_or("").to_string());
            let detail = format!(
                "{} · {} messages{}",
                super::switching::age(s.modified),
                s.message_count,
                s.git_branch.as_deref().map(|b| format!(" · {b}")).unwrap_or_default()
            );
            choice(what, detail, Pick::Run(format!("/resume {}", s.session_id)))
        })
        .collect();
    Some(Picker { title: "Resume a conversation".into(), choices })
}

fn rewind(d: &Driver) -> Option<Picker> {
    let points = d.engine.prompt_points();
    if points.is_empty() {
        return None;
    }
    let choices = points
        .iter()
        .enumerate()
        .rev()
        .map(|(i, p)| {
            let detail = match p.changed_files.len() {
                0 => String::new(),
                n => format!("{n} file(s) changed since"),
            };
            choice(p.text.lines().next().unwrap_or(""), detail, Pick::Step(format!("/rewind {}", i + 1)))
        })
        .collect();
    Some(Picker { title: "Rewind to before which prompt?".into(), choices })
}

fn rewind_action(d: &Driver, n: &str) -> Option<Picker> {
    let points = d.engine.prompt_points();
    let point = points.get(n.parse::<usize>().ok()?.checked_sub(1)?)?;
    let first: String = point.text.lines().next().unwrap_or("").chars().take(60).collect();
    let choices = REWIND_ACTIONS
        .iter()
        .filter(|(action, ..)| !(point.changed_files.is_empty() && matches!(*action, "code")))
        .map(|(action, label, detail)| {
            let command = format!("/rewind {n} {action}");
            // Going back in the conversation puts the prompt back in the input box.
            let pick = if matches!(*action, "both" | "conversation") {
                Pick::RunThenEdit { command, edit: point.text.clone() }
            } else {
                Pick::Run(command)
            };
            choice(*label, *detail, pick)
        })
        .collect();
    Some(Picker { title: format!("Rewind to before \"{first}\""), choices })
}

fn styles(d: &Driver) -> Picker {
    let current = &d.info.init.output_style;
    let choices = d
        .catalog
        .styles
        .iter()
        .map(|s| Choice {
            current: &s.name == current,
            ..choice(&s.name, &s.description, Pick::Run(format!("/output-style {}", s.name)))
        })
        .collect();
    Picker { title: "Select an output style".into(), choices }
}

fn permissions(d: &Driver) -> Picker {
    let mut choices = vec![];
    {
        let h = d.engine.handle();
        let p = h.permissions.read().unwrap();
        for (behavior, rules) in [("allow", &p.rules.allow), ("ask", &p.rules.ask), ("deny", &p.rules.deny)] {
            for r in rules {
                let text = r.to_string();
                choices.push(choice(
                    format!("Remove {text}"),
                    format!("{behavior} rule"),
                    Pick::Run(format!("/permissions remove {text}")),
                ));
            }
        }
    }
    for (behavior, article) in [("allow", "an"), ("ask", "an"), ("deny", "a")] {
        choices.push(choice(
            format!("Add {article} {behavior} rule…"),
            "e.g. Bash(npm test:*)",
            Pick::Edit(format!("/permissions add {behavior} ")),
        ));
    }
    choices.push(choice("Show the rules and directories", "", Pick::Run("/permissions list".into())));
    Picker { title: "Permission rules".into(), choices }
}
